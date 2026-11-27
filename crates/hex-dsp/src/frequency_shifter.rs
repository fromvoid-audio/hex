//! Bounded, allocation-free stereo frequency and ring shift DSP kernel.
//!
//! Provides two provider-independent processing modes with explicit units:
//! - [`ShiftMode::Frequency`]: zero-latency single-sideband (SSB) frequency
//!   shifting using an 8+8-section (16th-order) equiripple elliptic Hilbert
//!   all-pass phase-differencing network (`> 60 dB` opposite-sideband rejection
//!   across `20 Hz..23.9 kHz` at `48 kHz`) paired with a quadrature carrier
//!   oscillator. Positive `shift_hz` shifts partials upward (`f -> f + shift`);
//!   negative `shift_hz` shifts partials downward (`f -> f + shift`).
//! - [`ShiftMode::Ring`]: zero-latency, zero-tail bipolar ring modulation
//!   (`y = x * cos(theta)`), producing equal upper and lower sidebands
//!   (`f + |shift|` and `|f - |shift||`) at `-6.02 dB` each.
//!
//! All state is fixed-inline [`Copy`] with `unsafe_code = "forbid"` and zero
//! heap allocations. Parameter changes smooth over 5 ms via [`LinearRamp`].
//! Non-finite inputs never enter the Hilbert filter histories or carrier phase.
//! This module owns no device registration, UI, host clock, MIDI, feedback, or
//! pitch-shifting paths.

use crate::denormal::flush_quiet;
use crate::smoother::{LinearRamp, ramp_frames_for_ms};
use std::f64::consts::TAU;

/// Minimum supported sample rate in Hz.
pub const MIN_SAMPLE_RATE: u32 = 8_000;
/// Maximum supported sample rate in Hz.
pub const MAX_SAMPLE_RATE: u32 = 192_000;
/// Maximum absolute shift or stereo spread magnitude in Hz.
pub const MAX_SHIFT_HZ: f64 = 10_000.0;
const RAMP_MS: f64 = 5.0;
const HILBERT_STAGES: usize = 8;

/// Equiripple elliptic 16th-order (8+8) Hilbert all-pass coefficients (`f_min = 0.0005 * sr`),
/// where branch A cascades `H_k(z) = (a_k - z^-2) / (1 - a_k z^-2)` and branch B
/// cascades `H_k(z)` with a 1-sample delay so `angle(H_B) - angle(H_A) ≈ -90°`
/// across `[0.0005 * sr, 0.4995 * sr]` (`> 62 dB` opposite-sideband rejection).
const HILBERT_A: [f64; HILBERT_STAGES] = [
    0.045_505_247_288_079_566,
    0.326_762_072_417_976_5,
    0.631_075_407_109_083_7,
    0.824_669_184_732_413_5,
    0.922_297_994_504_043_7,
    0.966_802_364_084_486_6,
    0.986_467_630_104_477_5,
    0.995_632_241_489_968_4,
];

