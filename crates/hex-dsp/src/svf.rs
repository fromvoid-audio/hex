//! Topology-Preserving Transform (TPT) state-variable filters.
//!
//! Promoted from `hex-devices::operator_filter` into `hex-dsp` as a shared,
//! strictly validated, allocation-free filter primitive supporting both
//! single-stage TPT SVF sections and two-stage 12/24 dB morphing filters.

use crate::denormal::{flush_quiet, flush_quiet_f32};
use std::f64::consts::{FRAC_1_SQRT_2, PI};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSvf;

/// Output mode for a multi-mode [`SvfFrame`] / [`SvfFilter`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvfMode {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Morph,
}

impl SvfMode {
    pub const fn from_index(mode: u32) -> Result<Self, InvalidSvf> {
        match mode {
            0 => Ok(Self::LowPass),
            1 => Ok(Self::HighPass),
            2 => Ok(Self::BandPass),
            3 => Ok(Self::Notch),
            4 => Ok(Self::Morph),
            _ => Err(InvalidSvf),
        }
    }
}

/// Simultaneous outputs of a single-stage TPT state-variable filter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvfOutputs {
    pub low: f64,
    pub band: f64,
    pub high: f64,
}

/// Single-stage double-precision TPT state-variable filter integrator pair.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvfSection {
    state: [f64; 2],
}

impl SvfSection {
    pub const fn new() -> Self {
        Self { state: [0.0; 2] }
    }

    pub fn reset(&mut self) {
        self.state = [0.0; 2];
    }

    pub const fn state(self) -> [f64; 2] {
        self.state
    }

    pub const fn is_clear(self) -> bool {
        self.state[0] == 0.0 && self.state[1] == 0.0
    }

    /// Process one sample with prewarped cutoff `g = tan(pi * fc / fs)` and
    /// damping `k = 1 / Q`, flushing subnormal integrator states via [`flush_quiet`].
    #[inline]
    pub fn process_outputs(&mut self, input: f64, g: f64, k: f64) -> SvfOutputs {
        let norm = 1.0 / (1.0 + g * (g + k));
        self.process_outputs_precomputed(input, g, k, norm)
    }

    /// Process one sample with precomputed `norm = 1 / (1 + g * (g + k))`.
    #[inline]
    pub fn process_outputs_precomputed(
        &mut self,
        input: f64,
        g: f64,
        k: f64,
        norm: f64,
    ) -> SvfOutputs {
        let [s1, s2] = self.state;
        let high = (input - (k + g) * s1 - s2) * norm;
        let band = g * high + s1;
        let low = g * band + s2;
        self.state = [flush_quiet(g * high + band), flush_quiet(g * band + low)];
        SvfOutputs { low, band, high }
    }
}

/// Single-precision TPT state-variable section coefficients for bounded
/// real-time ramps (e.g., reverb input high/low-pass filters).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvfSectionF32 {
    a1: f32,
    a2: f32,
    a3: f32,
    damping: f32,
}

impl SvfSectionF32 {
    /// Construct validated `f32` TPT SVF coefficients from prewarped `g >= 0` and `damping > 0`.
    pub fn new(g: f32, damping: f32) -> Result<Self, InvalidSvf> {
        if !g.is_finite() || g < 0.0 || !damping.is_finite() || damping <= 0.0 {
            return Err(InvalidSvf);
        }
        Ok(Self::from_validated(g, damping))
    }

    /// Construct directly from already-validated `g` and `damping`.
    #[inline]
    pub fn from_validated(g: f32, damping: f32) -> Self {
        let a1 = 1.0 / (1.0 + g * (g + damping));
        let a2 = g * a1;
        Self {
            a1,
            a2,
            a3: g * a2,
            damping,
        }
    }

    /// Apply one sample to an `f32` integrator pair, flushing subnormals and
    /// bounding recursive state to `[-bound, bound]`.
    #[inline]
    pub fn apply_bounded(self, state: &mut [f32; 2], x: f32, high: bool, bound: f32) -> f32 {
        let clean = |value: f32| {
            if value.is_nan() {
                0.0
            } else {
                flush_quiet_f32(value).clamp(-bound, bound)
            }
        };
        let v3 = x - state[1];
        let v1 = self.a1 * state[0] + self.a2 * v3;
        let v2 = state[1] + self.a2 * state[0] + self.a3 * v3;
        state[0] = clean(2.0 * v1 - state[0]);
        state[1] = clean(2.0 * v2 - state[1]);
        clean(if high { x - self.damping * v1 - v2 } else { v2 })
    }
}

