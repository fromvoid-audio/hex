//! Original nonlinear stereo saturation for the native Lattice device chain.
//!
//! Supports all eight Ableton Live 12–equivalent shaping modes (`analog clip`,
//! `digital clip`, `soft sine`, `medium curve`, `hard curve`, `sinoid fold`,
//! `bass shaper`, and the 6-parameter `waveshaper`), pre/post complementary
//! `base` low-shelf and `color` peaking EQ stages, a post-shaper `dc` blocker,
//! and a post-output 0 dB `soft clip` ceiling.
//!
//! Drive and curve shaping run inside the shared 8x oversampled ADAA stage
//! (`crate::oversampling`, Pulse #4146) so monotonic curves stay below -60 dB of
//! aliasing at +36 dB drive. The device declares the stage's 11 frames of
//! latency; the dry path is delayed by the same 11 frames, so the zero-wet
//! endpoint is the exact dry signal, delayed. Bypass advances every history and
//! passes the input through undelayed (the host counts bypass as zero latency).
//! Non-finite input samples pass through to the host's audio fault boundary
//! without entering the histories.
//!
//! Saved state format:
//! - v1 (`13` bytes): `drive_db`, `output_db`, `dry_wet` (`f32` LE) + `curve`
//!   (`0 = analog clip`, `1 = digital clip`). Migrates losslessly in memory to
//!   v2 with transparent defaults (`base = 0 dB`, `color = false`,
//!   `soft_clip_out = false`, `dc_filter = false`), rendering bit-identically
//!   to v1.
//! - v2 (`56` bytes): 13 `f32` LE fields (`drive_db`, `output_db`, `dry_wet`,
//!   `base_db`, `color_freq_hz`, `color_width`, `color_depth_db`, `ws_drive`,
//!   `ws_curve`, `ws_depth`, `ws_lin`, `ws_damp`, `ws_period`) followed by 4
//!   tag bytes (`curve: 0..=7`, `color: 0..=1`, `soft_clip_out: 0..=1`,
//!   `dc_filter: 0..=1`).

use crate::oversampling::{BLOCK_CHUNK, Kernel, LATENCY_FRAMES, Oversampler, Shaper};
use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;
use std::f64::consts::{FRAC_1_SQRT_2, FRAC_2_PI, FRAC_PI_2, LN_2, PI};
mod automation;

pub const SATURATOR_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.saturator");
pub const SATURATOR_STATE_VERSION: u32 = 2;
const STATE_BYTES_V1: usize = 13;
const STATE_BYTES: usize = 56;
const PARAM_COUNT: usize = 17;

const BASE_SHELF_HZ: f64 = 150.0;
const DC_BLOCKER_HZ: f64 = 10.0;
const BASS_SHAPER_BIAS: f64 = 0.14;
const WS_DAMP_SLOPE: f64 = 4.0;
const WS_TAG_A: u8 = 8;
const WS_TAG_B: u8 = 9;

/// Automation mask bits for parameters that can activate IIR/DC filter tails.
const FILTER_AUTOMATION_MASK: u32 = (1u32 << (parameter::BASE.value() - 1))
    | (1u32 << (parameter::COLOR.value() - 1))
    | (1u32 << (parameter::COLOR_DEPTH.value() - 1))
    | (1u32 << (parameter::DC_FILTER.value() - 1));

const CURVE_LABELS: &[&str] = &[
    "analog clip",
    "digital clip",
    "soft sine",
    "medium curve",
    "hard curve",
    "sinoid fold",
    "bass shaper",
    "waveshaper",
];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const DRIVE: ParameterId = ParameterId::from_static(1);
    pub const OUTPUT: ParameterId = ParameterId::from_static(2);
    pub const DRY_WET: ParameterId = ParameterId::from_static(3);
    pub const CURVE: ParameterId = ParameterId::from_static(4);
    pub const BASE: ParameterId = ParameterId::from_static(5);
    pub const COLOR: ParameterId = ParameterId::from_static(6);
    pub const COLOR_FREQ: ParameterId = ParameterId::from_static(7);
    pub const COLOR_WIDTH: ParameterId = ParameterId::from_static(8);
    pub const COLOR_DEPTH: ParameterId = ParameterId::from_static(9);
    pub const SOFT_CLIP_OUT: ParameterId = ParameterId::from_static(10);
    pub const DC_FILTER: ParameterId = ParameterId::from_static(11);
    pub const WS_DRIVE: ParameterId = ParameterId::from_static(12);
    pub const WS_CURVE: ParameterId = ParameterId::from_static(13);
    pub const WS_DEPTH: ParameterId = ParameterId::from_static(14);
    pub const WS_LIN: ParameterId = ParameterId::from_static(15);
    pub const WS_DAMP: ParameterId = ParameterId::from_static(16);
    pub const WS_PERIOD: ParameterId = ParameterId::from_static(17);
}

const LOG_SCALAR_ENVELOPE: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
        ..crate::parameter_binding::SCALAR_ENVELOPE
    };

