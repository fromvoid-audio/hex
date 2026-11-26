//! Version-25 extended LFO capabilities: range (`low`, `high`, `sync`),
//! `filter` destination, `r<key` rate key-tracking, `amt<vel` velocity depth,
//! and per-voice sloped LFO envelope reusing `mod_env::Machine`.
//!
//! Neutral defaults (`range = low`, `dest_filter = false`, `r<key = 0%`,
//! `amt<vel = 0%`, `lfo env enabled = false`) preserve bit-identical PCM for
//! all historical v1..v24 LFO patches. Designed for clean migration into the
//! shared `"lfo 1"` slot modulator (#3887/#3892).
use super::*;

pub(super) const FIRST_ID: u32 = 711;
pub(super) const COUNT: usize = 18;
pub(super) const BYTES: usize = COUNT * 4;

pub(super) const RANGE_ID: u32 = FIRST_ID;
pub(super) const DEST_FILTER_ID: u32 = FIRST_ID + 1;
pub(super) const RATE_KEY_ID: u32 = FIRST_ID + 2;
pub(super) const AMOUNT_VEL_ID: u32 = FIRST_ID + 3;
pub(super) const ENV_ENABLED_ID: u32 = FIRST_ID + 4;
pub(super) const ENV_INITIAL_ID: u32 = FIRST_ID + 5;
pub(super) const ENV_PEAK_ID: u32 = FIRST_ID + 6;
pub(super) const ENV_SUSTAIN_ID: u32 = FIRST_ID + 7;
pub(super) const ENV_END_ID: u32 = FIRST_ID + 8;
pub(super) const ENV_ATTACK_ID: u32 = FIRST_ID + 9;
pub(super) const ENV_DECAY_ID: u32 = FIRST_ID + 10;
pub(super) const ENV_RELEASE_ID: u32 = FIRST_ID + 11;
pub(super) const ENV_ATTACK_SLOPE_ID: u32 = FIRST_ID + 12;
pub(super) const ENV_DECAY_SLOPE_ID: u32 = FIRST_ID + 13;
pub(super) const ENV_RELEASE_SLOPE_ID: u32 = FIRST_ID + 14;
pub(super) const ENV_LOOP_ID: u32 = FIRST_ID + 15;
pub(super) const ENV_LOOP_TIME_ID: u32 = FIRST_ID + 16;
pub(super) const ENV_BEAT_ID: u32 = FIRST_ID + 17;

