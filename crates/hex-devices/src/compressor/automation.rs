//! Shared typed controls supply canonical targets; the existing compressor
//! alone owns detector, gain and mix smoothing. No authored curves are retained.
use super::*;

fn time_alpha(milliseconds: f32, sample_rate: u32) -> f64 {
    (-1.0 / (f64::from(milliseconds) * (f64::from(sample_rate) / 1_000.0))).exp()
}

impl PreparedCompressor {
    pub(super) fn new(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<Self, DeviceContractError> {
        state.require_asset_free_processing()?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
            || parameters.windows(2).any(|p| p[0] >= p[1])
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut admitted = [false; 8];
        for &id in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, id)?;
            binding.semantics.validate(binding.descriptor)?;
            if !binding.declares_arrangement_automation() {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            let index = BINDINGS
                .iter()
                .position(|b| b.descriptor.id == id)
                .ok_or(DeviceContractError::UnknownParameter)?;
            admitted[index] = true;
        }
        let settings = decode(state)?;
        Ok(Self {
            admitted,
            settings,
            sample_rate,
            attack_alpha: time_alpha(settings.attack_ms, sample_rate),
            release_alpha: time_alpha(settings.release_ms, sample_rate),
            gain_alpha: (-1.0 / (GAIN_SMOOTH_MS * (f64::from(sample_rate) / 1_000.0))).exp(),
            rms_alpha: (-1.0 / (RMS_INTEGRATION_SECONDS * f64::from(sample_rate))).exp(),
        })
    }
    pub(super) fn supports(&self, id: ParameterId) -> bool {
        BINDINGS
            .iter()
            .position(|b| b.descriptor.id == id)
            .is_some_and(|i| self.admitted[i])
    }
}

