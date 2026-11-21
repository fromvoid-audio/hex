//! Signed Resident Main replacement history at the real paused callback.
use super::*;
use hex_audio::plugin_effects::EffectBinding;
use hex_session::service::{DeviceInsertOutcome, GraphEditOperation};

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn stopped_resident_main_vst3_replacement_undo_waits_for_its_exact_receipt() {
    let _native = native_fixture();
    let files = Files::new();
    let (mut h, original, installed, original_binding) = installed_replacement(&files);
    assert_stopped_history(
        &mut h,
        &installed,
        &original,
        original_binding,
        Request::Undo,
    );
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn stopped_resident_main_vst3_replacement_redo_waits_for_its_exact_receipt() {
    let _native = native_fixture();
    let files = Files::new();
    let (mut h, original, installed, original_binding) = installed_replacement(&files);
    let replaced_binding = binding(&h).unwrap().1;
    let restored = assert_stopped_history(
        &mut h,
        &installed,
        &original,
        original_binding,
        Request::Undo,
    );
    assert_stopped_history(
        &mut h,
        &restored,
        &installed,
        replaced_binding,
        Request::Redo,
    );
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn rolling_resident_main_vst3_replacement_undo_preserves_the_predecessor() {
    let _native = native_fixture();
    let files = Files::new();
    let (mut h, _, installed, _) = installed_replacement(&files);
    let predecessor = status(&h).recording_source().unwrap();
    let incumbent = binding(&h).unwrap();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    wait(|| {
        h.pump();
        let pcm = h.render_plugin_block();
        status(&h).playing.load(Acquire) && pcm.iter().flatten().any(|sample| sample.abs() > 0.001)
    });

    h.service.submit(Request::Undo).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert_history_unchanged(&event, &installed);
            assert_eq!(status(&h).recording_source(), Some(predecessor));
            assert_eq!(binding(&h), Some(incumbent));
            assert!(status(&h).playing.load(Acquire));
            assert!(event.device_insert_complete.is_none());
            if event.request_complete {
                assert_eq!(
                    event.error.as_deref(),
                    Some("stop playback to undo or redo this vst3 replacement"),
                );
                break;
            }
        }
        assert!(Instant::now() < deadline, "rolling Undo did not refuse");
        std::thread::yield_now();
    }
}

fn installed_replacement(files: &Files) -> (ResidentHarness, Event, Event, EffectBinding) {
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
    let old_binding = binding(&h).unwrap().1;

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
    h.applied(applied);
    let [replacement] = installed.state.project.main_devices() else {
        panic!("replacement Main slot");
    };
    assert_ne!(replacement.id(), old.id());
    let predecessor = status(&h).recording_source().unwrap();
    assert_eq!(predecessor.context, receipt.submission.context);
    assert_eq!(
        predecessor.audio_revision,
        receipt.submission.audio_revision
    );
    assert!(!status(&h).playing.load(Acquire));
    wait(|| {
        h.pump();
        h.render_plugin_block();
        h.plugin_records.load(Acquire) == 1
            && binding(&h).is_some_and(|(device, worker)| {
                device == replacement.id().value() && worker != old_binding
            })
    });
    (h, original, installed, old_binding)
}

fn assert_stopped_history(
    h: &mut ResidentHarness,
    installed: &Event,
    target: &Event,
    retired_target: EffectBinding,
    request: Request,
) -> Event {
    let (direction, can_redo) = match request {
        Request::Undo => ("Undo", true),
        Request::Redo => ("Redo", false),
        _ => panic!("replacement history direction"),
    };
    let incumbent = binding(h).unwrap();
    let predecessor = status(h).recording_source().unwrap();
    assert_eq!(
        predecessor.context.editor_revision,
        installed.state.revision
    );
    assert_eq!(
        incumbent.0,
        installed.state.project.main_devices()[0].id().value()
    );

    let assert_incumbent = |h: &ResidentHarness| {
        assert_eq!(status(h).recording_source(), Some(predecessor));
        assert_eq!(binding(h), Some(incumbent));
        assert!(!status(h).playing.load(Acquire));
    };
    let assert_pending = |event: &Event| {
        if let Some(error) = &event.error {
            assert_history_unchanged(event, installed);
            panic!(
                "stopped Resident Main replacement {direction} refused before exact install: {error}"
            );
        }
        assert!(
            !event.request_complete,
            "stopped Resident Main replacement {direction} completed before callback: revision {} -> {}, receipt {:?}",
            installed.state.revision, event.state.revision, event.device_insert_complete,
        );
        assert_history_unchanged(event, installed);
    };
    let (prepared, enqueue, _) = h.probe_next_native_publication();
    h.service.submit(request).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while prepared.try_recv().is_err() {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert_incumbent(h);
            assert_pending(&event);
        }
        h.render_plugin_block();
        assert!(
            Instant::now() < deadline,
            "Main replacement {direction} did not prepare"
        );
        std::thread::yield_now();
    }
    assert_incumbent(h);
    let (id, fresh, accepted) = h.plugin_progress.lock().unwrap().newest_source.unwrap();
    assert_eq!(id, target.state.project.main_devices()[0].id().value());
    assert!(
        !accepted,
        "restored worker was accepted before the callback"
    );
    assert_ne!(fresh, retired_target);
    assert_ne!(fresh, incumbent.1);
    enqueue.send(()).unwrap();

    // Leave the callback paused at the exact predecessor while Session
    // reports pending history or exposes an early terminal.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert_incumbent(h);
            assert_pending(&event);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Main replacement {direction} did not go pending"
        );
        std::thread::yield_now();
    }
    let restored = state::edits::ordinary_terminal(h);
    assert!(restored.error.is_none(), "{:?}", restored.error);
    let receipt = restored
        .device_insert_complete
        .expect("exact history receipt");
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(restored.state.project, target.state.project);
    assert_eq!(restored.state.revision, installed.state.revision + 1);
    assert_eq!(restored.state.can_redo, can_redo);
    let actual = status(h).recording_source().unwrap();
    assert_eq!(receipt.submission.context, actual.context);
    assert_eq!(receipt.submission.audio_revision, actual.audio_revision);
    assert_eq!(
        receipt.submission.output_identity,
        predecessor.output_identity
    );
    h.applied(predecessor.audio_revision);
    wait(|| {
        h.pump();
        h.render_plugin_block();
        h.plugin_records.load(Acquire) == 1 && binding(h) == Some((id, fresh))
    });
    assert!(!status(h).playing.load(Acquire));
    restored
}

fn assert_history_unchanged(event: &Event, incumbent: &Event) {
    assert_eq!(event.state.project, incumbent.state.project);
    assert_eq!(event.state.revision, incumbent.state.revision);
    assert_eq!(event.state.can_undo, incumbent.state.can_undo);
    assert_eq!(event.state.can_redo, incumbent.state.can_redo);
}

fn binding(h: &ResidentHarness) -> Option<(u64, EffectBinding)> {
    h.plugin_progress.lock().unwrap().source_bindings[0]
        .map(|(device, binding, _)| (device, binding))
}

fn status(h: &ResidentHarness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}