const HILBERT_B: [f64; HILBERT_STAGES] = [
    0.166_521_338_385_619_47,
    0.489_495_404_630_733_8,
    0.742_536_655_262_680_3,
    0.882_597_060_057_332,
    0.949_014_083_790_284_2,
    0.978_601_907_847_088_3,
    0.991_816_169_339_483_8,
    0.998_629_038_973_024_9,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidFrequencyShifter {
    SampleRate,
    ShiftHz,
    StereoSpreadHz,
    StereoPhaseDegrees,
    DryWet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftMode {
    /// Single-sideband frequency shift via analytic Hilbert pair.
    Frequency,
    /// Bipolar ring modulation (`x * cos(theta)`).
    Ring,
}

/// Validated control-side parameters with explicit physical units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrequencyShiftParameters {
    pub mode: ShiftMode,
    /// Frequency shift (or ring carrier frequency) in Hz.
    pub shift_hz: f64,
    /// Symmetric stereo frequency offset in Hz (`L = shift + spread`, `R = shift - spread`).
    pub stereo_spread_hz: f64,
    /// Right-channel carrier phase offset relative to left in degrees (`0.0..=360.0`).
    pub stereo_phase_degrees: f64,
    /// Dry/wet mix in `[0.0, 1.0]`.
    pub dry_wet: f64,
}

impl Default for FrequencyShiftParameters {
    fn default() -> Self {
        Self {
            mode: ShiftMode::Frequency,
            shift_hz: 0.0,
            stereo_spread_hz: 0.0,
            stereo_phase_degrees: 0.0,
            dry_wet: 1.0,
        }
    }
}

impl FrequencyShiftParameters {
    pub fn validate(self, sample_rate: u32) -> Result<(), InvalidFrequencyShifter> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(InvalidFrequencyShifter::SampleRate);
        }
        let max_hz = MAX_SHIFT_HZ.min(f64::from(sample_rate) * 0.45);
        if !self.shift_hz.is_finite() || self.shift_hz.abs() > max_hz {
            return Err(InvalidFrequencyShifter::ShiftHz);
        }
        if !self.stereo_spread_hz.is_finite() || self.stereo_spread_hz.abs() > max_hz {
            return Err(InvalidFrequencyShifter::StereoSpreadHz);
        }
        if !self.stereo_phase_degrees.is_finite()
            || !(0.0..=360.0).contains(&self.stereo_phase_degrees)
        {
            return Err(InvalidFrequencyShifter::StereoPhaseDegrees);
        }
        if !self.dry_wet.is_finite() || !(0.0..=1.0).contains(&self.dry_wet) {
            return Err(InvalidFrequencyShifter::DryWet);
        }
        Ok(())
    }
}

/// One 2-delay-tap all-pass section `H(z) = (a - z^-2) / (1 - a z^-2)`.
#[derive(Clone, Copy, Debug, Default)]
struct AllPass2 {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl AllPass2 {
    #[inline]
    fn process(&mut self, input: f64, coeff: f64) -> f64 {
        let out = flush_quiet(coeff * (input + self.y2) - self.x2);
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = out;
        out
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn is_quiet(self) -> bool {
        self.x1 == 0.0 && self.x2 == 0.0 && self.y1 == 0.0 && self.y2 == 0.0
    }
}

/// Single-channel 8+8-section elliptic Hilbert transformer producing an analytic
/// `(in_phase, quadrature)` pair with `angle(Q) - angle(I) ≈ -90°`.
#[derive(Clone, Copy, Debug, Default)]
struct HilbertChannel {
    branch_a: [AllPass2; HILBERT_STAGES],
    branch_b: [AllPass2; HILBERT_STAGES],
    delay_b: f64,
}

impl HilbertChannel {
    #[inline]
    fn process(&mut self, input: f64) -> (f64, f64) {
        let mut a = input;
        for (stage, &coeff) in self.branch_a.iter_mut().zip(&HILBERT_A) {
            a = stage.process(a, coeff);
        }
        let delayed = self.delay_b;
        self.delay_b = input;
        let mut b = delayed;
        for (stage, &coeff) in self.branch_b.iter_mut().zip(&HILBERT_B) {
            b = stage.process(b, coeff);
        }
        (a, b)
    }

    fn reset(&mut self) {
        for stage in &mut self.branch_a {
            stage.reset();
        }
        for stage in &mut self.branch_b {
            stage.reset();
        }
        self.delay_b = 0.0;
    }

