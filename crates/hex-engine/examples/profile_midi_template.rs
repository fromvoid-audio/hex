use std::{hint::black_box, time::Instant};

use hex_engine::{MidiEdgeBuffer, PreparedMidiSourceTemplate, PreparedTempoMap};
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
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(2)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected midi clip".into()),
    };
    let source_length = BeatTime::from_quarters(2)?;
    editor.execute(Command::SetMidiClipSource {
        clip,
        length: BeatTime::from_quarters(2)?,
        source_length,
        source_start: BeatTime::ZERO,
        loop_start: BeatTime::ZERO,
        loop_end: source_length,
        looping: false,
    })?;
    let gate = BeatTime::from_ratio(1, 12_000)?;
    for index in 0..128_u64 {
        let start = BeatTime::from_raw(gate.raw() * index);
        editor.execute(Command::AddMidiNote {
            clip,
            start,
            duration: gate,
            pitch: (index % 128) as u8,
            channel: 0,
            velocity: 100,
        })?;
    }
    let project = editor.project();
    let timing = PreparedTempoMap::prepare(project.tempo_map().clone(), sample_rate)?;
    let template = PreparedMidiSourceTemplate::prepare(
        track,
        project.midi_clip(clip).ok_or("missing clip")?,
        &timing,
    )?;
    let mut buffer = MidiEdgeBuffer::default();
    let query_count = 10_000_u64;
    for _ in 0..100 {
        template.query(BeatTime::ZERO, &timing, 0, 256, &mut buffer)?;
        black_box(&buffer);
    }
    let edges_per_query = buffer.len();
    let start = Instant::now();
    let mut consumed = 0_u64;
    for _ in 0..query_count {
        template.query(BeatTime::ZERO, &timing, 0, 256, &mut buffer)?;
        consumed = consumed.wrapping_add(
            buffer
                .events()
                .map(|event| {
                    u64::from(event.pitch) + event.frame + event.note.value() + event.iteration
                })
                .sum::<u64>(),
        );
        black_box(&buffer);
    }
    let elapsed = start.elapsed();
    black_box(consumed);
    let operations = query_count * 256;
    println!(
        "queries={query_count} edges_per_query={edges_per_query} max_edge_operations={operations} elapsed_ms={:.3} ns_per_query={:.1} consumed={consumed}",
        elapsed.as_secs_f64() * 1_000.0,
        elapsed.as_nanos() as f64 / query_count as f64,
    );
    println!("fixture is a dense one-shot source; 100 warmup queries and setup excluded");
    println!("result is a local microbenchmark, not an audio-deadline claim");
    Ok(())
}
