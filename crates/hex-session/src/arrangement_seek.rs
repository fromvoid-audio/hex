//! Bounded control-side Arrangement seek preparation.
//!
//! This worker only constructs a fresh stateful candidate (ordinary interactive seeks and
//! sections start device history empty; linear and explicitly history-preserving
//! interactive modes replay). It does not publish
//! to audio, fence live routes, or mutate the canonical project worker.

use hex_engine::{
    ArrangementPrerollError, MAX_ARRANGEMENT_PREROLL_FRAMES, PreparedRevision, SessionPlayback,
    native_device_resources::PreparedDeviceResources, playback::Playback,
};
use std::{
    io,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

/// Maximum prefix replay a request may ask the engine to perform. Ordinary
/// interactive and section destinations replay no prefix and are not limited by it.
pub const MAX_SEEK_WORK_FRAMES: u64 = MAX_ARRANGEMENT_PREROLL_FRAMES;
/// Dedicated control-side preparation stack for the bounded runtime
/// constructor and preroll renderer. This reservation is independent of the
/// candidate's accounted runtime/scratch ceiling, defined by
/// [`hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES`].
pub const SEEK_WORKER_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Candidate policy for Arrangement seek preparation. The default remains the
/// export-style linear owner; callers that will admit Session routes opt into
/// an interactive Session runtime explicitly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArrangementSeekMode {
    #[default]
    LinearArrangement,
    /// Replay bounded linear Arrangement history on control, retaining an
    /// interactive runtime. Enabled loops refuse; file-backed requests require
    /// their bounded page preparer to honor this exact policy.
    HistoryPreservingInteractive,
    Interactive,
    /// Fresh playing section loop. The region must be enabled, destination must
    /// equal its start, and start_playing must be true; malformed specs refuse.
    SectionLoop(hex_project::LoopRegion),
    /// Fresh playing section with a finite end and no enabled canonical loop.
    SectionOnce(hex_project::LoopRegion),
}

/// Transient click intent. The region's enabled bit selects a canonical loop
/// or a one-shot section. Both use the predecessor clock for quantized launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionLaunch {
    pub region: hex_project::LoopRegion,
    pub quantization: hex_engine::LaunchQuantization,
}

impl SectionLaunch {
    pub fn mode(self) -> ArrangementSeekMode {
        if self.region.enabled() {
            ArrangementSeekMode::SectionLoop(self.region)
        } else {
            ArrangementSeekMode::SectionOnce(self.region)
        }
    }

