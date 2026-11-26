//! Version-23 filter waveshaper stage and nonlinear filter circuit drive.
//!
//! All nonlinear circuit (`osr`, `ms2`, `smp`, `prd`) and shaper (`soft`, `hard`,
//! `sine`, `4bit`) transfer laws are original bounded Hex mathematical functions
//! executed inside the existing 2x oversampled voice path before decimation.
//! No analog hardware emulation is claimed.
//!
//! Neutral defaults (`filter circuit = clean`, `filter drive = 0 dB`,
//! `shaper type = off`) bypass the nonlinear stage completely and preserve
//! bit-identical renders for all historical v1..v22 patches.
use super::*;

pub(super) const FIRST_ID: u32 = 687;
pub(super) const COUNT: usize = 4;
pub(super) const BYTES: usize = COUNT * 4;

pub(super) const SHAPER_TYPE_ID: u32 = FIRST_ID;
pub(super) const SHAPER_DRIVE_ID: u32 = FIRST_ID + 1;
pub(super) const SHAPER_DRY_WET_ID: u32 = FIRST_ID + 2;
pub(super) const FILTER_DRIVE_ID: u32 = FIRST_ID + 3;

pub(super) const SHAPER_TYPES: [&str; 5] = ["off", "soft", "hard", "sine", "4bit"];