const BINDINGS: [crate::parameter_binding::Binding<SaturatorSettings>; PARAM_COUNT] = [
    crate::parameter_binding::Binding::float(
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
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.drive_db,
        |c, v| c.drive_db = v,
    ),
    crate::parameter_binding::Binding::float(
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
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.output_db,
        |c, v| c.output_db = v,
    ),
    crate::parameter_binding::Binding::float(
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
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.dry_wet,
        |c, v| c.dry_wet = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::CURVE,
            label: "curve",
            kind: ParameterKind::Choice {
                labels: CURVE_LABELS,
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::CHOICE_ENVELOPE,
        |c| c.curve.to_index(),
        |c, v| {
            c.curve =
                SaturationCurve::from_index(v).ok_or(DeviceContractError::InvalidParameterValue)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::BASE,
            label: "base",
            kind: ParameterKind::Float {
                min: -36.0,
                max: 36.0,
                unit: Some("db"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.base_db,
        |c, v| c.base_db = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::COLOR,
            label: "color",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.color,
        |c, v| c.color = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::COLOR_FREQ,
            label: "freq",
            kind: ParameterKind::Float {
                min: 30.0,
                max: 15_000.0,
                unit: Some("hz"),
            },
            default: ParameterValue::Float(1_000.0),
            presentation: ParameterPresentation::Logarithmic,
        },
        LOG_SCALAR_ENVELOPE,
        |c| c.color_freq_hz,
        |c, v| c.color_freq_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::COLOR_WIDTH,
            label: "width",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.5),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.color_width,
        |c, v| c.color_width = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::COLOR_DEPTH,
            label: "depth",
            kind: ParameterKind::Float {
                min: -24.0,
                max: 24.0,
                unit: Some("db"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.color_depth_db,
        |c, v| c.color_depth_db = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::SOFT_CLIP_OUT,
            label: "soft clip",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.soft_clip_out,
        |c, v| c.soft_clip_out = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::DC_FILTER,
            label: "dc",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::BOOLEAN_ENVELOPE,
        |c| c.dc_filter,
        |c, v| c.dc_filter = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::WS_DRIVE,
            label: "ws drive",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.5),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.ws_drive,
        |c, v| c.ws_drive = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::WS_CURVE,
            label: "ws curve",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.ws_curve,
        |c, v| c.ws_curve = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::WS_DEPTH,
            label: "ws depth",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.ws_depth,
        |c, v| c.ws_depth = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::WS_LIN,
            label: "ws lin",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.5),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.ws_lin,
        |c, v| c.ws_lin = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::WS_DAMP,
            label: "ws damp",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.ws_damp,
        |c, v| c.ws_damp = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::WS_PERIOD,
            label: "ws period",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        crate::parameter_binding::SCALAR_ENVELOPE,
        |c| c.ws_period,
        |c, v| c.ws_period = v,
    ),
];
const DESCRIPTORS: &[ParameterDescriptor] =
    &crate::parameter_binding::descriptors(&BINDINGS, BINDINGS[0].descriptor);

#[derive(Clone, Copy, Debug, PartialEq)]
struct SaturatorSettings {
    drive_db: f32,
    output_db: f32,
    dry_wet: f32,
    curve: SaturationCurve,
    base_db: f32,
    color: bool,
    color_freq_hz: f32,
    color_width: f32,
    color_depth_db: f32,
    soft_clip_out: bool,
    dc_filter: bool,
    ws_drive: f32,
    ws_curve: f32,
    ws_depth: f32,
    ws_lin: f32,
    ws_damp: f32,
    ws_period: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaturationCurve {
    AnalogClip,
    DigitalClip,
    SoftSine,
    MediumCurve,
    HardCurve,
    SinoidFold,
    BassShaper,
    Waveshaper,
}

impl SaturationCurve {
    fn to_index(self) -> u32 {
        match self {
            Self::AnalogClip => 0,
            Self::DigitalClip => 1,
            Self::SoftSine => 2,
            Self::MediumCurve => 3,
            Self::HardCurve => 4,
            Self::SinoidFold => 5,
            Self::BassShaper => 6,
            Self::Waveshaper => 7,
        }
    }

    fn from_index(index: u32) -> Option<Self> {
        match index {
            0 => Some(Self::AnalogClip),
            1 => Some(Self::DigitalClip),
            2 => Some(Self::SoftSine),
            3 => Some(Self::MediumCurve),
            4 => Some(Self::HardCurve),
            5 => Some(Self::SinoidFold),
            6 => Some(Self::BassShaper),
            7 => Some(Self::Waveshaper),
            _ => None,
        }
    }
}

impl Default for SaturatorSettings {
    fn default() -> Self {
        Self {
            drive_db: 0.0,
            output_db: 0.0,
            dry_wet: 1.0,
            curve: SaturationCurve::AnalogClip,
            base_db: 0.0,
            color: false,
            color_freq_hz: 1_000.0,
            color_width: 0.5,
            color_depth_db: 0.0,
            soft_clip_out: false,
            dc_filter: false,
            ws_drive: 0.5,
            ws_curve: 0.0,
            ws_depth: 0.0,
            ws_lin: 0.5,
            ws_damp: 0.0,
            ws_period: 0.0,
        }
    }
}

impl SaturatorSettings {
    fn base_active(self) -> bool {
        self.base_db != 0.0
    }

    fn color_active(self) -> bool {
        self.color && self.color_depth_db != 0.0
    }

    fn has_active_filters(self) -> bool {
        self.base_active() || self.color_active()
    }

    fn waveshaper_differs(self, other: Self) -> bool {
        self.ws_drive != other.ws_drive
            || self.ws_curve != other.ws_curve
            || self.ws_depth != other.ws_depth
            || self.ws_lin != other.ws_lin
            || self.ws_damp != other.ws_damp
            || self.ws_period != other.ws_period
    }
}

fn read_bool_tag(byte: u8) -> Result<bool, DeviceContractError> {
    match byte {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DeviceContractError::MalformedState),
    }
}

fn decode(state: &NativeDeviceState) -> Result<SaturatorSettings, DeviceContractError> {
    if state.type_id() != SATURATOR_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    let payload = state.payload();
    let read_f32 = |offset: usize| -> Result<f32, DeviceContractError> {
        let bytes: [u8; 4] = payload
            .get(offset..offset + 4)
            .ok_or(DeviceContractError::MalformedState)?
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(f32::from_le_bytes(bytes))
    };
    let settings = match state.state_version() {
        1 => {
            if payload.len() != STATE_BYTES_V1 || payload[12] > 1 {
                return Err(DeviceContractError::MalformedState);
            }
            let curve = SaturationCurve::from_index(u32::from(payload[12]))
                .ok_or(DeviceContractError::MalformedState)?;
            SaturatorSettings {
                drive_db: read_f32(0)?,
                output_db: read_f32(4)?,
                dry_wet: read_f32(8)?,
                curve,
                ..SaturatorSettings::default()
            }
        }
        SATURATOR_STATE_VERSION => {
            if payload.len() != STATE_BYTES {
                return Err(DeviceContractError::MalformedState);
            }
            let curve = SaturationCurve::from_index(u32::from(payload[52]))
                .ok_or(DeviceContractError::MalformedState)?;
            SaturatorSettings {
                drive_db: read_f32(0)?,
                output_db: read_f32(4)?,
                dry_wet: read_f32(8)?,
                base_db: read_f32(12)?,
                color_freq_hz: read_f32(16)?,
                color_width: read_f32(20)?,
                color_depth_db: read_f32(24)?,
                ws_drive: read_f32(28)?,
                ws_curve: read_f32(32)?,
                ws_depth: read_f32(36)?,
                ws_lin: read_f32(40)?,
                ws_damp: read_f32(44)?,
                ws_period: read_f32(48)?,
                curve,
                color: read_bool_tag(payload[53])?,
                soft_clip_out: read_bool_tag(payload[54])?,
                dc_filter: read_bool_tag(payload[55])?,
            }
        }
        _ => return Err(DeviceContractError::UnsupportedStateVersion),
    };
    validate_settings(settings)?;
    Ok(settings)
}

fn validate_settings(settings: SaturatorSettings) -> Result<(), DeviceContractError> {
    for binding in &BINDINGS {
        binding.descriptor.validate(binding.read(&settings))?;
    }
    Ok(())
}

fn encode(settings: SaturatorSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate_settings(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    for value in [
        settings.drive_db,
        settings.output_db,
        settings.dry_wet,
        settings.base_db,
        settings.color_freq_hz,
        settings.color_width,
        settings.color_depth_db,
        settings.ws_drive,
        settings.ws_curve,
        settings.ws_depth,
        settings.ws_lin,
        settings.ws_damp,
        settings.ws_period,
    ] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    payload.push(settings.curve.to_index() as u8);
    payload.push(u8::from(settings.color));
    payload.push(u8::from(settings.soft_clip_out));
    payload.push(u8::from(settings.dc_filter));
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
        Ok(crate::parameter_binding::binding(&BINDINGS, id)?.read(&decode(state)?))
    }

    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        crate::parameter_binding::parameter_semantics_with_sample_rate_modulation(
            &BINDINGS,
            state,
            id,
            decode,
            runtime_parameter_automation_supported,
        )
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        crate::parameter_binding::patch_parameter(&BINDINGS, state, id, value, decode, encode)
    }
}

// The current concrete direct-control path implements only these fixed
// IDs, with value-only/no-smoothing updates. Other families may explicitly
// implement structural/smoothed delivery without changing Binding metadata.
fn runtime_parameter_automation_supported(parameter: ParameterId) -> bool {
    let implemented = parameter
        .value()
        .checked_sub(1)
        .is_some_and(|index| (index as usize) < PARAM_COUNT);
    let Ok(binding) = crate::parameter_binding::binding(&BINDINGS, parameter) else {
        return false;
    };
    implemented
        && binding.declares_arrangement_automation()
        && binding.semantics.normalization
            != hex_lattice_contract::ParameterNormalization::Unspecified
        && binding.semantics.stepping != hex_lattice_contract::ParameterStepping::Unspecified
        && binding.semantics.change_impact == hex_lattice_contract::ParameterChangeImpact::ValueOnly
        && binding.semantics.smoothing == hex_lattice_contract::ParameterSmoothing::None
}

impl NativeDeviceDefinition for SaturatorDefinition {
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode(state)?;
        let Ok(binding) = crate::parameter_binding::binding(&BINDINGS, parameter) else {
            return Ok(false);
        };
        binding.semantics.validate(binding.descriptor)?;
        Ok(runtime_parameter_automation_supported(parameter))
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: hex_lattice::ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
        Ok(crate::inline_owned::prepared_owned_runtime(
            Self::prepare_owned(state, context.sample_rate, parameters)?,
        ))
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<hex_lattice::PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(crate::inline_owned::owned_factory(
            Self::prepare_owned(state, context.sample_rate, &[])?,
        )))
    }

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
        Ok(Some(Box::new(Self::prepare_owned(
            state,
            context.sample_rate,
            &[],
        )?)))
    }
}

