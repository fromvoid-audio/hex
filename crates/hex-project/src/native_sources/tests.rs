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
