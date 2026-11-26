//! Version-24 per-operator oscillator envelope slopes, end level, and note-pitch loop ratio.
//!
//! Neutral defaults (`attack slope = 0%`, `decay slope = 0%`, `release slope = 0%`,
//! `end = 0.0`, `loop note ratio = 1.0x`) preserve bit-identical PCM to historical
//! v1..v23 `OperatorEnvelope` laws.
use super::*;

pub(super) const FIRST_ID: u32 = 691;
pub(super) const FIELDS_PER_OP: usize = 5;
pub(super) const COUNT: usize = OPERATORS * FIELDS_PER_OP;
pub(super) const BYTES: usize = COUNT * 4;

pub(super) const fn param_id(operator: u32, field: u32) -> ParameterId {
    if operator == 0 || operator > OPERATORS as u32 || field >= FIELDS_PER_OP as u32 {
        return ParameterId::from_static(0);
    }
    ParameterId::from_static(FIRST_ID + (operator - 1) * FIELDS_PER_OP as u32 + field)
}

const LABELS: [&str; COUNT] = [
    "op1 attack slope",
    "op1 decay slope",
    "op1 release slope",
    "op1 end",
    "op1 loop note ratio",
    "op2 attack slope",
    "op2 decay slope",
    "op2 release slope",
    "op2 end",
    "op2 loop note ratio",
    "op3 attack slope",
    "op3 decay slope",
    "op3 release slope",
    "op3 end",
    "op3 loop note ratio",
    "op4 attack slope",
    "op4 decay slope",
    "op4 release slope",
    "op4 end",
    "op4 loop note ratio",
];

pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    let mut out = [ParameterDescriptor {
        id: ParameterId::from_static(FIRST_ID),
        label: LABELS[0],
        kind: ParameterKind::Float {
            min: -100.0,
            max: 100.0,
            unit: Some("%"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    }; COUNT];
    let mut i = 0;
    while i < COUNT {
        let field = i % FIELDS_PER_OP;
        out[i].id = ParameterId::from_static(FIRST_ID + i as u32);
        out[i].label = LABELS[i];
        match field {
            0..=2 => {
                out[i].kind = ParameterKind::Float {
                    min: -100.0,
                    max: 100.0,
                    unit: Some("%"),
                };
                out[i].default = ParameterValue::Float(0.0);
                out[i].presentation = ParameterPresentation::Linear;
            }
            3 => {
                out[i].kind = ParameterKind::Float {
                    min: 0.0,
                    max: 1.0,
                    unit: None,
                };
                out[i].default = ParameterValue::Float(0.0);
                out[i].presentation = ParameterPresentation::Linear;
            }
            _ => {
                out[i].kind = ParameterKind::Float {
                    min: 0.25,
                    max: 4.0,
                    unit: Some("x"),
                };
                out[i].default = ParameterValue::Float(1.0);
                out[i].presentation = ParameterPresentation::Logarithmic;
            }
        }
        i += 1;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Settings {
    pub(super) words: [[u32; FIELDS_PER_OP]; OPERATORS],
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            words: [[0, 0, 0, 0, 1.0_f32.to_bits()]; OPERATORS],
        }
    }
}

impl Settings {
    #[inline]
    pub(super) fn attack_slope(self, op: usize) -> f64 {
        f64::from(f32::from_bits(self.words[op][0]))
    }

    #[inline]
    pub(super) fn decay_slope(self, op: usize) -> f64 {
        f64::from(f32::from_bits(self.words[op][1]))
    }

    #[inline]
    pub(super) fn release_slope(self, op: usize) -> f64 {
        f64::from(f32::from_bits(self.words[op][2]))
    }

    #[inline]
    pub(super) fn end_level(self, op: usize) -> f64 {
        f64::from(f32::from_bits(self.words[op][3]))
    }

    #[inline]
    pub(super) fn loop_note_ratio(self, op: usize) -> f64 {
        f64::from(f32::from_bits(self.words[op][4]))
    }

    pub(super) fn is_neutral(self) -> bool {
        (0..OPERATORS).all(|op| {
            self.attack_slope(op) == 0.0
                && self.decay_slope(op) == 0.0
                && self.release_slope(op) == 0.0
                && self.end_level(op) == 0.0
        })
    }

    pub(super) fn read(self, id: ParameterId) -> Option<ParameterValue> {
        let i = id.value().checked_sub(FIRST_ID)? as usize;
        let word = *self.words.get(i / FIELDS_PER_OP)?.get(i % FIELDS_PER_OP)?;
        Some(ParameterValue::Float(f32::from_bits(word)))
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
        let mut out = Self::default();
        for (slot, chunk) in out.words.iter_mut().flatten().zip(bytes.chunks_exact(4)) {
            *slot = u32::from_le_bytes(
                chunk
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
        }
        out.validate()?;
        Ok(out)
    }

    pub(super) fn append(self, payload: &mut Vec<u8>) {
        for word in self.words.iter().flatten() {
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
        let ParameterValue::Float(v) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        self.words[i / FIELDS_PER_OP][i % FIELDS_PER_OP] = v.to_bits();
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
        gate: usize,
    ) -> Vec<[f32; 2]> {
        let tuning = hex_lattice::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: rate,
            tuning: &tuning,
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        };
        let prepared = OPERATOR_METADATA.prepare_instrument(state, &context).unwrap();
        let factory = prepared
            .prepare_mutable_runtime()
            .unwrap()
            .or_else(|| prepared.prepare_mutable_continuation_runtime().unwrap())
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
                    velocity: 127,
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
    fn v23_migration_is_bit_identical_and_rejects_v24_tag_and_note_pitch_on_v23() {
        let default = OPERATOR_METADATA.default_state().unwrap();
        assert_eq!(default.state_version(), OPERATOR_STATE_VERSION);
        let settings = OperatorSettings::from_state(&default).unwrap();
        let v23 = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            FILTER_SHAPER_STATE_VERSION,
            tlv_codec::encode_v23(&settings).unwrap(),
        )
        .unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&v23).unwrap();
        assert_eq!(migrated, default);
        assert_eq!(
            render(&v23, 48_000, 256, 64, 60, 128),
            render(&migrated, 48_000, 256, 64, 60, 128)
        );
        // Tag 20 must be rejected on v23.
        assert!(tlv_codec::decode(default.payload(), FILTER_SHAPER_STATE_VERSION).is_err());

        // Loop mode 5 ("note-pitch") must be rejected on pre-v24 states.
        let note_pitch_state = OPERATOR_METADATA
            .patch_parameter(
                &default,
                ParameterId::from_static(362),
                ParameterValue::Choice(5),
            )
            .unwrap();
        let np_settings = OperatorSettings::from_state(&note_pitch_state).unwrap();
        let bad_v23 = tlv_codec::encode_v23(&np_settings).unwrap();
        assert!(tlv_codec::decode(&bad_v23, FILTER_SHAPER_STATE_VERSION).is_err());
        let bad_v17 = np_settings.to_v17_state().unwrap();
        assert!(OperatorSettings::from_state(&bad_v17).is_err());

        // Out-of-range parameters are rejected.
        for (id, bad) in [
            (
                parameter::oscillator_attack_slope(1),
                ParameterValue::Float(101.0),
            ),
            (
                parameter::oscillator_decay_slope(2),
                ParameterValue::Float(-101.0),
            ),
            (
                parameter::oscillator_release_slope(3),
                ParameterValue::Float(f32::NAN),
            ),
            (parameter::oscillator_end(4), ParameterValue::Float(-0.01)),
            (parameter::oscillator_end(1), ParameterValue::Float(1.01)),
            (
                parameter::oscillator_loop_note_ratio(1),
                ParameterValue::Float(0.24),
            ),
            (
                parameter::oscillator_loop_note_ratio(1),
                ParameterValue::Float(4.01),
            ),
        ] {
            assert!(
                OPERATOR_METADATA
                    .patch_parameter(&default, id, bad)
                    .is_err()
            );
        }
    }

    #[test]
    fn slopes_end_level_and_note_pitch_sync_shape_audio_and_stay_partition_invariant() {
        let mut base = OPERATOR_METADATA.default_state().unwrap();
        for (id, val) in [
            (
                parameter::envelope(1, parameter::EnvelopeField::Attack),
                ParameterValue::Float(0.01),
            ),
            (
                parameter::envelope(1, parameter::EnvelopeField::Decay),
                ParameterValue::Float(0.02),
            ),
            (
                parameter::envelope(1, parameter::EnvelopeField::Sustain),
                ParameterValue::Float(0.3),
            ),
            (
                parameter::envelope(1, parameter::EnvelopeField::Release),
                ParameterValue::Float(0.01),
            ),
        ] {
            base = OPERATOR_METADATA.patch_parameter(&base, id, val).unwrap();
        }
        let convex = OPERATOR_METADATA
            .patch_parameter(
                &base,
                parameter::oscillator_attack_slope(1),
                ParameterValue::Float(85.0),
            )
            .unwrap();
        let concave = OPERATOR_METADATA
            .patch_parameter(
                &base,
                parameter::oscillator_attack_slope(1),
                ParameterValue::Float(-85.0),
            )
            .unwrap();
        let energy = |buf: &[[f32; 2]]| -> f64 {
            buf.iter()
                .map(|f| f64::from(f[0]) * f64::from(f[0]))
                .sum()
        };
        let e_convex = energy(&render(&convex, 48_000, 480, 64, 60, 480));
        let e_concave = energy(&render(&concave, 48_000, 480, 64, 60, 480));
        assert!(
            e_convex > e_concave * 1.5,
            "convex attack must accumulate more early energy than concave ({e_convex} vs {e_concave})"
        );

        // Non-zero end level on modulator op2 alters release tail of carrier op1.
        let mut mod_base = base.clone();
        mod_base = OPERATOR_METADATA
            .patch_parameter(
                &mod_base,
                parameter::oscillator_level(2),
                ParameterValue::Float(0.9),
            )
            .unwrap();
        mod_base = OPERATOR_METADATA
            .patch_parameter(
                &mod_base,
                parameter::envelope(2, parameter::EnvelopeField::Release),
                ParameterValue::Float(0.002),
            )
            .unwrap();
        let mod_with_end = OPERATOR_METADATA
            .patch_parameter(
                &mod_base,
                parameter::oscillator_end(2),
                ParameterValue::Float(0.8),
            )
            .unwrap();
        let r0 = render(&mod_base, 48_000, 1024, 64, 60, 100);
        let r1 = render(&mod_with_end, 48_000, 1024, 64, 60, 100);
        assert_ne!(r0[250..600], r1[250..600]);

        // Note-pitch sync loops faster on higher pitches and higher ratios, and is
        // block-partition invariant across 8 kHz..192 kHz.
        let mut synced = base;
        for (id, val) in [
            (ParameterId::from_static(362), ParameterValue::Choice(5)), // op1 loop = note-pitch
            (
                parameter::oscillator_loop_note_ratio(1),
                ParameterValue::Float(2.0),
            ),
            (
                parameter::oscillator_decay_slope(1),
                ParameterValue::Float(-60.0),
            ),
            (
                parameter::oscillator_release_slope(1),
                ParameterValue::Float(40.0),
            ),
            (parameter::oscillator_end(1), ParameterValue::Float(0.25)),
        ] {
            synced = OPERATOR_METADATA.patch_parameter(&synced, id, val).unwrap();
        }
        let synced_half = OPERATOR_METADATA
            .patch_parameter(
                &synced,
                parameter::oscillator_loop_note_ratio(1),
                ParameterValue::Float(0.5),
            )
            .unwrap();
        assert_ne!(
            render(&synced, 48_000, 512, 64, 69, 400),
            render(&synced_half, 48_000, 512, 64, 69, 400)
        );
        for rate in [8_000, 48_000, 192_000] {
            let whole = render(&synced, rate, 256, 256, 69, 128);
            let split = render(&synced, rate, 256, 17, 69, 128);
            assert_eq!(whole, split, "block-partition mismatch at {rate} Hz");
            assert!(whole.iter().flatten().all(|s| s.is_finite()));
        }
    }
}
