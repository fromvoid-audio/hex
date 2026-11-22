//! Validated immutable native device registry for lookup, state validation and preparation.
use crate::{
    DevicePreparationContext, NativeDeviceDefinition, PreparedNativeDevice,
    PreparedNativeProcessor, PreparedRuntimeDevice, ProcessorInstanceIdentity, StereoMatrix,
    parameter_automation,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, MAX_NATIVE_DEVICE_TYPES, NativeDeviceMetadata,
    NativeDeviceState,
};

/// Borrowed immutable device registry. Duplicate type IDs are rejected at
/// construction so resolution remains a bounded linear scan over built-ins.
pub struct NativeDeviceRegistry<'a> {
    definitions: &'a [&'a dyn NativeDeviceDefinition],
}
impl<'a> NativeDeviceRegistry<'a> {
    pub fn new(
        definitions: &'a [&'a dyn NativeDeviceDefinition],
    ) -> Result<Self, DeviceContractError> {
        if definitions.len() > MAX_NATIVE_DEVICE_TYPES {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        for (index, definition) in definitions.iter().enumerate() {
            NativeDeviceMetadata::type_id(*definition).validate()?;
            if definition.display_name().is_some_and(|name| {
                name.is_empty()
                    || name.len() > 128
                    || name.trim() != name
                    || name
                        .chars()
                        .any(|character| character.is_control() || character.is_uppercase())
            }) {
                return Err(DeviceContractError::InvalidDescriptor);
            }
            if definition.state_version() == 0 {
                return Err(DeviceContractError::InvalidStateVersion);
            }
            if definitions[..index].iter().any(|prior| {
                NativeDeviceMetadata::type_id(*prior) == NativeDeviceMetadata::type_id(*definition)
            }) {
                return Err(DeviceContractError::DuplicateDeviceType);
            }
            hex_lattice_contract::validate_parameter_descriptors(definition.descriptors())?;
        }
        Ok(Self { definitions })
    }
    pub fn definition(&self, type_id: DeviceTypeId) -> Option<&'a dyn NativeDeviceDefinition> {
        self.definitions
            .iter()
            .copied()
            .find(|definition| NativeDeviceMetadata::type_id(*definition) == type_id)
    }
    pub fn definitions(&self) -> &'a [&'a dyn NativeDeviceDefinition] {
        self.definitions
    }
    pub fn default_state(
        &self,
        type_id: DeviceTypeId,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(type_id)
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let state = definition.default_state()?;
        self.validate_current(
            definition,
            definition
                .migrate_state(&state)?
                .with_host_metadata_from(&state)?,
        )
    }
    pub fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )
    }
    /// Read an exact hosted effect route from the selected runtime definition.
    pub fn retained_foreign_effect_identity(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<ProcessorInstanceIdentity, DeviceContractError> {
        context.validate_for(state)?;
        self.definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?
            .retained_foreign_effect_identity(identity, state, context)
    }

    /// Validate instance-bound runtime state without changing canonical state.
    pub fn prepare_instance_state(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        state.require_asset_free_processing()?;
        context.validate_for(state)?;
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        self.validate_current(
            definition,
            definition
                .prepare_instance_state(identity, state, context)?
                .with_host_metadata_from(state)?,
        )
    }
    pub fn patch_parameters(
        &self,
        state: &NativeDeviceState,
        snapshot: &hex_lattice_contract::DeviceParameterSnapshot,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        hex_lattice_contract::validate_parameter_snapshot_for_write(
            definition, &current, snapshot,
        )?;
        let patched = definition.patch_parameters(&current, snapshot)?;
        self.validate_current(
            definition,
            definition
                .migrate_state(&patched)?
                .with_host_metadata_from(state)?,
        )
    }

    pub fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        hex_lattice_contract::validate_parameter_write(definition, &current, parameter, value)?;
        let patched = definition.patch_parameter(&current, parameter, value)?;
        self.validate_current(
            definition,
            definition
                .migrate_state(&patched)?
                .with_host_metadata_from(state)?,
        )
    }
    pub fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        state.require_asset_free_processing()?;
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        definition.prepare_matrix(&current)
    }
    pub fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        state.require_asset_free_processing()?;
        context.validate_for(state)?;
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        definition.prepare_processor(&current, context)
    }
    pub fn prepare_device(
        &self,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
    ) -> Result<PreparedNativeDevice, DeviceContractError> {
        state.require_asset_free_processing()?;
        context.validate_for(state)?;
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        Ok(PreparedNativeDevice {
            matrix: definition.prepare_effect_matrix(&current)?,
            processor: definition.prepare_processor(&current, context)?,
        })
    }
    pub fn prepare_runtime_device(
        &self,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
    ) -> Result<PreparedRuntimeDevice, DeviceContractError> {
        self.prepare_runtime_device_for(None, state, context)
    }
    /// Prepare an actual canonical instance; codec and resource validation are
    /// identical to class-only preparation. Instance-bound factories can
    /// select their exact control-prepared facts without a type/state cache.
    pub fn prepare_runtime_instance(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
    ) -> Result<PreparedRuntimeDevice, DeviceContractError> {
        self.prepare_runtime_device_for(Some(identity), state, context)
    }
    pub fn prepare_runtime_instance_with_automation(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        parameters: &[hex_lattice_contract::ParameterId],
    ) -> Result<PreparedRuntimeDevice, DeviceContractError> {
        if parameters.is_empty() {
            return self.prepare_runtime_instance(identity, state, context);
        }
        if parameters.len() > parameter_automation::MAX_AUTOMATION_LANES
            || parameters.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        state.require_asset_free_processing()?;
        context.validate_for(state)?;
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        for &parameter in parameters {
            parameter.validate()?;
            if !definition.supports_parameter_automation(&current, parameter)?
                || !hex_lattice_contract::validated_parameter_semantics(
                    definition, &current, parameter,
                )?
                .is_some_and(|semantics| {
                    semantics.access == hex_lattice_contract::ParameterAccess::ReadWrite
                        && semantics.automation
                            == hex_lattice_contract::ParameterAutomation::DeviceArrangementEnvelope
                })
            {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
        }
        let result = definition
            .prepare_automated_runtime_instance(identity, &current, context, parameters)?;
        if !result.matrix.is_identity()
            || result.processor.as_ref().is_none_or(|factory| {
                parameters
                    .iter()
                    .any(|id| !factory.supports_parameter_automation(*id))
            })
        {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        Ok(result)
    }
    fn prepare_runtime_device_for(
        &self,
        identity: Option<ProcessorInstanceIdentity>,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
    ) -> Result<PreparedRuntimeDevice, DeviceContractError> {
        state.require_asset_free_processing()?;
        context.validate_for(state)?;
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = match identity {
            Some(identity) => self.prepare_instance_state(identity, state, context)?,
            None => self.migrate_state(state)?,
        };
        Ok(PreparedRuntimeDevice {
            matrix: definition.prepare_effect_matrix(&current)?,
            processor: match identity {
                Some(identity) => {
                    definition.prepare_instance_runtime_factory(identity, &current, context)?
                }
                None => definition.prepare_runtime_factory(&current, context)?,
            },
        })
    }
    fn validate_current(
        &self,
        definition: &'a dyn NativeDeviceDefinition,
        state: NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        state.type_id().validate()?;
        if state.type_id() != definition.type_id() {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != definition.state_version() {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        // Validate the complete state-owned descriptor set once at this
        // boundary. Parameter values are checked when read, patched, or
        // presented; eagerly reading every value here would repeatedly decode
        // large device states (for example, once per Operator control).
        hex_lattice_contract::validated_state_descriptors(definition, &state)?;
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_rejects_unknown_device_lookup() {
        let registry = NativeDeviceRegistry::new(&[]).unwrap();
        let unknown = DeviceTypeId::new("hex.test.unknown").unwrap();
        assert!(registry.definition(unknown).is_none());
        assert!(registry.definitions().is_empty());
    }
}