    fn is_quiet(&self) -> bool {
        self.delay_b == 0.0
            && self.branch_a.iter().all(|s| s.is_quiet())
            && self.branch_b.iter().all(|s| s.is_quiet())
    }
}

/// Bounded, fixed-inline stereo frequency and ring shift DSP kernel.
#[derive(Clone, Copy, Debug)]
pub struct FrequencyShifter {
    sample_rate: u32,
    ramp_frames: u32,
    mode: ShiftMode,
    shift_hz: LinearRamp,
    stereo_spread_hz: LinearRamp,
    stereo_phase_cycles: LinearRamp,
    dry_wet: LinearRamp,
    phase_cycles: [f64; 2],
    hilbert: [HilbertChannel; 2],
    silent_frames: u32,
    tail_limit: u32,
}

impl FrequencyShifter {
    /// Construct a validated stereo frequency/ring shift kernel.
    pub fn new(
        sample_rate: u32,
        params: FrequencyShiftParameters,
    ) -> Result<Self, InvalidFrequencyShifter> {
        params.validate(sample_rate)?;
        let ramp_frames = ramp_frames_for_ms(sample_rate, RAMP_MS).unwrap_or(1);
        let tail_limit = tail_frames_for_mode(sample_rate, params.mode);
        Ok(Self {
            sample_rate,
            ramp_frames,
            mode: params.mode,
            shift_hz: LinearRamp::settled(params.shift_hz),
            stereo_spread_hz: LinearRamp::settled(params.stereo_spread_hz),
            stereo_phase_cycles: LinearRamp::settled(
                (params.stereo_phase_degrees / 360.0).rem_euclid(1.0),
            ),
            dry_wet: LinearRamp::settled(params.dry_wet),
            phase_cycles: [0.0; 2],
            hilbert: [HilbertChannel::default(); 2],
            silent_frames: tail_limit,
            tail_limit,
        })
    }

    /// Retarget validated parameters with 5 ms smoothing on continuous controls.
    pub fn set_parameters(
        &mut self,
        params: FrequencyShiftParameters,
    ) -> Result<(), InvalidFrequencyShifter> {
        params.validate(self.sample_rate)?;
        if params.mode != self.mode {
            self.mode = params.mode;
            self.tail_limit = tail_frames_for_mode(self.sample_rate, params.mode);
            if params.mode == ShiftMode::Ring {
                for ch in &mut self.hilbert {
                    ch.reset();
                }
                self.silent_frames = 0;
            } else {
                self.silent_frames = self.silent_frames.min(self.tail_limit);
            }
        }
        self.shift_hz.retarget(params.shift_hz, self.ramp_frames);
        self.stereo_spread_hz
            .retarget(params.stereo_spread_hz, self.ramp_frames);
        let next_phase = (params.stereo_phase_degrees / 360.0).rem_euclid(1.0);
        let current_phase = self.stereo_phase_cycles.value().rem_euclid(1.0);
        let mut delta = (next_phase - current_phase + 0.5).rem_euclid(1.0) - 0.5;
        if delta <= -0.5 {
            delta += 1.0;
        }
        self.stereo_phase_cycles.reset(current_phase);
        self.stereo_phase_cycles
            .retarget(current_phase + delta, self.ramp_frames);
        self.dry_wet.retarget(params.dry_wet, self.ramp_frames);
        Ok(())
    }

    /// Clear Hilbert filter histories and carrier phases to deterministic zero state.
    pub fn reset(&mut self) {
        self.phase_cycles = [0.0; 2];
        for ch in &mut self.hilbert {
            ch.reset();
        }
        self.shift_hz.reset(self.shift_hz.target());
        self.stereo_spread_hz.reset(self.stereo_spread_hz.target());
        self.stereo_phase_cycles
            .reset(self.stereo_phase_cycles.target().rem_euclid(1.0));
        self.dry_wet.reset(self.dry_wet.target());
        self.silent_frames = self.tail_limit;
    }

    /// Declared algorithmic latency in frames (zero for IIR Hilbert & Ring modes).
    pub const fn algorithmic_latency_frames(&self) -> u32 {
        0
    }

    /// Finite tail bound in frames after which silence drains to [`Self::is_quiescent`].
    pub const fn max_tail_frames(&self) -> u64 {
        self.tail_limit as u64
    }

