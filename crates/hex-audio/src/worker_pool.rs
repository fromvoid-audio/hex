//! Bounded lock-free real-time worker pool implementing `hex_engine::WaveExecutor`.
//!
//! Spawns a bounded set of persistent worker threads on the control thread with
//! preallocated SPSC `rtrb` channels, preallocated `WaveTaskSlot` buffers, and a
//! single shared read-only `Arc<WaveClockWindow>` whose strong count returns to 1
//! after every wave (zero callback allocations, zero callback drops, zero mutexes).
//!
//! On macOS, active workers non-fatally borrow CoreAudio Mach time-constraint
//! scheduling via `cpal::platform::CoreAudioWorkerTimeConstraint` and release it
//! when idle or shutting down.

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
            Ordering::{Acquire, Relaxed, Release},
        },
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Upper bound on persistent background worker threads (caller thread also participates,
/// giving up to 8 concurrent DSP threads).
pub const MAX_WORKERS: usize = 7;

/// Duration workers remain in a low-latency atomic spin after completing a wave
/// before releasing real-time priority and entering short idle sleeps.
const HOT_SPIN_WINDOW: Duration = Duration::from_millis(25);

/// Sleep cadence while idle (sub-millisecond wake on playback start, minimal idle CPU).
const IDLE_POLL_INTERVAL: Duration = Duration::from_micros(250);

/// Channel capacity per worker: enough for a full wave plus BeginWave/EndWave framing.
const WORKER_QUEUE_CAPACITY: usize = MAX_WAVE_TASKS + 4;

enum WorkerRequest {
    BeginWave(Arc<WaveClockWindow>),
    Task {
        slot_index: usize,
        task: Box<WaveTaskSlot>,
    },
    EndWave,
}

enum WorkerReply {
    Task {
        slot_index: usize,
        task: Box<WaveTaskSlot>,
    },
    WaveDone(Arc<WaveClockWindow>),
}

struct WorkerEndpoint {
    tx: Producer<WorkerRequest>,
    rx: Consumer<WorkerReply>,
    handle: Option<JoinHandle<()>>,
}

struct WorkerShared {
    wave_epoch: AtomicU64,
    shutdown: AtomicBool,
    worker_exited: AtomicBool,
}

/// Guard ensuring `worker_exited` is flagged if a worker thread ever terminates early
/// so the callback spin-wait can never deadlock.
struct ExitFlagGuard<'a> {
    flag: &'a AtomicBool,
    clean: bool,
}

