//! Original stereo 3-band DJ isolator EQ (`hex.eq_three`) with phase-coherent
//! Linkwitz-Riley crossovers (`24 dB/oct` LR4 and `48 dB/oct` LR8) for the
//! registered native host (`#2683`).
//!
//! # Crossover & Flat-Sum Law
//! The input is split at `freq_low` into `low_raw` and `upper`, then `upper` is
//! split at `freq_high` into `mid` and `high`. `low_raw` passes through the
//! exact complementary phase-matching all-pass network at `freq_high` (one
//! 2nd-order all-pass for LR4; two cascaded 2nd-order Butterworth all-passes
//! for LR8). Because `H_lp(s)^2 + H_hp(s)^2 = B(-s) / B(s)` for both 2nd-order
//! and 4th-order Butterworth factors, `low + mid + high` sums to a flat-magnitude
//! all-pass response (`0.0 dB` across the audio band) when all three band gains
//! are `0 dB` and no band is killed.
//!
//! # Saved-State Compatibility
//! - **Version 1 (current)**: 9 little-endian 32-bit words (36 bytes) in
//!   descriptor order (`gain_low`, `gain_mid`, `gain_high`, `kill_low`,
//!   `kill_mid`, `kill_high`, `freq_low`, `freq_high`, `slope`).
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

pub const EQ_THREE_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.eq_three");
pub const EQ_THREE_STATE_VERSION: u32 = 1;
const STATE_WORDS: usize = 9;
const STATE_BYTES: usize = STATE_WORDS * size_of::<u32>();
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const SMOOTHING_MS: f64 = 5.0;
const MIN_GAIN_DB: f32 = -60.0;
const MAX_GAIN_DB: f32 = 6.0;
const MIN_FREQ_LOW_HZ: f32 = 50.0;
const MAX_FREQ_LOW_HZ: f32 = 5_000.0;
const MIN_FREQ_HIGH_HZ: f32 = 500.0;
const MAX_FREQ_HIGH_HZ: f32 = 18_000.0;

pub const SLOPES: &[&str] = &["24 db/oct", "48 db/oct"];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const GAIN_LOW: ParameterId = ParameterId::from_static(1);
    pub const GAIN_MID: ParameterId = ParameterId::from_static(2);
    pub const GAIN_HIGH: ParameterId = ParameterId::from_static(3);
    pub const KILL_LOW: ParameterId = ParameterId::from_static(4);
    pub const KILL_MID: ParameterId = ParameterId::from_static(5);
    pub const KILL_HIGH: ParameterId = ParameterId::from_static(6);
    pub const FREQ_LOW: ParameterId = ParameterId::from_static(7);
    pub const FREQ_HIGH: ParameterId = ParameterId::from_static(8);
    pub const SLOPE: ParameterId = ParameterId::from_static(9);
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    gain_low_db: f32,
    gain_mid_db: f32,
    gain_high_db: f32,
    kill_low: bool,
    kill_mid: bool,
    kill_high: bool,
    freq_low_hz: f32,
    freq_high_hz: f32,
    slope: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            gain_low_db: 0.0,
            gain_mid_db: 0.0,
            gain_high_db: 0.0,
            kill_low: false,
            kill_mid: false,
            kill_high: false,
            freq_low_hz: 250.0,
            freq_high_hz: 2_500.0,
            slope: 0,
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

const KILL_SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    ..crate::parameter_binding::BOOLEAN_ENVELOPE
};

