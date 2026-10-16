//! Optimized preparation microbenchmark; no native deadline/listening claim.
use hex_warp::*;
use std::{hint::black_box, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source: Vec<_> = (0..48_000)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            [
                (0.3 * (t * 440.0 * std::f64::consts::TAU).sin()
                    + 0.1 * (t * 880.0 * std::f64::consts::TAU).sin()) as f32,
                (0.2 * (t * 440.0 * std::f64::consts::TAU + 0.7).sin()) as f32,
            ]
        })
        .collect();
    let map = TimeMap::new(
        vec![MapSegment {
            output_start: 0,
            output_end: 72_000,
            source_start_q32: 0,
            source_end_q32: 48_000_i128 << 32,
        }],
        72_000,
    )?;
    println!(
        "rate=48000 stereo source=48000 output=72000 pitch=300c warmup=1 measured=5 timer=Instant allocation=included"
    );
    for mode in [
        Mode::RePitch,
        Mode::Beats {
            division_frames: Some(6_000),
            tail: Tail::Forward,
            decay: 100,
        },
        Mode::Tones { grain_ms: 40 },
        Mode::Texture {
            grain_ms: 40,
            fluctuation: 25,
            seed: 1,
        },
        Mode::Complex,
        Mode::ComplexPro {
            formants: 100,
            envelope: 128,
        },
    ] {
        let request = Request {
            attacks: &[],
            input: Input {
                frames: &source,
                sample_rate: 48_000,
                region_start: 0,
                region_length: 48_000,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            map: &map,
            output_frames: 72_000,
            pitch_cents: 300,
            mode,
        };
        black_box(render(&request, &mut || false)?);
        let mut times = Vec::new();
        let mut scratch = 0;
        let mut work = 0;
        let mut hash = 0xcbf29ce484222325_u64;
        for _ in 0..5 {
            let before = Instant::now();
            let output = render(black_box(&request), &mut || false)?;
            times.push(before.elapsed().as_secs_f64() * 1_000.0);
            scratch = output.peak_scratch_bytes;
            work = output.work_units;
            for frame in &output.frames {
                for sample in frame {
                    for byte in sample.to_le_bytes() {
                        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                    }
                }
            }
            black_box(output);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "{mode:?} median_ms={:.3} min_ms={:.3} max_ms={:.3} scratch={scratch} work={work} hash={hash:016x}",
            times[2], times[0], times[4]
        );
    }
    Ok(())
}
