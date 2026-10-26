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
        parameter
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
        fn inline<T: Copy>(_: &T) {}
        inline(automation_mask);
        inline(settings);
        inline(sample_rate);
        inline(drive_gain);
        inline(output_gain);
    }
}

impl SaturatorRuntime {
    pub(super) fn check_inline_ownership(&self) {
        let Self {
            base,
            settings,
            sample_rate,
            drive_gain,
            output_gain,
        } = self;
        fn inline<T: Copy>(_: &T) {}
        inline(base);
        inline(settings);
        inline(sample_rate);
        inline(drive_gain);
        inline(output_gain);
    }
    pub(super) fn apply_automation(
        &mut self,
        values: &dyn hex_lattice::NativeParameterAutomation,
        offset: usize,
    ) -> Result<(), DeviceContractError> {
        let mut settings = self.base.settings;
        for &parameter in values.parameter_ids() {
            if !self.base.supports(parameter) {
                return Err(DeviceContractError::UnknownParameter);
            }
            let Some(value) = values.value_at(parameter, offset) else {
                continue;
            };
            let descriptor = DESCRIPTORS
                .iter()
                .find(|descriptor| descriptor.id == parameter)
                .ok_or(DeviceContractError::UnknownParameter)?;
            descriptor.validate(value)?;
            match (parameter, value) {
                (parameter::DRIVE, ParameterValue::Float(value)) => settings.drive_db = value,
                (parameter::OUTPUT, ParameterValue::Float(value)) => settings.output_db = value,
                (parameter::DRY_WET, ParameterValue::Float(value)) => settings.dry_wet = value,
                (parameter::CURVE, ParameterValue::Choice(0)) => {
                    settings.curve = SaturationCurve::SoftClip
                }
                (parameter::CURVE, ParameterValue::Choice(1)) => {
                    settings.curve = SaturationCurve::DigitalClip
                }
                _ => return Err(DeviceContractError::InvalidParameterValue),
            }
        }
        // Constant/held controls reuse scalar coefficients; only a changing dB
        // value evaluates the original bounded gain conversion.
        let drive = if settings.drive_db == self.settings.drive_db {
            self.drive_gain
        } else {
            amplitude_from_db(settings.drive_db)?
        };
        let output = if settings.output_db == self.settings.output_db {
            self.output_gain
        } else {
            amplitude_from_db(settings.output_db)?
        };
        self.settings = settings;
        self.drive_gain = drive;
        self.output_gain = output;
        Ok(())
    }
}
