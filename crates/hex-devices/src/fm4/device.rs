//! Original generic state and instrument-definition adapter for the FM4 kernel.
//!
//! This built-in definition is admitted by the native instrument registry. Its
//! version-1 payload is a bounded, exact little-endian sequence
//! of 36 finite `f32` fields; preparation validates it through the standalone
//! oscillator, routing and envelope constructors.
mod runtime;

use super::{Envelope, OPERATORS, Oscillator, PreparedFm4, Routing};
use hex_lattice::{NativeInstrumentDefinition, PreparedAnalyticNote, PreparedNativeInstrument};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};

pub const FM4_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.fm4");
pub const FM4_STATE_VERSION: u32 = 1;

const OSCILLATOR_FIELDS: usize = OPERATORS * 3;
const ROUTING_FIELDS: usize = OPERATORS * OPERATORS;
const CARRIER_FIELDS: usize = OPERATORS;
const ENVELOPE_FIELDS: usize = 4;
const FIELD_COUNT: usize = OSCILLATOR_FIELDS + ROUTING_FIELDS + CARRIER_FIELDS + ENVELOPE_FIELDS;
const EDITABLE_PARAMETER_COUNT: usize = FIELD_COUNT - OPERATORS;
const PAYLOAD_BYTES: usize = FIELD_COUNT * std::mem::size_of::<f32>();
const PHASE_MAX: f32 = f32::from_bits(0x3f7f_ffff);

/// Static generic parameter IDs are stable within the FM4 definition.
/// Oscillator fields are ratio/level/phase in operator order, followed by
/// source-major routing depths, carrier weights, then attack/decay/sustain/release.
pub mod parameter {
    use hex_lattice_contract::ParameterId;

    pub const fn oscillator_ratio(operator: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static((operator - 1) * 3 + 1)
    }

    pub const fn oscillator_level(operator: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static((operator - 1) * 3 + 2)
    }

    pub const fn oscillator_phase(operator: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static((operator - 1) * 3 + 3)
    }

