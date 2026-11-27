//! Original stereo 3-band musical channel-strip EQ (`hex.channel_eq`) for the
//! registered native host (`#2683`).
//!
//! # Signal Path
//! 1. Switchable 80 Hz 2nd-order Butterworth high-pass filter with 5 ms de-click
//!    crossfade when toggled.
//! 2. 100 Hz low shelf (`-15..+15 dB`, slope `0.7`).
//! 3. Sweepable mid peaking bell (`120..7500 Hz`, `-12..+12 dB`) with musical
//!    proportional Q (broader ~0.7 Q for subtle shaping, tighter ~2.0 Q at full
//!    boost/cut).
//! 4. 3.5 kHz high shelf (`-15..+15 dB`, slope `0.7`).
//! 5. Output trim (`-12..+12 dB`).
//!
//! # Saved-State Compatibility
//! - **Version 1 (current)**: 6 little-endian 32-bit words (24 bytes) in
//!   descriptor order (`high_pass`, `low_gain`, `mid_gain`, `mid_freq`,
//!   `high_gain`, `output_gain`).
//! - Newer (`> 1`), zero, or malformed payloads are refused explicitly without
//!   mutating or discarding project data.

use hex_dsp::{BiquadCoefficients, Decibels, OnePoleSmoother, flush_quiet};
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

pub const CHANNEL_EQ_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.channel_eq");
pub const CHANNEL_EQ_STATE_VERSION: u32 = 1;
const STATE_WORDS: usize = 6;
const STATE_BYTES: usize = STATE_WORDS * size_of::<u32>();
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const SMOOTHING_MS: f64 = 5.0;
const HPF_HZ: f64 = 80.0;
const LOW_SHELF_HZ: f64 = 100.0;
const HIGH_SHELF_HZ: f64 = 3_500.0;
const SHELF_SLOPE: f64 = 0.7;

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const HIGH_PASS: ParameterId = ParameterId::from_static(1);
    pub const LOW_GAIN: ParameterId = ParameterId::from_static(2);
    pub const MID_GAIN: ParameterId = ParameterId::from_static(3);
    pub const MID_FREQ: ParameterId = ParameterId::from_static(4);
    pub const HIGH_GAIN: ParameterId = ParameterId::from_static(5);
    pub const OUTPUT_GAIN: ParameterId = ParameterId::from_static(6);
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    high_pass: bool,
    low_gain_db: f32,
    mid_gain_db: f32,
    mid_freq_hz: f32,
    high_gain_db: f32,
    output_gain_db: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            high_pass: false,
            low_gain_db: 0.0,
            mid_gain_db: 0.0,
            mid_freq_hz: 1_000.0,
            high_gain_db: 0.0,
            output_gain_db: 0.0,
        }
    }
}

const GAIN_SEMANTICS: ParameterSemantics = ParameterSemantics {
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

const HPF_SEMANTICS: ParameterSemantics = ParameterSemantics {
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

const BINDINGS: [crate::parameter_binding::Binding<Settings>; STATE_WORDS] = [
    crate::parameter_binding::Binding::boolean(
        ParameterDescriptor {
            id: parameter::HIGH_PASS,
            label: "high pass",
            kind: ParameterKind::Bool,
            default: ParameterValue::Bool(false),
            presentation: ParameterPresentation::Linear,
        },
        HPF_SEMANTICS,
        |s| s.high_pass,
        |s, v| s.high_pass = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::LOW_GAIN,
            "low",
            -15.0,
            15.0,
            0.0,
            Some("db"),
            ParameterPresentation::Linear,
        ),
        GAIN_SEMANTICS,
        |s| s.low_gain_db,
        |s, v| s.low_gain_db = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::MID_GAIN,
            "mid",
            -12.0,
            12.0,
            0.0,
            Some("db"),
            ParameterPresentation::Linear,
        ),
        GAIN_SEMANTICS,
        |s| s.mid_gain_db,
        |s, v| s.mid_gain_db = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::MID_FREQ,
            "mid freq",
            120.0,
            7_500.0,
            1_000.0,
            Some("hz"),
            ParameterPresentation::Logarithmic,
        ),
        FREQ_SEMANTICS,
        |s| s.mid_freq_hz,
        |s, v| s.mid_freq_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::HIGH_GAIN,
            "high",
            -15.0,
            15.0,
            0.0,
            Some("db"),
            ParameterPresentation::Linear,
        ),
        GAIN_SEMANTICS,
        |s| s.high_gain_db,
        |s, v| s.high_gain_db = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::OUTPUT_GAIN,
            "output",
            -12.0,
            12.0,
            0.0,
            Some("db"),
            ParameterPresentation::Linear,
        ),
        GAIN_SEMANTICS,
        |s| s.output_gain_db,
        |s, v| s.output_gain_db = v,
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
    if state.type_id() != CHANNEL_EQ_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != CHANNEL_EQ_STATE_VERSION {
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
    let high_pass = match words[0] {
        0 => false,
        1 => true,
        _ => return Err(DeviceContractError::MalformedState),
    };
    let settings = Settings {
        high_pass,
        low_gain_db: f32::from_bits(words[1]),
        mid_gain_db: f32::from_bits(words[2]),
        mid_freq_hz: f32::from_bits(words[3]),
        high_gain_db: f32::from_bits(words[4]),
        output_gain_db: f32::from_bits(words[5]),
    };
    validate(settings)?;
    Ok(settings)
}

