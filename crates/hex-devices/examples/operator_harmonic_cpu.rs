//! Design-only reconstruction microbenchmark, not a registered instrument.
//! Original additive/table fixture; no claims about Live or complete Operator DSP.
use std::{hint::black_box, time::Instant};
const N: usize = 2048;
const V: usize = 16;
const O: usize = 4;
const H: usize = 64;
const SCALE: f64 = 18_446_744_073_709_551_616.0;
#[derive(Clone, Copy)]
struct History {
    phase: [[u64; O]; V],
    previous: [[f64; O]; V],
}
fn steps(pitch: u8, rate: u32) -> [u64; O] {
    let hz = 440.0 * 2.0_f64.powf((f64::from(pitch) - 69.0) / 12.0);
    [0.5, 0.99_f32 as f64, 1.0, 2.0]
        .map(|ratio| (hz * ratio / f64::from(rate) * SCALE).round() as u64)
}
fn table() -> [f32; N + 1] {
    let mut t = [0.0; N + 1];
    for (i, value) in t.iter_mut().take(N).enumerate() {
        *value = (std::f64::consts::TAU * i as f64 / N as f64).sin() as f32;
    }
    t[N] = t[0];
    t
}
fn sample(t: &[f32; N + 1], phase: u64) -> f64 {
    let index = (phase >> 53) as usize;
    let fraction = (phase & ((1_u64 << 53) - 1)) as f64 / (1_u64 << 53) as f64;
    f64::from(t[index]) + (f64::from(t[index + 1]) - f64::from(t[index])) * fraction
}
fn render<const RECONSTRUCT: bool>(
    history: &mut History,
    cached: &[[u64; O]; V],
    t: &[f32; N + 1],
    rate: u32,
    output: &mut [f64],
) {
    // Reconstruct once per live voice per block, not once per sample. Partitions
    // therefore expose the upper cost from host segmentation/event boundaries.
    let local = if RECONSTRUCT {
        std::array::from_fn(|v| steps(black_box(36 + v as u8), black_box(rate)))
    } else {
        *cached
    };
    for out in output {
        let mut mixed = 0.0;
        for (v, voice_steps) in local.iter().enumerate() {
            for (o, step) in voice_steps.iter().enumerate() {
                let feedback = (history.previous[v][o] * 0.125 * SCALE) as i64 as u64;
                let phase = history.phase[v][o].wrapping_add(feedback);
                let mut sum = 0.0;
                for h in 1..=H {
                    // Strict unmodulated below-Nyquist fixture bound; not an
                    // accepted active-FM/LFO anti-aliasing contract.
                    if (*step as u128) * (h as u128) >= (1_u128 << 63) {
                        break;
                    }
                    sum += sample(t, phase.wrapping_mul(h as u64)) / h as f64;
                }
                history.previous[v][o] = sum / 8.0;
                history.phase[v][o] = history.phase[v][o].wrapping_add(*step);
                mixed += history.previous[v][o] / (V * O) as f64;
            }
        }
        *out = mixed;
    }
}
fn run<const R: bool>(rate: u32, partition: usize, frames: usize) -> (u128, History, f64) {
    let t = table();
    let cached = std::array::from_fn(|v| steps(36 + v as u8, rate));
    let mut history = History {
        phase: [[0; O]; V],
        previous: [[0.0; O]; V],
    };
    let mut output = [0.0; 256];
    let started = Instant::now();
    let mut checksum = 0.0;
    for offset in (0..frames).step_by(partition) {
        let count = partition.min(frames - offset);
        render::<R>(&mut history, &cached, &t, rate, &mut output[..count]);
        checksum += black_box(output[..count].iter().sum::<f64>());
    }
    (started.elapsed().as_nanos(), history, checksum)
}
fn main() {
    println!(
        "design-only original additive fixture voices={V} oscillators={O} partials={H} frames=4096 allocations_in_render=0_by_source_not_instrumented"
    );
    for rate in [8_000, 48_000, 192_000] {
        for partition in [1, 17, 64, 128, 256] {
            let _ = run::<false>(rate, partition, 512);
            let _ = run::<true>(rate, partition, 512);
            let mut cached = [0; 7];
            let mut rebuilt = [0; 7];
            for trial in 0..7 {
                let (a, b) = if trial % 2 == 0 {
                    (
                        run::<false>(rate, partition, 4096),
                        run::<true>(rate, partition, 4096),
                    )
                } else {
                    let b = run::<true>(rate, partition, 4096);
                    (run::<false>(rate, partition, 4096), b)
                };
                assert_eq!(a.1.phase, b.1.phase);
                assert_eq!(a.1.previous, b.1.previous);
                assert_eq!(a.2.to_bits(), b.2.to_bits());
                cached[trial] = a.0;
                rebuilt[trial] = b.0;
            }
            cached.sort();
            rebuilt.sort();
            println!(
                "rate={rate} partition={partition} cached_median_ns={} rebuilt_median_ns={} ratio={:.4} cached_range={}-{} rebuilt_range={}-{}",
                cached[3],
                rebuilt[3],
                rebuilt[3] as f64 / cached[3] as f64,
                cached[0],
                cached[6],
                rebuilt[0],
                rebuilt[6]
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_pcm_and_history_match_reconstruction_and_partitioning() {
        let t = table();
        for rate in [8_000, 44_100, 48_000, 192_000] {
            let cached = std::array::from_fn(|v| steps(36 + v as u8, rate));
            let initial = History {
                phase: [[0; O]; V],
                previous: [[0.0; O]; V],
            };
            let mut baseline = initial;
            let mut expected = [0.0; 257];
            render::<false>(&mut baseline, &cached, &t, rate, &mut expected);
            for partition in [1, 17, 64, 128, 256] {
                let mut actual = [0.0; 257];
                let mut history = initial;
                for chunk in actual.chunks_mut(partition) {
                    render::<true>(&mut history, &cached, &t, rate, chunk);
                }
                assert_eq!(actual, expected);
                assert_eq!(history.phase, baseline.phase);
                assert_eq!(history.previous, baseline.previous);
                assert!(actual.iter().all(|v| v.is_finite()));
            }
        }
    }
}