pub(super) const fn descriptors() -> [ParameterDescriptor; COUNT] {
    [
        ParameterDescriptor {
            id: ParameterId::from_static(SHAPER_TYPE_ID),
            label: "shaper type",
            kind: ParameterKind::Choice {
                labels: &SHAPER_TYPES,
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        ParameterDescriptor {
            id: ParameterId::from_static(SHAPER_DRIVE_ID),
            label: "shaper drive",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 100.0,
                unit: Some("%"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        ParameterDescriptor {
            id: ParameterId::from_static(SHAPER_DRY_WET_ID),
            label: "shaper dry/wet",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 100.0,
                unit: Some("%"),
            },
            default: ParameterValue::Float(100.0),
            presentation: ParameterPresentation::Linear,
        },
        ParameterDescriptor {
            id: ParameterId::from_static(FILTER_DRIVE_ID),
            label: "filter drive",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 24.0,
                unit: Some("db"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
    ]
}

/// Original Hex nonlinear filter circuit saturation laws.
///
/// Every mode maps `0.0 -> 0.0`, rejects non-finite inputs to `0.0`, and stays
/// strictly bounded in `[-1.0, 1.0]` for any finite input and drive in `0..=24 dB`:
/// - `0` (`clean`): linear bypass.
/// - `1` (`osr`): smooth symmetric hyperbolic tangent with gentle drive scaling.
/// - `2` (`ms2`): asymmetric soft-knee positive / hyperbolic negative clipper.
/// - `3` (`smp`): cubic-boosted arctangent saturation.
/// - `4` (`prd`): asymmetric dual-stage hyperbolic saturation.
#[inline]
pub(super) fn apply_circuit(x: f64, circuit: u32, drive_db: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return 0.0;
    }
    if circuit == 0 {
        return x;
    }
    let drive = 10.0_f64.powf(drive_db.clamp(0.0, 24.0) / 20.0);
    let y = match circuit {
        1 => {
            let u = (x * drive).clamp(-32.0, 32.0);
            u.tanh() / (1.0 + 0.12 * (drive - 1.0)).min(2.2)
        }
        2 => {
            let u = (x * drive * 1.15).clamp(-32.0, 32.0);
            if u >= 0.0 {
                u / (1.0 + u)
            } else {
                (1.1 * u).tanh() * 0.92
            }
        }
        3 => {
            let u = (x * drive).clamp(-16.0, 16.0);
            let c = u.clamp(-4.0, 4.0);
            std::f64::consts::FRAC_2_PI * (1.35 * u + 0.25 * c * c * c).atan()
        }
        4 => {
            let u = (x * drive * 1.25).clamp(-32.0, 32.0);
            let asym = u + 0.18 * (u * u).min(4.0) / (1.0 + u.abs());
            asym.tanh()
        }
        _ => x,
    };
    if y.is_finite() { y } else { 0.0 }
}

/// Original Hex waveshaper transfer laws.
///
/// Every mode maps `0.0 -> 0.0`, rejects non-finite inputs to `0.0`, and stays
/// strictly bounded:
/// - `0` (`off`): linear bypass.
/// - `1` (`soft`): hyperbolic tangent soft clipper (`tanh`).
/// - `2` (`hard`): bounded unity hard clipper (`clamp(-1, 1)`).
/// - `3` (`sine`): sine wavefolder (`sin(u * pi / 2)`).
/// - `4` (`4bit`): 4-bit symmetric signed amplitude quantizer (`round(clamp(u) * 7.0) / 7.0`).
#[inline]
pub(super) fn apply_shaper(x: f64, shaper_type: u32, drive_pct: f64, wet_pct: f64) -> f64 {
    if !x.is_finite() {
        return 0.0;
    }
    if shaper_type == 0 || x == 0.0 {
        return x;
    }
    let wet = (wet_pct / 100.0).clamp(0.0, 1.0);
    if wet == 0.0 {
        return x;
    }
    let gain = 1.0 + 15.0 * (drive_pct / 100.0).clamp(0.0, 1.0);
    let u = (x * gain).clamp(-64.0, 64.0);
    let shaped = match shaper_type {
        1 => u.tanh(),
        2 => u.clamp(-1.0, 1.0),
        3 => (u * std::f64::consts::FRAC_PI_2).sin(),
        4 => (u.clamp(-1.0, 1.0) * 7.0).round() / 7.0,
        _ => x,
    };
    let out = x + (shaped - x) * wet;
    if out.is_finite() { out } else { 0.0 }
}

/// Process one voice subframe inside the 2x oversampled voice path before decimation.
#[inline]
pub(super) fn process_subframe(
    sample: f32,
    filter_enabled: bool,
    circuit: u32,
    filter_drive_db: f64,
    shaper_type: u32,
    shaper_drive_pct: f64,
    shaper_wet_pct: f64,
) -> f32 {
    let mut x = f64::from(sample);
    if !x.is_finite() {
        return 0.0;
    }
    if filter_enabled && circuit != 0 {
        x = apply_circuit(x, circuit, filter_drive_db);
    }
    if shaper_type != 0 {
        x = apply_shaper(x, shaper_type, shaper_drive_pct, shaper_wet_pct);
    }
    if x.is_finite() { x as f32 } else { 0.0 }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Settings {
    pub(super) shaper_type: u32,
    pub(super) shaper_drive: f32,
    pub(super) shaper_dry_wet: f32,
    pub(super) filter_drive: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shaper_type: 0,
            shaper_drive: 0.0,
            shaper_dry_wet: 100.0,
            filter_drive: 0.0,
        }
    }
}

impl Settings {
    pub(super) fn is_neutral(self) -> bool {
        self.shaper_type == 0 && self.filter_drive == 0.0
    }

    pub(super) fn read(self, id: ParameterId) -> Option<ParameterValue> {
        match id.value() {
            SHAPER_TYPE_ID => Some(ParameterValue::Choice(self.shaper_type)),
            SHAPER_DRIVE_ID => Some(ParameterValue::Float(self.shaper_drive)),
            SHAPER_DRY_WET_ID => Some(ParameterValue::Float(self.shaper_dry_wet)),
            FILTER_DRIVE_ID => Some(ParameterValue::Float(self.filter_drive)),
            _ => None,
        }
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
        let read_u32 = |offset: usize| -> Result<u32, DeviceContractError> {
            let raw: [u8; 4] = bytes[offset..offset + 4]
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?;
            Ok(u32::from_le_bytes(raw))
        };
        let settings = Self {
            shaper_type: read_u32(0)?,
            shaper_drive: f32::from_bits(read_u32(4)?),
            shaper_dry_wet: f32::from_bits(read_u32(8)?),
            filter_drive: f32::from_bits(read_u32(12)?),
        };
        settings.validate()?;
        Ok(settings)
    }

    pub(super) fn append(self, payload: &mut Vec<u8>) {
        payload.extend_from_slice(&self.shaper_type.to_le_bytes());
        payload.extend_from_slice(&self.shaper_drive.to_le_bytes());
        payload.extend_from_slice(&self.shaper_dry_wet.to_le_bytes());
        payload.extend_from_slice(&self.filter_drive.to_le_bytes());
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
        match (id.value(), value) {
            (SHAPER_TYPE_ID, ParameterValue::Choice(v)) => self.shaper_type = v,
            (SHAPER_DRIVE_ID, ParameterValue::Float(v)) => self.shaper_drive = v,
            (SHAPER_DRY_WET_ID, ParameterValue::Float(v)) => self.shaper_dry_wet = v,
            (FILTER_DRIVE_ID, ParameterValue::Float(v)) => self.filter_drive = v,
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
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
                    pitch: 60,
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
    fn circuit_and_shaper_laws_are_bounded_zero_preserving_and_distinct() {
        for circuit in 0..=4 {
            assert_eq!(apply_circuit(0.0, circuit, 24.0), 0.0);
            assert_eq!(apply_circuit(f64::NAN, circuit, 12.0), 0.0);
            assert_eq!(apply_circuit(f64::INFINITY, circuit, 12.0), 0.0);
            assert_eq!(apply_circuit(f64::NEG_INFINITY, circuit, 12.0), 0.0);
            for x in [-1e6, -4.0, -0.5, 0.5, 4.0, 1e6] {
                let y = apply_circuit(x, circuit, 24.0);
                assert!(y.is_finite());
                if circuit != 0 {
                    assert!(y.abs() <= 1.0001, "circuit {circuit} exceeded bound: {y}");
                }
            }
        }
        // All 4 nonlinear circuits produce distinct outputs on the same driven sample.
        let circuit_vals: Vec<f64> = (1..=4).map(|c| apply_circuit(0.45, c, 12.0)).collect();
        for i in 0..circuit_vals.len() {
            for j in i + 1..circuit_vals.len() {
                assert!(
                    (circuit_vals[i] - circuit_vals[j]).abs() > 1e-3,
                    "circuits {} and {} too close: {} vs {}",
                    i + 1,
                    j + 1,
                    circuit_vals[i],
                    circuit_vals[j]
                );
            }
        }

        for shaper in 0..=4 {
            assert_eq!(apply_shaper(0.0, shaper, 100.0, 100.0), 0.0);
            assert_eq!(apply_shaper(f64::NAN, shaper, 100.0, 100.0), 0.0);
            assert_eq!(apply_shaper(f64::INFINITY, shaper, 100.0, 100.0), 0.0);
            // Dry/wet 0% returns dry input bit-identically.
            assert_eq!(apply_shaper(0.37, shaper, 85.0, 0.0), 0.37);
            for x in [-1e6, -2.0, -0.35, 0.35, 2.0, 1e6] {
                let y = apply_shaper(x, shaper, 100.0, 100.0);
                assert!(y.is_finite());
                if shaper != 0 {
                    assert!(y.abs() <= 1.0001, "shaper {shaper} exceeded bound: {y}");
                }
            }
        }
        let shaper_vals: Vec<f64> = (1..=4)
            .map(|s| apply_shaper(0.25, s, 10.0, 100.0))
            .collect();
        for i in 0..shaper_vals.len() {
            for j in i + 1..shaper_vals.len() {
                assert!(
                    (shaper_vals[i] - shaper_vals[j]).abs() > 1e-3,
                    "shapers {} and {} too close: {} vs {}",
                    i + 1,
                    j + 1,
                    shaper_vals[i],
                    shaper_vals[j]
                );
            }
        }
    }

    #[test]
    fn v22_migration_is_bit_identical_and_rejects_v23_tag_and_circuits_on_v22() {
        let mut base = OPERATOR_METADATA.default_state().unwrap();
        assert_eq!(base.state_version(), OPERATOR_STATE_VERSION);
        base = OPERATOR_METADATA
            .patch_parameter(
                &base,
                ParameterId::from_static(20_001),
                ParameterValue::Bool(true),
            )
            .unwrap();
        let settings = OperatorSettings::from_state(&base).unwrap();
        let v22 = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            FILTER_ENV_STATE_VERSION,
            tlv_codec::encode_v22(&settings).unwrap(),
        )
        .unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&v22).unwrap();
        assert_eq!(migrated, base);
        assert_eq!(
            render(&v22, 48_000, 256, 64, 128),
            render(&migrated, 48_000, 256, 64, 128)
        );
        // Tag 19 must be rejected on v22.
        assert!(tlv_codec::decode(base.payload(), FILTER_ENV_STATE_VERSION).is_err());

        // Non-zero filter circuit (> 0) must be rejected if presented inside a pre-v23 state.
        let osr_state = OPERATOR_METADATA
            .patch_parameter(
                &base,
                ParameterId::from_static(20_004),
                ParameterValue::Choice(1),
            )
            .unwrap();
        let osr_settings = OperatorSettings::from_state(&osr_state).unwrap();
        let bad_v22 = tlv_codec::encode_v22(&osr_settings).unwrap();
        assert!(tlv_codec::decode(&bad_v22, FILTER_ENV_STATE_VERSION).is_err());
        let bad_v17 = osr_settings.to_v17_state().unwrap();
        assert!(OperatorSettings::from_state(&bad_v17).is_err());

        // Out-of-range parameters are rejected.
        for (id, bad) in [
            (parameter::SHAPER_TYPE, ParameterValue::Choice(5)),
            (parameter::SHAPER_DRIVE, ParameterValue::Float(-0.1)),
            (parameter::SHAPER_DRIVE, ParameterValue::Float(100.5)),
            (parameter::SHAPER_DRY_WET, ParameterValue::Float(101.0)),
            (parameter::FILTER_DRIVE, ParameterValue::Float(-1.0)),
            (parameter::FILTER_DRIVE, ParameterValue::Float(24.5)),
            (ParameterId::from_static(20_004), ParameterValue::Choice(5)),
        ] {
            assert!(OPERATOR_METADATA.patch_parameter(&base, id, bad).is_err());
        }
    }

    #[test]
    fn nonlinear_circuits_and_shaper_render_in_oversampled_voice_path_across_rates() {
        let mut clean = OPERATOR_METADATA.default_state().unwrap();
        for (id, val) in [
            (parameter::GLOBAL_MODE, ParameterValue::Choice(1)),
            (ParameterId::from_static(20_001), ParameterValue::Bool(true)),
            (parameter::FILTER_FREQUENCY, ParameterValue::Float(2_000.0)),
            (parameter::oscillator_level(1), ParameterValue::Float(1.0)),
        ] {
            clean = OPERATOR_METADATA.patch_parameter(&clean, id, val).unwrap();
        }
        let clean_pcm = render(&clean, 48_000, 256, 64, 200);

        // Each nonlinear circuit with drive differs from clean and stays finite/bounded.
        for circuit in 1..=4 {
            let mut driven = clean.clone();
            driven = OPERATOR_METADATA
                .patch_parameter(
                    &driven,
                    ParameterId::from_static(20_004),
                    ParameterValue::Choice(circuit),
                )
                .unwrap();
            driven = OPERATOR_METADATA
                .patch_parameter(
                    &driven,
                    parameter::FILTER_DRIVE,
                    ParameterValue::Float(18.0),
                )
                .unwrap();
            let pcm = render(&driven, 48_000, 256, 64, 200);
            assert_ne!(pcm, clean_pcm, "circuit {circuit} must alter output");
            assert!(
                pcm.iter()
                    .flatten()
                    .all(|s| s.is_finite() && s.abs() <= 2.0)
            );
        }

        // Each shaper type with drive alters output; dry/wet=0 matches clean.
        for shaper in 1..=4 {
            let mut shaped = clean.clone();
            shaped = OPERATOR_METADATA
                .patch_parameter(
                    &shaped,
                    parameter::SHAPER_TYPE,
                    ParameterValue::Choice(shaper),
                )
                .unwrap();
            shaped = OPERATOR_METADATA
                .patch_parameter(
                    &shaped,
                    parameter::SHAPER_DRIVE,
                    ParameterValue::Float(75.0),
                )
                .unwrap();
            let pcm = render(&shaped, 48_000, 256, 64, 200);
            assert_ne!(pcm, clean_pcm, "shaper {shaper} must alter output");
            assert!(
                pcm.iter()
                    .flatten()
                    .all(|s| s.is_finite() && s.abs() <= 2.0)
            );

            let dry = OPERATOR_METADATA
                .patch_parameter(
                    &shaped,
                    parameter::SHAPER_DRY_WET,
                    ParameterValue::Float(0.0),
                )
                .unwrap();
            assert_eq!(render(&dry, 48_000, 256, 64, 200), clean_pcm);
        }

        // Combined circuit + shaper is block-partition invariant across sample rates.
        let mut combined = clean;
        for (id, val) in [
            (ParameterId::from_static(20_004), ParameterValue::Choice(2)),
            (parameter::FILTER_DRIVE, ParameterValue::Float(15.0)),
            (parameter::SHAPER_TYPE, ParameterValue::Choice(3)),
            (parameter::SHAPER_DRIVE, ParameterValue::Float(50.0)),
            (parameter::SHAPER_DRY_WET, ParameterValue::Float(65.0)),
        ] {
            combined = OPERATOR_METADATA
                .patch_parameter(&combined, id, val)
                .unwrap();
        }
        for rate in [8_000, 48_000, 192_000] {
            let whole = render(&combined, rate, 256, 256, 128);
            let split = render(&combined, rate, 256, 19, 128);
            assert_eq!(whole, split, "block-partition mismatch at {rate} Hz");
            assert!(whole.iter().flatten().all(|s| s.is_finite()));
        }
    }
}
