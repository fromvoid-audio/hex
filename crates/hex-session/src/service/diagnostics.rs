//! Read-only control-plane diagnostic observations, not admission, cancellation,
//! project state or audio acknowledgements. Optional observers receive bounded
//! safe projections and scalar stamps through the existing request/event FIFOs;
//! no extra queue, worker, logger dependency or audio callback is introduced.
//!
//! Coverage: exhaustive Request/Command kinds, scalar IDs/values and bounded
//! selection metadata; text/path/blob contents are excluded and unsupported
//! fields are explicitly named as omitted. Worker outcome preparation, queueing
//! and client receipt are separate observations. Asynchronous continuation/job
//! terminals retain their existing typed IDs; an internal wakeup is not a user
//! action. Direct transport and view/input intent are instrumented by UI owners.
mod summary;

use super::{Event, Request, SubmitError};
use std::{
    cell::Cell,
    fmt::{self, Write},
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const MAX_DETAILS_BYTES: usize = 1024;
thread_local! {
    static ACTIVE_UI: Cell<(usize, u64)> = const { Cell::new((0, 0)) };
}
static NEXT_SERVICE: AtomicU64 = AtomicU64::new(0);

/// Diagnostic identity only; never accepted as a freshness/publication token.
/// Fixed Copy metadata travels in the existing bounded control FIFOs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticOrigin {
    Submitted,
    Continuation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagnosticRequest {
    pub request_id: u64,
    pub origin: DiagnosticOrigin,
    pub ui_action_id: Option<u64>,
    pub kind: &'static str,
    pub operation: Option<&'static str>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticStage {
    SubmitAttempt,
    LateSubmitObserved,
    InternalWakeup,
    SubmitReturned {
        result: &'static str,
    },
    WorkerBegin {
        generation: u64,
        revision: u64,
    },
    OutcomeReady {
        generation: Option<u64>,
        revision: u64,
        error: bool,
        request_complete: bool,
    },
    OutcomeBackpressure,
    OutcomeQueued,
    ConsumerClosed,
    ShutdownAbandoned,
    ClientReceived {
        generation: Option<u64>,
        revision: u64,
        error: bool,
    },
    IdentityExhausted,
}
/// Borrow during a control callback. Details are capped at 1024 UTF-8 bytes;
/// truncation is explicit. No project, source, reader or native owner escapes.
pub struct DiagnosticEvent {
    pub service_id: u64,
    pub request: Option<DiagnosticRequest>,
    pub stage: DiagnosticStage,
    pub details: String,
    pub details_truncated: bool,
}
/// Caller must keep observation nonwaiting and independent of application
/// behavior. The supplied desktop observer uses the bounded session logger.
pub struct DiagnosticObserver {
    callback: Box<dyn Fn(&DiagnosticEvent) + Send + Sync>,
}
impl DiagnosticObserver {
    pub fn new(callback: impl Fn(&DiagnosticEvent) + Send + Sync + 'static) -> Self {
        Self {
            callback: Box::new(callback),
        }
    }
    /// Parent on this thread only; deferred/control work reports None.
    pub fn current_ui_action(self: &Arc<Self>) -> Option<u64> {
        ACTIVE_UI.with(|active| {
            let (owner, id) = active.get();
            (owner == Arc::as_ptr(self) as usize && id != 0).then_some(id)
        })
    }
    /// Scope only synchronous UI handling. Deferred work must not inherit a
    /// stale action ID. This guard is deliberately not Send/Sync.
    pub fn enter_ui_action(self: &Arc<Self>, action_id: u64) -> DiagnosticActionScope {
        let previous =
            ACTIVE_UI.with(|active| active.replace((Arc::as_ptr(self) as usize, action_id)));
        DiagnosticActionScope {
            _observer: Arc::clone(self),
            previous,
            _ui: PhantomData,
        }
    }
}
pub struct DiagnosticActionScope {
    _observer: Arc<DiagnosticObserver>,
    previous: (usize, u64),
    _ui: PhantomData<Rc<()>>,
}
impl Drop for DiagnosticActionScope {
    fn drop(&mut self) {
        ACTIVE_UI.with(|active| active.set(self.previous));
    }
}

/// Control-worker phases of one structural section edit or its history step,
/// timed with `Instant` on the worker only (never inside the audio callback).
/// `fence` is the callback-observed `stopped_arrangement` lease, `edit` the
/// canonical apply/commit around preparation, `prepare` the resident
/// `prepare_transition`, `publish` the handoff to the native sink and `finish`
/// the sink's edit terminal, and `view` the post-transition view-state
/// construction. These lie within the request's dispatched worker
/// time (`worker_us`). `parked` is the time the request waited parked for its
/// stopped-arrangement fence before dispatch (#4544), so it lies outside
/// `worker_us`. Reported on `OutcomeReady` as `section_*_us`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SectionPhases {
    pub parked: Duration,
    pub fence: Duration,
    pub edit: Duration,
    pub prepare: Duration,
    pub publish: Duration,
    pub finish: Duration,
    pub view: Duration,
}

