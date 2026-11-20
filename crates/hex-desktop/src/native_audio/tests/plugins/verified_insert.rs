//! Signed hosted insertion on the production verified-file publication path.
use super::*;
use hex_audio::audio_diagnostics::{AudioDiagnosticKind, AudioDiagnosticsFlags};
use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_insert_keeps_the_eq_gesture_commit() {
    let _native = native_fixture();
    let files = Files::new();
    let (reached, facts) = mpsc::sync_channel(1);
    let (resume, resumed) = mpsc::sync_channel(1);
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.facts_gate = Some(crate::native_audio::plugins::FactsGate::new(
            reached, resumed,
        ));
    });
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    h.applied(h.context.unwrap());
    let inserted = h.run_verified_device_insert(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::eq::EqState::default().encode().unwrap(),
    });
    let Some(Created::Device(device)) = inserted.created else {
        panic!("fixture EQ")
    };
    let predecessor = h.applied(h.context.unwrap());
    let expect_sample = |h: &Harness, expected: f32| {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut observation = None;
        loop {
            let pcm = h.render();
            for event in h.events.try_iter() {
                if let EditorAudioEvent::Publication { context, state, .. } = event {
                    observation = Some((context, state));
                }
            }
            if (pcm[63][0] - expected).abs() < 1e-5 {
                break;
            }
            if Instant::now() >= deadline {
                let status = h.audio.as_ref().unwrap().status().unwrap();
                panic!(
                    "expected {expected}, rendered {:?}, applied {:?}, rejected {:?}, observation {observation:?}",
                    pcm[63],
                    status.applied_audio_source(),
                    status.rejected_revision()
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    expect_sample(&h, 0.25);
    let input = request(&inserted.state, PluginEffectDestination::Track(track));
    let operation = Arc::clone(&input.control);
    assert!(
        h.run(Request::InsertPluginEffect(input))
            .plugin_effect_complete
            .is_empty()
    );
    facts.recv_timeout(Duration::from_secs(3)).unwrap();

    let parameter = hex_engine::eq::OUTPUT_GAIN;
    let value = ParameterValue::Float(0.5);
    let key = GestureKey {
        generation: predecessor.context.project_generation,
        token: 1,
    };
    let gesture = |h: &mut Harness, request| {
        h.service
            .as_ref()
            .unwrap()
            .submit(Request::DeviceGesture(request))
            .unwrap();
        loop {
            let event = h.next();
            assert!(event.error.is_none(), "{:?}", event.error);
            if event.gesture_ack.is_some() {
                break event;
            }
            assert!(event.plugin_effect_complete.is_empty());
        }
    };
    let begun = gesture(
        &mut h,
        GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter { device, parameter },
        },
    );
    assert!(begun.gesture_ack.unwrap().accepted);
    let changed = gesture(
        &mut h,
        GestureRequest::Change {
            key,
            sequence: 1,
            value,
        },
    );
    assert!(changed.gesture_ack.unwrap().accepted);
    assert_eq!(changed.state.project, inserted.state.project);
    assert!(changed.device_preview.is_some());
    assert!(
        h.observation
            .lock()
            .unwrap()
            .received_verified_device
            .is_some()
    );
    expect_sample(&h, 0.125);
    let committed = gesture(
        &mut h,
        GestureRequest::Commit {
            key,
            sequence: 2,
            manual_edit: true,
            value,
        },
    );
    assert!(committed.gesture_ack.unwrap().accepted);
    assert!(
        committed.plugin_effect_complete.is_empty(),
        "the accepted EQ commit must leave the insertion pending"
    );
    let chosen = h.context.unwrap();
    assert_ne!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .unwrap()
            .context,
        chosen
    );
    resume.send(()).unwrap();
    let applied = h.applied(chosen);
    loop {
        let mut event = h.next_with_audio();
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(offer) = event.device_insert_confirmation.take() {
            assert!(offer.proceed());
        }
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(Arc::ptr_eq(&terminal.control, &operation) && terminal.accepted);
            assert_eq!(event.state.revision, committed.state.revision + 1);
            assert_eq!(
                event.state.project.device(device),
                committed.state.project.device(device)
            );
            assert!(h.applied(h.context.unwrap()).audio_revision > applied.audio_revision);
            break;
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_insert_installs_original_worker_and_retains_it_for_eq_edit() {
    let _native = native_fixture();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let (reached, facts) = mpsc::sync_channel(1);
    let (_resume, resumed) = mpsc::sync_channel(1);
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.progress = Some(Arc::clone(&progress));
        sink.plugins.facts_gate = Some(crate::native_audio::plugins::FactsGate::new(
            reached, resumed,
        ));
    });
    let files = Files::new();
    let send = |h: &Harness, request| h.service.as_ref().unwrap().submit(request).unwrap();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 131_072)));
    let track = imported.state.project.tracks()[0].id();
    h.applied(h.context.unwrap());
    let inserted = h.run_verified_device_insert(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::eq::EqState::default().encode().unwrap(),
    });
    let Some(Created::Device(eq)) = inserted.created else {
        panic!("fixture EQ")
    };
    h.applied(h.context.unwrap());
    let state = inserted.state;
    let destination = PluginEffectDestination::Track(track);
    let mut invalid = request(&state, destination);
    invalid.selection.executable_sha256 = "0".repeat(64);
    let invalid_operation = Arc::clone(&invalid.control);
    send(&h, Request::InsertPluginEffect(invalid));
    let refused = h.next_with_audio();
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, state.project);
    assert_eq!(refused.state.revision, state.revision);
    assert!(Arc::ptr_eq(
        &refused.plugin_effect_complete[0].control,
        &invalid_operation
    ));
    assert!(!refused.plugin_effect_complete[0].accepted);

    let cancelled = request(&state, destination);
    let cancelled_operation = Arc::clone(&cancelled.control);
    send(&h, Request::InsertPluginEffect(cancelled));
    assert!(h.next_with_audio().plugin_effect_complete.is_empty());
    facts.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(cancelled_operation.cancel());
    loop {
        let event = h.next_with_audio();
        assert_eq!(event.state.project, state.project);
        assert_eq!(event.state.revision, state.revision);
        assert!(event.device_insert_complete.is_none());
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(Arc::ptr_eq(&terminal.control, &cancelled_operation));
            assert!(!terminal.accepted);
            break;
        }
    }

    let insert = request(&state, destination);
    let operation = Arc::clone(&insert.control);
    h.audio
        .as_ref()
        .unwrap()
        .control
        .set_audio_diagnostics(AudioDiagnosticsFlags::READINESS);
    send(&h, Request::InsertPluginEffect(insert));
    let accepted = loop {
        let mut event = h.next_with_audio();
        if let Some(error) = event.error.as_ref() {
            let control = &mut h.audio.as_mut().unwrap().control;
            let handoff: Vec<_> = std::iter::from_fn(|| control.take_audio_diagnostic())
                .filter_map(|trace| match trace.kind {
                    AudioDiagnosticKind::StreamPreflightFailed { handoff, .. } => handoff,
                    _ => None,
                })
                .collect();
            panic!("verified insert refused: {error}; handoff={handoff:?}");
        }
        if let Some(offer) = event.device_insert_confirmation.take() {
            assert!(offer.proceed());
        }
        if let Some(terminal) = event
            .plugin_effect_complete
            .iter()
            .find(|terminal| Arc::ptr_eq(&terminal.control, &operation))
        {
            assert!(terminal.accepted);
            break event;
        }
        assert_eq!(event.state.project, state.project);
    };
    let Some(Created::Device(vst)) = accepted.created else {
        panic!("fixture VST3")
    };
    assert_eq!(
        accepted.device_insert_complete.unwrap().outcome,
        hex_session::service::DeviceInsertOutcome::Installed
    );
    let started = h.applied(h.context.unwrap());
    wait(|| progress.lock().unwrap().source_bindings[0].is_some());
    let (device, binding, _) = progress.lock().unwrap().source_bindings[0].unwrap();
    assert_eq!(device, vst.value());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let render = |h: &Harness, block: u64| {
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let start = block * 256;
        assert_eq!(status.performance_frame.load(Acquire), start);
        let mut sample = 0.0;
        wait(|| {
            sample = render_hosted_quantum(h);
            let position = status.performance_frame.load(Acquire);
            assert!(position <= start + 256);
            if position == start {
                std::thread::sleep(Duration::from_millis(1));
            }
            position == start + 256
        });
        wait(|| {
            progress.lock().unwrap().source_bindings[0].is_some_and(
                |(actual_device, actual_binding, count)| {
                    assert_eq!((actual_device, actual_binding), (device, binding));
                    count.is_some_and(|count| count > block)
                },
            )
        });
        sample
    };
    let original = |block: u64| {
        // The verified WAV is 0.25. The original SDK fixture increases gain
        // by 0.125 per block, delivered two physical quanta after input.
        block
            .checked_sub(2)
            .map_or(0.0, |input| 0.25 * (0.5 + input as f32 * 0.125))
    };
    for block in 0..4 {
        let sample = render(&h, block);
        assert!(
            (sample - original(block)).abs() < 1e-5,
            "block {block}: {sample}"
        );
    }

    h.run(Request::SetDeviceParameter {
        device: eq,
        parameter: hex_engine::eq::OUTPUT_GAIN,
        value: ParameterValue::Float(0.5),
    });
    let target = h.context.unwrap();
    let mut first = None;
    for block in 4..32 {
        let sample = render(&h, block);
        let applied = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .unwrap();
        if applied.context == target {
            assert!(applied.audio_revision > started.audio_revision);
            let first = *first.get_or_insert(block);
            // Allow the EQ handoff ramp and the bridge's two queued quanta
            // to finish before comparing steady-state output.
            if block >= first + 5 {
                assert!(
                    (sample - 0.5 * original(block)).abs() < 1e-5,
                    "block {block}: {sample}"
                );
                if block > first + 5 {
                    return;
                }
            }
        }
        // Let the Session publisher prepare and queue the exact successor
        // between deterministic synthetic host callbacks.
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("verified eq successor did not render");
}

fn render_hosted_quantum(h: &Harness) -> f32 {
    let mut block = [[0.0; 2]; 256];
    let mut host = [0.0; 512];
    let mut slot = h.backend.lock().unwrap();
    let backend = slot.as_mut().unwrap();
    let counts = allocation_counter::measure(|| {
        backend.render_native_direct(&mut block, &mut host).unwrap();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0),
    );
    host[510]
}
