//! Lock-free sample-peak handoff. Positive finite floats preserve bit ordering.
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

#[derive(Debug, Default)]
pub struct DeviceMeter {
    input: AtomicU32,
    output: AtomicU32,
}
impl DeviceMeter {
    /// CONTROL ONLY: complete exclusive payload declaration for this value.
    /// The containing Arc allocation/header and each owning handle are separate.
    /// No heap/shared/Weak children exist; new fields require owning-module audit.
    pub fn control_storage_declaration(&self) -> crate::PreparedStorageDeclaration {
        let Self {
            input: _,
            output: _,
        } = self;
        crate::PreparedStorageDeclaration::ExclusiveOnly {
            bytes: std::mem::size_of::<Self>(),
        }
    }

    /// Accumulate every block until the UI drains it, retaining short transients.
    pub fn observe_input(&self, frames: &[[f32; 2]]) {
        Self::observe(&self.input, frames);
    }
    pub fn observe_output(&self, frames: &[[f32; 2]]) {
        Self::observe(&self.output, frames);
    }
    /// Recover a device tap from an analytic source's already mixed lanes.
    /// Unity lanes are exact; fader normalization carries f32 rounding only.
    /// Muted/zero-gain lanes contribute no measured signal.
    pub fn observe_mixed(&self, frames: &[[f32; 2]], gains: [f32; 2], input: bool) {
        let mut peak = 0.0_f32;
        for frame in frames {
            for channel in 0..2 {
                if gains[channel] > 0.0 {
                    let value = (frame[channel] / gains[channel]).abs();
                    if value.is_finite() {
                        peak = peak.max(value);
                    }
                }
            }
        }
        let target = if input { &self.input } else { &self.output };
        target.fetch_max(peak.to_bits(), Relaxed);
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
    fn owning_declaration_covers_only_fixed_payload_and_preserves_meter_behavior() {
        let meter = DeviceMeter::default();
        assert_eq!(
            meter.control_storage_declaration(),
            crate::PreparedStorageDeclaration::ExclusiveOnly {
                bytes: std::mem::size_of::<DeviceMeter>()
            }
        );
        meter.observe_input(&[[0.5, -0.25]]);
        assert_eq!(meter.take_peaks(), [0.5, 0.0]);
    }
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
