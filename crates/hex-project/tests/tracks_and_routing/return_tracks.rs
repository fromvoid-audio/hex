use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{Command, Created, Editor, Project};

fn device(payload: &[u8]) -> NativeDeviceState {
    NativeDeviceState::new(
        DeviceTypeId::new("hex.test.effect").unwrap(),
        3,
        payload.to_vec(),
    )
    .unwrap()
}

fn created_return(editor: &mut Editor, name: &str) -> hex_project::ReturnId {
    match editor
        .execute(Command::AddReturnTrack { name: name.into() })
        .unwrap()
    {
        Created::ReturnTrack(id) => id,
        other => panic!("unexpected result: {other:?}"),
    }
}

fn created_track(editor: &mut Editor) -> hex_project::TrackId {
    match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn return_sends_and_effect_chains_roundtrip_through_one_step_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = created_track(&mut editor);
    let return_id = created_return(&mut editor, "reverb");
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: return_id,
            gain: 0.375,
        })
        .unwrap();
    let first = match editor
        .execute(Command::InsertReturnDevice {
            return_track: return_id,
            before: None,
            state: device(&[1, 2]),
        })
        .unwrap()
    {
        Created::Device(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let second = match editor
        .execute(Command::InsertReturnDevice {
            return_track: return_id,
            before: Some(first),
            state: device(&[3]),
        })
        .unwrap()
    {
        Created::Device(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    editor
        .execute(Command::MoveReturnDevice {
            device: first,
            before: None,
        })
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: first,
            state: device(&[9, 8, 7]),
        })
        .unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device: first,
            bypassed: true,
        })
        .unwrap();
    let committed = editor.project().snapshot();
    assert_eq!(committed.return_tracks[0].devices[0].id, second.value());
    assert_eq!(committed.return_tracks[0].devices[1].id, first.value());
    assert!(committed.return_tracks[0].devices[1].bypassed);
    assert_eq!(
        committed.return_tracks[0].devices[1].state,
        device(&[9, 8, 7])
    );
    assert_eq!(committed.tracks[0].sends[0].gain, 0.375);
    assert_eq!(
        Project::from_snapshot(committed.clone())
            .unwrap()
            .snapshot(),
        committed
    );

    assert!(editor.undo());
    assert!(!editor.project().device(first).unwrap().bypassed());
    assert!(editor.redo());
    assert!(editor.project().device(first).unwrap().bypassed());
    assert!(editor.undo());
    assert!(editor.undo());
    assert_eq!(
        editor.project().device(first).unwrap().state(),
        &device(&[1, 2])
    );
    assert!(editor.undo());
    assert_eq!(editor.project().return_tracks()[0].devices()[0].id(), first);
    assert!(editor.undo());
    assert_eq!(editor.project().tracks()[0].sends()[0].gain(), 0.375);
    assert!(editor.redo());
    assert!(editor.redo());
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), committed);
    editor.execute(Command::RemoveDevice(first)).unwrap();
    assert!(editor.project().device(first).is_none());
    assert!(editor.undo());
    assert!(editor.project().device(first).is_some());
}

#[test]
fn zero_send_is_omitted_and_return_removal_cascades_atomically() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let track = created_track(&mut editor);
    let return_id = created_return(&mut editor, "delay");
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: return_id,
            gain: 1.0,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: return_id,
            gain: 0.0,
        })
        .unwrap();
    assert!(editor.project().tracks()[0].sends().is_empty());
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: return_id,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::RemoveReturnTrack(return_id))
        .unwrap();
    assert!(editor.project().return_tracks().is_empty());
    assert!(editor.project().tracks()[0].sends().is_empty());
    assert!(editor.undo());
    assert_eq!(
        editor.project().tracks()[0].sends()[0].return_track(),
        return_id
    );
    assert!(
        editor
            .project()
            .return_tracks()
            .iter()
            .any(|item| item.id() == return_id)
    );
    editor.execute(Command::RemoveTrack(track)).unwrap();
    assert!(editor.project().tracks().is_empty());
    assert!(editor.undo());
    assert_eq!(
        editor.project().tracks()[0].sends()[0].return_track(),
        return_id
    );
}

#[test]
fn return_mix_controls_validate_and_share_single_step_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let return_id = created_return(&mut editor, "bus");
    editor
        .execute(Command::RenameReturnTrack {
            return_track: return_id,
            name: "master verb".into(),
        })
        .unwrap();
    editor
        .execute(Command::ReturnGain {
            return_track: return_id,
            gain: 0.75,
        })
        .unwrap();
    editor
        .execute(Command::ReturnMute {
            return_track: return_id,
            muted: true,
        })
        .unwrap();
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::ReturnGain {
                return_track: return_id,
                gain: f32::INFINITY,
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(editor.project().return_tracks()[0].name(), "master verb");
    assert_eq!(editor.project().return_tracks()[0].gain(), 0.75);
    assert!(editor.project().return_tracks()[0].muted());
    assert!(editor.undo());
    assert!(!editor.project().return_tracks()[0].muted());
    assert!(editor.redo());
    assert!(editor.project().return_tracks()[0].muted());
}

