//! A bounded, sample-stepped bipolar modulation oscillator.
//!
//! The caller owns transport policy: phase advances only when samples are rendered;
//! seek, loop and retrigger do not implicitly reset it. A caller may explicitly
//! reset it at a discontinuity. Changing the prepared rate preserves phase.
//! Output is bipolar in [-1, 1]. There is no implicit interpolation or
//! anti-aliasing; rates above Nyquist are refused.

use std::f64::consts::TAU;

/// An invalid sample rate, frequency, tempo, beat division or phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidLfo;

/// Validated phase increment, prepared outside the audio callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LfoRate(f64);

impl LfoRate {
    /// Prepare a free-running frequency in cycles per second.
    pub fn free(sample_rate: f64, frequency_hz: f64) -> Result<Self, InvalidLfo> {
        if !sample_rate.is_finite()
            || sample_rate <= 0.0
            || !frequency_hz.is_finite()
            || frequency_hz < 0.0
            || frequency_hz > sample_rate * 0.5
        {
            return Err(InvalidLfo);
        }
        Ok(Self(frequency_hz / sample_rate))
    }

    /// Scale a validated step and saturate at Nyquist.
    pub fn scaled(self, multiplier: f64) -> Result<Self, InvalidLfo> {
        if !multiplier.is_finite() || multiplier < 0.0 {
            return Err(InvalidLfo);
        }
        Ok(Self((self.0 * multiplier).min(0.5)))
    }

    /// Prepare a tempo-synced rate, where `beats_per_cycle` uses quarter-note beats.
    pub fn tempo(
        sample_rate: f64,
        beats_per_minute: f64,
        beats_per_cycle: f64,
    ) -> Result<Self, InvalidLfo> {
        if !beats_per_minute.is_finite()
            || beats_per_minute <= 0.0
            || !beats_per_cycle.is_finite()
            || beats_per_cycle <= 0.0
        {
            return Err(InvalidLfo);
        }
        Self::free(sample_rate, beats_per_minute / 60.0 / beats_per_cycle)
    }
}

/// Periodic output shape, evaluated at the oscillator's current phase.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LfoWaveform {
    #[default]
    Sine,
    Triangle,
    RisingRamp,
    Square,
}

/// Scalar runtime with constant work and no allocation per output sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModulationLfo {
    phase: f64,
    rate: LfoRate,
    waveform: LfoWaveform,
}

impl ModulationLfo {
    /// Construct a source at a phase in cycles, in the half-open range [0, 1).
    pub fn new(rate: LfoRate, phase: f64) -> Result<Self, InvalidLfo> {
        Self::new_with_waveform(rate, phase, LfoWaveform::Sine)
    }

    /// Construct an explicit shape at a phase in cycles.
    pub fn new_with_waveform(
        rate: LfoRate,
        phase: f64,
        waveform: LfoWaveform,
    ) -> Result<Self, InvalidLfo> {
        if !phase.is_finite() || !(0.0..1.0).contains(&phase) {
            return Err(InvalidLfo);
        }
        Ok(Self {
            phase,
            rate,
            waveform,
        })
    }

    /// Replace a validated rate without resetting phase.
    pub fn set_rate(&mut self, rate: LfoRate) {
        self.rate = rate;
    }

    /// Inherit phase without replacing this source's prepared rate or shape.
    /// The host must first establish that both sources have the same identity.
    pub fn inherit_phase(&mut self, previous: &Self) {
        self.phase = previous.phase;
    }

    /// Explicitly retrigger at a phase in cycles, in the half-open range [0, 1).
    pub fn reset(&mut self, phase: f64) -> Result<(), InvalidLfo> {
        if !phase.is_finite() || !(0.0..1.0).contains(&phase) {
            return Err(InvalidLfo);
        }
        self.phase = phase;
        Ok(())
    }

