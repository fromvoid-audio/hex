//! Original stereo Auto Filter (`hex.filter`).
//!
//! # Saved-State Compatibility
//! - **Version 2 (current)**: 22 32-bit little-endian words (88 bytes) in
//!   descriptor order, adding DJ / Notch+LP / Comb / Resampling / Vowel modes,
//!   circuit saturation variants + drive, beat-synced LFO with stereo spin and
//!   deterministic S&H, envelope peak-hold, and envelope key routing (`#8951`).
//! - **Version 1 (supported historical format)**: 13 32-bit little-endian words
//!   (52 bytes). Migrated losslessly on control in [`Settings::decode`] by
//!   filling words `13..22` with transparent clean defaults so migrated v1
//!   projects render bit-for-bit identically to v1.
//! - Newer (`> 2`), zero, or malformed payloads are refused explicitly without
//!   mutating or discarding project data.

mod runtime;

use hex_lattice::{
    DevicePreparationContext, NativeDeviceDefinition, PreparedNativeProcessor,
    PreparedRuntimeDevice, ProcessorInstanceIdentity, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterAutomation, ParameterDescriptor, ParameterId, ParameterKind, ParameterModulation,
    ParameterNormalization, ParameterPresentation, ParameterSemantics, ParameterSmoothing,
    ParameterValue,
};
pub use runtime::{FilterRuntime, PreparedFilter};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.filter");
const V1_VERSION: u32 = 1;
const V1_COUNT: usize = 13;
const VERSION: u32 = 2;
const COUNT: usize = 22;
const WORD_BYTES: usize = size_of::<u32>();

pub const MODES: &[&str] = &[
    "low-pass",
    "high-pass",
    "band-pass",
    "notch",
    "morph",
    "dj",
    "notch + lp",
    "comb",
    "resampling",
    "vowel",
];
pub const LFO_SHAPES: &[&str] = &["sine", "triangle", "saw up", "saw down", "square", "s&h"];
pub const LFO_SYNC_RATES: &[&str] = &[
    "1/1", "1/2", "1/4", "1/8", "1/16", "1/32", "1/4t", "1/8t", "1/4d", "1/8d",
];
pub const ENV_KEY_ROUTINGS: &[&str] = &["stereo", "left", "right", "mid", "side"];

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const TYPE: ParameterId = ParameterId::from_static(1);
    pub const SLOPE: ParameterId = ParameterId::from_static(2);
    pub const CUTOFF: ParameterId = ParameterId::from_static(3);
    pub const RESONANCE: ParameterId = ParameterId::from_static(4);
    pub const MORPH: ParameterId = ParameterId::from_static(5);
    pub const MIX: ParameterId = ParameterId::from_static(6);
    pub const LFO_AMOUNT: ParameterId = ParameterId::from_static(7);
    pub const LFO_RATE: ParameterId = ParameterId::from_static(8);
    pub const LFO_SHAPE: ParameterId = ParameterId::from_static(9);
    pub const LFO_PHASE: ParameterId = ParameterId::from_static(10);
    pub const ENV_AMOUNT: ParameterId = ParameterId::from_static(11);
    pub const ENV_RELEASE: ParameterId = ParameterId::from_static(12);
    pub const ENV_ATTACK: ParameterId = ParameterId::from_static(13);
    pub const CIRCUIT: ParameterId = ParameterId::from_static(14);
    pub const DRIVE: ParameterId = ParameterId::from_static(15);
    pub const LFO_SYNC: ParameterId = ParameterId::from_static(16);
    pub const LFO_SYNC_RATE: ParameterId = ParameterId::from_static(17);
    pub const LFO_STEREO_MODE: ParameterId = ParameterId::from_static(18);
    pub const LFO_SPIN: ParameterId = ParameterId::from_static(19);
    pub const ENV_HOLD: ParameterId = ParameterId::from_static(20);
    pub const ENV_KEY_ROUTING: ParameterId = ParameterId::from_static(21);
    pub const LFO_OFFSET: ParameterId = ParameterId::from_static(22);
}

const TYPE: usize = 0;
const SLOPE: usize = 1;
const CUTOFF: usize = 2;
const RESONANCE: usize = 3;
const MORPH: usize = 4;
const MIX: usize = 5;
const LFO_AMOUNT: usize = 6;
const LFO_RATE: usize = 7;
const LFO_SHAPE: usize = 8;
const LFO_PHASE: usize = 9;
const ENV_AMOUNT: usize = 10;
const ENV_RELEASE: usize = 11;
const ENV_ATTACK: usize = 12;
const CIRCUIT: usize = 13;
const DRIVE: usize = 14;
const LFO_SYNC: usize = 15;
const LFO_SYNC_RATE: usize = 16;
const LFO_STEREO_MODE: usize = 17;
const LFO_SPIN: usize = 18;
const ENV_HOLD: usize = 19;
const ENV_KEY_ROUTING: usize = 20;
const LFO_OFFSET: usize = 21;

const fn choice(
    index: usize,
    label: &'static str,
    labels: &'static [&'static str],
) -> ParameterDescriptor {
    choice_with_default(index, label, labels, 0)
}

const fn choice_with_default(
    index: usize,
    label: &'static str,
    labels: &'static [&'static str],
    default: u32,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(index as u32 + 1),
        label,
        kind: ParameterKind::Choice { labels },
        default: ParameterValue::Choice(default),
        presentation: ParameterPresentation::Linear,
    }
}

