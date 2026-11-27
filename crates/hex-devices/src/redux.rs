//! Original stereo digital sample-rate reducer and bitcrusher (`hex.redux`) for
//! the registered native host (`#2685`).
//!
//! # Signal Path
//! 1. Optional pre-filter anti-aliasing 2nd-order Butterworth low-pass tracking
//!    `0.45 * min(rate, sample_rate)` (5 ms crossfade when toggled).
//! 2. Fractional sample-and-hold clock downsampler (`20..40000 Hz`) with
//!    deterministic seeded per-channel clock jitter (`0..1`).
//! 3. Continuous bit-depth quantizer (`1..16` bits) with adjustable companding
//!    `shape` (`0` = uniform mid-tread staircase, `1` = mu-law-style soft
//!    companding that preserves low-level detail).
//! 4. Optional post-filter reconstruction low-pass tracking
//!    `0.45 * min(rate, sample_rate)` to smooth staircase imaging.
//! 5. Smooth `dry/wet` mix.
//!
//! # Saved-State Compatibility
//! - **Version 1 (current)**: 7 little-endian 32-bit words (28 bytes) in
//!   descriptor order (`rate`, `jitter`, `bits`, `shape`, `pre_filter`,
//!   `post_filter`, `dry_wet`).
//! - Newer (`> 1`), zero, or malformed payloads are refused explicitly without
//!   mutating or discarding project data.

use hex_dsp::{BiquadCoefficients, OnePoleSmoother, flush_quiet};
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

pub const REDUX_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.redux");
pub const REDUX_STATE_VERSION: u32 = 1;
const STATE_WORDS: usize = 7;
const STATE_BYTES: usize = STATE_WORDS * size_of::<u32>();
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const SMOOTHING_MS: f64 = 5.0;
const JITTER_SEEDS: [u32; 2] = [0x243f_6a88, 0x85a3_08d3];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const RATE: ParameterId = ParameterId::from_static(1);
    pub const JITTER: ParameterId = ParameterId::from_static(2);
    pub const BITS: ParameterId = ParameterId::from_static(3);
    pub const SHAPE: ParameterId = ParameterId::from_static(4);
    pub const PRE_FILTER: ParameterId = ParameterId::from_static(5);
    pub const POST_FILTER: ParameterId = ParameterId::from_static(6);
    pub const DRY_WET: ParameterId = ParameterId::from_static(7);
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    rate_hz: f32,
    jitter: f32,
    bits: f32,
    shape: f32,
    pre_filter: bool,
    post_filter: bool,
    dry_wet: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            rate_hz: 40_000.0,
            jitter: 0.0,
            bits: 16.0,
            shape: 0.0,
            pre_filter: false,
            post_filter: false,
            dry_wet: 1.0,
        }
    }
}

const SCALAR_SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    modulation: ParameterModulation::SampleRate,
    ..crate::parameter_binding::SCALAR_ENVELOPE
};

const RATE_SEMANTICS: ParameterSemantics = ParameterSemantics {
    normalization: ParameterNormalization::PositiveLogarithmic,
    smoothing: ParameterSmoothing::DeviceManaged,
    modulation: ParameterModulation::SampleRate,
    ..crate::parameter_binding::SCALAR_ENVELOPE
};

const BOOL_SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    ..crate::parameter_binding::BOOLEAN_ENVELOPE
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

const fn bool_desc(id: ParameterId, label: &'static str, default: bool) -> ParameterDescriptor {
    ParameterDescriptor {
        id,
        label,
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(default),
        presentation: ParameterPresentation::Linear,
    }
}

