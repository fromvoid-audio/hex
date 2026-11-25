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

type Coefficients = hex_dsp::BiquadCoefficients;

type BandStages = [Coefficients; 4];
type CoefficientBank = [BandStages; BAND_COUNT];
type CoefficientTarget = [CoefficientBank; 2];

fn channels(target: &CoefficientTarget) -> [&CoefficientBank; 2] {
    let [left, right] = target;
    [left, right]
}

/// Prepared coefficients and admitted bindings for a closed EQ8 instance.
#[derive(Clone)]
pub struct PreparedEq8 {
    admitted: [bool; automation::COUNT],
    control_values: automation::ControlValues,
    canonical_banks: [CoefficientBank; 5],
    sample_rate: u32,
    mode: ChannelMode,
    output_gain: f64,
    enabled_bands: [[bool; BAND_COUNT]; 2],
}

impl PreparedEq8 {
    fn new(settings: Settings, sample_rate: u32) -> Result<Self, DeviceContractError> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut banks = [[[Coefficients::IDENTITY; 4]; BAND_COUNT]; 5];
        for (bank, bank_stages) in banks.iter_mut().enumerate() {
            for (band_index, stages) in bank_stages.iter_mut().enumerate() {
                let band = settings.banks[bank][band_index];
                if band.enabled {
                    *stages = design_band(band, sample_rate)?;
                }
            }
        }
        let selected = automation::selected(settings.mode);
        let mut enabled_bands = [[false; BAND_COUNT]; 2];
        for (channel, bank_index) in selected.into_iter().enumerate() {
            for (band, enabled) in enabled_bands[channel].iter_mut().enumerate() {
                *enabled = settings.banks[bank_index][band].enabled;
            }
        }
        Ok(Self {
            admitted: [false; automation::COUNT],
            control_values: automation::ControlValues::new(settings),
            canonical_banks: banks,
            sample_rate,
            mode: settings.mode,
            output_gain: f64::from(settings.output_gain),
            enabled_bands,
        })
    }

    fn runtime(&self) -> Eq8Runtime {
        let coefficients = automation::selected(self.mode).map(|bank| self.canonical_banks[bank]);
        let controls = crate::parameter_binding::Controls::new(self.control_values, self.admitted);
        let active_bands = active_band_flags(&coefficients);
        let band_automation_admitted = self.admitted[..automation::GAIN_SLOT].iter().any(|&v| v);
        Eq8Runtime {
            controls,
            tail_controls_frozen: false,
            routing: automation::routing::RoutingAutomation::new(self.canonical_banks),
            automated_target: None,
            band_automation_admitted,
            band_targets_pending: band_automation_admitted,
            sample_rate: self.sample_rate,
            mode: self.mode,
            canonical_mode: self.mode,
            current_gain: self.output_gain,
            target_gain: self.output_gain,
            coefficients,
            smooth_remaining: 0,
            active_bands,
            history: [[[[0.0; 2]; 4]; BAND_COUNT]; 2],
            enabled_bands: self.enabled_bands,
            audition_selection: None,
            audition_request: None,
            audition_mix: 0.0,
            audition_history: [[[0.0; 2]; 4]; 2],
            mode_fade: ModeFade::INACTIVE,
            rollback: Box::new(Eq8Rollback::new(
                controls,
                self.mode,
                self.output_gain,
                coefficients,
                active_bands,
                self.enabled_bands,
                band_automation_admitted,
            )),
        }
    }
}

impl PreparedOwnedProcessorRuntime for PreparedEq8 {
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        automation::slot(id).is_some_and(|i| self.admitted[i])
    }
    fn parameter_automation_descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        self.supports_parameter_automation(id)
            .then(|| automation::descriptor(id))
            .flatten()
    }
    fn parameter_modulation_semantics(
        &self,
        id: ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        automation::semantics(id).filter(|semantics| {
            self.supports_parameter_automation(id)
                && semantics.modulation == hex_lattice_contract::ParameterModulation::SampleRate
        })
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
        std::mem::size_of::<Eq8Runtime>() + std::mem::size_of::<Eq8Rollback>()
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        Ok(Box::new(self.runtime()))
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn frequency_response_at_hz(&self, frequency_hz: f32) -> Option<[f32; 2]> {
        if !frequency_hz.is_finite()
            || frequency_hz <= 0.0
            || frequency_hz >= self.sample_rate as f32 * 0.5
        {
            return None;
        }
        let selected = automation::selected(self.mode);
        let response = std::array::from_fn(|channel| {
            let mut magnitude = self.output_gain;
            for band in 0..BAND_COUNT {
                for stage in 0..4 {
                    magnitude *= response_magnitude(
                        self.canonical_banks[selected[channel]][band][stage],
                        f64::from(frequency_hz),
                        f64::from(self.sample_rate),
                    );
                }
            }
            finite_f32(magnitude)
        });
        Some(response)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Unbounded
    }
}