pub(super) struct Hooks {
    service_id: u64,
    next_request: AtomicU64,
    observer: OnceLock<Arc<DiagnosticObserver>>,
    // Request-wait loop iterations of this service's worker (#4574). A scalar
    // count only; it never gates admission, publication or shutdown.
    worker_wakes: AtomicU64,
    // Per-service test ceiling for the older-reader save preflight (0: the
    // real hex-storage limit). Shared with this service's worker only.
    #[cfg(test)]
    legacy_reader_limit: AtomicU64,
}
impl Default for Hooks {
    fn default() -> Self {
        let service_id = NEXT_SERVICE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .ok()
            .and_then(|n| n.checked_add(1))
            .unwrap_or(0);
        Self {
            service_id,
            next_request: AtomicU64::new(0),
            observer: OnceLock::new(),
            worker_wakes: AtomicU64::new(0),
            #[cfg(test)]
            legacy_reader_limit: AtomicU64::new(0),
        }
    }
}
impl Hooks {
    /// Count one worker request-wait iteration; saturates instead of wrapping.
    pub fn record_worker_wake(&self) {
        let _ = self
            .worker_wakes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1));
    }
    pub fn worker_wakes(&self) -> u64 {
        self.worker_wakes.load(Ordering::Relaxed)
    }
    /// Older-reader byte ceiling for save consent: the real limit in production.
    pub fn legacy_reader_limit(&self) -> usize {
        #[cfg(test)]
        if let Ok(limit @ 1..) = usize::try_from(self.legacy_reader_limit.load(Ordering::Relaxed)) {
            return limit;
        }
        hex_storage::LEGACY_READER_DOCUMENT_BYTES
    }
    #[cfg(test)]
    pub fn set_legacy_reader_limit(&self, bytes: u64) {
        self.legacy_reader_limit.store(bytes, Ordering::Relaxed);
    }
    pub fn install(&self, observer: Arc<DiagnosticObserver>) -> Result<(), &'static str> {
        self.observer
            .set(observer)
            .map_err(|_| "session diagnostic observer is already installed")
    }
    pub fn begin(&self, request: &Request) -> Option<DiagnosticRequest> {
        self.begin_observation(request, DiagnosticOrigin::Submitted, true)
    }
    pub fn begin_at_worker(
        &self,
        request: &Request,
        origin: DiagnosticOrigin,
    ) -> Option<DiagnosticRequest> {
        self.begin_observation(request, origin, false)
    }
    fn begin_observation(
        &self,
        request: &Request,
        origin: DiagnosticOrigin,
        submitted: bool,
    ) -> Option<DiagnosticRequest> {
        let observer = self.observer.get()?;
        let request_id = self
            .next_request
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .ok()
            .and_then(|n| n.checked_add(1));
        let Some(request_id) = request_id.filter(|_| self.service_id != 0) else {
            self.emit(None, DiagnosticStage::IdentityExhausted, None);
            return None;
        };
        let description = summary::request(request);
        let parent = if submitted {
            observer.current_ui_action()
        } else {
            None
        };
        let stamp = DiagnosticRequest {
            request_id,
            origin,
            ui_action_id: parent,
            kind: description.name,
            operation: description.operation,
        };
        self.emit(
            Some(stamp),
            if submitted {
                DiagnosticStage::SubmitAttempt
            } else if origin == DiagnosticOrigin::Continuation {
                DiagnosticStage::InternalWakeup
            } else {
                DiagnosticStage::LateSubmitObserved
            },
            Some(description),
        );
        Some(stamp)
    }
    pub fn returned(&self, request: Option<DiagnosticRequest>, result: &Result<(), SubmitError>) {
        if request.is_none() {
            return;
        }
        let result = match result {
            Ok(()) => "enqueued",
            Err(SubmitError::Full(_)) => "full",
            Err(SubmitError::Busy(..)) => "busy",
            Err(SubmitError::Closed(_)) => "closed",
        };
        self.emit(request, DiagnosticStage::SubmitReturned { result }, None);
    }
    pub fn emit(
        &self,
        request: Option<DiagnosticRequest>,
        stage: DiagnosticStage,
        description: Option<Description>,
    ) {
        let Some(observer) = self.observer.get() else {
            return;
        };
        let (details, details_truncated) = description.map_or((String::new(), false), |d| {
            (d.details.text, d.details.truncated)
        });
        (observer.callback)(&DiagnosticEvent {
            service_id: self.service_id,
            request,
            stage,
            details,
            details_truncated,
        });
    }
    /// `worker` (WorkerBegin to OutcomeReady) and `section` phases are worker
    /// observations; client receipt reports neither.
    pub fn outcome(
        &self,
        request: Option<DiagnosticRequest>,
        event: &Event,
        received: bool,
        worker: Option<Duration>,
        section: Option<SectionPhases>,
    ) {
        if self.observer.get().is_none() {
            return;
        }
        let generation = event
            .state
            .canonical_audio_source
            .as_ref()
            .map(|source| source.context().project_generation);
        let revision = event.state.revision;
        let stage = if received {
            DiagnosticStage::ClientReceived {
                generation,
                revision,
                error: event.error.is_some(),
            }
        } else {
            DiagnosticStage::OutcomeReady {
                generation,
                revision,
                error: event.error.is_some(),
                request_complete: event.request_complete,
            }
        };
        let mut description = Description::new("outcome");
        if let Some(worker) = worker {
            description.field("worker_us", format_args!("{}", worker.as_micros()));
        }
        if let Some(phases) = section {
            for (name, value) in [
                ("section_parked_us", phases.parked),
                ("section_fence_us", phases.fence),
                ("section_edit_us", phases.edit),
                ("section_prepare_us", phases.prepare),
                ("section_publish_us", phases.publish),
                ("section_finish_us", phases.finish),
                ("section_view_us", phases.view),
            ] {
                description.field(name, format_args!("{}", value.as_micros()));
            }
        }
        if let Some(ack) = &event.gesture_ack {
            description.field("gesture_generation", format_args!("{}", ack.key.generation));
            description.field("gesture_token", format_args!("{}", ack.key.token));
            description.field("gesture_sequence", format_args!("{}", ack.sequence));
            description.field("gesture_phase", format_args!("{:?}", ack.phase));
            description.field("gesture_accepted", format_args!("{}", ack.accepted));
            description.field("preview_retained", format_args!("{}", ack.preview_retained));
            description.field("reason_present", format_args!("{}", ack.reason.is_some()));
        }
        if let Some(created) = &event.created {
            description.nested("created", summary::created(created));
        }
        description.field(
            "project_replaced",
            format_args!("{}", event.project_replaced),
        );
        description.field("save_complete", format_args!("{}", event.save_complete));
        description.field("open_complete", format_args!("{}", event.open_complete));
        description.field(
            "sampler_edit_complete",
            format_args!("{:?}", event.sampler_edit_complete),
        );
        description.field(
            "computer_keyboard_complete",
            format_args!("{:?}", event.computer_keyboard_complete),
        );
        if let Some(source) = &event.state.canonical_audio_source {
            description.field(
                "audio_source_revision",
                format_args!("{}", source.context().editor_revision),
            );
        }
        if let Some(preview) = &event.device_preview {
            description.field("preview", format_args!("{:?}", preview.identity));
        }
        self.emit(request, stage, Some(description));
    }
}

