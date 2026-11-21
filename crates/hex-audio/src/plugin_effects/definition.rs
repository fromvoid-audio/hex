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
/// resources through the endpoint's actual return. Private offline preparation
/// may borrow an actual stopped lease and reserved epoch; it must complete that
/// epoch before rendering. Neither view grants live Root source admission.
/// Intentional Off instead retains one exact authored-state declaration without
/// borrowing an SDK owner; it cannot construct active processing or an endpoint.
pub struct NativeEffectDefinition<'a> {
    // Borrow only immutable configuration across pure compiler threads. The
    // lifetime retains each original owner; joining workers are not Sync.
    effects: Vec<&'a Configuration>,
    canonical: &'a [CapturedCanonicalBases],
    unavailable: Vec<(ProcessorInstanceIdentity, &'a NativeDeviceState)>,
    powered_off: Option<(ProcessorInstanceIdentity, u32, &'a NativeDeviceState)>,
    retained_effect: Option<RetainedTrackEffectSource>,
}

/// Exact logical Track destination of an original physical effect binding.
/// Root verifies the worker and configuration for each scoped compiler borrow.
#[derive(Clone, Copy, Debug)]
pub struct RetainedTrackEffectSource {
    pub(super) predecessor: crate::prepared::AppliedAudioSource,
    original: ProcessorInstanceIdentity,
    receiver: ProcessorInstanceIdentity,
    binding: Binding,
}

impl RetainedTrackEffectSource {
    pub fn for_track_move(
        predecessor: crate::prepared::AppliedAudioSource,
        physical: u64,
        receiver: u64,
        device: u64,
        binding: Binding,
    ) -> Result<Self, DeviceContractError> {
        if physical == receiver
            || !binding.is_valid()
            || binding.instance != device
            || binding.root != predecessor.output_identity
        {
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        Ok(Self {
            predecessor,
            original: ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(physical), device)?,
            receiver: ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(receiver), device)?,
            binding,
        })
    }

    pub fn binding(self) -> Binding {
        self.binding
    }
}

/// One exact preserved device with no native processing source. This grants no
/// source, factory, endpoint or ACK; only identity-matrix placeholder rendering.
#[derive(Clone, Copy)]
pub struct UnavailableEffect<'a> {
    pub owner: hex_lattice::ProcessorChainOwner,
    pub device_id: u64,
    pub state: &'a NativeDeviceState,
}
/// Exact authored Main metadata selected for an intentionally disabled stage.
/// No source, factory, endpoint, SDK owner or publication authority is borrowed.
#[derive(Clone, Copy)]
pub struct PoweredOffEffect<'a> {
    pub owner: hex_lattice::ProcessorChainOwner,
    pub device_id: u64,
    pub sample_rate: u32,
    pub state: &'a NativeDeviceState,
}

