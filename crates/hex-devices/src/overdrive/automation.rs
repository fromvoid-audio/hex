//! Borrowed canonical controls retarget the original 5 ms coefficient ramps.
//! Only changed coefficient groups perform bounded conversion; held targets
//! retain the original trajectory. No curves, clocks or resources are owned here.
use super::*;
impl PreparedOverdrive {
    pub(super) fn new(
        state: &NativeDeviceState,
        rate: u32,
        parameters: &[ParameterId],
    ) -> Result<Self, DeviceContractError> {
        state.require_asset_free_processing()?;
        validate_rate(rate)?;
        if parameters.windows(2).any(|p| p[0] >= p[1]) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut admitted = [false; 6];
        for &id in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, id)?;
            binding.semantics.validate(binding.descriptor)?;
            admitted[index(id)?] = true;
        }
        let settings = decode(state)?;
        Ok(Self {
            settings,
            admitted,
            targets: targets(settings, rate),
            rate,
            kernel: Kernel::new(),
        })
    }
    pub(super) fn supports(&self, id: ParameterId) -> bool {
        index(id).is_ok_and(|i| self.admitted[i])
    }
}
impl OverdriveRuntime {
    fn apply_values(
        &mut self,
        values: &dyn hex_lattice::NativeParameterAutomation,
        resolved: &mut crate::parameter_binding::Resolved<6>,
        offset: usize,
    ) -> Result<(), DeviceContractError> {
        let Some(next) = self.controls.sample(&BINDINGS, resolved, values, offset)? else {
            return Ok(());
        };
        let settings = next.effective();
        let old = self.controls.effective();
        let mut target = self.targets;
        if settings.0[..2] != old.0[..2] {
            let [hp, lp] = band_poles(settings, self.rate);
            target[0] = hp;
            target[1] = lp;
        }
        if settings.0[2] != old.0[2] {
            target[2] = 10.0_f64.powf(f64::from(settings.0[2]) * 1.5);
        }
        if settings.0[3] != old.0[3] {
            target[3] = pole(
                (500.0 * 40.0_f64.powf(f64::from(settings.0[3]))).min(f64::from(self.rate) * 0.45),
                f64::from(self.rate),
            );
        }
        target[4] = f64::from(settings.0[4]);
        target[5] = f64::from(settings.0[5]);
        self.controls.install(next);
        if target != self.targets {
            self.targets = target;
            self.remaining = (self.rate / 200).max(1);
            for (step, (&target, &current)) in self
                .step
                .iter_mut()
                .zip(self.targets.iter().zip(&self.current))
            {
                *step = (target - current) / f64::from(self.remaining);
            }
        }
        Ok(())
    }
    pub(super) fn process_automated(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn hex_lattice::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.apply_values(values, &mut resolved, offset)?;
            let original = *frame;
            self.frame(frame);
            if bypassed {
                *frame = original;
            }
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
    impl hex_lattice::NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            self.value
        }
    }
    #[test]
    fn all_six_controls_match_established_ramped_pcm_and_base_fallback() {
        let state = OVERDRIVE_DEFINITION.default_state().unwrap();
        for (id, value) in [
            (parameter::FREQUENCY, 500.0),
            (parameter::BANDWIDTH, 2.0),
            (parameter::DRIVE, 0.9),
            (parameter::TONE, 0.1),
            (parameter::DYNAMICS, 0.8),
            (parameter::DRY_WET, 0.0),
        ] {
            let mut actual = PreparedOverdrive::new(&state, 8000, &[id])
                .unwrap()
                .create_runtime();
            let mut old = PreparedOverdrive::new(&state, 8000, &[])
                .unwrap()
                .create_runtime();
            let mut a = [[0.15, -0.2]; 73];
            let mut b = a;
            actual.process_block(&mut a, &R);
            old.process_block(&mut b, &R);
            assert_eq!(a, b);
            let value = ParameterValue::Float(value);
            let changed = OVERDRIVE_DEFINITION
                .patch_parameter(&state, id, value)
                .unwrap();
            let mut reference = PreparedOverdrive::new(&changed, 8000, &[])
                .unwrap()
                .create_runtime();
            reference.copy_history_from(old.as_ref());
            let mut values = Values {
                ids: vec![id],
                value: Some(value),
            };
            let signal: Vec<_> = (0..513)
                .map(|i| {
                    let v = (i as f32 * 0.23).sin() * 0.2;
                    [v, -v * 0.3]
                })
                .collect();
            let mut output = signal.clone();
            let mut expected = signal;
            reference.process_block(&mut expected, &R);
            for block in output.chunks_mut(13) {
                actual
                    .process_block_with_automation(block, None, &values, &R)
                    .unwrap();
            }
            assert_eq!(output, expected, "control {}", id.value());
            let mut fallback = PreparedOverdrive::new(&state, 8000, &[])
                .unwrap()
                .create_runtime();
            fallback.copy_history_from(reference.as_ref());
            values.value = None;
            let mut a = [[0.13, -0.1]; 113];
            let mut b = a;
            actual
                .process_block_with_automation(&mut a, None, &values, &R)
                .unwrap();
            fallback.process_block(&mut b, &R);
            assert_eq!(a, b, "fallback {}", id.value());
        }
    }
    #[test]
    fn mapping_unprepared_and_invalid_targets_refuse_without_poisoning_history() {
        let state = OVERDRIVE_DEFINITION.default_state().unwrap();
        for b in &BINDINGS {
            b.semantics.validate(b.descriptor).unwrap();
            assert!(
                OVERDRIVE_DEFINITION
                    .supports_parameter_automation(&state, b.descriptor.id)
                    .unwrap()
            );
        }
        let frequency = &BINDINGS[0];
        let ParameterValue::Float(mid) = frequency
            .semantics
            .normalization
            .denormalize(frequency.descriptor, 0.5)
            .unwrap()
        else {
            panic!("frequency");
        };
        assert!((mid - 1000.0).abs() < 0.001);
        assert!(
            PreparedOverdrive::new(&state, 8000, &[parameter::DRIVE, parameter::FREQUENCY])
                .is_err()
        );
        assert!(PreparedOverdrive::new(&state, 8000, &[ParameterId::from_static(7)]).is_err());
        let prepared = PreparedOverdrive::new(&state, 8000, &[parameter::DRIVE]).unwrap();
        let mut actual = prepared.create_runtime();
        let mut reference = prepared.create_runtime();
        let mut values = Values {
            ids: vec![parameter::DRIVE],
            value: Some(ParameterValue::Float(f32::NAN)),
        };
        let mut a = [[0.1; 2]; 17];
        assert!(
            actual
                .process_block_with_automation(&mut a, None, &values, &R)
                .is_err()
        );
        assert_eq!(a, [[0.1; 2]; 17]);
        values.value = Some(ParameterValue::Float(0.9));
        let mut b = a;
        actual
            .process_block_with_automation(&mut a, None, &values, &R)
            .unwrap();
        reference
            .process_block_with_automation(&mut b, None, &values, &R)
            .unwrap();
        assert_eq!(a, b);
    }
    #[test]
    fn automated_handoff_bypass_and_reset_keep_filter_envelope_and_latency_state() {
        let state = OVERDRIVE_DEFINITION.default_state().unwrap();
        let prepared = PreparedOverdrive::new(&state, 8000, &[parameter::TONE]).unwrap();
        let values = Values {
            ids: vec![parameter::TONE],
            value: Some(ParameterValue::Float(0.1)),
        };
        let mut source = prepared.create_runtime();
        source
            .process_block_with_automation(&mut [[0.15; 2]; 19], None, &values, &R)
            .unwrap();
        let mut copied = prepared.create_runtime();
        copied.copy_history_from(source.as_ref());
        let mut bypassed = prepared.create_runtime();
        bypassed.copy_history_from(source.as_ref());
        let mut a = [[0.15; 2]; 13];
        let mut b = a;
        source
            .process_block_with_automation(&mut a, None, &values, &R)
            .unwrap();
        copied
            .process_block_with_automation(&mut b, None, &values, &R)
            .unwrap();
        assert_eq!(a, b);
        b.fill([0.15; 2]);
        bypassed
            .process_bypassed_block_with_automation(&mut b, None, &values, &R)
            .unwrap();
        assert_eq!(b, [[0.15; 2]; 13]);
        a.fill([0.15; 2]);
        source
            .process_block_with_automation(&mut a, None, &values, &R)
            .unwrap();
        bypassed
            .process_block_with_automation(&mut b, None, &values, &R)
            .unwrap();
        assert_eq!(a, b);
        copied.reset();
        let mut fresh = PreparedOverdrive::new(&state, 8000, &[])
            .unwrap()
            .create_runtime();
        let absent = Values {
            ids: vec![],
            value: None,
        };
        a.fill([0.15; 2]);
        b = a;
        copied
            .process_block_with_automation(&mut a, None, &absent, &R)
            .unwrap();
        fresh.process_block(&mut b, &R);
        assert_eq!(a, b);
    }
}
