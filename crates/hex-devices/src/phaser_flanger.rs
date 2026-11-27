//! Original stereo Phaser-Flanger-Doubler (`hex.phaser_flanger`) for the
//! registered native host (`#2684`).
//!
//! # Modes
//! - `phaser`: 2–12 cascaded 1st-order bilinear all-pass stages per channel
//!   with LFO-swept pole frequency and soft-clipped bipolar feedback. Blending
//!   with dry creates deep harmonic notches.
//! - `flanger`: Short cubic-Hermite interpolated delay line (`0.1..20 ms`) with
//!   LFO sweep and soft-clipped bipolar feedback for classic metallic comb
//!   filtering.
//! - `doubler`: Modulated stereo doubling delay (`delay + 10 ms`, bounded to
//!   inline ring capacity) with stereo LFO phase offset and feedback bypassed.
//!
//! # Saved-State Compatibility
//! - **Version 1 (current)**: 10 little-endian 32-bit words (40 bytes) in
//!   descriptor order (`mode`, `notches`, `center_freq`, `delay_ms`,
//!   `feedback`, `lfo_rate`, `lfo_amount`, `lfo_shape`, `lfo_phase`,
//!   `dry_wet`).
//! - Newer (`> 1`), zero, or malformed payloads are refused explicitly without
//!   mutating or discarding project data.

use hex_dsp::{OnePoleSmoother, flush_quiet};
use hex_lattice::{
    DevicePreparationContext, NativeDeviceDefinition, NativeProcessorRuntime, OutputTail,
    PreparedNativeProcessor, PreparedProcessorFactory, PreparedRuntimeDevice,
    ProcessorInstanceIdentity, StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterModulation, ParameterNormalization,
    ParameterPresentation, ParameterSemantics, ParameterSmoothing, ParameterValue,
};
use std::any::Any;

pub const PHASER_FLANGER_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.phaser_flanger");
pub const PHASER_FLANGER_STATE_VERSION: u32 = 1;
const STATE_WORDS: usize = 10;
const STATE_BYTES: usize = STATE_WORDS * size_of::<u32>();
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const SMOOTHING_MS: f64 = 5.0;
const MAX_ALLPASS_STAGES: usize = 12;
const DELAY_RING_LEN: usize = 4_096;
const DELAY_RING_MASK: usize = DELAY_RING_LEN - 1;
const SH_SEEDS: [u32; 2] = [0x3c6e_f372, 0xa54f_f53a];

pub const MODES: &[&str] = &["phaser", "flanger", "doubler"];
pub const NOTCH_CHOICES: &[&str] = &["2", "4", "6", "8", "10", "12"];
pub const LFO_SHAPES: &[&str] = &["sine", "triangle", "saw up", "saw down", "s&h"];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const MODE: ParameterId = ParameterId::from_static(1);
    pub const NOTCHES: ParameterId = ParameterId::from_static(2);
    pub const CENTER_FREQ: ParameterId = ParameterId::from_static(3);
    pub const DELAY_MS: ParameterId = ParameterId::from_static(4);
    pub const FEEDBACK: ParameterId = ParameterId::from_static(5);
    pub const LFO_RATE: ParameterId = ParameterId::from_static(6);
    pub const LFO_AMOUNT: ParameterId = ParameterId::from_static(7);
    pub const LFO_SHAPE: ParameterId = ParameterId::from_static(8);
    pub const LFO_PHASE: ParameterId = ParameterId::from_static(9);
    pub const DRY_WET: ParameterId = ParameterId::from_static(10);
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    mode: u32,
    notches: u32,
    center_freq_hz: f32,
    delay_ms: f32,
    feedback: f32,
    lfo_rate_hz: f32,
    lfo_amount: f32,
    lfo_shape: u32,
    lfo_phase_deg: f32,
    dry_wet: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: 0,
            notches: 2, // 6 stages
            center_freq_hz: 800.0,
            delay_ms: 2.5,
            feedback: 0.35,
            lfo_rate_hz: 0.5,
            lfo_amount: 0.5,
            lfo_shape: 0,
            lfo_phase_deg: 90.0,
            dry_wet: 0.5,
        }
    }
}

const SCALAR_SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    modulation: ParameterModulation::SampleRate,
    ..crate::parameter_binding::SCALAR_ENVELOPE
};