/// Prepared TPT SVF coefficient frame for one or two cascaded 2-pole sections
/// with continuous wet/dry mix and 12/24 dB cascade blend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvfFrame {
    g: f64,
    k: f64,
    weights: [f64; 3],
    wet: f64,
    cascade: f64,
}

impl SvfFrame {
    /// Validate cutoff, sample rate, resonance (`0..=1`), morph (`0..=1`),
    /// wet (`0..=1`), and cascade (`0..=1`) and prewarp cutoff `g`.
    pub fn new(
        cutoff_hz: f64,
        sample_rate: u32,
        mode: SvfMode,
        resonance: f64,
        morph: f64,
        wet: f64,
        cascade: f64,
    ) -> Result<Self, InvalidSvf> {
        if sample_rate == 0
            || !cutoff_hz.is_finite()
            || cutoff_hz <= 0.0
            || cutoff_hz >= f64::from(sample_rate) * 0.5
        {
            return Err(InvalidSvf);
        }
        let g = (PI * cutoff_hz / f64::from(sample_rate)).tan();
        Self::from_prewarped(g, mode, resonance, morph, wet, cascade)
    }

    /// Construct from a prewarped `g >= 0` and normalized parameters in `0..=1`.
    pub fn from_prewarped(
        g: f64,
        mode: SvfMode,
        resonance: f64,
        morph: f64,
        wet: f64,
        cascade: f64,
    ) -> Result<Self, InvalidSvf> {
        if !g.is_finite()
            || g < 0.0
            || !(0.0..=1.0).contains(&resonance)
            || !(0.0..=1.0).contains(&morph)
            || !(0.0..=1.0).contains(&wet)
            || !(0.0..=1.0).contains(&cascade)
        {
            return Err(InvalidSvf);
        }
        Ok(Self::clean_filter(g, mode, resonance, morph, wet, cascade))
    }

    /// Coefficients for the clean low, high, band, notch and morph sections.
    /// Callers validate authored ranges and prewarp a bounded cutoff.
    pub fn clean_filter(
        g: f64,
        mode: SvfMode,
        resonance: f64,
        morph: f64,
        wet: f64,
        cascade: f64,
    ) -> Self {
        let q = FRAC_1_SQRT_2 + (20.0 - FRAC_1_SQRT_2) * resonance * resonance;
        let weights = match mode {
            SvfMode::LowPass => [1.0, 0.0, 0.0],
            SvfMode::HighPass => [0.0, 0.0, 1.0],
            SvfMode::BandPass => [0.0, 1.0 / q, 0.0],
            SvfMode::Notch => [1.0, 0.0, 1.0],
            SvfMode::Morph if morph <= 0.5 => [1.0 - 2.0 * morph, 2.0 * morph / q, 0.0],
            SvfMode::Morph => [0.0, (2.0 - 2.0 * morph) / q, 2.0 * morph - 1.0],
        };
        Self {
            g,
            k: 1.0 / q,
            weights,
            wet,
            cascade,
        }
    }

    /// Inactive history sentinel; activation installs the prepared target coefficients.
    pub const fn dry() -> Self {
        Self {
            g: 0.0,
            k: 2.0,
            weights: [1.0, 0.0, 0.0],
            wet: 0.0,
            cascade: 1.0,
        }
    }

    pub const fn g(self) -> f64 {
        self.g
    }

    pub const fn k(self) -> f64 {
        self.k
    }

    pub const fn weights(self) -> [f64; 3] {
        self.weights
    }

    pub const fn wet(self) -> f64 {
        self.wet
    }

    pub const fn with_dry(self) -> Self {
        Self { wet: 0.0, ..self }
    }

    pub const fn with_full_wet(self) -> Self {
        Self { wet: 1.0, ..self }
    }

    pub fn with_wet(self, wet: f64) -> Result<Self, InvalidSvf> {
        if !(0.0..=1.0).contains(&wet) {
            return Err(InvalidSvf);
        }
        Ok(Self { wet, ..self })
    }

    pub fn set_wet(&mut self, wet: f64) -> Result<(), InvalidSvf> {
        if !(0.0..=1.0).contains(&wet) {
            return Err(InvalidSvf);
        }
        self.wet = wet;
        Ok(())
    }

