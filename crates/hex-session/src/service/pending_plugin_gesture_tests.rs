//! Cancellation routing only; native terminal and service event coverage are separate.
use super::*;
use crate::device_gesture::{GestureKey, PluginParameterValue};

#[derive(Default)]
struct Sink {
    cancelled: Vec<(hex_engine::AudioSourceContext, GestureRequest)>,
    finishes: usize,
}

struct StaleSink {
    expected_context: hex_engine::AudioSourceContext,
    expected_request: GestureRequest,
    disposition: Option<StalePluginGestureDisposition>,
}
impl WorkerAudioSink for StaleSink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn poll(&mut self) {}
    fn close(&mut self) {}
    fn finish_stale_plugin_parameter(
        &mut self,
        context: hex_engine::AudioSourceContext,
        request: GestureRequest,
    ) -> StalePluginGestureDisposition {
        assert_eq!(context, self.expected_context);
        assert_eq!(request, self.expected_request);
        self.disposition.take().unwrap()
    }
}

#[test]
fn stale_terminal_waits_for_actual_disposition_and_retires_only_its_key() {
    let context = hex_engine::AudioSourceContext {
        project_generation: 9,
        editor_revision: 3,
    };
    let key = GestureKey {
        generation: 9,
        token: 4,
    };
    let request = GestureRequest::ChangePlugin {
        key,
        sequence: 7,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    let other = GestureRequest::ChangePlugin {
        key: GestureKey {
            generation: 9,
            token: 5,
        },
        sequence: 7,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    let mut machine = crate::device_gesture::GestureMachine::default();
    let mut completed = Some(other);
    for (disposition, pending, lost) in [
        (StalePluginGestureDisposition::Pending, true, false),
        (StalePluginGestureDisposition::Restored, false, false),
        (
            StalePluginGestureDisposition::SourceLost("SOURCE LOST".into()),
            false,
            true,
        ),
    ] {
        let mut awaiting = Some(key);
        let mut sink = StaleSink {
            expected_context: context,
            expected_request: request,
            disposition: Some(disposition),
        };
        let outcome = finish_stale_plugin_gesture(
            Some(&mut sink),
            context,
            request,
            &mut machine,
            &mut awaiting,
            &mut completed,
        );
        assert_eq!(outcome.pending, pending);
        assert!(!outcome.ack.accepted && !outcome.canonical_changed);
        assert_eq!(outcome.ack.source_lost, lost);
        assert_eq!((outcome.ack.key, outcome.ack.sequence), (key, 7));
        assert!(outcome.playback.is_none() && outcome.preview_project.is_none());
        assert_eq!(awaiting, pending.then_some(key));
        assert_eq!(completed, Some(other));
    }
    completed = Some(request);
    let mut awaiting = Some(key);
    let mut sink = StaleSink {
        expected_context: context,
        expected_request: request,
        disposition: Some(StalePluginGestureDisposition::Restored),
    };
    let outcome = finish_stale_plugin_gesture(
        Some(&mut sink),
        context,
        request,
        &mut machine,
        &mut awaiting,
        &mut completed,
    );
    assert!(!outcome.pending && awaiting.is_none() && completed.is_none());
}
impl WorkerAudioSink for Sink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn poll(&mut self) {}
    fn close(&mut self) {}
    fn cancel_pending_plugin_parameter(
        &mut self,
        context: hex_engine::AudioSourceContext,
        request: GestureRequest,
    ) {
        self.cancelled.push((context, request));
    }
    fn finish_plugin_parameter_commit(
        &mut self,
        _: hex_engine::AudioSourceContext,
        _: GestureKey,
        _: u64,
        _: bool,
    ) {
        self.finishes += 1;
    }
}

#[test]
fn cancellation_routes_only_the_exact_pending_request_without_finishing_it() {
    let context = hex_engine::AudioSourceContext {
        project_generation: 9,
        editor_revision: 3,
    };
    let key = GestureKey {
        generation: 9,
        token: 4,
    };
    let request = GestureRequest::ChangePlugin {
        key,
        sequence: 7,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    let mut pending = PendingPluginGesture {
        request,
        context,
        document_intent: 0,
        fifo_completion: true,
        rebase: false,
        barrier: false,
        preview: None,
        publication: None,
        stopped_recovery: None,
        cancellation_requested: false,
        explicit_cancel_requested: false,
    };
    let mut sink = Sink::default();
    for wrong in [
        GestureRequest::ChangePlugin {
            key,
            sequence: 8,
            value: PluginParameterValue::new(0.75).unwrap(),
        },
        GestureRequest::ChangePlugin {
            key: GestureKey {
                generation: 9,
                token: 5,
            },
            sequence: 7,
            value: PluginParameterValue::new(0.75).unwrap(),
        },
        GestureRequest::CommitPlugin {
            key,
            sequence: 7,
            value: PluginParameterValue::new(0.75).unwrap(),
        },
        GestureRequest::ChangePlugin {
            key,
            sequence: 7,
            value: PluginParameterValue::new(0.5).unwrap(),
        },
    ] {
        assert!(!pending.request_cancel(wrong, true, &mut sink));
    }
    assert!(sink.cancelled.is_empty());
    assert!(pending.request_cancel(request, true, &mut sink));
    assert!(
        pending.request_cancel(request, true, &mut sink),
        "duplicate intent is idempotent"
    );
    assert_eq!(sink.cancelled, [(context, request)]);
    assert!(pending.explicit_cancel_requested);
    assert_eq!(sink.finishes, 0, "cancel intent is not a terminal");
    assert_eq!(pending.request, request);
}

#[test]
fn changed_context_requests_one_cancel_but_keeps_original_custody() {
    let context = hex_engine::AudioSourceContext {
        project_generation: 9,
        editor_revision: 3,
    };
    let request = GestureRequest::ChangePlugin {
        key: GestureKey {
            generation: 9,
            token: 4,
        },
        sequence: 7,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    let mut pending = PendingPluginGesture {
        request,
        context,
        document_intent: 0,
        fifo_completion: true,
        rebase: false,
        barrier: false,
        preview: None,
        publication: None,
        stopped_recovery: None,
        cancellation_requested: false,
        explicit_cancel_requested: false,
    };
    let mut sink = Sink::default();
    assert!(!pending.cancel_if_stale(context, 0, &mut sink));
    let newer = hex_engine::AudioSourceContext {
        editor_revision: 4,
        ..context
    };
    assert!(pending.cancel_if_stale(newer, 0, &mut sink));
    assert!(pending.cancel_if_stale(newer, 0, &mut sink));
    assert_eq!(sink.cancelled, [(context, request)]);
    assert!(!pending.explicit_cancel_requested);
    assert_eq!(sink.finishes, 0, "staleness does not synthesize a terminal");
    assert_eq!(pending.request, request);
    assert_eq!(pending.context, context);
}

#[test]
fn accepted_new_document_intent_cancels_before_context_changes() {
    let context = hex_engine::AudioSourceContext {
        project_generation: 9,
        editor_revision: 3,
    };
    let request = GestureRequest::ChangePlugin {
        key: GestureKey {
            generation: 9,
            token: 4,
        },
        sequence: 7,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    let mut pending = PendingPluginGesture {
        request,
        context,
        document_intent: 0,
        fifo_completion: true,
        rebase: false,
        barrier: false,
        preview: None,
        publication: None,
        stopped_recovery: None,
        cancellation_requested: false,
        explicit_cancel_requested: false,
    };
    let mut sink = Sink::default();
    assert!(pending.cancel_if_stale(context, 1, &mut sink));
    assert!(pending.cancel_if_stale(context, 1, &mut sink));
    assert_eq!(sink.cancelled, [(context, request)]);
    assert!(!pending.explicit_cancel_requested);
    assert_eq!(sink.finishes, 0);
    assert_eq!(pending.request, request);
}

#[test]
fn cancellation_intent_bypasses_a_full_fifo_without_creating_a_completion() {
    let (request_tx, request_rx) = mpsc::sync_channel(1);
    let (event_tx, event_rx) = mpsc::sync_channel(1);
    let (_export_tx, export_rx) = mpsc::sync_channel(1);
    let (terminal_tx, terminal_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
    let diagnostics = Arc::new(diagnostics::Hooks::default());
    let service = Service {
        export_test_hook: Arc::new(std::sync::Mutex::new(None)),
        requests: request_cancellation::Sender::new(
            request_tx,
            Arc::new(std::sync::atomic::AtomicU64::new(0)),
        ),
        plugin_cancel: Arc::new(std::sync::Mutex::new(None)),
        plugin_terminal: terminal_tx,
        events: diagnostics::EventReceiver::new(event_rx, Arc::clone(&diagnostics)),
        diagnostics,
        export_events: export_rx,
        export_busy: Arc::new(AtomicBool::new(false)),
        shutdown: Arc::new(AtomicBool::new(false)),
        arrangement_seek: None,
        resident_seek_results: None,
        resident_seek_count: AtomicUsize::new(0),
        _worker: thread::spawn(move || {
            let _ = release_rx.recv();
        }),
    };
    service.submit(Request::Undo).unwrap();
    let request = GestureRequest::ChangePlugin {
        key: GestureKey {
            generation: 9,
            token: 4,
        },
        sequence: 7,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    service
        .submit(Request::CancelPendingPluginParameter { request })
        .unwrap();
    service
        .submit(Request::CancelPendingPluginParameter { request })
        .unwrap();
    let other = GestureRequest::ChangePlugin {
        key: GestureKey {
            generation: 9,
            token: 4,
        },
        sequence: 8,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    assert!(matches!(
        service.submit(Request::CancelPendingPluginParameter { request: other }),
        Err(SubmitError::Busy(_))
    ));
    let restore = GestureRequest::Cancel { key: request.key() };
    service.submit_plugin_cancel(restore).unwrap();
    assert!(matches!(
        service.submit_plugin_cancel(restore),
        Err(SubmitError::Full(_))
    ));
    assert!(
        matches!(terminal_rx.try_recv().unwrap().request, Request::DeviceGesture(actual) if actual == restore)
    );
    assert_eq!(*service.plugin_cancel.lock().unwrap(), Some(request));
    assert!(matches!(
        request_rx.try_recv().unwrap().request,
        Request::Undo
    ));
    assert!(matches!(service.try_event(), Err(TryRecvError::Empty)));
    drop(event_tx);
    release_tx.send(()).unwrap();
}