fn amplitude_from_db(db: f32) -> Result<f64, DeviceContractError> {
    hex_dsp::Decibels::new(f64::from(db))
        .and_then(hex_dsp::Decibels::linear_amplitude)
        .map(hex_dsp::LinearAmplitude::value)
        .map_err(|_| DeviceContractError::InvalidParameterValue)
}

fn base_filter_pair(
    sample_rate: u32,
    base_db: f32,
) -> Result<(hex_dsp::BiquadCoefficients, hex_dsp::BiquadCoefficients), DeviceContractError> {
    if base_db == 0.0 {
        return Ok((
            hex_dsp::BiquadCoefficients::IDENTITY,
            hex_dsp::BiquadCoefficients::IDENTITY,
        ));
    }
    let sr = f64::from(sample_rate);
    let freq = BASE_SHELF_HZ.min(sr * 0.45);
    let pre = hex_dsp::BiquadCoefficients::low_shelf_q(freq, sr, FRAC_1_SQRT_2, f64::from(base_db))
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
    let post =
        hex_dsp::BiquadCoefficients::low_shelf_q(freq, sr, FRAC_1_SQRT_2, -f64::from(base_db))
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
    Ok((pre, post))
}

fn color_filter_pair(
    sample_rate: u32,
    settings: SaturatorSettings,
) -> Result<(hex_dsp::BiquadCoefficients, hex_dsp::BiquadCoefficients), DeviceContractError> {
    if !settings.color_active() {
        return Ok((
            hex_dsp::BiquadCoefficients::IDENTITY,
            hex_dsp::BiquadCoefficients::IDENTITY,
        ));
    }
    let sr = f64::from(sample_rate);
    let freq = f64::from(settings.color_freq_hz).clamp(30.0, sr * 0.45);
    let q = 0.5 * 8.0_f64.powf(f64::from(settings.color_width));
    let depth = f64::from(settings.color_depth_db);
    let pre = hex_dsp::BiquadCoefficients::peaking(freq, sr, q, depth)
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
    let post = hex_dsp::BiquadCoefficients::peaking(freq, sr, q, -depth)
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
    Ok((pre, post))
}

fn dc_blocker_pole(sample_rate: u32) -> f64 {
    hex_dsp::frequency_pole(DC_BLOCKER_HZ, f64::from(sample_rate)).unwrap_or(0.998)
}

fn tail_limit_frames(sample_rate: u32, settings: SaturatorSettings, automation_mask: u32) -> u32 {
    if settings.has_active_filters()
        || settings.dc_filter
        || (automation_mask & FILTER_AUTOMATION_MASK) != 0
    {
        sample_rate.saturating_mul(2)
    } else {
        TAIL_FRAMES as u32
    }
}

