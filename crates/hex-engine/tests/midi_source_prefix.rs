//! Independent public event compiler evidence for a note crossing source zero.
use hex_engine::{compile::midi_events, midi_compile::MidiEventKind};
use hex_project::{BeatTime, Command, Created, Editor, MidiNoteUpdate, Project, Tempo};

#[test]
fn source_prefix_keeps_unmoved_gate_deadlines_and_clips_the_crossing_author_at_play_start() {
    let beat = |n, d| BeatTime::from_ratio(n, d).unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "prefix".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(4, 1),
            length: beat(4, 1),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let mut ids = Vec::new();
    for (start, duration, pitch) in [
        (beat(0, 1), beat(3, 1), 64),
        (beat(1, 2), beat(1, 1), 60),
        (beat(0, 1), beat(1, 4), 60),
    ] {
        let Created::Note(id) = editor
            .execute(Command::AddMidiNote {
                clip,
                start,
                duration,
                pitch,
                channel: 2,
                velocity: 71,
            })
            .unwrap()
        else {
            panic!("note")
        };
        ids.push(id);
    }
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(17, 4),
            tempo: Tempo::new(93_500_000).unwrap(),
        })
        .unwrap();
    let before = midi_events(editor.project()).unwrap();
    editor
        .execute(Command::UpdateMidiNotesWithSourcePrefix {
            clip,
            prefix: beat(1, 2),
            notes: vec![MidiNoteUpdate {
                note: ids[1],
                start: BeatTime::ZERO,
                duration: beat(1, 1),
                pitch: 60,
                channel: 2,
                velocity: 71,
            }],
        })
        .unwrap();
    let after = midi_events(editor.project()).unwrap();
    let held = |edges: &[hex_engine::midi_compile::CompiledMidiEvent]| {
        edges
            .iter()
            .copied()
            .filter(|edge| edge.note == ids[0])
            .collect::<Vec<_>>()
    };
    assert_eq!(held(&after), held(&before));
    assert_eq!(held(&after).len(), 2);
    assert!(
        !after.iter().any(|edge| edge.note == ids[2]),
        "negative crossing author overwrites the old positive victim"
    );
    let moved = after
        .iter()
        .filter(|edge| edge.note == ids[1])
        .map(|edge| {
            (
                edge.kind,
                edge.frame,
                edge.channel,
                edge.velocity,
                edge.iteration,
            )
        })
        .collect::<Vec<_>>();
    let frame = |beat| editor.project().tempo_map().beat_to_frame(beat).unwrap();
    assert_eq!(
        moved,
        vec![
            (MidiEventKind::NoteOn, frame(beat(4, 1)), 2, 71, 0),
            (MidiEventKind::NoteOff, frame(beat(9, 2)), 2, 71, 0)
        ]
    );
}
