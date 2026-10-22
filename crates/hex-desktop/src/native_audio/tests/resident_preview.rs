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

pub(super) struct ResidentHarness {
    pub(super) service: Service,
    events: mpsc::Receiver<EditorAudioEvent>,
    callback: Arc<Mutex<Option<HeadlessBackend>>>,
    pub(super) audio: Option<EditorAudio>,
    pub(super) submitted: Option<(u64, ResidentPublicationMetadata)>,
    pub(super) refuse: Arc<AtomicBool>,
}
impl ResidentHarness {
    pub(super) fn new() -> Self {
        Self::with_frame_limit(8192)
    }
    pub(super) fn with_frame_limit(frame_limit: u64) -> Self {
        let (sender, events) = mpsc::sync_channel(4);
        let callback = Arc::new(Mutex::new(None));
        let target = Arc::clone(&callback);
        let mut inner = NativeAudioSink::new(sender);
        inner.output_factory = Some(Box::new(move |_| {
            // The prior fixture's async native carrier can still own the exclusive Lease
            // after its control owner has returned. Retry exactly as the existing Harness does.
            let (mut output, backend) = match hex_audio::worker_fixture::open(48_000) {
                Ok(pair) => pair,
                Err(error) if error == "audio output is open or still shutting down" => {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
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
            frame_limit,
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
    pub(super) fn next(&mut self) -> Event {
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
    pub(super) fn run(&mut self, request: Request) -> Event {
        self.service.submit(request).unwrap();
        let event = self.next();
        assert!(event.error.is_none(), "{:?}", event.error);
        assert!(
            event.playback.is_none(),
            "native graph must stay worker-private"
        );
        event
    }
    pub(super) fn render(&self) -> [hex_engine::Frame; 64] {
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
    pub(super) fn applied(&mut self, after: u64) -> (u64, ResidentPublicationMetadata) {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            // Stop advancing once the callback has installed the successor. Scalar UI delivery
            // may lag independently; spinning extra audio blocks here can exhaust short media
            // before its literal PCM oracle, without indicating a renderer failure.
            if self
                .audio
                .as_ref()
                .and_then(EditorAudio::status)
                .is_none_or(|status| status.applied_revision.load(Acquire) <= after)
            {
                self.render();
            }
            if let Some((revision, reason)) = self
                .audio
                .as_ref()
                .and_then(EditorAudio::status)
                .and_then(|status| status.rejected_revision())
                .filter(|(revision, _)| *revision > after)
            {
                panic!(
                    "native graph rejected after {after}: revision={revision}, reason={reason:?}, submitted={:?}",
                    self.submitted
                );
            }
            if let Some((revision, metadata)) =
                self.submitted.filter(|(revision, _)| *revision > after)
                && let Some(audio) = self.audio.as_ref()
                && audio.status().unwrap().applied_revision.load(Acquire) == revision
            {
                return (revision, metadata);
            }
            assert!(
                Instant::now() < end,
                "exact native ACK timed out: after={after}, submitted={:?}, actual={:?}",
                self.submitted,
                self.audio.as_ref().and_then(EditorAudio::status).map(|s| (
                    s.applied_revision.load(Acquire),
                    s.position.load(Acquire),
                    s.render_fault.load(Acquire)
                ))
            );
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

impl ResidentHarness {
    pub(super) fn seek_intent(
        &self,
        request_id: u64,
        revision: u64,
        device_preview: Option<hex_session::service::DevicePreviewIdentity>,
        tuning_preview: Option<hex_session::service::TuningPreviewIdentity>,
    ) -> hex_session::service::ResidentArrangementSeekIntent {
        let control = &self.audio.as_ref().unwrap().control;
        let fence = control.capture_fresh_runtime_fence().unwrap();
        hex_session::service::ResidentArrangementSeekIntent {
            request_id,
            project_generation: 0,
            project_revision: revision,
            destination_frame: 129,
            start_playing: true,
            fence: hex_session::service::ResidentFreshRuntimeFence {
                output_identity: control.output_identity().unwrap(),
                active_audio_revision: fence.expected_active_revision(),
                route_generation: fence.route_state_generation(),
                admitted_command_ticket: fence.admitted_command_ticket(),
            },
            device_preview,
            tuning_preview,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub(super) fn seek_result(&mut self) -> hex_session::service::ResidentArrangementSeekResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            assert!(
                self.service.try_arrangement_seek_result().is_none(),
                "native Playback must not escape through legacy result getter"
            );
            if let Some(result) = self.service.try_resident_arrangement_seek_result() {
                return result;
            }
            assert!(Instant::now() < deadline, "seek result timed out");
            std::thread::yield_now();
        }
    }
}

#[test]
fn loaded_resident_tuning_and_preview_seek_use_actual_sources_and_keep_refused_overlay() {
    use hex_project::{BeatTime, Command, Created};
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};
    use hex_session::tuning_gesture::{TuningGestureKey, TuningGestureRequest};
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::new();
    assert!(h.next().error.is_none());
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
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    (applied, _) = h.applied(applied);
    let canonical = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    }));
    (applied, _) = h.applied(applied);
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
    (applied, _) = h.applied(applied);
    let identity = preview.device_preview.as_ref().unwrap().identity;

    // An explicit input request during a live overlay must not publish the canonical gain.
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 127, 1))).unwrap();
    let keyboard = h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: canonical.state.revision,
        request_id: 9,
        track,
        input: Arc::clone(&input),
    });
    let (keyboard_audio, metadata) = h.applied(applied);
    assert_eq!(metadata.device_preview, Some(identity));
    assert_eq!(metadata.computer_keyboard_request, Some(9));
    assert_eq!(keyboard.device_preview.as_ref().unwrap().identity, identity);
    assert_eq!(
        h.render(),
        [[0.125; 2]; 64],
        "original PCM 8192/32768 × preview gain0.5"
    );
    // A refused replacement input must not cancel the installed client or advance its selection.
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    let rejected_input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    h.service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: canonical.state.revision,
            request_id: 11,
            track,
            input: Arc::clone(&rejected_input),
        })
        .unwrap();
    let refused_input = h.next();
    assert!(
        refused_input
            .error
            .as_ref()
            .unwrap()
            .contains("final admission refusal")
    );
    assert!(rejected_input.is_cancelled());
    assert!(!input.is_cancelled());
    assert_eq!(
        refused_input.device_preview.as_ref().unwrap().identity,
        identity
    );
    assert_eq!(h.render(), [[0.125; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    input.set_key(0, None).unwrap();
    h.render();

    let intent = h.seek_intent(1, canonical.state.revision, Some(identity), None);
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    let seek = h.seek_result();
    let ResidentPublicationAcceptance::Submitted {
        output_identity,
        audio_revision,
    } = seek.result.unwrap()
    else {
        panic!("actual native submission")
    };
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(output_identity)
    );
    let (seek_audio, metadata) = h.applied(keyboard_audio);
    assert_eq!(seek_audio, audio_revision);
    assert_eq!(metadata.device_preview, Some(identity));
    assert_eq!(metadata.seek_request, Some(1));
    assert_eq!(
        h.render(),
        [[0.125; 2]; 64],
        "nonzero preroll must use the actual source-bearing overlay"
    );
    input.set_key(0, Some((60, 127, 2))).unwrap();
    assert_eq!(
        h.render(),
        [[0.25; 2]; 64],
        "seek attaches the same live monitor only after historical preroll"
    );
    input.set_key(0, None).unwrap();
    h.render();

    let mut stale = identity;
    stale.sequence += 1;
    h.service
        .submit_resident_arrangement_seek(h.seek_intent(
            2,
            canonical.state.revision,
            Some(stale),
            None,
        ))
        .unwrap();
    assert!(h.next().error.as_ref().unwrap().contains("preview changed"));
    assert!(
        h.seek_result()
            .result
            .unwrap_err()
            .contains("preview changed")
    );
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        seek_audio
    );
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit_resident_arrangement_seek(h.seek_intent(
            3,
            canonical.state.revision,
            Some(identity),
            None,
        ))
        .unwrap();
    assert!(h.next().error.is_none());
    assert!(
        h.seek_result()
            .result
            .unwrap_err()
            .contains("final admission refusal")
    );
    assert_eq!(h.render(), [[0.125; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    // Terminal input cancellation changes physical input words, not the accepted selection
    // version. The first seek must prune that still-captured binding on acceptance; the actual
    // Service post-drive mirror must let the next seek capture the same updated preview version.
    // Cancel before dispatch so this owning regression needs no worker scheduling race or delay.
    input.cancel();
    let mut last_seek_audio = seek_audio;
    for request_id in [4, 5] {
        h.service
            .submit_resident_arrangement_seek(h.seek_intent(
                request_id,
                canonical.state.revision,
                Some(identity),
                None,
            ))
            .unwrap();
        assert!(h.next().error.is_none());
        let result = h.seek_result();
        let ResidentPublicationAcceptance::Submitted { audio_revision, .. } =
            result.result.unwrap()
        else {
            panic!("native repeated preview seek")
        };
        let (actual, metadata) = h.applied(last_seek_audio);
        assert_eq!(actual, audio_revision);
        assert_eq!(metadata.device_preview, Some(identity));
        assert_eq!(metadata.seek_request, Some(request_id));
        assert_eq!(h.render(), [[0.125; 2]; 64]);
        last_seek_audio = actual;
    }
    h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    (applied, _) = h.applied(last_seek_audio);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    h.render();

    let key = TuningGestureKey {
        generation: 0,
        token: 1,
    };
    let tuned = super::sampler::tuning(880.0);
    h.run(Request::TuningGesture(TuningGestureRequest::Begin { key }));
    let preview = h.run(Request::TuningGesture(TuningGestureRequest::Change {
        key,
        sequence: 1,
        tuning: tuned,
    }));
    (applied, _) = h.applied(applied);
    let identity = preview.tuning_preview.as_ref().unwrap().identity;
    assert_eq!(
        preview
            .tuning_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .tuning(),
        tuned
    );
    let next = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    next.set_key(0, Some((60, 127, 1))).unwrap();
    let prepared = h.run(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: canonical.state.revision,
        request_id: 10,
        track,
        input: Arc::clone(&next),
    });
    let (next_applied, metadata) = h.applied(applied);
    applied = next_applied;
    assert_eq!(metadata.tuning_preview, Some(identity));
    assert_eq!(prepared.tuning_preview.as_ref().unwrap().identity, identity);
    assert!(!next.is_cancelled());
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(Request::TuningGesture(TuningGestureRequest::Commit {
            key,
            sequence: 2,
            tuning: super::sampler::tuning(660.0),
        }))
        .unwrap();
    let refused = h.next();
    assert!(
        refused
            .tuning_gesture_ack
            .as_ref()
            .unwrap()
            .preview_retained
    );
    assert_eq!(refused.active_tuning_gesture, preview.active_tuning_gesture);
    assert_eq!(refused.tuning_preview.as_ref().unwrap().identity, identity);
    assert_eq!(refused.state.project, canonical.state.project);
    assert_eq!(refused.state.revision, canonical.state.revision);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied
    );
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let cancelled = h.run(Request::TuningGesture(TuningGestureRequest::Cancel { key }));
    let (_, metadata) = h.applied(applied);
    assert!(metadata.tuning_preview.is_none());
    assert!(cancelled.active_tuning_gesture.is_none());
    assert!(cancelled.tuning_preview.is_none());
    assert_eq!(cancelled.state.project, canonical.state.project);
    assert_eq!(cancelled.state.revision, canonical.state.revision);
    assert_eq!(h.render(), [[0.25; 2]; 64]);
    assert!(loaded.playback.is_none());
}