    pub const fn cascade(self) -> f64 {
        self.cascade
    }

    /// Shift the prewarped cutoff by `octaves`, clamped to `[20 Hz, 0.45 * rate]`.
    /// Returns `self` unchanged if `rate == 0` or `octaves` is non-finite.
    pub fn with_cutoff_offset(self, octaves: f64, rate: u32) -> Self {
        if rate == 0 || !octaves.is_finite() {
            return self;
        }
        let maximum = PI * 0.45;
        let minimum = (PI * 20.0 / f64::from(rate)).min(maximum);
        let shifted = self.g.atan() * octaves.exp2();
        let angle = if shifted.is_finite() {
            shifted.clamp(minimum, maximum)
        } else {
            maximum
        };
        Self {
            g: angle.tan(),
            ..self
        }
    }

    /// Linearly interpolate between `self` and `to` at `t` in `[0, 1]`.
    pub fn interpolate(self, to: Self, t: f64) -> Self {
        let lerp = |a: f64, b: f64| a + (b - a) * t;
        if t == 0.0 {
            return self;
        }
        if t >= 1.0 {
            return to;
        }
        Self {
            g: lerp(self.g, to.g),
            k: lerp(self.k, to.k),
            weights: std::array::from_fn(|i| lerp(self.weights[i], to.weights[i])),
            wet: lerp(self.wet, to.wet),
            cascade: lerp(self.cascade, to.cascade),
        }
    }

    /// Analytical magnitude response at `frequency_hz` for binary wet/cascade
    /// settings (`wet` in `{0, 1}`, `cascade` in `{0, 1}`).
    pub fn magnitude_at_hz(self, frequency_hz: f64, sample_rate: u32) -> f64 {
        if self.wet == 0.0 {
            return 1.0;
        }
        if sample_rate == 0
            || !frequency_hz.is_finite()
            || frequency_hz < 0.0
            || frequency_hz >= f64::from(sample_rate) * 0.5
            || !self.g.is_finite()
            || self.g <= 0.0
        {
            return 0.0;
        }
        let s = (PI * frequency_hz / f64::from(sample_rate)).tan() / self.g;
        let [low, band, high] = self.weights;
        let numerator = (low - high * s * s).powi(2) + (band * s).powi(2);
        let denominator = (1.0 - s * s).powi(2) + (self.k * s).powi(2);
        if !denominator.is_finite() || denominator <= f64::MIN_POSITIVE {
            return 0.0;
        }
        let squared = numerator / denominator;
        let filtered = if self.cascade == 0.0 {
            squared.sqrt()
        } else {
            squared
        };
        if filtered.is_finite() { filtered } else { 0.0 }
    }
}

/// Two cascaded TPT state-variable filter sections supporting 12 dB and 24 dB
/// slopes, morphing modes, and continuous wet/dry mix.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SvfFilter {
    states: [[f64; 2]; 2],
}

impl SvfFilter {
    pub const fn new() -> Self {
        Self {
            states: [[0.0; 2]; 2],
        }
    }

    pub fn reset(&mut self) {
        self.states = [[0.0; 2]; 2];
    }

    pub fn is_quiet(self) -> bool {
        self.states
            .into_iter()
            .flatten()
            .all(|level| level.abs() < 1e-12)
    }

