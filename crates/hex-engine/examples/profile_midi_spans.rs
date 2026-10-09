use std::{hint::black_box, time::Instant};

use hex_engine::{
    MidiInstrumentRenderContext, MidiSpanBuffer, PreparedMidiInstrumentTemplate,
    PreparedMidiSourceTemplate, PreparedTempoMap, PreparedTrackMix, compile,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sample_rate = 48_000;
    let mut editor = Editor::new(Project::new(sample_rate)?, 8)?;
    let track = match editor.execute(Command::AddMidiTrack {
        name: "profile".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected instrument track".into()),
    };
    let source_length = BeatTime::from_quarters(8)?;
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: source_length,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected midi clip".into()),
    };
    editor.execute(Command::SetMidiClipSource {
        clip,
        length: source_length,
        source_length,
        source_start: BeatTime::ZERO,
        loop_start: BeatTime::ZERO,
        loop_end: source_length,
        looping: false,
    })?;
    let gate = BeatTime::from_quarters(4)?;
    for index in 0..64_u64 {
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: gate,
            pitch: 24 + index as u8,
            channel: 0,
            velocity: 100,
        })?;
    }
    let project = editor.project();
    let timing = PreparedTempoMap::prepare(project.tempo_map().clone(), sample_rate)?;
    let source = PreparedMidiSourceTemplate::prepare(
        track,
        project.midi_clip(clip).ok_or("missing source clip")?,
        &timing,
    )?;
    let table = hex_devices::SineTable::new();
    let mut templates = Vec::new();
    templates.try_reserve_exact(100)?;
    for _ in 0..100 {
        templates.push(PreparedMidiInstrumentTemplate::prepare(&source, &table)?);
    }
    let mut buffer = MidiSpanBuffer::default();
    let mut output = [[0.0_f32; 2]; 64];
    let query_start = 3_000;
    for _ in 0..100 {
        templates[0].render(
            BeatTime::ZERO,
            &timing,
            &table,
            query_start,
            &mut output,
            &mut buffer,
        )?;
    }
    let one_source_spans = buffer.len();
    if one_source_spans != 64 {
        return Err("expected exactly 64 simultaneous source spans".into());
    }
    let single_count = 1_000_u64;
    let started = Instant::now();
    let mut consumed = 0_u64;
    for _ in 0..single_count {
        templates[0].render(
            BeatTime::ZERO,
            &timing,
            &table,
            query_start,
            &mut output,
            &mut buffer,
        )?;
        consumed = consumed.wrapping_add(
            buffer
                .spans()
                .map(|span| span.start_frame ^ span.end_frame ^ span.note.value())
                .sum::<u64>(),
        );
        black_box(&output);
    }
    let single_elapsed = started.elapsed();

    let aggregate_count = 100_u64;
    let mut scratch = [[0.0_f32; 2]; 64];
    let started = Instant::now();
    for _ in 0..aggregate_count {
        output.fill([0.0; 2]);
        for template in &templates {
            template.render(
                BeatTime::ZERO,
                &timing,
                &table,
                query_start,
                &mut scratch,
                &mut buffer,
            )?;
            for (mixed, sample) in output.iter_mut().zip(scratch) {
                mixed[0] += sample[0];
                mixed[1] += sample[1];
            }
            consumed = consumed.wrapping_add(buffer.len() as u64);
        }
        black_box(&output);
    }
    let aggregate_elapsed = started.elapsed();
    black_box(consumed);
    println!(
        "single_queries={single_count} spans_per_query={one_source_spans} frame_samples_per_query=64 sample_ops={} elapsed_ms={:.3} ns_per_query={:.1}",
        single_count * one_source_spans as u64 * 64,
        single_elapsed.as_secs_f64() * 1_000.0,
        single_elapsed.as_nanos() as f64 / single_count as f64,
    );
    println!(
        "aggregate_queries={} sources=100 voices_per_source=64 voices_per_block=6400 span_queries={} spans_per_source_query={} frame_samples_per_source_query=64 sample_ops={} elapsed_ms={:.3} ns_per_source_query={:.1} consumed={consumed}",
        aggregate_count,
        aggregate_count * 100,
        one_source_spans,
        aggregate_count * 100 * one_source_spans as u64 * 64,
        aggregate_elapsed.as_secs_f64() * 1_000.0,
        aggregate_elapsed.as_nanos() as f64 / (aggregate_count * 100) as f64,
    );
    println!(
        "fixture: 64 notes start at frame 0, gate=4 beats; query starts at frame 3000 (62.5 ms) and renders 64 frames at 48 kHz. setup and 100 warmup queries are excluded; this microbenchmark is not callback-deadline or allocation-instrumentation evidence"
    );

    // Independent finite note scheduling verifies scaled accumulation into an
    // existing destination before timing the ongoing-source path.
    editor.execute(Command::Gain { track, gain: 0.73 })?;
    editor.execute(Command::Balance {
        track,
        balance: 0.27,
    })?;
    let finite = compile::project(editor.project(), &std::collections::BTreeMap::new())?;
    let mix = PreparedTrackMix::new(0.73, 0.27, false)?;
    let context = MidiInstrumentRenderContext::new(BeatTime::ZERO, &timing, &table);
    let mut expected = [[0.125, -0.25]; 64];
    finite.tracks()[0].render_additive(query_start, &mut expected)?;
    let mut actual = [[0.125, -0.25]; 64];
    templates[0].render_additive(context, mix, query_start, &mut actual, &mut buffer)?;
    if expected != actual {
        return Err("additive ongoing/finite scaled accumulation mismatch".into());
    }

    let mixes = (0..100)
        .map(|index| {
            PreparedTrackMix::new(
                0.5 + (index % 7) as f32 * 0.07,
                (index % 9) as f32 * 0.1 - 0.4,
                false,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    for _ in 0..10 {
        output.fill([0.0; 2]);
        for (template, mix) in templates.iter().zip(&mixes) {
            template.render_additive(context, *mix, query_start, &mut output, &mut buffer)?;
        }
        black_box(&output);
    }
    let mut timings = Vec::with_capacity(aggregate_count as usize);
    let mut checksum = 0_u64;
    for _ in 0..aggregate_count {
        output.fill([0.0; 2]);
        let started = Instant::now();
        for (template, mix) in templates.iter().zip(&mixes) {
            template.render_additive(context, *mix, query_start, &mut output, &mut buffer)?;
        }
        timings.push(started.elapsed().as_nanos());
        for frame in black_box(&output) {
            checksum = checksum.wrapping_add(u64::from(frame[0].to_bits()));
            checksum = checksum.wrapping_add(u64::from(frame[1].to_bits()));
        }
    }
    timings.sort_unstable();
    println!(
        "additive: replicas=100 held_voices_per_replica=64 block_frames=64 rate=48000 warmups=10 samples={} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} above_period={} checksum={checksum}",
        timings.len(),
        timings[timings.len() / 2] as f64 / 1_000.0,
        timings[timings.len() * 95 / 100 - 1] as f64 / 1_000.0,
        timings[timings.len() * 99 / 100 - 1] as f64 / 1_000.0,
        timings[timings.len() - 1] as f64 / 1_000.0,
        timings.iter().filter(|&&ns| ns > 1_333_333).count(),
    );
    println!(
        "additive scope: fixed held window, varied gain/balance, existing destination checked against finite scheduling; excludes setup, checksum and destination clearing from timings. replicated templates are not a canonical 100-track/native deadline test"
    );
    Ok(())
}
