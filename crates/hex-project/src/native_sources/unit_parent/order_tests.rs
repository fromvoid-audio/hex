use crate::{Command, Created, DeviceId, Editor, Invalid, Project};

#[test]
fn source_reorder_checks_typed_owner_and_keeps_bound_noops_harmless() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut sources = Vec::new();
    for count in [2, 1] {
        let Created::NativeSource(source) = editor
            .execute(Command::CreateNativeSource {
                name: "source".into(),
                devices: vec![crate::default_instrument_state().unwrap(); count],
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
        sources.push((source, instrument));
    }
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
            source: Some(sources[1].0),
        })
        .unwrap();
    editor
        .execute(Command::RenameNativeSource {
            source: sources[0].0,
            name: "temporary".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    for (source, device, anchor) in [
        (sources[0].0, sources[1].1, None),
        (sources[0].0, sources[0].1, Some(sources[1].1)),
        (crate::SourceId(999_999), sources[0].1, None),
        (sources[1].0, sources[1].1, Some(sources[0].1)),
    ] {
        assert!(
            editor
                .execute(Command::MoveNativeSourceDevice {
                    source,
                    device,
                    before: anchor
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
    for anchor in [None, Some(sources[1].1)] {
        editor
            .execute(Command::MoveNativeSourceDevice {
                source: sources[1].0,
                device: sources[1].1,
                before: anchor,
            })
            .unwrap();
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
    assert!(editor.redo());
}

#[test]
fn source_unit_moves_keep_complete_subtrees_instrument_identity_and_history() {
    let mut setup = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::NativeSource(source) = setup
        .execute(Command::CreateNativeSource {
            name: "source".into(),
            devices: vec![crate::default_instrument_state().unwrap(); 6],
            instrument_index: 2,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let mut snapshot = setup.project().snapshot();
    let devices = &mut snapshot.native_sources[0].devices;
    devices[1].parent = Some(devices[0].id);
    devices[3].parent = Some(devices[2].id);
    devices[4].parent = Some(devices[2].id);
    devices[3].bypassed = true;
    let project = Project::from_snapshot(snapshot).unwrap();
    let ids: Vec<_> = project
        .native_source(source)
        .unwrap()
        .devices()
        .map(crate::Device::id)
        .collect();
    for (moving, before, order, position) in [
        (2, Some(0), vec![2, 3, 4, 0, 1, 5], 0),
        (0, None, vec![2, 3, 4, 5, 0, 1], 0),
        (4, Some(3), vec![0, 1, 2, 4, 3, 5], 2),
        (5, Some(0), vec![5, 0, 1, 2, 3, 4], 3),
    ] {
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        let command = Command::MoveNativeSourceDevice {
            source,
            device: ids[moving],
            before: before.map(|index| ids[index]),
        };
        assert!(matches!(
            command.clip_scope(),
            crate::ClipCommandScope::Unrelated
        ));
        let before = editor.project().snapshot();
        let refused: Result<_, Invalid> = editor.execute_prepared(command.clone(), |_| {
            Err::<(), _>(Invalid("prepare refused"))
        });
        assert_eq!(refused.unwrap_err(), Invalid("prepare refused"));
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
        editor.execute(command).unwrap();
        let current = editor.project().native_source(source).unwrap();
        assert_eq!(
            current.devices().map(crate::Device::id).collect::<Vec<_>>(),
            order.into_iter().map(|i| ids[i]).collect::<Vec<_>>()
        );
        assert_eq!(current.instrument_index(), position);
        assert_eq!(current.instrument().id(), ids[2]);
        for device in current.devices() {
            assert_eq!(Some(device), project.device(device.id()));
        }
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
    }
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor
        .execute(Command::RenameNativeSource {
            source,
            name: "temporary".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    for anchor in [Some(ids[0]), Some(ids[1])] {
        editor
            .execute(Command::MoveNativeSourceDevice {
                source,
                device: ids[0],
                before: anchor,
            })
            .unwrap();
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
    for (device, anchor) in [
        (ids[0], Some(ids[3])),
        (ids[3], None),
        (ids[0], Some(DeviceId(999_999))),
        (DeviceId(999_999), None),
    ] {
        assert!(
            editor
                .execute(Command::MoveNativeSourceDevice {
                    source,
                    device,
                    before: anchor
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
    assert!(editor.redo());
}
