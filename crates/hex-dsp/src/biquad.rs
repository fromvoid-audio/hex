//! Validated safe-Rust Transposed Direct Form II biquad filters.
//!
//! Provides RBJ audio-EQ cookbook, Butterworth, and fourth-order Linkwitz–Riley
//! (LR4) coefficient constructors, exact frequency-response evaluation, and
//! inline TDF-II processing with subnormal flushing.

use crate::denormal::{flush_below, flush_quiet, flush_quiet_pair};
use std::f64::consts::{PI, SQRT_2, TAU};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidBiquad;

/// Normalized second-order IIR coefficients (`a0 = 1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiquadCoefficients {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Default for BiquadCoefficients {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl BiquadCoefficients {
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// Construct validated normalized coefficients (all finite).
    pub fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Result<Self, InvalidBiquad> {
        let result = Self { b0, b1, b2, a1, a2 };
        if [result.b0, result.b1, result.b2, result.a1, result.a2]
            .into_iter()
            .all(f64::is_finite)
        {
            Ok(result)
        } else {
            Err(InvalidBiquad)
        }
    }

    /// Normalize raw transfer coefficients by `a0` and verify all outputs are finite.
    pub fn normalized(
        b0: f64,
        b1: f64,
        b2: f64,
        a0: f64,
        a1: f64,
        a2: f64,
    ) -> Result<Self, InvalidBiquad> {
        if !a0.is_finite() || a0.abs() < f64::MIN_POSITIVE {
            return Err(InvalidBiquad);
        }
        Self::new(b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
    }

    fn validate_frequency(
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<(f64, f64), InvalidBiquad> {
        if !sample_rate.is_finite()
            || sample_rate <= 0.0
            || !frequency_hz.is_finite()
            || frequency_hz <= 0.0
            || frequency_hz >= sample_rate * 0.5
        {
            return Err(InvalidBiquad);
        }
        let omega = TAU * frequency_hz / sample_rate;
        Ok((omega.cos(), omega.sin()))
    }

    fn validate_q(q: f64) -> Result<(), InvalidBiquad> {
        if q.is_finite() && q > 0.0 {
            Ok(())
        } else {
            Err(InvalidBiquad)
        }
    }

    /// RBJ second-order low-pass filter.
    pub fn low_pass(frequency_hz: f64, sample_rate: f64, q: f64) -> Result<Self, InvalidBiquad> {
        Self::validate_q(q)?;
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let alpha = sine / (2.0 * q);
        Self::normalized(
            (1.0 - cosine) * 0.5,
            1.0 - cosine,
            (1.0 - cosine) * 0.5,
            1.0 + alpha,
            -2.0 * cosine,
            1.0 - alpha,
        )
    }

    /// RBJ second-order high-pass filter.
    pub fn high_pass(frequency_hz: f64, sample_rate: f64, q: f64) -> Result<Self, InvalidBiquad> {
        Self::validate_q(q)?;
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let alpha = sine / (2.0 * q);
        Self::normalized(
            (1.0 + cosine) * 0.5,
            -(1.0 + cosine),
            (1.0 + cosine) * 0.5,
            1.0 + alpha,
            -2.0 * cosine,
            1.0 - alpha,
        )
    }

    /// RBJ constant-0 dB-peak band-pass filter.
    pub fn band_pass_peak(
        frequency_hz: f64,
        sample_rate: f64,
        q: f64,
    ) -> Result<Self, InvalidBiquad> {
        Self::validate_q(q)?;
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let alpha = sine / (2.0 * q);
        Self::normalized(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cosine, 1.0 - alpha)
    }

    /// RBJ constant-skirt-gain band-pass filter (peak gain = Q).
    pub fn band_pass_skirt(
        frequency_hz: f64,
        sample_rate: f64,
        q: f64,
    ) -> Result<Self, InvalidBiquad> {
        Self::validate_q(q)?;
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let alpha = sine / (2.0 * q);
        let half_sin = sine * 0.5;
        Self::normalized(
            half_sin,
            0.0,
            -half_sin,
            1.0 + alpha,
            -2.0 * cosine,
            1.0 - alpha,
        )
    }

    /// RBJ notch (band-stop) filter.
    pub fn notch(frequency_hz: f64, sample_rate: f64, q: f64) -> Result<Self, InvalidBiquad> {
        Self::validate_q(q)?;
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let alpha = sine / (2.0 * q);
        Self::normalized(
            1.0,
            -2.0 * cosine,
            1.0,
            1.0 + alpha,
            -2.0 * cosine,
            1.0 - alpha,
        )
    }

    /// RBJ second-order all-pass filter.
    pub fn all_pass(frequency_hz: f64, sample_rate: f64, q: f64) -> Result<Self, InvalidBiquad> {
        Self::validate_q(q)?;
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let alpha = sine / (2.0 * q);
        Self::normalized(
            1.0 - alpha,
            -2.0 * cosine,
            1.0 + alpha,
            1.0 + alpha,
            -2.0 * cosine,
            1.0 - alpha,
        )
    }

    /// RBJ peaking EQ filter.
    pub fn peaking(
        frequency_hz: f64,
        sample_rate: f64,
        q: f64,
        gain_db: f64,
    ) -> Result<Self, InvalidBiquad> {
        Self::validate_q(q)?;
        if !gain_db.is_finite() {
            return Err(InvalidBiquad);
        }
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let a = 10.0_f64.powf(gain_db / 40.0);
        let alpha = sine / (2.0 * q);
        Self::normalized(
            1.0 + alpha * a,
            -2.0 * cosine,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cosine,
            1.0 - alpha / a,
        )
    }

    /// RBJ low-shelf filter parameterized directly by `Q` (`alpha = sin(w0) / (2 * Q)`).
    pub fn low_shelf_q(
        frequency_hz: f64,
        sample_rate: f64,
        q: f64,
        gain_db: f64,
    ) -> Result<Self, InvalidBiquad> {
        Self::validate_q(q)?;
        if !gain_db.is_finite() {
            return Err(InvalidBiquad);
        }
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let a = 10.0_f64.powf(gain_db / 40.0);
        let alpha = sine / (2.0 * q);
        Self::shelf_from_alpha(cosine, a, alpha, true)
    }

    /// RBJ high-shelf filter parameterized directly by `Q` (`alpha = sin(w0) / (2 * Q)`).
    pub fn high_shelf_q(
        frequency_hz: f64,
        sample_rate: f64,
        q: f64,
        gain_db: f64,
    ) -> Result<Self, InvalidBiquad> {
        Self::validate_q(q)?;
        if !gain_db.is_finite() {
            return Err(InvalidBiquad);
        }
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let a = 10.0_f64.powf(gain_db / 40.0);
        let alpha = sine / (2.0 * q);
        Self::shelf_from_alpha(cosine, a, alpha, false)
    }

    /// RBJ low-shelf filter parameterized by shelf slope `S`.
    pub fn low_shelf_slope(
        frequency_hz: f64,
        sample_rate: f64,
        slope: f64,
        gain_db: f64,
    ) -> Result<Self, InvalidBiquad> {
        Self::validate_q(slope)?;
        if !gain_db.is_finite() {
            return Err(InvalidBiquad);
        }
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let a = 10.0_f64.powf(gain_db / 40.0);
        let radicand = (a + 1.0 / a) * (1.0 / slope - 1.0) + 2.0;
        if !radicand.is_finite() || radicand < 0.0 {
            return Err(InvalidBiquad);
        }
        let alpha = sine * 0.5 * radicand.sqrt();
        Self::shelf_from_alpha(cosine, a, alpha, true)
    }

    /// RBJ high-shelf filter parameterized by shelf slope `S`.
    pub fn high_shelf_slope(
        frequency_hz: f64,
        sample_rate: f64,
        slope: f64,
        gain_db: f64,
    ) -> Result<Self, InvalidBiquad> {
        Self::validate_q(slope)?;
        if !gain_db.is_finite() {
            return Err(InvalidBiquad);
        }
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let a = 10.0_f64.powf(gain_db / 40.0);
        let radicand = (a + 1.0 / a) * (1.0 / slope - 1.0) + 2.0;
        if !radicand.is_finite() || radicand < 0.0 {
            return Err(InvalidBiquad);
        }
        let alpha = sine * 0.5 * radicand.sqrt();
        Self::shelf_from_alpha(cosine, a, alpha, false)
    }

    fn shelf_from_alpha(cosine: f64, a: f64, alpha: f64, low: bool) -> Result<Self, InvalidBiquad> {
        let beta = 2.0 * a.sqrt() * alpha;
        if low {
            Self::normalized(
                a * ((a + 1.0) - (a - 1.0) * cosine + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cosine),
                a * ((a + 1.0) - (a - 1.0) * cosine - beta),
                (a + 1.0) + (a - 1.0) * cosine + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * cosine),
                (a + 1.0) + (a - 1.0) * cosine - beta,
            )
        } else {
            Self::normalized(
                a * ((a + 1.0) + (a - 1.0) * cosine + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cosine),
                a * ((a + 1.0) + (a - 1.0) * cosine - beta),
                (a + 1.0) - (a - 1.0) * cosine + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * cosine),
                (a + 1.0) - (a - 1.0) * cosine - beta,
            )
        }
    }

    /// Second-order Butterworth low-pass filter (`Q = 1 / sqrt(2)`).
    pub fn butterworth_low_pass(
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<Self, InvalidBiquad> {
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let alpha = sine / SQRT_2;
        let norm = 1.0 / (1.0 + alpha);
        let b0 = (1.0 - cosine) * 0.5 * norm;
        Self::new(b0, 2.0 * b0, b0, -2.0 * cosine * norm, (1.0 - alpha) * norm)
    }

    /// Second-order Butterworth high-pass filter (`Q = 1 / sqrt(2)`).
    pub fn butterworth_high_pass(
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<Self, InvalidBiquad> {
        let (cosine, sine) = Self::validate_frequency(frequency_hz, sample_rate)?;
        let alpha = sine / SQRT_2;
        let norm = 1.0 / (1.0 + alpha);
        let b0 = (1.0 + cosine) * 0.5 * norm;
        Self::new(
            b0,
            -2.0 * b0,
            b0,
            -2.0 * cosine * norm,
            (1.0 - alpha) * norm,
        )
    }

    /// Section Q for stage `stage_index` (`0..stages`) of an even-order
    /// `2 * stages`-pole Butterworth cascade.
    pub fn butterworth_section_q(stages: usize, stage_index: usize) -> Result<f64, InvalidBiquad> {
        if stages == 0 || stage_index >= stages {
            return Err(InvalidBiquad);
        }
        let angle = (2 * stage_index + 1) as f64 * PI / (4 * stages) as f64;
        let q = 1.0 / (2.0 * angle.cos());
        if q.is_finite() && q > 0.0 {
            Ok(q)
        } else {
            Err(InvalidBiquad)
        }
    }

    /// Fourth-order Linkwitz–Riley (LR4) low-pass cascade (two identical 2-pole Butterworth stages).
    pub fn linkwitz_riley_4_low_pass(
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<[Self; 2], InvalidBiquad> {
        let stage = Self::butterworth_low_pass(frequency_hz, sample_rate)?;
        Ok([stage, stage])
    }

    /// Fourth-order Linkwitz–Riley (LR4) high-pass cascade (two identical 2-pole Butterworth stages).
    pub fn linkwitz_riley_4_high_pass(
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<[Self; 2], InvalidBiquad> {
        let stage = Self::butterworth_high_pass(frequency_hz, sample_rate)?;
        Ok([stage, stage])
    }

    /// Evaluate linear transfer magnitude `|H(e^{j * omega})|`.
    pub fn magnitude_at_omega(self, omega: f64) -> f64 {
        let (sin, cos) = omega.sin_cos();
        let (sin2, cos2) = (2.0 * omega).sin_cos();
        let real_numerator = self.b0 + self.b1 * cos + self.b2 * cos2;
        let imaginary_numerator = -self.b1 * sin - self.b2 * sin2;
        let real_denominator = 1.0 + self.a1 * cos + self.a2 * cos2;
        let imaginary_denominator = -self.a1 * sin - self.a2 * sin2;
        let numerator = real_numerator.hypot(imaginary_numerator);
        let denominator = real_denominator.hypot(imaginary_denominator);
        if denominator > f64::MIN_POSITIVE {
            numerator / denominator
        } else {
            f64::MAX
        }
    }

    /// Evaluate linear transfer magnitude at `frequency_hz` for `sample_rate`.
    pub fn magnitude_at_hz(
        self,
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<f64, InvalidBiquad> {
        if !sample_rate.is_finite()
            || sample_rate <= 0.0
            || !frequency_hz.is_finite()
            || frequency_hz < 0.0
            || frequency_hz > sample_rate * 0.5
        {
            return Err(InvalidBiquad);
        }
        Ok(self.magnitude_at_omega(TAU * frequency_hz / sample_rate))
    }

    /// Linearly advance each coefficient by `(target - current) / divisor`.
    #[inline]
    pub fn step_toward(&mut self, target: Self, divisor: f64) {
        self.b0 += (target.b0 - self.b0) / divisor;
        self.b1 += (target.b1 - self.b1) / divisor;
        self.b2 += (target.b2 - self.b2) / divisor;
        self.a1 += (target.a1 - self.a1) / divisor;
        self.a2 += (target.a2 - self.a2) / divisor;
    }

    /// Process one sample through Transposed Direct Form II using [`flush_quiet`].
    #[inline]
    pub fn process_tdf2(self, input: f64, state: &mut [f64; 2]) -> f64 {
        let output = self.b0 * input + state[0];
        state[0] = flush_quiet((self.b1 * input + state[1]) - self.a1 * output);
        state[1] = flush_quiet(self.b2 * input - self.a2 * output);
        flush_quiet(output)
    }

    /// Process one sample through Transposed Direct Form II with a caller-specified floor.
    #[inline]
    pub fn process_tdf2_with_floor(self, input: f64, state: &mut [f64; 2], floor: f64) -> f64 {
        let output = self.b0 * input + state[0];
        state[0] = flush_below(self.b1 * input - self.a1 * output + state[1], floor);
        state[1] = flush_below(self.b2 * input - self.a2 * output, floor);
        flush_below(output, floor)
    }

    /// Process one sample through Transposed Direct Form II, clearing both state
    /// elements together via [`flush_quiet_pair`] to avoid high-pass limit cycles.
    #[inline]
    pub fn process_tdf2_paired_flush(self, input: f64, state: &mut [f64; 2]) -> f64 {
        let output = self.b0 * input + state[0];
        *state = [
            self.b1 * input - self.a1 * output + state[1],
            self.b2 * input - self.a2 * output,
        ];
        flush_quiet_pair(state);
        output
    }
}

/// Inline Transposed Direct Form II biquad filter owning its coefficients and state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biquad {
    coefficients: BiquadCoefficients,
    state: [f64; 2],
}

impl Default for Biquad {
    fn default() -> Self {
        Self::identity()
    }
}

impl Biquad {
    pub const fn identity() -> Self {
        Self {
            coefficients: BiquadCoefficients::IDENTITY,
            state: [0.0; 2],
        }
    }

    pub const fn new(coefficients: BiquadCoefficients) -> Self {
        Self {
            coefficients,
            state: [0.0; 2],
        }
    }

    pub fn low_pass(frequency_hz: f64, sample_rate: f64, q: f64) -> Result<Self, InvalidBiquad> {
        Ok(Self::new(BiquadCoefficients::low_pass(
            frequency_hz,
            sample_rate,
            q,
        )?))
    }

    pub fn high_pass(frequency_hz: f64, sample_rate: f64, q: f64) -> Result<Self, InvalidBiquad> {
        Ok(Self::new(BiquadCoefficients::high_pass(
            frequency_hz,
            sample_rate,
            q,
        )?))
    }

    pub fn butterworth_low_pass(
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<Self, InvalidBiquad> {
        Ok(Self::new(BiquadCoefficients::butterworth_low_pass(
            frequency_hz,
            sample_rate,
        )?))
    }

    pub fn butterworth_high_pass(
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<Self, InvalidBiquad> {
        Ok(Self::new(BiquadCoefficients::butterworth_high_pass(
            frequency_hz,
            sample_rate,
        )?))
    }

    pub fn peaking(
        frequency_hz: f64,
        sample_rate: f64,
        q: f64,
        gain_db: f64,
    ) -> Result<Self, InvalidBiquad> {
        Ok(Self::new(BiquadCoefficients::peaking(
            frequency_hz,
            sample_rate,
            q,
            gain_db,
        )?))
    }

    pub fn notch(frequency_hz: f64, sample_rate: f64, q: f64) -> Result<Self, InvalidBiquad> {
        Ok(Self::new(BiquadCoefficients::notch(
            frequency_hz,
            sample_rate,
            q,
        )?))
    }

    pub fn low_shelf_q(
        frequency_hz: f64,
        sample_rate: f64,
        q: f64,
        gain_db: f64,
    ) -> Result<Self, InvalidBiquad> {
        Ok(Self::new(BiquadCoefficients::low_shelf_q(
            frequency_hz,
            sample_rate,
            q,
            gain_db,
        )?))
    }

    pub fn high_shelf_q(
        frequency_hz: f64,
        sample_rate: f64,
        q: f64,
        gain_db: f64,
    ) -> Result<Self, InvalidBiquad> {
        Ok(Self::new(BiquadCoefficients::high_shelf_q(
            frequency_hz,
            sample_rate,
            q,
            gain_db,
        )?))
    }

    pub fn linkwitz_riley_4_low_pass(
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<[Self; 2], InvalidBiquad> {
        let [a, b] = BiquadCoefficients::linkwitz_riley_4_low_pass(frequency_hz, sample_rate)?;
        Ok([Self::new(a), Self::new(b)])
    }

    pub fn linkwitz_riley_4_high_pass(
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<[Self; 2], InvalidBiquad> {
        let [a, b] = BiquadCoefficients::linkwitz_riley_4_high_pass(frequency_hz, sample_rate)?;
        Ok([Self::new(a), Self::new(b)])
    }

    pub const fn coefficients(self) -> BiquadCoefficients {
        self.coefficients
    }

    pub fn set_coefficients(&mut self, coefficients: BiquadCoefficients) {
        self.coefficients = coefficients;
    }

    pub const fn state(self) -> [f64; 2] {
        self.state
    }

    pub fn reset(&mut self) {
        self.state = [0.0; 2];
    }

    /// Return a copy preserving coefficients with zeroed signal memory.
    pub const fn without_state(self) -> Self {
        Self {
            coefficients: self.coefficients,
            state: [0.0; 2],
        }
    }

    pub const fn is_quiet(self) -> bool {
        self.state[0] == 0.0 && self.state[1] == 0.0
    }

    #[inline]
    pub fn process(&mut self, input: f64) -> f64 {
        self.coefficients.process_tdf2(input, &mut self.state)
    }

    #[inline]
    pub fn process_paired_flush(&mut self, input: f64) -> f64 {
        self.coefficients
            .process_tdf2_paired_flush(input, &mut self.state)
    }

    pub fn magnitude_at_omega(self, omega: f64) -> f64 {
        self.coefficients.magnitude_at_omega(omega)
    }

    pub fn magnitude_at_hz(
        self,
        frequency_hz: f64,
        sample_rate: f64,
    ) -> Result<f64, InvalidBiquad> {
        self.coefficients.magnitude_at_hz(frequency_hz, sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dft_magnitude(impulse: &[f64], frequency_hz: f64, sample_rate: f64) -> f64 {
        let omega = TAU * frequency_hz / sample_rate;
        let (sin, cos) = omega.sin_cos();
        let (mut x, mut y, mut re, mut im) = (1.0, 0.0, 0.0, 0.0);
        for &sample in impulse {
            re += sample * x;
            im += sample * y;
            (x, y) = (x * cos + y * sin, y * cos - x * sin);
        }
        re.hypot(im)
    }

    #[test]
    fn rbj_constructors_match_impulse_dft_and_frequency_response() {
        let rate = 48_000.0;
        let q_butterworth = std::f64::consts::FRAC_1_SQRT_2;
        let filters = [
            Biquad::low_pass(1_000.0, rate, q_butterworth).unwrap(),
            Biquad::high_pass(1_000.0, rate, 0.9).unwrap(),
            Biquad::peaking(2_000.0, rate, 2.0, 6.0).unwrap(),
            Biquad::notch(1_500.0, rate, 4.0).unwrap(),
            Biquad::low_shelf_q(400.0, rate, q_butterworth, 9.0).unwrap(),
            Biquad::high_shelf_q(4_000.0, rate, q_butterworth, -6.0).unwrap(),
            Biquad::new(BiquadCoefficients::low_shelf_slope(400.0, rate, 1.0, 6.0).unwrap()),
            Biquad::new(BiquadCoefficients::high_shelf_slope(4_000.0, rate, 1.0, -6.0).unwrap()),
            Biquad::new(BiquadCoefficients::band_pass_peak(1_200.0, rate, 1.5).unwrap()),
            Biquad::new(BiquadCoefficients::all_pass(1_000.0, rate, 1.0).unwrap()),
        ];

        for mut filter in filters {
            let impulse: Vec<f64> = (0..4096)
                .map(|i| filter.process(if i == 0 { 1.0 } else { 0.0 }))
                .collect();
            for hz in [100.0, 400.0, 1_000.0, 2_000.0, 6_000.0] {
                let analytical = filter.magnitude_at_hz(hz, rate).unwrap();
                let measured = dft_magnitude(&impulse, hz, rate);
                assert!(
                    (analytical - measured).abs() < 1e-4 * (1.0 + analytical),
                    "hz={hz}: analytical={analytical} vs dft={measured}"
                );
            }
        }
    }

    #[test]
    fn lr4_crossover_has_minus_6_db_crossover_and_flat_magnitude_sum() {
        let rate = 48_000.0;
        let cutoff = 250.0;
        let mut lp = Biquad::linkwitz_riley_4_low_pass(cutoff, rate).unwrap();
        let mut hp = Biquad::linkwitz_riley_4_high_pass(cutoff, rate).unwrap();

        let lp_at_fc = lp[0].magnitude_at_hz(cutoff, rate).unwrap()
            * lp[1].magnitude_at_hz(cutoff, rate).unwrap();
        let hp_at_fc = hp[0].magnitude_at_hz(cutoff, rate).unwrap()
            * hp[1].magnitude_at_hz(cutoff, rate).unwrap();
        assert!((lp_at_fc - 0.5).abs() < 1e-12);
        assert!((hp_at_fc - 0.5).abs() < 1e-12);

        // LR4 LP + HP sums to an all-pass with unity magnitude everywhere.
        let sum_impulse: Vec<f64> = (0..8192)
            .map(|i| {
                let x = if i == 0 { 1.0 } else { 0.0 };
                let lo0 = lp[0].process(x);
                let lo = lp[1].process(lo0);
                let hi0 = hp[0].process(x);
                let hi = hp[1].process(hi0);
                lo + hi
            })
            .collect();
        for hz in [40.0, 120.0, 250.0, 1_000.0, 8_000.0] {
            let mag = dft_magnitude(&sum_impulse, hz, rate);
            assert!((mag - 1.0).abs() < 5e-4, "hz={hz}: sum mag={mag}");
        }
    }

    #[test]
    fn rejects_invalid_inputs_and_drains_silence_to_quiet() {
        assert_eq!(Biquad::low_pass(0.0, 48_000.0, 0.7), Err(InvalidBiquad));
        assert_eq!(
            Biquad::low_pass(24_000.0, 48_000.0, 0.7),
            Err(InvalidBiquad)
        );
        assert_eq!(Biquad::low_pass(1_000.0, 0.0, 0.7), Err(InvalidBiquad));
        assert_eq!(Biquad::low_pass(1_000.0, 48_000.0, 0.0), Err(InvalidBiquad));
        assert_eq!(
            Biquad::peaking(1_000.0, 48_000.0, 1.0, f64::NAN),
            Err(InvalidBiquad)
        );
        assert_eq!(
            BiquadCoefficients::butterworth_section_q(0, 0),
            Err(InvalidBiquad)
        );
        assert_eq!(
            BiquadCoefficients::butterworth_section_q(2, 2),
            Err(InvalidBiquad)
        );

        let mut hp = Biquad::butterworth_high_pass(200.0, 48_000.0).unwrap();
        let _ = hp.process_paired_flush(1.0);
        assert!(!hp.is_quiet());
        for _ in 0..8_000 {
            let _ = hp.process_paired_flush(0.0);
        }
        assert!(hp.is_quiet());
    }
}
