//! Actual default native consumer in explicit verified mode. Only output creation
//! is replaced by the existing headless backend; reader/page/publication stay real.
use super::*;
use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};

fn actual_native_harness() -> Harness {
    let backend = Arc::new(Mutex::new(None));
    let factory_backend = Arc::clone(&backend);
    let (events, receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(events);
    assert!(sink.resident_publication_enabled());
    sink.output_factory = Some(Box::new(move |rate| {
        let mut slot = factory_backend.lock().unwrap();
        if slot.is_some() {
            return Ok(None);
        }
        match hex_audio::worker_fixture::open(rate.unwrap_or(48_000)) {
            Ok((output, callback)) => {
                *slot = Some(callback);
                Ok(Some(output))
            }
            Err(error) if error == "audio output is open or still shutting down" => Ok(None),
            Err(error) => Err(error),
        }
    }));
    let observation = Arc::new(Mutex::new(Observation::default()));
    let service = Service::with_audio_sink(
        16,
        0,
        None,
        Box::new(ObservedSink::rendered(sink, Arc::clone(&observation))),
    )
    .unwrap();
    Harness {
        rendered_setup: None,
        service: Some(service),
        events: receiver,
        backend,
        audio: None,
        context: None,
        observation,
        wait: Duration::from_secs(10),
    }
}

#[test]
fn verified_held_track_gain_zero_to_positive_reaches_callback_before_commit() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let path = files.wav(48_000, 480_000);
    let mut h = actual_native_harness();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(path));
    let source = imported.state.canonical_audio_source.as_ref().unwrap();
    assert!(matches!(
        source.required_sources().next().unwrap().1,
        AudioSourceBinding::VerifiedFile(_)
    ));
    let track = imported.state.project.tracks()[0].id();
    let mut canonical = h.run(Request::Edit(hex_project::Command::Gain {
        track,
        gain: 0.0,
    }));
    let applied = h.applied(h.context.unwrap());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        assert!(h.render().iter().flatten().all(|sample| *sample == 0.0));
    }
    let key = GestureKey {
        generation: applied.context.project_generation,
        token: 1,
    };
    let began = h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::TrackGain { track },
    }));
    assert!(
        began.gesture_ack.as_ref().unwrap().accepted,
        "{:?}",
        began.gesture_ack
    );
    let preview = h.run(Request::DeviceGesture(GestureRequest::Change {
        key,
        sequence: 1,
        value: ParameterValue::Float(1.0),
    }));
    assert!(
        preview.gesture_ack.as_ref().unwrap().accepted,
        "{:?}",
        preview.gesture_ack
    );
    assert!(
        preview.playback.is_none(),
        "verified sources never use resident fallback"
    );
    assert_eq!(preview.state.project, canonical.state.project);
    assert_eq!(preview.state.revision, canonical.state.revision);
    assert_eq!(preview.state.unsaved, canonical.state.unsaved);
    let mut applied = wait_gain(&h, applied.audio_revision, 1.0);
    let initial_preview = h
        .observation
        .lock()
        .unwrap()
        .received_verified_gain
        .clone()
        .unwrap();
    assert_eq!(
        initial_preview.preview_identity(),
        Some(preview.device_preview.as_ref().unwrap().identity)
    );
    for (sequence, gain) in [(2, 0.0), (3, 2.0)] {
        let changed = h.run(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence,
            value: ParameterValue::Float(gain),
        }));
        assert!(changed.gesture_ack.as_ref().unwrap().accepted);
        assert_eq!(changed.state.project, canonical.state.project);
        assert_eq!(changed.state.revision, canonical.state.revision);
        applied = wait_gain(&h, applied.audio_revision, gain);
    }
    // Fill native publication capacity while the callback is deliberately idle.
    // Logical changes stay bounded/coalesced; the final absolute value must win.
    for sequence in 4..=35 {
        let changed = h.run(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence,
            value: ParameterValue::Float(sequence as f32 / 64.0),
        }));
        assert!(changed.gesture_ack.as_ref().unwrap().accepted);
        assert_eq!(changed.state.project, canonical.state.project);
    }
    let final_gain = 35.0 / 64.0;
    applied = wait_gain(&h, applied.audio_revision, final_gain);
    canonical = h.run(Request::Edit(hex_project::Command::RenameTrack {
        track,
        name: "rebased source".into(),
    }));
    assert_eq!(canonical.state.project.track(track).unwrap().gain(), 0.0);
    assert_eq!(
        canonical
            .device_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .track(track)
            .unwrap()
            .gain(),
        final_gain
    );
    applied = wait_gain(&h, applied.audio_revision, final_gain);
    let old_preview = h
        .observation
        .lock()
        .unwrap()
        .received_verified_gain
        .clone()
        .unwrap();
    let cancelled = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    assert!(cancelled.gesture_ack.as_ref().unwrap().accepted);
    assert_eq!(cancelled.state.project, canonical.state.project);
    assert_eq!(cancelled.state.can_undo, canonical.state.can_undo);
    applied = wait_gain(&h, applied.audio_revision, 0.0);
    let old_terminal = h
        .observation
        .lock()
        .unwrap()
        .received_verified_gain
        .clone()
        .unwrap();
    h.observation.lock().unwrap().replay_verified_gain = Some(Arc::clone(&old_preview));
    stable_gain(&h, applied, 0.0);

    let second = GestureKey { token: 2, ..key };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key: second,
        target: GestureTarget::TrackGain { track },
    }));
    let changed = h.run(Request::DeviceGesture(GestureRequest::Change {
        key: second,
        sequence: 1,
        value: ParameterValue::Float(2.0),
    }));
    assert!(changed.gesture_ack.as_ref().unwrap().accepted);
    applied = wait_gain(&h, applied.audio_revision, 2.0);
    h.observation.lock().unwrap().replay_verified_gain = Some(old_terminal);
    stable_gain(&h, applied, 2.0);
    let committed = h.run(Request::DeviceGesture(GestureRequest::Commit {
        key: second,
        sequence: 1,
        value: ParameterValue::Float(2.0),
    }));
    assert!(committed.gesture_ack.as_ref().unwrap().accepted);
    assert_eq!(committed.state.revision, canonical.state.revision + 1);
    assert_eq!(committed.state.project.track(track).unwrap().gain(), 2.0);
    applied = wait_gain(&h, applied.audio_revision, 2.0);
    let undo = h.run(Request::Undo);
    assert_eq!(undo.state.project, canonical.state.project);
    applied = wait_gain(&h, applied.audio_revision, 0.0);
    h.run(Request::Redo);
    applied = wait_gain(&h, applied.audio_revision, 2.0);
    assert_eq!(applied.context, h.context.unwrap());

    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    let empty = h.applied(h.context.unwrap());
    h.observation.lock().unwrap().replay_verified_gain = Some(old_preview);
    stable_gain(&h, empty, 0.0);
}

