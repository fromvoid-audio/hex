//! Version-22 per-voice filter envelope and direct `freq<vel` / `freq<key` modifiers.
//!
//! Cutoff law matches `docs/design/vector-brief.md`:
//! `effective_cutoff = base_cutoff * 2^(env_level * amount_st / 12 + vel_oct + key_oct)`,
//! clamped to `[20 Hz, min(20 kHz, 0.45 * Fs)]`.
//! Neutral defaults (`amount = 0 st`, `freq<vel = 0%`, `freq<key = 0%`) preserve
//! bit-identical renders for all historical v1..v21 patches.
use super::*;

pub(super) const FIRST_ID: u32 = 671;
pub(super) const COUNT: usize = 16;
pub(super) const BYTES: usize = COUNT * 4;

pub(super) const AMOUNT_ID: u32 = FIRST_ID;
pub(super) const INITIAL_ID: u32 = FIRST_ID + 1;
pub(super) const PEAK_ID: u32 = FIRST_ID + 2;
pub(super) const SUSTAIN_ID: u32 = FIRST_ID + 3;
pub(super) const END_ID: u32 = FIRST_ID + 4;
pub(super) const ATTACK_ID: u32 = FIRST_ID + 5;
pub(super) const DECAY_ID: u32 = FIRST_ID + 6;
pub(super) const RELEASE_ID: u32 = FIRST_ID + 7;
pub(super) const ATTACK_SLOPE_ID: u32 = FIRST_ID + 8;
pub(super) const DECAY_SLOPE_ID: u32 = FIRST_ID + 9;
pub(super) const RELEASE_SLOPE_ID: u32 = FIRST_ID + 10;
pub(super) const LOOP_ID: u32 = FIRST_ID + 11;
pub(super) const LOOP_TIME_ID: u32 = FIRST_ID + 12;
pub(super) const BEAT_ID: u32 = FIRST_ID + 13;
pub(super) const FREQ_VEL_ID: u32 = FIRST_ID + 14;
pub(super) const FREQ_KEY_ID: u32 = FIRST_ID + 15;

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

pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    [
        float_desc(AMOUNT_ID, "filter env amount", -96.0, 96.0, Some("st"), 0.0),
        float_desc(INITIAL_ID, "filter env initial", 0.0, 100.0, Some("%"), 0.0),
        float_desc(PEAK_ID, "filter env peak", 0.0, 100.0, Some("%"), 100.0),
        float_desc(
            SUSTAIN_ID,
            "filter env sustain",
            0.0,
            100.0,
            Some("%"),
            50.0,
        ),
        float_desc(END_ID, "filter env end", 0.0, 100.0, Some("%"), 0.0),
        float_desc(ATTACK_ID, "filter env attack", 0.0, 20.0, Some("s"), 0.005),
        float_desc(DECAY_ID, "filter env decay", 0.001, 60.0, Some("s"), 0.1),
        float_desc(
            RELEASE_ID,
            "filter env release",
            0.001,
            60.0,
            Some("s"),
            0.1,
        ),
        float_desc(
            ATTACK_SLOPE_ID,
            "filter env attack slope",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        float_desc(
            DECAY_SLOPE_ID,
            "filter env decay slope",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        float_desc(
            RELEASE_SLOPE_ID,
            "filter env release slope",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        ParameterDescriptor {
            id: ParameterId::from_static(LOOP_ID),
            label: "filter env loop",
            kind: ParameterKind::Choice {
                labels: &mod_env::LOOPS,
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        float_desc(
            LOOP_TIME_ID,
            "filter env loop time",
            0.001,
            60.0,
            Some("s"),
            0.1,
        ),
        ParameterDescriptor {
            id: ParameterId::from_static(BEAT_ID),
            label: "filter env beat",
            kind: ParameterKind::Choice {
                labels: &BEAT_LABELS,
            },
            default: ParameterValue::Choice(6),
            presentation: ParameterPresentation::Linear,
        },
        float_desc(
            FREQ_VEL_ID,
            "filter freq vel",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        float_desc(
            FREQ_KEY_ID,
            "filter freq key",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
    ]
}

/// Destination filter frequency at a given envelope level (`0..=100%`) and
/// bipolar depth (`-96..=+96 st`), matching `docs/design/vector-brief.md`.
pub fn filter_envelope_destination_hz(base_hz: f64, amount_st: f64, level_pct: f64) -> f64 {
    let level = (level_pct / 100.0).clamp(0.0, 1.0);
    let depth = amount_st.clamp(-96.0, 96.0);
    (base_hz * 2.0_f64.powf(depth * level / 12.0)).clamp(20.0, 20_000.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Settings {
    words: [u32; COUNT],
}

impl Default for Settings {
    fn default() -> Self {
        let mut words = [0_u32; COUNT];
        words[2] = 100.0_f32.to_bits(); // peak 100%
        words[3] = 50.0_f32.to_bits(); // sustain 50%
        words[5] = 0.005_f32.to_bits(); // attack
        words[6] = 0.1_f32.to_bits(); // decay
        words[7] = 0.1_f32.to_bits(); // release
        words[12] = 0.1_f32.to_bits(); // loop time
        words[13] = 6; // 1 beat
        Self { words }
    }
}

impl Settings {
    pub(super) fn amount(self) -> f32 {
        f32::from_bits(self.words[0])
    }

    pub(super) fn freq_vel(self) -> f32 {
        f32::from_bits(self.words[14])
    }

    pub(super) fn freq_key(self) -> f32 {
        f32::from_bits(self.words[15])
    }

    pub(super) fn is_neutral(self) -> bool {
        self.amount() == 0.0 && self.freq_vel() == 0.0 && self.freq_key() == 0.0
    }

    pub(super) fn needs_clock(self, filter_armed: bool) -> bool {
        filter_armed && self.amount() != 0.0 && matches!(self.words[11], 2 | 3)
    }

    pub(super) fn needs_running(self, filter_armed: bool) -> bool {
        filter_armed && self.amount() != 0.0 && self.words[11] == 3
    }

    pub(super) fn spec(self) -> mod_env::Spec {
        mod_env::Spec {
            initial: f32::from_bits(self.words[1]),
            peak: f32::from_bits(self.words[2]),
            sustain: f32::from_bits(self.words[3]),
            end: f32::from_bits(self.words[4]),
            attack: f32::from_bits(self.words[5]),
            decay: f32::from_bits(self.words[6]),
            release: f32::from_bits(self.words[7]),
            attack_slope: f32::from_bits(self.words[8]),
            decay_slope: f32::from_bits(self.words[9]),
            release_slope: f32::from_bits(self.words[10]),
            loop_mode: self.words[11],
            loop_time: f32::from_bits(self.words[12]),
            beat_choice: self.words[13],
        }
    }

    /// Direct velocity + keyboard tracking octave offset for one voice note.
    #[inline]
    pub(super) fn modifier_octaves(self, pitch: u8, velocity: u8) -> f64 {
        let vel_oct = (f64::from(self.freq_vel()) / 100.0) * 4.0 * (f64::from(velocity) / 127.0);
        let key_oct = (f64::from(self.freq_key()) / 100.0) * ((f64::from(pitch) - 60.0) / 12.0);
        vel_oct + key_oct
    }

    /// Envelope octave offset at `age` for a given semitone depth `amount_st`.
    #[inline]
    pub(super) fn envelope_octaves(machine: &mod_env::Machine, age: u64, amount_st: f64) -> f64 {
        if amount_st == 0.0 {
            return 0.0;
        }
        let level = (machine.value(age) / 100.0).clamp(0.0, 1.0);
        (amount_st.clamp(-96.0, 96.0) * level) / 12.0
    }

    pub(super) fn read(self, id: ParameterId) -> Option<ParameterValue> {
        let i = id.value().checked_sub(FIRST_ID)? as usize;
        let word = *self.words.get(i)?;
        Some(match i {
            11 | 13 => ParameterValue::Choice(word),
            _ => ParameterValue::Float(f32::from_bits(word)),
        })
    }

    pub(super) fn validate(self) -> Result<(), DeviceContractError> {
        for d in descriptors() {
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
            ParameterValue::Choice(v) => v,
            ParameterValue::Float(v) => v.to_bits(),
            ParameterValue::Bool(_) => return Err(DeviceContractError::InvalidParameterValue),
        };
        Ok(true)
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
        pitch: u8,
        velocity: u8,
        gate: usize,
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
                    pitch,
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
    fn filter_envelope_ids_and_defaults_remain_stable_in_the_registered_catalog() {
        use ParameterValue::{Choice, Float};
        let expected = [
            (671, "filter env amount", Float(0.0)),
            (672, "filter env initial", Float(0.0)),
            (673, "filter env peak", Float(100.0)),
            (674, "filter env sustain", Float(50.0)),
            (675, "filter env end", Float(0.0)),
            (676, "filter env attack", Float(0.005)),
            (677, "filter env decay", Float(0.1)),
            (678, "filter env release", Float(0.1)),
            (679, "filter env attack slope", Float(0.0)),
            (680, "filter env decay slope", Float(0.0)),
            (681, "filter env release slope", Float(0.0)),
            (682, "filter env loop", Choice(0)),
            (683, "filter env loop time", Float(0.1)),
            (684, "filter env beat", Choice(6)),
            (685, "filter freq vel", Float(0.0)),
            (686, "filter freq key", Float(0.0)),
        ];
        let actual: Vec<_> = OPERATOR_METADATA
            .descriptors()
            .iter()
            .filter(|descriptor| (671..=686).contains(&descriptor.id.value()))
            .map(|descriptor| (descriptor.id.value(), descriptor.label, descriptor.default))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn brief_cutoff_law_and_v21_migration_are_exact() {
        // From docs/design/vector-brief.md: 500 Hz + 24 st at 50% peak -> 1000 Hz,
        // 100% peak -> 2000 Hz, 0% -> 500 Hz.
        assert!((filter_envelope_destination_hz(500.0, 24.0, 50.0) - 1000.0).abs() < 1e-9);
        assert!((filter_envelope_destination_hz(500.0, 24.0, 100.0) - 2000.0).abs() < 1e-9);
        assert!((filter_envelope_destination_hz(500.0, 24.0, 0.0) - 500.0).abs() < 1e-9);
        assert!((filter_envelope_destination_hz(500.0, -12.0, 100.0) - 250.0).abs() < 1e-9);

        let default = OPERATOR_METADATA.default_state().unwrap();
        assert_eq!(default.state_version(), OPERATOR_STATE_VERSION);
        let settings = OperatorSettings::from_state(&default).unwrap();
        let v21 = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            PITCH_ENV_STATE_VERSION,
            tlv_codec::encode_v21(&settings).unwrap(),
        )
        .unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&v21).unwrap();
        assert_eq!(migrated, default);
        assert_eq!(
            render(&v21, 48_000, 256, 64, 60, 127, 128),
            render(&migrated, 48_000, 256, 64, 60, 127, 128)
        );
        // Tag 18 must be rejected on v21.
        assert!(tlv_codec::decode(default.payload(), PITCH_ENV_STATE_VERSION).is_err());

        for (id, bad) in [
            (parameter::FILTER_ENV_AMOUNT, ParameterValue::Float(96.5)),
            (parameter::FILTER_ENV_AMOUNT, ParameterValue::Float(-96.5)),
            (parameter::FILTER_ENV_PEAK, ParameterValue::Float(101.0)),
            (parameter::FILTER_FREQ_VEL, ParameterValue::Float(101.0)),
            (parameter::FILTER_FREQ_KEY, ParameterValue::Float(-101.0)),
        ] {
            assert!(
                OPERATOR_METADATA
                    .patch_parameter(&default, id, bad)
                    .is_err()
            );
        }
    }

    #[test]
    fn filter_envelope_and_direct_vel_key_modifiers_shape_cutoff_across_rates() {
        let mut base = OPERATOR_METADATA.default_state().unwrap();
        for (id, val) in [
            (parameter::waveform(1), ParameterValue::Choice(1)), // saw d
            (ParameterId::from_static(20_001), ParameterValue::Bool(true)),
            (parameter::FILTER_FREQUENCY, ParameterValue::Float(300.0)),
        ] {
            base = OPERATOR_METADATA.patch_parameter(&base, id, val).unwrap();
        }
        let env_open = OPERATOR_METADATA
            .patch_parameter(
                &base,
                parameter::FILTER_ENV_AMOUNT,
                ParameterValue::Float(48.0),
            )
            .unwrap();
        let dry = render(&base, 48_000, 512, 64, 60, 127, 400);
        let swept = render(&env_open, 48_000, 512, 64, 60, 127, 400);
        let hf = |buf: &[[f32; 2]]| -> f64 {
            buf.windows(2)
                .map(|w| {
                    let d = f64::from(w[1][0] - w[0][0]);
                    d * d
                })
                .sum()
        };
        assert!(
            hf(&swept) > hf(&dry) * 1.5,
            "positive filter env amount must open filter cutoff ({} vs {})",
            hf(&swept),
            hf(&dry)
        );

        // Direct freq<vel and freq<key modifiers open filter for louder / higher notes.
        let vel_mod = OPERATOR_METADATA
            .patch_parameter(
                &base,
                parameter::FILTER_FREQ_VEL,
                ParameterValue::Float(100.0),
            )
            .unwrap();
        let soft = render(&vel_mod, 48_000, 256, 64, 60, 24, 200);
        let hard = render(&vel_mod, 48_000, 256, 64, 60, 127, 200);
        assert!(hf(&hard) > hf(&soft) * 4.0);

        let key_mod = OPERATOR_METADATA
            .patch_parameter(
                &base,
                parameter::FILTER_FREQ_KEY,
                ParameterValue::Float(100.0),
            )
            .unwrap();
        let low = render(&key_mod, 48_000, 256, 64, 48, 127, 200);
        let high = render(&key_mod, 48_000, 256, 64, 84, 127, 200);
        assert_ne!(low, high);

        // Partition invariance across sample rates.
        let mut combined = env_open;
        combined = OPERATOR_METADATA
            .patch_parameter(
                &combined,
                parameter::FILTER_FREQ_VEL,
                ParameterValue::Float(50.0),
            )
            .unwrap();
        combined = OPERATOR_METADATA
            .patch_parameter(
                &combined,
                parameter::FILTER_FREQ_KEY,
                ParameterValue::Float(50.0),
            )
            .unwrap();
        for rate in [8_000, 48_000, 192_000] {
            let whole = render(&combined, rate, 256, 256, 72, 110, 128);
            let split = render(&combined, rate, 256, 19, 72, 110, 128);
            assert_eq!(whole, split, "block-partition mismatch at {rate} Hz");
            assert!(whole.iter().flatten().all(|s| s.is_finite()));
        }
    }
}