const DIRTY_SCALARS: u8 = 1 << 0;
const DIRTY_COEFFS: u8 = 1 << 1;
const DIRTY_AUDITION: u8 = 1 << 2;
const DIRTY_MODE_FADE: u8 = 1 << 3;
const DIRTY_CONTROLS: u8 = 1 << 4;
const DIRTY_AUTOMATED_TARGET: u8 = 1 << 5;
const DIRTY_PENDING_ROUTING: u8 = 1 << 6;

/// Preallocated bounded dirty-range rollback journal for `Eq8Runtime`.
/// Immutable canonical coefficient banks live once on `Eq8Runtime` and are never
/// duplicated or copied across speculative block boundaries.
#[derive(Clone, Debug)]
struct Eq8Rollback {
    open: bool,
    committed_quiescent: bool,
    dirty_flags: u8,
    dirty_history: u16,
    mode: ChannelMode,
    current_gain: f64,
    target_gain: f64,
    smooth_remaining: u16,
    active_bands: [[bool; BAND_COUNT]; 2],
    enabled_bands: [[bool; BAND_COUNT]; 2],
    band_targets_pending: bool,
    tail_controls_frozen: bool,
    history: [[[[f64; 2]; 4]; BAND_COUNT]; 2],
    coefficients: [[[Coefficients; 4]; BAND_COUNT]; 2],
    audition_selection: Option<Eq8AuditionSelection>,
    audition_request: Option<Eq8AuditionSelection>,
    audition_mix: f64,
    audition_history: [[[f64; 2]; 4]; 2],
    mode_fade: ModeFade,
    controls: crate::parameter_binding::Controls<automation::ControlValues, { automation::COUNT }>,
    automated_target: Option<CoefficientTarget>,
    pending_routing: Option<automation::routing::PendingRouting>,
}