/// Closed control-prepared Saturator factory. All fields are fixed inline;
/// canonical envelope backing stays in the host's existing program owner.
#[derive(Clone, Copy, Debug)]
pub struct PreparedSaturator {
    automation_mask: u32,
    settings: SaturatorSettings,
    sample_rate: u32,
    drive_gain: f64,
    output_gain: f64,
    pre_base: hex_dsp::BiquadCoefficients,
    post_base: hex_dsp::BiquadCoefficients,
    pre_color: hex_dsp::BiquadCoefficients,
    post_color: hex_dsp::BiquadCoefficients,
    dc_pole: f64,
    tail_limit: u32,
}

impl PreparedNativeProcessor for PreparedSaturator {
    fn supports_latency_aligned_bypass(&self) -> bool {
        true
    }

    fn supports_power_off(&self) -> bool {
        true
    }

    fn parameter_automation_descriptor(
        &self,
        parameter: ParameterId,
    ) -> Option<ParameterDescriptor> {
        self.supports(parameter)
            .then(|| DESCRIPTORS.iter().find(|d| d.id == parameter).copied())
            .flatten()
    }
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.supports(parameter)
    }

    fn parameter_modulation_semantics(
        &self,
        parameter: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        if !self.supports_parameter_automation(parameter) {
            return None;
        }
        let binding = crate::parameter_binding::binding(&BINDINGS, parameter).ok()?;
        Some(hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            ..binding.semantics
        })
    }

    fn runtime_version(&self) -> u32 {
        2
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<SaturatorRuntime>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(self))
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(LATENCY_FRAMES as u32)
    }

    fn output_tail(&self) -> OutputTail {
        // Oversampler histories, dry delay, and any active Base/Color/DC IIR
        // stages drain and flush within this exact bound.
        OutputTail::Finite {
            max_tail_frames: u64::from(self.tail_limit),
        }
    }
}

impl crate::inline_owned::InlinePreparedProcessor for PreparedSaturator {
    type Runtime = SaturatorRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        self.check_inline_ownership();
        let runtime = SaturatorRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(
                self.settings,
                std::array::from_fn(|index| self.automation_mask & (1u32 << index) != 0),
            ),
            settings: self.settings,
            sample_rate: self.sample_rate,
            gain_ramps: [
                hex_dsp::LinearRamp::settled(self.drive_gain),
                hex_dsp::LinearRamp::settled(self.output_gain),
            ],
            kernel: Kernel::shared(),
            channels: [Oversampler::default(); 2],
            delay: [[0.0; 2]; LATENCY_FRAMES],
            delay_index: 0,
            pre_base: [hex_dsp::Biquad::new(self.pre_base); 2],
            post_base: [hex_dsp::Biquad::new(self.post_base); 2],
            pre_color: [hex_dsp::Biquad::new(self.pre_color); 2],
            post_color: [hex_dsp::Biquad::new(self.post_color); 2],
            dc_pole: self.dc_pole,
            dc_in: [0.0; 2],
            dc_out: [0.0; 2],
            ws_tag: WS_TAG_A,
            silent_frames: self.tail_limit,
            tail_limit: self.tail_limit,
        };
        runtime.check_inline_ownership();
        runtime
    }
}

/// Zero input frames after which every history is exactly zero and output is
/// silent when no Base/Color/DC IIR stages are active: the oversampler drain,
/// which outlasts the 11-frame dry delay.
const TAIL_FRAMES: u64 = crate::oversampling::DRAIN_FRAMES as u64;
const _: () = assert!(crate::oversampling::DRAIN_FRAMES >= LATENCY_FRAMES);

/// Closed fixed-inline DSP owner, including the last effective controls used by
/// transactional history copying. It owns no program, asset or external handle.
///
/// A direct drive or output edit arrives as a new runtime. Each fallback gain
/// ramps from the previous effective value to its new target over 5 ms, even if
/// another control is automated. Eligibility is independent of value presence;
/// authored gains keep their per-frame law. Each ramp is independent of sample
/// rate and block partition.
/// Curve changes switch immediately: clip laws are not crossfaded.
#[derive(Clone, Copy, Debug)]
pub struct SaturatorRuntime {
    base: PreparedSaturator,
    controls: crate::parameter_binding::Controls<SaturatorSettings, PARAM_COUNT>,
    settings: SaturatorSettings,
    sample_rate: u32,
    gain_ramps: [hex_dsp::LinearRamp; 2],
    kernel: &'static Kernel,
    channels: [Oversampler; 2],
    /// Dry input, delayed by the oversampler latency.
    delay: [[f64; 2]; LATENCY_FRAMES],
    delay_index: usize,
    pre_base: [hex_dsp::Biquad; 2],
    post_base: [hex_dsp::Biquad; 2],
    pre_color: [hex_dsp::Biquad; 2],
    post_color: [hex_dsp::Biquad; 2],
    dc_pole: f64,
    dc_in: [f64; 2],
    dc_out: [f64; 2],
    ws_tag: u8,
    silent_frames: u32,
    tail_limit: u32,
}

/// Direct-edit gain ramp frames: 5 ms at every rate, like Compressor's dry/wet.
fn gain_ramp_frames(sample_rate: u32) -> u32 {
    hex_dsp::ramp_frames_for_ms(sample_rate, 5.0).unwrap_or(1)
}

impl SaturatorRuntime {
    #[inline]
    fn drive_gain(&self) -> f64 {
        self.gain_ramps[0].value()
    }

    #[inline]
    fn output_gain(&self) -> f64 {
        self.gain_ramps[1].value()
    }

    #[inline]
    fn gains_settled(&self) -> bool {
        self.gain_ramps[0].is_settled() && self.gain_ramps[1].is_settled()
    }

    #[inline]
    fn flip_ws_tag(&mut self) {
        self.ws_tag = if self.ws_tag == WS_TAG_A {
            WS_TAG_B
        } else {
            WS_TAG_A
        };
    }

