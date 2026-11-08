use super::*;
use hex_lattice::ForeignNote;
use hex_project::{Command, Created, Editor, Project};

#[test]
fn foreign_gates_keep_channel_and_identity_across_segment_boundaries() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "instrument".into(),
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
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 2,
            velocity: 101,
        })
        .unwrap();
    let note = editor.project().midi_clip(clip).unwrap().notes()[0].id();
    let mut onsets = MutableOnsetBuffer::default();
    for (iteration, (start, end, channel)) in [(11, 14, 2), (13, 16, 3)].into_iter().enumerate() {
        onsets
            .push(MutableNoteOnset {
                key: MutableOccurrenceKey::new(track, clip, note, iteration as u64, 0),
                onset_beat: BeatTime::from_raw(start),
                gate_end_beat: BeatTime::from_raw(end),
                source_start: start,
                source_gate_end: end,
                pitch: 60,
                channel,
                velocity: 101,
            })
            .unwrap();
    }

    let mut scratch = Vec::with_capacity(MAX_MUTABLE_OCCURRENCE_PROOFS);
    let mut next_id = 70_000;
    let mut events = Vec::new();
    for (performance_start, source_start) in [(100, 10), (104, 14)] {
        stage_authored_gates(
            &mut scratch,
            &mut next_id,
            &mut onsets,
            AuthoredGateContext {
                track: Some(track),
                performance_start,
                source_start,
                width: 4,
                source_age: None,
                tail_frames: 0,
                max_active: 4,
                steals_releasing: false,
                live: 0,
                suppressions: &[],
            },
            |gate| {
                let note = ForeignNote::try_from(gate)?;
                events.push((
                    note.frame,
                    note.channel,
                    note.pitch,
                    note.velocity,
                    note.on,
                    note.note_id.unwrap().get(),
                ));
                Ok(())
            },
        )
        .unwrap();
        onsets.clear();
    }
    assert_eq!(
        events,
        [
            (1, 2, 60, 101, true, 70_001),
            (3, 3, 60, 101, true, 70_002),
            (0, 2, 60, 0, false, 70_001),
            (2, 3, 60, 0, false, 70_002),
        ]
    );
}

#[test]
fn foreign_note_ids_and_offsets_refuse_narrowing() {
    let gate = AuthoredGate {
        frame_offset: 0,
        occurrence_id: 1 + i32::MAX as u64,
        pitch: 60,
        channel: 0,
        velocity: 101,
        on: true,
    };
    assert_eq!(
        ForeignNote::try_from(gate),
        Err(MutableInstrumentRuntimeError::IdentityExhausted)
    );
    assert_eq!(
        ForeignNote::try_from(AuthoredGate {
            frame_offset: usize::from(u16::MAX) + 1,
            occurrence_id: 1,
            ..gate
        }),
        Err(MutableInstrumentRuntimeError::Arithmetic)
    );
}
