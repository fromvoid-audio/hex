//! Borrowed final values use the shared binding cache; Gate owns only its
//! coefficients and detector/envelope history. Invalid coupled values leave
//! the current frame and runtime unchanged, allowing the host to refuse output.
use super::*;

impl PreparedGate {
    pub(super) fn new(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<Self, DeviceContractError> {
        state.require_asset_free_processing()?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
            || parameters.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut admitted = [false; 6];
        for &id in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, id)?;
            binding.semantics.validate(binding.descriptor)?;
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
            threshold: db_to_gain(settings.threshold_db)?,
            return_level: db_to_gain(settings.return_db)?,
            floor_gain: db_to_gain(settings.floor_db)?,
            attack_frames: checked_frames(settings.attack_ms, sample_rate)?,
            hold_frames: checked_frames(settings.hold_ms.max(MIN_HOLD_MS), sample_rate)?,
            release_frames: checked_frames(settings.release_ms, sample_rate)?,
        })
    }

    pub(super) fn supports(&self, id: ParameterId) -> bool {
        BINDINGS
            .iter()
            .position(|b| b.descriptor.id == id)
            .is_some_and(|index| self.admitted[index])
    }
}

impl GateRuntime {
    pub(super) fn install_settings(&mut self, settings: GateSettings) {
        let previous = self.settings;
        self.settings = settings;
        self.retarget_envelope(previous);
    }