impl Eq8Rollback {
    fn new(
        controls: crate::parameter_binding::Controls<
            automation::ControlValues,
            { automation::COUNT },
        >,
        mode: ChannelMode,
        gain: f64,
        coefficients: [[[Coefficients; 4]; BAND_COUNT]; 2],
        active_bands: [[bool; BAND_COUNT]; 2],
        enabled_bands: [[bool; BAND_COUNT]; 2],
        band_targets_pending: bool,
    ) -> Self {
        Self {
            open: false,
            committed_quiescent: true,
            dirty_flags: 0,
            dirty_history: 0,
            mode,
            current_gain: gain,
            target_gain: gain,
            smooth_remaining: 0,
            active_bands,
            enabled_bands,
            band_targets_pending,
            tail_controls_frozen: false,
            history: [[[[0.0; 2]; 4]; BAND_COUNT]; 2],
            coefficients,
            audition_selection: None,
            audition_request: None,
            audition_mix: 0.0,
            audition_history: [[[0.0; 2]; 4]; 2],
            mode_fade: ModeFade::INACTIVE,
            controls,
            automated_target: None,
            pending_routing: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Eq8Runtime {
    controls: crate::parameter_binding::Controls<automation::ControlValues, { automation::COUNT }>,
    tail_controls_frozen: bool,
    // One fixed canonical bank inventory serves both normal playback and
    // mode changes; never duplicated in rollback storage.
    routing: automation::routing::RoutingAutomation,
    automated_target: Option<CoefficientTarget>,
    band_automation_admitted: bool,
    band_targets_pending: bool,
    sample_rate: u32,
    mode: ChannelMode,
    canonical_mode: ChannelMode,
    current_gain: f64,
    target_gain: f64,
    coefficients: [[[Coefficients; 4]; BAND_COUNT]; 2],
    smooth_remaining: u16,
    active_bands: [[bool; BAND_COUNT]; 2],
    history: [[[[f64; 2]; 4]; BAND_COUNT]; 2],
    enabled_bands: [[bool; BAND_COUNT]; 2],
    audition_selection: Option<Eq8AuditionSelection>,
    audition_request: Option<Eq8AuditionSelection>,
    audition_mix: f64,
    audition_history: [[[f64; 2]; 4]; 2],
    mode_fade: ModeFade,
    rollback: Box<Eq8Rollback>,
}

/// Coefficient and output-gain smoothing lasts 256 frames at the 48 kHz
/// reference rate (16/3 ms). Other rates scale the frame count so the
/// smoothing time is the same everywhere (43 frames at 8 kHz, 1024 at 192 kHz).
const COEFFICIENT_SMOOTH_REFERENCE_FRAMES: u32 = 256;
const COEFFICIENT_SMOOTH_REFERENCE_RATE: u32 = 48_000;

/// Frames for one coefficient handoff at `sample_rate`; at least one frame.
fn coefficient_smooth_frames(sample_rate: u32) -> u16 {
    let frames = u64::from(sample_rate) * u64::from(COEFFICIENT_SMOOTH_REFERENCE_FRAMES);
    let frames = frames.div_ceil(u64::from(COEFFICIENT_SMOOTH_REFERENCE_RATE));
    u16::try_from(frames).unwrap_or(u16::MAX).max(1)
}

/// Frames for a channel-mode crossfade: 256 at 48 kHz, scaled by rate.
fn mode_fade_frames(sample_rate: u32) -> u32 {
    let frames = (u64::from(sample_rate) * 256).div_ceil(48_000).max(1);
    u32::try_from(frames).unwrap_or(u32::MAX)
}

/// The outgoing routing after a stereo/left-right/mid-side switch (Pulse
/// #4106). Lane histories cannot be reinterpreted under a new routing, so the
/// previous routing keeps filtering with its frozen effective coefficients,
/// history, gain and band audition (at its frozen audition mix) while the
/// output crossfades linearly into the new routing, which starts from empty
/// history. The crossfade ends exactly on the new routing, then the outgoing
/// history is cleared.
///
/// Capacity is one outgoing branch, in fixed inline storage with no
/// allocation. Same-mode handoffs carry an active branch unchanged. A further
/// ordinary canonical mode change while the branch is still fading is refused as transiently busy
/// (`transition_busy_from`), so the host defers it for at most one fade
/// (256 frames at 48 kHz, 16/3 ms) instead of dropping the older routing.
#[derive(Clone, Copy, Debug)]
struct ModeFade {
    mode: ChannelMode,
    gain: f64,
    coefficients: [[[Coefficients; 4]; BAND_COUNT]; 2],
    history: [[[[f64; 2]; 4]; BAND_COUNT]; 2],
    audition_selection: Option<Eq8AuditionSelection>,
    audition_mix: f64,
    audition_history: [[[f64; 2]; 4]; 2],
    remaining: u32,
    total: u32,
}

impl ModeFade {
    const INACTIVE: Self = Self {
        mode: ChannelMode::Stereo,
        gain: 1.0,
        coefficients: [[[Coefficients::IDENTITY; 4]; BAND_COUNT]; 2],
        history: [[[[0.0; 2]; 4]; BAND_COUNT]; 2],
        audition_selection: None,
        audition_mix: 0.0,
        audition_history: [[[0.0; 2]; 4]; 2],
        remaining: 0,
        total: 0,
    };

    fn from_outgoing(previous: &Eq8Runtime, sample_rate: u32) -> Self {
        let total = mode_fade_frames(sample_rate);
        Self {
            mode: previous.mode,
            gain: previous.current_gain,
            coefficients: previous.coefficients,
            history: previous.history,
            audition_selection: previous.audition_selection,
            audition_mix: previous.audition_mix,
            audition_history: previous.audition_history,
            remaining: total,
            total,
        }
    }

    fn is_active(&self) -> bool {
        self.remaining > 0
    }

    fn blend(&mut self, input: StereoFrame, current: StereoFrame) -> StereoFrame {
        if !self.is_active() {
            return current;
        }
        let source = route_input(self.mode, input);
        let mut filtered = source;
        for (channel, value) in filtered.iter_mut().enumerate() {
            for band in 0..BAND_COUNT {
                *value = process_band(
                    *value,
                    &self.coefficients[channel][band],
                    &mut self.history[channel][band],
                );
            }
        }
        let output = route_output(self.mode, filtered);
        let normal = [
            finite_f32(output[0] * self.gain),
            finite_f32(output[1] * self.gain),
        ];
        let outgoing = match self.audition_selection {
            Some(selection) => {
                let audition = audition_signal(
                    self.mode,
                    selection,
                    &self.coefficients,
                    &mut self.audition_history,
                    source,
                );
                mix_audition(normal, audition, self.gain, self.audition_mix)
            }
            None => normal,
        };
        self.remaining -= 1;
        if self.remaining == 0 {
            self.history = [[[[0.0; 2]; 4]; BAND_COUNT]; 2];
            self.audition_history = [[[0.0; 2]; 4]; 2];
            self.audition_selection = None;
            self.audition_mix = 0.0;
            return current;
        }
        let weight = f64::from(self.remaining) / f64::from(self.total);
        std::array::from_fn(|channel| {
            let incoming = f64::from(current[channel]);
            finite_f32(incoming + (f64::from(outgoing[channel]) - incoming) * weight)
        })
    }
}

/// The solo signal of one auditioned band under `mode`, before output gain.
fn audition_signal(
    mode: ChannelMode,
    selection: Eq8AuditionSelection,
    coefficients: &[[[Coefficients; 4]; BAND_COUNT]; 2],
    history: &mut [[[f64; 2]; 4]; 2],
    source: [f64; 2],
) -> [f64; 2] {
    let mut audition = source;
    let channels: &[usize] = match mode {
        ChannelMode::Stereo => &[0, 1],
        ChannelMode::LeftRight | ChannelMode::MidSide => {
            if selection.edit_lane() == 0 {
                &[0]
            } else {
                &[1]
            }
        }
    };
    for &channel in channels {
        audition[channel] = process_band(
            audition[channel],
            &coefficients[channel][selection.band()],
            &mut history[channel],
        );
    }
    match mode {
        ChannelMode::Stereo | ChannelMode::LeftRight => audition,
        ChannelMode::MidSide => [audition[0] + audition[1], audition[0] - audition[1]],
    }
}

/// Crossfade the normal output toward the gained audition by `mix`.
fn mix_audition(normal: StereoFrame, audition: [f64; 2], gain: f64, mix: f64) -> StereoFrame {
    std::array::from_fn(|channel| {
        let normal = f64::from(normal[channel]);
        finite_f32(
            (normal + (audition[channel] * gain - normal) * mix)
                .clamp(-f64::from(f32::MAX), f64::from(f32::MAX)),
        )
    })
}

fn route_input(mode: ChannelMode, input: StereoFrame) -> [f64; 2] {
    match mode {
        ChannelMode::Stereo | ChannelMode::LeftRight => [f64::from(input[0]), f64::from(input[1])],
        ChannelMode::MidSide => [
            (f64::from(input[0]) + f64::from(input[1])) * 0.5,
            (f64::from(input[0]) - f64::from(input[1])) * 0.5,
        ],
    }
}

fn route_output(mode: ChannelMode, filtered: [f64; 2]) -> [f64; 2] {
    match mode {
        ChannelMode::Stereo | ChannelMode::LeftRight => filtered,
        ChannelMode::MidSide => [filtered[0] + filtered[1], filtered[0] - filtered[1]],
    }
}

impl Eq8Runtime {
    /// Control-side storage observation: inline runtime bytes plus preallocated
    /// rollback buffer bytes.
    pub fn control_storage(&self) -> Option<(usize, [usize; 2])> {
        Some((
            std::mem::size_of::<Self>(),
            [std::mem::size_of::<Eq8Rollback>(), 0],
        ))
    }

    pub(super) fn snapshot_scalars(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_SCALARS) == 0 {
            self.rollback.mode = self.mode;
            self.rollback.current_gain = self.current_gain;
            self.rollback.target_gain = self.target_gain;
            self.rollback.smooth_remaining = self.smooth_remaining;
            self.rollback.active_bands = self.active_bands;
            self.rollback.enabled_bands = self.enabled_bands;
            self.rollback.band_targets_pending = self.band_targets_pending;
            self.rollback.tail_controls_frozen = self.tail_controls_frozen;
            self.rollback.dirty_flags |= DIRTY_SCALARS;
        }
    }

    pub(super) fn snapshot_band_history(&mut self, channel: usize, band: usize) {
        let bit = 1_u16 << (channel * BAND_COUNT + band);
        if self.rollback.open && (self.rollback.dirty_history & bit) == 0 {
            self.rollback.history[channel][band] = self.history[channel][band];
            self.rollback.dirty_history |= bit;
        }
    }

    pub(super) fn snapshot_all_history(&mut self) {
        if !self.rollback.open || self.rollback.dirty_history == u16::MAX {
            return;
        }
        for channel in 0..2 {
            for band in 0..BAND_COUNT {
                self.snapshot_band_history(channel, band);
            }
        }
    }

    pub(super) fn snapshot_coeffs(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_COEFFS) == 0 {
            self.rollback.coefficients = self.coefficients;
            self.rollback.dirty_flags |= DIRTY_COEFFS;
        }
    }

    pub(super) fn snapshot_audition(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_AUDITION) == 0 {
            self.rollback.audition_selection = self.audition_selection;
            self.rollback.audition_request = self.audition_request;
            self.rollback.audition_mix = self.audition_mix;
            self.rollback.audition_history = self.audition_history;
            self.rollback.dirty_flags |= DIRTY_AUDITION;
        }
    }