const LOG_SEMANTICS: ParameterSemantics = ParameterSemantics {
    normalization: ParameterNormalization::PositiveLogarithmic,
    smoothing: ParameterSmoothing::DeviceManaged,
    modulation: ParameterModulation::SampleRate,
    ..crate::parameter_binding::SCALAR_ENVELOPE
};

const CHOICE_SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    ..crate::parameter_binding::CHOICE_ENVELOPE
};

const fn float_desc(
    id: ParameterId,
    label: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: Option<&'static str>,
    presentation: ParameterPresentation,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id,
        label,
        kind: ParameterKind::Float { min, max, unit },
        default: ParameterValue::Float(default),
        presentation,
    }
}

const BINDINGS: [crate::parameter_binding::Binding<Settings>; STATE_WORDS] = [
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::MODE,
            label: "mode",
            kind: ParameterKind::Choice { labels: MODES },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        CHOICE_SEMANTICS,
        |s| s.mode,
        |s, v| {
            if (v as usize) < MODES.len() {
                s.mode = v;
                Ok(())
            } else {
                Err(DeviceContractError::InvalidParameterValue)
            }
        },
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::NOTCHES,
            label: "notches",
            kind: ParameterKind::Choice {
                labels: NOTCH_CHOICES,
            },
            default: ParameterValue::Choice(2),
            presentation: ParameterPresentation::Linear,
        },
        CHOICE_SEMANTICS,
        |s| s.notches,
        |s, v| {
            if (v as usize) < NOTCH_CHOICES.len() {
                s.notches = v;
                Ok(())
            } else {
                Err(DeviceContractError::InvalidParameterValue)
            }
        },
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::CENTER_FREQ,
            "center freq",
            50.0,
            12_000.0,
            800.0,
            Some("hz"),
            ParameterPresentation::Logarithmic,
        ),
        LOG_SEMANTICS,
        |s| s.center_freq_hz,
        |s, v| s.center_freq_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::DELAY_MS,
            "delay",
            0.1,
            20.0,
            2.5,
            Some("ms"),
            ParameterPresentation::Logarithmic,
        ),
        LOG_SEMANTICS,
        |s| s.delay_ms,
        |s, v| s.delay_ms = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::FEEDBACK,
            "feedback",
            -0.95,
            0.95,
            0.35,
            None,
            ParameterPresentation::Linear,
        ),
        SCALAR_SEMANTICS,
        |s| s.feedback,
        |s, v| s.feedback = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::LFO_RATE,
            "lfo rate",
            0.02,
            20.0,
            0.5,
            Some("hz"),
            ParameterPresentation::Logarithmic,
        ),
        LOG_SEMANTICS,
        |s| s.lfo_rate_hz,
        |s, v| s.lfo_rate_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::LFO_AMOUNT,
            "lfo amount",
            0.0,
            1.0,
            0.5,
            None,
            ParameterPresentation::Linear,
        ),
        SCALAR_SEMANTICS,
        |s| s.lfo_amount,
        |s, v| s.lfo_amount = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::LFO_SHAPE,
            label: "lfo shape",
            kind: ParameterKind::Choice { labels: LFO_SHAPES },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        CHOICE_SEMANTICS,
        |s| s.lfo_shape,
        |s, v| {
            if (v as usize) < LFO_SHAPES.len() {
                s.lfo_shape = v;
                Ok(())
            } else {
                Err(DeviceContractError::InvalidParameterValue)
            }
        },
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::LFO_PHASE,
            "lfo phase",
            0.0,
            180.0,
            90.0,
            Some("deg"),
            ParameterPresentation::Linear,
        ),
        SCALAR_SEMANTICS,
        |s| s.lfo_phase_deg,
        |s, v| s.lfo_phase_deg = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::DRY_WET,
            "dry/wet",
            0.0,
            1.0,
            0.5,
            None,
            ParameterPresentation::Linear,
        ),
        SCALAR_SEMANTICS,
        |s| s.dry_wet,
        |s, v| s.dry_wet = v,
    ),
];

const DESCRIPTORS: &[ParameterDescriptor] =
    &crate::parameter_binding::descriptors(&BINDINGS, BINDINGS[0].descriptor);

