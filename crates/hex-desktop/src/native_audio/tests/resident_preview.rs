//! Exercise the opt-in consumer before production cutover, with only native output creation
//! substituted. Session transactions, prospective admission, publication and callback stay real.
use super::*;
use hex_engine::{
    PreparedPlaybackObservation,
    native_device_resources::{DeviceResourceAdmission, PreparedDeviceResources},
    playback::Playback,
};
use hex_session::service::{
    ResidentPublicationAcceptance, ResidentPublicationMetadata, ResidentPublicationPolicy,
    ResidentPublicationRefusal,
};

struct ResidentSink {
    inner: NativeAudioSink,
    refuse: Arc<AtomicBool>,
}
impl WorkerAudioSink for ResidentSink {
    fn resident_publication_enabled(&self) -> bool {
        true
    }
    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        self.inner.resident_admission()
    }
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Mirror the exact production ownership boundary.
    fn publish_resident(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        let refuse = self.refuse.load(Acquire);
        self.inner.publish_resident(
            playback,
            resources,
            observation,
            metadata,
            policy,
            limit,
            &mut |actual| {
                admit(actual)?;
                if refuse {
                    Err("fixture final admission refusal".into())
                } else {
                    Ok(())
                }
            },
        )
    }
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String> {
        self.inner.initialize(shutdown)
    }
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
        self.inner.canonical_changed(source);
    }
    fn native_sampler_hold(&self) -> hex_session::service::NativeSamplerReservationHold {
        self.inner.native_sampler_hold()
    }
    fn poll(&mut self) {
        self.inner.poll();
    }
    fn close(&mut self) {
        self.inner.close();
    }
}

struct ResidentHarness {
    service: Service,
    events: mpsc::Receiver<EditorAudioEvent>,
    callback: Arc<Mutex<Option<HeadlessBackend>>>,
    audio: Option<EditorAudio>,
    submitted: Option<(u64, ResidentPublicationMetadata)>,
    refuse: Arc<AtomicBool>,
}
impl ResidentHarness {
    fn new() -> Self {
        let (sender, events) = mpsc::sync_channel(4);
        let callback = Arc::new(Mutex::new(None));
        let target = Arc::clone(&callback);
        let mut inner = NativeAudioSink::new(sender);
        inner.output_factory = Some(Box::new(move |_| {
            let (mut output, backend) = hex_audio::worker_fixture::open(48_000)?;
            output
                .publication
                .control()
                .unwrap()
                .configure_device_resource_publication_on_control(
                    AudioPolicy::new()?.resident_native.storage,
                )
                .map_err(|e| format!("{e:?}"))?;
            *target.lock().unwrap() = Some(backend);
            Ok(Some(output))
        }));
        let refuse = Arc::new(AtomicBool::new(false));
        let service = Service::with_audio_sink_and_native_reservation(
            16,
            8192,
            None,
            Box::new(ResidentSink {
                inner,
                refuse: Arc::clone(&refuse),
            }),
            AudioPolicy::new().unwrap().resident_native,
        )
        .unwrap();
        Self {
            service,
            events,
            callback,
            audio: None,
            submitted: None,
            refuse,
        }
    }
    fn pump(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                EditorAudioEvent::Opened { audio, .. } => self.audio = Some(audio),
                EditorAudioEvent::Publication {
                    state:
                        EditorPublicationState::ResidentSubmitted {
                            audio_revision,
                            metadata,
                            ..
                        },
                    ..
                } => self.submitted = Some((audio_revision, metadata)),
                EditorAudioEvent::Unavailable { error, .. } => panic!("{error}"),
                _ => {}
            }
        }
    }
    fn next(&mut self) -> Event {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            if let Ok(event) = self.service.try_event() {
                return event;
            }
            assert!(Instant::now() < end, "canonical event timed out");
            std::thread::yield_now();
        }
    }
    fn run(&mut self, request: Request) -> Event {
        self.service.submit(request).unwrap();
        let event = self.next();
        assert!(event.error.is_none(), "{:?}", event.error);
        assert!(
            event.playback.is_none(),
            "native graph must stay worker-private"
        );
        event
    }
    fn render(&self) -> [hex_engine::Frame; 64] {
        let mut pcm = [[0.0; 2]; 64];
        let mut slot = self.callback.lock().unwrap();
        let callback = slot.as_mut().unwrap();
        let allocations = allocation_counter::measure(|| callback.render(&mut pcm).unwrap());
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        pcm
    }
    fn applied(&mut self, after: u64) -> (u64, ResidentPublicationMetadata) {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            self.render();
            if let Some((revision, metadata)) =
                self.submitted.filter(|(revision, _)| *revision > after)
                && let Some(audio) = self.audio.as_ref()
                && audio.status().unwrap().applied_revision.load(Acquire) == revision
            {
                return (revision, metadata);
            }
            assert!(Instant::now() < end, "exact native ACK timed out");
            std::thread::yield_now();
        }
    }
}

