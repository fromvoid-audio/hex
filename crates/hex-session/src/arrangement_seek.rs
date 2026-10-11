//! Bounded control-side Arrangement seek preparation.
//!
//! This worker only constructs a fresh stateful candidate. It does not publish
//! to audio, fence live routes, or mutate the canonical project worker.

use hex_engine::{
    ArrangementPrerollError, MAX_ARRANGEMENT_PREROLL_FRAMES, PreparedRevision, SessionPlayback,
    playback::Playback,
};
use std::{
    io,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

/// Maximum render work a request may ask the engine to perform.
pub const MAX_SEEK_WORK_FRAMES: u64 = MAX_ARRANGEMENT_PREROLL_FRAMES;
/// Dedicated control-side preparation stack for the bounded runtime
/// constructor and preroll renderer. This reservation is independent of the
/// candidate's 32 MiB accounted runtime/scratch ceiling.
pub const SEEK_WORKER_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Candidate policy for Arrangement seek preparation. The default remains the
/// export-style linear owner; callers that will admit Session routes opt into
/// an interactive Session runtime explicitly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArrangementSeekMode {
    #[default]
    LinearArrangement,
    Interactive,
}

/// Control-side identity and destination for one prepared seek candidate.
#[derive(Clone)]
pub struct ArrangementSeekSpec {
    pub mode: ArrangementSeekMode,
    pub generation: u64,
    pub revision: u64,
    pub prepared_revision: Arc<PreparedRevision>,
    pub destination_frame: u64,
    pub work_budget_frames: u64,
    pub start_playing: bool,
}

/// One bounded result. The candidate retains the same prepared revision Arc.
pub struct ArrangementSeekResult {
    pub request_id: u64,
    pub mode: ArrangementSeekMode,
    pub generation: u64,
    pub revision: u64,
    pub prepared_revision: Arc<PreparedRevision>,
    pub destination_frame: u64,
    pub work_budget_frames: u64,
    pub start_playing: bool,
    pub result: Result<Playback, ArrangementPrerollError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangementSeekSubmitError {
    Closed,
    RequestIdsExhausted,
    RetirementPending,
}

type Preparer =
    dyn Fn(ArrangementSeekSpec, &AtomicBool) -> Result<Playback, ArrangementPrerollError> + Send;

struct Work {
    request_id: u64,
    spec: ArrangementSeekSpec,
}

struct Running {
    request_id: u64,
    cancelled: Arc<AtomicBool>,
}

struct Mailbox {
    next_request_id: Option<u64>,
    latest_request_id: Option<u64>,
    pending: Option<Work>,
    running: Option<Running>,
    ready: Option<ArrangementSeekResult>,
    retired: Option<ArrangementSeekResult>,
    shutdown: bool,
    #[cfg(test)]
    test_pause_drain: bool,
    #[cfg(test)]
    last_result_drop_thread: Option<thread::ThreadId>,
}

struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
}

/// Dedicated seek worker, independent of canonical edit and export workers.
/// At most one request is running, one newer request is pending, and one result
/// is ready. Under the normal path it constructs one candidate at a time; the
/// fixed ready/retired slots may briefly own two completed candidates while a
/// stale result is reclaimed on the worker before pending preparation begins.
pub struct ArrangementSeekWorker {
    shared: Arc<Shared>,
    _worker: JoinHandle<()>,
}

impl ArrangementSeekWorker {
    pub fn new() -> io::Result<Self> {
        Self::with_preparer(prepare)
    }