fn validate(settings: Settings) -> Result<(), DeviceContractError> {
    for binding in &BINDINGS {
        binding.descriptor.validate(binding.read(&settings))?;
    }
    Ok(())
}

fn decode(state: &NativeDeviceState) -> Result<Settings, DeviceContractError> {
    if state.type_id() != PHASER_FLANGER_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != PHASER_FLANGER_STATE_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    if state.payload().len() != STATE_BYTES {
        return Err(DeviceContractError::MalformedState);
    }
    let mut words = [0u32; STATE_WORDS];
    for (slot, chunk) in words.iter_mut().zip(state.payload().chunks_exact(4)) {
        let bytes: [u8; 4] = chunk
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        *slot = u32::from_le_bytes(bytes);
    }
    let settings = Settings {
        mode: words[0],
        notches: words[1],
        center_freq_hz: f32::from_bits(words[2]),
        delay_ms: f32::from_bits(words[3]),
        feedback: f32::from_bits(words[4]),
        lfo_rate_hz: f32::from_bits(words[5]),
        lfo_amount: f32::from_bits(words[6]),
        lfo_shape: words[7],
        lfo_phase_deg: f32::from_bits(words[8]),
        dry_wet: f32::from_bits(words[9]),
    };
    validate(settings)?;
    Ok(settings)
}

fn encode(settings: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let words = [
        settings.mode,
        settings.notches,
        settings.center_freq_hz.to_bits(),
        settings.delay_ms.to_bits(),
        settings.feedback.to_bits(),
        settings.lfo_rate_hz.to_bits(),
        settings.lfo_amount.to_bits(),
        settings.lfo_shape,
        settings.lfo_phase_deg.to_bits(),
        settings.dry_wet.to_bits(),
    ];
    let mut bytes = Vec::with_capacity(STATE_BYTES);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    NativeDeviceState::new(PHASER_FLANGER_TYPE_ID, PHASER_FLANGER_STATE_VERSION, bytes)
}

fn automation_supported(parameter: ParameterId) -> bool {
    crate::parameter_binding::binding(&BINDINGS, parameter)
        .is_ok_and(crate::parameter_binding::Binding::declares_arrangement_automation)
}

pub struct PhaserFlangerDefinition;
pub static PHASER_FLANGER_DEFINITION: PhaserFlangerDefinition = PhaserFlangerDefinition;

impl NativeDeviceMetadata for PhaserFlangerDefinition {
    fn type_id(&self) -> DeviceTypeId {
        PHASER_FLANGER_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("phaser-flanger")
    }
    fn state_version(&self) -> u32 {
        PHASER_FLANGER_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        encode(Settings::default())
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
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<ParameterSemantics>, DeviceContractError> {
        crate::parameter_binding::parameter_semantics_for_state(&BINDINGS, state, id, decode)
    }
}

impl PhaserFlangerDefinition {
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedPhaserFlanger, DeviceContractError> {
        state.require_asset_free_processing()?;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate)
            || parameters.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = decode(state)?;
        let mut admitted = [false; STATE_WORDS];
        for &parameter in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, parameter)?;
            binding.semantics.validate(binding.descriptor)?;
            if !automation_supported(parameter) {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            if let Some(index) = BINDINGS.iter().position(|b| b.descriptor.id == parameter) {
                admitted[index] = true;
            }
        }
        let smoothing = hex_dsp::one_pole_coefficient(sample_rate, SMOOTHING_MS * 0.001)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let make_smoother = |initial: f64| -> Result<OnePoleSmoother, DeviceContractError> {
            OnePoleSmoother::from_coefficient(initial, smoothing)
                .and_then(|s| s.with_snap_epsilon(1.0e-6))
                .map_err(|_| DeviceContractError::InvalidParameterValue)
        };
        Ok(PreparedPhaserFlanger {
            settings,
            sample_rate,
            admitted,
            center_freq: make_smoother(f64::from(settings.center_freq_hz))?,
            delay_ms: make_smoother(f64::from(settings.delay_ms))?,
            feedback: make_smoother(f64::from(settings.feedback))?,
            lfo_rate: make_smoother(f64::from(settings.lfo_rate_hz))?,
            lfo_amount: make_smoother(f64::from(settings.lfo_amount))?,
            lfo_phase: make_smoother(f64::from(settings.lfo_phase_deg))?,
            dry_wet: make_smoother(f64::from(settings.dry_wet))?,
            silence_frames: sample_rate.saturating_mul(2),
        })
    }
}

