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
    unavailable: Vec<(ProcessorInstanceIdentity, &'a NativeDeviceState)>,
}

/// One exact preserved device with no native processing source. This grants no
/// source, factory, endpoint or ACK; only identity-matrix placeholder rendering.
#[derive(Clone, Copy)]
pub struct UnavailableEffect<'a> {
    pub owner: hex_lattice::ProcessorChainOwner,
    pub device_id: u64,
    pub state: &'a NativeDeviceState,
}
impl<'a> NativeEffectDefinition<'a> {
    pub(crate) fn unavailable(
        entries: &[UnavailableEffect<'a>],
    ) -> Result<Self, DeviceContractError> {
        Self::from_selected(std::iter::empty())?.including_unavailable(entries)
    }

    pub(crate) fn including_unavailable(
        &self,
        entries: &[UnavailableEffect<'a>],
    ) -> Result<Self, DeviceContractError> {
        if self
            .effects
            .len()
            .checked_add(entries.len())
            .is_none_or(|n| n > hex_lattice::MAX_STATEFUL_PROCESSORS)
            || !self.unavailable.is_empty()
        {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        let mut effects = Vec::new();
        effects
            .try_reserve_exact(self.effects.len())
            .map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?;
        effects.extend(self.effects.iter().copied());
        let mut unavailable: Vec<(ProcessorInstanceIdentity, &'a NativeDeviceState)> = Vec::new();
        unavailable
            .try_reserve_exact(entries.len())
            .map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?;
        for entry in entries {
            entry.state.require_asset_free_processing()?;
            hex_plugin_state::decode(entry.state)
                .map_err(|_| DeviceContractError::MalformedState)?;
            let identity = ProcessorInstanceIdentity::new(entry.owner, entry.device_id)?;
            if effects
                .iter()
                .any(|effect| effect.identity.device_id() == identity.device_id())
                || unavailable
                    .iter()
                    .any(|(old, _)| old.device_id() == identity.device_id())
            {
                return Err(DeviceContractError::InvalidId);
            }
            unavailable.push((identity, entry.state));
        }
        Ok(Self {
            effects,
            unavailable,
        })
    }
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
        Ok(Self {
            effects: selected,
            unavailable: Vec::new(),
        })
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
            .find(|effect| effect.identity == identity);
        let Some(effect) = effect else {
            if self
                .unavailable
                .iter()
                .any(|(missing, preserved)| *missing == identity && state.processing_eq(preserved))
            {
                return Ok(None);
            }
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        };
        if effect.sample_rate != context.sample_rate || !state.processing_eq(&effect.state) {
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        Ok(Some(PreparedProcessorFactory::Foreign(effect.prepared)))
    }
}

#[cfg(test)]
mod unavailable_tests {
    use super::*;

    #[test]
    fn unavailable_effect_is_exact_preserved_metadata_not_a_foreign_factory() {
        let state = hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [2; 32],
                    info_sha256: [3; 32],
                },
            },
            component: Some(&[4, 5]),
            controller: Some(&[]),
        })
        .unwrap();
        let original = state.clone();
        let owner = hex_lattice::ProcessorChainOwner::Main;
        let definition = NativeEffectDefinition::unavailable(&[UnavailableEffect {
            owner,
            device_id: 7,
            state: &state,
        }])
        .unwrap();
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
        let registry = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
        let context = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        };
        let identity = ProcessorInstanceIdentity::new(owner, 7).unwrap();
        let prepared = registry
            .prepare_runtime_instance(identity, &state, &context)
            .unwrap();
        assert!(prepared.matrix.is_identity() && prepared.processor.is_none());
        assert_eq!(registry.migrate_state(&state).unwrap(), original);
        assert!(
            registry
                .prepare_runtime_instance(
                    ProcessorInstanceIdentity::new(owner, 8).unwrap(),
                    &state,
                    &context,
                )
                .is_err()
        );
        assert!(registry.prepare_runtime_device(&state, &context).is_err());
        let unsupported =
            NativeDeviceState::new(hex_plugin_state::TYPE_ID, 2, state.payload().to_vec()).unwrap();
        assert!(
            NativeEffectDefinition::unavailable(&[UnavailableEffect {
                owner,
                device_id: 7,
                state: &unsupported
            },])
            .is_err()
        );
        assert_eq!(state, original);
    }
}