    /// Return the current sample then advance exactly one frame.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let value = match self.waveform {
            LfoWaveform::Sine => (TAU * self.phase).sin(),
            LfoWaveform::Triangle => {
                let distance = (self.phase - 0.25).abs();
                1.0 - 4.0 * distance.min(1.0 - distance)
            }
            LfoWaveform::RisingRamp => 2.0 * self.phase - 1.0,
            LfoWaveform::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        };
        self.phase += self.rate.0;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        value as f32
    }

    /// Render into caller-owned storage. An empty block leaves phase unchanged.
    /// The resulting immutable samples may be shared by multiple routes.
    pub fn render(&mut self, output: &mut [f32]) {
        for sample in output {
            *sample = self.next_sample();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_rates_bound_phase_steps_and_keep_zero_stopped() {
        let base = LfoRate::free(8.0, 1.0).unwrap();
        assert!(base.scaled(f64::NAN).is_err());
        assert!(base.scaled(-1.0).is_err());
        let rate = base.scaled(16.0).unwrap();
        let mut ramp =
            ModulationLfo::new_with_waveform(rate, 0.0, LfoWaveform::RisingRamp).unwrap();
        let mut samples = [0.0; 3];
        ramp.render(&mut samples);
        assert_eq!(samples, [-1.0, 0.0, -1.0]);
        let zero = LfoRate::free(8.0, 0.0).unwrap().scaled(16.0).unwrap();
        ramp.set_rate(zero);
        ramp.render(&mut samples);
        assert_eq!(samples, [0.0; 3]);
    }

    #[test]
    fn classic_waveforms_follow_their_bipolar_phase_laws() {
        use LfoWaveform::*;
        let cases = [
            (Triangle, [0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -0.5]),
            (RisingRamp, [-1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75]),
            (Square, [1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0]),
        ];
        for (waveform, expected) in cases {
            let rate = LfoRate::free(8.0, 1.0).unwrap();
            let mut oscillator = ModulationLfo::new_with_waveform(rate, 0.0, waveform).unwrap();
            let mut samples = [0.0; 8];
            oscillator.render(&mut samples[..3]);
            oscillator.render(&mut samples[3..]);
            assert_eq!(samples, expected);
            assert_eq!(oscillator.next_sample(), expected[0]);
        }
    }

    #[test]
    fn quarter_cycles_and_tempo_changes_preserve_phase() {
        let mut lfo = ModulationLfo::new(LfoRate::free(8.0, 2.0).unwrap(), 0.0).unwrap();
        for expected in [0.0, 1.0, 0.0, -1.0] {
            assert!((lfo.next_sample() - expected).abs() < 1e-6);
        }
        assert!(lfo.next_sample().abs() < 1e-6);
        lfo.set_rate(LfoRate::tempo(8.0, 120.0, 2.0).unwrap());
        assert!((lfo.next_sample() - 1.0).abs() < 1e-6);
        assert!((lfo.next_sample() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        lfo.reset(0.25).unwrap();
        assert!((lfo.next_sample() - 1.0).abs() < 1e-6);
        let mut replacement = ModulationLfo::new(LfoRate::free(8.0, 2.0).unwrap(), 0.0).unwrap();
        replacement.inherit_phase(&lfo);
        assert!((replacement.next_sample() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((replacement.next_sample() + std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn partitioning_and_invalid_inputs() {
        let rate = LfoRate::tempo(48_000.0, 123.0, 3.0).unwrap();
        let mut contiguous = ModulationLfo::new(rate, 0.3).unwrap();
        let mut partitioned = contiguous;
        let mut expected = [0.0; 101];
        contiguous.render(&mut expected);
        let mut actual = Vec::new();
        for len in [3, 1, 47, 50] {
            let start = actual.len();
            actual.resize(start + len, 0.0);
            partitioned.render(&mut actual[start..]);
        }
        assert_eq!(actual, expected);
        let before_empty = partitioned;
        partitioned.render(&mut []);
        assert_eq!(partitioned, before_empty);
        for (sample_rate, frequency) in [
            (0.0, 1.0),
            (f64::NAN, 1.0),
            (48.0, 25.0),
            (48.0, f64::INFINITY),
            (48.0, -1.0),
        ] {
            assert_eq!(LfoRate::free(sample_rate, frequency), Err(InvalidLfo));
        }
        assert_eq!(LfoRate::tempo(48.0, 120.0, 0.0), Err(InvalidLfo));
        assert_eq!(ModulationLfo::new(rate, 1.0), Err(InvalidLfo));
        let before = partitioned;
        assert_eq!(partitioned.reset(f64::NAN), Err(InvalidLfo));
        assert_eq!(partitioned, before);
    }

    #[test]
    fn samples_follow_the_independent_sine_law_at_audio_rate() {
        let mut source = ModulationLfo::new(LfoRate::free(48_000.0, 7.0).unwrap(), 0.125).unwrap();
        for frame in 0..48_000 {
            let expected = (TAU * (0.125 + f64::from(frame) * 7.0 / 48_000.0)).sin();
            assert!((f64::from(source.next_sample()) - expected).abs() < 1e-6);
        }
    }
}
