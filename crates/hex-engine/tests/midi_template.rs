use hex_engine::{
    MidiEdgeBuffer, MidiInstrumentRenderContext, MidiSpanBuffer, MidiTemplateError, MidiTiming,
    PreparedMidiInstrumentTemplate, PreparedMidiSourceTemplate, PreparedTempoError,
    PreparedTempoMap, SessionMidiOrigin,
    midi_compile::{MidiEventKind, midi_events},
};
use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, MidiClip, Project, Tempo, TempoMap, TrackId,
};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}

fn tempo(micro_bpm: u32) -> Tempo {
    Tempo::new(micro_bpm).unwrap()
}

struct Fixture {
    editor: Editor,
    track: TrackId,
    clip: ClipId,
}

fn fixture(
    sample_rate: u32,
    arrangement_start: BeatTime,
    arrangement_length: BeatTime,
    source_length: BeatTime,
    source_start: BeatTime,
    loop_range: Option<(BeatTime, BeatTime)>,
) -> Fixture {
    let mut editor = Editor::new(Project::new(sample_rate).unwrap(), 32).unwrap();
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected instrument track"),
    };
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: arrangement_start,
            length: arrangement_length,
        })
        .unwrap()
    {
        Created::Clip(clip) => clip,
        _ => panic!("expected midi clip"),
    };
    let (loop_start, loop_end) = loop_range.unwrap_or((BeatTime::ZERO, source_length));
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            length: arrangement_length,
            source_length,
            source_start,
            loop_start,
            loop_end,
            looping: loop_range.is_some(),
        })
        .unwrap();
    Fixture {
        editor,
        track,
        clip,
    }
}

