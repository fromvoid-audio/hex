//! Original standalone borrowed-PCM kernel timing; no host/callback deadline claim.
use hex_devices::sampler::{Mode, PreparedSampler, Region, Sample, Settings};
use std::{hint::black_box, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const RATE: u32 = 48_000;
    let pcm: Vec<_> = (0..RATE)
        .map(|frame| {
            let left =
                (f64::from(frame) * 440.0 * std::f64::consts::TAU / f64::from(RATE)).sin() as f32;
            [left * 0.1, -left * 0.05]
        })
        .collect();
    let sample = Sample::new(&pcm, RATE)?;
    let mut settings = Settings::new(sample.whole_region());
    settings.mode = Mode::Classic {
        loop_region: Some(Region {
            start: 137,
            end: pcm.len() - 97,
        }),
    };
    settings.detune_cents = 7.0;
    // Notes 48.. around root 60, all detuned +7 cents. Transpose -24 keeps
    // every voice below unity (the linear path, a same-run reference); 0
    // mixes linear voices (notes < 60) with slightly pitched-up ones (notes
    // >= 60 run at rate > 1 because of the detune); +24 pitches every voice
    // up (rate about 2-12), the band-limited path.
    for transpose in [-24.0, 0.0, 24.0] {
        settings.transpose_semitones = transpose;
        profile(sample, settings, transpose)?;
    }
    Ok(())
}

fn profile(
    sample: Sample<'_>,
    settings: Settings<'_>,
    transpose: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    const RATE: u32 = 48_000;
    const BLOCK: usize = 128;
    const TRIALS: usize = 2_000;
    const WARMUP: usize = 100;
    let sampler = PreparedSampler::new(sample, RATE, settings)?;
    for voice_count in [1, 16, 32] {
        let voices: Vec<_> = (0..voice_count)
            .map(|index| {
                sampler
                    .prepare_voice(48 + index as u8, 100)?
                    .ok_or(hex_devices::Invalid("unexpected silent voice"))
            })
            .collect::<Result<Vec<_>, hex_devices::Invalid>>()?;
        let mut durations = vec![0_u128; TRIALS];
        let mut output = [[0.0_f32; 2]; BLOCK];
        let mut checksum = 0.0_f64;
        for block in 0..WARMUP + TRIALS {
            let start = (block * BLOCK) as u64;
            let clock = Instant::now();
            output.fill([0.0; 2]);
            for voice in &voices {
                for (offset, frame) in output.iter_mut().enumerate() {
                    let value = voice.sample(start + offset as u64, u64::MAX);
                    frame[0] += value[0];
                    frame[1] += value[1];
                }
            }
            black_box(&output);
            let elapsed = clock.elapsed().as_nanos();
            if block >= WARMUP {
                durations[block - WARMUP] = elapsed;
            }
            checksum += f64::from(output[0][0]);
        }
        durations.sort_unstable();
        println!(
            "transpose={transpose} voices={voice_count} rate={RATE} block={BLOCK} warmup={WARMUP} trials={TRIALS} min_ns={} p50_ns={} p95_ns={} p99_ns={} max_ns={} checksum={checksum:.9}",
            durations[0],
            durations[TRIALS / 2],
            durations[TRIALS * 95 / 100],
            durations[TRIALS * 99 / 100],
            durations[TRIALS - 1]
        );
    }
    Ok(())
}
