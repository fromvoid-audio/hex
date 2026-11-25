//! Validated immutable native modulator registry for lookup, codec validation,
//! output preparation, and source-to-source target resolution.
//!
//! Mirrors `NativeDeviceRegistry` and `NativeInstrumentRegistry` so `hex-engine`
//! can prepare and route any registered built-in modulator without hardcoded
//! `hex.modulator.*` type-ID match tables.

use crate::{
    MAX_MODULATION_SOURCES, ModulationSourceId, PreparedModulationSource, SourceModulationParameter,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, ModulationOutputId, NativeDeviceState, ParameterId,
    modulator::MAX_NATIVE_MODULATOR_TYPES,
};

/// Control-side musical and sample-rate context supplied when preparing a
/// modulator's signal outputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModulatorPreparationContext {
    pub sample_rate: u32,
    pub fallback_bpm: f64,
    pub maximum_bpm: f64,
    pub instrument_events: bool,
}

/// One prepared signal output emitted by a modulator slot.
#[derive(Clone, Copy, Debug)]
pub struct ModulatorPreparedOutput {
    pub output: ModulationOutputId,
    pub source: PreparedModulationSource,
}

/// Registered built-in modulator codec and preparation definition.
pub trait NativeModulatorDefinition: Sync {
    /// Canonical stable modulator type identifier (`hex.modulator.*`).
    fn type_id(&self) -> DeviceTypeId;

    /// Lowercase human-readable display name.
    fn display_name(&self) -> &'static str;

    /// Number of device-local signal buffers occupied by an enabled instance of
    /// this modulator. Occurrence-owned inputs (such as note pressure) return 0.
    fn output_count(&self) -> usize {
        1
    }

    /// True when this modulator is owned per voice/occurrence by the instrument
    /// runtime rather than occupying a shared device-local bank signal buffer.
    fn is_occurrence_owned(&self) -> bool {
        false
    }

    /// True when assignments from this modulator retain their exact on/off
    /// identity range `[-1.0, 1.0]` rather than arbitrary remapped ranges.
    fn uses_identity_range(&self) -> bool {
        false
    }

    /// Default validated state envelope for fresh insertion.
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError>;

    /// Validate a persisted or edited modulator state envelope.
    fn validate_state(&self, state: &NativeDeviceState) -> Result<(), DeviceContractError>;

    /// Decode `state` and append its prepared signal outputs in stable output
    /// order to `out`. Occurrence-owned modulators validate `state` and append
    /// no bank outputs.
    fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), DeviceContractError>;

    /// Resolve a source-to-source modulation destination targeting `parameter`
    /// on this modulator type into its `(output_id, SourceModulationParameter)`.
    fn resolve_target_parameter(
        &self,
        _parameter: ParameterId,
        _target: Option<&PreparedModulationSource>,
    ) -> Option<(ModulationOutputId, SourceModulationParameter)> {
        None
    }
}

/// Borrowed immutable modulator registry. Duplicate type IDs and malformed
/// metadata are rejected at construction so lookup remains a bounded linear scan.
pub struct NativeModulatorRegistry<'a> {
    definitions: &'a [&'a dyn NativeModulatorDefinition],
}