fn add_note(fixture: &mut Fixture, start: BeatTime, duration: BeatTime, pitch: u8) {
    fixture
        .editor
        .execute(Command::AddMidiNote {
            clip: fixture.clip,
            start,
            duration,
            pitch,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
}

fn prepare(fixture: &Fixture) -> (PreparedMidiSourceTemplate, PreparedTempoMap) {
    let project = fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip: &MidiClip = project.midi_clip(fixture.clip).unwrap();
    (
        PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap(),
        timing,
    )
}

fn query(
    template: &PreparedMidiSourceTemplate,
    timing: &PreparedTempoMap,
    launch: BeatTime,
    start: u64,
    end: u64,
) -> Vec<hex_engine::midi_compile::CompiledMidiEvent> {
    let mut buffer = MidiEdgeBuffer::default();
    template
        .query(launch, timing, start, end, &mut buffer)
        .unwrap();
    let events: Vec<_> = buffer.events().copied().collect();
    assert!(events.windows(2).all(|pair| {
        (
            pair[0].frame,
            if pair[0].kind == MidiEventKind::NoteOff {
                0
            } else {
                1
            },
            pair[0].track.value(),
            pair[0].clip.value(),
            pair[0].note.value(),
            pair[0].iteration,
        ) <= (
            pair[1].frame,
            if pair[1].kind == MidiEventKind::NoteOff {
                0
            } else {
                1
            },
            pair[1].track.value(),
            pair[1].clip.value(),
            pair[1].note.value(),
            pair[1].iteration,
        )
    }));
    events
}

#[test]
fn ongoing_source_query_matches_finite_projection_across_partitions_and_seeks() {
    let mut fixture = fixture(
        48_000,
        beat(3),
        beat(40),
        beat(8),
        beat(2),
        Some((beat(1), beat(4))),
    );
    fixture
        .editor
        .execute(Command::SetTempoPoint {
            beat: beat(5),
            tempo: tempo(60_000_000),
        })
        .unwrap();
    add_note(&mut fixture, beat(0), beat(1), 48); // hidden before source and loop
    add_note(&mut fixture, beat(1), beat(1), 52); // repeat only; off meets next onset
    add_note(&mut fixture, beat(2), beat(3), 55); // intro and repeat, gate cut at seam
    add_note(&mut fixture, beat(3), beat(1), 59); // intro/repeat coincident with pitch 55 off
    let (template, timing) = prepare(&fixture);
    let launch = beat(3);
    let expected: Vec<_> = midi_events(fixture.editor.project())
        .unwrap()
        .into_iter()
        .filter(|event| event.frame < timing.beat_to_frame(beat(23)).unwrap())
        .collect();
    let end_frame = timing.beat_to_frame(beat(23)).unwrap();
    let mut actual = Vec::new();
    for start in (0..end_frame).step_by(256) {
        actual.extend(query(
            &template,
            &timing,
            launch,
            start,
            (start + 256).min(end_frame),
        ));
    }
    actual.sort_by_key(|event| {
        (
            event.frame,
            if event.kind == MidiEventKind::NoteOff {
                0
            } else {
                1
            },
            event.track.value(),
            event.clip.value(),
            event.note.value(),
            event.iteration,
        )
    });
    assert_eq!(actual, expected);

    // Exercise small block partitions around the first intro-to-repeat seam
    // and the tempo change, without reducing the whole multi-minute horizon to
    // one-frame calls.
    let seam_frame = timing.beat_to_frame(beat(5)).unwrap();
    let focused_start = seam_frame.saturating_sub(512);
    let focused_end = seam_frame + 512;
    let focused_expected: Vec<_> = expected
        .iter()
        .copied()
        .filter(|event| event.frame >= focused_start && event.frame < focused_end)
        .collect();
    for partition in [1_u64, 7, 31, 127, 255] {
        let mut partitioned = Vec::new();
        let mut start = focused_start;
        while start < focused_end {
            let end = (start + partition).min(focused_end);
            partitioned.extend(query(&template, &timing, launch, start, end));
            start = end;
        }
        assert_eq!(partitioned, focused_expected, "partition={partition}");
    }

    let late_start = timing.beat_to_frame(beat(17)).unwrap();
    let late_end = timing.beat_to_frame(beat(20)).unwrap();
    let mut forward = Vec::new();
    let mut late_starts: Vec<_> = (late_start..late_end).step_by(256).collect();
    for start in &late_starts {
        forward.extend(query(
            &template,
            &timing,
            launch,
            *start,
            (*start + 256).min(late_end),
        ));
    }
    let mut reverse = Vec::new();
    late_starts.reverse();
    for start in late_starts {
        reverse.extend(query(
            &template,
            &timing,
            launch,
            start,
            (start + 256).min(late_end),
        ));
    }
    forward.sort_by_key(|event| (event.frame, event.iteration, event.note.value()));
    reverse.sort_by_key(|event| (event.frame, event.iteration, event.note.value()));
    assert_eq!(forward, reverse);
    assert!(forward.iter().any(|event| event.iteration > 1));
    assert!(forward.windows(2).all(|pair| {
        pair[0].frame < pair[1].frame
            || (pair[0].frame == pair[1].frame
                && !(pair[0].kind == MidiEventKind::NoteOn
                    && pair[1].kind == MidiEventKind::NoteOff))
    }));
}

#[test]
fn intro_before_loop_start_plays_once_then_loop_source_repeats() {
    let mut fixture = fixture(
        44_100,
        beat(1),
        beat(20),
        beat(8),
        BeatTime::from_ratio(1, 2).unwrap(),
        Some((beat(2), beat(4))),
    );
    add_note(
        &mut fixture,
        BeatTime::from_ratio(3, 4).unwrap(),
        BeatTime::from_ratio(1, 2).unwrap(),
        48,
    ); // intro before the loop start, no repeats
    add_note(&mut fixture, beat(2), beat(1), 52); // intro plus every repeat
    add_note(&mut fixture, beat(3), beat(2), 55); // gate cut at loop seam
    let (template, timing) = prepare(&fixture);
    let launch = beat(1);
    let frame_end = timing.beat_to_frame(beat(12)).unwrap();
    let expected: Vec<_> = midi_events(fixture.editor.project())
        .unwrap()
        .into_iter()
        .filter(|event| event.frame < frame_end)
        .collect();
    let mut actual = Vec::new();
    for start in (0..frame_end).step_by(256) {
        actual.extend(query(
            &template,
            &timing,
            launch,
            start,
            (start + 256).min(frame_end),
        ));
    }
    assert_eq!(actual, expected);
    let intro_note_id = expected
        .iter()
        .find(|event| event.pitch == 48 && event.kind == MidiEventKind::NoteOn)
        .unwrap()
        .note;
    assert_eq!(
        actual
            .iter()
            .filter(|event| event.note == intro_note_id && event.kind == MidiEventKind::NoteOn)
            .count(),
        1
    );

    let project = fixture.editor.project();
    let clip = project.midi_clip(fixture.clip).unwrap();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
    let intro_id = clip
        .notes()
        .iter()
        .find(|note| note.pitch() == 48)
        .unwrap()
        .id();
    let repeat_id = clip
        .notes()
        .iter()
        .find(|note| note.pitch() == 52)
        .unwrap()
        .id();
    let mut spans = MidiSpanBuffer::default();
    let intro_frame = timing
        .beat_to_frame(BeatTime::from_ratio(5, 4).unwrap())
        .unwrap();
    instrument
        .query(launch, &timing, intro_frame, intro_frame + 256, &mut spans)
        .unwrap();
    assert!(
        spans
            .spans()
            .any(|span| span.note == intro_id && span.iteration == 0)
    );

    let first_repeat = timing
        .beat_to_frame(BeatTime::from_ratio(9, 2).unwrap())
        .unwrap();
    instrument
        .query(
            launch,
            &timing,
            first_repeat,
            first_repeat + 256,
            &mut spans,
        )
        .unwrap();
    assert!(
        spans
            .spans()
            .any(|span| span.note == repeat_id && span.iteration == 1)
    );
}

#[test]
fn ongoing_instrument_spans_and_render_match_finite_compiler_with_loop_release_tails() {
    let mut fixture = fixture(
        48_000,
        beat(3),
        beat(32),
        beat(8),
        beat(2),
        Some((beat(1), beat(4))),
    );
    fixture
        .editor
        .execute(Command::SetTempoPoint {
            beat: beat(5),
            tempo: tempo(60_000_000),
        })
        .unwrap();
    add_note(&mut fixture, beat(2), beat(1), 48); // intro only
    add_note(&mut fixture, beat(3), beat(2), 55); // gate clipped at seam
    add_note(&mut fixture, beat(1), beat(1), 60); // repeat only

    let project = fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip = project.midi_clip(fixture.clip).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
    let launch = clip.start();

    let loop_seam = timing.beat_to_frame(beat(5)).unwrap();
    let query_start = loop_seam.saturating_sub(128);
    let query_end = query_start + 256;
    let mut spans = MidiSpanBuffer::default();
    instrument
        .query(launch, &timing, query_start, query_end, &mut spans)
        .unwrap();
    assert!(spans.spans().any(|span| {
        span.iteration == 0 && span.gate_end_frame == loop_seam && span.end_frame > loop_seam
    }));
    assert!(
        spans
            .spans()
            .any(|span| span.iteration == 1 && span.start_frame == loop_seam)
    );

    let mut expected = [[0.0_f32; 2]; 256];
    let compiled =
        hex_engine::compile::project(project, &std::collections::BTreeMap::new()).unwrap();
    let mut renderer = hex_engine::Renderer::new(&compiled);
    renderer.seek(query_start);
    renderer.play();
    renderer.render(&mut expected).unwrap();
    let mut actual = [[0.0_f32; 2]; 256];
    instrument
        .render(
            launch,
            &timing,
            &table,
            query_start,
            &mut actual,
            &mut spans,
        )
        .unwrap();
    assert_eq!(actual, expected);

    let compiled =
        hex_engine::compile::project(project, &std::collections::BTreeMap::new()).unwrap();
    let later_seam = timing.beat_to_frame(beat(8)).unwrap();
    let later_start = later_seam.saturating_sub(128);
    instrument
        .query(launch, &timing, later_start, later_start + 256, &mut spans)
        .unwrap();
    assert!(spans.spans().any(|span| {
        span.iteration == 1 && span.gate_end_frame == later_seam && span.end_frame > later_seam
    }));
    assert!(
        spans
            .spans()
            .any(|span| span.iteration == 2 && span.start_frame == later_seam)
    );
    let mut later_finite = [[0.0_f32; 2]; 256];
    let mut later_renderer = hex_engine::Renderer::new(&compiled);
    later_renderer.seek(later_start);
    later_renderer.play();
    later_renderer.render(&mut later_finite).unwrap();
    let mut later_actual = [[0.0_f32; 2]; 256];
    instrument
        .render(
            launch,
            &timing,
            &table,
            later_start,
            &mut later_actual,
            &mut spans,
        )
        .unwrap();
    assert_eq!(later_actual, later_finite);

    // Compare a longer nonzero-launch render across the tempo seam and loop
    // boundary, then replay it with varied partitions including one-frame
    // queries. The fixture and expected output are allocated once.
    let horizon = 160_000_usize;
    let mut finite = vec![[0.0_f32; 2]; horizon];
    let mut renderer = hex_engine::Renderer::new(&compiled);
    renderer.play();
    renderer.render(&mut finite).unwrap();
    for partition in [1_usize, 31, 256] {
        let mut rendered = vec![[0.0_f32; 2]; horizon];
        let mut cursor = 0;
        while cursor < horizon {
            let end = (cursor + partition).min(horizon);
            instrument
                .render(
                    launch,
                    &timing,
                    &table,
                    cursor as u64,
                    &mut rendered[cursor..end],
                    &mut spans,
                )
                .unwrap();
            cursor = end;
        }
        assert_eq!(rendered, finite, "partition={partition}");
    }

    let seek_start = 145_000_usize;
    let seek_end = 160_000_usize;
    let mut seek_expected = vec![[0.0_f32; 2]; seek_end - seek_start];
    let mut seek_renderer = hex_engine::Renderer::new(&compiled);
    seek_renderer.seek(seek_start as u64);
    seek_renderer.play();
    seek_renderer.render(&mut seek_expected).unwrap();
    let mut seek_actual = vec![[0.0_f32; 2]; seek_end - seek_start];
    let mut cursor = seek_start;
    while cursor < seek_end {
        let end = (cursor + 127).min(seek_end);
        instrument
            .render(
                launch,
                &timing,
                &table,
                cursor as u64,
                &mut seek_actual[cursor - seek_start..end - seek_start],
                &mut spans,
            )
            .unwrap();
        cursor = end;
    }
    assert_eq!(seek_actual, seek_expected);

    let mut tiny = [[7.0_f32; 2]; 257];
    assert_eq!(
        instrument.render(launch, &timing, &table, query_start, &mut tiny, &mut spans),
        Err(MidiTemplateError::QueryTooWide)
    );
    assert_eq!(tiny, [[0.0; 2]; 257]);
    assert!(spans.is_empty());
}

#[test]
fn additive_instrument_mixing_matches_finite_track_with_coefficients_and_master_seam() {
    let mut fixture = fixture(
        48_000,
        beat(1),
        beat(32),
        beat(8),
        BeatTime::ZERO,
        Some((BeatTime::ZERO, beat(4))),
    );
    fixture
        .editor
        .execute(Command::SetTempoPoint {
            beat: beat(2),
            tempo: tempo(90_000_000),
        })
        .unwrap();
    for (start, duration, pitch) in [
        (beat(0), beat(2), 48),
        (beat(1), beat(2), 55),
        (beat(3), beat(1), 64),
    ] {
        add_note(&mut fixture, start, duration, pitch);
    }
    fixture
        .editor
        .execute(Command::Gain {
            track: fixture.track,
            gain: 0.758_499_15,
        })
        .unwrap();
    fixture
        .editor
        .execute(Command::Balance {
            track: fixture.track,
            balance: -0.17,
        })
        .unwrap();
    fixture
        .editor
        .execute(Command::MasterGain { gain: 0.62 })
        .unwrap();

    let project = fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip = project.midi_clip(fixture.clip).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
    let context = MidiInstrumentRenderContext::new(clip.start(), &timing, &table);
    let compiled =
        hex_engine::compile::project(project, &std::collections::BTreeMap::new()).unwrap();
    let track = compiled.tracks().first().unwrap();
    let horizon = 150_000_usize; // crosses the tempo change and first loop seam
    let mut finite_track = vec![[0.0_f32; 2]; horizon];
    track.render_additive(0, &mut finite_track).unwrap();

    for partition in [31_usize, 127, 256] {
        let mut additive = vec![[0.0_f32; 2]; horizon];
        let mut spans = MidiSpanBuffer::default();
        let mut cursor = 0;
        while cursor < horizon {
            let end = (cursor + partition).min(horizon);
            instrument
                .render_additive(
                    context,
                    track.mix(),
                    cursor as u64,
                    &mut additive[cursor..end],
                    &mut spans,
                )
                .unwrap();
            cursor = end;
        }
        assert_eq!(additive, finite_track, "partition={partition}");
    }

    let mut expected_renderer = finite_track.clone();
    compiled.master_mix().apply(&mut expected_renderer);
    let mut renderer = hex_engine::Renderer::new(&compiled);
    renderer.play();
    let mut rendered = vec![[0.0_f32; 2]; horizon];
    renderer.render(&mut rendered).unwrap();
    assert_eq!(rendered, expected_renderer);
    assert_ne!(
        rendered, finite_track,
        "additive source must not apply master"
    );

    // Demonstrate that this fixture detects the old sum-then-scale shortcut:
    // standalone rendering sums overlapping voices before channel gain, while
    // additive rendering must match the finite per-voice order bit for bit.
    let overlap_start = timing.beat_to_frame(beat(2)).unwrap();
    let mut mono_sum = [[0.0_f32; 2]; 256];
    let mut spans = MidiSpanBuffer::default();
    instrument
        .render(
            clip.start(),
            &timing,
            &table,
            overlap_start,
            &mut mono_sum,
            &mut spans,
        )
        .unwrap();
    assert!(
        spans.len() >= 2,
        "rounding fixture needs overlapping voices"
    );
    let gains = track.mix().channel_gain();
    let sum_then_scale_differs = mono_sum.iter().enumerate().any(|(offset, mono)| {
        let finite = finite_track[overlap_start as usize + offset];
        mono[0] * gains[0] != finite[0] || mono[0] * gains[1] != finite[1]
    });
    assert!(sum_then_scale_differs);

    // Nonzero destinations remain additive, using the same per-voice channel
    // multiplication order as the finite track path.
    let initial = [0.125_f32, -0.25_f32];
    let mut expected_additive = vec![initial; 256];
    track
        .render_additive(143_900, &mut expected_additive)
        .unwrap();
    let mut actual_additive = vec![initial; 256];
    let mut spans = MidiSpanBuffer::default();
    instrument
        .render_additive(
            context,
            track.mix(),
            143_900,
            &mut actual_additive,
            &mut spans,
        )
        .unwrap();
    assert_eq!(actual_additive, expected_additive);
}

#[test]
fn prepared_track_coefficients_are_applied_per_voice_before_summing() {
    let mix = hex_engine::PreparedTrackMix::new(0.511_274_7, 0.0, false).unwrap();
    let voices = [0.281_715_42_f32, -0.242_831_48_f32];
    let mut per_voice = [0.0_f32; 2];
    for sample in voices {
        mix.add_frame(&mut per_voice, [sample; 2]);
    }
    let sum_then_scale = (voices[0] + voices[1]) * mix.channel_gain()[0];
    assert_eq!(per_voice[0], 0.019_880_377);
    assert_ne!(per_voice[0], sum_then_scale);
}

#[test]
fn additive_instrument_validates_muted_queries_and_preserves_destination_on_errors() {
    let mut fixture = fixture(
        48_000,
        BeatTime::ZERO,
        beat(16),
        beat(8),
        BeatTime::ZERO,
        None,
    );
    add_note(&mut fixture, BeatTime::ZERO, beat(1), 60);
    let project = fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip = project.midi_clip(fixture.clip).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
    let context = MidiInstrumentRenderContext::new(clip.start(), &timing, &table);
    let mut spans = MidiSpanBuffer::default();

    let mut muted_output = [[0.125_f32, -0.25_f32]; 257];
    let before = muted_output;
    let muted = hex_engine::PreparedTrackMix::new(0.5, 0.0, true).unwrap();
    instrument
        .render_additive(context, muted, 0, &mut muted_output[..32], &mut spans)
        .unwrap();
    assert_eq!(muted_output, before);
    assert!(!spans.is_empty(), "muting must not skip a valid query");

    let compiled =
        hex_engine::compile::project(project, &std::collections::BTreeMap::new()).unwrap();
    let track = compiled.tracks().first().unwrap();
    let active_mix = track.mix();
    let mut finite = [[0.0_f32; 2]; 32];
    track.render_additive(0, &mut finite).unwrap();
    let mut additive = [[0.0_f32; 2]; 32];
    instrument
        .render_additive(context, active_mix, 0, &mut additive, &mut spans)
        .unwrap();
    assert_eq!(additive, finite, "one-shot source path");

    let fast_timing = PreparedTempoMap::prepare(
        TempoMap::constant(48_000, tempo(120_000_001)).unwrap(),
        48_000,
    )
    .unwrap();
    let fast_context = MidiInstrumentRenderContext::new(clip.start(), &fast_timing, &table);
    assert_eq!(
        instrument.render_additive(context, muted, 0, &mut muted_output[..32], &mut spans),
        Ok(())
    );
    assert_eq!(
        instrument.render_additive(fast_context, muted, 0, &mut muted_output[..32], &mut spans),
        Err(MidiTemplateError::TempoMapTooFast)
    );
    assert_eq!(muted_output, before);
    assert!(spans.is_empty());

    let wrong_timing = PreparedTempoMap::prepare(
        TempoMap::constant(44_100, tempo(120_000_000)).unwrap(),
        44_100,
    )
    .unwrap();
    let wrong_context = MidiInstrumentRenderContext::new(clip.start(), &wrong_timing, &table);
    instrument
        .render_additive(context, muted, 0, &mut muted_output[..32], &mut spans)
        .unwrap();
    assert!(!spans.is_empty());
    assert_eq!(
        instrument.render_additive(wrong_context, muted, 0, &mut muted_output[..32], &mut spans),
        Err(MidiTemplateError::TempoRateMismatch)
    );
    assert_eq!(muted_output, before);
    assert!(spans.is_empty());

    instrument
        .render_additive(context, muted, 0, &mut muted_output[..32], &mut spans)
        .unwrap();
    assert!(!spans.is_empty());
    assert_eq!(
        instrument.render_additive(context, muted, 0, &mut muted_output[..257], &mut spans),
        Err(MidiTemplateError::QueryTooWide)
    );
    assert_eq!(muted_output, before);
    assert!(spans.is_empty());
    instrument
        .render_additive(context, muted, 0, &mut muted_output[..32], &mut spans)
        .unwrap();
    assert!(!spans.is_empty());
    assert_eq!(
        instrument.render_additive(context, muted, u64::MAX, &mut muted_output[..2], &mut spans,),
        Err(MidiTemplateError::ArithmeticOverflow)
    );
    assert_eq!(muted_output, before);
    assert!(spans.is_empty());
}

#[test]
fn ongoing_instrument_admits_repeating_32_note_chord_and_matches_finite_pcm() {
    let mut fixture = fixture(
        48_000,
        BeatTime::ZERO,
        beat(12),
        beat(4),
        BeatTime::ZERO,
        Some((BeatTime::ZERO, beat(4))),
    );
    for index in 0..32 {
        add_note(&mut fixture, BeatTime::ZERO, beat(4), 48 + index);
    }

    let project = fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip = project.midi_clip(fixture.clip).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table)
        .expect("a 32-note chord has 64 voices at its first seam");
    let compiled =
        hex_engine::compile::project(project, &std::collections::BTreeMap::new()).unwrap();
    let horizon = 9 * 24_000_usize;
    let mut expected = vec![[0.0_f32; 2]; horizon];
    let mut renderer = hex_engine::Renderer::new(&compiled);
    renderer.play();
    renderer.render(&mut expected).unwrap();

    let mut spans = MidiSpanBuffer::default();
    for partition in [31_usize, 256] {
        let mut actual = vec![[0.0_f32; 2]; horizon];
        let mut cursor = 0;
        while cursor < horizon {
            let end = (cursor + partition).min(horizon);
            instrument
                .render(
                    clip.start(),
                    &timing,
                    &table,
                    cursor as u64,
                    &mut actual[cursor..end],
                    &mut spans,
                )
                .unwrap();
            cursor = end;
        }
        assert_eq!(actual, expected, "partition={partition}");
    }

    // Query one-frame slices around both the first and a later repeat seam.
    for seam_beat in [4_u64, 8] {
        let seam = timing.beat_to_frame(beat(seam_beat)).unwrap();
        instrument
            .query(clip.start(), &timing, seam, seam + 256, &mut spans)
            .unwrap();
        assert_eq!(spans.len(), 64, "voices at beat {seam_beat}");
        for frame in seam.saturating_sub(32)..seam + 32 {
            let mut actual = [[0.0_f32; 2]; 1];
            instrument
                .render(
                    clip.start(),
                    &timing,
                    &table,
                    frame,
                    &mut actual,
                    &mut spans,
                )
                .unwrap();
            assert_eq!(actual[0], expected[frame as usize], "frame={frame}");
        }
    }
}

#[test]
fn ongoing_instrument_keeps_distinct_onsets_that_share_a_rounded_frame() {
    let mut quantized_fixture = fixture(
        8_000,
        BeatTime::ZERO,
        beat(2),
        beat(2),
        BeatTime::ZERO,
        None,
    );
    quantized_fixture
        .editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: tempo(937_500_000),
        })
        .unwrap();
    let first_raw = 1_u64 << 22;
    add_note(
        &mut quantized_fixture,
        BeatTime::from_raw(first_raw),
        beat(1),
        60,
    );
    add_note(
        &mut quantized_fixture,
        BeatTime::from_raw(first_raw + 1),
        beat(1),
        62,
    );
    let project = quantized_fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip = project.midi_clip(quantized_fixture.clip).unwrap();
    let source =
        PreparedMidiSourceTemplate::prepare(quantized_fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
    let bucket_end = timing.beat_lower_bound_for_frame(2).unwrap();
    let latest_raw = u64::try_from(bucket_end - 1).unwrap();
    assert_eq!(
        timing
            .beat_to_frame(BeatTime::from_raw(latest_raw))
            .unwrap(),
        1
    );
    assert!(u128::from(first_raw + 1) <= u128::from(latest_raw));

    let mut spans = MidiSpanBuffer::default();
    instrument
        .query(BeatTime::ZERO, &timing, 1, 2, &mut spans)
        .unwrap();
    let onsets: Vec<_> = spans.spans().collect();
    assert_eq!(onsets.len(), 2);
    assert_eq!(onsets[0].start_frame, 1);
    assert_eq!(onsets[1].start_frame, 1);
    assert_ne!(onsets[0].note, onsets[1].note);

    let other_rate = fixture(
        44_100,
        BeatTime::ZERO,
        beat(2),
        beat(2),
        BeatTime::ZERO,
        None,
    );
    let other_project = other_rate.editor.project();
    let other_timing = PreparedTempoMap::prepare(
        other_project.tempo_map().clone(),
        other_project.sample_rate(),
    )
    .unwrap();
    assert_eq!(
        instrument.query(BeatTime::ZERO, &other_timing, 1, 2, &mut spans),
        Err(MidiTemplateError::TempoRateMismatch)
    );
    assert!(spans.is_empty());

    let mut faster = fixture(
        8_000,
        BeatTime::ZERO,
        beat(2),
        beat(2),
        BeatTime::ZERO,
        None,
    );
    faster
        .editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: tempo(1_000_000_000),
        })
        .unwrap();
    let faster_project = faster.editor.project();
    let faster_timing = PreparedTempoMap::prepare(
        faster_project.tempo_map().clone(),
        faster_project.sample_rate(),
    )
    .unwrap();
    assert_eq!(
        instrument.query(BeatTime::ZERO, &faster_timing, 1, 2, &mut spans),
        Err(MidiTemplateError::TempoMapTooFast)
    );
    assert!(spans.is_empty());
}

