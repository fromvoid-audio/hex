//! Original zero-latency eight-band parametric equalizer.
//!
//! Canonical coefficients are prepared on control from the RBJ audio-EQ
//! cookbook equations. The callback retains fixed coefficient banks and
//! transposed-direct-form-II histories. Shared gain, frequency, Q, enable and
//! filter-type automation designs active-bank targets in fixed inline storage
//! and reuses the rate-scaled coefficient/gain smoother. Routing automation
//! uses one outgoing fade and one pending latest target (see automation::routing).
//! Compatible copies retain admitted trajectories; missing views restore canonical
//! controls, and detached tails freeze their targets.
mod automation;
use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorAudition, OutputTail, OwnedProcessorRuntime,
    PreparedOwnedProcessorRuntime, PreparedProcessorFactory, ProcessorAuditionSelection,
    StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any as StdAny;
use std::sync::Arc;

#[path = "eq/mod.rs"]
pub mod flexible;

pub const EQ8_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.eq8");
pub const BAND_COUNT: usize = 8;
pub const EQ8_RUNTIME_BYTES: usize = std::mem::size_of::<Eq8Runtime>();
pub const EQ8_AUDITION_CROSSFADE_FRAMES: u16 = 256;

/// Sample-rate-scaled audition crossfade length (~5.33 ms, 256 frames at 48 kHz).
pub const fn eq8_audition_crossfade_frames(sample_rate: u32) -> u32 {
    let frames = (sample_rate as u64 * EQ8_AUDITION_CROSSFADE_FRAMES as u64).div_ceil(48_000);
    if frames == 0 { 1 } else { frames as u32 }
}
const STATE_VERSION: u32 = 1;
const STATE_BYTES: usize = 1 + 4 + 5 * BAND_COUNT * 14;
const MIN_HZ: f32 = 20.0;
const MAX_HZ: f32 = 20_000.0;
const MAX_GAIN_DB: f32 = 24.0;
const MIN_Q: f32 = 0.1;
const MAX_Q: f32 = 18.0;
const MAX_SAMPLE_RATE: u32 = 192_000;
const MIN_SAMPLE_RATE: u32 = 8_000;

/// A transient EQ band audition target. `edit_lane` selects left/right or
/// mid/side in those modes; stereo mode auditions the band on both channels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Eq8AuditionSelection {
    band: u8,
    edit_lane: u8,
}

impl Eq8AuditionSelection {
    pub fn new(band: usize, edit_lane: usize) -> Result<Self, DeviceContractError> {
        if band >= BAND_COUNT || edit_lane >= 2 {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Self {
            band: band as u8,
            edit_lane: edit_lane as u8,
        })
    }

    pub const fn band(self) -> usize {
        self.band as usize
    }

    pub const fn edit_lane(self) -> usize {
        self.edit_lane as usize
    }
}

pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const fn band(bank: usize, band: usize, field: u32) -> ParameterId {
        ParameterId::from_static((bank as u32) * 40 + (band as u32) * 5 + field + 1)
    }
    pub const fn enabled(bank: usize, band_index: usize) -> ParameterId {
        band(bank, band_index, 0)
    }
    pub const fn kind(bank: usize, band_index: usize) -> ParameterId {
        band(bank, band_index, 1)
    }
    pub const fn frequency(bank: usize, band_index: usize) -> ParameterId {
        band(bank, band_index, 2)
    }
    pub const fn gain(bank: usize, band_index: usize) -> ParameterId {
        band(bank, band_index, 3)
    }
    pub const fn q(bank: usize, band_index: usize) -> ParameterId {
        band(bank, band_index, 4)
    }
    pub const MODE: ParameterId = ParameterId::from_static(201);
    pub const OUTPUT_GAIN: ParameterId = ParameterId::from_static(202);
}

const PARAMETER_LABELS: [&str; 5] = ["enabled", "type", "frequency", "gain", "q"];
const BAND_TYPES: &[&str] = &[
    "bell",
    "low shelf",
    "high shelf",
    "low cut 12",
    "low cut 48",
    "high cut 12",
    "high cut 48",
    "notch",
];
const CHANNEL_MODES: &[&str] = &["stereo", "left/right", "mid/side"];
const DEFAULT_FREQUENCIES: [f32; BAND_COUNT] = [
    40.0, 100.0, 250.0, 500.0, 1_000.0, 2_500.0, 6_000.0, 12_000.0,
];