impl<'a> NativeModulatorRegistry<'a> {
    pub fn new(
        definitions: &'a [&'a dyn NativeModulatorDefinition],
    ) -> Result<Self, DeviceContractError> {
        if definitions.len() > MAX_NATIVE_MODULATOR_TYPES {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        for (index, definition) in definitions.iter().enumerate() {
            definition.type_id().validate()?;
            let name = definition.display_name();
            if name.is_empty()
                || name.len() > 128
                || name.trim() != name
                || name
                    .chars()
                    .any(|character| character.is_control() || character.is_uppercase())
            {
                return Err(DeviceContractError::InvalidDescriptor);
            }
            if definition.output_count() > MAX_MODULATION_SOURCES {
                return Err(DeviceContractError::RuntimeBudgetExceeded);
            }
            if definition.is_occurrence_owned() && definition.output_count() != 0 {
                return Err(DeviceContractError::InvalidDescriptor);
            }
            if definitions[..index]
                .iter()
                .any(|prior| prior.type_id() == definition.type_id())
            {
                return Err(DeviceContractError::DuplicateDeviceType);
            }
        }
        Ok(Self { definitions })
    }

    pub fn definition(&self, type_id: DeviceTypeId) -> Option<&'a dyn NativeModulatorDefinition> {
        self.definitions
            .iter()
            .copied()
            .find(|definition| definition.type_id() == type_id)
    }

    pub fn definitions(&self) -> &'a [&'a dyn NativeModulatorDefinition] {
        self.definitions
    }

    pub fn output_count(&self, type_id: DeviceTypeId) -> Option<usize> {
        self.definition(type_id)
            .map(NativeModulatorDefinition::output_count)
    }

    pub fn is_occurrence_owned(&self, type_id: DeviceTypeId) -> bool {
        self.definition(type_id)
            .is_some_and(NativeModulatorDefinition::is_occurrence_owned)
    }

    pub fn uses_identity_range(&self, type_id: DeviceTypeId) -> bool {
        self.definition(type_id)
            .is_some_and(NativeModulatorDefinition::uses_identity_range)
    }

    pub fn default_state(
        &self,
        type_id: DeviceTypeId,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(type_id)
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let state = definition.default_state()?;
        definition.validate_state(&state)?;
        Ok(state)
    }

    pub fn validate_state(&self, state: &NativeDeviceState) -> Result<(), DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        definition.validate_state(state)
    }

    pub fn prepare_outputs(
        &self,
        id: ModulationSourceId,
        state: &NativeDeviceState,
        context: ModulatorPreparationContext,
        out: &mut Vec<ModulatorPreparedOutput>,
    ) -> Result<(), DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let before = out.len();
        definition.prepare_outputs(id, state, context, out)?;
        if out.len().saturating_sub(before) != definition.output_count() {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        Ok(())
    }

    pub fn resolve_target_parameter(
        &self,
        type_id: DeviceTypeId,
        parameter: ParameterId,
        target: Option<&PreparedModulationSource>,
    ) -> Option<(ModulationOutputId, SourceModulationParameter)> {
        self.definition(type_id)?
            .resolve_target_parameter(parameter, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyModulator {
        id: DeviceTypeId,
        name: &'static str,
    }

    impl NativeModulatorDefinition for DummyModulator {
        fn type_id(&self) -> DeviceTypeId {
            self.id
        }
        fn display_name(&self) -> &'static str {
            self.name
        }
        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(self.id, 1, vec![0; 4])
        }
        fn validate_state(&self, state: &NativeDeviceState) -> Result<(), DeviceContractError> {
            if state.type_id() != self.id || state.payload().len() != 4 {
                return Err(DeviceContractError::MalformedState);
            }
            Ok(())
        }
        fn prepare_outputs(
            &self,
            id: ModulationSourceId,
            state: &NativeDeviceState,
            context: ModulatorPreparationContext,
            out: &mut Vec<ModulatorPreparedOutput>,
        ) -> Result<(), DeviceContractError> {
            self.validate_state(state)?;
            out.push(ModulatorPreparedOutput {
                output: ModulationOutputId::MAIN,
                source: PreparedModulationSource::macro_value(id, context.sample_rate, 0.5)?,
            });
            Ok(())
        }
    }

    #[test]
    fn registry_rejects_duplicates_and_uppercase_names_and_prepares_outputs() {
        let a = DummyModulator {
            id: DeviceTypeId::from_static("hex.modulator.test-a"),
            name: "test a",
        };
        let dup = DummyModulator {
            id: DeviceTypeId::from_static("hex.modulator.test-a"),
            name: "test dup",
        };
        let upper = DummyModulator {
            id: DeviceTypeId::from_static("hex.modulator.test-b"),
            name: "Upper",
        };
        assert_eq!(
            NativeModulatorRegistry::new(&[&a, &dup]).err(),
            Some(DeviceContractError::DuplicateDeviceType)
        );
        assert_eq!(
            NativeModulatorRegistry::new(&[&upper]).err(),
            Some(DeviceContractError::InvalidDescriptor)
        );

        let defs: [&dyn NativeModulatorDefinition; 1] = [&a];
        let registry = NativeModulatorRegistry::new(&defs).unwrap();
        let state = registry.default_state(a.id).unwrap();
        let mut outputs = Vec::new();
        registry
            .prepare_outputs(
                ModulationSourceId::new(1).unwrap(),
                &state,
                ModulatorPreparationContext {
                    sample_rate: 48_000,
                    fallback_bpm: 120.0,
                    maximum_bpm: 120.0,
                    instrument_events: false,
                },
                &mut outputs,
            )
            .unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].output, ModulationOutputId::MAIN);
    }
}