impl NativeDeviceDefinition for PhaserFlangerDefinition {
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
        Ok(automation_supported(parameter))
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        parameters: &[ParameterId],
    ) -> Result<PreparedRuntimeDevice, DeviceContractError> {
        Ok(crate::inline_owned::prepared_owned_runtime(
            Self::prepare_owned(state, context.sample_rate, parameters)?,
        ))
    }
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(crate::inline_owned::owned_factory(
            Self::prepare_owned(state, context.sample_rate, &[])?,
        )))
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
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        Ok(Some(Box::new(Self::prepare_owned(
            state,
            context.sample_rate,
            &[],
        )?)))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PreparedPhaserFlanger {
    settings: Settings,
    sample_rate: u32,
    admitted: [bool; STATE_WORDS],
    center_freq: OnePoleSmoother,
    delay_ms: OnePoleSmoother,
    feedback: OnePoleSmoother,
    lfo_rate: OnePoleSmoother,
    lfo_amount: OnePoleSmoother,
    lfo_phase: OnePoleSmoother,
    dry_wet: OnePoleSmoother,
    silence_frames: u32,
}

impl PreparedPhaserFlanger {
    fn supports(&self, parameter: ParameterId) -> bool {
        BINDINGS
            .iter()
            .position(|b| b.descriptor.id == parameter)
            .is_some_and(|index| self.admitted[index])
    }
}

impl PreparedNativeProcessor for PreparedPhaserFlanger {
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
    fn parameter_modulation_semantics(&self, parameter: ParameterId) -> Option<ParameterSemantics> {
        self.supports(parameter)
            .then(|| {
                crate::parameter_binding::binding(&BINDINGS, parameter)
                    .ok()
                    .map(|b| b.semantics)
            })
            .flatten()
    }
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        size_of::<PhaserFlangerRuntime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(self))
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(self.silence_frames),
        }
    }
}

impl crate::inline_owned::InlinePreparedProcessor for PreparedPhaserFlanger {
    type Runtime = PhaserFlangerRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        PhaserFlangerRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(self.settings, self.admitted),
            mode: self.settings.mode,
            notches: self.settings.notches,
            lfo_shape: self.settings.lfo_shape,
            center_freq: self.center_freq,
            delay_ms: self.delay_ms,
            feedback: self.feedback,
            lfo_rate: self.lfo_rate,
            lfo_amount: self.lfo_amount,
            lfo_phase: self.lfo_phase,
            dry_wet: self.dry_wet,
            lfo_cycle: 0.0,
            channels: [
                ChannelState::new(SH_SEEDS[0]),
                ChannelState::new(SH_SEEDS[1]),
            ],
            silent_frames: 0,
        }
    }
}