    #[inline]
    pub fn process(&mut self, input: f64, frame: SvfFrame) -> f64 {
        let mut x = input;
        let mut first = 0.0;
        let a = 1.0 / (1.0 + frame.g * (frame.g + frame.k));
        for (index, state) in self.states.iter_mut().enumerate() {
            let v1 = a * (state[0] + frame.g * (x - state[1]));
            let v2 = state[1] + frame.g * v1;
            let high = x - frame.k * v1 - v2;
            state[0] = flush_quiet(2.0 * v1 - state[0]);
            state[1] = flush_quiet(2.0 * v2 - state[1]);
            x = frame.weights[0] * v2 + frame.weights[1] * v1 + frame.weights[2] * high;
            if index == 0 {
                first = x;
            }
        }
        let filtered = first + (x - first) * frame.cascade;
        input + (filtered - input) * frame.wet
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn svf_filter_magnitude_matches_impulse_dft_across_modes_and_slopes() {
        let rate = 48_000_u32;
        let cutoff = 2_400.0;
        for mode in [
            SvfMode::LowPass,
            SvfMode::HighPass,
            SvfMode::BandPass,
            SvfMode::Notch,
            SvfMode::Morph,
        ] {
            for cascade in [0.0, 1.0] {
                let frame = SvfFrame::new(cutoff, rate, mode, 0.5, 0.35, 1.0, cascade).unwrap();
                let mut filter = SvfFilter::new();
                let impulse: Vec<f64> = (0..4096)
                    .map(|i| filter.process(if i == 0 { 1.0 } else { 0.0 }, frame))
                    .collect();
                for hz in [600.0, cutoff, 7_200.0] {
                    let omega = TAU * hz / f64::from(rate);
                    let (sin, cos) = omega.sin_cos();
                    let (mut x, mut y, mut re, mut im) = (1.0, 0.0, 0.0, 0.0);
                    for &sample in &impulse {
                        re += sample * x;
                        im += sample * y;
                        (x, y) = (x * cos + y * sin, y * cos - x * sin);
                    }
                    let measured = re.hypot(im);
                    let expected = frame.magnitude_at_hz(hz, rate);
                    assert!(
                        (expected - measured).abs() < 5e-5 * (1.0 + measured),
                        "mode={mode:?} cascade={cascade} hz={hz}: expected={expected} vs dft={measured}"
                    );
                }
            }
        }
    }

    #[test]
    fn single_section_preserves_svf_conservation_law_and_validates_inputs() {
        let g = (PI * 1_000.0 / 48_000.0).tan();
        let k = std::f64::consts::SQRT_2;
        let mut section = SvfSection::new();
        for i in 0..64 {
            let x = (i as f64 * 0.13).sin();
            let out = section.process_outputs(x, g, k);
            let reconstructed = out.low + k * out.band + out.high;
            assert!((reconstructed - x).abs() < 1e-12);
        }
        assert!(SvfFrame::new(0.0, 48_000, SvfMode::LowPass, 0.0, 0.0, 1.0, 0.0).is_err());
        assert!(SvfFrame::new(24_000.0, 48_000, SvfMode::LowPass, 0.0, 0.0, 1.0, 0.0).is_err());
        assert!(SvfFrame::new(1_000.0, 48_000, SvfMode::LowPass, 1.5, 0.0, 1.0, 0.0).is_err());
        assert!(SvfSectionF32::new(-1.0, 1.0).is_err());
        assert!(SvfSectionF32::new(0.1, 0.0).is_err());
    }

    #[test]
    fn svf_frame_guards_low_rates_zero_g_and_wet_validation() {
        let frame = SvfFrame::new(1_000.0, 48_000, SvfMode::LowPass, 0.25, 0.0, 1.0, 0.0).unwrap();
        // Low / zero rates and extreme/NaN octave shifts must never panic or yield NaN.
        for rate in [0_u32, 1, 20, 44, 45, 48_000] {
            for octaves in [-10.0, 0.0, 10.0, 2_000.0, f64::NAN, f64::INFINITY] {
                let shifted = frame.with_cutoff_offset(octaves, rate);
                assert!(shifted.g().is_finite() && shifted.g() >= 0.0);
            }
        }
        // Zero g or zero/invalid sample_rate in magnitude_at_hz must never divide by zero or return NaN.
        let zero_g_wet = SvfFrame::dry().with_full_wet();
        assert_eq!(zero_g_wet.magnitude_at_hz(1_000.0, 48_000), 0.0);
        assert_eq!(frame.magnitude_at_hz(1_000.0, 0), 0.0);
        assert_eq!(frame.magnitude_at_hz(f64::NAN, 48_000), 0.0);
        assert_eq!(frame.magnitude_at_hz(-10.0, 48_000), 0.0);
        assert_eq!(frame.magnitude_at_hz(24_000.0, 48_000), 0.0);

        // Wet setter enforces 0..=1.
        let mut mutable = frame;
        assert!(mutable.with_wet(-0.1).is_err());
        assert!(mutable.with_wet(1.1).is_err());
        assert!(mutable.with_wet(f64::NAN).is_err());
        assert!(mutable.set_wet(2.0).is_err());
        assert_eq!(mutable.wet(), 1.0);
        mutable.set_wet(0.5).unwrap();
        assert_eq!(mutable.wet(), 0.5);
        assert_eq!(mutable.with_dry().wet(), 0.0);
    }
}
