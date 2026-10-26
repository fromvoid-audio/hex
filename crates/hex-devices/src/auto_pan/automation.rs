//! Producer-independent canonical values feed the existing Auto Pan transitions.
//! This adapter owns no curves, clock, modulation graph or resource custody.
use super::*;

pub(super) fn supported(parameter: ParameterId) -> bool {
    let Ok(binding) = crate::parameter_binding::binding(&BINDINGS, parameter) else {
        return false;
    };
    parameter
        .value()
        .checked_sub(1)
        .is_some_and(|index| index < 4)
        && binding.declares_arrangement_automation()
        && binding.semantics.smoothing == hex_lattice_contract::ParameterSmoothing::DeviceManaged
        && binding.semantics.change_impact == hex_lattice_contract::ParameterChangeImpact::ValueOnly
}

impl AutoPanDefinition {
    /// Asset-free, fixed-inline constructor used by the closed storage owner.
    /// Each admitted parameter uses the original 5 ms transition, including the
    /// first available value and fallback to authored base on absent origin.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedAutoPan, DeviceContractError> {
        state.require_asset_free_processing()?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
            || parameters.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = decode(state)?;
        let mut automation_mask = 0u8;
        for parameter in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, *parameter)?;
            binding.semantics.validate(binding.descriptor)?;
            if !supported(*parameter) {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            let shift = parameter
                .value()
                .checked_sub(1)
                .ok_or(DeviceContractError::UnknownParameter)?;
            automation_mask |= 1u8
                .checked_shl(shift)
                .ok_or(DeviceContractError::UnknownParameter)?;
        }
        let result = PreparedAutoPan {
            settings,
            sample_rate,
            automation_mask,
            ramp_frames: ((f64::from(sample_rate) * DEFAULT_RAMP_MS / 1_000.0).ceil() as u32)
                .max(1),
        };
        result.check_inline_ownership();
        Ok(result)
    }
}
impl PreparedAutoPan {
    pub(super) fn supports(&self, parameter: ParameterId) -> bool {
        supported(parameter)
            && parameter
                .value()
                .checked_sub(1)
                .and_then(|shift| 1u8.checked_shl(shift))
                .is_some_and(|bit| self.automation_mask & bit != 0)
    }
    pub(super) fn check_inline_ownership(&self) {
        let Self {
            settings,
            sample_rate,
            ramp_frames,
            automation_mask,
        } = self;
        let _: (&u32, &u32, &u8) = (sample_rate, ramp_frames, automation_mask);
        settings.check_inline();
    }
}
impl AutoPanSettings {
    fn check_inline(&self) {
        let Self {
            amount,
            rate_hz,
            right_phase_degrees,
            waveform,
        } = self;
        let _: (&f32, &f32, &f32, &Waveform) = (amount, rate_hz, right_phase_degrees, waveform);
    }
}
impl ScalarRamp {
    fn check_inline(&self) {
        let Self {
            current,
            target,
            step,
            remaining,
        } = self;
        let _: (&f64, &f64, &f64, &u32) = (current, target, step, remaining);
    }
}
impl AutoPanTargets {
    fn check_inline(&self) {
        let Self {
            amount,
            rate_hz,
            right_phase_degrees,
            waveform,
        } = self;
        let _: (&f64, &f64, &f64, &Waveform) = (amount, rate_hz, right_phase_degrees, waveform);
    }
}
impl AutoPanDspState {
    fn check_inline(&self) {
        let Self {
            sample_rate,
            ramp_frames,
            phase_cycles,
            amount,
            rate_hz,
            right_phase_degrees,
            waveform_weights,
            targets,
        } = self;
        let _: (&u32, &u32, &f64) = (sample_rate, ramp_frames, phase_cycles);
        amount.check_inline();
        rate_hz.check_inline();
        right_phase_degrees.check_inline();
        let _: &[ScalarRamp; 5] = waveform_weights;
        for ramp in waveform_weights {
            ramp.check_inline();
        }
        targets.check_inline();
    }
}
impl AutoPanRuntime {
    pub(super) fn check_inline_ownership(&self) {
        let Self {
            base,
            controls,
            dsp,
        } = self;
        base.check_inline_ownership();
        controls.check_inline(AutoPanSettings::check_inline);
        dsp.check_inline();
    }
    pub(super) fn process_automated(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn hex_lattice::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) = self
                .controls
                .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                let targets = AutoPanTargets::from(next.effective());
                // No fallible DSP derivation follows validated typed values.
                self.dsp.retarget(targets);
                self.controls.install(next);
            }
            let original = *frame;
            self.dsp.process_frame(frame);
            if bypassed {
                *frame = original;
            }
        }
        Ok(())
    }
}