/// First-order bilinear all-pass coefficient with `-90°` phase shift at `freq_hz`.
fn allpass_coeff(freq_hz: f64, sample_rate: f64) -> f64 {
    let clamped = freq_hz.clamp(20.0, sample_rate * 0.45);
    let tan_w = (std::f64::consts::PI * clamped / sample_rate).tan();
    (tan_w - 1.0) / (tan_w + 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ChannelState {
    allpass_z: [f64; MAX_ALLPASS_STAGES],
    feedback_sample: f64,
    ring: [f32; DELAY_RING_LEN],
    write_pos: usize,
    sh_value: f64,
    sh_last_phase: f64,
    rng: u32,
    seed: u32,
}

impl ChannelState {
    const fn new(seed: u32) -> Self {
        Self {
            allpass_z: [0.0; MAX_ALLPASS_STAGES],
            feedback_sample: 0.0,
            ring: [0.0; DELAY_RING_LEN],
            write_pos: 0,
            sh_value: 0.0,
            sh_last_phase: 1.0,
            rng: seed,
            seed,
        }
    }

    fn is_clear(&self) -> bool {
        self.write_pos == 0
            && self.feedback_sample == 0.0
            && self.allpass_z == [0.0; MAX_ALLPASS_STAGES]
            && self.rng == self.seed
            && self.ring.iter().all(|&s| s == 0.0)
    }

    fn next_sh(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        f64::from(x) / 2_147_483_648.0 - 1.0
    }

    fn eval_lfo(&mut self, phase: f64, shape: u32) -> f64 {
        let p = phase - phase.floor();
        if p < self.sh_last_phase {
            self.sh_value = self.next_sh();
        }
        self.sh_last_phase = p;
        match shape {
            0 => (std::f64::consts::TAU * p).sin(),
            1 => {
                if p < 0.25 {
                    4.0 * p
                } else if p < 0.75 {
                    2.0 - 4.0 * p
                } else {
                    4.0 * p - 4.0
                }
            }
            2 => 2.0 * p - 1.0,
            3 => 1.0 - 2.0 * p,
            _ => self.sh_value,
        }
    }

    fn read_ring_hermite(&self, delay_frames: f64) -> f64 {
        let clamped = delay_frames.clamp(1.0, (DELAY_RING_LEN - 4) as f64);
        let int_delay = clamped.floor() as usize;
        let frac = clamped - int_delay as f64;
        let idx0 = self.write_pos.wrapping_sub(int_delay) & DELAY_RING_MASK;
        let idx1 = idx0.wrapping_sub(1) & DELAY_RING_MASK;
        let idx_m1 = idx0.wrapping_add(1) & DELAY_RING_MASK;
        let idx2 = idx1.wrapping_sub(1) & DELAY_RING_MASK;
        let ym1 = f64::from(self.ring[idx_m1]);
        let y0 = f64::from(self.ring[idx0]);
        let y1 = f64::from(self.ring[idx1]);
        let y2 = f64::from(self.ring[idx2]);
        let c0 = y0;
        let c1 = 0.5 * (y1 - ym1);
        let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
        let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
        flush_quiet(((c3 * frac + c2) * frac + c1) * frac + c0)
    }

    fn process_phaser(&mut self, input: f64, stages: usize, coeff: f64, feedback: f64) -> f64 {
        let mut x = input + (self.feedback_sample * feedback).tanh();
        for z in &mut self.allpass_z[..stages] {
            let y = coeff * x + *z;
            *z = flush_quiet(x - coeff * y);
            x = y;
        }
        self.feedback_sample = flush_quiet(x.tanh());
        x
    }

    fn process_delay_mode(&mut self, input: f64, delay_frames: f64, feedback: f64) -> f64 {
        let delayed = self.read_ring_hermite(delay_frames);
        let fb = if feedback.abs() > 1.0e-6 {
            (delayed * feedback).tanh()
        } else {
            0.0
        };
        let to_write = flush_quiet((input + fb).clamp(-4.0, 4.0));
        self.ring[self.write_pos] = to_write as f32;
        self.write_pos = (self.write_pos + 1) & DELAY_RING_MASK;
        delayed
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PhaserFlangerRuntime {
    base: PreparedPhaserFlanger,
    controls: crate::parameter_binding::Controls<Settings, STATE_WORDS>,
    mode: u32,
    notches: u32,
    lfo_shape: u32,
    center_freq: OnePoleSmoother,
    delay_ms: OnePoleSmoother,
    feedback: OnePoleSmoother,
    lfo_rate: OnePoleSmoother,
    lfo_amount: OnePoleSmoother,
    lfo_phase: OnePoleSmoother,
    dry_wet: OnePoleSmoother,
    lfo_cycle: f64,
    channels: [ChannelState; 2],
    silent_frames: u32,
}

impl PhaserFlangerRuntime {
    fn clear_history(&mut self) {
        self.lfo_cycle = 0.0;
        self.channels = [
            ChannelState::new(SH_SEEDS[0]),
            ChannelState::new(SH_SEEDS[1]),
        ];
    }

    fn apply_targets(&mut self, settings: Settings) {
        self.mode = settings.mode;
        self.notches = settings.notches;
        self.lfo_shape = settings.lfo_shape;
        self.center_freq
            .set_target(f64::from(settings.center_freq_hz));
        self.delay_ms.set_target(f64::from(settings.delay_ms));
        self.feedback.set_target(f64::from(settings.feedback));
        self.lfo_rate.set_target(f64::from(settings.lfo_rate_hz));
        self.lfo_amount.set_target(f64::from(settings.lfo_amount));
        self.lfo_phase.set_target(f64::from(settings.lfo_phase_deg));
        self.dry_wet.set_target(f64::from(settings.dry_wet));
    }

    fn process_frame(&mut self, frame: &mut StereoFrame) {
        let center_freq = self.center_freq.advance();
        let delay_ms = self.delay_ms.advance();
        let feedback = self.feedback.advance();
        let lfo_rate = self.lfo_rate.advance();
        let lfo_amount = self.lfo_amount.advance();
        let lfo_phase_cycles = self.lfo_phase.advance() / 360.0;
        let wet_mix = self.dry_wet.advance();

        if !frame.iter().all(|s| s.is_finite()) {
            *frame = [0.0; 2];
        }
        if *frame == [0.0; 2] {
            let limit = self.base.silence_frames;
            self.silent_frames = self.silent_frames.saturating_add(1).min(limit);
            if self.silent_frames == limit {
                if !self.channels.iter().all(ChannelState::is_clear) {
                    self.clear_history();
                }
                return;
            }
        } else {
            self.silent_frames = 0;
        }

        let sr = f64::from(self.base.sample_rate);
        let stages = ((self.notches as usize + 1) * 2).clamp(2, MAX_ALLPASS_STAGES);
        let base_phase = self.lfo_cycle;
        self.lfo_cycle += lfo_rate / sr;
        if self.lfo_cycle >= 1.0 {
            self.lfo_cycle -= self.lfo_cycle.floor();
        }

        for (ch_index, (sample, channel)) in frame.iter_mut().zip(&mut self.channels).enumerate() {
            let dry = f64::from(*sample);
            let ch_phase = if ch_index == 0 {
                base_phase
            } else {
                base_phase + lfo_phase_cycles
            };
            let lfo = channel.eval_lfo(ch_phase, self.lfo_shape);
            let wet = match self.mode {
                0 => {
                    // Phaser: LFO sweeps center frequency up to +/- 2.5 octaves.
                    let freq = center_freq * 2.0_f64.powf(lfo * lfo_amount * 2.5);
                    let coeff = allpass_coeff(freq, sr);
                    channel.process_phaser(dry, stages, coeff, feedback)
                }
                1 => {
                    // Flanger: short modulated delay line with bipolar feedback.
                    let mod_ms = (delay_ms * (1.0 + 0.85 * lfo_amount * lfo)).max(0.05);
                    let delay_frames = mod_ms * 0.001 * sr;
                    channel.process_delay_mode(dry, delay_frames, feedback)
                }
                _ => {
                    // Doubler: longer stereo delay without feedback.
                    let base_ms = delay_ms + 10.0;
                    let mod_ms = (base_ms + 4.0 * lfo_amount * lfo).max(1.0);
                    let delay_frames = mod_ms * 0.001 * sr;
                    channel.process_delay_mode(dry, delay_frames, 0.0)
                }
            };
            *sample = if wet_mix == 0.0 {
                dry as f32
            } else {
                (dry + (wet - dry) * wet_mix) as f32
            };
        }
    }

    fn process_automated(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn hex_lattice::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) = self
                .controls
                .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                self.apply_targets(next.effective());
                self.controls.install(next);
            }
            let original = *frame;
            self.process_frame(frame);
            if bypassed {
                *frame = original;
            }
        }
        Ok(())
    }
}

impl NativeProcessorRuntime for PhaserFlangerRuntime {
    fn supports_parameter_automation(&self, parameter: ParameterId) -> bool {
        self.base.supports(parameter)
    }
    crate::parameter_binding::delegate_automated_block_processing!(process_automated);
    fn process_block(&mut self, frames: &mut [StereoFrame], _: &dyn hex_lattice::DeviceResources) {
        for frame in frames {
            self.process_frame(frame);
        }
    }
    fn process_bypassed_block(
        &mut self,
        frames: &mut [StereoFrame],
        _: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            let mut scratch = *frame;
            self.process_frame(&mut scratch);
        }
    }
    fn reset(&mut self) {
        self.controls.reset();
        self.mode = self.base.settings.mode;
        self.notches = self.base.settings.notches;
        self.lfo_shape = self.base.settings.lfo_shape;
        self.center_freq = self.base.center_freq;
        self.delay_ms = self.base.delay_ms;
        self.feedback = self.base.feedback;
        self.lfo_rate = self.base.lfo_rate;
        self.lfo_amount = self.base.lfo_amount;
        self.lfo_phase = self.base.lfo_phase;
        self.dry_wet = self.base.dry_wet;
        self.clear_history();
        self.silent_frames = 0;
    }
    fn is_quiescent(&self) -> bool {
        self.channels.iter().all(ChannelState::is_clear)
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| previous.base.sample_rate == self.base.sample_rate)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|previous| previous.base.sample_rate == self.base.sample_rate)
        else {
            return;
        };
        self.channels = previous.channels;
        self.lfo_cycle = previous.lfo_cycle;
        self.silent_frames = previous.silent_frames;
        self.center_freq.inherit_value(&previous.center_freq);
        self.delay_ms.inherit_value(&previous.delay_ms);
        self.feedback.inherit_value(&previous.feedback);
        self.lfo_rate.inherit_value(&previous.lfo_rate);
        self.lfo_amount.inherit_value(&previous.lfo_amount);
        self.lfo_phase.inherit_value(&previous.lfo_phase);
        self.dry_wet.inherit_value(&previous.dry_wet);
        if self.base.admitted == previous.base.admitted && self.base.admitted.iter().any(|&a| a) {
            self.controls.inherit(&previous.controls);
            self.center_freq.set_target(previous.center_freq.target());
            self.delay_ms.set_target(previous.delay_ms.target());
            self.feedback.set_target(previous.feedback.target());
            self.lfo_rate.set_target(previous.lfo_rate.target());
            self.lfo_amount.set_target(previous.lfo_amount.target());
            self.lfo_phase.set_target(previous.lfo_phase.target());
            self.dry_wet.set_target(previous.dry_wet.target());
        }
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        self.can_copy_history_from(previous)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_rms(state: &NativeDeviceState, sample_rate: u32, freq_hz: f64) -> f64 {
        let prep = PhaserFlangerDefinition::prepare_owned(state, sample_rate, &[]).unwrap();
        let mut rt = crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep);
        let total = 8_192;
        let settle = 4_096;
        let mut frames: Vec<StereoFrame> = (0..total)
            .map(|i| {
                let x = (std::f64::consts::TAU * freq_hz * i as f64 / f64::from(sample_rate)).sin()
                    as f32;
                [x, x]
            })
            .collect();
        rt.process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let energy: f64 = frames[settle..]
            .iter()
            .map(|f| f64::from(f[0]) * f64::from(f[0]))
            .sum();
        (energy / (total - settle) as f64).sqrt()
    }

    #[test]
    fn phaser_creates_exact_notch_and_flanger_creates_comb_cancellation() {
        let base = PHASER_FLANGER_DEFINITION.default_state().unwrap();
        // 2-stage phaser at 1000 Hz, 50/50 dry/wet, no feedback, static LFO -> exact -180 deg notch at 1000 Hz!
        let phaser = PHASER_FLANGER_DEFINITION
            .patch_parameter(&base, parameter::MODE, ParameterValue::Choice(0))
            .and_then(|s| {
                PHASER_FLANGER_DEFINITION.patch_parameter(
                    &s,
                    parameter::NOTCHES,
                    ParameterValue::Choice(0), // 2 stages
                )
            })
            .and_then(|s| {
                PHASER_FLANGER_DEFINITION.patch_parameter(
                    &s,
                    parameter::CENTER_FREQ,
                    ParameterValue::Float(1_000.0),
                )
            })
            .and_then(|s| {
                PHASER_FLANGER_DEFINITION.patch_parameter(
                    &s,
                    parameter::FEEDBACK,
                    ParameterValue::Float(0.0),
                )
            })
            .and_then(|s| {
                PHASER_FLANGER_DEFINITION.patch_parameter(
                    &s,
                    parameter::LFO_AMOUNT,
                    ParameterValue::Float(0.0),
                )
            })
            .and_then(|s| {
                PHASER_FLANGER_DEFINITION.patch_parameter(
                    &s,
                    parameter::DRY_WET,
                    ParameterValue::Float(0.5),
                )
            })
            .unwrap();
        let notch_1k = sine_rms(&phaser, 48_000, 1_000.0);
        let pass_100 = sine_rms(&phaser, 48_000, 100.0);
        assert!(
            20.0 * (notch_1k / pass_100).log10() < -35.0,
            "2-stage phaser must deeply cancel 1000 hz notch (notch={notch_1k:.5}, pass={pass_100:.5})"
        );

        // Flanger with 1.0 ms static delay cancels 500 Hz and reinforces 1000 Hz.
        let flanger = PHASER_FLANGER_DEFINITION
            .patch_parameter(&phaser, parameter::MODE, ParameterValue::Choice(1))
            .and_then(|s| {
                PHASER_FLANGER_DEFINITION.patch_parameter(
                    &s,
                    parameter::DELAY_MS,
                    ParameterValue::Float(1.0),
                )
            })
            .unwrap();
        let comb_null_500 = sine_rms(&flanger, 48_000, 500.0);
        let comb_peak_1000 = sine_rms(&flanger, 48_000, 1_000.0);
        assert!(
            20.0 * (comb_null_500 / comb_peak_1000).log10() < -30.0,
            "1 ms flanger must cancel 500 hz relative to 1000 hz ({comb_null_500:.5} vs {comb_peak_1000:.5})"
        );
    }

    #[test]
    fn feedback_stays_bounded_doubler_widens_stereo_and_partition_invariance_holds() {
        let base = PHASER_FLANGER_DEFINITION.default_state().unwrap();
        let high_fb = PHASER_FLANGER_DEFINITION
            .patch_parameter(&base, parameter::MODE, ParameterValue::Choice(1))
            .and_then(|s| {
                PHASER_FLANGER_DEFINITION.patch_parameter(
                    &s,
                    parameter::FEEDBACK,
                    ParameterValue::Float(0.95),
                )
            })
            .and_then(|s| {
                PHASER_FLANGER_DEFINITION.patch_parameter(
                    &s,
                    parameter::LFO_SHAPE,
                    ParameterValue::Choice(4), // S&H
                )
            })
            .unwrap();

        let prep = PhaserFlangerDefinition::prepare_owned(&high_fb, 48_000, &[]).unwrap();
        let mut single = crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep);
        let mut chunked =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep);
        let mut input = vec![[0.0f32; 2]; 1_024];
        input[0] = [1.0, 1.0];
        let mut out_single = input.clone();
        single.process_block(&mut out_single, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(
            out_single
                .iter()
                .all(|f| f[0].is_finite() && f[1].is_finite() && f[0].abs() <= 2.5),
            "high feedback must remain strictly finite and bounded"
        );

        let mut out_chunked = input;
        let mut pos = 0;
        for size in [1, 7, 19, 64, 193, 740] {
            let end = (pos + size).min(out_chunked.len());
            chunked.process_block(
                &mut out_chunked[pos..end],
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            pos = end;
        }
        assert_eq!(out_single, out_chunked);

        // Doubler with 180 deg phase widens mono input into stereo difference.
        let doubler = PHASER_FLANGER_DEFINITION
            .patch_parameter(&base, parameter::MODE, ParameterValue::Choice(2))
            .and_then(|s| {
                PHASER_FLANGER_DEFINITION.patch_parameter(
                    &s,
                    parameter::LFO_PHASE,
                    ParameterValue::Float(180.0),
                )
            })
            .unwrap();
        let prep_d = PhaserFlangerDefinition::prepare_owned(&doubler, 48_000, &[]).unwrap();
        let mut rt_d = crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep_d);
        let mut mono: Vec<StereoFrame> = (0..4_096)
            .map(|i| {
                let x = (std::f64::consts::TAU * 330.0 * i as f64 / 48_000.0).sin() as f32;
                [x, x]
            })
            .collect();
        rt_d.process_block(&mut mono, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let diff_energy: f64 = mono[1_024..]
            .iter()
            .map(|f| {
                let d = f64::from(f[0] - f[1]);
                d * d
            })
            .sum();
        assert!(diff_energy > 1.0, "doubler must produce stereo width");

        // Malformed states refused cleanly.
        assert!(
            decode(
                &NativeDeviceState::new(PHASER_FLANGER_TYPE_ID, 2, vec![0; STATE_BYTES]).unwrap()
            )
            .is_err()
        );
        assert!(
            decode(&NativeDeviceState::new(PHASER_FLANGER_TYPE_ID, 1, vec![0; 8]).unwrap())
                .is_err()
        );
    }
}