#[test]
fn ongoing_instrument_preparation_reuses_nyquist_validation() {
    let mut fixture = fixture(
        8_000,
        BeatTime::ZERO,
        beat(2),
        beat(2),
        BeatTime::ZERO,
        None,
    );
    add_note(&mut fixture, BeatTime::ZERO, beat(1), 127);
    let project = fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip = project.midi_clip(fixture.clip).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    assert!(matches!(
        PreparedMidiInstrumentTemplate::prepare(&source, &table),
        Err(MidiTemplateError::InvalidSource(_))
    ));
}

#[test]
fn ongoing_instrument_ignores_unplayed_nyquist_notes_before_source_start() {
    let mut fixture = fixture(8_000, BeatTime::ZERO, beat(2), beat(2), beat(1), None);
    add_note(
        &mut fixture,
        BeatTime::from_ratio(1, 2).unwrap(),
        BeatTime::from_ratio(1, 4).unwrap(),
        127,
    );
    let project = fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip = project.midi_clip(fixture.clip).unwrap();
    assert_eq!(clip.notes().len(), 1);
    let source = PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
    let mut spans = MidiSpanBuffer::default();
    instrument
        .query(clip.start(), &timing, 0, 256, &mut spans)
        .unwrap();
    assert!(spans.is_empty());
}

