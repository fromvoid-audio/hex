//! Shared 8x oversampled waveshaping for native nonlinear devices (Pulse
//! #3958 Overdrive, #4146 Saturator).
//!
//! One linear-phase Kaiser-windowed sinc kernel (96 taps at the oversampled
//! rate, cutoff 0.4167 of the base rate, beta 10) serves both the polyphase
//! interpolator and the decimator. The device's `Shaper` runs on every
//! oversampled sample through first-order antiderivative anti-aliasing
//! (ADAA), which suppresses the residual aliasing that 8x alone leaves for
//! near-square transfers at high drive. All state is inline and fixed size;
//! processing never allocates.
//!
//! Latency: each filter delays (96 - 1) / 2 oversampled samples and ADAA adds
//! half a sample (95.5). The decimator emits the newest of the eight phases,
//! 7 samples after the base-rate input, so the round trip is 88.5 oversampled
//! samples = 11.0625 base frames, declared as 11. The 0.0625-frame remainder
//! is a fractional delay that integer latency cannot express.
//!
//! Does not own drive, output gain, mix, dry-path alignment or latency
//! declaration; each device applies those around `Oversampler::process`.

pub(crate) const FACTOR: usize = 8;
pub(crate) const TAPS: usize = 96;
const PHASE_TAPS: usize = TAPS / FACTOR;
/// Declared base-rate latency of the oversampled stage (11.0625 rounded).
pub(crate) const LATENCY_FRAMES: usize = 11;
const CUTOFF: f64 = 20.0 / 48.0 / FACTOR as f64;
const BETA: f64 = 10.0;
/// Below this oversampled step, ADAA uses the midpoint value to avoid
/// cancellation in the antiderivative difference.
const ADAA_EPSILON: f64 = 1.0e-5;
const FLOOR: f64 = 1.0e-20;

/// A memoryless odd transfer with a closed-form antiderivative.
pub(crate) trait Shaper {
    /// Distinct per transfer law, so a law change refreshes the cached
    /// antiderivative instead of mixing two laws in one ADAA difference.
    fn tag(&self) -> u8;
    fn value(&self, x: f64) -> f64;
    /// Must be continuous and satisfy `antiderivative(0) == 0`.
    fn antiderivative(&self, x: f64) -> f64;
}

/// Shared interpolation/decimation kernel, normalized to unity DC gain, plus
/// its polyphase split (branch `p` holds taps `p, p + FACTOR, ...`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Kernel {
    taps: [f64; TAPS],
    phases: [[f64; PHASE_TAPS]; FACTOR],
}

impl Kernel {
    /// Control-side construction; uses transcendental functions.
    pub(crate) fn new() -> Self {
        let middle = (TAPS - 1) as f64 / 2.0;
        let mut taps: [f64; TAPS] = std::array::from_fn(|n| {
            let t = n as f64 - middle;
            let sinc = if t == 0.0 {
                2.0 * CUTOFF
            } else {
                (std::f64::consts::TAU * CUTOFF * t).sin() / (std::f64::consts::PI * t)
            };
            let r = t / middle;
            sinc * bessel_i0(BETA * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(BETA)
        });
        // Normalize each polyphase branch to exactly 1 / FACTOR of the DC
        // gain (and so the whole kernel to unity). Unequal branch gains would
        // modulate a held input at the base rate, so the shaped steady state
        // would miss the static transfer that devices preview.
        for phase in 0..FACTOR {
            let gain: f64 = taps.iter().skip(phase).step_by(FACTOR).sum();
            for tap in taps.iter_mut().skip(phase).step_by(FACTOR) {
                *tap /= gain * FACTOR as f64;
            }
        }
        let phases = std::array::from_fn(|p| std::array::from_fn(|j| taps[p + FACTOR * j]));
        Self { taps, phases }
    }
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..64 {
        term *= (x / (2.0 * k as f64)).powi(2);
        sum += term;
    }
    sum
}

fn quiet(value: f64) -> f64 {
    if value.abs() < FLOOR { 0.0 } else { value }
}

/// First-order ADAA between the previous and current argument, given both
/// arguments' antiderivatives.
fn adaa(
    shaper: &impl Shaper,
    current: f64,
    current_antiderivative: f64,
    previous: f64,
    previous_antiderivative: f64,
) -> f64 {
    let delta = current - previous;
    if delta.abs() < ADAA_EPSILON {
        shaper.value((current + previous) * 0.5)
    } else {
        (current_antiderivative - previous_antiderivative) / delta
    }
}

/// One channel's oversampler history.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Oversampler {
    /// Base-rate input, newest first.
    input: [f64; PHASE_TAPS],
    /// Shaped oversampled output, mirrored so the newest `TAPS` samples are
    /// always contiguous starting at `write` (newest first).
    shaped: [f64; 2 * TAPS],
    write: usize,
    /// Previous driven argument for ADAA, its antiderivative, and the law
    /// that antiderivative belongs to.
    previous: f64,
    previous_antiderivative: f64,
    previous_tag: u8,
    /// Consecutive zero inputs, saturating at `DRAIN_FRAMES`.
    zero_run: usize,
}
/// Zero inputs after which every history above is exactly zero: the input
/// history clears after `PHASE_TAPS`, the ADAA argument one phase later, and
/// the shaped history after a further `TAPS / FACTOR` frames.
pub(crate) const DRAIN_FRAMES: usize = PHASE_TAPS + TAPS / FACTOR + 1;

impl Default for Oversampler {
    fn default() -> Self {
        Self {
            input: [0.0; PHASE_TAPS],
            shaped: [0.0; 2 * TAPS],
            write: 0,
            previous: 0.0,
            previous_antiderivative: 0.0,
            previous_tag: 0,
            zero_run: 0,
        }
    }
}

