//! Original two-pole Butterworth high/low cascade; not reference coefficients.
use hex_dsp::{Biquad, BiquadCoefficients};

fn butterworth(frequency: f64, rate: u32, high: bool) -> BiquadCoefficients {
    let clamped = frequency.clamp(5.0, f64::from(rate) * 0.45);
    let result = if high {
        BiquadCoefficients::butterworth_high_pass(clamped, f64::from(rate))
    } else {
        BiquadCoefficients::butterworth_low_pass(clamped, f64::from(rate))
    };
    result.unwrap_or(BiquadCoefficients::IDENTITY)
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct BandPass {
    high: Biquad,
    low: Biquad,
}

impl BandPass {
    /// Preserve coefficients, but never duplicate the donor's signal memory.
    pub(super) fn without_signal_memory(self) -> Self {
        Self {
            high: self.high.without_state(),
            low: self.low.without_state(),
        }
    }

    pub(super) fn tune(&mut self, frequency: f64, width: f64, rate: u32) {
        let span = 2.0_f64.powf(width * 0.5);
        self.high
            .set_coefficients(butterworth(frequency / span, rate, true));
        self.low
            .set_coefficients(butterworth(frequency * span, rate, false));
    }

    pub(super) fn process(&mut self, x: f64) -> f64 {
        self.low
            .process_paired_flush(self.high.process_paired_flush(x))
    }

    pub(super) fn quiet(self) -> bool {
        self.high.is_quiet() && self.low.is_quiet()
    }

    pub(super) fn magnitude(self, hz: f64, rate: u32) -> f64 {
        let w = std::f64::consts::TAU * hz / f64::from(rate);
        self.high.magnitude_at_omega(w) * self.low.magnitude_at_omega(w)
    }
}