impl Drop for ExitFlagGuard<'_> {
    fn drop(&mut self) {
        if !self.clean {
            self.flag.store(true, Release);
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
struct WorkerDeadline;

#[cfg(not(target_os = "macos"))]
impl WorkerDeadline {
    fn for_current_thread(_sample_rate: u32) -> Self {
        Self
    }
    fn enter(&mut self) {}
    fn leave(&mut self) {}
}

fn worker_loop(
    shared: Arc<WorkerShared>,
    mut rx: Consumer<WorkerRequest>,
    mut tx: Producer<WorkerReply>,
    sample_rate: u32,
) {
    let mut exit_guard = ExitFlagGuard {
        flag: &shared.worker_exited,
        clean: false,
    };
    let mut deadline = WorkerDeadline::for_current_thread(sample_rate);
    deadline.enter();
    let mut last_active = Instant::now();
    let mut observed_epoch = shared.wave_epoch.load(Acquire);
    let mut active_clock: Option<Arc<WaveClockWindow>> = None;

    loop {
        let mut progressed = false;
        while let Ok(msg) = rx.pop() {
            progressed = true;
            match msg {
                WorkerRequest::BeginWave(clock) => {
                    deadline.enter();
                    active_clock = Some(clock);
                }
                WorkerRequest::Task {
                    slot_index,
                    mut task,
                } => {
                    if let Some(clock) = active_clock.as_deref() {
                        task.run(clock);
                    }
                    let mut reply = WorkerReply::Task { slot_index, task };
                    while let Err(rtrb::PushError::Full(returned)) = tx.push(reply) {
                        reply = returned;
                        std::hint::spin_loop();
                    }
                }
                WorkerRequest::EndWave => {
                    if let Some(clock) = active_clock.take() {
                        let mut reply = WorkerReply::WaveDone(clock);
                        while let Err(rtrb::PushError::Full(returned)) = tx.push(reply) {
                            reply = returned;
                            std::hint::spin_loop();
                        }
                    }
                    last_active = Instant::now();
                }
            }
        }

        if progressed {
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

        if last_active.elapsed() < HOT_SPIN_WINDOW {
            std::hint::spin_loop();
        } else {
            deadline.leave();
            thread::sleep(IDLE_POLL_INTERVAL);
        }
    }

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
    staged: usize,
    sample_rate: u32,
}

impl AudioWorkerPool {
    /// Spawn a bounded worker pool on control with `worker_count` background threads
    /// (clamped to `0..=MAX_WORKERS`). `worker_count == 0` runs serially inline.
    pub fn new(worker_count: usize, sample_rate: u32) -> Result<Self, Invalid> {
        let clamped_workers = worker_count.min(MAX_WORKERS);
        let shared = Arc::new(WorkerShared {
            wave_epoch: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            worker_exited: AtomicBool::new(false),
        });
        let mut workers: Vec<WorkerEndpoint> = Vec::with_capacity(clamped_workers);
        for index in 0..clamped_workers {
            let (req_tx, req_rx) = RingBuffer::new(WORKER_QUEUE_CAPACITY);
            let (rep_tx, rep_rx) = RingBuffer::new(WORKER_QUEUE_CAPACITY);
            let worker_shared = Arc::clone(&shared);
            let builder = thread::Builder::new().name(format!("hex-dsp-wave-{index}"));
            let handle = builder
                .spawn(move || worker_loop(worker_shared, req_rx, rep_tx, sample_rate))
                .map_err(|_| {
                    shared.shutdown.store(true, Release);
                    shared.wave_epoch.fetch_add(1, Release);
                    for mut spawned in workers.drain(..) {
                        if let Some(handle) = spawned.handle.take() {
                            let _ = handle.join();
                        }
                    }
                    Invalid("could not spawn audio dsp wave worker thread")
                })?;
            workers.push(WorkerEndpoint {
                tx: req_tx,
                rx: rep_rx,
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
            staged: 0,
            sample_rate,
        })
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
        self.shared.shutdown.store(true, Release);
        self.shared.wave_epoch.fetch_add(1, Release);
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
        let active_threads = (self.workers.len() + 1).min(total);
        let active_workers = active_threads - 1;
        let base_chunk = total / active_threads;
        let extra = total % active_threads;

        // Caller thread executes chunk 0 locally while workers execute chunks 1..active_threads.
        let caller_count = base_chunk + usize::from(extra > 0);
        let mut cursor = caller_count;
        let mut dispatched_workers = 0usize;

        for w in 0..active_workers {
            let chunk_len = base_chunk + usize::from(w + 1 < extra);
            if chunk_len == 0 {
                break;
            }
            let start = cursor;
            let end = cursor + chunk_len;
            cursor = end;

            let worker = &mut self.workers[w];
            if worker.tx.slots() < chunk_len + 2 {
                // Unexpected queue saturation: leave remaining tasks for inline execution.
                cursor = start;
                break;
            }
            if worker
                .tx
                .push(WorkerRequest::BeginWave(Arc::clone(&self.clock)))
                .is_err()
            {
                cursor = start;
                break;
            }
            let mut pushed = 0usize;
            for slot_index in start..end {
                if let Some(task) = self.slots[slot_index].take() {
                    match worker.tx.push(WorkerRequest::Task { slot_index, task }) {
                        Ok(()) => pushed += 1,
                        Err(rtrb::PushError::Full(WorkerRequest::Task { slot_index, task })) => {
                            self.slots[slot_index] = Some(task);
                            break;
                        }
                        Err(_) => break,
                    }
                }
            }
            let _ = worker.tx.push(WorkerRequest::EndWave);
            if pushed > 0 {
                dispatched_workers += 1;
            }
        }

        if dispatched_workers > 0 {
            self.shared.wave_epoch.fetch_add(1, Release);
        }

        // Caller executes its own slice (plus any tail not dispatched to workers).
        {
            let clock = &*self.clock;
            for slot in self.slots[..caller_count].iter_mut().flatten() {
                slot.run(clock);
            }
            for slot in self.slots[cursor..total].iter_mut().flatten() {
                slot.run(clock);
            }
        }

        // Drain replies from all dispatched workers lock-free.
        let mut worker_done = [false; MAX_WORKERS];
        let mut remaining_workers = dispatched_workers;
        while remaining_workers > 0 {
            let mut any = false;
            for (w, done) in worker_done[..dispatched_workers].iter_mut().enumerate() {
                if *done {
                    continue;
                }
                while let Ok(reply) = self.workers[w].rx.pop() {
                    any = true;
                    match reply {
                        WorkerReply::Task { slot_index, task } => {
                            self.slots[slot_index] = Some(task);
                        }
                        WorkerReply::WaveDone(clock_clone) => {
                            // `self.clock` remains alive (strong count >= 1), so this drop
                            // only decrements the atomic refcount with zero deallocation.
                            drop(clock_clone);
                            *done = true;
                            remaining_workers -= 1;
                            break;
                        }
                    }
                }
            }
            if !any {
                if self.shared.worker_exited.load(Acquire) {
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
            (
                0,
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
}
