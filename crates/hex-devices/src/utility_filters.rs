//! Original fixed-state Utility filters. These are not yet admitted to the
//! native registry: track summation and captured-release ownership come first.
use crate::utility::UtilityError;
use hex_lattice::StereoFrame;

const DC_CUTOFF_HZ: f64 = 5.0;
const HISTORY_FLOOR: f64 = 1.0e-30;

/// Control-prepared coefficients and settings; construction validates all input.
/// Bass mono uses an original fourth-order Linkwitz–Riley high-pass on the side
/// signal. Audition uses the corresponding low-pass on the mid signal. Exact
/// Live responses have not been measured.
#[derive(Clone, Copy, Debug)]
pub struct PreparedUtilityFilters {
    sample_rate: u32,
    dc_enabled: bool,
    bass_enabled: bool,
    audition: bool,
    dc_pole: f64,
    low: BiquadCoefficients,
    high: BiquadCoefficients,
}

impl PreparedUtilityFilters {
    pub fn new(
        sample_rate: u32,
        dc_enabled: bool,
        bass_enabled: bool,
        cutoff_hz: f64,
        audition: bool,
    ) -> Result<Self, UtilityError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(UtilityError(
                "utility filter rate must be between 8000 and 192000 hz",
            ));
        }
        if !cutoff_hz.is_finite() || !(50.0..=500.0).contains(&cutoff_hz) {
            return Err(UtilityError(
                "bass mono cutoff must be finite and between 50 and 500 hz",
            ));
        }
        let omega = std::f64::consts::TAU * cutoff_hz / f64::from(sample_rate);
        let cosine = omega.cos();
        let alpha = omega.sin() / std::f64::consts::SQRT_2;
        let inverse = 1.0 / (1.0 + alpha);
        let a1 = -2.0 * cosine * inverse;
        let a2 = (1.0 - alpha) * inverse;
        let low_b0 = (1.0 - cosine) * 0.5 * inverse;
        let high_b0 = (1.0 + cosine) * 0.5 * inverse;
        Ok(Self {
            sample_rate,
            dc_enabled,
            bass_enabled,
            audition,
            dc_pole: (-std::f64::consts::TAU * DC_CUTOFF_HZ / f64::from(sample_rate)).exp(),
            low: BiquadCoefficients {
                b0: low_b0,
                b1: 2.0 * low_b0,
                b2: low_b0,
                a1,
                a2,
            },
            high: BiquadCoefficients {
                b0: high_b0,
                b1: -2.0 * high_b0,
                b2: high_b0,
                a1,
                a2,
            },
        })
    }

    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }
    pub const fn algorithmic_latency_frames(self) -> u32 {
        0
    }
    pub const fn is_identity(self) -> bool {
        !self.dc_enabled && !self.bass_enabled
    }
}

#[derive(Clone, Copy, Debug)]
struct BiquadCoefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct BiquadHistory {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}
impl BiquadHistory {
    fn sample(&mut self, input: f64, coefficients: BiquadCoefficients) -> f64 {
        let output =
            coefficients.b0 * input + coefficients.b1 * self.x1 + coefficients.b2 * self.x2
                - coefficients.a1 * self.y1
                - coefficients.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = quiet(output);
        self.y1
    }
}

fn quiet(value: f64) -> f64 {
    if value.abs() < HISTORY_FLOOR {
        0.0
    } else {
        value
    }
}

/// Audio-owned fixed-size history. No heap resources, locks, or scheduling.
/// Callers must reset on seek/stop, retain on pause/loop, and only inherit history
/// for a compatible stable device instance. Finite PCM input is a caller contract.
#[derive(Clone, Copy, Debug)]
pub struct UtilityFilterProcessor {
    prepared: PreparedUtilityFilters,
    dc_input: [f64; 2],
    dc_output: [f64; 2],
    side_high: [BiquadHistory; 2],
    mid_low: [BiquadHistory; 2],
}

impl UtilityFilterProcessor {
    pub fn new(prepared: PreparedUtilityFilters) -> Self {
        Self {
            prepared,
            dc_input: [0.0; 2],
            dc_output: [0.0; 2],
            side_high: [BiquadHistory::default(); 2],
            mid_low: [BiquadHistory::default(); 2],
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.prepared);
    }

    /// Preserve physical sample histories when coefficients/settings change.
    /// A sample-rate change is incompatible and resets all histories.
    pub fn reconfigure(&mut self, prepared: PreparedUtilityFilters) {
        let compatible = prepared.sample_rate == self.prepared.sample_rate;
        self.prepared = prepared;
        if !compatible {
            self.reset();
        }
    }

    /// Copy only fixed-size history, leaving the candidate's prepared settings.
    /// Identity/type/instance matching belongs to the engine. Rate mismatch leaves
    /// a fresh reset candidate and explicitly refuses inheritance.
    pub fn inherit_history_from(&mut self, previous: &Self) -> bool {
        if self.prepared.sample_rate != previous.prepared.sample_rate {
            self.reset();
            return false;
        }
        self.dc_input = previous.dc_input;
        self.dc_output = previous.dc_output;
        self.side_high = previous.side_high;
        self.mid_low = previous.mid_low;
        true
    }

    pub fn process_frame(&mut self, input: StereoFrame) -> StereoFrame {
        let mut signal = [f64::from(input[0]), f64::from(input[1])];
        // Histories advance even while each mode is bypassed. Enabling a mode
        // therefore never resurrects an old signal; exact neutral output remains.
        for (channel, sample) in signal.iter_mut().enumerate() {
            let filtered = quiet(
                *sample - self.dc_input[channel] + self.prepared.dc_pole * self.dc_output[channel],
            );
            self.dc_input[channel] = *sample;
            self.dc_output[channel] = filtered;
            if self.prepared.dc_enabled {
                *sample = filtered;
            }
        }
        let mid = (signal[0] + signal[1]) * 0.5;
        let mut side = (signal[0] - signal[1]) * 0.5;
        let mut low_mid = mid;
        for stage in &mut self.side_high {
            side = stage.sample(side, self.prepared.high);
        }
        for stage in &mut self.mid_low {
            low_mid = stage.sample(low_mid, self.prepared.low);
        }
        if !self.prepared.bass_enabled {
            if !self.prepared.dc_enabled {
                return input;
            }
            return [signal[0] as f32, signal[1] as f32];
        }
        if self.prepared.audition {
            [low_mid as f32; 2]
        } else {
            [(mid + side) as f32, (mid - side) as f32]
        }
    }

    pub fn process_block(&mut self, frames: &mut [StereoFrame]) {
        for frame in frames {
            *frame = self.process_frame(*frame);
        }
    }
}

#[cfg(test)]
mod tests;
