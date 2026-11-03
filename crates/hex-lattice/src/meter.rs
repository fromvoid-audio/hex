//! Lock-free peak handoff. Positive finite floats preserve bit ordering.
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// Provider-reported attenuation, in positive dB, for its two working lanes.
/// Lane meaning follows the device's active routing. Only committed processing
/// publishes readings; no signal-ratio estimate or speculative history is used.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DynamicsMeter {
    pub reduction_db: [f32; 2],
    pub clipped: bool,
    /// Peak after the processor's input gain, before dynamics processing.
    pub post_input_peak: f32,
}

/// Sample peaks drained from one [`DeviceMeter`], as linear amplitude.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DevicePeaks {
    /// Device input, after host input gain, as the larger of both channels.
    pub input: f32,
    /// Device output after host output gain, left then right.
    pub output: [f32; 2],
}

/// Scalar endpoint retained by charged playback observations. On 64-bit hosts,
/// align its payload with the Arc counter header so payload-plus-header storage
/// declarations include all trailing allocation padding as fields are added.
#[cfg_attr(target_pointer_width = "64", repr(align(8)))]
#[derive(Debug, Default)]
pub struct DeviceMeter {
    input: AtomicU32,
    /// Post-device left/right sample peaks for the passive inter-device meter.
    output: [AtomicU32; 2],
    track_output: AtomicU32,
    reduction: [AtomicU32; 2],
    clipped: AtomicU32,
    post_input_peak: AtomicU32,
}
impl DeviceMeter {
    /// CONTROL ONLY: complete exclusive payload declaration for this value.
    /// The containing Arc allocation/header and each owning handle are separate.
    /// No heap/shared/Weak children exist; new fields require owning-module audit.
    pub fn control_storage_declaration(&self) -> crate::PreparedStorageDeclaration {
        let Self {
            input: _,
            output: _,
            track_output: _,
            reduction: _,
            clipped: _,
            post_input_peak: _,
        } = self;
        crate::PreparedStorageDeclaration::ExclusiveOnly {
            bytes: std::mem::size_of::<Self>(),
        }
    }

