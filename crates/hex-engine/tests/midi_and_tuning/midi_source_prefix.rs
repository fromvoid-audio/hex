//! Public event compiler evidence for retained negative onsets and play-start admission.
use hex_engine::{compile::midi_events, midi_compile::MidiEventKind};
use hex_project::{BeatTime, Command, Created, Editor, MidiNoteUpdate, Project, Tempo};

#[test]
fn source_prefix_keeps_unmoved_deadlines_and_existing_source_start_admission() {
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
    assert!(
        moved.is_empty(),
        "the existing source-start contract does not chase a pre-marker onset"
    );
    assert!(
        editor.project().midi_note(ids[1]).is_some(),
        "the out-of-window note remains authored"
    );
    let start = editor.project().midi_source(clip).unwrap().source_start();
    editor
        .execute(Command::UpdateMidiNote {
            note: ids[1],
            start,
            duration: beat(1, 1),
            pitch: 60,
            channel: 2,
            velocity: 71,
        })
        .unwrap();
    let admitted = midi_events(editor.project()).unwrap();
    assert_eq!(held(&admitted), held(&before));
    let moved = admitted
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
            (MidiEventKind::NoteOff, frame(beat(5, 1)), 2, 71, 0)
        ]
    );
}

#[test]
fn source_prefix_preserves_crossing_gates_in_looped_trimmed_and_reversed_windows() {
    use hex_project::TrimEdge;
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    for (looping, trimmed, reversed) in [
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
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
                start: beat(4),
                length: beat(9),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::SetMidiClipSource {
                clip,
                source_length: beat(6),
                source_start: BeatTime::ZERO,
                loop_start: beat(1),
                loop_end: beat(4),
                looping,
                length: beat(9),
            })
            .unwrap();
        let mut add_note = |start, duration, pitch| {
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
            id
        };
        let opening = add_note(BeatTime::ZERO, beat(2), 60);
        let crossing = add_note(beat(3), beat(2), 64);
        let moved = add_note(beat(2), beat(2), 67);
        if trimmed {
            editor
                .execute(Command::TrimMidiClipEdge {
                    clip,
                    edge: TrimEdge::Left,
                    beat: beat(5),
                })
                .unwrap();
        }
        if reversed {
            editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        }
        let before = midi_events(editor.project()).unwrap();
        let old_clip = editor.project().midi_clip(clip).unwrap().clone();
        let duration = beat(2);
        editor
            .execute(Command::UpdateMidiNotesWithSourcePrefix {
                clip,
                prefix: beat(1),
                notes: vec![MidiNoteUpdate {
                    note: moved,
                    start: BeatTime::ZERO,
                    duration,
                    pitch: 67,
                    channel: 2,
                    velocity: 71,
                }],
            })
            .unwrap();

        let new_clip = editor.project().midi_clip(clip).unwrap();
        assert_eq!(new_clip.start(), old_clip.start());
        assert_eq!(new_clip.length(), old_clip.length());
        assert_eq!(new_clip.playback_offset(), old_clip.playback_offset());
        assert_eq!(new_clip.content_view(), old_clip.content_view());
        assert_eq!(new_clip.loop_start(), beat(2));
        assert_eq!(new_clip.loop_end(), beat(5));
        assert_eq!(new_clip.looping(), looping);
        assert_eq!(
            editor.project().midi_note(moved).unwrap().duration(),
            duration
        );
        let after = midi_events(editor.project()).unwrap();
        assert!(before.iter().any(|edge| edge.note == moved));
        assert_eq!(
            after,
            before
                .into_iter()
                .filter(|edge| edge.note != moved)
                .collect::<Vec<_>>()
        );

        for note in [opening, crossing] {
            let edges: Vec<_> = after.iter().filter(|edge| edge.note == note).collect();
            assert!(!edges.is_empty());
            assert!(edges.chunks(2).all(|pair| pair.len() == 2
                && pair[0].kind == MidiEventKind::NoteOn
                && pair[1].kind == MidiEventKind::NoteOff
                && pair[0].frame < pair[1].frame));
        }
    }
}
