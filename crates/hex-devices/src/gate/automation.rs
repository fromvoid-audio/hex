//! Borrowed final values use the shared binding cache; Gate owns only its
//! coefficients and detector/envelope history. Invalid coupled values leave
//! the current frame and runtime unchanged, allowing the host to refuse output.
use super::*;

impl GateSettings {
    fn check_inline(&self) {
        let Self {
            threshold_db,
            return_db,
            floor_db,
            attack_ms,
            hold_ms,
            release_ms,
            flip,
            lookahead,
            sidechain_gain_db,
            sidechain_mix,
            sidechain_eq_mode,
            sidechain_eq_freq_hz,
            sidechain_eq_q,
        } = self;
        let _: [&f32; 10] = [
            threshold_db,
            return_db,
            floor_db,
            attack_ms,
            hold_ms,
            release_ms,
            sidechain_gain_db,
            sidechain_mix,
            sidechain_eq_freq_hz,
            sidechain_eq_q,
        ];
        let _: &bool = flip;
        let _: &GateLookahead = lookahead;
        let _: &SidechainEqMode = sidechain_eq_mode;
    }
}

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
        let mut admitted = [false; PARAM_COUNT];
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
        let sidechain_gain_linear = if settings.sidechain_gain_db == 0.0 {
            1.0
        } else {
            db_to_gain(settings.sidechain_gain_db)?
        };
        let prepared = Self {
            admitted,
            settings,
            sample_rate,
            threshold: db_to_gain(settings.threshold_db)?,
            return_level: db_to_gain(settings.return_db)?,
            floor_gain: db_to_gain(settings.floor_db)?,
            attack_frames: checked_frames(settings.attack_ms, sample_rate)?,
            hold_frames: checked_frames(settings.hold_ms.max(MIN_HOLD_MS), sample_rate)?,
            release_frames: checked_frames(settings.release_ms, sample_rate)?,
            sidechain_gain_linear,
            lookahead_frames: settings.lookahead.frames(sample_rate),
        };
        prepared.check_inline();
        Ok(prepared)
    }

    fn check_inline(&self) {
        let Self {
            admitted,
            settings,
            sample_rate,
            threshold,
            return_level,
            floor_gain,
            attack_frames,
            hold_frames,
            release_frames,
            sidechain_gain_linear,
            lookahead_frames,
        } = self;
        settings.check_inline();
        let _: &[bool; PARAM_COUNT] = admitted;
        let _: [&f64; 4] = [threshold, return_level, floor_gain, sidechain_gain_linear];
        let _: [&u32; 5] = [
            sample_rate,
            attack_frames,
            hold_frames,
            release_frames,
            lookahead_frames,
        ];
    }

    pub(super) fn supports(&self, id: ParameterId) -> bool {
        BINDINGS
            .iter()
            .position(|b| b.descriptor.id == id)
            .is_some_and(|index| self.admitted[index])
    }
}

impl GateRuntime {
    pub(super) fn check_inline_ownership(&self) {
        let Self {
            base,
            controls,
            settings,
            sample_rate,
            threshold,
            return_level,
            floor_gain,
            attack_frames,
            hold_frames,
            release_frames,
            sidechain_gain_linear,
            lookahead_frames,
            open,
            hold_elapsed,
            gain_ramp,
            meter,
            sidechain_eq,
            lookahead_pos,
            lookahead_buf,
        } = self;
        base.check_inline();
        controls.check_inline(GateSettings::check_inline);
        settings.check_inline();
        let _: &bool = open;
        let _: [&f64; 4] = [threshold, return_level, floor_gain, sidechain_gain_linear];
        let _: [&u32; 6] = [
            sample_rate,
            attack_frames,
            hold_frames,
            release_frames,
            lookahead_frames,
            hold_elapsed,
        ];
        let _: &hex_dsp::LinearRamp = gain_ramp;
        let _: &DynamicsMeter = meter;
        let _: &SidechainEq = sidechain_eq;
        let _: &usize = lookahead_pos;
        let _: &[StereoFrame; MAX_LOOKAHEAD_FRAMES] = lookahead_buf;
    }

    pub(super) fn install_settings(&mut self, settings: GateSettings) {
        let previous = self.settings;
        self.settings = settings;
        self.retarget_envelope(previous);
    }