fn encode(settings: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let words = [
        u32::from(settings.high_pass),
        settings.low_gain_db.to_bits(),
        settings.mid_gain_db.to_bits(),
        settings.mid_freq_hz.to_bits(),
        settings.high_gain_db.to_bits(),
        settings.output_gain_db.to_bits(),
    ];
    let mut bytes = Vec::with_capacity(STATE_BYTES);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    NativeDeviceState::new(CHANNEL_EQ_TYPE_ID, CHANNEL_EQ_STATE_VERSION, bytes)
}

fn automation_supported(parameter: ParameterId) -> bool {
    crate::parameter_binding::binding(&BINDINGS, parameter)
        .is_ok_and(crate::parameter_binding::Binding::declares_arrangement_automation)
}

pub struct ChannelEqDefinition;
pub static CHANNEL_EQ_DEFINITION: ChannelEqDefinition = ChannelEqDefinition;

impl NativeDeviceMetadata for ChannelEqDefinition {
    fn type_id(&self) -> DeviceTypeId {
        CHANNEL_EQ_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("channel eq")
    }
    fn state_version(&self) -> u32 {
        CHANNEL_EQ_STATE_VERSION
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

fn proportional_mid_q(mid_gain_db: f64) -> f64 {
    let norm = (mid_gain_db.abs() / 12.0).clamp(0.0, 1.0);
    0.7 + 1.3 * norm
}

fn db_to_linear(db: f64) -> f64 {
    Decibels::new(db)
        .and_then(Decibels::linear_amplitude)
        .map_or(1.0, hex_dsp::LinearAmplitude::value)
}

#[derive(Clone, Copy, Debug)]
struct FilterBank {
    hpf: BiquadCoefficients,
    low: BiquadCoefficients,
    mid: BiquadCoefficients,
    high: BiquadCoefficients,
    output_gain: f64,
}

impl FilterBank {
    fn compute(
        low_db: f64,
        mid_db: f64,
        mid_hz: f64,
        high_db: f64,
        output_db: f64,
        sample_rate: u32,
    ) -> Self {
        let rate = f64::from(sample_rate);
        let max_hz = (rate * 0.45).max(100.0);
        let hpf = BiquadCoefficients::butterworth_high_pass(HPF_HZ.min(max_hz), rate)
            .unwrap_or(BiquadCoefficients::IDENTITY);
        let low = if low_db.abs() < 1.0e-5 {
            BiquadCoefficients::IDENTITY
        } else {
            BiquadCoefficients::low_shelf_slope(LOW_SHELF_HZ.min(max_hz), rate, low_db, SHELF_SLOPE)
                .unwrap_or(BiquadCoefficients::IDENTITY)
        };
        let mid = if mid_db.abs() < 1.0e-5 {
            BiquadCoefficients::IDENTITY
        } else {
            BiquadCoefficients::peaking(
                mid_hz.clamp(20.0, max_hz),
                rate,
                proportional_mid_q(mid_db),
                mid_db,
            )
            .unwrap_or(BiquadCoefficients::IDENTITY)
        };
        let high = if high_db.abs() < 1.0e-5 {
            BiquadCoefficients::IDENTITY
        } else {
            BiquadCoefficients::high_shelf_slope(
                HIGH_SHELF_HZ.min(max_hz),
                rate,
                high_db,
                SHELF_SLOPE,
            )
            .unwrap_or(BiquadCoefficients::IDENTITY)
        };
        Self {
            hpf,
            low,
            mid,
            high,
            output_gain: db_to_linear(output_db),
        }
    }

    fn magnitude_at_hz(&self, frequency_hz: f64, sample_rate: u32, hpf_enabled: bool) -> f64 {
        let rate = f64::from(sample_rate);
        let hpf_mag = if hpf_enabled {
            self.hpf.magnitude_at_hz(frequency_hz, rate).unwrap_or(1.0)
        } else {
            1.0
        };
        let low_mag = self.low.magnitude_at_hz(frequency_hz, rate).unwrap_or(1.0);
        let mid_mag = self.mid.magnitude_at_hz(frequency_hz, rate).unwrap_or(1.0);
        let high_mag = self.high.magnitude_at_hz(frequency_hz, rate).unwrap_or(1.0);
        hpf_mag * low_mag * mid_mag * high_mag * self.output_gain
    }
}

impl ChannelEqDefinition {
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedChannelEq, DeviceContractError> {
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
        let filters = FilterBank::compute(
            f64::from(settings.low_gain_db),
            f64::from(settings.mid_gain_db),
            f64::from(settings.mid_freq_hz),
            f64::from(settings.high_gain_db),
            f64::from(settings.output_gain_db),
            sample_rate,
        );
        Ok(PreparedChannelEq {
            settings,
            sample_rate,
            admitted,
            hpf_mix: make_smoother(if settings.high_pass { 1.0 } else { 0.0 })?,
            low_db: make_smoother(f64::from(settings.low_gain_db))?,
            mid_db: make_smoother(f64::from(settings.mid_gain_db))?,
            mid_hz: make_smoother(f64::from(settings.mid_freq_hz))?,
            high_db: make_smoother(f64::from(settings.high_gain_db))?,
            output_db: make_smoother(f64::from(settings.output_gain_db))?,
            filters,
            silence_frames: sample_rate / 2,
        })
    }
}

impl NativeDeviceDefinition for ChannelEqDefinition {
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
pub struct PreparedChannelEq {
    settings: Settings,
    sample_rate: u32,
    admitted: [bool; STATE_WORDS],
    hpf_mix: OnePoleSmoother,
    low_db: OnePoleSmoother,
    mid_db: OnePoleSmoother,
    mid_hz: OnePoleSmoother,
    high_db: OnePoleSmoother,
    output_db: OnePoleSmoother,
    filters: FilterBank,
    silence_frames: u32,
}

impl PreparedChannelEq {
    fn supports(&self, parameter: ParameterId) -> bool {
        BINDINGS
            .iter()
            .position(|b| b.descriptor.id == parameter)
            .is_some_and(|index| self.admitted[index])
    }
}

impl PreparedNativeProcessor for PreparedChannelEq {
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
        size_of::<ChannelEqRuntime>()
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
    fn frequency_response_at_hz(&self, frequency_hz: f32) -> Option<[f32; 2]> {
        if !frequency_hz.is_finite() || frequency_hz <= 0.0 {
            return None;
        }
        let mag = self.filters.magnitude_at_hz(
            f64::from(frequency_hz),
            self.sample_rate,
            self.settings.high_pass,
        ) as f32;
        Some([mag, mag])
    }
}

impl crate::inline_owned::InlinePreparedProcessor for PreparedChannelEq {
    type Runtime = ChannelEqRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        ChannelEqRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(self.settings, self.admitted),
            hpf_mix: self.hpf_mix,
            low_db: self.low_db,
            mid_db: self.mid_db,
            mid_hz: self.mid_hz,
            high_db: self.high_db,
            output_db: self.output_db,
            filters: self.filters,
            channels: [ChannelState::default(); 2],
            silent_frames: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ChannelState {
    hpf: [f64; 2],
    low: [f64; 2],
    mid: [f64; 2],
    high: [f64; 2],
}

impl ChannelState {
    fn is_clear(&self) -> bool {
        *self == Self::default()
    }

    fn process(&mut self, input: f64, bank: &FilterBank, hpf_mix: f64) -> f64 {
        let hp_out = bank.hpf.process_tdf2_paired_flush(input, &mut self.hpf);
        let stage0 = input + (hp_out - input) * hpf_mix;
        let stage1 = bank.low.process_tdf2_paired_flush(stage0, &mut self.low);
        let stage2 = bank.mid.process_tdf2_paired_flush(stage1, &mut self.mid);
        let stage3 = bank.high.process_tdf2_paired_flush(stage2, &mut self.high);
        flush_quiet(stage3 * bank.output_gain)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ChannelEqRuntime {
    base: PreparedChannelEq,
    controls: crate::parameter_binding::Controls<Settings, STATE_WORDS>,
    hpf_mix: OnePoleSmoother,
    low_db: OnePoleSmoother,
    mid_db: OnePoleSmoother,
    mid_hz: OnePoleSmoother,
    high_db: OnePoleSmoother,
    output_db: OnePoleSmoother,
    filters: FilterBank,
    channels: [ChannelState; 2],
    silent_frames: u32,
}

impl ChannelEqRuntime {
    fn apply_targets(&mut self, settings: Settings) {
        self.hpf_mix
            .set_target(if settings.high_pass { 1.0 } else { 0.0 });
        self.low_db.set_target(f64::from(settings.low_gain_db));
        self.mid_db.set_target(f64::from(settings.mid_gain_db));
        self.mid_hz.set_target(f64::from(settings.mid_freq_hz));
        self.high_db.set_target(f64::from(settings.high_gain_db));
        self.output_db
            .set_target(f64::from(settings.output_gain_db));
    }

    fn advance_parameters(&mut self) -> f64 {
        let hpf_mix = self.hpf_mix.advance();
        if !self.low_db.is_settled()
            || !self.mid_db.is_settled()
            || !self.mid_hz.is_settled()
            || !self.high_db.is_settled()
            || !self.output_db.is_settled()
        {
            self.filters = FilterBank::compute(
                self.low_db.advance(),
                self.mid_db.advance(),
                self.mid_hz.advance(),
                self.high_db.advance(),
                self.output_db.advance(),
                self.base.sample_rate,
            );
        }
        hpf_mix
    }

    fn process_frame(&mut self, frame: &mut StereoFrame) {
        let hpf_mix = self.advance_parameters();
        if !frame.iter().all(|s| s.is_finite()) {
            *frame = [0.0; 2];
        }
        if *frame == [0.0; 2] {
            let limit = self.base.silence_frames;
            self.silent_frames = self.silent_frames.saturating_add(1).min(limit);
            if self.silent_frames == limit {
                if !self.channels.iter().all(ChannelState::is_clear) {
                    self.channels = [ChannelState::default(); 2];
                }
                return;
            }
        } else {
            self.silent_frames = 0;
        }
        for (sample, channel) in frame.iter_mut().zip(&mut self.channels) {
            *sample = channel.process(f64::from(*sample), &self.filters, hpf_mix) as f32;
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

impl NativeProcessorRuntime for ChannelEqRuntime {
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
        self.hpf_mix = self.base.hpf_mix;
        self.low_db = self.base.low_db;
        self.mid_db = self.base.mid_db;
        self.mid_hz = self.base.mid_hz;
        self.high_db = self.base.high_db;
        self.output_db = self.base.output_db;
        self.filters = self.base.filters;
        self.channels = [ChannelState::default(); 2];
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
        self.hpf_mix.inherit_value(&previous.hpf_mix);
        self.low_db.inherit_value(&previous.low_db);
        self.mid_db.inherit_value(&previous.mid_db);
        self.mid_hz.inherit_value(&previous.mid_hz);
        self.high_db.inherit_value(&previous.high_db);
        self.output_db.inherit_value(&previous.output_db);
        self.filters = FilterBank::compute(
            self.low_db.value(),
            self.mid_db.value(),
            self.mid_hz.value(),
            self.high_db.value(),
            self.output_db.value(),
            self.base.sample_rate,
        );
        if self.base.admitted == previous.base.admitted && self.base.admitted.iter().any(|&a| a) {
            self.controls.inherit(&previous.controls);
            self.hpf_mix.set_target(previous.hpf_mix.target());
            self.low_db.set_target(previous.low_db.target());
            self.mid_db.set_target(previous.mid_db.target());
            self.mid_hz.set_target(previous.mid_hz.target());
            self.high_db.set_target(previous.high_db.target());
            self.output_db.set_target(previous.output_db.target());
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

    fn measure_gain_db(state: &NativeDeviceState, sample_rate: u32, freq_hz: f64) -> f64 {
        let prepared = ChannelEqDefinition::prepare_owned(state, sample_rate, &[]).unwrap();
        let mut runtime =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prepared);
        let total = sample_rate as usize / 2;
        let settle = total / 2;
        let mut frames = Vec::with_capacity(total);
        for i in 0..total {
            let x =
                (std::f64::consts::TAU * freq_hz * i as f64 / f64::from(sample_rate)).sin() as f32;
            frames.push([x, x]);
        }
        runtime.process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let energy: f64 = frames[settle..]
            .iter()
            .map(|f| f64::from(f[0]) * f64::from(f[0]))
            .sum();
        let rms = (energy / (total - settle) as f64).sqrt();
        20.0 * (rms / std::f64::consts::FRAC_1_SQRT_2).log10()
    }

    #[test]
    fn default_channel_eq_is_transparent_and_matches_analytic_response() {
        let default = CHANNEL_EQ_DEFINITION.default_state().unwrap();
        let prepared = ChannelEqDefinition::prepare_owned(&default, 48_000, &[]).unwrap();
        for freq in [40.0, 100.0, 1_000.0, 3_500.0, 10_000.0] {
            let measured = measure_gain_db(&default, 48_000, freq);
            assert!(
                measured.abs() < 0.01,
                "default channel eq must be flat at {freq} hz (got {measured:.4} db)"
            );
            let analytic = prepared.frequency_response_at_hz(freq as f32).unwrap()[0];
            assert!((analytic - 1.0).abs() < 1.0e-5);
        }
    }

    #[test]
    fn hpf_shelves_mid_bell_and_output_trim_match_analytic_oracle() {
        let base = CHANNEL_EQ_DEFINITION.default_state().unwrap();
        let shaped = CHANNEL_EQ_DEFINITION
            .patch_parameter(&base, parameter::HIGH_PASS, ParameterValue::Bool(true))
            .and_then(|s| {
                CHANNEL_EQ_DEFINITION.patch_parameter(
                    &s,
                    parameter::LOW_GAIN,
                    ParameterValue::Float(6.0),
                )
            })
            .and_then(|s| {
                CHANNEL_EQ_DEFINITION.patch_parameter(
                    &s,
                    parameter::MID_FREQ,
                    ParameterValue::Float(1_200.0),
                )
            })
            .and_then(|s| {
                CHANNEL_EQ_DEFINITION.patch_parameter(
                    &s,
                    parameter::MID_GAIN,
                    ParameterValue::Float(-6.0),
                )
            })
            .and_then(|s| {
                CHANNEL_EQ_DEFINITION.patch_parameter(
                    &s,
                    parameter::HIGH_GAIN,
                    ParameterValue::Float(4.5),
                )
            })
            .and_then(|s| {
                CHANNEL_EQ_DEFINITION.patch_parameter(
                    &s,
                    parameter::OUTPUT_GAIN,
                    ParameterValue::Float(-3.0),
                )
            })
            .unwrap();
        let prepared = ChannelEqDefinition::prepare_owned(&shaped, 48_000, &[]).unwrap();
        for freq in [35.0, 80.0, 1_200.0, 8_000.0] {
            let measured_db = measure_gain_db(&shaped, 48_000, freq);
            let analytic_mag = prepared.frequency_response_at_hz(freq as f32).unwrap()[0];
            let analytic_db = 20.0 * f64::from(analytic_mag).log10();
            assert!(
                (measured_db - analytic_db).abs() < 0.15,
                "freq={freq} hz measured={measured_db:.3} db vs analytic={analytic_db:.3} db"
            );
        }
        // HPF attenuates 35 Hz significantly relative to 100 Hz.
        assert!(measure_gain_db(&shaped, 48_000, 35.0) < -6.0);
    }

    #[test]
    fn block_partition_invariant_finite_tail_and_malformed_state_refusal() {
        let state = CHANNEL_EQ_DEFINITION
            .patch_parameter(
                &CHANNEL_EQ_DEFINITION.default_state().unwrap(),
                parameter::MID_GAIN,
                ParameterValue::Float(8.0),
            )
            .unwrap();
        let prepared = ChannelEqDefinition::prepare_owned(&state, 48_000, &[]).unwrap();
        let mut single =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prepared);
        let mut chunked =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prepared);
        let input: Vec<StereoFrame> = (0..512)
            .map(|i| {
                let s = ((i as f32) * 0.17).sin() * 0.4;
                [s, s * 0.5]
            })
            .collect();
        let mut out_single = input.clone();
        single.process_block(&mut out_single, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut out_chunked = input;
        let mut pos = 0;
        for size in [1, 5, 17, 63, 128, 298] {
            let end = (pos + size).min(out_chunked.len());
            chunked.process_block(
                &mut out_chunked[pos..end],
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            pos = end;
        }
        assert_eq!(out_single, out_chunked);

        let mut silence = vec![[0.0f32; 2]; 25_000];
        single.process_block(&mut silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(single.is_quiescent());

        assert!(
            decode(&NativeDeviceState::new(CHANNEL_EQ_TYPE_ID, 2, vec![0; STATE_BYTES]).unwrap())
                .is_err()
        );
        assert!(
            decode(&NativeDeviceState::new(CHANNEL_EQ_TYPE_ID, 1, vec![0; 4]).unwrap()).is_err()
        );
        let mut bad_bool = state.payload().to_vec();
        bad_bool[0..4].copy_from_slice(&3u32.to_le_bytes());
        assert!(decode(&NativeDeviceState::new(CHANNEL_EQ_TYPE_ID, 1, bad_bool).unwrap()).is_err());
    }
}