impl CompressorRuntime {
    pub(super) fn apply_values(
        &mut self,
        values: &dyn NativeParameterAutomation,
        resolved: &mut crate::parameter_binding::Resolved<8>,
        offset: usize,
    ) -> Result<(), DeviceContractError> {
        let Some(next) = self.controls.sample(&BINDINGS, resolved, values, offset)? else {
            return Ok(());
        };
        let settings = next.effective();
        if settings.attack_ms != self.settings.attack_ms {
            self.attack_alpha = time_alpha(settings.attack_ms, self.sample_rate);
        }
        if settings.release_ms != self.settings.release_ms {
            self.release_alpha = time_alpha(settings.release_ms, self.sample_rate);
        }
        if settings.dry_wet != self.settings.dry_wet {
            self.mix_remaining = if self.mix != f64::from(settings.dry_wet) {
                self.sample_rate.div_ceil(200)
            } else {
                0
            };
        }
        if settings.detector != self.settings.detector {
            self.detector_remaining =
                if self.detector_mix != f64::from(settings.detector == Detector::Rms) {
                    self.sample_rate.div_ceil(200)
                } else {
                    0
                };
        }
        self.controls.install(next);
        self.settings = settings;
        Ok(())
    }
    pub(super) fn process_automated(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.apply_values(values, &mut resolved, offset)?;
            self.process_frame(frame, *frame, bypassed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::EMPTY_DEVICE_RESOURCES as R;
    struct Values {
        ids: Vec<ParameterId>,
        value: Option<ParameterValue>,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            self.value
        }
    }
    #[test]
    fn seven_controls_match_established_state_handoff_pcm_and_fallback() {
        let base = COMPRESSOR_DEFINITION.default_state().unwrap();
        for (id, value) in [
            (parameter::THRESHOLD, -30.0),
            (parameter::RATIO, 10.0),
            (parameter::ATTACK, 1.0),
            (parameter::RELEASE, 10.0),
            (parameter::KNEE, 12.0),
            (parameter::MAKEUP, 12.0),
            (parameter::DRY_WET, 0.25),
        ] {
            let value = ParameterValue::Float(value);
            let mut dynamic = PreparedCompressor::new(&base, 8000, &[id])
                .unwrap()
                .create_runtime();
            let mut prior = PreparedCompressor::new(&base, 8000, &[])
                .unwrap()
                .create_runtime();
            let mut a = [[0.5, -0.25]; 137];
            let mut b = a;
            dynamic.process_block(&mut a, &R);
            prior.process_block(&mut b, &R);
            assert_eq!(a, b);
            let state = COMPRESSOR_DEFINITION
                .patch_parameter(&base, id, value)
                .unwrap();
            let mut reference = PreparedCompressor::new(&state, 8000, &[])
                .unwrap()
                .create_runtime();
            reference.copy_history_from(prior.as_ref());
            let mut values = Values {
                ids: vec![id],
                value: Some(value),
            };
            let signal: Vec<_> = (0..800)
                .map(|i| if i < 400 { [0.5, -0.25] } else { [0.02, -0.01] })
                .collect();
            let mut actual = signal.clone();
            let mut expected = signal;
            reference.process_block(&mut expected, &R);
            for block in actual.chunks_mut(13) {
                dynamic
                    .process_block_with_automation(block, None, &values, &R)
                    .unwrap();
            }
            assert_eq!(actual, expected, "parameter {}", id.value());
            let mut fallback = PreparedCompressor::new(&base, 8000, &[])
                .unwrap()
                .create_runtime();
            fallback.copy_history_from(reference.as_ref());
            values.value = None;
            let mut actual = [[0.2, -0.1]; 99];
            let mut expected = actual;
            dynamic
                .process_block_with_automation(&mut actual, None, &values, &R)
                .unwrap();
            fallback.process_block(&mut expected, &R);
            assert_eq!(actual, expected, "fallback {}", id.value());
        }
    }
    #[test]
    fn metadata_mapping_and_invalid_delivery_are_explicit_and_atomic() {
        let state = COMPRESSOR_DEFINITION.default_state().unwrap();
        for binding in &BINDINGS {
            binding.semantics.validate(binding.descriptor).unwrap();
            assert_eq!(
                COMPRESSOR_DEFINITION
                    .supports_parameter_automation(&state, binding.descriptor.id)
                    .unwrap(),
                binding.descriptor.id != parameter::DETECTOR
            );
        }
        assert!(PreparedCompressor::new(&state, 8000, &[parameter::DETECTOR]).is_err());
        assert!(
            PreparedCompressor::new(&state, 8000, &[parameter::RATIO, parameter::RATIO]).is_err()
        );
        let attack = &BINDINGS[2];
        let ParameterValue::Float(mid) = attack
            .semantics
            .normalization
            .denormalize(attack.descriptor, 0.5)
            .unwrap()
        else {
            panic!("attack");
        };
        assert!((mid - 10.0_f32.sqrt()).abs() < 1e-5);
        let prepared = PreparedCompressor::new(&state, 8000, &[parameter::RATIO]).unwrap();
        let mut actual = prepared.create_runtime();
        let mut reference = prepared.create_runtime();
        let mut bad = Values {
            ids: vec![parameter::RATIO],
            value: Some(ParameterValue::Float(f32::NAN)),
        };
        let mut output = [[0.5; 2]; 5];
        assert!(
            actual
                .process_block_with_automation(&mut output, None, &bad, &R)
                .is_err()
        );
        assert_eq!(output, [[0.5; 2]; 5]);
        bad.value = Some(ParameterValue::Float(2.0));
        let mut expected = output;
        actual
            .process_block_with_automation(&mut output, None, &bad, &R)
            .unwrap();
        reference
            .process_block_with_automation(&mut expected, None, &bad, &R)
            .unwrap();
        assert_eq!(output, expected);
    }
    #[test]
    fn automated_history_bypass_and_auxiliary_keep_the_original_detector() {
        let state = COMPRESSOR_DEFINITION.default_state().unwrap();
        let prepared = PreparedCompressor::new(&state, 8000, &[parameter::MAKEUP]).unwrap();
        let values = Values {
            ids: vec![parameter::MAKEUP],
            value: Some(ParameterValue::Float(6.0)),
        };
        let mut source = prepared.create_runtime();
        source
            .process_block_with_automation(&mut [[0.5; 2]; 19], None, &values, &R)
            .unwrap();
        let mut copied = prepared.create_runtime();
        copied.copy_history_from(source.as_ref());
        let mut bypassed = prepared.create_runtime();
        bypassed.copy_history_from(source.as_ref());
        let mut a = [[0.5; 2]; 13];
        let mut b = a;
        source
            .process_block_with_automation(&mut a, None, &values, &R)
            .unwrap();
        copied
            .process_block_with_automation(&mut b, None, &values, &R)
            .unwrap();
        assert_eq!(a, b);
        b.fill([0.5; 2]);
        bypassed
            .process_bypassed_block_with_automation(&mut b, None, &values, &R)
            .unwrap();
        assert_eq!(b, [[0.5; 2]; 13]);
        a.fill([0.5; 2]);
        source
            .process_block_with_automation(&mut a, None, &values, &R)
            .unwrap();
        bypassed
            .process_block_with_automation(&mut b, None, &values, &R)
            .unwrap();
        assert_eq!(a, b);
        let mut aux = prepared.create_runtime();
        let mut self_key = prepared.create_runtime();
        let mut a = [[0.5; 2]; 13];
        let mut b = a;
        let key = a;
        aux.process_block_with_auxiliary(
            StereoAuxiliaryBlock::new(&mut a, Some(&key), &mut [], 13).unwrap(),
            None,
            Some(&values),
            false,
            &R,
        )
        .unwrap();
        self_key
            .process_block_with_automation(&mut b, None, &values, &R)
            .unwrap();
        assert_eq!(a, b);
    }
    #[test]
    fn canonical_mode_and_base_changes_reach_a_runtime_with_an_unchanged_lane() {
        let base = COMPRESSOR_DEFINITION.default_state().unwrap();
        let values = Values {
            ids: vec![parameter::MAKEUP],
            value: Some(ParameterValue::Float(6.0)),
        };
        let mut source = PreparedCompressor::new(&base, 8000, &values.ids)
            .unwrap()
            .create_runtime();
        source
            .process_block_with_automation(&mut [[0.5; 2]; 83], None, &values, &R)
            .unwrap();
        let changed = COMPRESSOR_DEFINITION
            .patch_parameter(&base, parameter::DETECTOR, ParameterValue::Choice(1))
            .unwrap();
        let changed = COMPRESSOR_DEFINITION
            .patch_parameter(&changed, parameter::DRY_WET, ParameterValue::Float(0.25))
            .unwrap();
        let mut candidate = PreparedCompressor::new(&changed, 8000, &values.ids)
            .unwrap()
            .create_runtime();
        candidate.copy_history_from(source.as_ref());
        let reference_state = COMPRESSOR_DEFINITION
            .patch_parameter(&changed, parameter::MAKEUP, ParameterValue::Float(6.0))
            .unwrap();
        let mut reference = PreparedCompressor::new(&reference_state, 8000, &[])
            .unwrap()
            .create_runtime();
        reference.copy_history_from(source.as_ref());
        let mut actual = [[0.01; 2]; 199];
        let mut expected = actual;
        candidate
            .process_block_with_automation(&mut actual, None, &values, &R)
            .unwrap();
        reference.process_block(&mut expected, &R);
        assert_eq!(actual, expected);
        candidate.reset();
        let mut fresh = PreparedCompressor::new(&changed, 8000, &[])
            .unwrap()
            .create_runtime();
        let absent = Values {
            ids: vec![],
            value: None,
        };
        actual.fill([0.1; 2]);
        expected = actual;
        candidate
            .process_block_with_automation(&mut actual, None, &absent, &R)
            .unwrap();
        fresh.process_block(&mut expected, &R);
        assert_eq!(actual, expected);
    }
}