#[test]
fn ongoing_instrument_admission_rejects_voice_pileups() {
    let mut fixture = fixture(
        48_000,
        BeatTime::ZERO,
        beat(4),
        beat(4),
        BeatTime::ZERO,
        None,
    );
    for index in 0..65 {
        add_note(&mut fixture, beat(0), beat(1), 36 + index);
    }
    let project = fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip = project.midi_clip(fixture.clip).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    assert_eq!(
        PreparedMidiInstrumentTemplate::prepare(&source, &table).err(),
        Some(MidiTemplateError::TooManyVoices)
    );
}

#[test]
fn ongoing_instrument_admission_counts_repeat_voices_that_start_during_intro_release() {
    let mut fixture = fixture(
        48_000,
        BeatTime::ZERO,
        beat(8),
        beat(4),
        BeatTime::ZERO,
        Some((BeatTime::ZERO, beat(3))),
    );
    for index in 0..63 {
        add_note(
            &mut fixture,
            BeatTime::from_ratio(29, 10).unwrap(),
            BeatTime::from_ratio(1, 10).unwrap(),
            36 + index,
        );
    }
    for index in 0..64 {
        add_note(
            &mut fixture,
            BeatTime::from_ratio(1, 20).unwrap(),
            BeatTime::from_ratio(1, 100).unwrap(),
            36 + index,
        );
    }
    let project = fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip = project.midi_clip(fixture.clip).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    assert_eq!(
        PreparedMidiInstrumentTemplate::prepare(&source, &table).err(),
        Some(MidiTemplateError::TooManyVoices)
    );
}

