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

mod onset_provenance {
    use super::*;

    #[test]
    fn source_edges_spans_and_gate_capture_keep_crossing_and_real_onset_distinct() {
        let (mut editor, track, clip, crossing) = fixture(true);
        let Created::Note(boundary) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: b(3),
                duration: b(1),
                pitch: 67,
                channel: 0,
                velocity: 88,
            })
            .unwrap()
        else {
            panic!("boundary note")
        };
        let timing =
            PreparedTempoMap::prepare(editor.project().tempo_map().clone(), 8_000).unwrap();
        let raw = editor.project().midi_clip(clip).unwrap().source();
        for offset in [b(3), b(9)] {
            let source = PreparedMidiSourceTemplate::prepare_source_window(
                track, clip, raw, offset, &timing,
            )
            .unwrap();
            let instrument =
                PreparedMidiInstrumentTemplate::prepare(&source, &hex_devices::SineTable::new())
                    .unwrap();
            let captured = instrument.capture_gate_template().unwrap();
            let mut edges = MidiEdgeBuffer::default();
            for (start, end, kind) in [
                (0, 1, MidiEventKind::NoteOn),
                (3_999, 4_001, MidiEventKind::NoteOff),
            ] {
                source.query(b(0), &timing, start, end, &mut edges).unwrap();
                let actual = edges
                    .events()
                    .map(|edge| (edge.note, edge.kind, edge.starts_inside_gate))
                    .collect::<Vec<_>>();
                assert_eq!(actual.len(), 2);
                assert!(actual.contains(&(crossing, kind, true)));
                assert!(actual.contains(&(boundary, kind, false)));
            }
            let mut original = MidiSpanBuffer::default();
            instrument
                .query(b(0), &timing, 0, 1, &mut original)
                .unwrap();
            let mut copy = MidiSpanBuffer::default();
            captured
                .query_with_origin(SessionMidiOrigin::launch(b(0)), &timing, 0, 1, &mut copy)
                .unwrap();
            let metadata = |buffer: &MidiSpanBuffer| {
                buffer
                    .spans()
                    .map(|span| {
                        (
                            span.note,
                            span.iteration,
                            span.start_frame,
                            span.gate_end_frame,
                            span.starts_inside_gate,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(metadata(&original), metadata(&copy));
            assert!(
                original
                    .spans()
                    .any(|span| span.note == crossing && span.starts_inside_gate)
            );
            assert!(
                original
                    .spans()
                    .any(|span| span.note == boundary && !span.starts_inside_gate)
            );
            source
                .query(b(0), &timing, 8_000, 8_001, &mut edges)
                .unwrap();
            assert!(
                edges
                    .events()
                    .any(|edge| edge.note == crossing && !edge.starts_inside_gate),
                "later original repeat remains a real onset"
            );
        }
    }
}

mod note_chase {
    use super::*;
    use crate::{MidiInstrumentRenderContext, NoteChase, PreparedTrackMix};

    fn source_fixture() -> (PreparedMidiSourceTemplate, PreparedTempoMap, NoteId, NoteId) {
        let (mut editor, track, clip, crossing) = fixture(true);
        let Created::Note(boundary) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: b(3),
                duration: BeatTime::from_ratio(1, 4).unwrap(),
                pitch: 64,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("boundary note");
        };
        let timing =
            PreparedTempoMap::prepare(editor.project().tempo_map().clone(), 8_000).unwrap();
        let source = PreparedMidiSourceTemplate::prepare_source_window(
            track,
            clip,
            editor.project().midi_clip(clip).unwrap().source(),
            b(3),
            &timing,
        )
        .unwrap();
        (source, timing, crossing, boundary)
    }

    struct RejectClippedGate<'a> {
        timing: &'a PreparedTempoMap,
        sample_rate: u32,
    }
    impl MidiTiming for RejectClippedGate<'_> {
        fn sample_rate(&self) -> u32 {
            self.sample_rate
        }
        fn max_micro_bpm(&self) -> u64 {
            u64::from(self.timing.max_micro_bpm())
        }
        fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, crate::PreparedTempoError> {
            if beat == b(1) {
                return Err(crate::PreparedTempoError::BeatToFrame(
                    "clipped gate conversion",
                ));
            }
            self.timing.beat_to_frame(beat)
        }
        fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, crate::PreparedTempoError> {
            self.timing.frame_to_beat(frame)
        }
        fn beat_lower_bound_for_frame(
            &self,
            frame: u64,
        ) -> Result<u128, crate::PreparedTempoError> {
            self.timing.beat_lower_bound_for_frame(frame)
        }
    }

    #[test]
    fn skipped_occurrences_do_not_convert_but_query_validation_still_applies() {
        let (source, timing, _, boundary) = source_fixture();
        let instrument =
            PreparedMidiInstrumentTemplate::prepare(&source, &hex_devices::SineTable::new())
                .unwrap();
        let gates = instrument.capture_gate_template().unwrap();
        let origin = SessionMidiOrigin::launch(b(0));
        let mut timing = RejectClippedGate {
            timing: &timing,
            sample_rate: 8_000,
        };
        let mut edges = MidiEdgeBuffer::default();
        let mut spans = MidiSpanBuffer::default();
        for choice in [NoteChase::On, NoteChase::Off] {
            let expected = if choice == NoteChase::On {
                Err(MidiTemplateError::TempoConversion)
            } else {
                Ok(())
            };
            assert_eq!(
                source.query_with_origin_and_note_chase(
                    origin,
                    &timing,
                    (4_000, 4_001),
                    &mut edges,
                    choice
                ),
                expected
            );
            assert_eq!(edges.events().count(), 0, "skipped gate has no orphan off");
            assert_eq!(
                instrument.query_with_origin_and_note_chase(
                    origin,
                    &timing,
                    (0, 1),
                    &mut spans,
                    choice
                ),
                expected
            );
            assert_eq!(
                spans.spans().map(|s| s.note).collect::<Vec<_>>(),
                if choice == NoteChase::Off {
                    vec![boundary]
                } else {
                    vec![]
                }
            );
            assert_eq!(
                gates.query_with_origin_and_note_chase(origin, &timing, (0, 1), &mut spans, choice),
                expected
            );
            assert_eq!(
                spans.spans().map(|s| s.note).collect::<Vec<_>>(),
                if choice == NoteChase::Off {
                    vec![boundary]
                } else {
                    vec![]
                }
            );
        }
        timing.sample_rate = 8_001;
        for choice in [NoteChase::On, NoteChase::Off] {
            assert_eq!(
                source.query_with_origin_and_note_chase(
                    origin,
                    &timing,
                    (0, 1),
                    &mut edges,
                    choice
                ),
                Err(MidiTemplateError::TempoRateMismatch)
            );
            assert_eq!(
                instrument.query_with_origin_and_note_chase(
                    origin,
                    &timing,
                    (0, 1),
                    &mut spans,
                    choice
                ),
                Err(MidiTemplateError::TempoRateMismatch)
            );
            assert_eq!(spans.spans().count(), 0);
        }
    }

    #[test]
    fn queries_skip_only_the_clipped_intro_and_its_paired_off_across_partitions() {
        let (source, timing, crossing, boundary) = source_fixture();
        let table = hex_devices::SineTable::new();
        let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
        let gates = instrument.capture_gate_template().unwrap();
        for choice in [NoteChase::On, NoteChase::Off] {
            for width in [1, 17, 256] {
                let mut edges = MidiEdgeBuffer::default();
                let mut spans = MidiSpanBuffer::default();
                let mut gate_spans = MidiSpanBuffer::default();
                let mut events = Vec::new();
                let mut saw_repeat = false;
                for start in (0..8_256).step_by(width) {
                    let end = (start + width as u64).min(8_256);
                    let origin = SessionMidiOrigin::launch(b(0));
                    source
                        .query_with_origin_and_note_chase(
                            origin,
                            &timing,
                            (start, end),
                            &mut edges,
                            choice,
                        )
                        .unwrap();
                    if start == 0 {
                        assert_eq!(edges.onsets, if choice == NoteChase::On { 2 } else { 1 });
                    }
                    events.extend(
                        edges
                            .events()
                            .map(|e| (e.frame, e.kind, e.note, e.iteration)),
                    );
                    instrument
                        .query_with_origin_and_note_chase(
                            origin,
                            &timing,
                            (start, end),
                            &mut spans,
                            choice,
                        )
                        .unwrap();
                    gates
                        .query_with_origin_and_note_chase(
                            origin,
                            &timing,
                            (start, end),
                            &mut gate_spans,
                            choice,
                        )
                        .unwrap();
                    let identity = |s: &crate::MidiRenderSpan| {
                        (s.note, s.iteration, s.start_frame, s.gate_end_frame)
                    };
                    assert_eq!(
                        spans.spans().map(identity).collect::<Vec<_>>(),
                        gate_spans.spans().map(identity).collect::<Vec<_>>()
                    );
                    for span in spans.spans() {
                        assert!(
                            choice == NoteChase::On || span.note != crossing || span.iteration != 0
                        );
                        saw_repeat |= span.note == crossing && span.iteration == 1;
                    }
                }
                let mut expected = vec![
                    (0, MidiEventKind::NoteOn, boundary, 0),
                    (1_000, MidiEventKind::NoteOff, boundary, 0),
                    (8_000, MidiEventKind::NoteOn, crossing, 1),
                ];
                if choice == NoteChase::On {
                    expected.insert(0, (0, MidiEventKind::NoteOn, crossing, 0));
                    expected.insert(3, (4_000, MidiEventKind::NoteOff, crossing, 0));
                }
                assert_eq!(events, expected, "choice={choice:?}, width={width}");
                assert!(saw_repeat);
            }
        }
        let origin = SessionMidiOrigin::launch(b(0));
        let mut raw = MidiEdgeBuffer::default();
        let mut explicit = MidiEdgeBuffer::default();
        source
            .query_with_origin(origin, &timing, 0, 1, &mut raw)
            .unwrap();
        source
            .query_with_origin_and_note_chase(origin, &timing, (0, 1), &mut explicit, NoteChase::On)
            .unwrap();
        let identity =
            |e: &crate::midi_compile::CompiledMidiEvent| (e.frame, e.kind, e.note, e.iteration);
        assert_eq!(
            raw.events().map(identity).collect::<Vec<_>>(),
            explicit.events().map(identity).collect::<Vec<_>>()
        );
        assert_eq!(
            source.max_query_onsets(),
            2,
            "density admission remains conservative"
        );
        for choice in [NoteChase::On, NoteChase::Off] {
            assert_eq!(
                source.query_with_origin_and_note_chase(
                    origin,
                    &timing,
                    (0, 257),
                    &mut explicit,
                    choice
                ),
                Err(MidiTemplateError::QueryTooWide)
            );
            assert_eq!(explicit.events().count(), 0);
            let mut spans = MidiSpanBuffer::default();
            instrument
                .query_with_origin(origin, &timing, 0, 1, &mut spans)
                .unwrap();
            assert_eq!(
                instrument.query_with_origin_and_note_chase(
                    origin,
                    &timing,
                    (0, 257),
                    &mut spans,
                    choice
                ),
                Err(MidiTemplateError::QueryTooWide)
            );
            assert_eq!(spans.spans().count(), 0);
        }
    }

    #[test]
    fn analytic_pcm_matches_removed_intro_oracle_and_keeps_cue_and_handoff_floors() {
        let (mut source, timing, crossing, boundary) = source_fixture();
        let table = hex_devices::SineTable::new();
        let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
        // Remove this specific initial occurrence, retaining the crossing note's
        // authored repeats. The oracle does not inspect the clipping metadata.
        source.intro.retain(|e| e.note != crossing);
        source.intro_spans.retain(|s| s.note != crossing);
        let oracle = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
        let render = |template: &PreparedMidiInstrumentTemplate, choice, widths: &[usize]| {
            let mut output = vec![[0.0; 2]; 8_256];
            let mut spans = MidiSpanBuffer::default();
            let mut start = 0;
            let mut block = 0;
            while start < output.len() {
                let end = (start + widths[block % widths.len()]).min(output.len());
                template
                    .render_additive(
                        MidiInstrumentRenderContext::new(b(0), &timing, &table)
                            .with_note_chase(choice),
                        PreparedTrackMix::identity(),
                        start as u64,
                        &mut output[start..end],
                        &mut spans,
                    )
                    .unwrap();
                start = end;
                block += 1;
            }
            output
        };
        let expected = render(&oracle, NoteChase::On, &[256]);
        assert!(
            expected[..1_000].iter().flatten().any(|s| *s != 0.0),
            "genuine boundary note remains audible"
        );
        assert!(
            expected[8_000..].iter().flatten().any(|s| *s != 0.0),
            "later repeat remains audible"
        );
        for widths in [&[1, 7, 193][..], &[256][..]] {
            assert_eq!(render(&instrument, NoteChase::Off, widths), expected);
        }
        assert_ne!(render(&instrument, NoteChase::On, &[256]), expected);
        let mut spans = MidiSpanBuffer::default();
        for choice in [NoteChase::On, NoteChase::Off] {
            let origin = SessionMidiOrigin::new(b(0), b(2));
            instrument
                .query_with_origin_and_note_chase(origin, &timing, (0, 1), &mut spans, choice)
                .unwrap();
            assert_eq!(
                spans
                    .spans()
                    .map(|s| (s.note, s.iteration))
                    .collect::<Vec<_>>(),
                vec![(crossing, 1)],
                "cue floor includes equality and excludes both old intro gates"
            );
            instrument
                .query_after_onset_with_origin_and_note_chase(
                    SessionMidiOrigin::launch(b(0)),
                    &timing,
                    (0, 1),
                    &mut spans,
                    b(1),
                    choice,
                )
                .unwrap();
            assert_eq!(
                spans.spans().count(),
                0,
                "historical floor remains independent"
            );
        }
        instrument
            .query_with_origin(SessionMidiOrigin::launch(b(0)), &timing, 0, 1, &mut spans)
            .unwrap();
        assert_eq!(
            spans.spans().map(|s| s.note).collect::<Vec<_>>(),
            vec![crossing, boundary],
            "raw queries retain default on"
        );
    }
}

