use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{
    Command, Created, DeviceComparison, DeviceComparisonAction as Action, DeviceId,
    DeviceVariant as Slot, Editor, Project,
};

fn state(value: u8) -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::from_static("test.device"), 1, vec![value]).unwrap()
}
fn fixture() -> (Editor, DeviceId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state(1),
        })
        .unwrap()
    else {
        panic!("device");
    };
    (Editor::new(editor.project().clone(), 32).unwrap(), device)
}
fn compare(editor: &mut Editor, device: DeviceId, action: Action) {
    let expected = editor.project().device(device).unwrap().active_variant();
    editor
        .execute(Command::CompareDevice {
            device,
            expected,
            action,
        })
        .unwrap();
}
fn value(editor: &Editor, device: DeviceId) -> u8 {
    editor.project().device(device).unwrap().state().payload()[0]
}

#[test]
fn two_independent_variants_copy_direction_and_exact_history() {
    let (mut editor, device) = fixture();
    compare(&mut editor, device, Action::CopyToInactive);
    assert!(!editor.can_undo());
    compare(&mut editor, device, Action::Switch);
    assert_eq!(value(&editor, device), 1);
    assert_eq!(
        editor.project().device(device).unwrap().active_variant(),
        Slot::B
    );
    assert!(editor.undo());
    assert!(!editor.can_undo());
    compare(&mut editor, device, Action::CopyToInactive);
    assert!(
        editor.can_redo(),
        "copying conceptual identical B must retain redo"
    );
    assert!(editor.redo());
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state(2),
        })
        .unwrap();
    compare(&mut editor, device, Action::Switch);
    assert_eq!(value(&editor, device), 1);
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state(3),
        })
        .unwrap();
    compare(&mut editor, device, Action::Switch);
    assert_eq!(value(&editor, device), 2);
    compare(&mut editor, device, Action::Switch);
    assert_eq!(value(&editor, device), 3);
    let before = editor.project().clone();
    compare(&mut editor, device, Action::CopyToInactive);
    assert_eq!(value(&editor, device), 3);
    assert_eq!(
        editor.project().device(device).unwrap().active_variant(),
        Slot::A
    );
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    compare(&mut editor, device, Action::Switch);
    assert_eq!(value(&editor, device), 3);
}

#[test]
fn host_trims_and_bypass_stay_global_while_provider_replacement_resets_pair() {
    let (mut editor, device) = fixture();
    compare(&mut editor, device, Action::Switch);
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state(9)
                .with_input_gain_millidb(-6000)
                .unwrap()
                .with_output_gain_millidb(3000)
                .unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let current = editor.project().device(device).unwrap();
    assert_eq!(current.authored_states().count(), 2);
    assert_eq!(current.comparison().unwrap().inactive().payload(), &[1]);
    for state in current.authored_states() {
        assert_eq!(state.input_gain_millidb(), -6000);
        assert_eq!(state.output_gain_millidb(), 3000);
    }
    compare(&mut editor, device, Action::Switch);
    let active = editor.project().device(device).unwrap();
    assert_eq!(active.state().payload(), &[1]);
    assert_eq!(active.state().input_gain_millidb(), -6000);
    assert_eq!(active.state().output_gain_millidb(), 3000);
    assert!(active.bypassed());
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: NativeDeviceState::new(DeviceTypeId::from_static("test.other"), 1, vec![7])
                .unwrap(),
        })
        .unwrap();
    let changed = editor.project().device(device).unwrap();
    assert_eq!(changed.active_variant(), Slot::A);
    assert!(changed.comparison().is_none());
    assert_eq!(changed.authored_states().count(), 1);
}

#[test]
fn stale_or_unpreparable_comparison_preserves_project_and_both_history_stacks() {
    let (mut editor, device) = fixture();
    compare(&mut editor, device, Action::Switch);
    let before = editor.project().clone();
    let mut preparations = 0;
    let result: Result<_, hex_project::Invalid> = editor.execute_prepared(
        Command::CompareDevice {
            device,
            expected: Slot::A,
            action: Action::Switch,
        },
        |_| {
            preparations += 1;
            Ok(())
        },
    );
    assert!(result.is_err());
    assert_eq!(preparations, 0);
    assert_eq!(editor.project(), &before);
    let result: Result<_, hex_project::Invalid> = editor.execute_prepared(
        Command::CompareDevice {
            device,
            expected: Slot::B,
            action: Action::Switch,
        },
        |_| Err::<(), _>(hex_project::Invalid("injected admission refusal")),
    );
    assert!(result.is_err());
    assert_eq!(editor.project(), &before);
    assert!(editor.undo());
    assert!(!editor.can_undo());
    assert!(editor.can_redo());
    let before = editor.project().clone();
    let result = editor.execute(Command::ReplaceDeviceComparison {
        device,
        state: state(1),
        comparison: Some(DeviceComparison::new(
            Slot::A,
            NativeDeviceState::new(DeviceTypeId::from_static("test.other"), 1, vec![]).unwrap(),
        )),
    });
    assert!(result.is_err());
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
}

#[test]
fn snapshot_duplicate_and_move_preserve_comparison_but_stale_removal_refuses() {
    let (mut editor, device) = fixture();
    compare(&mut editor, device, Action::Switch);
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state(2),
        })
        .unwrap();
    let restored = Project::from_snapshot(editor.project().snapshot()).unwrap();
    assert_eq!(&restored, editor.project());
    let Created::Track(destination) = editor
        .execute(Command::AddTrack {
            name: "destination".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    editor
        .execute(Command::MoveTrackDevice {
            device,
            track: destination,
            before: None,
        })
        .unwrap();
    assert_eq!(
        editor.project().device(device).unwrap().comparison(),
        restored.device(device).unwrap().comparison()
    );
    let mut snapshot = editor.project().snapshot();
    let mut duplicate = snapshot.tracks[1].devices[0].clone();
    duplicate.id = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.tracks[1].devices.push(duplicate);
    let mut cloned = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    let copy = cloned.project().tracks()[1].devices()[1].id();
    cloned
        .execute(Command::ReplaceDeviceState {
            device: copy,
            state: state(8),
        })
        .unwrap();
    assert_eq!(value(&cloned, device), 2);
    assert_eq!(value(&cloned, copy), 8);
    editor.execute(Command::RemoveDevice(device)).unwrap();
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::CompareDevice {
                device,
                expected: Slot::B,
                action: Action::Switch
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
}

#[test]
fn inactive_asset_binding_remains_valid_and_prevents_media_deletion() {
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    let (mut editor, device) = fixture();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "Samples/source.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let bound = state(1)
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: bound,
        })
        .unwrap();
    compare(&mut editor, device, Action::Switch);
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state(2),
        })
        .unwrap();
    assert!(
        editor
            .project()
            .device(device)
            .unwrap()
            .state()
            .asset_references()
            .is_empty()
    );
    assert_eq!(
        editor
            .project()
            .device(device)
            .unwrap()
            .comparison()
            .unwrap()
            .inactive()
            .asset_references()[0]
            .asset
            .value(),
        asset.value()
    );
    let before = editor.project().clone();
    assert!(editor.execute(Command::RemoveAsset(asset)).is_err());
    assert_eq!(editor.project(), &before);
    let mut snapshot = before.snapshot();
    snapshot.assets.clear();
    assert!(
        Project::from_snapshot(snapshot).is_err(),
        "inactive references are part of current-format integrity"
    );
    compare(&mut editor, device, Action::CopyToInactive);
    editor.execute(Command::RemoveAsset(asset)).unwrap();
    assert!(editor.project().asset(asset).is_none());
}
