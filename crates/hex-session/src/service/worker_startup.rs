//! One startup aggregate, so spawn failure cannot drop completion before its
//! actual Session/Event children. No new queue, flag or readiness state.
use super::*;
use crate::snapshot_exports::{SnapshotWorkerControl, SnapshotWorkerStop};

/// Actual pre-Session worker operands, borrowed only during negotiation.
/// The registered family must be inspected separately under its real guards.
/// Neither this view nor its phase query grants storage/admission authority.
/// ```compile_fail
/// fn save<'a>(view: hex_session::service::WorkerStartupView<'a>)
///     -> hex_session::service::WorkerStartupView<'static> { view }
/// ```
/// ```compile_fail
/// fn send(view: hex_session::service::WorkerStartupView<'static>) {
///     std::thread::spawn(move || drop(view));
/// }
/// ```
pub struct WorkerStartupView<'a> {
    pub(super) startup: &'a WorkerStartup,
    pub(super) queues: &'a WorkerStartupQueues,
    pub(super) event_sender: &'a EventSender,
    pub(super) recovery: &'a Option<PathBuf>,
    pub(super) cancellation: &'a SessionCancellation,
    pub(super) last_source: &'a Option<(
        hex_engine::AudioSourceContext,
        hex_engine::SharedMetadataCacheIdentity,
    )>,
    pub(super) startup_error: &'a Option<String>,
    pub(super) main_files: &'a Option<NativeMainFiles>,
}
impl WorkerStartupView<'_> {
    pub fn main_file_slots_on_control(
        &self,
    ) -> Result<
        impl hex_engine::sampler_host_fields::FileSlotsFields<'_>,
        hex_engine::AudioStorageError,
    > {
        self.main_files
            .as_ref()
            .map(NativeMainFiles::fields)
            .ok_or(hex_engine::AudioStorageError::UnknownStorage)
    }
    pub fn cancellation_on_control(&self) -> &dyn hex_engine::CancellationQuery {
        self.cancellation
    }
    /// Reports the actual constructor phase; it is not an empty-project claim.
    pub fn has_session_on_control(&self) -> bool {
        self.startup.session.is_some()
    }
    pub fn worker_control_on_control(&self) -> Option<&SnapshotWorkerControl> {
        self.startup.control.as_ref()
    }
    /// No selected totals can qualify the still-opaque Event/export transports.
    /// Retain their ACTUAL operands here until their qualified records replace
    /// those paths; a zero occupancy or pre-Session phase cannot waive them.
    pub fn require_complete_storage_on_control(&self) -> Result<(), hex_engine::AudioStorageError> {
        let Self {
            startup,
            queues,
            event_sender,
            recovery,
            cancellation,
            last_source,
            startup_error,
            main_files,
        } = self;
        let _ = (
            startup,
            queues,
            event_sender,
            recovery,
            cancellation,
            last_source,
            startup_error,
            main_files,
        );
        Err(hex_engine::AudioStorageError::UnknownStorage)
    }
}
// This is stored by the actual WorkerEvents constructor, not supplied by a
// native caller. Entered does not mean prepared or published; it is never reset
// by an ordinary refusal or by Session metadata cleanup.
#[derive(Default, PartialEq, Eq)]
pub(super) enum InitialSnapshotPhase {
    #[default]
    Fresh,
    Entered,
    Legacy,
}

