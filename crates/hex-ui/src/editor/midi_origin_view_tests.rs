//! Maximum canonical history depth and ordinary source edits share one view origin.
use super::State;
use hex_project::{BeatTime, Command, Created, Editor, MidiNoteUpdate, Project};

#[test]
fn all_128_canonical_prefix_edges_restore_their_original_view_origins() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 128).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "history".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
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
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    else {
        panic!("note")
    };
    let mut state = State::default();
    for _ in 0..128 {
        let old = editor.project().midi_clip(clip).unwrap().source_identity();
        editor
            .execute(Command::UpdateMidiNotesWithSourcePrefix {
                clip,
                prefix: BeatTime::ONE,
                notes: vec![MidiNoteUpdate {
                    note,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                }],
            })
            .unwrap();
        let new = editor.project().midi_clip(clip).unwrap().source_identity();
        state.record_prefix(7, clip, &old, &new, BeatTime::ONE.raw());
        assert_eq!(
            state.advance(7, clip, &old, &new),
            i128::from(BeatTime::ONE.raw())
        );
    }
    for _ in 0..128 {
        let old = editor.project().midi_clip(clip).unwrap().source_identity();
        assert!(editor.undo());
        let new = editor.project().midi_clip(clip).unwrap().source_identity();
        assert_eq!(
            state.advance(7, clip, &old, &new),
            -i128::from(BeatTime::ONE.raw())
        );
    }
    let old = editor.project().midi_clip(clip).unwrap().source_identity();
    assert!(editor.redo());
    let new = editor.project().midi_clip(clip).unwrap().source_identity();
    assert_eq!(
        state.advance(7, clip, &old, &new),
        i128::from(BeatTime::ONE.raw())
    );
    editor
        .execute(Command::SetMidiNotesActive {
            clip,
            notes: vec![note],
            active: false,
        })
        .unwrap();
    let ordinary = editor.project().midi_clip(clip).unwrap().source_identity();
    assert_eq!(state.advance(7, clip, &new, &ordinary), 0);
    assert_eq!(
        state.advance(8, clip, &ordinary, &old),
        0,
        "a different project generation cannot reuse history metadata"
    );
}