const SLOPE_SEMANTICS: ParameterSemantics = ParameterSemantics {
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
            parameter::GAIN_LOW,
            "gain low",
            MIN_GAIN_DB,
            MAX_GAIN_DB,
            0.0,
            Some("db"),
            ParameterPresentation::Linear,
        ),
        GAIN_SEMANTICS,
        |s| s.gain_low_db,
        |s, v| s.gain_low_db = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::GAIN_MID,
            "gain mid",
            MIN_GAIN_DB,
            MAX_GAIN_DB,
            0.0,
            Some("db"),
            ParameterPresentation::Linear,
        ),
        GAIN_SEMANTICS,
        |s| s.gain_mid_db,
        |s, v| s.gain_mid_db = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::GAIN_HIGH,
            "gain high",
            MIN_GAIN_DB,
            MAX_GAIN_DB,
            0.0,
            Some("db"),
            ParameterPresentation::Linear,
        ),
        GAIN_SEMANTICS,
        |s| s.gain_high_db,
        |s, v| s.gain_high_db = v,
    ),
    crate::parameter_binding::Binding::boolean(
        bool_desc(parameter::KILL_LOW, "kill low", false),
        KILL_SEMANTICS,
        |s| s.kill_low,
        |s, v| s.kill_low = v,
    ),
    crate::parameter_binding::Binding::boolean(
        bool_desc(parameter::KILL_MID, "kill mid", false),
        KILL_SEMANTICS,
        |s| s.kill_mid,
        |s, v| s.kill_mid = v,
    ),
    crate::parameter_binding::Binding::boolean(
        bool_desc(parameter::KILL_HIGH, "kill high", false),
        KILL_SEMANTICS,
        |s| s.kill_high,
        |s, v| s.kill_high = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::FREQ_LOW,
            "freq low",
            MIN_FREQ_LOW_HZ,
            MAX_FREQ_LOW_HZ,
            250.0,
            Some("hz"),
            ParameterPresentation::Logarithmic,
        ),
        FREQ_SEMANTICS,
        |s| s.freq_low_hz,
        |s, v| s.freq_low_hz = v,
    ),
    crate::parameter_binding::Binding::float(
        float_desc(
            parameter::FREQ_HIGH,
            "freq high",
            MIN_FREQ_HIGH_HZ,
            MAX_FREQ_HIGH_HZ,
            2_500.0,
            Some("hz"),
            ParameterPresentation::Logarithmic,
        ),
        FREQ_SEMANTICS,
        |s| s.freq_high_hz,
        |s, v| s.freq_high_hz = v,
    ),
    crate::parameter_binding::Binding::choice(
        ParameterDescriptor {
            id: parameter::SLOPE,
            label: "slope",
            kind: ParameterKind::Choice { labels: SLOPES },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        },
        SLOPE_SEMANTICS,
        |s| s.slope,
        |s, v| {
            if (v as usize) < SLOPES.len() {
                s.slope = v;
                Ok(())
            } else {
                Err(DeviceContractError::InvalidParameterValue)
            }
        },
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
    if state.type_id() != EQ_THREE_TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != EQ_THREE_STATE_VERSION {
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
        gain_low_db: f32::from_bits(words[0]),
        gain_mid_db: f32::from_bits(words[1]),
        gain_high_db: f32::from_bits(words[2]),
        kill_low: decode_bool_word(words[3])?,
        kill_mid: decode_bool_word(words[4])?,
        kill_high: decode_bool_word(words[5])?,
        freq_low_hz: f32::from_bits(words[6]),
        freq_high_hz: f32::from_bits(words[7]),
        slope: words[8],
    };
    validate(settings)?;
    Ok(settings)
}

fn encode(settings: Settings) -> Result<NativeDeviceState, DeviceContractError> {
    validate(settings)?;
    let words = [
        settings.gain_low_db.to_bits(),
        settings.gain_mid_db.to_bits(),
        settings.gain_high_db.to_bits(),
        u32::from(settings.kill_low),
        u32::from(settings.kill_mid),
        u32::from(settings.kill_high),
        settings.freq_low_hz.to_bits(),
        settings.freq_high_hz.to_bits(),
        settings.slope,
    ];
    let mut bytes = Vec::with_capacity(STATE_BYTES);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    NativeDeviceState::new(EQ_THREE_TYPE_ID, EQ_THREE_STATE_VERSION, bytes)
}

fn automation_supported(parameter: ParameterId) -> bool {
    crate::parameter_binding::binding(&BINDINGS, parameter)
        .is_ok_and(crate::parameter_binding::Binding::declares_arrangement_automation)
}

pub struct EqThreeDefinition;
pub static EQ_THREE_DEFINITION: EqThreeDefinition = EqThreeDefinition;

