//! Original nonlinear stereo saturation for the native Lattice device chain.

use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const SATURATOR_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.saturator");
pub const SATURATOR_STATE_VERSION: u32 = 1;
const STATE_BYTES: usize = 13;

const CURVE_LABELS: &[&str] = &["soft clip", "digital clip"];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const DRIVE: ParameterId = ParameterId::from_static(1);
    pub const OUTPUT: ParameterId = ParameterId::from_static(2);
    pub const DRY_WET: ParameterId = ParameterId::from_static(3);
    pub const CURVE: ParameterId = ParameterId::from_static(4);
}

const DESCRIPTORS: &[ParameterDescriptor] = &[
    ParameterDescriptor {
        id: parameter::DRIVE,
        label: "drive",
        kind: ParameterKind::Float {
            min: -36.0,
            max: 36.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::OUTPUT,
        label: "output",
        kind: ParameterKind::Float {
            min: -36.0,
            max: 6.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::DRY_WET,
        label: "dry/wet",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::CURVE,
        label: "curve",
        kind: ParameterKind::Choice {
            labels: CURVE_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
];

#[derive(Clone, Copy, Debug, PartialEq)]
struct SaturatorSettings {
    drive_db: f32,
    output_db: f32,
    dry_wet: f32,
    curve: SaturationCurve,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaturationCurve {
    SoftClip,
    DigitalClip,
}

impl Default for SaturatorSettings {
    fn default() -> Self {
        Self {
            drive_db: 0.0,
            output_db: 0.0,
            dry_wet: 1.0,
            curve: SaturationCurve::SoftClip,
        }
    }
}

fn decode(state: &NativeDeviceState) -> Result<SaturatorSettings, DeviceContractError> {
    if state.type_id() != SATURATOR_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != SATURATOR_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let payload = state.payload();
    if payload.len() != STATE_BYTES || payload[12] > 1 {
        return Err(DeviceContractError::MalformedState);
    }
    let read = |offset| {
        f32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ])
    };
    let settings = SaturatorSettings {
        drive_db: read(0),
        output_db: read(4),
        dry_wet: read(8),
        curve: match payload[12] {
            0 => SaturationCurve::SoftClip,
            1 => SaturationCurve::DigitalClip,
            _ => return Err(DeviceContractError::MalformedState),
        },
    };
    validate_settings(settings)?;
    Ok(settings)
}

fn validate_settings(settings: SaturatorSettings) -> Result<(), DeviceContractError> {
    for (id, value) in [
        (parameter::DRIVE, settings.drive_db),
        (parameter::OUTPUT, settings.output_db),
        (parameter::DRY_WET, settings.dry_wet),
    ] {
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(ParameterValue::Float(value))?;
    }
    Ok(())
}

fn encode(settings: SaturatorSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate_settings(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    payload.extend_from_slice(&settings.drive_db.to_le_bytes());
    payload.extend_from_slice(&settings.output_db.to_le_bytes());
    payload.extend_from_slice(&settings.dry_wet.to_le_bytes());
    payload.push(match settings.curve {
        SaturationCurve::SoftClip => 0,
        SaturationCurve::DigitalClip => 1,
    });
    NativeDeviceState::new(SATURATOR_TYPE_ID, SATURATOR_STATE_VERSION, payload)
}

/// Built-in Saturator metadata, portable state codec and control-side processor.
pub struct SaturatorDefinition;

pub static SATURATOR_DEFINITION: SaturatorDefinition = SaturatorDefinition;

impl NativeDeviceMetadata for SaturatorDefinition {
    fn display_name(&self) -> Option<&'static str> {
        Some("saturator")
    }

    fn type_id(&self) -> DeviceTypeId {
        SATURATOR_TYPE_ID
    }

    fn state_version(&self) -> u32 {
        SATURATOR_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(SaturatorSettings::default())
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        encode(decode(state)?)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = decode(state)?;
        match id {
            parameter::DRIVE => Ok(ParameterValue::Float(settings.drive_db)),
            parameter::OUTPUT => Ok(ParameterValue::Float(settings.output_db)),
            parameter::DRY_WET => Ok(ParameterValue::Float(settings.dry_wet)),
            parameter::CURVE => Ok(ParameterValue::Choice(match settings.curve {
                SaturationCurve::SoftClip => 0,
                SaturationCurve::DigitalClip => 1,
            })),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = decode(state)?;
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(value)?;
        match (id, value) {
            (parameter::DRIVE, ParameterValue::Float(value)) => settings.drive_db = value,
            (parameter::OUTPUT, ParameterValue::Float(value)) => settings.output_db = value,
            (parameter::DRY_WET, ParameterValue::Float(value)) => settings.dry_wet = value,
            (parameter::CURVE, ParameterValue::Choice(0)) => {
                settings.curve = SaturationCurve::SoftClip
            }
            (parameter::CURVE, ParameterValue::Choice(1)) => {
                settings.curve = SaturationCurve::DigitalClip
            }
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
        encode(settings)
    }
}

impl NativeDeviceDefinition for SaturatorDefinition {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let _ = decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }

    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let _ = decode(state)?;
        Ok(StereoMatrix::identity())
    }

    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Some(Box::new(PreparedSaturator {
            settings: decode(state)?,
            sample_rate,
            drive_gain: gain_from_db(state, parameter::DRIVE)?,
            output_gain: gain_from_db(state, parameter::OUTPUT)?,
        })))
    }
}

fn gain_from_db(state: &NativeDeviceState, id: ParameterId) -> Result<f64, DeviceContractError> {
    let ParameterValue::Float(db) = SATURATOR_DEFINITION.read_parameter(state, id)? else {
        return Err(DeviceContractError::InvalidParameterValue);
    };
    amplitude_from_db(db)
}

fn amplitude_from_db(db: f32) -> Result<f64, DeviceContractError> {
    hex_dsp::Decibels::new(f64::from(db))
        .and_then(hex_dsp::Decibels::linear_amplitude)
        .map(hex_dsp::LinearAmplitude::value)
        .map_err(|_| DeviceContractError::InvalidParameterValue)
}

struct PreparedSaturator {
    settings: SaturatorSettings,
    sample_rate: u32,
    drive_gain: f64,
    output_gain: f64,
}

impl PreparedNativeProcessor for PreparedSaturator {
    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<SaturatorRuntime>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(SaturatorRuntime {
            settings: self.settings,
            sample_rate: self.sample_rate,
            drive_gain: self.drive_gain,
            output_gain: self.output_gain,
        })
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
}

struct SaturatorRuntime {
    settings: SaturatorSettings,
    sample_rate: u32,
    drive_gain: f64,
    output_gain: f64,
}

impl NativeProcessorRuntime for SaturatorRuntime {
    fn process_block(&mut self, frames: &mut [StereoFrame]) {
        let settings = self.settings;
        let drive = self.drive_gain;
        let output = self.output_gain;
        let wet = f64::from(settings.dry_wet);
        for frame in frames {
            if wet == 0.0 {
                continue;
            }
            frame[0] = process_sample(f64::from(frame[0]), settings, drive, output, wet) as f32;
            frame[1] = process_sample(f64::from(frame[1]), settings, drive, output, wet) as f32;
        }
    }

    fn process_bypassed_block(&mut self, _frames: &mut [StereoFrame]) {}

    fn reset(&mut self) {}

    fn is_quiescent(&self) -> bool {
        true
    }

    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.sample_rate == previous.sample_rate)
    }

    fn copy_history_from(&mut self, _previous: &dyn NativeProcessorRuntime) {}

    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.sample_rate == previous.sample_rate)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn soft_clip(input: f64) -> f64 {
    let magnitude = input.abs();
    if magnitude <= 0.5 {
        input
    } else {
        input.signum() * (1.0 - 0.25 / magnitude)
    }
}

