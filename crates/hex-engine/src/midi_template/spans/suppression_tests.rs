use super::*;
use crate::mutable_instrument::{MutableOccurrenceKey, MutableOccurrenceSignature};
use hex_project::{Command, Created, Editor, Project};
use std::mem::{align_of, offset_of, size_of};

// Exact original fields: compiler-measured historical representation, not an
// assumed 120-byte constant or a second production storage path.
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Original {
    track: hex_project::TrackId,
    clip: hex_project::ClipId,
    note: hex_project::NoteId,
    iteration: u64,
    loop_pass: u64,
    until_frame: u64,
    signature: Option<MutableOccurrenceSignature>,
}

fn key() -> MutableOccurrenceKey {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "proof".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_raw(8 << 32),
        })
        .unwrap()
    else {
        panic!("clip");
    };
    let Created::Note(note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(1 << 32),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap()
    else {
        panic!("note");
    };
    MutableOccurrenceKey {
        track,
        clip,
        note,
        iteration: u64::MAX,
        loop_pass: u64::MAX - 1,
    }
}

#[test]
fn original_and_compact_layouts_are_compiled_and_analytic_details_are_canonical() {
    assert_eq!(size_of::<Original>(), 120);
    assert_eq!(size_of::<MidiOccurrenceSuppression>(), 80);
    assert_eq!(align_of::<MidiOccurrenceSuppression>(), 8);
    assert_eq!(offset_of!(MidiOccurrenceSuppression, until_frame), 40);
    assert_eq!(offset_of!(MidiOccurrenceSuppression, onset_beat), 48);
    assert_eq!(offset_of!(MidiOccurrenceSuppression, pitch), 64);
    assert_eq!(offset_of!(MidiOccurrenceSuppression, padding), 68);
    assert_eq!(
        2 * 16_384 * (size_of::<Original>() - size_of::<MidiOccurrenceSuppression>()),
        1_310_720
    );
    let analytic = MidiOccurrenceSuppression::analytic(key(), u64::MAX);
    assert_eq!(analytic.mutable_signature(), None);
    assert_eq!(
        (analytic.onset_beat, analytic.gate_end_beat),
        (BeatTime::ZERO, BeatTime::ZERO)
    );
    assert_eq!(
        [
            analytic.pitch,
            analytic.channel,
            analytic.velocity,
            analytic.kind
        ],
        [0; 4]
    );
    assert_eq!(analytic.padding, [0; 4]);
}

#[test]
fn mutable_signature_roundtrips_every_identity_and_timing_bit() {
    let key = key();
    for (onset, gate, pitch, channel, velocity) in [
        (0, 1, 0, 0, 1),
        (u64::MAX - 1, u64::MAX, 127, 15, 127),
        (0x1234_5678_9abc_def0, 0xfedc_ba98_7654_3210, 61, 7, 99),
    ] {
        let signature = MutableOccurrenceSignature::from_parts(
            key,
            BeatTime::from_raw(onset),
            BeatTime::from_raw(gate),
            pitch,
            channel,
            velocity,
        );
        let compact = MidiOccurrenceSuppression::mutable(signature, u64::MAX);
        assert_eq!(compact.mutable_signature(), Some(signature));
        assert_eq!(compact.until_frame, u64::MAX);
        assert_eq!(compact.padding, [0; 4]);
    }
}

#[test]
fn semantic_sort_and_search_match_original_none_before_some_and_all_details() {
    let base = key();
    let mut originals = Vec::new();
    let mut compact = Vec::new();
    for iteration in [0, 2, u64::MAX] {
        let key = MutableOccurrenceKey { iteration, ..base };
        let marker = compact.len() as u64;
        compact.push(MidiOccurrenceSuppression::analytic(key, marker));
        originals.push(Original {
            track: key.track,
            clip: key.clip,
            note: key.note,
            iteration,
            loop_pass: key.loop_pass,
            until_frame: marker,
            signature: None,
        });
        for changed in 0..5 {
            let (onset, gate, pitch, channel, velocity) = match changed {
                0 => (10, 20, 60, 0, 100),
                1 => (11, 20, 60, 0, 100),
                2 => (10, 21, 60, 0, 100),
                3 => (10, 20, 61, 0, 100),
                _ => (10, 20, 60, 1, 99),
            };
            let signature = MutableOccurrenceSignature::from_parts(
                key,
                BeatTime::from_raw(onset),
                BeatTime::from_raw(gate),
                pitch,
                channel,
                velocity,
            );
            let marker = compact.len() as u64;
            compact.push(MidiOccurrenceSuppression::mutable(signature, marker));
            originals.push(Original {
                track: key.track,
                clip: key.clip,
                note: key.note,
                iteration,
                loop_pass: key.loop_pass,
                until_frame: marker,
                signature: Some(signature),
            });
        }
    }
    compact.reverse();
    compact.sort_unstable_by_key(|entry| {
        (
            entry.track,
            entry.clip,
            entry.note,
            entry.iteration,
            entry.loop_pass,
            entry.mutable_signature(),
        )
    });
    originals.sort_unstable_by_key(|entry| {
        (
            entry.track,
            entry.clip,
            entry.note,
            entry.iteration,
            entry.loop_pass,
            entry.signature,
        )
    });
    assert_eq!(
        compact.iter().map(|e| e.until_frame).collect::<Vec<_>>(),
        originals.iter().map(|e| e.until_frame).collect::<Vec<_>>()
    );
    for original in originals {
        let found = compact
            .binary_search_by_key(
                &(
                    original.track,
                    original.clip,
                    original.note,
                    original.iteration,
                    original.loop_pass,
                    original.signature,
                ),
                |entry| {
                    (
                        entry.track,
                        entry.clip,
                        entry.note,
                        entry.iteration,
                        entry.loop_pass,
                        entry.mutable_signature(),
                    )
                },
            )
            .unwrap();
        assert_eq!(compact[found].until_frame, original.until_frame);
    }
}
