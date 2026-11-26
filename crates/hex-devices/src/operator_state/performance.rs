//! Version-19 global performance controls (`time`, `tone`, `volume`).
//!
//! Neutral defaults (`time = 0%`, `tone = 100%`, `volume = 0 dB`) preserve
//! bit-identical renders for all historical v1..v18 patches.
use super::*;

pub(super) const COUNT: usize = 3;
pub(super) const BYTES: usize = COUNT * size_of::<f32>();
pub(super) const TIME_ID: u32 = 617;
pub(super) const TONE_ID: u32 = 618;
pub(super) const VOLUME_ID: u32 = 619;
pub(super) const MIN_VOLUME_DB: f32 = -72.0;
pub(super) const MAX_VOLUME_DB: f32 = 6.0;

pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    [
        ParameterDescriptor {
            id: ParameterId::from_static(TIME_ID),
            label: "time",
            kind: ParameterKind::Float {
                min: -100.0,
                max: 100.0,
                unit: Some("%"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        ParameterDescriptor {
            id: ParameterId::from_static(TONE_ID),
            label: "tone",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 100.0,
                unit: Some("%"),
            },
            default: ParameterValue::Float(100.0),
            presentation: ParameterPresentation::Linear,
        },
        ParameterDescriptor {
            id: ParameterId::from_static(VOLUME_ID),
            label: "volume",
            kind: ParameterKind::Float {
                min: MIN_VOLUME_DB,
                max: MAX_VOLUME_DB,
                unit: Some("db"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings {
    pub time: f32,
    pub tone: f32,
    pub volume: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            time: 0.0,
            tone: 100.0,
            volume: 0.0,
        }
    }
}

impl Settings {
    pub(super) fn is_neutral(self) -> bool {
        self.time == 0.0 && self.tone == 100.0 && self.volume == 0.0
    }

    pub(super) fn time_rate(self) -> f64 {
        time_rate_for(f64::from(self.time))
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
        let word = |offset: usize| -> Result<f32, DeviceContractError> {
            let raw: [u8; 4] = bytes
                .get(offset..offset + 4)
                .ok_or(DeviceContractError::MalformedState)?
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?;
            Ok(f32::from_le_bytes(raw))
        };
        let settings = Self {
            time: word(0)?,
            tone: word(4)?,
            volume: word(8)?,
        };
        settings.validate()?;
        Ok(settings)
    }

    pub(super) fn append(self, payload: &mut Vec<u8>) {
        payload.extend_from_slice(&self.time.to_le_bytes());
        payload.extend_from_slice(&self.tone.to_le_bytes());
        payload.extend_from_slice(&self.volume.to_le_bytes());
    }

    pub(super) fn read(self, id: ParameterId) -> Option<ParameterValue> {
        Some(match id.value() {
            TIME_ID => ParameterValue::Float(self.time),
            TONE_ID => ParameterValue::Float(self.tone),
            VOLUME_ID => ParameterValue::Float(self.volume),
            _ => return None,
        })
    }

    pub(super) fn patch(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<bool, DeviceContractError> {
        let index = match id.value() {
            TIME_ID => 0,
            TONE_ID => 1,
            VOLUME_ID => 2,
            _ => return Ok(false),
        };
        descriptors()[index].validate(value)?;
        let ParameterValue::Float(v) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        match index {
            0 => self.time = v,
            1 => self.tone = v,
            _ => self.volume = v,
        }
        Ok(true)
    }
}

/// Envelope advance speed multiplier derived from global `time` (-100..+100%).
///
/// Negative values accelerate envelopes (up to 4x faster at -100%); positive
/// values lengthen envelopes (down to 0.25x speed at +100%). At 0% returns
/// exact 1.0.
#[inline]
pub(super) fn time_rate_for(time_pct: f64) -> f64 {
    if time_pct == 0.0 {
        1.0
    } else {
        (-time_pct.clamp(-100.0, 100.0) / 50.0).exp2()
    }
}

/// Linear amplitude gain derived from global `volume` (-72..+6 dB).
///
/// At or below -72 dB outputs exact silence (`0.0`); at 0 dB returns exact
/// unity (`1.0`).
#[inline]
pub(super) fn volume_gain_for(volume_db: f64) -> f32 {
    if volume_db <= f64::from(MIN_VOLUME_DB) {
        0.0
    } else if volume_db == 0.0 {
        1.0
    } else {
        10.0_f64.powf(volume_db.clamp(f64::from(MIN_VOLUME_DB), f64::from(MAX_VOLUME_DB)) / 20.0)
            as f32
    }
}

/// One-pole low-pass coefficient for global `tone` (0..100%).
///
/// Returns `None` at 100% (exact bypass). Below 100% maps exponentially from
/// 200 Hz (0%) to 20 kHz (100%), clamped to `0.45 * sample_rate`.
#[inline]
pub(super) fn tone_alpha_for(tone_pct: f64, sample_rate: u32) -> Option<f64> {
    if tone_pct >= 100.0 || sample_rate == 0 {
        return None;
    }
    let normalized = tone_pct.clamp(0.0, 100.0) / 100.0;
    let cutoff = (200.0 * 100.0_f64.powf(normalized)).min(0.45 * f64::from(sample_rate));
    let alpha =
        (1.0 - (-std::f64::consts::TAU * cutoff / f64::from(sample_rate)).exp()).clamp(0.0, 1.0);
    Some(alpha)
}

/// Lightweight stereo one-pole low-pass filter for Vector's global `tone` control.
#[derive(Clone, Copy, Default)]
pub(super) struct ToneFilter {
    state: [f64; 2],
}

impl ToneFilter {
    #[inline]
    pub(super) fn process_sample(&mut self, frame: [f32; 2], alpha: Option<f64>) -> [f32; 2] {
        let Some(alpha) = alpha else {
            self.state = [f64::from(frame[0]), f64::from(frame[1])];
            return frame;
        };
        let mut out = [0.0_f32; 2];
        for ch in 0..2 {
            let y = self.state[ch] + alpha * (f64::from(frame[ch]) - self.state[ch]);
            self.state[ch] = hex_dsp::flush_below(y, hex_dsp::POWER_FLOOR);
            out[ch] = y as f32;
        }
        out
    }

    pub(super) fn reset(&mut self) {
        self.state = [0.0; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{
        DevicePreparationContext, NativeInstrumentEvent, NativeInstrumentRuntimeLimits,
        NativeInstrumentRuntimePair, NativeParameterAutomation,
    };

    fn render_notes(
        state: &NativeDeviceState,
        rate: u32,
        frames: usize,
        chunk_size: usize,
        gate_frame: usize,
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
            .expect("mutable runtime");
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
            let end = (offset + chunk_size).min(frames);
            let mut events = Vec::new();
            if offset == 0 {
                events.push(NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 127,
                });
            }
            if gate_frame >= offset && gate_frame < end {
                events.push(NativeInstrumentEvent::NoteOff {
                    frame_offset: gate_frame - offset,
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
    fn descriptors_validation_and_v18_migration_are_strict_and_lossless() {
        let default = OPERATOR_METADATA.default_state().unwrap();
        assert_eq!(default.state_version(), OPERATOR_STATE_VERSION);
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&default, parameter::TIME),
            Ok(ParameterValue::Float(0.0))
        );
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&default, parameter::TONE),
            Ok(ParameterValue::Float(100.0))
        );
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&default, parameter::VOLUME),
            Ok(ParameterValue::Float(0.0))
        );

        // Historical v18 TLV migrates cleanly to v19 with neutral performance defaults.
        let settings = OperatorSettings::from_state(&default).unwrap();
        let v18_bytes = tlv_codec::encode_v18(&settings).unwrap();
        let v18 = NativeDeviceState::new(OPERATOR_TYPE_ID, TLV_STATE_VERSION, v18_bytes).unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&v18).unwrap();
        assert_eq!(migrated, default);
        assert_eq!(
            render_notes(&v18, 48_000, 256, 64, 128),
            render_notes(&migrated, 48_000, 256, 64, 128),
            "v18 -> v19 migration must render bit-identically at neutral defaults"
        );

        // Reject out-of-range and non-finite inputs.
        for (id, bad) in [
            (parameter::TIME, -100.1),
            (parameter::TIME, 100.1),
            (parameter::TIME, f32::NAN),
            (parameter::TONE, -0.1),
            (parameter::TONE, 100.1),
            (parameter::TONE, f32::INFINITY),
            (parameter::VOLUME, -72.1),
            (parameter::VOLUME, 6.1),
            (parameter::VOLUME, f32::NEG_INFINITY),
        ] {
            assert!(
                OPERATOR_METADATA
                    .patch_parameter(&default, id, ParameterValue::Float(bad))
                    .is_err()
            );
        }
        assert!(Settings::decode(&[0_u8; BYTES - 1]).is_err());
    }

    #[test]
    fn volume_tone_and_time_drive_audible_output_and_are_block_partition_invariant() {
        // Sawtooth carrier with short attack/decay/release so time, tone, and volume
        // all produce unmistakable measurable changes.
        let mut base = OPERATOR_METADATA.default_state().unwrap();
        for (id, value) in [
            (parameter::waveform(1), ParameterValue::Choice(1)), // saw d
            (
                parameter::envelope(1, parameter::EnvelopeField::Attack),
                ParameterValue::Float(0.002),
            ),
            (
                parameter::envelope(1, parameter::EnvelopeField::Decay),
                ParameterValue::Float(0.01),
            ),
            (
                parameter::envelope(1, parameter::EnvelopeField::Sustain),
                ParameterValue::Float(0.2),
            ),
            (
                parameter::envelope(1, parameter::EnvelopeField::Release),
                ParameterValue::Float(0.01),
            ),
        ] {
            base = OPERATOR_METADATA.patch_parameter(&base, id, value).unwrap();
        }

        // Volume: -72 dB is exact silence, -6 dB attenuates ~half, +6 dB boosts ~double.
        let unity = render_notes(&base, 48_000, 256, 64, 200);
        let silent_state = OPERATOR_METADATA
            .patch_parameter(&base, parameter::VOLUME, ParameterValue::Float(-72.0))
            .unwrap();
        let silent = render_notes(&silent_state, 48_000, 256, 64, 200);
        assert!(silent.iter().all(|f| *f == [0.0, 0.0]));

        let minus_six = OPERATOR_METADATA
            .patch_parameter(&base, parameter::VOLUME, ParameterValue::Float(-6.0206))
            .unwrap();
        let half = render_notes(&minus_six, 48_000, 256, 64, 200);
        for (u, h) in unity.iter().zip(&half) {
            assert!((h[0] - u[0] * 0.5).abs() < 1e-4);
            assert!((h[1] - u[1] * 0.5).abs() < 1e-4);
        }
        // Block-partition invariance on volume + tone + time combined across rates.
        for rate in [8_000, 48_000, 192_000] {
            let mut combined = base.clone();
            combined = OPERATOR_METADATA
                .patch_parameter(&combined, parameter::TIME, ParameterValue::Float(-50.0))
                .unwrap();
            combined = OPERATOR_METADATA
                .patch_parameter(&combined, parameter::TONE, ParameterValue::Float(25.0))
                .unwrap();
            combined = OPERATOR_METADATA
                .patch_parameter(&combined, parameter::VOLUME, ParameterValue::Float(-3.0))
                .unwrap();
            let whole = render_notes(&combined, rate, 256, 256, 120);
            let split = render_notes(&combined, rate, 256, 17, 120);
            assert_eq!(whole, split, "block-partition mismatch at {rate} Hz");
            assert!(whole.iter().flatten().all(|s| s.is_finite()));
        }

        // Tone: 0% attenuates high-frequency sawtooth harmonic energy relative to 100%.
        let dark_state = OPERATOR_METADATA
            .patch_parameter(&base, parameter::TONE, ParameterValue::Float(0.0))
            .unwrap();
        let dark = render_notes(&dark_state, 48_000, 512, 64, 500);
        let bright = render_notes(&base, 48_000, 512, 64, 500);
        let hf_energy = |buf: &[[f32; 2]]| -> f64 {
            buf.windows(2)
                .map(|w| {
                    let d = f64::from(w[1][0] - w[0][0]);
                    d * d
                })
                .sum()
        };
        assert!(
            hf_energy(&dark) < hf_energy(&bright) * 0.25,
            "tone=0% must significantly attenuate high-frequency slope energy ({} vs {})",
            hf_energy(&dark),
            hf_energy(&bright)
        );

        // Time: -100% decays much faster than +100% during the decay segment.
        let fast_state = OPERATOR_METADATA
            .patch_parameter(&base, parameter::TIME, ParameterValue::Float(-100.0))
            .unwrap();
        let slow_state = OPERATOR_METADATA
            .patch_parameter(&base, parameter::TIME, ParameterValue::Float(100.0))
            .unwrap();
        let fast = render_notes(&fast_state, 48_000, 1024, 64, 1000);
        let slow = render_notes(&slow_state, 48_000, 1024, 64, 1000);
        let tail_rms = |buf: &[[f32; 2]]| -> f64 {
            (buf[512..960]
                .iter()
                .map(|f| f64::from(f[0]) * f64::from(f[0]))
                .sum::<f64>()
                / 448.0)
                .sqrt()
        };
        assert!(
            tail_rms(&slow) > tail_rms(&fast) * 1.3,
            "positive time must lengthen envelope decay relative to negative time ({} vs {})",
            tail_rms(&slow),
            tail_rms(&fast)
        );
    }

    #[test]
    fn sample_rate_automation_drives_volume_tone_and_time() {
        struct Single {
            id: [ParameterId; 1],
            values: Vec<f32>,
        }
        impl NativeParameterAutomation for Single {
            fn parameter_ids(&self) -> &[ParameterId] {
                &self.id
            }
            fn value_at(&self, parameter: ParameterId, offset: usize) -> Option<ParameterValue> {
                (parameter == self.id[0])
                    .then(|| self.values.get(offset).copied().map(ParameterValue::Float))
                    .flatten()
            }
        }

        let state = OPERATOR_METADATA
            .patch_parameter(
                &OPERATOR_METADATA.default_state().unwrap(),
                parameter::waveform(1),
                ParameterValue::Choice(1),
            )
            .unwrap();
        let tuning = hex_lattice::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        };
        let factory = OPERATOR_METADATA
            .prepare_instrument(&state, &context)
            .unwrap()
            .prepare_mutable_runtime()
            .unwrap()
            .unwrap();
        let limits = NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 64,
            max_tail_frames: 48_000 * 600,
        };
        let run = |id: ParameterId, val: f32| {
            let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
            let mut out = [[0.0_f32; 2]; 256];
            let events = [NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 127,
            }];
            let auto = Single {
                id: [id],
                values: vec![val; 256],
            };
            pair.begin_block();
            pair.process_block_with_clock_and_automation(
                &events,
                &mut out,
                None,
                &[],
                &auto,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
            pair.commit_block();
            out
        };

        assert_ne!(run(parameter::VOLUME, 0.0), run(parameter::VOLUME, -12.0));
        assert_ne!(run(parameter::TONE, 100.0), run(parameter::TONE, 10.0));
        assert_ne!(run(parameter::TIME, -100.0), run(parameter::TIME, 100.0));
    }
}
