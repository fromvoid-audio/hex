use hex_project::{BeatTime, Command, Created, Editor, MidiOneShotBoundary, Project};

fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn fixture() -> Project {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = e
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(1),
            length: beat(12),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    e.execute(Command::AddMidiNote {
        clip,
        start: beat(10),
        duration: beat(2),
        pitch: 60,
        channel: 0,
        velocity: 100,
    })
    .unwrap();
    e.execute(Command::SetMidiClipSourceWithOverwrite {
        clip,
        source_length: beat(12),
        source_start: beat(2),
        loop_start: beat(2),
        loop_end: beat(6),
        looping: true,
        length: beat(20),
    })
    .unwrap();
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: beat(12),
        source_start: beat(2),
        loop_start: beat(2),
        loop_end: beat(6),
        looping: false,
        length: beat(20),
    })
    .unwrap();
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    e.execute(Command::CopyArrangementMidiToSession {
        source: clip,
        scene,
    })
    .unwrap();
    e.project().clone()
}

#[test]
fn current_format_preserves_bounded_once_notes_and_both_source_owners() {
    let project = fixture();
    let bytes = hex_storage::encode(&project).unwrap();
    let decoded = hex_storage::decode(&bytes).unwrap();
    assert_eq!(decoded.snapshot(), project.snapshot());
    let clip = &decoded.tracks()[0].midi_clips()[0];
    let hex_project::SessionCellContent::Midi(cell) = decoded.session_cells()[0].content() else {
        panic!("cell")
    };
    for source in [clip.source(), cell.source()] {
        assert_eq!(source.one_shot_boundary(), MidiOneShotBoundary::LoopEnd);
        assert!(!source.looping());
        assert_eq!(source.notes().len(), 1);
        assert_eq!(source.notes()[0].start(), beat(10));
        assert_eq!(source.source_length(), beat(12));
        assert_eq!(
            source
                .source_playback(beat(20))
                .unwrap()
                .note_occurrences(beat(10), beat(2))
                .unwrap()
                .count(),
            0
        );
    }
    // The ordinary policy also round trips as an explicit current-format value.
    let mut snapshot = decoded.snapshot();
    let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } =
        &mut snapshot.tracks[0].content
    else {
        panic!("hybrid")
    };
    midi_clips[0].one_shot_boundary = MidiOneShotBoundary::SourceEnd;
    let ordinary = Project::from_snapshot(snapshot).unwrap();
    let ordinary = hex_storage::decode(&hex_storage::encode(&ordinary).unwrap()).unwrap();
    assert_eq!(
        ordinary.tracks()[0].midi_clips()[0]
            .source_playback()
            .unwrap()
            .note_occurrences(beat(10), beat(2))
            .unwrap()
            .count(),
        1
    );
}

fn rewrite(value: &mut serde_json::Value, replacement: Option<serde_json::Value>) -> usize {
    match value {
        serde_json::Value::Object(map) => {
            let mut count = 0;
            if map.contains_key("one_shot_boundary") {
                if let Some(next) = replacement.clone() {
                    map.insert("one_shot_boundary".into(), next);
                } else {
                    map.remove("one_shot_boundary");
                }
                count += 1;
            }
            count
                + map
                    .values_mut()
                    .map(|v| rewrite(v, replacement.clone()))
                    .sum::<usize>()
        }
        serde_json::Value::Array(values) => values
            .iter_mut()
            .map(|v| rewrite(v, replacement.clone()))
            .sum(),
        _ => 0,
    }
}

#[test]
fn current_format_refuses_missing_or_unknown_boundary() {
    let original = hex_storage::encode(&fixture()).unwrap();
    for replacement in [
        None,
        Some(serde_json::json!("unknown")),
        Some(serde_json::json!(42)),
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        assert_eq!(rewrite(&mut value, replacement), 2);
        let malformed = serde_json::to_vec(&value).unwrap();
        assert!(hex_storage::decode(&malformed).is_err());
    }
    assert!(hex_storage::decode(&original).is_ok());
}
