use hex_engine::Audio;
use hex_media::{ResampleLimits, resample};
use std::{error::Error, time::Instant};

const WARMUPS: usize = 2;
const ITERATIONS: usize = 8;
const MAX_FRAMES: usize = 48_000;

fn main() -> Result<(), Box<dyn Error>> {
    profile_case("44.1k → 48k", 44_100, 48_000)?;
    profile_case("48k → 44.1k", 48_000, 44_100)?;
    profile_case("48k → 16k", 48_000, 16_000)?;
    Ok(())
}

fn profile_case(name: &str, source_rate: u32, target_rate: u32) -> Result<(), Box<dyn Error>> {
    let source_frames = usize::try_from(source_rate)?;
    let source = Audio::new(source_rate, generated_stereo(source_rate, source_frames))?;
    let limits = ResampleLimits {
        max_source_frames: MAX_FRAMES,
        max_output_frames: MAX_FRAMES,
    };
    let convert = || resample(&source, target_rate, limits);

    for _ in 0..WARMUPS {
        let output = convert()?;
        std::hint::black_box(digest(output.frames()));
    }

    let mut times = Vec::with_capacity(ITERATIONS);
    let mut expected_digest = None;
    let mut output_frames = 0;
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let output = convert()?;
        let elapsed = started.elapsed();
        let current_digest = digest(output.frames());
        if expected_digest.is_some_and(|expected| expected != current_digest) {
            return Err("resampler produced inconsistent output across iterations".into());
        }
        expected_digest = Some(current_digest);
        output_frames = output.frames().len();
        times.push(elapsed);
        std::hint::black_box(output);
    }

    times.sort_unstable();
    let middle = ITERATIONS / 2;
    let median = (times[middle - 1].as_secs_f64() + times[middle].as_secs_f64()) / 2.0;
    let p95_index = (ITERATIONS * 95).div_ceil(100).saturating_sub(1);
    let p95 = times[p95_index].as_secs_f64();
    let maximum = times[ITERATIONS - 1].as_secs_f64();
    println!(
        "{name}: input={source_frames} output={output_frames} iterations={ITERATIONS} digest={:016x} median={:.3}ms p95={:.3}ms max={:.3}ms",
        expected_digest.unwrap_or_default(),
        median * 1_000.0,
        p95 * 1_000.0,
        maximum * 1_000.0,
    );
    Ok(())
}

fn generated_stereo(sample_rate: u32, frames: usize) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|index| {
            let seconds = index as f64 / f64::from(sample_rate);
            let left = (std::f64::consts::TAU * 997.0 * seconds).sin() * 0.5;
            let right = (std::f64::consts::TAU * 1_501.0 * seconds).sin() * 0.5;
            [left as f32, right as f32]
        })
        .collect()
}

fn digest(frames: &[[f32; 2]]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for sample in frames.iter().flatten() {
        for byte in sample.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}