#[test]
fn ongoing_instrument_interval_index_keeps_earlier_right_subtree_onsets() {
    let mut fixture = fixture(
        48_000,
        BeatTime::ZERO,
        beat(8),
        beat(8),
        BeatTime::ZERO,
        None,
    );
    for index in 0..7_u64 {
        add_note(
            &mut fixture,
            BeatTime::from_ratio(index, 16).unwrap(),
            BeatTime::from_ratio(1, 32).unwrap(),
            48 + index as u8,
        );
    }
    let project = fixture.editor.project();
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let clip = project.midi_clip(fixture.clip).unwrap();
    let target = clip.notes().iter().find(|note| note.pitch() == 52).unwrap();
    let target_frame = timing.beat_to_frame(target.start()).unwrap();
    let source = PreparedMidiSourceTemplate::prepare(fixture.track, clip, &timing).unwrap();
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
    let mut spans = MidiSpanBuffer::default();
    instrument
        .query(
            BeatTime::ZERO,
            &timing,
            target_frame,
            target_frame + 256,
            &mut spans,
        )
        .unwrap();
    assert!(spans.spans().any(|span| span.note == target.id()));
}

#[test]
fn repeat_voice_density_rejects_short_periods_and_empty_repeats_skip_iteration_work() {
    let short_period = BeatTime::from_ratio(1, 4_000).unwrap();
    let mut active = fixture(
        48_000,
        BeatTime::ZERO,
        short_period,
        short_period,
        BeatTime::ZERO,
        Some((BeatTime::ZERO, short_period)),
    );
    add_note(&mut active, BeatTime::ZERO, short_period, 48);
    let active_project = active.editor.project();
    let active_timing = PreparedTempoMap::prepare(
        active_project.tempo_map().clone(),
        active_project.sample_rate(),
    )
    .unwrap();
    let active_clip = active_project.midi_clip(active.clip).unwrap();
    let active_source =
        PreparedMidiSourceTemplate::prepare(active.track, active_clip, &active_timing).unwrap();
    let table = hex_devices::SineTable::new();
    assert_eq!(
        PreparedMidiInstrumentTemplate::prepare(&active_source, &table).err(),
        Some(MidiTemplateError::TooManyVoices)
    );

    let empty = fixture(
        48_000,
        BeatTime::ZERO,
        short_period,
        short_period,
        BeatTime::ZERO,
        Some((BeatTime::ZERO, short_period)),
    );
    let empty_project = empty.editor.project();
    let empty_timing = PreparedTempoMap::prepare(
        empty_project.tempo_map().clone(),
        empty_project.sample_rate(),
    )
    .unwrap();
    let empty_clip = empty_project.midi_clip(empty.clip).unwrap();
    let empty_source =
        PreparedMidiSourceTemplate::prepare(empty.track, empty_clip, &empty_timing).unwrap();
    assert!(PreparedMidiInstrumentTemplate::prepare(&empty_source, &table).is_ok());
}