impl Oversampler {
    pub(crate) fn is_quiescent(&self) -> bool {
        self.previous == 0.0
            && self.input.iter().all(|v| *v == 0.0)
            && self.shaped.iter().all(|v| *v == 0.0)
    }

    /// Processes one base-rate sample: interpolate 8x, multiply by `drive`,
    /// shape through ADAA and return the decimated output. Once drained by
    /// silence it returns exact zeros without filtering.
    pub(crate) fn process(
        &mut self,
        input: f64,
        drive: f64,
        kernel: &Kernel,
        shaper: &impl Shaper,
    ) -> f64 {
        let input = quiet(input);
        if input == 0.0 {
            if self.zero_run >= DRAIN_FRAMES {
                return 0.0;
            }
            self.zero_run += 1;
        } else {
            self.zero_run = 0;
        }
        if self.previous_tag != shaper.tag() {
            self.previous_tag = shaper.tag();
            self.previous_antiderivative = shaper.antiderivative(self.previous);
        }
        self.input.copy_within(0..PHASE_TAPS - 1, 1);
        self.input[0] = input;
        let scale = FACTOR as f64 * drive;
        for branch in &kernel.phases {
            let interpolated: f64 = branch.iter().zip(&self.input).map(|(h, x)| h * x).sum();
            let driven = quiet(interpolated * scale);
            let antiderivative = shaper.antiderivative(driven);
            let shaped = quiet(adaa(
                shaper,
                driven,
                antiderivative,
                self.previous,
                self.previous_antiderivative,
            ));
            self.previous = driven;
            self.previous_antiderivative = antiderivative;
            self.write = (self.write + TAPS - 1) % TAPS;
            self.shaped[self.write] = shaped;
            self.shaped[self.write + TAPS] = shaped;
        }
        self.shaped[self.write..self.write + TAPS]
            .iter()
            .zip(&kernel.taps)
            .map(|(x, h)| x * h)
            .sum()
    }
}

#[cfg(test)]
pub(crate) mod spectrum;

#[cfg(test)]
mod tests {
    use super::*;

    struct Tanh;
    impl Shaper for Tanh {
        fn tag(&self) -> u8 {
            1
        }
        fn value(&self, x: f64) -> f64 {
            x.tanh()
        }
        fn antiderivative(&self, x: f64) -> f64 {
            let a = x.abs();
            a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
        }
    }

    #[test]
    fn kernel_is_symmetric_with_unity_dc_and_phase_gains() {
        let kernel = Kernel::new();
        for n in 0..TAPS {
            assert!((kernel.taps[n] - kernel.taps[TAPS - 1 - n]).abs() < 1e-15);
        }
        assert!((kernel.taps.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        // Each polyphase branch must carry 1/FACTOR of the DC gain, or the
        // interpolator would add an image at the base rate.
        for phase in 0..FACTOR {
            let gain: f64 = (0..PHASE_TAPS)
                .map(|j| kernel.taps[phase + FACTOR * j])
                .sum();
            assert!(
                (gain * FACTOR as f64 - 1.0).abs() < 1e-12,
                "phase {phase}: {gain}"
            );
        }
    }

    #[test]
    fn adaa_matches_the_transfer_for_slow_arguments_and_is_finite_at_extremes() {
        let f = |x| Tanh.antiderivative(x);
        for v in [-3.0, -0.5, 0.0, 0.25, 2.0] {
            let (a, b) = (v + 1e-3, v - 1e-3);
            assert!((adaa(&Tanh, a, f(a), b, f(b)) - f64::tanh(v)).abs() < 1e-6);
            assert!((adaa(&Tanh, v, f(v), v, f(v)) - f64::tanh(v)).abs() < 1e-15);
        }
        for (a, b) in [(800.0, -800.0), (1e6, 1e6 - 1.0), (-1e6, 3.0)] {
            let y = adaa(&Tanh, a, f(a), b, f(b));
            assert!(y.is_finite() && y.abs() <= 1.0, "{a},{b}: {y}");
        }
    }

    #[test]
    fn silence_drains_every_history_before_the_fast_path() {
        let kernel = Kernel::new();
        let mut stage = Oversampler::default();
        for n in 0..300 {
            stage.process(((n as f64) * 0.7).sin(), 30.0, &kernel, &Tanh);
        }
        for _ in 0..DRAIN_FRAMES {
            stage.process(0.0, 30.0, &kernel, &Tanh);
        }
        // The skipped path is only taken once the computed path is all zero.
        assert!(stage.is_quiescent());
        assert_eq!(stage.process(0.0, 30.0, &kernel, &Tanh), 0.0);
    }

    #[test]
    fn small_signal_passes_with_unity_gain_and_declared_delay() {
        // A unit impulse at low drive is linear: its centroid is the
        // round-trip delay, and its sum is the DC gain.
        let kernel = Kernel::new();
        let mut stage = Oversampler::default();
        let drive = 1.0e-3;
        let response: Vec<f64> = (0..64)
            .map(|n| stage.process(if n == 0 { 1.0 } else { 0.0 }, drive, &kernel, &Tanh) / drive)
            .collect();
        let sum: f64 = response.iter().sum();
        let centroid: f64 = response
            .iter()
            .enumerate()
            .map(|(n, y)| n as f64 * y)
            .sum::<f64>()
            / sum;
        assert!((sum - 1.0).abs() < 1e-6, "dc gain {sum}");
        assert!(
            (centroid - 88.5 / FACTOR as f64).abs() < 1e-3,
            "delay {centroid}"
        );
        assert_eq!(centroid.round() as usize, LATENCY_FRAMES);
    }
}