pub(super) const RANGES: [&str; 3] = ["low", "high", "sync"];
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
        ParameterDescriptor {
            id: ParameterId::from_static(RANGE_ID),
            label: "lfo range",
            kind: ParameterKind::Choice { labels: &RANGES },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        bool_desc(DEST_FILTER_ID, "lfo filter", false),
        float_desc(RATE_KEY_ID, "lfo r<key", -100.0, 100.0, Some("%"), 0.0),
        float_desc(AMOUNT_VEL_ID, "lfo amt<vel", -100.0, 100.0, Some("%"), 0.0),
        bool_desc(ENV_ENABLED_ID, "lfo env enabled", false),
        float_desc(
            ENV_INITIAL_ID,
            "lfo env initial",
            0.0,
            100.0,
            Some("%"),
            0.0,
        ),
        float_desc(ENV_PEAK_ID, "lfo env peak", 0.0, 100.0, Some("%"), 100.0),
        float_desc(
            ENV_SUSTAIN_ID,
            "lfo env sustain",
            0.0,
            100.0,
            Some("%"),
            100.0,
        ),
        float_desc(ENV_END_ID, "lfo env end", 0.0, 100.0, Some("%"), 0.0),
        float_desc(ENV_ATTACK_ID, "lfo env attack", 0.0, 20.0, Some("s"), 0.005),
        float_desc(ENV_DECAY_ID, "lfo env decay", 0.001, 60.0, Some("s"), 0.1),
        float_desc(
            ENV_RELEASE_ID,
            "lfo env release",
            0.001,
            60.0,
            Some("s"),
            0.1,
        ),
        float_desc(
            ENV_ATTACK_SLOPE_ID,
            "lfo env attack slope",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        float_desc(
            ENV_DECAY_SLOPE_ID,
            "lfo env decay slope",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        float_desc(
            ENV_RELEASE_SLOPE_ID,
            "lfo env release slope",
            -100.0,
            100.0,
            Some("%"),
            0.0,
        ),
        ParameterDescriptor {
            id: ParameterId::from_static(ENV_LOOP_ID),
            label: "lfo env loop",
            kind: ParameterKind::Choice {
                labels: &mod_env::LOOPS,
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        float_desc(
            ENV_LOOP_TIME_ID,
            "lfo env loop time",
            0.001,
            60.0,
            Some("s"),
            0.1,
        ),
        ParameterDescriptor {
            id: ParameterId::from_static(ENV_BEAT_ID),
            label: "lfo env beat",
            kind: ParameterKind::Choice {
                labels: &BEAT_LABELS,
            },
            default: ParameterValue::Choice(6),
            presentation: ParameterPresentation::Linear,
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Settings {
    words: [u32; COUNT],
}

impl Default for Settings {
    fn default() -> Self {
        let mut words = [0_u32; COUNT];
        words[6] = 100.0_f32.to_bits(); // peak 100%
        words[7] = 100.0_f32.to_bits(); // sustain 100%
        words[9] = 0.005_f32.to_bits(); // attack
        words[10] = 0.1_f32.to_bits(); // decay
        words[11] = 0.1_f32.to_bits(); // release
        words[16] = 0.1_f32.to_bits(); // loop time
        words[17] = 6; // 1 beat
        Self { words }
    }
}

impl Settings {
    #[inline]
    pub(super) fn is_high(self) -> bool {
        self.words[0] == 1
    }

    #[inline]
    pub(super) fn is_sync(self) -> bool {
        self.words[0] == 2
    }

    #[inline]
    pub(super) fn set_range(&mut self, range: u32) {
        self.words[0] = range.min(2);
    }

    #[inline]
    pub(super) fn dest_filter(self) -> bool {
        self.words[1] != 0
    }

    #[inline]
    pub(super) fn rate_key(self) -> f32 {
        f32::from_bits(self.words[2])
    }

    #[inline]
    pub(super) fn amount_vel(self) -> f32 {
        f32::from_bits(self.words[3])
    }

    #[inline]
    pub(super) fn env_enabled(self) -> bool {
        self.words[4] != 0
    }

    pub(super) fn is_neutral(self) -> bool {
        self.words[0] == 0
            && self.words[1] == 0
            && self.rate_key() == 0.0
            && self.amount_vel() == 0.0
            && self.words[4] == 0
    }

    pub(super) fn needs_clock(self, lfo_active: bool) -> bool {
        lfo_active && self.env_enabled() && matches!(self.words[15], 2 | 3)
    }

    pub(super) fn needs_running(self, lfo_active: bool) -> bool {
        lfo_active && self.env_enabled() && self.words[15] == 3
    }

    pub(super) fn spec(self) -> mod_env::Spec {
        mod_env::Spec {
            initial: f32::from_bits(self.words[5]),
            peak: f32::from_bits(self.words[6]),
            sustain: f32::from_bits(self.words[7]),
            end: f32::from_bits(self.words[8]),
            attack: f32::from_bits(self.words[9]),
            decay: f32::from_bits(self.words[10]),
            release: f32::from_bits(self.words[11]),
            attack_slope: f32::from_bits(self.words[12]),
            decay_slope: f32::from_bits(self.words[13]),
            release_slope: f32::from_bits(self.words[14]),
            loop_mode: self.words[15],
            loop_time: f32::from_bits(self.words[16]),
            beat_choice: self.words[17],
        }
    }

    /// Keyboard rate multiplier for a played MIDI pitch (`r<key` in `-100..=+100%`).
    #[inline]
    pub(super) fn rate_key_multiplier(self, pitch: u8) -> f64 {
        let rk = f64::from(self.rate_key());
        if rk == 0.0 {
            return 1.0;
        }
        ((rk / 100.0) * (f64::from(pitch) - 60.0) / 12.0)
            .exp2()
            .clamp(0.03125, 32.0)
    }

    /// Velocity depth scalar in `[0.0, 1.0]` for `amt<vel` in `-100..=+100%`.
    #[inline]
    pub(super) fn amount_vel_scale(self, velocity: u8) -> f64 {
        let av = f64::from(self.amount_vel());
        if av == 0.0 {
            return 1.0;
        }
        let v = (f64::from(velocity) / 127.0).clamp(0.0, 1.0);
        if av > 0.0 {
            (1.0 - (av / 100.0) * (1.0 - v)).clamp(0.0, 1.0)
        } else {
            (1.0 + (av / 100.0) * v).clamp(0.0, 1.0)
        }
    }

    pub(super) fn read(self, id: ParameterId) -> Option<ParameterValue> {
        let i = id.value().checked_sub(FIRST_ID)? as usize;
        let word = *self.words.get(i)?;
        Some(match i {
            0 | 15 | 17 => ParameterValue::Choice(word),
            1 | 4 => ParameterValue::Bool(word != 0),
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
        let prepared = OPERATOR_METADATA
            .prepare_instrument(state, &context)
            .unwrap();
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
    fn v24_migration_is_bit_identical_and_rejects_v25_tag_and_waveforms_on_v24() {
        let mut base = OPERATOR_METADATA.default_state().unwrap();
        assert_eq!(base.state_version(), OPERATOR_STATE_VERSION);
        base = OPERATOR_METADATA
            .patch_parameter(&base, parameter::LFO_ENABLED, ParameterValue::Bool(true))
            .unwrap();
        base = OPERATOR_METADATA
            .patch_parameter(&base, parameter::LFO_RATE, ParameterValue::Float(7.5))
            .unwrap();
        base = OPERATOR_METADATA
            .patch_parameter(&base, parameter::LFO_AMOUNT, ParameterValue::Float(25.0))
            .unwrap();
        let settings = OperatorSettings::from_state(&base).unwrap();
        let v24 = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            OSC_ENV_SHAPE_STATE_VERSION,
            tlv_codec::encode_v24(&settings).unwrap(),
        )
        .unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&v24).unwrap();
        assert_eq!(migrated, base);
        assert_eq!(
            render(&v24, 48_000, 256, 64, 60, 127, 128),
            render(&migrated, 48_000, 256, 64, 60, 127, 128)
        );
        // Tag 21 must be rejected on v24.
        assert!(tlv_codec::decode(base.payload(), OSC_ENV_SHAPE_STATE_VERSION).is_err());

        // Saw down (5) and noise (6) waveforms must be rejected on pre-v25 states.
        for wf in [5_u32, 6] {
            let wf_state = OPERATOR_METADATA
                .patch_parameter(&base, parameter::LFO_WAVEFORM, ParameterValue::Choice(wf))
                .unwrap();
            let wf_settings = OperatorSettings::from_state(&wf_state).unwrap();
            let bad_v24 = tlv_codec::encode_v24(&wf_settings).unwrap();
            assert!(tlv_codec::decode(&bad_v24, OSC_ENV_SHAPE_STATE_VERSION).is_err());
            let bad_v17 = wf_settings.to_v17_state().unwrap();
            assert!(OperatorSettings::from_state(&bad_v17).is_err());
        }

        // Invalid ranges refused.
        for (id, bad) in [
            (parameter::LFO_RANGE, ParameterValue::Choice(3)),
            (parameter::LFO_RATE_KEY, ParameterValue::Float(101.0)),
            (parameter::LFO_AMOUNT_VEL, ParameterValue::Float(-101.0)),
            (parameter::LFO_WAVEFORM, ParameterValue::Choice(7)),
            (parameter::LFO_ENV_PEAK, ParameterValue::Float(101.0)),
        ] {
            assert!(OPERATOR_METADATA.patch_parameter(&base, id, bad).is_err());
        }
    }

    #[test]
    fn high_range_waveforms_filter_dest_modifiers_and_lfo_env_are_bounded_and_invariant() {
        let mut base = OPERATOR_METADATA.default_state().unwrap();
        for (id, val) in [
            (parameter::LFO_ENABLED, ParameterValue::Bool(true)),
            (parameter::LFO_RATE, ParameterValue::Float(5.0)),
            (parameter::LFO_AMOUNT, ParameterValue::Float(35.0)),
        ] {
            base = OPERATOR_METADATA.patch_parameter(&base, id, val).unwrap();
        }
        let low_pcm = render(&base, 48_000, 256, 64, 60, 127, 200);

        // High range (audio-rate 500 Hz) produces distinct audio-rate FM vs 5 Hz low range.
        let high = OPERATOR_METADATA
            .patch_parameter(&base, parameter::LFO_RANGE, ParameterValue::Choice(1))
            .unwrap();
        let high_pcm = render(&high, 48_000, 256, 64, 60, 127, 200);
        assert_ne!(low_pcm, high_pcm);

        // Saw down (5) and noise (6) waveforms produce distinct, finite modulation.
        let saw_down = OPERATOR_METADATA
            .patch_parameter(&high, parameter::LFO_WAVEFORM, ParameterValue::Choice(5))
            .unwrap();
        let noise = OPERATOR_METADATA
            .patch_parameter(&high, parameter::LFO_WAVEFORM, ParameterValue::Choice(6))
            .unwrap();
        let saw_pcm = render(&saw_down, 48_000, 256, 64, 60, 127, 200);
        let noise_pcm = render(&noise, 48_000, 256, 64, 60, 127, 200);
        assert_ne!(saw_pcm, high_pcm);
        assert_ne!(noise_pcm, saw_pcm);

        // Filter destination modulates cutoff even when all operator pitch destinations are off.
        let mut filt = base.clone();
        for (id, val) in [
            (parameter::waveform(1), ParameterValue::Choice(1)),
            (ParameterId::from_static(20_001), ParameterValue::Bool(true)),
            (parameter::FILTER_FREQUENCY, ParameterValue::Float(800.0)),
            (parameter::lfo_destination(1), ParameterValue::Bool(false)),
            (parameter::lfo_destination(2), ParameterValue::Bool(false)),
            (parameter::lfo_destination(3), ParameterValue::Bool(false)),
            (parameter::lfo_destination(4), ParameterValue::Bool(false)),
        ] {
            filt = OPERATOR_METADATA.patch_parameter(&filt, id, val).unwrap();
        }
        let filt_off = render(&filt, 48_000, 512, 64, 60, 127, 400);
        let filt_on_state = OPERATOR_METADATA
            .patch_parameter(
                &filt,
                parameter::LFO_DEST_FILTER,
                ParameterValue::Bool(true),
            )
            .unwrap();
        let filt_on = render(&filt_on_state, 48_000, 512, 64, 60, 127, 400);
        assert_ne!(filt_off, filt_on);

        // LFO envelope with slow attack attenuates early LFO depth vs immediate full depth.
        let mut with_env = high.clone();
        for (id, val) in [
            (parameter::LFO_ENV_ENABLED, ParameterValue::Bool(true)),
            (parameter::LFO_ENV_INITIAL, ParameterValue::Float(0.0)),
            (parameter::LFO_ENV_PEAK, ParameterValue::Float(100.0)),
            (parameter::LFO_ENV_ATTACK, ParameterValue::Float(0.5)),
            (parameter::LFO_RATE_KEY, ParameterValue::Float(100.0)),
            (parameter::LFO_AMOUNT_VEL, ParameterValue::Float(100.0)),
        ] {
            with_env = OPERATOR_METADATA
                .patch_parameter(&with_env, id, val)
                .unwrap();
        }
        let env_pcm = render(&with_env, 48_000, 256, 64, 72, 127, 200);
        assert_ne!(env_pcm, render(&high, 48_000, 256, 64, 72, 127, 200));

        // Block-partition invariance and finiteness across 8 kHz..192 kHz even at max rate (10 kHz).
        let mut extreme = with_env;
        extreme = OPERATOR_METADATA
            .patch_parameter(&extreme, parameter::LFO_RATE, ParameterValue::Float(100.0))
            .unwrap();
        extreme = OPERATOR_METADATA
            .patch_parameter(&extreme, parameter::LFO_WAVEFORM, ParameterValue::Choice(6))
            .unwrap();
        extreme = OPERATOR_METADATA
            .patch_parameter(
                &extreme,
                ParameterId::from_static(20_001),
                ParameterValue::Bool(true),
            )
            .unwrap();
        extreme = OPERATOR_METADATA
            .patch_parameter(
                &extreme,
                parameter::LFO_DEST_FILTER,
                ParameterValue::Bool(true),
            )
            .unwrap();
        for rate in [8_000, 48_000, 192_000] {
            let whole = render(&extreme, rate, 256, 256, 84, 100, 128);
            let split = render(&extreme, rate, 256, 17, 84, 100, 128);
            assert_eq!(whole, split, "block-partition mismatch at {rate} Hz");
            assert!(whole.iter().flatten().all(|s| s.is_finite()));
        }
    }
}