#[test]
fn quantized_distinct_onsets_survive_and_each_buffer_keeps_off_before_on() {
    let mut fixture = fixture(
        8_000,
        BeatTime::ZERO,
        beat(2),
        beat(2),
        BeatTime::ZERO,
        None,
    );
    fixture
        .editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: tempo(937_500_000),
        })
        .unwrap();
    let first = 1_u64 << 22;
    add_note(&mut fixture, BeatTime::from_raw(first), beat(1), 60);
    add_note(&mut fixture, BeatTime::from_raw(first + 1), beat(1), 62);
    let (template, timing) = prepare(&fixture);
    let events = query(&template, &timing, BeatTime::ZERO, 1, 2);
    let onsets: Vec<_> = events
        .iter()
        .filter(|event| event.kind == MidiEventKind::NoteOn)
        .collect();
    assert_eq!(onsets.len(), 2);
    assert_eq!(onsets[0].frame, onsets[1].frame);
    assert_ne!(onsets[0].note, onsets[1].note);
}

#[test]
fn ongoing_template_ignores_arrangement_duration_but_one_shot_stops_at_source_end() {
    let mut fixture = fixture(
        48_000,
        BeatTime::ZERO,
        beat(1),
        beat(4),
        BeatTime::ZERO,
        None,
    );
    add_note(
        &mut fixture,
        beat(2),
        BeatTime::from_ratio(1, 256).unwrap(),
        64,
    );
    let (template, timing) = prepare(&fixture);
    let start = timing.beat_to_frame(beat(2)).unwrap();
    let events = query(&template, &timing, BeatTime::ZERO, start, start + 256);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, MidiEventKind::NoteOn);
    assert_eq!(events[1].kind, MidiEventKind::NoteOff);
    assert_eq!(events[0].frame, start);

    let end_of_source = timing.beat_to_frame(beat(4)).unwrap();
    assert!(
        query(
            &template,
            &timing,
            BeatTime::ZERO,
            end_of_source,
            end_of_source + 100
        )
        .is_empty()
    );
}

#[test]
fn session_owned_source_prepares_without_arrangement_placement() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "keys".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "session keys".into(),
            source_length: beat(4),
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("session clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(1),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let project = editor.project();
    assert!(project.midi_clip(clip).is_none());
    let timing =
        PreparedTempoMap::prepare(project.tempo_map().clone(), project.sample_rate()).unwrap();
    let source = PreparedMidiSourceTemplate::prepare_source(
        track,
        clip,
        project.midi_source(clip).unwrap(),
        &timing,
    )
    .unwrap();
    assert_eq!(source.clip_id(), clip);
    assert!(!query(&source, &timing, BeatTime::ZERO, 24_000, 24_001).is_empty());
}

#[test]
fn query_refusals_reset_visible_length_and_maps_must_match_admission() {
    let mut fixture = fixture(
        48_000,
        BeatTime::ZERO,
        beat(16),
        beat(8),
        BeatTime::ZERO,
        Some((BeatTime::ZERO, beat(8))),
    );
    add_note(&mut fixture, beat(1), beat(1), 60);
    let (template, timing) = prepare(&fixture);
    let mut buffer = MidiEdgeBuffer::default();
    template
        .query(BeatTime::ZERO, &timing, 24_000, 24_256, &mut buffer)
        .unwrap();
    assert!(!buffer.is_empty());

    assert_eq!(
        template.query(BeatTime::ZERO, &timing, 0, 257, &mut buffer),
        Err(MidiTemplateError::QueryTooWide)
    );
    assert!(buffer.is_empty());
    assert_eq!(
        template.query(BeatTime::ZERO, &timing, 10, 9, &mut buffer),
        Err(MidiTemplateError::InvalidFrameRange)
    );
    assert!(buffer.is_empty());

    let faster = PreparedTempoMap::prepare(
        TempoMap::constant(48_000, tempo(120_000_001)).unwrap(),
        48_000,
    )
    .unwrap();
    assert_eq!(
        template.query(BeatTime::ZERO, &faster, 0, 256, &mut buffer),
        Err(MidiTemplateError::TempoMapTooFast)
    );
    let wrong_rate = PreparedTempoMap::prepare(
        TempoMap::constant(44_100, tempo(120_000_000)).unwrap(),
        44_100,
    )
    .unwrap();
    assert_eq!(
        template.query(BeatTime::ZERO, &wrong_rate, 0, 256, &mut buffer),
        Err(MidiTemplateError::TempoRateMismatch)
    );
    assert!(buffer.is_empty());
}

