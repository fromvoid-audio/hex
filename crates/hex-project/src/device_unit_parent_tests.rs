use crate::{Command, Created, Device, DeviceChainOwner, DeviceId, Editor, Project};

fn fixture(kind: usize) -> (Editor, DeviceChainOwner, Vec<DeviceId>, DeviceId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Device(instrument) = editor
        .execute(Command::InsertInstrument {
            track,
            state: crate::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument")
    };
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let owner = match kind {
        0 => DeviceChainOwner::Main,
        1 => DeviceChainOwner::Return(bus),
        _ => DeviceChainOwner::Track(track),
    };
    let mut ids = Vec::new();
    for _ in 0..7 {
        let state = crate::default_instrument_state().unwrap();
        let command = match owner {
            DeviceChainOwner::Main => Command::InsertMainDevice {
                before: None,
                state,
            },
            DeviceChainOwner::Return(return_track) => Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            },
            DeviceChainOwner::Track(track) => Command::InsertDevice {
                track,
                before: None,
                state,
            },
        };
        let Created::Device(id) = editor.execute(command).unwrap() else {
            panic!("effect")
        };
        ids.push(id);
    }
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].instrument_position = Some(if kind == 2 { 4 } else { 0 });
    let chain = match owner {
        DeviceChainOwner::Main => &mut snapshot.main_devices,
        DeviceChainOwner::Return(_) => &mut snapshot.return_tracks[0].devices,
        DeviceChainOwner::Track(_) => &mut snapshot.tracks[0].devices,
    };
    chain[1].parent = Some(ids[0].value());
    chain[2].parent = Some(ids[1].value());
    chain[3].parent = Some(ids[0].value());
    chain[5].parent = Some(ids[4].value());
    chain[2].bypassed = true;
    chain[2].comparison = Some(crate::DeviceComparison::new(
        crate::DeviceVariant::B,
        chain[2].state.clone(),
    ));
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap(),
        owner,
        ids,
        instrument,
    )
}
fn chain(project: &Project, owner: DeviceChainOwner) -> &[Device] {
    match owner {
        DeviceChainOwner::Main => project.main_devices(),
        DeviceChainOwner::Return(id) => project
            .return_tracks()
            .iter()
            .find(|t| t.id() == id)
            .unwrap()
            .devices(),
        DeviceChainOwner::Track(id) => project.track(id).unwrap().devices(),
    }
}

#[test]
fn attaching_and_detaching_preserve_complete_units_and_instrument_adjacent_membership() {
    for kind in 0..3 {
        for (moving, parent, order, gap) in [
            (4, Some(0), vec![0, 1, 2, 3, 4, 5, 6], 6),
            (6, Some(1), vec![0, 1, 2, 6, 3, 4, 5], 5),
            (1, Some(4), vec![0, 3, 4, 5, 1, 2, 6], 2),
            (1, None, vec![0, 3, 1, 2, 4, 5, 6], 4),
        ] {
            let (mut editor, owner, ids, _) = fixture(kind);
            let before = editor.project().snapshot();
            let authored = editor.project().devices().cloned().collect::<Vec<_>>();
            editor
                .execute(Command::ReparentDevice {
                    device: ids[moving],
                    parent: parent.map(|i| ids[i]),
                })
                .unwrap();
            assert_eq!(
                chain(editor.project(), owner)
                    .iter()
                    .map(Device::id)
                    .collect::<Vec<_>>(),
                order.into_iter().map(|i| ids[i]).collect::<Vec<_>>()
            );
            for mut device in authored {
                if device.id() == ids[moving] {
                    device.parent = parent.map(|i| ids[i]);
                }
                assert_eq!(editor.project().device(device.id()), Some(&device));
            }
            if let DeviceChainOwner::Track(track) = owner {
                assert_eq!(
                    editor.project().track(track).unwrap().instrument_position(),
                    Some(gap)
                );
            }
            let after = editor.project().snapshot();
            Project::from_snapshot(after.clone()).unwrap();
            assert!(editor.undo());
            assert_eq!(editor.project().snapshot(), before);
            assert!(editor.redo());
            assert_eq!(editor.project().snapshot(), after);
        }
    }
}

#[test]
fn reparent_refusals_and_unchanged_membership_preserve_project_and_redo() {
    for kind in 0..3 {
        let (mut editor, owner, ids, instrument) = fixture(kind);
        let state = crate::default_instrument_state().unwrap();
        let foreign_command = if kind == 0 {
            Command::InsertReturnDevice {
                return_track: editor.project().return_tracks()[0].id(),
                before: None,
                state,
            }
        } else {
            Command::InsertMainDevice {
                before: None,
                state,
            }
        };
        let Created::Device(foreign) = editor.execute(foreign_command).unwrap() else {
            panic!("foreign")
        };
        let mut editor = Editor::new(editor.project().clone(), 16).unwrap();
        let before = editor.project().snapshot();
        for (device, parent) in [
            (ids[0], Some(ids[0])),
            (ids[0], Some(ids[2])),
            (ids[0], Some(foreign)),
            (ids[0], Some(DeviceId(u64::MAX))),
            (DeviceId(u64::MAX), None),
            (instrument, Some(ids[0])),
            (ids[0], Some(instrument)),
        ] {
            assert!(
                editor
                    .execute(Command::ReparentDevice { device, parent })
                    .is_err()
            );
            assert_eq!(editor.project().snapshot(), before);
            assert!(!editor.can_undo());
        }
        let reached = std::cell::Cell::new(false);
        let refused: Result<_, crate::Invalid> = editor.execute_prepared(
            Command::ReparentDevice {
                device: ids[4],
                parent: Some(ids[0]),
            },
            |candidate| {
                reached.set(true);
                assert_eq!(candidate.device(ids[4]).unwrap().parent(), Some(ids[0]));
                Err::<(), _>(crate::Invalid("preparation refused"))
            },
        );
        assert!(refused.is_err());
        assert!(reached.get());
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
        editor.execute(Command::RemoveDevice(ids[6])).unwrap();
        assert!(editor.undo());
        assert!(editor.can_redo());
        for (device, parent) in [(ids[1], Some(ids[0])), (ids[0], None)] {
            let command = Command::ReparentDevice { device, parent };
            assert_eq!(command.clip_scope(), crate::ClipCommandScope::Unrelated);
            editor.execute(command).unwrap();
            assert!(editor.can_redo());
            assert_eq!(editor.project().snapshot(), before);
        }
        let mut deep = before;
        let next = deep.next_id;
        deep.next_id += 2;
        let devices = match owner {
            DeviceChainOwner::Main => &mut deep.main_devices,
            DeviceChainOwner::Return(_) => &mut deep.return_tracks[0].devices,
            DeviceChainOwner::Track(_) => &mut deep.tracks[0].devices,
        };
        for id in [next, next + 1] {
            let mut leaf = devices[6].clone();
            leaf.id = id;
            leaf.parent = None;
            devices.push(leaf);
        }
        for i in 0..8 {
            devices[i].parent = if i == 0 {
                None
            } else {
                Some(devices[i - 1].id)
            };
        }
        let parent = DeviceId(devices[7].id);
        let leaf = DeviceId(devices[8].id);
        if kind == 2 {
            deep.tracks[0].instrument_position = Some(8);
        }
        let mut editor = Editor::new(Project::from_snapshot(deep.clone()).unwrap(), 16).unwrap();
        assert!(
            editor
                .execute(Command::ReparentDevice {
                    device: leaf,
                    parent: Some(parent)
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), deep);
        assert!(!editor.can_undo());
    }
}
