use super::*;
use hex_project::{Created, Editor, Tempo};

fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn fixture() -> (Project, TrackId, ClipId) {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "record".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(2),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: BeatTime::ZERO,
            loop_end: beat(4),
            looping: false,
            length: beat(2),
        })
        .unwrap();
    (editor.project().clone(), track, clip)
}
fn note(start_frame: u64, end_frame: u64, pitch: u8) -> RecordedMidiNote {
    RecordedMidiNote {
        start_frame,
        end_frame,
        pitch,
        channel: 3,
        velocity: 93,
    }
}

#[test]
fn frame_take_maps_trimmed_source_closes_held_notes_and_is_one_persisted_history_edit() {
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    assert_eq!(
        (binding.first_frame(), binding.end_frame()),
        (8_000, 16_000)
    );
    let before = project.snapshot();
    let captured = [note(9_000, 10_000, 60), note(15_000, 18_000, 64)];
    let command = finish_take(&project, &binding, &captured, 18_000)
        .unwrap()
        .unwrap();
    assert_eq!(project.snapshot(), before);
    let mut editor = Editor::new(project, 8).unwrap();
    assert!(!editor.can_undo());
    editor.execute(command).unwrap();
    let notes = editor.project().midi_clip(clip).unwrap().notes();
    assert_eq!(notes.len(), 2);
    for (n, numerator) in notes.iter().zip([5, 11]) {
        assert_eq!(n.start(), BeatTime::from_ratio(numerator, 4).unwrap());
        assert_eq!(n.duration(), BeatTime::from_ratio(1, 4).unwrap());
        assert_eq!((n.channel(), n.velocity()), (3, 93));
    }
    let after = editor.project().snapshot();
    let encoded = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&encoded).unwrap().snapshot(), after);
    assert!(editor.undo());
    let mut restored = before;
    // Undo preserves the canonical global ID high-water mark.
    restored.next_id = after.next_id;
    assert_eq!(editor.project().snapshot(), restored);
    assert!(!editor.can_undo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn source_substitution_tempo_and_geometry_changes_refuse_without_consuming_the_take() {
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    let captured = [note(9_000, 10_000, 60)];
    let replacement = Project::from_snapshot(project.snapshot()).unwrap();
    assert!(finish_take(&replacement, &binding, &captured, 12_000).is_err());
    for command in [
        Command::SetProjectTempo {
            tempo: Tempo::new(130_000_000).unwrap(),
        },
        Command::MoveMidiClip {
            clip,
            track,
            start: beat(3),
        },
    ] {
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        editor.execute(command).unwrap();
        let before = editor.project().snapshot();
        assert!(finish_take(editor.project(), &binding, &captured, 12_000).is_err());
        assert_eq!(editor.project().snapshot(), before);
    }
    assert!(
        finish_take(&project, &binding, &captured, 12_000)
            .unwrap()
            .is_some()
    );
    assert_eq!(captured, [note(9_000, 10_000, 60)]);
}

#[test]
fn malformed_overfull_and_unsorted_takes_refuse_while_empty_take_has_no_edit() {
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    assert!(
        finish_take(&project, &binding, &[], 12_000)
            .unwrap()
            .is_none()
    );
    let valid = note(9_000, 10_000, 60);
    for invalid in [
        RecordedMidiNote {
            start_frame: 7_999,
            ..valid
        },
        RecordedMidiNote {
            start_frame: 16_000,
            end_frame: 17_000,
            ..valid
        },
        RecordedMidiNote {
            end_frame: 9_000,
            ..valid
        },
        RecordedMidiNote {
            end_frame: 12_001,
            ..valid
        },
        RecordedMidiNote {
            pitch: 128,
            ..valid
        },
        RecordedMidiNote {
            channel: 16,
            ..valid
        },
        RecordedMidiNote {
            velocity: 0,
            ..valid
        },
        RecordedMidiNote {
            velocity: 128,
            ..valid
        },
    ] {
        assert!(finish_take(&project, &binding, &[invalid], 12_000).is_err());
    }
    assert!(
        finish_take(
            &project,
            &binding,
            &[valid; MAX_MIDI_TAKE_NOTES + 1],
            12_000
        )
        .is_err()
    );
    assert!(
        finish_take(
            &project,
            &binding,
            &[note(10_000, 11_000, 64), valid],
            12_000
        )
        .is_err()
    );
}

#[test]
fn unrelated_name_or_mixer_edits_do_not_substitute_an_unchanged_note_source() {
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::RenameTrack {
            track,
            name: "new name".into(),
        })
        .unwrap();
    editor.execute(Command::Gain { track, gain: 0.5 }).unwrap();
    assert!(
        finish_take(
            editor.project(),
            &binding,
            &[note(9_000, 10_000, 60)],
            12_000
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn subframe_clip_edge_is_projected_inside_the_authored_source() {
    let (project, track, clip) = fixture();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::MoveMidiClip {
            clip,
            track,
            start: BeatTime::from_raw(beat(2).raw() + 1),
        })
        .unwrap();
    let binding = MidiTakeBinding::capture(editor.project(), track, clip).unwrap();
    let command = finish_take(
        editor.project(),
        &binding,
        &[note(binding.first_frame(), binding.first_frame() + 1, 60)],
        binding.first_frame() + 1,
    )
    .unwrap()
    .unwrap();
    editor.execute(command).unwrap();
    let n = &editor.project().midi_clip(clip).unwrap().notes()[0];
    assert_eq!(n.start(), beat(1));
    assert!(n.duration() > BeatTime::ZERO);
}

#[test]
fn unsupported_capture_targets_refuse_before_any_note_or_history_edit() {
    let (project, track, clip) = fixture();
    for command in [
        Command::RemoveInstrument { track },
        Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: BeatTime::ZERO,
            loop_end: beat(4),
            looping: true,
            length: beat(2),
        },
        Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: BeatTime::ZERO,
            loop_end: beat(4),
            looping: false,
            length: beat(4),
        },
    ] {
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        editor.execute(command).unwrap();
        let before = editor.project().snapshot();
        assert!(MidiTakeBinding::capture(editor.project(), track, clip).is_err());
        assert_eq!(editor.project().snapshot(), before);
    }
}
