//! Control-owner custody tests; actual callback receipt behavior is tested by desktop.
use super::*;

struct QueuedSink {
    inner: OfflineSink,
    terminal: ResidentPublicationTerminal,
    finished: Vec<(hex_engine::AudioSourceContext, bool)>,
    offline: bool,
}
impl WorkerAudioSink for QueuedSink {
    fn retained_resident_publication(
        &self,
        context: hex_engine::AudioSourceContext,
    ) -> Option<ResidentRetainedReceipt> {
        self.offline
            .then_some(ResidentRetainedReceipt { id: 51, context })
    }
    fn poll_retained_resident_publication(
        &mut self,
        receipt: ResidentRetainedReceipt,
    ) -> ResidentPublicationTerminal {
        if self.offline && receipt.id == 51 {
            self.terminal
        } else {
            ResidentPublicationTerminal::Indeterminate
        }
    }
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String> {
        self.inner.initialize(shutdown)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        self.inner.canonical_changed(source)
    }
    fn poll(&mut self) {
        self.inner.poll()
    }
    fn close(&mut self) {
        self.inner.close()
    }
    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        self.inner.resident_admission()
    }
    fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
        self.inner.native_sampler_hold()
    }
    fn prepare_resident_effects(
        &mut self,
        input: ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<PreparedRevision>, String> {
        self.inner.prepare_resident_effects(input, prepare)
    }
    fn finish_resident_effect_edit(
        &mut self,
        context: hex_engine::AudioSourceContext,
        accepted: bool,
    ) {
        self.finished.push((context, accepted));
    }
    fn poll_resident_publication_terminal(
        &mut self,
        receipt: ResidentPublicationReceipt,
    ) -> ResidentPublicationTerminal {
        assert_eq!((receipt.output_identity, receipt.audio_revision), (41, 7));
        self.terminal
    }
    #[allow(clippy::result_large_err)]
    fn publish_resident(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        match self.inner.publish_resident(
            playback,
            resources,
            observation,
            metadata,
            policy,
            limit,
            admit,
        )? {
            ResidentPublicationAcceptance::OfflineRetained => {
                if self.offline {
                    return Ok(ResidentPublicationAcceptance::OfflineRetained);
                }
                Ok(ResidentPublicationAcceptance::Submitted {
                    output_identity: 41,
                    audio_revision: 7,
                })
            }
            other => Ok(other),
        }
    }
}

