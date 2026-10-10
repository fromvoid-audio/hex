//! Original Operator control state; not registered or exposed in the app yet.
//!
//! Generic opaque version-1 data holds 56 little-endian finite f32 fields.
//! The old FM4 device keeps its separate type, state layout and global ADSR.
use crate::{
    fm4::{OPERATORS, Oscillator, PreparedOperatorConfiguration, PreparedOperatorFm4, Routing},
    operator_envelope::OperatorEnvelope,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};

pub const OPERATOR_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.operator");
pub const OPERATOR_STATE_VERSION: u32 = 1;
const CORE_FIELDS: usize = 32;
const ENVELOPE_FIELDS: usize = 6;
const FIELD_COUNT: usize = CORE_FIELDS + OPERATORS * ENVELOPE_FIELDS;
const PARAMETER_COUNT: usize = FIELD_COUNT - OPERATORS;
const PAYLOAD_BYTES: usize = FIELD_COUNT * size_of::<f32>();
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
    result
}

static PARAMETERS: [ParameterDescriptor; PARAMETER_COUNT] = parameters();

/// Validated control-side configuration. It owns no runtime voices or assets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OperatorSettings {
    fields: [f32; FIELD_COUNT],
}

impl OperatorSettings {
    /// Original defaults; no measured Live transfer/default compatibility claim.
    pub fn defaults() -> Self {
        let mut fields = [0.0; FIELD_COUNT];
        for descriptor in PARAMETERS {
            if let ParameterValue::Float(value) = descriptor.default {
                fields[descriptor.id.value() as usize - 1] = value;
            }
        }
        Self { fields }
    }

    pub fn from_state(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != OPERATOR_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != OPERATOR_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if state.payload().len() != PAYLOAD_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut fields = [0.0; FIELD_COUNT];
        for (index, bytes) in state.payload().chunks_exact(size_of::<f32>()).enumerate() {
            let bytes: [u8; 4] = bytes
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?;
            let field = fields
                .get_mut(index)
                .ok_or(DeviceContractError::MalformedState)?;
            *field = f32::from_le_bytes(bytes);
        }
        let result = Self { fields };
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
        for descriptor in PARAMETERS {
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

    /// Standalone control preparation only; no native sampler/registry admission.
    pub fn prepare_configuration(
        &self,
        sample_rate: u32,
    ) -> Result<PreparedOperatorConfiguration, DeviceContractError> {
        let (oscillators, routing, envelopes) = self.kernel_parts()?;
        PreparedOperatorConfiguration::new(sample_rate, oscillators, routing, envelopes)
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
        PreparedOperatorFm4::new(
            sample_rate,
            pitch,
            velocity,
            oscillators,
            routing,
            envelopes,
        )
        .map_err(|_| DeviceContractError::InvalidParameterValue)
    }
}

/// Metadata-only control contract. Registration awaits shared-context ownership.
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
        OperatorSettings::from_state(state)?;
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let settings = OperatorSettings::from_state(state)?;
        let index = parameter_index(parameter)?;
        settings
            .fields
            .get(index)
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
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        let mut settings = OperatorSettings::from_state(state)?;
        let field = settings
            .fields
            .get_mut(parameter_index(parameter)?)
            .ok_or(DeviceContractError::UnknownParameter)?;
        *field = value;
        settings.to_state()
    }
}

fn parameter_index(parameter: ParameterId) -> Result<usize, DeviceContractError> {
    if !PARAMETERS
        .iter()
        .any(|descriptor| descriptor.id == parameter)
    {
        return Err(DeviceContractError::UnknownParameter);
    }
    usize::try_from(parameter.value())
        .ok()
        .and_then(|id| id.checked_sub(1))
        .ok_or(DeviceContractError::UnknownParameter)
}

#[cfg(test)]
mod tests;