/// The actual negotiated Session before its first current/compiled/Event birth.
/// This finite borrowed phase is constructed only by WorkerEvents on the worker.
/// No model owner, copied Session or quote can be extracted from it.
/// ```compile_fail
/// fn escape<'a>(roots: hex_session::service::WorkerInitialSessionView<'a>)
///     -> hex_session::service::WorkerInitialSessionView<'static> { roots }
/// ```
/// ```compile_fail
/// fn send(roots: hex_session::service::WorkerInitialSessionView<'static>) {
///     std::thread::spawn(move || drop(roots));
/// }
/// ```
pub struct WorkerInitialSessionView<'a> {
    pub(super) startup: &'a WorkerStartup,
    pub(super) queues: &'a WorkerStartupQueues,
    pub(super) event_sender: &'a EventSender,
    pub(super) recovery: &'a Option<PathBuf>,
    pub(super) cancellation: &'a SessionCancellation,
    pub(super) last_source: &'a Option<(
        hex_engine::AudioSourceContext,
        hex_engine::SharedMetadataCacheIdentity,
    )>,
    pub(super) startup_error: &'a Option<String>,
    pub(super) main_files: &'a Option<NativeMainFiles>,
    pub(super) phase: &'a mut InitialSnapshotPhase,
}
impl WorkerInitialSessionView<'_> {
    pub fn main_file_slots_on_control(
        &self,
    ) -> Result<
        impl hex_engine::sampler_host_fields::FileSlotsFields<'_>,
        hex_engine::AudioStorageError,
    > {
        self.main_files
            .as_ref()
            .map(NativeMainFiles::fields)
            .ok_or(hex_engine::AudioStorageError::UnknownStorage)
    }
    /// Actual transport class only. This is no storage/native certificate.
    pub fn has_registered_event_transport_on_control(&self) -> bool {
        self.event_sender.is_registered()
    }
    /// Fixed transfer AFTER the closed Initial constructor and ALL of its
    /// family/Session/native guards end. Actual records retain source owners;
    /// Full/Busy leaves every producer field intact, with no legacy adoption.
    pub fn enqueue_initial_event_reads_on_control(
        &self,
        canonical: &mut Option<crate::snapshot_exports::SnapshotReadStamp>,
        device: &mut Option<crate::snapshot_exports::SnapshotReadStamp>,
        tuning: &mut Option<crate::snapshot_exports::SnapshotReadStamp>,
    ) -> Result<(), crate::snapshot_exports::SnapshotTransportError> {
        use crate::snapshot_exports::SnapshotTransportError;
        if *self.phase != InitialSnapshotPhase::Entered
            || hex_engine::CancellationQuery::is_cancelled(self.cancellation)
        {
            return Err(SnapshotTransportError::Stale);
        }
        let control = self
            .startup
            .control()
            .ok_or(SnapshotTransportError::Protocol)?;
        self.event_sender
            .enqueue_registered(control, canonical, device, tuning)
    }
    pub fn sample_rate_on_control(&self) -> Result<u32, hex_engine::AudioStorageError> {
        self.startup
            .session
            .as_ref()
            .map(|session| session.project().sample_rate())
            .ok_or(hex_engine::AudioStorageError::SourceMismatch)
    }
    /// No allocation or model mutation: take the SAME Session writers only.
    /// Initial cannot enter the ordinary edit stage/commit operation. Full
    /// startup/family/native preflight remains mandatory before source birth.
    pub fn original_sampler_initial_session_loan_on_control(
        &mut self,
    ) -> Result<crate::OriginalSamplerSessionLoan<'_>, hex_engine::AudioStorageError> {
        use hex_engine::{AudioStorageError, CancellationQuery};
        if *self.phase != InitialSnapshotPhase::Fresh
            || self.last_source.is_some()
            || self.cancellation.is_cancelled()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let session = self
            .startup
            .session
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?;
        let loan = session.original_sampler_initial_writers_on_control()?;
        *self.phase = InitialSnapshotPhase::Entered;
        Ok(loan)
    }
    /// Reborrow only the SAME pending Initial operation after a nonmutating
    /// busy/cap/row refusal. This neither starts another initial operation nor
    /// resets its purpose, failure latch, source fields or worker phase.
    pub fn resume_original_sampler_initial_session_loan_on_control(
        &mut self,
    ) -> Result<crate::OriginalSamplerSessionLoan<'_>, hex_engine::AudioStorageError> {
        use hex_engine::{AudioStorageError, CancellationQuery};
        if *self.phase != InitialSnapshotPhase::Entered
            || self.last_source.is_some()
            || self.cancellation.is_cancelled()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.startup
            .session
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .resume_original_sampler_initial_writers_on_control()
    }
    /// These ACTUAL transports remain unresolved. The constructor phase proves
    /// pre-current absence, never absence of the real queues/control children.
    pub fn require_complete_storage_on_control(&self) -> Result<(), hex_engine::AudioStorageError> {
        let Self {
            startup,
            queues,
            event_sender,
            recovery,
            cancellation,
            last_source,
            startup_error,
            main_files,
            phase,
        } = self;
        let _ = (
            startup,
            queues,
            event_sender,
            recovery,
            cancellation,
            last_source,
            startup_error,
            main_files,
            phase,
        );
        Err(hex_engine::AudioStorageError::UnknownStorage)
    }
}

