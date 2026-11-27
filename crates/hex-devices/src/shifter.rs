//! Original native stereo Shifter (`hex.shifter`) with Pitch, Frequency (SSB),
//! and Ring modulation modes, stereo Wide spread, Tone low-pass, feedback Delay,
//! built-in LFO with Duty Cycle and Phase/Spin stereo, Envelope Follower, Drive,
//! Output gain, and Dry/Wet mix.
//!
//! Architecture & bounds:
//! - Zero algorithmic latency (`0` frames) and bounded finite tail (`2 s` without
//!   delay or `8 s` with delay/automation enabled, flushing to strict quiescence).
//! - Twin preallocated transaction rings (`committed` + `scratch`) for both the
//!   pitch-shifter grain buffer (`<= 150 ms + 16` frames) and tiered feedback
//!   delay line (`64` stub frames when delay is off and unautomated; `2 s + 8`
//!   frames when delay is enabled or delay controls are automated). All heap
//!   storage is allocated once on control via `try_reserve_exact`; callback paths
//!   perform zero allocations, zero locks, `O(dirty_len)` commit/rollback, and
//!   `O(1)` reset.
//! - Frequency and Ring modes reuse `hex_dsp::FrequencyShifter` (16th-order
//!   elliptic Hilbert SSB + quadrature carrier). Pitch mode uses dual Hann-
//!   windowed fractional-delay grains with 4-point Catmull-Rom interpolation.
//! - Tempo-synced Delay and LFO divisions follow `NativeInstrumentClock` with
//!   a deterministic 120 BPM fallback when no clock is supplied.
//! - Full sample-rate automation across all 28 parameters, smoothed bypass, and
//!   authored power-off support.
//!
//! Saved state format:
//! - v1 (`112` bytes): 28 little-endian 4-byte fields in parameter-ID order
//!   (`u32` tags for Choice/Bool parameters, `f32` scalars for Float parameters).
//!
//! This is an independent native implementation; measured Live 12.4.6 reference
//! comparisons, MIDI pitch-tracking host ingress, and macOS listening review
//! remain tracked separately in Pulse #3267.

use hex_dsp::{FrequencyShiftParameters, FrequencyShifter, LinearRamp, ShiftMode};
use hex_lattice::{
    DevicePreparationContext, NativeDeviceDefinition, NativeInstrumentClock,
    NativeParameterAutomation, OutputTail, OwnedProcessorRuntime, PreparedOwnedProcessorRuntime,
    PreparedProcessorFactory, PreparedRuntimeDevice, ProcessorInstanceIdentity, StereoFrame,
    StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;
use std::f64::consts::TAU;
use std::sync::Arc;

pub const SHIFTER_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.shifter");
pub const SHIFTER_STATE_VERSION: u32 = 1;
const PARAM_COUNT: usize = 28;
const STATE_BYTES: usize = PARAM_COUNT * 4;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const RAMP_MS: f64 = 5.0;
const FALLBACK_TEMPO_MICRO_BPM: u32 = 120_000_000;
const PITCH_RING_SECONDS: f64 = 0.250;
const PITCH_RING_GUARD: usize = 32;
const DELAY_RING_SECONDS: usize = 2;
const DELAY_RING_GUARD: usize = 8;
const DELAY_STUB_FRAMES: usize = 64;
const TAIL_SECONDS_BASE: u32 = 2;
const TAIL_SECONDS_DELAY: u32 = 8;
const MAX_LFO_PITCH_ST: f64 = 24.0;
const MAX_LFO_FREQ_HZ: f64 = 2_500.0;
const RANDOM_STEPS_PER_CYCLE: usize = 8;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const MODE: ParameterId = ParameterId::from_static(1);
    pub const COARSE_ST: ParameterId = ParameterId::from_static(2);
    pub const FINE_CT: ParameterId = ParameterId::from_static(3);
    pub const COARSE_HZ: ParameterId = ParameterId::from_static(4);
    pub const FINE_HZ: ParameterId = ParameterId::from_static(5);
    pub const WIDE: ParameterId = ParameterId::from_static(6);
    pub const WINDOW_MS: ParameterId = ParameterId::from_static(7);
    pub const TONE_HZ: ParameterId = ParameterId::from_static(8);
    pub const DELAY_ENABLED: ParameterId = ParameterId::from_static(9);
    pub const DELAY_SYNC: ParameterId = ParameterId::from_static(10);
    pub const DELAY_TIME_MS: ParameterId = ParameterId::from_static(11);
    pub const DELAY_DIVISION: ParameterId = ParameterId::from_static(12);
    pub const FEEDBACK: ParameterId = ParameterId::from_static(13);
    pub const LFO_WAVEFORM: ParameterId = ParameterId::from_static(14);
    pub const LFO_RATE_MODE: ParameterId = ParameterId::from_static(15);
    pub const LFO_RATE_HZ: ParameterId = ParameterId::from_static(16);
    pub const LFO_SYNC_DIVISION: ParameterId = ParameterId::from_static(17);
    pub const LFO_DUTY: ParameterId = ParameterId::from_static(18);
    pub const LFO_STEREO_MODE: ParameterId = ParameterId::from_static(19);
    pub const LFO_STEREO_AMOUNT: ParameterId = ParameterId::from_static(20);
    pub const LFO_AMOUNT: ParameterId = ParameterId::from_static(21);
    pub const ENV_ENABLED: ParameterId = ParameterId::from_static(22);
    pub const ENV_ATTACK_MS: ParameterId = ParameterId::from_static(23);
    pub const ENV_RELEASE_MS: ParameterId = ParameterId::from_static(24);
    pub const ENV_AMOUNT: ParameterId = ParameterId::from_static(25);
    pub const DRIVE_ENABLED: ParameterId = ParameterId::from_static(26);
    pub const OUTPUT_GAIN_DB: ParameterId = ParameterId::from_static(27);
    pub const DRY_WET: ParameterId = ParameterId::from_static(28);
}

const SMOOTHED_ENVELOPE: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        modulation: hex_lattice_contract::ParameterModulation::SampleRate,
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
        ..crate::parameter_binding::SCALAR_ENVELOPE
    };

const LOG_SMOOTHED_ENVELOPE: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
        ..SMOOTHED_ENVELOPE
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