    fn with_preparer(
        preparer: impl Fn(ArrangementSeekSpec, &AtomicBool) -> Result<Playback, ArrangementPrerollError>
        + Send
        + 'static,
    ) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox {
                next_request_id: Some(1),
                latest_request_id: None,
                pending: None,
                running: None,
                ready: None,
                retired: None,
                shutdown: false,
                #[cfg(test)]
                test_pause_drain: false,
                #[cfg(test)]
                last_result_drop_thread: None,
            }),
            wake: Condvar::new(),
        });
        let thread_shared = Arc::clone(&shared);
        let preparer: Box<Preparer> = Box::new(preparer);
        let worker = thread::Builder::new()
            .name("hex-arrangement-seek".into())
            .stack_size(SEEK_WORKER_STACK_BYTES)
            .spawn(move || run_worker(thread_shared, preparer))?;
        Ok(Self {
            shared,
            _worker: worker,
        })
    }

    /// Admit without waiting for preroll. This replaces pending work and
    /// invalidates any ready result. Replaced pending request metadata and its
    /// prepared-revision handle are dropped by the submitter after unlocking;
    /// unread runtime results are reclaimed by the worker outside the lock.
    pub fn submit(&self, spec: ArrangementSeekSpec) -> Result<u64, ArrangementSeekSubmitError> {
        let mut mailbox = lock(&self.shared.mailbox);
        if mailbox.shutdown {
            return Err(ArrangementSeekSubmitError::Closed);
        }
        if mailbox.retired.is_some() {
            return Err(ArrangementSeekSubmitError::RetirementPending);
        }
        let Some(request_id) = mailbox.next_request_id else {
            return Err(ArrangementSeekSubmitError::RequestIdsExhausted);
        };
        mailbox.next_request_id = request_id.checked_add(1);
        mailbox.latest_request_id = Some(request_id);
        if let Some(running) = mailbox.running.as_ref() {
            running.cancelled.store(true, Ordering::Release);
        }
        let old_pending = mailbox.pending.replace(Work { request_id, spec });
        if mailbox.retired.is_none() {
            mailbox.retired = mailbox.ready.take();
        }
        drop(mailbox);
        drop(old_pending);
        self.shared.wake.notify_one();
        Ok(request_id)
    }

    /// Invalidate all admitted work. Pending request metadata and its prepared
    /// revision handle are reclaimed by the caller after unlocking; unread
    /// runtime results are reclaimed by the worker outside the lock. A running
    /// bounded preroll observes cancellation at its next render-block boundary.
    pub fn invalidate(&self) {
        let mut mailbox = lock(&self.shared.mailbox);
        mailbox.latest_request_id = None;
        if let Some(running) = mailbox.running.as_ref() {
            running.cancelled.store(true, Ordering::Release);
        }
        let pending = mailbox.pending.take();
        if mailbox.retired.is_none() {
            mailbox.retired = mailbox.ready.take();
        }
        drop(mailbox);
        drop(pending);
        self.shared.wake.notify_one();
    }

    /// Consume a completed result without waiting.
    pub fn try_result(&self) -> Option<ArrangementSeekResult> {
        let mut mailbox = lock(&self.shared.mailbox);
        if mailbox
            .ready
            .as_ref()
            .is_some_and(|result| mailbox.latest_request_id == Some(result.request_id))
        {
            mailbox.ready.take()
        } else {
            None
        }
    }

    /// Return a UI-stale candidate to the worker's bounded retirement slot.
    /// The result is returned unchanged when the slot is occupied or the
    /// worker is shutting down, so the caller can retain ownership and retry.
    #[allow(clippy::result_large_err)] // Preserve the candidate intact on backpressure.
    pub fn retire_result(
        &self,
        result: ArrangementSeekResult,
    ) -> Result<(), ArrangementSeekResult> {
        let mut mailbox = lock(&self.shared.mailbox);
        if mailbox.shutdown || mailbox.retired.is_some() {
            return Err(result);
        }
        mailbox.retired = Some(result);
        drop(mailbox);
        self.shared.wake.notify_one();
        Ok(())
    }

    /// Return a UI-held result during owner teardown. Prefer the normal
    /// retirement slot; if it is occupied, use the ready slot only when empty.
    /// The worker's shutdown path drains both slots on its own thread.
    #[allow(clippy::result_large_err)] // The result remains intact if both bounded slots are occupied.
    pub fn retire_result_for_shutdown(
        &self,
        result: ArrangementSeekResult,
    ) -> Result<(), ArrangementSeekResult> {
        let mut mailbox = lock(&self.shared.mailbox);
        if mailbox.shutdown {
            return Err(result);
        }
        if mailbox.retired.is_none() {
            mailbox.retired = Some(result);
        } else if mailbox.ready.is_none() {
            mailbox.ready = Some(result);
        } else {
            return Err(result);
        }
        drop(mailbox);
        self.shared.wake.notify_one();
        Ok(())
    }
}

impl Drop for ArrangementSeekWorker {
    fn drop(&mut self) {
        let mut mailbox = lock(&self.shared.mailbox);
        mailbox.shutdown = true;
        mailbox.latest_request_id = None;
        if let Some(running) = mailbox.running.as_ref() {
            running.cancelled.store(true, Ordering::Release);
        }
        drop(mailbox);
        self.shared.wake.notify_one();
        // Detach: the UI never joins this thread. The worker drains and drops
        // pending/ready candidates itself before it exits.
    }
}