/// Actual worker destinations travel together through failed spawn/startup.
/// They are moved once into run_worker; no receiver/queue clone is fabricated.
pub(super) struct WorkerStartupQueues {
    pub(super) requests: RequestReceiver,
    pub(super) export_work: WorkerExportSender,
    pub(super) export_events: ExportResultSender,
}
pub(super) enum ServiceStop {
    Unqualified(Arc<AtomicBool>),
    Registered(SnapshotWorkerStop),
}
impl ServiceStop {
    pub(super) fn request_cancel(&self) {
        match self {
            Self::Unqualified(flag) => flag.store(true, Ordering::Release),
            Self::Registered(stop) => {
                let _ = stop.request_cancel();
            }
        }
    }
}
pub(super) struct WorkerStartup {
    session: Option<Session>,
    events: Option<WorkerEvents>,
    queues: Option<WorkerStartupQueues>,
    pending_cancellation: Option<SessionCancellation>,
    control: Option<SnapshotWorkerControl>,
}
impl WorkerStartup {
    pub(super) fn new(
        session: Option<Session>,
        control: Option<SnapshotWorkerControl>,
        cancellation: SessionCancellation,
    ) -> Self {
        Self {
            session,
            events: None,
            queues: None,
            pending_cancellation: Some(cancellation),
            control,
        }
    }
    pub(super) fn install_events(&mut self, events: WorkerEvents) {
        self.events = Some(events);
    }
    pub(super) fn install_queues(&mut self, queues: &mut Option<WorkerStartupQueues>) -> bool {
        if self.queues.is_some() || queues.is_none() {
            return false;
        }
        self.queues = queues.take();
        true
    }
    pub(super) fn take_queues(&mut self) -> Option<WorkerStartupQueues> {
        self.queues.take()
    }
    pub(super) fn take_cancellation(&mut self) -> Option<SessionCancellation> {
        self.pending_cancellation.take()
    }
    pub(super) fn take_events(&mut self) -> Option<WorkerEvents> {
        self.events.take()
    }
    pub(super) fn has_session(&self) -> bool {
        self.session.is_some()
    }
    pub(super) fn install_negotiated_session(&mut self, incoming: &mut Option<Session>) -> bool {
        if self.session.is_some() || incoming.is_none() {
            return false;
        }
        self.session = incoming.take();
        true
    }
    pub(super) fn take_session(&mut self) -> Option<Session> {
        self.session.take()
    }
    pub(super) fn control(&self) -> Option<&SnapshotWorkerControl> {
        self.control.as_ref()
    }
}

