//! Stereo-linked feed-forward dynamics processor with Ableton Live 12 parity controls.
//!
//! Supports Compress and Expand modes, Peak/RMS detection, Linear and Logarithmic
//! envelopes, program-dependent Auto Release, Auto Makeup, 0/1/10 ms Lookahead
//! (with declared algorithmic latency), and stereo Sidechain Gain, Mix, and EQ
//! filtering (HP/BP/LP/Notch/Bell).
//!
//! Seventeen controls consume the shared borrowed final-value stream; Lookahead is
//! structural because it changes declared algorithmic latency.
mod automation;
mod sidechain_eq;
use sidechain_eq::{SidechainEq, SidechainEqMode};

use hex_lattice::{
    DynamicsMeter, NativeDeviceDefinition, NativeInstrumentClock, NativeParameterAutomation,
    NativeProcessorRuntime, OutputTail, PreparedNativeProcessor, StereoAuxiliaryBlock, StereoFrame,
    StereoMatrix,
};
use hex_lattice_contract::{
    AuxiliaryInput, DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const COMPRESSOR_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.compressor");
/// Current state version. Supported saved versions (Pulse #3956, #3278), each a
/// little-endian prefix of the next:
/// - v1 (11 Oct 2026, 24 bytes): no dry/wet; migrates fully wet (1.0), as v1 rendered.
/// - v2 (18 Oct 2026, 28 bytes): no detector; migrates to Peak, v2's only detector.
/// - v3 (18 Oct 2026, 32 bytes): adds the detector choice.
/// - v4 (26 Nov 2026, 72 bytes): adds mode, envelope, auto release, auto makeup,
///   lookahead, sidechain gain/mix, and sidechain EQ mode/freq/Q; v3 defaults
///   migrate losslessly to bit-exact v3 processing.
pub const COMPRESSOR_STATE_VERSION: u32 = 4;
const STATE_BYTES: usize = 72;
const PARAM_COUNT: usize = 18;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const MAX_LOOKAHEAD_FRAMES: usize = 1_920; // 10 ms at 192 kHz
const GAIN_SMOOTH_MS: f64 = 5.0;
const RMS_INTEGRATION_SECONDS: f64 = 0.010;
const AUTO_RELEASE_INTEGRATION_SECONDS: f64 = 0.250;
const SILENCE_FLOOR_DB: f64 = -120.0;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const THRESHOLD: ParameterId = ParameterId::from_static(1);
    pub const RATIO: ParameterId = ParameterId::from_static(2);
    pub const ATTACK: ParameterId = ParameterId::from_static(3);
    pub const RELEASE: ParameterId = ParameterId::from_static(4);
    pub const KNEE: ParameterId = ParameterId::from_static(5);
    pub const MAKEUP: ParameterId = ParameterId::from_static(6);
    pub const DRY_WET: ParameterId = ParameterId::from_static(7);
    pub const DETECTOR: ParameterId = ParameterId::from_static(8);
    pub const MODE: ParameterId = ParameterId::from_static(9);
    pub const ENVELOPE: ParameterId = ParameterId::from_static(10);
    pub const AUTO_RELEASE: ParameterId = ParameterId::from_static(11);
    pub const AUTO_MAKEUP: ParameterId = ParameterId::from_static(12);
    pub const LOOKAHEAD: ParameterId = ParameterId::from_static(13);
    pub const SIDECHAIN_GAIN: ParameterId = ParameterId::from_static(14);
    pub const SIDECHAIN_MIX: ParameterId = ParameterId::from_static(15);
    pub const SIDECHAIN_EQ_MODE: ParameterId = ParameterId::from_static(16);
    pub const SIDECHAIN_EQ_FREQ: ParameterId = ParameterId::from_static(17);
    pub const SIDECHAIN_EQ_Q: ParameterId = ParameterId::from_static(18);
}

