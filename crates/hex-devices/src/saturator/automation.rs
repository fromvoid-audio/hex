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
        let mut automation_mask = 0u32;
        for parameter in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, *parameter)?;
            binding.semantics.validate(binding.descriptor)?;
            if !runtime_parameter_automation_supported(*parameter) {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            let shift = parameter
                .value()
                .checked_sub(1)
                .filter(|shift| (*shift as usize) < PARAM_COUNT)
                .ok_or(DeviceContractError::UnknownParameter)?;
            automation_mask |= 1u32
                .checked_shl(shift)
                .ok_or(DeviceContractError::UnknownParameter)?;
        }
        let _ = Kernel::shared();
        let settings = decode(state)?;
        let (pre_base, post_base) = base_filter_pair(sample_rate, settings.base_db)?;
        let (pre_color, post_color) = color_filter_pair(sample_rate, settings)?;
        let prepared = PreparedSaturator {
            automation_mask,
            settings,
            sample_rate,
            drive_gain: amplitude_from_db(settings.drive_db)?,
            output_gain: amplitude_from_db(settings.output_db)?,
            pre_base,
            post_base,
            pre_color,
            post_color,
            dc_pole: dc_blocker_pole(sample_rate),
            tail_limit: tail_limit_frames(sample_rate, settings, automation_mask),
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
                .and_then(|shift| 1u32.checked_shl(shift))
                .is_some_and(|bit| self.automation_mask & bit != 0)
    }
    pub(super) fn check_inline_ownership(&self) {
        let Self {
            automation_mask,
            settings,
            sample_rate,
            drive_gain,
            output_gain,
            pre_base,
            post_base,
            pre_color,
            post_color,
            dc_pole,
            tail_limit,
        } = self;
        let _: (&u32, &u32, &f64, &f64, &f64, &u32) = (
            automation_mask,
            sample_rate,
            drive_gain,
            output_gain,
            dc_pole,
            tail_limit,
        );
        let _: (
            &hex_dsp::BiquadCoefficients,
            &hex_dsp::BiquadCoefficients,
            &hex_dsp::BiquadCoefficients,
            &hex_dsp::BiquadCoefficients,
        ) = (pre_base, post_base, pre_color, post_color);
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
            pre_base,
            post_base,
            pre_color,
            post_color,
            dc_pole,
            dc_in,
            dc_out,
            ws_tag,
            silent_frames,
            tail_limit,
        } = self;
        base.check_inline_ownership();
        controls.check_inline(SaturatorSettings::check_inline);
        settings.check_inline();
        let _: (&u32, &[hex_dsp::LinearRamp; 2], &f64, &[f64; 2], &[f64; 2]) =
            (sample_rate, gain_ramps, dc_pole, dc_in, dc_out);
        let _: (
            &&'static Kernel,
            &[Oversampler; 2],
            &[[f64; 2]; LATENCY_FRAMES],
            &usize,
        ) = (kernel, channels, delay, delay_index);
        let _: (&u8, &u32, &u32) = (ws_tag, silent_frames, tail_limit);
        let _: (
            &[hex_dsp::Biquad; 2],
            &[hex_dsp::Biquad; 2],
            &[hex_dsp::Biquad; 2],
            &[hex_dsp::Biquad; 2],
        ) = (pre_base, post_base, pre_color, post_color);
    }
    pub(super) fn apply_automation(
        &mut self,
        values: &dyn hex_lattice::NativeParameterAutomation,
        resolved: &mut crate::parameter_binding::Resolved<PARAM_COUNT>,
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
        let base_changed = settings.base_db != self.settings.base_db;
        let color_changed = settings.color != self.settings.color
            || settings.color_freq_hz != self.settings.color_freq_hz
            || settings.color_width != self.settings.color_width
            || settings.color_depth_db != self.settings.color_depth_db;
        let ws_changed = settings.waveshaper_differs(self.settings);

        let base_pair = if base_changed {
            Some(base_filter_pair(self.sample_rate, settings.base_db)?)
        } else {
            None
        };
        let color_pair = if color_changed {
            Some(color_filter_pair(self.sample_rate, settings)?)
        } else {
            None
        };

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
        if let Some((pre, post)) = base_pair {
            for channel in 0..2 {
                self.pre_base[channel].set_coefficients(pre);
                self.post_base[channel].set_coefficients(post);
                if !settings.base_active() {
                    self.pre_base[channel].reset();
                    self.post_base[channel].reset();
                }
            }
        }
        if let Some((pre, post)) = color_pair {
            for channel in 0..2 {
                self.pre_color[channel].set_coefficients(pre);
                self.post_color[channel].set_coefficients(post);
                if !settings.color_active() {
                    self.pre_color[channel].reset();
                    self.post_color[channel].reset();
                }
            }
        }
        if !settings.dc_filter && self.settings.dc_filter {
            self.dc_in = [0.0; 2];
            self.dc_out = [0.0; 2];
        }
        if ws_changed {
            self.flip_ws_tag();
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
            base_db,
            color,
            color_freq_hz,
            color_width,
            color_depth_db,
            soft_clip_out,
            dc_filter,
            ws_drive,
            ws_curve,
            ws_depth,
            ws_lin,
            ws_damp,
            ws_period,
        } = self;
        let _: (
            &f32,
            &f32,
            &f32,
            &SaturationCurve,
            &f32,
            &bool,
            &f32,
            &f32,
            &f32,
            &bool,
            &bool,
        ) = (
            drive_db,
            output_db,
            dry_wet,
            curve,
            base_db,
            color,
            color_freq_hz,
            color_width,
            color_depth_db,
            soft_clip_out,
            dc_filter,
        );
        let _: (&f32, &f32, &f32, &f32, &f32, &f32) =
            (ws_drive, ws_curve, ws_depth, ws_lin, ws_damp, ws_period);
    }
}
