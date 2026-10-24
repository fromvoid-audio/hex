//! Actual instance-bound factory selection; metadata never admits a source.
use super::*;
use hex_lattice::{
    DevicePreparationContext, NativeDeviceDefinition, PreparedProcessorFactory, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceTypeId, NativeDeviceMetadata, ParameterDescriptor, ParameterId, ParameterValue,
};

/// Borrow actual constructor-paired owners, or Root's original worker while
/// its endpoint is loaned, during pure graph preparation. Copied IDs or equal
/// state cannot construct these instances. Root/publication retains the same
/// resources through the endpoint's actual return.
pub struct NativeEffectDefinition<'a> {
    // Borrow only immutable configuration across pure compiler threads. The
    // lifetime retains each original owner; joining workers are not Sync.
    effects: Vec<&'a Configuration>,
}
impl<'a> NativeEffectDefinition<'a> {
    pub fn new(effects: &'a [NativeEffect]) -> Result<Self, DeviceContractError> {
        if effects.len() > hex_lattice::MAX_STATEFUL_PROCESSORS {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        Self::from_effects(effects.iter())
    }
    /// Borrow actual owners selected from existing Root cells. Equal copied
    /// configurations cannot construct this view or substitute another source.
    pub(crate) fn from_effects(
        effects: impl IntoIterator<Item = &'a NativeEffect>,
    ) -> Result<Self, DeviceContractError> {
        Self::from_selected(effects.into_iter().map(|effect| {
            (
                &effect.callback.configuration,
                effect.callback.matches_worker(&effect.worker),
            )
        }))
    }
    pub(super) fn from_selected(
        effects: impl IntoIterator<Item = (&'a Configuration, bool)>,
    ) -> Result<Self, DeviceContractError> {
        let mut selected: Vec<&'a Configuration> = Vec::new();
        for (configuration, paired) in effects {
            if selected.len() == hex_lattice::MAX_STATEFUL_PROCESSORS {
                return Err(DeviceContractError::RuntimeBudgetExceeded);
            }
            if !paired
                || selected
                    .iter()
                    .any(|old| old.identity.device_id() == configuration.identity.device_id())
                || selected.first().is_some_and(|first| {
                    first.binding.root != configuration.binding.root
                        || first.sample_rate != configuration.sample_rate
                        || first.prepared.quantum_frames()
                            != configuration.prepared.quantum_frames()
                })
            {
                return Err(DeviceContractError::InvalidId);
            }
            selected
                .try_reserve(1)
                .map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?;
            selected.push(configuration);
        }
        Ok(Self { effects: selected })
    }
}
impl NativeDeviceMetadata for NativeEffectDefinition<'_> {
    fn type_id(&self) -> DeviceTypeId {
        hex_plugin_state::TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("vst3")
    }
    fn state_version(&self) -> u32 {
        hex_plugin_state::STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[] // Parameter delivery is a separate host contract; never fake controls.
    }
    fn descriptors_for_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<Vec<ParameterDescriptor>, DeviceContractError> {
        self.migrate_state(state)?;
        Ok(Vec::new())
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        state.require_asset_free_processing()?;
        hex_plugin_state::decode(state).map_err(|_| DeviceContractError::MalformedState)?;
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        _: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        self.migrate_state(state)?;
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        _: ParameterId,
        _: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        self.migrate_state(state)?;
        Err(DeviceContractError::UnknownParameter)
    }
}
impl NativeDeviceDefinition for NativeEffectDefinition<'_> {
    fn prepare_processor(
        &self,
        _: &NativeDeviceState,
        _: &DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedNativeProcessor>>, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        self.migrate_state(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_runtime_factory(
        &self,
        _: &NativeDeviceState,
        _: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }
    fn prepare_instance_runtime_factory(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        let effect = self
            .effects
            .iter()
            .find(|effect| effect.identity == identity)
            .ok_or(DeviceContractError::ForeignProcessingUnavailable)?;
        if effect.sample_rate != context.sample_rate || !state.processing_eq(&effect.state) {
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        Ok(Some(PreparedProcessorFactory::Foreign(effect.prepared)))
    }
}
