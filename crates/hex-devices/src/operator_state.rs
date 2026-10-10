//! Original Operator control state and registered shared-context instrument.
//!
//! Version-3 data preserves the version-2 prefix and appends fixed tuning.
//! The old FM4 device keeps its separate type, state layout and global ADSR.
use crate::operator_waveform::OperatorWaveform;
use crate::{
    SineTable,
    fm4::{
        OPERATORS, Oscillator, PreparedOperatorConfiguration, PreparedOperatorFm4,
        PreparedOperatorNote, Routing,
    },
    operator_envelope::OperatorEnvelope,
};
use hex_lattice::{
    NativeInstrumentDefinition, PreparedAnalyticNote, PreparedNativeInstrument,
    PreparedSharedAnalyticContext, PreparedSharedAnalyticNote, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};

pub const OPERATOR_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.operator");
pub const OPERATOR_STATE_VERSION: u32 = 3;
const WAVEFORM_STATE_VERSION: u32 = 2;
mod tuning;
const LEGACY_STATE_VERSION: u32 = 1;
const CORE_FIELDS: usize = 32;
const ENVELOPE_FIELDS: usize = 6;
const FIELD_COUNT: usize = CORE_FIELDS + OPERATORS * ENVELOPE_FIELDS;
const FLOAT_PARAMETER_COUNT: usize = FIELD_COUNT - OPERATORS;
const WAVEFORM_COUNT: usize = OPERATORS;
const PARAMETER_COUNT: usize = FLOAT_PARAMETER_COUNT + WAVEFORM_COUNT + 1 + OPERATORS * 3;
const LEGACY_PAYLOAD_BYTES: usize = FIELD_COUNT * size_of::<f32>();
const WAVEFORM_PAYLOAD_BYTES: usize = LEGACY_PAYLOAD_BYTES + WAVEFORM_COUNT * size_of::<u32>();
const PAYLOAD_BYTES: usize = WAVEFORM_PAYLOAD_BYTES + tuning::BYTES;
const ENVELOPE_DEFAULTS: [f32; ENVELOPE_FIELDS] = [0.0, 1.0, 0.7, 0.005, 0.1, 0.1];

pub mod parameter {
    pub use crate::fm4::device::parameter::{
        carrier, modulation, oscillator_level, oscillator_phase, oscillator_ratio,
    };
    use hex_lattice_contract::ParameterId;

    /// Stable per-operator envelope field order in the opaque state.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u32)]
    pub enum EnvelopeField {
        Initial,
        Peak,
        Sustain,
        Attack,
        Decay,
        Release,
    }

    /// Operator numbers are one-based. Zero denotes an invalid endpoint.
    pub const fn envelope(operator: u32, field: EnvelopeField) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(33 + (operator - 1) * 6 + field as u32)
    }

    /// Operator numbers are one-based. Zero denotes an invalid endpoint.
    pub const fn waveform(operator: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(56 + operator)
    }

    pub const fn fixed(operator: u32) -> ParameterId {
        tuning_parameter(operator, 0)
    }
    pub const fn fixed_frequency(operator: u32) -> ParameterId {
        tuning_parameter(operator, 1)
    }
    pub const fn fixed_multiplier(operator: u32) -> ParameterId {
        tuning_parameter(operator, 2)
    }
    const fn tuning_parameter(operator: u32, field: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(62 + (operator - 1) * 3 + field)
    }

    /// Derived topology selector. Custom is inferred from the stored routing.
    pub const fn routing_preset() -> ParameterId {
        ParameterId::from_static(61)
    }
}

const ENVELOPE_LABELS: [&str; OPERATORS * ENVELOPE_FIELDS] = [
    "op1 initial",
    "op1 peak",
    "op1 sustain",
    "op1 attack",
    "op1 decay",
    "op1 release",
    "op2 initial",
    "op2 peak",
    "op2 sustain",
    "op2 attack",
    "op2 decay",
    "op2 release",
    "op3 initial",
    "op3 peak",
    "op3 sustain",
    "op3 attack",
    "op3 decay",
    "op3 release",
    "op4 initial",
    "op4 peak",
    "op4 sustain",
    "op4 attack",
    "op4 decay",
    "op4 release",
];

