//! CPU render microbenchmark, not an audio-device or UI benchmark.
use hex_engine::live::{Command, LiveEngine};
use std::{hint::black_box, time::Instant};
fn main() -> Result<(), hex_engine::Invalid> {
    for block in [64, 128, 256, 512] {
        let mut engine = LiveEngine::demo(48_000)?;
        engine.apply(Command::Play);
        let mut timings = Vec::with_capacity(4000);
        for iteration in 0..4200 {
            let start = Instant::now();
            engine.begin_block();
            for _ in 0..block {
                black_box(engine.next_frame());
            }
            let elapsed = start.elapsed().as_nanos();
            if iteration >= 200 {
                timings.push(elapsed);
            }
        }
        timings.sort_unstable();
        let deadline = block as f64 / 48_000.0 * 1e6;
        println!(
            "{block:3} frames: p50={:.1}µs p99={:.1}µs max={:.1}µs deadline={deadline:.1}µs",
            timings[2000] as f64 / 1000.0,
            timings[3960] as f64 / 1000.0,
            timings[3999] as f64 / 1000.0
        );
    }
    Ok(())
}