/// One receiver for the existing event channel; unwrap metadata only after the
/// exact Event is received. Public Event and all of its ownership stay intact.
pub(super) struct EventReceiver {
    receiver: std::sync::mpsc::Receiver<ObservedEvent>,
    diagnostics: Arc<Hooks>,
}
impl EventReceiver {
    pub fn new(
        receiver: std::sync::mpsc::Receiver<ObservedEvent>,
        diagnostics: Arc<Hooks>,
    ) -> Self {
        Self {
            receiver,
            diagnostics,
        }
    }
    fn received(&self, observed: ObservedEvent) -> Event {
        self.diagnostics
            .outcome(observed.request, &observed.event, true, None, None);
        observed.event
    }
    pub fn try_recv(&self) -> Result<Event, std::sync::mpsc::TryRecvError> {
        self.receiver
            .try_recv()
            .map(|observed| self.received(observed))
    }
    #[cfg(test)]
    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Event, std::sync::mpsc::RecvTimeoutError> {
        self.receiver
            .recv_timeout(timeout)
            .map(|observed| self.received(observed))
    }
}

pub(super) struct ObservedEvent {
    pub event: Event,
    pub request: Option<DiagnosticRequest>,
}

struct Details {
    text: String,
    truncated: bool,
}
impl Details {
    fn new() -> Self {
        Self {
            text: String::new(),
            truncated: false,
        }
    }
}
impl fmt::Write for Details {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let available = MAX_DETAILS_BYTES.saturating_sub(self.text.len());
        let mut end = value.len().min(available);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&value[..end]);
        if end < value.len() {
            self.truncated = true;
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}
pub(super) struct Description {
    name: &'static str,
    operation: Option<&'static str>,
    details: Details,
}
impl Description {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            operation: None,
            details: Details::new(),
        }
    }
    fn field(&mut self, name: &'static str, value: fmt::Arguments<'_>) {
        if self.details.truncated {
            return;
        }
        let _ = write!(self.details, " {name}={value}");
    }
    fn omitted(&mut self, fields: &'static str) {
        if !fields.is_empty() {
            self.field("omitted_fields", format_args!("{fields:?}"));
        }
    }
    fn nested(&mut self, label: &'static str, child: Description) {
        self.operation = Some(child.name);
        self.field(label, format_args!("{}", child.name));
        let _ = self.details.write_str(&child.details.text);
        self.details.truncated |= child.details.truncated;
    }
}
struct FirstIds<'a>(&'a [hex_project::TrackId]);
impl fmt::Debug for FirstIds<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().take(8).map(|id| id.value()))
            .finish()
    }
}
struct Parameter<'a>(&'a hex_lattice_contract::ParameterValue);
impl fmt::Debug for Parameter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use hex_lattice_contract::ParameterValue::*;
        match self.0 {
            Float(value) => write!(f, "float({value:?})"),
            Bool(value) => write!(f, "bool({value})"),
            Choice(value) => write!(f, "choice({value})"),
        }
    }
}

