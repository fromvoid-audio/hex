use hex_project::{Command, Created, Editor, Project, TrackDeviceSlot, TrackId};
fn track(result: Created) -> TrackId {
    match result {
        Created::Track(id) | Created::Imported { track: id, .. } => id,
        other => panic!("new track expected: {other:?}"),
    }
}
fn empty(project: &Project, id: TrackId) {
    let t = project.track(id).unwrap();
    assert!(
        t.instrument().is_none(),
        "ordinary creation must not seed an instrument"
    );
    assert_eq!(t.instrument_position(), None);
    assert!(t.devices().is_empty());
    assert_eq!(t.ordered_device_chain().count(), 0);
}
#[test]
fn ordinary_audio_midi_menu_and_import_routes_create_empty_stable_tracks() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    for command in [
        Command::AddTrack {
            name: "ordinary".into(),
        },
        Command::AddMidiTrack {
            name: "midi".into(),
        },
        Command::InsertTrack {
            name: "menu".into(),
            before_track: None,
        },
        Command::ImportAudioTrack {
            name: "raw import".into(),
            path: "not-read.wav".into(),
            sample_rate: 48_000,
            frames: 480,
        },
        Command::ImportPreparedAudioTrack {
            name: "prepared".into(),
            path: "not-read2.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 480,
            playback_frames: 480,
            conversion_version: 0,
        },
        Command::ImportPreparedAudioTrackAt {
            name: "positioned".into(),
            before_track: None,
            start: 480,
            path: "not-read3.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 480,
            playback_frames: 480,
            conversion_version: 0,
        },
    ] {
        let id = track(editor.execute(command).unwrap());
        empty(editor.project(), id);
        let after = editor.project().snapshot();
        assert!(editor.undo());
        assert!(editor.project().track(id).is_none());
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        empty(editor.project(), id);
        assert_eq!(Project::from_snapshot(after).unwrap(), *editor.project());
    }
}
#[test]
fn explicit_device_drop_creates_only_chosen_device_and_keeps_existing_tracks() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let state = hex_project::default_instrument_state().unwrap();
    let instrument = track(
        editor
            .execute(Command::InsertTrackWithDevice {
                name: "chosen instrument".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: state.clone(),
            })
            .unwrap(),
    );
    let authored = editor.project().track(instrument).unwrap().clone();
    assert_eq!(authored.ordered_device_chain().count(), 1);
    assert_eq!(authored.instrument().unwrap().state(), &state);
    let effect = hex_lattice_contract::NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        1,
        Vec::new(),
    )
    .unwrap();
    let fx = track(
        editor
            .execute(Command::InsertTrackWithDevice {
                name: "chosen effect".into(),
                before_track: None,
                slot: TrackDeviceSlot::Effect,
                state: effect.clone(),
            })
            .unwrap(),
    );
    let t = editor.project().track(fx).unwrap();
    assert!(
        t.instrument().is_none(),
        "effect drop must not add an unrelated Sine"
    );
    assert_eq!(t.ordered_device_chain().count(), 1);
    assert_eq!(t.devices()[0].state(), &effect);
    let new = track(
        editor
            .execute(Command::AddTrack {
                name: "empty neighbor".into(),
            })
            .unwrap(),
    );
    empty(editor.project(), new);
    assert_eq!(editor.project().track(instrument).unwrap(), &authored);
    let after = editor.project().snapshot();
    assert!(editor.undo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}
#[test]
fn ordinary_creation_allocates_only_track_identity_and_later_instrument_is_explicit() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let next = e.project().snapshot().next_id;
    let id = track(
        e.execute(Command::AddMidiTrack {
            name: "empty keys".into(),
        })
        .unwrap(),
    );
    empty(e.project(), id);
    assert_eq!(e.project().snapshot().next_id, next + 1);
    let Created::Device(device) = e
        .execute(Command::InsertInstrument {
            track: id,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("explicit instrument")
    };
    assert_eq!(
        e.project().track(id).unwrap().instrument().unwrap().id(),
        device
    );
    assert!(e.undo());
    empty(e.project(), id);
    assert!(e.redo());
    assert_eq!(
        e.project().track(id).unwrap().instrument().unwrap().id(),
        device
    );
}

#[test]
fn midi_clip_transfer_to_new_track_keeps_destination_empty_and_source_devices_owned() {
    use hex_project::BeatTime;
    for copy in [false, true] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let source = track(
            editor
                .execute(Command::InsertTrackWithDevice {
                    name: "authored source".into(),
                    before_track: None,
                    slot: TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap(),
        );
        editor
            .execute(Command::InsertDevice {
                track: source,
                before: None,
                state: hex_lattice_contract::NativeDeviceState::new(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                    1,
                    Vec::new(),
                )
                .unwrap(),
            })
            .unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: source,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let Created::Note(note) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 64,
                channel: 2,
                velocity: 91,
            })
            .unwrap()
        else {
            panic!("note")
        };
        let before = editor.project().clone();
        let start = BeatTime::from_quarters(2).unwrap();
        let command = |gain| {
            if copy {
                Command::CopyMidiClipToNewTrack {
                    initial_gain: Some(gain),
                    clip,
                    name: "copied clip".into(),
                    before_track: None,
                    start,
                }
            } else {
                Command::MoveMidiClipToNewTrack {
                    initial_gain: Some(gain),
                    clip,
                    name: "moved clip".into(),
                    before_track: None,
                    start,
                }
            }
        };
        for invalid in [f32::NAN, f32::INFINITY, -0.1, 2.1] {
            assert!(editor.execute(command(invalid)).is_err());
            assert_eq!(editor.project(), &before);
        }
        let refused = editor.execute_prepared(command(0.25), |_| {
            Err::<(), _>(hex_project::Invalid("refused"))
        });
        assert!(refused.is_err());
        assert_eq!(editor.project(), &before);
        let destination = track(editor.execute(command(0.25)).unwrap());
        empty(editor.project(), destination);
        assert_eq!(editor.project().track(destination).unwrap().gain(), 0.25);
        let retained = editor.project().track(source).unwrap();
        let original = before.track(source).unwrap();
        assert_eq!(retained.instrument(), original.instrument());
        assert_eq!(retained.devices(), original.devices());
        assert_eq!(retained.midi_clips().len(), usize::from(copy));
        let transferred = &editor.project().track(destination).unwrap().midi_clips()[0];
        assert_eq!(transferred.start(), start);
        assert_eq!(transferred.length(), BeatTime::ONE);
        assert_eq!(transferred.notes().len(), 1);
        let copied_note = &transferred.notes()[0];
        assert_eq!(
            (
                copied_note.start(),
                copied_note.duration(),
                copied_note.pitch(),
                copied_note.channel(),
                copied_note.velocity()
            ),
            (BeatTime::ZERO, BeatTime::ONE, 64, 2, 91)
        );
        if copy {
            assert_ne!(transferred.id(), clip);
            assert_ne!(copied_note.id(), note);
            assert_eq!(retained.midi_clips(), original.midi_clips());
        } else {
            assert_eq!(transferred.id(), clip);
            assert_eq!(copied_note.id(), note);
        }
        let after = editor.project().snapshot();
        assert!(editor.undo());
        let mut expected = before.snapshot();
        expected.next_id = after.next_id;
        assert_eq!(editor.project().snapshot(), expected);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}