const BINDINGS: [crate::parameter_binding::Binding<ShifterSettings>; PARAM_COUNT] = [
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::MODE,
            label: "mode",
            kind: ParameterKind::Choice {
                labels: &["pitch", "freq", "ring"],
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.mode as u32,
        |c, v| {
            c.mode = ShifterMode::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::COARSE_ST,
            label: "coarse",
            kind: ParameterKind::Float {
                min: -24.0,
                max: 24.0,
                unit: Some("st"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.coarse_st,
        |c, v| c.coarse_st = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::FINE_CT,
            label: "fine",
            kind: ParameterKind::Float {
                min: -50.0,
                max: 50.0,
                unit: Some("ct"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.fine_ct,
        |c, v| c.fine_ct = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::COARSE_HZ,
            label: "coarse",
            kind: ParameterKind::Float {
                min: -5_000.0,
                max: 5_000.0,
                unit: Some("hz"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.coarse_hz,
        |c, v| c.coarse_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::FINE_HZ,
            label: "fine",
            kind: ParameterKind::Float {
                min: -500.0,
                max: 500.0,
                unit: Some("hz"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.fine_hz,
        |c, v| c.fine_hz = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::WIDE,
            label: "wide",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_BOOL_ENVELOPE,
        |c| c.wide,
        |c, v| c.wide = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::WINDOW_MS,
            label: "window",
            kind: ParameterKind::Float {
                min: 1.0,
                max: 100.0,
                unit: Some("ms"),
            },
            default: ParameterValue::Float(10.0),
            presentation: ParameterPresentation::Logarithmic,
        },
        LOG_SMOOTHED_ENVELOPE,
        |c| c.window_ms,
        |c, v| c.window_ms = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::TONE_HZ,
            label: "tone",
            kind: ParameterKind::Float {
                min: 100.0,
                max: 22_000.0,
                unit: Some("hz"),
            },
            default: ParameterValue::Float(22_000.0),
            presentation: ParameterPresentation::Logarithmic,
        },
        LOG_SMOOTHED_ENVELOPE,
        |c| c.tone_hz,
        |c, v| c.tone_hz = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::DELAY_ENABLED,
            label: "delay",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_BOOL_ENVELOPE,
        |c| c.delay_enabled,
        |c, v| c.delay_enabled = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::DELAY_SYNC,
            label: "delay sync",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(true),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_BOOL_ENVELOPE,
        |c| c.delay_sync,
        |c, v| c.delay_sync = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::DELAY_TIME_MS,
            label: "time",
            kind: ParameterKind::Float {
                min: 1.0,
                max: 2_000.0,
                unit: Some("ms"),
            },
            default: ParameterValue::Float(250.0),
            presentation: ParameterPresentation::Logarithmic,
        },
        LOG_SMOOTHED_ENVELOPE,
        |c| c.delay_time_ms,
        |c, v| c.delay_time_ms = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::DELAY_DIVISION,
            label: "div",
            kind: ParameterKind::Choice {
                labels: SYNC_DIVISION_LABELS,
            },
            default: ParameterValue::Choice(9),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.delay_division.to_choice(),
        |c, v| {
            c.delay_division = SyncDivision::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::FEEDBACK,
            label: "feedback",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 0.95,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.feedback,
        |c, v| c.feedback = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::LFO_WAVEFORM,
            label: "lfo wave",
            kind: ParameterKind::Choice {
                labels: &["sine", "tri", "saw up", "saw dn", "square", "s&h"],
            },
            default: ParameterValue::Choice(1),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.lfo_waveform as u32,
        |c, v| {
            c.lfo_waveform = LfoWaveform::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::LFO_RATE_MODE,
            label: "lfo sync",
            kind: ParameterKind::Choice {
                labels: &["hz", "sync"],
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.lfo_rate_mode as u32,
        |c, v| {
            c.lfo_rate_mode = RateMode::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::LFO_RATE_HZ,
            label: "rate",
            kind: ParameterKind::Float {
                min: 0.05,
                max: 40.0,
                unit: Some("hz"),
            },
            default: ParameterValue::Float(0.5),
            presentation: ParameterPresentation::Logarithmic,
        },
        LOG_SMOOTHED_ENVELOPE,
        |c| c.lfo_rate_hz,
        |c, v| c.lfo_rate_hz = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::LFO_SYNC_DIVISION,
            label: "lfo div",
            kind: ParameterKind::Choice {
                labels: SYNC_DIVISION_LABELS,
            },
            default: ParameterValue::Choice(9),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.lfo_sync_division.to_choice(),
        |c, v| {
            c.lfo_sync_division = SyncDivision::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::LFO_DUTY,
            label: "duty c",
            kind: ParameterKind::Float {
                min: -1.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.lfo_duty,
        |c, v| c.lfo_duty = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::LFO_STEREO_MODE,
            label: "stereo",
            kind: ParameterKind::Choice {
                labels: &["phase", "spin"],
            },
            default: ParameterValue::Choice(1),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_CHOICE_ENVELOPE,
        |c| c.lfo_stereo_mode as u32,
        |c, v| {
            c.lfo_stereo_mode = StereoMode::from_choice(v)?;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::LFO_STEREO_AMOUNT,
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
        |c| c.lfo_stereo_amount,
        |c, v| c.lfo_stereo_amount = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::LFO_AMOUNT,
            label: "amount",
            kind: ParameterKind::Float {
                min: 0.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.lfo_amount,
        |c, v| c.lfo_amount = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::ENV_ENABLED,
            label: "env fol",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_BOOL_ENVELOPE,
        |c| c.env_enabled,
        |c, v| c.env_enabled = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::ENV_ATTACK_MS,
            label: "attack",
            kind: ParameterKind::Float {
                min: 0.5,
                max: 500.0,
                unit: Some("ms"),
            },
            default: ParameterValue::Float(6.0),
            presentation: ParameterPresentation::Logarithmic,
        },
        LOG_SMOOTHED_ENVELOPE,
        |c| c.env_attack_ms,
        |c, v| c.env_attack_ms = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::ENV_RELEASE_MS,
            label: "release",
            kind: ParameterKind::Float {
                min: 5.0,
                max: 2_000.0,
                unit: Some("ms"),
            },
            default: ParameterValue::Float(200.0),
            presentation: ParameterPresentation::Logarithmic,
        },
        LOG_SMOOTHED_ENVELOPE,
        |c| c.env_release_ms,
        |c, v| c.env_release_ms = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::ENV_AMOUNT,
            label: "env amt",
            kind: ParameterKind::Float {
                min: -1.0,
                max: 1.0,
                unit: None,
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.env_amount,
        |c, v| c.env_amount = v,
    ),
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::DRIVE_ENABLED,
            label: "drive",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_BOOL_ENVELOPE,
        |c| c.drive_enabled,
        |c, v| c.drive_enabled = v,
    ),
    crate::parameter_binding::Binding::float(
        ParameterDescriptor {
            id: parameter::OUTPUT_GAIN_DB,
            label: "output",
            kind: ParameterKind::Float {
                min: -24.0,
                max: 24.0,
                unit: Some("db"),
            },
            default: ParameterValue::Float(0.0),
            presentation: ParameterPresentation::Linear,
        },
        SMOOTHED_ENVELOPE,
        |c| c.output_gain_db,
        |c, v| c.output_gain_db = v,
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
        SMOOTHED_ENVELOPE,
        |c| c.dry_wet,
        |c, v| c.dry_wet = v,
    ),
];

const DESCRIPTORS: &[ParameterDescriptor] =
    &crate::parameter_binding::descriptors(&BINDINGS, BINDINGS[0].descriptor);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ShifterMode {
    Pitch = 0,
    Freq = 1,
    Ring = 2,
}

impl ShifterMode {
    fn from_choice(v: u32) -> Result<Self, DeviceContractError> {
        match v {
            0 => Ok(Self::Pitch),
            1 => Ok(Self::Freq),
            2 => Ok(Self::Ring),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum LfoWaveform {
    Sine = 0,
    Triangle = 1,
    SawUp = 2,
    SawDown = 3,
    Square = 4,
    SampleAndHold = 5,
}

impl LfoWaveform {
    fn from_choice(v: u32) -> Result<Self, DeviceContractError> {
        match v {
            0 => Ok(Self::Sine),
            1 => Ok(Self::Triangle),
            2 => Ok(Self::SawUp),
            3 => Ok(Self::SawDown),
            4 => Ok(Self::Square),
            5 => Ok(Self::SampleAndHold),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }

    /// Evaluate bipolar LFO shape in `[-1.0, 1.0]` with Duty Cycle symmetry warp.
    fn eval(self, phase: f64, cycle: u64, duty: f64) -> f64 {
        let p = warp_duty(phase.rem_euclid(1.0), duty);
        match self {
            Self::Sine => (TAU * p).sin(),
            Self::Triangle => 1.0 - 4.0 * ((p + 0.25).rem_euclid(1.0) - 0.5).abs(),
            Self::SawUp => 2.0 * p - 1.0,
            Self::SawDown => 1.0 - 2.0 * p,
            Self::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Self::SampleAndHold => {
                let sub = ((p * RANDOM_STEPS_PER_CYCLE as f64).floor() as usize)
                    .min(RANDOM_STEPS_PER_CYCLE - 1);
                let step = cycle
                    .wrapping_mul(RANDOM_STEPS_PER_CYCLE as u64)
                    .wrapping_add(sub as u64);
                random_bipolar(step)
            }
        }
    }
}

#[inline]
fn warp_duty(phase: f64, duty: f64) -> f64 {
    if duty == 0.0 {
        return phase;
    }
    let mid = (0.5 + 0.45 * duty.clamp(-1.0, 1.0)).clamp(0.05, 0.95);
    if phase < mid {
        0.5 * phase / mid
    } else {
        0.5 + 0.5 * (phase - mid) / (1.0 - mid)
    }
}

fn random_bipolar(step_index: u64) -> f64 {
    let mut z = step_index
        .wrapping_add(0x9E37_79B9_7F4A_7C15)
        .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 30)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let unit = (z >> 11) as f64 / ((1u64 << 53) - 1) as f64;
    2.0 * unit - 1.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum RateMode {
    Hz = 0,
    Sync = 1,
}

impl RateMode {
    fn from_choice(v: u32) -> Result<Self, DeviceContractError> {
        match v {
            0 => Ok(Self::Hz),
            1 => Ok(Self::Sync),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum StereoMode {
    Phase = 0,
    Spin = 1,
}

impl StereoMode {
    fn from_choice(v: u32) -> Result<Self, DeviceContractError> {
        match v {
            0 => Ok(Self::Phase),
            1 => Ok(Self::Spin),
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SyncDivision(u8);

impl SyncDivision {
    const DEFAULT: Self = Self(9); // 1/4

    fn from_choice(v: u32) -> Result<Self, DeviceContractError> {
        if (v as usize) < SYNC_DIVISION_BEATS.len() {
            Ok(Self(v as u8))
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct ShifterSettings {
    mode: ShifterMode,
    coarse_st: f32,
    fine_ct: f32,
    coarse_hz: f32,
    fine_hz: f32,
    wide: bool,
    window_ms: f32,
    tone_hz: f32,
    delay_enabled: bool,
    delay_sync: bool,
    delay_time_ms: f32,
    delay_division: SyncDivision,
    feedback: f32,
    lfo_waveform: LfoWaveform,
    lfo_rate_mode: RateMode,
    lfo_rate_hz: f32,
    lfo_sync_division: SyncDivision,
    lfo_duty: f32,
    lfo_stereo_mode: StereoMode,
    lfo_stereo_amount: f32,
    lfo_amount: f32,
    env_enabled: bool,
    env_attack_ms: f32,
    env_release_ms: f32,
    env_amount: f32,
    drive_enabled: bool,
    output_gain_db: f32,
    dry_wet: f32,
}

impl Default for ShifterSettings {
    fn default() -> Self {
        Self {
            mode: ShifterMode::Pitch,
            coarse_st: 0.0,
            fine_ct: 0.0,
            coarse_hz: 0.0,
            fine_hz: 0.0,
            wide: false,
            window_ms: 10.0,
            tone_hz: 22_000.0,
            delay_enabled: false,
            delay_sync: true,
            delay_time_ms: 250.0,
            delay_division: SyncDivision::DEFAULT,
            feedback: 0.0,
            lfo_waveform: LfoWaveform::Triangle,
            lfo_rate_mode: RateMode::Hz,
            lfo_rate_hz: 0.5,
            lfo_sync_division: SyncDivision::DEFAULT,
            lfo_duty: 0.0,
            lfo_stereo_mode: StereoMode::Spin,
            lfo_stereo_amount: 0.0,
            lfo_amount: 0.0,
            env_enabled: false,
            env_attack_ms: 6.0,
            env_release_ms: 200.0,
            env_amount: 0.0,
            drive_enabled: false,
            output_gain_db: 0.0,
            dry_wet: 1.0,
        }
    }
}

fn validate(settings: ShifterSettings) -> Result<(), DeviceContractError> {
    for binding in &BINDINGS {
        binding.descriptor.validate(binding.read(&settings))?;
    }
    Ok(())
}

fn decode(state: &NativeDeviceState) -> Result<ShifterSettings, DeviceContractError> {
    if state.type_id() != SHIFTER_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != SHIFTER_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let bytes = state.payload();
    if bytes.len() != STATE_BYTES {
        return Err(DeviceContractError::MalformedState);
    }
    let read_f32 = |index: usize| -> Result<f32, DeviceContractError> {
        let off = index * 4;
        let slice: [u8; 4] = bytes
            .get(off..off + 4)
            .ok_or(DeviceContractError::MalformedState)?
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(f32::from_le_bytes(slice))
    };
    let read_u32 = |index: usize| -> Result<u32, DeviceContractError> {
        let off = index * 4;
        let slice: [u8; 4] = bytes
            .get(off..off + 4)
            .ok_or(DeviceContractError::MalformedState)?
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(u32::from_le_bytes(slice))
    };
    let read_bool = |index: usize| -> Result<bool, DeviceContractError> {
        match read_u32(index)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DeviceContractError::MalformedState),
        }
    };
    let settings = ShifterSettings {
        mode: ShifterMode::from_choice(read_u32(0)?)?,
        coarse_st: read_f32(1)?,
        fine_ct: read_f32(2)?,
        coarse_hz: read_f32(3)?,
        fine_hz: read_f32(4)?,
        wide: read_bool(5)?,
        window_ms: read_f32(6)?,
        tone_hz: read_f32(7)?,
        delay_enabled: read_bool(8)?,
        delay_sync: read_bool(9)?,
        delay_time_ms: read_f32(10)?,
        delay_division: SyncDivision::from_choice(read_u32(11)?)?,
        feedback: read_f32(12)?,
        lfo_waveform: LfoWaveform::from_choice(read_u32(13)?)?,
        lfo_rate_mode: RateMode::from_choice(read_u32(14)?)?,
        lfo_rate_hz: read_f32(15)?,
        lfo_sync_division: SyncDivision::from_choice(read_u32(16)?)?,
        lfo_duty: read_f32(17)?,
        lfo_stereo_mode: StereoMode::from_choice(read_u32(18)?)?,
        lfo_stereo_amount: read_f32(19)?,
        lfo_amount: read_f32(20)?,
        env_enabled: read_bool(21)?,
        env_attack_ms: read_f32(22)?,
        env_release_ms: read_f32(23)?,
        env_amount: read_f32(24)?,
        drive_enabled: read_bool(25)?,
        output_gain_db: read_f32(26)?,
        dry_wet: read_f32(27)?,
    };
    validate(settings)?;
    Ok(settings)
}

fn encode(settings: ShifterSettings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let mut payload = Vec::with_capacity(STATE_BYTES);
    let words: [[u8; 4]; PARAM_COUNT] = [
        (settings.mode as u32).to_le_bytes(),
        settings.coarse_st.to_le_bytes(),
        settings.fine_ct.to_le_bytes(),
        settings.coarse_hz.to_le_bytes(),
        settings.fine_hz.to_le_bytes(),
        u32::from(settings.wide).to_le_bytes(),
        settings.window_ms.to_le_bytes(),
        settings.tone_hz.to_le_bytes(),
        u32::from(settings.delay_enabled).to_le_bytes(),
        u32::from(settings.delay_sync).to_le_bytes(),
        settings.delay_time_ms.to_le_bytes(),
        settings.delay_division.to_choice().to_le_bytes(),
        settings.feedback.to_le_bytes(),
        (settings.lfo_waveform as u32).to_le_bytes(),
        (settings.lfo_rate_mode as u32).to_le_bytes(),
        settings.lfo_rate_hz.to_le_bytes(),
        settings.lfo_sync_division.to_choice().to_le_bytes(),
        settings.lfo_duty.to_le_bytes(),
        (settings.lfo_stereo_mode as u32).to_le_bytes(),
        settings.lfo_stereo_amount.to_le_bytes(),
        settings.lfo_amount.to_le_bytes(),
        u32::from(settings.env_enabled).to_le_bytes(),
        settings.env_attack_ms.to_le_bytes(),
        settings.env_release_ms.to_le_bytes(),
        settings.env_amount.to_le_bytes(),
        u32::from(settings.drive_enabled).to_le_bytes(),
        settings.output_gain_db.to_le_bytes(),
        settings.dry_wet.to_le_bytes(),
    ];
    for word in words {
        payload.extend_from_slice(&word);
    }
    NativeDeviceState::new(SHIFTER_TYPE_ID, SHIFTER_STATE_VERSION, payload)
}

fn parameter_slot(id: ParameterId) -> Option<usize> {
    id.value()
        .checked_sub(1)
        .map(|i| i as usize)
        .filter(|&i| i < PARAM_COUNT)
}

/// Control-side LFO curve preview computed directly from the provider's exact
/// waveform, Duty Cycle, stereo Phase/Spin, and LFO Amount laws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShifterLfoPreview {
    pub left: [[f32; 2]; 129],
    pub right: [[f32; 2]; 129],
}

/// Compute the provider-owned LFO preview for a validated `hex.shifter` state.
pub fn lfo_preview(state: &NativeDeviceState) -> Result<ShifterLfoPreview, DeviceContractError> {
    let settings = decode(state)?;
    const SAMPLES: usize = 129;
    let duty = f64::from(settings.lfo_duty);
    let amount = f64::from(settings.lfo_amount);
    let stereo = f64::from(settings.lfo_stereo_amount);
    let mut left = [[0.0_f32; 2]; SAMPLES];
    let mut right = [[0.0_f32; 2]; SAMPLES];
    for i in 0..SAMPLES {
        let x = i as f64 / (SAMPLES - 1) as f64;
        let phase_l = match settings.lfo_stereo_mode {
            StereoMode::Phase => x,
            StereoMode::Spin => (x * (1.0 + 0.5 * stereo)).rem_euclid(1.0),
        };
        let phase_r = match settings.lfo_stereo_mode {
            StereoMode::Phase => (x + stereo).rem_euclid(1.0),
            StereoMode::Spin => (x * (1.0 - 0.5 * stereo).max(0.1)).rem_euclid(1.0),
        };
        let yl = 0.5 + 0.45 * amount * settings.lfo_waveform.eval(phase_l, 0, duty);
        let yr = 0.5 + 0.45 * amount * settings.lfo_waveform.eval(phase_r, 0, duty);
        left[i] = [x as f32, yl.clamp(0.0, 1.0) as f32];
        right[i] = [x as f32, yr.clamp(0.0, 1.0) as f32];
    }
    Ok(ShifterLfoPreview { left, right })
}

/// Registered native stereo Shifter definition.
pub struct ShifterDefinition;
pub static SHIFTER_DEFINITION: ShifterDefinition = ShifterDefinition;

impl ShifterDefinition {
    /// Prepare the owned factory without allocating signal rings on control.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedShifter, DeviceContractError> {
        state.require_asset_free_processing()?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
            || parameters.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = decode(state)?;
        let mut admitted = [false; PARAM_COUNT];
        for &id in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, id)?;
            binding.semantics.validate(binding.descriptor)?;
            let slot = parameter_slot(id).ok_or(DeviceContractError::UnknownParameter)?;
            admitted[slot] = true;
        }
        Ok(PreparedShifter {
            settings,
            rate: sample_rate,
            admitted,
        })
    }
}

impl NativeDeviceMetadata for ShifterDefinition {
    fn type_id(&self) -> DeviceTypeId {
        SHIFTER_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("shifter")
    }
    fn state_version(&self) -> u32 {
        SHIFTER_STATE_VERSION
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
        encode(ShifterSettings::default())
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

impl NativeDeviceDefinition for ShifterDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        Some(crate::source_effect_arc_bytes::<PreparedShifter>())
    }
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode(state)?;
        Ok(parameter_slot(id).is_some())
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<PreparedRuntimeDevice, DeviceContractError> {
        let factory = Self::prepare_owned(state, context.sample_rate, parameters)?;
        Ok(PreparedRuntimeDevice {
            matrix: StereoMatrix::identity(),
            processor: Some(PreparedProcessorFactory::Owned(Arc::new(factory))),
        })
    }
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        decode(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        _context: &DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedNativeProcessor>>, DeviceContractError> {
        decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(PreparedProcessorFactory::Owned(Arc::new(
            Self::prepare_owned(state, context.sample_rate, &[])?,
        ))))
    }
}

/// Control-prepared Shifter factory with bounded inline parameters.
#[derive(Clone, Copy, Debug)]
pub struct PreparedShifter {
    settings: ShifterSettings,
    rate: u32,
    admitted: [bool; PARAM_COUNT],
}

impl PreparedShifter {
    fn pitch_ring_len(&self) -> usize {
        ((f64::from(self.rate) * PITCH_RING_SECONDS).ceil() as usize) + PITCH_RING_GUARD
    }

    fn delay_capable(&self) -> bool {
        let delay_slots = [
            parameter::DELAY_ENABLED,
            parameter::DELAY_SYNC,
            parameter::DELAY_TIME_MS,
            parameter::DELAY_DIVISION,
            parameter::FEEDBACK,
        ];
        self.settings.delay_enabled
            || delay_slots
                .into_iter()
                .any(|id| parameter_slot(id).is_some_and(|i| self.admitted[i]))
    }

    fn delay_ring_len(&self) -> usize {
        if self.delay_capable() {
            (self.rate as usize) * DELAY_RING_SECONDS + DELAY_RING_GUARD
        } else {
            DELAY_STUB_FRAMES
        }
    }

    fn tail_limit(&self) -> u32 {
        let secs = if self.delay_capable() {
            TAIL_SECONDS_DELAY
        } else {
            TAIL_SECONDS_BASE
        };
        self.rate.saturating_mul(secs)
    }
}

impl PreparedOwnedProcessorRuntime for PreparedShifter {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        parameter_slot(id).is_some_and(|i| self.admitted[i])
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        self.supports_parameter_automation(id)
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
        self.supports_parameter_automation(id)
            .then(|| {
                crate::parameter_binding::binding(&BINDINGS, id)
                    .ok()
                    .map(|b| b.semantics)
            })
            .flatten()
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
    fn declared_pair_bytes(&self) -> usize {
        std::mem::size_of::<ShifterRuntime>()
            + (self.pitch_ring_len() + self.delay_ring_len())
                * 2
                * std::mem::size_of::<StereoFrame>()
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        let alloc_ring = |len: usize| -> Result<Vec<StereoFrame>, DeviceContractError> {
            let mut ring = Vec::new();
            ring.try_reserve_exact(len)
                .map_err(|_| DeviceContractError::RuntimeAllocation)?;
            ring.resize(len, [0.0; 2]);
            Ok(ring)
        };
        let pitch_len = self.pitch_ring_len();
        let delay_len = self.delay_ring_len();
        let scalar = ShifterScalar::new(
            self.settings,
            self.rate,
            self.admitted,
            self.tail_limit(),
            delay_len,
        )?;
        let runtime = ShifterRuntime {
            settings: self.settings,
            rate: self.rate,
            tail_limit: self.tail_limit(),
            pitch_committed: alloc_ring(pitch_len)?,
            pitch_scratch: alloc_ring(pitch_len)?,
            delay_committed: alloc_ring(delay_len)?,
            delay_scratch: alloc_ring(delay_len)?,
            primary: scalar,
            speculative: scalar,
            pitch_dirty_start: 0,
            pitch_dirty_len: 0,
            delay_dirty_start: 0,
            delay_dirty_len: 0,
            open: false,
            pristine: true,
        };
        if runtime.fixed_storage_pair_bytes() != Some(self.declared_pair_bytes()) {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        Ok(Box::new(runtime))
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(self.tail_limit()),
        }
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
}

#[derive(Clone, Copy, Debug)]
struct ShifterScalar {
    controls: crate::parameter_binding::Controls<ShifterSettings, PARAM_COUNT>,
    targets: ShifterSettings,
    freq_shifter: FrequencyShifter,
    pitch_next: usize,
    pitch_valid: usize,
    pitch_grain_phase: [[f64; 2]; 2],
    pitch_grain_delay: [[f64; 2]; 2],
    delay_next: usize,
    delay_valid: usize,
    delay_frames_smooth: f64,
    lfo_phase: [f64; 2],
    lfo_cycle: [u64; 2],
    env_level: f64,
    tone_state: [f64; 2],
    tempo_micro_bpm: u32,
    silent_frames: u32,
    coarse_st: LinearRamp,
    fine_ct: LinearRamp,
    coarse_hz: LinearRamp,
    fine_hz: LinearRamp,
    window_ms: LinearRamp,
    tone_hz: LinearRamp,
    feedback: LinearRamp,
    lfo_rate_hz: LinearRamp,
    lfo_duty: LinearRamp,
    lfo_stereo_amount: LinearRamp,
    lfo_amount: LinearRamp,
    env_amount: LinearRamp,
    output_gain_db: LinearRamp,
    dry_wet: LinearRamp,
}

impl ShifterScalar {
    fn new(
        settings: ShifterSettings,
        rate: u32,
        admitted: [bool; PARAM_COUNT],
        tail_limit: u32,
        delay_len: usize,
    ) -> Result<Self, DeviceContractError> {
        let freq_mode = match settings.mode {
            ShifterMode::Ring => ShiftMode::Ring,
            ShifterMode::Pitch | ShifterMode::Freq => ShiftMode::Frequency,
        };
        let freq_shifter = FrequencyShifter::new(
            rate,
            FrequencyShiftParameters {
                mode: freq_mode,
                shift_hz: 0.0,
                stereo_spread_hz: 0.0,
                stereo_phase_degrees: 0.0,
                dry_wet: 1.0,
            },
        )
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let initial_delay = delay_target_frames(
            settings,
            rate,
            FALLBACK_TEMPO_MICRO_BPM,
            delay_len.saturating_sub(4) as f64,
        );
        Ok(Self {
            controls: crate::parameter_binding::Controls::new(settings, admitted),
            targets: settings,
            freq_shifter,
            pitch_next: 0,
            pitch_valid: 0,
            pitch_grain_phase: [[0.0, 0.5], [0.0, 0.5]],
            pitch_grain_delay: [[4.0, 4.0], [4.0, 4.0]],
            delay_next: 0,
            delay_valid: 0,
            delay_frames_smooth: initial_delay,
            lfo_phase: [0.0; 2],
            lfo_cycle: [0; 2],
            env_level: 0.0,
            tone_state: [0.0; 2],
            tempo_micro_bpm: FALLBACK_TEMPO_MICRO_BPM,
            silent_frames: tail_limit,
            coarse_st: LinearRamp::settled(f64::from(settings.coarse_st)),
            fine_ct: LinearRamp::settled(f64::from(settings.fine_ct)),
            coarse_hz: LinearRamp::settled(f64::from(settings.coarse_hz)),
            fine_hz: LinearRamp::settled(f64::from(settings.fine_hz)),
            window_ms: LinearRamp::settled(f64::from(settings.window_ms)),
            tone_hz: LinearRamp::settled(f64::from(settings.tone_hz)),
            feedback: LinearRamp::settled(f64::from(settings.feedback)),
            lfo_rate_hz: LinearRamp::settled(f64::from(settings.lfo_rate_hz)),
            lfo_duty: LinearRamp::settled(f64::from(settings.lfo_duty)),
            lfo_stereo_amount: LinearRamp::settled(f64::from(settings.lfo_stereo_amount)),
            lfo_amount: LinearRamp::settled(f64::from(settings.lfo_amount)),
            env_amount: LinearRamp::settled(f64::from(settings.env_amount)),
            output_gain_db: LinearRamp::settled(f64::from(settings.output_gain_db)),
            dry_wet: LinearRamp::settled(f64::from(settings.dry_wet)),
        })
    }

    fn is_quiescent(&self, tail_limit: u32) -> bool {
        (self.silent_frames >= tail_limit
            || (self.pitch_valid == 0 && self.delay_valid == 0 && self.freq_shifter.is_quiescent()))
            && self.tone_state == [0.0; 2]
            && self.env_level == 0.0
    }

    fn retarget(&mut self, next: ShifterSettings, ramp_frames: u32) {
        if next.mode != self.targets.mode {
            let freq_mode = match next.mode {
                ShifterMode::Ring => ShiftMode::Ring,
                ShifterMode::Pitch | ShifterMode::Freq => ShiftMode::Frequency,
            };
            let _ = self.freq_shifter.set_parameters(FrequencyShiftParameters {
                mode: freq_mode,
                shift_hz: 0.0,
                stereo_spread_hz: 0.0,
                stereo_phase_degrees: 0.0,
                dry_wet: 1.0,
            });
        }
        if next.coarse_st != self.targets.coarse_st {
            self.coarse_st
                .retarget(f64::from(next.coarse_st), ramp_frames);
        }
        if next.fine_ct != self.targets.fine_ct {
            self.fine_ct.retarget(f64::from(next.fine_ct), ramp_frames);
        }
        if next.coarse_hz != self.targets.coarse_hz {
            self.coarse_hz
                .retarget(f64::from(next.coarse_hz), ramp_frames);
        }
        if next.fine_hz != self.targets.fine_hz {
            self.fine_hz.retarget(f64::from(next.fine_hz), ramp_frames);
        }
        if next.window_ms != self.targets.window_ms {
            self.window_ms
                .retarget(f64::from(next.window_ms), ramp_frames);
        }
        if next.tone_hz != self.targets.tone_hz {
            self.tone_hz.retarget(f64::from(next.tone_hz), ramp_frames);
        }
        if next.feedback != self.targets.feedback {
            self.feedback
                .retarget(f64::from(next.feedback), ramp_frames);
        }
        if next.lfo_rate_hz != self.targets.lfo_rate_hz {
            self.lfo_rate_hz
                .retarget(f64::from(next.lfo_rate_hz), ramp_frames);
        }
        if next.lfo_duty != self.targets.lfo_duty {
            self.lfo_duty
                .retarget(f64::from(next.lfo_duty), ramp_frames);
        }
        if next.lfo_stereo_amount != self.targets.lfo_stereo_amount {
            self.lfo_stereo_amount
                .retarget(f64::from(next.lfo_stereo_amount), ramp_frames);
        }
        if next.lfo_amount != self.targets.lfo_amount {
            self.lfo_amount
                .retarget(f64::from(next.lfo_amount), ramp_frames);
        }
        if next.env_amount != self.targets.env_amount {
            self.env_amount
                .retarget(f64::from(next.env_amount), ramp_frames);
        }
        if next.output_gain_db != self.targets.output_gain_db {
            self.output_gain_db
                .retarget(f64::from(next.output_gain_db), ramp_frames);
        }
        if next.dry_wet != self.targets.dry_wet {
            self.dry_wet.retarget(f64::from(next.dry_wet), ramp_frames);
        }
        self.targets = next;
    }
}

fn delay_target_frames(
    settings: ShifterSettings,
    rate: u32,
    tempo_micro_bpm: u32,
    max_delay_frames: f64,
) -> f64 {
    let sr = f64::from(rate);
    let seconds = if settings.delay_sync {
        let bpm = (f64::from(tempo_micro_bpm.max(1)) / 1_000_000.0).clamp(20.0, 999.0);
        (60.0 / bpm) * settings.delay_division.beats()
    } else {
        f64::from(settings.delay_time_ms) * 0.001
    };
    (seconds * sr).clamp(1.0, max_delay_frames.max(1.0))
}

#[inline]
fn catmull_rom(y0: f64, y1: f64, y2: f64, y3: f64, t: f64) -> f64 {
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * t + b) * t + c) * t + y1
}

#[inline]
fn read_ring_cubic(
    ring: &[StereoFrame],
    write_pos: usize,
    valid: usize,
    delay_frames: f64,
    channel: usize,
) -> f64 {
    let len = ring.len();
    let max_d = (len.saturating_sub(4)).max(1) as f64;
    let d = delay_frames.clamp(1.0, max_d);
    let base = d.floor() as usize;
    let frac = d - base as f64;
    let sample_at = |lag: usize| -> f64 {
        if lag == 0 || lag > valid {
            return 0.0;
        }
        let idx = (write_pos + len - (lag % len)) % len;
        f64::from(ring[idx][channel])
    };
    let y0 = sample_at(base.saturating_sub(1));
    let y1 = sample_at(base);
    let y2 = sample_at(base + 1);
    let y3 = sample_at(base + 2);
    catmull_rom(y0, y1, y2, y3, frac)
}

#[derive(Clone, Copy)]
struct PhaseAlignQuery {
    write_pos: usize,
    valid: usize,
    base_start: f64,
    ref_delay: f64,
    max_search: usize,
    stride: usize,
    channel: usize,
}

/// Find the bounded delay offset `off in 0..=max_search` minimizing waveform MSE
/// between `base_start + off` and `ref_delay` so overlapping pitch grains crossfade
/// in phase without comb-filtering cancellation. Zero heap allocations.
fn find_phase_aligned_offset(ring: &[StereoFrame], query: PhaseAlignQuery) -> f64 {
    let PhaseAlignQuery {
        write_pos,
        valid,
        base_start,
        ref_delay,
        max_search,
        stride,
        channel,
    } = query;
    if max_search == 0 || valid < 32 {
        return 0.0;
    }
    const TAPS: usize = 16;
    let len = ring.len();
    let max_d = len.saturating_sub(4);
    let sample_int = |lag: usize| -> f64 {
        if lag == 0 || lag > valid || lag > max_d {
            return 0.0;
        }
        let idx = (write_pos + len - (lag % len)) % len;
        f64::from(ring[idx][channel])
    };

    let ref_base = (ref_delay.round() as usize).clamp(1, max_d);
    let mut ref_win = [0.0_f64; TAPS];
    let mut ref_energy = 0.0_f64;
    for (j, slot) in ref_win.iter_mut().enumerate() {
        let s = sample_int(ref_base + j * stride);
        *slot = s;
        ref_energy += s * s;
    }
    if ref_energy < 1e-10 {
        return 0.0;
    }

    let start_int = (base_start.round() as usize).clamp(1, max_d);
    let eval_err = |off: usize| -> f64 {
        let cand_base = start_int + off;
        let mut err = 0.0_f64;
        for (j, &rv) in ref_win.iter().enumerate() {
            let diff = sample_int(cand_base + j * stride) - rv;
            err += diff * diff;
        }
        err
    };

    let coarse_step = 4_usize;
    let mut best_off = 0_usize;
    let mut best_err = f64::INFINITY;
    let mut off = 0_usize;
    while off <= max_search {
        let err = eval_err(off);
        if err < best_err {
            best_err = err;
            best_off = off;
        }
        off += coarse_step;
    }
    let refine_lo = best_off.saturating_sub(coarse_step - 1);
    let refine_hi = (best_off + coarse_step - 1).min(max_search);
    for cand in refine_lo..=refine_hi {
        let err = eval_err(cand);
        if err < best_err {
            best_err = err;
            best_off = cand;
        }
    }
    best_off as f64
}

/// Bounded transactional stereo Shifter runtime owning fixed preallocated rings.
pub struct ShifterRuntime {
    settings: ShifterSettings,
    rate: u32,
    tail_limit: u32,
    pitch_committed: Vec<StereoFrame>,
    pitch_scratch: Vec<StereoFrame>,
    delay_committed: Vec<StereoFrame>,
    delay_scratch: Vec<StereoFrame>,
    primary: ShifterScalar,
    speculative: ShifterScalar,
    pitch_dirty_start: usize,
    pitch_dirty_len: usize,
    delay_dirty_start: usize,
    delay_dirty_len: usize,
    open: bool,
    pristine: bool,
}

impl ShifterRuntime {
    /// Control-side exact storage observation matching [`PreparedOwnedProcessorRuntime::declared_pair_bytes`].
    pub fn control_storage(&self) -> Option<(usize, [usize; 2])> {
        let bytes = |ring: &Vec<StereoFrame>| {
            ring.capacity()
                .checked_mul(std::mem::size_of::<StereoFrame>())
        };
        let committed = bytes(&self.pitch_committed)?.checked_add(bytes(&self.delay_committed)?)?;
        let scratch = bytes(&self.pitch_scratch)?.checked_add(bytes(&self.delay_scratch)?)?;
        Some((std::mem::size_of::<Self>(), [committed, scratch]))
    }

    fn copy_interval(src: &[StereoFrame], dst: &mut [StereoFrame], start: usize, len: usize) {
        let n = len.min(src.len());
        if n == 0 {
            return;
        }
        let first = n.min(src.len() - start);
        dst[start..start + first].copy_from_slice(&src[start..start + first]);
        if first < n {
            let rest = n - first;
            dst[..rest].copy_from_slice(&src[..rest]);
        }
    }

    fn ramp_frames(&self) -> u32 {
        hex_dsp::ramp_frames_for_ms(self.rate, RAMP_MS).unwrap_or(1)
    }

    fn process_frame(&mut self, frame: &mut StereoFrame, tempo: Option<u32>, bypassed: bool) {
        let s = &mut self.speculative;
        if let Some(t) = tempo.filter(|v| *v > 0) {
            s.tempo_micro_bpm = t;
        }
        let sr = f64::from(self.rate);

        let coarse_st = s.coarse_st.advance();
        let fine_ct = s.fine_ct.advance();
        let coarse_hz = s.coarse_hz.advance();
        let fine_hz = s.fine_hz.advance();
        let window_ms = s.window_ms.advance();
        let tone_hz = s.tone_hz.advance();
        let feedback = s.feedback.advance().clamp(0.0, 0.95);
        let lfo_rate_hz = s.lfo_rate_hz.advance();
        let lfo_duty = s.lfo_duty.advance();
        let lfo_stereo = s.lfo_stereo_amount.advance();
        let lfo_amount = s.lfo_amount.advance();
        let env_amount = s.env_amount.advance();
        let output_gain_db = s.output_gain_db.advance();
        let dry_wet = s.dry_wet.advance();

        let targets = s.targets;

        // Advance LFO phases deterministically even on silence so tempo/free phase stays coherent.
        let base_lfo_hz = match targets.lfo_rate_mode {
            RateMode::Hz => lfo_rate_hz,
            RateMode::Sync => {
                let bpm = (f64::from(s.tempo_micro_bpm.max(1)) / 1_000_000.0).clamp(20.0, 999.0);
                (bpm / 60.0) / targets.lfo_sync_division.beats()
            }
        };
        let (inc_l, inc_r) = match targets.lfo_stereo_mode {
            StereoMode::Phase => (base_lfo_hz / sr, base_lfo_hz / sr),
            StereoMode::Spin => (
                base_lfo_hz * (1.0 + 0.5 * lfo_stereo) / sr,
                base_lfo_hz * (1.0 - 0.5 * lfo_stereo).max(0.1) / sr,
            ),
        };
        let phase_l = s.lfo_phase[0];
        let phase_r = match targets.lfo_stereo_mode {
            StereoMode::Phase => (s.lfo_phase[0] + lfo_stereo).rem_euclid(1.0),
            StereoMode::Spin => s.lfo_phase[1],
        };
        let cycle_l = s.lfo_cycle[0];
        let cycle_r = match targets.lfo_stereo_mode {
            StereoMode::Phase => s.lfo_cycle[0],
            StereoMode::Spin => s.lfo_cycle[1],
        };
        let next_l = s.lfo_phase[0] + inc_l;
        if next_l >= 1.0 {
            let wraps = next_l.floor() as u64;
            s.lfo_phase[0] = next_l - wraps as f64;
            s.lfo_cycle[0] = s.lfo_cycle[0].wrapping_add(wraps);
        } else {
            s.lfo_phase[0] = next_l;
        }
        let next_r = s.lfo_phase[1] + inc_r;
        if next_r >= 1.0 {
            let wraps = next_r.floor() as u64;
            s.lfo_phase[1] = next_r - wraps as f64;
            s.lfo_cycle[1] = s.lfo_cycle[1].wrapping_add(wraps);
        } else {
            s.lfo_phase[1] = next_r;
        }

        let in_l = f64::from(frame[0]);
        let in_r = f64::from(frame[1]);
        if !in_l.is_finite() || !in_r.is_finite() {
            return;
        }

        let silent = frame[0].to_bits() == 0 && frame[1].to_bits() == 0;
        if silent {
            s.silent_frames = s.silent_frames.saturating_add(1).min(self.tail_limit);
        } else {
            s.silent_frames = 0;
        }

        if silent && s.is_quiescent(self.tail_limit) {
            return;
        }

        // Envelope follower (peak detector with independent attack/release).
        let peak = in_l.abs().max(in_r.abs());
        let env_coeff = if peak > s.env_level {
            hex_dsp::one_pole_pole(self.rate, f64::from(targets.env_attack_ms) * 0.001)
                .unwrap_or(0.0)
        } else {
            hex_dsp::one_pole_pole(self.rate, f64::from(targets.env_release_ms) * 0.001)
                .unwrap_or(0.0)
        };
        s.env_level = hex_dsp::flush_quiet(peak + env_coeff * (s.env_level - peak));
        let env_mod = if targets.env_enabled {
            s.env_level.min(2.0) * env_amount
        } else {
            0.0
        };

        let lfo_val_l = targets.lfo_waveform.eval(phase_l, cycle_l, lfo_duty) * lfo_amount;
        let lfo_val_r = targets.lfo_waveform.eval(phase_r, cycle_r, lfo_duty) * lfo_amount;

        // Feedback delay read before shifting so feedback re-enters the shifter
        // (producing classic ascending/descending harmonic pitch/freq echoes).
        let delay_len = self.delay_scratch.len();
        let delay_active = targets.delay_enabled && delay_len > DELAY_STUB_FRAMES;
        let delay_return = if delay_active {
            let max_delay = (delay_len.saturating_sub(4)) as f64;
            let target_delay =
                delay_target_frames(targets, self.rate, s.tempo_micro_bpm, max_delay);
            let delay_alpha = 1.0 - (-1.0 / (sr * 0.010)).exp();
            s.delay_frames_smooth += (target_delay - s.delay_frames_smooth) * delay_alpha;
            let dl = read_ring_cubic(
                &self.delay_scratch,
                s.delay_next,
                s.delay_valid,
                s.delay_frames_smooth,
                0,
            );
            let dr = read_ring_cubic(
                &self.delay_scratch,
                s.delay_next,
                s.delay_valid,
                s.delay_frames_smooth,
                1,
            );
            [
                4.0 * ((feedback * dl) / 4.0).tanh(),
                4.0 * ((feedback * dr) / 4.0).tanh(),
            ]
        } else {
            [0.0, 0.0]
        };

        let shifter_in = [in_l + delay_return[0], in_r + delay_return[1]];

        // Core Pitch / Freq / Ring shift stage.
        let shifted = match targets.mode {
            ShifterMode::Pitch => {
                let pitch_len = self.pitch_scratch.len();
                self.pitch_scratch[s.pitch_next] = [shifter_in[0] as f32, shifter_in[1] as f32];
                s.pitch_next = (s.pitch_next + 1) % pitch_len;
                if s.pitch_valid < pitch_len {
                    s.pitch_valid += 1;
                }
                self.pitch_dirty_len = (self.pitch_dirty_len + 1).min(pitch_len);

                let fine_st = fine_ct * 0.01;
                let base_st = if targets.wide {
                    [coarse_st + fine_st, coarse_st - fine_st]
                } else {
                    [coarse_st + fine_st, coarse_st + fine_st]
                };
                let max_window = (sr * 0.100).min((pitch_len.saturating_sub(32)) as f64 * 0.5);
                let win_frames = (window_ms * sr * 0.001).clamp(32.0, max_window);
                let max_search = ((self.rate as usize) / 60)
                    .min(1_024)
                    .min(pitch_len.saturating_sub(win_frames as usize + 32));
                let search_stride = ((self.rate as usize) / 24_000).max(1);
                let lfo_st = [lfo_val_l * MAX_LFO_PITCH_ST, lfo_val_r * MAX_LFO_PITCH_ST];
                let env_st = env_mod * MAX_LFO_PITCH_ST;
                let max_ring_delay = (pitch_len.saturating_sub(6)) as f64;

                let mut out = [0.0_f64; 2];
                for ch in 0..2 {
                    let semi = (base_st[ch] + lfo_st[ch] + env_st).clamp(-36.0, 36.0);
                    let ratio = 2.0_f64.powf(semi / 12.0);
                    let rate_delta = 1.0 - ratio;
                    if rate_delta.abs() < 1e-6 {
                        out[ch] = read_ring_cubic(
                            &self.pitch_scratch,
                            s.pitch_next,
                            s.pitch_valid,
                            4.0,
                            ch,
                        );
                        continue;
                    }
                    // Keep grain delay travel bounded to <= win_frames across the full ±36 st range.
                    let lifetime = win_frames / rate_delta.abs().max(1.0);
                    let phase_step = 1.0 / lifetime.max(16.0);
                    let travel = rate_delta.abs() * lifetime;
                    let base_start = if rate_delta < 0.0 { 4.0 + travel } else { 4.0 };

                    let mut sample_sum = 0.0;
                    for g in 0..2 {
                        let p = s.pitch_grain_phase[ch][g];
                        let w = 0.5 * (1.0 - (TAU * p).cos());
                        let d = s.pitch_grain_delay[ch][g].clamp(1.0, max_ring_delay);
                        sample_sum += w * read_ring_cubic(
                            &self.pitch_scratch,
                            s.pitch_next,
                            s.pitch_valid,
                            d,
                            ch,
                        );
                    }
                    out[ch] = sample_sum;

                    // Advance both grains; whenever a grain wraps at weight 0, align its new
                    // starting delay to match the currently peaking companion grain's phase.
                    for g in 0..2 {
                        let next_p = s.pitch_grain_phase[ch][g] + phase_step;
                        if next_p >= 1.0 {
                            s.pitch_grain_phase[ch][g] = next_p - next_p.floor();
                            let other = 1 - g;
                            let aligned = find_phase_aligned_offset(
                                &self.pitch_scratch,
                                PhaseAlignQuery {
                                    write_pos: s.pitch_next,
                                    valid: s.pitch_valid,
                                    base_start,
                                    ref_delay: s.pitch_grain_delay[ch][other],
                                    max_search,
                                    stride: search_stride,
                                    channel: ch,
                                },
                            );
                            s.pitch_grain_delay[ch][g] =
                                (base_start + aligned).clamp(1.0, max_ring_delay);
                        } else {
                            s.pitch_grain_phase[ch][g] = next_p;
                            s.pitch_grain_delay[ch][g] = (s.pitch_grain_delay[ch][g] + rate_delta)
                                .clamp(1.0, max_ring_delay);
                        }
                    }
                }
                out
            }
            ShifterMode::Freq | ShifterMode::Ring => {
                let base_hz = if targets.wide {
                    [coarse_hz + fine_hz, coarse_hz - fine_hz]
                } else {
                    [coarse_hz + fine_hz, coarse_hz + fine_hz]
                };
                let mod_l = base_hz[0] + (lfo_val_l + env_mod) * MAX_LFO_FREQ_HZ;
                let mod_r = base_hz[1] + (lfo_val_r + env_mod) * MAX_LFO_FREQ_HZ;
                s.freq_shifter
                    .process_frame_modulated(shifter_in, [mod_l, mod_r])
            }
        };

        // Tone 1-pole low-pass filter on the shifted wet path (transparent at 22 kHz).
        let toned = if tone_hz >= 21_990.0 {
            s.tone_state = [0.0; 2];
            shifted
        } else {
            let pole = hex_dsp::frequency_pole(tone_hz.clamp(100.0, sr * 0.45), sr).unwrap_or(0.0);
            let mut out = [0.0_f64; 2];
            for ch in 0..2 {
                let y = hex_dsp::flush_quiet(shifted[ch] + pole * (s.tone_state[ch] - shifted[ch]));
                s.tone_state[ch] = y;
                out[ch] = y;
            }
            out
        };

        // Write shifted + toned wet sample into delay ring when delay is active.
        if delay_active {
            self.delay_scratch[s.delay_next] = [toned[0] as f32, toned[1] as f32];
            s.delay_next = (s.delay_next + 1) % delay_len;
            if s.delay_valid < delay_len {
                s.delay_valid += 1;
            }
            self.delay_dirty_len = (self.delay_dirty_len + 1).min(delay_len);
        }

        // Optional soft-saturation Drive stage + Output Gain.
        let output_linear = 10.0_f64.powf(output_gain_db / 20.0);
        let drive_norm = 2.0_f64.tanh();
        let wet_out = if targets.drive_enabled {
            [
                ((toned[0] * 2.0).tanh() / drive_norm) * output_linear,
                ((toned[1] * 2.0).tanh() / drive_norm) * output_linear,
            ]
        } else {
            [toned[0] * output_linear, toned[1] * output_linear]
        };

        // Flush histories to exact zero once sustained silence reaches the finite tail bound.
        if silent && s.silent_frames >= self.tail_limit {
            s.pitch_valid = 0;
            s.delay_valid = 0;
            s.freq_shifter.reset();
            s.tone_state = [0.0; 2];
            s.env_level = 0.0;
        }

        if bypassed || dry_wet == 0.0 {
            return;
        }
        if dry_wet == 1.0 {
            frame[0] = wet_out[0] as f32;
            frame[1] = wet_out[1] as f32;
        } else {
            let dry = 1.0 - dry_wet;
            frame[0] = (in_l * dry + wet_out[0] * dry_wet) as f32;
            frame[1] = (in_r * dry + wet_out[1] * dry_wet) as f32;
        }
    }

    fn process_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        values: &dyn NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        if !self.open {
            self.begin_block();
        }
        let ramp_frames = self.ramp_frames();
        let mut resolved = self.speculative.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) =
                self.speculative
                    .controls
                    .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                let effective = next.effective();
                self.speculative.retarget(effective, ramp_frames);
                self.speculative.controls.install(next);
            }
            let tempo = clock.and_then(|c| c.tempo_micro_bpm_at_offset(offset));
            self.process_frame(frame, tempo, bypassed);
        }
        Ok(())
    }

    fn inherit_scalars(&mut self, old: ShifterScalar, old_settings: ShifterSettings) {
        self.pristine = false;
        let ramp_frames = self.ramp_frames();
        let mut inherited = old;
        let mut controls = self.primary.controls;
        controls.inherit(&old.controls);
        inherited.controls = controls;
        if self.settings != old_settings {
            inherited.retarget(self.settings, ramp_frames);
        }
        self.primary = inherited;
        self.speculative = inherited;
        self.pitch_dirty_start = inherited.pitch_next;
        self.pitch_dirty_len = 0;
        self.delay_dirty_start = inherited.delay_next;
        self.delay_dirty_len = 0;
        self.open = false;
    }
}

impl OwnedProcessorRuntime for ShifterRuntime {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        self.primary.controls.supports(&BINDINGS, id)
    }
    crate::parameter_binding::delegate_automated_block_processing!(with_clock process_automation);

    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        let (inline, [a, b]) = self.control_storage()?;
        inline.checked_add(a)?.checked_add(b)
    }
    fn begin_block(&mut self) {
        if self.open {
            self.rollback_block();
        }
        self.pitch_dirty_start = self.speculative.pitch_next;
        self.pitch_dirty_len = 0;
        self.delay_dirty_start = self.speculative.delay_next;
        self.delay_dirty_len = 0;
        self.open = true;
    }
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        if !self.open {
            self.begin_block();
        }
        for frame in frames {
            self.process_frame(frame, None, false);
        }
    }
    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _: &dyn hex_lattice::DeviceResources,
    ) {
        if !self.open {
            self.begin_block();
        }
        for (offset, frame) in frames.iter_mut().enumerate() {
            let tempo = clock.and_then(|c| c.tempo_micro_bpm_at_offset(offset));
            self.process_frame(frame, tempo, false);
        }
    }
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        _: &dyn hex_lattice::DeviceResources,
    ) {
        if !self.open {
            self.begin_block();
        }
        for (offset, frame) in frames.iter_mut().enumerate() {
            let tempo = clock.and_then(|c| c.tempo_micro_bpm_at_offset(offset));
            self.process_frame(frame, tempo, true);
        }
    }
    fn commit_block(&mut self) {
        if !self.open {
            return;
        }
        Self::copy_interval(
            &self.pitch_scratch,
            &mut self.pitch_committed,
            self.pitch_dirty_start,
            self.pitch_dirty_len,
        );
        Self::copy_interval(
            &self.delay_scratch,
            &mut self.delay_committed,
            self.delay_dirty_start,
            self.delay_dirty_len,
        );
        self.primary = self.speculative;
        self.pitch_dirty_len = 0;
        self.delay_dirty_len = 0;
        self.open = false;
        self.pristine = false;
    }
    fn rollback_block(&mut self) {
        if !self.open {
            return;
        }
        Self::copy_interval(
            &self.pitch_committed,
            &mut self.pitch_scratch,
            self.pitch_dirty_start,
            self.pitch_dirty_len,
        );
        Self::copy_interval(
            &self.delay_committed,
            &mut self.delay_scratch,
            self.delay_dirty_start,
            self.delay_dirty_len,
        );
        self.speculative = self.primary;
        self.pitch_dirty_len = 0;
        self.delay_dirty_len = 0;
        self.open = false;
    }
    fn reset(&mut self) {
        self.pristine = false;
        let admitted = std::array::from_fn(|i| self.primary.controls.admitted(i));
        if let Ok(s) = ShifterScalar::new(
            self.settings,
            self.rate,
            admitted,
            self.tail_limit,
            self.delay_committed.len(),
        ) {
            self.primary = s;
            self.speculative = s;
        }
        self.pitch_dirty_start = 0;
        self.pitch_dirty_len = 0;
        self.delay_dirty_start = 0;
        self.delay_dirty_len = 0;
        self.open = false;
    }
    fn committed_is_quiescent(&self) -> bool {
        self.primary.is_quiescent(self.tail_limit)
    }
    fn speculative_is_quiescent(&self) -> bool {
        self.speculative.is_quiescent(self.tail_limit)
    }
    fn can_inherit_from(&self, old: &dyn OwnedProcessorRuntime) -> bool {
        old.as_any().downcast_ref::<Self>().is_some_and(|old| {
            !self.open
                && !old.open
                && self.rate == old.rate
                && self.pitch_committed.len() == old.pitch_committed.len()
                && self.delay_committed.len() == old.delay_committed.len()
        })
    }
    fn inherit_from(&mut self, old: &dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(old) {
            return;
        }
        if let Some(old) = old.as_any().downcast_ref::<Self>() {
            let p_len = old.pitch_committed.len();
            let p_start = (old.primary.pitch_next + p_len - old.primary.pitch_valid) % p_len;
            Self::copy_interval(
                &old.pitch_committed,
                &mut self.pitch_committed,
                p_start,
                old.primary.pitch_valid,
            );
            Self::copy_interval(
                &old.pitch_committed,
                &mut self.pitch_scratch,
                p_start,
                old.primary.pitch_valid,
            );
            let d_len = old.delay_committed.len();
            let d_start = (old.primary.delay_next + d_len - old.primary.delay_valid) % d_len;
            Self::copy_interval(
                &old.delay_committed,
                &mut self.delay_committed,
                d_start,
                old.primary.delay_valid,
            );
            Self::copy_interval(
                &old.delay_committed,
                &mut self.delay_scratch,
                d_start,
                old.primary.delay_valid,
            );
            self.inherit_scalars(old.primary, old.settings);
        }
    }
    fn transfer_history_from(&mut self, old: &mut dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(old) {
            return;
        }
        if let Some(old) = old.as_any_mut().and_then(|v| v.downcast_mut::<Self>()) {
            std::mem::swap(&mut self.pitch_committed, &mut old.pitch_committed);
            std::mem::swap(&mut self.pitch_scratch, &mut old.pitch_scratch);
            std::mem::swap(&mut self.delay_committed, &mut old.delay_committed);
            std::mem::swap(&mut self.delay_scratch, &mut old.delay_scratch);
            self.inherit_scalars(old.primary, old.settings);
            old.reset();
        }
    }
    fn can_fork_timing_without_signal_history_from(
        &self,
        previous: &dyn OwnedProcessorRuntime,
    ) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            self.pristine
                && self.primary.pitch_valid == 0
                && self.speculative.pitch_valid == 0
                && self.primary.delay_valid == 0
                && self.speculative.delay_valid == 0
                && !self.open
                && !old.open
                && self.rate == old.rate
                && self.settings == old.settings
                && self.pitch_committed.len() == old.pitch_committed.len()
                && self.delay_committed.len() == old.delay_committed.len()
        })
    }
    fn fork_timing_without_signal_history_from(
        &mut self,
        previous: &dyn OwnedProcessorRuntime,
    ) -> bool {
        if !self.can_fork_timing_without_signal_history_from(previous) {
            return false;
        }
        let Some(old) = previous.as_any().downcast_ref::<Self>() else {
            return false;
        };
        let mut controls = self.primary.controls;
        controls.inherit(&old.primary.controls);
        let mut scalar = old.primary;
        scalar.controls = controls;
        scalar.pitch_valid = 0;
        scalar.delay_valid = 0;
        scalar.freq_shifter.reset();
        scalar.tone_state = [0.0; 2];
        scalar.env_level = 0.0;
        scalar.silent_frames = self.tail_limit;
        self.primary = scalar;
        self.speculative = scalar;
        self.pitch_dirty_start = scalar.pitch_next;
        self.pitch_dirty_len = 0;
        self.delay_dirty_start = scalar.delay_next;
        self.delay_dirty_len = 0;
        self.pristine = false;
        true
    }
    fn can_split_tail_input_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        self.can_fork_timing_without_signal_history_from(previous)
    }
    fn can_retarget_parameters_from(&self, old: &dyn OwnedProcessorRuntime) -> bool {
        self.can_inherit_from(old)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests;
