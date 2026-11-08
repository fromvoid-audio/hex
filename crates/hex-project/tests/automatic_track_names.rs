use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{Command, Created, Editor, Project, TrackDeviceSlot, TrackId};

fn editor() -> Editor {
    Editor::new(Project::new(48_000).unwrap(), 16).unwrap()
}
fn track(result: Created) -> TrackId {
    match result {
        Created::Track(id) => id,
        other => panic!("new track expected: {other:?}"),
    }
}
fn instrument() -> NativeDeviceState {
    hex_project::default_instrument_state().unwrap()
}
fn effect() -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::from_static("hex.utility"), 1, Vec::new()).unwrap()
}
fn load(editor: &mut Editor, track: TrackId, name: &str) -> Result<Created, hex_project::Invalid> {
    let replacing = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .map(hex_project::Device::id);
    editor.execute(Command::LoadInstrument {
        track,
        replacing,
        state: instrument(),
        device_name: name.into(),
    })
}
fn name(editor: &Editor, id: TrackId) -> (&str, bool) {
    let track = editor.project().track(id).unwrap();
    (track.name(), track.name_is_automatic())
}

#[test]
fn default_track_follows_each_loaded_instrument_until_renamed() {
    let mut editor = editor();
    let id = track(
        editor
            .execute(Command::InsertDefaultTrack { before_track: None })
            .unwrap(),
    );
    assert_eq!(name(&editor, id), ("track 1", true));
    load(&mut editor, id, "sampler").unwrap();
    assert_eq!(name(&editor, id), ("sampler", true));
    // Replacing a device-derived name keeps following the instrument.
    load(&mut editor, id, "sine").unwrap();
    assert_eq!(name(&editor, id), ("sine", true));
    // Renaming to the same text still makes the name the user's.
    editor
        .execute(Command::RenameTrack {
            track: id,
            name: "sine".into(),
        })
        .unwrap();
    load(&mut editor, id, "sampler").unwrap();
    assert_eq!(name(&editor, id), ("sine", false));
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
}

#[test]
fn user_named_tracks_keep_their_names_even_when_they_look_default() {
    let mut editor = editor();
    for user_name in ["1 audio", "track 1", "lead"] {
        let id = track(
            editor
                .execute(Command::AddTrack {
                    name: user_name.into(),
                })
                .unwrap(),
        );
        load(&mut editor, id, "sampler").unwrap();
        assert_eq!(name(&editor, id), (user_name, false));
        assert!(editor.project().track(id).unwrap().instrument().is_some());
    }
}

