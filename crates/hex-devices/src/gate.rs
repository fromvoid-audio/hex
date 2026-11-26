//! Stereo-linked hysteresis gate with Ableton Live 12 parity controls.
//!
//! Supports Threshold/Return hysteresis, Floor attenuation, Attack/Hold/Release
//! envelopes, Flip (inverted ducking) mode, 0/1.5/10 ms Lookahead with declared
//! algorithmic latency, external stereo sidechain routing, Sidechain Gain/Mix/EQ
//! filtering, and live `DynamicsMeter` reporting.
//!
//! Twelve controls consume the shared borrowed final-value stream; Lookahead is
//! structural because it changes declared algorithmic latency.
mod automation;
use crate::compressor::sidechain_eq::{SidechainEq, SidechainEqMode};

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

pub const GATE_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.gate");
/// Current state version. Supported saved versions (Pulse #3280):
/// - v1 (24 bytes): threshold, return, floor, attack, hold, release.
/// - v2 (26 Nov 2026, 52 bytes): adds flip, lookahead, sidechain gain/mix, and
///   sidechain EQ mode/freq/Q; v1 migrates losslessly to bit-exact v1 behavior.
pub const GATE_STATE_VERSION: u32 = 2;
const STATE_BYTES: usize = 52;
const PARAM_COUNT: usize = 13;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const MAX_LOOKAHEAD_FRAMES: usize = 1_920; // 10 ms at 192 kHz
/// Shortest internal hold, whatever the hold control says (Pulse #4107).
///
/// The detector is the instantaneous linked peak, so a low-frequency tone
/// dips below the return level around every zero crossing. With no hold the
/// gate closed there and chattered. Holding at least 5 ms rides through those
/// dips (a sine at amplitude `a` stays below return `r` for
/// `2 asin(r / a) / omega`, about 4.75 ms at 40 Hz just above a 5 dB
/// hysteresis), so the hold control's range below 5 ms acts as 5 ms. Lower
/// tones, or a return level close to the threshold, can still exceed it.
/// This is a hex decision, not a measured reference behaviour.
const MIN_HOLD_MS: f32 = 5.0;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const THRESHOLD: ParameterId = ParameterId::from_static(1);
    pub const RETURN: ParameterId = ParameterId::from_static(2);
    pub const FLOOR: ParameterId = ParameterId::from_static(3);
    pub const ATTACK: ParameterId = ParameterId::from_static(4);
    pub const HOLD: ParameterId = ParameterId::from_static(5);
    pub const RELEASE: ParameterId = ParameterId::from_static(6);
    pub const FLIP: ParameterId = ParameterId::from_static(7);
    pub const LOOKAHEAD: ParameterId = ParameterId::from_static(8);
    pub const SIDECHAIN_GAIN: ParameterId = ParameterId::from_static(9);
    pub const SIDECHAIN_MIX: ParameterId = ParameterId::from_static(10);
    pub const SIDECHAIN_EQ_MODE: ParameterId = ParameterId::from_static(11);
    pub const SIDECHAIN_EQ_FREQ: ParameterId = ParameterId::from_static(12);
    pub const SIDECHAIN_EQ_Q: ParameterId = ParameterId::from_static(13);
}

