//! Existing 32-slot FIFO, with a scalar accepted-source-intent stamp. This
//! permits ordered native preparation to poll cancellation without consuming a
//! later request, adding a queue or emitting an out-of-order ordinary terminal.
//! Fresh catalog requests also retain their configured data directory at accepted
//! submission; later configuration never changes an already queued construction.
use super::*;
use std::sync::atomic::AtomicU64;

pub(super) struct QueuedRequest {
    /// Frozen application configuration for fresh catalog creation only.
    pub native_device_defaults: Option<Arc<PathBuf>>,
    pub request: Request,
    pub recording_barrier: Option<super::recording_barrier::Completion>,
    pub(super) publication_wait_failure: Option<&'static str>,
    pub(super) publication_waited: bool,
    pub document_intent: u64,
    pub save_correlation: Option<super::SaveCorrelation>,
    pub diagnostic: Option<super::diagnostics::DiagnosticRequest>,
    pub diagnostic_origin: super::diagnostics::DiagnosticOrigin,
}
impl QueuedRequest {
    pub(super) fn continuation(request: Request, intent: &AtomicU64) -> Self {
        Self {
            native_device_defaults: None,
            request,
            recording_barrier: None,
            publication_wait_failure: None,
            publication_waited: false,
            document_intent: intent.load(Ordering::Acquire),
            save_correlation: None,
            diagnostic: None,
            diagnostic_origin: super::diagnostics::DiagnosticOrigin::Continuation,
        }
    }
}

/// Read-only cancellation of control-side preparation. This is neither a
/// canonical generation nor publication authority. Admission can win before a
/// later source intent; callers must recheck at their actual admission hook.
#[derive(Clone)]
pub struct RequestCancellation {
    shutdown: Arc<AtomicBool>,
    document_intent: Arc<AtomicU64>,
    expected_intent: u64,
    publication_waited: bool,
    bounce: Option<hex_export::ExportControl>,
}
impl RequestCancellation {
    pub(super) fn new(
        shutdown: Arc<AtomicBool>,
        document_intent: Arc<AtomicU64>,
        expected_intent: u64,
    ) -> Self {
        Self {
            shutdown,
            document_intent,
            expected_intent,
            publication_waited: false,
            bounce: None,
        }
    }
    pub(super) fn with_bounce(mut self, control: &hex_export::ExportControl) -> Self {
        self.bounce = Some(control.clone());
        self
    }
    pub(super) fn after_publication_wait(mut self, waited: bool) -> Self {
        self.publication_waited = waited;
        self
    }
    /// Whether this request must recheck document intent at graph admission.
    pub fn requires_publication_admission_check(&self) -> bool {
        self.publication_waited
    }
    /// Shutdown alone interrupts FIFO document replacement.
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }
    pub fn is_cancelled(&self) -> bool {
        self.is_shutting_down()
            || self.document_intent.load(Ordering::Acquire) != self.expected_intent
            || self
                .bounce
                .as_ref()
                .is_some_and(hex_export::ExportControl::is_cancelled)
    }
}

/// Sole UI producer, like Service (which is not Sync). Only a successfully
/// queued New/Open/audio restart advances cancellation intent. Full/closed sends return the
/// original request without cancelling any admitted operation. No extra queue.
pub(super) struct Sender {
    device_defaults_directory: Option<Arc<PathBuf>>,
    channel: SyncSender<QueuedRequest>,
    document_intent: Arc<AtomicU64>,
    single_producer: std::marker::PhantomData<std::cell::Cell<()>>,
}
impl Sender {
    pub(super) fn new(channel: SyncSender<QueuedRequest>, document_intent: Arc<AtomicU64>) -> Self {
        Self {
            device_defaults_directory: None,
            channel,
            document_intent,
            single_producer: std::marker::PhantomData,
        }
    }
    pub(super) fn set_device_defaults_directory(&mut self, root: PathBuf) {
        self.device_defaults_directory = Some(Arc::new(root));
    }
    #[allow(clippy::result_large_err)] // Exact original request on bounded admission refusal.
    #[cfg(test)]
    pub(super) fn submit(&self, request: Request) -> Result<(), SubmitError> {
        self.submit_traced(request, None)
    }
    #[allow(clippy::result_large_err)] // Exact request custody is unchanged by diagnostic metadata.
    pub(super) fn submit_traced(
        &self,
        request: Request,
        diagnostic: Option<super::diagnostics::DiagnosticRequest>,
    ) -> Result<(), SubmitError> {
        self.submit_traced_save(request, diagnostic, None)
    }

