//! Deterministic TPDF dither for PCM16 export quantisation.
//!
//! Every PCM16 writer (Arrangement main export and each stem file) quantises
//! through one [`Pcm16Quantiser`]. Each nonzero sample is scaled to `±32767`,
//! offset by triangular (TPDF) dither spanning `(-1, +1)` LSB, rounded and
//! clamped to the symmetric `±32767` range. The dither is the difference of two
//! independent uniform values, giving the standard ±1 LSB triangular density.
//!
//! The dither value is a pure hash of a fixed seed, the output stream, the
//! absolute timeline frame and the channel, so:
//! - identical renders produce byte-identical files;
//! - a range export equals the matching slice of a longer export of the same
//!   render, because dither is anchored to timeline frames, not file offsets;
//! - the two channels of a frame, and different stems, get independent dither.
//!
//! Exact `0.0` input stays `0`, so digital silence remains bit-exact zero.
//! Over detection looks at the undithered input: only samples beyond full scale
//! count as clipped, never dither landing on the clamp.
//!
//! Per-sample cost is one 64-bit integer hash and a few float operations, with
//! no state, allocation or I/O.

use hex_engine::OutputCaptureId;

/// Fixed base seed; changing it changes every dithered PCM16 export's bytes.
const BASE_SEED: u64 = 0x6a09_e667_f3bc_c909;
/// Stream-key namespaces keeping track, return and main streams distinct.
const TRACK_STREAMS: u64 = 1 << 62;
const RETURN_STREAMS: u64 = 2 << 62;
/// Full-scale PCM16 magnitude used by the symmetric quantiser.
const FULL_SCALE: f32 = 32_767.0;
/// Scales a 24-bit integer draw into `[0, 1)`.
const UNIT_SCALE: f32 = 1.0 / 16_777_216.0;

/// PCM16 quantiser with deterministic, timeline-anchored TPDF dither for one
/// output stream.
#[derive(Clone, Copy)]
pub(crate) struct Pcm16Quantiser {
    seed: u64,
}

impl Pcm16Quantiser {
    /// Quantiser for the Arrangement main (or captured-mix) export file.
    pub(crate) fn main() -> Self {
        Self::with_stream(0)
    }

    /// Quantiser for one stem file, keyed by its capture owner so a stem's
    /// bytes do not depend on which other stems share the batch.
    pub(crate) fn stem(id: OutputCaptureId) -> Self {
        Self::with_stream(match id {
            OutputCaptureId::Track(id) => TRACK_STREAMS ^ id.value(),
            OutputCaptureId::Return(id) => RETURN_STREAMS ^ id.value(),
        })
    }

    fn with_stream(stream: u64) -> Self {
        Self {
            seed: mix(BASE_SEED ^ mix(stream)),
        }
    }

    /// Quantise one finite sample at an absolute timeline frame and channel,
    /// reporting whether the input exceeded full scale.
    pub(crate) fn quantise(self, sample: f32, frame: u64, channel: usize) -> (i16, bool) {
        let over = sample.abs() > 1.0;
        if sample == 0.0 {
            return (0, over);
        }
        let position = frame.wrapping_mul(2).wrapping_add(channel as u64);
        let bits = mix(self.seed ^ mix(position));
        let first = ((bits >> 40) as u32) as f32 * UNIT_SCALE;
        let second = (((bits >> 16) & 0xff_ffff) as u32) as f32 * UNIT_SCALE;
        let scaled = sample.clamp(-1.0, 1.0) * FULL_SCALE + (first - second);
        (scaled.round().clamp(-FULL_SCALE, FULL_SCALE) as i16, over)
    }
}