const DESCRIPTORS: &[ParameterDescriptor] = &[
    ParameterDescriptor {
        id: parameter::THRESHOLD,
        label: "threshold",
        kind: ParameterKind::Float {
            min: -72.0,
            max: 0.0,
            unit: Some("dbfs"),
        },
        default: ParameterValue::Float(-40.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RETURN,
        label: "return",
        kind: ParameterKind::Float {
            min: -84.0,
            max: 0.0,
            unit: Some("dbfs"),
        },
        default: ParameterValue::Float(-45.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::FLOOR,
        label: "floor",
        kind: ParameterKind::Float {
            min: -80.0,
            max: 0.0,
            unit: Some("db"),
        },
        default: ParameterValue::Float(-60.0),
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
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::HOLD,
        label: "hold",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 2_000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(50.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::RELEASE,
        label: "release",
        kind: ParameterKind::Float {
            min: 1.0,
            max: 2_000.0,
            unit: Some("ms"),
        },
        default: ParameterValue::Float(100.0),
        presentation: ParameterPresentation::Logarithmic,
    },
    ParameterDescriptor {
        id: parameter::FLIP,
        label: "flip",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: parameter::LOOKAHEAD,
        label: "lookahead",
        kind: ParameterKind::Choice {
            labels: &["0 ms", "1.5 ms", "10 ms"],
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
enum GateLookahead {
    Off = 0,
    OnePointFiveMs = 1,
    TenMs = 2,
}
impl GateLookahead {
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Off),
            1 => Ok(Self::OnePointFiveMs),
            2 => Ok(Self::TenMs),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
    fn frames(self, sample_rate: u32) -> u32 {
        let ms = match self {
            Self::Off => return 0,
            Self::OnePointFiveMs => 1.5,
            Self::TenMs => 10.0,
        };
        let frames = (ms * f64::from(sample_rate) / 1_000.0).ceil() as u32;
        frames.min(MAX_LOOKAHEAD_FRAMES as u32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GateSettings {
    threshold_db: f32,
    return_db: f32,
    floor_db: f32,
    attack_ms: f32,
    hold_ms: f32,
    release_ms: f32,
    flip: bool,
    lookahead: GateLookahead,
    sidechain_gain_db: f32,
    sidechain_mix: f32,
    sidechain_eq_mode: SidechainEqMode,
    sidechain_eq_freq_hz: f32,
    sidechain_eq_q: f32,
}

const MODULATED_SCALAR: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        modulation: hex_lattice_contract::ParameterModulation::SampleRate,
        ..crate::parameter_binding::SCALAR_ENVELOPE
    };
const BINDINGS: [crate::parameter_binding::Binding<GateSettings>; PARAM_COUNT] = [
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[0],
        MODULATED_SCALAR,
        |c| c.threshold_db,
        |c, v| c.threshold_db = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[1],
        MODULATED_SCALAR,
        |c| c.return_db,
        |c, v| c.return_db = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[2],
        MODULATED_SCALAR,
        |c| c.floor_db,
        |c, v| c.floor_db = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[3],
        hex_lattice_contract::ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..MODULATED_SCALAR
        },
        |c| c.attack_ms,
        |c, v| c.attack_ms = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[4],
        MODULATED_SCALAR,
        |c| c.hold_ms,
        |c, v| c.hold_ms = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[5],
        hex_lattice_contract::ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..MODULATED_SCALAR
        },
        |c| c.release_ms,
        |c, v| c.release_ms = v,
    ),
    crate::parameter_binding::Binding::boolean(
        DESCRIPTORS[6],
        hex_lattice_contract::ParameterSemantics {
            stepping: hex_lattice_contract::ParameterStepping::Boolean,
            modulation: hex_lattice_contract::ParameterModulation::SampleRate,
            smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |c| c.flip,
        |c, v| c.flip = v,
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[7],
        hex_lattice_contract::ParameterSemantics {
            automation: hex_lattice_contract::ParameterAutomation::Unsupported,
            modulation: hex_lattice_contract::ParameterModulation::Unsupported,
            change_impact: hex_lattice_contract::ParameterChangeImpact::Structural,
            ..crate::parameter_binding::CHOICE_ENVELOPE
        },
        |c| c.lookahead as u32,
        |c, v| {
            c.lookahead = GateLookahead::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[8],
        MODULATED_SCALAR,
        |c| c.sidechain_gain_db,
        |c, v| c.sidechain_gain_db = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[9],
        MODULATED_SCALAR,
        |c| c.sidechain_mix,
        |c, v| c.sidechain_mix = v,
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[10],
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
        DESCRIPTORS[11],
        hex_lattice_contract::ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..MODULATED_SCALAR
        },
        |c| c.sidechain_eq_freq_hz,
        |c, v| c.sidechain_eq_freq_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[12],
        hex_lattice_contract::ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..MODULATED_SCALAR
        },
        |c| c.sidechain_eq_q,
        |c, v| c.sidechain_eq_q = v,
    ),
];

impl Default for GateSettings {
    fn default() -> Self {
        Self {
            threshold_db: -40.0,
            return_db: -45.0,
            floor_db: -60.0,
            attack_ms: 1.0,
            hold_ms: 50.0,
            release_ms: 100.0,
            flip: false,
            lookahead: GateLookahead::Off,
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

fn decode(state: &NativeDeviceState) -> Result<GateSettings, DeviceContractError> {
    if state.type_id() != GATE_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    let expected = match state.state_version() {
        1 => 24,
        GATE_STATE_VERSION => STATE_BYTES,
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
    let defaults = GateSettings::default();
    let settings = GateSettings {
        threshold_db: read_f32(0),
        return_db: read_f32(4),
        floor_db: read_f32(8),
        attack_ms: read_f32(12),
        hold_ms: read_f32(16),
        release_ms: read_f32(20),
        flip: if expected == STATE_BYTES {
            decode_bool_u32(read_u32(24))?
        } else {
            defaults.flip
        },
        lookahead: if expected == STATE_BYTES {
            GateLookahead::from_choice(read_u32(28))?
        } else {
            defaults.lookahead
        },
        sidechain_gain_db: if expected == STATE_BYTES {
            read_f32(32)
        } else {
            defaults.sidechain_gain_db
        },
        sidechain_mix: if expected == STATE_BYTES {
            read_f32(36)
        } else {
            defaults.sidechain_mix
        },
        sidechain_eq_mode: if expected == STATE_BYTES {
            SidechainEqMode::from_choice(read_u32(40))?
        } else {
            defaults.sidechain_eq_mode
        },
        sidechain_eq_freq_hz: if expected == STATE_BYTES {
            read_f32(44)
        } else {
            defaults.sidechain_eq_freq_hz
        },
        sidechain_eq_q: if expected == STATE_BYTES {
            read_f32(48)
        } else {
            defaults.sidechain_eq_q
        },
    };
    validate(settings)?;
    Ok(settings)
}

fn validate(settings: GateSettings) -> Result<(), DeviceContractError> {
    for (id, value) in [
        (parameter::THRESHOLD, settings.threshold_db),
        (parameter::RETURN, settings.return_db),
        (parameter::FLOOR, settings.floor_db),
        (parameter::ATTACK, settings.attack_ms),
        (parameter::HOLD, settings.hold_ms),
        (parameter::RELEASE, settings.release_ms),
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
    if settings.return_db > settings.threshold_db {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(())
}

fn encode(settings: GateSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    for value in [
        settings.threshold_db,
        settings.return_db,
        settings.floor_db,
        settings.attack_ms,
        settings.hold_ms,
        settings.release_ms,
    ] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    payload.extend_from_slice(&u32::from(settings.flip).to_le_bytes());
    payload.extend_from_slice(&(settings.lookahead as u32).to_le_bytes());
    payload.extend_from_slice(&settings.sidechain_gain_db.to_le_bytes());
    payload.extend_from_slice(&settings.sidechain_mix.to_le_bytes());
    payload.extend_from_slice(&(settings.sidechain_eq_mode as u32).to_le_bytes());
    payload.extend_from_slice(&settings.sidechain_eq_freq_hz.to_le_bytes());
    payload.extend_from_slice(&settings.sidechain_eq_q.to_le_bytes());
    NativeDeviceState::new(GATE_TYPE_ID, GATE_STATE_VERSION, payload)
}

/// Registered Lattice Gate metadata, state codec, and control-side preparation.
pub struct GateDefinition;

pub static GATE_DEFINITION: GateDefinition = GateDefinition;

impl GateDefinition {
    /// Prepare the asset-free inline factory for sealed native ownership.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedGate, DeviceContractError> {
        PreparedGate::new(state, rate, parameters)
    }
}

impl NativeDeviceMetadata for GateDefinition {
    fn type_id(&self) -> DeviceTypeId {
        GATE_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("gate")
    }
    fn state_version(&self) -> u32 {
        GATE_STATE_VERSION
    }
    fn auxiliary_input(&self) -> AuxiliaryInput {
        AuxiliaryInput::Stereo
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(GateSettings::default())
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

impl NativeDeviceDefinition for GateDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        // The prepared factory contains only inline settings and coefficients.
        Some(crate::source_effect_arc_bytes::<PreparedGate>())
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
            PreparedGate::new(state, context.sample_rate, parameters)?,
        ))
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<hex_lattice::PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(crate::inline_owned::owned_factory(PreparedGate::new(
            state,
            context.sample_rate,
            &[],
        )?)))
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
        Ok(Some(Box::new(PreparedGate::new(
            state,
            context.sample_rate,
            &[],
        )?)))
    }
}

fn db_to_gain(db: f32) -> Result<f64, DeviceContractError> {
    hex_dsp::Decibels::new(f64::from(db))
        .and_then(hex_dsp::Decibels::linear_amplitude)
        .map(hex_dsp::LinearAmplitude::value)
        .map_err(|_| DeviceContractError::InvalidParameterValue)
}

fn checked_frames(milliseconds: f32, sample_rate: u32) -> Result<u32, DeviceContractError> {
    let frames = (f64::from(milliseconds) * f64::from(sample_rate) / 1_000.0).ceil();
    if !frames.is_finite() || frames < 1.0 || frames > f64::from(u32::MAX) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    Ok(frames as u32)
}

/// Closed Gate factory with no indirect storage.
#[derive(Clone, Copy)]
pub struct PreparedGate {
    admitted: [bool; PARAM_COUNT],
    settings: GateSettings,
    sample_rate: u32,
    threshold: f64,
    return_level: f64,
    floor_gain: f64,
    attack_frames: u32,
    hold_frames: u32,
    release_frames: u32,
    sidechain_gain_linear: f64,
    lookahead_frames: u32,
}

impl PreparedNativeProcessor for PreparedGate {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.supports(id)
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        self.supports(id)
            .then(|| {
                crate::parameter_binding::binding(&BINDINGS, id)
                    .ok()
                    .map(|b| b.descriptor)
            })
            .flatten()
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
        std::mem::size_of::<GateRuntime>()
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

impl crate::inline_owned::InlinePreparedProcessor for PreparedGate {
    type Runtime = GateRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        let closed_gain = if self.settings.flip {
            1.0
        } else {
            self.floor_gain
        };
        let runtime = GateRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(self.settings, self.admitted),
            settings: self.settings,
            sample_rate: self.sample_rate,
            threshold: self.threshold,
            return_level: self.return_level,
            floor_gain: self.floor_gain,
            attack_frames: self.attack_frames,
            hold_frames: self.hold_frames,
            release_frames: self.release_frames,
            sidechain_gain_linear: self.sidechain_gain_linear,
            lookahead_frames: self.lookahead_frames,
            open: false,
            hold_elapsed: 0,
            gain_ramp: hex_dsp::LinearRamp::settled(closed_gain),
            meter: DynamicsMeter::default(),
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct GateHistory {
    open: bool,
    hold_elapsed: u32,
    gain_ramp: hex_dsp::LinearRamp,
}

/// Exact inline runtime recognized by the sealed constructor's storage ledger.
#[derive(Clone, Copy)]
pub struct GateRuntime {
    base: PreparedGate,
    controls: crate::parameter_binding::Controls<GateSettings, PARAM_COUNT>,
    settings: GateSettings,
    sample_rate: u32,
    threshold: f64,
    return_level: f64,
    floor_gain: f64,
    attack_frames: u32,
    hold_frames: u32,
    release_frames: u32,
    sidechain_gain_linear: f64,
    lookahead_frames: u32,
    open: bool,
    hold_elapsed: u32,
    gain_ramp: hex_dsp::LinearRamp,
    meter: DynamicsMeter,
    sidechain_eq: SidechainEq,
    lookahead_pos: usize,
    lookahead_buf: [StereoFrame; MAX_LOOKAHEAD_FRAMES],
}

impl GateRuntime {
    fn history(&self) -> GateHistory {
        GateHistory {
            open: self.open,
            hold_elapsed: self.hold_elapsed,
            gain_ramp: self.gain_ramp,
        }
    }

    fn set_history(&mut self, history: GateHistory) {
        self.open = history.open;
        self.hold_elapsed = history.hold_elapsed;
        self.gain_ramp = history.gain_ramp;
    }

    fn target_gain(&self, open: bool) -> f64 {
        if open != self.settings.flip {
            1.0
        } else {
            self.floor_gain
        }
    }

    fn reset_history(&mut self) {
        self.open = false;
        self.hold_elapsed = 0;
        self.gain_ramp.reset(self.target_gain(false));
        self.meter = DynamicsMeter::default();
        self.sidechain_eq.reset();
        self.lookahead_pos = 0;
        self.lookahead_buf[..self.lookahead_frames as usize].fill([0.0; 2]);
    }

    fn start_ramp(&mut self, target: f64, frames: u32) {
        self.gain_ramp.retarget(target, frames);
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
        if peak >= self.threshold {
            if !self.open {
                self.open = true;
                self.hold_elapsed = 0;
                self.start_ramp(self.target_gain(true), self.attack_frames);
            } else {
                self.hold_elapsed = 0;
            }
        } else if self.open {
            if peak <= self.return_level {
                // `hold_frames` is at least `MIN_HOLD_MS`, never zero.
                if self.hold_elapsed < self.hold_frames {
                    self.hold_elapsed += 1;
                } else {
                    self.open = false;
                    self.hold_elapsed = 0;
                    self.start_ramp(self.target_gain(false), self.release_frames);
                }
            } else {
                self.hold_elapsed = 0;
            }
        }
        let gain = self.gain_ramp.advance();
        if !bypassed {
            let reduction = hex_dsp::LinearAmplitude::new(gain.max(1e-30))
                .ok()
                .and_then(hex_dsp::LinearAmplitude::decibels)
                .map_or(80.0, |db| (-db.value()).max(0.0) as f32);
            let peak_reduction = self.meter.reduction_db[0].max(reduction);
            self.meter.reduction_db = [peak_reduction; 2];
            self.meter.post_input_peak = self.meter.post_input_peak.max(peak as f32);
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
        if bypassed {
            if self.lookahead_frames > 0 {
                frame[0] = audio_left as f32;
                frame[1] = audio_right as f32;
            }
            return;
        }
        frame[0] = (audio_left * gain) as f32;
        frame[1] = (audio_right * gain) as f32;
    }
}

impl NativeProcessorRuntime for GateRuntime {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.base.supports(id)
    }
    crate::parameter_binding::delegate_automated_block_processing!(process_automated);
    fn supports_detached_source_rejoin(&self) -> bool {
        true
    }
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
        let modulates_threshold =
            automation.is_some_and(|v| v.has_sample_rate_modulation(parameter::THRESHOLD));
        let modulates_return =
            automation.is_some_and(|v| v.has_sample_rate_modulation(parameter::RETURN));
        let (main, key) = block.into_parts();
        for (offset, (frame, key)) in main.iter_mut().zip(key).enumerate() {
            if let (Some(values), Some(resolved)) = (automation, resolved.as_mut()) {
                self.apply_values(
                    values,
                    resolved,
                    offset,
                    modulates_threshold,
                    modulates_return,
                )?;
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
        self.threshold = self.base.threshold;
        self.return_level = self.base.return_level;
        self.floor_gain = self.base.floor_gain;
        self.attack_frames = self.base.attack_frames;
        self.hold_frames = self.base.hold_frames;
        self.release_frames = self.base.release_frames;
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
        let Some(previous) = previous.as_any().downcast_ref::<Self>().filter(|previous| {
            self.sample_rate == previous.sample_rate
                && self.lookahead_frames == previous.lookahead_frames
        }) else {
            return;
        };
        self.set_history(previous.history());
        self.meter = DynamicsMeter::default();
        if self.base.admitted.iter().any(|&v| v) && previous.base.admitted.iter().any(|&v| v) {
            self.controls.inherit(&previous.controls);
            self.settings = previous.settings;
            self.threshold = previous.threshold;
            self.return_level = previous.return_level;
            self.floor_gain = previous.floor_gain;
            self.attack_frames = previous.attack_frames;
            self.hold_frames = previous.hold_frames;
            self.release_frames = previous.release_frames;
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
            self.retarget_envelope(previous.settings);
        }
        if self.lookahead_frames > 0 {
            let len = self.lookahead_frames as usize;
            self.lookahead_buf[..len].copy_from_slice(&previous.lookahead_buf[..len]);
            self.lookahead_pos = previous.lookahead_pos % len;
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

#[cfg(test)]
mod tests;
