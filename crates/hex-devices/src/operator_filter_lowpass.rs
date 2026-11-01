//! Private Hex-original Butterworth proof. Not registered or used by Operator.
//! No finite tail, modulation, circuit, migration or callback admission claim.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRate,
    InvalidCutoff,
    InvalidSample,
    OutputOverflow,
    IncompatibleHistory,
}

/// Control-prepared normalized denominator `1 + a1 z^-1 + a2 z^-2`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedLowpass {
    coefficients: [f64; 5],
    rate: u32,
}

impl PreparedLowpass {
    pub fn prepare(cutoff_hz: f64, rate: u32) -> Result<Self, Error> {
        if !(8_000..=192_000).contains(&rate) {
            return Err(Error::InvalidRate);
        }
        let maximum = 20_000.0_f64.min(0.45 * f64::from(rate));
        if !cutoff_hz.is_finite() || !(20.0..=maximum).contains(&cutoff_hz) {
            return Err(Error::InvalidCutoff);
        }
        // Bilinear transform of H(s)=1/(s² + sqrt(2)s + 1), prewarped
        // so the authored cutoff is exactly the digital -3 dB frequency.
        let k = (std::f64::consts::PI * cutoff_hz / f64::from(rate)).tan();
        let k2 = k * k;
        let denominator = 1.0 + std::f64::consts::SQRT_2 * k + k2;
        let b0 = k2 / denominator;
        Ok(Self {
            coefficients: [
                b0,
                2.0 * b0,
                b0,
                2.0 * (k2 - 1.0) / denominator,
                (1.0 - std::f64::consts::SQRT_2 * k + k2) / denominator,
            ],
            rate,
        })
    }

    pub const fn coefficients(self) -> [f64; 5] {
        self.coefficients
    }
}

/// Stereo physical sample history, all f64. Subnormal history is flushed to zero;
/// other finite values are retained without a hidden limiter. This does not assume
/// either arm64 or x86 has flush-to-zero enabled by the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lowpass {
    prepared: PreparedLowpass,
    // x[n-1], x[n-2], y[n-1], y[n-2] independently for each channel.
    history: [[f64; 4]; 2],
}

impl Lowpass {
    pub const fn new(prepared: PreparedLowpass) -> Self {
        Self {
            prepared,
            history: [[0.0; 4]; 2],
        }
    }

    /// Atomic for one stereo frame: failed input/output leaves both lanes
    /// untouched. This is not an audio-owned block transaction or ACK API.
    pub fn process_frame(&mut self, input: [f32; 2]) -> Result<[f32; 2], Error> {
        if input.iter().any(|value| !value.is_finite()) {
            return Err(Error::InvalidSample);
        }
        let [b0, b1, b2, a1, a2] = self.prepared.coefficients;
        let mut next = self.history;
        let mut result = [0.0; 2];
        for channel in 0..2 {
            let x = f64::from(input[channel]);
            let [x1, x2, y1, y2] = self.history[channel];
            let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            if !y.is_finite() || y.abs() > f64::from(f32::MAX) {
                return Err(Error::OutputOverflow);
            }
            result[channel] = y as f32;
            next[channel] = [quiet(x), quiet(x1), quiet(y), quiet(y1)];
        }
        self.history = next;
        Ok(result)
    }

    pub fn reset(&mut self) {
        self.history = [[0.0; 4]; 2];
    }

    /// Exact law/rate match only. No coefficient retarget or owner identity
    /// authorization: stable device/occurrence matching belongs to a future host.
    pub fn copy_history_from(&mut self, source: &Self) -> Result<(), Error> {
        if self.prepared != source.prepared {
            return Err(Error::IncompatibleHistory);
        }
        self.history = source.history;
        Ok(())
    }
}

fn quiet(value: f64) -> f64 {
    if value.is_subnormal() { 0.0 } else { value }
}

#[cfg(test)]
mod denormal_tests {
    use super::*;

    #[test]
    fn silence_clears_subnormal_history_and_impulse_decay_stays_normal() {
        let prepared = PreparedLowpass::prepare(20.0, 48_000).unwrap();
        let mut filter = Lowpass::new(prepared);
        filter.history = [[f64::from_bits(1); 4]; 2];
        filter.process_frame([0.0; 2]).unwrap();
        assert!(filter.history.iter().flatten().all(|v| !v.is_subnormal()));

        filter.reset();
        filter.process_frame([1.0, -1.0]).unwrap();
        for _ in 0..480_000 {
            filter.process_frame([0.0; 2]).unwrap();
            assert!(filter.history.iter().flatten().all(|v| !v.is_subnormal()));
        }
    }
}
