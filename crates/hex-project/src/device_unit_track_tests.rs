use crate::{Command, Created, DeviceId, Editor, Project, TrackId};

fn fixture(position: usize) -> (Editor, TrackId, TrackId, DeviceId, Vec<DeviceId>) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("source")
    };
    let Created::Track(other) = editor
        .execute(Command::AddMidiTrack {
            name: "other".into(),
        })
        .unwrap()
    else {
        panic!("other")
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
    let mut ids = Vec::new();
    for _ in 0..7 {
        let Created::Device(id) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: crate::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("effect")
        };
        ids.push(id);
    }
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].instrument_position = Some(position);
    let devices = &mut snapshot.tracks[0].devices;
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
        track,
        other,
        instrument,
        ids,
    )
}

fn command(mixed: bool, device: DeviceId, track: TrackId, before: Option<DeviceId>) -> Command {
    if mixed {
        Command::MoveTrackDevice {
            device,
            track,
            before,
        }
    } else {
        Command::MoveDevice {
            device,
            track,
            before,
        }
    }
}

#[test]
fn local_unit_moves_preserve_authored_subtrees_and_account_for_the_full_instrument_gap() {
    for mixed in [false, true] {
        for (position, moving, anchor, expected, gap) in [
            (4, 0, Some(4), vec![0, 1, 2, 3, 4, 5, 6], 0),
            (4, 0, None, vec![4, 5, 6, 0, 1, 2, 3], 0),
            (4, 4, Some(0), vec![4, 5, 0, 1, 2, 3, 6], 6),
            (0, 1, Some(4), vec![0, 3, 1, 2, 4, 5, 6], 0),
            (7, 4, Some(0), vec![4, 5, 0, 1, 2, 3, 6], 7),
        ] {
            let (mut editor, track, _, _, ids) = fixture(position);
            let before = editor.project().snapshot();
            let authored = editor.project().devices().cloned().collect::<Vec<_>>();
            editor
                .execute(command(mixed, ids[moving], track, anchor.map(|i| ids[i])))
                .unwrap();
            let current = editor.project().track(track).unwrap();
            assert_eq!(
                current
                    .devices()
                    .iter()
                    .map(crate::Device::id)
                    .collect::<Vec<_>>(),
                expected.into_iter().map(|i| ids[i]).collect::<Vec<_>>()
            );
            assert_eq!(current.instrument_position(), Some(gap));
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
fn explicit_instrument_anchor_changes_position_even_when_the_effect_order_is_unchanged() {
    let (mut editor, track, _, instrument, ids) = fixture(0);
    let before = editor.project().snapshot();
    editor
        .execute(command(true, ids[0], track, Some(instrument)))
        .unwrap();
    assert_eq!(
        editor.project().track(track).unwrap().instrument_position(),
        Some(4)
    );
    assert_eq!(
        editor
            .project()
            .track(track)
            .unwrap()
            .devices()
            .iter()
            .map(crate::Device::id)
            .collect::<Vec<_>>(),
        ids
    );
    let ahead = editor.project().snapshot();
    editor
        .execute(command(true, ids[0], track, Some(ids[4])))
        .unwrap();
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), ahead);
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn self_invalid_placement_and_unsupported_transfer_preserve_the_track_and_history() {
    for mixed in [false, true] {
        let (mut editor, track, other, _, ids) = fixture(4);
        let before = editor.project().snapshot();
        editor
            .execute(command(mixed, ids[0], track, Some(ids[2])))
            .unwrap();
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
        for (device, target, anchor) in [
            (ids[6], track, Some(ids[5])),
            (ids[1], track, Some(ids[4])),
            (ids[0], other, None),
            (ids[3], other, None),
        ] {
            assert!(
                editor
                    .execute(command(mixed, device, target, anchor))
                    .is_err()
            );
            assert_eq!(editor.project().snapshot(), before);
            assert!(!editor.can_undo());
        }
        editor.execute(Command::RemoveDevice(ids[6])).unwrap();
        let removed = editor.project().snapshot();
        assert!(
            editor
                .execute(command(mixed, ids[0], track, Some(ids[6])))
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), removed);
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
        assert!(editor.execute(command(mixed, ids[0], other, None)).is_err());
        assert!(editor.can_redo());
    }
}
