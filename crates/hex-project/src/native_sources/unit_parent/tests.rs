use crate::{Command, Created, DeviceId, Editor, Invalid, Project, SourceId};

fn fixture(count: usize, instrument_index: usize) -> (Editor, SourceId, Vec<DeviceId>) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "unit".into(),
            devices: vec![crate::default_instrument_state().unwrap(); count],
            instrument_index,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let ids = editor
        .project()
        .native_source(source)
        .unwrap()
        .devices()
        .map(crate::Device::id)
        .collect();
    (
        Editor::new(editor.project().clone(), 16).unwrap(),
        source,
        ids,
    )
}

#[test]
fn initialized_source_children_insert_in_one_prepared_history_step() {
    let (mut editor, source, ids) = fixture(3, 1);
    let original = editor.project().native_source(source).unwrap().clone();
    let state = crate::default_instrument_state()
        .unwrap()
        .with_output_gain_millidb(-3000)
        .unwrap();
    let comparison = crate::DeviceComparison::new(crate::DeviceVariant::B, state.clone());
    let command = Command::InitializeDevice(Box::new(
        crate::DeviceInitialization::new(
            Command::InsertNestedDevice {
                parent: ids[1],
                state: state.clone(),
            },
            Some(comparison.clone()),
        )
        .unwrap(),
    ));
    let before = editor.project().snapshot();
    let refused: Result<_, Invalid> = editor.execute_prepared(command.clone(), |_| {
        Err::<(), _>(Invalid("prepare refused"))
    });
    assert_eq!(refused.unwrap_err(), Invalid("prepare refused"));
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
    let Created::Device(child) = editor.execute(command).unwrap() else {
        panic!("child")
    };
    let authored = editor.project().device(child).unwrap();
    assert_eq!(authored.state(), &state);
    assert_eq!(authored.comparison(), Some(&comparison));
    assert_eq!(authored.parent(), Some(ids[1]));
    assert_eq!(original.devices().count(), 3);
    let current = editor.project().native_source(source).unwrap();
    assert_eq!(
        current.devices().map(crate::Device::id).collect::<Vec<_>>(),
        [ids[0], ids[1], child, ids[2]]
    );
    assert_eq!(current.instrument_index(), 1);
    let after = editor.project().snapshot();
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap(),
        *editor.project()
    );
    assert!(editor.undo());
    assert_eq!(editor.project().native_source(source), Some(&original));
    assert!(!editor.can_undo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
    let Created::Device(grandchild) = editor
        .execute(Command::InsertNestedDevice {
            parent: child,
            state,
        })
        .unwrap()
    else {
        panic!("grandchild")
    };
    assert_eq!(
        editor.project().device(grandchild).unwrap().parent(),
        Some(child)
    );
    assert_eq!(
        editor
            .project()
            .native_source(source)
            .unwrap()
            .devices()
            .map(crate::Device::id)
            .collect::<Vec<_>>(),
        [ids[0], ids[1], child, grandchild, ids[2]]
    );
}

#[test]
fn source_reparenting_moves_complete_units_and_recomputes_instrument_position() {
    let (editor, source, ids) = fixture(5, 1);
    let mut snapshot = editor.project().snapshot();
    snapshot.native_sources[0].devices[3].parent = Some(ids[2].value());
    snapshot.native_sources[0].devices[3].bypassed = true;
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    for (device, parent, expected, position) in [
        (
            ids[2],
            Some(ids[1]),
            vec![ids[0], ids[1], ids[2], ids[3], ids[4]],
            1,
        ),
        (
            ids[0],
            Some(ids[2]),
            vec![ids[1], ids[2], ids[3], ids[0], ids[4]],
            0,
        ),
        (
            ids[2],
            None,
            vec![ids[1], ids[2], ids[3], ids[0], ids[4]],
            0,
        ),
    ] {
        let before = editor.project().snapshot();
        editor
            .execute(Command::ReparentDevice { device, parent })
            .unwrap();
        let current = editor.project().native_source(source).unwrap();
        assert_eq!(
            current.devices().map(crate::Device::id).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(current.instrument_index(), position);
        assert_eq!(current.instrument().id(), ids[1]);
        assert_eq!(current.instrument().parent(), None);
        assert_eq!(editor.project().device(device).unwrap().parent(), parent);
        assert!(editor.project().device(ids[3]).unwrap().bypassed());
        let after = editor.project().snapshot();
        assert_eq!(
            Project::from_snapshot(after.clone()).unwrap(),
            *editor.project()
        );
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}

#[test]
fn invalid_source_membership_edits_preserve_registry_identities_and_redo() {
    let (mut editor, source, ids) = fixture(4, 0);
    let Created::NativeSource(other) = editor
        .execute(Command::CreateNativeSource {
            name: "other".into(),
            devices: vec![crate::default_instrument_state().unwrap()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let foreign = editor
        .project()
        .native_source(other)
        .unwrap()
        .instrument()
        .id();
    editor
        .execute(Command::ReparentDevice {
            device: ids[2],
            parent: Some(ids[1]),
        })
        .unwrap();
    editor
        .execute(Command::RenameNativeSource {
            source,
            name: "temporary".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    for command in [
        Command::ReparentDevice {
            device: ids[1],
            parent: Some(ids[1]),
        },
        Command::ReparentDevice {
            device: ids[1],
            parent: Some(ids[2]),
        },
        Command::ReparentDevice {
            device: ids[1],
            parent: Some(foreign),
        },
        Command::ReparentDevice {
            device: ids[0],
            parent: Some(ids[3]),
        },
        Command::InsertNestedDevice {
            parent: ids[1],
            state: crate::default_instrument_state()
                .unwrap()
                .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
                    slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
                    asset: hex_lattice_contract::DeviceAssetKey::new(999_999).unwrap(),
                }])
                .unwrap(),
        },
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
    assert!(editor.redo());
}

#[test]
fn source_depth_and_node_limits_refuse_atomically() {
    let (editor, _, ids) = fixture(9, 0);
    let mut snapshot = editor.project().snapshot();
    for index in 1..8 {
        snapshot.native_sources[0].devices[index].parent = Some(ids[index - 1].value());
    }
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    let before = editor.project().snapshot();
    for command in [
        Command::ReparentDevice {
            device: ids[8],
            parent: Some(ids[7]),
        },
        Command::InsertNestedDevice {
            parent: ids[7],
            state: crate::default_instrument_state().unwrap(),
        },
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
    }
    let (mut editor, _, ids) = fixture(super::super::MAX_SOURCE_DEVICES, 0);
    let before = editor.project().snapshot();
    assert_eq!(
        editor
            .execute(Command::InsertNestedDevice {
                parent: ids[0],
                state: crate::default_instrument_state().unwrap()
            })
            .unwrap_err(),
        Invalid("source device limit reached")
    );
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
}

#[test]
fn bound_singleton_refuses_child_insertion_but_allows_noop_membership() {
    let (mut editor, source, ids) = fixture(1, 0);
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "bound".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::BindTrackSource {
            track,
            source: Some(source),
        })
        .unwrap();
    editor
        .execute(Command::RenameNativeSource {
            source,
            name: "temporary".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    assert_eq!(
        editor
            .execute(Command::InsertNestedDevice {
                parent: ids[0],
                state: crate::default_instrument_state().unwrap()
            })
            .unwrap_err(),
        Invalid("editing bound source chains is not supported yet")
    );
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.can_redo());
    editor
        .execute(Command::ReparentDevice {
            device: ids[0],
            parent: None,
        })
        .unwrap();
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.can_redo());
    assert!(editor.redo());
}
