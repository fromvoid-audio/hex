//! Negative-local note authoring reuses the existing unsigned source format.
use hex_project::{BeatTime, Command, Created, Editor, MidiNoteUpdate, Project};

#[test]
fn rebased_notes_keep_complete_project_state_through_codec_and_history() {
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "prefix".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(4),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let Created::Note(note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(1),
            pitch: 60,
            channel: 2,
            velocity: 71,
        })
        .unwrap()
    else {
        panic!("note")
    };
    editor
        .execute(Command::SetMidiNotesActive {
            clip,
            notes: vec![note],
            active: false,
        })
        .unwrap();
    let Created::Clips { first: alias, .. } = editor
        .execute(Command::DuplicateAliasClips(vec![clip]))
        .unwrap()
    else {
        panic!("alias")
    };
    editor
        .execute(Command::SetMidiClipSource {
            clip: alias,
            source_length: beat(4),
            source_start: BeatTime::ZERO,
            loop_start: beat(1),
            loop_end: beat(4),
            looping: true,
            length: beat(4),
        })
        .unwrap();
    editor.execute(Command::ReverseClips(vec![alias])).unwrap();
    let before = editor.project().clone();
    editor
        .execute(Command::UpdateMidiNotesWithSourcePrefix {
            clip,
            prefix: beat(2),
            notes: vec![MidiNoteUpdate {
                note,
                start: BeatTime::ZERO,
                duration: beat(1),
                pitch: 62,
                channel: 2,
                velocity: 71,
            }],
        })
        .unwrap();
    let after = editor.project().clone();
    let bytes = hex_storage::encode(&after).unwrap();
    assert_eq!(hex_storage::decode(&bytes).unwrap(), after);
    assert_eq!(
        hex_storage::encode(&hex_storage::decode(&bytes).unwrap()).unwrap(),
        bytes
    );
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
}
