#[test]
fn split_midi_trim_preview_keeps_the_retained_source_window() {
    use hex_project::{BeatTime, Command, Created, Editor, Project, TimeRange, TrimEdge};
    let b = |n| BeatTime::from_quarters(n).unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: b(0),
            length: b(12),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: b(4),
            source_start: b(0),
            loop_start: b(1),
            loop_end: b(4),
            looping: true,
            length: b(12),
        })
        .unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: b(2),
            duration: b(2),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::SetTimeRangeActive {
            range: TimeRange::new(vec![track], 72_000, 192_000).unwrap(),
            active: false,
        })
        .unwrap();
    let project = editor.project();
    let tail = &project.track(track).unwrap().midi_clips()[2];
    let trim = tail.preview_edge_trim(TrimEdge::Right, b(12)).unwrap();
    assert_eq!(
        super::midi_trim_note_shapes(project, tail, trim, 192_000, 288_000, 0),
        vec![(0.0, 0.5, 60), (0.75, 0.25, 60)]
    );
}