impl Drop for WorkerStartup {
    fn drop(&mut self) {
        drop(self.session.take());
        // Native close precedes completion, independently of retained UI reads.
        drop(self.events.take());
        drop(self.queues.take());
        drop(self.pending_cancellation.take());
        drop(self.control.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_phase_uses_same_session_writers_and_cannot_enter_edit_or_reenter() {
        use hex_engine::sampler_host_fields::{
            OriginalSamplerPendingSession, OriginalSamplerResidentFields,
        };
        let signal =
            hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
        let job = hex_engine::RetainedControlJob::try_from_owned_on_control(
            &mut Some(0_u8),
            usize::MAX,
            |_: &mut u8| Ok::<_, ()>(hex_engine::RetainedControlJobProgress::Pending),
        )
        .unwrap();
        let session = Session::new_with_retained_cancellation_on_control(
            44_100,
            8,
            0,
            signal.bind_worker_on_control(job.route()),
        )
        .unwrap();
        let shutdown = session.shutdown.clone();
        let mut startup = WorkerStartup::new(Some(session), None, shutdown.clone());
        let (request_sender, requests) = mpsc::sync_channel(1);
        let (export_work, _work) = mpsc::sync_channel(1);
        let (export_events, _exports) = mpsc::sync_channel(1);
        let queues = WorkerStartupQueues {
            requests: RequestReceiver::Unqualified(requests),
            export_work: export_work.into(),
            export_events: export_events.into(),
        };
        let (event_sender, _events) = mpsc::sync_channel(1);
        let event_sender = EventSender::Unqualified(event_sender);
        let recovery = None;
        let last_source = None;
        let startup_error = None;
        let mut phase = InitialSnapshotPhase::Fresh;
        {
            let mut roots = WorkerInitialSessionView {
                startup: &startup,
                queues: &queues,
                event_sender: &event_sender,
                recovery: &recovery,
                cancellation: &shutdown,
                last_source: &last_source,
                startup_error: &startup_error,
                main_files: &None,
                phase: &mut phase,
            };
            assert_eq!(roots.sample_rate_on_control(), Ok(44_100));
            {
                let mut loan = roots
                    .original_sampler_initial_session_loan_on_control()
                    .unwrap();
                assert_eq!(
                    loan.pending_context_on_control().unwrap(),
                    hex_engine::AudioSourceContext {
                        project_generation: 0,
                        editor_revision: 0
                    }
                );
                let (resident, _writers) = loan.fields_and_writer_on_control();
                resident.validate_remaining_fields().unwrap();
                let pending = resident.pending_source().unwrap();
                assert!(pending.project.is_none() && pending.source.is_none());
                // This initial loan has no Editor request. Even a direct call
                // cannot allocate through the edit producer or fake a Noop.
                assert_eq!(
                    OriginalSamplerPendingSession::stage_and_capture_on_control(&mut loan),
                    Err(hex_engine::AudioStorageError::SourceMismatch)
                );
            }
            assert!(matches!(
                roots.original_sampler_initial_session_loan_on_control(),
                Err(hex_engine::AudioStorageError::SourceMismatch)
            ));
            {
                let mut same = roots
                    .resume_original_sampler_initial_session_loan_on_control()
                    .unwrap();
                let (fields, _) = same.fields_and_writer_on_control();
                assert!(fields.pending_source().unwrap().source.is_none());
            }
        }
        let session = startup.session.as_mut().unwrap();
        assert_eq!(
            session.commit_original_sampler_pending_on_control(),
            Err(hex_engine::AudioStorageError::SourceMismatch)
        );
        session.clear_original_sampler_pending_on_control().unwrap();
        assert!(!session.can_undo() && !session.can_redo());
        assert_eq!(session.project().sample_rate(), 44_100);
        // Metadata cleanup cannot reset the actual worker's once-only phase.
        assert!(phase == InitialSnapshotPhase::Entered);
        drop(request_sender);
    }

    #[test]
    fn failed_startup_drops_actual_queues_and_duplicate_install_retains_incoming() {
        let make_queues = || {
            let (request_sender, requests) = mpsc::sync_channel(1);
            let (export_work, work_receiver) = mpsc::sync_channel(1);
            let (export_events, event_receiver) = mpsc::sync_channel(1);
            (
                Some(WorkerStartupQueues {
                    requests: RequestReceiver::Unqualified(requests),
                    export_work: export_work.into(),
                    export_events: export_events.into(),
                }),
                request_sender,
                work_receiver,
                event_receiver,
            )
        };
        let mut startup = WorkerStartup::new(
            None,
            None,
            SessionCancellation::Unqualified(Arc::new(AtomicBool::new(false))),
        );
        let (mut first, first_requests, first_work, first_events) = make_queues();
        let (mut second, second_requests, second_work, second_events) = make_queues();
        assert!(startup.install_queues(&mut first));
        assert!(first.is_none());
        assert!(!startup.install_queues(&mut second));
        assert!(second.is_some());
        drop(startup);
        assert!(matches!(
            first_requests.try_send(Request::Edit(Command::AddTrack {
                name: "closed".into()
            })),
            Err(TrySendError::Disconnected(_))
        ));
        assert!(matches!(
            first_work.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        assert!(matches!(
            first_events.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        second_requests
            .try_send(Request::Edit(Command::AddTrack {
                name: "retained".into(),
            }))
            .unwrap();
        assert!(
            second
                .as_ref()
                .unwrap()
                .requests
                .unqualified()
                .unwrap()
                .try_recv()
                .is_ok()
        );
        drop(second);
        assert!(matches!(
            second_work.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        assert!(matches!(
            second_events.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
    }
}
