use crate::{Command, Created, DeviceId, Editor, Invalid, Project};

#[test]
fn source_effect_subtrees_remove_in_one_prepared_history_step_without_losing_media() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "inactive.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "source".into(),
            devices: vec![crate::default_instrument_state().unwrap(); 6],
            instrument_index: 2,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let mut snapshot = editor.project().snapshot();
    let devices = &mut snapshot.native_sources[0].devices;
    devices[1].parent = Some(devices[0].id);
    devices[3].parent = Some(devices[2].id);
    devices[4].parent = Some(devices[3].id);
    devices[4].bypassed = true;
    devices[4].comparison = Some(crate::DeviceComparison::new(
        crate::DeviceVariant::B,
        devices[4]
            .state
            .clone()
            .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
                slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
                asset: hex_lattice_contract::DeviceAssetKey::new(asset.value()).unwrap(),
            }])
            .unwrap(),
    ));
    let project = Project::from_snapshot(snapshot).unwrap();
    let original = project.native_source(source).unwrap().clone();
    let ids: Vec<_> = original.devices().map(crate::Device::id).collect();
    for (selected, retained, position, media_retained) in [
        (ids[0], vec![ids[2], ids[3], ids[4], ids[5]], 0, true),
        (ids[3], vec![ids[0], ids[1], ids[2], ids[5]], 2, false),
    ] {
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        let before = editor.project().snapshot();
        let refused: Result<_, Invalid> = editor
            .execute_prepared(Command::RemoveDevice(selected), |_| {
                Err::<(), _>(Invalid("prepare refused"))
            });
        assert_eq!(refused.unwrap_err(), Invalid("prepare refused"));
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
        editor.execute(Command::RemoveDevice(selected)).unwrap();
        let current = editor.project().native_source(source).unwrap();
        assert_eq!(
            current.devices().map(crate::Device::id).collect::<Vec<_>>(),
            retained
        );
        assert_eq!(current.instrument_index(), position);
        assert_eq!(current.instrument(), original.instrument());
        for device in current.devices() {
            assert_eq!(
                Some(device),
                original.devices().find(|old| old.id() == device.id())
            );
        }
        assert_eq!(original.devices().count(), 6);
        let after = editor.project().snapshot();
        assert_eq!(
            Project::from_snapshot(after.clone()).unwrap(),
            *editor.project()
        );
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        assert!(editor.execute(Command::RemoveDevice(selected)).is_err());
        assert_eq!(editor.project().snapshot(), after);
        let mut media = Editor::new(editor.project().clone(), 8).unwrap();
        assert_eq!(
            media.execute(Command::RemoveAsset(asset)).is_err(),
            media_retained
        );
        assert_eq!(
            Project::from_snapshot(media.project().snapshot()).unwrap(),
            *media.project()
        );
    }
}

#[test]
fn source_instrument_and_missing_device_removal_refuse_without_losing_redo() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "source".into(),
            devices: vec![crate::default_instrument_state().unwrap()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let instrument = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    editor
        .execute(Command::RenameNativeSource {
            source,
            name: "temporary".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    for device in [instrument, DeviceId(999_999)] {
        assert!(editor.execute(Command::RemoveDevice(device)).is_err());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
    assert!(editor.redo());
    editor.execute(Command::RemoveNativeSource(source)).unwrap();
    assert!(editor.project().native_source(source).is_none());
    assert!(editor.undo());
    assert_eq!(
        editor
            .project()
            .native_source(source)
            .unwrap()
            .instrument()
            .id(),
        instrument
    );
}