const DESCRIPTORS: &[ParameterDescriptor] = &[
    ParameterDescriptor {
        id: parameter::THRESHOLD,
        label: "threshold",
        kind: ParameterKind::Float {
            min: -60.0,
            max: 0.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(-18.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RATIO,
        label: "ratio",
        kind: ParameterKind::Float {
            min: 1.0,
            max: 20.0,
            unit: None,
        },
        default: ParameterValue::Float(4.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::ATTACK,
        label: "attack",
        kind: ParameterKind::Float {
            min: 0.1,
            max: 100.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(10.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::RELEASE,
        label: "release",
        kind: ParameterKind::Float {
            min: 5.0,
            max: 1_000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(100.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::KNEE,
        label: "knee",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 12.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(6.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::MAKEUP,
        label: "makeup",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 24.0,
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
        id: parameter::DETECTOR,
        label: "detector",
        kind: ParameterKind::Choice {
            labels: &["peak", "rms"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::MODE,
        label: "mode",
        kind: ParameterKind::Choice {
            labels: &["compress", "expand"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::ENVELOPE,
        label: "envelope",
        kind: ParameterKind::Choice {
            labels: &["lin", "log"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::AUTO_RELEASE,
        label: "auto rel",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::AUTO_MAKEUP,
        label: "auto makeup",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::LOOKAHEAD,
        label: "lookahead",
        kind: ParameterKind::Choice {
            labels: &["0 ms", "1 ms", "10 ms"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SIDECHAIN_GAIN,
        label: "sc gain",
        kind: ParameterKind::Float {
            min: -24.0,
            max: 24.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SIDECHAIN_MIX,
        label: "sc mix",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SIDECHAIN_EQ_MODE,
        label: "sc eq",
        kind: ParameterKind::Choice {
            labels: &["off", "hp", "bp", "lp", "notch", "bell"],
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::SIDECHAIN_EQ_FREQ,
        label: "sc freq",
        kind: ParameterKind::Float {
            min: 30.0,
            max: 15_000.0,
            unit: Some("hz"),
        },
        default: ParameterValue::Float(100.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::SIDECHAIN_EQ_Q,
        label: "sc q",
        kind: ParameterKind::Float {
            min: 0.1,
            max: 10.0,
            unit: None,
        },
        default: ParameterValue::Float(0.707),
        presentation: ParameterPresentation::Logarithmic,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum Detector {
    Peak = 0,
    Rms = 1,
}
impl Detector {
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Peak),
            1 => Ok(Self::Rms),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum CompressorMode {
    Compress = 0,
    Expand = 1,
}
impl CompressorMode {
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Compress),
            1 => Ok(Self::Expand),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum EnvelopeMode {
    Linear = 0,
    Logarithmic = 1,
}
impl EnvelopeMode {
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Linear),
            1 => Ok(Self::Logarithmic),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum Lookahead {
    Off = 0,
    OneMs = 1,
    TenMs = 2,
}
impl Lookahead {
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Off),
            1 => Ok(Self::OneMs),
            2 => Ok(Self::TenMs),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
    fn frames(self, sample_rate: u32) -> u32 {
        let ms = match self {
            Self::Off => return 0,
            Self::OneMs => 1.0,
            Self::TenMs => 10.0,
        };
        let frames = (ms * f64::from(sample_rate) / 1_000.0).ceil() as u32;
        frames.min(MAX_LOOKAHEAD_FRAMES as u32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CompressorSettings {
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    knee_db: f32,
    makeup_db: f32,
    dry_wet: f32,
    detector: Detector,
    mode: CompressorMode,
    envelope: EnvelopeMode,
    auto_release: bool,
    auto_makeup: bool,
    lookahead: Lookahead,
    sidechain_gain_db: f32,
    sidechain_mix: f32,
    sidechain_eq_mode: SidechainEqMode,
    sidechain_eq_freq_hz: f32,
    sidechain_eq_q: f32,
}

const BINDINGS: [crate::parameter_binding::Binding<CompressorSettings>; PARAM_COUNT] = [
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[0],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.threshold_db,
        |c, v| c.threshold_db = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[1],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.ratio,
        |c, v| c.ratio = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[2],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.attack_ms,
        |c, v| c.attack_ms = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[3],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.release_ms,
        |c, v| c.release_ms = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[4],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.knee_db,
        |c, v| c.knee_db = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[5],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.makeup_db,
        |c, v| c.makeup_db = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[6],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.dry_wet,
        |c, v| c.dry_wet = v,
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[7],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |c| c.detector as u32,
        |c, v| {
            c.detector = Detector::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[8],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |c| c.mode as u32,
        |c, v| {
            c.mode = CompressorMode::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[9],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |c| c.envelope as u32,
        |c, v| {
            c.envelope = EnvelopeMode::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::boolean(
        DESCRIPTORS[10],
        hex_lattice_contract::ParameterSemantics {
            stepping: hex_lattice_contract::ParameterStepping::Boolean,
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |c| c.auto_release,
        |c, v| c.auto_release = v,
    ),
    crate::parameter_binding::Binding::boolean(
        DESCRIPTORS[11],
        hex_lattice_contract::ParameterSemantics {
            stepping: hex_lattice_contract::ParameterStepping::Boolean,
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |c| c.auto_makeup,
        |c, v| c.auto_makeup = v,
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[12],
        hex_lattice_contract::ParameterSemantics {
            automation: hex_lattice_contract::ParameterAutomation::Unsupported,
            modulation: hex_lattice_contract::ParameterModulation::Unsupported,
            change_impact: hex_lattice_contract::ParameterChangeImpact::Structural,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |c| c.lookahead as u32,
        |c, v| {
            c.lookahead = Lookahead::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[13],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.sidechain_gain_db,
        |c, v| c.sidechain_gain_db = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[14],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.sidechain_mix,
        |c, v| c.sidechain_mix = v,
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[15],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |c| c.sidechain_eq_mode as u32,
        |c, v| {
            c.sidechain_eq_mode = SidechainEqMode::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[16],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.sidechain_eq_freq_hz,
        |c, v| c.sidechain_eq_freq_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[17],
        hex_lattice_contract::ParameterSemantics {
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..crate::parameter_binding::SCALAR_ENVELOPE
        },
        |c| c.sidechain_eq_q,
        |c, v| c.sidechain_eq_q = v,
    ),
];

impl Default for CompressorSettings {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            knee_db: 6.0,
            makeup_db: 0.0,
            dry_wet: 1.0,
            detector: Detector::Peak,
            mode: CompressorMode::Compress,
            envelope: EnvelopeMode::Linear,
            auto_release: false,
            auto_makeup: false,
            lookahead: Lookahead::Off,
            sidechain_gain_db: 0.0,
            sidechain_mix: 1.0,
            sidechain_eq_mode: SidechainEqMode::Off,
            sidechain_eq_freq_hz: 100.0,
            sidechain_eq_q: 0.707,
        }
    }
}

fn decode_bool_u32(value: u32) -> Result<bool, DeviceContractError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DeviceContractError::InvalidParameterValue),
    }
}

fn decode(state: &NativeDeviceState) -> Result<CompressorSettings, DeviceContractError> {
    if state.type_id() != COMPRESSOR_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    let expected = match state.state_version() {
        1 => 24,
        2 => 28,
        3 => 32,
        COMPRESSOR_STATE_VERSION => STATE_BYTES,
        _ => return Err(DeviceContractError::UnsupportedStateVersion),
    };
    let bytes = state.payload();
    if bytes.len() != expected {
        return Err(DeviceContractError::MalformedState);
    }
    let read_f32 = |offset| {
        f32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    let read_u32 = |offset| {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    let defaults = CompressorSettings::default();
    let settings = CompressorSettings {
        threshold_db: read_f32(0),
        ratio: read_f32(4),
        attack_ms: read_f32(8),
        release_ms: read_f32(12),
        knee_db: read_f32(16),
        makeup_db: read_f32(20),
        dry_wet: if expected >= 28 { read_f32(24) } else { 1.0 },
        detector: if expected >= 32 {
            Detector::from_choice(read_u32(28))?
        } else {
            Detector::Peak
        },
        mode: if expected == STATE_BYTES {
            CompressorMode::from_choice(read_u32(32))?
        } else {
            defaults.mode
        },
        envelope: if expected == STATE_BYTES {
            EnvelopeMode::from_choice(read_u32(36))?
        } else {
            defaults.envelope
        },
        auto_release: if expected == STATE_BYTES {
            decode_bool_u32(read_u32(40))?
        } else {
            defaults.auto_release
        },
        auto_makeup: if expected == STATE_BYTES {
            decode_bool_u32(read_u32(44))?
        } else {
            defaults.auto_makeup
        },
        lookahead: if expected == STATE_BYTES {
            Lookahead::from_choice(read_u32(48))?
        } else {
            defaults.lookahead
        },
        sidechain_gain_db: if expected == STATE_BYTES {
            read_f32(52)
        } else {
            defaults.sidechain_gain_db
        },
        sidechain_mix: if expected == STATE_BYTES {
            read_f32(56)
        } else {
            defaults.sidechain_mix
        },
        sidechain_eq_mode: if expected == STATE_BYTES {
            SidechainEqMode::from_choice(read_u32(60))?
        } else {
            defaults.sidechain_eq_mode
        },
        sidechain_eq_freq_hz: if expected == STATE_BYTES {
            read_f32(64)
        } else {
            defaults.sidechain_eq_freq_hz
        },
        sidechain_eq_q: if expected == STATE_BYTES {
            read_f32(68)
        } else {
            defaults.sidechain_eq_q
        },
    };
    validate(settings)?;
    Ok(settings)
}

fn validate(settings: CompressorSettings) -> Result<(), DeviceContractError> {
    for (id, value) in [
        (parameter::THRESHOLD, settings.threshold_db),
        (parameter::RATIO, settings.ratio),
        (parameter::ATTACK, settings.attack_ms),
        (parameter::RELEASE, settings.release_ms),
        (parameter::KNEE, settings.knee_db),
        (parameter::MAKEUP, settings.makeup_db),
        (parameter::DRY_WET, settings.dry_wet),
        (parameter::SIDECHAIN_GAIN, settings.sidechain_gain_db),
        (parameter::SIDECHAIN_MIX, settings.sidechain_mix),
        (parameter::SIDECHAIN_EQ_FREQ, settings.sidechain_eq_freq_hz),
        (parameter::SIDECHAIN_EQ_Q, settings.sidechain_eq_q),
    ] {
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(ParameterValue::Float(value))?;
    }
    Ok(())
}

fn encode(settings: CompressorSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    for value in [
        settings.threshold_db,
        settings.ratio,
        settings.attack_ms,
        settings.release_ms,
        settings.knee_db,
        settings.makeup_db,
        settings.dry_wet,
    ] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    for value in [
        settings.detector as u32,
        settings.mode as u32,
        settings.envelope as u32,
        u32::from(settings.auto_release),
        u32::from(settings.auto_makeup),
        settings.lookahead as u32,
    ] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    payload.extend_from_slice(&settings.sidechain_gain_db.to_le_bytes());
    payload.extend_from_slice(&settings.sidechain_mix.to_le_bytes());
    payload.extend_from_slice(&(settings.sidechain_eq_mode as u32).to_le_bytes());
    payload.extend_from_slice(&settings.sidechain_eq_freq_hz.to_le_bytes());
    payload.extend_from_slice(&settings.sidechain_eq_q.to_le_bytes());
    NativeDeviceState::new(COMPRESSOR_TYPE_ID, COMPRESSOR_STATE_VERSION, payload)
}

/// Built-in Compressor metadata, portable state codec, and control-side processor.
pub struct CompressorDefinition;

pub static COMPRESSOR_DEFINITION: CompressorDefinition = CompressorDefinition;

impl NativeDeviceMetadata for CompressorDefinition {
    fn type_id(&self) -> DeviceTypeId {
        COMPRESSOR_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("compressor")
    }
    fn state_version(&self) -> u32 {
        COMPRESSOR_STATE_VERSION
    }
    fn auxiliary_input(&self) -> AuxiliaryInput {
        AuxiliaryInput::Stereo
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(CompressorSettings::default())
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
        Ok(crate::parameter_binding::binding(&BINDINGS, id)?.read(&settings))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        crate::parameter_binding::patch_parameter(&BINDINGS, state, id, value, decode, encode)
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        crate::parameter_binding::parameter_semantics_for_state(&BINDINGS, state, id, decode)
    }
}

impl CompressorDefinition {
    /// Closed fixed-inline factory for the same native detector, smoothing and
    /// auxiliary-input paths as the registered runtime.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedCompressor, DeviceContractError> {
        PreparedCompressor::new(state, sample_rate, parameters)
    }
}

impl NativeDeviceDefinition for CompressorDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        // The prepared factory contains only inline settings and coefficients.
        Some(crate::source_effect_arc_bytes::<PreparedCompressor>())
    }
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode(state)?;
        Ok(crate::parameter_binding::binding(&BINDINGS, id)
            .is_ok_and(|b| b.declares_arrangement_automation()))
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: hex_lattice::ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
        Ok(crate::inline_owned::prepared_owned_runtime(
            PreparedCompressor::new(state, context.sample_rate, parameters)?,
        ))
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<hex_lattice::PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(crate::inline_owned::owned_factory(
            PreparedCompressor::new(state, context.sample_rate, &[])?,
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
        Ok(Some(Box::new(PreparedCompressor::new(
            state,
            context.sample_rate,
            &[],
        )?)))
    }
}

/// Closed native Compressor factory with fixed inline settings.
#[derive(Clone, Copy)]
pub struct PreparedCompressor {
    admitted: [bool; PARAM_COUNT],
    settings: CompressorSettings,
    sample_rate: u32,
    attack_alpha: f64,
    release_alpha: f64,
    gain_alpha: f64,
    rms_alpha: f64,
    auto_release_alpha: f64,
    sidechain_gain_linear: f64,
    lookahead_frames: u32,
}

impl PreparedNativeProcessor for PreparedCompressor {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.supports(id)
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        crate::parameter_binding::binding(&BINDINGS, id)
            .ok()
            .filter(|_| self.supports(id))
            .map(|b| b.descriptor)
    }
    fn parameter_modulation_semantics(
        &self,
        id: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        crate::parameter_binding::binding(&BINDINGS, id)
            .ok()
            .filter(|binding| {
                self.supports(id)
                    && binding.semantics.modulation
                        == hex_lattice_contract::ParameterModulation::SampleRate
            })
            .map(|binding| binding.semantics)
    }
    fn supports_smoothed_bypass(&self) -> bool {
        true
    }

    fn supports_power_off(&self) -> bool {
        true
    }

    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<CompressorRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(self))
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.lookahead_frames)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(self.lookahead_frames),
        }
    }
}

impl crate::inline_owned::InlinePreparedProcessor for PreparedCompressor {
    type Runtime = CompressorRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        let initial_makeup = f64::from(self.settings.makeup_db) + auto_makeup_db(self.settings);
        let runtime = CompressorRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(self.settings, self.admitted),
            settings: self.settings,
            sample_rate: self.sample_rate,
            attack_alpha: self.attack_alpha,
            release_alpha: self.release_alpha,
            gain_alpha: self.gain_alpha,
            rms_alpha: self.rms_alpha,
            auto_release_alpha: self.auto_release_alpha,
            sidechain_gain_linear: self.sidechain_gain_linear,
            lookahead_frames: self.lookahead_frames,
            energy: [0.0; 2],
            detector_ramp: hex_dsp::LinearRamp::settled(f64::from(
                self.settings.detector == Detector::Rms,
            )),
            envelope: 0.0,
            log_envelope_db: SILENCE_FLOOR_DB,
            auto_release_env_db: 0.0,
            gain_db: initial_makeup,
            reduction_db: 0.0,
            meter: DynamicsMeter::default(),
            mix_ramp: hex_dsp::LinearRamp::settled(f64::from(self.settings.dry_wet)),
            sidechain_eq: SidechainEq::new(
                self.settings.sidechain_eq_mode,
                self.settings.sidechain_eq_freq_hz,
                self.settings.sidechain_eq_q,
                self.sample_rate,
            ),
            lookahead_pos: 0,
            lookahead_buf: [[0.0; 2]; MAX_LOOKAHEAD_FRAMES],
        };
        runtime.check_inline_ownership();
        runtime
    }
}

/// Exact inline runtime recognized by the sealed constructor's storage ledger.
#[derive(Clone, Copy)]
pub struct CompressorRuntime {
    base: PreparedCompressor,
    controls: crate::parameter_binding::Controls<CompressorSettings, PARAM_COUNT>,
    settings: CompressorSettings,
    sample_rate: u32,
    attack_alpha: f64,
    release_alpha: f64,
    gain_alpha: f64,
    rms_alpha: f64,
    auto_release_alpha: f64,
    sidechain_gain_linear: f64,
    lookahead_frames: u32,
    energy: [f64; 2],
    detector_ramp: hex_dsp::LinearRamp,
    envelope: f64,
    log_envelope_db: f64,
    auto_release_env_db: f64,
    gain_db: f64,
    /// Smoothed attenuation of the wet path, excluding automated makeup gain.
    reduction_db: f64,
    meter: DynamicsMeter,
    mix_ramp: hex_dsp::LinearRamp,
    sidechain_eq: SidechainEq,
    lookahead_pos: usize,
    lookahead_buf: [StereoFrame; MAX_LOOKAHEAD_FRAMES],
}

impl CompressorRuntime {
    fn reset_history(&mut self) {
        self.energy = [0.0; 2];
        self.detector_ramp
            .reset(f64::from(self.settings.detector == Detector::Rms));
        self.envelope = 0.0;
        self.log_envelope_db = SILENCE_FLOOR_DB;
        self.auto_release_env_db = 0.0;
        self.gain_db = f64::from(self.settings.makeup_db) + auto_makeup_db(self.settings);
        self.reduction_db = 0.0;
        self.meter = DynamicsMeter::default();
        self.mix_ramp.reset(f64::from(self.settings.dry_wet));
        self.sidechain_eq.reset();
        self.lookahead_pos = 0;
        self.lookahead_buf[..self.lookahead_frames as usize].fill([0.0; 2]);
    }

    fn process_frame(&mut self, frame: &mut StereoFrame, key: StereoFrame, bypassed: bool) {
        let left = if frame[0].is_finite() {
            f64::from(frame[0])
        } else {
            0.0
        };
        let right = if frame[1].is_finite() {
            f64::from(frame[1])
        } else {
            0.0
        };
        let [raw_key_left, raw_key_right] = key.map(|sample| {
            if sample.is_finite() {
                f64::from(sample)
            } else {
                0.0
            }
        });
        let sc_mix = f64::from(self.settings.sidechain_mix);
        let [mut key_left, mut key_right] = if sc_mix == 1.0 {
            [raw_key_left, raw_key_right]
        } else if sc_mix == 0.0 {
            [left, right]
        } else {
            [
                left * (1.0 - sc_mix) + raw_key_left * sc_mix,
                right * (1.0 - sc_mix) + raw_key_right * sc_mix,
            ]
        };
        if self.sidechain_gain_linear != 1.0 {
            key_left *= self.sidechain_gain_linear;
            key_right *= self.sidechain_gain_linear;
        }
        if self.settings.sidechain_eq_mode != SidechainEqMode::Off {
            let filtered = self.sidechain_eq.process([key_left, key_right]);
            key_left = filtered[0];
            key_right = filtered[1];
        }
        let peak = key_left.abs().max(key_right.abs());
        // Independent mean-square histories stay warm even while Peak is selected.
        for (energy, input) in self.energy.iter_mut().zip([key_left, key_right]) {
            *energy = hex_dsp::flush_below(
                self.rms_alpha * *energy + (1.0 - self.rms_alpha) * input * input,
                hex_dsp::POWER_FLOOR,
            );
        }
        let detector_mix = self.detector_ramp.advance_by_remaining();
        // Exact Peak fast path retains the original amplitude-follower arithmetic.
        let detector = if detector_mix == 0.0 {
            peak
        } else {
            let rms = self.energy[0].max(self.energy[1]).sqrt();
            if detector_mix == 1.0 {
                rms
            } else {
                (1.0 - detector_mix) * peak + detector_mix * rms
            }
        };
        let effective_release = if self.settings.auto_release {
            let sustained = (self.auto_release_env_db / 12.0).clamp(0.0, 1.0);
            let fast_ms = f64::from(self.settings.release_ms).min(50.0);
            let slow_ms = f64::from(self.settings.release_ms).max(250.0);
            let eff_ms = (fast_ms + (slow_ms - fast_ms) * sustained).clamp(5.0, 1_000.0);
            hex_dsp::one_pole_pole(self.sample_rate, eff_ms * 0.001).unwrap_or(self.release_alpha)
        } else {
            self.release_alpha
        };
        let coefficient = if detector > self.envelope {
            self.attack_alpha
        } else {
            effective_release
        };
        self.envelope =
            hex_dsp::flush_quiet(coefficient * self.envelope + (1.0 - coefficient) * detector);
        let detector_db = hex_dsp::LinearAmplitude::new(detector)
            .ok()
            .and_then(hex_dsp::LinearAmplitude::decibels)
            .map_or(SILENCE_FLOOR_DB, hex_dsp::Decibels::value)
            .max(SILENCE_FLOOR_DB);
        let log_coeff = if detector_db > self.log_envelope_db {
            self.attack_alpha
        } else {
            effective_release
        };
        self.log_envelope_db = (log_coeff * self.log_envelope_db + (1.0 - log_coeff) * detector_db)
            .clamp(SILENCE_FLOOR_DB, 60.0);
        let level_db = match self.settings.envelope {
            EnvelopeMode::Linear => hex_dsp::LinearAmplitude::new(self.envelope)
                .ok()
                .and_then(hex_dsp::LinearAmplitude::decibels)
                .map_or(SILENCE_FLOOR_DB, hex_dsp::Decibels::value),
            EnvelopeMode::Logarithmic => self.log_envelope_db,
        };
        let reduction_db = gain_reduction_db_for_mode(
            level_db,
            f64::from(self.settings.threshold_db),
            f64::from(self.settings.ratio),
            f64::from(self.settings.knee_db),
            self.settings.mode,
        );
        self.auto_release_env_db = hex_dsp::flush_quiet(
            self.auto_release_alpha * self.auto_release_env_db
                + (1.0 - self.auto_release_alpha) * reduction_db.abs(),
        );
        let target_gain_db =
            reduction_db + f64::from(self.settings.makeup_db) + auto_makeup_db(self.settings);
        self.gain_db = hex_dsp::flush_subnormal(
            self.gain_alpha * self.gain_db + (1.0 - self.gain_alpha) * target_gain_db,
        );
        self.reduction_db = hex_dsp::flush_quiet(
            self.gain_alpha * self.reduction_db + (1.0 - self.gain_alpha) * reduction_db,
        );
        let meter_detector = match self.settings.envelope {
            EnvelopeMode::Linear => self.envelope as f32,
            EnvelopeMode::Logarithmic => hex_dsp::Decibels::new(self.log_envelope_db)
                .and_then(hex_dsp::Decibels::linear_amplitude)
                .map_or(0.0, |v| v.value() as f32),
        };
        if !bypassed {
            let peak_reduction = self.meter.reduction_db[0].max(-self.reduction_db as f32);
            self.meter.reduction_db = [peak_reduction; 2];
            self.meter.post_input_peak = self.meter.post_input_peak.max(meter_detector);
        }
        let (audio_left, audio_right) = if self.lookahead_frames > 0 {
            let delayed = self.lookahead_buf[self.lookahead_pos];
            self.lookahead_buf[self.lookahead_pos] = [left as f32, right as f32];
            self.lookahead_pos += 1;
            if self.lookahead_pos >= self.lookahead_frames as usize {
                self.lookahead_pos = 0;
            }
            (f64::from(delayed[0]), f64::from(delayed[1]))
        } else {
            (left, right)
        };
        // A finite 5 ms ramp reaches exact endpoints, independently of block
        // partitions. Detector/gain history advances even at fully dry output.
        let mix = self.mix_ramp.advance_by_remaining();
        if bypassed {
            if self.lookahead_frames > 0 {
                frame[0] = audio_left as f32;
                frame[1] = audio_right as f32;
            }
            return;
        }
        if mix == 0.0 {
            frame[0] = audio_left as f32;
            frame[1] = audio_right as f32;
            return;
        }
        let gain = hex_dsp::Decibels::new(self.gain_db)
            .and_then(hex_dsp::Decibels::linear_amplitude)
            .map_or(0.0, hex_dsp::LinearAmplitude::value);
        if mix == 1.0 {
            // Preserve the original fully-wet operation order and rounding.
            frame[0] = (audio_left * gain) as f32;
            frame[1] = (audio_right * gain) as f32;
        } else {
            frame[0] = (audio_left * (1.0 - mix) + audio_left * gain * mix) as f32;
            frame[1] = (audio_right * (1.0 - mix) + audio_right * gain * mix) as f32;
        }
    }
}

impl NativeProcessorRuntime for CompressorRuntime {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.base.supports(id)
    }
    crate::parameter_binding::delegate_automated_block_processing!(process_automated);
    fn process_block_with_auxiliary(
        &mut self,
        block: StereoAuxiliaryBlock<'_>,
        _clock: Option<&dyn NativeInstrumentClock>,
        automation: Option<&dyn NativeParameterAutomation>,
        bypassed: bool,
        _resources: &dyn hex_lattice::DeviceResources,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = automation
            .map(|values| self.controls.resolve(&BINDINGS, values))
            .transpose()?;
        let (main, key) = block.into_parts();
        for (offset, (frame, key)) in main.iter_mut().zip(key).enumerate() {
            if let (Some(values), Some(resolved)) = (automation, resolved.as_mut()) {
                self.apply_values(values, resolved, offset)?;
            }
            let key = key.map(|sample| if sample.is_finite() { sample } else { 0.0 });
            self.process_frame(frame, key, bypassed);
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            self.process_frame(frame, *frame, false);
        }
    }
    fn process_bypassed_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            self.process_frame(frame, *frame, true);
        }
    }
    fn reset(&mut self) {
        self.controls.reset();
        self.settings = self.base.settings;
        self.attack_alpha = self.base.attack_alpha;
        self.release_alpha = self.base.release_alpha;
        self.sidechain_gain_linear = self.base.sidechain_gain_linear;
        self.sidechain_eq = SidechainEq::new(
            self.settings.sidechain_eq_mode,
            self.settings.sidechain_eq_freq_hz,
            self.settings.sidechain_eq_q,
            self.sample_rate,
        );
        self.reset_history();
    }
    fn is_quiescent(&self) -> bool {
        self.lookahead_frames == 0
            || self.lookahead_buf[..self.lookahead_frames as usize]
                .iter()
                .all(|frame| frame[0] == 0.0 && frame[1] == 0.0)
    }
    fn take_dynamics_meter(&mut self) -> Option<DynamicsMeter> {
        Some(std::mem::take(&mut self.meter))
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| {
                self.sample_rate == previous.sample_rate
                    && self.lookahead_frames == previous.lookahead_frames
            })
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>().filter(|previous| {
            self.sample_rate == previous.sample_rate
                && self.lookahead_frames == previous.lookahead_frames
        }) {
            if self.base.admitted.iter().any(|&v| v) && previous.base.admitted.iter().any(|&v| v) {
                self.controls.inherit(&previous.controls);
                self.settings = previous.settings;
                self.attack_alpha = previous.attack_alpha;
                self.release_alpha = previous.release_alpha;
                self.sidechain_gain_linear = previous.sidechain_gain_linear;
                self.sidechain_eq = previous.sidechain_eq;
            } else {
                let mut eq = previous.sidechain_eq;
                eq.update(
                    self.settings.sidechain_eq_mode,
                    self.settings.sidechain_eq_freq_hz,
                    self.settings.sidechain_eq_q,
                    self.sample_rate,
                );
                self.sidechain_eq = eq;
            }
            self.energy = previous.energy;
            self.detector_ramp = previous.detector_ramp;
            if self.settings.detector != previous.settings.detector {
                self.detector_ramp.retarget(
                    f64::from(self.settings.detector == Detector::Rms),
                    hex_dsp::ramp_frames_for_ms(self.sample_rate, 5.0).unwrap_or(1),
                );
            }
            self.envelope = previous.envelope;
            self.log_envelope_db = previous.log_envelope_db;
            self.auto_release_env_db = previous.auto_release_env_db;
            self.gain_db = previous.gain_db;
            self.reduction_db = previous.reduction_db;
            // A scratch twin starts a new render transaction. Its previous
            // readings may belong to an aborted block.
            self.meter = DynamicsMeter::default();
            self.mix_ramp = previous.mix_ramp;
            if self.settings.dry_wet != previous.settings.dry_wet {
                // Same preparation/history handoff as the existing gain smoother.
                self.mix_ramp.retarget(
                    f64::from(self.settings.dry_wet),
                    hex_dsp::ramp_frames_for_ms(self.sample_rate, 5.0).unwrap_or(1),
                );
            }
            if self.lookahead_frames > 0 {
                let len = self.lookahead_frames as usize;
                self.lookahead_buf[..len].copy_from_slice(&previous.lookahead_buf[..len]);
                self.lookahead_pos = previous.lookahead_pos % len;
            }
        }
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| {
                self.sample_rate == previous.sample_rate
                    && self.lookahead_frames == previous.lookahead_frames
            })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn auto_makeup_db(settings: CompressorSettings) -> f64 {
    if !settings.auto_makeup || settings.mode != CompressorMode::Compress {
        return 0.0;
    }
    (-0.5 * f64::from(settings.threshold_db) * (1.0 - 1.0 / f64::from(settings.ratio)))
        .clamp(0.0, 24.0)
}

fn gain_reduction_db_for_mode(
    level_db: f64,
    threshold_db: f64,
    ratio: f64,
    knee_db: f64,
    mode: CompressorMode,
) -> f64 {
    match mode {
        CompressorMode::Compress => gain_reduction_db(level_db, threshold_db, ratio, knee_db),
        CompressorMode::Expand => {
            let under = threshold_db - level_db;
            let slope = 1.0 - ratio;
            let raw = if knee_db <= 0.0 {
                if under > 0.0 { slope * under } else { 0.0 }
            } else {
                let knee_start = -knee_db * 0.5;
                let knee_end = knee_db * 0.5;
                if under <= knee_start {
                    0.0
                } else if under >= knee_end {
                    slope * under
                } else {
                    slope * (under - knee_start).powi(2) / (2.0 * knee_db)
                }
            };
            raw.max(-60.0)
        }
    }
}

fn gain_reduction_db(level_db: f64, threshold_db: f64, ratio: f64, knee_db: f64) -> f64 {
    let over_threshold = level_db - threshold_db;
    let ratio_slope = 1.0 / ratio - 1.0;
    if knee_db <= 0.0 {
        return if over_threshold > 0.0 {
            ratio_slope * over_threshold
        } else {
            0.0
        };
    }
    let knee_start = -knee_db * 0.5;
    let knee_end = knee_db * 0.5;
    if over_threshold <= knee_start {
        0.0
    } else if over_threshold >= knee_end {
        ratio_slope * over_threshold
    } else {
        ratio_slope * (over_threshold - knee_start).powi(2) / (2.0 * knee_db)
    }
}

/// Bounded control-side static knee curve, independent of detector timing.
#[derive(Clone, Debug, PartialEq)]
pub struct CompressorTransferCurve {
    pub points: [[f32; 2]; 129],
}

/// Steady detector input is -60..0 dB; output is normalized to -66..24 dB.
/// Uses the same gain-reduction law as the runtime, including manual and auto makeup.
pub fn transfer_curve(
    state: &NativeDeviceState,
) -> Result<CompressorTransferCurve, DeviceContractError> {
    let settings = decode(state)?;
    let makeup = f64::from(settings.makeup_db) + auto_makeup_db(settings);
    let points = std::array::from_fn(|index| {
        let x = index as f64 / 128.0;
        let input_db = -60.0 + x * 60.0;
        let wet_gain_db = gain_reduction_db_for_mode(
            input_db,
            f64::from(settings.threshold_db),
            f64::from(settings.ratio),
            f64::from(settings.knee_db),
            settings.mode,
        ) + makeup;
        let mix = f64::from(settings.dry_wet);
        let output_db = if mix == 1.0 {
            input_db + wet_gain_db
        } else if mix == 0.0 {
            input_db
        } else {
            let wet_linear = hex_dsp::Decibels::new(wet_gain_db)
                .and_then(hex_dsp::Decibels::linear_amplitude)
                .map_or(0.0, hex_dsp::LinearAmplitude::value);
            let blended = (1.0 - mix) + mix * wet_linear;
            input_db
                + hex_dsp::LinearAmplitude::new(blended)
                    .ok()
                    .and_then(hex_dsp::LinearAmplitude::decibels)
                    .map_or(SILENCE_FLOOR_DB, hex_dsp::Decibels::value)
        };
        [x as f32, ((output_db + 66.0) / 90.0) as f32]
    });
    Ok(CompressorTransferCurve { points })
}

#[cfg(test)]
mod tests;