fn prepare(
    spec: ArrangementSeekSpec,
    cancelled: &AtomicBool,
) -> Result<Playback, ArrangementPrerollError> {
    match spec.mode {
        ArrangementSeekMode::LinearArrangement => {
            let runtime = SessionPlayback::prepare_arrangement_preroll_from_revision(
                Arc::clone(&spec.prepared_revision),
                spec.destination_frame,
                spec.work_budget_frames,
                spec.start_playing,
                cancelled,
            )?;
            Playback::from_arrangement_preroll(runtime)
        }
        ArrangementSeekMode::Interactive => {
            let runtime = SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                Arc::clone(&spec.prepared_revision),
                spec.destination_frame,
                spec.work_budget_frames,
                spec.start_playing,
                cancelled,
            )?;
            Playback::from_interactive_arrangement_preroll(runtime)
        }
    }
}

fn run_worker(shared: Arc<Shared>, preparer: Box<Preparer>) {
    'worker: loop {
        let work = {
            let mut mailbox = lock(&shared.mailbox);
            loop {
                if mailbox.shutdown {
                    let pending = mailbox.pending.take();
                    let ready = mailbox.ready.take();
                    let retired = mailbox.retired.take();
                    #[cfg(test)]
                    if ready.is_some() || retired.is_some() {
                        mailbox.last_result_drop_thread = Some(thread::current().id());
                    }
                    drop(mailbox);
                    drop(pending);
                    drop(ready);
                    drop(retired);
                    return;
                }
                #[cfg(test)]
                if mailbox.test_pause_drain {
                    mailbox = wait(&shared.wake, mailbox);
                    continue;
                }
                if let Some(retired) = mailbox.retired.take() {
                    #[cfg(test)]
                    {
                        mailbox.last_result_drop_thread = Some(thread::current().id());
                    }
                    drop(mailbox);
                    drop(retired);
                    continue 'worker;
                }
                if mailbox
                    .ready
                    .as_ref()
                    .is_some_and(|ready| mailbox.latest_request_id != Some(ready.request_id))
                {
                    let stale_ready = mailbox.ready.take();
                    #[cfg(test)]
                    {
                        mailbox.last_result_drop_thread = Some(thread::current().id());
                    }
                    drop(mailbox);
                    drop(stale_ready);
                    continue 'worker;
                }
                if let Some(work) = mailbox.pending.take() {
                    let cancelled = Arc::new(AtomicBool::new(false));
                    mailbox.running = Some(Running {
                        request_id: work.request_id,
                        cancelled: Arc::clone(&cancelled),
                    });
                    break (work, cancelled);
                }
                mailbox = wait(&shared.wake, mailbox);
            }
        };

        let (work, cancelled) = work;
        let result = preparer(work.spec.clone(), &cancelled);
        let result = ArrangementSeekResult {
            request_id: work.request_id,
            mode: work.spec.mode,
            generation: work.spec.generation,
            revision: work.spec.revision,
            prepared_revision: Arc::clone(&work.spec.prepared_revision),
            destination_frame: work.spec.destination_frame,
            work_budget_frames: work.spec.work_budget_frames,
            start_playing: work.spec.start_playing,
            result,
        };
        let mut mailbox = lock(&shared.mailbox);
        if mailbox
            .running
            .as_ref()
            .is_some_and(|running| running.request_id == work.request_id)
        {
            mailbox.running = None;
        }
        let accepted = !mailbox.shutdown
            && mailbox.latest_request_id == Some(work.request_id)
            && !cancelled.load(Ordering::Acquire);
        if accepted {
            let superseded = mailbox.ready.replace(result);
            drop(mailbox);
            drop(superseded);
        } else {
            drop(mailbox);
            drop(result);
        }
    }
}