    #[inline]
    fn shape_oversampled(
        stage: &mut Oversampler,
        value: f64,
        drive_gain: f64,
        kernel: &Kernel,
        settings: SaturatorSettings,
        ws_tag: u8,
    ) -> f64 {
        match settings.curve {
            SaturationCurve::AnalogClip => {
                stage.process_sample(value, drive_gain, kernel, &AnalogClip)
            }
            SaturationCurve::DigitalClip => {
                stage.process_sample(value, drive_gain, kernel, &DigitalClip)
            }
            SaturationCurve::SoftSine => stage.process_sample(value, drive_gain, kernel, &SoftSine),
            SaturationCurve::MediumCurve => {
                stage.process_sample(value, drive_gain, kernel, &MediumCurve)
            }
            SaturationCurve::HardCurve => {
                stage.process_sample(value, drive_gain, kernel, &HardCurve)
            }
            SaturationCurve::SinoidFold => {
                stage.process_sample(value, drive_gain, kernel, &SinoidFold)
            }
            SaturationCurve::BassShaper => {
                stage.process_sample(value, drive_gain, kernel, &BassShaper)
            }
            SaturationCurve::Waveshaper => {
                let shaper = Waveshaper::from_settings(settings, ws_tag);
                stage.process_sample(value, drive_gain, kernel, &shaper)
            }
        }
    }

    /// Renders one frame and advances every history. Bypassed frames keep
    /// their input; an active direct-edit ramp advances only when audible.
    fn render_frame(&mut self, frame: &mut StereoFrame, bypassed: bool) {
        if !bypassed {
            self.advance_gain_ramp();
        }
        let settings = self.settings;
        let wet = f64::from(settings.dry_wet);
        let input = frame.map(|sample| {
            if sample.is_finite() {
                f64::from(sample)
            } else {
                0.0
            }
        });
        let silent_input = input[0] == 0.0 && input[1] == 0.0;
        if silent_input {
            self.silent_frames = self.silent_frames.saturating_add(1).min(self.tail_limit);
        } else {
            self.silent_frames = 0;
        }
        let drive_gain = self.drive_gain();
        let output_gain = self.output_gain();
        let base_active = settings.base_active();
        let color_active = settings.color_active();
        let mut processed = [0.0; 2];
        for channel in 0..2 {
            let mut sample = input[channel];
            if base_active {
                sample = self.pre_base[channel].process_paired_flush(sample);
            }
            if color_active {
                sample = self.pre_color[channel].process_paired_flush(sample);
            }
            let mut shaped = Self::shape_oversampled(
                &mut self.channels[channel],
                sample,
                drive_gain,
                self.kernel,
                settings,
                self.ws_tag,
            );
            if color_active {
                shaped = self.post_color[channel].process_paired_flush(shaped);
            }
            if base_active {
                shaped = self.post_base[channel].process_paired_flush(shaped);
            }
            if settings.dc_filter {
                let out = hex_dsp::flush_quiet(
                    shaped - self.dc_in[channel] + self.dc_pole * self.dc_out[channel],
                );
                self.dc_in[channel] = hex_dsp::flush_quiet(shaped);
                self.dc_out[channel] = out;
                shaped = out;
            }
            let mut out = shaped * output_gain;
            if settings.soft_clip_out {
                out = soft_clip(out);
            }
            processed[channel] = out;
        }
        let dry = std::mem::replace(&mut self.delay[self.delay_index], input);
        self.delay_index = (self.delay_index + 1) % LATENCY_FRAMES;
        if silent_input && self.silent_frames >= self.tail_limit {
            self.clear_history();
        }
        if bypassed {
            return;
        }
        for channel in 0..2 {
            frame[channel] = if wet == 0.0 {
                dry[channel]
            } else if wet == 1.0 {
                processed[channel]
            } else {
                dry[channel] * (1.0 - wet) + processed[channel] * wet
            } as f32;
        }
    }

    fn clear_history(&mut self) {
        self.channels = [Oversampler::default(); 2];
        self.delay = [[0.0; 2]; LATENCY_FRAMES];
        self.delay_index = 0;
        for filter in &mut self.pre_base {
            filter.reset();
        }
        for filter in &mut self.post_base {
            filter.reset();
        }
        for filter in &mut self.pre_color {
            filter.reset();
        }
        for filter in &mut self.post_color {
            filter.reset();
        }
        self.dc_in = [0.0; 2];
        self.dc_out = [0.0; 2];
    }

    /// Advance each fallback gain independently; eligibility alone never
    /// suppresses a direct edit. Authored values retain their per-frame law.
    #[inline]
    fn advance_gain_ramp(&mut self) {
        if self.gains_settled() {
            return;
        }
        for (index, ramp) in self.gain_ramps.iter_mut().enumerate() {
            if ramp.is_settled() || self.controls.has_value(index) {
                continue;
            }
            ramp.advance_by_remaining();
        }
    }

    /// Bypass is already a discontinuity; resume from exact fallback targets.
    fn finish_gain_ramp(&mut self) {
        if self.gains_settled() {
            return;
        }
        for (index, (ramp, target)) in self
            .gain_ramps
            .iter_mut()
            .zip([self.base.drive_gain, self.base.output_gain])
            .enumerate()
        {
            if ramp.is_active() {
                if !self.controls.has_value(index) {
                    ramp.reset(target);
                } else {
                    ramp.reset(ramp.value());
                }
            }
        }
    }

    fn sync_filter_coefficients(&mut self) -> Result<(), DeviceContractError> {
        let (pre_base, post_base) = base_filter_pair(self.sample_rate, self.settings.base_db)?;
        let (pre_color, post_color) = color_filter_pair(self.sample_rate, self.settings)?;
        for channel in 0..2 {
            self.pre_base[channel].set_coefficients(pre_base);
            self.post_base[channel].set_coefficients(post_base);
            if !self.settings.base_active() {
                self.pre_base[channel].reset();
                self.post_base[channel].reset();
            }
            self.pre_color[channel].set_coefficients(pre_color);
            self.post_color[channel].set_coefficients(post_color);
            if !self.settings.color_active() {
                self.pre_color[channel].reset();
                self.post_color[channel].reset();
            }
        }
        if !self.settings.dc_filter {
            self.dc_in = [0.0; 2];
            self.dc_out = [0.0; 2];
        }
        Ok(())
    }
}