impl<'a> NativeEffectDefinition<'a> {
    /// Prepare one resource-free Main or Return Off declaration. The output owner must
    /// separately prove the exact source, supported stopped or running timing
    /// policy, and graph installation through ordinary admission.
    pub fn powered_off(entries: &[PoweredOffEffect<'a>]) -> Result<Self, DeviceContractError> {
        let [entry] = entries else {
            return Err(DeviceContractError::InvalidId);
        };
        if !matches!(
            entry.owner,
            ProcessorChainOwner::Main | ProcessorChainOwner::Return(_)
        ) {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        if entry.sample_rate == 0 {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        entry.state.require_asset_free_processing()?;
        hex_plugin_state::decode(entry.state).map_err(|_| DeviceContractError::MalformedState)?;
        let identity = ProcessorInstanceIdentity::new(entry.owner, entry.device_id)?;
        let mut definition = Self::from_selected(std::iter::empty())?;
        definition.powered_off = Some((identity, entry.sample_rate, entry.state));
        Ok(definition)
    }

    fn is_powered_off(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        rate: u32,
    ) -> bool {
        self.powered_off
            .is_some_and(|(id, sample_rate, preserved)| {
                id == identity && sample_rate == rate && state.processing_eq(preserved)
            })
    }

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
            || self.powered_off.is_some()
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
            canonical: self.canonical,
            unavailable,
            powered_off: None,
            retained_effect: self.retained_effect,
        })
    }
    /// Borrow actual zero-PCM private offline leases and their reserved epochs.
    /// Their original owners must complete those epochs before any rendering;
    /// this definition cannot admit sources into a live Root.
    pub(super) fn from_offline_preparations(
        effects: &'a [super::OfflineEffectPreparation],
    ) -> Result<Self, DeviceContractError> {
        let mut selected = Vec::new();
        selected
            .try_reserve_exact(effects.len().min(hex_lattice::MAX_STATEFUL_PROCESSORS))
            .map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?;
        for effect in effects {
            let configuration = effect.configuration()?;
            Self::push_configuration(&mut selected, configuration)?;
        }
        Ok(Self {
            effects: selected,
            canonical: &[],
            unavailable: Vec::new(),
            powered_off: None,
            retained_effect: None,
        })
    }
    fn push_configuration(
        selected: &mut Vec<&'a Configuration>,
        configuration: &'a Configuration,
    ) -> Result<(), DeviceContractError> {
        if selected.len() == hex_lattice::MAX_STATEFUL_PROCESSORS {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        if selected
            .iter()
            .any(|old| old.identity.device_id() == configuration.identity.device_id())
            || selected.first().is_some_and(|first| {
                first.binding.root != configuration.binding.root
                    || first.sample_rate != configuration.sample_rate
                    || first.prepared.quantum_frames() != configuration.prepared.quantum_frames()
            })
        {
            return Err(DeviceContractError::InvalidId);
        }
        selected
            .try_reserve(1)
            .map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?;
        selected.push(configuration);
        Ok(())
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
    pub(crate) fn including_canonical_bases(
        mut self,
        facts: &'a [CapturedCanonicalBases],
    ) -> Result<Self, DeviceContractError> {
        if facts.len() > self.effects.len() || !self.canonical.is_empty() {
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        for (index, current) in facts.iter().enumerate() {
            let effect = self
                .effects
                .iter()
                .copied()
                .find(|effect| effect.binding == current.binding())
                .ok_or(DeviceContractError::ForeignProcessingUnavailable)?;
            if current.prepared() != effect.prepared
                || current.sample_rate() != effect.sample_rate
                || !effect
                    .state_verifier
                    .is_some_and(|v| v.approves_processing(current.canonical_state()))
                || !effect.authored.same_parameters(&current.capabilities)
            {
                return Err(DeviceContractError::ForeignProcessingUnavailable);
            }
            // Duplicate bindings cannot choose between different canonical bases.
            if facts[..index]
                .iter()
                .any(|old| old.binding() == current.binding())
            {
                return Err(DeviceContractError::ForeignProcessingUnavailable);
            }
        }
        self.canonical = facts;
        Ok(self)
    }

    fn canonical_for(
        &self,
        effect: &Configuration,
        state: &NativeDeviceState,
    ) -> Result<Option<&CapturedCanonicalBases>, DeviceContractError> {
        if state.processing_eq(&effect.state) {
            return Ok(None);
        }
        if !effect.state_verifier.is_some_and(|v| v.approves(state)) {
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        self.canonical
            .iter()
            .find(|facts| {
                facts.binding() == effect.binding && facts.canonical_state().processing_eq(state)
            })
            .map(Some)
            .ok_or(DeviceContractError::ForeignProcessingUnavailable)
    }

    fn exact_configuration(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        rate: u32,
    ) -> Result<Option<&Configuration>, DeviceContractError> {
        if let Some(effect) = self
            .effects
            .iter()
            .copied()
            .find(|effect| effect.identity == identity)
        {
            if effect.sample_rate != rate {
                return Err(DeviceContractError::ForeignProcessingUnavailable);
            }
            self.canonical_for(effect, state)?;
            return Ok(Some(effect));
        }
        if self.is_powered_off(identity, state, rate) {
            return Ok(None);
        }
        if self
            .unavailable
            .iter()
            .any(|(missing, preserved)| *missing == identity && state.processing_eq(preserved))
        {
            return Ok(None);
        }
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }

    /// Pure exact-source eligibility for a proved 512-frame passive Off cut.
    /// This borrows constructed metadata; it neither dispatches nor retires DSP.
    pub fn matches_power_off_on_control(
        &self,
        owner: ProcessorChainOwner,
        binding: Binding,
        device: u64,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<bool, DeviceContractError> {
        if !matches!(
            owner,
            ProcessorChainOwner::Main | ProcessorChainOwner::Return(_)
        ) {
            return Ok(false);
        }
        let identity = ProcessorInstanceIdentity::new(owner, device)?;
        Ok(self
            .exact_configuration(identity, state, sample_rate)?
            .is_some_and(|effect| {
                let prepared = effect.prepared;
                prepared.binding() == Some(binding)
                    && !prepared.is_instrument_profile()
                    && prepared.quantum_frames() == 256
                    && prepared.bridge_latency_frames() == 512
                    && prepared.processor_latency_frames() == 0
            }))
    }

    pub(super) fn including_retained_effect_source(
        mut self,
        retained: RetainedTrackEffectSource,
    ) -> Result<Self, DeviceContractError> {
        if !self.effects.iter().any(|effect| {
            effect.identity == retained.original
                && effect.binding == retained.binding
                && supports_retained_track_effect(effect)
        }) {
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        self.retained_effect = Some(retained);
        Ok(self)
    }

    /// Observe the one directly movable profile in the exact Root selection.
    pub fn matches_track_effect_move_on_control(
        &self,
        original: ProcessorInstanceIdentity,
        binding: Binding,
        state: &NativeDeviceState,
        rate: u32,
    ) -> bool {
        self.exact_configuration(original, state, rate)
            .ok()
            .flatten()
            .is_some_and(|effect| {
                effect.binding == binding
                    && effect.state.processing_eq(state)
                    && supports_retained_track_effect(effect)
            })
    }

    /// Lend one exact original effect configuration to a scoped compiler.
    pub(crate) fn including_retained_track_effect(
        &self,
        predecessor: crate::prepared::AppliedAudioSource,
        receiver: ProcessorInstanceIdentity,
        original: &hex_lattice::PreparedRuntimeInstance,
    ) -> Result<Self, DeviceContractError> {
        let unavailable = DeviceContractError::ForeignProcessingUnavailable;
        let identity = ProcessorInstanceIdentity::new(original.owner, original.device_id)?;
        let prepared = original.foreign_processor().ok_or(unavailable)?;
        let binding = prepared.binding().ok_or(unavailable)?;
        let [configuration] = self.effects.as_slice() else {
            return Err(unavailable);
        };
        if !binding.is_valid()
            || binding.instance != identity.device_id()
            || receiver.device_id() != identity.device_id()
            || receiver.owner() == identity.owner()
            || !matches!(receiver.owner(), ProcessorChainOwner::Track(_))
            || !matches!(identity.owner(), ProcessorChainOwner::Track(_))
            || prepared.is_instrument_profile()
            || original.bypassed
            || !original.automation.is_empty()
            || original.modulation.is_some()
            || configuration.identity != identity
            || configuration.binding != binding
            || configuration.prepared != prepared
            || configuration.sample_rate != original.sample_rate
            || original
                .configuration
                .as_ref()
                .is_none_or(|state| !configuration.state.processing_eq(state))
        {
            return Err(unavailable);
        }
        let mut retained = Self::from_selected([(*configuration, true)])?;
        retained.retained_effect = Some(RetainedTrackEffectSource {
            predecessor,
            original: identity,
            receiver,
            binding,
        });
        Ok(retained)
    }

    /// Copy the processing declaration from a real source that matches this
    /// exact authored state and rate. This does not loan or publish an endpoint.
    pub fn prepared_processor_on_control(
        &self,
        node: &hex_lattice::PreparedRuntimeInstance,
    ) -> Result<Option<hex_lattice::PreparedForeignProcessor>, DeviceContractError> {
        let identity = ProcessorInstanceIdentity::new(node.owner, node.device_id)?;
        let state = node
            .configuration
            .as_deref()
            .ok_or(DeviceContractError::ForeignProcessingUnavailable)?;
        Ok(self
            .exact_configuration(identity, state, node.sample_rate)?
            .map(|effect| effect.prepared))
    }

    pub(super) fn from_selected(
        effects: impl IntoIterator<Item = (&'a Configuration, bool)>,
    ) -> Result<Self, DeviceContractError> {
        let mut selected: Vec<&'a Configuration> = Vec::new();
        for (configuration, paired) in effects {
            if !paired {
                return Err(DeviceContractError::InvalidId);
            }
            Self::push_configuration(&mut selected, configuration)?;
        }
        Ok(Self {
            effects: selected,
            canonical: &[],
            unavailable: Vec::new(),
            powered_off: None,
            retained_effect: None,
        })
    }
}

fn supports_retained_track_effect(effect: &Configuration) -> bool {
    let profile = effect.prepared;
    !profile.is_instrument_profile()
        && profile.quantum_frames() == 256
        && profile.bridge_latency_frames() == 512
        && profile.processor_latency_frames() == 0
        && effect.dense_raw_id.is_none()
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
    fn retained_foreign_effect_identity(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<ProcessorInstanceIdentity, DeviceContractError> {
        let Some(route) = self
            .retained_effect
            .filter(|route| route.receiver == identity)
        else {
            return Ok(identity);
        };
        self.exact_configuration(route.original, state, context.sample_rate)?
            .filter(|effect| effect.binding == route.binding)
            .map(|effect| effect.identity)
            .ok_or(DeviceContractError::ForeignProcessingUnavailable)
    }

    fn prepare_instance_state(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        self.migrate_state(state)?;
        match self.exact_configuration(identity, state, context.sample_rate)? {
            Some(effect) => self
                .canonical_for(effect, state)?
                .map_or(&effect.state, CapturedCanonicalBases::canonical_state)
                .clone()
                .with_host_metadata_from(state),
            None => Ok(state.clone()),
        }
    }

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
        if self.is_powered_off(identity, state, context.sample_rate) {
            return Ok(Some(PreparedProcessorFactory::PoweredOff));
        }
        Ok(self
            .exact_configuration(identity, state, context.sample_rate)?
            .map(|effect| PreparedProcessorFactory::Foreign(effect.prepared)))
    }
    fn foreign_automation_source(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<hex_lattice::ForeignAutomationSource<'_>>, DeviceContractError> {
        let Some(effect) = self.exact_configuration(identity, state, context.sample_rate)? else {
            return Ok(None);
        };
        let capabilities = self
            .canonical_for(effect, state)?
            .map_or(&effect.authored, |facts| &facts.capabilities);
        Ok(capabilities
            .view(
                effect.binding,
                effect.sample_rate,
                effect.prepared.quantum_frames(),
                effect.program_revision,
            )
            .map(|mut source| {
                source.dense_parameter = effect.dense_raw_id;
                source
            }))
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

#[cfg(test)]
mod instrument_selection_tests;
#[cfg(test)]
mod powered_off_tests;