fn lock(mutex: &Mutex<Mailbox>) -> MutexGuard<'_, Mailbox> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn wait<'a>(wake: &Condvar, mailbox: MutexGuard<'a, Mailbox>) -> MutexGuard<'a, Mailbox> {
    wake.wait(mailbox)
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    fn prepared() -> Arc<PreparedRevision> {
        Session::new(48_000, 4, 0)
            .expect("session")
            .compile_prepared_revision(None)
            .expect("empty project compiles")
    }

    fn spec(prepared_revision: Arc<PreparedRevision>, generation: u64) -> ArrangementSeekSpec {
        ArrangementSeekSpec {
            mode: ArrangementSeekMode::LinearArrangement,
            generation,
            revision: generation + 10,
            prepared_revision,
            destination_frame: 0,
            work_budget_frames: 0,
            start_playing: false,
        }
    }

    fn wait_for_ready(worker: &ArrangementSeekWorker) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if lock(&worker.shared.mailbox).ready.is_some() {
                return;
            }
            assert!(Instant::now() < deadline, "worker did not publish a result");
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_for_idle(worker: &ArrangementSeekWorker) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mailbox = lock(&worker.shared.mailbox);
            if mailbox.running.is_none()
                && mailbox.pending.is_none()
                && mailbox.retired.is_none()
                && mailbox.ready.is_none()
            {
                return;
            }
            drop(mailbox);
            assert!(Instant::now() < deadline, "worker did not become idle");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn newer_request_cancels_running_and_replaces_pending_work() {
        let graph = prepared();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = Arc::clone(&calls);
        let worker = ArrangementSeekWorker::with_preparer(move |spec, cancelled| {
            let call = worker_calls.fetch_add(1, Ordering::AcqRel);
            started_tx.send(spec.generation).expect("started receiver");
            if call == 0 {
                release_rx.recv().expect("release first preroll");
            }
            prepare(spec, cancelled)
        })
        .expect("seek worker");

        let first = worker.submit(spec(Arc::clone(&graph), 1)).expect("first");
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(2)), Ok(1));
        let superseded = worker.submit(spec(Arc::clone(&graph), 2)).expect("second");
        let latest = worker.submit(spec(Arc::clone(&graph), 3)).expect("third");
        assert!(first < superseded && superseded < latest);
        release_tx.send(()).expect("release");
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(2)), Ok(3));
        wait_for_ready(&worker);
        let result = worker.try_result().expect("latest result");
        assert_eq!(result.request_id, latest);
        assert_eq!(result.generation, 3);
        assert_eq!(result.revision, 13);
        assert_eq!(result.destination_frame, 0);
        assert_eq!(result.work_budget_frames, 0);
        assert!(!result.start_playing);
        assert!(Arc::ptr_eq(&result.prepared_revision, &graph));
        assert!(result.result.is_ok());
        assert_eq!(calls.load(Ordering::Acquire), 2, "superseded work ran");
        assert!(worker.try_result().is_none());
    }

    #[test]
    fn invalidation_cancels_running_and_discards_pending_and_ready_results() {
        let graph = prepared();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = ArrangementSeekWorker::with_preparer(move |spec, cancelled| {
            started_tx.send(()).expect("started receiver");
            release_rx.recv().expect("release preroll");
            prepare(spec, cancelled)
        })
        .expect("seek worker");
        worker.submit(spec(Arc::clone(&graph), 1)).expect("first");
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("started");
        worker.submit(spec(Arc::clone(&graph), 2)).expect("pending");
        worker.invalidate();
        release_tx.send(()).expect("release");
        wait_for_idle(&worker);
        assert!(worker.try_result().is_none());

        let ready_worker = ArrangementSeekWorker::new().expect("ready worker");
        ready_worker
            .submit(spec(Arc::clone(&graph), 3))
            .expect("ready request");
        wait_for_ready(&ready_worker);
        ready_worker.invalidate();
        assert!(ready_worker.try_result().is_none());
    }

    #[test]
    fn ready_slot_never_blocks_new_admission_and_is_replaced() {
        let graph = prepared();
        let worker = ArrangementSeekWorker::new().expect("seek worker");
        worker.submit(spec(Arc::clone(&graph), 1)).expect("first");
        wait_for_ready(&worker);
        let second = worker.submit(spec(Arc::clone(&graph), 2)).expect("second");
        assert!(
            lock(&worker.shared.mailbox)
                .ready
                .as_ref()
                .is_none_or(|result| result.request_id == second)
        );
        wait_for_ready(&worker);
        let result = worker.try_result().expect("second result");
        assert_eq!(result.request_id, second);
        assert_eq!(result.generation, 2);
    }

    #[test]
    fn retired_result_owns_backpressure_until_worker_destruction() {
        let graph = prepared();
        let worker = ArrangementSeekWorker::new().expect("seek worker");
        worker.submit(spec(Arc::clone(&graph), 1)).expect("first");
        wait_for_ready(&worker);
        let result = worker.try_result().expect("completed result");
        {
            let mut mailbox = lock(&worker.shared.mailbox);
            mailbox.test_pause_drain = true;
        }
        assert!(worker.retire_result(result).is_ok());
        assert!(matches!(
            worker.submit(spec(Arc::clone(&graph), 2)),
            Err(ArrangementSeekSubmitError::RetirementPending)
        ));
        {
            let mut mailbox = lock(&worker.shared.mailbox);
            mailbox.test_pause_drain = false;
        }
        worker.shared.wake.notify_one();
        wait_for_idle(&worker);
        assert!(worker.submit(spec(Arc::clone(&graph), 3)).is_ok());
    }

    #[test]
    fn shutdown_handoff_uses_empty_ready_slot_when_retirement_is_occupied() {
        let retired_graph = prepared();
        let held_graph = prepared();
        let overflow_graph = prepared();
        let retired_weak = Arc::downgrade(&retired_graph);
        let held_weak = Arc::downgrade(&held_graph);
        let worker = ArrangementSeekWorker::new().expect("seek worker");
        let shared = Arc::clone(&worker.shared);
        let completed = |graph, generation| {
            let candidate = ArrangementSeekWorker::new().expect("candidate worker");
            candidate
                .submit(spec(graph, generation))
                .expect("candidate request");
            wait_for_ready(&candidate);
            let result = candidate.try_result().expect("candidate result");
            drop(candidate);
            result
        };
        let retired = completed(Arc::clone(&retired_graph), 1);
        let held = completed(Arc::clone(&held_graph), 2);
        let overflow = completed(Arc::clone(&overflow_graph), 3);
        {
            let mut mailbox = lock(&worker.shared.mailbox);
            // Hold both bounded slots until owner teardown; shutdown must drain
            // them even while normal worker drainage is paused.
            mailbox.test_pause_drain = true;
            mailbox.retired = Some(retired);
        }
        drop(retired_graph);
        drop(held_graph);

        assert!(worker.retire_result_for_shutdown(held).is_ok());
        let overflow = worker
            .retire_result_for_shutdown(overflow)
            .expect_err("both bounded worker slots are already occupied");
        assert!(Arc::ptr_eq(&overflow.prepared_revision, &overflow_graph));
        assert!(retired_weak.upgrade().is_some());
        assert!(held_weak.upgrade().is_some());
        drop(overflow);
        drop(overflow_graph);
        drop(worker);
        let deadline = Instant::now() + Duration::from_secs(2);
        while retired_weak.upgrade().is_some() || held_weak.upgrade().is_some() {
            assert!(
                Instant::now() < deadline,
                "worker did not drain teardown results"
            );
            thread::yield_now();
        }
        let observed_drop_thread = lock(&shared.mailbox)
            .last_result_drop_thread
            .expect("shutdown worker recorded result destruction");
        assert_ne!(observed_drop_thread, thread::current().id());
    }

    #[test]
    fn worker_retires_unread_candidate_before_preparing_replacement() {
        let first_graph = prepared();
        let first_weak = Arc::downgrade(&first_graph);
        let (retired_tx, retired_rx) = mpsc::channel();
        let weak_for_worker = first_weak.clone();
        let worker = ArrangementSeekWorker::with_preparer(move |spec, cancelled| {
            if spec.generation == 2 {
                retired_tx
                    .send(weak_for_worker.upgrade().is_none())
                    .expect("retirement receiver");
            }
            prepare(spec, cancelled)
        })
        .expect("seek worker");
        worker
            .submit(spec(Arc::clone(&first_graph), 1))
            .expect("first");
        wait_for_ready(&worker);
        drop(first_graph);

        let second_graph = prepared();
        worker.submit(spec(second_graph, 2)).expect("replacement");
        assert_eq!(
            retired_rx.recv_timeout(Duration::from_secs(2)),
            Ok(true),
            "pending preparation began before stale result was reclaimed"
        );
        wait_for_ready(&worker);
        assert!(first_weak.upgrade().is_none());
    }

    #[test]
    fn defensive_retirement_and_stale_ready_slots_are_only_dropped_by_worker() {
        let retired_graph = prepared();
        let ready_graph = prepared();
        let retired_weak = Arc::downgrade(&retired_graph);
        let ready_weak = Arc::downgrade(&ready_graph);
        let worker = ArrangementSeekWorker::new().expect("seek worker");
        let make_result = |graph: Arc<PreparedRevision>, request_id: u64| {
            let preparer_worker = ArrangementSeekWorker::new().expect("candidate worker");
            let request_id = preparer_worker
                .submit(spec(graph, request_id))
                .expect("candidate request");
            wait_for_ready(&preparer_worker);
            let result = preparer_worker
                .try_result()
                .expect("prepared candidate result");
            assert_eq!(result.request_id, request_id);
            drop(preparer_worker);
            result
        };
        {
            let mut mailbox = lock(&worker.shared.mailbox);
            mailbox.test_pause_drain = true;
            mailbox.latest_request_id = Some(4);
            mailbox.retired = Some(make_result(Arc::clone(&retired_graph), 2));
            mailbox.ready = Some(make_result(Arc::clone(&ready_graph), 3));
        }
        drop(retired_graph);
        drop(ready_graph);
        worker.invalidate();
        assert!(retired_weak.upgrade().is_some());
        assert!(ready_weak.upgrade().is_some());

        let mut mailbox = lock(&worker.shared.mailbox);
        mailbox.test_pause_drain = false;
        drop(mailbox);
        worker.shared.wake.notify_one();
        wait_for_idle(&worker);
        let deadline = Instant::now() + Duration::from_secs(2);
        while retired_weak.upgrade().is_some() || ready_weak.upgrade().is_some() {
            assert!(
                Instant::now() < deadline,
                "worker slots cleared before their runtime owners were dropped"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn bounded_work_refusal_is_returned_with_request_identity() {
        let graph = prepared();
        let worker = ArrangementSeekWorker::new().expect("seek worker");
        let mut invalid = spec(graph.clone(), 7);
        invalid.destination_frame = 1;
        invalid.work_budget_frames = 0;
        let request_id = worker.submit(invalid).expect("bounded request");
        wait_for_ready(&worker);
        let result = worker.try_result().expect("refusal result");
        assert_eq!(result.request_id, request_id);
        assert_eq!(result.generation, 7);
        assert_eq!(result.revision, 17);
        assert!(matches!(
            result.result,
            Err(ArrangementPrerollError::WorkBudgetExceeded)
        ));
        assert!(Arc::ptr_eq(&result.prepared_revision, &graph));
    }

    #[test]
    fn dropping_worker_cancels_and_wakes_without_joining() {
        struct ExitSignal(mpsc::Sender<thread::ThreadId>);
        impl Drop for ExitSignal {
            fn drop(&mut self) {
                let _ = self.0.send(thread::current().id());
            }
        }

        let graph = prepared();
        let (exit_tx, exit_rx) = mpsc::channel();
        let signal = ExitSignal(exit_tx);
        let worker = ArrangementSeekWorker::with_preparer(move |_spec, cancelled| {
            let _keep_signal = &signal;
            while !cancelled.load(Ordering::Acquire) {
                thread::yield_now();
            }
            Err(ArrangementPrerollError::Cancelled)
        })
        .expect("seek worker");
        worker.submit(spec(graph, 1)).expect("request");
        let worker_thread = worker._worker.thread().id();
        let caller_thread = thread::current().id();
        drop(worker);
        let drop_thread = exit_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("detached worker exited");
        assert_eq!(drop_thread, worker_thread);
        assert_ne!(drop_thread, caller_thread);
    }

    #[test]
    fn result_identity_is_invalidated_if_newer_admission_wins_publish_race() {
        let graph = prepared();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = Arc::clone(&calls);
        let worker = ArrangementSeekWorker::with_preparer(move |spec, cancelled| {
            if worker_calls.fetch_add(1, Ordering::AcqRel) == 0 {
                started_tx.send(()).expect("started receiver");
                release_rx.recv().expect("release preroll");
            }
            prepare(spec, cancelled)
        })
        .expect("seek worker");
        worker.submit(spec(Arc::clone(&graph), 1)).expect("first");
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first started");
        worker.submit(spec(graph, 2)).expect("second");
        release_tx.send(()).expect("release first");
        wait_for_ready(&worker);
        let result = worker.try_result().expect("new result");
        assert_eq!(result.generation, 2);
    }
}
