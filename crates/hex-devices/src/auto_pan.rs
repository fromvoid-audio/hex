//! Original stereo Auto Pan-Tremolo with bounded parameter transitions and
//! tempo-synced or free-running modulation.
//!
//! Supports Ableton Live 12 Auto Pan-Tremolo modes and controls:
//! - `mode`: `panning` (stereo offset between L/R) or `tremolo` (in-phase L/R)
//! - `amount`, `rate` (`0.05..=20.0 hz`), `rate_mode` (`hz` | `sync`),
//!   `sync_division` (16 musical divisions from `1/64` to `8 bars` driven by
//!   `NativeInstrumentClock` with 120 BPM fallback when clockless)
//! - `waveform`: `sine`, `triangle`, `saw up`, `saw down`, `square`, `random`
//! - `stereo_mode`: `phase` (`right_phase` 0..360 deg with shortest-arc ramp)
//!   or `spin` (continuous L/R frequency detune)
//! - `offset` (0..360 deg phase offset), `shape` (0..1 waveform steepening),
//!   `invert` (LFO polarity flip), and `mod_mode` (`attenuate` unipolar gain
//!   vs `modulate` bipolar ring-mod gain).
//!
//! Supported saved state versions:
//! - v1 (11 Oct 2026, 12 bytes): amount, rate, right_phase (`f32` LE); migrates
//!   losslessly to v3 with `waveform = sine` and transparent defaults.
//! - v2 (18 Oct 2026, 16 bytes): adds `waveform` (`u32` LE, `0..=4`); migrates
//!   losslessly to v3 with transparent defaults, rendering bit-identically.
//! - v3 (52 bytes): 6 `f32` LE scalars (`amount`, `rate_hz`,
//!   `right_phase_degrees`, `offset_degrees`, `shape`, `spin`) + 7 `u32` LE
//!   tags (`waveform`, `mode`, `rate_mode`, `sync_division`, `invert`,
//!   `stereo_mode`, `mod_mode`).

use hex_lattice::{
    NativeDeviceDefinition, NativeInstrumentClock, NativeProcessorRuntime, OutputTail,
    PreparedNativeProcessor, StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;
mod automation;

pub const AUTO_PAN_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.auto-pan");
pub const AUTO_PAN_STATE_VERSION: u32 = 3;
const STATE_BYTES_V1: usize = 12;
const STATE_BYTES_V2: usize = 16;
const STATE_BYTES: usize = 52;
const PARAM_COUNT: usize = 13;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const DEFAULT_RAMP_MS: f64 = 5.0;
const FALLBACK_TEMPO_BPM: f64 = 120.0;
const Q32_32_SCALE: f64 = 4_294_967_296.0;
/// Square, saw, and random step discontinuities become a linear transition of
/// this duration at any sample rate (Pulse #3959), so an edge never steps the
/// gain in one sample.
const EDGE_SECONDS: f64 = 0.001;
const RANDOM_STEPS_PER_CYCLE: usize = 8;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const AMOUNT: ParameterId = ParameterId::from_static(1);
    pub const RATE: ParameterId = ParameterId::from_static(2);
    pub const RIGHT_PHASE: ParameterId = ParameterId::from_static(3);
    pub const WAVEFORM: ParameterId = ParameterId::from_static(4);
    pub const MODE: ParameterId = ParameterId::from_static(5);
    pub const RATE_MODE: ParameterId = ParameterId::from_static(6);
    pub const SYNC_DIVISION: ParameterId = ParameterId::from_static(7);
    pub const OFFSET: ParameterId = ParameterId::from_static(8);
    pub const SHAPE: ParameterId = ParameterId::from_static(9);
    pub const INVERT: ParameterId = ParameterId::from_static(10);
    pub const STEREO_MODE: ParameterId = ParameterId::from_static(11);
    pub const SPIN: ParameterId = ParameterId::from_static(12);
    pub const MOD_MODE: ParameterId = ParameterId::from_static(13);
}

const SMOOTHED_ENVELOPE: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        modulation: hex_lattice_contract::ParameterModulation::SampleRate,
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
        ..crate::parameter_binding::SCALAR_ENVELOPE
    };

const SMOOTHED_CHOICE_ENVELOPE: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        modulation: hex_lattice_contract::ParameterModulation::SampleRate,
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
        ..crate::parameter_binding::CHOICE_ENVELOPE
    };

const SMOOTHED_BOOL_ENVELOPE: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        modulation: hex_lattice_contract::ParameterModulation::SampleRate,
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
        ..crate::parameter_binding::BOOLEAN_ENVELOPE
    };

const SYNC_DIVISION_LABELS: &[&str] = &[
    "1/64", "1/32", "1/16t", "1/16", "1/16d", "1/8t", "1/8", "1/8d", "1/4t", "1/4", "1/4d", "1/2",
    "1 bar", "2 bars", "4 bars", "8 bars",
];

const SYNC_DIVISION_BEATS: [f64; 16] = [
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 6.0,
    1.0 / 4.0,
    3.0 / 8.0,
    1.0 / 3.0,
    0.5,
    0.75,
    2.0 / 3.0,
    1.0,
    1.5,
    2.0,
    4.0,
    8.0,
    16.0,
    32.0,
];

