//! Typed effective controls for the shared host's borrowed automation values.
//! This adapter owns no envelope evaluation, timing, storage or project state.
use super::*;

impl SaturatorDefinition {
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedSaturator, DeviceContractError> {
        state.require_asset_free_processing()?;
        if !(8_000..=192_000).contains(&sample_rate)
            || parameters.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut automation_mask = 0u8;
        for parameter in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, *parameter)?;
            binding.semantics.validate(binding.descriptor)?;
            if !runtime_parameter_automation_supported(*parameter) {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            let shift = parameter
                .value()
                .checked_sub(1)
                .filter(|shift| *shift < 4)
                .ok_or(DeviceContractError::UnknownParameter)?;
            automation_mask |= 1u8
                .checked_shl(shift)
                .ok_or(DeviceContractError::UnknownParameter)?;
        }
        let _ = Kernel::shared();
        let settings = decode(state)?;
        let prepared = PreparedSaturator {
            automation_mask,
            settings,
            sample_rate,
            drive_gain: amplitude_from_db(settings.drive_db)?,
            output_gain: amplitude_from_db(settings.output_db)?,
        };
        prepared.check_inline_ownership();
        Ok(prepared)
    }
}

impl PreparedSaturator {
    pub(super) fn supports(&self, parameter: ParameterId) -> bool {
        runtime_parameter_automation_supported(parameter)
            && parameter
                .value()
                .checked_sub(1)
                .and_then(|shift| 1u8.checked_shl(shift))
                .is_some_and(|bit| self.automation_mask & bit != 0)
    }
    pub(super) fn check_inline_ownership(&self) {
        let Self {
            automation_mask,
            settings,
            sample_rate,
            drive_gain,
            output_gain,
        } = self;
        let _: (&u8, &u32, &f64, &f64) = (automation_mask, sample_rate, drive_gain, output_gain);
        settings.check_inline();
    }
}

impl SaturatorRuntime {
    pub(super) fn check_inline_ownership(&self) {
        let Self {
            base,
            controls,
            settings,
            sample_rate,
            gain_ramps,
            kernel,
            channels,
            delay,
            delay_index,
        } = self;
        base.check_inline_ownership();
        controls.check_inline(SaturatorSettings::check_inline);
        settings.check_inline();
        let _: (&u32, &[hex_dsp::LinearRamp; 2]) = (sample_rate, gain_ramps);
        let _: (
            &&'static Kernel,
            &[Oversampler; 2],
            &[[f64; 2]; LATENCY_FRAMES],
            &usize,
        ) = (kernel, channels, delay, delay_index);
    }
    pub(super) fn apply_automation(
        &mut self,
        values: &dyn hex_lattice::NativeParameterAutomation,
        resolved: &mut crate::parameter_binding::Resolved<4>,
        offset: usize,
    ) -> Result<(), DeviceContractError> {
        let Some(next) = self.controls.sample(&BINDINGS, resolved, values, offset)? else {
            return Ok(());
        };
        let settings = next.effective();
        // Held controls reuse scalar coefficients once direct ramps settle.
        // Changes use the original bounded gain conversion.
        let targets = [
            if settings.drive_db == self.settings.drive_db && self.gain_ramps[0].is_settled() {
                self.gain_ramps[0].value()
            } else {
                amplitude_from_db(settings.drive_db)?
            },
            if settings.output_db == self.settings.output_db && self.gain_ramps[1].is_settled() {
                self.gain_ramps[1].value()
            } else {
                amplitude_from_db(settings.output_db)?
            },
        ];
        let was_present = [self.controls.has_value(0), self.controls.has_value(1)];
        self.controls.install(next);
        let frames = gain_ramp_frames(self.sample_rate);
        for (index, target) in targets.into_iter().enumerate() {
            let parameter = [parameter::DRIVE, parameter::OUTPUT][index];
            // An omitted reading route keeps its existing immediate fallback.
            // A retained route with no value represents base selection instead.
            let omitted = self.base.supports(parameter)
                && values.parameter_ids().binary_search(&parameter).is_err();
            let ramp = &mut self.gain_ramps[index];
            if self.controls.has_value(index) || omitted {
                ramp.reset(target);
            } else {
                if was_present[index] && ramp.is_settled() && ramp.value() != target {
                    ramp.retarget(target, frames);
                }
                if ramp.is_settled() {
                    ramp.reset(target);
                }
            }
        }
        self.settings = settings;
        Ok(())
    }
}

impl SaturatorSettings {
    fn check_inline(&self) {
        let Self {
            drive_db,
            output_db,
            dry_wet,
            curve,
        } = self;
        let _: (&f32, &f32, &f32, &SaturationCurve) = (drive_db, output_db, dry_wet, curve);
    }
}