    pub(super) fn snapshot_mode_fade(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_MODE_FADE) == 0 {
            self.rollback.mode_fade = self.mode_fade;
            self.rollback.dirty_flags |= DIRTY_MODE_FADE;
        }
    }

    pub(super) fn snapshot_controls(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_CONTROLS) == 0 {
            self.rollback.controls = self.controls;
            self.rollback.dirty_flags |= DIRTY_CONTROLS;
        }
    }

    pub(super) fn snapshot_automated_target(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_AUTOMATED_TARGET) == 0 {
            self.rollback.automated_target = self.automated_target;
            self.rollback.dirty_flags |= DIRTY_AUTOMATED_TARGET;
        }
    }

    pub(super) fn snapshot_pending_routing(&mut self) {
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_PENDING_ROUTING) == 0 {
            self.rollback.pending_routing = self.routing.pending();
            self.rollback.dirty_flags |= DIRTY_PENDING_ROUTING;
        }
    }

    /// Request a transient band audition. Invalid or disabled bands leave the
    /// current audition request untouched. Mode- or lane changes crossfade
    /// through the normal cascade and clear the parallel history at the switch.
    pub fn set_audition_selection(
        &mut self,
        selection: Option<Eq8AuditionSelection>,
    ) -> Result<(), DeviceContractError> {
        if selection.is_some_and(|selection| !self.selection_is_enabled(selection)) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        self.audition_request = selection;
        Ok(())
    }

    pub fn process_frame(&mut self, input: StereoFrame) -> StereoFrame {
        self.advance_routing();
        let current = self.process_current_routing(input);
        if self.mode_fade.is_active() {
            self.snapshot_mode_fade();
        }
        self.mode_fade.blend(input, current)
    }

    fn process_current_routing(&mut self, input: StereoFrame) -> StereoFrame {
        if self.smooth_remaining > 0 {
            self.snapshot_scalars();
            self.snapshot_coeffs();
            self.advance_coefficients();
        }
        if self.audition_selection != self.audition_request || self.audition_selection.is_some() {
            self.snapshot_audition();
            self.advance_audition();
        }
        let source = route_input(self.mode, input);
        let mut filtered = source;
        for (channel, filtered_channel) in filtered.iter_mut().enumerate() {
            for band in 0..BAND_COUNT {
                if !self.active_bands[channel][band] {
                    continue;
                }
                self.snapshot_band_history(channel, band);
                *filtered_channel = process_band(
                    *filtered_channel,
                    &self.coefficients[channel][band],
                    &mut self.history[channel][band],
                );
                let target = self
                    .routing
                    .effective_target(self.automated_target.as_ref(), self.canonical_mode)[channel]
                    [band];
                let still_active = (0..4).any(|stage| {
                    self.coefficients[channel][band][stage] != Coefficients::IDENTITY
                        || target[stage] != Coefficients::IDENTITY
                        || self.history[channel][band][stage]
                            .iter()
                            .any(|value| *value != 0.0)
                });
                if !still_active {
                    self.snapshot_scalars();
                    self.active_bands[channel][band] = false;
                }
            }
        }
        let output = route_output(self.mode, filtered);
        let normal = [
            finite_f32(output[0] * self.current_gain),
            finite_f32(output[1] * self.current_gain),
        ];
        let Some(selection) = self.audition_selection else {
            return normal;
        };
        let audition = audition_signal(
            self.mode,
            selection,
            &self.coefficients,
            &mut self.audition_history,
            source,
        );
        mix_audition(normal, audition, self.current_gain, self.audition_mix)
    }

    pub fn process(&mut self, frames: &mut [StereoFrame]) {
        for frame in frames {
            *frame = self.process_frame(*frame);
        }
    }

    pub fn reset(&mut self) {
        self.controls.reset();
        self.mode = self.controls.base().mode;
        self.reset_routing();
        self.target_gain = f64::from(self.controls.effective().gain);
        self.automated_target = None;
        self.band_targets_pending = self.band_automation_admitted;
        self.tail_controls_frozen = false;
        self.history = [[[[0.0; 2]; 4]; BAND_COUNT]; 2];
        self.audition_history = [[[0.0; 2]; 4]; 2];
        self.coefficients = self.routing.canonical_target(self.mode).map(|bank| *bank);
        self.current_gain = self.target_gain;
        self.smooth_remaining = 0;
        self.active_bands = active_band_flags(&self.coefficients);
        self.audition_selection = None;
        self.audition_request = None;
        self.audition_mix = 0.0;
        self.mode_fade = ModeFade::INACTIVE;
        self.install_enabled_bands(&self.controls.base().banks);
        self.rollback.open = false;
        self.rollback.committed_quiescent = true;
        self.rollback.dirty_flags = 0;
        self.rollback.dirty_history = 0;
    }

    pub fn inherit_history_from(&mut self, previous: &Self) -> bool {
        self.inherit_routing(previous);
        if self.sample_rate == previous.sample_rate && self.mode == previous.mode {
            self.inherit_enabled_bands(previous);
            if (self.band_automation_admitted || self.controls.admitted(automation::GAIN_SLOT))
                && (previous.band_automation_admitted
                    || previous.controls.admitted(automation::GAIN_SLOT))
            {
                self.controls.inherit(&previous.controls);
            }
            if self.controls.admitted(automation::GAIN_SLOT)
                && previous.controls.admitted(automation::GAIN_SLOT)
                && previous.controls.has_value(automation::GAIN_SLOT)
            {
                self.target_gain = previous.target_gain;
            }
            if self.band_automation_admitted {
                self.automated_target = Some(previous.current_target().map(|bank| *bank));
                self.band_targets_pending = true;
            }
        }
        if self.sample_rate != previous.sample_rate {
            self.reset();
            false
        } else if self.mode != previous.mode {
            // The lane interpretation changes (stereo/LR/MS), so previous
            // histories cannot be reused in the new routing. Start it empty
            // and crossfade out of the previous routing (see `ModeFade`).
            self.reset();
            // Lane histories are not inherited (false); only the fade is.
            self.mode_fade = ModeFade::from_outgoing(previous, self.sample_rate);
            false
        } else {
            // Copy the fading branch only while it is active; an inactive
            // branch's storage is never read.
            if previous.mode_fade.is_active() {
                self.mode_fade = previous.mode_fade;
            } else {
                self.mode_fade.remaining = 0;
            }
            self.current_gain = previous.current_gain;
            let same_target = previous.current_target() == self.current_target()
                && previous.target_gain == self.target_gain;
            self.smooth_remaining = if same_target {
                previous.smooth_remaining
            } else {
                coefficient_smooth_frames(self.sample_rate)
            };
            self.coefficients = previous.coefficients;
            self.history = previous.history;
            self.refresh_active_bands();
            if previous
                .audition_selection
                .is_none_or(|selection| self.selection_is_enabled(selection))
                && previous
                    .audition_request
                    .is_none_or(|selection| self.selection_is_enabled(selection))
            {
                self.audition_selection = previous.audition_selection;
                self.audition_request = previous.audition_request;
                self.audition_mix = previous.audition_mix;
                self.audition_history = previous.audition_history;
            } else {
                self.clear_audition();
            }
            true
        }
    }

    fn selection_is_enabled(&self, selection: Eq8AuditionSelection) -> bool {
        match self.mode {
            ChannelMode::Stereo => {
                self.enabled_bands[0][selection.band()] && self.enabled_bands[1][selection.band()]
            }
            ChannelMode::LeftRight | ChannelMode::MidSide => {
                self.enabled_bands[selection.edit_lane()][selection.band()]
            }
        }
    }

    fn advance_audition(&mut self) {
        let step = 1.0 / f64::from(eq8_audition_crossfade_frames(self.sample_rate));
        if self.audition_selection != self.audition_request {
            if self.audition_mix > 0.0 {
                self.audition_mix = hex_dsp::flush_subnormal((self.audition_mix - step).max(0.0));
                if self.audition_mix == 0.0 {
                    self.audition_selection = self.audition_request;
                    self.audition_history = [[[0.0; 2]; 4]; 2];
                }
                return;
            }
            self.audition_selection = self.audition_request;
            self.audition_history = [[[0.0; 2]; 4]; 2];
        }
        if self.audition_selection.is_some() {
            self.audition_mix = (self.audition_mix + step).min(1.0);
        } else {
            self.audition_mix = 0.0;
        }
    }

    fn clear_audition(&mut self) {
        self.audition_selection = None;
        self.audition_request = None;
        self.audition_mix = 0.0;
        self.audition_history = [[[0.0; 2]; 4]; 2];
    }

    fn advance_coefficients(&mut self) {
        if self.smooth_remaining == 0 {
            return;
        }
        let divisor = f64::from(self.smooth_remaining);
        let targets = self
            .routing
            .effective_target(self.automated_target.as_ref(), self.canonical_mode);
        for (current, target) in self.coefficients.iter_mut().zip(targets) {
            for (current, target) in current.iter_mut().flatten().zip(target.iter().flatten()) {
                current.step_toward(*target, divisor);
            }
        }
        self.smooth_remaining -= 1;
        if self.smooth_remaining == 0 {
            self.coefficients = targets.map(|bank| *bank);
            self.current_gain = self.target_gain;
            self.refresh_active_bands();
        } else {
            self.current_gain = hex_dsp::flush_subnormal(
                self.current_gain + (self.target_gain - self.current_gain) / divisor,
            );
        }
    }

    fn refresh_active_bands(&mut self) {
        let targets = self
            .routing
            .effective_target(self.automated_target.as_ref(), self.canonical_mode);
        for (channel, target) in targets.into_iter().enumerate() {
            for (band, target) in target.iter().enumerate() {
                self.active_bands[channel][band] = (0..4).any(|stage| {
                    self.coefficients[channel][band][stage] != Coefficients::IDENTITY
                        || target[stage] != Coefficients::IDENTITY
                        || self.history[channel][band][stage]
                            .iter()
                            .any(|value| *value != 0.0)
                });
            }
        }
    }

    pub fn is_quiescent(&self) -> bool {
        !self.mode_fade.is_active()
            && self.audition_selection.is_none()
            && self.audition_request.is_none()
            && self.audition_mix == 0.0
            && self
                .audition_history
                .iter()
                .flatten()
                .flatten()
                .all(|value| *value == 0.0)
            && self
                .history
                .iter()
                .flatten()
                .flatten()
                .flatten()
                .all(|value| *value == 0.0)
    }
}

