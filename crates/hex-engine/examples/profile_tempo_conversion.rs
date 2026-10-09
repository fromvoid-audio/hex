//! Release microbenchmark for borrowed, prepared tempo event/block conversions.
//! This measures conversion throughput only; it is not a callback deadline test.
use hex_engine::PreparedTempoMap;
use hex_project::{BeatTime, Tempo, TempoMap, TempoPoint};
use std::{hint::black_box, time::Instant};

const SAMPLE_RATE: u32 = 48_000;
const EVENT_QUERIES: usize = 4_096;
const BLOCK_QUERIES: usize = 2_048;
const ITERATIONS: usize = 200;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut points = Vec::with_capacity(1_024);
    for index in 0..1_024_u64 {
        let beat = BeatTime::from_quarters(index * 4)?;
        let tempo = Tempo::new(70_000_000 + (index % 31) as u32 * 2_000_000)?;
        points.push(TempoPoint::new(beat, tempo));
    }
    let canonical = TempoMap::new(SAMPLE_RATE, points)?;
    let prepared = PreparedTempoMap::prepare(canonical, SAMPLE_RATE)?;

    let mut event_beats = Vec::with_capacity(EVENT_QUERIES);
    let max_raw = BeatTime::from_quarters(4_092)?.raw();
    for index in 0..EVENT_QUERIES {
        let raw = (u128::from(max_raw) * index as u128 / (EVENT_QUERIES - 1) as u128) as u64;
        event_beats.push(BeatTime::from_raw(raw));
    }
    let mut block_frames = Vec::with_capacity(BLOCK_QUERIES);
    for index in 0..BLOCK_QUERIES {
        let beat = event_beats[index * (EVENT_QUERIES - 1) / (BLOCK_QUERIES - 1)];
        block_frames.push(prepared.beat_to_frame(beat)?);
    }

    let mut checksum = 0_u64;
    for _ in 0..8 {
        for beat in &event_beats {
            checksum ^= prepared.beat_to_frame(*beat)?;
        }
        for frame in &block_frames {
            checksum ^= prepared.frame_to_beat(*frame)?.raw();
        }
    }

    let started = Instant::now();
    for _ in 0..ITERATIONS {
        for beat in &event_beats {
            checksum = checksum
                .rotate_left(1)
                .wrapping_add(prepared.beat_to_frame(black_box(*beat))?);
        }
        for frame in &block_frames {
            checksum = checksum
                .rotate_left(1)
                .wrapping_add(prepared.frame_to_beat(black_box(*frame))?.raw());
        }
    }
    let elapsed = started.elapsed();
    let event_operations = EVENT_QUERIES * ITERATIONS;
    let block_operations = BLOCK_QUERIES * ITERATIONS;
    let operations = event_operations + block_operations;
    let nanos = elapsed.as_nanos();
    let ns_per_conversion = nanos as f64 / operations as f64;
    let conversions_per_second = operations as f64 / elapsed.as_secs_f64();
    println!(
        "prepared tempo: {} points · {} event + {} block conversions = {} operations in {:.3} ms ({ns_per_conversion:.1} ns/op, {:.0} op/s) · checksum {}",
        prepared.point_count(),
        event_operations,
        block_operations,
        operations,
        elapsed.as_secs_f64() * 1_000.0,
        conversions_per_second,
        black_box(checksum)
    );
    Ok(())
}