    /// Accumulate every block until the UI drains it, retaining short transients.
    pub fn observe_input(&self, frames: &[[f32; 2]]) {
        Self::observe(&self.input, frames);
    }
    /// Foreign providers receive untrimmed frames for aligned dry history.
    /// Meter their actual wet input without changing that borrowed signal.
    pub(crate) fn observe_input_with_gain(&self, frames: &[[f32; 2]], gain: f32) {
        Self::observe_with_gain(&self.input, frames, gain);
    }
    /// Accumulate left and right independently for the stereo output meter.
    pub fn observe_output(&self, frames: &[[f32; 2]]) {
        let mut peaks = [0.0_f32; 2];
        for frame in frames {
            for (peak, sample) in peaks.iter_mut().zip(frame) {
                if sample.is_finite() {
                    *peak = peak.max(sample.abs());
                }
            }
        }
        Self::publish_output(&self.output, peaks);
    }
    fn publish_output(output: &[AtomicU32; 2], peaks: [f32; 2]) {
        for (target, peak) in output.iter().zip(peaks) {
            target.fetch_max(peak.to_bits(), Relaxed);
        }
    }
    /// Accumulate the containing track's post-fader output independently of
    /// the instrument's own input and output taps.
    pub fn observe_track_output(&self, frames: &[[f32; 2]]) {
        Self::observe(&self.track_output, frames);
    }
    /// Drain the track-output peak from its single display consumer.
    pub fn take_track_output(&self) -> f32 {
        f32::from_bits(self.track_output.swap(0, Relaxed))
    }
    /// Recover a device tap from an analytic source's already mixed lanes.
    /// Unity lanes are exact; fader normalization carries f32 rounding only.
    /// Muted/zero-gain lanes contribute no measured signal.
    pub fn observe_mixed(&self, frames: &[[f32; 2]], gains: [f32; 2], input: bool) {
        let mut peaks = [0.0_f32; 2];
        for frame in frames {
            for channel in 0..2 {
                if gains[channel] > 0.0 {
                    let value = (frame[channel] / gains[channel]).abs();
                    if value.is_finite() {
                        peaks[channel] = peaks[channel].max(value);
                    }
                }
            }
        }
        if input {
            self.input
                .fetch_max(peaks[0].max(peaks[1]).to_bits(), Relaxed);
        } else {
            Self::publish_output(&self.output, peaks);
        }
    }
    fn observe(peak: &AtomicU32, frames: &[[f32; 2]]) {
        Self::observe_with_gain(peak, frames, 1.0);
    }
    fn observe_with_gain(peak: &AtomicU32, frames: &[[f32; 2]], gain: f32) {
        let value = frames.iter().flatten().fold(0.0_f32, |peak, sample| {
            let sample = sample * gain;
            if sample.is_finite() {
                peak.max(sample.abs())
            } else {
                peak
            }
        });
        peak.fetch_max(value.to_bits(), Relaxed);
    }
    /// One UI consumer drains the maxima since its last refresh: the
    /// channel-combined input peak and the post-device left/right peaks.
    pub fn take_peaks(&self) -> DevicePeaks {
        DevicePeaks {
            input: f32::from_bits(self.input.swap(0, Relaxed)),
            output: self
                .output
                .each_ref()
                .map(|peak| f32::from_bits(peak.swap(0, Relaxed))),
        }
    }
    pub(crate) fn observe_dynamics(&self, reading: DynamicsMeter) {
        if reading.post_input_peak.is_finite() && reading.post_input_peak > 0.0 {
            self.post_input_peak
                .fetch_max(reading.post_input_peak.to_bits(), Relaxed);
        }
        for (target, value) in self.reduction.iter().zip(reading.reduction_db) {
            // Both signs of zero mean no attenuation. The -0 bit pattern
            // would otherwise outrank every positive finite reading.
            if value.is_finite() && value > 0.0 {
                target.fetch_max(value.to_bits(), Relaxed);
            }
        }
        if reading.clipped {
            self.clipped.store(1, Relaxed);
        }
    }
    /// One UI consumer drains independently of the input/output sample peaks.
    pub fn take_dynamics(&self) -> DynamicsMeter {
        DynamicsMeter {
            reduction_db: std::array::from_fn(|i| {
                f32::from_bits(self.reduction[i].swap(0, Relaxed))
            }),
            clipped: self.clipped.swap(0, Relaxed) != 0,
            post_input_peak: f32::from_bits(self.post_input_peak.swap(0, Relaxed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_payload_plus_counter_header_accounts_for_arc_tail_padding() {
        use std::{mem::size_of, sync::atomic::AtomicUsize};
        // The pinned native ArcInner is repr(C): strong, weak, then data.
        #[repr(C)]
        struct ArcInnerLayout {
            strong: AtomicUsize,
            weak: AtomicUsize,
            data: DeviceMeter,
        }
        assert_eq!(
            size_of::<DeviceMeter>() + 2 * size_of::<usize>(),
            size_of::<ArcInnerLayout>(),
        );
    }

    #[test]
    fn short_transients_survive_quiet_blocks_and_invalid_samples() {
        let meter = DeviceMeter::default();
        meter.observe_input(&[[0.1, -1.2]]);
        meter.observe_output(&[[0.4, -0.8]]);
        meter.observe_track_output(&[[0.3, -0.6]]);
        for _ in 0..10 {
            meter.observe_input(&[[0.01; 2]]);
        }
        meter.observe_output(&[[f32::NAN, f32::INFINITY]]);
        assert_eq!(
            meter.take_peaks(),
            DevicePeaks {
                input: 1.2,
                output: [0.4, 0.8],
            }
        );
        assert_eq!(meter.take_track_output(), 0.6);
        assert_eq!(meter.take_track_output(), 0.0);
        assert_eq!(meter.take_peaks(), DevicePeaks::default());
    }

    #[test]
    fn output_channels_stay_independent_for_direct_and_mixed_taps() {
        let meter = DeviceMeter::default();
        meter.observe_output(&[[0.5, 0.0], [-0.25, f32::NAN]]);
        meter.observe_output(&[[0.0, -0.125]]);
        assert_eq!(meter.take_peaks().output, [0.5, 0.125]);
        // A muted lane contributes no evidence; the other lane is normalized.
        meter.observe_mixed(&[[0.5, 0.3]], [0.5, 0.0], false);
        meter.observe_mixed(&[[0.2, 0.6]], [0.5, 0.5], true);
        assert_eq!(
            meter.take_peaks(),
            DevicePeaks {
                input: 1.2,
                output: [1.0, 0.0],
            }
        );
    }

    #[test]
    fn negative_zero_does_not_hide_later_attenuation() {
        let meter = DeviceMeter::default();
        meter.observe_dynamics(DynamicsMeter {
            reduction_db: [-0.0, 0.0],
            clipped: false,
            post_input_peak: 0.0,
        });
        meter.observe_dynamics(DynamicsMeter {
            reduction_db: [6.0, 3.0],
            clipped: false,
            post_input_peak: 0.0,
        });
        assert_eq!(meter.take_dynamics().reduction_db, [6.0, 3.0]);
    }

    #[test]
    fn dynamics_peaks_are_independent_and_ignore_invalid_readings() {
        let meter = DeviceMeter::default();
        meter.observe_input(&[[0.25, -0.5]]);
        meter.observe_output(&[[0.75, -1.0]]);
        meter.observe_dynamics(DynamicsMeter {
            reduction_db: [3.0, 5.5],
            clipped: false,
            post_input_peak: 1.25,
        });
        meter.observe_dynamics(DynamicsMeter {
            reduction_db: [f32::NAN, f32::INFINITY],
            clipped: true,
            post_input_peak: f32::INFINITY,
        });
        meter.observe_dynamics(DynamicsMeter {
            reduction_db: [-2.0, f32::NEG_INFINITY],
            clipped: false,
            post_input_peak: 0.0,
        });

        assert_eq!(
            meter.take_peaks(),
            DevicePeaks {
                input: 0.5,
                output: [0.75, 1.0],
            }
        );
        assert_eq!(
            meter.take_dynamics(),
            DynamicsMeter {
                reduction_db: [3.0, 5.5],
                clipped: true,
                post_input_peak: 1.25,
            }
        );
        assert_eq!(meter.take_dynamics(), DynamicsMeter::default());
        assert_eq!(meter.take_peaks(), DevicePeaks::default());
        assert_eq!(
            meter.control_storage_declaration(),
            crate::PreparedStorageDeclaration::ExclusiveOnly {
                bytes: std::mem::size_of::<DeviceMeter>(),
            }
        );
    }
}