impl NativeProcessorRuntime for SaturatorRuntime {
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.base.supports(parameter)
    }
    fn process_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.apply_automation(values, &mut resolved, offset)?;
            self.render_frame(frame, false);
        }
        Ok(())
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        values: &dyn hex_lattice::NativeParameterAutomation,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        self.finish_gain_ramp();
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            self.apply_automation(values, &mut resolved, offset)?;
            self.finish_gain_ramp();
            self.render_frame(frame, true);
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        if self.gains_settled()
            && self.is_quiescent()
            && frames
                .iter()
                .all(|f| (f[0].to_bits() | f[1].to_bits()) == 0)
        {
            self.delay_index = (self.delay_index + frames.len()) % LATENCY_FRAMES;
            return;
        }
        if !self.gains_settled() || self.settings.has_active_filters() || self.settings.dc_filter {
            for frame in frames {
                self.render_frame(frame, false);
            }
            return;
        }
        let settings = self.settings;
        let wet = f64::from(settings.dry_wet);
        let dry_mix = 1.0 - wet;
        let drive = self.drive_gain();
        let output_gain = self.output_gain();
        let soft_clip_out = settings.soft_clip_out;
        let kernel = self.kernel;
        let ws_tag = self.ws_tag;
        for chunk in frames.chunks_mut(BLOCK_CHUNK) {
            let n = chunk.len();
            if !chunk.iter().all(|f| f[0].is_finite() && f[1].is_finite()) {
                for frame in chunk {
                    self.render_frame(frame, false);
                }
                continue;
            }
            let mut left = [0.0_f32; BLOCK_CHUNK];
            let mut right = [0.0_f32; BLOCK_CHUNK];
            let mut dry_l = [0.0_f64; BLOCK_CHUNK];
            let mut dry_r = [0.0_f64; BLOCK_CHUNK];
            for i in 0..n {
                let in_l = chunk[i][0];
                let in_r = chunk[i][1];
                if in_l == 0.0 && in_r == 0.0 {
                    self.silent_frames = self.silent_frames.saturating_add(1).min(self.tail_limit);
                } else {
                    self.silent_frames = 0;
                }
                left[i] = in_l;
                right[i] = in_r;
                let dry = std::mem::replace(
                    &mut self.delay[self.delay_index],
                    [f64::from(in_l), f64::from(in_r)],
                );
                self.delay_index = (self.delay_index + 1) % LATENCY_FRAMES;
                dry_l[i] = dry[0];
                dry_r[i] = dry[1];
            }
            let [left_stage, right_stage] = &mut self.channels;
            match settings.curve {
                SaturationCurve::AnalogClip => Oversampler::process_stereo_block(
                    left_stage,
                    right_stage,
                    &mut left[..n],
                    &mut right[..n],
                    drive,
                    kernel,
                    &AnalogClip,
                ),
                SaturationCurve::DigitalClip => Oversampler::process_stereo_block(
                    left_stage,
                    right_stage,
                    &mut left[..n],
                    &mut right[..n],
                    drive,
                    kernel,
                    &DigitalClip,
                ),
                SaturationCurve::SoftSine => Oversampler::process_stereo_block(
                    left_stage,
                    right_stage,
                    &mut left[..n],
                    &mut right[..n],
                    drive,
                    kernel,
                    &SoftSine,
                ),
                SaturationCurve::MediumCurve => Oversampler::process_stereo_block(
                    left_stage,
                    right_stage,
                    &mut left[..n],
                    &mut right[..n],
                    drive,
                    kernel,
                    &MediumCurve,
                ),
                SaturationCurve::HardCurve => Oversampler::process_stereo_block(
                    left_stage,
                    right_stage,
                    &mut left[..n],
                    &mut right[..n],
                    drive,
                    kernel,
                    &HardCurve,
                ),
                SaturationCurve::SinoidFold => Oversampler::process_stereo_block(
                    left_stage,
                    right_stage,
                    &mut left[..n],
                    &mut right[..n],
                    drive,
                    kernel,
                    &SinoidFold,
                ),
                SaturationCurve::BassShaper => Oversampler::process_stereo_block(
                    left_stage,
                    right_stage,
                    &mut left[..n],
                    &mut right[..n],
                    drive,
                    kernel,
                    &BassShaper,
                ),
                SaturationCurve::Waveshaper => {
                    let shaper = Waveshaper::from_settings(settings, ws_tag);
                    Oversampler::process_stereo_block(
                        left_stage,
                        right_stage,
                        &mut left[..n],
                        &mut right[..n],
                        drive,
                        kernel,
                        &shaper,
                    );
                }
            }
            if wet == 0.0 {
                for i in 0..n {
                    chunk[i][0] = dry_l[i] as f32;
                    chunk[i][1] = dry_r[i] as f32;
                }
            } else if wet == 1.0 {
                for i in 0..n {
                    let mut l = f64::from(left[i]) * output_gain;
                    let mut r = f64::from(right[i]) * output_gain;
                    if soft_clip_out {
                        l = soft_clip(l);
                        r = soft_clip(r);
                    }
                    chunk[i][0] = l as f32;
                    chunk[i][1] = r as f32;
                }
            } else {
                for i in 0..n {
                    let mut l = f64::from(left[i]) * output_gain;
                    let mut r = f64::from(right[i]) * output_gain;
                    if soft_clip_out {
                        l = soft_clip(l);
                        r = soft_clip(r);
                    }
                    chunk[i][0] = (dry_l[i] * dry_mix + l * wet) as f32;
                    chunk[i][1] = (dry_r[i] * dry_mix + r * wet) as f32;
                }
            }
        }
    }

    fn process_bypassed_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.finish_gain_ramp();
        for frame in frames {
            self.render_frame(frame, true);
        }
    }

    fn reset(&mut self) {
        self.controls.reset();
        self.settings = self.base.settings;
        self.gain_ramps = [
            hex_dsp::LinearRamp::settled(self.base.drive_gain),
            hex_dsp::LinearRamp::settled(self.base.output_gain),
        ];
        self.pre_base = [hex_dsp::Biquad::new(self.base.pre_base); 2];
        self.post_base = [hex_dsp::Biquad::new(self.base.post_base); 2];
        self.pre_color = [hex_dsp::Biquad::new(self.base.pre_color); 2];
        self.post_color = [hex_dsp::Biquad::new(self.base.post_color); 2];
        self.ws_tag = WS_TAG_A;
        self.silent_frames = self.tail_limit;
        self.clear_history();
    }

    fn is_quiescent(&self) -> bool {
        self.channels.iter().all(Oversampler::is_quiescent)
            && self.delay.iter().flatten().all(|v| *v == 0.0)
            && self.pre_base.iter().all(|b| b.is_quiet())
            && self.post_base.iter().all(|b| b.is_quiet())
            && self.pre_color.iter().all(|b| b.is_quiet())
            && self.post_color.iter().all(|b| b.is_quiet())
            && self.dc_in.iter().all(|v| *v == 0.0)
            && self.dc_out.iter().all(|v| *v == 0.0)
    }

    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.sample_rate == previous.sample_rate)
    }

    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|previous| self.sample_rate == previous.sample_rate)
        else {
            return;
        };
        self.channels = previous.channels;
        self.delay = previous.delay;
        self.delay_index = previous.delay_index;
        self.pre_base = previous.pre_base;
        self.post_base = previous.post_base;
        self.pre_color = previous.pre_color;
        self.post_color = previous.post_color;
        self.dc_in = previous.dc_in;
        self.dc_out = previous.dc_out;
        self.ws_tag = previous.ws_tag;
        self.silent_frames = previous.silent_frames.min(self.tail_limit);
        if self.base.automation_mask != 0 && previous.base.automation_mask != 0 {
            self.controls.inherit(&previous.controls);
            self.settings = previous.settings;
        }
        if self.settings.waveshaper_differs(previous.settings) {
            self.flip_ws_tag();
        }
        let _ = self.sync_filter_coefficients();
        let frames = gain_ramp_frames(self.sample_rate);
        for (index, (target, old_target, parameter)) in [
            (
                self.base.drive_gain,
                previous.base.drive_gain,
                parameter::DRIVE,
            ),
            (
                self.base.output_gain,
                previous.base.output_gain,
                parameter::OUTPUT,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let mut ramp = previous.gain_ramps[index];
            if ramp.value() == target {
                ramp.reset(target);
            } else if target == old_target
                && (self.base.supports(parameter) || !previous.controls.has_value(index))
            {
                // Transactional twins retain each in-flight trajectory.
            } else {
                ramp.retarget(target, frames);
            }
            self.gain_ramps[index] = ramp;
        }
    }

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

