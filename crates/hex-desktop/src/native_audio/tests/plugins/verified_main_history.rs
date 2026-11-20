//! Fresh catalog Main history uses the exact existing Verified insert receipt.
use super::verified_insert::{await_verified_retirement, render_hosted_quantum};
use super::*;
use hex_session::service::DeviceInsertOutcome;

fn status(h: &Harness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_verified_main_history_restores_a_fresh_worker_only_after_exact_consent() {
    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    let mut selection = selection();
    let module = directory.join("history-main.vst3");
    super::ui::copy_original_module(&selection.module, &module);
    selection.module = module;
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection, "Fx", 0).unwrap();
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
    let imported = h.run(Request::Import(files.wav(48_000, 1_048_576)));
    h.applied(h.context.unwrap());
    let submit = |h: &Harness, request| h.service.as_ref().unwrap().submit(request).unwrap();
    let mut insertion = request(&imported.state, PluginEffectDestination::Main);
    insertion.selection = selection.clone();
    submit(&h, Request::InsertPluginEffect(insertion));
    let inserted = loop {
        let event = h.next_with_audio();
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(terminal) = event.plugin_effect_complete.first() {
            assert!(terminal.accepted);
            break event;
        }
    };
    let Some(Created::Device(vst)) = inserted.created else {
        panic!("effect")
    };
    h.applied(h.context.unwrap());
    wait(|| progress.lock().unwrap().source_bindings[0].is_some());
    let original = progress.lock().unwrap().source_bindings[0].unwrap().1;
    let removed = h.run_verified_device_insert(Command::RemoveDevice(vst));
    let predecessor = h.applied(h.context.unwrap());
    await_verified_retirement(&h, &progress);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..4 {
        let mut sample = 0.0;
        wait(|| {
            sample = render_hosted_quantum(&h);
            status(&h).performance_frame.load(Acquire) == (block + 1) * 256
        });
        assert_eq!(sample, 0.25);
    }
    let approved = std::cell::Cell::new(false);
    let next_event = |h: &Harness| {
        let mut event = None;
        wait(|| {
            if let Ok(next) = h.service.as_ref().unwrap().try_event() {
                event = Some(next);
                return true;
            }
            let frame = status(h).performance_frame.load(Acquire);
            let sample = render_hosted_quantum(h);
            if !approved.get() && status(h).performance_frame.load(Acquire) == frame + 256 {
                assert_eq!(status(h).applied_audio_source(), Some(predecessor));
                assert_eq!(
                    sample, 0.25,
                    "history preparation or consent changed incumbent PCM"
                );
            }
            wait(|| {
                h.backend
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .native_workers_idle_on_control()
                    == Some(true)
            });
            std::thread::sleep(Duration::from_millis(1));
            false
        });
        event.unwrap()
    };
    let offer = |h: &Harness| loop {
        let event = next_event(h);
        assert!(
            event.error.is_none(),
            "running Main history refused: {:?}",
            event.error
        );
        assert!(!event.request_complete);
        assert_eq!(event.state.project, removed.state.project);
        assert_eq!(event.state.revision, removed.state.revision);
        assert_eq!(event.state.can_undo, removed.state.can_undo);
        assert_eq!(event.state.can_redo, removed.state.can_redo);
        if let Some(offer) = event.device_insert_confirmation {
            break offer;
        }
    };
    // Catalog resolution is the full saved provider identity, never a path
    // alias or a different currently available signed processor.
    let wrong = signed_effect_selection("effect");
    assert_ne!(
        (&wrong.cid, &wrong.executable_sha256, &wrong.info_sha256),
        (
            &selection.cid,
            &selection.executable_sha256,
            &selection.info_sha256
        )
    );
    hex_ui::write_plugin_effect_test_catalog(&directory, &wrong, "Fx", 0).unwrap();
    submit(&h, Request::Undo);
    let unavailable = loop {
        let event = next_event(&h);
        if event.request_complete {
            break event;
        }
    };
    assert!(
        unavailable
            .error
            .as_deref()
            .is_some_and(|message| message.contains("saved vst3 binary or class is unavailable")),
        "wrong provider was accepted: {:?}",
        unavailable.error
    );
    assert_eq!(unavailable.state.project, removed.state.project);
    assert_eq!(unavailable.state.revision, removed.state.revision);
    assert_eq!(unavailable.state.can_undo, removed.state.can_undo);
    assert_eq!(unavailable.state.can_redo, removed.state.can_redo);
    assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
    assert!(status(&h).playing.load(Acquire));
    assert!(progress.lock().unwrap().newest_source.is_none());
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection, "Fx", 0).unwrap();

    submit(&h, Request::Undo);
    assert!(offer(&h).cancel());
    let cancelled = loop {
        let event = next_event(&h);
        if event.request_complete {
            break event;
        }
    };
    assert_eq!(
        cancelled.device_insert_complete.unwrap().outcome,
        DeviceInsertOutcome::Cancelled
    );
    assert_eq!(cancelled.state.project, removed.state.project);
    assert_eq!(cancelled.state.revision, removed.state.revision);
    assert_eq!(cancelled.state.can_undo, removed.state.can_undo);
    assert_eq!(cancelled.state.can_redo, removed.state.can_redo);
    assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
    assert!(status(&h).playing.load(Acquire));
    // A newer command invalidates the offered cut, even if it leaves the
    // canonical source and audible playback otherwise unchanged.
    submit(&h, Request::Undo);
    let stale = offer(&h);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    render_hosted_quantum(&h);
    assert!(stale.proceed());
    let refused = loop {
        let event = next_event(&h);
        if event.request_complete {
            break event;
        }
    };
    assert_eq!(
        refused.error.as_deref(),
        Some("device insert confirmation is stale")
    );
    assert_eq!(refused.state.project, removed.state.project);
    assert_eq!(refused.state.revision, removed.state.revision);
    assert_eq!(refused.state.can_undo, removed.state.can_undo);
    assert_eq!(refused.state.can_redo, removed.state.can_redo);
    assert_eq!(status(&h).applied_audio_source(), Some(predecessor));
    assert!(status(&h).playing.load(Acquire));

    submit(&h, Request::Undo);
    let consent = offer(&h);
    assert!(consent.proceed());
    approved.set(true);
    let installed = loop {
        let event = next_event(&h);
        assert!(event.error.is_none(), "{:?}", event.error);
        if event.request_complete {
            break event;
        }
        assert_eq!(event.state.project, removed.state.project);
    };
    assert!(installed.created.is_none());
    assert_eq!(installed.state.project, inserted.state.project);
    assert_eq!(installed.state.revision, removed.state.revision + 1);
    let receipt = installed.device_insert_complete.unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    let actual = status(&h).applied_audio_source().unwrap();
    assert_eq!(
        receipt.submission.output_identity,
        predecessor.output_identity
    );
    assert_eq!(receipt.submission.context, actual.context);
    assert_eq!(receipt.submission.audio_revision, actual.audio_revision);
    assert_ne!(actual, predecessor);
    assert!(status(&h).playing.load(Acquire));
    let (device, restored, accepted) = progress.lock().unwrap().newest_source.unwrap();
    assert_eq!(device, vst.value());
    assert!(accepted);
    assert_ne!(restored, original, "history resurrected a deleted worker");
    // Removing it again is outside the running insert-only history proof.
    // Preserve the explicitly settled fallback rather than broadening history.
    submit(&h, Request::Redo);
    let unsupported = loop {
        let event = next_event(&h);
        if event.request_complete {
            break event;
        }
    };
    assert_eq!(
        unsupported.error.as_deref(),
        Some("stop playback to undo or redo this vst3 edit")
    );
    assert_eq!(unsupported.state.project, installed.state.project);
    assert_eq!(unsupported.state.revision, installed.state.revision);
    assert_eq!(status(&h).applied_audio_source(), Some(actual));
    assert!(status(&h).playing.load(Acquire));
}
