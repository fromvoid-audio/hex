//! Bounded original two-oscillator native subtractive synthesizer.
//!
//! Each voice sums two band-limited oscillators (reusing control-prepared
//! mipmapped periodic tables from `operator_waveform` and `shared_sine_table`),
//! filters the mix through two cascaded double-precision TPT state-variable
//! low-pass sections (`hex_dsp::SvfSection`, 24 dB/oct), and shapes the output
//! with a sample-accurate linear ADSR amplitude envelope.
//!
//! # Bounds & Real-Time Safety
//! - Fixed inline storage: 64 voices, 259 events/block, zero heap allocations
//!   during preflight, rendering, history transfer, retargeting, or reset.
//! - Zero algorithmic buffering latency (`algorithmic_latency_frames() == Some(0)`).
//! - Exact finite release tail (`max_release_tail_frames() == envelope.release`);
//!   the amplitude envelope reaches exact `0.0` at retirement so voices become
//!   quiescent by the declared bound.
//! - Per-oscillator harmonic cap selection strictly keeps all partials below
//!   Nyquist; an oscillator transposed at or above Nyquist mutes cleanly rather
//!   than aliasing.
//! - TPT SVF damping is clamped to `[0.178, sqrt(2)]` and cutoff to
//!   `[20 Hz, 0.45 * sample_rate]`, guaranteeing passive unconditional stability
//!   across `8..=192 kHz` with subnormal flushing on every sample.
//!
//! No claim of named Live device parity; native UI and reference studies remain
//! separate.

use crate::fm4::Envelope;
use crate::operator_waveform::{self, OperatorWaveform, OperatorWaveformBank};
use hex_dsp::SvfSection;
use hex_lattice::{
    DeviceResources, InstrumentHistoryRetargetPreparation, MutableInstrumentRuntime,
    NativeInstrumentClock, NativeInstrumentDefinition, NativeInstrumentEvent,
    NativeInstrumentExpression, NativeInstrumentOccurrenceGain,
    NativeInstrumentRuntimeError as RuntimeError, PreparedAnalyticNote,
    PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime, PreparedNativeInstrument,
    StereoFrame, TuningTable,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;
use std::f64::consts::{PI, SQRT_2};

pub const SUBTRACTIVE_TYPE_ID: DeviceTypeId =
    DeviceTypeId::from_static("hex.instrument.subtractive");
pub const SUBTRACTIVE_STATE_VERSION: u32 = 1;

const FIELD_COUNT: usize = 14;
const PAYLOAD_BYTES: usize = FIELD_COUNT * std::mem::size_of::<u32>();
const VOICES: usize = 64;
const EVENTS: usize = 259;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const TWO_POW_64: f64 = 18_446_744_073_709_551_616.0;

const WAVE_LABELS: &[&str] = &["sine", "saw", "square", "triangle"];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const OSC1_WAVE: ParameterId = ParameterId::from_static(1);
    pub const OSC1_SEMITONES: ParameterId = ParameterId::from_static(2);
    pub const OSC1_DETUNE: ParameterId = ParameterId::from_static(3);
    pub const OSC1_LEVEL: ParameterId = ParameterId::from_static(4);
    pub const OSC2_WAVE: ParameterId = ParameterId::from_static(5);
    pub const OSC2_SEMITONES: ParameterId = ParameterId::from_static(6);
    pub const OSC2_DETUNE: ParameterId = ParameterId::from_static(7);
    pub const OSC2_LEVEL: ParameterId = ParameterId::from_static(8);
    pub const FILTER_CUTOFF: ParameterId = ParameterId::from_static(9);
    pub const FILTER_RESONANCE: ParameterId = ParameterId::from_static(10);
    pub const ATTACK: ParameterId = ParameterId::from_static(11);
    pub const DECAY: ParameterId = ParameterId::from_static(12);
    pub const SUSTAIN: ParameterId = ParameterId::from_static(13);
    pub const RELEASE: ParameterId = ParameterId::from_static(14);
}

const fn float_parameter(
    id: u32,
    label: &'static str,
    min: f32,
    max: f32,
    unit: Option<&'static str>,
    default: f32,
    presentation: ParameterPresentation,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(id),
        label,
        kind: ParameterKind::Float { min, max, unit },
        default: ParameterValue::Float(default),
        presentation,
    }
}

const fn choice_parameter(
    id: u32,
    label: &'static str,
    labels: &'static [&'static str],
    default: u32,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(id),
        label,
        kind: ParameterKind::Choice { labels },
        default: ParameterValue::Choice(default),
        presentation: ParameterPresentation::Linear,
    }
}

