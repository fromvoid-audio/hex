//! Stable-ID movement may grow source storage while the Arrangement window stays fixed.
use hex_project::{BeatTime, Command, Created, Editor, MidiNoteUpdate, Project};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}

fn fixture() -> (Editor, hex_project::ClipId, hex_project::NoteId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
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
    let Created::Note(note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(0),
            duration: beat(1),
            pitch: 60,
            channel: 2,
            velocity: 93,
        })
        .unwrap()
    else {
        panic!("note")
    };
    (editor, clip, note)
}
fn update(note: hex_project::NoteId, start: BeatTime, duration: BeatTime) -> MidiNoteUpdate {
    MidiNoteUpdate {
        note,
        start,
        duration,
        pitch: 60,
        channel: 2,
        velocity: 93,
    }
}

#[test]
fn stable_note_extent_growth_preserves_window_markers_ids_history_and_snapshot() {
    let (mut editor, clip, note) = fixture();
    let before = editor.project().snapshot();
    let next_id = before.next_id;
    let notes = vec![update(note, beat(5), beat(1))];
    assert!(
        editor
            .execute(Command::UpdateMidiNotes {
                clip,
                notes: notes.clone()
            })
            .is_err()
    );
    let mut command = Command::UpdateMidiNotesWithSourceExtent { clip, notes };
    assert!(command.supports_sampler_midi_edit());
    assert!(command.prepare_sampler_midi_edit(editor.project()).unwrap());
    editor.execute(command).unwrap();
    let changed = editor.project().snapshot();
    let placed = editor.project().midi_clip(clip).unwrap();
    assert_eq!((placed.start(), placed.length()), (beat(2), beat(4)));
    assert_eq!(
        (
            placed.source_start(),
            placed.loop_start(),
            placed.loop_end()
        ),
        (beat(0), beat(0), beat(4))
    );
    assert_eq!(placed.source_length(), beat(6));
    let moved = editor.project().midi_note(note).unwrap();
    assert_eq!(
        (
            moved.start(),
            moved.duration(),
            moved.pitch(),
            moved.channel(),
            moved.velocity()
        ),
        (beat(5), beat(1), 60, 2, 93)
    );
    assert_eq!(changed.next_id, next_id);
    let reopened = Project::from_snapshot(changed.clone()).unwrap();
    assert_eq!(reopened.snapshot(), changed);
    let mut noop = Command::UpdateMidiNotesWithSourceExtent {
        clip,
        notes: vec![update(note, beat(5), beat(1))],
    };
    assert!(!noop.prepare_sampler_midi_edit(editor.project()).unwrap());
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), changed);
    editor
        .execute(Command::UpdateMidiNotesWithSourceExtent {
            clip,
            notes: vec![update(note, beat(1), beat(1))],
        })
        .unwrap();
    assert_eq!(
        editor.project().midi_clip(clip).unwrap().source_length(),
        beat(6),
        "moving back never deletes retained source time"
    );
}

#[test]
fn extent_update_uses_existing_overwrite_and_atomic_refusal_contracts() {
    let (mut editor, clip, note) = fixture();
    let Created::Note(victim) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(3),
            duration: beat(1),
            pitch: 60,
            channel: 9,
            velocity: 70,
        })
        .unwrap()
    else {
        panic!("victim")
    };
    let before = editor.project().snapshot();
    for notes in [
        vec![],
        vec![update(note, beat(5), BeatTime::ZERO)],
        vec![update(note, BeatTime::from_raw(u64::MAX), beat(1))],
        vec![
            update(note, beat(5), beat(1)),
            update(note, beat(6), beat(1)),
        ],
    ] {
        assert!(
            editor
                .execute(Command::UpdateMidiNotesWithSourceExtent { clip, notes })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
    }
    editor
        .execute(Command::UpdateMidiNotesWithSourceExtent {
            clip,
            notes: vec![update(note, beat(3), beat(3))],
        })
        .unwrap();
    assert!(editor.project().midi_note(victim).is_none());
    assert_eq!(
        editor.project().midi_note(note).unwrap().duration(),
        beat(3)
    );
    assert_eq!(
        editor.project().midi_clip(clip).unwrap().source_length(),
        beat(6)
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
}