#[test]
fn malformed_return_and_send_snapshots_are_rejected() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let track = created_track(&mut editor);
    let return_id = created_return(&mut editor, "bus");
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: return_id,
            gain: 0.25,
        })
        .unwrap();
    let valid = editor.project().snapshot();

    let mut duplicate = valid.clone();
    let send = duplicate.tracks[0].sends[0];
    duplicate.tracks[0].sends.push(send);
    assert!(Project::from_snapshot(duplicate).is_err());

    let mut missing = valid.clone();
    missing.tracks[0].sends[0].return_track = u64::MAX;
    assert!(Project::from_snapshot(missing).is_err());

    for gain in [f32::NAN, f32::INFINITY, -0.1, 2.01, 0.0] {
        let mut bad = valid.clone();
        bad.tracks[0].sends[0].gain = gain;
        assert!(Project::from_snapshot(bad).is_err());
    }

    let mut duplicate_id = valid.clone();
    duplicate_id.return_tracks[0].id = duplicate_id.tracks[0].id;
    assert!(Project::from_snapshot(duplicate_id).is_err());

    let mut bad_highwater = valid;
    bad_highwater.next_id = bad_highwater.return_tracks[0].id;
    assert!(Project::from_snapshot(bad_highwater).is_err());
}

#[test]
fn return_and_send_caps_and_command_validation_are_bounded() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let track = created_track(&mut editor);
    let mut returns = Vec::new();
    for index in 0..hex_project::MAX_RETURN_TRACKS {
        returns.push(created_return(&mut editor, &format!("return {index}")));
    }
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::AddReturnTrack {
                name: "overflow".into(),
            })
            .is_err()
    );
    for return_id in returns {
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: return_id,
                gain: 0.25,
            })
            .unwrap();
    }
    assert_eq!(
        editor.project().tracks()[0].sends().len(),
        hex_project::MAX_RETURN_SENDS_PER_TRACK
    );
    let return_id = editor.project().return_tracks()[0].id();
    for index in 0..hex_project::MAX_DEVICES_PER_TRACK {
        editor
            .execute(Command::InsertReturnDevice {
                return_track: return_id,
                before: None,
                state: device(&[index as u8]),
            })
            .unwrap();
    }
    assert!(
        editor
            .execute(Command::InsertReturnDevice {
                return_track: return_id,
                before: None,
                state: device(&[99]),
            })
            .is_err()
    );
    let before_full = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: editor.project().return_tracks()[0].id(),
                gain: f32::NAN,
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before_full);
    assert_eq!(before.return_tracks.len(), hex_project::MAX_RETURN_TRACKS);
}

#[test]
fn undo_branch_keeps_return_and_device_ids_below_the_allocator_highwater() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let first = created_return(&mut editor, "first");
    assert!(editor.undo());
    let second = created_return(&mut editor, "second");
    assert!(second.value() > first.value());
    let device_id = match editor
        .execute(Command::InsertReturnDevice {
            return_track: second,
            before: None,
            state: device(&[]),
        })
        .unwrap()
    {
        Created::Device(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    assert!(device_id.value() > second.value());
    assert!(editor.project().snapshot().next_id > device_id.value());
}

#[test]
fn return_initial_gain_is_validated_prepared_and_undoes_atomically() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    created_return(&mut editor, "original");
    let before = editor.project().clone();
    for gain in [f32::NAN, f32::INFINITY, -0.1, 2.1] {
        assert!(
            editor
                .execute(Command::AddReturnTrackWithGain {
                    name: "new".into(),
                    gain
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
    }
    assert!(
        editor
            .execute_prepared(
                Command::AddReturnTrackWithGain {
                    name: "new".into(),
                    gain: 0.25
                },
                |_| Err::<(), _>(hex_project::Invalid("refused")),
            )
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    let Created::ReturnTrack(id) = editor
        .execute(Command::AddReturnTrackWithGain {
            name: "new".into(),
            gain: 0.25,
        })
        .unwrap()
    else {
        panic!("return")
    };
    assert_eq!(editor.project().return_tracks()[1].id(), id);
    assert_eq!(editor.project().return_tracks()[1].gain(), 0.25);
    assert_eq!(editor.project().return_tracks()[0].gain(), 1.0);
    let after = editor.project().snapshot();
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap(),
        *editor.project()
    );
    assert!(editor.undo());
    let mut restored = editor.project().snapshot();
    restored.next_id = before.snapshot().next_id;
    assert_eq!(restored, before.snapshot());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}