fn process_band(mut input: f64, coefficients: &BandStages, history: &mut [[f64; 2]; 4]) -> f64 {
    for stage in 0..4 {
        input = coefficients[stage].process_tdf2_with_floor(
            input,
            &mut history[stage],
            hex_dsp::POWER_FLOOR,
        );
    }
    input
}

fn active_band_flags(
    coefficients: &[[[Coefficients; 4]; BAND_COUNT]; 2],
) -> [[bool; BAND_COUNT]; 2] {
    let mut active = [[false; BAND_COUNT]; 2];
    for channel in 0..2 {
        for band in 0..BAND_COUNT {
            active[channel][band] =
                (0..4).any(|stage| coefficients[channel][band][stage] != Coefficients::IDENTITY);
        }
    }
    active
}

impl OwnedProcessorRuntime for Eq8Runtime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>() + std::mem::size_of::<Eq8Rollback>())
    }
    fn begin_block(&mut self) {
        if self.rollback.open {
            self.rollback_block();
        }
        self.rollback.committed_quiescent = self.is_quiescent();
        self.rollback.dirty_flags = 0;
        self.rollback.dirty_history = 0;
        self.rollback.open = true;
    }
    fn commit_block(&mut self) {
        self.rollback.open = false;
        self.rollback.dirty_flags = 0;
        self.rollback.dirty_history = 0;
    }
    fn rollback_block(&mut self) {
        if !self.rollback.open {
            return;
        }
        let flags = self.rollback.dirty_flags;
        let mut bits = self.rollback.dirty_history;
        while bits != 0 {
            let slot = bits.trailing_zeros() as usize;
            let channel = slot / BAND_COUNT;
            let band = slot % BAND_COUNT;
            self.history[channel][band] = self.rollback.history[channel][band];
            bits &= bits - 1;
        }
        if (flags & DIRTY_SCALARS) != 0 {
            self.mode = self.rollback.mode;
            self.current_gain = self.rollback.current_gain;
            self.target_gain = self.rollback.target_gain;
            self.smooth_remaining = self.rollback.smooth_remaining;
            self.active_bands = self.rollback.active_bands;
            self.enabled_bands = self.rollback.enabled_bands;
            self.band_targets_pending = self.rollback.band_targets_pending;
            self.tail_controls_frozen = self.rollback.tail_controls_frozen;
        }
        if (flags & DIRTY_COEFFS) != 0 {
            self.coefficients = self.rollback.coefficients;
        }
        if (flags & DIRTY_AUDITION) != 0 {
            self.audition_selection = self.rollback.audition_selection;
            self.audition_request = self.rollback.audition_request;
            self.audition_mix = self.rollback.audition_mix;
            self.audition_history = self.rollback.audition_history;
        }
        if (flags & DIRTY_MODE_FADE) != 0 {
            self.mode_fade = self.rollback.mode_fade;
        }
        if (flags & DIRTY_CONTROLS) != 0 {
            self.controls = self.rollback.controls;
        }
        if (flags & DIRTY_AUTOMATED_TARGET) != 0 {
            self.automated_target = self.rollback.automated_target;
        }
        if (flags & DIRTY_PENDING_ROUTING) != 0 {
            self.routing.install(self.rollback.pending_routing);
        }
        self.rollback.open = false;
        self.rollback.dirty_flags = 0;
        self.rollback.dirty_history = 0;
    }
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        if !frames.is_empty() && !self.tail_controls_frozen {
            self.restore_canonical_controls();
        }
        self.process(frames);
    }
    fn begin_output_tail(&mut self) {
        self.tail_controls_frozen = true;
        if self.rollback.open {
            self.rollback.tail_controls_frozen = true;
        }
    }
    fn supports_parameter_automation(&self, id: ParameterId) -> bool {
        automation::slot(id).is_some_and(|i| self.controls.admitted(i))
    }
    crate::parameter_binding::delegate_automated_block_processing!(process_automation);
    fn reset(&mut self) {
        Eq8Runtime::reset(self);
    }
    fn committed_is_quiescent(&self) -> bool {
        if self.rollback.open {
            self.rollback.committed_quiescent
        } else {
            Eq8Runtime::is_quiescent(self)
        }
    }
    fn speculative_is_quiescent(&self) -> bool {
        Eq8Runtime::is_quiescent(self)
    }
    fn can_inherit_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        !self.rollback.open
            && previous
                .as_any()
                .downcast_ref::<Self>()
                .is_some_and(|previous| {
                    !previous.rollback.open && self.sample_rate == previous.sample_rate
                })
    }
    /// A channel-mode change retargets through `ModeFade` instead of a reset,
    /// so session playback keeps one chain rather than detaching the old
    /// suffix as a ringing tail beside a fresh EQ. Within one mode, the
    /// canonical enabled-band set must match except where either runtime has
    /// admitted that exact enable control. Its fixed-stage smoother owns the
    /// transition, including lane removal and canonical fallback.
    fn can_retarget_parameters_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        !self.rollback.open
            && previous
                .as_any()
                .downcast_ref::<Self>()
                .is_some_and(|previous| {
                    !previous.rollback.open
                        && self.sample_rate == previous.sample_rate
                        && if self.preserves_routing(previous) {
                            self.enabled_layout_compatible(previous, previous.mode)
                        } else {
                            self.mode != previous.mode
                                || self.enabled_layout_compatible(previous, self.mode)
                        }
                })
    }
    fn inherit_from(&mut self, previous: &dyn OwnedProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            let _ = self.inherit_history_from(previous);
        }
    }
    /// Ordinary canonical mode changes wait for the occupied fade (Pulse #4106).
    /// Two admitted routing runtimes preserve that branch instead; their own
    /// latest validated target waits in fixed storage beside it.
    fn transition_busy_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| {
                self.sample_rate == previous.sample_rate
                    && !self.preserves_routing(previous)
                    && self.mode != previous.mode
                    && previous.mode_fade.is_active()
            })
    }
    fn as_any(&self) -> &dyn StdAny {
        self
    }
    fn transient_audition(&mut self) -> Option<&mut dyn NativeProcessorAudition> {
        Some(self)
    }
    fn transient_audition_requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        NativeProcessorAudition::requested_selection(self)
    }
}