#[test]
fn staged_history_keeps_canonical_state_until_exact_terminal() {
    let files = sampler_export::tests::Files::new(false);
    let mut session = interactive_sampler_session(&files);
    let original = session.project().clone();
    session
        .editor
        .execute(Command::AddTrack {
            name: "pending".into(),
        })
        .unwrap();
    let edited = session.project().clone();
    let observed = Arc::new(Observed::default());
    let mut sink = QueuedSink {
        inner: OfflineSink {
            observed,
            pending: None,
        },
        terminal: ResidentPublicationTerminal::Pending,
        finished: Vec::new(),
        offline: false,
    };
    let context = hex_engine::AudioSourceContext {
        project_generation: 0,
        editor_revision: 3,
    };
    let current = session
        .resident_seek_inputs(sampler_export::Admission::default(), None, None)
        .unwrap()
        .prepare(
            session.project(),
            &mut sink,
            &session.media_bank(),
            session.sine_table(),
            &session.audio_transforms,
            &session.audio_clip_waveforms,
            context,
        )
        .unwrap();
    let token = session
        .editor
        .prepare_plugin_history(false)
        .unwrap()
        .unwrap();
    let expected_undo = token.project().clone();
    assert_eq!(expected_undo.tracks().len(), original.tracks().len());
    let pending = session
        .stage_plugin_publication(
            token,
            context,
            &mut sink,
            sampler_export::Admission::default(),
            Some(&current),
        )
        .unwrap();
    assert_eq!(session.project(), &edited);
    let PendingPluginPublication { receipt, .. } = &pending;
    assert_eq!(receipt.unwrap().context.editor_revision, 4);
    let pending = match pending.resolve(&mut session, &mut sink) {
        PluginPublicationResolution::Waiting(pending) => pending,
        _ => panic!("expected pending"),
    };
    sink.terminal = ResidentPublicationTerminal::Indeterminate;
    let pending = match pending.resolve(&mut session, &mut sink) {
        PluginPublicationResolution::Indeterminate(pending) => pending,
        _ => panic!("expected indeterminate"),
    };
    let mut pending = pending;
    for terminal in [
        ResidentPublicationTerminal::OutputClosedUnapplied,
        ResidentPublicationTerminal::OutputClosedApplied,
        ResidentPublicationTerminal::OutputClosedRefused(
            hex_engine::playback::PlaybackHandoffRejection::RuntimeFenceChanged,
        ),
    ] {
        sink.terminal = terminal;
        pending = match pending.resolve(&mut session, &mut sink) {
            PluginPublicationResolution::Indeterminate(pending) => pending,
            _ => panic!("retired output still requires source recovery"),
        };
        assert_eq!(session.project(), &edited);
        assert!(sink.finished.is_empty());
    }
    assert_eq!(session.project(), &edited);
    sink.terminal = ResidentPublicationTerminal::Refused(
        hex_engine::playback::PlaybackHandoffRejection::RuntimeFenceChanged,
    );
    assert!(matches!(
        pending.resolve(&mut session, &mut sink),
        PluginPublicationResolution::Refused(_)
    ));
    assert_eq!(session.project(), &edited);
    assert_eq!(
        sink.finished.last(),
        Some(&(
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            false
        ))
    );
    sink.inner.pending = None; // Simulated callback returned its actual graph on refusal.
    let token = session
        .editor
        .prepare_plugin_history(false)
        .unwrap()
        .unwrap();
    let pending = session
        .stage_plugin_publication(
            token,
            context,
            &mut sink,
            sampler_export::Admission::default(),
            Some(&current),
        )
        .unwrap();
    sink.terminal = ResidentPublicationTerminal::Applied;
    assert!(matches!(
        pending.resolve(&mut session, &mut sink),
        PluginPublicationResolution::Applied(_)
    ));
    assert_eq!(session.project(), &expected_undo);
    assert!(session.editor.can_redo());
    assert_eq!(
        sink.finished.last(),
        Some(&(
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            true
        ))
    );
    // The sink retains this redo while output is unavailable. Retention itself
    // does not advance history; only the exact later callback terminal does.
    sink.inner.pending = None;
    sink.offline = true;
    sink.terminal = ResidentPublicationTerminal::Pending;
    let redo_context = hex_engine::AudioSourceContext {
        editor_revision: 4,
        ..context
    };
    let token = session
        .editor
        .prepare_plugin_history(true)
        .unwrap()
        .unwrap();
    let pending = session
        .stage_plugin_publication(
            token,
            redo_context,
            &mut sink,
            sampler_export::Admission::default(),
            Some(&current),
        )
        .unwrap();
    assert_eq!(session.project(), &expected_undo);
    let pending = match pending.resolve(&mut session, &mut sink) {
        PluginPublicationResolution::Waiting(pending) => pending,
        _ => panic!("offline custody must wait for the callback"),
    };
    sink.terminal = ResidentPublicationTerminal::Applied;
    assert!(matches!(
        pending.resolve(&mut session, &mut sink),
        PluginPublicationResolution::Applied(_)
    ));
    assert_eq!(session.project(), &edited);
    assert_eq!(
        sink.finished.last(),
        Some(&(
            hex_engine::AudioSourceContext {
                editor_revision: 5,
                ..context
            },
            true
        ))
    );
}

#[test]
fn completed_history_publication_wakes_before_the_held_fifo() {
    use crate::service::audio_sink::WorkerEvents;
    use crate::service::request_cancellation::QueuedRequest;
    use std::sync::mpsc;

    let files = sampler_export::tests::Files::new(false);
    let mut session = interactive_sampler_session(&files);
    session
        .editor
        .execute(Command::AddTrack {
            name: "pending".into(),
        })
        .unwrap();
    let observed = Arc::new(Observed::default());
    let mut sink = QueuedSink {
        inner: OfflineSink {
            observed,
            pending: None,
        },
        terminal: ResidentPublicationTerminal::Applied,
        finished: Vec::new(),
        offline: false,
    };
    let context = hex_engine::AudioSourceContext {
        project_generation: 0,
        editor_revision: 3,
    };
    let current = session
        .resident_seek_inputs(sampler_export::Admission::default(), None, None)
        .unwrap()
        .prepare(
            session.project(),
            &mut sink,
            &session.media_bank(),
            session.sine_table(),
            &session.audio_transforms,
            &session.audio_clip_waveforms,
            context,
        )
        .unwrap();
    let token = session
        .editor
        .prepare_plugin_history(false)
        .unwrap()
        .unwrap();
    let publication = session
        .stage_plugin_publication(
            token,
            context,
            &mut sink,
            sampler_export::Admission::default(),
            Some(&current),
        )
        .unwrap();
    let mut pending = crate::service::PendingPluginHistory {
        publication,
        fifo_completion: true,
    };
    let (event_tx, _event_rx) = mpsc::sync_channel(2);
    let mut events = WorkerEvents::new(
        event_tx,
        Arc::new(AtomicBool::new(false)),
        Some(Box::new(sink)),
    );
    let (tx, rx) = mpsc::sync_channel(2);
    let intent = Arc::new(std::sync::atomic::AtomicU64::new(0));
    tx.send(QueuedRequest::continuation(Request::Undo, &intent))
        .unwrap();
    tx.send(QueuedRequest::continuation(Request::Redo, &intent))
        .unwrap();
    let (_terminal_tx, terminal_rx) = mpsc::sync_channel(1);
    let mailbox = std::sync::Mutex::new(None);
    let mut awaiting = None;
    let mut deferred = None;
    let first = events
        .next_request_with_control_poll(
            &rx,
            &intent,
            &mailbox,
            &terminal_rx,
            &mut awaiting,
            None,
            None,
            &mut deferred,
            true,
            None,
            None,
            None,
            Some(&mut pending),
            None,
            context,
            |_| {},
        )
        .unwrap();
    assert!(matches!(first.request, Request::ContinuePluginHistory));
    assert!(matches!(
        deferred.as_ref().map(|queued| &queued.request),
        Some(Request::Undo)
    ));
    assert!(matches!(rx.try_recv().unwrap().request, Request::Redo));
}