    /// True when all internal filter histories are strictly zero.
    pub fn is_quiescent(&self) -> bool {
        self.hilbert.iter().all(HilbertChannel::is_quiet)
    }

    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub const fn mode(&self) -> ShiftMode {
        self.mode
    }

    /// Process one stereo frame in `f64` precision. Non-finite inputs bypass
    /// filter histories and return untouched to preserve upstream fault boundaries.
    #[inline]
    pub fn process_frame(&mut self, input: [f64; 2]) -> [f64; 2] {
        self.process_frame_modulated(input, [0.0, 0.0])
    }

    /// Process one stereo frame with instantaneous per-channel frequency shift
    /// offsets in Hz (e.g., from an LFO or envelope follower), clamped to the
    /// sample-rate-safe frequency limit. Passing `[0.0, 0.0]` is bit-identical
    /// to [`Self::process_frame`].
    #[inline]
    pub fn process_frame_modulated(
        &mut self,
        input: [f64; 2],
        shift_offset_hz: [f64; 2],
    ) -> [f64; 2] {
        let shift_hz = self.shift_hz.advance();
        let spread_hz = self.stereo_spread_hz.advance();
        let phase_offset = self.stereo_phase_cycles.advance();
        let wet = self.dry_wet.advance();

        let sr = f64::from(self.sample_rate);
        let max_hz = MAX_SHIFT_HZ.min(sr * 0.45);
        let offset_l = if shift_offset_hz[0].is_finite() {
            shift_offset_hz[0]
        } else {
            0.0
        };
        let offset_r = if shift_offset_hz[1].is_finite() {
            shift_offset_hz[1]
        } else {
            0.0
        };
        let rate_l = (shift_hz + spread_hz + offset_l).clamp(-max_hz, max_hz);
        let rate_r = (shift_hz - spread_hz + offset_r).clamp(-max_hz, max_hz);
        let theta_l = TAU * self.phase_cycles[0];
        let theta_r = TAU * (self.phase_cycles[1] + phase_offset).rem_euclid(1.0);
        let (sin_l, cos_l) = theta_l.sin_cos();
        let (sin_r, cos_r) = theta_r.sin_cos();

        self.phase_cycles[0] = (self.phase_cycles[0] + rate_l / sr).rem_euclid(1.0);
        self.phase_cycles[1] = (self.phase_cycles[1] + rate_r / sr).rem_euclid(1.0);

        if !input[0].is_finite() || !input[1].is_finite() {
            return input;
        }

        let silent = input[0] == 0.0 && input[1] == 0.0;
        if silent {
            self.silent_frames = self.silent_frames.saturating_add(1).min(self.tail_limit);
        } else {
            self.silent_frames = 0;
        }

        let processed = match self.mode {
            ShiftMode::Frequency => {
                if silent && self.is_quiescent() {
                    [0.0, 0.0]
                } else {
                    let (ia, qa) = self.hilbert[0].process(input[0]);
                    let (ib, qb) = self.hilbert[1].process(input[1]);
                    if silent && self.silent_frames >= self.tail_limit {
                        self.hilbert[0].reset();
                        self.hilbert[1].reset();
                    }
                    [ia * cos_l - qa * sin_l, ib * cos_r - qb * sin_r]
                }
            }
            ShiftMode::Ring => [input[0] * cos_l, input[1] * cos_r],
        };

        if wet == 0.0 {
            input
        } else if wet == 1.0 {
            processed
        } else {
            let dry = 1.0 - wet;
            [
                input[0] * dry + processed[0] * wet,
                input[1] * dry + processed[1] * wet,
            ]
        }
    }