/// Curve 0 (`analog clip`, v1 `soft clip`): `x` inside `|x| <= 0.5`,
/// `sgn(x) * (1 - 0.25 / |x|)` outside, with exact C1 antiderivative.
struct AnalogClip;
impl Shaper for AnalogClip {
    fn tag(&self) -> u8 {
        1
    }
    fn value(&self, x: f64) -> f64 {
        soft_clip(x)
    }
    fn antiderivative(&self, x: f64) -> f64 {
        let magnitude = x.abs();
        if magnitude <= 0.5 {
            0.5 * x * x
        } else {
            magnitude - 0.25 * hex_dsp::fast_ln_f64(magnitude) - 0.375 - 0.25 * LN_2
        }
    }
}

/// Curve 1 (`digital clip`): hard clamp to `[-1, 1]` with exact C1 antiderivative.
struct DigitalClip;
impl Shaper for DigitalClip {
    fn tag(&self) -> u8 {
        2
    }
    fn value(&self, x: f64) -> f64 {
        x.clamp(-1.0, 1.0)
    }
    fn antiderivative(&self, x: f64) -> f64 {
        if x.abs() <= 1.0 {
            0.5 * x * x
        } else {
            x.abs() - 0.5
        }
    }
}

/// Curve 2 (`soft sine`): `sin(pi/2 * x)` inside `|x| <= 1`, clamped to `±1`
/// outside, with exact C1 antiderivative (`F(0) = 0`).
struct SoftSine;
impl Shaper for SoftSine {
    fn tag(&self) -> u8 {
        3
    }
    fn value(&self, x: f64) -> f64 {
        if x.abs() <= 1.0 {
            (FRAC_PI_2 * x).sin()
        } else {
            x.signum()
        }
    }
    fn antiderivative(&self, x: f64) -> f64 {
        let magnitude = x.abs();
        if magnitude <= 1.0 {
            FRAC_2_PI * (1.0 - (FRAC_PI_2 * x).cos())
        } else {
            magnitude - 1.0 + FRAC_2_PI
        }
    }
}

/// Curve 3 (`medium curve`): smooth hyperbolic tangent `tanh(x)` with exact C∞
/// antiderivative `ln(cosh(x))`.
struct MediumCurve;
impl Shaper for MediumCurve {
    fn tag(&self) -> u8 {
        4
    }
    fn value(&self, x: f64) -> f64 {
        x.tanh()
    }
    fn antiderivative(&self, x: f64) -> f64 {
        hex_dsp::fast_ln_cosh_f64(x)
    }
}

/// Curve 4 (`hard curve`): linear inside `|x| <= 0.75`, tight hyperbolic knee
/// `sgn(x) * (1 - 0.0625 / (|x| - 0.5))` outside, with exact C1 antiderivative.
struct HardCurve;
impl Shaper for HardCurve {
    fn tag(&self) -> u8 {
        5
    }
    fn value(&self, x: f64) -> f64 {
        let magnitude = x.abs();
        if magnitude <= 0.75 {
            x
        } else {
            x.signum() * (1.0 - 0.0625 / (magnitude - 0.5))
        }
    }
    fn antiderivative(&self, x: f64) -> f64 {
        let magnitude = x.abs();
        if magnitude <= 0.75 {
            0.5 * x * x
        } else {
            magnitude - 0.0625 * hex_dsp::fast_ln_f64(magnitude - 0.5) - 0.46875 - 0.125 * LN_2
        }
    }
}

/// Curve 5 (`sinoid fold`): continuous sinusoidal wavefolder `sin(pi/2 * x)`
/// with exact C∞ antiderivative `(2/pi) * (1 - cos(pi/2 * x))`.
struct SinoidFold;
impl Shaper for SinoidFold {
    fn tag(&self) -> u8 {
        6
    }
    fn value(&self, x: f64) -> f64 {
        (FRAC_PI_2 * x).sin()
    }
    fn antiderivative(&self, x: f64) -> f64 {
        FRAC_2_PI * (1.0 - (FRAC_PI_2 * x).cos())
    }
}