#[test]
fn beat_domain_boundary_query_keeps_last_edges_and_reports_exhaustion() {
    let mut fixture = fixture(
        48_000,
        BeatTime::ZERO,
        beat(1),
        beat(1),
        BeatTime::ZERO,
        Some((BeatTime::ZERO, beat(1))),
    );
    add_note(
        &mut fixture,
        BeatTime::ZERO,
        BeatTime::from_ratio(1, 4).unwrap(),
        60,
    );
    let (template, timing) = prepare(&fixture);
    let final_aligned_beat = (u64::MAX / BeatTime::ONE.raw()) * BeatTime::ONE.raw();
    let frame = timing
        .beat_to_frame(BeatTime::from_raw(final_aligned_beat))
        .unwrap();
    let mut buffer = MidiEdgeBuffer::default();
    template
        .query(BeatTime::ZERO, &timing, frame, frame + 1, &mut buffer)
        .unwrap();
    assert!(
        buffer
            .events()
            .any(|event| { event.kind == MidiEventKind::NoteOn && event.iteration > 1 })
    );

    let last_frame = timing.beat_to_frame(BeatTime::from_raw(u64::MAX)).unwrap();
    assert_eq!(
        template.query(
            BeatTime::ZERO,
            &timing,
            last_frame + 1,
            last_frame + 2,
            &mut buffer,
        ),
        Err(MidiTemplateError::BeatDomainExhausted)
    );
    assert!(buffer.is_empty());

    let source = PreparedMidiSourceTemplate::prepare(
        fixture.track,
        fixture.editor.project().midi_clip(fixture.clip).unwrap(),
        &timing,
    )
    .unwrap();
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
    let mut spans = MidiSpanBuffer::default();
    assert_eq!(
        instrument.query(
            BeatTime::ZERO,
            &timing,
            last_frame + 1,
            last_frame + 2,
            &mut spans,
        ),
        Err(MidiTemplateError::BeatDomainExhausted)
    );
    assert!(spans.is_empty());
}

#[test]
fn density_refuses_clustered_edges_and_admits_dense_spread_out_notes() {
    let mut clustered = fixture(
        48_000,
        BeatTime::ZERO,
        beat(4),
        beat(4),
        BeatTime::ZERO,
        None,
    );
    for pitch in 0..129 {
        // The repeated pitch has an adjacent gate; all 129 onsets still fall
        // within 24 frames, so this tests onset pressure rather than overwrite.
        add_note(
            &mut clustered,
            BeatTime::from_ratio(pitch / 128, 1024).unwrap(),
            if pitch == 0 {
                BeatTime::from_ratio(1, 1024).unwrap()
            } else {
                beat(1)
            },
            (pitch % 128) as u8,
        );
    }
    let project = clustered.editor.project();
    let timing = PreparedTempoMap::prepare(project.tempo_map().clone(), 48_000).unwrap();
    assert!(matches!(
        PreparedMidiSourceTemplate::prepare(
            clustered.track,
            project.midi_clip(clustered.clip).unwrap(),
            &timing
        ),
        Err(MidiTemplateError::TooManyOnsets)
    ));

    let mut spread = fixture(
        48_000,
        BeatTime::ZERO,
        beat(300),
        beat(300),
        BeatTime::ZERO,
        Some((BeatTime::ZERO, beat(300))),
    );
    for index in 0..129_u64 {
        add_note(&mut spread, beat(index * 2), beat(1), (index % 128) as u8);
    }
    let (template, timing) = prepare(&spread);
    let end = timing.beat_to_frame(beat(256)).unwrap();
    let events = query(&template, &timing, BeatTime::ZERO, 0, end.min(256));
    assert!(events.len() <= 256);
}

#[test]
fn short_playable_gates_and_loop_periods_are_rejected() {
    let mut short_gate = fixture(
        8_000,
        BeatTime::ZERO,
        beat(4),
        beat(4),
        BeatTime::ZERO,
        None,
    );
    add_note(&mut short_gate, BeatTime::ZERO, BeatTime::from_raw(1), 60);
    let project = short_gate.editor.project();
    let timing = PreparedTempoMap::prepare(project.tempo_map().clone(), 8_000).unwrap();
    assert!(matches!(
        PreparedMidiSourceTemplate::prepare(
            short_gate.track,
            project.midi_clip(short_gate.clip).unwrap(),
            &timing
        ),
        Err(MidiTemplateError::GateTooShortForAdmission)
    ));

    let short_loop = fixture(
        8_000,
        BeatTime::ZERO,
        beat(1),
        BeatTime::from_raw(1 << 22),
        BeatTime::ZERO,
        Some((BeatTime::ZERO, BeatTime::from_raw(1 << 10))),
    );
    let project = short_loop.editor.project();
    let timing = PreparedTempoMap::prepare(project.tempo_map().clone(), 8_000).unwrap();
    assert!(matches!(
        PreparedMidiSourceTemplate::prepare(
            short_loop.track,
            project.midi_clip(short_loop.clip).unwrap(),
            &timing
        ),
        Err(MidiTemplateError::LoopPeriodTooShortForAdmission)
    ));
}

#[test]
fn origin_cue_reuses_canonical_intro_loop_and_unreachable_source_projection() {
    let mut f = fixture(
        8_000,
        BeatTime::ZERO,
        beat(12),
        beat(6),
        beat(3),
        Some((beat(1), beat(4))),
    );
    add_note(
        &mut f,
        BeatTime::from_ratio(3, 2).unwrap(),
        BeatTime::from_ratio(1, 4).unwrap(),
        64,
    );
    add_note(&mut f, beat(3), BeatTime::from_ratio(1, 4).unwrap(), 67);
    let (source, timing) = prepare(&f);
    for (point, elapsed) in [
        (beat(3), Some(BeatTime::ZERO)),
        (
            BeatTime::from_ratio(7, 2).unwrap(),
            Some(BeatTime::from_ratio(1, 2).unwrap()),
        ),
        (
            BeatTime::from_ratio(3, 2).unwrap(),
            Some(BeatTime::from_ratio(3, 2).unwrap()),
        ),
        (BeatTime::ZERO, None),
        (beat(5), None),
    ] {
        assert_eq!(source.earliest_elapsed_for_source(point).unwrap(), elapsed);
    }
    assert!(source.earliest_elapsed_for_source(beat(6)).is_err());
    let origin = SessionMidiOrigin::new(
        BeatTime::from_ratio(1, 2).unwrap(),
        BeatTime::from_ratio(9, 2).unwrap(),
    );
    let start = timing.beat_to_frame(origin.performance_anchor).unwrap();
    let mut edges = MidiEdgeBuffer::default();
    source
        .query_with_origin(origin, &timing, start, start + 256, &mut edges)
        .unwrap();
    let onset = edges
        .events()
        .find(|e| e.kind == MidiEventKind::NoteOn)
        .unwrap();
    assert_eq!((onset.frame, onset.pitch, onset.iteration), (start, 64, 2));
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
    let mut spans = MidiSpanBuffer::default();
    instrument
        .query_with_origin(origin, &timing, start, start + 256, &mut spans)
        .unwrap();
    let onset = spans.spans().find(|s| s.pitch == 64).unwrap();
    assert_eq!(
        (onset.onset_beat, onset.gate_end_beat, onset.iteration),
        (
            origin.performance_anchor,
            BeatTime::from_ratio(3, 4).unwrap(),
            2
        )
    );
}