const ROUTING_PRESET_LABELS: [&str; 4] = ["custom", "serial", "parallel", "two pairs"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperatorRoutingPreset {
    Custom,
    Serial,
    Parallel,
    TwoPairs,
}

impl OperatorRoutingPreset {
    const fn from_choice(choice: u32) -> Option<Self> {
        match choice {
            0 => Some(Self::Custom),
            1 => Some(Self::Serial),
            2 => Some(Self::Parallel),
            3 => Some(Self::TwoPairs),
            _ => None,
        }
    }

    const fn choice(self) -> u32 {
        match self {
            Self::Custom => 0,
            Self::Serial => 1,
            Self::Parallel => 2,
            Self::TwoPairs => 3,
        }
    }

    const fn routing_fields(self) -> Option<[f32; 20]> {
        let mut fields = [0.0; 20];
        match self {
            Self::Custom => None,
            Self::Serial => {
                fields[4] = 0.25; // op2 -> op1
                fields[9] = 0.25; // op3 -> op2
                fields[14] = 0.25; // op4 -> op3
                fields[16] = 1.0; // op1 carrier
                Some(fields)
            }
            Self::Parallel => {
                fields[16] = 0.25;
                fields[17] = 0.25;
                fields[18] = 0.25;
                fields[19] = 0.25;
                Some(fields)
            }
            Self::TwoPairs => {
                fields[4] = 0.25; // op2 -> op1
                fields[14] = 0.25; // op4 -> op3
                fields[16] = 0.5; // op1 carrier
                fields[18] = 0.5; // op3 carrier
                Some(fields)
            }
        }
    }
}

const fn parameters() -> [ParameterDescriptor; PARAMETER_COUNT] {
    let mut result = [crate::fm4::device::PARAMETERS[0]; PARAMETER_COUNT];
    let mut index = 0;
    while index < CORE_FIELDS - OPERATORS {
        result[index] = crate::fm4::device::PARAMETERS[index];
        index += 1;
    }
    let mut envelope = 0;
    while envelope < ENVELOPE_LABELS.len() {
        let field = envelope % ENVELOPE_FIELDS;
        let time = field >= 3;
        result[index] = ParameterDescriptor {
            id: ParameterId::from_static((CORE_FIELDS + envelope + 1) as u32),
            label: ENVELOPE_LABELS[envelope],
            kind: ParameterKind::Float {
                min: 0.0,
                max: if time { 120.0 } else { 1.0 },
                unit: if time { Some("s") } else { None },
            },
            default: ParameterValue::Float(ENVELOPE_DEFAULTS[field]),
            presentation: ParameterPresentation::Linear,
        };
        index += 1;
        envelope += 1;
    }
    let waveforms = [
        "op1 waveform",
        "op2 waveform",
        "op3 waveform",
        "op4 waveform",
    ];
    let mut operator = 0;
    while operator < OPERATORS {
        result[index] = ParameterDescriptor {
            id: ParameterId::from_static((57 + operator) as u32),
            label: waveforms[operator],
            kind: ParameterKind::Choice {
                labels: OperatorWaveform::labels(),
            },
            default: ParameterValue::Choice(0),
            presentation: ParameterPresentation::Linear,
        };
        index += 1;
        operator += 1;
    }
    result[index] = ParameterDescriptor {
        id: ParameterId::from_static(61),
        label: "routing preset",
        kind: ParameterKind::Choice {
            labels: &ROUTING_PRESET_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    };
    index += 1;
    let tuning = tuning::descriptors();
    let mut offset = 0;
    while offset < tuning.len() {
        result[index + offset] = tuning[offset];
        offset += 1;
    }
    result
}

static PARAMETERS: [ParameterDescriptor; PARAMETER_COUNT] = parameters();

/// Validated control-side configuration. It owns no runtime voices or assets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OperatorSettings {
    fields: [f32; FIELD_COUNT],
    waveforms: [OperatorWaveform; OPERATORS],
    tuning: tuning::TuningState,
}

impl OperatorSettings {
    /// Original defaults; no measured Live transfer/default compatibility claim.
    pub fn defaults() -> Self {
        let mut fields = [0.0; FIELD_COUNT];
        for descriptor in PARAMETERS.iter().take(FLOAT_PARAMETER_COUNT) {
            if let ParameterValue::Float(value) = descriptor.default {
                fields[descriptor.id.value() as usize - 1] = value;
            }
        }
        Self {
            fields,
            waveforms: [OperatorWaveform::Sine; OPERATORS],
            tuning: tuning::TuningState::default(),
        }
    }

    pub fn from_state(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != OPERATOR_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        let expected_len = match state.state_version() {
            LEGACY_STATE_VERSION => LEGACY_PAYLOAD_BYTES,
            WAVEFORM_STATE_VERSION => WAVEFORM_PAYLOAD_BYTES,
            OPERATOR_STATE_VERSION => PAYLOAD_BYTES,
            _ => return Err(DeviceContractError::UnsupportedStateVersion),
        };
        if state.payload().len() != expected_len {
            return Err(DeviceContractError::MalformedState);
        }
        let mut fields = [0.0; FIELD_COUNT];
        for (index, bytes) in state.payload()[..LEGACY_PAYLOAD_BYTES]
            .chunks_exact(size_of::<f32>())
            .enumerate()
        {
            let bytes: [u8; 4] = bytes
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?;
            let field = fields
                .get_mut(index)
                .ok_or(DeviceContractError::MalformedState)?;
            *field = f32::from_le_bytes(bytes);
        }
        let mut waveforms = [OperatorWaveform::Sine; OPERATORS];
        if state.state_version() >= WAVEFORM_STATE_VERSION {
            for (index, bytes) in state.payload()[LEGACY_PAYLOAD_BYTES..WAVEFORM_PAYLOAD_BYTES]
                .chunks_exact(size_of::<u32>())
                .enumerate()
            {
                let bytes: [u8; 4] = bytes
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?;
                let choice = u32::from_le_bytes(bytes);
                waveforms[index] = OperatorWaveform::from_choice(choice)
                    .ok_or(DeviceContractError::MalformedState)?;
            }
        }
        let tuning = if state.state_version() == OPERATOR_STATE_VERSION {
            tuning::TuningState::decode(&state.payload()[WAVEFORM_PAYLOAD_BYTES..])?
        } else {
            tuning::TuningState::default()
        };
        let result = Self {
            fields,
            waveforms,
            tuning,
        };
        result.kernel_parts()?;
        Ok(result)
    }

    pub fn to_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.kernel_parts()?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(PAYLOAD_BYTES)
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        for field in self.fields {
            payload.extend_from_slice(&field.to_le_bytes());
        }
        for waveform in self.waveforms {
            payload.extend_from_slice(&waveform.choice().to_le_bytes());
        }
        self.tuning.append(&mut payload);
        NativeDeviceState::new(OPERATOR_TYPE_ID, OPERATOR_STATE_VERSION, payload)
    }

    /// Decode validated original oscillator, graph and envelope laws on control.
    pub fn kernel_parts(
        &self,
    ) -> Result<
        (
            [Oscillator; OPERATORS],
            Routing,
            [OperatorEnvelope; OPERATORS],
        ),
        DeviceContractError,
    > {
        self.tuning.modes()?;
        for descriptor in PARAMETERS.iter().take(FLOAT_PARAMETER_COUNT) {
            descriptor.validate(ParameterValue::Float(
                self.fields[descriptor.id.value() as usize - 1],
            ))?;
        }
        if (0..OPERATORS).any(|operator| self.fields[12 + operator * OPERATORS + operator] != 0.0) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let oscillators = std::array::from_fn(|operator| {
            let offset = operator * 3;
            Oscillator::new(
                f64::from(self.fields[offset]),
                f64::from(self.fields[offset + 1]),
                f64::from(self.fields[offset + 2]),
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)
        });
        let [a, b, c, d] = oscillators;
        let routing = Routing::new(
            std::array::from_fn(|source| {
                std::array::from_fn(|target| {
                    f64::from(self.fields[12 + source * OPERATORS + target])
                })
            }),
            std::array::from_fn(|operator| f64::from(self.fields[28 + operator])),
        )
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let envelopes = std::array::from_fn(|operator| {
            let offset = CORE_FIELDS + operator * ENVELOPE_FIELDS;
            OperatorEnvelope::new(
                f64::from(self.fields[offset]),
                f64::from(self.fields[offset + 1]),
                f64::from(self.fields[offset + 2]),
                f64::from(self.fields[offset + 3]),
                f64::from(self.fields[offset + 4]),
                f64::from(self.fields[offset + 5]),
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)
        });
        let [ea, eb, ec, ed] = envelopes;
        Ok(([a?, b?, c?, d?], routing, [ea?, eb?, ec?, ed?]))
    }

    fn routing_preset(&self) -> OperatorRoutingPreset {
        for preset in [
            OperatorRoutingPreset::Serial,
            OperatorRoutingPreset::Parallel,
            OperatorRoutingPreset::TwoPairs,
        ] {
            if let Some(expected) = preset.routing_fields()
                && self.fields[12..32] == expected
            {
                return preset;
            }
        }
        OperatorRoutingPreset::Custom
    }

    fn apply_routing_preset(&mut self, preset: OperatorRoutingPreset) {
        if let Some(fields) = preset.routing_fields() {
            self.fields[12..32].copy_from_slice(&fields);
        }
    }

    /// Standalone control preparation only; no native sampler/registry admission.
    pub fn prepare_configuration(
        &self,
        sample_rate: u32,
    ) -> Result<PreparedOperatorConfiguration, DeviceContractError> {
        let (oscillators, routing, envelopes) = self.kernel_parts()?;
        PreparedOperatorConfiguration::new_with_tuning(
            sample_rate,
            oscillators,
            routing,
            envelopes,
            self.waveforms,
            self.tuning.modes()?,
        )
        .map_err(|_| DeviceContractError::InvalidParameterValue)
    }

    /// Standalone control preparation only; no native sampler/registry admission.
    pub fn prepare_note(
        &self,
        sample_rate: u32,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedOperatorFm4, DeviceContractError> {
        let (oscillators, routing, envelopes) = self.kernel_parts()?;
        PreparedOperatorFm4::new_with_tuning(
            sample_rate,
            pitch,
            velocity,
            oscillators,
            routing,
            envelopes,
            self.waveforms,
            self.tuning.modes()?,
        )
        .map_err(|_| DeviceContractError::InvalidParameterValue)
    }
}

/// Generic Operator control metadata and shared analytic instrument definition.
#[derive(Clone, Copy, Debug, Default)]
pub struct OperatorMetadata;
pub static OPERATOR_METADATA: OperatorMetadata = OperatorMetadata;

impl NativeDeviceMetadata for OperatorMetadata {
    fn type_id(&self) -> DeviceTypeId {
        OPERATOR_TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("operator")
    }
    fn state_version(&self) -> u32 {
        OPERATOR_STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &PARAMETERS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        OperatorSettings::defaults().to_state()
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let settings = OperatorSettings::from_state(state)?;
        settings.to_state()
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = OperatorSettings::from_state(state)?;
        if let Some(value) = settings.tuning.read(parameter) {
            return Ok(value);
        }
        if parameter == parameter::routing_preset() {
            return Ok(ParameterValue::Choice(settings.routing_preset().choice()));
        }
        if let Some(index) = waveform_index(parameter) {
            return settings
                .waveforms
                .get(index)
                .copied()
                .map(|waveform| ParameterValue::Choice(waveform.choice()))
                .ok_or(DeviceContractError::UnknownParameter);
        }
        settings
            .fields
            .get(parameter_index(parameter)?)
            .copied()
            .map(ParameterValue::Float)
            .ok_or(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let descriptor = PARAMETERS
            .iter()
            .find(|descriptor| descriptor.id == parameter)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(value)?;
        let mut settings = OperatorSettings::from_state(state)?;
        if settings.tuning.patch(parameter, value)? {
            return settings.to_state();
        }
        if parameter == parameter::routing_preset() {
            let ParameterValue::Choice(choice) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            let preset = OperatorRoutingPreset::from_choice(choice)
                .ok_or(DeviceContractError::InvalidParameterValue)?;
            settings.apply_routing_preset(preset);
            return settings.to_state();
        }
        if let Some(index) = waveform_index(parameter) {
            let ParameterValue::Choice(value) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            settings.waveforms[index] = OperatorWaveform::from_choice(value)
                .ok_or(DeviceContractError::InvalidParameterValue)?;
            return settings.to_state();
        }
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        let field = settings
            .fields
            .get_mut(parameter_index(parameter)?)
            .ok_or(DeviceContractError::UnknownParameter)?;
        *field = value;
        settings.to_state()
    }
}

impl NativeInstrumentDefinition for OperatorMetadata {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let settings = OperatorSettings::from_state(state)?;
        let configuration = settings.prepare_configuration(sample_rate)?;
        // The shared table is initialized on the control side before any
        // callback can borrow it for note sampling.
        let sine_table = crate::shared_sine_table();
        Ok(Box::new(PreparedOperatorInstrument {
            configuration,
            sine_table,
        }))
    }
}

struct PreparedOperatorInstrument {
    configuration: PreparedOperatorConfiguration,
    sine_table: &'static SineTable,
}

impl PreparedNativeInstrument for PreparedOperatorInstrument {
    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }

    fn prepare_shared_analytic_context(
        &self,
    ) -> Result<Option<Box<dyn PreparedSharedAnalyticContext>>, DeviceContractError> {
        Ok(Some(Box::new(PreparedOperatorContext {
            configuration: self.configuration.clone(),
            sine_table: self.sine_table,
        })))
    }
}