const fn value(
    index: usize,
    label: &'static str,
    unit: &'static str,
    min: f32,
    max: f32,
    default: f32,
    presentation: ParameterPresentation,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(index as u32 + 1),
        label,
        kind: ParameterKind::Float {
            min,
            max,
            unit: Some(unit),
        },
        default: ParameterValue::Float(default),
        presentation,
    }
}

use ParameterPresentation::{Linear, Logarithmic};

const DESCRIPTORS: [ParameterDescriptor; COUNT] = [
    choice(TYPE, "type", MODES),
    choice(SLOPE, "slope", &["12 db", "24 db"]),
    value(CUTOFF, "cutoff", "hz", 20.0, 20_000.0, 1000.0, Logarithmic),
    value(RESONANCE, "resonance", "%", 0.0, 100.0, 0.0, Linear),
    value(MORPH, "morph", "%", 0.0, 100.0, 50.0, Linear),
    value(MIX, "dry/wet", "%", 0.0, 100.0, 100.0, Linear),
    value(LFO_AMOUNT, "lfo amount", "oct", 0.0, 4.0, 0.0, Linear),
    value(LFO_RATE, "lfo rate", "hz", 0.02, 30.0, 0.5, Logarithmic),
    choice(LFO_SHAPE, "lfo shape", LFO_SHAPES),
    value(LFO_PHASE, "lfo stereo phase", "°", 0.0, 360.0, 0.0, Linear),
    value(ENV_AMOUNT, "envelope amount", "oct", -4.0, 4.0, 0.0, Linear),
    value(
        ENV_RELEASE,
        "envelope release",
        "ms",
        5.0,
        2000.0,
        120.0,
        Logarithmic,
    ),
    value(
        ENV_ATTACK,
        "envelope attack",
        "ms",
        0.1,
        200.0,
        5.0,
        Logarithmic,
    ),
    choice(CIRCUIT, "circuit", crate::operator_filter::CIRCUITS),
    value(DRIVE, "drive", "db", 0.0, 24.0, 0.0, Linear),
    choice(LFO_SYNC, "lfo sync", &["free", "sync"]),
    choice_with_default(LFO_SYNC_RATE, "lfo sync rate", LFO_SYNC_RATES, 2),
    choice(LFO_STEREO_MODE, "lfo stereo mode", &["phase", "spin"]),
    value(LFO_SPIN, "lfo spin", "%", 0.0, 50.0, 0.0, Linear),
    value(ENV_HOLD, "envelope hold", "ms", 0.0, 500.0, 0.0, Linear),
    choice(ENV_KEY_ROUTING, "envelope key", ENV_KEY_ROUTINGS),
    value(LFO_OFFSET, "lfo offset", "°", 0.0, 360.0, 0.0, Linear),
];

const STATIC_CHOICE: ParameterSemantics = ParameterSemantics {
    automation: ParameterAutomation::Unsupported,
    modulation: ParameterModulation::Unsupported,
    smoothing: ParameterSmoothing::None,
    ..crate::parameter_binding::CHOICE_ENVELOPE
};

/// Continuous parameters smoothed by the device:
/// - `CUTOFF`, `RESONANCE`, `MORPH`, `MIX`: 5 ms SVF coefficient ramp on
///   discrete edits and stepped automation; direct sample-rate tracking when
///   driven by `SampleRate` modulation (`has_sample_rate_modulation`).
/// - `LFO_AMOUNT`, `LFO_PHASE`, `ENV_AMOUNT`: routed through the per-channel
///   2 ms `OnePoleSmoother` applied to the combined cutoff modulation offset.
const MODULATED_LINEAR: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    modulation: ParameterModulation::SampleRate,
    ..crate::parameter_binding::SCALAR_ENVELOPE
};

const MODULATED_LOG: ParameterSemantics = ParameterSemantics {
    normalization: ParameterNormalization::PositiveLogarithmic,
    ..MODULATED_LINEAR
};

/// Continuous logarithmic parameters applied directly without an extra ramp:
/// - `LFO_RATE`: integrated phase-continuously into the stereo LFO accumulator.
/// - `ENV_ATTACK`, `ENV_RELEASE`: update cached one-pole follower coefficients
///   directly per sample.
const MODULATED_DIRECT_LOG: ParameterSemantics = ParameterSemantics {
    normalization: ParameterNormalization::PositiveLogarithmic,
    smoothing: ParameterSmoothing::None,
    modulation: ParameterModulation::SampleRate,
    ..crate::parameter_binding::SCALAR_ENVELOPE
};