#[test]
fn one_undo_restores_device_and_name_together() {
    let mut editor = editor();
    let id = track(
        editor
            .execute(Command::InsertDefaultTrack { before_track: None })
            .unwrap(),
    );
    let before = editor.project().snapshot();
    load(&mut editor, id, "sampler").unwrap();
    let after = editor.project().snapshot();
    assert!(editor.undo());
    // Undo retains the ID high-water mark; everything else is restored.
    let mut expected = before;
    expected.next_id = after.next_id;
    assert_eq!(editor.project().snapshot(), expected);
    assert!(editor.project().track(id).unwrap().instrument().is_none());
    assert_eq!(name(&editor, id), ("track 1", true));
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn effects_and_refused_loads_never_rename() {
    let mut editor = editor();
    let id = track(
        editor
            .execute(Command::InsertDefaultTrack { before_track: None })
            .unwrap(),
    );
    let Created::Device(utility) = editor
        .execute(Command::InsertDevice {
            track: id,
            before: None,
            state: effect(),
        })
        .unwrap()
    else {
        panic!("effect device expected")
    };
    assert_eq!(name(&editor, id), ("track 1", true));
    let before = editor.project().snapshot();
    // A stale replacement target and an invalid name both refuse atomically.
    let stale = editor.execute(Command::LoadInstrument {
        track: id,
        replacing: Some(utility),
        state: instrument(),
        device_name: "sampler".into(),
    });
    assert!(stale.is_err());
    assert!(load(&mut editor, id, " ").is_err());
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn new_device_tracks_name_instruments_and_default_name_effects() {
    let mut editor = editor();
    let instrument_track = track(
        editor
            .execute(Command::InsertDefaultTrackWithDevice {
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: instrument(),
                device_name: "sine".into(),
            })
            .unwrap(),
    );
    assert_eq!(name(&editor, instrument_track), ("sine", true));
    let effect_track = track(
        editor
            .execute(Command::InsertDefaultTrackWithDevice {
                before_track: None,
                slot: TrackDeviceSlot::Effect,
                state: effect(),
                device_name: "utility".into(),
            })
            .unwrap(),
    );
    assert_eq!(name(&editor, effect_track), ("track 2", true));
    load(&mut editor, effect_track, "sampler").unwrap();
    assert_eq!(name(&editor, effect_track), ("sampler", true));
}

#[test]
fn snapshot_refuses_malformed_automatic_name_inventories() {
    let mut editor = editor();
    for _ in 0..2 {
        editor
            .execute(Command::InsertDefaultTrack { before_track: None })
            .unwrap();
    }
    let valid = editor.project().snapshot();
    assert_eq!(valid.automatic_track_names.len(), 2);
    let mut reversed = valid.clone();
    reversed.automatic_track_names.reverse();
    let mut duplicate = valid.clone();
    duplicate.automatic_track_names[1] = duplicate.automatic_track_names[0];
    let mut ownerless = valid;
    ownerless.automatic_track_names.push(u64::MAX);
    for snapshot in [reversed, duplicate, ownerless] {
        assert!(Project::from_snapshot(snapshot).is_err());
    }
}

#[test]
fn device_track_initial_gain_is_atomic_and_preserves_automatic_names() {
    for (slot, state, label, expected_name) in [
        (TrackDeviceSlot::Instrument, instrument(), "sine", "sine"),
        (TrackDeviceSlot::Effect, effect(), "utility", "track 1"),
    ] {
        let mut editor = editor();
        let before = editor.project().snapshot();
        let created = editor
            .execute(Command::InsertDefaultTrackWithDeviceAndGain {
                before_track: None,
                slot,
                state,
                device_name: label.into(),
                gain: 0.25,
            })
            .unwrap();
        let id = track(created);
        assert_eq!(name(&editor, id), (expected_name, true));
        assert_eq!(editor.project().track(id).unwrap().gain(), 0.25);
        let after = editor.project().clone();
        assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
        assert!(editor.undo());
        let mut restored = editor.project().snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        assert!(editor.redo());
        assert_eq!(editor.project(), &after);
    }
}

#[test]
fn device_track_initial_gain_refusal_keeps_project_and_redo() {
    let mut editor = editor();
    let stale = track(
        editor
            .execute(Command::InsertDefaultTrack { before_track: None })
            .unwrap(),
    );
    assert!(editor.undo());
    let before = editor.project().snapshot();
    for gain in [f32::NAN, f32::INFINITY, -0.1, 2.1] {
        assert!(
            editor
                .execute(Command::InsertDefaultTrackWithDeviceAndGain {
                    before_track: None,
                    slot: TrackDeviceSlot::Instrument,
                    state: instrument(),
                    device_name: "sine".into(),
                    gain,
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
    assert!(
        editor
            .execute(Command::InsertDefaultTrackWithDeviceAndGain {
                before_track: Some(stale),
                slot: TrackDeviceSlot::Effect,
                state: effect(),
                device_name: "utility".into(),
                gain: 0.5,
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.can_redo());
    let result = editor.execute_prepared(
        Command::InsertDefaultTrackWithDeviceAndGain {
            before_track: None,
            slot: TrackDeviceSlot::Effect,
            state: effect(),
            device_name: "utility".into(),
            gain: 0.5,
        },
        |_| Err::<(), _>(hex_project::Invalid("refused")),
    );
    assert!(result.is_err());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.can_redo());
}