/// Curve 6 (`bass shaper`): asymmetric warm saturation generating rich 2nd and
/// 3rd harmonics with unit small-signal gain and `f(0) = F(0) = 0`.
struct BassShaper;
impl Shaper for BassShaper {
    fn tag(&self) -> u8 {
        7
    }
    fn value(&self, x: f64) -> f64 {
        let tb = BASS_SHAPER_BIAS.tanh();
        let inv_sech2 = 1.0 / (1.0 - tb * tb);
        ((x + BASS_SHAPER_BIAS).tanh() - tb) * inv_sech2
    }
    fn antiderivative(&self, x: f64) -> f64 {
        let tb = BASS_SHAPER_BIAS.tanh();
        let inv_sech2 = 1.0 / (1.0 - tb * tb);
        let f0 = hex_dsp::fast_ln_cosh_f64(BASS_SHAPER_BIAS);
        (hex_dsp::fast_ln_cosh_f64(x + BASS_SHAPER_BIAS) - f0 - x * tb) * inv_sech2
    }
}

/// Curve 7 (`waveshaper`): 6-parameter C∞ closed-form shaper controlled by
/// `ws_drive`, `ws_curve`, `ws_depth`, `ws_lin`, `ws_damp`, and `ws_period`,
/// with exact closed-form antiderivative (`f(0) = F(0) = 0`).
#[derive(Clone, Copy, Debug)]
struct Waveshaper {
    tag: u8,
    a: f64,
    k1: f64,
    b: f64,
    k2: f64,
    r: f64,
    omega: f64,
    d: f64,
}

impl Waveshaper {
    fn from_settings(settings: SaturatorSettings, tag: u8) -> Self {
        let drive = f64::from(settings.ws_drive);
        let curve = f64::from(settings.ws_curve);
        let depth = f64::from(settings.ws_depth);
        let lin = f64::from(settings.ws_lin);
        let damp = f64::from(settings.ws_damp);
        let period = f64::from(settings.ws_period);

        let k_base = 0.25 + 3.75 * drive;
        let k1 = (k_base * (1.6 - lin)).max(0.15);
        let alpha = 1.0 - 0.45 * curve;
        let k2 = k1 * (1.0 + 3.0 * curve);
        let beta = 0.65 * curve;
        let ripple = 0.65 * depth;
        let omega = PI * (1.0 + 5.0 * period);
        let damp_weight = (0.85 * alpha * k1 / WS_DAMP_SLOPE).min(0.55) * damp;
        let norm = 1.0 / (alpha + beta + 0.5 * ripple).max(1.0);
        Self {
            tag,
            a: alpha * norm,
            k1,
            b: beta * norm,
            k2,
            r: ripple * norm,
            omega,
            d: damp_weight * norm,
        }
    }
}

impl Shaper for Waveshaper {
    fn tag(&self) -> u8 {
        self.tag
    }
    fn value(&self, x: f64) -> f64 {
        let mut y = self.a * (self.k1 * x).tanh();
        if self.b != 0.0 {
            y += self.b * (self.k2 * x).tanh();
        }
        if self.r != 0.0 {
            y += self.r * (self.omega * x).sin();
        }
        if self.d != 0.0 {
            y -= self.d * (WS_DAMP_SLOPE * x).tanh();
        }
        y
    }
    fn antiderivative(&self, x: f64) -> f64 {
        let mut f = (self.a / self.k1) * hex_dsp::fast_ln_cosh_f64(self.k1 * x);
        if self.b != 0.0 {
            f += (self.b / self.k2) * hex_dsp::fast_ln_cosh_f64(self.k2 * x);
        }
        if self.r != 0.0 {
            f += (self.r / self.omega) * (1.0 - (self.omega * x).cos());
        }
        if self.d != 0.0 {
            f -= (self.d / WS_DAMP_SLOPE) * hex_dsp::fast_ln_cosh_f64(WS_DAMP_SLOPE * x);
        }
        f
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

fn shape_value(driven: f64, settings: SaturatorSettings) -> f64 {
    match settings.curve {
        SaturationCurve::AnalogClip => AnalogClip.value(driven),
        SaturationCurve::DigitalClip => DigitalClip.value(driven),
        SaturationCurve::SoftSine => SoftSine.value(driven),
        SaturationCurve::MediumCurve => MediumCurve.value(driven),
        SaturationCurve::HardCurve => HardCurve.value(driven),
        SaturationCurve::SinoidFold => SinoidFold.value(driven),
        SaturationCurve::BassShaper => BassShaper.value(driven),
        SaturationCurve::Waveshaper => Waveshaper::from_settings(settings, WS_TAG_A).value(driven),
    }
}

#[inline]
/// The static transfer: the value a held finite input settles to once the
/// oversampled stage and dry delay are full (`transfer_curve` previews it).
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
    let clipped = shape_value(driven, settings);
    let mut processed = clipped * output;
    if settings.soft_clip_out {
        processed = soft_clip(processed);
    }
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

/// Sample the current drive, curve, waveshaper, output, soft clip and mix
/// through the same law as DSP. The output axis expands for peaks above ±1
/// rather than clipping the preview.
pub fn transfer_curve(
    state: &NativeDeviceState,
) -> Result<SaturatorTransferCurve, DeviceContractError> {
    let settings = decode(state)?;
    let drive = amplitude_from_db(settings.drive_db)?;
    let output = amplitude_from_db(settings.output_db)?;
    let wet = f64::from(settings.dry_wet);
    let raw: [f64; 129] = std::array::from_fn(|index| {
        let x = index as f64 / 128.0;
        let input = x * 2.0 - 1.0;
        process_sample(input, settings, drive, output, wet)
    });
    let peak = if settings.soft_clip_out {
        1.0
    } else {
        let max_sample = raw.iter().copied().map(f64::abs).fold(0.0_f64, f64::max);
        output.max(max_sample).max(1.0)
    };
    let points = std::array::from_fn(|index| {
        let x = index as f64 / 128.0;
        [x as f32, ((raw[index] / peak + 1.0) * 0.5) as f32]
    });
    Ok(SaturatorTransferCurve {
        points,
        output_peak: peak as f32,
    })
}

#[cfg(test)]
mod tests;