    pub(super) fn retarget_envelope(&mut self, previous: GateSettings) {
        let attack_changed = self.settings.attack_ms != previous.attack_ms;
        let release_changed = self.settings.release_ms != previous.release_ms;
        let floor_changed = self.settings.floor_db != previous.floor_db;
        let flip_changed = self.settings.flip != previous.flip;
        let target = self.target_gain(self.open);
        if flip_changed {
            let frames = if self.open {
                self.attack_frames
            } else {
                self.release_frames
            };
            self.start_ramp(target, frames);
            return;
        }
        if self.open
            && (floor_changed && self.settings.flip
                || (attack_changed
                    && self.gain_ramp.is_active()
                    && self.gain_ramp.target() == target))
        {
            self.start_ramp(target, self.attack_frames);
        } else if !self.open
            && ((floor_changed && !self.settings.flip)
                || (release_changed && self.gain_ramp.is_active()))
        {
            self.start_ramp(target, self.release_frames);
        }
    }

    pub(super) fn apply_values(
        &mut self,
        values: &dyn hex_lattice::NativeParameterAutomation,
        resolved: &mut crate::parameter_binding::Resolved<PARAM_COUNT>,
        offset: usize,
        modulates_threshold: bool,
        modulates_return: bool,
    ) -> Result<(), DeviceContractError> {
        let next = self.controls.sample(&BINDINGS, resolved, values, offset)?;
        let mut settings = next
            .as_ref()
            .map_or_else(|| self.controls.effective(), |next| next.effective());
        // Keep the unmodulated detector level authoritative. When both
        // levels are modulated, the opening threshold governs the return.
        if settings.return_db > settings.threshold_db {
            if modulates_return {
                settings.return_db = settings.threshold_db;
            } else if modulates_threshold {
                settings.threshold_db = settings.return_db;
            } else {
                return Err(DeviceContractError::InvalidParameterValue);
            }
        }
        if settings != self.settings {
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
            let sidechain_gain_linear =
                if settings.sidechain_gain_db == self.settings.sidechain_gain_db {
                    self.sidechain_gain_linear
                } else if settings.sidechain_gain_db == 0.0 {
                    1.0
                } else {
                    db_to_gain(settings.sidechain_gain_db)?
                };
            let attack = checked_frames(settings.attack_ms, self.sample_rate)?;
            let hold = checked_frames(settings.hold_ms.max(MIN_HOLD_MS), self.sample_rate)?;
            let release = checked_frames(settings.release_ms, self.sample_rate)?;
            let eq_changed = settings.sidechain_eq_mode != self.settings.sidechain_eq_mode
                || settings.sidechain_eq_freq_hz != self.settings.sidechain_eq_freq_hz
                || settings.sidechain_eq_q != self.settings.sidechain_eq_q;
            self.threshold = threshold;
            self.return_level = return_level;
            self.floor_gain = floor_gain;
            self.sidechain_gain_linear = sidechain_gain_linear;
            self.attack_frames = attack;
            self.hold_frames = hold;
            self.release_frames = release;
            if eq_changed {
                self.sidechain_eq.update(
                    settings.sidechain_eq_mode,
                    settings.sidechain_eq_freq_hz,
                    settings.sidechain_eq_q,
                    self.sample_rate,
                );
            }
            self.install_settings(settings);
        }
        if let Some(next) = next {
            self.controls.install(next);
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
        let modulates_threshold = values.has_sample_rate_modulation(parameter::THRESHOLD);
        let modulates_return = values.has_sample_rate_modulation(parameter::RETURN);
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.apply_values(
                values,
                &mut resolved,
                offset,
                modulates_threshold,
                modulates_return,
            )?;
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
        let bindings: Vec<_> = BINDINGS
            .iter()
            .filter(|b| b.declares_arrangement_automation())
            .collect();
        Values {
            ids: bindings.iter().map(|b| b.descriptor.id).collect(),
            values: bindings.iter().map(|b| Some(b.read(&settings))).collect(),
        }
    }
    #[test]
    fn declared_mapping_and_admission_are_explicit() {
        let state = GATE_DEFINITION.default_state().unwrap();
        for binding in &BINDINGS {
            binding.semantics.validate(binding.descriptor).unwrap();
            let expected = binding.descriptor.id != parameter::LOOKAHEAD;
            assert_eq!(
                GATE_DEFINITION
                    .supports_parameter_automation(&state, binding.descriptor.id)
                    .unwrap(),
                expected
            );
        }
        assert!(PreparedGate::new(&state, 8000, &[parameter::LOOKAHEAD]).is_err());
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
            ..GateSettings::default()
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
        assert!(PreparedGate::new(&base, 8000, &[ParameterId::from_static(99)]).is_err());
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