fn wait_gain(h: &Harness, after: u64, gain: f32) -> hex_audio::prepared::AppliedAudioSource {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let before = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire);
        let pcm = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(
            status.position.load(Acquire) >= before,
            "gain must not reset the musical clock"
        );
        if let Some(actual) = status.applied_audio_source()
            && actual.context == h.context.unwrap()
            && actual.audio_revision > after
            && pcm
                .iter()
                .flatten()
                .all(|sample| (*sample - 0.25 * gain).abs() < 0.0001)
        {
            return actual;
        }
        assert!(
            Instant::now() < deadline,
            "absolute gain {gain} did not reach literal verified PCM"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn stable_gain(h: &Harness, applied: hex_audio::prepared::AppliedAudioSource, gain: f32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut frames = 0;
    while frames < 20 * 64 {
        let pcm = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let committed = if status.stream_buffering.load(Acquire) {
            usize::try_from(status.stream_committed_frames.load(Acquire)).unwrap()
        } else {
            pcm.len()
        };
        assert!(committed <= pcm.len());
        // This headless producer can outrun the page worker. Validate its exact
        // committed prefix and explicit BUFFERING silence, never count missing
        // pages as successful target PCM or as physical-device timing evidence.
        for (index, frame) in pcm.iter().enumerate() {
            let expected = if index < committed { 0.25 * gain } else { 0.0 };
            assert!(
                frame
                    .iter()
                    .all(|sample| (*sample - expected).abs() < 0.0001),
                "gain {gain}, committed={committed}, frame={index}: {frame:?}"
            );
        }
        assert_eq!(status.applied_audio_source(), Some(applied));
        frames += committed;
        assert!(
            Instant::now() < deadline,
            "verified page worker made no sustained progress"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