const BINDINGS: [crate::parameter_binding::Binding<Settings>; COUNT] = [
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[TYPE],
        STATIC_CHOICE,
        |s| s.0[TYPE],
        |s, v| {
            s.0[TYPE] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[SLOPE],
        STATIC_CHOICE,
        |s| s.0[SLOPE],
        |s, v| {
            s.0[SLOPE] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[CUTOFF],
        MODULATED_LOG,
        |s| s.f32_at(CUTOFF),
        |s, v| s.set_f32(CUTOFF, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[RESONANCE],
        MODULATED_LINEAR,
        |s| s.f32_at(RESONANCE),
        |s, v| s.set_f32(RESONANCE, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[MORPH],
        MODULATED_LINEAR,
        |s| s.f32_at(MORPH),
        |s, v| s.set_f32(MORPH, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[MIX],
        MODULATED_LINEAR,
        |s| s.f32_at(MIX),
        |s, v| s.set_f32(MIX, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[LFO_AMOUNT],
        MODULATED_LINEAR,
        |s| s.f32_at(LFO_AMOUNT),
        |s, v| s.set_f32(LFO_AMOUNT, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[LFO_RATE],
        MODULATED_DIRECT_LOG,
        |s| s.f32_at(LFO_RATE),
        |s, v| s.set_f32(LFO_RATE, v),
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[LFO_SHAPE],
        STATIC_CHOICE,
        |s| s.0[LFO_SHAPE],
        |s, v| {
            s.0[LFO_SHAPE] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[LFO_PHASE],
        MODULATED_LINEAR,
        |s| s.f32_at(LFO_PHASE),
        |s, v| s.set_f32(LFO_PHASE, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[ENV_AMOUNT],
        MODULATED_LINEAR,
        |s| s.f32_at(ENV_AMOUNT),
        |s, v| s.set_f32(ENV_AMOUNT, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[ENV_RELEASE],
        MODULATED_DIRECT_LOG,
        |s| s.f32_at(ENV_RELEASE),
        |s, v| s.set_f32(ENV_RELEASE, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[ENV_ATTACK],
        MODULATED_DIRECT_LOG,
        |s| s.f32_at(ENV_ATTACK),
        |s, v| s.set_f32(ENV_ATTACK, v),
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[CIRCUIT],
        STATIC_CHOICE,
        |s| s.0[CIRCUIT],
        |s, v| {
            s.0[CIRCUIT] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[DRIVE],
        MODULATED_LINEAR,
        |s| s.f32_at(DRIVE),
        |s, v| s.set_f32(DRIVE, v),
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[LFO_SYNC],
        STATIC_CHOICE,
        |s| s.0[LFO_SYNC],
        |s, v| {
            s.0[LFO_SYNC] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[LFO_SYNC_RATE],
        STATIC_CHOICE,
        |s| s.0[LFO_SYNC_RATE],
        |s, v| {
            s.0[LFO_SYNC_RATE] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[LFO_STEREO_MODE],
        STATIC_CHOICE,
        |s| s.0[LFO_STEREO_MODE],
        |s, v| {
            s.0[LFO_STEREO_MODE] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[LFO_SPIN],
        MODULATED_LINEAR,
        |s| s.f32_at(LFO_SPIN),
        |s, v| s.set_f32(LFO_SPIN, v),
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[ENV_HOLD],
        MODULATED_LINEAR,
        |s| s.f32_at(ENV_HOLD),
        |s, v| s.set_f32(ENV_HOLD, v),
    ),
    crate::parameter_binding::Binding::choice(
        DESCRIPTORS[ENV_KEY_ROUTING],
        STATIC_CHOICE,
        |s| s.0[ENV_KEY_ROUTING],
        |s, v| {
            s.0[ENV_KEY_ROUTING] = v;
            Ok(())
        },
    ),
    crate::parameter_binding::Binding::float(
        DESCRIPTORS[LFO_OFFSET],
        MODULATED_LINEAR,
        |s| s.f32_at(LFO_OFFSET),
        |s, v| s.set_f32(LFO_OFFSET, v),
    ),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Settings([u32; COUNT]);

impl Settings {
    fn default_values() -> Result<Self, DeviceContractError> {
        let mut settings = Self([0; COUNT]);
        for (index, descriptor) in DESCRIPTORS.iter().enumerate() {
            settings.write(index, descriptor.default)?;
        }
        Ok(settings)
    }

    fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        let word_count = match state.state_version() {
            V1_VERSION => V1_COUNT,
            VERSION => COUNT,
            _ => return Err(DeviceContractError::UnsupportedStateVersion),
        };
        if state.payload().len() != word_count * WORD_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut settings = Self::default_values()?;
        for (word, bytes) in settings.0[..word_count]
            .iter_mut()
            .zip(state.payload().chunks_exact(WORD_BYTES))
        {
            *word = u32::from_le_bytes(
                bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
        }
        if state.state_version() == V1_VERSION {
            // Historical v1 only permitted TYPE 0..=4 and LFO_SHAPE 0..=4.
            if settings.0[TYPE] >= 5 || settings.0[LFO_SHAPE] >= 5 {
                return Err(DeviceContractError::MalformedState);
            }
        }
        for (index, descriptor) in DESCRIPTORS.iter().enumerate() {
            descriptor.validate(settings.read(index))?;
        }
        Ok(settings)
    }

    fn encode(self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(
            TYPE_ID,
            VERSION,
            self.0.into_iter().flat_map(u32::to_le_bytes).collect(),
        )
    }

    fn f32_at(self, index: usize) -> f32 {
        f32::from_bits(self.0[index])
    }

    fn set_f32(&mut self, index: usize, value: f32) {
        self.0[index] = value.to_bits();
    }

    fn float(self, index: usize) -> f64 {
        f64::from(f32::from_bits(self.0[index]))
    }

    fn read(self, index: usize) -> ParameterValue {
        if matches!(DESCRIPTORS[index].kind, ParameterKind::Choice { .. }) {
            ParameterValue::Choice(self.0[index])
        } else {
            ParameterValue::Float(f32::from_bits(self.0[index]))
        }
    }

    fn write(&mut self, index: usize, value: ParameterValue) -> Result<(), DeviceContractError> {
        DESCRIPTORS[index].validate(value)?;
        self.0[index] = match value {
            ParameterValue::Float(value) => value.to_bits(),
            ParameterValue::Choice(value) => value,
            ParameterValue::Bool(_) => return Err(DeviceContractError::InvalidParameterValue),
        };
        Ok(())
    }
}

fn parameter_index(id: ParameterId) -> Result<usize, DeviceContractError> {
    let index = id
        .value()
        .checked_sub(1)
        .ok_or(DeviceContractError::UnknownParameter)? as usize;
    if index < COUNT {
        Ok(index)
    } else {
        Err(DeviceContractError::UnknownParameter)
    }
}

/// Built-in effect metadata and validated control-side construction.
pub struct FilterDefinition;

pub static DEFINITION: FilterDefinition = FilterDefinition;

impl FilterDefinition {
    /// Sealed inline preparation for track, source and bus effect chains.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedFilter, DeviceContractError> {
        state.require_asset_free_processing()?;
        if parameters.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut admitted = [false; COUNT];
        for &id in parameters {
            let binding = crate::parameter_binding::binding(&BINDINGS, id)?;
            binding.semantics.validate(binding.descriptor)?;
            if !binding.declares_arrangement_automation() {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            admitted[parameter_index(id)?] = true;
        }
        PreparedFilter::new(Settings::decode(state)?, rate, admitted)
    }
}

impl NativeDeviceMetadata for FilterDefinition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("filter")
    }
    fn state_version(&self) -> u32 {
        VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<ParameterSemantics>, DeviceContractError> {
        crate::parameter_binding::parameter_semantics_for_state(
            &BINDINGS,
            state,
            id,
            Settings::decode,
        )
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Settings::default_values()?.encode()
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Settings::decode(state)?.encode()
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Ok(Settings::decode(state)?.read(parameter_index(id)?))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = Settings::decode(state)?;
        settings.write(parameter_index(id)?, value)?;
        settings.encode()
    }
}

impl NativeDeviceDefinition for FilterDefinition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        Some(crate::source_effect_arc_bytes::<PreparedFilter>())
    }
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        Settings::decode(state)?;
        Ok(crate::parameter_binding::binding(&BINDINGS, parameter)
            .is_ok_and(|binding| binding.declares_arrangement_automation()))
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
    ) -> Result<Option<hex_lattice::PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(crate::inline_owned::owned_factory(
            Self::prepare_owned(state, context.sample_rate, &[])?,
        )))
    }
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Settings::decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Settings::decode(state)?;
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

/// Static wet response before LFO and envelope modulation. Coordinates are
/// normalized across 20 Hz to the project rate's supported upper cutoff and
/// -48 to +48 dB. The provider's prepared kernel owns these response values.
pub fn response(
    state: &NativeDeviceState,
    rate: u32,
) -> Result<[[f32; 2]; 129], DeviceContractError> {
    modulated_response(state, rate, 0.0)
}

/// Live modulated wet response at `cutoff_octave_offset` octaves across all 10
/// Auto Filter modes (`#8951`). Coordinates match [`response`].
pub fn modulated_response(
    state: &NativeDeviceState,
    rate: u32,
    cutoff_octave_offset: f64,
) -> Result<[[f32; 2]; 129], DeviceContractError> {
    if !cutoff_octave_offset.is_finite() {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let prepared = FilterDefinition::prepare_owned(state, rate, &[])?;
    let upper = 20_000.0_f64.min(f64::from(rate) * 0.45);
    Ok(std::array::from_fn(|index| {
        let x = index as f64 / 128.0;
        let frequency = 20.0 * (upper / 20.0).powf(x);
        let magnitude = runtime::mode_magnitude_at_hz(&prepared, cutoff_octave_offset, frequency);
        let y = ((20.0 * magnitude.max(1e-6).log10() + 48.0) / 96.0).clamp(0.0, 1.0) as f32;
        [x as f32, y]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeParameterAutomation, OutputTail};

    struct RampAutomation {
        ids: Vec<ParameterId>,
        start: f32,
        end: f32,
        length: usize,
        offset_base: usize,
    }

    impl NativeParameterAutomation for RampAutomation {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, id: ParameterId, frame_offset: usize) -> Option<ParameterValue> {
            if !self.ids.contains(&id) {
                return None;
            }
            let frame = (self.offset_base + frame_offset).min(self.length);
            let t = frame as f32 / self.length.max(1) as f32;
            Some(ParameterValue::Float(
                self.start + (self.end - self.start) * t,
            ))
        }
    }

    fn parameter(index: usize) -> ParameterId {
        DESCRIPTORS[index].id
    }

    fn patch(state: &NativeDeviceState, index: usize, value: ParameterValue) -> NativeDeviceState {
        DEFINITION
            .patch_parameter(state, parameter(index), value)
            .unwrap()
    }

    fn signal(frequency: f64) -> Vec<[f32; 2]> {
        (0..8192)
            .map(|sample| {
                let sample =
                    (std::f64::consts::TAU * frequency * f64::from(sample) / 48_000.0).sin() as f32;
                [sample; 2]
            })
            .collect()
    }

    fn render(state: &NativeDeviceState, signal: &mut [[f32; 2]], block: usize) {
        let mut runtime = FilterDefinition::prepare_owned(state, 48_000, &[])
            .unwrap()
            .create_runtime();
        for block in signal.chunks_mut(block) {
            runtime.process_block(block, &EMPTY_DEVICE_RESOURCES);
        }
    }

    fn power(signal: &[[f32; 2]]) -> f32 {
        signal[4096..].iter().map(|frame| frame[0] * frame[0]).sum()
    }

    #[test]
    fn cutoff_type_and_slope_follow_the_audio_response_and_dry_is_transparent() {
        let low = DEFINITION.default_state().unwrap();
        let high = patch(&low, TYPE, ParameterValue::Choice(1));
        let steep = patch(&low, SLOPE, ParameterValue::Choice(1));
        let dry = patch(&low, MIX, ParameterValue::Float(0.0));
        let mut pass = signal(4000.0);
        let input = pass.clone();
        let mut stop = input.clone();
        let mut steep_stop = input.clone();
        let mut dry_output = input.clone();
        render(&high, &mut pass, 127);
        render(&low, &mut stop, 127);
        render(&steep, &mut steep_stop, 127);
        render(&dry, &mut dry_output, 127);
        assert!(power(&pass) > 0.95 * power(&input));
        assert!(power(&stop) < power(&input) * 0.005);
        assert!(power(&steep_stop) < power(&stop) * 0.005);
        assert_eq!(dry_output, input);
        assert!(response(&low, 48_000).unwrap()[128][1] < response(&high, 48_000).unwrap()[128][1]);
    }

    #[test]
    fn cutoff_modulation_respects_polarity_stereo_phase_and_block_partition() {
        let state = DEFINITION.default_state().unwrap();
        let positive = patch(&state, ENV_AMOUNT, ParameterValue::Float(3.0));
        let negative = patch(&state, ENV_AMOUNT, ParameterValue::Float(-3.0));
        let lfo = patch(&state, LFO_AMOUNT, ParameterValue::Float(2.0));
        let lfo = patch(&lfo, LFO_PHASE, ParameterValue::Float(180.0));
        let input = signal(2000.0);
        let mut closed = input.clone();
        let mut open = input.clone();
        let mut whole = input.clone();
        let mut partitioned = input.clone();
        render(&negative, &mut closed, 256);
        render(&positive, &mut open, 256);
        render(&lfo, &mut whole, 8192);
        render(&lfo, &mut partitioned, 113);
        assert!(power(&open) > power(&closed) * 10.0);
        assert_eq!(whole, partitioned);
        assert!(whole.iter().any(|frame| (frame[0] - frame[1]).abs() > 0.1));
    }

    #[test]
    fn codec_round_trips_and_refuses_unvalidated_values_and_formats() {
        let original = DEFINITION.default_state().unwrap();
        let changed = patch(&original, CUTOFF, ParameterValue::Float(100.0));
        assert_eq!(DEFINITION.migrate_state(&changed).unwrap(), changed);
        assert_eq!(
            DEFINITION.read_parameter(&changed, parameter(CUTOFF)),
            Ok(ParameterValue::Float(100.0))
        );
        assert!(
            DEFINITION
                .patch_parameter(&changed, parameter(CUTOFF), ParameterValue::Float(f32::NAN))
                .is_err()
        );
        for version in [VERSION + 1, VERSION] {
            let broken = NativeDeviceState::new(TYPE_ID, version, vec![0; 4]).unwrap();
            assert!(DEFINITION.migrate_state(&broken).is_err());
        }
    }

    #[test]
    fn continuous_parameters_declare_automation_and_sample_rate_modulation_and_choices_refuse() {
        let state = DEFINITION.default_state().unwrap();
        let continuous = [
            parameter::CUTOFF,
            parameter::RESONANCE,
            parameter::MORPH,
            parameter::MIX,
            parameter::LFO_AMOUNT,
            parameter::LFO_RATE,
            parameter::LFO_PHASE,
            parameter::ENV_AMOUNT,
            parameter::ENV_RELEASE,
            parameter::ENV_ATTACK,
        ];
        for id in continuous {
            assert_eq!(
                DEFINITION.supports_parameter_automation(&state, id),
                Ok(true)
            );
            let semantics = DEFINITION
                .parameter_semantics_for_state(&state, id)
                .unwrap()
                .expect("semantics declared");
            assert_eq!(semantics.modulation, ParameterModulation::SampleRate);
            assert_eq!(
                semantics.automation,
                ParameterAutomation::DeviceArrangementEnvelope
            );
            let prepared = FilterDefinition::prepare_owned(&state, 48_000, &[id]).unwrap();
            assert!(prepared.supports_parameter_automation(id));
            assert_eq!(prepared.parameter_modulation_semantics(id), Some(semantics));
            assert!(prepared.parameter_automation_descriptor(id).is_some());
        }

        for discrete in [parameter::TYPE, parameter::SLOPE, parameter::LFO_SHAPE] {
            assert_eq!(
                DEFINITION.supports_parameter_automation(&state, discrete),
                Ok(false)
            );
            assert_eq!(
                FilterDefinition::prepare_owned(&state, 48_000, &[discrete]).err(),
                Some(DeviceContractError::UnsupportedProcessingClass)
            );
        }

        let unknown = ParameterId::from_static(99);
        assert_eq!(
            DEFINITION.supports_parameter_automation(&state, unknown),
            Ok(false)
        );
        assert_eq!(
            FilterDefinition::prepare_owned(&state, 48_000, &[unknown]).err(),
            Some(DeviceContractError::UnknownParameter)
        );
    }

    #[test]
    fn automated_cutoff_sweep_matches_across_block_partitions_and_preserves_bypass() {
        let state = DEFINITION.default_state().unwrap();
        let prepared =
            FilterDefinition::prepare_owned(&state, 48_000, &[parameter::CUTOFF]).unwrap();
        let input = signal(4000.0);

        let mut whole = input.clone();
        let mut rt_whole = prepared.create_runtime();
        let auto_whole = RampAutomation {
            ids: vec![parameter::CUTOFF],
            start: 150.0,
            end: 12_000.0,
            length: input.len(),
            offset_base: 0,
        };
        rt_whole
            .process_block_with_automation(&mut whole, None, &auto_whole, &EMPTY_DEVICE_RESOURCES)
            .unwrap();

        let mut partitioned = input.clone();
        let mut rt_part = prepared.create_runtime();
        let mut offset = 0;
        for chunk in partitioned.chunks_mut(113) {
            let auto = RampAutomation {
                ids: vec![parameter::CUTOFF],
                start: 150.0,
                end: 12_000.0,
                length: input.len(),
                offset_base: offset,
            };
            rt_part
                .process_block_with_automation(chunk, None, &auto, &EMPTY_DEVICE_RESOURCES)
                .unwrap();
            offset += chunk.len();
        }

        assert_eq!(whole, partitioned);
        let early_power: f32 = whole[256..1024].iter().map(|f| f[0] * f[0]).sum();
        let late_power: f32 = whole[7168..7936].iter().map(|f| f[0] * f[0]).sum();
        assert!(
            late_power > early_power * 20.0,
            "automated cutoff opening must raise high-frequency power ({early_power} -> {late_power})"
        );

        let mut bypassed = input.clone();
        let mut rt_bypass = prepared.create_runtime();
        rt_bypass
            .process_bypassed_block_with_automation(
                &mut bypassed,
                None,
                &auto_whole,
                &EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
        assert_eq!(bypassed, input);
    }

    #[test]
    fn finite_output_tail_drains_deterministically_even_at_extreme_resonance() {
        let state = DEFINITION.default_state().unwrap();
        let extreme = patch(&state, CUTOFF, ParameterValue::Float(20.0));
        let extreme = patch(&extreme, RESONANCE, ParameterValue::Float(100.0));
        let extreme = patch(&extreme, SLOPE, ParameterValue::Choice(1));
        let extreme = patch(&extreme, LFO_AMOUNT, ParameterValue::Float(1.0));
        let extreme = patch(&extreme, ENV_AMOUNT, ParameterValue::Float(2.0));

        let prepared = FilterDefinition::prepare_owned(&extreme, 48_000, &[]).unwrap();
        let OutputTail::Finite { max_tail_frames } = prepared.output_tail() else {
            panic!("hex.filter must declare a finite OutputTail");
        };
        assert!(max_tail_frames > 0 && max_tail_frames <= 48_000 * 6);

        let mut runtime = prepared.create_runtime();
        assert!(runtime.is_quiescent(), "fresh runtime starts quiescent");

        let mut impulse = [[0.0_f32; 2]; 256];
        impulse[0] = [1.0, -1.0];
        runtime.process_block(&mut impulse, &EMPTY_DEVICE_RESOURCES);
        assert!(
            !runtime.is_quiescent(),
            "resonant impulse must excite filter history"
        );

        let mut silence = vec![[0.0_f32; 2]; max_tail_frames as usize];
        for chunk in silence.chunks_mut(256) {
            runtime.process_block(chunk, &EMPTY_DEVICE_RESOURCES);
        }
        assert!(
            runtime.is_quiescent(),
            "filter must become quiescent within max_tail_frames"
        );
        assert!(silence.iter().flatten().all(|s| s.is_finite()));

        let mut after = [[0.0_f32; 2]; 128];
        runtime.process_block(&mut after, &EMPTY_DEVICE_RESOURCES);
        assert_eq!(after, [[0.0_f32; 2]; 128]);
    }

    #[test]
    fn sample_rate_cutoff_modulation_tracks_without_five_ms_lag() {
        struct StepModulation {
            id: [ParameterId; 1],
            sample_rate_modulated: bool,
        }
        impl NativeParameterAutomation for StepModulation {
            fn parameter_ids(&self) -> &[ParameterId] {
                &self.id
            }
            fn value_at(&self, id: ParameterId, frame_offset: usize) -> Option<ParameterValue> {
                if id != self.id[0] {
                    return None;
                }
                // Step immediately from 100 Hz to 16 kHz at sample 8.
                Some(ParameterValue::Float(if frame_offset < 8 {
                    100.0
                } else {
                    16_000.0
                }))
            }
            fn has_sample_rate_modulation(&self, parameter: ParameterId) -> bool {
                self.sample_rate_modulated && parameter == self.id[0]
            }
        }

        let state = patch(
            &DEFINITION.default_state().unwrap(),
            CUTOFF,
            ParameterValue::Float(100.0),
        );
        let prepared =
            FilterDefinition::prepare_owned(&state, 48_000, &[parameter::CUTOFF]).unwrap();

        // 8 kHz tone: strongly attenuated at 100 Hz, nearly transparent at 16 kHz.
        let input: Vec<[f32; 2]> = (0..64)
            .map(|i| {
                let s = (std::f64::consts::TAU * 8_000.0 * i as f64 / 48_000.0).sin() as f32;
                [s, s]
            })
            .collect();

        let mut modulated = input.clone();
        let mut rt_mod = prepared.create_runtime();
        rt_mod
            .process_block_with_automation(
                &mut modulated,
                None,
                &StepModulation {
                    id: [parameter::CUTOFF],
                    sample_rate_modulated: true,
                },
                &EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();

        let mut automated = input;
        let mut rt_auto = prepared.create_runtime();
        rt_auto
            .process_block_with_automation(
                &mut automated,
                None,
                &StepModulation {
                    id: [parameter::CUTOFF],
                    sample_rate_modulated: false,
                },
                &EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();

        // Within the first 32 samples after the step (~0.67 ms, well inside the 5 ms ramp),
        // sample-rate modulation must track immediately and pass far more 8 kHz energy than
        // stepped automation's 5 ms anti-zipper ramp.
        let mod_energy: f32 = modulated[16..48].iter().map(|f| f[0] * f[0]).sum();
        let auto_energy: f32 = automated[16..48].iter().map(|f| f[0] * f[0]).sum();
        assert!(
            mod_energy > auto_energy * 4.0 && mod_energy > 5.0,
            "sample-rate cutoff modulation must bypass the 5 ms ramp (mod={mod_energy}, auto={auto_energy})"
        );
    }

    #[test]
    fn env_attack_and_release_modulation_matches_across_partitions() {
        let base = patch(
            &DEFINITION.default_state().unwrap(),
            ENV_AMOUNT,
            ParameterValue::Float(3.0),
        );
        let prepared = FilterDefinition::prepare_owned(
            &base,
            48_000,
            &[parameter::ENV_RELEASE, parameter::ENV_ATTACK],
        )
        .unwrap();

        let mut input = signal(2_000.0);
        input.truncate(1024);
        for frame in &mut input[512..] {
            *frame = [0.0, 0.0];
        }

        let mut whole = input.clone();
        let mut rt_whole = prepared.create_runtime();
        let auto_whole = RampAutomation {
            ids: vec![parameter::ENV_RELEASE, parameter::ENV_ATTACK],
            start: 5.0,
            end: 150.0,
            length: input.len(),
            offset_base: 0,
        };
        rt_whole
            .process_block_with_automation(&mut whole, None, &auto_whole, &EMPTY_DEVICE_RESOURCES)
            .unwrap();

        let mut partitioned = input.clone();
        let mut rt_part = prepared.create_runtime();
        let mut offset = 0;
        for chunk in partitioned.chunks_mut(73) {
            let auto = RampAutomation {
                ids: vec![parameter::ENV_RELEASE, parameter::ENV_ATTACK],
                start: 5.0,
                end: 150.0,
                length: input.len(),
                offset_base: offset,
            };
            rt_part
                .process_block_with_automation(chunk, None, &auto, &EMPTY_DEVICE_RESOURCES)
                .unwrap();
            offset += chunk.len();
        }
        assert_eq!(whole, partitioned);
    }

    #[test]
    fn historical_v1_state_migrates_losslessly_and_renders_identically() {
        // Construct an exact historical 13-word (52-byte) v1 payload (low-pass, 12 dB, 1400 Hz, 35% res).
        let v1_words: [u32; V1_COUNT] = [
            0,                   // TYPE: low-pass
            0,                   // SLOPE: 12 db
            1400.0f32.to_bits(), // CUTOFF
            35.0f32.to_bits(),   // RESONANCE
            50.0f32.to_bits(),   // MORPH
            100.0f32.to_bits(),  // MIX
            0.0f32.to_bits(),    // LFO_AMOUNT
            0.5f32.to_bits(),    // LFO_RATE
            0,                   // LFO_SHAPE
            0.0f32.to_bits(),    // LFO_PHASE
            0.0f32.to_bits(),    // ENV_AMOUNT
            120.0f32.to_bits(),  // ENV_RELEASE
            5.0f32.to_bits(),    // ENV_ATTACK
        ];
        let v1_state = NativeDeviceState::new(
            TYPE_ID,
            V1_VERSION,
            v1_words.into_iter().flat_map(u32::to_le_bytes).collect(),
        )
        .unwrap();
        let migrated = DEFINITION.migrate_state(&v1_state).unwrap();
        assert_eq!(migrated.state_version(), VERSION);
        assert_eq!(migrated.payload().len(), COUNT * WORD_BYTES);
        assert_eq!(
            DEFINITION.read_parameter(&migrated, parameter::CUTOFF),
            Ok(ParameterValue::Float(1400.0))
        );
        assert_eq!(
            DEFINITION.read_parameter(&migrated, parameter::CIRCUIT),
            Ok(ParameterValue::Choice(0))
        );
        assert_eq!(
            DEFINITION.read_parameter(&migrated, parameter::DRIVE),
            Ok(ParameterValue::Float(0.0))
        );

        let mut direct_v1 = signal(2500.0);
        let mut migrated_v2 = direct_v1.clone();
        render(&v1_state, &mut direct_v1, 128);
        render(&migrated, &mut migrated_v2, 128);
        assert_eq!(direct_v1, migrated_v2);

        // Historical v1 payload with out-of-range v2-only mode must be rejected.
        let mut bad_v1 = v1_words;
        bad_v1[TYPE] = 7;
        let bad_v1_state = NativeDeviceState::new(
            TYPE_ID,
            V1_VERSION,
            bad_v1.into_iter().flat_map(u32::to_le_bytes).collect(),
        )
        .unwrap();
        assert_eq!(
            DEFINITION.migrate_state(&bad_v1_state),
            Err(DeviceContractError::MalformedState)
        );
    }

    #[test]
    fn extended_auto_filter_modes_circuits_and_modulated_response_behave_deterministically() {
        let base = DEFINITION.default_state().unwrap();
        // Verify all 5 new filter modes (dj, notch+lp, comb, resampling, vowel) render finite,
        // distinct, block-partition-invariant audio and valid response curves.
        for mode_idx in 5..=9u32 {
            let mut st = patch(&base, TYPE, ParameterValue::Choice(mode_idx));
            st = patch(&st, CUTOFF, ParameterValue::Float(1200.0));
            st = patch(&st, RESONANCE, ParameterValue::Float(55.0));
            st = patch(&st, MORPH, ParameterValue::Float(25.0));

            let input = signal(1500.0);
            let mut whole = input.clone();
            let mut split = input.clone();
            render(&st, &mut whole, 8192);
            render(&st, &mut split, 97);
            assert_eq!(
                whole, split,
                "mode {mode_idx} must be block-partition invariant"
            );
            assert!(whole.iter().flatten().all(|s| s.is_finite()));

            let resp = response(&st, 48_000).unwrap();
            let mod_resp = modulated_response(&st, 48_000, 1.5).unwrap();
            assert!(resp.iter().flatten().all(|v| v.is_finite()));
            assert!(mod_resp.iter().flatten().all(|v| v.is_finite()));
            assert_ne!(
                resp, mod_resp,
                "mode {mode_idx} modulated response must track offset"
            );
        }

        // Circuit variants + drive produce distinct harmonic coloration vs clean.
        let clean = patch(&base, DRIVE, ParameterValue::Float(0.0));
        let mut clean_out = signal(400.0);
        render(&clean, &mut clean_out, 128);
        for circuit in 1..=4u32 {
            let mut driven = patch(&base, CIRCUIT, ParameterValue::Choice(circuit));
            driven = patch(&driven, DRIVE, ParameterValue::Float(18.0));
            let mut out = signal(400.0);
            render(&driven, &mut out, 128);
            assert!(out.iter().flatten().all(|s| s.is_finite()));
            assert_ne!(out, clean_out, "circuit {circuit} + drive must color audio");
        }
    }

    #[test]
    fn beat_synced_lfo_stereo_spin_sample_and_hold_and_keyed_envelope_hold_work() {
        struct FixedTempoClock(u32);
        impl hex_lattice::NativeInstrumentClock for FixedTempoClock {
            fn beat_at_offset(&self, _offset: usize) -> Option<u64> {
                None
            }
            fn tempo_micro_bpm_at_offset(&self, _offset: usize) -> Option<u32> {
                Some(self.0)
            }
        }

        let base = DEFINITION.default_state().unwrap();
        // 1. Beat-synced LFO responds to host clock tempo changes (60 BPM vs 180 BPM).
        let mut synced = patch(&base, LFO_AMOUNT, ParameterValue::Float(2.5));
        synced = patch(&synced, LFO_SYNC, ParameterValue::Choice(1));
        synced = patch(&synced, LFO_SYNC_RATE, ParameterValue::Choice(2)); // 1/4 note
        let prep_sync = FilterDefinition::prepare_owned(&synced, 48_000, &[]).unwrap();
        let mut slow_out = signal(1800.0);
        let mut fast_out = slow_out.clone();
        let mut rt_slow = prep_sync.create_runtime();
        let mut rt_fast = prep_sync.create_runtime();
        let slow_clk = FixedTempoClock(60_000_000);
        let fast_clk = FixedTempoClock(180_000_000);
        rt_slow.process_block_with_clock(&mut slow_out, Some(&slow_clk), &EMPTY_DEVICE_RESOURCES);
        rt_fast.process_block_with_clock(&mut fast_out, Some(&fast_clk), &EMPTY_DEVICE_RESOURCES);
        assert_ne!(slow_out, fast_out, "beat-synced LFO must follow host tempo");

        // 2. Stereo spin produces L/R divergence even when LFO_PHASE == 0.
        let mut spin = patch(&base, LFO_AMOUNT, ParameterValue::Float(2.0));
        spin = patch(&spin, LFO_RATE, ParameterValue::Float(6.0));
        spin = patch(&spin, LFO_STEREO_MODE, ParameterValue::Choice(1));
        spin = patch(&spin, LFO_SPIN, ParameterValue::Float(35.0));
        let mut spin_out = signal(1800.0);
        render(&spin, &mut spin_out, 128);
        assert!(
            spin_out.iter().any(|f| (f[0] - f[1]).abs() > 0.05),
            "stereo spin must diverge L/R LFO modulation"
        );

        // 3. Sample-and-hold (shape 5) is deterministic across arbitrary block partitions.
        let mut sh = patch(&base, LFO_AMOUNT, ParameterValue::Float(2.0));
        sh = patch(&sh, LFO_RATE, ParameterValue::Float(12.0));
        sh = patch(&sh, LFO_SHAPE, ParameterValue::Choice(5));
        let mut sh_whole = signal(1800.0);
        let mut sh_part = sh_whole.clone();
        render(&sh, &mut sh_whole, 8192);
        render(&sh, &mut sh_part, 79);
        assert_eq!(sh_whole, sh_part);

        // 4. Envelope key routing (left vs right) and peak-hold window.
        let mut key_left = patch(&base, CUTOFF, ParameterValue::Float(200.0));
        key_left = patch(&key_left, ENV_AMOUNT, ParameterValue::Float(3.5));
        key_left = patch(&key_left, ENV_HOLD, ParameterValue::Float(60.0));
        key_left = patch(&key_left, ENV_KEY_ROUTING, ParameterValue::Choice(1)); // left only
        let key_right = patch(&key_left, ENV_KEY_ROUTING, ParameterValue::Choice(2)); // right only

        // Feed loud tone on Left only, quiet high-freq probe on Right!
        let stereo_probe: Vec<[f32; 2]> = (0..4096)
            .map(|i| {
                let s = (std::f64::consts::TAU * 2500.0 * i as f64 / 48_000.0).sin() as f32;
                if i < 512 {
                    [s, s * 0.02]
                } else {
                    [0.0, s * 0.02]
                }
            })
            .collect();
        let mut out_lkey = stereo_probe.clone();
        let mut out_rkey = stereo_probe;
        render(&key_left, &mut out_lkey, 128);
        render(&key_right, &mut out_rkey, 128);
        let probe_lkey: f32 = out_lkey[600..2400].iter().map(|f| f[1] * f[1]).sum();
        let probe_rkey: f32 = out_rkey[600..2400].iter().map(|f| f[1] * f[1]).sum();
        assert!(
            probe_lkey > probe_rkey * 5.0,
            "left-keyed envelope with 60ms hold must keep right probe open longer ({probe_lkey} vs {probe_rkey})"
        );
    }
}