pub static PARAMETERS: [ParameterDescriptor; FIELD_COUNT] = [
    choice_parameter(1, "osc1 wave", WAVE_LABELS, 1),
    float_parameter(
        2,
        "osc1 semitones",
        -24.0,
        24.0,
        Some("st"),
        0.0,
        ParameterPresentation::Linear,
    ),
    float_parameter(
        3,
        "osc1 detune",
        -100.0,
        100.0,
        Some("ct"),
        0.0,
        ParameterPresentation::Linear,
    ),
    float_parameter(
        4,
        "osc1 level",
        0.0,
        1.0,
        None,
        0.7,
        ParameterPresentation::Linear,
    ),
    choice_parameter(5, "osc2 wave", WAVE_LABELS, 2),
    float_parameter(
        6,
        "osc2 semitones",
        -24.0,
        24.0,
        Some("st"),
        -12.0,
        ParameterPresentation::Linear,
    ),
    float_parameter(
        7,
        "osc2 detune",
        -100.0,
        100.0,
        Some("ct"),
        7.0,
        ParameterPresentation::Linear,
    ),
    float_parameter(
        8,
        "osc2 level",
        0.0,
        1.0,
        None,
        0.3,
        ParameterPresentation::Linear,
    ),
    float_parameter(
        9,
        "filter cutoff",
        20.0,
        20_000.0,
        Some("hz"),
        4_000.0,
        ParameterPresentation::Logarithmic,
    ),
    float_parameter(
        10,
        "filter resonance",
        0.0,
        95.0,
        Some("%"),
        15.0,
        ParameterPresentation::Linear,
    ),
    float_parameter(
        11,
        "attack",
        0.0,
        10.0,
        Some("s"),
        0.005,
        ParameterPresentation::Linear,
    ),
    float_parameter(
        12,
        "decay",
        0.0,
        10.0,
        Some("s"),
        0.15,
        ParameterPresentation::Linear,
    ),
    float_parameter(
        13,
        "sustain",
        0.0,
        1.0,
        None,
        0.7,
        ParameterPresentation::Linear,
    ),
    float_parameter(
        14,
        "release",
        0.0,
        10.0,
        Some("s"),
        0.15,
        ParameterPresentation::Linear,
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Waveform {
    Sine,
    Saw,
    Square,
    Triangle,
}

impl Waveform {
    const fn from_choice(choice: u32) -> Result<Self, DeviceContractError> {
        match choice {
            0 => Ok(Self::Sine),
            1 => Ok(Self::Saw),
            2 => Ok(Self::Square),
            3 => Ok(Self::Triangle),
            _ => Err(DeviceContractError::MalformedState),
        }
    }

    const fn as_choice(self) -> u32 {
        match self {
            Self::Sine => 0,
            Self::Saw => 1,
            Self::Square => 2,
            Self::Triangle => 3,
        }
    }

    const fn operator_waveform(self) -> OperatorWaveform {
        match self {
            Self::Sine => OperatorWaveform::Sine,
            Self::Saw => OperatorWaveform::Saw,
            Self::Square => OperatorWaveform::Square,
            Self::Triangle => OperatorWaveform::Triangle,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct OscillatorSettings {
    wave: Waveform,
    semitones: f32,
    detune_cents: f32,
    level: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SubtractiveSettings {
    osc: [OscillatorSettings; 2],
    cutoff_hz: f32,
    resonance_pct: f32,
    attack_s: f32,
    decay_s: f32,
    sustain: f32,
    release_s: f32,
}

impl SubtractiveSettings {
    const fn defaults() -> Self {
        Self {
            osc: [
                OscillatorSettings {
                    wave: Waveform::Saw,
                    semitones: 0.0,
                    detune_cents: 0.0,
                    level: 0.7,
                },
                OscillatorSettings {
                    wave: Waveform::Square,
                    semitones: -12.0,
                    detune_cents: 7.0,
                    level: 0.3,
                },
            ],
            cutoff_hz: 4_000.0,
            resonance_pct: 15.0,
            attack_s: 0.005,
            decay_s: 0.15,
            sustain: 0.7,
            release_s: 0.15,
        }
    }

    fn encode(self) -> Vec<u8> {
        let words: [u32; FIELD_COUNT] = [
            self.osc[0].wave.as_choice(),
            self.osc[0].semitones.to_bits(),
            self.osc[0].detune_cents.to_bits(),
            self.osc[0].level.to_bits(),
            self.osc[1].wave.as_choice(),
            self.osc[1].semitones.to_bits(),
            self.osc[1].detune_cents.to_bits(),
            self.osc[1].level.to_bits(),
            self.cutoff_hz.to_bits(),
            self.resonance_pct.to_bits(),
            self.attack_s.to_bits(),
            self.decay_s.to_bits(),
            self.sustain.to_bits(),
            self.release_s.to_bits(),
        ];
        let mut out = Vec::with_capacity(PAYLOAD_BYTES);
        for word in words {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out
    }

    fn from_state(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != SUBTRACTIVE_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != SUBTRACTIVE_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let bytes = state.payload();
        if bytes.len() != PAYLOAD_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut words = [0_u32; FIELD_COUNT];
        for (word, chunk) in words.iter_mut().zip(bytes.chunks_exact(4)) {
            *word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        let read_float = |index: usize| -> Result<f32, DeviceContractError> {
            let value = f32::from_bits(words[index]);
            PARAMETERS[index]
                .validate(ParameterValue::Float(value))
                .map_err(|_| DeviceContractError::MalformedState)?;
            Ok(value)
        };
        let settings = Self {
            osc: [
                OscillatorSettings {
                    wave: Waveform::from_choice(words[0])?,
                    semitones: read_float(1)?,
                    detune_cents: read_float(2)?,
                    level: read_float(3)?,
                },
                OscillatorSettings {
                    wave: Waveform::from_choice(words[4])?,
                    semitones: read_float(5)?,
                    detune_cents: read_float(6)?,
                    level: read_float(7)?,
                },
            ],
            cutoff_hz: read_float(8)?,
            resonance_pct: read_float(9)?,
            attack_s: read_float(10)?,
            decay_s: read_float(11)?,
            sustain: read_float(12)?,
            release_s: read_float(13)?,
        };
        Envelope::new(
            f64::from(settings.attack_s),
            f64::from(settings.decay_s),
            f64::from(settings.sustain),
            f64::from(settings.release_s),
        )
        .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(settings)
    }

    fn read_parameter(self, id: ParameterId) -> Result<ParameterValue, DeviceContractError> {
        Ok(match id {
            parameter::OSC1_WAVE => ParameterValue::Choice(self.osc[0].wave.as_choice()),
            parameter::OSC1_SEMITONES => ParameterValue::Float(self.osc[0].semitones),
            parameter::OSC1_DETUNE => ParameterValue::Float(self.osc[0].detune_cents),
            parameter::OSC1_LEVEL => ParameterValue::Float(self.osc[0].level),
            parameter::OSC2_WAVE => ParameterValue::Choice(self.osc[1].wave.as_choice()),
            parameter::OSC2_SEMITONES => ParameterValue::Float(self.osc[1].semitones),
            parameter::OSC2_DETUNE => ParameterValue::Float(self.osc[1].detune_cents),
            parameter::OSC2_LEVEL => ParameterValue::Float(self.osc[1].level),
            parameter::FILTER_CUTOFF => ParameterValue::Float(self.cutoff_hz),
            parameter::FILTER_RESONANCE => ParameterValue::Float(self.resonance_pct),
            parameter::ATTACK => ParameterValue::Float(self.attack_s),
            parameter::DECAY => ParameterValue::Float(self.decay_s),
            parameter::SUSTAIN => ParameterValue::Float(self.sustain),
            parameter::RELEASE => ParameterValue::Float(self.release_s),
            _ => return Err(DeviceContractError::UnknownParameter),
        })
    }

    fn patch_parameter(
        mut self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let index = id
            .value()
            .checked_sub(1)
            .and_then(|offset| usize::try_from(offset).ok())
            .filter(|&index| index < FIELD_COUNT)
            .ok_or(DeviceContractError::UnknownParameter)?;
        PARAMETERS[index].validate(value)?;
        match (id, value) {
            (parameter::OSC1_WAVE, ParameterValue::Choice(c)) => {
                self.osc[0].wave = Waveform::from_choice(c)?;
            }
            (parameter::OSC1_SEMITONES, ParameterValue::Float(v)) => self.osc[0].semitones = v,
            (parameter::OSC1_DETUNE, ParameterValue::Float(v)) => self.osc[0].detune_cents = v,
            (parameter::OSC1_LEVEL, ParameterValue::Float(v)) => self.osc[0].level = v,
            (parameter::OSC2_WAVE, ParameterValue::Choice(c)) => {
                self.osc[1].wave = Waveform::from_choice(c)?;
            }
            (parameter::OSC2_SEMITONES, ParameterValue::Float(v)) => self.osc[1].semitones = v,
            (parameter::OSC2_DETUNE, ParameterValue::Float(v)) => self.osc[1].detune_cents = v,
            (parameter::OSC2_LEVEL, ParameterValue::Float(v)) => self.osc[1].level = v,
            (parameter::FILTER_CUTOFF, ParameterValue::Float(v)) => self.cutoff_hz = v,
            (parameter::FILTER_RESONANCE, ParameterValue::Float(v)) => self.resonance_pct = v,
            (parameter::ATTACK, ParameterValue::Float(v)) => self.attack_s = v,
            (parameter::DECAY, ParameterValue::Float(v)) => self.decay_s = v,
            (parameter::SUSTAIN, ParameterValue::Float(v)) => self.sustain = v,
            (parameter::RELEASE, ParameterValue::Float(v)) => self.release_s = v,
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
        NativeDeviceState::new(
            SUBTRACTIVE_TYPE_ID,
            SUBTRACTIVE_STATE_VERSION,
            self.encode(),
        )?
        .with_input_gain_millidb(state.input_gain_millidb())?
        .with_output_gain_millidb(state.output_gain_millidb())
    }
}

pub static SUBTRACTIVE_DEFINITION: SubtractiveDefinition = SubtractiveDefinition;

pub struct SubtractiveDefinition;

impl NativeDeviceMetadata for SubtractiveDefinition {
    fn type_id(&self) -> DeviceTypeId {
        SUBTRACTIVE_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("subtractive")
    }

    fn state_version(&self) -> u32 {
        SUBTRACTIVE_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &PARAMETERS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(
            SUBTRACTIVE_TYPE_ID,
            SUBTRACTIVE_STATE_VERSION,
            SubtractiveSettings::defaults().encode(),
        )
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        SubtractiveSettings::from_state(state)?;
        Ok(state.clone())
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        SubtractiveSettings::from_state(state)?.read_parameter(parameter)
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        SubtractiveSettings::from_state(state)?.patch_parameter(state, parameter, value)
    }
}

impl NativeInstrumentDefinition for SubtractiveDefinition {
    fn resource_factory_inline_bound(&self) -> Option<usize> {
        Some(std::mem::size_of::<Factory>())
    }

    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let factory = Factory::prepare(state, context)?;
        Ok(Box::new(PreparedSubtractiveInstrument { factory }))
    }
}

struct PreparedSubtractiveInstrument {
    factory: Factory,
}

impl PreparedNativeInstrument for PreparedSubtractiveInstrument {
    fn validate_event_note(&self, pitch: u8, velocity: u8) -> Result<(), DeviceContractError> {
        self.factory.validate_note_on(pitch, velocity)
    }

    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(Some(Box::new(self.factory)))
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn prepare_note(
        &self,
        _pitch: u8,
        _velocity: u8,
    ) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
}

#[derive(Clone, Copy)]
struct KeyPitch {
    steps: [u64; 2],
}

#[derive(Clone, Copy)]
struct FilterCoeffs {
    g: f64,
    k: f64,
    norm: f64,
}

impl FilterCoeffs {
    fn compute(sample_rate: u32, cutoff_hz: f64, resonance_pct: f64) -> Self {
        let fs = f64::from(sample_rate);
        let fc = cutoff_hz.clamp(20.0, 0.45 * fs);
        let g = (PI * fc / fs).tan();
        let res = (resonance_pct / 100.0).clamp(0.0, 0.95);
        // Passive damping in [0.178, sqrt(2)]: Butterworth/Linkwitz-Riley at
        // 0% resonance and bounded crisp resonance without blowup at 95%.
        let k = SQRT_2 * (1.0 - 0.92 * res);
        let norm = 1.0 / (1.0 + g * (g + k));
        Self { g, k, norm }
    }
}

#[derive(Clone, Copy)]
struct PreparedAdsr {
    attack: u64,
    decay: u64,
    sustain: f64,
    release: u64,
}

#[derive(Clone, Copy)]
struct Config {
    identity: [u8; PAYLOAD_BYTES],
    tuning: TuningTable,
    sample_rate: u32,
    waves: [Waveform; 2],
    levels: [f64; 2],
    cutoff_hz: f64,
    resonance_pct: f64,
    filter: FilterCoeffs,
    envelope: PreparedAdsr,
    keys: [Option<KeyPitch>; 128],
    sine: &'static crate::SineTable,
    bank: &'static OperatorWaveformBank,
    smoothing_frames: u32,
}

impl Config {
    fn same_target(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.tuning == other.tuning
            && self.sample_rate == other.sample_rate
    }
}

fn phase_step_and_cap(frequency_hz: f64, sample_rate: u32) -> (u64, u8) {
    let nyquist = 0.5 * f64::from(sample_rate);
    if !frequency_hz.is_finite() || frequency_hz <= 0.0 || frequency_hz >= nyquist {
        return (0, 0);
    }
    let step = ((frequency_hz / f64::from(sample_rate)) * TWO_POW_64)
        .round()
        .clamp(1.0, (1_u64 << 63) as f64 - 1.0) as u64;
    match operator_waveform::harmonic_cap_index(step) {
        Some(cap) => (step, cap),
        None => (0, 0),
    }
}

fn bent_steps_and_caps(
    base_steps: [u64; 2],
    pitch_offset_cents: i16,
    channel_pitch_bend: i16,
) -> ([u64; 2], [u8; 2]) {
    let channel_cents = if channel_pitch_bend == 0 {
        0.0
    } else {
        f64::from(channel_pitch_bend)
            / if channel_pitch_bend < 0 {
                8192.0
            } else {
                8191.0
            }
            * 200.0
    };
    let total_cents = f64::from(pitch_offset_cents) + channel_cents;
    let ratio = if total_cents == 0.0 {
        1.0
    } else {
        2.0_f64.powf(total_cents / 1200.0)
    };
    let mut steps = [0_u64; 2];
    let mut caps = [0_u8; 2];
    for k in 0..2 {
        if base_steps[k] == 0 {
            continue;
        }
        let scaled = (base_steps[k] as f64) * ratio;
        if !scaled.is_finite() || scaled < 1.0 || scaled >= (1_u64 << 63) as f64 {
            continue;
        }
        let step = scaled.round() as u64;
        if let Some(cap) = operator_waveform::harmonic_cap_index(step) {
            steps[k] = step;
            caps[k] = cap;
        }
    }
    (steps, caps)
}

#[derive(Clone, Copy)]
struct Factory {
    config: Config,
}

impl Factory {
    fn prepare(
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Self, DeviceContractError> {
        let sample_rate = context.sample_rate;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = SubtractiveSettings::from_state(state)?;
        let bank = operator_waveform::prepared_bank()
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let sine = crate::shared_sine_table();
        let ratios = [
            2.0_f64.powf(
                (f64::from(settings.osc[0].semitones)
                    + f64::from(settings.osc[0].detune_cents) / 100.0)
                    / 12.0,
            ),
            2.0_f64.powf(
                (f64::from(settings.osc[1].semitones)
                    + f64::from(settings.osc[1].detune_cents) / 100.0)
                    / 12.0,
            ),
        ];
        let mut keys = [None; 128];
        let nyquist = 0.5 * f64::from(sample_rate);
        for (pitch, slot) in keys.iter_mut().enumerate() {
            let base_hz = context.tuning.frequencies()[pitch];
            if !base_hz.is_finite() || base_hz <= 0.0 || base_hz >= nyquist {
                continue;
            }
            let (s0, _) = phase_step_and_cap(base_hz * ratios[0], sample_rate);
            let (s1, _) = phase_step_and_cap(base_hz * ratios[1], sample_rate);
            *slot = Some(KeyPitch { steps: [s0, s1] });
        }
        let frames = |seconds: f32| (f64::from(seconds) * f64::from(sample_rate)).round() as u64;
        let mut identity = [0_u8; PAYLOAD_BYTES];
        identity.copy_from_slice(state.payload());
        let cutoff_hz = f64::from(settings.cutoff_hz);
        let resonance_pct = f64::from(settings.resonance_pct);
        Ok(Self {
            config: Config {
                identity,
                tuning: *context.tuning,
                sample_rate,
                waves: [settings.osc[0].wave, settings.osc[1].wave],
                levels: [
                    f64::from(settings.osc[0].level),
                    f64::from(settings.osc[1].level),
                ],
                cutoff_hz,
                resonance_pct,
                filter: FilterCoeffs::compute(sample_rate, cutoff_hz, resonance_pct),
                envelope: PreparedAdsr {
                    attack: frames(settings.attack_s),
                    decay: frames(settings.decay_s),
                    sustain: f64::from(settings.sustain),
                    release: frames(settings.release_s),
                },
                keys,
                sine,
                bank,
                smoothing_frames: ((u64::from(sample_rate) * 5 + 500) / 1000).max(1) as u32,
            },
        })
    }

    fn validate_note_on(&self, pitch: u8, velocity: u8) -> Result<(), DeviceContractError> {
        if (1..=127).contains(&velocity)
            && self
                .config
                .keys
                .get(usize::from(pitch))
                .is_some_and(Option::is_some)
        {
            Ok(())
        } else {
            Err(DeviceContractError::InvalidParameterValue)
        }
    }
}

struct Retarget {
    identity: [u8; PAYLOAD_BYTES],
    tuning: TuningTable,
    sample_rate: u32,
}

impl PreparedInstrumentHistoryRetarget for Retarget {
    fn declared_fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    id: u64,
    pitch: u8,
    velocity: u8,
    age: u64,
    gate: Option<u64>,
    release_ceiling: u64,
    phases: [u64; 2],
    steps: [u64; 2],
    caps: [u8; 2],
    pitch_offset_cents: i16,
    filter: [SvfSection; 2],
}

impl Voice {
    fn advance_metadata(&mut self, frames: u64) {
        if self.id == 0 {
            return;
        }
        self.age = self.age.saturating_add(frames);
        if self
            .gate
            .is_some_and(|gate| self.age.saturating_sub(gate) >= self.release_ceiling)
        {
            *self = Self::default();
        }
    }

    fn retarget_steps(&mut self, base: KeyPitch, channel_pitch_bend: i16) {
        let (steps, caps) =
            bent_steps_and_caps(base.steps, self.pitch_offset_cents, channel_pitch_bend);
        self.steps = steps;
        self.caps = caps;
    }
}

#[derive(Clone, Copy)]
struct SmoothParam {
    current: f64,
    target: f64,
    remaining: u32,
}

impl SmoothParam {
    const fn settled(value: f64) -> Self {
        Self {
            current: value,
            target: value,
            remaining: 0,
        }
    }

    fn retarget(&mut self, target: f64, frames: u32) {
        if (self.current - target).abs() <= f64::EPSILON {
            self.current = target;
            self.target = target;
            self.remaining = 0;
        } else {
            self.target = target;
            self.remaining = frames.max(1);
        }
    }

    #[inline]
    fn step(&mut self) -> f64 {
        if self.remaining != 0 {
            self.current += (self.target - self.current) / f64::from(self.remaining);
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            }
        }
        self.current
    }
}

struct Runtime {
    config: Config,
    voices: [Voice; VOICES],
    channel_pitch_bend: i16,
    levels: [SmoothParam; 2],
    cutoff_hz: SmoothParam,
    resonance_pct: SmoothParam,
    active_filter: FilterCoeffs,
}

impl Runtime {
    fn new(config: Config) -> Self {
        Self {
            voices: [Voice::default(); VOICES],
            channel_pitch_bend: 0,
            levels: [
                SmoothParam::settled(config.levels[0]),
                SmoothParam::settled(config.levels[1]),
            ],
            cutoff_hz: SmoothParam::settled(config.cutoff_hz),
            resonance_pct: SmoothParam::settled(config.resonance_pct),
            active_filter: config.filter,
            config,
        }
    }

    fn apply_event(
        config: &Config,
        voices: &mut [Voice; VOICES],
        channel_pitch_bend: &mut i16,
        event: NativeInstrumentEvent,
    ) -> Result<(), RuntimeError> {
        match event {
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                let Some(key) = config.keys.get(usize::from(pitch)).copied().flatten() else {
                    return Err(RuntimeError::InvalidEventBatch);
                };
                if !(1..=127).contains(&velocity)
                    || occurrence_id == 0
                    || voices.iter().any(|voice| voice.id == occurrence_id)
                {
                    return Err(RuntimeError::InvalidEventBatch);
                }
                let voice = voices
                    .iter_mut()
                    .find(|voice| voice.id == 0)
                    .ok_or(RuntimeError::EventCapacityExceeded)?;
                let (steps, caps) = bent_steps_and_caps(key.steps, 0, *channel_pitch_bend);
                *voice = Voice {
                    id: occurrence_id,
                    pitch,
                    velocity,
                    age: 0,
                    gate: None,
                    release_ceiling: config.envelope.release,
                    phases: [0, 0],
                    steps,
                    caps,
                    pitch_offset_cents: 0,
                    filter: [SvfSection::new(), SvfSection::new()],
                };
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                if let Some(voice) = voices.iter_mut().find(|voice| voice.id == occurrence_id) {
                    if voice.gate.is_none() {
                        voice.gate = Some(voice.age);
                    }
                    voice.advance_metadata(0);
                }
            }
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(RuntimeError::UnsupportedNotePressure);
            }
            NativeInstrumentEvent::NotePitchBend {
                occurrence_id,
                cents,
                ..
            } => {
                if !(-4_800..=4_800).contains(&cents) {
                    return Err(RuntimeError::InvalidEventBatch);
                }
                if let Some(voice) = voices
                    .iter_mut()
                    .find(|voice| voice.id == occurrence_id.get())
                    && voice.pitch_offset_cents != cents
                {
                    voice.pitch_offset_cents = cents;
                    if let Some(key) = config.keys[usize::from(voice.pitch)] {
                        voice.retarget_steps(key, *channel_pitch_bend);
                    }
                }
            }
            NativeInstrumentEvent::NoteTimbre { value, .. } => {
                if value > 127 {
                    return Err(RuntimeError::InvalidEventBatch);
                }
            }
            NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                if cc > 127 || value > 127 {
                    return Err(RuntimeError::InvalidEventBatch);
                }
                if cc == 121 && *channel_pitch_bend != 0 {
                    *channel_pitch_bend = 0;
                    Self::retarget_all_voices(config, voices, 0);
                }
            }
            NativeInstrumentEvent::PitchBend { value, .. } => {
                if !(-8192..=8191).contains(&value) {
                    return Err(RuntimeError::InvalidEventBatch);
                }
                if *channel_pitch_bend != value {
                    *channel_pitch_bend = value;
                    Self::retarget_all_voices(config, voices, value);
                }
            }
            NativeInstrumentEvent::Expression { expression, .. } => {
                if let NativeInstrumentExpression::PitchBend(value) = expression
                    && *channel_pitch_bend != value
                {
                    *channel_pitch_bend = value;
                    Self::retarget_all_voices(config, voices, value);
                }
            }
        }
        Ok(())
    }

    fn retarget_all_voices(config: &Config, voices: &mut [Voice; VOICES], bend: i16) {
        for voice in voices.iter_mut().filter(|voice| voice.id != 0) {
            if let Some(key) = config.keys[usize::from(voice.pitch)] {
                voice.retarget_steps(key, bend);
            }
        }
    }

    #[inline]
    fn sample_oscillator(config: &Config, osc_index: usize, phase: u64, step: u64, cap: u8) -> f64 {
        if step == 0 {
            return 0.0;
        }
        match config.waves[osc_index] {
            Waveform::Sine => f64::from(config.sine.sample_phase(phase)),
            wave => f64::from(
                config
                    .bank
                    .sample_phase(wave.operator_waveform(), cap, phase),
            ),
        }
    }

    fn render(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
    ) {
        let mut cursor = 0_usize;
        for (frame_index, slot) in frames.iter_mut().enumerate() {
            while let Some(&event) = events.get(cursor)
                && event.frame_offset() == frame_index
            {
                let _ = Self::apply_event(
                    &self.config,
                    &mut self.voices,
                    &mut self.channel_pitch_bend,
                    event,
                );
                cursor += 1;
            }

            let level0 = self.levels[0].step();
            let level1 = self.levels[1].step();
            let filter_ramping = self.cutoff_hz.remaining != 0 || self.resonance_pct.remaining != 0;
            let cutoff = self.cutoff_hz.step();
            let resonance = self.resonance_pct.step();
            if filter_ramping {
                self.active_filter =
                    FilterCoeffs::compute(self.config.sample_rate, cutoff, resonance);
            }
            let fc = self.active_filter;

            let mut sum = 0.0_f64;
            for voice in &mut self.voices {
                if voice.id == 0 {
                    continue;
                }
                let amp = envelope_amplitude(
                    self.config.envelope,
                    voice.age,
                    voice.gate,
                    voice.release_ceiling,
                );
                if amp > 0.0 || voice.gate.is_none() {
                    let s0 = Self::sample_oscillator(
                        &self.config,
                        0,
                        voice.phases[0],
                        voice.steps[0],
                        voice.caps[0],
                    );
                    let s1 = Self::sample_oscillator(
                        &self.config,
                        1,
                        voice.phases[1],
                        voice.steps[1],
                        voice.caps[1],
                    );
                    let dry = s0 * level0 + s1 * level1;
                    let stage1 = voice.filter[0]
                        .process_outputs_precomputed(dry, fc.g, fc.k, fc.norm)
                        .low;
                    let filtered = voice.filter[1]
                        .process_outputs_precomputed(stage1, fc.g, fc.k, fc.norm)
                        .low;
                    let vel = f64::from(voice.velocity) / 127.0;
                    let occ_gain = occurrence_gain_at(gains, voice.id, frame_index);
                    // Scale by 0.5 headroom so two full-level oscillators stay comfortably within [-1, 1].
                    sum += 0.5 * filtered * amp * vel * occ_gain;
                }
                voice.phases[0] = voice.phases[0].wrapping_add(voice.steps[0]);
                voice.phases[1] = voice.phases[1].wrapping_add(voice.steps[1]);
                voice.advance_metadata(1);
            }
            let out = if sum.is_finite() { sum as f32 } else { 0.0 };
            *slot = [out, out];
        }
    }
}

#[inline]
fn occurrence_gain_at(
    gains: &[NativeInstrumentOccurrenceGain],
    occurrence_id: u64,
    frame_index: usize,
) -> f64 {
    if gains.is_empty() {
        return 1.0;
    }
    let Ok(index) = gains.binary_search_by_key(&occurrence_id, |entry| entry.occurrence_id())
    else {
        return 1.0;
    };
    gains[index].value_at_offset(frame_index as u64)
}

#[inline]
fn hold_amplitude(env: PreparedAdsr, age: u64) -> f64 {
    if env.attack == 0 {
        if env.decay == 0 || age >= env.decay {
            env.sustain
        } else {
            1.0 + (env.sustain - 1.0) * (age as f64 / env.decay as f64)
        }
    } else if age < env.attack {
        age as f64 / env.attack as f64
    } else {
        let decay_age = age - env.attack;
        if env.decay == 0 || decay_age >= env.decay {
            env.sustain
        } else {
            1.0 + (env.sustain - 1.0) * (decay_age as f64 / env.decay as f64)
        }
    }
}

#[inline]
fn envelope_amplitude(env: PreparedAdsr, age: u64, gate: Option<u64>, release_ceiling: u64) -> f64 {
    match gate {
        None => hold_amplitude(env, age),
        Some(gate_age) => {
            let rel_age = age.saturating_sub(gate_age);
            if release_ceiling == 0 || rel_age >= release_ceiling {
                0.0
            } else {
                let start = hold_amplitude(env, gate_age);
                start * (1.0 - rel_age as f64 / release_ceiling as f64)
            }
        }
    }
}

impl PreparedMutableInstrumentRuntime for Factory {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }

    fn supports_occurrence_gain(&self) -> bool {
        true
    }

    fn resource_factory_inline_bytes(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>())
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn runtime_version(&self) -> u32 {
        1
    }

    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            Retarget {
                identity: self.config.identity,
                tuning: self.config.tuning,
                sample_rate: self.config.sample_rate,
            },
        )))
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }

    fn max_events_per_block(&self) -> usize {
        EVENTS
    }

    fn max_active_occurrences(&self) -> usize {
        VOICES
    }

    fn max_release_tail_frames(&self) -> u64 {
        self.config.envelope.release
    }

    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime::new(self.config))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl MutableInstrumentRuntime for Runtime {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }

    fn supports_occurrence_gain(&self) -> bool {
        true
    }

    fn active_occurrence_ids(&self, output: &mut [u64]) -> Result<usize, RuntimeError> {
        let count = self.voices.iter().filter(|voice| voice.id != 0).count();
        if count > output.len() {
            return Err(RuntimeError::ActiveOccurrenceCapacityExceeded);
        }
        for (slot, voice) in output
            .iter_mut()
            .zip(self.voices.iter().filter(|voice| voice.id != 0))
        {
            *slot = voice.id;
        }
        output[..count].sort_unstable();
        Ok(count)
    }

    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        _: &dyn DeviceResources,
    ) -> Result<(), RuntimeError> {
        if events.len() > EVENTS {
            return Err(RuntimeError::EventCapacityExceeded);
        }
        let mut voices = self.voices;
        let mut bend = self.channel_pitch_bend;
        let mut previous = 0;
        for &event in events {
            let offset = event.frame_offset();
            if offset < previous || offset >= frame_count {
                return Err(RuntimeError::InvalidEventBatch);
            }
            for voice in &mut voices {
                voice.advance_metadata((offset - previous) as u64);
            }
            Self::apply_event(&self.config, &mut voices, &mut bend, event)?;
            previous = offset;
        }
        Ok(())
    }

    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), RuntimeError> {
        if gains.iter().any(|gain| {
            !self
                .voices
                .iter()
                .any(|voice| voice.id == gain.occurrence_id())
                && !events.iter().any(|event| {
                    matches!(
                        event,
                        NativeInstrumentEvent::NoteOn { occurrence_id, .. }
                            if *occurrence_id == gain.occurrence_id()
                    )
                })
        }) {
            return Err(RuntimeError::InvalidOccurrenceGain);
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: &dyn DeviceResources,
    ) {
        self.render(events, frames, &[]);
    }

    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        _: &dyn DeviceResources,
    ) {
        self.render(events, frames, gains);
    }

    fn reset(&mut self) {
        *self = Self::new(self.config);
    }

    fn is_quiescent(&self) -> bool {
        self.voices.iter().all(|voice| voice.id == 0)
    }

    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.config.same_target(&previous.config))
    }

    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.voices = previous.voices;
            self.channel_pitch_bend = previous.channel_pitch_bend;
            self.levels = previous.levels;
            self.cutoff_hz = previous.cutoff_hz;
            self.resonance_pct = previous.resonance_pct;
            self.active_filter = previous.active_filter;
        }
    }

    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        let (Some(previous), Some(proof)) = (
            previous.as_any().downcast_ref::<Self>(),
            proof.as_any().downcast_ref::<Retarget>(),
        ) else {
            return false;
        };
        self.config.identity == proof.identity
            && self.config.tuning == proof.tuning
            && self.config.sample_rate == proof.sample_rate
            && self.config.sample_rate == previous.config.sample_rate
            && previous
                .voices
                .iter()
                .filter(|voice| voice.id != 0)
                .all(|voice| {
                    self.config
                        .keys
                        .get(usize::from(voice.pitch))
                        .is_some_and(Option::is_some)
                })
    }

    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        _: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.voices = previous.voices;
            self.channel_pitch_bend = previous.channel_pitch_bend;
            for voice in self.voices.iter_mut().filter(|voice| voice.id != 0) {
                voice.release_ceiling = voice.release_ceiling.min(self.config.envelope.release);
                voice.advance_metadata(0);
                if voice.id == 0 {
                    continue;
                }
                if let Some(key) = self.config.keys[usize::from(voice.pitch)] {
                    voice.retarget_steps(key, self.channel_pitch_bend);
                }
            }
            let frames = self.config.smoothing_frames;
            self.levels = previous.levels;
            self.levels[0].retarget(self.config.levels[0], frames);
            self.levels[1].retarget(self.config.levels[1], frames);
            self.cutoff_hz = previous.cutoff_hz;
            self.cutoff_hz.retarget(self.config.cutoff_hz, frames);
            self.resonance_pct = previous.resonance_pct;
            self.resonance_pct
                .retarget(self.config.resonance_pct, frames);
            self.active_filter = previous.active_filter;
        }
    }

    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests;
