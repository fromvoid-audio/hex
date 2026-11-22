//! Running catalog history uses the existing exact Verified install receipts.
use super::verified_insert::{await_verified_retirement, render_hosted_quantum};
use super::*;
use hex_session::service::{DeviceInsertOutcome, GraphEditOperation};

fn status(h: &Harness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_verified_main_history_restores_and_retires_the_exact_worker() {
    assert_running_verified_history(HistoryOwner::Main);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_verified_track_history_restores_and_retires_the_exact_worker() {
    assert_running_verified_history(HistoryOwner::Track);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_verified_return_history_restores_and_retires_the_exact_worker() {
    assert_running_verified_history(HistoryOwner::Return);
}

enum HistoryOwner {
    Main,
    Track,
    Return,
}

fn assert_running_verified_history(owner: HistoryOwner) {
    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    let mut selection = selection();
    let module = directory.join("history.vst3");
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
    let track = imported.state.project.tracks()[0].id();
    let (state, destination, removed_sample) = match owner {
        HistoryOwner::Main => (imported.state, PluginEffectDestination::Main, 0.25),
        HistoryOwner::Track => (imported.state, PluginEffectDestination::Track(track), 0.25),
        HistoryOwner::Return => {
            let added = h.run(Request::Edit(Command::AddReturnTrack {
                name: "wet".into(),
            }));
            let Some(Created::ReturnTrack(return_track)) = added.created else {
                panic!("return track");
            };
            h.applied(h.context.unwrap());
            let routed = h.run(Request::Edit(Command::SetReturnSend {
                track,
                return_track,
                gain: 0.5,
            }));
            h.applied(h.context.unwrap());
            (
                routed.state,
                PluginEffectDestination::Return(return_track),
                0.375,
            )
        }
    };
    let mut insertion = request(&state, destination);
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
        assert_eq!(sample, removed_sample);
    }
    let approved = std::cell::Cell::new(false);
    let phase = std::cell::Cell::new("provider substitution");
    let next_event = |h: &Harness| {
        let mut event = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        wait(|| {
            assert!(
                Instant::now() < deadline,
                "history event timed out: {}",
                phase.get()
            );
            if let Ok(next) = h.service.as_ref().unwrap().try_event() {
                event = Some(next);
                return true;
            }
            let frame = status(h).performance_frame.load(Acquire);
            let sample = render_hosted_quantum(h);
            if !approved.get() && status(h).performance_frame.load(Acquire) == frame + 256 {
                assert_eq!(status(h).applied_audio_source(), Some(predecessor));
                assert_eq!(
                    sample,
                    removed_sample,
                    "phase={} position={} performance={} playing={} paused={:?}",
                    phase.get(),
                    status(h).position.load(Acquire),
                    status(h).performance_frame.load(Acquire),
                    status(h).playing.load(Acquire),
                    status(h).paused_input_interval()
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
            "running {destination:?} history refused: {:?}",
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
    // The fixture writer updates individual module records. Replace the owned
    // catalog so the original exact provider is absent, rather than adding an
    // unrelated available provider beside it.
    fs::remove_file(directory.join("plugin-catalog.json")).unwrap();
    hex_ui::write_plugin_effect_test_catalog(&directory, &wrong, "Fx", 0).unwrap();
    let saved =
        Hosting::saved_identity_on_control(inserted.state.project.device(vst).unwrap().state())
            .unwrap();
    let catalog =
        hex_plugin_catalog::Catalog::load(&directory.join("plugin-catalog.json")).unwrap();
    assert_eq!(catalog.entries.len(), 1);
    assert_eq!(
        catalog
            .resolve_effect(
                saved.class.bytes(),
                saved.binary.executable_sha256,
                saved.binary.info_sha256,
            )
            .err()
            .as_deref(),
        Some("saved vst3 binary or class is unavailable")
    );
    submit(&h, Request::Undo);
    let unavailable = loop {
        let event = next_event(&h);
        assert!(
            event.device_insert_confirmation.is_none(),
            "unavailable provider unexpectedly offered for installation"
        );
        if event.request_complete {
            break event;
        }
    };
    assert!(
        unavailable
            .error
            .as_deref()
            .is_some_and(|message| message.contains("saved vst3 binary or class is unavailable")),
        "unexpected wrong-provider result: {:?}",
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

    phase.set("cancel preparation");
    submit(&h, Request::Undo);
    assert!(offer(&h).cancel());
    phase.set("cancel completion");
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
    phase.set("stale preparation");
    submit(&h, Request::Undo);
    let stale = offer(&h);
    phase.set("stale completion");
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

    phase.set("success preparation");
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
    let authored_before_redo = &installed.state;
    for proceed in [false, true] {
        phase.set("redo preparation");
        submit(&h, Request::Redo);
        let offer = loop {
            let event = next_event(&h);
            assert!(
                event.error.is_none(),
                "running {destination:?} removal history refused: {:?}",
                event.error
            );
            assert!(!event.request_complete);
            assert_eq!(event.state.project, authored_before_redo.project);
            assert_eq!(event.state.revision, authored_before_redo.revision);
            if let Some(offer) = event.device_insert_confirmation {
                break offer;
            }
        };
        assert_eq!(offer.operation(), GraphEditOperation::RemoveDevice);
        assert_eq!(offer.submission().output_identity, actual.output_identity);
        assert_eq!(status(&h).applied_audio_source(), Some(actual));
        wait(|| {
            let before = status(&h).performance_frame.load(Acquire);
            let sample = render_hosted_quantum(&h);
            assert_eq!(status(&h).applied_audio_source(), Some(actual));
            let trial = progress.lock().unwrap().newest_source.unwrap();
            assert_eq!(trial, (vst.value(), restored, true));
            status(&h).performance_frame.load(Acquire) == before + 256
                && sample.is_finite()
                && sample > 0.0
        });
        phase.set("redo completion");
        assert!(if proceed {
            offer.proceed()
        } else {
            offer.cancel()
        });
        let result = loop {
            let event = next_event(&h);
            assert!(event.error.is_none(), "{:?}", event.error);
            if event.request_complete {
                break event;
            }
            assert_eq!(event.state.project, authored_before_redo.project);
        };
        let receipt = result.device_insert_complete.unwrap();
        if !proceed {
            assert_eq!(receipt.outcome, DeviceInsertOutcome::Cancelled);
            assert_eq!(result.state.project, authored_before_redo.project);
            assert_eq!(result.state.revision, authored_before_redo.revision);
            assert_eq!(result.state.can_undo, authored_before_redo.can_undo);
            assert_eq!(result.state.can_redo, authored_before_redo.can_redo);
            assert_eq!(status(&h).applied_audio_source(), Some(actual));
            continue;
        }
        assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
        assert!(result.created.is_none());
        assert_eq!(result.state.project, removed.state.project);
        assert_eq!(result.state.revision, authored_before_redo.revision + 1);
        let installed = status(&h).applied_audio_source().unwrap();
        assert_ne!(installed, actual);
        assert_eq!(installed.output_identity, actual.output_identity);
        assert_eq!(installed.context, receipt.submission.context);
        assert_eq!(installed.audio_revision, receipt.submission.audio_revision);
        await_verified_retirement(&h, &progress);
        assert!(status(&h).playing.load(Acquire));
    }
}