struct Snapshot<'a>(&'a hex_lattice_contract::DeviceParameterSnapshot);
impl fmt::Debug for Snapshot<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(
                self.0
                    .entries()
                    .iter()
                    .map(|(id, value)| (id.value(), Parameter(value))),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::Service;
    use hex_project::Command;
    use std::sync::Mutex;

    #[test]
    fn action_projections_redact_unicode_names_and_paths_for_rename_import_and_save() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "fixture".into(),
            })
            .unwrap()
        else {
            panic!("track fixture");
        };
        let name = "私的な曲名🎵";
        let path = std::path::PathBuf::from("/private/秘密の録音🎵.wav");
        let requests = [
            Request::Edit(Command::RenameTrack {
                track,
                name: name.into(),
            }),
            Request::ImportToTrack {
                path: path.clone(),
                track,
                start: 123,
            },
            Request::SaveNew(path.clone()),
            Request::Edit(Command::SetTrackTags {
                track,
                tags: vec![hex_project::TrackTag::new(name).unwrap()],
            }),
        ];
        let hooks = Hooks::default();
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&records);
        hooks
            .install(Arc::new(DiagnosticObserver::new(move |event| {
                sink.lock().unwrap().push((
                    event.request,
                    event.details.clone(),
                    event.details_truncated,
                ));
            })))
            .unwrap();
        for request in &requests {
            hooks.begin(request).expect("request identity");
        }
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 4);
        for (index, (stamp, text, truncated)) in records.iter().enumerate() {
            assert!(!truncated);
            assert!(!text.contains(name));
            assert!(!text.contains("秘密"));
            assert!(!text.contains("/private/"));
            assert_eq!(stamp.unwrap().request_id, index as u64 + 1);
        }
        assert!(records[3].1.contains("set_track_tags"));
        assert!(records[3].1.contains("tags_count=1"));
        assert!(records[0].1.contains(&format!("track={}", track.value())));
        assert!(
            records[0]
                .1
                .contains(&format!("name_redacted_bytes={}", name.len()))
        );
        assert!(records[1].1.contains(&format!("track={}", track.value())));
        assert!(records[1].1.contains("start=123"));
        assert!(
            records[1]
                .1
                .contains(&format!("path_redacted_bytes={}", path.as_os_str().len()))
        );
        assert!(
            records[2]
                .1
                .contains(&format!("value0_redacted_bytes={}", path.as_os_str().len()))
        );
    }

    #[test]
    fn existing_asset_import_projection_names_its_asset_and_redacts_names() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
        let hex_project::Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "fixture.wav".into(),
                sample_rate: 48_000,
                frames: 16,
            })
            .unwrap()
        else {
            panic!("asset fixture");
        };
        let name = "私的な曲名🎵";
        let description = summary::request(&Request::Edit(Command::ImportExistingAudio {
            asset,
            placement: hex_project::ExistingAudioPlacement::NewTrack {
                name: name.into(),
                before_track: None,
                start: 7,
            },
        }));
        assert_eq!(
            description.operation,
            Some("import_existing_audio_track_at")
        );
        let text = &description.details.text;
        assert!(!text.contains(name));
        assert!(text.contains(&format!("asset={}", asset.value())));
        assert!(text.contains(&format!("name_redacted_bytes={}", name.len())));
        assert!(text.contains("start=7"));
    }

    #[test]
    fn real_service_preserves_outcomes_and_correlates_refused_edit_without_private_text() {
        let service = Service::new(48_000, 8, 0).unwrap();
        // Initial state predates observer installation and has no fabricated request.
        let initial = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&records);
        let observer = Arc::new(DiagnosticObserver::new(move |event| {
            sink.lock()
                .unwrap()
                .push((event.request, event.stage, event.details.clone()));
        }));
        service
            .set_diagnostic_observer(Arc::clone(&observer))
            .unwrap();
        {
            let _scope = observer.enter_ui_action(42);
            // An oversized private name produces a real refusal, not a mocked terminal.
            service
                .submit(Request::Edit(Command::AddTrack {
                    name: "private name /secret/path".repeat(100),
                }))
                .unwrap();
        }
        let event = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(event.request_complete);
        assert!(event.error.is_some());
        assert_eq!(event.state.revision, initial.state.revision);
        let stamp = records
            .lock()
            .unwrap()
            .iter()
            .find_map(|(stamp, stage, _)| {
                matches!(stage, DiagnosticStage::SubmitAttempt)
                    .then_some(*stamp)
                    .flatten()
            })
            .unwrap();
        // Reception may beat the observer notification after successful queue
        // publication. Wait without holding its mutex, so the worker can emit it.
        let deadline = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
        loop {
            if records.lock().unwrap().iter().any(|(id, stage, _)| {
                *id == Some(stamp) && *stage == DiagnosticStage::OutcomeQueued
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "queued outcome diagnostic missing"
            );
            std::thread::yield_now();
        }
        let rows = records.lock().unwrap();
        assert_eq!(stamp.ui_action_id, Some(42));
        assert_eq!(stamp.operation, Some("add_track"));
        assert!(
            rows.iter().any(
                |(id, stage, _)| *id == Some(stamp) && *stage == DiagnosticStage::OutcomeQueued
            )
        );
        assert!(rows.iter().any(
            |(id, s, _)| *id == Some(stamp) && matches!(s, DiagnosticStage::WorkerBegin { .. })
        ));
        assert!(rows.iter().any(|(id, s, _)| *id == Some(stamp)
            && matches!(s, DiagnosticStage::ClientReceived { error: true, .. })));
        let text = rows.iter().map(|(_, _, s)| s.as_str()).collect::<String>();
        assert!(text.contains("name_redacted_bytes"));
        assert!(!text.contains("private name"));
        assert!(!text.contains("/secret/path"));
    }

    #[test]
    fn off_is_empty_and_ui_parent_is_thread_local_scoped_and_not_inherited_by_wakeups() {
        let hooks = Hooks::default();
        assert!(hooks.begin(&Request::Undo).is_none());
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&records);
        let observer = Arc::new(DiagnosticObserver::new(move |event| {
            sink.lock().unwrap().push((event.request, event.stage));
        }));
        hooks.install(Arc::clone(&observer)).unwrap();
        let _outer = observer.enter_ui_action(9);
        assert_eq!(hooks.begin(&Request::Undo).unwrap().ui_action_id, Some(9));
        std::thread::scope(|scope| {
            scope.spawn(|| assert_eq!(hooks.begin(&Request::Undo).unwrap().ui_action_id, None));
        });
        {
            let _inner = observer.enter_ui_action(10);
            assert_eq!(hooks.begin(&Request::Undo).unwrap().ui_action_id, Some(10));
        }
        assert_eq!(hooks.begin(&Request::Undo).unwrap().ui_action_id, Some(9));
        let wakeup = hooks
            .begin_at_worker(&Request::Undo, DiagnosticOrigin::Continuation)
            .unwrap();
        assert_eq!(wakeup.ui_action_id, None);
        hooks.returned(Some(wakeup), &Err(SubmitError::Full(Request::Undo)));
        let rows = records.lock().unwrap();
        assert!(
            rows.iter()
                .any(|(_, stage)| *stage == DiagnosticStage::InternalWakeup)
        );
        assert!(
            rows.iter()
                .any(|(_, stage)| *stage == DiagnosticStage::SubmitReturned { result: "full" })
        );
    }

    #[test]
    fn event_pressure_retains_each_canonical_outcome_and_its_diagnostic_identity() {
        let service = Service::new(48_000, 8, 0).unwrap();
        let stages = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stages);
        service
            .set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
                sink.lock().unwrap().push((event.request, event.stage));
            })))
            .unwrap();
        let deadline = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
        let mut admitted = 0;
        while admitted < 8 {
            assert!(
                std::time::Instant::now() < deadline,
                "request queue did not progress"
            );
            match service.submit(Request::Edit(Command::AddTrack {
                name: "fixture".into(),
            })) {
                Ok(()) => admitted += 1,
                Err(SubmitError::Full(_)) | Err(SubmitError::Busy(..)) => crate::test_wait::pause(),
                Err(SubmitError::Closed(_)) => panic!("service closed"),
            }
        }
        while !stages
            .lock()
            .unwrap()
            .iter()
            .any(|(_, stage)| *stage == DiagnosticStage::OutcomeBackpressure)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "event backpressure was not observed"
            );
            crate::test_wait::pause();
        }
        for revision in 0..=8 {
            let event = service
                .events
                .recv_timeout(crate::test_wait::HANG_BOUND)
                .unwrap();
            assert_eq!(event.state.revision, revision);
            assert_eq!(event.state.project.tracks().len(), revision as usize);
            assert!(event.error.is_none());
        }
        let rows = stages.lock().unwrap();
        let received: Vec<_> = rows
            .iter()
            .filter_map(|(stamp, stage)| {
                matches!(stage, DiagnosticStage::ClientReceived { .. })
                    .then_some(*stamp)
                    .flatten()
                    .map(|s| s.request_id)
            })
            .collect();
        assert_eq!(received.len(), 8);
        assert!(received.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn bounded_utf8_details_and_identity_exhaustion_are_explicit() {
        let mut details = Details::new();
        let _ = write!(details, "{}", "é".repeat(2048));
        assert_eq!(details.text.len(), MAX_DETAILS_BYTES);
        assert!(details.truncated);
        let hooks = Hooks::default();
        let stages = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stages);
        hooks
            .install(Arc::new(DiagnosticObserver::new(move |e| {
                sink.lock().unwrap().push(e.stage)
            })))
            .unwrap();
        hooks.next_request.store(u64::MAX, Ordering::Relaxed);
        assert!(hooks.begin(&Request::Undo).is_none());
        assert_eq!(
            *stages.lock().unwrap(),
            vec![DiagnosticStage::IdentityExhausted]
        );
    }
}
