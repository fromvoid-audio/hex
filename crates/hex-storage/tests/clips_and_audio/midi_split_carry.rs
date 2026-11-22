use hex_project::{BeatTime, Command, Created, Editor, Project};

#[test]
fn subframe_split_carry_roundtrips_the_retained_note_and_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "notes".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(BeatTime::ONE.raw() + 1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let frame = editor
        .project()
        .tempo_map()
        .beat_to_frame(BeatTime::ONE)
        .unwrap();
    editor
        .execute(Command::SplitTracksAtFrame {
            tracks: vec![track],
            frame,
        })
        .unwrap();
    let split = editor.project().clone();
    let right = &split.track(track).unwrap().midi_clips()[1];
    assert!(right.content_view().is_some());
    assert_eq!(
        right.notes()[0].duration(),
        BeatTime::from_raw(BeatTime::ONE.raw() + 1)
    );
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(&split).unwrap()).unwrap(),
        split
    );
    assert!(editor.undo());
    assert!(editor.redo());
    assert_eq!(editor.project(), &split);
}
