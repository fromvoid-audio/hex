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
            drive_gain,
            output_gain,
            kernel,
            channels,
            delay,
            delay_index,
            gain_ramp_remaining,
        } = self;
        base.check_inline_ownership();
        controls.check_inline(SaturatorSettings::check_inline);
        settings.check_inline();
        let _: (&u32, &f64, &f64, &u32) =
            (sample_rate, drive_gain, output_gain, gain_ramp_remaining);
        let _: (
            &Kernel,
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
        // Constant/held controls reuse scalar coefficients; only a changing dB
        // value evaluates the original bounded gain conversion.
        let drive = if (!self.base.supports(parameter::DRIVE) && self.gain_ramp_remaining != 0)
            || settings.drive_db == self.settings.drive_db
        {
            self.drive_gain
        } else {
            amplitude_from_db(settings.drive_db)?
        };
        let output = if (!self.base.supports(parameter::OUTPUT) && self.gain_ramp_remaining != 0)
            || settings.output_db == self.settings.output_db
        {
            self.output_gain
        } else {
            amplitude_from_db(settings.output_db)?
        };
        self.controls.install(next);
        self.settings = settings;
        self.drive_gain = drive;
        self.output_gain = output;
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
