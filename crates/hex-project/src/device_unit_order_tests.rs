use crate::{Command, Created, DeviceId, Editor, Project};

fn fixture(return_chain: bool) -> (Editor, Vec<DeviceId>, DeviceId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::ReturnTrack(other) = editor
        .execute(Command::AddReturnTrack {
            name: "other".into(),
        })
        .unwrap()
    else {
        panic!("other")
    };
    let mut ids = Vec::new();
    for _ in 0..7 {
        let state = crate::default_instrument_state().unwrap();
        let command = if return_chain {
            Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state,
            }
        } else {
            Command::InsertMainDevice {
                before: None,
                state,
            }
        };
        let Created::Device(id) = editor.execute(command).unwrap() else {
            panic!("device")
        };
        ids.push(id);
    }
    let Created::Device(foreign) = editor
        .execute(Command::InsertReturnDevice {
            return_track: other,
            before: None,
            state: crate::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("foreign")
    };
    let mut snapshot = editor.project().snapshot();
    let devices = if return_chain {
        &mut snapshot.return_tracks[0].devices
    } else {
        &mut snapshot.main_devices
    };
    devices[1].parent = Some(ids[0].value());
    devices[2].parent = Some(ids[1].value());
    devices[3].parent = Some(ids[0].value());
    devices[5].parent = Some(ids[4].value());
    devices[2].bypassed = true;
    devices[2].comparison = Some(crate::DeviceComparison::new(
        crate::DeviceVariant::B,
        devices[2].state.clone(),
    ));
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap(),
        ids,
        foreign,
    )
}

fn command(return_chain: bool, device: DeviceId, before: Option<DeviceId>) -> Command {
    if return_chain {
        Command::MoveReturnDevice { device, before }
    } else {
        Command::MoveMainDevice { device, before }
    }
}

fn order(editor: &Editor, return_chain: bool) -> Vec<DeviceId> {
    let devices = if return_chain {
        editor.project().return_tracks()[0].devices()
    } else {
        editor.project().main_devices()
    };
    devices.iter().map(crate::Device::id).collect()
}

#[test]
fn roots_and_nested_units_reorder_whole_authored_subtrees_in_one_history_step() {
    for return_chain in [false, true] {
        for case in 0..4 {
            let (mut editor, ids, _) = fixture(return_chain);
            let before = editor.project().snapshot();
            let authored = editor.project().devices().cloned().collect::<Vec<_>>();
            let (device, anchor, expected) = match case {
                0 => (ids[0], None, vec![4, 5, 6, 0, 1, 2, 3]),
                1 => (ids[4], Some(ids[0]), vec![4, 5, 0, 1, 2, 3, 6]),
                2 => (ids[1], Some(ids[4]), vec![0, 3, 1, 2, 4, 5, 6]),
                _ => (ids[3], Some(ids[1]), vec![0, 3, 1, 2, 4, 5, 6]),
            };
            editor
                .execute(command(return_chain, device, anchor))
                .unwrap();
            assert_eq!(
                order(&editor, return_chain),
                expected.into_iter().map(|i| ids[i]).collect::<Vec<_>>()
            );
            for device in authored {
                assert_eq!(editor.project().device(device.id()), Some(&device));
            }
            let after = editor.project().snapshot();
            assert!(editor.undo());
            assert_eq!(editor.project().snapshot(), before);
            assert!(editor.redo());
            assert_eq!(editor.project().snapshot(), after);
        }
    }
}

#[test]
fn own_span_policy_and_invalid_targets_preserve_state_and_history() {
    for return_chain in [false, true] {
        let (mut editor, ids, foreign) = fixture(return_chain);
        let before = editor.project().snapshot();
        for anchor in [ids[0], ids[2]] {
            assert_eq!(
                editor
                    .execute(command(return_chain, ids[0], Some(anchor)))
                    .is_ok(),
                !return_chain
            );
            assert_eq!(editor.project().snapshot(), before);
            assert!(!editor.can_undo());
        }
        for (device, anchor) in [(ids[0], foreign), (ids[6], ids[5])] {
            assert!(
                editor
                    .execute(command(return_chain, device, Some(anchor)))
                    .is_err()
            );
            assert_eq!(editor.project().snapshot(), before);
            assert!(!editor.can_undo());
        }
        editor.execute(Command::RemoveDevice(ids[6])).unwrap();
        let removed = editor.project().snapshot();
        assert!(
            editor
                .execute(command(return_chain, ids[0], Some(ids[6])))
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), removed);
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
        assert!(
            editor
                .execute(command(return_chain, ids[0], Some(foreign)))
                .is_err()
        );
        assert!(editor.can_redo());
    }
}