const BINDINGS: [crate::parameter_binding::Binding<Settings>; STATE_WORDS] = [
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::RATE,
            "rate",
            20.0,
            40_000.0,
            40_000.0,
            Some("hz"),
            ParameterPresentation::Logarithmic,
        ),
        RATE_SEMANTICS,
        |s| s.rate_hz,
        |s, v| s.rate_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::JITTER,
            "jitter",
            0.0,
            1.0,
            0.0,
            None,
            ParameterPresentation::Linear,
        ),
        SCALAR_SEMANTICS,
        |s| s.jitter,
        |s, v| s.jitter = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::BITS,
            "bits",
            1.0,
            16.0,
            16.0,
            Some("bit"),
            ParameterPresentation::Linear,
        ),
        SCALAR_SEMANTICS,
        |s| s.bits,
        |s, v| s.bits = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::SHAPE,
            "shape",
            0.0,
            1.0,
            0.0,
            None,
            ParameterPresentation::Linear,
        ),
        SCALAR_SEMANTICS,
        |s| s.shape,
        |s, v| s.shape = v,
    ),
    crate::parameter_binding::Binding::boolean(
        bool_desc(parameter::PRE_FILTER, "pre filter", false),
        BOOL_SEMANTICS,
        |s| s.pre_filter,
        |s, v| s.pre_filter = v,
    ),
    crate::parameter_binding::Binding::boolean(
        bool_desc(parameter::POST_FILTER, "post filter", false),
        BOOL_SEMANTICS,
        |s| s.post_filter,
        |s, v| s.post_filter = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::DRY_WET,
            "dry/wet",
            0.0,
            1.0,
            1.0,
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

fn decode_bool_word(word: u32) -> Result<bool, DeviceContractError> {
    match word {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DeviceContractError::MalformedState),
    }
}

fn decode(state: &NativeDeviceState) -> Result<Settings, DeviceContractError> {
    if state.type_id() != REDUX_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != REDUX_STATE_VERSION {
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
        rate_hz: f32::from_bits(words[0]),
        jitter: f32::from_bits(words[1]),
        bits: f32::from_bits(words[2]),
        shape: f32::from_bits(words[3]),
        pre_filter: decode_bool_word(words[4])?,
        post_filter: decode_bool_word(words[5])?,
        dry_wet: f32::from_bits(words[6]),
    };
    validate(settings)?;
    Ok(settings)
}

fn encode(settings: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let words = [
        settings.rate_hz.to_bits(),
        settings.jitter.to_bits(),
        settings.bits.to_bits(),
        settings.shape.to_bits(),
        u32::from(settings.pre_filter),
        u32::from(settings.post_filter),
        settings.dry_wet.to_bits(),
    ];
    let mut bytes = Vec::with_capacity(STATE_BYTES);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    NativeDeviceState::new(REDUX_TYPE_ID, REDUX_STATE_VERSION, bytes)
}

fn automation_supported(parameter: ParameterId) -> bool {
    crate::parameter_binding::binding(&BINDINGS, parameter)
        .is_ok_and(crate::parameter_binding::Binding::declares_arrangement_automation)
}

pub struct ReduxDefinition;
pub static REDUX_DEFINITION: ReduxDefinition = ReduxDefinition;