    pub fn matches_loop(self, current: Option<hex_project::LoopRegion>) -> bool {
        if self.region.enabled() {
            current == Some(self.region)
        } else {
            current.is_none_or(|region| !region.enabled())
        }
    }
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
    resident: Option<ResidentWork>,
}
struct ResidentWork {
    job: u64,
    resources: PreparedDeviceResources,
    cancelled: Arc<AtomicBool>,
    files: Option<(
        crate::service::ResidentFileSet,
        crate::service::ResidentFileSeekPreparer,
    )>,
}
/// Existing pending/running/ready/retired slots plus the worker's bounded local destruction.
/// Canonical native dispatch must retain at most this many actual anchored jobs.
pub(crate) const MAX_RESIDENT_SEEK_JOBS: usize = 5;

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
    ready_job: Option<u64>,
    retired_job: Option<u64>,
    completed: [Option<u64>; MAX_RESIDENT_SEEK_JOBS],
    completion_overflow: bool,
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
    worker: Option<JoinHandle<()>>,
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
                ready_job: None,
                retired_job: None,
                completed: [None; MAX_RESIDENT_SEEK_JOBS],
                completion_overflow: false,
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
            worker: Some(worker),
        })
    }

    /// Admit without waiting for preroll. This replaces pending work and
    /// invalidates any ready result. Replaced pending request metadata and its
    /// prepared-revision handle are dropped by the submitter after unlocking;
    /// unread runtime results are reclaimed by the worker outside the lock.
    pub fn submit(&self, spec: ArrangementSeekSpec) -> Result<u64, ArrangementSeekSubmitError> {
        self.submit_work(spec, None)
    }

    /// Same bounded worker and slots as headless seeks, with an actual same-scope capture.
    /// Refusal destroys only the unaccepted capture on the caller; accepted jobs retain their
    /// canonical anchor until a returned owner transfer or genuine post-drop completion.
    pub(crate) fn submit_resident(
        &self,
        spec: ArrangementSeekSpec,
        job: u64,
        resources: PreparedDeviceResources,
        cancelled: Arc<AtomicBool>,
        files: Option<(
            crate::service::ResidentFileSet,
            crate::service::ResidentFileSeekPreparer,
        )>,
    ) -> Result<u64, ArrangementSeekSubmitError> {
        self.submit_work(
            spec,
            Some(ResidentWork {
                job,
                resources,
                cancelled,
                files,
            }),
        )
    }

    fn submit_work(
        &self,
        spec: ArrangementSeekSpec,
        resident: Option<ResidentWork>,
    ) -> Result<u64, ArrangementSeekSubmitError> {
        let mut mailbox = lock(&self.shared.mailbox);
        if mailbox.shutdown || mailbox.completion_overflow {
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
        let old_pending = mailbox.pending.replace(Work {
            request_id,
            spec,
            resident,
        });
        if mailbox.retired.is_none() {
            mailbox.retired = mailbox.ready.take();
            mailbox.retired_job = mailbox.ready_job.take();
        }
        drop(mailbox);
        drop_work(&self.shared, old_pending);
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
            mailbox.retired_job = mailbox.ready_job.take();
        }
        drop(mailbox);
        drop_work(&self.shared, pending);
        self.shared.wake.notify_one();
    }

    /// Consume a completed result without waiting.
    pub fn try_result(&self) -> Option<ArrangementSeekResult> {
        let mut mailbox = lock(&self.shared.mailbox);
        if mailbox.ready_job.is_none()
            && mailbox
                .ready
                .as_ref()
                .is_some_and(|result| mailbox.latest_request_id == Some(result.request_id))
        {
            mailbox.ready.take()
        } else {
            None
        }
    }

    /// Native results never pass through the public UI-owned Playback result endpoint.
    pub(crate) fn try_resident_result(&self) -> Option<(u64, ArrangementSeekResult)> {
        let mut mailbox = lock(&self.shared.mailbox);
        if mailbox
            .ready
            .as_ref()
            .is_none_or(|result| mailbox.latest_request_id != Some(result.request_id))
        {
            return None;
        }
        let job = mailbox.ready_job?;
        let result = mailbox.ready.take()?;
        mailbox.ready_job = None;
        Some((job, result))
    }
    /// Completion is written only after every payload/result owner physically drops. Neither
    /// cancellation nor running=None is evidence of released resource capacity.
    pub(crate) fn try_resident_completion(&self) -> Option<u64> {
        lock(&self.shared.mailbox)
            .completed
            .iter_mut()
            .find_map(Option::take)
    }
    /// Only the canonical worker may join this native-owned worker on teardown. The UI-owned
    /// headless worker keeps its existing nonjoining Drop behavior.
    pub(crate) fn shutdown_and_join_on_control(mut self) {
        self.request_shutdown();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
    fn request_shutdown(&self) {
        let mut mailbox = lock(&self.shared.mailbox);
        mailbox.shutdown = true;
        mailbox.latest_request_id = None;
        if let Some(running) = mailbox.running.as_ref() {
            running.cancelled.store(true, Ordering::Release);
        }
        drop(mailbox);
        self.shared.wake.notify_one();
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
        self.request_shutdown();
    }
}

fn finish_drop(shared: &Shared, job: Option<u64>) {
    if let Some(job) = job {
        let mut mailbox = lock(&shared.mailbox);
        if let Some(slot) = mailbox.completed.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(job);
        } else {
            // Canonical admission bounds live jobs to the fixed completion capacity. A broken
            // caller contract fails closed; only joining teardown can release remaining anchors.
            mailbox.completion_overflow = true;
        }
    }
}
fn drop_work(shared: &Shared, work: Option<Work>) {
    let job = work
        .as_ref()
        .and_then(|work| work.resident.as_ref().map(|r| r.job));
    drop(work);
    finish_drop(shared, job);
}
fn drop_result(shared: &Shared, result: Option<ArrangementSeekResult>, job: Option<u64>) {
    drop(result);
    finish_drop(shared, job);
}