// Refuse any attempt to convert a historical performance onset. The wrapper
// retains GLOBAL bounds and tempo conversion; it never offsets the timing map.
struct NoHistoryTiming<'a> {
    timing: &'a PreparedTempoMap,
    anchor: BeatTime,
}
impl MidiTiming for NoHistoryTiming<'_> {
    fn sample_rate(&self) -> u32 {
        self.timing.sample_rate()
    }
    fn max_micro_bpm(&self) -> u64 {
        u64::from(self.timing.max_micro_bpm())
    }
    fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, PreparedTempoError> {
        if beat < self.anchor {
            return Err(PreparedTempoError::BeatToFrame(
                "historical onset must be filtered",
            ));
        }
        self.timing.beat_to_frame(beat)
    }
    fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, PreparedTempoError> {
        self.timing.frame_to_beat(frame)
    }
    fn beat_lower_bound_for_frame(&self, frame: u64) -> Result<u128, PreparedTempoError> {
        self.timing.beat_lower_bound_for_frame(frame)
    }
}
#[test]
fn origin_filters_chased_tails_and_orphan_offs_before_global_tempo_conversion_and_matches_literal_pcm()
 {
    let mut f = fixture(
        8_000,
        BeatTime::ZERO,
        beat(16),
        beat(16),
        BeatTime::ZERO,
        None,
    );
    add_note(&mut f, BeatTime::ZERO, beat(12), 60);
    add_note(&mut f, beat(10), beat(1), 64);
    add_note(&mut f, beat(11), BeatTime::from_ratio(1, 2).unwrap(), 67);
    let mut literal = fixture(
        8_000,
        BeatTime::ZERO,
        beat(5),
        beat(5),
        BeatTime::ZERO,
        None,
    );
    add_note(&mut literal, beat(1), beat(1), 64);
    add_note(
        &mut literal,
        beat(2),
        BeatTime::from_ratio(1, 2).unwrap(),
        67,
    );
    for f in [&mut f, &mut literal] {
        for (at, micro_bpm) in [
            (BeatTime::from_ratio(3, 2).unwrap(), 90_000_000),
            (BeatTime::from_ratio(9, 4).unwrap(), 150_000_000),
        ] {
            f.editor
                .execute(Command::SetTempoPoint {
                    beat: at,
                    tempo: tempo(micro_bpm),
                })
                .unwrap();
        }
    }
    let (source, timing) = prepare(&f);
    let origin = SessionMidiOrigin::new(beat(1), beat(10));
    let clock = NoHistoryTiming {
        timing: &timing,
        anchor: origin.performance_anchor,
    };
    let table = hex_devices::SineTable::new();
    let instrument = PreparedMidiInstrumentTemplate::prepare(&source, &table).unwrap();
    let start = timing.beat_to_frame(beat(1)).unwrap();
    let end = timing.beat_to_frame(beat(4)).unwrap();
    let mut edges = MidiEdgeBuffer::default();
    let mut actual = Vec::new();
    let mut frame = start - 128;
    while frame < end {
        let next = (frame + 127).min(end);
        source
            .query_with_origin(origin, &clock, frame, next, &mut edges)
            .unwrap();
        actual.extend(
            edges
                .events()
                .map(|e| (e.frame, e.kind, e.pitch, e.velocity)),
        );
        frame = next;
    }
    let expected: Vec<_> = midi_events(literal.editor.project())
        .unwrap()
        .into_iter()
        .map(|e| (e.frame, e.kind, e.pitch, e.velocity))
        .collect();
    assert_eq!(actual, expected);
    assert!(
        !actual.iter().any(|e| e.2 == 60),
        "no chased onset or orphan NoteOff from the pre-cue gate"
    );
    let mut spans = MidiSpanBuffer::default();
    instrument
        .query_with_origin(origin, &clock, start, start + 1, &mut spans)
        .unwrap();
    let first = spans.spans().next().unwrap();
    assert_eq!(
        (first.pitch, first.onset_beat, first.gate_end_beat),
        (64, beat(1), beat(2))
    );
    let compiled =
        hex_engine::compile::project(literal.editor.project(), &std::collections::BTreeMap::new())
            .unwrap();
    let track = &compiled.tracks()[0];
    let mut expected = vec![[0.0; 2]; end as usize];
    track.render_additive(0, &mut expected).unwrap();
    for partition in [73, 256] {
        let mut output = vec![[0.0; 2]; end as usize];
        let context =
            MidiInstrumentRenderContext::with_origin(origin, &clock, &table).after_onset(beat(1));
        let mut at = 0;
        while at < output.len() {
            let next = (at + partition).min(output.len());
            instrument
                .render_additive(
                    context,
                    track.mix(),
                    at as u64,
                    &mut output[at..next],
                    &mut spans,
                )
                .unwrap();
            at = next;
        }
        assert_eq!(output, expected, "global cue PCM partition={partition}");
    }
    let mut direct = [[0.0; 2]; 256];
    instrument
        .render_with_origin(origin, &clock, &table, start, &mut direct, &mut spans)
        .unwrap();
    assert_eq!(direct, expected[start as usize..start as usize + 256]);
    assert!(direct.iter().flatten().any(|x| x.abs() > 1e-5));
    let mut output = [[1.0; 2]; 257];
    assert_eq!(
        instrument.render_with_origin(origin, &clock, &table, start, &mut output, &mut spans),
        Err(MidiTemplateError::QueryTooWide)
    );
    assert!(spans.is_empty() && output.iter().flatten().all(|x| *x == 0.0));
    let exhausted = SessionMidiOrigin::new(BeatTime::ZERO, BeatTime::from_raw(u64::MAX));
    assert_eq!(
        source.query_with_origin(exhausted, &timing, 0, 1, &mut edges),
        Err(MidiTemplateError::BeatDomainExhausted)
    );
    assert!(edges.is_empty());
    assert_eq!(
        instrument.query_with_origin(origin, &timing, start + 1, start, &mut spans),
        Err(MidiTemplateError::InvalidFrameRange)
    );
    assert!(spans.is_empty());
}
