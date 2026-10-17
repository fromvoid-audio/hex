use hex_engine::{MidiSpanBuffer, PreparedRevision, SessionPlaybackOwner};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn applied_source_gate_query_covers_all_notes_not_the_sampled_drawing_preview() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 64).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "activity".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip");
    };
    for pitch in 40..80 {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "activity".into(),
        })
        .unwrap()
    else {
        panic!("scene");
    };
    let Created::SessionClip(session) = editor
        .execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("session clip");
    };
    let prepared = PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(hex_devices::SineTable::new()),
        None,
    )
    .unwrap();
    assert_eq!(prepared.midi_preview().get(clip).unwrap().samples.len(), 32);
    let mut spans = MidiSpanBuffer::default();
    for owner in [
        SessionPlaybackOwner::Arrangement,
        SessionPlaybackOwner::Session { clip: session },
    ] {
        let keys = prepared
            .midi_gate_keys(track, owner, Some(BeatTime::ZERO), (0, 0), &mut spans)
            .unwrap();
        for pitch in 0..128 {
            assert_eq!(
                keys[pitch / 64] & (1_u64 << (pitch % 64)) != 0,
                (40..80).contains(&pitch)
            );
        }
        assert_eq!(
            prepared
                .midi_gate_keys(
                    track,
                    owner,
                    Some(BeatTime::ZERO),
                    (12000, 12000),
                    &mut spans
                )
                .unwrap(),
            [0; 2],
            "release tails are not held gates"
        );
    }
    assert!(
        prepared
            .midi_gate_keys(
                track,
                SessionPlaybackOwner::Session { clip: session },
                None,
                (0, 0),
                &mut spans
            )
            .is_err()
    );
    assert!(
        prepared
            .midi_gate_keys(
                track,
                SessionPlaybackOwner::Arrangement,
                None,
                (u64::MAX, 0),
                &mut spans
            )
            .is_err()
    );
    assert_eq!(
        prepared
            .midi_gate_keys(
                track,
                SessionPlaybackOwner::Silence,
                None,
                (0, 0),
                &mut spans
            )
            .unwrap(),
        [0; 2]
    );
}
