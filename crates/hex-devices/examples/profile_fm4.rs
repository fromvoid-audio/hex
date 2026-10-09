//! Standalone original FM4 kernel microbenchmark; no audio device or engine.
use hex_devices::{
    SineTable,
    fm4::{Envelope, Oscillator, PreparedFm4, Routing},
};
use std::{error::Error, hint::black_box, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let table = SineTable::new();
    let mut matrix = [[0.0; 4]; 4];
    matrix[3][2] = 0.05;
    matrix[2][1] = 0.1;
    matrix[1][0] = 0.2;
    let routing = Routing::new(matrix, [1.0, 0.0, 0.0, 0.0])?;
    let oscillators = [Oscillator::new(1.0, 0.25, 0.0)?; 4];
    let envelope = Envelope::new(0.005, 0.1, 0.5, 0.05)?;
    let voices: Vec<_> = (0..100)
        .map(|index| PreparedFm4::new(48000, 60 + index % 12, 100, oscillators, routing, envelope))
        .collect::<Result<_, _>>()?;
    let render = |start: u64| {
        let mut block = [0.0; 128];
        for voice in &voices {
            for (offset, output) in block.iter_mut().enumerate() {
                *output += voice.sample(&table, start + offset as u64, u64::MAX);
            }
        }
        black_box(block);
    };
    for block in 0..100 {
        render(block * 128);
    }
    let mut durations = Vec::with_capacity(2000);
    let started = Instant::now();
    for block in 100..2100 {
        let block_start = Instant::now();
        render(block * 128);
        durations.push(block_start.elapsed());
    }
    let elapsed = started.elapsed();
    durations.sort_unstable();
    println!(
        "fm4 standalone: 48000 hz, 100 held voices, 4-stage sine routing, 128 frames, 100 warmups, 2000 blocks"
    );
    println!(
        "prepared bytes/note={} total={:.3}s p50/p95/p99/max={:.1}/{:.1}/{:.1}/{:.1}us; nominal period=2666.7us",
        std::mem::size_of::<PreparedFm4>(),
        elapsed.as_secs_f64(),
        durations[1000].as_secs_f64() * 1e6,
        durations[1900].as_secs_f64() * 1e6,
        durations[1980].as_secs_f64() * 1e6,
        durations[1999].as_secs_f64() * 1e6
    );
    Ok(())
}
