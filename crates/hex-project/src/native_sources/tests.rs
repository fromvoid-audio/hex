use super::*;
use crate::{Editor, default_instrument_state};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};

fn create(name: &str) -> Command {
    Command::CreateNativeSource {
        name: name.into(),
        devices: vec![default_instrument_state().unwrap()],
        instrument_index: 0,
    }
}
fn add(editor: &mut Editor, name: &str) -> SourceId {
    let Created::NativeSource(id) = editor.execute(create(name)).unwrap() else {
        panic!("source")
    };
    id
}

#[test]
fn independent_copy_keeps_the_complete_source_unit_with_fresh_internal_identities() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "source-child.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "copy".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "complete source".into(),
            devices: vec![default_instrument_state().unwrap(); 5],
            instrument_index: 1,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let mut snapshot = editor.project().snapshot();
    let devices = &mut snapshot.native_sources[0].devices;
    devices[2].parent = Some(devices[1].id);
    devices[3].parent = Some(devices[2].id);
    devices[3].bypassed = true;
    devices[3].comparison = Some(crate::DeviceComparison::new(
        crate::DeviceVariant::B,
        devices[3]
            .state
            .clone()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(asset.value()).unwrap(),
            }])
            .unwrap(),
    ));
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    let child = editor
        .project()
        .native_source(source)
        .unwrap()
        .devices()
        .nth(2)
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceAutomation {
            device: child,
            parameter: hex_lattice_contract::ParameterId::from_static(1),
            points: vec![crate::AutomationPoint {
                beat: crate::BeatTime::ZERO,
                value: crate::AutomationValue::new(0.25).unwrap(),
                outgoing: crate::AutomationInterpolation::Linear,
            }],
        })
        .unwrap();
    let mut project = editor.project().clone();
    let original = project.native_source(source).unwrap().clone();
    // Exercise the copy primitive before nonempty binding/playback admission.
    // Its receiver is unpublished here, just as during a clipboard candidate.
    let copied = project
        .copy_source_binding(&SourceBinding(original.clone()), track)
        .unwrap();
    assert_eq!(copied.0.devices().count(), original.devices().count());
    assert_eq!(copied.0.instrument_index(), original.instrument_index());
    assert_ne!(copied.source_id(), source);
    assert!(!Arc::ptr_eq(&copied.0.definition, &original.definition));
    assert_eq!(copied.receiver(), Some(track));
    let ids: Vec<_> = copied.0.devices().map(Device::id).collect();
    for (index, (new, old)) in copied.0.devices().zip(original.devices()).enumerate() {
        assert!(!original.devices().any(|item| item.id() == new.id()));
        assert_eq!(
            new.parent(),
            match index {
                2 => Some(ids[1]),
                3 => Some(ids[2]),
                _ => None,
            }
        );
        let mut normalized = new.clone();
        normalized.id = old.id();
        normalized.parent = old.parent();
        assert_eq!(&normalized, old);
    }
    project
        .set_native_source_receiver(copied.source_id(), None)
        .unwrap();
    let restored = Project::from_snapshot(project.snapshot()).unwrap();
    assert_eq!(restored, project);
    project
        .edit_source_device(copied.source_id(), ids[2], |device| {
            device.bypassed = true;
            Ok(())
        })
        .unwrap();
    assert_eq!(project.native_source(source), Some(&original));
    assert!(!copied.0.devices().nth(2).unwrap().bypassed());
    let mut editor = Editor::new(project, 8).unwrap();
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::BindTrackSource {
                track,
                source: Some(copied.source_id())
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    editor.execute(Command::RemoveNativeSource(source)).unwrap();
    assert!(
        editor.execute(Command::RemoveAsset(asset)).is_err(),
        "copied inactive child state retains its media"
    );
    editor
        .execute(Command::RemoveNativeSource(copied.source_id()))
        .unwrap();
    editor.execute(Command::RemoveAsset(asset)).unwrap();
}
#[test]
fn inventory_retains_identity_and_edits_across_global_history_and_track_deletion() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let first = add(&mut editor, "First Source");
    let second = add(&mut editor, "second");
    assert_eq!(
        editor
            .project()
            .native_sources()
            .iter()
            .map(|s| s.id())
            .collect::<Vec<_>>(),
        [first, second]
    );
    let device = editor
        .project()
        .native_source(first)
        .unwrap()
        .instrument()
        .id();
    let state = default_instrument_state()
        .unwrap()
        .with_output_gain_millidb(-6000)
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state.clone(),
        })
        .unwrap();
    editor
        .execute(Command::RenameNativeSource {
            source: first,
            name: "Kept Name".into(),
        })
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "temporary".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor.execute(Command::RemoveTrack(track)).unwrap();
    assert_eq!(
        editor.project().native_source(first).unwrap().name(),
        "Kept Name"
    );
    assert_eq!(editor.project().device(device).unwrap().state(), &state);
    let kept = editor.project().native_sources().to_vec();
    editor.execute(Command::RemoveNativeSource(first)).unwrap();
    assert_eq!(editor.project().native_sources()[0].id(), second);
    assert!(editor.undo());
    assert_eq!(editor.project().native_sources(), kept);
    assert!(editor.redo());
    assert!(editor.project().native_source(first).is_none());
    assert!(editor.undo());
    assert!(editor.undo()); // track removal
    assert!(editor.project().track(track).is_some());
    assert!(editor.undo()); // track creation
    assert!(editor.undo()); // source rename
    assert_eq!(
        editor.project().native_source(first).unwrap().name(),
        "First Source"
    );
    assert!(editor.undo()); // device state
    assert_eq!(
        editor.project().device(device).unwrap().state(),
        &default_instrument_state().unwrap()
    );
}
#[test]
fn zero_user_sources_outlive_history_and_ids_never_rewind() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 1).unwrap();
    let first = add(&mut editor, "retained");
    let second = add(&mut editor, "undone");
    assert!(editor.undo());
    let third = add(&mut editor, "branch");
    assert!(third.value() > second.value());
    assert!(editor.project().native_source(first).is_some());
    assert!(!editor.can_redo());
    let restored = Project::from_snapshot(editor.project().snapshot()).unwrap();
    assert_eq!(&restored, editor.project());
}
#[test]
fn source_media_including_inactive_comparison_is_kept_until_explicit_removal() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "sample.wav".into(),
            sample_rate: 48000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let source = add(&mut editor, "resource");
    let device = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let state = default_instrument_state()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceComparison {
            device,
            state: default_instrument_state().unwrap(),
            comparison: Some(crate::DeviceComparison::new(crate::DeviceVariant::A, state)),
        })
        .unwrap();
    let before = editor.project().clone();
    assert!(editor.execute(Command::RemoveAsset(asset)).is_err());
    assert_eq!(editor.project(), &before);
    editor.execute(Command::RemoveNativeSource(source)).unwrap();
    editor.execute(Command::RemoveAsset(asset)).unwrap();
    assert!(editor.undo());
    assert!(editor.undo());
    assert_eq!(editor.project().native_sources(), before.native_sources());
}
#[test]
fn invalid_chain_media_and_preparation_refusals_leave_state_and_history_intact() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    add(&mut editor, "existing");
    let before = editor.project().clone();
    for command in [
        Command::CreateNativeSource {
            name: "empty".into(),
            devices: vec![],
            instrument_index: 0,
        },
        Command::CreateNativeSource {
            name: "bad index".into(),
            devices: vec![default_instrument_state().unwrap()],
            instrument_index: 1,
        },
        create("\n"),
        Command::CreateNativeSource {
            name: "too many".into(),
            devices: vec![default_instrument_state().unwrap(); MAX_SOURCE_DEVICES + 1],
            instrument_index: 0,
        },
        Command::CreateNativeSource {
            name: "missing media".into(),
            devices: vec![
                default_instrument_state()
                    .unwrap()
                    .with_asset_references(vec![DeviceAssetReference {
                        slot: DeviceAssetSlot::new(1).unwrap(),
                        asset: DeviceAssetKey::new(999).unwrap(),
                    }])
                    .unwrap(),
            ],
            instrument_index: 0,
        },
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project(), &before);
    }
    assert!(
        editor
            .execute_prepared(create("refused"), |_| Err::<(), _>(Invalid(
                "prepare refused"
            )))
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.undo());
    assert!(editor.project().native_sources().is_empty());
}
#[test]
fn snapshots_refuse_duplicate_cross_owner_ids_missing_media_and_count_overflow() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let id = add(&mut editor, "source");
    let good = editor.project().snapshot();
    let mut bad = good.clone();
    bad.native_sources[0].devices[0].id = id.value();
    assert!(Project::from_snapshot(bad).is_err());
    let mut bad = good.clone();
    bad.next_id = id.value();
    assert!(Project::from_snapshot(bad).is_err());
    let mut bad = good.clone();
    bad.native_sources[0].instrument_index = 5;
    assert!(Project::from_snapshot(bad).is_err());
    let mut bad = good;
    bad.native_sources = vec![bad.native_sources[0].clone(); MAX_NATIVE_SOURCES + 1];
    assert!(Project::from_snapshot(bad).is_err());
}