const fn make_descriptors() -> [ParameterDescriptor; 202] {
    let mut descriptors = [ParameterDescriptor {
        id: ParameterId::from_static(1),
        label: "a enabled",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    }; 202];
    let mut bank = 0;
    while bank < 5 {
        let mut band = 0;
        while band < BAND_COUNT {
            let base = bank * 40 + band * 5;
            descriptors[base] = ParameterDescriptor {
                id: parameter::enabled(bank, band),
                label: PARAMETER_LABELS[0],
                kind: ParameterKind::Bool,
                default: ParameterValue::Bool(false),
                presentation: ParameterPresentation::Linear,
            };
            descriptors[base + 1] = ParameterDescriptor {
                id: parameter::kind(bank, band),
                label: PARAMETER_LABELS[1],
                kind: ParameterKind::Choice { labels: BAND_TYPES },
                default: ParameterValue::Choice(0),
                presentation: ParameterPresentation::Linear,
            };
            descriptors[base + 2] = ParameterDescriptor {
                id: parameter::frequency(bank, band),
                label: PARAMETER_LABELS[2],
                kind: ParameterKind::Float {
                    min: MIN_HZ,
                    max: MAX_HZ,
                    unit: Some("hz"),
                },
                default: ParameterValue::Float(DEFAULT_FREQUENCIES[band]),
                presentation: ParameterPresentation::Logarithmic,
            };
            descriptors[base + 3] = ParameterDescriptor {
                id: parameter::gain(bank, band),
                label: PARAMETER_LABELS[3],
                kind: ParameterKind::Float {
                    min: -MAX_GAIN_DB,
                    max: MAX_GAIN_DB,
                    unit: Some("db"),
                },
                default: ParameterValue::Float(0.0),
                presentation: ParameterPresentation::Linear,
            };
            descriptors[base + 4] = ParameterDescriptor {
                id: parameter::q(bank, band),
                label: PARAMETER_LABELS[4],
                kind: ParameterKind::Float {
                    min: MIN_Q,
                    max: MAX_Q,
                    unit: None,
                },
                default: ParameterValue::Float(std::f32::consts::FRAC_1_SQRT_2),
                presentation: ParameterPresentation::Linear,
            };
            band += 1;
        }
        bank += 1;
    }
    descriptors[200] = ParameterDescriptor {
        id: parameter::MODE,
        label: "channel mode",
        kind: ParameterKind::Choice {
            labels: CHANNEL_MODES,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    };
    descriptors[201] = ParameterDescriptor {
        id: parameter::OUTPUT_GAIN,
        label: "output gain",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 16.0,
            unit: Some("linear"),
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::LinearAmplitudeDecibels,
    };
    descriptors
}

static DESCRIPTORS: [ParameterDescriptor; 202] = make_descriptors();

mod state;
use state::{Band, ChannelMode, Settings, decode, encode};

pub struct Eq8Definition;

impl NativeDeviceMetadata for Eq8Definition {
    fn type_id(&self) -> DeviceTypeId {
        EQ8_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("eq")
    }
    fn state_version(&self) -> u32 {
        STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
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
        let settings = decode(state)?;
        if id == parameter::MODE {
            return Ok(ParameterValue::Choice(match settings.mode {
                ChannelMode::Stereo => 0,
                ChannelMode::LeftRight => 1,
                ChannelMode::MidSide => 2,
            }));
        }
        if id == parameter::OUTPUT_GAIN {
            return Ok(ParameterValue::Float(settings.output_gain));
        }
        let (bank, field) = decode_parameter_id(id)?;
        let band = settings.banks[bank][field / 5];
        Ok(match field % 5 {
            0 => ParameterValue::Bool(band.enabled),
            1 => ParameterValue::Choice(u32::from(band.kind)),
            2 => ParameterValue::Float(band.frequency_hz),
            3 => ParameterValue::Float(band.gain_db),
            4 => ParameterValue::Float(band.q),
            _ => return Err(DeviceContractError::UnknownParameter),
        })
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        decode(state)?;
        Ok(automation::semantics(id))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut settings = decode(state)?;
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(value)?;
        if id == parameter::MODE {
            settings.mode = match value {
                ParameterValue::Choice(0) => ChannelMode::Stereo,
                ParameterValue::Choice(1) => ChannelMode::LeftRight,
                ParameterValue::Choice(2) => ChannelMode::MidSide,
                _ => return Err(DeviceContractError::InvalidParameterValue),
            };
            return encode(settings);
        }
        if id == parameter::OUTPUT_GAIN {
            let ParameterValue::Float(gain) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            settings.output_gain = gain;
            return encode(settings);
        }
        let (bank, field) = decode_parameter_id(id)?;
        let band = &mut settings.banks[bank][field / 5];
        match (field % 5, value) {
            (0, ParameterValue::Bool(v)) => band.enabled = v,
            (1, ParameterValue::Choice(v)) => band.kind = v as u8,
            (2, ParameterValue::Float(v)) => band.frequency_hz = v,
            (3, ParameterValue::Float(v)) => band.gain_db = v,
            (4, ParameterValue::Float(v)) => band.q = v,
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
        encode(settings)
    }

    fn parameter_enabled(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        let _ = decode(state)?;
        let descriptor = DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        if id.value() <= 200 {
            let (bank, field) = decode_parameter_id(id)?;
            let kind = match self.read_parameter(state, parameter::kind(bank, field / 5))? {
                ParameterValue::Choice(kind) => kind,
                _ => return Err(DeviceContractError::MalformedState),
            };
            if field % 5 == 3 && matches!(kind, 3..=7) {
                return Ok(false);
            }
        }
        let value = self.read_parameter(state, id)?;
        descriptor.validate(value)?;
        Ok(true)
    }
}

fn decode_parameter_id(id: ParameterId) -> Result<(usize, usize), DeviceContractError> {
    let value = id.value();
    if !(1..=200).contains(&value) {
        return Err(DeviceContractError::UnknownParameter);
    }
    let zero = value - 1;
    Ok(((zero / 40) as usize, (zero % 40) as usize))
}

impl Eq8Definition {
    /// Asset-free fixed-inline factory used by the sealed native constructor.
    pub fn prepare_owned(
        state: &NativeDeviceState,
        sample_rate: u32,
        parameters: &[ParameterId],
    ) -> Result<PreparedEq8, DeviceContractError> {
        if parameters.is_empty() {
            state.require_asset_free_processing()?;
            PreparedEq8::new(decode(state)?, sample_rate)
        } else {
            automation::prepare(state, sample_rate, parameters)
        }
    }
}

impl NativeDeviceDefinition for Eq8Definition {
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        Some(crate::source_effect_arc_bytes::<PreparedEq8>())
    }
    fn supports_parameter_automation(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<bool, DeviceContractError> {
        decode(state)?;
        Ok(automation::slot(id).is_some())
    }
    fn prepare_automated_runtime_instance(
        &self,
        _identity: hex_lattice::ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        ids: &[ParameterId],
    ) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
        Ok(hex_lattice::PreparedRuntimeDevice {
            matrix: StereoMatrix::identity(),
            processor: Some(PreparedProcessorFactory::Owned(Arc::new(
                automation::prepare(state, context.sample_rate, ids)?,
            ))),
        })
    }
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
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
        _context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedNativeProcessor>>, DeviceContractError> {
        let _ = decode(state)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }

    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(PreparedProcessorFactory::Owned(Arc::new(
            Self::prepare_owned(state, context.sample_rate, &[])?,
        ))))
    }
}

mod runtime;
#[cfg(test)]
use runtime::mode_fade_frames;
use runtime::{
    BandStages, CoefficientBank, CoefficientTarget, Coefficients, ModeFade, channels,
    coefficient_smooth_frames, design_band, finite_f32, quiet, response_magnitude,
};
pub use runtime::{Eq8Runtime, PreparedEq8};

pub static EQ8_DEFINITION: Eq8Definition = Eq8Definition;

#[cfg(test)]
#[path = "eq8/mode_fade_tests.rs"]
mod mode_fade_tests;
#[cfg(test)]
#[path = "eq8/steady_state_digest_tests.rs"]
mod steady_state_digest_tests;
#[cfg(test)]
#[path = "eq8/tests.rs"]
mod tests;