impl NativeDeviceMetadata for EqThreeDefinition {
    fn type_id(&self) -> DeviceTypeId {
        EQ_THREE_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("eq three")
    }
    fn state_version(&self) -> u32 {
        EQ_THREE_STATE_VERSION
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

fn band_linear_gain(gain_db: f32, killed: bool) -> f64 {
    if killed || gain_db <= MIN_GAIN_DB {
        return 0.0;
    }
    Decibels::new(f64::from(gain_db))
        .and_then(Decibels::linear_amplitude)
        .map_or(1.0, hex_dsp::LinearAmplitude::value)
}

fn effective_frequencies(freq_low: f64, freq_high: f64, sample_rate: u32) -> (f64, f64) {
    let max_hz = (f64::from(sample_rate) * 0.45).max(100.0);
    let high = freq_high.clamp(f64::from(MIN_FREQ_HIGH_HZ).min(max_hz), max_hz);
    let low = freq_low.clamp(f64::from(MIN_FREQ_LOW_HZ).min(high), high);
    (low, high)
}

/// Precomputed filter stages for one crossover frequency.
///
/// For `24 dB/oct` (LR4), stages `0..2` carry two cascaded 2nd-order Butterworth
/// biquads (`Q = 1/sqrt(2)`) and `all_pass` carries 1 complementary 2nd-order
/// all-pass (`Q = 1/sqrt(2)`).
/// For `48 dB/oct` (LR8), stages `0..4` carry two cascaded 4th-order Butterworth
/// filters (`Q1, Q2, Q1, Q2`) and `all_pass` carries 2 cascaded complementary
/// 2nd-order all-pass filters (`Q1, Q2`).
#[derive(Clone, Copy, Debug)]
struct CrossoverPair {
    low_lp: [BiquadCoefficients; 4],
    low_hp: [BiquadCoefficients; 4],
    high_lp: [BiquadCoefficients; 4],
    high_hp: [BiquadCoefficients; 4],
    low_phase_ap: [BiquadCoefficients; 2],
    stages: usize,
    ap_stages: usize,
}

impl CrossoverPair {
    fn compute(freq_low: f64, freq_high: f64, slope_48: bool, sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate);
        let (fl, fh) = effective_frequencies(freq_low, freq_high, sample_rate);
        let unity = BiquadCoefficients::IDENTITY;
        if !slope_48 {
            let q = std::f64::consts::FRAC_1_SQRT_2;
            let lp_l = BiquadCoefficients::low_pass(fl, rate, q).unwrap_or(unity);
            let hp_l = BiquadCoefficients::high_pass(fl, rate, q).unwrap_or(unity);
            let lp_h = BiquadCoefficients::low_pass(fh, rate, q).unwrap_or(unity);
            let hp_h = BiquadCoefficients::high_pass(fh, rate, q).unwrap_or(unity);
            let ap_h = BiquadCoefficients::all_pass(fh, rate, q).unwrap_or(unity);
            Self {
                low_lp: [lp_l, lp_l, unity, unity],
                low_hp: [hp_l, hp_l, unity, unity],
                high_lp: [lp_h, lp_h, unity, unity],
                high_hp: [hp_h, hp_h, unity, unity],
                low_phase_ap: [ap_h, unity],
                stages: 2,
                ap_stages: 1,
            }
        } else {
            let q1 = BiquadCoefficients::butterworth_section_q(2, 0).unwrap_or(0.541_196_100_146);
            let q2 = BiquadCoefficients::butterworth_section_q(2, 1).unwrap_or(1.306_562_964_876);
            let lp_l1 = BiquadCoefficients::low_pass(fl, rate, q1).unwrap_or(unity);
            let lp_l2 = BiquadCoefficients::low_pass(fl, rate, q2).unwrap_or(unity);
            let hp_l1 = BiquadCoefficients::high_pass(fl, rate, q1).unwrap_or(unity);
            let hp_l2 = BiquadCoefficients::high_pass(fl, rate, q2).unwrap_or(unity);
            let lp_h1 = BiquadCoefficients::low_pass(fh, rate, q1).unwrap_or(unity);
            let lp_h2 = BiquadCoefficients::low_pass(fh, rate, q2).unwrap_or(unity);
            let hp_h1 = BiquadCoefficients::high_pass(fh, rate, q1).unwrap_or(unity);
            let hp_h2 = BiquadCoefficients::high_pass(fh, rate, q2).unwrap_or(unity);
            let ap_h1 = BiquadCoefficients::all_pass(fh, rate, q1).unwrap_or(unity);
            let ap_h2 = BiquadCoefficients::all_pass(fh, rate, q2).unwrap_or(unity);
            Self {
                low_lp: [lp_l1, lp_l2, lp_l1, lp_l2],
                low_hp: [hp_l1, hp_l2, hp_l1, hp_l2],
                high_lp: [lp_h1, lp_h2, lp_h1, lp_h2],
                high_hp: [hp_h1, hp_h2, hp_h1, hp_h2],
                low_phase_ap: [ap_h1, ap_h2],
                stages: 4,
                ap_stages: 2,
            }
        }
    }
}

impl EqThreeDefinition {
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedEqThree, DeviceContractError> {
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
        let gain_smoothers = [
            make_smoother(band_linear_gain(settings.gain_low_db, settings.kill_low))?,
            make_smoother(band_linear_gain(settings.gain_mid_db, settings.kill_mid))?,
            make_smoother(band_linear_gain(settings.gain_high_db, settings.kill_high))?,
        ];
        let freq_low_smoother = make_smoother(f64::from(settings.freq_low_hz))?;
        let freq_high_smoother = make_smoother(f64::from(settings.freq_high_hz))?;
        let crossover = CrossoverPair::compute(
            f64::from(settings.freq_low_hz),
            f64::from(settings.freq_high_hz),
            settings.slope != 0,
            sample_rate,
        );
        Ok(PreparedEqThree {
            settings,
            sample_rate,
            admitted,
            gain_smoothers,
            freq_low_smoother,
            freq_high_smoother,
            crossover,
            silence_frames: sample_rate / 2,
        })
    }
}

impl NativeDeviceDefinition for EqThreeDefinition {
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
pub struct PreparedEqThree {
    settings: Settings,
    sample_rate: u32,
    admitted: [bool; STATE_WORDS],
    gain_smoothers: [OnePoleSmoother; 3],
    freq_low_smoother: OnePoleSmoother,
    freq_high_smoother: OnePoleSmoother,
    crossover: CrossoverPair,
    silence_frames: u32,
}

impl PreparedEqThree {
    fn supports(&self, parameter: ParameterId) -> bool {
        BINDINGS
            .iter()
            .position(|b| b.descriptor.id == parameter)
            .is_some_and(|index| self.admitted[index])
    }
}

impl PreparedNativeProcessor for PreparedEqThree {
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
        size_of::<EqThreeRuntime>()
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

impl crate::inline_owned::InlinePreparedProcessor for PreparedEqThree {
    type Runtime = EqThreeRuntime;
    fn create_inline_runtime(&self) -> Self::Runtime {
        EqThreeRuntime {
            base: *self,
            controls: crate::parameter_binding::Controls::new(self.settings, self.admitted),
            gains: self.gain_smoothers,
            freq_low: self.freq_low_smoother,
            freq_high: self.freq_high_smoother,
            slope_48: self.settings.slope != 0,
            crossover: self.crossover,
            channels: [ChannelState::default(); 2],
            silent_frames: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ChannelState {
    low_lp: [[f64; 2]; 4],
    low_hp: [[f64; 2]; 4],
    high_lp: [[f64; 2]; 4],
    high_hp: [[f64; 2]; 4],
    low_ap: [[f64; 2]; 2],
}

impl ChannelState {
    fn is_clear(&self) -> bool {
        *self == Self::default()
    }

    fn process(&mut self, input: f64, c: &CrossoverPair, gains: [f64; 3]) -> f64 {
        let mut low_raw = input;
        let mut upper = input;
        for i in 0..c.stages {
            low_raw = c.low_lp[i].process_tdf2_paired_flush(low_raw, &mut self.low_lp[i]);
            upper = c.low_hp[i].process_tdf2_paired_flush(upper, &mut self.low_hp[i]);
        }
        let mut mid = upper;
        let mut high = upper;
        for i in 0..c.stages {
            mid = c.high_lp[i].process_tdf2_paired_flush(mid, &mut self.high_lp[i]);
            high = c.high_hp[i].process_tdf2_paired_flush(high, &mut self.high_hp[i]);
        }
        let mut low = low_raw;
        for i in 0..c.ap_stages {
            low = c.low_phase_ap[i].process_tdf2_paired_flush(low, &mut self.low_ap[i]);
        }
        flush_quiet(low * gains[0] + mid * gains[1] + high * gains[2])
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EqThreeRuntime {
    base: PreparedEqThree,
    controls: crate::parameter_binding::Controls<Settings, STATE_WORDS>,
    gains: [OnePoleSmoother; 3],
    freq_low: OnePoleSmoother,
    freq_high: OnePoleSmoother,
    slope_48: bool,
    crossover: CrossoverPair,
    channels: [ChannelState; 2],
    silent_frames: u32,
}

impl EqThreeRuntime {
    fn apply_targets(&mut self, settings: Settings) {
        self.gains[0].set_target(band_linear_gain(settings.gain_low_db, settings.kill_low));
        self.gains[1].set_target(band_linear_gain(settings.gain_mid_db, settings.kill_mid));
        self.gains[2].set_target(band_linear_gain(settings.gain_high_db, settings.kill_high));
        self.freq_low.set_target(f64::from(settings.freq_low_hz));
        self.freq_high.set_target(f64::from(settings.freq_high_hz));
        let next_slope = settings.slope != 0;
        if next_slope != self.slope_48 {
            self.slope_48 = next_slope;
            self.crossover = CrossoverPair::compute(
                self.freq_low.value(),
                self.freq_high.value(),
                self.slope_48,
                self.base.sample_rate,
            );
        }
    }

    fn advance_parameters(&mut self) -> [f64; 3] {
        let gains = [
            self.gains[0].advance(),
            self.gains[1].advance(),
            self.gains[2].advance(),
        ];
        if !self.freq_low.is_settled() || !self.freq_high.is_settled() {
            let fl = self.freq_low.advance();
            let fh = self.freq_high.advance();
            self.crossover = CrossoverPair::compute(fl, fh, self.slope_48, self.base.sample_rate);
        }
        gains
    }

    fn process_frame(&mut self, frame: &mut StereoFrame) {
        let gains = self.advance_parameters();
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
            *sample = channel.process(f64::from(*sample), &self.crossover, gains) as f32;
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

impl NativeProcessorRuntime for EqThreeRuntime {
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
        self.gains = self.base.gain_smoothers;
        self.freq_low = self.base.freq_low_smoother;
        self.freq_high = self.base.freq_high_smoother;
        self.slope_48 = self.base.settings.slope != 0;
        self.crossover = self.base.crossover;
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
        for (dst, src) in self.gains.iter_mut().zip(&previous.gains) {
            dst.inherit_value(src);
        }
        self.freq_low.inherit_value(&previous.freq_low);
        self.freq_high.inherit_value(&previous.freq_high);
        self.crossover = CrossoverPair::compute(
            self.freq_low.value(),
            self.freq_high.value(),
            self.slope_48,
            self.base.sample_rate,
        );
        if self.base.admitted == previous.base.admitted && self.base.admitted.iter().any(|&a| a) {
            self.controls.inherit(&previous.controls);
            for (dst, src) in self.gains.iter_mut().zip(&previous.gains) {
                dst.set_target(src.target());
            }
            self.freq_low.set_target(previous.freq_low.target());
            self.freq_high.set_target(previous.freq_high.target());
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

    fn rms_of_sine(state: &NativeDeviceState, sample_rate: u32, freq_hz: f64) -> f64 {
        let prepared = EqThreeDefinition::prepare_owned(state, sample_rate, &[]).unwrap();
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
        (energy / (total - settle) as f64).sqrt()
    }

    #[test]
    fn flat_sum_is_unity_across_audio_band_for_both_lr4_and_lr8_slopes() {
        let ref_rms = std::f64::consts::FRAC_1_SQRT_2;
        for slope in [0u32, 1u32] {
            let state = EQ_THREE_DEFINITION
                .patch_parameter(
                    &EQ_THREE_DEFINITION.default_state().unwrap(),
                    parameter::SLOPE,
                    ParameterValue::Choice(slope),
                )
                .unwrap();
            for freq in [60.0, 250.0, 800.0, 2_500.0, 8_000.0] {
                let rms = rms_of_sine(&state, 48_000, freq);
                let db_err = 20.0 * (rms / ref_rms).log10();
                assert!(
                    db_err.abs() < 0.05,
                    "slope={slope} freq={freq} hz deviated by {db_err:.4} db from flat unity"
                );
            }
        }
    }

    #[test]
    fn band_kills_and_gains_isolate_low_mid_and_high_cleanly() {
        let base = EQ_THREE_DEFINITION.default_state().unwrap();
        // Kill mid + high -> only low passes.
        let low_only = EQ_THREE_DEFINITION
            .patch_parameter(&base, parameter::KILL_MID, ParameterValue::Bool(true))
            .and_then(|s| {
                EQ_THREE_DEFINITION.patch_parameter(
                    &s,
                    parameter::KILL_HIGH,
                    ParameterValue::Bool(true),
                )
            })
            .unwrap();
        let low_at_60 = rms_of_sine(&low_only, 48_000, 60.0);
        let low_at_4k = rms_of_sine(&low_only, 48_000, 4_000.0);
        assert!(low_at_60 > 0.65, "low passband should retain 60 hz");
        assert!(
            20.0 * (low_at_4k / low_at_60).log10() < -40.0,
            "low band must attenuate 4 khz by > 40 db"
        );

        // Kill low + high -> only mid passes.
        let mid_only = EQ_THREE_DEFINITION
            .patch_parameter(&base, parameter::KILL_LOW, ParameterValue::Bool(true))
            .and_then(|s| {
                EQ_THREE_DEFINITION.patch_parameter(
                    &s,
                    parameter::KILL_HIGH,
                    ParameterValue::Bool(true),
                )
            })
            .unwrap();
        let mid_at_800 = rms_of_sine(&mid_only, 48_000, 800.0);
        let mid_at_50 = rms_of_sine(&mid_only, 48_000, 50.0);
        let mid_at_10k = rms_of_sine(&mid_only, 48_000, 10_000.0);
        assert!(mid_at_800 > 0.65);
        assert!(20.0 * (mid_at_50 / mid_at_800).log10() < -25.0);
        assert!(20.0 * (mid_at_10k / mid_at_800).log10() < -20.0);

        // +6 dB boost on high band amplifies 8 kHz by ~6 dB.
        let high_boost = EQ_THREE_DEFINITION
            .patch_parameter(&base, parameter::GAIN_HIGH, ParameterValue::Float(6.0))
            .unwrap();
        let boost_db = 20.0
            * (rms_of_sine(&high_boost, 48_000, 8_000.0) / std::f64::consts::FRAC_1_SQRT_2).log10();
        assert!(
            (boost_db - 6.0).abs() < 0.2,
            "expected +6 db boost at 8 khz, got {boost_db:.3} db"
        );
    }

    #[test]
    fn block_partition_invariant_finite_tail_and_malformed_state_refusal() {
        let state = EQ_THREE_DEFINITION
            .patch_parameter(
                &EQ_THREE_DEFINITION.default_state().unwrap(),
                parameter::GAIN_LOW,
                ParameterValue::Float(-12.0),
            )
            .unwrap();
        let prepared = EqThreeDefinition::prepare_owned(&state, 48_000, &[]).unwrap();
        let mut single =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prepared);
        let mut chunked =
            crate::inline_owned::InlinePreparedProcessor::create_inline_runtime(&prepared);

        let input: Vec<StereoFrame> = (0..512)
            .map(|i| {
                let s = ((i as f32) * 0.13).sin() * 0.5;
                [s, -s * 0.7]
            })
            .collect();
        let mut out_single = input.clone();
        single.process_block(&mut out_single, &hex_lattice::EMPTY_DEVICE_RESOURCES);

        let mut out_chunked = input;
        let mut pos = 0;
        for size in [1, 7, 19, 64, 128, 293] {
            let end = (pos + size).min(out_chunked.len());
            chunked.process_block(
                &mut out_chunked[pos..end],
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            pos = end;
        }
        assert_eq!(out_single, out_chunked);

        // Finite tail reaches quiescence after silence window.
        let mut silence = vec![[0.0f32; 2]; 25_000];
        single.process_block(&mut silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        assert!(single.is_quiescent());

        // Malformed states refused cleanly.
        assert!(
            decode(&NativeDeviceState::new(EQ_THREE_TYPE_ID, 2, vec![0; STATE_BYTES]).unwrap())
                .is_err()
        );
        assert!(decode(&NativeDeviceState::new(EQ_THREE_TYPE_ID, 1, vec![0; 4]).unwrap()).is_err());
        let mut bad_bool = state.payload().to_vec();
        bad_bool[12..16].copy_from_slice(&2u32.to_le_bytes());
        assert!(decode(&NativeDeviceState::new(EQ_THREE_TYPE_ID, 1, bad_bool).unwrap()).is_err());
    }
}
