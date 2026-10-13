//! Lock-free sample-peak handoff. Positive finite floats preserve bit ordering.
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

#[derive(Debug, Default)]
pub struct DeviceMeter {
    input: AtomicU32,
    output: AtomicU32,
}
impl DeviceMeter {
    /// Accumulate every block until the UI drains it, retaining short transients.
    pub fn observe_input(&self, frames: &[[f32; 2]]) {
        Self::observe(&self.input, frames);
    }
    pub fn observe_output(&self, frames: &[[f32; 2]]) {
        Self::observe(&self.output, frames);
    }
    fn observe(peak: &AtomicU32, frames: &[[f32; 2]]) {
        let value = frames.iter().flatten().fold(0.0_f32, |peak, sample| {
            if sample.is_finite() {
                peak.max(sample.abs())
            } else {
                peak
            }
        });
        peak.fetch_max(value.to_bits(), Relaxed);
    }
    /// One UI consumer drains the maximum since its last refresh.
    pub fn take_peaks(&self) -> [f32; 2] {
        [
            f32::from_bits(self.input.swap(0, Relaxed)),
            f32::from_bits(self.output.swap(0, Relaxed)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_transients_survive_quiet_blocks_and_invalid_samples() {
        let meter = DeviceMeter::default();
        meter.observe_input(&[[0.1, -1.2]]);
        meter.observe_output(&[[0.4, -0.8]]);
        for _ in 0..10 {
            meter.observe_input(&[[0.01; 2]]);
        }
        meter.observe_output(&[[f32::NAN, f32::INFINITY]]);
        assert_eq!(meter.take_peaks(), [1.2, 0.8]);
        assert_eq!(meter.take_peaks(), [0.0; 2]);
    }
}