#[test]
fn loaded_resident_preview_refused_commit_keeps_pcm_and_cancel_restores_canonical() {
    use hex_project::{Command, Created};
    use hex_session::device_gesture::ParameterValue;
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    let initial = h.next();
    assert!(initial.error.is_none(), "{:?}", initial.error);
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    let loaded = h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: files.wav(48_000, 8192),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let device = loaded
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: loaded.state.revision,
        request_id: 2,
        track,
        input: Arc::clone(&input),
    });
    (applied, _) = h.applied(applied);
    input.set_key(0, Some((60, 127, 1))).unwrap();
    assert!(h.render().iter().flatten().any(|sample| *sample != 0.0));
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::TrackGain { track },
    }));
    let preview = h.run(Request::DeviceGesture(GestureRequest::Change {
        key,
        sequence: 1,
        value: ParameterValue::Float(0.5),
    }));
    let (preview_audio, metadata) = h.applied(applied);
    assert_eq!(metadata.device_preview.unwrap().sequence, 1);
    assert_eq!(preview.state.project, loaded.state.project);
    assert_eq!(
        preview
            .device_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .track(track)
            .unwrap()
            .gain(),
        0.5
    );
    assert_eq!(
        preview
            .device_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        device
    );
    let preview_pcm = h.render();
    assert!(preview_pcm.iter().flatten().any(|sample| *sample != 0.0));
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::DeviceGesture(GestureRequest::Commit {
            key,
            sequence: 2,
            value: ParameterValue::Float(0.25),
        }))
        .unwrap();
    let refused = h.next();
    let ack = refused.gesture_ack.as_ref().unwrap();
    assert!(!ack.accepted && ack.preview_retained, "{ack:?}");
    assert_eq!(refused.active_device_gesture, preview.active_device_gesture);
    assert_eq!(refused.state.project, loaded.state.project);
    assert_eq!(refused.state.revision, loaded.state.revision);
    assert_eq!(refused.state.can_undo, loaded.state.can_undo);
    assert_eq!(
        refused.device_preview.as_ref().unwrap().identity,
        preview.device_preview.as_ref().unwrap().identity
    );
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        preview_audio
    );
    assert_eq!(
        h.render(),
        preview_pcm,
        "refused commit must preserve actual audible preview"
    );
    h.service
        .submit(Request::DeviceGesture(GestureRequest::Cancel { key }))
        .unwrap();
    let refused_cancel = h.next();
    assert!(refused_cancel.gesture_ack.unwrap().preview_retained);
    assert_eq!(
        refused_cancel.active_device_gesture,
        preview.active_device_gesture
    );
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let cancelled = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    let (_, metadata) = h.applied(preview_audio);
    assert!(metadata.device_preview.is_none());
    assert!(cancelled.active_device_gesture.is_none());
    assert!(cancelled.device_preview.is_none());
    assert_eq!(cancelled.state.project, loaded.state.project);
    assert_eq!(cancelled.state.revision, loaded.state.revision);
    assert_eq!(
        h.render(),
        preview_pcm.map(|frame| frame.map(|sample| sample * 2.0))
    );
    // Both an untouched Commit and a preview returned to its original value publish a new
    // audio revision in the SAME editor context, without fabricating canonical history.
    for (token, changed) in [(2, false), (3, true)] {
        let after = h.submitted.unwrap().0;
        let key = GestureKey {
            generation: 0,
            token,
        };
        h.run(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }));
        let mut last = after;
        if changed {
            h.run(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(0.25),
            }));
            (last, _) = h.applied(after);
        }
        let committed = h.run(Request::DeviceGesture(GestureRequest::Commit {
            key,
            sequence: 2,
            value: ParameterValue::Float(1.0),
        }));
        let (_, metadata) = h.applied(last);
        assert_eq!(metadata.context.editor_revision, loaded.state.revision);
        assert!(metadata.device_preview.is_none());
        assert!(committed.active_device_gesture.is_none());
        assert!(committed.device_preview.is_none());
        assert_eq!(committed.state.project, loaded.state.project);
        assert_eq!(committed.state.revision, loaded.state.revision);
        assert_eq!(committed.state.can_undo, loaded.state.can_undo);
        assert_eq!(committed.state.can_redo, loaded.state.can_redo);
    }
}