#[test]
#[ignore = "optimized timing evidence; run with --release and --ignored"]
fn profile_default_source_window_queries() -> Result<(), &'static str> {
    use std::{hint::black_box, time::Instant};
    if cfg!(debug_assertions) {
        return Err("use --release");
    }
    let (editor, track, clip, _) = fixture(true);
    let timing = PreparedTempoMap::prepare(editor.project().tempo_map().clone(), 8_000).unwrap();
    let source = PreparedMidiSourceTemplate::prepare_source_window(
        track,
        clip,
        editor.project().midi_clip(clip).unwrap().source(),
        b(3),
        &timing,
    )
    .unwrap();
    let instrument =
        PreparedMidiInstrumentTemplate::prepare(&source, &hex_devices::SineTable::new()).unwrap();
    let gates = instrument.capture_gate_template().unwrap();
    let origin = SessionMidiOrigin::launch(b(0));
    let mut edges = MidiEdgeBuffer::default();
    let mut spans = MidiSpanBuffer::default();
    for sample in 0..4 {
        let start = Instant::now();
        for iteration in 0..20_000 {
            let frame = [0, 3_800, 7_800, 8_000][iteration % 4];
            source
                .query_with_origin(origin, black_box(&timing), frame, frame + 256, &mut edges)
                .unwrap();
            black_box(edges.events().count());
            instrument
                .query_with_origin(origin, black_box(&timing), frame, frame + 256, &mut spans)
                .unwrap();
            black_box(spans.spans().count());
            gates
                .query_with_origin(origin, black_box(&timing), frame, frame + 256, &mut spans)
                .unwrap();
            black_box(spans.spans().count());
        }
        if sample != 0 {
            eprintln!(
                "default source-window query sample={sample} rate=8000 tempo=120 block=256 queries-per-sample=60000 warmup-samples=1 elapsed={:?}; prepared scratch, intro and repeat; headless query timing only",
                start.elapsed()
            );
        }
    }
    Ok(())
}
