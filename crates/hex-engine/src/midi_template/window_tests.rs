use super::*;
use crate::{MidiSpanBuffer, PreparedMidiInstrumentTemplate, PreparedTempoMap};
use hex_project::{Command, Created, Editor, Project};
fn b(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn fixture(looping: bool) -> (Editor, TrackId, ClipId, NoteId) {
    fixture_at(looping, 8_000)
}
fn fixture_at(looping: bool, sample_rate: u32) -> (Editor, TrackId, ClipId, NoteId) {
    let mut e = Editor::new(Project::new(sample_rate).unwrap(), 16).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "window".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = e
        .execute(Command::PlaceMidiClip {
            track,
            start: b(0),
            length: b(12),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: b(4),
        source_start: b(0),
        loop_start: b(1),
        loop_end: b(4),
        looping,
        length: b(12),
    })
    .unwrap();
    let Created::Note(note) = e
        .execute(Command::AddMidiNote {
            clip,
            start: b(2),
            duration: b(2),
            pitch: 60,
            channel: 0,
            velocity: 93,
        })
        .unwrap()
    else {
        panic!("note")
    };
    (e, track, clip, note)
}
#[test]
fn elapsed_window_source_edges_gates_and_handoff_use_original_iteration() {
    let (e, track, clip, note) = fixture(true);
    let timing = PreparedTempoMap::prepare(e.project().tempo_map().clone(), 8_000).unwrap();
    let raw = e.project().midi_clip(clip).unwrap().source();
    for (offset, iteration, remaining) in [
        (b(3), 0, b(1)),
        (b(9), 2, b(1)),
        (b(3_000_000_000), 999_999_999, b(1)),
    ] {
        let source =
            PreparedMidiSourceTemplate::prepare_source_window(track, clip, raw, offset, &timing)
                .unwrap();
        let hit = source
            .occurrence_with_origin(note, iteration, SessionMidiOrigin::launch(b(0)))
            .unwrap();
        assert_eq!((hit.onset, hit.gate_end), (b(0), remaining));
        let mut edges = MidiEdgeBuffer::default();
        source.query(b(0), &timing, 0, 1, &mut edges).unwrap();
        let events = edges.events().collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_eq!(
            (events[0].frame, events[0].iteration, events[0].kind),
            (0, iteration, MidiEventKind::NoteOn)
        );
        let instrument =
            PreparedMidiInstrumentTemplate::prepare(&source, &hex_devices::SineTable::new())
                .unwrap();
        let gates = instrument.capture_gate_template().unwrap();
        let mut spans = MidiSpanBuffer::default();
        gates
            .query_with_origin(SessionMidiOrigin::launch(b(0)), &timing, 0, 1, &mut spans)
            .unwrap();
        let gate = spans.spans().next().unwrap();
        assert_eq!(
            (gate.iteration, gate.onset_beat, gate.gate_end_beat),
            (iteration, b(0), remaining)
        );
        let canonical = raw
            .playback_for_duration(b(2))
            .unwrap()
            .with_playback_offset(offset)
            .unwrap();
        let expected = canonical
            .note_occurrences(b(2), b(2))
            .unwrap()
            .iter()
            .next()
            .unwrap();
        assert_eq!(
            (
                expected.iteration(),
                expected.timeline_start(),
                expected.gate_duration()
            ),
            (iteration, b(0), remaining)
        );
    }
}
#[test]
fn expired_one_shot_and_inactive_loop_prepare_empty_source_work() {
    for looping in [false, true] {
        let (e, track, clip, _) = fixture(looping);
        let timing = PreparedTempoMap::prepare(e.project().tempo_map().clone(), 8_000).unwrap();
        let raw = e.project().midi_clip(clip).unwrap().source();
        let source = PreparedMidiSourceTemplate::prepare_source_window_active(
            track,
            clip,
            raw,
            b(9),
            !looping,
            &timing,
        )
        .unwrap();
        assert_eq!(
            (source.max_query_edges(), source.max_query_onsets()),
            (0, 0)
        );
        assert!(source.intro.is_empty());
        assert!(source.repeats.is_empty());

        let mut edges = MidiEdgeBuffer::default();
        source.query(b(0), &timing, 0, 256, &mut edges).unwrap();
        assert!(edges.events().next().is_none());
    }
}

#[test]
fn looped_edges_cross_tempo_change_and_partition_boundaries_without_loss() {
    use hex_project::Tempo;
    for sample_rate in [8_000_u32, 44_100, 48_000] {
        let (mut editor, track, clip, first) = fixture_at(true, sample_rate);
        let quarter = u64::from(sample_rate) / 2;
        editor
            .execute(Command::SetTempoPoint {
                beat: b(4),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let Created::Note(second) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: b(1),
                duration: b(1),
                pitch: 62,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("note")
        };
        let timing =
            PreparedTempoMap::prepare(editor.project().tempo_map().clone(), sample_rate).unwrap();
        let source = PreparedMidiSourceTemplate::prepare_source_window(
            track,
            clip,
            editor.project().midi_clip(clip).unwrap().source(),
            b(0),
            &timing,
        )
        .unwrap();
        // At 120 bpm a beat is half a second before beat 4; at 60 bpm
        // each later beat is a full second. Source loop repeats beats 1..4.
        let expected = [
            (quarter, MidiEventKind::NoteOn, second, 0),
            (2 * quarter, MidiEventKind::NoteOff, second, 0),
            (2 * quarter, MidiEventKind::NoteOn, first, 0),
            (4 * quarter, MidiEventKind::NoteOff, first, 0),
            (4 * quarter, MidiEventKind::NoteOn, second, 1),
            (6 * quarter, MidiEventKind::NoteOff, second, 1),
            (6 * quarter, MidiEventKind::NoteOn, first, 1),
            (10 * quarter, MidiEventKind::NoteOff, first, 1),
            (10 * quarter, MidiEventKind::NoteOn, second, 2),
            (12 * quarter, MidiEventKind::NoteOff, second, 2),
            (12 * quarter, MidiEventKind::NoteOn, first, 2),
        ];
        for block in [1_u64, 17, 64, 128, 256, 512] {
            let mut actual = Vec::new();
            let mut buffer = MidiEdgeBuffer::default();
            let mut block_start = 0;
            while block_start < 12 * quarter + 1 {
                let block_end = (block_start + block).min(12 * quarter + 1);
                let mut start = block_start;
                while start < block_end {
                    let end = (start + MIDI_TEMPLATE_QUERY_FRAMES).min(block_end);
                    source
                        .query(b(0), &timing, start, end, &mut buffer)
                        .unwrap();
                    actual.extend(
                        buffer
                            .events()
                            .map(|event| (event.frame, event.kind, event.note, event.iteration)),
                    );
                    start = end;
                }
                block_start = block_end;
            }
            assert_eq!(actual, expected, "block size {block}");
            source
                .query(b(0), &timing, quarter, quarter + 1, &mut buffer)
                .unwrap();
            assert_eq!(buffer.events().count(), 1);
            assert_eq!(
                source.query(b(0), &timing, 0, 257, &mut buffer),
                Err(MidiTemplateError::QueryTooWide)
            );
            assert_eq!(
                buffer.events().count(),
                0,
                "refused query clears prior edges"
            );
        }
    }
}

#[test]
fn cued_loop_excludes_pre_cue_gate_and_keeps_future_note_offs_across_tempo_change() {
    use hex_project::Tempo;
    let (mut editor, track, clip, note) = fixture(true);
    editor
        .execute(Command::SetTempoPoint {
            beat: b(8),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let timing = PreparedTempoMap::prepare(editor.project().tempo_map().clone(), 8_000).unwrap();
    let source = PreparedMidiSourceTemplate::prepare_source_window(
        track,
        clip,
        editor.project().midi_clip(clip).unwrap().source(),
        b(0),
        &timing,
    )
    .unwrap();
    // Cue at global beat 5 from elapsed beat 3: the note that began at
    // elapsed beat 2 is not chased, nor may its old note-off be emitted.
    // Repeats at elapsed 5 and 8 project to global beats 7 and 10.
    let origin = SessionMidiOrigin::new(b(5), b(3));
    let expected = [
        (28_000, MidiEventKind::NoteOn, note, 1),
        (40_000, MidiEventKind::NoteOff, note, 1),
        (48_000, MidiEventKind::NoteOn, note, 2),
        (64_000, MidiEventKind::NoteOff, note, 2),
    ];
    for block in [1_u64, 17, 64, 128, 256, 512] {
        let mut actual = Vec::new();
        let mut buffer = MidiEdgeBuffer::default();
        let mut block_start = 0;
        while block_start < 64_001 {
            let block_end = (block_start + block).min(64_001);
            let mut start = block_start;
            while start < block_end {
                let end = (start + MIDI_TEMPLATE_QUERY_FRAMES).min(block_end);
                source
                    .query_with_origin(origin, &timing, start, end, &mut buffer)
                    .unwrap();
                actual.extend(
                    buffer
                        .events()
                        .map(|event| (event.frame, event.kind, event.note, event.iteration)),
                );
                start = end;
            }
            block_start = block_end;
        }
        assert_eq!(actual, expected, "block size {block}");
    }
}