/// splitmix64 finaliser: a bijective, well-distributed 64-bit mix.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quantise_all(quantiser: Pcm16Quantiser, start: u64, samples: &[f32]) -> Vec<i16> {
        samples
            .iter()
            .zip(start..)
            .map(|(s, frame)| quantiser.quantise(*s, frame, 0).0)
            .collect()
    }

    fn sine(amplitude: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| amplitude * (std::f32::consts::TAU * 997.0 * n as f32 / 48_000.0).sin())
            .collect()
    }

    #[test]
    fn minus_90_dbfs_sine_is_dithered_not_silenced_or_squared() {
        // -90 dBFS is about 1.04 LSB peak: undithered rounding yields only
        // {-1, 0, 1} in a square-like pattern locked to the sine.
        let amplitude = 10_f32.powf(-90.0 / 20.0);
        let input = sine(amplitude, 48_000);
        let output = quantise_all(Pcm16Quantiser::main(), 0, &input);

        assert!(output.iter().any(|q| *q != 0), "not rounded to silence");
        let distinct: std::collections::BTreeSet<_> = output.iter().collect();
        assert!(distinct.len() >= 4, "more than a three-level square wave");
        assert!(output.iter().all(|q| q.abs() <= 3), "bounded dither");

        // The signal survives: output correlates with the sine at unity gain.
        let lsb: Vec<f64> = input.iter().map(|s| f64::from(*s) * 32_767.0).collect();
        let (mut dot, mut energy) = (0.0, 0.0);
        for (x, q) in lsb.iter().zip(&output) {
            dot += x * f64::from(*q);
            energy += x * x;
        }
        let gain = dot / energy;
        assert!((gain - 1.0).abs() < 0.1, "unbiased signal gain {gain}");

        // Error power matches TPDF (1/6 LSB^2) plus rounding (1/12 LSB^2) = 0.25.
        let errors: Vec<f64> = lsb
            .iter()
            .zip(&output)
            .map(|(x, q)| f64::from(*q) - x)
            .collect();
        let power = errors.iter().map(|e| e * e).sum::<f64>() / errors.len() as f64;
        assert!((0.2..0.3).contains(&power), "tpdf noise floor {power}");

        // Noise-like: the error is uncorrelated with the signal and white
        // (negligible lag-one autocorrelation), not a tone locked to the sine.
        let signal_correlation = lsb.iter().zip(&errors).map(|(x, e)| x * e).sum::<f64>() / energy;
        assert!(signal_correlation.abs() < 0.05, "{signal_correlation}");
        let lag_one = errors.windows(2).map(|w| w[0] * w[1]).sum::<f64>()
            / errors.iter().map(|e| e * e).sum::<f64>();
        assert!(lag_one.abs() < 0.05, "white error spectrum {lag_one}");
    }

    #[test]
    fn dither_is_reproducible_timeline_anchored_and_independent_per_stream() {
        let input = sine(0.001, 4_096);
        let main = Pcm16Quantiser::main();
        let full = quantise_all(main, 1_000, &input);
        assert_eq!(full, quantise_all(main, 1_000, &input));
        // A range starting later reproduces the same slice.
        assert_eq!(&full[100..], quantise_all(main, 1_100, &input[100..]));
        // Capture ids are not publicly constructible; use their stream keys.
        let track = Pcm16Quantiser::with_stream(TRACK_STREAMS ^ 1);
        let other = Pcm16Quantiser::with_stream(RETURN_STREAMS ^ 1);
        assert_ne!(full, quantise_all(track, 1_000, &input));
        assert_ne!(
            quantise_all(track, 1_000, &input),
            quantise_all(other, 1_000, &input)
        );
        // Channels of one frame differ in dither.
        let left: Vec<_> = (0..4_096).map(|f| main.quantise(0.001, f, 0).0).collect();
        let right: Vec<_> = (0..4_096).map(|f| main.quantise(0.001, f, 1).0).collect();
        assert_ne!(left, right);
    }

    #[test]
    fn silence_stays_exact_zero_and_output_stays_within_one_lsb_of_rounding() {
        let main = Pcm16Quantiser::main();
        for frame in 0..1_000 {
            assert_eq!(main.quantise(0.0, frame, 0), (0, false));
            assert_eq!(main.quantise(-0.0, frame, 1), (0, false));
        }
        for sample in [0.5_f32, -0.5, 0.25, 1.0e-3, -0.999] {
            let rounded = (sample * 32_767.0).round() as i32;
            for frame in 0..1_000 {
                let (q, over) = main.quantise(sample, frame, 0);
                assert!(!over);
                assert!((i32::from(q) - rounded).abs() <= 1, "{sample} -> {q}");
            }
        }
    }

    #[test]
    fn clipping_counts_input_overs_and_clamps_symmetrically() {
        let main = Pcm16Quantiser::main();
        for frame in 0..1_000 {
            let (q, over) = main.quantise(1.0, frame, 0);
            assert!(!over && (32_766..=32_767).contains(&q));
            let (q, over) = main.quantise(-1.0, frame, 1);
            assert!(!over && (-32_767..=-32_766).contains(&q));
            let (q, over) = main.quantise(1.5, frame, 0);
            assert!(over && q >= 32_766);
            let (q, over) = main.quantise(-1.000_001, frame, 1);
            assert!(over && q <= -32_766);
        }
    }
}