#[test]
fn retained_devices_share_the_existing_project_device_budget() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 1).unwrap();
    let source = add(&mut editor, "template");
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "template".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut snapshot = editor.project().snapshot();
    let source_template = snapshot.native_sources[0].clone();
    let track_template = snapshot.tracks[0].clone();
    snapshot.native_sources.clear();
    snapshot.tracks.clear();
    let mut next = editor.project().next_identity();
    for _ in 0..crate::MAX_TRACKS {
        let mut s = source_template.clone();
        s.id = next;
        next += 1;
        s.devices = vec![s.devices[0].clone(); MAX_SOURCE_DEVICES];
        for d in &mut s.devices {
            d.id = next;
            next += 1;
        }
        snapshot.native_sources.push(s);
    }
    // Fill the remaining global budget using ordinary track devices.
    for _ in 0..crate::MAX_RETURN_TRACKS {
        let mut t = track_template.clone();
        t.id = next;
        next += 1;
        t.devices = vec![source_template.devices[0].clone(); crate::MAX_DEVICES_PER_TRACK];
        for d in &mut t.devices {
            d.id = next;
            next += 1;
        }
        snapshot.tracks.push(t);
    }
    snapshot.next_id = next;
    let project = Project::from_snapshot(snapshot).unwrap();
    assert_eq!(
        crate::returns::device_count(&project).unwrap(),
        crate::MAX_DEVICES
    );
    let mut candidate = project.clone();
    let binding = SourceBinding(candidate.native_sources()[0].clone());
    let before = candidate.snapshot();
    assert_eq!(
        candidate
            .copy_source_binding(&binding, project.tracks()[0].id())
            .unwrap_err(),
        Invalid("project device limit reached")
    );
    assert_eq!(candidate.snapshot(), before);
    // Leave room in this source while keeping the global node budget full.
    let mut full = project.snapshot();
    let spare = full.native_sources[0].devices.pop().unwrap();
    full.main_devices.push(spare);
    let mut candidate = Editor::new(Project::from_snapshot(full).unwrap(), 1).unwrap();
    let parent = candidate.project().native_sources()[0].instrument().id();
    let before = candidate.project().snapshot();
    assert_eq!(
        candidate
            .execute(Command::InsertNestedDevice {
                parent,
                state: default_instrument_state().unwrap(),
            })
            .unwrap_err(),
        Invalid("project device limit reached")
    );
    assert_eq!(candidate.project().snapshot(), before);
    assert!(!candidate.can_undo());
    let target = project.tracks()[0].id();
    let mut editor = Editor::new(project, 1).unwrap();
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::InsertInstrument {
                track: target,
                state: default_instrument_state().unwrap()
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(!editor.can_undo());
    assert!(editor.project().native_source(source).is_none());
    assert!(editor.project().track(track).is_none());
}

#[test]
fn independent_copy_refuses_source_and_identity_limits_before_changing_authored_state() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let source = add(&mut editor, "template");
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "target".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let original = editor.project().snapshot();
    let mut exhausted = original.clone();
    exhausted.next_id = u64::MAX;
    let mut project = Project::from_snapshot(exhausted).unwrap();
    let binding = SourceBinding(project.native_source(source).unwrap().clone());
    let before = project.snapshot();
    assert!(project.copy_source_binding(&binding, track).is_err());
    assert_eq!(project.snapshot(), before);
    let mut full = original.clone();
    full.native_sources.clear();
    let mut next = full.next_id;
    for _ in 0..MAX_NATIVE_SOURCES {
        let mut item = original.native_sources[0].clone();
        item.id = next;
        item.devices[0].id = next + 1;
        next += 2;
        full.native_sources.push(item);
    }
    full.next_id = next;
    let mut project = Project::from_snapshot(full).unwrap();
    let binding = SourceBinding(project.native_sources()[0].clone());
    let before = project.snapshot();
    assert_eq!(
        project.copy_source_binding(&binding, track).unwrap_err(),
        Invalid("native source limit reached")
    );
    assert_eq!(project.snapshot(), before);
}
