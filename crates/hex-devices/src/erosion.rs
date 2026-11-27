//! Original stereo audio-rate phase-modulation degradation effect
//! (`hex.erosion`) for the registered native host (`#2685`).
//!
//! # Signal Path
//! Modulates a short fractional delay line (`0..3 ms`) with cubic Hermite
//! interpolation at audio rate using one of three sources:
//! - `noise`: mono-correlated seeded white noise shaped by a band-pass filter
//!   (`frequency`, `width`).
//! - `wide noise`: independent seeded white noise per channel shaped by the
//!   same band-pass filter, producing stereo-decorrelated grit/air.
//! - `sine`: audio-rate sine carrier at `frequency`, producing clean FM/PM
//!   sidebands (`f_in ± k * f_mod`).
//!
//! At settled `amount == 0.0`, the delay is zero samples and input passes
//! untouched with zero algorithmic latency.
//!
//! # Saved-State Compatibility
//! - **Version 1 (current)**: 4 little-endian 32-bit words (16 bytes) in
//!   descriptor order (`mode`, `frequency`, `width`, `amount`).
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

pub const EROSION_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.erosion");
pub const EROSION_STATE_VERSION: u32 = 1;
const STATE_WORDS: usize = 4;
const STATE_BYTES: usize = STATE_WORDS * size_of::<u32>();
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const SMOOTHING_MS: f64 = 5.0;
const MAX_MOD_DELAY_SECONDS: f64 = 0.003;
/// Power-of-two ring capacity exceeding `192_000 * 0.003 + 4` (= 580) frames.
const DELAY_RING_LEN: usize = 1_024;
const DELAY_RING_MASK: usize = DELAY_RING_LEN - 1;
const NOISE_SEEDS: [u32; 2] = [0x6a09_e667, 0xbb67_ae85];

pub const MODES: &[&str] = &["noise", "wide noise", "sine"];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const MODE: ParameterId = ParameterId::from_static(1);
    pub const FREQUENCY: ParameterId = ParameterId::from_static(2);
    pub const WIDTH: ParameterId = ParameterId::from_static(3);
    pub const AMOUNT: ParameterId = ParameterId::from_static(4);
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    mode: u32,
    frequency_hz: f32,
    width: f32,
    amount: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: 0,
            frequency_hz: 4_000.0,
            width: 1.0,
            amount: 0.0,
        }
    }
}

const SCALAR_SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    modulation: ParameterModulation::SampleRate,
    ..crate::parameter_binding::SCALAR_ENVELOPE
};

const FREQ_SEMANTICS: ParameterSemantics = ParameterSemantics {
    normalization: ParameterNormalization::PositiveLogarithmic,
    smoothing: ParameterSmoothing::DeviceManaged,
    modulation: ParameterModulation::SampleRate,
    ..crate::parameter_binding::SCALAR_ENVELOPE
};

const MODE_SEMANTICS: ParameterSemantics = ParameterSemantics {
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
        MODE_SEMANTICS,
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
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::FREQUENCY,
            "frequency",
            300.0,
            18_000.0,
            4_000.0,
            Some("hz"),
            ParameterPresentation::Logarithmic,
        ),
        FREQ_SEMANTICS,
        |s| s.frequency_hz,
        |s, v| s.frequency_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::WIDTH,
            "width",
            0.1,
            5.0,
            1.0,
            None,
            ParameterPresentation::Linear,
        ),
        SCALAR_SEMANTICS,
        |s| s.width,
        |s, v| s.width = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::AMOUNT,
            "amount",
            0.0,
            1.0,
            0.0,
            None,
            ParameterPresentation::Linear,
        ),
        SCALAR_SEMANTICS,
        |s| s.amount,
        |s, v| s.amount = v,
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
    if state.type_id() != EROSION_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != EROSION_STATE_VERSION {
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
        frequency_hz: f32::from_bits(words[1]),
        width: f32::from_bits(words[2]),
        amount: f32::from_bits(words[3]),
    };
    validate(settings)?;
    Ok(settings)
}

fn encode(settings: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let words = [
        settings.mode,
        settings.frequency_hz.to_bits(),
        settings.width.to_bits(),
        settings.amount.to_bits(),
    ];
    let mut bytes = Vec::with_capacity(STATE_BYTES);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    NativeDeviceState::new(EROSION_TYPE_ID, EROSION_STATE_VERSION, bytes)
}

