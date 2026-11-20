//! Signed hosted graph edits on the production verified-file publication path.
use super::unsealed_admission::{
    HeldVerifiedEq, eq_gesture, expect_verified_sample, hold_verified_eq_preview,
};
use super::*;
use hex_audio::audio_diagnostics::{AudioDiagnosticKind, AudioDiagnosticsFlags};
use hex_session::device_gesture::{GestureKey, GestureRequest, ParameterValue};

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_removal_undo_installs_a_fresh_catalog_worker() {
    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection(), "Fx", 0).unwrap();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.catalog = Some(directory.join("plugin-catalog.json"));
        sink.plugins.progress = Some(Arc::clone(&progress));
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
    let submit = |h: &Harness, request| h.service.as_ref().unwrap().submit(request).unwrap();
    submit(
        &h,
        Request::InsertPluginEffect(request(
            &imported.state,
            PluginEffectDestination::Track(track),
        )),
    );
    let inserted = loop {
        let event = h.next_with_audio();
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(terminal.accepted);
            break event;
        }
    };
    let Some(Created::Device(vst)) = inserted.created else {
        panic!("fixture VST3");
    };
    h.applied(h.context.unwrap());
    wait(|| progress.lock().unwrap().source_bindings[0].is_some());
    let original = progress.lock().unwrap().source_bindings[0].unwrap().1;

    let removed = h.run_verified_device_insert(Command::RemoveDevice(vst));
    assert!(removed.state.project.device(vst).is_none());
    h.applied(h.context.unwrap());
    submit(&h, Request::Undo);
    let undone = loop {
        let event = h.next_with_audio();
        if event.request_complete {
            break event;
        }
    };
    assert!(
        undone.error.is_none(),
        "catalog VST3 Undo refused: {:?}",
        undone.error
    );
    assert_eq!(
        undone.state.project.device(vst),
        inserted.state.project.device(vst)
    );
    h.applied(h.context.unwrap());
    let (_, restored, accepted) = progress.lock().unwrap().newest_source.unwrap();
    assert!(accepted);
    assert_ne!(restored, original, "Undo reused the removed VST3 worker");
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_file_track_removal_keeps_one_compensated_hosted_return_worker() {
    use hex_session::service::DeviceInsertOutcome;

    const QUANTUM: u64 = 256;
    const LATENCY: u64 = 2 * QUANTUM;
    const FADE: u64 = 240;

    let _native = native_fixture();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.progress = Some(Arc::clone(&progress));
    });
    let first = Files::new();
    let second = Files::new();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(first.wav(48_000, 131_072)));
    let removed = imported.state.project.tracks()[0].id();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(second.wav(48_000, 131_072)));
    let kept = imported.state.project.tracks()[1].id();
    h.applied(h.context.unwrap());
    let added = h.run(Request::Edit(Command::AddReturnTrack {
        name: "shared".into(),
    }));
    let Some(Created::ReturnTrack(bus)) = added.created else {
        panic!("shared Return");
    };
    h.applied(h.context.unwrap());
    let mut state = added.state;
    for track in [removed, kept] {
        let sent = h.run(Request::Edit(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.5,
        }));
        assert!(sent.error.is_none(), "{:?}", sent.error);
        state = sent.state;
        h.applied(h.context.unwrap());
    }
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::InsertPluginEffect(request(
            &state,
            PluginEffectDestination::Return(bus),
        )))
        .unwrap();
    let before = loop {
        let event = h.next_with_audio();
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(terminal.accepted);
            break event;
        }
    };
    let Some(Created::Device(device)) = before.created else {
        panic!("Return VST3");
    };
    let predecessor = h.applied(h.context.unwrap());
    wait(|| progress.lock().unwrap().source_bindings[0].is_some());
    let binding = progress.lock().unwrap().source_bindings[0].unwrap().1;
    fn status(h: &Harness) -> &hex_audio::prepared::Status {
        h.audio.as_ref().unwrap().status().unwrap()
    }
    let render = |h: &Harness| {
        let status = status(h);
        let start = status.performance_frame.load(Acquire);
        let mut pcm = [0.0; 512];
        wait(|| {
            pcm = render_hosted_pcm(h);
            let position = status.performance_frame.load(Acquire);
            assert!(position <= start + QUANTUM);
            if position == start {
                std::thread::sleep(Duration::from_millis(1));
            }
            position == start + QUANTUM
        });
        wait(|| {
            let (actual_device, actual_binding, blocks) =
                progress.lock().unwrap().source_bindings[0].unwrap();
            assert_eq!((actual_device, actual_binding), (device.value(), binding));
            assert!(blocks.unwrap_or(0) <= start / QUANTUM + 1);
            blocks.is_some_and(|blocks| blocks == start / QUANTUM + 1)
        });
        (start, pcm)
    };
    let expected = |input: u64, removed_gain: f32| {
        let sum = 0.25 + 0.25 * removed_gain;
        let effect_gain = 0.5 + (input / QUANTUM) as f32 * 0.125;
        sum + sum * 0.5 * effect_gain
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        let (start, pcm) = render(&h);
        let sample = start
            .checked_sub(LATENCY)
            .map_or(0.0, |input| expected(input, 1.0));
        assert!(pcm.iter().all(|value| (*value - sample).abs() < 1e-5));
    }
    let pending = h.run(Request::Edit(Command::RemoveTrack(removed)));
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, before.state.project);
    wait(|| status(&h).publication_in_flight());

    let mut fade_start = None;
    let mut successor = None;
    for _ in 0..12 {
        let (start, pcm) = render(&h);
        for (offset, frame) in pcm.chunks_exact(2).enumerate() {
            let input = start + offset as u64 - LATENCY;
            if fade_start.is_none() && (frame[0] - expected(input, 1.0)).abs() > 1e-5 {
                assert_eq!(
                    offset, 0,
                    "the callback starts the fade at a block boundary"
                );
                fade_start = Some(input);
            }
            let gain = fade_start.map_or(1.0, |first| {
                FADE.saturating_sub(input - first + 1) as f32 / FADE as f32
            });
            let sample = expected(input, gain);
            assert!(
                frame.iter().all(|value| (*value - sample).abs() < 1e-5),
                "input frame {input}: {frame:?} vs {sample}"
            );
        }
        let applied = status(&h).applied_audio_source().unwrap();
        if applied != predecessor {
            let fade_start = fade_start.expect("drained track fade");
            assert!(start >= fade_start + LATENCY + FADE - 1);
            assert_eq!(applied.output_identity, predecessor.output_identity);
            successor = Some(applied);
            break;
        }
    }
    let successor = successor.expect("exact successor");
    let completed = h.next();
    assert!(completed.request_complete && completed.error.is_none());
    assert!(completed.device_insert_confirmation.is_none());
    let receipt = completed.device_insert_complete.unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(receipt.submission.audio_revision, successor.audio_revision);
    assert_eq!(completed.state.revision, before.state.revision + 1);
    assert!(completed.state.project.track(removed).is_none());
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_removal_fades_and_resumes_on_the_same_output() {
    use hex_session::service::{DeviceInsertOutcome, GraphEditOperation};

    const FADE: usize = 128;
    const QUANTUM: u64 = 256;

    let _native = native_fixture();
    let progress = Arc::new(Mutex::new(
        crate::native_audio::plugins::PreparationObservation::default(),
    ));
    let mut h = super::super::verified_gain::native_harness_with(|sink| {
        sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
        sink.plugins.progress = Some(Arc::clone(&progress));
    });
    let files = Files::new();
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
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::InsertPluginEffect(request(
            &imported.state,
            PluginEffectDestination::Track(track),
        )))
        .unwrap();
    let inserted = loop {
        let event = h.next_with_audio();
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(terminal.accepted);
            break event;
        }
    };
    let Some(Created::Device(vst)) = inserted.created else {
        panic!("fixture VST3");
    };
    let predecessor = h.applied(h.context.unwrap());
    fn status(h: &Harness) -> &hex_audio::prepared::Status {
        h.audio.as_ref().unwrap().status().unwrap()
    }
    wait(|| progress.lock().unwrap().source_bindings[0].is_some());
    let binding = progress.lock().unwrap().source_bindings[0].unwrap().1;
    let observe = || {
        let observation = progress.lock().unwrap();
        let (device, current, blocks) = observation.source_bindings[0].unwrap();
        assert_eq!((device, current), (vst.value(), binding));
        blocks.unwrap_or(0)
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let original_sample = |frame: u64| {
        // The signed effect steps its gain per quantum and has a two-quantum bridge.
        (frame / QUANTUM)
            .checked_sub(2)
            .map_or(0.0, |input| 0.25 * (0.5 + input as f32 * 0.125))
    };
    let play_original = |h: &Harness| {
        let status = status(h);
        let start = status.performance_frame.load(Acquire);
        let mut pcm = [0.0; 512];
        wait(|| {
            pcm = render_hosted_pcm(h);
            let current = status.performance_frame.load(Acquire);
            if current == start {
                std::thread::sleep(Duration::from_millis(1));
            }
            current > start
        });
        assert_eq!(status.performance_frame.load(Acquire), start + QUANTUM);
        wait(|| observe() > start / QUANTUM);
        let sample = original_sample(start);
        assert!(pcm.iter().all(|value| (*value - sample).abs() < 1e-5));
    };
    for _ in 0..4 {
        play_original(&h);
    }
    for proceed in [false, true] {
        let pending = h.run(Request::Edit(Command::RemoveDevice(vst)));
        assert!(!pending.request_complete);
        assert_eq!(pending.state.project, inserted.state.project);
        wait(|| status(&h).publication_in_flight());
        play_original(&h);
        let proposed = h.next();
        let offer = proposed
            .device_insert_confirmation
            .expect("exact removal offer");
        assert_eq!(offer.operation(), GraphEditOperation::RemoveDevice);
        assert_eq!(
            offer.submission().output_identity,
            predecessor.output_identity
        );
        assert_eq!(proposed.state.project, inserted.state.project);
        assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
        play_original(&h);
        if !proceed {
            assert!(offer.cancel());
            let cancelled = h.next();
            assert_eq!(cancelled.state.project, inserted.state.project);
            assert_eq!(
                cancelled.device_insert_complete.unwrap().outcome,
                DeviceInsertOutcome::Cancelled
            );
            play_original(&h);
            continue;
        }
        assert!(offer.proceed());
        wait(|| status(&h).publication_in_flight());
        let deadline = Instant::now() + Duration::from_millis(300);
        let (cut_start, faded) = loop {
            let start = status(&h).position.load(Acquire);
            let faded = render_hosted_pcm(&h);
            let position = status(&h).position.load(Acquire);
            if position == start + FADE as u64 {
                break (start, faded);
            }
            assert_eq!(
                position,
                start + QUANTUM,
                "while waiting for the approved cut"
            );
            assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
            let sample = original_sample(start);
            assert!(faded.iter().all(|value| (*value - sample).abs() < 1e-5));
            wait(|| observe() > start / QUANTUM);
            assert!(Instant::now() < deadline, "approved cut did not start");
        };
        assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
        let sample = original_sample(cut_start);
        for (index, frame) in faded.chunks_exact(2).enumerate() {
            let expected = sample * FADE.saturating_sub(index + 1) as f32 / FADE as f32;
            assert!(frame.iter().all(|value| (*value - expected).abs() < 1e-5));
        }
        let resumed = render_hosted_pcm(&h);
        assert_eq!(
            status(&h).position.load(Acquire),
            cut_start + FADE as u64 + QUANTUM
        );
        for (index, frame) in resumed.chunks_exact(2).enumerate() {
            let expected = 0.25 * (index + 1).min(FADE) as f32 / FADE as f32;
            assert!(frame.iter().all(|value| (*value - expected).abs() < 1e-5));
        }
        let removed = h.next();
        assert!(removed.error.is_none(), "{:?}", removed.error);
        let receipt = removed.device_insert_complete.unwrap();
        assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
        assert!(removed.state.project.device(vst).is_none());
        assert_eq!(removed.state.revision, inserted.state.revision + 1);
        let active = status(&h).applied_audio_source().unwrap();
        assert_eq!(active.context, h.context.unwrap());
        assert_eq!(active.audio_revision, receipt.submission.audio_revision);
        assert_eq!(active.output_identity, predecessor.output_identity);
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_insert_keeps_the_eq_gesture_commit() {
    let _native = native_fixture();
    let files = Files::new();
    let HeldVerifiedEq {
        mut h,
        operation,
        device,
        key,
        resume_facts,
        progress,
        ..
    } = hold_verified_eq_preview(&files);
    let committed = commit_held_eq(&mut h, key, device);
    let chosen = h.context.unwrap();
    resume_facts.send(()).unwrap();
    wait(|| {
        let observed = progress.lock().unwrap();
        observed.facts_ready_for_session || observed.facts_attempt_from_session
    });
    h.run(Request::QueryDeviceAutomationCloseSafety { request_id: 27 });
    assert!(
        !progress.lock().unwrap().facts_attempt_from_session,
        "native facts must wait for the exact canonical EQ receipt"
    );
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let applied = loop {
        render_hosted_quantum(&h);
        if let Some(source) = status.applied_audio_source()
            && source.context == chosen
        {
            break source;
        }
        assert!(Instant::now() < deadline, "canonical EQ was not applied");
        std::thread::sleep(Duration::from_millis(1));
    };
    loop {
        let Ok(mut event) = h.service.as_ref().unwrap().try_event() else {
            render_hosted_quantum(&h);
            assert!(Instant::now() < deadline, "VST3 insertion did not finish");
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
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
            let installed = status.applied_audio_source().unwrap();
            assert_eq!(
                installed.context,
                event
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context()
            );
            assert!(installed.audio_revision > applied.audio_revision);
            break;
        }
    }
}

fn commit_held_eq(h: &mut Harness, key: GestureKey, device: hex_project::DeviceId) -> Event {
    let committed = eq_gesture(
        h,
        GestureRequest::Commit {
            key,
            sequence: 2,
            manual_edit: true,
            value: ParameterValue::Float(0.5),
        },
    );
    assert!(committed.gesture_ack.as_ref().unwrap().accepted);
    assert!(committed.plugin_effect_complete.is_empty());
    assert_eq!(
        hex_engine::native_device_parameter(
            committed.state.project.device(device).unwrap().state(),
            hex_engine::eq::OUTPUT_GAIN,
        )
        .unwrap(),
        ParameterValue::Float(0.5),
    );
    assert_ne!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_audio_source()
            .unwrap()
            .context,
        h.context.unwrap(),
        "the canonical EQ callback has not run",
    );
    committed
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn verified_hosted_insert_cancel_and_fault_preserve_the_accepted_eq() {
    let _native = native_fixture();
    for fault in [false, true] {
        let files = Files::new();
        let HeldVerifiedEq {
            mut h,
            operation,
            device,
            key,
            resume_facts: _hold,
            ..
        } = hold_verified_eq_preview(&files);
        let committed = commit_held_eq(&mut h, key, device);
        let context = h.context.unwrap();
        let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        if fault {
            h.backend
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .report_device_error()
                .unwrap();
        } else {
            assert!(operation.cancel());
        }
        loop {
            let event = h.next();
            assert_eq!(event.state.project, committed.state.project);
            assert_eq!(event.state.revision, committed.state.revision);
            if let Some(terminal) = event.plugin_effect_complete.first() {
                assert!(Arc::ptr_eq(&terminal.control, &operation) && !terminal.accepted);
                if fault {
                    assert!(
                        event
                            .error
                            .as_deref()
                            .unwrap()
                            .contains("audio output fault")
                    );
                }
                break;
            }
        }
        if fault {
            h.audio.take().unwrap().control.close();
            h.backend.lock().unwrap().take();
            let restarted = h.run(Request::RestartAudio {
                project_generation: context.project_generation,
                request_id: 19,
            });
            assert_eq!(restarted.state.project, committed.state.project);
            h.open();
            assert_ne!(
                h.audio.as_ref().unwrap().control.output_identity(),
                Some(output)
            );
            h.applied(context);
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .unwrap();
        } else {
            assert_eq!(h.applied(context).output_identity, output);
        }
        expect_verified_sample(&h, 0.125);
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
    render_hosted_pcm(h)[510]
}

fn render_hosted_pcm(h: &Harness) -> [f32; 512] {
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
    host
}