    #[allow(clippy::result_large_err)] // Exact request custody is unchanged by save metadata.
    pub(super) fn submit_traced_save(
        &self,
        request: Request,
        diagnostic: Option<super::diagnostics::DiagnosticRequest>,
        save_correlation: Option<super::SaveCorrelation>,
    ) -> Result<(), SubmitError> {
        let replaces_document = matches!(
            request,
            Request::NewWithAudioMode { .. }
                | Request::Open(_)
                | Request::OpenWith { .. }
                | Request::OpenWithAudioMode { .. }
                | Request::RestartAudio { .. }
        );
        let intent = self.document_intent.load(Ordering::Acquire);
        let next = if replaces_document {
            let Some(next) = intent.checked_add(1) else {
                return Err(SubmitError::Busy(
                    request,
                    super::BusyReason::DocumentIntentExhausted,
                ));
            };
            Some(next)
        } else {
            None
        };
        let native_device_defaults = matches!(
            &request,
            Request::AddNativeDevice { .. }
                | Request::ReplaceNativeDevice { .. }
                | Request::InsertBrowserInstrumentTrack { .. }
                | Request::InsertEffectBusDevice { .. }
                | Request::DropBrowserDeviceOnArrangement { .. }
                | Request::DropNativeDevice {
                    source: NativeDeviceDropSource::Browser { .. },
                    ..
                }
        )
        .then(|| self.device_defaults_directory.clone())
        .flatten();
        match self.channel.try_send(QueuedRequest {
            native_device_defaults,
            request,
            recording_barrier: None,
            publication_wait_failure: None,
            publication_waited: false,
            document_intent: intent,
            save_correlation,
            diagnostic,
            diagnostic_origin: super::diagnostics::DiagnosticOrigin::Submitted,
        }) {
            Ok(()) => {
                if let Some(next) = next {
                    // Cancellation linearizes here, after successful queue
                    // admission and before submit returns. The worker may have
                    // already won its own admission; that remains accepted.
                    self.document_intent.store(next, Ordering::Release);
                }
                Ok(())
            }
            Err(TrySendError::Full(queued)) => Err(SubmitError::Full(queued.request)),
            Err(TrySendError::Disconnected(queued)) => Err(SubmitError::Closed(queued.request)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_defaults_capture_configuration_at_submission_without_leaking_to_supplied_state() {
        let (channel, receiver) = mpsc::sync_channel(4);
        let intent = Arc::new(AtomicU64::new(0));
        let mut sender = Sender::new(channel, intent);
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "fixture".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let fresh = || Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.utility",
            )),
            before: None,
        };
        sender.submit(fresh()).unwrap();
        sender.set_device_defaults_directory(PathBuf::from("/first"));
        sender.submit(fresh()).unwrap();
        sender.set_device_defaults_directory(PathBuf::from("/second"));
        sender.submit(fresh()).unwrap();
        sender
            .submit(Request::Edit(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            }))
            .unwrap();
        assert!(receiver.recv().unwrap().native_device_defaults.is_none());
        assert_eq!(
            receiver.recv().unwrap().native_device_defaults.as_deref(),
            Some(&PathBuf::from("/first"))
        );
        assert_eq!(
            receiver.recv().unwrap().native_device_defaults.as_deref(),
            Some(&PathBuf::from("/second"))
        );
        assert!(receiver.recv().unwrap().native_device_defaults.is_none());
    }

    #[test]
    fn correlated_save_uses_existing_bounded_fifo_and_retains_identity() {
        let (channel, receiver) = mpsc::sync_channel(1);
        let intent = Arc::new(AtomicU64::new(0));
        let sender = Sender::new(channel, intent);
        let correlation = SaveCorrelation {
            request_id: 7,
            project_generation: 2,
            revision: 9,
        };
        sender
            .submit_traced_save(Request::SaveCurrent, None, Some(correlation))
            .unwrap();
        assert!(matches!(
            sender.submit_traced_save(Request::SaveCurrent, None, Some(correlation)),
            Err(SubmitError::Full(Request::SaveCurrent))
        ));
        let queued = receiver.recv().unwrap();
        assert!(matches!(queued.request, Request::SaveCurrent));
        assert_eq!(queued.save_correlation, Some(correlation));
    }

    #[test]
    fn accepted_document_intent_cancels_earlier_work_without_reordering_fifo() {
        let (channel, receiver) = mpsc::sync_channel(3);
        let intent = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let sender = Sender::new(channel, Arc::clone(&intent));
        sender.submit(Request::Undo).unwrap();
        let earlier = receiver.recv().unwrap();
        let cancellation = RequestCancellation::new(
            Arc::clone(&shutdown),
            Arc::clone(&intent),
            earlier.document_intent,
        );
        assert!(!cancellation.is_cancelled());
        sender.submit(Request::Redo).unwrap();
        sender
            .submit(Request::Open(PathBuf::from("later.hex")))
            .unwrap();
        assert!(cancellation.is_cancelled());
        assert!(matches!(receiver.recv().unwrap().request, Request::Redo));
        assert!(matches!(receiver.recv().unwrap().request, Request::Open(_)));
        sender.submit(Request::Undo).unwrap();
        let next = receiver.recv().unwrap();
        let cancellation =
            RequestCancellation::new(Arc::clone(&shutdown), intent, next.document_intent);
        assert!(!cancellation.is_cancelled());
        sender
            .submit(Request::RestartAudio {
                project_generation: 0,
                request_id: 1,
            })
            .unwrap();
        assert!(cancellation.is_cancelled());
        let next = receiver.recv().unwrap();
        assert!(matches!(next.request, Request::RestartAudio { .. }));
        let cancellation = RequestCancellation::new(
            Arc::clone(&shutdown),
            Arc::clone(&sender.document_intent),
            sender.document_intent.load(Ordering::Acquire),
        );
        assert!(!cancellation.is_cancelled());
        shutdown.store(true, Ordering::Release);
        assert!(cancellation.is_cancelled());
    }

    #[test]
    fn refused_document_requests_do_not_cancel_or_consume_the_original_request() {
        let (channel, receiver) = mpsc::sync_channel(0);
        let intent = Arc::new(AtomicU64::new(0));
        let sender = Sender::new(channel, Arc::clone(&intent));
        assert!(
            matches!(sender.submit(Request::Open(PathBuf::from("full.hex"))), Err(SubmitError::Full(Request::Open(path))) if path == Path::new("full.hex"))
        );
        assert_eq!(intent.load(Ordering::Acquire), 0);
        drop(receiver);
        assert!(
            matches!(sender.submit(Request::Open(PathBuf::from("closed.hex"))), Err(SubmitError::Closed(Request::Open(path))) if path == Path::new("closed.hex"))
        );
        assert_eq!(intent.load(Ordering::Acquire), 0);
    }
}