impl NativeProcessorAudition for Eq8Runtime {
    fn can_select(&self, selection: ProcessorAuditionSelection) -> bool {
        Eq8AuditionSelection::new(selection.index as usize, usize::from(selection.lane))
            .is_ok_and(|selection| self.selection_is_enabled(selection))
    }

    fn requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        self.audition_request
            .map(|selection| ProcessorAuditionSelection {
                index: u32::from(selection.band),
                lane: selection.edit_lane,
            })
    }

    fn select(&mut self, selection: Option<ProcessorAuditionSelection>) {
        // `can_select` preflights this exact stable mapping; update both live
        // and any open rollback snapshot atomically.
        let selection = selection.map(|selection| Eq8AuditionSelection {
            band: selection.index as u8,
            edit_lane: selection.lane,
        });
        self.audition_request = selection;
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_AUDITION) != 0 {
            self.rollback.audition_request = selection;
        }
    }

    fn cancel_immediately(&mut self) {
        self.clear_audition();
        if self.rollback.open && (self.rollback.dirty_flags & DIRTY_AUDITION) != 0 {
            self.rollback.audition_selection = None;
            self.rollback.audition_request = None;
            self.rollback.audition_mix = 0.0;
            self.rollback.audition_history = [[[0.0; 2]; 4]; 2];
        }
    }
}