const BINDINGS: [crate::parameter_binding::Binding<AutoPanSettings>; PARAM_COUNT] = [
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::AMOUNT,
            label: "amount",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.5),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.amount,
        |c, v| c.amount = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::RATE,
            label: "rate",
            kind: ParameterKind::Float {
                min: 0.05,
                max: 20.0,
                unit: Some("hz"),
            },
            default: ParameterValue::Float(1.0),
            presentation: ParameterPresentation::Logarithmic,
        },
        hex_lattice_contract::ParameterSemantics {
            normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
            ..SMOOTHED_ENVELOPE
        },
        |c| c.rate_hz,
        |c, v| c.rate_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::RIGHT_PHASE,
            label: "right phase",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 360.0,
                unit: Some("deg"),
            },
            default: ParameterValue::Float(180.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.right_phase_degrees,
        |c, v| c.right_phase_degrees = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::WAVEFORM,
            label: "waveform",
            kind: ParameterKind::Choice {
                labels: &["sine", "triangle", "saw up", "saw down", "square", "random"],
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.waveform as u32,
        |c, v| {
            c.waveform = Waveform::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::MODE,
            label: "mode",
            kind: ParameterKind::Choice {
                labels: &["panning", "tremolo"],
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.mode as u32,
        |c, v| {
            c.mode = PanMode::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::RATE_MODE,
            label: "rate mode",
            kind: ParameterKind::Choice {
                labels: &["hz", "sync"],
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.rate_mode as u32,
        |c, v| {
            c.rate_mode = RateMode::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::SYNC_DIVISION,
            label: "sync",
            kind: ParameterKind::Choice {
                labels: SYNC_DIVISION_LABELS,
            },
            default: ParameterValue::Choice(9),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.sync_division.to_choice(),
        |c, v| {
            c.sync_division = SyncDivision::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::OFFSET,
            label: "offset",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 360.0,
                unit: Some("deg"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.offset_degrees,
        |c, v| c.offset_degrees = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::SHAPE,
            label: "shape",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.shape,
        |c, v| c.shape = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::INVERT,
            label: "invert",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_BOOL_ENVELOPE,
        |c| c.invert,
        |c, v| c.invert = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::STEREO_MODE,
            label: "stereo",
            kind: ParameterKind::Choice {
                labels: &["phase", "spin"],
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.stereo_mode as u32,
        |c, v| {
            c.stereo_mode = StereoMode::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::SPIN,
            label: "spin",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.spin,
        |c, v| c.spin = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::MOD_MODE,
            label: "att/mod",
            kind: ParameterKind::Choice {
                labels: &["attenuate", "modulate"],
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.mod_mode as u32,
        |c, v| {
            c.mod_mode = ModMode::from_choice(v)?;
            Ok(())
        },
    ),
];
const DESCRIPTORS: &[ParameterDescriptor] =
    &crate::parameter_binding::descriptors(&BINDINGS, BINDINGS[0].descriptor);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum Waveform {
    Sine = 0,
    Triangle = 1,
    SawUp = 2,
    SawDown = 3,
    Square = 4,
    Random = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum PanMode {
    Panning = 0,
    Tremolo = 1,
}

impl PanMode {
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Panning),
            1 => Ok(Self::Tremolo),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum RateMode {
    Hz = 0,
    Sync = 1,
}

impl RateMode {
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Hz),
            1 => Ok(Self::Sync),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SyncDivision(u8);

impl SyncDivision {
    const DEFAULT: Self = Self(9); // 1/4

    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        if (value as usize) < SYNC_DIVISION_BEATS.len() {
            Ok(Self(value as u8))
        } else {
            Err(DeviceContractError::InvalidParameterValue)
        }
    }

    fn to_choice(self) -> u32 {
        u32::from(self.0)
    }

    fn beats(self) -> f64 {
        SYNC_DIVISION_BEATS[usize::from(self.0)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum StereoMode {
    Phase = 0,
    Spin = 1,
}

impl StereoMode {
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Phase),
            1 => Ok(Self::Spin),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum ModMode {
    Attenuate = 0,
    Modulate = 1,
}

impl ModMode {
    fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        match value {
            0 => Ok(Self::Attenuate),
            1 => Ok(Self::Modulate),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

/// Deterministic splitmix64 step value in `[0.0, 1.0]` for the stepped Random LFO.
fn random_step_value(step_index: u64) -> f64 {
    let mut z = step_index
        .wrapping_add(0x9E37_79B9_7F4A_7C15)
        .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 30)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // Use top 53 bits for uniform IEEE-754 f64 in [0.0, 1.0].
    (z >> 11) as f64 / ((1u64 << 53) - 1) as f64
}

impl Waveform {
    const COUNT: usize = 6;
    const ALL: [Self; Self::COUNT] = [
        Self::Sine,
        Self::Triangle,
        Self::SawUp,
        Self::SawDown,
        Self::Square,
        Self::Random,
    ];
    pub(crate) fn from_choice(value: u32) -> Result<Self, DeviceContractError> {
        Self::ALL
            .get(value as usize)
            .copied()
            .ok_or(DeviceContractError::InvalidParameterValue)
    }
    /// Ideal shape with exact discontinuities (also the preview geometry).
    pub(crate) fn lfo(self, phase: f64) -> f64 {
        self.lfo_with_edge(phase, 0, 0.0)
    }

    /// Shape whose square/saw/random discontinuities are replaced by a linear
    /// transition across the `edge` cycles that end at each edge. Ending the
    /// transition at the edge keeps the edge phases and every value outside
    /// the transition unchanged. `edge` is clamped to `[0, 0.25]`.
    fn lfo_with_edge(self, phase: f64, cycle: u64, edge: f64) -> f64 {
        let edge = edge.clamp(0.0, 0.25);
        // Fraction of the transition completed before the edge at `at`.
        let approach = |at: f64| {
            let before = at - phase;
            (edge > 0.0 && before > 0.0 && before <= edge).then(|| 1.0 - before / edge)
        };
        match self {
            Self::SawUp => match approach(1.0) {
                Some(done) => (1.0 - edge) * (1.0 - done),
                None => phase,
            },
            Self::SawDown => match approach(1.0) {
                Some(done) => 1.0 - (1.0 - edge) * (1.0 - done),
                None => 1.0 - phase,
            },
            Self::Square => match (approach(0.5), approach(1.0)) {
                (Some(done), _) => 1.0 - done,
                (_, Some(done)) => done,
                _ => self.ideal(phase, cycle),
            },
            Self::Random => {
                let steps = RANDOM_STEPS_PER_CYCLE as f64;
                let scaled = phase * steps;
                let sub = (scaled.floor() as usize).min(RANDOM_STEPS_PER_CYCLE - 1);
                let step_index = cycle
                    .wrapping_mul(RANDOM_STEPS_PER_CYCLE as u64)
                    .wrapping_add(sub as u64);
                let current = random_step_value(step_index);
                let step_edge = (edge * steps).clamp(0.0, 0.25);
                let frac = scaled - sub as f64;
                let before = 1.0 - frac;
                if step_edge > 0.0 && before > 0.0 && before <= step_edge {
                    let done = 1.0 - before / step_edge;
                    let next = random_step_value(step_index.wrapping_add(1));
                    current + (next - current) * done
                } else {
                    current
                }
            }
            _ => self.ideal(phase, cycle),
        }
    }

    fn ideal(self, phase: f64, cycle: u64) -> f64 {
        match self {
            Self::Sine => (1.0 + (std::f64::consts::TAU * phase).sin()) * 0.5,
            Self::Triangle => 1.0 - (2.0 * (phase + 0.25).rem_euclid(1.0) - 1.0).abs(),
            Self::SawUp => phase,
            Self::SawDown => 1.0 - phase,
            // Original symmetric square: high on [0,1/2), low on [1/2,1).
            Self::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Random => {
                let sub = ((phase * RANDOM_STEPS_PER_CYCLE as f64).floor() as usize)
                    .min(RANDOM_STEPS_PER_CYCLE - 1);
                let step_index = cycle
                    .wrapping_mul(RANDOM_STEPS_PER_CYCLE as u64)
                    .wrapping_add(sub as u64);
                random_step_value(step_index)
            }
        }
    }
}

/// Apply Live 12's Shape sculpting around 0.5, keeping `[0, 1]` endpoints exact
/// and preserving bit-identical identity when `shape == 0.0`.
#[inline]
fn apply_shape(value: f64, shape: f64) -> f64 {
    if shape == 0.0 {
        return value;
    }
    let centered = 2.0 * value - 1.0;
    let k = 1.0 + shape * 15.0;
    (0.5 + 0.5 * (centered * k).tanh() / k.tanh()).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AutoPanSettings {
    amount: f32,
    rate_hz: f32,
    right_phase_degrees: f32,
    waveform: Waveform,
    mode: PanMode,
    rate_mode: RateMode,
    sync_division: SyncDivision,
    offset_degrees: f32,
    shape: f32,
    invert: bool,
    stereo_mode: StereoMode,
    spin: f32,
    mod_mode: ModMode,
}

impl Default for AutoPanSettings {
    fn default() -> Self {
        Self {
            amount: 0.5,
            rate_hz: 1.0,
            right_phase_degrees: 180.0,
            waveform: Waveform::Sine,
            mode: PanMode::Panning,
            rate_mode: RateMode::Hz,
            sync_division: SyncDivision::DEFAULT,
            offset_degrees: 0.0,
            shape: 0.0,
            invert: false,
            stereo_mode: StereoMode::Phase,
            spin: 0.0,
            mod_mode: ModMode::Attenuate,
        }
    }
}

fn decode(state: &NativeDeviceState) -> Result<AutoPanSettings, DeviceContractError> {
    if state.type_id() != AUTO_PAN_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    let bytes = state.payload();
    let read_f32 = |offset: usize| -> Result<f32, DeviceContractError> {
        let slice: [u8; 4] = bytes
            .get(offset..offset + 4)
            .ok_or(DeviceContractError::MalformedState)?
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(f32::from_le_bytes(slice))
    };
    let read_u32 = |offset: usize| -> Result<u32, DeviceContractError> {
        let slice: [u8; 4] = bytes
            .get(offset..offset + 4)
            .ok_or(DeviceContractError::MalformedState)?
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(u32::from_le_bytes(slice))
    };
    let settings = match state.state_version() {
        1 => {
            if bytes.len() != STATE_BYTES_V1 {
                return Err(DeviceContractError::MalformedState);
            }
            AutoPanSettings {
                amount: read_f32(0)?,
                rate_hz: read_f32(4)?,
                right_phase_degrees: read_f32(8)?,
                ..AutoPanSettings::default()
            }
        }
        2 => {
            if bytes.len() != STATE_BYTES_V2 {
                return Err(DeviceContractError::MalformedState);
            }
            let wave = read_u32(12)?;
            if wave > 4 {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            AutoPanSettings {
                amount: read_f32(0)?,
                rate_hz: read_f32(4)?,
                right_phase_degrees: read_f32(8)?,
                waveform: Waveform::from_choice(wave)?,
                ..AutoPanSettings::default()
            }
        }
        AUTO_PAN_STATE_VERSION => {
            if bytes.len() != STATE_BYTES {
                return Err(DeviceContractError::MalformedState);
            }
            let invert = match read_u32(40)? {
                0 => false,
                1 => true,
                _ => return Err(DeviceContractError::MalformedState),
            };
            AutoPanSettings {
                amount: read_f32(0)?,
                rate_hz: read_f32(4)?,
                right_phase_degrees: read_f32(8)?,
                offset_degrees: read_f32(12)?,
                shape: read_f32(16)?,
                spin: read_f32(20)?,
                waveform: Waveform::from_choice(read_u32(24)?)?,
                mode: PanMode::from_choice(read_u32(28)?)?,
                rate_mode: RateMode::from_choice(read_u32(32)?)?,
                sync_division: SyncDivision::from_choice(read_u32(36)?)?,
                invert,
                stereo_mode: StereoMode::from_choice(read_u32(44)?)?,
                mod_mode: ModMode::from_choice(read_u32(48)?)?,
            }
        }
        _ => return Err(DeviceContractError::UnsupportedStateVersion),
    };
    validate(settings)?;
    Ok(settings)
}

fn validate(settings: AutoPanSettings) -> Result<(), DeviceContractError> {
    for binding in &BINDINGS {
        binding.descriptor.validate(binding.read(&settings))?;
    }
    Ok(())
}

fn encode(settings: AutoPanSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    for value in [
        settings.amount,
        settings.rate_hz,
        settings.right_phase_degrees,
        settings.offset_degrees,
        settings.shape,
        settings.spin,
    ] {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    for tag in [
        settings.waveform as u32,
        settings.mode as u32,
        settings.rate_mode as u32,
        settings.sync_division.to_choice(),
        u32::from(settings.invert),
        settings.stereo_mode as u32,
        settings.mod_mode as u32,
    ] {
        payload.extend_from_slice(&tag.to_le_bytes());
    }
    NativeDeviceState::new(AUTO_PAN_TYPE_ID, AUTO_PAN_STATE_VERSION, payload)
}

/// Registered Lattice Auto Pan-Tremolo metadata, codec, and control-side preparation.
pub struct AutoPanDefinition;
pub static AUTO_PAN_DEFINITION: AutoPanDefinition = AutoPanDefinition;

impl NativeDeviceMetadata for AutoPanDefinition {
    fn type_id(&self) -> DeviceTypeId {
        AUTO_PAN_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("pan")
    }
    fn state_version(&self) -> u32 {
        AUTO_PAN_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        crate::parameter_binding::parameter_semantics_for_state(&BINDINGS, state, id, decode)
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(AutoPanSettings::default())
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
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        crate::parameter_binding::patch_parameter(&BINDINGS, state, id, value, decode, encode)
    }
}

impl NativeDeviceDefinition for AutoPanDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        // The prepared factory contains only inline settings and coefficients.
        Some(crate::source_effect_arc_bytes::<PreparedAutoPan>())
    }
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
        Ok(automation::supported(parameter))
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

/// Closed fixed-inline Auto Pan-Tremolo factory. Envelope storage remains host-owned.
#[derive(Clone, Copy, Debug)]
pub struct PreparedAutoPan {
    settings: AutoPanSettings,
    sample_rate: u32,
    ramp_frames: u32,
    automation_mask: u32,
}

impl PreparedNativeProcessor for PreparedAutoPan {
    fn supports_smoothed_bypass(&self) -> bool {
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
            .then(|| {
                crate::parameter_binding::binding(&BINDINGS, parameter)
                    .ok()
                    .map(|b| b.descriptor)
            })
            .flatten()
    }
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.supports(parameter)
    }

    fn parameter_modulation_semantics(
        &self,
        parameter: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        if !self.supports(parameter) {
            return None;
        }
        Some(
            crate::parameter_binding::binding(&BINDINGS, parameter)
                .ok()?
                .semantics,
        )
    }

    fn runtime_version(&self) -> u32 {
        2
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<AutoPanRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(self))
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
}

impl crate::inline_owned::InlinePreparedProcessor for PreparedAutoPan {
    type Runtime = AutoPanRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        self.check_inline_ownership();
        let targets = AutoPanTargets::from(self.settings);
        let right_phase_cycles = (targets.right_phase_degrees / 360.0).rem_euclid(1.0);
        let runtime = AutoPanRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(
                self.settings,
                std::array::from_fn(|index| self.automation_mask & (1u32 << index) != 0),
            ),
            dsp: AutoPanDspState {
                sample_rate: self.sample_rate,
                ramp_frames: self.ramp_frames,
                phase_cycles: 0.0,
                cycle_count: 0,
                right_phase_cycles,
                right_cycle_count: 0,
                amount: hex_dsp::LinearRamp::settled(targets.amount),
                rate_hz: hex_dsp::LinearRamp::settled(targets.rate_hz),
                right_phase_degrees: hex_dsp::LinearRamp::settled(targets.right_phase_degrees),
                offset_degrees: hex_dsp::LinearRamp::settled(targets.offset_degrees),
                shape: hex_dsp::LinearRamp::settled(targets.shape),
                spin: hex_dsp::LinearRamp::settled(targets.spin),
                tremolo_mix: hex_dsp::LinearRamp::settled(f64::from(
                    targets.mode == PanMode::Tremolo,
                )),
                invert_mix: hex_dsp::LinearRamp::settled(f64::from(targets.invert)),
                mod_mix: hex_dsp::LinearRamp::settled(f64::from(
                    targets.mod_mode == ModMode::Modulate,
                )),
                waveform_weights: std::array::from_fn(|i| {
                    hex_dsp::LinearRamp::settled(f64::from(i == targets.waveform as usize))
                }),
                targets,
            },
        };
        runtime.check_inline_ownership();
        runtime
    }
}

#[derive(Clone, Copy, Debug)]
struct AutoPanTargets {
    amount: f64,
    rate_hz: f64,
    right_phase_degrees: f64,
    waveform: Waveform,
    mode: PanMode,
    rate_mode: RateMode,
    sync_division: SyncDivision,
    offset_degrees: f64,
    shape: f64,
    invert: bool,
    stereo_mode: StereoMode,
    spin: f64,
    mod_mode: ModMode,
}

impl From<AutoPanSettings> for AutoPanTargets {
    fn from(settings: AutoPanSettings) -> Self {
        Self {
            amount: f64::from(settings.amount),
            rate_hz: f64::from(settings.rate_hz),
            right_phase_degrees: normalize_degrees(f64::from(settings.right_phase_degrees)),
            waveform: settings.waveform,
            mode: settings.mode,
            rate_mode: settings.rate_mode,
            sync_division: settings.sync_division,
            offset_degrees: normalize_degrees(f64::from(settings.offset_degrees)),
            shape: f64::from(settings.shape),
            invert: settings.invert,
            stereo_mode: settings.stereo_mode,
            spin: f64::from(settings.spin),
            mod_mode: settings.mod_mode,
        }
    }
}

/// Closed DSP history and effective-control owner, without program or asset ownership.
#[derive(Clone, Copy, Debug)]
pub struct AutoPanRuntime {
    base: PreparedAutoPan,
    controls: crate::parameter_binding::Controls<AutoPanSettings, PARAM_COUNT>,
    dsp: AutoPanDspState,
}

// Keep the frequently accessed DSP history together. The shared
// effective-control cache is sampled only by automated processing and must not
// interleave the ramp/oscillator fields used by ordinary processing.
#[derive(Clone, Copy, Debug)]
struct AutoPanDspState {
    sample_rate: u32,
    ramp_frames: u32,
    phase_cycles: f64,
    cycle_count: u64,
    right_phase_cycles: f64,
    right_cycle_count: u64,
    amount: hex_dsp::LinearRamp,
    rate_hz: hex_dsp::LinearRamp,
    right_phase_degrees: hex_dsp::LinearRamp,
    offset_degrees: hex_dsp::LinearRamp,
    shape: hex_dsp::LinearRamp,
    spin: hex_dsp::LinearRamp,
    tremolo_mix: hex_dsp::LinearRamp,
    invert_mix: hex_dsp::LinearRamp,
    mod_mix: hex_dsp::LinearRamp,
    waveform_weights: [hex_dsp::LinearRamp; Waveform::COUNT],
    targets: AutoPanTargets,
}

fn clock_timing(clock: Option<&dyn NativeInstrumentClock>, offset: usize) -> (f64, Option<f64>) {
    let Some(clock) = clock else {
        return (FALLBACK_TEMPO_BPM, None);
    };
    let bpm = clock
        .tempo_micro_bpm_at_offset(offset)
        .filter(|&micro_bpm| micro_bpm > 0)
        .map_or(FALLBACK_TEMPO_BPM, |micro_bpm| {
            f64::from(micro_bpm) / 1_000_000.0
        });
    let beat = clock
        .beat_at_offset(offset)
        .map(|q32| q32 as f64 / Q32_32_SCALE);
    (bpm, beat)
}

impl AutoPanDspState {
    fn reset_history(&mut self) {
        self.phase_cycles = 0.0;
        self.cycle_count = 0;
        self.right_phase_cycles = (self.targets.right_phase_degrees / 360.0).rem_euclid(1.0);
        self.right_cycle_count = 0;
        self.waveform_weights = std::array::from_fn(|i| {
            hex_dsp::LinearRamp::settled(f64::from(i == self.targets.waveform as usize))
        });
        self.amount = hex_dsp::LinearRamp::settled(self.targets.amount);
        self.rate_hz = hex_dsp::LinearRamp::settled(self.targets.rate_hz);
        self.right_phase_degrees = hex_dsp::LinearRamp::settled(self.targets.right_phase_degrees);
        self.offset_degrees = hex_dsp::LinearRamp::settled(self.targets.offset_degrees);
        self.shape = hex_dsp::LinearRamp::settled(self.targets.shape);
        self.spin = hex_dsp::LinearRamp::settled(self.targets.spin);
        self.tremolo_mix =
            hex_dsp::LinearRamp::settled(f64::from(self.targets.mode == PanMode::Tremolo));
        self.invert_mix = hex_dsp::LinearRamp::settled(f64::from(self.targets.invert));
        self.mod_mix =
            hex_dsp::LinearRamp::settled(f64::from(self.targets.mod_mode == ModMode::Modulate));
    }

    fn process_frame(&mut self, frame: &mut StereoFrame, tempo_bpm: f64, clock_beat: Option<f64>) {
        let left = f64::from(frame[0]);
        let right = f64::from(frame[1]);
        if !left.is_finite() || !right.is_finite() {
            return;
        }

        // Edits advance before this sample; phase is sampled before its clock advances.
        let amount = self.amount.advance();
        let rate_hz = self.rate_hz.advance();
        let right_phase_degrees = self.right_phase_degrees.advance();
        let offset_degrees = self.offset_degrees.advance();
        let shape = self.shape.advance();
        let spin = self.spin.advance();
        let tremolo_mix = self.tremolo_mix.advance();
        let invert_mix = self.invert_mix.advance();
        let mod_mix = self.mod_mix.advance();
        for weight in &mut self.waveform_weights {
            weight.advance();
        }

        let effective_rate_hz = match self.targets.rate_mode {
            RateMode::Hz => rate_hz,
            RateMode::Sync => (tempo_bpm / 60.0) / self.targets.sync_division.beats(),
        };

        let beat_locked = self.targets.rate_mode == RateMode::Sync
            && self.targets.stereo_mode == StereoMode::Phase;
        if beat_locked && let Some(beats) = clock_beat {
            let cycles = beats / self.targets.sync_division.beats();
            let base_cycle = cycles.floor().max(0.0) as u64;
            self.cycle_count = base_cycle;
            self.phase_cycles = cycles.rem_euclid(1.0);
        }

        let (left_phase, left_cycle) = if offset_degrees == 0.0 {
            (self.phase_cycles, self.cycle_count)
        } else {
            let shifted = self.phase_cycles + offset_degrees / 360.0;
            (
                shifted.rem_euclid(1.0),
                self.cycle_count
                    .wrapping_add(shifted.div_euclid(1.0) as i64 as u64),
            )
        };

        if amount != 0.0 && (frame[0].to_bits() | frame[1].to_bits()) != 0 {
            let edge = effective_rate_hz * EDGE_SECONDS;
            let raw_left = self.lfo(left_phase, left_cycle, edge);
            let shaped_left = apply_shape(raw_left, shape);
            let left_lfo = if invert_mix == 0.0 {
                shaped_left
            } else {
                shaped_left * (1.0 - 2.0 * invert_mix) + invert_mix
            };

            let right_lfo = if tremolo_mix == 1.0 {
                left_lfo
            } else {
                let (right_phase, right_cycle) = match self.targets.stereo_mode {
                    StereoMode::Phase => {
                        let shifted = left_phase + right_phase_degrees / 360.0;
                        (
                            shifted.rem_euclid(1.0),
                            left_cycle.wrapping_add(shifted.div_euclid(1.0) as i64 as u64),
                        )
                    }
                    StereoMode::Spin => {
                        if offset_degrees == 0.0 {
                            (self.right_phase_cycles, self.right_cycle_count)
                        } else {
                            let shifted = self.right_phase_cycles + offset_degrees / 360.0;
                            (
                                shifted.rem_euclid(1.0),
                                self.right_cycle_count
                                    .wrapping_add(shifted.div_euclid(1.0) as i64 as u64),
                            )
                        }
                    }
                };
                let right_edge = match self.targets.stereo_mode {
                    StereoMode::Phase => edge,
                    StereoMode::Spin => effective_rate_hz * (1.0 + spin * 0.5) * EDGE_SECONDS,
                };
                let raw_right = self.lfo(right_phase, right_cycle, right_edge);
                let shaped_right = apply_shape(raw_right, shape);
                let panning_right = if invert_mix == 0.0 {
                    shaped_right
                } else {
                    shaped_right * (1.0 - 2.0 * invert_mix) + invert_mix
                };
                if tremolo_mix == 0.0 {
                    panning_right
                } else {
                    panning_right * (1.0 - tremolo_mix) + left_lfo * tremolo_mix
                }
            };

            let base_gain = 1.0 - amount;
            let gain_from_lfo = |lfo: f64| {
                if mod_mix == 0.0 {
                    base_gain + amount * lfo
                } else {
                    let effective_lfo = lfo * (1.0 - mod_mix) + (2.0 * lfo - 1.0) * mod_mix;
                    base_gain + amount * effective_lfo
                }
            };
            frame[0] = (left * gain_from_lfo(left_lfo)) as f32;
            frame[1] = (right * gain_from_lfo(right_lfo)) as f32;
        }

        let next_left = self.phase_cycles + effective_rate_hz / f64::from(self.sample_rate);
        if next_left >= 1.0 {
            self.cycle_count = self.cycle_count.wrapping_add(next_left.floor() as u64);
        }
        self.phase_cycles = next_left.rem_euclid(1.0);
        match self.targets.stereo_mode {
            StereoMode::Spin => {
                let rate_r = effective_rate_hz * (1.0 + spin * 0.5);
                let next_right = self.right_phase_cycles + rate_r / f64::from(self.sample_rate);
                if next_right >= 1.0 {
                    self.right_cycle_count = self
                        .right_cycle_count
                        .wrapping_add(next_right.floor() as u64);
                }
                self.right_phase_cycles = next_right.rem_euclid(1.0);
            }
            StereoMode::Phase => {
                let shifted = self.phase_cycles + right_phase_degrees / 360.0;
                self.right_phase_cycles = shifted.rem_euclid(1.0);
                self.right_cycle_count = self
                    .cycle_count
                    .wrapping_add(shifted.div_euclid(1.0) as i64 as u64);
            }
        }
    }

    fn lfo(&self, phase: f64, cycle: u64, edge: f64) -> f64 {
        // Preserve the original Sine arithmetic and avoid blending when steady.
        if let Some(index) = self
            .waveform_weights
            .iter()
            .position(|weight| weight.value() == 1.0)
        {
            return Waveform::ALL[index].lfo_with_edge(phase, cycle, edge);
        }
        self.waveform_weights
            .iter()
            .zip(Waveform::ALL)
            .filter(|(weight, _)| weight.value() != 0.0)
            .map(|(weight, waveform)| weight.value() * waveform.lfo_with_edge(phase, cycle, edge))
            .sum::<f64>()
            .clamp(0.0, 1.0)
    }

    fn copy_history_and_retarget(&mut self, previous: &Self) {
        self.waveform_weights = previous.waveform_weights;
        self.phase_cycles = previous.phase_cycles;
        self.cycle_count = previous.cycle_count;
        self.right_phase_cycles = previous.right_phase_cycles;
        self.right_cycle_count = previous.right_cycle_count;
        self.amount = previous.amount;
        self.rate_hz = previous.rate_hz;
        self.right_phase_degrees = previous.right_phase_degrees;
        self.offset_degrees = previous.offset_degrees;
        self.shape = previous.shape;
        self.spin = previous.spin;
        self.tremolo_mix = previous.tremolo_mix;
        self.invert_mix = previous.invert_mix;
        self.mod_mix = previous.mod_mix;
        let targets = self.targets;
        self.targets = previous.targets;
        self.retarget(targets);
    }

    // Values were validated by the typed binding before reaching this infallible
    // DSP-only transition. Held values never restart an in-flight ramp.
    fn retarget(&mut self, targets: AutoPanTargets) {
        if targets.waveform != self.targets.waveform {
            for (index, weight) in self.waveform_weights.iter_mut().enumerate() {
                weight.retarget(
                    f64::from(index == targets.waveform as usize),
                    self.ramp_frames,
                );
            }
        }
        if targets.amount != self.targets.amount {
            self.amount.retarget(targets.amount, self.ramp_frames);
        }
        if targets.rate_hz != self.targets.rate_hz {
            self.rate_hz.retarget(targets.rate_hz, self.ramp_frames);
        }
        if targets.right_phase_degrees != self.targets.right_phase_degrees {
            let from = normalize_degrees(self.right_phase_degrees.value());
            // Start in the same wrapped coordinates used to choose the shortest arc.
            self.right_phase_degrees.reset(from);
            let delta = shortest_degrees(from, targets.right_phase_degrees);
            self.right_phase_degrees
                .retarget(from + delta, self.ramp_frames);
        }
        if targets.offset_degrees != self.targets.offset_degrees {
            let from = normalize_degrees(self.offset_degrees.value());
            self.offset_degrees.reset(from);
            let delta = shortest_degrees(from, targets.offset_degrees);
            self.offset_degrees.retarget(from + delta, self.ramp_frames);
        }
        if targets.shape != self.targets.shape {
            self.shape.retarget(targets.shape, self.ramp_frames);
        }
        if targets.spin != self.targets.spin {
            self.spin.retarget(targets.spin, self.ramp_frames);
        }
        if targets.mode != self.targets.mode {
            self.tremolo_mix.retarget(
                f64::from(targets.mode == PanMode::Tremolo),
                self.ramp_frames,
            );
        }
        if targets.invert != self.targets.invert {
            self.invert_mix
                .retarget(f64::from(targets.invert), self.ramp_frames);
        }
        if targets.mod_mode != self.targets.mod_mode {
            self.mod_mix.retarget(
                f64::from(targets.mod_mode == ModMode::Modulate),
                self.ramp_frames,
            );
        }
        self.targets = targets;
    }
}

fn normalize_degrees(degrees: f64) -> f64 {
    degrees.rem_euclid(360.0)
}

fn shortest_degrees(from: f64, to: f64) -> f64 {
    let mut delta = (to - from + 180.0).rem_euclid(360.0) - 180.0;
    if delta <= -180.0 {
        delta += 360.0;
    }
    delta
}

impl NativeProcessorRuntime for AutoPanRuntime {
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.base.supports(parameter)
    }
    crate::parameter_binding::delegate_automated_block_processing!(with_clock process_automated);

    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process_block_with_clock(frames, None, resources);
    }
    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        let sync = self.dsp.targets.rate_mode == RateMode::Sync;
        for (offset, frame) in frames.iter_mut().enumerate() {
            let (tempo_bpm, beat) = if sync {
                clock_timing(clock, offset)
            } else {
                (FALLBACK_TEMPO_BPM, None)
            };
            self.dsp.process_frame(frame, tempo_bpm, beat);
        }
    }
    fn process_bypassed_block(
        &mut self,
        frames: &mut [StereoFrame],
        resources: &dyn hex_lattice::DeviceResources,
    ) {
        self.process_bypassed_block_with_clock(frames, None, resources);
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        let sync = self.dsp.targets.rate_mode == RateMode::Sync;
        for (offset, frame) in frames.iter_mut().enumerate() {
            let original = *frame;
            let (tempo_bpm, beat) = if sync {
                clock_timing(clock, offset)
            } else {
                (FALLBACK_TEMPO_BPM, None)
            };
            self.dsp.process_frame(frame, tempo_bpm, beat);
            *frame = original;
        }
    }
    fn reset(&mut self) {
        self.controls.reset();
        self.dsp.targets = AutoPanTargets::from(self.base.settings);
        self.dsp.reset_history();
    }
    fn is_quiescent(&self) -> bool {
        true
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.dsp.sample_rate == previous.dsp.sample_rate)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|previous| self.dsp.sample_rate == previous.dsp.sample_rate)
        else {
            return;
        };
        if self.base.automation_mask != 0 {
            self.controls.inherit(&previous.controls);
            // Keep the candidate base/admission, but continue the actual effective
            // target and in-flight ramps until the next producer sample.
            self.dsp.targets = previous.dsp.targets;
        }
        self.dsp.copy_history_and_retarget(&previous.dsp);
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.dsp.sample_rate == previous.dsp.sample_rate)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Bounded, sampled steady-target channel gains over one cycle. Not live phase.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoPanWaveformPreview {
    pub left: [[f32; 2]; 129],
    pub right: [[f32; 2]; 129],
}

/// Uses the provider's exact waveform/amount/phase/offset/shape/invert/mode law,
/// never prepares a runtime. Rate changes the time represented by one cycle, not
/// its normalized geometry; the 1 ms square/saw/random edge transition is
/// therefore drawn as an ideal edge.
pub fn waveform_preview(
    state: &NativeDeviceState,
) -> Result<AutoPanWaveformPreview, DeviceContractError> {
    let settings = decode(state)?;
    let offset_cycles = f64::from(settings.offset_degrees) / 360.0;
    let right_offset = match settings.mode {
        PanMode::Tremolo => 0.0,
        PanMode::Panning => f64::from(settings.right_phase_degrees) / 360.0,
    };
    let amount = f64::from(settings.amount);
    let shape = f64::from(settings.shape);
    let channel = |extra_offset: f64| {
        std::array::from_fn(|index| {
            let x = index as f64 / 128.0;
            let phase = (x + offset_cycles + extra_offset).rem_euclid(1.0);
            let mut lfo = apply_shape(settings.waveform.lfo(phase), shape);
            if settings.invert {
                lfo = 1.0 - lfo;
            }
            let y = match settings.mod_mode {
                ModMode::Attenuate => 1.0 - amount + amount * lfo,
                ModMode::Modulate => {
                    let gain = 1.0 - amount + amount * (2.0 * lfo - 1.0);
                    (gain + 1.0) * 0.5
                }
            };
            [x as f32, y as f32]
        })
    };
    Ok(AutoPanWaveformPreview {
        left: channel(0.0),
        right: channel(right_offset),
    })
}

#[cfg(test)]
mod tests;
