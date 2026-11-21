//! Signed Resident effect replacement history at the real callback.
use super::*;
use hex_audio::plugin_effects::EffectBinding;
use hex_session::service::{DeviceInsertOutcome, GraphEditOperation};
use std::sync::atomic::Ordering::Release;

mod rolling;

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn stopped_resident_main_vst3_replacement_undo_waits_for_its_exact_receipt() {
    assert_stopped_undo(ReplacementOwner::Main);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn stopped_resident_return_vst3_replacement_undo_waits_for_its_exact_receipt() {
    assert_stopped_undo(ReplacementOwner::Return);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn stopped_resident_track_vst3_replacement_undo_waits_for_its_exact_receipt() {
    assert_stopped_undo(ReplacementOwner::Track);
}

fn assert_stopped_undo(owner: ReplacementOwner) {
    let _native = native_fixture();
    let files = Files::new();
    let (mut h, original, installed, original_binding, destination) =
        installed_replacement(&files, owner, 8192);
    assert_stopped_history(
        &mut h,
        &installed,
        &original,
        original_binding,
        destination,
        Request::Undo,
    );
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn stopped_resident_main_vst3_replacement_redo_waits_for_its_exact_receipt() {
    assert_stopped_redo(ReplacementOwner::Main);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn stopped_resident_return_vst3_replacement_redo_waits_for_its_exact_receipt() {
    assert_stopped_redo(ReplacementOwner::Return);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn stopped_resident_track_vst3_replacement_redo_waits_for_its_exact_receipt() {
    assert_stopped_redo(ReplacementOwner::Track);
}

fn assert_stopped_redo(bus: ReplacementOwner) {
    let _native = native_fixture();
    let files = Files::new();
    let (mut h, original, installed, original_binding, destination) =
        installed_replacement(&files, bus, 8192);
    let replaced_binding = binding(&h).unwrap().1;
    let restored = assert_stopped_history(
        &mut h,
        &installed,
        &original,
        original_binding,
        destination,
        Request::Undo,
    );
    assert_stopped_history(
        &mut h,
        &restored,
        &installed,
        replaced_binding,
        destination,
        Request::Redo,
    );
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn rolling_resident_main_vst3_replacement_undo_refuses_a_stale_source() {
    assert_rolling_refusal(ReplacementOwner::Main);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn rolling_resident_return_vst3_replacement_undo_refuses_a_stale_source() {
    assert_rolling_refusal(ReplacementOwner::Return);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn rolling_resident_track_vst3_replacement_undo_refuses_a_stale_source() {
    assert_rolling_refusal(ReplacementOwner::Track);
}

fn assert_rolling_refusal(bus: ReplacementOwner) {
    let _native = native_fixture();
    let files = Files::new();
    let (mut h, _, installed, _, destination) = installed_replacement(&files, bus, 8192);
    let predecessor = status(&h).recording_source().unwrap();
    let incumbent = binding(&h).unwrap();
    let newest_before = h.plugin_progress.lock().unwrap().newest_source;
    let minimum = if matches!(destination, PluginEffectDestination::Return(_)) {
        0.251
    } else {
        0.001
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    wait(|| {
        h.pump();
        let pcm = h.render_plugin_block();
        status(&h).playing.load(Acquire)
            && pcm.iter().flatten().any(|sample| sample.abs() > minimum)
    });

    // Change only the worker's read of the runtime revision. The actual
    // native source and its accepted worker continue to render.
    h.loop_witness_fault.store(2, Release);
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
                    Some("running replacement history source changed"),
                );
                assert_eq!(h.plugin_records.load(Acquire), 1);
                assert_eq!(
                    h.plugin_progress.lock().unwrap().newest_source,
                    newest_before
                );
                break;
            }
        }
        assert!(Instant::now() < deadline, "rolling Undo did not refuse");
        std::thread::yield_now();
    }
}

enum ReplacementOwner {
    Main,
    Return,
    Track,
}

fn installed_replacement(
    files: &Files,
    bus: ReplacementOwner,
    clip_frames: u32,
) -> (
    ResidentHarness,
    Event,
    Event,
    EffectBinding,
    PluginEffectDestination,
) {
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
    let imported = h.run(Request::Import(files.wav(48_000, clip_frames)));
    let (mut applied, _) = h.applied(applied);
    let (document, destination) = match bus {
        ReplacementOwner::Main => (imported.state, PluginEffectDestination::Main),
        ReplacementOwner::Track => {
            let [track] = imported.state.project.tracks() else {
                panic!("direct audio track");
            };
            assert!(!track.is_group() && track.parent_group().is_none());
            let destination = PluginEffectDestination::Track(track.id());
            (imported.state, destination)
        }
        ReplacementOwner::Return => {
            let track = imported.state.project.tracks()[0].id();
            let added = h.run(Request::Edit(Command::AddReturnTrack {
                name: "replacement return".into(),
            }));
            let Some(Created::ReturnTrack(return_track)) = added.created else {
                panic!("return bus");
            };
            (applied, _) = h.applied(applied);
            let routed = h.run(Request::Edit(Command::SetReturnSend {
                track,
                return_track,
                gain: 0.5,
            }));
            assert!(routed.error.is_none(), "{:?}", routed.error);
            let [send] = routed.state.project.track(track).unwrap().sends() else {
                panic!("exact return send");
            };
            assert_eq!(send.return_track(), return_track);
            assert_eq!(send.gain(), 0.5);
            (applied, _) = h.applied(applied);
            (routed.state, PluginEffectDestination::Return(return_track))
        }
    };
    h.service
        .submit(Request::InsertPluginEffect(request(&document, destination)))
        .unwrap();
    let original = terminal(&mut h);
    assert!(
        original.plugin_effect_complete[0].accepted,
        "{:?}",
        original.error
    );
    let (applied, _) = h.applied(applied);
    let old = occupied_effect(&original, destination);
    let replace = match destination {
        PluginEffectDestination::Main => PluginEffectDestination::ReplaceMainEffect {
            replacing: old.id(),
        },
        PluginEffectDestination::Return(return_track) => {
            PluginEffectDestination::ReplaceReturnEffect {
                return_track,
                replacing: old.id(),
            }
        }
        PluginEffectDestination::Track(track) => PluginEffectDestination::ReplaceTrackEffect {
            track,
            replacing: old.id(),
        },
        _ => panic!("resident effect"),
    };
    let old_binding = binding(&h).unwrap().1;

    h.enable_confirmed_device_inserts();
    h.service
        .submit(Request::InsertPluginEffect(request(
            &original.state,
            replace,
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
    let replacement = occupied_effect(&installed, destination);
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
    (h, original, installed, old_binding, destination)
}

fn assert_stopped_history(
    h: &mut ResidentHarness,
    installed: &Event,
    target: &Event,
    retired_target: EffectBinding,
    destination: PluginEffectDestination,
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
        occupied_effect(installed, destination).id().value()
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
                "stopped Resident {destination:?} replacement {direction} refused before exact install: {error}"
            );
        }
        assert!(
            !event.request_complete,
            "stopped Resident {destination:?} replacement {direction} completed before callback: revision {} -> {}, receipt {:?}",
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
            "{destination:?} replacement {direction} did not prepare"
        );
        std::thread::yield_now();
    }
    assert_incumbent(h);
    let (id, fresh, accepted) = h.plugin_progress.lock().unwrap().newest_source.unwrap();
    assert_eq!(id, occupied_effect(target, destination).id().value());
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
            "{destination:?} replacement {direction} did not go pending"
        );
        std::thread::yield_now();
    }
    let restored = state::edits::ordinary_terminal(h);
    assert!(restored.error.is_none(), "{:?}", restored.error);
    let receipt = restored
        .device_insert_complete
        .expect("exact history receipt");
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    let mut expected = target.state.project.snapshot();
    // Undo restores authored values without reusing a retired device ID.
    expected.next_id = expected
        .next_id
        .max(installed.state.project.next_identity());
    assert_eq!(restored.state.project.snapshot(), expected);
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

fn occupied_effect(event: &Event, destination: PluginEffectDestination) -> &hex_project::Device {
    let project = &event.state.project;
    let effects = match destination {
        PluginEffectDestination::Main => project.main_devices(),
        PluginEffectDestination::Return(return_track) => project
            .return_tracks()
            .iter()
            .find(|bus| bus.id() == return_track)
            .unwrap()
            .devices(),
        PluginEffectDestination::Track(track) => project.track(track).unwrap().devices(),
        _ => panic!("resident effect"),
    };
    let [effect] = effects else {
        panic!("exact occupied slot");
    };
    effect
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
