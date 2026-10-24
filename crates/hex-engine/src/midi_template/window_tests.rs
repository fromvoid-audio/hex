use super::*;
use crate::{MidiSpanBuffer, PreparedMidiInstrumentTemplate, PreparedTempoMap};
use hex_project::{Command, Created, Editor, Project};
fn b(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn fixture(looping: bool) -> (Editor, TrackId, ClipId, NoteId) {
    let mut e = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
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