fn prepare(
    spec: ArrangementSeekSpec,
    cancelled: &AtomicBool,
) -> Result<Playback, ArrangementPrerollError> {
    match spec.mode {
        ArrangementSeekMode::SectionLoop(region)
            if !region.enabled()
                || spec.destination_frame != region.start()
                || !spec.start_playing =>
        {
            Err(ArrangementPrerollError::CandidatePositionMismatch)
        }
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
        ArrangementSeekMode::HistoryPreservingInteractive => {
            let runtime = SessionPlayback::prepare_history_preserving_arrangement_preroll(
                Arc::clone(&spec.prepared_revision),
                None,
                spec.destination_frame,
                spec.work_budget_frames,
                spec.start_playing,
                cancelled,
            )?;
            Playback::from_interactive_arrangement_preroll(runtime)
        }
        ArrangementSeekMode::Interactive | ArrangementSeekMode::SectionOnce(_) => {
            let runtime = SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                Arc::clone(&spec.prepared_revision),
                spec.destination_frame,
                spec.work_budget_frames,
                spec.start_playing,
                cancelled,
            )?;
            Playback::from_interactive_arrangement_preroll(runtime)
        }
        ArrangementSeekMode::SectionLoop(region) => Playback::from_interactive_arrangement_preroll(
            SessionPlayback::prepare_section_arrangement_preroll(
                Arc::clone(&spec.prepared_revision),
                None,
                region,
                spec.work_budget_frames,
                cancelled,
            )?,
        ),
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
                    let ready_job = mailbox.ready_job.take();
                    let retired_job = mailbox.retired_job.take();
                    #[cfg(test)]
                    if ready.is_some() || retired.is_some() {
                        mailbox.last_result_drop_thread = Some(thread::current().id());
                    }
                    drop(mailbox);
                    drop_work(&shared, pending);
                    drop_result(&shared, ready, ready_job);
                    drop_result(&shared, retired, retired_job);
                    return;
                }
                #[cfg(test)]
                if mailbox.test_pause_drain {
                    mailbox = wait(&shared.wake, mailbox);
                    continue;
                }
                if let Some(retired) = mailbox.retired.take() {
                    let job = mailbox.retired_job.take();
                    #[cfg(test)]
                    {
                        mailbox.last_result_drop_thread = Some(thread::current().id());
                    }
                    drop(mailbox);
                    drop_result(&shared, Some(retired), job);
                    continue 'worker;
                }
                if mailbox
                    .ready
                    .as_ref()
                    .is_some_and(|ready| mailbox.latest_request_id != Some(ready.request_id))
                {
                    let stale_ready = mailbox.ready.take();
                    let job = mailbox.ready_job.take();
                    #[cfg(test)]
                    {
                        mailbox.last_result_drop_thread = Some(thread::current().id());
                    }
                    drop(mailbox);
                    drop_result(&shared, stale_ready, job);
                    continue 'worker;
                }
                if let Some(work) = mailbox.pending.take() {
                    let cancelled = work.resident.as_ref().map_or_else(
                        || Arc::new(AtomicBool::new(false)),
                        |r| Arc::clone(&r.cancelled),
                    );
                    mailbox.running = Some(Running {
                        request_id: work.request_id,
                        cancelled: Arc::clone(&cancelled),
                    });
                    break (work, cancelled);
                }
                mailbox = wait(&shared.wake, mailbox);
            }
        };

        let (mut work, cancelled) = work;
        let job = work.resident.as_ref().map(|r| r.job);
        let result = if let Some(resident) = work.resident.take() {
            let result = if let Some((sources, prepare)) = &resident.files {
                prepare(crate::service::ResidentFileSeekRequest {
                    spec: &work.spec,
                    resources: &resident.resources,
                    sources,
                    cancelled: &cancelled,
                })
            } else {
                let runtime = match work.spec.mode {
                    ArrangementSeekMode::LinearArrangement => {
                        Err(ArrangementPrerollError::MediaPreparation(
                            "resident seek requires an interactive runtime policy",
                        ))
                    }
                    ArrangementSeekMode::HistoryPreservingInteractive => {
                        SessionPlayback::prepare_history_preserving_arrangement_preroll(
                            Arc::clone(&work.spec.prepared_revision),
                            Some(&resident.resources),
                            work.spec.destination_frame,
                            work.spec.work_budget_frames,
                            work.spec.start_playing,
                            &cancelled,
                        )
                    }
                    ArrangementSeekMode::SectionLoop(region) => {
                        SessionPlayback::prepare_section_arrangement_preroll(
                            Arc::clone(&work.spec.prepared_revision),
                            Some(&resident.resources),
                            region,
                            work.spec.work_budget_frames,
                            &cancelled,
                        )
                    }
                    ArrangementSeekMode::Interactive | ArrangementSeekMode::SectionOnce(_) => {
                        SessionPlayback::prepare_interactive_arrangement_preroll_with_device_resources(
                            Arc::clone(&work.spec.prepared_revision),
                            &resident.resources,
                            work.spec.destination_frame,
                            work.spec.work_budget_frames,
                            work.spec.start_playing,
                            &cancelled,
                        )
                    }
                };
                // Keep one large constructor Result live, matching the existing preroll stack contract.
                runtime.and_then(Playback::from_interactive_arrangement_preroll)
            };
            drop(resident);
            result
        } else {
            preparer(work.spec.clone(), &cancelled)
        };
        let result = result.and_then(|mut playback| {
            if let ArrangementSeekMode::SectionOnce(region) = work.spec.mode {
                if work.spec.destination_frame != region.start() || !work.spec.start_playing {
                    return Err(ArrangementPrerollError::CandidatePositionMismatch);
                }
                playback
                    .arm_section_once(region)
                    .map_err(|_| ArrangementPrerollError::CandidatePositionMismatch)?;
            }
            Ok(playback)
        });
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
        let request_id = work.request_id;
        drop(work);
        let mut mailbox = lock(&shared.mailbox);
        if mailbox
            .running
            .as_ref()
            .is_some_and(|running| running.request_id == request_id)
        {
            mailbox.running = None;
        }
        let accepted = !mailbox.shutdown
            && mailbox.latest_request_id == Some(request_id)
            && !cancelled.load(Ordering::Acquire);
        if accepted {
            let superseded = mailbox.ready.replace(result);
            let superseded_job = std::mem::replace(&mut mailbox.ready_job, job);
            drop(mailbox);
            drop_result(&shared, superseded, superseded_job);
        } else {
            drop(mailbox);
            drop_result(&shared, Some(result), job);
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
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
        loop {
            if lock(&worker.shared.mailbox).ready.is_some() {
                return;
            }
            assert!(Instant::now() < deadline, "worker did not publish a result");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn section_specs_refuse_coercion_and_preserve_exact_result_metadata() {
        let graph = prepared();
        let worker = ArrangementSeekWorker::new().unwrap();
        let enabled = hex_project::LoopRegion::new(0, 256, true).unwrap();
        for (region, destination_frame, start_playing) in [
            (enabled, 1, true),
            (enabled, 0, false),
            (
                hex_project::LoopRegion::new(0, 256, false).unwrap(),
                0,
                true,
            ),
        ] {
            let request = worker
                .submit(ArrangementSeekSpec {
                    mode: ArrangementSeekMode::SectionLoop(region),
                    destination_frame,
                    work_budget_frames: destination_frame,
                    start_playing,
                    ..spec(Arc::clone(&graph), 1)
                })
                .unwrap();
            wait_for_ready(&worker);
            let result = worker.try_result().unwrap();
            assert_eq!(result.request_id, request);
            assert_eq!(result.mode, ArrangementSeekMode::SectionLoop(region));
            assert_eq!(
                (result.destination_frame, result.start_playing),
                (destination_frame, start_playing)
            );
            assert_eq!(
                result.result.err(),
                Some(ArrangementPrerollError::CandidatePositionMismatch)
            );
        }
        worker
            .submit(ArrangementSeekSpec {
                mode: ArrangementSeekMode::SectionLoop(enabled),
                start_playing: true,
                ..spec(Arc::clone(&graph), 2)
            })
            .unwrap();
        wait_for_ready(&worker);
        let playback = worker.try_result().unwrap().result.unwrap();
        assert_eq!(playback.section_loop(), Some(enabled));
        assert_eq!(playback.position(), 0);
        assert!(playback.playing());
        worker.shutdown_and_join_on_control();
    }

    fn wait_for_idle(worker: &ArrangementSeekWorker) {
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
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
    fn resident_result_stays_private_and_completion_follows_actual_retirement() {
        let project = hex_project::Project::new(48_000).unwrap();
        let cancel = AtomicBool::new(false);
        let resources = PreparedDeviceResources::prepare_on_control(
            &hex_engine::native_device_resources::ResidentDeviceAssets::default(),
            &project,
            10_000,
            &cancel,
        )
        .unwrap();
        let graph = Arc::new(
            PreparedRevision::prepare_with_device_resources(
                &project,
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                0,
                &cancel,
            )
            .unwrap(),
        );
        let worker = ArrangementSeekWorker::new().unwrap();
        worker
            .submit_resident(
                spec(Arc::clone(&graph), 1),
                71,
                resources.capture_on_control(10_000).unwrap(),
                Arc::new(AtomicBool::new(false)),
                None,
            )
            .unwrap();
        wait_for_ready(&worker);
        assert!(
            worker.try_result().is_none(),
            "public headless getter cannot take a native graph"
        );
        assert!(
            worker.try_resident_completion().is_none(),
            "ready/idle is not source retirement"
        );
        {
            let mut mailbox = lock(&worker.shared.mailbox);
            mailbox.test_pause_drain = true;
        }
        worker.invalidate();
        assert!(
            worker.try_resident_completion().is_none(),
            "cancellation alone releases nothing"
        );
        assert!(
            Arc::strong_count(&graph) > 1,
            "retired runtime still owns its exact revision"
        );
        {
            let mut mailbox = lock(&worker.shared.mailbox);
            mailbox.test_pause_drain = false;
        }
        worker.shared.wake.notify_one();
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
        loop {
            if let Some(id) = worker.try_resident_completion() {
                assert_eq!(id, 71);
                break;
            }
            assert!(Instant::now() < deadline, "post-drop completion missing");
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            Arc::strong_count(&graph),
            1,
            "completion follows actual runtime/result/revision destruction"
        );
        worker.shutdown_and_join_on_control();
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
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
        while retired_weak.upgrade().is_some() || held_weak.upgrade().is_some() {
            assert!(
                Instant::now() < deadline,
                "worker did not drain teardown results"
            );
            crate::test_wait::pause();
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
        let deadline = Instant::now() + crate::test_wait::HANG_BOUND;
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
    fn interactive_and_section_seeks_exceed_the_linear_replay_budget() {
        let graph = prepared();
        let worker = ArrangementSeekWorker::new().expect("seek worker");
        let destination = MAX_SEEK_WORK_FRAMES * 4 + 37;
        let region = hex_project::LoopRegion::new(destination, destination + 4096, true).unwrap();
        for (mode, expected) in [
            (ArrangementSeekMode::Interactive, Ok(destination)),
            (ArrangementSeekMode::SectionLoop(region), Ok(destination)),
            (
                ArrangementSeekMode::HistoryPreservingInteractive,
                Err(ArrangementPrerollError::WorkBudgetExceeded),
            ),
            (
                ArrangementSeekMode::LinearArrangement,
                Err(ArrangementPrerollError::WorkBudgetExceeded),
            ),
        ] {
            // Resident seeks submit the full replay budget, never the destination.
            worker
                .submit(ArrangementSeekSpec {
                    mode,
                    destination_frame: destination,
                    work_budget_frames: MAX_SEEK_WORK_FRAMES,
                    start_playing: true,
                    ..spec(Arc::clone(&graph), 3)
                })
                .expect("bounded request");
            wait_for_ready(&worker);
            let result = worker.try_result().expect("seek result");
            assert_eq!(result.mode, mode);
            assert_eq!(
                result.result.map(|playback| playback.position()),
                expected,
                "{mode:?}"
            );
        }
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
                crate::test_wait::pause();
            }
            Err(ArrangementPrerollError::Cancelled)
        })
        .expect("seek worker");
        worker.submit(spec(graph, 1)).expect("request");
        let worker_thread = worker.worker.as_ref().unwrap().thread().id();
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
    #[test]
    fn history_mode_worker_reconstructs_mutable_pcm_and_preserves_request_identity() {
        thread::Builder::new()
            .stack_size(SEEK_WORKER_STACK_BYTES)
            .spawn(|| {
                use hex_engine::session_playback::SessionPlaybackCommand;
                use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
                use hex_project::{BeatTime, Command, Created};
                let mut session = Session::new(48_000, 8, 0).unwrap();
                let state = hex_engine::default_native_device_state_for_class(
                    hex_engine::NativeDeviceClass::Instrument,
                    DeviceTypeId::new("hex.instrument.operator").unwrap(),
                )
                .unwrap();
                let state = hex_engine::patch_native_device_parameter_for_class(
                    hex_engine::NativeDeviceClass::Instrument,
                    &state,
                    ParameterId::new(78).unwrap(),
                    ParameterValue::Float(0.3),
                )
                .unwrap();
                let Created::Track(track) = session
                    .command(Command::InsertTrackWithDevice {
                        name: "history fixture".into(),
                        before_track: None,
                        slot: hex_project::TrackDeviceSlot::Instrument,
                        state,
                    })
                    .unwrap()
                else {
                    panic!("track")
                };
                let Created::Clip(clip) = session
                    .command(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::ZERO,
                        length: BeatTime::from_quarters(4).unwrap(),
                    })
                    .unwrap()
                else {
                    panic!("clip")
                };
                session
                    .command(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch: 60,
                        channel: 0,
                        velocity: 100,
                    })
                    .unwrap();
                let graph = session.compile_prepared_revision(None).unwrap();
                assert!(graph.arrangement_seek_requires_preroll());
                let worker = ArrangementSeekWorker::new().unwrap();
                for (destination, playing) in [(512, true), (24_004, false)] {
                    let mut oracle = SessionPlayback::new(Arc::clone(&graph)).unwrap();
                    oracle.apply(SessionPlaybackCommand::Play).unwrap();
                    let mut prefix = vec![[0.0; 2]; destination as usize];
                    for chunk in prefix.chunks_mut(127) {
                        oracle.render(chunk).unwrap();
                    }
                    let mut expected = [[0.0; 2]; 193];
                    oracle.render(&mut expected).unwrap();
                    assert!(
                        expected
                            .iter()
                            .flatten()
                            .any(|sample| sample.abs() > 1.0e-6)
                    );
                    let requested = ArrangementSeekSpec {
                        mode: ArrangementSeekMode::HistoryPreservingInteractive,
                        destination_frame: destination,
                        work_budget_frames: destination,
                        start_playing: playing,
                        ..spec(Arc::clone(&graph), 7)
                    };
                    let request = worker.submit(requested.clone()).unwrap();
                    wait_for_ready(&worker);
                    let result = worker.try_result().unwrap();
                    assert_eq!(
                        (
                            result.request_id,
                            result.mode,
                            result.generation,
                            result.revision
                        ),
                        (request, requested.mode, 7, 17)
                    );
                    assert!(Arc::ptr_eq(&result.prepared_revision, &graph));
                    assert_eq!(
                        (
                            result.destination_frame,
                            result.work_budget_frames,
                            result.start_playing
                        ),
                        (destination, destination, playing)
                    );
                    let mut candidate = result.result.unwrap();
                    assert_eq!(candidate.playing(), playing);
                    assert_eq!(candidate.position(), destination);
                    if !playing {
                        candidate.apply(hex_engine::playback::PlaybackCommand::Play);
                    }
                    let mut actual = [[0.0; 2]; 193];
                    for chunk in actual.chunks_mut(31) {
                        candidate.render(chunk).unwrap();
                    }
                    assert_eq!(actual, expected, "destination={destination}");
                }
                worker.shutdown_and_join_on_control();
            })
            .unwrap()
            .join()
            .unwrap();
    }
    #[test]
    fn resident_history_mode_honors_budget_and_forwards_file_policy_without_coercion() {
        use crate::service::{ResidentFileInput, ResidentFileMetadata, ResidentFileSet};
        static FILE_PREPARER_CALLS: AtomicUsize = AtomicUsize::new(0);
        fn unexpected_file_preparer(
            request: crate::service::ResidentFileSeekRequest<'_>,
        ) -> Result<Playback, ArrangementPrerollError> {
            assert_eq!(
                request.spec().mode,
                ArrangementSeekMode::HistoryPreservingInteractive
            );
            FILE_PREPARER_CALLS.fetch_add(1, Ordering::Relaxed);
            Err(ArrangementPrerollError::MediaPreparation(
                "unexpected file preparer",
            ))
        }
        let project = hex_project::Project::new(48_000).unwrap();
        let cancel = AtomicBool::new(false);
        let resources = PreparedDeviceResources::prepare_on_control(
            &hex_engine::native_device_resources::ResidentDeviceAssets::default(),
            &project,
            10_000,
            &cancel,
        )
        .unwrap();
        let graph = Arc::new(
            PreparedRevision::prepare_with_device_resources(
                &project,
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                0,
                &cancel,
            )
            .unwrap(),
        );
        let files = ResidentFileSet::capture(
            ResidentFileInput {
                context: hex_engine::AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 11,
                },
                semantic_source_epoch: 1,
                files: &[],
                pitches: &[],
            },
            ResidentFileMetadata::new(1, std::collections::BTreeMap::new()).unwrap(),
        )
        .unwrap();
        let worker = ArrangementSeekWorker::new().unwrap();
        let history = ArrangementSeekMode::HistoryPreservingInteractive;
        for (mode, budget, with_files, expected) in [
            (history, 17, false, Ok(17)),
            (
                history,
                0,
                false,
                Err(ArrangementPrerollError::WorkBudgetExceeded),
            ),
            (
                history,
                17,
                true,
                Err(ArrangementPrerollError::MediaPreparation(
                    "unexpected file preparer",
                )),
            ),
            (
                ArrangementSeekMode::LinearArrangement,
                17,
                false,
                Err(ArrangementPrerollError::MediaPreparation(
                    "resident seek requires an interactive runtime policy",
                )),
            ),
        ] {
            let request = worker
                .submit_resident(
                    ArrangementSeekSpec {
                        mode,
                        destination_frame: 17,
                        work_budget_frames: budget,
                        start_playing: true,
                        ..spec(Arc::clone(&graph), 1)
                    },
                    71,
                    resources.capture_on_control(10_000).unwrap(),
                    Arc::new(AtomicBool::new(false)),
                    with_files.then(|| {
                        (
                            files.clone(),
                            unexpected_file_preparer as crate::service::ResidentFileSeekPreparer,
                        )
                    }),
                )
                .unwrap();
            wait_for_ready(&worker);
            assert!(worker.try_result().is_none(), "native result stays private");
            let (job, result) = worker.try_resident_result().unwrap();
            assert_eq!((job, result.request_id, result.mode), (71, request, mode));
            assert!(Arc::ptr_eq(&result.prepared_revision, &graph));
            assert_eq!(result.result.map(|playback| playback.position()), expected);
        }
        assert_eq!(FILE_PREPARER_CALLS.load(Ordering::Relaxed), 1);
        worker.shutdown_and_join_on_control();
    }
}
