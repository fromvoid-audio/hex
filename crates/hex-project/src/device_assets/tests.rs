use super::*;
use crate::{Command, Created, DeviceId, Editor};
use hex_lattice_contract::{DeviceAssetSlot, DeviceTypeId, NativeDeviceState};

fn fixture() -> (Editor, DeviceId, AssetId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "old.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let state = editor
        .project()
        .device(device)
        .unwrap()
        .state()
        .clone()
        .with_input_gain_millidb(-600)
        .unwrap()
        .with_output_gain_millidb(300)
        .unwrap()
        .with_asset_references(
            [1, 7]
                .map(|slot| DeviceAssetReference {
                    slot: DeviceAssetSlot::new(slot).unwrap(),
                    asset: DeviceAssetKey::new(asset.value()).unwrap(),
                })
                .to_vec(),
        )
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    (
        Editor::new(editor.project().clone(), 8).unwrap(),
        device,
        asset,
    )
}
fn command(editor: &Editor, device: DeviceId) -> Command {
    let expected_state = editor.project().device(device).unwrap().state().clone();
    // A reconstructed DSP payload has no bindings/host gains of its own.
    // The canonical import must preserve those from the actual expected state.
    let state = NativeDeviceState::new(expected_state.type_id(), 1, vec![4, 3, 2, 1]).unwrap();
    Command::ImportDeviceAsset {
        device,
        expected_state,
        state,
        slot: DeviceAssetSlot::new(1).unwrap(),
        path: "replacement.wav".into(),
        source_sample_rate: 24_000,
        source_frames: 2,
    }
}

#[test]
fn device_asset_import_is_one_edit_and_round_trips_original_metadata_and_all_bindings() {
    let (mut editor, device, old) = fixture();
    let original = editor.project().device(device).unwrap().state().clone();
    let Created::Asset(asset) = editor.execute(command(&editor, device)).unwrap() else {
        panic!("asset")
    };
    assert_ne!(asset, old);
    let metadata = editor.project().asset(asset).unwrap();
    assert_eq!(metadata.source_sample_rate(), 24_000);
    assert_eq!(metadata.source_frames(), 2);
    assert_eq!(metadata.playback_frames(), 4);
    let assigned = editor.project().device(device).unwrap().state().clone();
    assert_eq!(assigned.payload(), &[4, 3, 2, 1]);
    assert_eq!(assigned.input_gain_millidb(), -600);
    assert_eq!(assigned.output_gain_millidb(), 300);
    assert_eq!(
        assigned
            .asset_references()
            .iter()
            .map(|r| (r.slot.value(), r.asset.value()))
            .collect::<Vec<_>>(),
        vec![(1, asset.value()), (7, old.value())]
    );
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
    assert!(editor.undo());
    assert_eq!(editor.project().device(device).unwrap().state(), &original);
    assert!(editor.project().asset(asset).is_none());
    assert!(editor.project().asset(old).is_some());
    assert!(!editor.can_undo());
    assert!(editor.redo());
    assert_eq!(editor.project().device(device).unwrap().state(), &assigned);
    assert!(editor.project().asset(asset).is_some());
}

#[test]
fn failed_preparation_or_stale_target_preserves_project_ids_and_both_history_stacks() {
    let (mut editor, device, _) = fixture();
    let state = editor
        .project()
        .device(device)
        .unwrap()
        .state()
        .clone()
        .with_output_gain_millidb(600)
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let edit = command(&editor, device);
    assert!(
        editor
            .execute_prepared(edit, |_| Err::<(), _>(Invalid("graph refused")))
            .is_err()
    );
    assert_eq!(*editor.project(), before);
    assert!(!editor.can_undo());
    assert!(editor.can_redo());
    let stale = command(&editor, device);
    assert!(editor.redo());
    let before = editor.project().clone();
    assert!(editor.execute(stale).is_err());
    assert_eq!(*editor.project(), before);
    assert!(editor.can_undo());
    assert!(!editor.can_redo());
}

#[test]
fn malformed_import_metadata_or_wrong_device_class_never_commits_an_asset() {
    let (mut editor, device, _) = fixture();
    for (rate, frames) in [
        (0, 2),
        (7_999, 2),
        (192_001, 2),
        (24_000, 0),
        (8_000, u64::MAX),
    ] {
        let mut edit = command(&editor, device);
        let Command::ImportDeviceAsset {
            source_sample_rate,
            source_frames,
            ..
        } = &mut edit
        else {
            panic!("command")
        };
        *source_sample_rate = rate;
        *source_frames = frames;
        let before = editor.project().clone();
        assert!(editor.execute(edit).is_err());
        assert_eq!(*editor.project(), before);
        assert!(!editor.can_undo());
    }
    let mut edit = command(&editor, device);
    let Command::ImportDeviceAsset { state, .. } = &mut edit else {
        panic!("command")
    };
    *state = NativeDeviceState::new(DeviceTypeId::from_static("test.foreign"), 1, vec![]).unwrap();
    let before = editor.project().clone();
    assert!(editor.execute(edit).is_err());
    assert_eq!(*editor.project(), before);
}

#[test]
fn full_binding_capacity_allows_replacement_but_refuses_an_extra_slot_atomically() {
    let (mut editor, device, old) = fixture();
    let state = editor
        .project()
        .device(device)
        .unwrap()
        .state()
        .clone()
        .with_asset_references(
            (2..=129)
                .map(|slot| DeviceAssetReference {
                    slot: DeviceAssetSlot::new(slot).unwrap(),
                    asset: DeviceAssetKey::new(old.value()).unwrap(),
                })
                .collect(),
        )
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    let mut editor = Editor::new(editor.project().clone(), 8).unwrap();
    let before = editor.project().clone();
    assert!(editor.execute(command(&editor, device)).is_err()); // slot1 would be the129th
    assert_eq!(*editor.project(), before);
    assert!(!editor.can_undo());
    let mut replace = command(&editor, device);
    let Command::ImportDeviceAsset { slot, .. } = &mut replace else {
        panic!("command")
    };
    *slot = DeviceAssetSlot::new(2).unwrap();
    let Created::Asset(asset) = editor.execute(replace).unwrap() else {
        panic!("asset")
    };
    let references = editor
        .project()
        .device(device)
        .unwrap()
        .state()
        .asset_references();
    assert_eq!(references.len(), MAX_DEVICE_ASSET_REFERENCES);
    assert_eq!(references[0].asset.value(), asset.value());
    assert!(
        references[1..]
            .iter()
            .all(|reference| reference.asset.value() == old.value())
    );
}