#[test]
fn stopped_gesture_graph_waits_for_callback_then_wakes_before_fifo() {
    use crate::device_gesture::{GestureKey, GestureRequest, PluginParameterValue};
    use crate::service::audio_sink::WorkerEvents;
    use crate::service::request_cancellation::QueuedRequest;
    use std::sync::mpsc;

    let files = sampler_export::tests::Files::new(false);
    let session = interactive_sampler_session(&files);
    let before = session.project().clone();
    let mut sink = QueuedSink {
        inner: OfflineSink {
            observed: Arc::new(Observed::default()),
            pending: None,
        },
        terminal: ResidentPublicationTerminal::Pending,
        finished: Vec::new(),
        offline: false,
    };
    let context = hex_engine::AudioSourceContext {
        project_generation: 0,
        editor_revision: 3,
    };
    let current = session
        .resident_seek_inputs(sampler_export::Admission::default(), None, None)
        .unwrap()
        .prepare(
            session.project(),
            &mut sink,
            &session.media_bank(),
            session.sine_table(),
            &session.audio_transforms,
            &session.audio_clip_waveforms,
            context,
        )
        .unwrap();
    let recovery = session
        .stage_stopped_plugin_recovery(
            context,
            &mut sink,
            sampler_export::Admission::default(),
            Some(&current),
        )
        .unwrap();
    assert_eq!(
        recovery.terminal(&mut sink),
        ResidentPublicationTerminal::Pending
    );
    assert_eq!(recovery.receipt.unwrap().context, context);
    assert_eq!(session.project(), &before);
    sink.terminal = ResidentPublicationTerminal::Applied;
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    let mut pending = crate::service::PendingPluginGesture {
        request: GestureRequest::ChangePlugin {
            key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        },
        context,
        document_intent: 0,
        fifo_completion: true,
        rebase: false,
        barrier: false,
        preview: None,
        publication: None,
        stopped_recovery: Some(recovery),
        cancellation_requested: false,
        explicit_cancel_requested: false,
    };
    let (event_tx, _event_rx) = mpsc::sync_channel(2);
    let mut events = WorkerEvents::new(
        event_tx,
        Arc::new(AtomicBool::new(false)),
        Some(Box::new(sink)),
    );
    let (tx, rx) = mpsc::sync_channel(2);
    let intent = Arc::new(std::sync::atomic::AtomicU64::new(0));
    tx.send(QueuedRequest::continuation(Request::Undo, &intent))
        .unwrap();
    let (_terminal_tx, terminal_rx) = mpsc::sync_channel(1);
    let mailbox = std::sync::Mutex::new(None);
    let mut awaiting = None;
    let mut deferred = None;
    let first = events
        .next_request_with_control_poll(
            &rx,
            &intent,
            &mailbox,
            &terminal_rx,
            &mut awaiting,
            None,
            None,
            &mut deferred,
            true,
            None,
            None,
            Some(&mut pending),
            None,
            None,
            context,
            |_| {},
        )
        .unwrap();
    assert!(matches!(
        first.request,
        Request::ContinuePluginParameterGesture
    ));
    assert!(matches!(
        deferred.as_ref().map(|queued| &queued.request),
        Some(Request::Undo)
    ));
    assert_eq!(session.project(), &before);
}
