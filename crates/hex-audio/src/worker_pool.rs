//! Bounded lock-free real-time worker pool implementing `hex_engine::WaveExecutor`.
//!
//! Spawns a bounded set of persistent worker threads on the control thread with
//! preallocated lock-free `ArrayQueue` work-stealing queues, per-worker SPSC
//! wave-framing channels, preallocated `WaveTaskSlot` buffers, and a single
//! shared read-only `Arc<WaveClockWindow>` whose strong count returns to 1 after
//! every wave (zero callback allocations, zero callback drops, zero mutexes).
//!
//! Workers spin for at most `HOT_SPIN_WINDOW` (`<= 150 us`) after completing a
//! wave and then park on an OS semaphore (`CoreAudioWorkerSemaphore` on macOS)
//! so cold-idle wakeups complete in microseconds without violating Mach RT
//! computation budgets. On macOS, active workers non-fatally borrow CoreAudio
//! Mach time-constraint scheduling via `cpal::platform::CoreAudioWorkerTimeConstraint`,
//! releasing it during sustained idle and re-entering off the synchronous
//! `BeginWave` critical path after completing a wave.

use crossbeam_queue::ArrayQueue;
use hex_engine::{
    Frame, Invalid,
    wave_executor::{
        MAX_WAVE_TASKS, PARALLEL_WAVE_MIN_TASKS, WaveClockWindow, WaveExecutor, WaveStageArrival,
        WaveTaskSlot,
    },
};
use hex_lattice::NativeRuntimeChain;
use hex_lattice_contract::DeviceContractError;
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    sync::{
        Arc,
        atomic::{
            AtomicBool, AtomicU64,
            Ordering::{Acquire, Relaxed, Release, SeqCst},
        },
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Upper bound on persistent background worker threads (caller thread also participates,
/// giving up to 8 concurrent DSP threads).
pub const MAX_WORKERS: usize = 7;

/// Bounded micro-spin duration (`<= 150 us`) workers stay hot after completing a
/// wave before parking on the OS semaphore, keeping CPU use well within the 2 ms
/// Mach RT computation quantum.
const HOT_SPIN_WINDOW: Duration = Duration::from_micros(100);

/// Sustained idle duration before releasing Mach real-time time-constraint
/// scheduling, avoiding per-wave `thread_policy_set` syscalls on hot wakeups.
const DEADLINE_HOLD_WINDOW: Duration = Duration::from_millis(500);

/// Upper bound the audio callback will spin waiting on a dispatched worker before
/// declaring the pool degraded and recovering without deadlocking the device thread.
const DEFAULT_STALL_BUDGET: Duration = Duration::from_millis(50);

/// Per-worker SPSC wave-framing capacity (1 `BeginWave` / `WaveDone` per wave).
const WORKER_CONTROL_CAPACITY: usize = 4;

enum WorkerRequest {
    BeginWave(Arc<WaveClockWindow>),
}

enum WorkerReply {
    WaveDone(Arc<WaveClockWindow>),
}

#[cfg(target_os = "macos")]
struct WorkerWakeup {
    sem: cpal::platform::CoreAudioWorkerSemaphore,
}

#[cfg(target_os = "macos")]
impl WorkerWakeup {
    fn new() -> Result<Self, Invalid> {
        let sem = cpal::platform::CoreAudioWorkerSemaphore::new()
            .map_err(|_| Invalid("could not create CoreAudio worker semaphore"))?;
        Ok(Self { sem })
    }

    #[inline]
    fn signal(&self) {
        self.sem.signal();
    }

    #[inline]
    fn wait(&self) {
        self.sem.wait();
    }

    #[inline]
    fn wait_timeout(&self, timeout: Duration) -> bool {
        self.sem.wait_timeout(timeout)
    }
}

#[cfg(not(target_os = "macos"))]
struct WorkerWakeup {
    state: std::sync::Mutex<u64>,
    cvar: std::sync::Condvar,
}

#[cfg(not(target_os = "macos"))]
impl WorkerWakeup {
    fn new() -> Result<Self, Invalid> {
        Ok(Self {
            state: std::sync::Mutex::new(0),
            cvar: std::sync::Condvar::new(),
        })
    }

    #[inline]
    fn signal(&self) {
        if let Ok(mut tokens) = self.state.lock() {
            *tokens = tokens.saturating_add(1);
            self.cvar.notify_one();
        }
    }

    fn wait(&self) {
        if let Ok(mut tokens) = self.state.lock() {
            while *tokens == 0 {
                match self.cvar.wait(tokens) {
                    Ok(guard) => tokens = guard,
                    Err(_) => return,
                }
            }
            *tokens -= 1;
        }
    }

    fn wait_timeout(&self, timeout: Duration) -> bool {
        if let Ok(mut tokens) = self.state.lock() {
            if *tokens > 0 {
                *tokens -= 1;
                return true;
            }
            if let Ok((mut guard, _)) = self.cvar.wait_timeout(tokens, timeout) {
                if *guard > 0 {
                    *guard -= 1;
                    return true;
                }
            }
        }
        false
    }
}

struct WorkerEndpoint {
    tx: Producer<WorkerRequest>,
    rx: Consumer<WorkerReply>,
    parked: Arc<AtomicBool>,
    wakeup: Arc<WorkerWakeup>,
    exited: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

struct WorkerShared {
    ready_queue: ArrayQueue<(usize, Box<WaveTaskSlot>)>,
    done_queue: ArrayQueue<(usize, Box<WaveTaskSlot>)>,
    wave_epoch: AtomicU64,
    shutdown: AtomicBool,
    worker_exited: AtomicBool,
    #[cfg(test)]
    worker_tasks_executed: AtomicU64,
}

/// Guard ensuring both per-worker and pool-wide exit flags are raised if a worker
/// thread ever terminates early so the callback wait loop can never deadlock.
struct ExitFlagGuard<'a> {
    pool_flag: &'a AtomicBool,
    worker_flag: &'a AtomicBool,
    clean: bool,
}

impl Drop for ExitFlagGuard<'_> {
    fn drop(&mut self) {
        if !self.clean {
            self.worker_flag.store(true, Release);
            self.pool_flag.store(true, Release);
        }
    }
}

#[cfg(target_os = "macos")]
struct WorkerDeadline {
    policy: Option<cpal::platform::CoreAudioWorkerTimeConstraint>,
    active: bool,
    refused: bool,
}

#[cfg(target_os = "macos")]
impl WorkerDeadline {
    fn for_current_thread(sample_rate: u32) -> Self {
        let rate = sample_rate.clamp(8_000, 192_000) as f64;
        let period = Duration::from_secs_f64(256.0 / rate);
        let constraint = period.mul_f64(0.8).min(Duration::from_millis(4));
        let computation = period
            .mul_f64(0.4)
            .min(Duration::from_millis(2))
            .min(constraint / 2);
        let policy = cpal::platform::CoreAudioWorkerTimeConstraint::for_current_thread(
            period,
            computation,
            constraint,
        )
        .ok();
        Self {
            policy,
            active: false,
            refused: false,
        }
    }

    fn is_active(&self) -> bool {
        self.active
    }

    fn enter(&mut self) {
        if !self.active
            && !self.refused
            && let Some(policy) = self.policy.as_mut()
        {
            if policy.enter().is_ok() {
                self.active = true;
            } else {
                self.refused = true;
            }
        }
    }

    fn leave(&mut self) {
        if self.active
            && let Some(policy) = self.policy.as_mut()
        {
            self.active = false;
            if policy.leave().is_err() {
                self.refused = true;
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
struct WorkerDeadline {
    active: bool,
}

#[cfg(not(target_os = "macos"))]
impl WorkerDeadline {
    fn for_current_thread(_sample_rate: u32) -> Self {
        Self { active: false }
    }
    fn is_active(&self) -> bool {
        self.active
    }
    fn enter(&mut self) {
        self.active = true;
    }
    fn leave(&mut self) {
        self.active = false;
    }
}

fn push_reply_until_shutdown(
    tx: &mut Producer<WorkerReply>,
    shared: &WorkerShared,
    mut reply: WorkerReply,
) -> bool {
    while let Err(rtrb::PushError::Full(returned)) = tx.push(reply) {
        if shared.shutdown.load(Acquire) {
            return false;
        }
        reply = returned;
        std::hint::spin_loop();
    }
    true
}

fn worker_loop(
    shared: Arc<WorkerShared>,
    parked: Arc<AtomicBool>,
    wakeup: Arc<WorkerWakeup>,
    worker_exited: Arc<AtomicBool>,
    mut rx: Consumer<WorkerRequest>,
    mut tx: Producer<WorkerReply>,
    sample_rate: u32,
) {
    let mut exit_guard = ExitFlagGuard {
        pool_flag: &shared.worker_exited,
        worker_flag: &worker_exited,
        clean: false,
    };
    let mut deadline = WorkerDeadline::for_current_thread(sample_rate);
    deadline.enter();
    let mut last_active = Instant::now();
    let mut observed_epoch = shared.wave_epoch.load(Acquire);

    'outer: loop {
        let mut progressed = false;
        while let Ok(WorkerRequest::BeginWave(clock)) = rx.pop() {
            progressed = true;
            // Steal tasks lock-free until the wave's ready queue drains,
            // without running `thread_policy_set` on the synchronous BeginWave path.
            while let Some((slot_index, mut task)) = shared.ready_queue.pop() {
                task.run(&clock);
                #[cfg(test)]
                shared.worker_tasks_executed.fetch_add(1, Relaxed);
                let mut pending = (slot_index, task);
                while let Err(returned) = shared.done_queue.push(pending) {
                    if shared.shutdown.load(Acquire) {
                        break 'outer;
                    }
                    pending = returned;
                    std::hint::spin_loop();
                }
            }
            if !push_reply_until_shutdown(&mut tx, &shared, WorkerReply::WaveDone(clock)) {
                break 'outer;
            }
            last_active = Instant::now();
            // Re-enter Mach RT scheduling off the callback critical path (after
            // releasing the callback via `WaveDone`) so the next wave starts hot.
            if !deadline.is_active() && !shared.shutdown.load(Acquire) {
                deadline.enter();
            }
        }

        if progressed {
            last_active = Instant::now();
            observed_epoch = shared.wave_epoch.load(Acquire);
            continue;
        }

        if shared.shutdown.load(Acquire) {
            break;
        }

        let current_epoch = shared.wave_epoch.load(Acquire);
        if current_epoch != observed_epoch {
            observed_epoch = current_epoch;
            continue;
        }

        let idle_for = last_active.elapsed();
        if idle_for < HOT_SPIN_WINDOW {
            std::hint::spin_loop();
            continue;
        }

        // Publish parked state with SeqCst before re-checking epoch/shutdown so
        // the callback's SeqCst epoch bump + parked check cannot miss a wakeup.
        parked.store(true, SeqCst);
        let recheck_epoch = shared.wave_epoch.load(SeqCst);
        if recheck_epoch != observed_epoch || shared.shutdown.load(SeqCst) {
            parked.store(false, Release);
            observed_epoch = recheck_epoch;
            continue;
        }

        if deadline.is_active() && idle_for < DEADLINE_HOLD_WINDOW {
            let remaining = DEADLINE_HOLD_WINDOW.saturating_sub(idle_for);
            let signaled = wakeup.wait_timeout(remaining);
            parked.store(false, Release);
            if !signaled
                && last_active.elapsed() >= DEADLINE_HOLD_WINDOW
                && shared.wave_epoch.load(Acquire) == observed_epoch
                && !shared.shutdown.load(Acquire)
            {
                deadline.leave();
            }
        } else {
            if deadline.is_active() {
                deadline.leave();
            }
            wakeup.wait();
            parked.store(false, Release);
        }
    }

    parked.store(false, Release);
    deadline.leave();
    exit_guard.clean = true;
}

/// Bounded lock-free real-time worker pool for topological wave DSP execution.
///
/// Construct and drop on the control thread; audio callbacks invoke only the
/// allocation-free `WaveExecutor` trait methods.
pub struct AudioWorkerPool {
    shared: Arc<WorkerShared>,
    workers: Vec<WorkerEndpoint>,
    clock: Arc<WaveClockWindow>,
    slots: Vec<Option<Box<WaveTaskSlot>>>,
    slot_owners: [usize; MAX_WAVE_TASKS],
    staged: usize,
    sample_rate: u32,
    stall_budget: Duration,
    #[cfg(test)]
    waves_executed: usize,
}

impl AudioWorkerPool {
    /// Spawn a bounded worker pool on control with `worker_count` background threads
    /// (clamped to `0..=MAX_WORKERS`). `worker_count == 0` runs serially inline.
    pub fn new(worker_count: usize, sample_rate: u32) -> Result<Self, Invalid> {
        let clamped_workers = worker_count.min(MAX_WORKERS);
        let shared = Arc::new(WorkerShared {
            ready_queue: ArrayQueue::new(MAX_WAVE_TASKS),
            done_queue: ArrayQueue::new(MAX_WAVE_TASKS),
            wave_epoch: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            worker_exited: AtomicBool::new(false),
            #[cfg(test)]
            worker_tasks_executed: AtomicU64::new(0),
        });
        let mut workers: Vec<WorkerEndpoint> = Vec::with_capacity(clamped_workers);
        for index in 0..clamped_workers {
            let (req_tx, req_rx) = RingBuffer::new(WORKER_CONTROL_CAPACITY);
            let (rep_tx, rep_rx) = RingBuffer::new(WORKER_CONTROL_CAPACITY);
            let worker_shared = Arc::clone(&shared);
            let parked = Arc::new(AtomicBool::new(false));
            let worker_parked = Arc::clone(&parked);
            let wakeup = Arc::new(WorkerWakeup::new().inspect_err(|_| {
                shared.shutdown.store(true, SeqCst);
                shared.wave_epoch.fetch_add(1, SeqCst);
                for mut spawned in workers.drain(..) {
                    spawned.wakeup.signal();
                    if let Some(handle) = spawned.handle.take() {
                        let _ = handle.join();
                    }
                }
            })?);
            let worker_wakeup = Arc::clone(&wakeup);
            let exited = Arc::new(AtomicBool::new(false));
            let worker_exit_flag = Arc::clone(&exited);
            let builder = thread::Builder::new().name(format!("hex-dsp-wave-{index}"));
            let handle = builder
                .spawn(move || {
                    worker_loop(
                        worker_shared,
                        worker_parked,
                        worker_wakeup,
                        worker_exit_flag,
                        req_rx,
                        rep_tx,
                        sample_rate,
                    )
                })
                .map_err(|_| {
                    shared.shutdown.store(true, SeqCst);
                    shared.wave_epoch.fetch_add(1, SeqCst);
                    for mut spawned in workers.drain(..) {
                        spawned.wakeup.signal();
                        if let Some(handle) = spawned.handle.take() {
                            let _ = handle.join();
                        }
                    }
                    Invalid("could not spawn audio dsp wave worker thread")
                })?;
            workers.push(WorkerEndpoint {
                tx: req_tx,
                rx: rep_rx,
                parked,
                wakeup,
                exited,
                handle: Some(handle),
            });
        }
        let mut slots = Vec::with_capacity(MAX_WAVE_TASKS);
        for _ in 0..MAX_WAVE_TASKS {
            slots.push(Some(Box::new(WaveTaskSlot::new_idle())));
        }
        Ok(Self {
            shared,
            workers,
            clock: Arc::new(WaveClockWindow::default()),
            slots,
            slot_owners: [0; MAX_WAVE_TASKS],
            staged: 0,
            sample_rate,
            stall_budget: DEFAULT_STALL_BUDGET,
            #[cfg(test)]
            waves_executed: 0,
        })
    }

    #[cfg(test)]
    pub(crate) fn waves_executed(&self) -> usize {
        self.waves_executed
    }

    /// Spawn a pool sized for the current machine (`available_parallelism - 1`, capped
    /// at `MAX_WORKERS`). Returns `None` on single-core machines or if spawning fails,
    /// allowing graceful single-thread fallback.
    pub fn for_hardware(sample_rate: u32) -> Option<Self> {
        let cores = thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        let worker_count = cores.saturating_sub(1).min(MAX_WORKERS);
        if worker_count == 0 {
            return None;
        }
        Self::new(worker_count, sample_rate).ok()
    }

    /// Configured sample rate used for real-time deadline period estimation.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn execute_inline(&mut self) {
        let clock = &*self.clock;
        for slot in self.slots[..self.staged].iter_mut().flatten() {
            slot.run(clock);
        }
    }
}

impl Drop for AudioWorkerPool {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, SeqCst);
        self.shared.wave_epoch.fetch_add(1, SeqCst);
        for worker in &self.workers {
            worker.wakeup.signal();
        }
        for worker in &mut self.workers {
            if let Some(handle) = worker.handle.take() {
                let _ = handle.join();
            }
        }
    }
}

impl WaveExecutor for AudioWorkerPool {
    fn worker_count(&self) -> usize {
        if self.shared.worker_exited.load(Relaxed) {
            0
        } else {
            self.workers.len()
        }
    }

    fn prepare_clock(&mut self) -> Option<&mut WaveClockWindow> {
        if self.staged != 0 {
            return None;
        }
        Arc::get_mut(&mut self.clock)
    }

    fn push_task(
        &mut self,
        owner_index: usize,
        chain: NativeRuntimeChain,
        audio: &[Frame],
        frame_offset: usize,
        stage_start: usize,
        stage_end: usize,
        compensated: bool,
        stage_arrivals: &[WaveStageArrival],
    ) -> Result<usize, NativeRuntimeChain> {
        if self.staged >= self.slots.len()
            || audio.len() > hex_engine::wave_executor::WAVE_MAX_FRAMES
            || stage_end.saturating_sub(stage_start) > hex_engine::wave_executor::MAX_WAVE_STAGES
            || (compensated && stage_arrivals.len() < stage_end.saturating_sub(stage_start))
        {
            return Err(chain);
        }
        let slot_index = self.staged;
        let Some(slot) = self.slots[slot_index].as_mut() else {
            return Err(chain);
        };
        slot.load(
            owner_index,
            chain,
            audio,
            frame_offset,
            stage_start,
            stage_end,
            compensated,
            stage_arrivals,
        );
        self.slot_owners[slot_index] = owner_index;
        self.staged += 1;
        Ok(slot_index)
    }

    fn execute_staged(&mut self) {
        if self.staged < PARALLEL_WAVE_MIN_TASKS
            || self.workers.is_empty()
            || self.shared.worker_exited.load(Acquire)
        {
            self.execute_inline();
            return;
        }

        let total = self.staged;
        #[cfg(test)]
        {
            self.waves_executed = self.waves_executed.saturating_add(1);
        }
        let active_workers = self.workers.len().min(total.saturating_sub(1));
        let mut worker_dispatched = [false; MAX_WORKERS];
        let mut dispatched_workers = 0usize;

        // Publish every staged task into the lock-free work-stealing queue before
        // signaling workers so woken workers immediately steal work on arrival.
        let mut pushed_tasks = 0usize;
        for slot_index in 0..total {
            if let Some(task) = self.slots[slot_index].take() {
                match self.shared.ready_queue.push((slot_index, task)) {
                    Ok(()) => pushed_tasks += 1,
                    Err((slot_index, task)) => {
                        self.slots[slot_index] = Some(task);
                    }
                }
            }
        }

        if pushed_tasks > 0 {
            for (w, dispatched) in worker_dispatched
                .iter_mut()
                .enumerate()
                .take(active_workers)
            {
                let worker = &mut self.workers[w];
                if worker.exited.load(Acquire) || worker.tx.slots() == 0 {
                    continue;
                }
                if worker
                    .tx
                    .push(WorkerRequest::BeginWave(Arc::clone(&self.clock)))
                    .is_ok()
                {
                    *dispatched = true;
                    dispatched_workers += 1;
                }
            }
        }

        if dispatched_workers > 0 {
            // SeqCst epoch bump + per-worker parked check pairs with worker's
            // SeqCst parked store + epoch recheck for lost-wakeup-free signaling.
            self.shared.wave_epoch.fetch_add(1, SeqCst);
            for (w, &dispatched) in worker_dispatched.iter().enumerate().take(active_workers) {
                if dispatched && self.workers[w].parked.load(SeqCst) {
                    self.workers[w].wakeup.signal();
                }
            }
        }

        // Caller participates in the same lock-free work-stealing queue over 0..total
        // and also executes any unpushed slot that remained resident locally.
        let mut completed_tasks = 0usize;
        for slot in self.slots[..total].iter_mut().flatten() {
            slot.run(&self.clock);
        }
        while let Some((slot_index, mut task)) = self.shared.ready_queue.pop() {
            task.run(&self.clock);
            self.slots[slot_index] = Some(task);
            completed_tasks += 1;
        }

        // Drain completed worker tasks and WaveDone framing under a bounded wait budget
        // so a wedged or exited worker can never spin the audio callback unboundedly.
        let mut worker_done = [false; MAX_WORKERS];
        let mut remaining_workers = dispatched_workers;
        let wait_start = Instant::now();
        while remaining_workers > 0 || completed_tasks < pushed_tasks {
            let mut any = false;
            while let Some((slot_index, task)) = self.shared.done_queue.pop() {
                self.slots[slot_index] = Some(task);
                completed_tasks += 1;
                any = true;
            }
            for (w, (dispatched, done)) in worker_dispatched
                .iter()
                .zip(worker_done.iter_mut())
                .enumerate()
                .take(active_workers)
            {
                if !*dispatched || *done {
                    continue;
                }
                if let Ok(WorkerReply::WaveDone(clock_clone)) = self.workers[w].rx.pop() {
                    any = true;
                    // `self.clock` remains alive (strong count >= 1), so this drop
                    // only decrements the atomic refcount with zero deallocation.
                    drop(clock_clone);
                    *done = true;
                    remaining_workers -= 1;
                }
                if !*done && self.workers[w].exited.load(Acquire) && self.workers[w].rx.is_empty() {
                    *done = true;
                    remaining_workers -= 1;
                    any = true;
                }
            }
            if !any && (remaining_workers > 0 || completed_tasks < pushed_tasks) {
                if wait_start.elapsed() >= self.stall_budget {
                    // Mark degraded so subsequent waves execute inline without waiting.
                    self.shared.worker_exited.store(true, Release);
                    for (w, (&dispatched, &done)) in worker_dispatched
                        .iter()
                        .zip(worker_done.iter())
                        .enumerate()
                        .take(active_workers)
                    {
                        if dispatched && !done {
                            self.workers[w].exited.store(true, Release);
                        }
                    }
                    // Recover any remaining unstarted or finished tasks without blocking.
                    while let Some((slot_index, mut task)) = self.shared.ready_queue.pop() {
                        task.run(&self.clock);
                        self.slots[slot_index] = Some(task);
                    }
                    while let Some((slot_index, task)) = self.shared.done_queue.pop() {
                        self.slots[slot_index] = Some(task);
                    }
                    break;
                }
                std::hint::spin_loop();
            }
        }
    }

    fn reclaim_task(
        &mut self,
        slot: usize,
        audio_out: &mut [Frame],
    ) -> (usize, NativeRuntimeChain, Option<DeviceContractError>) {
        if let Some(task) = self.slots.get_mut(slot).and_then(Option::as_mut) {
            task.take_result(audio_out)
        } else {
            let owner_index = self.slot_owners.get(slot).copied().unwrap_or(0);
            (
                owner_index,
                NativeRuntimeChain::empty(),
                Some(DeviceContractError::InvalidDescriptor),
            )
        }
    }

    fn finish_wave(&mut self) {
        self.staged = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::{
        Audio, PlaybackCommand, PreparedRevision, SerialWaveExecutor, SessionPlayback,
        SessionPlaybackCommand, SineTable, playback::Playback,
    };
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{Command, Created, Editor, Project, TrackSelectionIds};
    use std::collections::BTreeMap;

    fn dense_revision(track_count: usize, with_groups_and_returns: bool) -> Arc<PreparedRevision> {
        const RATE: u32 = 48_000;
        const FRAMES: usize = 4_096;
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 128).unwrap();
        let mut media = BTreeMap::new();
        let mut tracks = Vec::with_capacity(track_count);
        for i in 0..track_count {
            let Created::Imported { track, asset, .. } = editor
                .execute(Command::ImportPreparedAudioTrack {
                    name: format!("track-{i}"),
                    path: format!("memory/track-{i}.wav").into(),
                    source_sample_rate: RATE,
                    source_frames: FRAMES as u64,
                    playback_frames: FRAMES as u64,
                    conversion_version: hex_project::IDENTITY_CONVERSION,
                })
                .unwrap()
            else {
                panic!("expected imported track");
            };
            let frames: Vec<Frame> = (0..FRAMES)
                .map(|n| {
                    let phase = n as f32 * (0.011 + i as f32 * 0.0017);
                    [phase.sin() * 0.08, (phase * 1.07).cos() * 0.08]
                })
                .collect();
            media.insert(asset, Audio::new(RATE, frames).unwrap());
            for state in [
                hex_devices::eq8::EQ8_DEFINITION.default_state().unwrap(),
                hex_devices::filter::DEFINITION.default_state().unwrap(),
                hex_devices::compressor::COMPRESSOR_DEFINITION
                    .default_state()
                    .unwrap(),
            ] {
                editor
                    .execute(Command::InsertDevice {
                        track,
                        before: None,
                        state,
                    })
                    .unwrap();
            }
            tracks.push(track);
        }

        if with_groups_and_returns && track_count >= 12 {
            let Created::Track(group_a) = editor
                .execute(Command::GroupTracks {
                    tracks: TrackSelectionIds::new(&tracks[0..4]).unwrap(),
                    name: "group-a".into(),
                })
                .unwrap()
            else {
                panic!("expected group a");
            };
            let Created::Track(group_b) = editor
                .execute(Command::GroupTracks {
                    tracks: TrackSelectionIds::new(&tracks[4..8]).unwrap(),
                    name: "group-b".into(),
                })
                .unwrap()
            else {
                panic!("expected group b");
            };
            for group in [group_a, group_b] {
                editor
                    .execute(Command::InsertDevice {
                        track: group,
                        before: None,
                        state: hex_devices::compressor::COMPRESSOR_DEFINITION
                            .default_state()
                            .unwrap(),
                    })
                    .unwrap();
            }
            for r in 0..2 {
                let Created::ReturnTrack(ret) = editor
                    .execute(Command::AddReturnTrack {
                        name: format!("ret-{r}"),
                    })
                    .unwrap()
                else {
                    panic!("expected return");
                };
                let state = hex_devices::reverb::REVERB_DEFINITION
                    .default_state()
                    .unwrap();
                editor
                    .execute(Command::InsertReturnDevice {
                        return_track: ret,
                        before: None,
                        state,
                    })
                    .unwrap();
                for group in [group_a, group_b] {
                    editor
                        .execute(Command::SetReturnSend {
                            track: group,
                            return_track: ret,
                            gain: 0.25,
                        })
                        .unwrap();
                }
            }
        }
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
                .unwrap(),
        )
    }

    #[test]
    fn worker_pool_matches_serial_bit_for_bit_on_flat_and_grouped_graphs() {
        for (tracks, grouped) in [(16, false), (16, true)] {
            let revision = dense_revision(tracks, grouped);
            let mut serial_runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            let mut parallel_runtime = SessionPlayback::new(revision).unwrap();
            serial_runtime.apply(SessionPlaybackCommand::Play).unwrap();
            parallel_runtime
                .apply(SessionPlaybackCommand::Play)
                .unwrap();

            let mut serial_exec = SerialWaveExecutor::default();
            let mut pool = AudioWorkerPool::new(3, 48_000).unwrap();

            for _block in 0..8 {
                let mut expected = [[0.0_f32; 2]; 256];
                let mut actual = [[0.0_f32; 2]; 256];
                serial_runtime
                    .render_with_wave_executor(&mut expected, &mut serial_exec)
                    .unwrap();
                parallel_runtime
                    .render_with_wave_executor(&mut actual, &mut pool)
                    .unwrap();
                assert!(
                    expected.iter().flatten().any(|s| s.abs() > 1e-6),
                    "fixture must render audible non-zero signal (grouped={grouped})"
                );
                for (frame_idx, (e, a)) in expected.iter().zip(actual.iter()).enumerate() {
                    assert_eq!(
                        [e[0].to_bits(), e[1].to_bits()],
                        [a[0].to_bits(), a[1].to_bits()],
                        "bit-exact mismatch at frame {frame_idx} (grouped={grouped}): expected={e:?}, actual={a:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn worker_pool_gracefully_falls_back_with_zero_workers() {
        let mut pool = AudioWorkerPool::new(0, 48_000).unwrap();
        assert_eq!(pool.worker_count(), 0);
        let revision = dense_revision(8, false);
        let mut playback_a =
            Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        let mut playback_b = Playback::try_from_prepared_with_session(revision).unwrap();
        playback_a
            .try_apply_action(hex_engine::PlaybackAction::Transport(
                PlaybackCommand::PlayFrom(0),
            ))
            .unwrap();
        playback_b
            .try_apply_action(hex_engine::PlaybackAction::Transport(
                PlaybackCommand::PlayFrom(0),
            ))
            .unwrap();
        let mut out_a = [[0.0_f32; 2]; 256];
        let mut out_b = [[0.0_f32; 2]; 256];
        playback_a.render(&mut out_a).unwrap();
        playback_b
            .render_with_wave_executor(&mut out_b, &mut pool)
            .unwrap();
        assert_eq!(out_a, out_b);
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn callback_with_worker_pool_allocates_zero_bytes_during_parallel_wave_render() {
        let revision = dense_revision(16, true);
        let playback = Playback::try_from_prepared_with_session(revision).unwrap();

        let (mut control, mut callback) =
            crate::prepared::channel_with_worker_pool(48_000, 3).unwrap();
        control.publish(playback).unwrap();
        control.command(PlaybackCommand::Play).unwrap();

        let mut output = [[0.0_f32; 2]; 256];
        // Warm up first three blocks so compensation history fills and enters block wave path.
        for _ in 0..3 {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        }
        assert!(output.iter().flatten().any(|s| s.abs() > 1e-6));

        let info = allocation_counter::measure(|| {
            for _ in 0..4 {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            }
        });
        assert_eq!(
            (
                info.count_total,
                info.bytes_total,
                info.count_current,
                info.bytes_current,
            ),
            (0, 0, 0, 0),
            "parallel wave callback rendering must perform zero heap allocations/deallocations"
        );
    }

    fn utility_chain(owner_id: u64, device_id: u64, linear_gain: f32) -> NativeRuntimeChain {
        use hex_lattice::{
            DeviceMeter, NativeDeviceDefinition, PreparedProcessorInstance, ProcessorChainOwner,
        };
        use hex_lattice_contract::{DeviceTypeId, ParameterValue};
        let state =
            hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                .unwrap();
        let state = hex_devices::utility::UTILITY_DEFINITION
            .patch_parameter(
                &state,
                hex_devices::utility::parameter::GAIN,
                ParameterValue::Float(linear_gain),
            )
            .unwrap();
        let matrix = hex_devices::utility::UTILITY_DEFINITION
            .prepare_matrix(&state)
            .unwrap();
        let instance = PreparedProcessorInstance {
            meter: Arc::new(DeviceMeter::default()),
            owner: ProcessorChainOwner::Track(owner_id),
            device_id,
            device_type: DeviceTypeId::from_static("hex.utility"),
            matrix,
            bypassed: false,
            sample_rate: 48_000,
            configuration: Some(Arc::new(state)),
            definition: None,
        };
        NativeRuntimeChain::new(vec![instance]).unwrap()
    }

    #[test]
    fn saturated_queue_runs_tasks_inline_without_skipping_or_wedging_clock() {
        let mut pool = AudioWorkerPool::new(2, 48_000).unwrap();
        // Replace worker 0's channel with a saturated unconsumed ring buffer
        // (capacity 1, full) so worker 0 cannot accept BeginWave,
        // while worker 1 remains live and healthy.
        let (mut saturated_tx, _parked_rx) = RingBuffer::new(1);
        saturated_tx
            .push(WorkerRequest::BeginWave(Arc::new(
                WaveClockWindow::default(),
            )))
            .unwrap();
        let (_unused_rep_tx, empty_rep_rx) = RingBuffer::new(4);
        let original_handle = pool.workers[0].handle.take();
        pool.workers[0].tx = saturated_tx;
        pool.workers[0].rx = empty_rep_rx;

        let input = [[0.5_f32, -0.25_f32]; 16];
        let expected_gain = 0.5_f32;
        let clock = pool.prepare_clock().expect("clock available");
        clock.history_len = 0;
        clock.incoming_len = 0;
        let mut slots = Vec::new();
        for owner_index in 0..8 {
            let chain = utility_chain(
                owner_index as u64 + 1,
                owner_index as u64 + 100,
                expected_gain,
            );
            let slot = pool
                .push_task(owner_index, chain, &input, 0, 0, 1, false, &[])
                .ok()
                .unwrap();
            slots.push((owner_index, slot));
        }

        pool.execute_staged();

        for (expected_owner, slot) in slots {
            let mut out = [[0.0_f32; 2]; 16];
            let (owner, chain, err) = pool.reclaim_task(slot, &mut out);
            assert_eq!(owner, expected_owner);
            assert_eq!(chain.len(), 1);
            assert!(err.is_none());
            for (actual, src) in out.iter().zip(&input) {
                assert!(
                    (actual[0] - src[0] * expected_gain).abs() < 1e-5
                        && (actual[1] - src[1] * expected_gain).abs() < 1e-5,
                    "task {expected_owner} must not be skipped on queue saturation: actual={actual:?}"
                );
            }
        }
        pool.finish_wave();

        // Clock Arc strong count must return to 1 so the next wave can borrow it.
        assert!(pool.prepare_clock().is_some());
        pool.finish_wave();

        // Clean up the replaced worker thread cleanly.
        pool.workers[0].handle = original_handle;
        drop(pool);
    }

    #[test]
    fn worker_exited_reclaim_preserves_owner_index_without_overwriting_track_zero() {
        let mut pool = AudioWorkerPool::new(2, 48_000).unwrap();
        let input = [[0.25_f32, -0.5_f32]; 8];
        let clock = pool.prepare_clock().expect("clock available");
        clock.history_len = 0;
        clock.incoming_len = 0;

        for owner_index in 0..6 {
            let chain = utility_chain(owner_index as u64 + 1, owner_index as u64 + 200, 1.0);
            pool.push_task(owner_index, chain, &input, 0, 0, 1, false, &[])
                .ok()
                .unwrap();
        }

        // Simulate worker 0 exiting early while holding slot 4 (owner_index == 4).
        let _lost = pool.slots[4].take();
        pool.workers[0].exited.store(true, Release);
        pool.shared.worker_exited.store(true, Release);

        let mut chains: Vec<NativeRuntimeChain> =
            (0..6).map(|_| NativeRuntimeChain::empty()).collect();
        let mut saw_slot_4_err = false;
        for slot in 0..6 {
            let mut out = [[0.0_f32; 2]; 8];
            let (owner, chain, err) = pool.reclaim_task(slot, &mut out);
            assert_eq!(owner, slot, "reclaim_task must preserve exact owner index");
            if slot == 4 {
                assert_eq!(err, Some(DeviceContractError::InvalidDescriptor));
                assert!(chain.is_empty());
                saw_slot_4_err = true;
            } else {
                assert!(err.is_none());
                assert_eq!(chain.len(), 1);
            }
            chains[owner] = chain;
        }
        assert!(saw_slot_4_err);
        // Track 0's live chain must remain intact (never overwritten by slot 4's error).
        assert_eq!(chains[0].len(), 1);
        assert_eq!(pool.worker_count(), 0);
        pool.finish_wave();
    }

    #[test]
    fn worker_shutdown_breaks_out_of_full_reply_queue_cleanly() {
        let shared = Arc::new(WorkerShared {
            ready_queue: ArrayQueue::new(MAX_WAVE_TASKS),
            done_queue: ArrayQueue::new(MAX_WAVE_TASKS),
            wave_epoch: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            worker_exited: AtomicBool::new(false),
            worker_tasks_executed: AtomicU64::new(0),
        });
        let parked = Arc::new(AtomicBool::new(false));
        let wakeup = Arc::new(WorkerWakeup::new().unwrap());
        let exited = Arc::new(AtomicBool::new(false));
        let (mut req_tx, req_rx) = RingBuffer::new(4);
        // Capacity 1 reply queue, pre-filled and never drained so WaveDone spins in push_reply_until_shutdown.
        let (mut rep_tx, _rep_rx) = RingBuffer::new(1);
        rep_tx
            .push(WorkerReply::WaveDone(Arc::new(WaveClockWindow::default())))
            .unwrap();

        let worker_shared = Arc::clone(&shared);
        let worker_parked = Arc::clone(&parked);
        let worker_wakeup = Arc::clone(&wakeup);
        let worker_exited = Arc::clone(&exited);
        let handle = thread::Builder::new()
            .name("hex-dsp-wave-shutdown-test".into())
            .spawn(move || {
                worker_loop(
                    worker_shared,
                    worker_parked,
                    worker_wakeup,
                    worker_exited,
                    req_rx,
                    rep_tx,
                    48_000,
                )
            })
            .unwrap();

        let clock = Arc::new(WaveClockWindow::default());
        shared
            .ready_queue
            .push((0, Box::new(WaveTaskSlot::new_idle())))
            .ok()
            .unwrap();
        req_tx.push(WorkerRequest::BeginWave(clock)).unwrap();
        shared.wave_epoch.fetch_add(1, SeqCst);
        wakeup.signal();

        // Give worker a moment to finish the task and enter WaveDone spin-push.
        thread::sleep(Duration::from_millis(5));
        shared.shutdown.store(true, SeqCst);
        shared.wave_epoch.fetch_add(1, SeqCst);
        wakeup.signal();

        handle
            .join()
            .expect("worker thread must exit cleanly on shutdown");
        assert!(!exited.load(Acquire));
        assert!(!shared.worker_exited.load(Acquire));
    }

    #[test]
    fn cold_idle_wakeup_via_semaphore_completes_promptly_and_matches_serial() {
        let revision = dense_revision(16, false);
        let mut serial_runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut parallel_runtime = SessionPlayback::new(revision).unwrap();
        serial_runtime.apply(SessionPlaybackCommand::Play).unwrap();
        parallel_runtime
            .apply(SessionPlaybackCommand::Play)
            .unwrap();

        let mut serial_exec = SerialWaveExecutor::default();
        let mut pool = AudioWorkerPool::new(3, 48_000).unwrap();

        // Wait until all workers finish HOT_SPIN_WINDOW (<= 150 us) and park on their OS semaphore.
        let park_deadline = Instant::now() + Duration::from_millis(200);
        while !pool.workers.iter().all(|w| w.parked.load(Acquire)) {
            assert!(
                Instant::now() < park_deadline,
                "workers must park on semaphore after bounded HOT_SPIN_WINDOW"
            );
            thread::sleep(Duration::from_micros(200));
        }
        assert!(HOT_SPIN_WINDOW <= Duration::from_micros(150));

        let mut expected = [[0.0_f32; 2]; 256];
        let mut actual = [[0.0_f32; 2]; 256];
        serial_runtime
            .render_with_wave_executor(&mut expected, &mut serial_exec)
            .unwrap();
        let wake_start = Instant::now();
        parallel_runtime
            .render_with_wave_executor(&mut actual, &mut pool)
            .unwrap();
        let cold_wave_elapsed = wake_start.elapsed();
        assert!(
            cold_wave_elapsed < Duration::from_millis(100),
            "cold-idle semaphore wakeup wave took {cold_wave_elapsed:?}"
        );
        assert!(
            pool.shared.worker_tasks_executed.load(Acquire) > 0,
            "parked workers must wake and execute stolen tasks"
        );
        for (i, (e, a)) in expected.iter().zip(actual.iter()).enumerate() {
            assert_eq!(
                [e[0].to_bits(), e[1].to_bits()],
                [a[0].to_bits(), a[1].to_bits()],
                "cold-wakeup bit mismatch at frame {i}"
            );
        }
    }

    #[test]
    fn heterogeneous_load_work_stealing_distributes_tasks_and_matches_serial() {
        let mut pool = AudioWorkerPool::new(3, 48_000).unwrap();
        let mut serial = SerialWaveExecutor::default();
        let input = [[0.35_f32, -0.25_f32]; 256];
        let task_count = 16usize;

        let build_chain = |idx: usize| -> NativeRuntimeChain {
            use hex_lattice::{
                DeviceMeter, NativeDeviceDefinition, PreparedProcessorInstance,
                ProcessorChainOwner, StereoMatrix,
            };
            use hex_lattice_contract::DeviceTypeId;
            if idx == 0 {
                // One heavy straggler track with stacked 8x-oversampled Saturators.
                let mut instances = Vec::new();
                let ctx = hex_lattice::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                };
                for stage in 0..8u64 {
                    let state = hex_devices::saturator::SATURATOR_DEFINITION
                        .default_state()
                        .unwrap();
                    let prepared = hex_devices::saturator::SATURATOR_DEFINITION
                        .prepare_processor(&state, &ctx)
                        .unwrap()
                        .unwrap();
                    instances.push(PreparedProcessorInstance {
                        meter: Arc::new(DeviceMeter::default()),
                        owner: ProcessorChainOwner::Track(1),
                        device_id: 10 + stage,
                        device_type: DeviceTypeId::from_static("hex.saturator"),
                        matrix: StereoMatrix::identity(),
                        bypassed: false,
                        sample_rate: 48_000,
                        configuration: Some(Arc::new(state)),
                        definition: Some(Arc::from(prepared)),
                    });
                }
                NativeRuntimeChain::new(instances).unwrap()
            } else {
                utility_chain(idx as u64 + 1, idx as u64 + 100, 0.5)
            }
        };

        let clock_s = serial.prepare_clock().unwrap();
        clock_s.history_len = 0;
        clock_s.incoming_len = 0;
        let clock_p = pool.prepare_clock().unwrap();
        clock_p.history_len = 0;
        clock_p.incoming_len = 0;

        let mut serial_slots = Vec::new();
        let mut pool_slots = Vec::new();
        for idx in 0..task_count {
            let chain_s = build_chain(idx);
            let chain_p = build_chain(idx);
            let stages = chain_s.len();
            serial_slots.push(
                serial
                    .push_task(idx, chain_s, &input, 0, 0, stages, false, &[])
                    .ok()
                    .unwrap(),
            );
            pool_slots.push(
                pool.push_task(idx, chain_p, &input, 0, 0, stages, false, &[])
                    .ok()
                    .unwrap(),
            );
        }

        serial.execute_staged();
        pool.execute_staged();

        assert!(
            pool.shared.worker_tasks_executed.load(Acquire) > 0,
            "work-stealing workers must steal light tasks while heavy straggler runs"
        );
        for (s_slot, p_slot) in serial_slots.into_iter().zip(pool_slots) {
            let mut out_s = [[0.0_f32; 2]; 256];
            let mut out_p = [[0.0_f32; 2]; 256];
            let (owner_s, _, err_s) = serial.reclaim_task(s_slot, &mut out_s);
            let (owner_p, _, err_p) = pool.reclaim_task(p_slot, &mut out_p);
            assert_eq!(owner_s, owner_p);
            assert!(err_s.is_none() && err_p.is_none());
            for (e, a) in out_s.iter().zip(out_p.iter()) {
                assert_eq!(
                    [e[0].to_bits(), e[1].to_bits()],
                    [a[0].to_bits(), a[1].to_bits()]
                );
            }
        }
        serial.finish_wave();
        pool.finish_wave();
    }

    #[test]
    fn stalled_worker_recovers_within_bounded_budget_and_falls_back_inline() {
        let mut pool = AudioWorkerPool::new(2, 48_000).unwrap();
        pool.stall_budget = Duration::from_millis(5);
        // Replace worker 0 with a live-looking but unconsumed request queue and
        // never-populated reply queue so worker 0 accepts BeginWave and never replies.
        let (stalled_tx, _stalled_rx) = RingBuffer::new(4);
        let (_unused_rep_tx, empty_rep_rx) = RingBuffer::new(4);
        let original_handle = pool.workers[0].handle.take();
        pool.workers[0].tx = stalled_tx;
        pool.workers[0].rx = empty_rep_rx;

        let input = [[0.5_f32, -0.25_f32]; 16];
        let clock = pool.prepare_clock().unwrap();
        clock.history_len = 0;
        clock.incoming_len = 0;
        let mut slots = Vec::new();
        for owner_index in 0..6 {
            let chain = utility_chain(owner_index as u64 + 1, owner_index as u64 + 300, 0.5);
            slots.push(
                pool.push_task(owner_index, chain, &input, 0, 0, 1, false, &[])
                    .ok()
                    .unwrap(),
            );
        }

        let start = Instant::now();
        pool.execute_staged();
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(200),
            "bounded stall recovery must exit promptly, took {elapsed:?}"
        );
        assert_eq!(
            pool.worker_count(),
            0,
            "stalled worker must degrade pool to safe inline fallback"
        );
        for (expected_owner, slot) in slots.into_iter().enumerate() {
            let mut out = [[0.0_f32; 2]; 16];
            let (owner, _chain, err) = pool.reclaim_task(slot, &mut out);
            assert_eq!(owner, expected_owner);
            assert!(err.is_none());
        }
        pool.finish_wave();

        // Restore handle before drop so the background thread joins cleanly.
        pool.workers[0].handle = original_handle;
        drop(pool);
    }

    struct TempStreamWav {
        path: std::path::PathBuf,
        frames: u64,
        rate: u32,
    }

    impl TempStreamWav {
        fn new(frames: u32, rate: u32) -> Self {
            use std::io::Write;
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let seq = NEXT_ID.fetch_add(1, Relaxed);
            let data_bytes = frames.checked_mul(4).unwrap();
            let path = std::env::temp_dir().join(format!(
                "hex-worker-pool-stream-{}-{seq}.wav",
                std::process::id()
            ));
            let mut file = std::fs::File::create(&path).unwrap();
            let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
            bytes.extend_from_slice(b"RIFF");
            bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt ");
            bytes.extend_from_slice(&16u32.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&2u16.to_le_bytes());
            bytes.extend_from_slice(&rate.to_le_bytes());
            bytes.extend_from_slice(&rate.checked_mul(4).unwrap().to_le_bytes());
            bytes.extend_from_slice(&4u16.to_le_bytes());
            bytes.extend_from_slice(&16u16.to_le_bytes());
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&data_bytes.to_le_bytes());
            for frame in 0..frames {
                let phase = frame as f32 * 0.017;
                let left = (phase.sin() * 6_000.0) as i16;
                let right = ((phase * 1.13).cos() * 6_000.0) as i16;
                bytes.extend_from_slice(&left.to_le_bytes());
                bytes.extend_from_slice(&right.to_le_bytes());
            }
            file.write_all(&bytes).unwrap();
            file.sync_all().unwrap();
            Self {
                path,
                frames: u64::from(frames),
                rate,
            }
        }

        fn definition(
            &self,
            asset: hex_project::AssetId,
            generation: u64,
        ) -> hex_stream::SourceDefinition {
            let limits = hex_media::VerifiedWavReaderLimits::new(
                hex_media::WavReaderLimits::new(1024 * 1024, self.frames, 32, 32768),
                1024 * 1024,
            );
            let verified = hex_media::VerifiedWavReader::open(&self.path, limits).unwrap();
            let metadata = hex_engine::StreamedSourceMetadata::new(
                hex_engine::StreamSourceKey::new(generation, asset),
                self.rate,
                self.rate,
                self.frames,
                self.frames,
                hex_project::IDENTITY_CONVERSION,
                *verified.fingerprint(),
            )
            .unwrap();
            hex_stream::SourceDefinition::new(self.path.clone(), metadata, limits)
        }
    }

    impl Drop for TempStreamWav {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn setup_streamed_callback(
        worker_count: usize,
        wav: &TempStreamWav,
        track_count: usize,
    ) -> (
        crate::prepared::Control,
        crate::prepared::Callback,
        crate::AudioCandidateDomainToken,
    ) {
        const RATE: u32 = 48_000;
        const GENERATION: u64 = 11;
        let mut editor = Editor::new(Project::new(RATE).unwrap(), 128).unwrap();
        let mut definitions = Vec::with_capacity(track_count);
        let mut sources = BTreeMap::new();
        for i in 0..track_count {
            let Created::Imported { track, asset, .. } = editor
                .execute(Command::ImportPreparedAudioTrack {
                    name: format!("stream-{i}"),
                    path: wav.path.clone(),
                    source_sample_rate: RATE,
                    source_frames: wav.frames,
                    playback_frames: wav.frames,
                    conversion_version: hex_project::IDENTITY_CONVERSION,
                })
                .unwrap()
            else {
                panic!("expected imported track");
            };
            let def = wav.definition(asset, GENERATION);
            sources.insert(
                asset,
                hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(def.metadata.clone())),
            );
            definitions.push(def);
            for state in [
                hex_devices::eq8::EQ8_DEFINITION.default_state().unwrap(),
                hex_devices::filter::DEFINITION.default_state().unwrap(),
                hex_devices::compressor::COMPRESSOR_DEFINITION
                    .default_state()
                    .unwrap(),
            ] {
                editor
                    .execute(Command::InsertDevice {
                        track,
                        before: None,
                        state,
                    })
                    .unwrap();
            }
        }
        let store = hex_engine::native_device_resources::ResidentDeviceAssets::default();
        let cancel = AtomicBool::new(false);
        let resources =
            hex_engine::native_device_resources::PreparedDeviceResources::prepare_on_control(
                &store,
                editor.project(),
                1_000_000,
                &cancel,
            )
            .unwrap();
        let revision =
            hex_engine::PreparedRevision::prepare_with_prepared_audio_sources_and_device_resources(
                editor.project(),
                &sources,
                GENERATION,
                Arc::new(SineTable::new()),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                100_000,
                &cancel,
            )
            .unwrap();
        let playback =
            Playback::try_from_prepared_with_device_resources(Arc::new(revision), &resources)
                .unwrap();

        let (mut control, mut callback) = if worker_count == 0 {
            crate::prepared::channel(RATE).unwrap()
        } else {
            crate::prepared::channel_with_worker_pool(RATE, worker_count).unwrap()
        };
        control.enable_native_custody().unwrap();
        control
            .configure_device_resource_publication_on_control(
                hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 256,
                    root_rows: 128,
                    max_retained_bytes: 20_000_000,
                    max_construction_peak_bytes: 20_000_000,
                },
            )
            .unwrap();
        control
            .configure_ordinary_streaming_on_control(
                crate::AudioCandidateDomainReservation {
                    max_retained_data_bytes: 32 * 1024 * 1024,
                    max_construction_peak_data_bytes: 32 * 1024 * 1024,
                },
                track_count * 2,
                65_536,
            )
            .unwrap();
        let mut runtime = hex_stream::StreamRuntime::new(
            hex_stream::StreamConfig::with_pool_pages(track_count * 4 + 4),
            definitions,
        )
        .unwrap();
        runtime
            .wait_until_ready(Duration::from_secs(2))
            .expect("stream runtime ready");
        let domain_pair = runtime.split().ok().expect("split stream runtime");
        let domain = control
            .register_audio_candidate_domain_with_readiness(
                domain_pair,
                hex_stream::StreamReadinessReservation {
                    extents_per_role: track_count * 2,
                },
            )
            .unwrap_or_else(|_| panic!("register streamed domain"));
        callback.begin_block();
        control
            .publish_with_admission_and_source_on_control(
                playback,
                crate::prepared::PublicationPolicy::InheritRuntime,
                hex_engine::AudioSourceContext {
                    project_generation: GENERATION,
                    editor_revision: 1,
                },
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        (control, callback, domain)
    }

    #[cfg(feature = "allocation-probe")]
    fn wait_for_stream_playback(
        control: &crate::prepared::Control,
        callback: &mut crate::prepared::Callback,
    ) {
        let mut output = [[0.0_f32; 2]; 256];
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while control.status.position.load(Acquire) == 0 {
            callback.begin_block();
            callback.render(&mut output).unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for streamed playback readiness"
            );
            thread::yield_now();
        }
    }

    #[test]
    fn streamed_multi_track_worker_pool_matches_serial_bit_for_bit() {
        let wav = TempStreamWav::new(4096, 48_000);
        let (mut serial_control, mut serial_cb, serial_domain) =
            setup_streamed_callback(0, &wav, 8);
        let (mut parallel_control, mut parallel_cb, parallel_domain) =
            setup_streamed_callback(3, &wav, 8);

        // Wait for both independent stream workers to deliver initial pages and
        // start playback before comparing synchronized blocks.
        let mut serial_blocks = Vec::new();
        let mut parallel_blocks = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while serial_blocks.len() < 4 {
            let mut out = [[0.0_f32; 2]; 256];
            let before = serial_control.status.position.load(Acquire);
            serial_cb.begin_block();
            serial_cb.render(&mut out).unwrap();
            if serial_control.status.position.load(Acquire) > before {
                serial_blocks.push(out);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "serial stream ready (applied={:?}, rejected={:?}, pos={})",
                serial_control.status.applied_revision.load(Acquire),
                serial_control.status.rejected_revision(),
                serial_control.status.position.load(Acquire),
            );
            thread::yield_now();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while parallel_blocks.len() < 4 {
            let mut out = [[0.0_f32; 2]; 256];
            let before = parallel_control.status.position.load(Acquire);
            parallel_cb.begin_block();
            parallel_cb.render(&mut out).unwrap();
            if parallel_control.status.position.load(Acquire) > before {
                parallel_blocks.push(out);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "parallel stream ready"
            );
            thread::yield_now();
        }

        assert!(
            parallel_cb.wave_pool_waves_executed() > 0,
            "streamed multi-track playback must dispatch parallel waves to AudioWorkerPool"
        );
        for (block_idx, (expected, actual)) in
            serial_blocks.iter().zip(parallel_blocks.iter()).enumerate()
        {
            assert!(
                expected.iter().flatten().any(|s| s.abs() > 1e-6),
                "streamed block {block_idx} must contain audible signal"
            );
            for (frame_idx, (e, a)) in expected.iter().zip(actual.iter()).enumerate() {
                assert_eq!(
                    [e[0].to_bits(), e[1].to_bits()],
                    [a[0].to_bits(), a[1].to_bits()],
                    "streamed bit-exact mismatch at block {block_idx} frame {frame_idx}: expected={e:?}, actual={a:?}"
                );
            }
        }
        let _ = (serial_domain, parallel_domain);
        serial_control.reclaim();
        parallel_control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn streamed_multi_track_worker_pool_allocates_zero_bytes() {
        let wav = TempStreamWav::new(4096, 48_000);
        let (mut control, mut callback, _domain) = setup_streamed_callback(3, &wav, 8);
        wait_for_stream_playback(&control, &mut callback);

        let mut output = [[0.0_f32; 2]; 256];
        let waves_before = callback.wave_pool_waves_executed();
        let info = allocation_counter::measure(|| {
            for _ in 0..4 {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            }
        });
        assert!(
            callback.wave_pool_waves_executed() > waves_before,
            "measured streamed blocks must dispatch parallel waves"
        );
        assert!(output.iter().flatten().any(|s| s.abs() > 1e-6));
        assert_eq!(
            (
                info.count_total,
                info.bytes_total,
                info.count_current,
                info.bytes_current,
            ),
            (0, 0, 0, 0),
            "streamed parallel wave callback rendering must perform zero heap allocations/deallocations"
        );
        control.reclaim();
    }
}
