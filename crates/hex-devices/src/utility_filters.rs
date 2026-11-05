//! Original fixed-state Utility filters prepared by the native registry.
use crate::utility::UtilityError;
use hex_lattice::{NativeProcessorRuntime, StereoFrame};
use std::any::Any as StdAny;

const DC_CUTOFF_HZ: f64 = 5.0;
const HISTORY_FLOOR: f64 = 1.0e-30;

/// Control-prepared coefficients and settings; construction validates all input.
/// Bass mono uses an original fourth-order Linkwitz–Riley high-pass on the side
/// signal. Audition uses the corresponding low-pass on the mid signal. Exact
/// Live responses have not been measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedUtilityFilters {
    sample_rate: u32,
    dc_enabled: bool,
    bass_enabled: bool,
    audition: bool,
    output_muted: bool,
    dc_pole: f64,
    low: BiquadCoefficients,
    high: BiquadCoefficients,
}

impl PreparedUtilityFilters {
    pub(crate) fn check_inline(&self) {
        let Self {
            sample_rate,
            dc_enabled,
            bass_enabled,
            audition,
            output_muted,
            dc_pole,
            low,
            high,
        } = self;
        let _: (&u32, &bool, &bool, &bool, &bool, &f64) = (
            sample_rate,
            dc_enabled,
            bass_enabled,
            audition,
            output_muted,
            dc_pole,
        );
        low.check_inline();
        high.check_inline();
    }
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
            output_muted: false,
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
        !self.dc_enabled && !self.bass_enabled && !self.output_muted
    }

    /// Zero the processor's input and output while continuing to advance every
    /// filter history, including during compatible live mute handoffs.
    pub const fn with_output_mute(mut self, muted: bool) -> Self {
        self.output_muted = muted;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
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
impl BiquadCoefficients {
    fn check_inline(&self) {
        let Self { b0, b1, b2, a1, a2 } = self;
        let _: (&f64, &f64, &f64, &f64, &f64) = (b0, b1, b2, a1, a2);
    }
}
impl BiquadHistory {
    fn check_inline(&self) {
        let Self { x1, x2, y1, y2 } = self;
        let _: (&f64, &f64, &f64, &f64) = (x1, x2, y1, y2);
    }
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

    fn is_silent(self) -> bool {
        self.x1 == 0.0 && self.x2 == 0.0 && self.y1 == 0.0 && self.y2 == 0.0
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
    dc_history_valid: bool,
    bass_history_valid: bool,
    automation: Option<crate::utility::UtilityAutomation>,
    prepared: PreparedUtilityFilters,
    mute_ramp: Option<crate::utility::UtilityRamp>,
    dc_input: [f64; 2],
    dc_output: [f64; 2],
    side_high: [BiquadHistory; 2],
    mid_low: [BiquadHistory; 2],
}

impl UtilityFilterProcessor {
    pub fn new(prepared: PreparedUtilityFilters) -> Self {
        Self::with_automation(prepared, None)
    }
    pub(crate) fn with_automation(
        prepared: PreparedUtilityFilters,
        automation: Option<crate::utility::UtilityAutomation>,
    ) -> Self {
        let runtime = Self {
            dc_history_valid: true,
            bass_history_valid: true,
            automation,
            prepared,
            mute_ramp: None,
            dc_input: [0.0; 2],
            dc_output: [0.0; 2],
            side_high: [BiquadHistory::default(); 2],
            mid_low: [BiquadHistory::default(); 2],
        };
        runtime.check_inline_ownership();
        runtime
    }

    // The closed constructor's complete Box payload is fixed inline. Adding an
    // owning field must fail this exhaustive, Copy-bounded review seam instead
    // of silently making size_of_val omit a newly nested allocation/handle.
    fn check_inline_ownership(&self) {
        let Self {
            dc_history_valid,
            bass_history_valid,
            automation,
            prepared,
            mute_ramp,
            dc_input,
            dc_output,
            side_high,
            mid_low,
        } = self;
        let _: (&bool, &bool) = (dc_history_valid, bass_history_valid);
        if let Some(automation) = automation {
            automation.check_inline();
        }
        prepared.check_inline();
        if let Some(ramp) = mute_ramp {
            ramp.check_inline();
        }
        let _: (&[f64; 2], &[f64; 2]) = (dc_input, dc_output);
        for history in side_high.iter().chain(mid_low) {
            history.check_inline();
        }
    }

    pub fn reset(&mut self) {
        let automation = self.automation;
        *self = Self::with_automation(self.prepared, automation);
        if let Some(mut automation) = self.automation
            && let Ok((matrix, Some(filters))) =
                automation.configuration(None, 0, self.prepared.sample_rate, None)
        {
            automation.matrix = matrix;
            self.automation = Some(automation);
            self.prepared = filters;
        }
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
        self.mute_ramp = self.manual_mute_target().and_then(|target| {
            if Some(target) == previous.manual_mute_target() {
                previous.mute_ramp
            } else {
                let from = previous.mute_ramp.map_or(
                    f32::from(!previous.prepared.output_muted),
                    crate::utility::UtilityRamp::value,
                );
                crate::utility::UtilityRamp::new(
                    from,
                    f32::from(!target),
                    self.prepared.sample_rate,
                )
            }
        });
        self.dc_history_valid = previous.dc_history_valid;
        self.bass_history_valid = previous.bass_history_valid;
        self.dc_input = if previous.dc_history_valid {
            previous.dc_input
        } else {
            [0.0; 2]
        };
        self.dc_output = if previous.dc_history_valid {
            previous.dc_output
        } else {
            [0.0; 2]
        };
        self.side_high = if previous.bass_history_valid {
            previous.side_high
        } else {
            [BiquadHistory::default(); 2]
        };
        self.mid_low = if previous.bass_history_valid {
            previous.mid_low
        } else {
            [BiquadHistory::default(); 2]
        };
        if let (Some(target), Some(source)) = (&mut self.automation, previous.automation) {
            target.inherit(&source, self.prepared.sample_rate);
            self.prepared = previous.prepared;
        }
        true
    }

    fn manual_mute_target(&self) -> Option<bool> {
        self.automation.as_ref().map_or(
            Some(self.prepared.output_muted),
            crate::utility::UtilityAutomation::manual_mute_target,
        )
    }

    fn process_automated(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn hex_lattice::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        let mut resolved = self
            .automation
            .as_ref()
            .ok_or(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass)?
            .resolve(values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            let automation = self
                .automation
                .as_mut()
                .ok_or(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass)?;
            let (_, filters) = automation.configuration(
                Some((values, &mut resolved)),
                offset,
                self.prepared.sample_rate,
                Some(&self.prepared),
            )?;
            if let Some(filters) = filters {
                self.reconfigure(filters);
            }
            let internal = self.process_frame(*frame);
            if !bypassed {
                *frame = internal;
            }
        }
        Ok(())
    }

    pub fn process_frame(&mut self, input: StereoFrame) -> StereoFrame {
        let input = self
            .automation
            .as_ref()
            .map_or(input, |automation| automation.matrix.apply(input));
        let gain = if let Some(ramp) = self.mute_ramp.as_mut() {
            let gain = ramp.advance();
            if ramp.finished() {
                self.mute_ramp = None;
            }
            gain
        } else {
            f32::from(!self.prepared.output_muted)
        };
        let input = if gain == 0.0 { [0.0; 2] } else { input };
        let mut signal = [f64::from(input[0]), f64::from(input[1])];
        // Modes which this revision can enable retain their dormant histories.
        // A skipped mode is explicitly invalidated so a later revision starts it
        // cleanly instead of resurrecting history from before the skipped span.
        self.dc_history_valid = self
            .automation
            .as_ref()
            .is_none_or(|automation| automation.may_advance_dc())
            || self.prepared.dc_enabled;
        if self.dc_history_valid {
            for (channel, sample) in signal.iter_mut().enumerate() {
                let filtered = quiet(
                    *sample - self.dc_input[channel]
                        + self.prepared.dc_pole * self.dc_output[channel],
                );
                self.dc_input[channel] = *sample;
                self.dc_output[channel] = filtered;
                if self.prepared.dc_enabled {
                    *sample = filtered;
                }
            }
        }
        let mid = (signal[0] + signal[1]) * 0.5;
        let mut side = (signal[0] - signal[1]) * 0.5;
        let mut low_mid = mid;
        self.bass_history_valid = self
            .automation
            .as_ref()
            .is_none_or(|automation| automation.may_advance_bass())
            || self.prepared.bass_enabled;
        if self.bass_history_valid {
            for stage in &mut self.side_high {
                side = stage.sample(side, self.prepared.high);
            }
            for stage in &mut self.mid_low {
                low_mid = stage.sample(low_mid, self.prepared.low);
            }
        }
        let output = if !self.prepared.bass_enabled {
            if !self.prepared.dc_enabled {
                input
            } else {
                [signal[0] as f32, signal[1] as f32]
            }
        } else if self.prepared.audition {
            [low_mid as f32; 2]
        } else {
            [(mid + side) as f32, (mid - side) as f32]
        };
        if gain == 1.0 {
            output
        } else if gain == 0.0 {
            [0.0; 2]
        } else {
            output.map(|sample| sample * gain)
        }
    }

    pub fn process_block(&mut self, frames: &mut [StereoFrame]) {
        for frame in frames {
            *frame = self.process_frame(*frame);
        }
    }

    /// Whether the active filter configuration has no stored output residual.
    /// Checks every history used by the enabled stages, including both
    /// Linkwitz–Riley paths because audition may select either path.
    pub fn is_quiescent(&self) -> bool {
        let dc_silent = !self.prepared.dc_enabled
            || (self.dc_input.iter().all(|sample| *sample == 0.0)
                && self.dc_output.iter().all(|sample| *sample == 0.0));
        let bass_silent = !self.prepared.bass_enabled
            || (self.side_high.iter().all(|history| history.is_silent())
                && self.mid_low.iter().all(|history| history.is_silent()));
        dc_silent && bass_silent
    }
}

impl NativeProcessorRuntime for UtilityFilterProcessor {
    fn supports_parameter_automation(&self, parameter: hex_lattice_contract::ParameterId) -> bool {
        self.automation
            .is_some_and(|automation| automation.supports(parameter))
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.process_automated(frames, values, false)
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.process_automated(frames, values, true)
    }
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        UtilityFilterProcessor::process_block(self, frames);
    }

    fn reset(&mut self) {
        UtilityFilterProcessor::reset(self);
    }

    fn is_quiescent(&self) -> bool {
        UtilityFilterProcessor::is_quiescent(self)
    }

    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.prepared.sample_rate == previous.prepared.sample_rate)
    }

    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| {
                self.prepared.sample_rate == previous.prepared.sample_rate
                    && (self.automation.is_some()
                        || previous.automation.is_some()
                        || self.prepared.with_output_mute(false)
                            == previous.prepared.with_output_mute(false))
            })
    }

    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            let _ = self.inherit_history_from(previous);
        }
    }

    fn as_any(&self) -> &dyn StdAny {
        self
    }
}

#[cfg(test)]
mod tests;
