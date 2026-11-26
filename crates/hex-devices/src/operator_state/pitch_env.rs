//! Version-21 bipolar per-voice pitch envelope (`hex.instrument.operator`).
//!
//! Neutral default (`enabled = false`, `amount = 0%`) preserves bit-identical
//! renders for all historical v1..v20 patches.
use super::*;

pub(super) const FIRST_ID: u32 = 650;
pub(super) const COUNT: usize = 21;
pub(super) const BYTES: usize = COUNT * 4;

pub(super) const ENABLED_ID: u32 = FIRST_ID;
pub(super) const AMOUNT_ID: u32 = FIRST_ID + 1;
pub(super) const AMOUNT_VEL_ID: u32 = FIRST_ID + 2;
pub(super) const INITIAL_ID: u32 = FIRST_ID + 3;
pub(super) const PEAK_ID: u32 = FIRST_ID + 4;
pub(super) const SUSTAIN_ID: u32 = FIRST_ID + 5;
pub(super) const END_ID: u32 = FIRST_ID + 6;
pub(super) const ATTACK_ID: u32 = FIRST_ID + 7;
pub(super) const DECAY_ID: u32 = FIRST_ID + 8;
pub(super) const RELEASE_ID: u32 = FIRST_ID + 9;
pub(super) const ATTACK_SLOPE_ID: u32 = FIRST_ID + 10;
pub(super) const DECAY_SLOPE_ID: u32 = FIRST_ID + 11;
pub(super) const RELEASE_SLOPE_ID: u32 = FIRST_ID + 12;
pub(super) const LOOP_ID: u32 = FIRST_ID + 13;
pub(super) const LOOP_TIME_ID: u32 = FIRST_ID + 14;
pub(super) const BEAT_ID: u32 = FIRST_ID + 15;
pub(super) const DEST_A_ID: u32 = FIRST_ID + 16;
pub(super) const DEST_B_ID: u32 = FIRST_ID + 17;
pub(super) const DEST_C_ID: u32 = FIRST_ID + 18;
pub(super) const DEST_D_ID: u32 = FIRST_ID + 19;
pub(super) const DEST_LFO_ID: u32 = FIRST_ID + 20;

const BEAT_LABELS: [&str; 11] = [
    "1/64", "1/32", "1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8", "16",
];

const fn float_desc(
    id: u32,
    label: &'static str,
    min: f32,
    max: f32,
    unit: Option<&'static str>,
    default: f32,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(id),
        label,
        kind: ParameterKind::Float { min, max, unit },
        default: ParameterValue::Float(default),
        presentation: ParameterPresentation::Linear,
    }
}

const fn bool_desc(id: u32, label: &'static str, default: bool) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(id),
        label,
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(default),
        presentation: ParameterPresentation::Linear,
    }
}

pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    [
        bool_desc(ENABLED_ID, "pitch env enabled", false),
        float_desc(AMOUNT_ID, "pitch env amount", -100.0, 100.0, Some("%"), 0.0),
        float_desc(
            AMOUNT_VEL_ID,
            "pitch env amount vel",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        float_desc(
            INITIAL_ID,
            "pitch env initial",
            -48.0,
            48.0,
            Some("st"),
            0.0,
        ),
        float_desc(PEAK_ID, "pitch env peak", -48.0, 48.0, Some("st"), 0.0),
        float_desc(
            SUSTAIN_ID,
            "pitch env sustain",
            -48.0,
            48.0,
            Some("st"),
            0.0,
        ),
        float_desc(END_ID, "pitch env end", -48.0, 48.0, Some("st"), 0.0),
        float_desc(ATTACK_ID, "pitch env attack", 0.0, 20.0, Some("s"), 0.005),
        float_desc(DECAY_ID, "pitch env decay", 0.001, 60.0, Some("s"), 0.1),
        float_desc(RELEASE_ID, "pitch env release", 0.001, 60.0, Some("s"), 0.1),
        float_desc(
            ATTACK_SLOPE_ID,
            "pitch env attack slope",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        float_desc(
            DECAY_SLOPE_ID,
            "pitch env decay slope",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        float_desc(
            RELEASE_SLOPE_ID,
            "pitch env release slope",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        ParameterDescriptor {
            id: ParameterId::from_static(LOOP_ID),
            label: "pitch env loop",
            kind: ParameterKind::Choice {
                labels: &mod_env::LOOPS,
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        float_desc(
            LOOP_TIME_ID,
            "pitch env loop time",
            0.001,
            60.0,
            Some("s"),
            0.1,
        ),
        ParameterDescriptor {
            id: ParameterId::from_static(BEAT_ID),
            label: "pitch env beat",
            kind: ParameterKind::Choice {
                labels: &BEAT_LABELS,
            },
            default: ParameterValue::Choice(6),
            presentation: ParameterPresentation::Linear,
        },
        bool_desc(DEST_A_ID, "pitch env a", true),
        bool_desc(DEST_B_ID, "pitch env b", true),
        bool_desc(DEST_C_ID, "pitch env c", true),
        bool_desc(DEST_D_ID, "pitch env d", true),
        bool_desc(DEST_LFO_ID, "pitch env lfo", false),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Settings {
    words: [u32; COUNT],
}

impl Default for Settings {
    fn default() -> Self {
        let mut words = [0_u32; COUNT];
        words[7] = 0.005_f32.to_bits();
        words[8] = 0.1_f32.to_bits();
        words[9] = 0.1_f32.to_bits();
        words[14] = 0.1_f32.to_bits();
        words[15] = 6;
        words[16] = 1;
        words[17] = 1;
        words[18] = 1;
        words[19] = 1;
        Self { words }
    }
}

impl Settings {
    pub(super) fn enabled(self) -> bool {
        self.words[0] != 0
    }

    pub(super) fn amount(self) -> f32 {
        f32::from_bits(self.words[1])
    }

    pub(super) fn amount_vel(self) -> f32 {
        f32::from_bits(self.words[2])
    }

    pub(super) fn dest_operator(self, op: usize) -> bool {
        op < OPERATORS && self.words[16 + op] != 0
    }

    pub(super) fn dest_lfo(self) -> bool {
        self.words[20] != 0
    }

    pub(super) fn needs_clock(self) -> bool {
        self.enabled() && matches!(self.words[13], 2 | 3)
    }

    pub(super) fn needs_running(self) -> bool {
        self.enabled() && self.words[13] == 3
    }

    pub(super) fn spec(self) -> mod_env::Spec {
        mod_env::Spec {
            initial: f32::from_bits(self.words[3]),
            peak: f32::from_bits(self.words[4]),
            sustain: f32::from_bits(self.words[5]),
            end: f32::from_bits(self.words[6]),
            attack: f32::from_bits(self.words[7]),
            decay: f32::from_bits(self.words[8]),
            release: f32::from_bits(self.words[9]),
            attack_slope: f32::from_bits(self.words[10]),
            decay_slope: f32::from_bits(self.words[11]),
            release_slope: f32::from_bits(self.words[12]),
            loop_mode: self.words[13],
            loop_time: f32::from_bits(self.words[14]),
            beat_choice: self.words[15],
        }
    }

    pub(super) fn read(self, id: ParameterId) -> Option<ParameterValue> {
        let i = id.value().checked_sub(FIRST_ID)? as usize;
        let word = *self.words.get(i)?;
        Some(match i {
            0 | 16..=20 => ParameterValue::Bool(word != 0),
            13 | 15 => ParameterValue::Choice(word),
            _ => ParameterValue::Float(f32::from_bits(word)),
        })
    }

    pub(super) fn validate(self) -> Result<(), DeviceContractError> {
        for (i, d) in descriptors().iter().enumerate() {
            if matches!(d.kind, ParameterKind::Bool) && self.words[i] > 1 {
                return Err(DeviceContractError::MalformedState);
            }
            d.validate(
                self.read(d.id)
                    .ok_or(DeviceContractError::UnknownParameter)?,
            )?;
        }
        Ok(())
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self, DeviceContractError> {
        if bytes.len() != BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut words = [0_u32; COUNT];
        for (slot, chunk) in words.iter_mut().zip(bytes.chunks_exact(4)) {
            *slot = u32::from_le_bytes(
                chunk
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
        }
        let settings = Self { words };
        settings.validate()?;
        Ok(settings)
    }

    pub(super) fn append(self, payload: &mut Vec<u8>) {
        for word in self.words {
            payload.extend_from_slice(&word.to_le_bytes());
        }
    }

    pub(super) fn patch(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<bool, DeviceContractError> {
        let Some(i) = id
            .value()
            .checked_sub(FIRST_ID)
            .map(|i| i as usize)
            .filter(|i| *i < COUNT)
        else {
            return Ok(false);
        };
        descriptors()[i].validate(value)?;
        self.words[i] = match value {
            ParameterValue::Bool(v) => u32::from(v),
            ParameterValue::Choice(v) => v,
            ParameterValue::Float(v) => v.to_bits(),
        };
        Ok(true)
    }

    /// Evaluate the effective bipolar pitch offset in semitones for one voice
    /// sample, reusable as a per-voice modulation source evaluator.
    #[inline]
    pub(super) fn semitone_offset(
        self,
        machine: &mod_env::Machine,
        age: u64,
        velocity: u8,
        amount_pct: f64,
    ) -> f64 {
        let vel_norm = f64::from(velocity) / 127.0;
        let scale =
            ((amount_pct + f64::from(self.amount_vel()) * vel_norm) / 100.0).clamp(-2.0, 2.0);
        if scale == 0.0 {
            return 0.0;
        }
        (machine.value(age) * scale).clamp(-96.0, 96.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{
        DevicePreparationContext, NativeInstrumentEvent, NativeInstrumentRuntimeLimits,
        NativeInstrumentRuntimePair,
    };

    fn render(
        state: &NativeDeviceState,
        rate: u32,
        frames: usize,
        chunk: usize,
        gate: usize,
        velocity: u8,
    ) -> Vec<[f32; 2]> {
        let tuning = hex_lattice::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: rate,
            tuning: &tuning,
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        };
        let factory = OPERATOR_METADATA
            .prepare_instrument(state, &context)
            .unwrap()
            .prepare_mutable_runtime()
            .unwrap()
            .unwrap();
        let limits = NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 64,
            max_tail_frames: u64::from(rate) * 600,
        };
        let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
        let mut out = vec![[0.0_f32; 2]; frames];
        let mut offset = 0;
        while offset < frames {
            let end = (offset + chunk).min(frames);
            let mut events = Vec::new();
            if offset == 0 {
                events.push(NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity,
                });
            }
            if gate >= offset && gate < end {
                events.push(NativeInstrumentEvent::NoteOff {
                    frame_offset: gate - offset,
                    occurrence_id: 1,
                });
            }
            pair.begin_block();
            pair.process_block(
                &events,
                &mut out[offset..end],
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
            pair.commit_block();
            offset = end;
        }
        out
    }

    #[test]
    fn descriptors_validation_and_v20_migration_are_strict_and_lossless() {
        let default = OPERATOR_METADATA.default_state().unwrap();
        assert_eq!(default.state_version(), OPERATOR_STATE_VERSION);
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&default, parameter::PITCH_ENV_ENABLED),
            Ok(ParameterValue::Bool(false))
        );
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&default, parameter::PITCH_ENV_AMOUNT),
            Ok(ParameterValue::Float(0.0))
        );

        let settings = OperatorSettings::from_state(&default).unwrap();
        let v20 = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            PITCH_PERF_STATE_VERSION,
            tlv_codec::encode_v20(&settings).unwrap(),
        )
        .unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&v20).unwrap();
        assert_eq!(migrated, default);
        assert_eq!(
            render(&v20, 48_000, 256, 64, 128, 127),
            render(&migrated, 48_000, 256, 64, 128, 127)
        );
        // Tag 17 must be rejected on v20 states.
        assert!(tlv_codec::decode(default.payload(), PITCH_PERF_STATE_VERSION).is_err());

        for (id, bad) in [
            (parameter::PITCH_ENV_AMOUNT, ParameterValue::Float(101.0)),
            (parameter::PITCH_ENV_INITIAL, ParameterValue::Float(-49.0)),
            (parameter::PITCH_ENV_ATTACK, ParameterValue::Float(21.0)),
            (parameter::PITCH_ENV_DECAY, ParameterValue::Float(0.0)),
            (parameter::PITCH_ENV_LOOP, ParameterValue::Choice(5)),
        ] {
            assert!(
                OPERATOR_METADATA
                    .patch_parameter(&default, id, bad)
                    .is_err()
            );
        }
    }

    #[test]
    fn pitch_envelope_sweeps_pitch_respects_targets_and_is_partition_invariant() {
        let mut st = OPERATOR_METADATA.default_state().unwrap();
        for (id, val) in [
            (parameter::PITCH_ENV_ENABLED, ParameterValue::Bool(true)),
            (parameter::PITCH_ENV_AMOUNT, ParameterValue::Float(100.0)),
            (parameter::PITCH_ENV_INITIAL, ParameterValue::Float(12.0)),
            (parameter::PITCH_ENV_PEAK, ParameterValue::Float(24.0)),
            (parameter::PITCH_ENV_SUSTAIN, ParameterValue::Float(0.0)),
            (parameter::PITCH_ENV_END, ParameterValue::Float(-12.0)),
            (parameter::PITCH_ENV_ATTACK, ParameterValue::Float(0.004)),
            (parameter::PITCH_ENV_DECAY, ParameterValue::Float(0.01)),
            (parameter::PITCH_ENV_RELEASE, ParameterValue::Float(0.02)),
            (
                parameter::PITCH_ENV_ATTACK_SLOPE,
                ParameterValue::Float(40.0),
            ),
        ] {
            st = OPERATOR_METADATA.patch_parameter(&st, id, val).unwrap();
        }
        let dry = OPERATOR_METADATA.default_state().unwrap();
        for rate in [8_000, 48_000, 192_000] {
            let whole = render(&st, rate, 256, 256, 128, 127);
            let split = render(&st, rate, 256, 19, 128, 127);
            assert_eq!(whole, split, "block-partition mismatch at {rate} Hz");
            assert!(whole.iter().flatten().all(|s| s.is_finite()));
            assert_ne!(whole, render(&dry, rate, 256, 64, 128, 127));
        }

        // Disabling dest A leaves operator 1 unaffected (bit-identical to dry when only op1 sounds).
        let unrouted = OPERATOR_METADATA
            .patch_parameter(
                &st,
                parameter::PITCH_ENV_DEST_A,
                ParameterValue::Bool(false),
            )
            .unwrap();
        assert_eq!(
            render(&unrouted, 48_000, 256, 64, 128, 127),
            render(&dry, 48_000, 256, 64, 128, 127)
        );
    }
}
