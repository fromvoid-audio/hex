//! Signed Resident Main replacement history at the real paused callback.
use super::*;
use hex_session::service::{DeviceInsertOutcome, GraphEditOperation};

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn stopped_resident_main_vst3_replacement_undo_waits_for_its_exact_receipt() {
    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    hex_ui::write_plugin_effect_test_catalog(&directory, &selection(), "Fx", 0).unwrap();
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        directory.join("plugin-catalog.json"),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(48_000, 8192)));
    let (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &imported.state,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    let original = terminal(&mut h);
    assert!(
        original.plugin_effect_complete[0].accepted,
        "{:?}",
        original.error
    );
    let (applied, _) = h.applied(applied);
    let [old] = original.state.project.main_devices() else {
        panic!("original Main slot");
    };
    let binding = |h: &ResidentHarness| {
        let (device, binding, _) = h.plugin_progress.lock().unwrap().source_bindings[0].unwrap();
        (device, binding)
    };
    let old_binding = binding(&h).1;
    let source = |h: &ResidentHarness| status(h).recording_source().unwrap();

    h.enable_confirmed_device_inserts();
    h.service
        .submit(Request::InsertPluginEffect(request(
            &original.state,
            PluginEffectDestination::ReplaceMainEffect {
                replacing: old.id(),
            },
        )))
        .unwrap();
    let mut installed = None;
    wait(|| {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            if let Some(offer) = &event.device_insert_confirmation {
                assert_eq!(offer.operation(), GraphEditOperation::ReplaceDevice);
                assert!(offer.proceed());
            }
            if !event.plugin_effect_complete.is_empty() {
                installed = Some(event);
                return true;
            }
        }
        h.render_plugin_block();
        false
    });
    let installed = installed.unwrap();
    assert!(
        installed.plugin_effect_complete[0].accepted,
        "{:?}",
        installed.error
    );
    let receipt = installed.device_insert_complete.unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    let (applied, _) = h.applied(applied);
    let [replacement] = installed.state.project.main_devices() else {
        panic!("replacement Main slot");
    };
    assert_ne!(replacement.id(), old.id());
    let predecessor = source(&h);
    assert_eq!(predecessor.context, receipt.submission.context);
    assert_eq!(
        predecessor.audio_revision,
        receipt.submission.audio_revision
    );
    assert!(!status(&h).playing.load(Acquire));
    wait(|| {
        h.pump();
        h.render_plugin_block();
        h.plugin_records.load(Acquire) == 1 && binding(&h).0 == replacement.id().value()
    });
    let incumbent = binding(&h);
    assert_ne!(incumbent.1, old_binding);

    let assert_incumbent = |h: &ResidentHarness| {
        assert_eq!(source(h), predecessor);
        assert_eq!(binding(h), incumbent);
        assert!(!status(h).playing.load(Acquire));
    };
    let assert_pending = |event: &Event| {
        if let Some(error) = &event.error {
            assert_eq!(event.state.project, installed.state.project);
            assert_eq!(event.state.revision, installed.state.revision);
            assert_eq!(event.state.can_undo, installed.state.can_undo);
            assert_eq!(event.state.can_redo, installed.state.can_redo);
            panic!("stopped Resident Main replacement Undo refused before exact install: {error}");
        }
        assert!(
            !event.request_complete,
            "stopped Resident Main replacement Undo completed before callback: revision {} -> {}, receipt {:?}",
            installed.state.revision, event.state.revision, event.device_insert_complete,
        );
        assert_eq!(event.state.project, installed.state.project);
        assert_eq!(event.state.revision, installed.state.revision);
        assert_eq!(event.state.can_undo, installed.state.can_undo);
        assert_eq!(event.state.can_redo, installed.state.can_redo);
    };
    let (prepared, enqueue, _) = h.probe_next_native_publication();
    h.service.submit(Request::Undo).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while prepared.try_recv().is_err() {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert_incumbent(&h);
            assert_pending(&event);
        }
        h.render_plugin_block();
        assert!(
            Instant::now() < deadline,
            "Main replacement Undo did not prepare"
        );
        std::thread::yield_now();
    }
    assert_incumbent(&h);
    let (id, fresh, accepted) = h.plugin_progress.lock().unwrap().newest_source.unwrap();
    assert_eq!(id, old.id().value());
    assert!(
        !accepted,
        "restored worker was accepted before the callback"
    );
    assert_ne!(fresh, old_binding);
    assert_ne!(fresh, incumbent.1);
    enqueue.send(()).unwrap();

    // Leave the callback paused at the exact predecessor while Session
    // reports pending history or exposes an early terminal.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert_incumbent(&h);
            assert_pending(&event);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Main replacement Undo did not go pending"
        );
        std::thread::yield_now();
    }
    let restored = state::edits::ordinary_terminal(&mut h);
    assert!(restored.error.is_none(), "{:?}", restored.error);
    let receipt = restored
        .device_insert_complete
        .expect("exact history receipt");
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(restored.state.project, original.state.project);
    assert_eq!(restored.state.revision, installed.state.revision + 1);
    assert!(restored.state.can_redo);
    assert_eq!(receipt.submission.context, source(&h).context);
    assert_eq!(receipt.submission.audio_revision, source(&h).audio_revision);
    assert_eq!(
        receipt.submission.output_identity,
        predecessor.output_identity
    );
    h.applied(applied);
    wait(|| {
        h.pump();
        h.render_plugin_block();
        h.plugin_records.load(Acquire) == 1 && binding(&h).1 == fresh
    });
    assert!(!status(&h).playing.load(Acquire));
}

fn status(h: &ResidentHarness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}
