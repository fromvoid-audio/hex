use crate::{Command, Created, DeviceId, DeviceSelectionIds, Editor, Project, TrackId};

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
fn selected_track_move_keeps_subtrees_and_instrument_in_source_order() {
    let (mut editor, source, destination, instrument, ids) = fixture(4);
    let Created::Device(existing) = editor
        .execute(Command::InsertDevice {
            track: destination,
            before: None,
            state: crate::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("destination effect")
    };
    let move_selected = |ids: &[DeviceId]| Command::MoveTrackDevices {
        source,
        selected: DeviceSelectionIds::new(ids).unwrap(),
        destination,
    };
    let selected = [ids[6], instrument, ids[2], ids[0]];
    editor
        .execute(Command::InsertInstrument {
            track: destination,
            state: crate::default_instrument_state().unwrap(),
        })
        .unwrap();
    let occupied = editor.project().snapshot();
    assert!(editor.execute(move_selected(&selected)).is_err());
    assert_eq!(editor.project().snapshot(), occupied);
    assert!(editor.undo());
    assert!(
        editor
            .project()
            .track(destination)
            .unwrap()
            .instrument()
            .is_none()
    );

    let before = editor.project().snapshot();
    for invalid in [[ids[1], ids[6]], [ids[0], existing]] {
        assert!(editor.execute(move_selected(&invalid)).is_err());
        assert_eq!(editor.project().snapshot(), before);
    }
    let refused = editor.execute_validated(move_selected(&selected), |candidate| {
        assert_eq!(
            candidate
                .track(destination)
                .unwrap()
                .instrument()
                .unwrap()
                .id(),
            instrument
        );
        Err(crate::Invalid("preparation refused"))
    });
    assert_eq!(refused.unwrap_err(), crate::Invalid("preparation refused"));
    assert_eq!(editor.project().snapshot(), before);
    let original = editor.project().clone();
    editor.execute(move_selected(&selected)).unwrap();
    let project = editor.project();
    let target = project.track(destination).unwrap();
    assert_eq!(target.instrument().unwrap().id(), instrument);
    assert_eq!(target.instrument_position(), Some(5));
    assert_eq!(
        target
            .devices()
            .iter()
            .map(crate::Device::id)
            .collect::<Vec<_>>(),
        [existing, ids[0], ids[1], ids[2], ids[3], ids[6]]
    );
    assert_eq!(
        project
            .track(source)
            .unwrap()
            .devices()
            .iter()
            .map(crate::Device::id)
            .collect::<Vec<_>>(),
        [ids[4], ids[5]]
    );
    for device in original.devices() {
        assert_eq!(project.device(device.id()), Some(device));
    }
    let after = project.snapshot();
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn reorder_selection_normalizes_a_moving_child_anchor_and_keeps_serial_units() {
    let (mut editor, track, _, instrument, ids) = fixture(4);
    let reorder = |selected: &[DeviceId], before| Command::ReorderDevices {
        owner: crate::DeviceChainOwner::Track(track),
        selected: DeviceSelectionIds::new(selected).unwrap(),
        before: Some(before),
    };
    let original = editor.project().clone();
    assert!(
        editor
            .execute(reorder(&[instrument, ids[6]], ids[2]))
            .is_err()
    );
    assert_eq!(editor.project(), &original);
    let selected = [ids[6], ids[2], instrument, ids[0]];
    editor.execute(reorder(&selected, ids[2])).unwrap();
    let project = editor.project();
    let chain = project.track(track).unwrap();
    assert_eq!(chain.instrument_position(), Some(4));
    assert_eq!(
        chain
            .devices()
            .iter()
            .map(crate::Device::id)
            .collect::<Vec<_>>(),
        [ids[0], ids[1], ids[2], ids[3], ids[6], ids[4], ids[5]]
    );
    for device in original.devices() {
        assert_eq!(project.device(device.id()), Some(device));
    }
    let reordered = project.clone();
    editor.execute(reorder(&selected, instrument)).unwrap();
    assert_eq!(editor.project(), &reordered);
    assert!(editor.undo());
    assert_eq!(editor.project(), &original);
    assert!(editor.redo());
    assert_eq!(editor.project(), &reordered);
}

#[test]
fn selected_track_move_uses_the_exact_track_binding_for_shared_instruments() {
    let (mut editor, receiver, source, instrument, _) = fixture(4);
    let retained = editor.project().track(receiver).unwrap().source_id();
    editor
        .execute(Command::BindTrackSource {
            track: source,
            source: retained,
        })
        .unwrap();
    let Created::Track(destination) = editor
        .execute(Command::AddMidiTrack {
            name: "destination".into(),
        })
        .unwrap()
    else {
        panic!("destination")
    };
    editor
        .execute(Command::MoveTrackDevices {
            source,
            selected: DeviceSelectionIds::new(&[instrument]).unwrap(),
            destination,
        })
        .unwrap();
    let project = editor.project();
    let receiver = project.track(receiver).unwrap();
    assert!(receiver.receives_source_audio());
    assert_eq!(receiver.instrument().unwrap().id(), instrument);
    assert!(project.track(source).unwrap().instrument().is_none());
    assert_eq!(
        project
            .track(destination)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        instrument
    );
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
        assert!(editor.execute(command(mixed, ids[3], other, None)).is_err());
        assert!(editor.can_redo());
    }
}

fn transfer_fixture(
    source_gap: usize,
    target_gap: usize,
) -> (
    Editor,
    TrackId,
    TrackId,
    DeviceId,
    Vec<DeviceId>,
    Vec<DeviceId>,
) {
    let (mut editor, source, target, _, ids) = fixture(source_gap);
    let Created::Device(instrument) = editor
        .execute(Command::InsertInstrument {
            track: target,
            state: crate::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("target instrument")
    };
    let mut targets = Vec::new();
    for _ in 0..3 {
        let Created::Device(id) = editor
            .execute(Command::InsertDevice {
                track: target,
                before: None,
                state: crate::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("target effect")
        };
        targets.push(id);
    }
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[1].instrument_position = Some(target_gap);
    snapshot.tracks[1].devices[1].parent = Some(targets[0].value());
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap(),
        source,
        target,
        instrument,
        ids,
        targets,
    )
}

#[test]
fn root_units_transfer_between_tracks_preserving_every_node_and_both_instrument_gaps() {
    for mixed in [false, true] {
        for (source_gap, target_gap, anchor, expected_gap) in [
            (4, 2, Some(0), 6),
            (0, 2, None, 2),
            (7, 0, Some(2), 0),
            (4, 2, Some(2), 2),
        ] {
            let (mut editor, source, target, _, ids, targets) =
                transfer_fixture(source_gap, target_gap);
            let before = editor.project().snapshot();
            let authored = editor.project().devices().cloned().collect::<Vec<_>>();
            editor
                .execute(command(mixed, ids[0], target, anchor.map(|i| targets[i])))
                .unwrap();
            assert_eq!(
                editor
                    .project()
                    .track(source)
                    .unwrap()
                    .devices()
                    .iter()
                    .map(crate::Device::id)
                    .collect::<Vec<_>>(),
                ids[4..]
            );
            assert_eq!(
                editor
                    .project()
                    .track(source)
                    .unwrap()
                    .instrument_position(),
                Some(source_gap.saturating_sub(4))
            );
            assert_eq!(
                editor
                    .project()
                    .track(target)
                    .unwrap()
                    .instrument_position(),
                Some(expected_gap)
            );
            let position = anchor.unwrap_or(3);
            let expected = targets[..position]
                .iter()
                .chain(ids[..4].iter())
                .chain(targets[position..].iter())
                .copied()
                .collect::<Vec<_>>();
            assert_eq!(
                editor
                    .project()
                    .track(target)
                    .unwrap()
                    .devices()
                    .iter()
                    .map(crate::Device::id)
                    .collect::<Vec<_>>(),
                expected
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
    let (mut editor, _, target, instrument, ids, _) = transfer_fixture(4, 2);
    editor
        .execute(command(true, ids[0], target, Some(instrument)))
        .unwrap();
    assert_eq!(
        editor
            .project()
            .track(target)
            .unwrap()
            .instrument_position(),
        Some(6)
    );
}

#[test]
fn cross_track_capacity_placement_and_preparation_refusals_preserve_history() {
    for mixed in [false, true] {
        let (mut editor, _, target, _, ids, targets) = transfer_fixture(4, 2);
        let before = editor.project().snapshot();
        for (moving, anchor) in [
            (ids[0], Some(targets[1])),
            (ids[0], Some(ids[4])),
            (ids[1], None),
        ] {
            assert!(
                editor
                    .execute(command(mixed, moving, target, anchor))
                    .is_err()
            );
            assert_eq!(editor.project().snapshot(), before);
            assert!(!editor.can_undo());
        }
        let reached = std::cell::Cell::new(false);
        let refused: Result<_, crate::Invalid> =
            editor.execute_prepared(command(mixed, ids[0], target, None), |candidate| {
                reached.set(true);
                assert_eq!(candidate.track(target).unwrap().devices().len(), 7);
                Err::<(), _>(crate::Invalid("preparation refused"))
            });
        assert!(refused.is_err());
        assert!(reached.get());
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.can_undo());
        editor.execute(Command::RemoveDevice(ids[6])).unwrap();
        assert!(editor.undo());
        assert!(editor.can_redo());
        assert!(
            editor
                .execute(command(mixed, ids[0], target, Some(ids[6])))
                .is_err()
        );
        assert!(editor.can_redo());
        assert_eq!(editor.project().snapshot(), before);

        let mut full = before;
        while full.tracks[1].devices.len() < crate::MAX_DEVICES_PER_TRACK - 1 {
            let mut leaf = full.tracks[1].devices[2].clone();
            leaf.id = full.next_id;
            full.next_id += 1;
            full.tracks[1].devices.push(leaf);
        }
        let mut editor = Editor::new(Project::from_snapshot(full.clone()).unwrap(), 16).unwrap();
        assert!(
            editor
                .execute(command(mixed, ids[0], target, None))
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), full);
        assert!(!editor.can_undo());
        full.tracks[1]
            .devices
            .truncate(crate::MAX_DEVICES_PER_TRACK - 4);
        let mut editor = Editor::new(Project::from_snapshot(full).unwrap(), 16).unwrap();
        editor
            .execute(command(mixed, ids[0], target, None))
            .unwrap();
        assert_eq!(
            editor.project().track(target).unwrap().devices().len(),
            crate::MAX_DEVICES_PER_TRACK
        );
    }
}