    /// Returns a parameter ID for a directed connection; self-feedback is reserved.
    /// The zero ID sentinel denotes an invalid or reserved endpoint pair.
    pub const fn modulation(source: u32, target: u32) -> ParameterId {
        if source == 0 || source > 4 || target == 0 || target > 4 || source == target {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(12 + (source - 1) * 4 + target)
    }

    pub const fn carrier(operator: u32) -> ParameterId {
        if operator == 0 || operator > 4 {
            return ParameterId::from_static(0);
        }
        ParameterId::from_static(28 + operator)
    }

    pub const ATTACK: ParameterId = ParameterId::from_static(33);
    pub const DECAY: ParameterId = ParameterId::from_static(34);
    pub const SUSTAIN: ParameterId = ParameterId::from_static(35);
    pub const RELEASE: ParameterId = ParameterId::from_static(36);
}

const fn float_parameter(
    id: u32,
    label: &'static str,
    min: f32,
    max: f32,
    unit: Option<&'static str>,
    default: f32,
) -> ParameterDescriptor {
    ParameterDescriptor {
        id: ParameterId::from_static(id),
        label,
        kind: ParameterKind::Float { min, max, unit },
        default: ParameterValue::Float(default),
        presentation: ParameterPresentation::Linear,
    }
}

// The first 28 descriptors define the original shared oscillator/routing
// domains. Operator's independent control codec reuses those exact domains.
pub(crate) static PARAMETERS: [ParameterDescriptor; EDITABLE_PARAMETER_COUNT] = [
    float_parameter(1, "op1 ratio", 1.0 / 128.0, 128.0, Some("x"), 1.0),
    float_parameter(2, "op1 level", 0.0, 8.0, Some("linear"), 1.0),
    float_parameter(3, "op1 phase", 0.0, PHASE_MAX, Some("turns"), 0.0),
    float_parameter(4, "op2 ratio", 1.0 / 128.0, 128.0, Some("x"), 2.0),
    float_parameter(5, "op2 level", 0.0, 8.0, Some("linear"), 1.0),
    float_parameter(6, "op2 phase", 0.0, PHASE_MAX, Some("turns"), 0.0),
    float_parameter(7, "op3 ratio", 1.0 / 128.0, 128.0, Some("x"), 3.0),
    float_parameter(8, "op3 level", 0.0, 8.0, Some("linear"), 1.0),
    float_parameter(9, "op3 phase", 0.0, PHASE_MAX, Some("turns"), 0.0),
    float_parameter(10, "op4 ratio", 1.0 / 128.0, 128.0, Some("x"), 4.0),
    float_parameter(11, "op4 level", 0.0, 8.0, Some("linear"), 1.0),
    float_parameter(12, "op4 phase", 0.0, PHASE_MAX, Some("turns"), 0.0),
    float_parameter(14, "op1 to op2", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(15, "op1 to op3", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(16, "op1 to op4", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(17, "op2 to op1", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(19, "op2 to op3", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(20, "op2 to op4", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(21, "op3 to op1", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(22, "op3 to op2", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(24, "op3 to op4", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(25, "op4 to op1", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(26, "op4 to op2", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(27, "op4 to op3", -8.0, 8.0, Some("turns"), 0.0),
    float_parameter(29, "op1 carrier", -1.0, 1.0, Some("linear"), 1.0),
    float_parameter(30, "op2 carrier", -1.0, 1.0, Some("linear"), 0.0),
    float_parameter(31, "op3 carrier", -1.0, 1.0, Some("linear"), 0.0),
    float_parameter(32, "op4 carrier", -1.0, 1.0, Some("linear"), 0.0),
    float_parameter(33, "attack", 0.0, 120.0, Some("s"), 0.005),
    float_parameter(34, "decay", 0.0, 120.0, Some("s"), 0.1),
    float_parameter(35, "sustain", 0.0, 1.0, None, 0.7),
    float_parameter(36, "release", 0.0, 120.0, Some("s"), 0.1),
];

#[derive(Debug, Clone, Copy, PartialEq)]
struct Fm4Settings {
    oscillators: [[f32; 3]; OPERATORS],
    modulation: [[f32; OPERATORS]; OPERATORS],
    carriers: [f32; OPERATORS],
    envelope: [f32; ENVELOPE_FIELDS],
}

impl Fm4Settings {
    fn defaults() -> Self {
        let mut settings = Self {
            oscillators: [[1.0, 1.0, 0.0]; OPERATORS],
            modulation: [[0.0; OPERATORS]; OPERATORS],
            carriers: [0.0; OPERATORS],
            envelope: [0.005, 0.1, 0.7, 0.1],
        };
        for (index, oscillator) in settings.oscillators.iter_mut().enumerate() {
            oscillator[0] = (index + 1) as f32;
        }
        settings.carriers[0] = 1.0;
        settings
    }

    fn from_state(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != FM4_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != FM4_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let bytes = state.payload();
        if bytes.len() != PAYLOAD_BYTES {
            return Err(DeviceContractError::MalformedState);
        }
        let mut fields = [0.0_f32; FIELD_COUNT];
        for (index, chunk) in bytes.chunks_exact(4).enumerate() {
            let Some(field) = fields.get_mut(index) else {
                return Err(DeviceContractError::MalformedState);
            };
            *field = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        let mut settings = Self::defaults();
        let mut cursor = 0;
        for oscillator in &mut settings.oscillators {
            for field in oscillator {
                *field = fields[cursor];
                cursor += 1;
            }
        }
        for source in &mut settings.modulation {
            for depth in source {
                *depth = fields[cursor];
                cursor += 1;
            }
        }
        for carrier in &mut settings.carriers {
            *carrier = fields[cursor];
            cursor += 1;
        }
        settings.envelope.copy_from_slice(&fields[cursor..]);
        settings.validate()?;
        Ok(settings)
    }

    fn validate(self) -> Result<(), DeviceContractError> {
        if self
            .modulation
            .iter()
            .enumerate()
            .any(|(operator, row)| row[operator] != 0.0)
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        self.kernel_parts().map(|_| ())
    }

    fn kernel_parts(
        self,
    ) -> Result<([Oscillator; OPERATORS], Routing, Envelope), DeviceContractError> {
        let oscillators = [
            Oscillator::new(
                f64::from(self.oscillators[0][0]),
                f64::from(self.oscillators[0][1]),
                f64::from(self.oscillators[0][2]),
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)?,
            Oscillator::new(
                f64::from(self.oscillators[1][0]),
                f64::from(self.oscillators[1][1]),
                f64::from(self.oscillators[1][2]),
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)?,
            Oscillator::new(
                f64::from(self.oscillators[2][0]),
                f64::from(self.oscillators[2][1]),
                f64::from(self.oscillators[2][2]),
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)?,
            Oscillator::new(
                f64::from(self.oscillators[3][0]),
                f64::from(self.oscillators[3][1]),
                f64::from(self.oscillators[3][2]),
            )
            .map_err(|_| DeviceContractError::InvalidParameterValue)?,
        ];
        let modulation = self.modulation.map(|row| row.map(f64::from));
        let carriers = self.carriers.map(f64::from);
        let routing = Routing::new(modulation, carriers)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let envelope = Envelope::new(
            f64::from(self.envelope[0]),
            f64::from(self.envelope[1]),
            f64::from(self.envelope[2]),
            f64::from(self.envelope[3]),
        )
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        Ok((oscillators, routing, envelope))
    }

    fn to_state(self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let mut bytes = Vec::with_capacity(PAYLOAD_BYTES);
        for oscillator in self.oscillators {
            for field in oscillator {
                bytes.extend_from_slice(&field.to_le_bytes());
            }
        }
        for source in self.modulation {
            for depth in source {
                bytes.extend_from_slice(&depth.to_le_bytes());
            }
        }
        for carrier in self.carriers {
            bytes.extend_from_slice(&carrier.to_le_bytes());
        }
        for stage in self.envelope {
            bytes.extend_from_slice(&stage.to_le_bytes());
        }
        NativeDeviceState::new(FM4_TYPE_ID, FM4_STATE_VERSION, bytes)
    }

    fn read_parameter(self, id: ParameterId) -> Result<ParameterValue, DeviceContractError> {
        let value = match id.value() {
            1..=12 => {
                let offset = (id.value() - 1) as usize;
                self.oscillators[offset / 3][offset % 3]
            }
            13..=28 => {
                let offset = (id.value() - 13) as usize;
                if offset / OPERATORS == offset % OPERATORS {
                    return Err(DeviceContractError::UnknownParameter);
                }
                self.modulation[offset / OPERATORS][offset % OPERATORS]
            }
            29..=32 => self.carriers[(id.value() - 29) as usize],
            33..=36 => self.envelope[(id.value() - 33) as usize],
            _ => return Err(DeviceContractError::UnknownParameter),
        };
        Ok(ParameterValue::Float(value))
    }

    fn patch_parameter(
        mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let descriptor = PARAMETERS
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(value)?;
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        match id.value() {
            1..=12 => {
                let offset = (id.value() - 1) as usize;
                self.oscillators[offset / 3][offset % 3] = value;
            }
            13..=28 => {
                let offset = (id.value() - 13) as usize;
                self.modulation[offset / OPERATORS][offset % OPERATORS] = value;
            }
            29..=32 => self.carriers[(id.value() - 29) as usize] = value,
            33..=36 => self.envelope[(id.value() - 33) as usize] = value,
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        self.to_state()
    }
}

/// Original FM4 native instrument definition admitted by the built-in registry.
#[derive(Debug, Clone, Copy, Default)]
pub struct Fm4Definition;

pub static FM4_DEFINITION: Fm4Definition = Fm4Definition;

impl NativeDeviceMetadata for Fm4Definition {
    fn type_id(&self) -> DeviceTypeId {
        FM4_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("fm4")
    }

    fn state_version(&self) -> u32 {
        FM4_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &PARAMETERS
    }

    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<Option<hex_lattice_contract::ParameterSemantics>, DeviceContractError> {
        Fm4Settings::from_state(state)?.read_parameter(id)?;
        use hex_lattice_contract::{
            ParameterAccess, ParameterAutomation, ParameterChangeImpact, ParameterModulation,
            ParameterNormalization, ParameterSemantics, ParameterSmoothing, ParameterStepping,
        };
        Ok((29..=32)
            .contains(&id.value())
            .then_some(ParameterSemantics {
                access: ParameterAccess::ReadWrite,
                normalization: ParameterNormalization::Linear,
                stepping: ParameterStepping::Continuous,
                automation: ParameterAutomation::DeviceArrangementEnvelope,
                modulation: ParameterModulation::Undeclared,
                smoothing: ParameterSmoothing::DeviceManaged,
                change_impact: ParameterChangeImpact::ValueOnly,
            }))
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Fm4Settings::defaults().to_state()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Fm4Settings::from_state(state)?;
        Ok(state.clone())
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Fm4Settings::from_state(state)?.read_parameter(parameter)
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Fm4Settings::from_state(state)?.patch_parameter(parameter, value)
    }
}

impl NativeInstrumentDefinition for Fm4Definition {
    fn resource_factory_inline_bound(&self) -> Option<usize> {
        Some(std::mem::size_of::<runtime::Factory>())
    }

    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        let tuning = context.tuning;
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let settings = Fm4Settings::from_state(state)?;
        let (oscillators, routing, envelope) = settings.kernel_parts()?;
        let _ = crate::shared_sine_table();
        let runtime = runtime::Factory::prepare(state, context)?;
        Ok(Box::new(PreparedFm4Instrument {
            tuning: *tuning,
            sample_rate,
            oscillators,
            routing,
            envelope,
            runtime,
        }))
    }
}

/// Inline note preparation remains available; registered hosts use the same
/// bounded mutable factory for Arrangement, Session and live input. The existing
/// graph transaction owns voice history and prepared retarget proofs.
struct PreparedFm4Instrument {
    runtime: runtime::Factory,
    tuning: hex_lattice::TuningTable,
    sample_rate: u32,
    oscillators: [Oscillator; OPERATORS],
    routing: Routing,
    envelope: Envelope,
}

impl PreparedNativeInstrument for PreparedFm4Instrument {
    fn validate_event_note(&self, pitch: u8, velocity: u8) -> Result<(), DeviceContractError> {
        self.runtime.validate_note_on(pitch, velocity)
    }

    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>>, DeviceContractError>
    {
        Ok(Some(Box::new(self.runtime)))
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedAnalyticNote, DeviceContractError> {
        let frequency = self
            .tuning
            .frequency(pitch)
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        let prepared = PreparedFm4::new_with_frequency(
            self.sample_rate,
            frequency,
            velocity,
            self.oscillators,
            self.routing,
            self.envelope,
        )
        .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        Ok(prepared.into_analytic_note())
    }
}

#[cfg(test)]
mod tests;