    /// Process a stereo `f32` block in place, bit-identical across arbitrary
    /// block partitions.
    pub fn process_block(&mut self, frames: &mut [[f32; 2]]) {
        for frame in frames {
            let out = self.process_frame([f64::from(frame[0]), f64::from(frame[1])]);
            frame[0] = out[0] as f32;
            frame[1] = out[1] as f32;
        }
    }
}

const fn tail_frames_for_mode(sample_rate: u32, mode: ShiftMode) -> u32 {
    match mode {
        ShiftMode::Frequency => sample_rate.saturating_mul(2),
        ShiftMode::Ring => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dft_magnitude_at(signal: &[f64], hz: f64, sample_rate: f64) -> f64 {
        let n = signal.len();
        let omega = TAU * hz / sample_rate;
        let (mut re, mut im) = (0.0, 0.0);
        // Apply Hann window for clean spectral leakage suppression.
        let mut window_sum = 0.0;
        for (i, &sample) in signal.iter().enumerate() {
            let w = 0.5 * (1.0 - (TAU * i as f64 / n as f64).cos());
            window_sum += w;
            let angle = omega * i as f64;
            re += sample * w * angle.cos();
            im -= sample * w * angle.sin();
        }
        2.0 * re.hypot(im) / window_sum
    }

    #[test]
    fn frequency_mode_shifts_up_and_down_with_over_60db_sideband_rejection() {
        let sr = 48_000_u32;
        let sr_f = f64::from(sr);
        let settle = 2_048;
        let window = 8_192;
        for (input_hz, shift_hz) in [(1_000.0, 250.0), (1_000.0, -250.0), (4_000.0, 750.0)] {
            let mut shifter = FrequencyShifter::new(
                sr,
                FrequencyShiftParameters {
                    mode: ShiftMode::Frequency,
                    shift_hz,
                    ..FrequencyShiftParameters::default()
                },
            )
            .unwrap();
            let mut left = Vec::with_capacity(window);
            for i in 0..(settle + window) {
                let s = (TAU * input_hz * i as f64 / sr_f).sin();
                let out = shifter.process_frame([s, s]);
                if i >= settle {
                    left.push(out[0]);
                }
            }
            let desired_hz = input_hz + shift_hz;
            let unwanted_hz = (input_hz - shift_hz).abs();
            let desired_mag = dft_magnitude_at(&left, desired_hz, sr_f);
            let unwanted_mag = dft_magnitude_at(&left, unwanted_hz, sr_f);
            let rejection_db = 20.0 * (desired_mag / unwanted_mag.max(1e-12)).log10();
            assert!(
                (desired_mag - 1.0).abs() < 0.02,
                "in={input_hz}, shift={shift_hz}: desired_mag={desired_mag}"
            );
            assert!(
                rejection_db > 60.0,
                "in={input_hz}, shift={shift_hz}: sideband rejection {rejection_db:.2} dB < 60 dB"
            );
        }
    }

    #[test]
    fn ring_mode_produces_exact_equal_sidebands_and_suppresses_fundamental() {
        let sr = 48_000_u32;
        let sr_f = f64::from(sr);
        let input_hz = 1_000.0;
        let carrier_hz = 250.0;
        let n = 8_192;
        let mut shifter = FrequencyShifter::new(
            sr,
            FrequencyShiftParameters {
                mode: ShiftMode::Ring,
                shift_hz: carrier_hz,
                ..FrequencyShiftParameters::default()
            },
        )
        .unwrap();
        assert_eq!(shifter.algorithmic_latency_frames(), 0);
        assert_eq!(shifter.max_tail_frames(), 0);
        assert!(shifter.is_quiescent());

        let out: Vec<f64> = (0..n)
            .map(|i| {
                let s = (TAU * input_hz * i as f64 / sr_f).sin();
                shifter.process_frame([s, s])[0]
            })
            .collect();
        let upper = dft_magnitude_at(&out, input_hz + carrier_hz, sr_f);
        let lower = dft_magnitude_at(&out, input_hz - carrier_hz, sr_f);
        let fundamental = dft_magnitude_at(&out, input_hz, sr_f);
        assert!((upper - 0.5).abs() < 1e-3, "upper={upper}");
        assert!((lower - 0.5).abs() < 1e-3, "lower={lower}");
        assert!(fundamental < 1e-4, "fundamental={fundamental}");
    }

    #[test]
    fn stereo_spread_and_phase_offset_separate_channels_independently() {
        let sr = 48_000_u32;
        let sr_f = f64::from(sr);
        let mut spread = FrequencyShifter::new(
            sr,
            FrequencyShiftParameters {
                mode: ShiftMode::Frequency,
                shift_hz: 200.0,
                stereo_spread_hz: 50.0,
                ..FrequencyShiftParameters::default()
            },
        )
        .unwrap();
        let settle = 2_048;
        let window = 8_192;
        let (mut l_out, mut r_out) = (Vec::with_capacity(window), Vec::with_capacity(window));
        for i in 0..(settle + window) {
            let s = (TAU * 1_000.0 * i as f64 / sr_f).sin();
            let [l, r] = spread.process_frame([s, s]);
            if i >= settle {
                l_out.push(l);
                r_out.push(r);
            }
        }
        // Left shifts to 1000 + 200 + 50 = 1250 Hz; Right shifts to 1000 + 200 - 50 = 1150 Hz.
        assert!((dft_magnitude_at(&l_out, 1_250.0, sr_f) - 1.0).abs() < 0.02);
        assert!((dft_magnitude_at(&r_out, 1_150.0, sr_f) - 1.0).abs() < 0.02);
        assert!(dft_magnitude_at(&l_out, 1_150.0, sr_f) < 1e-3);
        assert!(dft_magnitude_at(&r_out, 1_250.0, sr_f) < 1e-3);

        // Stereo phase 180 deg in Ring mode inverts Right relative to Left exactly.
        let mut phase = FrequencyShifter::new(
            sr,
            FrequencyShiftParameters {
                mode: ShiftMode::Ring,
                shift_hz: 300.0,
                stereo_phase_degrees: 180.0,
                ..FrequencyShiftParameters::default()
            },
        )
        .unwrap();
        for i in 0..128 {
            let s = (TAU * 440.0 * i as f64 / sr_f).sin();
            let [l, r] = phase.process_frame([s, s]);
            assert!((l + r).abs() < 1e-12, "frame {i}: l={l}, r={r}");
        }
    }

    #[test]
    fn silence_nonfinite_reset_quiescence_and_irregular_partitions_hold_across_rates() {
        for sr in [8_000_u32, 44_100, 48_000, 96_000, 192_000] {
            let params = FrequencyShiftParameters {
                mode: ShiftMode::Frequency,
                shift_hz: 120.0,
                stereo_spread_hz: 15.0,
                stereo_phase_degrees: 45.0,
                dry_wet: 0.75,
            };
            let mut whole = FrequencyShifter::new(sr, params).unwrap();
            let mut chunked = FrequencyShifter::new(sr, params).unwrap();
            let input: Vec<[f32; 2]> = (0..300)
                .map(|i| {
                    let t = i as f64 / f64::from(sr);
                    [
                        ((TAU * 330.0 * t).sin() * 0.7) as f32,
                        ((TAU * 550.0 * t).cos() * 0.5) as f32,
                    ]
                })
                .collect();
            let mut a = input.clone();
            let mut b = input;
            whole.process_block(&mut a);
            let mut pos = 0;
            for step in [1, 3, 7, 13, 29, 64, 183] {
                let end = (pos + step).min(b.len());
                chunked.process_block(&mut b[pos..end]);
                pos = end;
            }
            assert_eq!(a, b, "sr={sr}: partition mismatch");

            // Non-finite input passes through without poisoning Hilbert histories.
            let bad = whole.process_frame([f64::NAN, f64::INFINITY]);
            assert!(bad[0].is_nan() && bad[1].is_infinite());
            let after = whole.process_frame([0.5, -0.5]);
            assert!(after[0].is_finite() && after[1].is_finite());

            // Reset restores deterministic zero state.
            whole.reset();
            assert!(whole.is_quiescent());
        }

        // Quiescence drain within declared max_tail_frames.
        let mut sh = FrequencyShifter::new(
            48_000,
            FrequencyShiftParameters {
                mode: ShiftMode::Frequency,
                shift_hz: 100.0,
                ..FrequencyShiftParameters::default()
            },
        )
        .unwrap();
        for _ in 0..64 {
            let _ = sh.process_frame([1.0, -1.0]);
        }
        assert!(!sh.is_quiescent());
        for _ in 0..sh.max_tail_frames() {
            let _ = sh.process_frame([0.0, 0.0]);
        }
        assert!(sh.is_quiescent());
    }

    #[test]
    fn parameter_validation_and_inline_size_bounds_hold() {
        assert!(std::mem::size_of::<FrequencyShifter>() <= 2048);
        assert_eq!(
            FrequencyShifter::new(0, FrequencyShiftParameters::default()).unwrap_err(),
            InvalidFrequencyShifter::SampleRate
        );
        assert_eq!(
            FrequencyShifter::new(
                48_000,
                FrequencyShiftParameters {
                    shift_hz: f64::NAN,
                    ..FrequencyShiftParameters::default()
                }
            )
            .unwrap_err(),
            InvalidFrequencyShifter::ShiftHz
        );
        assert_eq!(
            FrequencyShifter::new(
                8_000,
                FrequencyShiftParameters {
                    shift_hz: 4_000.0, // > 0.45 * 8000
                    ..FrequencyShiftParameters::default()
                }
            )
            .unwrap_err(),
            InvalidFrequencyShifter::ShiftHz
        );
        assert_eq!(
            FrequencyShifter::new(
                48_000,
                FrequencyShiftParameters {
                    stereo_phase_degrees: 361.0,
                    ..FrequencyShiftParameters::default()
                }
            )
            .unwrap_err(),
            InvalidFrequencyShifter::StereoPhaseDegrees
        );
        assert_eq!(
            FrequencyShifter::new(
                48_000,
                FrequencyShiftParameters {
                    dry_wet: 1.5,
                    ..FrequencyShiftParameters::default()
                }
            )
            .unwrap_err(),
            InvalidFrequencyShifter::DryWet
        );
    }

    #[test]
    fn modulated_shift_offset_matches_unmodulated_and_shifts_per_channel() {
        let sr = 48_000_u32;
        let sr_f = f64::from(sr);
        let params = FrequencyShiftParameters {
            mode: ShiftMode::Frequency,
            shift_hz: 150.0,
            ..FrequencyShiftParameters::default()
        };
        let mut plain = FrequencyShifter::new(sr, params).unwrap();
        let mut zero_mod = FrequencyShifter::new(sr, params).unwrap();
        for i in 0..256 {
            let s = (TAU * 1_000.0 * i as f64 / sr_f).sin();
            assert_eq!(
                plain.process_frame([s, -s]),
                zero_mod.process_frame_modulated([s, -s], [0.0, 0.0])
            );
        }

        let mut modulated = FrequencyShifter::new(
            sr,
            FrequencyShiftParameters {
                mode: ShiftMode::Frequency,
                shift_hz: 100.0,
                ..FrequencyShiftParameters::default()
            },
        )
        .unwrap();
        let settle = 2_048;
        let window = 8_192;
        let (mut l_out, mut r_out) = (Vec::with_capacity(window), Vec::with_capacity(window));
        for i in 0..(settle + window) {
            let s = (TAU * 1_000.0 * i as f64 / sr_f).sin();
            let [l, r] = modulated.process_frame_modulated([s, s], [200.0, -50.0]);
            if i >= settle {
                l_out.push(l);
                r_out.push(r);
            }
        }
        // Left: 1000 + 100 + 200 = 1300 Hz; Right: 1000 + 100 - 50 = 1050 Hz.
        assert!((dft_magnitude_at(&l_out, 1_300.0, sr_f) - 1.0).abs() < 0.02);
        assert!((dft_magnitude_at(&r_out, 1_050.0, sr_f) - 1.0).abs() < 0.02);
    }
}