#[inline]
fn process_sample(
    sample: f64,
    settings: SaturatorSettings,
    drive: f64,
    output: f64,
    wet: f64,
) -> f64 {
    if !sample.is_finite() {
        return sample;
    }
    let driven = sample * drive;
    let clipped = match settings.curve {
        SaturationCurve::SoftClip => soft_clip(driven),
        SaturationCurve::DigitalClip => driven.clamp(-1.0, 1.0),
    };
    let processed = clipped * output;
    if wet == 1.0 {
        processed
    } else {
        sample * (1.0 - wet) + processed * wet
    }
}

/// Bounded control-side view of the actual sample-processing law.
/// Coordinates are normalized; the horizontal input range is ±1 and the
/// vertical output range is ±`output_peak`. No audio history is inspected.
#[derive(Clone, Debug, PartialEq)]
pub struct SaturatorTransferCurve {
    pub points: [[f32; 2]; 129],
    pub output_peak: f32,
}

/// Sample the current drive, curve, output and mix through the same law as DSP.
/// The output axis expands for positive output gain rather than hiding peaks.
pub fn transfer_curve(
    state: &NativeDeviceState,
) -> Result<SaturatorTransferCurve, DeviceContractError> {
    let settings = decode(state)?;
    let drive = amplitude_from_db(settings.drive_db)?;
    let output = amplitude_from_db(settings.output_db)?;
    let peak = output.max(1.0);
    let wet = f64::from(settings.dry_wet);
    let points = std::array::from_fn(|index| {
        let x = index as f64 / 128.0;
        let input = x * 2.0 - 1.0;
        let value = process_sample(input, settings, drive, output, wet);
        [x as f32, ((value / peak + 1.0) * 0.5) as f32]
    });
    Ok(SaturatorTransferCurve {
        points,
        output_peak: peak as f32,
    })
}

#[cfg(test)]
mod tests;