fn quiet(value: f64) -> f64 {
    hex_dsp::flush_below(value, hex_dsp::POWER_FLOOR)
}

fn finite_f32(value: f64) -> f32 {
    if !value.is_finite() {
        0.0
    } else {
        value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
    }
}

fn response_magnitude(coefficients: Coefficients, frequency_hz: f64, sample_rate: f64) -> f64 {
    coefficients.magnitude_at_omega(std::f64::consts::TAU * frequency_hz / sample_rate)
}

fn design_band(band: Band, sample_rate: u32) -> Result<BandStages, DeviceContractError> {
    let rate = f64::from(sample_rate);
    let nyquist = rate * 0.5;
    let frequency = f64::from(band.frequency_hz).min(nyquist * 0.98);
    if frequency <= 0.0 || !frequency.is_finite() {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let q = f64::from(band.q);
    let gain_db = f64::from(band.gain_db);
    let mut stages = [Coefficients::IDENTITY; 4];
    let map_err = |_| DeviceContractError::InvalidParameterValue;
    match band.kind {
        0 => stages[0] = Coefficients::peaking(frequency, rate, q, gain_db).map_err(map_err)?,
        1 => {
            stages[0] =
                Coefficients::low_shelf_slope(frequency, rate, q, gain_db).map_err(map_err)?
        }
        2 => {
            stages[0] =
                Coefficients::high_shelf_slope(frequency, rate, q, gain_db).map_err(map_err)?
        }
        3 => stages[0] = Coefficients::high_pass(frequency, rate, q).map_err(map_err)?,
        4 => {
            let mut section_qs = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];
            section_qs[3] *= q / f64::from(std::f32::consts::FRAC_1_SQRT_2);
            for (stage, section_q) in section_qs.into_iter().enumerate() {
                stages[stage] =
                    Coefficients::high_pass(frequency, rate, section_q).map_err(map_err)?;
            }
        }
        5 => stages[0] = Coefficients::low_pass(frequency, rate, q).map_err(map_err)?,
        6 => {
            let mut section_qs = [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4];
            section_qs[3] *= q / f64::from(std::f32::consts::FRAC_1_SQRT_2);
            for (stage, section_q) in section_qs.into_iter().enumerate() {
                stages[stage] =
                    Coefficients::low_pass(frequency, rate, section_q).map_err(map_err)?;
            }
        }
        7 => stages[0] = Coefficients::notch(frequency, rate, q).map_err(map_err)?,
        _ => return Err(DeviceContractError::InvalidParameterValue),
    }
    Ok(stages)
}

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