    pub(super) fn retarget_envelope(&mut self, previous: GateSettings) {
        let attack_changed = self.settings.attack_ms != previous.attack_ms;
        let release_changed = self.settings.release_ms != previous.release_ms;
        let floor_changed = self.settings.floor_db != previous.floor_db;
        if self.open && attack_changed && self.ramp_remaining > 0 && self.ramp_target == 1.0 {
            self.start_ramp(1.0, self.attack_frames);
        } else if !self.open && (floor_changed || (release_changed && self.ramp_remaining > 0)) {
            self.start_ramp(self.floor_gain, self.release_frames);
        }
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
                let settings = next.effective();
                validate(settings)?;
                // Stage every fallible conversion before installing the candidate.
                let threshold = if settings.threshold_db == self.settings.threshold_db {
                    self.threshold
                } else {
                    db_to_gain(settings.threshold_db)?
                };
                let return_level = if settings.return_db == self.settings.return_db {
                    self.return_level
                } else {
                    db_to_gain(settings.return_db)?
                };
                let floor_gain = if settings.floor_db == self.settings.floor_db {
                    self.floor_gain
                } else {
                    db_to_gain(settings.floor_db)?
                };
                let attack = checked_frames(settings.attack_ms, self.sample_rate)?;
                let hold = checked_frames(settings.hold_ms.max(MIN_HOLD_MS), self.sample_rate)?;
                let release = checked_frames(settings.release_ms, self.sample_rate)?;
                self.controls.install(next);
                self.threshold = threshold;
                self.return_level = return_level;
                self.floor_gain = floor_gain;
                self.attack_frames = attack;
                self.hold_frames = hold;
                self.release_frames = release;
                self.install_settings(settings);
            }
            let original = *frame;
            self.process_frame(frame);
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
        values: Vec<Option<ParameterValue>>,
    }
    impl hex_lattice::NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
            self.ids
                .iter()
                .position(|&p| p == id)
                .and_then(|i| self.values[i])
        }
    }
    fn values(settings: GateSettings) -> Values {
        Values {
            ids: BINDINGS.iter().map(|b| b.descriptor.id).collect(),
            values: BINDINGS.iter().map(|b| Some(b.read(&settings))).collect(),
        }
    }
    #[test]
    fn declared_mapping_and_admission_are_explicit() {
        let state = GATE_DEFINITION.default_state().unwrap();
        for binding in &BINDINGS {
            binding.semantics.validate(binding.descriptor).unwrap();
            assert!(
                GATE_DEFINITION
                    .supports_parameter_automation(&state, binding.descriptor.id)
                    .unwrap()
            );
        }
        let attack = &BINDINGS[3];
        let midpoint = attack
            .semantics
            .normalization
            .denormalize(attack.descriptor, 0.5)
            .unwrap();
        let ParameterValue::Float(value) = midpoint else {
            panic!("float attack");
        };
        assert!((value - 10.0_f32.sqrt()).abs() < 1e-5);
    }
    #[test]
    fn all_controls_match_static_pcm_across_hold_hysteresis_and_irregular_blocks() {
        let base = encode(GateSettings {
            floor_db: -20.0,
            ..GateSettings::default()
        })
        .unwrap();
        let settings = GateSettings {
            threshold_db: -12.0,
            return_db: -18.0,
            floor_db: -20.0,
            attack_ms: 2.0,
            hold_ms: 7.0,
            release_ms: 3.0,
        };
        let input = values(settings);
        let mut actual = PreparedGate::new(&base, 8000, &input.ids)
            .unwrap()
            .create_runtime();
        let mut reference = PreparedGate::new(&encode(settings).unwrap(), 8000, &[])
            .unwrap()
            .create_runtime();
        let signal: Vec<_> = (0..240)
            .map(|i| {
                if i < 40 {
                    [0.5, -0.25]
                } else if i < 80 {
                    [0.2, -0.1]
                } else {
                    [0.0625, -0.03125]
                }
            })
            .collect();
        let mut expected = signal.clone();
        reference.process_block(&mut expected, &R);
        let mut output = signal;
        let mut start = 0;
        for count in [1, 13, 7, 31].into_iter().cycle() {
            if start == output.len() {
                break;
            }
            let end = (start + count).min(output.len());
            actual
                .process_block_with_automation(&mut output[start..end], None, &input, &R)
                .unwrap();
            start = end;
        }
        assert_eq!(output, expected);
        assert!((output[15][0] - 0.5).abs() < 1e-6);
        assert!((output[200][0] - 0.00625).abs() < 1e-6);
        // Removing every target restores the immutable base, including after reset.
        actual.reset();
        let absent = Values {
            ids: vec![],
            values: vec![],
        };
        let mut output = [[0.001; 2]; 4];
        actual
            .process_block_with_automation(&mut output, None, &absent, &R)
            .unwrap();
        assert!(output.iter().all(|s| (s[0] - 0.0001).abs() < 1e-10));
    }
    #[test]
    fn conflicting_pair_refuses_atomically_and_valid_pair_can_retry() {
        let base = GATE_DEFINITION.default_state().unwrap();
        let settings = GateSettings {
            threshold_db: -60.0,
            return_db: -65.0,
            ..GateSettings::default()
        };
        let mut input = values(settings);
        let prepared = PreparedGate::new(&base, 8000, &input.ids).unwrap();
        let mut actual = prepared.create_runtime();
        let mut reference = prepared.create_runtime();
        input.values[1] = Some(ParameterValue::Float(-10.0));
        let mut output = [[0.5; 2]; 1];
        assert_eq!(
            actual.process_block_with_automation(&mut output, None, &input, &R),
            Err(DeviceContractError::InvalidParameterValue)
        );
        assert_eq!(output, [[0.5; 2]; 1]);
        input.values[1] = Some(ParameterValue::Float(-65.0));
        actual
            .process_block_with_automation(&mut output, None, &input, &R)
            .unwrap();
        let mut expected = [[0.5; 2]; 1];
        reference
            .process_block_with_automation(&mut expected, None, &input, &R)
            .unwrap();
        assert_eq!(output, expected);
        assert!(PreparedGate::new(&base, 8000, &[parameter::HOLD, parameter::THRESHOLD]).is_err());
        assert!(PreparedGate::new(&base, 8000, &[ParameterId::from_static(7)]).is_err());
    }
    #[test]
    fn copied_automated_history_and_bypass_keep_the_same_envelope() {
        let base = GATE_DEFINITION.default_state().unwrap();
        let input = values(GateSettings {
            attack_ms: 20.0,
            ..GateSettings::default()
        });
        let prepared = PreparedGate::new(&base, 8000, &input.ids).unwrap();
        let mut source = prepared.create_runtime();
        source
            .process_block_with_automation(&mut [[0.5; 2]; 17], None, &input, &R)
            .unwrap();
        let mut copied = prepared.create_runtime();
        copied.copy_history_from(source.as_ref());
        let mut bypassed = prepared.create_runtime();
        bypassed.copy_history_from(source.as_ref());
        let mut a = [[0.5; 2]; 19];
        let mut b = a;
        source
            .process_block_with_automation(&mut a, None, &input, &R)
            .unwrap();
        copied
            .process_block_with_automation(&mut b, None, &input, &R)
            .unwrap();
        assert_eq!(a, b);
        b.fill([0.5; 2]);
        bypassed
            .process_bypassed_block_with_automation(&mut b, None, &input, &R)
            .unwrap();
        assert_eq!(b, [[0.5; 2]; 19]);
        a.fill([0.5; 2]);
        source
            .process_block_with_automation(&mut a, None, &input, &R)
            .unwrap();
        bypassed
            .process_block_with_automation(&mut b, None, &input, &R)
            .unwrap();
        assert_eq!(a, b);
    }
}