struct PreparedOperatorContext {
    configuration: PreparedOperatorConfiguration,
    sine_table: &'static SineTable,
}

impl PreparedSharedAnalyticContext for PreparedOperatorContext {
    fn declared_context_bytes(&self) -> usize {
        size_of::<Self>()
    }

    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedSharedAnalyticNote, DeviceContractError> {
        let note = self
            .configuration
            .prepare_note(pitch, velocity)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        Ok(PreparedSharedAnalyticNote::new(
            note.into_shared_payload(),
            self.configuration.release_frames(),
        ))
    }

    fn sample(
        &self,
        note: &PreparedSharedAnalyticNote,
        age_frames: u64,
        gate_frames: u64,
    ) -> StereoFrame {
        if note.release_frames() != self.configuration.release_frames() {
            return [0.0; 2];
        }
        let Some(note) = PreparedOperatorNote::from_shared_payload(note.payload()) else {
            return [0.0; 2];
        };
        if !self.configuration.note_matches_configuration(&note) {
            return [0.0; 2];
        }
        let sample = self
            .configuration
            .sample(self.sine_table, &note, age_frames, gate_frames);
        [sample, sample]
    }
}

fn parameter_index(parameter: ParameterId) -> Result<usize, DeviceContractError> {
    if !PARAMETERS
        .iter()
        .take(FLOAT_PARAMETER_COUNT)
        .any(|descriptor| descriptor.id == parameter)
    {
        return Err(DeviceContractError::UnknownParameter);
    }
    usize::try_from(parameter.value())
        .ok()
        .and_then(|id| id.checked_sub(1))
        .ok_or(DeviceContractError::UnknownParameter)
}

fn waveform_index(parameter: ParameterId) -> Option<usize> {
    let index = parameter.value().checked_sub(57)? as usize;
    (index < OPERATORS).then_some(index)
}

#[cfg(test)]
mod fixed_tests;
#[cfg(test)]
mod tests;