impl NativeDeviceMetadata for ReduxDefinition {
    fn type_id(&self) -> DeviceTypeId {
        REDUX_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("redux")
    }
    fn state_version(&self) -> u32 {
        REDUX_STATE_VERSION
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

fn tracking_low_pass(rate_hz: f64, sample_rate: u32) -> BiquadCoefficients {
    let sr = f64::from(sample_rate);
    let cutoff = (0.45 * rate_hz.min(sr)).clamp(20.0, sr * 0.45);
    BiquadCoefficients::butterworth_low_pass(cutoff, sr).unwrap_or(BiquadCoefficients::IDENTITY)
}

/// Quantize a sample to `bits` (`1..=16`) with optional mu-law-style companding `shape`.
fn quantize_sample(sample: f64, bits: f64, shape: f64) -> f64 {
    if bits >= 16.0 {
        return sample;
    }
    let levels = 2.0_f64.powf((bits - 1.0).clamp(0.0, 15.0));
    let mu = 15.0 * shape.clamp(0.0, 1.0);
    if mu <= 1.0e-6 {
        (sample * levels).round() / levels
    } else {
        let sign = sample.signum();
        let mag = sample.abs().min(1.0);
        let compressed = (1.0 + mu * mag).ln() / (1.0 + mu).ln();
        let stepped = (compressed * levels).round() / levels;
        let expanded = ((1.0 + mu).powf(stepped) - 1.0) / mu;
        sign * expanded
    }
}

impl ReduxDefinition {
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedRedux, DeviceContractError> {
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
        let filter = tracking_low_pass(f64::from(settings.rate_hz), sample_rate);
        Ok(PreparedRedux {
            settings,
            sample_rate,
            admitted,
            rate: make_smoother(f64::from(settings.rate_hz))?,
            jitter: make_smoother(f64::from(settings.jitter))?,
            bits: make_smoother(f64::from(settings.bits))?,
            shape: make_smoother(f64::from(settings.shape))?,
            pre_mix: make_smoother(if settings.pre_filter { 1.0 } else { 0.0 })?,
            post_mix: make_smoother(if settings.post_filter { 1.0 } else { 0.0 })?,
            dry_wet: make_smoother(f64::from(settings.dry_wet))?,
            filter,
            silence_frames: sample_rate / 2,
        })
    }
}

impl NativeDeviceDefinition for ReduxDefinition {
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
pub struct PreparedRedux {
    settings: Settings,
    sample_rate: u32,
    admitted: [bool; STATE_WORDS],
    rate: OnePoleSmoother,
    jitter: OnePoleSmoother,
    bits: OnePoleSmoother,
    shape: OnePoleSmoother,
    pre_mix: OnePoleSmoother,
    post_mix: OnePoleSmoother,
    dry_wet: OnePoleSmoother,
    filter: BiquadCoefficients,
    silence_frames: u32,
}

impl PreparedRedux {
    fn supports(&self, parameter: ParameterId) -> bool {
        BINDINGS
            .iter()
            .position(|b| b.descriptor.id == parameter)
            .is_some_and(|index| self.admitted[index])
    }
}

impl PreparedNativeProcessor for PreparedRedux {
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
        size_of::<ReduxRuntime>()
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

impl crate::inline_owned::InlinePreparedProcessor for PreparedRedux {
    type Runtime = ReduxRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        ReduxRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(self.settings, self.admitted),
            rate: self.rate,
            jitter: self.jitter,
            bits: self.bits,
            shape: self.shape,
            pre_mix: self.pre_mix,
            post_mix: self.post_mix,
            dry_wet: self.dry_wet,
            filter: self.filter,
            channels: [
                ChannelState::new(JITTER_SEEDS[0]),
                ChannelState::new(JITTER_SEEDS[1]),
            ],
            silent_frames: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ChannelState {
    pre_lpf: [f64; 2],
    post_lpf: [f64; 2],
    phase: f64,
    threshold: f64,
    held: f64,
    rng: u32,
    seed: u32,
}

impl ChannelState {
    const fn new(seed: u32) -> Self {
        Self {
            pre_lpf: [0.0; 2],
            post_lpf: [0.0; 2],
            phase: 1.0,
            threshold: 1.0,
            held: 0.0,
            rng: seed,
            seed,
        }
    }

    fn is_clear(&self) -> bool {
        *self == Self::new(self.seed)
    }

    fn next_bipolar(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        f64::from(x) / 2_147_483_648.0 - 1.0
    }

    #[allow(clippy::too_many_arguments)]
    fn process(
        &mut self,
        input: f64,
        filter: &BiquadCoefficients,
        rate_ratio: f64,
        jitter: f64,
        bits: f64,
        shape: f64,
        pre_mix: f64,
        post_mix: f64,
    ) -> f64 {
        let pre_filtered = filter.process_tdf2_paired_flush(input, &mut self.pre_lpf);
        let pre = input + (pre_filtered - input) * pre_mix;

        if rate_ratio >= 1.0 && jitter <= 1.0e-6 {
            self.held = pre;
            self.phase = 1.0;
            self.threshold = 1.0;
        } else {
            if self.phase >= self.threshold {
                self.phase -= self.threshold;
                if self.phase >= 1.0 {
                    self.phase = self.phase.fract();
                }
                self.held = pre;
                let jitter_offset = if jitter > 1.0e-6 {
                    self.next_bipolar() * 0.5 * jitter
                } else {
                    0.0
                };
                self.threshold = (1.0 + jitter_offset).clamp(0.25, 1.75);
            }
            self.phase += rate_ratio;
        }

        let crushed = quantize_sample(self.held, bits, shape);
        let post_filtered = filter.process_tdf2_paired_flush(crushed, &mut self.post_lpf);
        flush_quiet(crushed + (post_filtered - crushed) * post_mix)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ReduxRuntime {
    base: PreparedRedux,
    controls: crate::parameter_binding::Controls<Settings, STATE_WORDS>,
    rate: OnePoleSmoother,
    jitter: OnePoleSmoother,
    bits: OnePoleSmoother,
    shape: OnePoleSmoother,
    pre_mix: OnePoleSmoother,
    post_mix: OnePoleSmoother,
    dry_wet: OnePoleSmoother,
    filter: BiquadCoefficients,
    channels: [ChannelState; 2],
    silent_frames: u32,
}

impl ReduxRuntime {
    fn clear_history(&mut self) {
        self.channels = [
            ChannelState::new(JITTER_SEEDS[0]),
            ChannelState::new(JITTER_SEEDS[1]),
        ];
    }

    fn apply_targets(&mut self, settings: Settings) {
        self.rate.set_target(f64::from(settings.rate_hz));
        self.jitter.set_target(f64::from(settings.jitter));
        self.bits.set_target(f64::from(settings.bits));
        self.shape.set_target(f64::from(settings.shape));
        self.pre_mix
            .set_target(if settings.pre_filter { 1.0 } else { 0.0 });
        self.post_mix
            .set_target(if settings.post_filter { 1.0 } else { 0.0 });
        self.dry_wet.set_target(f64::from(settings.dry_wet));
    }

    fn process_frame(&mut self, frame: &mut StereoFrame) {
        let rate_moving = !self.rate.is_settled();
        let rate_hz = self.rate.advance();
        if rate_moving {
            self.filter = tracking_low_pass(rate_hz, self.base.sample_rate);
        }
        let jitter = self.jitter.advance();
        let bits = self.bits.advance();
        let shape = self.shape.advance();
        let pre_mix = self.pre_mix.advance();
        let post_mix = self.post_mix.advance();
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

        let rate_ratio = (rate_hz / f64::from(self.base.sample_rate)).clamp(1.0e-5, 1.0);
        for (sample, channel) in frame.iter_mut().zip(&mut self.channels) {
            let dry = f64::from(*sample);
            let wet = channel.process(
                dry,
                &self.filter,
                rate_ratio,
                jitter,
                bits,
                shape,
                pre_mix,
                post_mix,
            );
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

impl NativeProcessorRuntime for ReduxRuntime {
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
        self.rate = self.base.rate;
        self.jitter = self.base.jitter;
        self.bits = self.base.bits;
        self.shape = self.base.shape;
        self.pre_mix = self.base.pre_mix;
        self.post_mix = self.base.post_mix;
        self.dry_wet = self.base.dry_wet;
        self.filter = self.base.filter;
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
        self.silent_frames = previous.silent_frames;
        self.rate.inherit_value(&previous.rate);
        self.jitter.inherit_value(&previous.jitter);
        self.bits.inherit_value(&previous.bits);
        self.shape.inherit_value(&previous.shape);
        self.pre_mix.inherit_value(&previous.pre_mix);
        self.post_mix.inherit_value(&previous.post_mix);
        self.dry_wet.inherit_value(&previous.dry_wet);
        self.filter = tracking_low_pass(self.rate.value(), self.base.sample_rate);
        if self.base.admitted == previous.base.admitted && self.base.admitted.iter().any(|&a| a) {
            self.controls.inherit(&previous.controls);
            self.rate.set_target(previous.rate.target());
            self.jitter.set_target(previous.jitter.target());
            self.bits.set_target(previous.bits.target());
            self.shape.set_target(previous.shape.target());
            self.pre_mix.set_target(previous.pre_mix.target());
            self.post_mix.set_target(previous.post_mix.target());
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

    #[test]
    fn sample_rate_reduction_holds_exact_staircase_and_bit_quantization_reduces_levels() {
        // 48 kHz -> 6 kHz holds every 8 samples cleanly.
        let base = REDUX_DEFINITION.default_state().unwrap();
        let downsampled = REDUX_DEFINITION
            .patch_parameter(&base, parameter::RATE, ParameterValue::Float(6_000.0))
            .unwrap();
        let prepared = ReduxDefinition::prepare_owned(&downsampled, 48_000, &[]).unwrap();
        let mut runtime =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prepared);
        let mut frames: Vec<StereoFrame> = (0..32).map(|i| [(i as f32) / 32.0; 2]).collect();
        runtime.process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        for chunk in frames.chunks_exact(8) {
            let first = chunk[0][0];
            for sample in chunk {
                assert_eq!(sample[0], first, "8-sample hold staircase must stay flat");
            }
        }

        // 3-bit quantization produces at most 9 distinct values on [-1, 1] ramp.
        let crushed = REDUX_DEFINITION
            .patch_parameter(&base, parameter::BITS, ParameterValue::Float(3.0))
            .unwrap();
        let prepared_bits = ReduxDefinition::prepare_owned(&crushed, 48_000, &[]).unwrap();
        let mut rt_bits =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prepared_bits);
        let mut ramp: Vec<StereoFrame> = (0..401)
            .map(|i| {
                let x = -1.0 + 2.0 * (i as f32) / 400.0;
                [x, x]
            })
            .collect();
        rt_bits.process_block(&mut ramp, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut distinct: Vec<i32> = ramp
            .iter()
            .map(|f| (f[0] * 1_000.0).round() as i32)
            .collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), 9, "3-bit mid-tread quantizer has 9 levels");
    }

    #[test]
    fn post_filter_smooths_staircase_and_jitter_is_block_partition_invariant() {
        let base = REDUX_DEFINITION.default_state().unwrap();
        let raw_state = REDUX_DEFINITION
            .patch_parameter(&base, parameter::RATE, ParameterValue::Float(3_000.0))
            .and_then(|s| {
                REDUX_DEFINITION.patch_parameter(&s, parameter::JITTER, ParameterValue::Float(0.6))
            })
            .unwrap();
        let filtered_state = REDUX_DEFINITION
            .patch_parameter(
                &raw_state,
                parameter::POST_FILTER,
                ParameterValue::Bool(true),
            )
            .unwrap();

        let input: Vec<StereoFrame> = (0..1_024)
            .map(|i| {
                let x = (std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).sin() as f32;
                [x, x]
            })
            .collect();

        // Measure high-frequency step energy (RMS of first difference).
        let hf_step_rms = |state: &NativeDeviceState| -> f64 {
            let prep = ReduxDefinition::prepare_owned(state, 48_000, &[]).unwrap();
            let mut rt = crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep);
            let mut buf = input.clone();
            rt.process_block(&mut buf, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            let sum_sq: f64 = buf
                .windows(2)
                .map(|w| {
                    let d = f64::from(w[1][0] - w[0][0]);
                    d * d
                })
                .sum();
            (sum_sq / buf.len() as f64).sqrt()
        };
        let raw_rough = hf_step_rms(&raw_state);
        let filtered_rough = hf_step_rms(&filtered_state);
        assert!(
            filtered_rough < raw_rough * 0.5,
            "post-filter must reduce staircase step energy by > 2x ({filtered_rough:.5} vs {raw_rough:.5})"
        );

        // Partition invariance with jitter enabled!
        let prep = ReduxDefinition::prepare_owned(&raw_state, 48_000, &[]).unwrap();
        let mut single = crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep);
        let mut chunked =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep);
        let mut out_single = input.clone();
        single.process_block(&mut out_single, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut out_chunked = input;
        let mut pos = 0;
        for size in [1, 3, 13, 64, 191, 752] {
            let end = (pos + size).min(out_chunked.len());
            chunked.process_block(
                &mut out_chunked[pos..end],
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            pos = end;
        }
        assert_eq!(out_single, out_chunked);

        // Finite tail & malformed state refusal.
        let mut silence = vec![[0.0f32; 2]; 25_000];
        single.process_block(&mut silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(single.is_quiescent());
        assert!(
            decode(&NativeDeviceState::new(REDUX_TYPE_ID, 2, vec![0; STATE_BYTES]).unwrap())
                .is_err()
        );
        assert!(decode(&NativeDeviceState::new(REDUX_TYPE_ID, 1, vec![0; 4]).unwrap()).is_err());
    }
}
