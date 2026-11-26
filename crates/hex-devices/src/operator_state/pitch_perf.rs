//! Version-20 global pitch performance controls (`transpose`, `spread`, `glide`).
//!
//! Neutral defaults (`transpose = 0 st`, `spread = 0%`, `glide enabled = false`,
//! `glide time = 0.05 s`) preserve bit-identical renders for all historical
//! v1..v19 patches.
use super::*;

pub(super) const COUNT: usize = 4;
pub(super) const BYTES: usize = COUNT * 4;
pub(super) const TRANSPOSE_ID: u32 = 640;
pub(super) const SPREAD_ID: u32 = 641;
pub(super) const GLIDE_ENABLED_ID: u32 = 642;
pub(super) const GLIDE_TIME_ID: u32 = 643;
pub(super) const MIN_TRANSPOSE_ST: f32 = -48.0;
pub(super) const MAX_TRANSPOSE_ST: f32 = 48.0;
pub(super) const MIN_GLIDE_SECONDS: f32 = 0.001;
pub(super) const MAX_GLIDE_SECONDS: f32 = 5.0;
pub(super) const DEFAULT_GLIDE_SECONDS: f32 = 0.05;

pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    [
        ParameterDescriptor {
            id: ParameterId::from_static(TRANSPOSE_ID),
            label: "transpose",
            kind: ParameterKind::Float {
                min: MIN_TRANSPOSE_ST,
                max: MAX_TRANSPOSE_ST,
                unit: Some("st"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        ParameterDescriptor {
            id: ParameterId::from_static(SPREAD_ID),
            label: "spread",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 100.0,
                unit: Some("%"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        ParameterDescriptor {
            id: ParameterId::from_static(GLIDE_ENABLED_ID),
            label: "glide enabled",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        ParameterDescriptor {
            id: ParameterId::from_static(GLIDE_TIME_ID),
            label: "glide time",
            kind: ParameterKind::Float {
                min: MIN_GLIDE_SECONDS,
                max: MAX_GLIDE_SECONDS,
                unit: Some("s"),
            },
            default: ParameterValue::Float(DEFAULT_GLIDE_SECONDS),
            presentation: ParameterPresentation::Logarithmic,
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings {
    pub transpose: f32,
    pub spread: f32,
    pub glide_enabled: bool,
    pub glide_time: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            transpose: 0.0,
            spread: 0.0,
            glide_enabled: false,
            glide_time: DEFAULT_GLIDE_SECONDS,
        }
    }
}

impl Settings {
    pub(super) fn is_neutral(self) -> bool {
        self.transpose == 0.0 && self.spread == 0.0 && !self.glide_enabled
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
        let u32_word = |offset: usize| -> Result<u32, DeviceContractError> {
            let raw: [u8; 4] = bytes
                .get(offset..offset + 4)
                .ok_or(DeviceContractError::MalformedState)?
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?;
            Ok(u32::from_le_bytes(raw))
        };
        let transpose = f32::from_bits(u32_word(0)?);
        let spread = f32::from_bits(u32_word(4)?);
        let glide_flag = u32_word(8)?;
        if glide_flag > 1 {
            return Err(DeviceContractError::MalformedState);
        }
        let glide_time = f32::from_bits(u32_word(12)?);
        let settings = Self {
            transpose,
            spread,
            glide_enabled: glide_flag != 0,
            glide_time,
        };
        settings.validate()?;
        Ok(settings)
    }

    pub(super) fn append(self, payload: &mut Vec<u8>) {
        payload.extend_from_slice(&self.transpose.to_le_bytes());
        payload.extend_from_slice(&self.spread.to_le_bytes());
        payload.extend_from_slice(&u32::from(self.glide_enabled).to_le_bytes());
        payload.extend_from_slice(&self.glide_time.to_le_bytes());
    }

    pub(super) fn read(self, id: ParameterId) -> Option<ParameterValue> {
        Some(match id.value() {
            TRANSPOSE_ID => ParameterValue::Float(self.transpose),
            SPREAD_ID => ParameterValue::Float(self.spread),
            GLIDE_ENABLED_ID => ParameterValue::Bool(self.glide_enabled),
            GLIDE_TIME_ID => ParameterValue::Float(self.glide_time),
            _ => return None,
        })
    }

    pub(super) fn patch(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<bool, DeviceContractError> {
        let index = match id.value() {
            TRANSPOSE_ID => 0,
            SPREAD_ID => 1,
            GLIDE_ENABLED_ID => 2,
            GLIDE_TIME_ID => 3,
            _ => return Ok(false),
        };
        descriptors()[index].validate(value)?;
        match (index, value) {
            (0, ParameterValue::Float(v)) => self.transpose = v,
            (1, ParameterValue::Float(v)) => self.spread = v,
            (2, ParameterValue::Bool(v)) => self.glide_enabled = v,
            (3, ParameterValue::Float(v)) => self.glide_time = v,
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
        Ok(true)
    }
}

/// Per-sample exponential decay multiplier for portamento glide.
///
/// Reaches within ~0.67% (`e^-5`) of the target pitch after `glide_time_s`.
#[inline]
pub(super) fn glide_decay_coeff(glide_time_s: f64, sample_rate: u32) -> f64 {
    if sample_rate == 0 {
        return 0.0;
    }
    let frames = (glide_time_s.clamp(f64::from(MIN_GLIDE_SECONDS), f64::from(MAX_GLIDE_SECONDS))
        * f64::from(sample_rate))
    .max(1.0);
    (-5.0 / frames).exp()
}

/// Apply a semitone pitch offset (transpose + glide) weighted by each operator's
/// `pitch_keytrack` (0% fixed oscillators remain unaffected; 100% follows normally).
#[inline]
pub(super) fn scale_steps_by_semitones(
    steps: [u64; OPERATORS],
    semitones: f64,
    keytrack_pct: [f32; OPERATORS],
) -> [u64; OPERATORS] {
    if semitones == 0.0 {
        return steps;
    }
    let full_ratio = (semitones / 12.0).exp2();
    std::array::from_fn(|op| {
        let kt = f64::from(keytrack_pct[op]);
        if kt <= 0.0 {
            steps[op]
        } else {
            let ratio = if kt >= 100.0 {
                full_ratio
            } else {
                (semitones * (kt / 100.0) / 12.0).exp2()
            };
            (((steps[op] as f64) * ratio).round() as u64).clamp(1, (1_u64 << 63) - 1)
        }
    })
}

/// Fixed inline stereo detune-spread stage for Vector's global `spread` control.
///
/// Uses a bounded 1024-sample ring buffer per channel with quadrature-modulated
/// fractional delay taps so even a single monophonic voice gains smooth stereo
/// pitch detuning and width. At `spread == 0%` the stage is bypassed bit-identically.
const SPREAD_BUFFER_LEN: usize = 1024;
const SPREAD_BUFFER_MASK: usize = SPREAD_BUFFER_LEN - 1;
const SPREAD_LFO_HZ: f64 = 0.35;

#[derive(Clone, Copy)]
pub(super) struct SpreadStage {
    left_buf: [f32; SPREAD_BUFFER_LEN],
    right_buf: [f32; SPREAD_BUFFER_LEN],
    write_pos: usize,
    lfo_phase: f64,
}

impl Default for SpreadStage {
    fn default() -> Self {
        Self {
            left_buf: [0.0; SPREAD_BUFFER_LEN],
            right_buf: [0.0; SPREAD_BUFFER_LEN],
            write_pos: 0,
            lfo_phase: 0.0,
        }
    }
}

impl SpreadStage {
    pub(super) fn reset(&mut self) {
        self.left_buf.fill(0.0);
        self.right_buf.fill(0.0);
        self.write_pos = 0;
        self.lfo_phase = 0.0;
    }

    #[inline]
    fn read_linear(buf: &[f32; SPREAD_BUFFER_LEN], write_pos: usize, delay_samples: f64) -> f32 {
        let clamped = delay_samples.clamp(1.0, (SPREAD_BUFFER_LEN - 2) as f64);
        let int_delay = clamped.floor() as usize;
        let frac = (clamped - int_delay as f64) as f32;
        let idx0 = write_pos.wrapping_sub(int_delay) & SPREAD_BUFFER_MASK;
        let idx1 = write_pos.wrapping_sub(int_delay + 1) & SPREAD_BUFFER_MASK;
        let a = buf[idx0];
        let b = buf[idx1];
        a + (b - a) * frac
    }

    #[inline]
    pub(super) fn process_sample(
        &mut self,
        frame: [f32; 2],
        spread_pct: f64,
        sample_rate: u32,
    ) -> [f32; 2] {
        let amount = (spread_pct.clamp(0.0, 100.0) / 100.0) as f32;
        if amount == 0.0 || sample_rate == 0 {
            return frame;
        }
        let pos = self.write_pos;
        self.left_buf[pos] = frame[0];
        self.right_buf[pos] = frame[1];
        // Scale base delay (3 ms) and modulation depth (up to 2.2 ms at 100%)
        // smoothly with sample rate, bounded inside the 1024-sample ring buffer.
        let rate_f = f64::from(sample_rate);
        let max_safe = (SPREAD_BUFFER_LEN - 4) as f64;
        let center_delay = (0.003 * rate_f).min(max_safe * 0.45);
        let mod_depth = (0.0022 * rate_f * f64::from(amount)).min(center_delay * 0.85);
        let angle = std::f64::consts::TAU * self.lfo_phase;
        let mod_l = angle.sin();
        let mod_r = angle.cos();
        let wet_l = Self::read_linear(&self.left_buf, pos, center_delay + mod_depth * mod_l);
        let wet_r = Self::read_linear(&self.right_buf, pos, center_delay - mod_depth * mod_r);
        self.write_pos = (pos + 1) & SPREAD_BUFFER_MASK;
        self.lfo_phase = (self.lfo_phase + SPREAD_LFO_HZ / rate_f).fract();
        let mix = 0.5 * amount;
        [
            frame[0] * (1.0 - mix) + wet_l * mix,
            frame[1] * (1.0 - mix) + wet_r * mix,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{
        DevicePreparationContext, NativeInstrumentEvent, NativeInstrumentRuntimeLimits,
        NativeInstrumentRuntimePair, NativeParameterAutomation,
    };

    fn render_sequence(
        state: &NativeDeviceState,
        rate: u32,
        frames: usize,
        chunk_size: usize,
        events: &[NativeInstrumentEvent],
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
            let mut slice_events = Vec::new();
            for &ev in events {
                let at = ev.frame_offset();
                if at >= offset && at < end {
                    let shifted = match ev {
                        NativeInstrumentEvent::NoteOn {
                            occurrence_id,
                            pitch,
                            velocity,
                            ..
                        } => NativeInstrumentEvent::NoteOn {
                            frame_offset: at - offset,
                            occurrence_id,
                            pitch,
                            velocity,
                        },
                        NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                            NativeInstrumentEvent::NoteOff {
                                frame_offset: at - offset,
                                occurrence_id,
                            }
                        }
                        other => other,
                    };
                    slice_events.push(shifted);
                }
            }
            pair.begin_block();
            pair.process_block(
                &slice_events,
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
    fn descriptors_validation_and_v19_migration_are_strict_and_lossless() {
        let default = OPERATOR_METADATA.default_state().unwrap();
        assert_eq!(default.state_version(), OPERATOR_STATE_VERSION);
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&default, parameter::TRANSPOSE),
            Ok(ParameterValue::Float(0.0))
        );
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&default, parameter::SPREAD),
            Ok(ParameterValue::Float(0.0))
        );
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&default, parameter::GLIDE_ENABLED),
            Ok(ParameterValue::Bool(false))
        );
        assert_eq!(
            OPERATOR_METADATA.read_parameter(&default, parameter::GLIDE_TIME),
            Ok(ParameterValue::Float(DEFAULT_GLIDE_SECONDS))
        );

        // Historical v19 TLV migrates losslessly to v20 and renders bit-identically.
        let settings = OperatorSettings::from_state(&default).unwrap();
        let v19 = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            PERFORMANCE_STATE_VERSION,
            tlv_codec::encode_v19(&settings).unwrap(),
        )
        .unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&v19).unwrap();
        assert_eq!(migrated, default);
        let note = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 127,
        }];
        assert_eq!(
            render_sequence(&v19, 48_000, 256, 64, &note),
            render_sequence(&migrated, 48_000, 256, 64, &note)
        );
        // Tag 16 is rejected if presented inside a v19 state.
        assert!(tlv_codec::decode(default.payload(), PERFORMANCE_STATE_VERSION).is_err());

        for (id, bad) in [
            (parameter::TRANSPOSE, ParameterValue::Float(-48.1)),
            (parameter::TRANSPOSE, ParameterValue::Float(48.1)),
            (parameter::TRANSPOSE, ParameterValue::Float(f32::NAN)),
            (parameter::SPREAD, ParameterValue::Float(-0.1)),
            (parameter::SPREAD, ParameterValue::Float(100.1)),
            (parameter::GLIDE_TIME, ParameterValue::Float(0.0005)),
            (parameter::GLIDE_TIME, ParameterValue::Float(5.1)),
            (parameter::GLIDE_TIME, ParameterValue::Float(f32::INFINITY)),
        ] {
            assert!(
                OPERATOR_METADATA
                    .patch_parameter(&default, id, bad)
                    .is_err()
            );
        }
        let mut bad_bool = Vec::new();
        Settings::default().append(&mut bad_bool);
        bad_bool[8..12].copy_from_slice(&2_u32.to_le_bytes());
        assert!(Settings::decode(&bad_bool).is_err());
    }

    #[test]
    fn transpose_matches_note_shift_and_respects_fixed_oscillator_keytrack() {
        let base = OPERATOR_METADATA.default_state().unwrap();
        let transposed = OPERATOR_METADATA
            .patch_parameter(&base, parameter::TRANSPOSE, ParameterValue::Float(12.0))
            .unwrap();
        let note_c4 = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 127,
        }];
        let note_c5 = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 72,
            velocity: 127,
        }];
        let shifted = render_sequence(&transposed, 48_000, 256, 64, &note_c4);
        let octave_up = render_sequence(&base, 48_000, 256, 64, &note_c5);
        for (a, b) in shifted.iter().zip(&octave_up) {
            assert!(
                (a[0] - b[0]).abs() < 1e-4,
                "transpose +12 st should match playing C5 ({} vs {})",
                a[0],
                b[0]
            );
        }

        // Fixed oscillator (pitch_keytrack == 0%) must ignore global transpose.
        let fixed_base = OPERATOR_METADATA
            .patch_parameter(&base, parameter::fixed(1), ParameterValue::Bool(true))
            .unwrap();
        let fixed_transposed = OPERATOR_METADATA
            .patch_parameter(
                &fixed_base,
                parameter::TRANSPOSE,
                ParameterValue::Float(24.0),
            )
            .unwrap();
        assert_eq!(
            render_sequence(&fixed_base, 48_000, 256, 64, &note_c4),
            render_sequence(&fixed_transposed, 48_000, 256, 64, &note_c4)
        );
    }

    #[test]
    fn spread_widens_single_note_stereo_and_glide_slides_mono_and_poly_across_rates() {
        let base = OPERATOR_METADATA.default_state().unwrap();
        let note = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 127,
        }];
        let dry = render_sequence(&base, 48_000, 512, 64, &note);
        assert!(dry.iter().all(|f| f[0] == f[1]));

        let wide_state = OPERATOR_METADATA
            .patch_parameter(&base, parameter::SPREAD, ParameterValue::Float(100.0))
            .unwrap();
        let wide = render_sequence(&wide_state, 48_000, 512, 64, &note);
        assert!(wide.iter().flatten().all(|s| s.is_finite()));
        assert!(
            wide.iter().any(|f| (f[0] - f[1]).abs() > 1e-3),
            "100% spread must produce genuine L/R stereo width on a single note"
        );

        // Glide in both mono (voices=1) and poly (voices=8) across rates and partitions.
        for (voices_choice, rate) in [(0_u32, 8_000), (0, 48_000), (7, 48_000), (0, 192_000)] {
            let mut st = OPERATOR_METADATA
                .patch_parameter(
                    &base,
                    parameter::VOICES,
                    ParameterValue::Choice(voices_choice),
                )
                .unwrap();
            st = OPERATOR_METADATA
                .patch_parameter(&st, parameter::GLIDE_ENABLED, ParameterValue::Bool(true))
                .unwrap();
            st = OPERATOR_METADATA
                .patch_parameter(&st, parameter::GLIDE_TIME, ParameterValue::Float(0.04))
                .unwrap();
            st = OPERATOR_METADATA
                .patch_parameter(&st, parameter::SPREAD, ParameterValue::Float(50.0))
                .unwrap();
            let seq = [
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 48,
                    velocity: 127,
                },
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 64,
                    occurrence_id: 2,
                    pitch: 72,
                    velocity: 127,
                },
                NativeInstrumentEvent::NoteOff {
                    frame_offset: 192,
                    occurrence_id: 2,
                },
            ];
            let whole = render_sequence(&st, rate, 320, 320, &seq);
            let split = render_sequence(&st, rate, 320, 19, &seq);
            assert_eq!(whole, split, "block-partition mismatch at {rate} Hz");
            assert!(whole.iter().flatten().all(|s| s.is_finite()));

            let no_glide = OPERATOR_METADATA
                .patch_parameter(&st, parameter::GLIDE_ENABLED, ParameterValue::Bool(false))
                .unwrap();
            let stepped = render_sequence(&no_glide, rate, 320, 64, &seq);
            assert_ne!(
                &whole[64..192],
                &stepped[64..192],
                "glide must audibly slide second note pitch (voices choice {voices_choice}, {rate} Hz)"
            );
        }
    }

    #[test]
    fn sample_rate_automation_drives_transpose_and_spread() {
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
        let state = OPERATOR_METADATA.default_state().unwrap();
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
        assert_ne!(
            run(parameter::TRANSPOSE, 0.0),
            run(parameter::TRANSPOSE, 7.0)
        );
        assert_ne!(run(parameter::SPREAD, 0.0), run(parameter::SPREAD, 80.0));
    }
}