fn automation_supported(parameter: ParameterId) -> bool {
    crate::parameter_binding::binding(&BINDINGS, parameter)
        .is_ok_and(crate::parameter_binding::Binding::declares_arrangement_automation)
}

pub struct ErosionDefinition;
pub static EROSION_DEFINITION: ErosionDefinition = ErosionDefinition;

impl NativeDeviceMetadata for ErosionDefinition {
    fn type_id(&self) -> DeviceTypeId {
        EROSION_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("erosion")
    }
    fn state_version(&self) -> u32 {
        EROSION_STATE_VERSION
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

fn noise_bandpass(freq_hz: f64, width: f64, sample_rate: u32) -> BiquadCoefficients {
    let sr = f64::from(sample_rate);
    let cutoff = freq_hz.clamp(20.0, sr * 0.45);
    let q = width.clamp(0.1, 5.0);
    BiquadCoefficients::band_pass_peak(cutoff, sr, q).unwrap_or(BiquadCoefficients::IDENTITY)
}

impl ErosionDefinition {
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedErosion, DeviceContractError> {
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
        let bandpass = noise_bandpass(
            f64::from(settings.frequency_hz),
            f64::from(settings.width),
            sample_rate,
        );
        let max_delay_frames =
            (f64::from(sample_rate) * MAX_MOD_DELAY_SECONDS).min((DELAY_RING_LEN - 4) as f64);
        Ok(PreparedErosion {
            settings,
            sample_rate,
            admitted,
            frequency: make_smoother(f64::from(settings.frequency_hz))?,
            width: make_smoother(f64::from(settings.width))?,
            amount: make_smoother(f64::from(settings.amount))?,
            bandpass,
            max_delay_frames,
            silence_frames: (max_delay_frames.ceil() as u32).saturating_add(sample_rate / 4),
        })
    }
}

impl NativeDeviceDefinition for ErosionDefinition {
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
pub struct PreparedErosion {
    settings: Settings,
    sample_rate: u32,
    admitted: [bool; STATE_WORDS],
    frequency: OnePoleSmoother,
    width: OnePoleSmoother,
    amount: OnePoleSmoother,
    bandpass: BiquadCoefficients,
    max_delay_frames: f64,
    silence_frames: u32,
}

impl PreparedErosion {
    fn supports(&self, parameter: ParameterId) -> bool {
        BINDINGS
            .iter()
            .position(|b| b.descriptor.id == parameter)
            .is_some_and(|index| self.admitted[index])
    }
}

impl PreparedNativeProcessor for PreparedErosion {
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
        size_of::<ErosionRuntime>()
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

impl crate::inline_owned::InlinePreparedProcessor for PreparedErosion {
    type Runtime = ErosionRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        ErosionRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(self.settings, self.admitted),
            mode: self.settings.mode,
            frequency: self.frequency,
            width: self.width,
            amount: self.amount,
            bandpass: self.bandpass,
            sine_phase: 0.0,
            channels: [
                ChannelState::new(NOISE_SEEDS[0]),
                ChannelState::new(NOISE_SEEDS[1]),
            ],
            silent_frames: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ChannelState {
    ring: [f32; DELAY_RING_LEN],
    write_pos: usize,
    noise_bp: [f64; 2],
    rng: u32,
    seed: u32,
}

impl ChannelState {
    const fn new(seed: u32) -> Self {
        Self {
            ring: [0.0; DELAY_RING_LEN],
            write_pos: 0,
            noise_bp: [0.0; 2],
            rng: seed,
            seed,
        }
    }

    fn is_clear(&self) -> bool {
        self.write_pos == 0
            && self.noise_bp == [0.0; 2]
            && self.rng == self.seed
            && self.ring.iter().all(|&s| s == 0.0)
    }

    fn next_white(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        f64::from(x) / 2_147_483_648.0 - 1.0
    }

    fn next_bandpassed_noise(&mut self, bp: &BiquadCoefficients) -> f64 {
        let w = self.next_white();
        bp.process_tdf2_paired_flush(w, &mut self.noise_bp)
            .clamp(-1.0, 1.0)
    }

    fn push_and_read_delayed(&mut self, input: f32, delay_frames: f64) -> f32 {
        self.ring[self.write_pos] = input;
        let out = if delay_frames <= 1.0e-6 {
            input
        } else {
            let clamped = delay_frames.clamp(0.0, (DELAY_RING_LEN - 4) as f64);
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
            // 4-point Catmull-Rom / cubic Hermite interpolation.
            let c0 = y0;
            let c1 = 0.5 * (y1 - ym1);
            let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
            let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
            flush_quiet(((c3 * frac + c2) * frac + c1) * frac + c0) as f32
        };
        self.write_pos = (self.write_pos + 1) & DELAY_RING_MASK;
        out
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ErosionRuntime {
    base: PreparedErosion,
    controls: crate::parameter_binding::Controls<Settings, STATE_WORDS>,
    mode: u32,
    frequency: OnePoleSmoother,
    width: OnePoleSmoother,
    amount: OnePoleSmoother,
    bandpass: BiquadCoefficients,
    sine_phase: f64,
    channels: [ChannelState; 2],
    silent_frames: u32,
}

impl ErosionRuntime {
    fn clear_history(&mut self) {
        self.sine_phase = 0.0;
        self.channels = [
            ChannelState::new(NOISE_SEEDS[0]),
            ChannelState::new(NOISE_SEEDS[1]),
        ];
    }

    fn apply_targets(&mut self, settings: Settings) {
        self.mode = settings.mode;
        self.frequency.set_target(f64::from(settings.frequency_hz));
        self.width.set_target(f64::from(settings.width));
        self.amount.set_target(f64::from(settings.amount));
    }

    fn process_frame(&mut self, frame: &mut StereoFrame) {
        let freq_or_width_moving = !self.frequency.is_settled() || !self.width.is_settled();
        let freq_hz = self.frequency.advance();
        let width = self.width.advance();
        if freq_or_width_moving {
            self.bandpass = noise_bandpass(freq_hz, width, self.base.sample_rate);
        }
        let amount = self.amount.advance();

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

        // Advance audio-rate sine carrier continuously to keep phase deterministic.
        let sine_mod = (std::f64::consts::TAU * self.sine_phase).sin();
        self.sine_phase += freq_hz / f64::from(self.base.sample_rate);
        if self.sine_phase >= 1.0 {
            self.sine_phase -= self.sine_phase.floor();
        }

        let bp = self.bandpass;
        let mod_pair = match self.mode {
            0 => {
                let m = self.channels[0].next_bandpassed_noise(&bp);
                [m, m]
            }
            1 => [
                self.channels[0].next_bandpassed_noise(&bp),
                self.channels[1].next_bandpassed_noise(&bp),
            ],
            _ => [sine_mod, sine_mod],
        };

        let swing = amount * self.base.max_delay_frames;
        for i in 0..2 {
            let delay = swing * (0.5 + 0.5 * mod_pair[i]);
            frame[i] = self.channels[i].push_and_read_delayed(frame[i], delay);
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

impl NativeProcessorRuntime for ErosionRuntime {
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
        self.frequency = self.base.frequency;
        self.width = self.base.width;
        self.amount = self.base.amount;
        self.bandpass = self.base.bandpass;
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
        self.sine_phase = previous.sine_phase;
        self.silent_frames = previous.silent_frames;
        self.frequency.inherit_value(&previous.frequency);
        self.width.inherit_value(&previous.width);
        self.amount.inherit_value(&previous.amount);
        self.bandpass = noise_bandpass(
            self.frequency.value(),
            self.width.value(),
            self.base.sample_rate,
        );
        if self.base.admitted == previous.base.admitted && self.base.admitted.iter().any(|&a| a) {
            self.controls.inherit(&previous.controls);
            self.frequency.set_target(previous.frequency.target());
            self.width.set_target(previous.width.target());
            self.amount.set_target(previous.amount.target());
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

    fn dft_magnitude(samples: &[f32], sample_rate: u32, freq_hz: f64) -> f64 {
        let n = samples.len() as f64;
        let mut re = 0.0;
        let mut im = 0.0;
        for (i, &s) in samples.iter().enumerate() {
            let angle = std::f64::consts::TAU * freq_hz * i as f64 / f64::from(sample_rate);
            re += f64::from(s) * angle.cos();
            im -= f64::from(s) * angle.sin();
        }
        (re * re + im * im).sqrt() * 2.0 / n
    }

    #[test]
    fn zero_amount_is_transparent_and_sine_mode_creates_fm_sidebands() {
        let default = EROSION_DEFINITION.default_state().unwrap();
        let prep_dry = ErosionDefinition::prepare_owned(&default, 48_000, &[]).unwrap();
        let mut rt_dry =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep_dry);
        let input: Vec<StereoFrame> = (0..2_048)
            .map(|i| {
                let x = (std::f64::consts::TAU * 500.0 * i as f64 / 48_000.0).sin() as f32;
                [x, x]
            })
            .collect();
        let mut out_dry = input.clone();
        rt_dry.process_block(&mut out_dry, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert_eq!(out_dry, input, "amount=0 must pass signal bit-identically");

        // Sine mode at 1000 Hz modulates 500 Hz input -> strong sideband at 1500 Hz!
        let sine_state = EROSION_DEFINITION
            .patch_parameter(&default, parameter::MODE, ParameterValue::Choice(2))
            .and_then(|s| {
                EROSION_DEFINITION.patch_parameter(
                    &s,
                    parameter::FREQUENCY,
                    ParameterValue::Float(1_000.0),
                )
            })
            .and_then(|s| {
                EROSION_DEFINITION.patch_parameter(
                    &s,
                    parameter::AMOUNT,
                    ParameterValue::Float(0.4),
                )
            })
            .unwrap();
        let prep_sine = ErosionDefinition::prepare_owned(&sine_state, 48_000, &[]).unwrap();
        let mut rt_sine =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep_sine);
        let mut out_sine = input;
        rt_sine.process_block(&mut out_sine, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mono: Vec<f32> = out_sine[256..].iter().map(|f| f[0]).collect();
        let sideband_1500 = dft_magnitude(&mono, 48_000, 1_500.0);
        assert!(
            sideband_1500 > 0.15,
            "sine phase modulation must create strong 1500 hz sideband (got {sideband_1500:.4})"
        );
    }

    #[test]
    fn noise_vs_wide_noise_stereo_correlation_partition_invariance_and_malformed_refusal() {
        let base = EROSION_DEFINITION.default_state().unwrap();
        let mono_noise = EROSION_DEFINITION
            .patch_parameter(&base, parameter::MODE, ParameterValue::Choice(0))
            .and_then(|s| {
                EROSION_DEFINITION.patch_parameter(
                    &s,
                    parameter::AMOUNT,
                    ParameterValue::Float(0.5),
                )
            })
            .unwrap();
        let wide_noise = EROSION_DEFINITION
            .patch_parameter(&mono_noise, parameter::MODE, ParameterValue::Choice(1))
            .unwrap();

        let input: Vec<StereoFrame> = (0..1_024)
            .map(|i| {
                let x = (std::f64::consts::TAU * 600.0 * i as f64 / 48_000.0).sin() as f32;
                [x, x]
            })
            .collect();

        // Mono noise preserves L == R on identical stereo input.
        let prep_mono = ErosionDefinition::prepare_owned(&mono_noise, 48_000, &[]).unwrap();
        let mut rt_mono =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep_mono);
        let mut out_mono = input.clone();
        rt_mono.process_block(&mut out_mono, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(out_mono.iter().all(|f| f[0] == f[1]));

        // Wide noise decorrelates L and R.
        let prep_wide = ErosionDefinition::prepare_owned(&wide_noise, 48_000, &[]).unwrap();
        let mut rt_wide =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep_wide);
        let mut out_wide = input.clone();
        rt_wide.process_block(&mut out_wide, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let diff_energy: f64 = out_wide[256..]
            .iter()
            .map(|f| {
                let d = f64::from(f[0] - f[1]);
                d * d
            })
            .sum();
        assert!(
            diff_energy > 0.5,
            "wide noise must decorrelate L/R channels (diff_energy={diff_energy:.4})"
        );

        // Block-partition invariance for wide noise!
        let mut chunked =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prep_wide);
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
        assert_eq!(out_wide, out_chunked);

        // Finite tail reaches quiescence.
        let mut silence = vec![[0.0f32; 2]; 14_000];
        rt_wide.process_block(&mut silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(rt_wide.is_quiescent());

        // Malformed states refused.
        assert!(
            decode(&NativeDeviceState::new(EROSION_TYPE_ID, 2, vec![0; STATE_BYTES]).unwrap())
                .is_err()
        );
        assert!(decode(&NativeDeviceState::new(EROSION_TYPE_ID, 1, vec![0; 4]).unwrap()).is_err());
    }
}
