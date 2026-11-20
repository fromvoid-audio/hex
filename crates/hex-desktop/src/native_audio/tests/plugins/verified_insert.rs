//! Signed hosted insertion on the production verified-file publication path.
use super::*;
use hex_audio::audio_diagnostics::{AudioDiagnosticKind, AudioDiagnosticsFlags};
use hex_session::device_gesture::ParameterValue;

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
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut original = None;
    wait(|| {
        let sample = h.render()[63][0];
        std::thread::sleep(Duration::from_millis(1));
        if sample != 0.0 {
            original = Some(sample);
        }
        original.is_some()
            && progress.lock().unwrap().source_bindings[0]
                .is_some_and(|(_, _, n)| n.unwrap_or(0) > 2)
    });
    let (device, binding, completed) = progress.lock().unwrap().source_bindings[0].unwrap();
    assert_eq!(device, vst.value());

    h.run(Request::SetDeviceParameter {
        device: eq,
        parameter: hex_engine::eq::OUTPUT_GAIN,
        value: ParameterValue::Float(0.5),
    });
    let applied = h.applied(h.context.unwrap());
    assert!(applied.audio_revision > started.audio_revision);
    wait(|| {
        let sample = h.render()[63][0];
        std::thread::sleep(Duration::from_millis(1));
        (sample - 0.5 * original.unwrap()).abs() < 1e-5
            && progress.lock().unwrap().source_bindings[0]
                .is_some_and(|(_, original, n)| original == binding && n > completed)
    });
}
