//! Prepared chain construction, metering, latency queries, and processor audition state.
use super::*;

impl NativeRuntimeChain {
    /// Bounded scalar observation of the actual retained foreign entries.
    /// May be used by their audio owner without allocation or ownership moves;
    /// it does not permit concurrent access or establish native source custody.
    pub fn foreign_bindings(
        &self,
    ) -> impl Iterator<Item = Option<hex_lattice_contract::ForeignProcessorBinding>> {
        self.entries
            .iter()
            .filter_map(|entry| match &entry.runtime {
                RuntimeOwnership::Foreign(value) => Some(value.prepared.binding()),
                _ => None,
            })
    }

    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            stateful_count: 0,
            declared_pair_bytes: 0,
        }
    }

    pub fn new(instances: Vec<PreparedProcessorInstance>) -> Result<Self, DeviceContractError> {
        Self::new_prepared_iter(instances.into_iter().map(PreparedRuntimeInstance::from))
    }

    pub fn new_prepared(
        instances: Vec<PreparedRuntimeInstance>,
    ) -> Result<Self, DeviceContractError> {
        Self::new_prepared_iter(instances.into_iter())
    }

    fn new_prepared_iter(
        instances: impl ExactSizeIterator<Item = PreparedRuntimeInstance>,
    ) -> Result<Self, DeviceContractError> {
        Self::from_prepared(instances.map(|instance| (instance, None)))
    }

    /// Build a source receiver's runtime on control from its retained native
    /// recipe. The recipe keeps its declaration; the exclusive runtime
    /// owns its automation program and any mutable modulation history.
    pub fn new_from_source_recipe(
        instances: &[PreparedRuntimeInstance],
    ) -> Result<Self, DeviceContractError> {
        if instances.iter().any(|instance| {
            !matches!(instance.owner, ProcessorChainOwner::Source(_))
                || instance.automation.foreign().is_some()
                || instance
                    .modulation
                    .as_ref()
                    .is_some_and(|modulation| modulation.foreign_effect().is_some())
        }) {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        Self::from_prepared(instances.iter().map(|instance| {
            let entry = PreparedRuntimeInstance {
                modulation: None,
                automation: instance.automation.clone(),
                meter: std::sync::Arc::clone(&instance.meter),
                owner: instance.owner,
                device_id: instance.device_id,
                device_type: instance.device_type,
                matrix: instance.matrix,
                bypassed: instance.bypassed,
                sample_rate: instance.sample_rate,
                configuration: instance.configuration.clone(),
                definition: instance.definition.clone(),
            };
            (entry, instance.modulation.as_ref())
        }))
    }

    fn from_prepared<'a>(
        instances: impl ExactSizeIterator<
            Item = (PreparedRuntimeInstance, Option<&'a PreparedModulationState>),
        >,
    ) -> Result<Self, DeviceContractError> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(instances.len())
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        let mut declared_pair_bytes = 0_usize;
        let mut stateful_count = 0_usize;
        let mut automation_bytes = 0usize;
        for (instance, borrowed_modulation) in instances {
            let prepared_modulation = borrowed_modulation.or(instance.modulation.as_ref());
            if let Some(prepared) = prepared_modulation {
                prepared.validate_sample_rate(instance.sample_rate)?;
            }
            automation_bytes = automation_bytes
                .checked_add(
                    instance
                        .automation
                        .storage_bytes()
                        .ok_or(DeviceContractError::RuntimeBudgetExceeded)?,
                )
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            if automation_bytes > MAX_PROCESSOR_BANK_RUNTIME_BYTES * 2 {
                return Err(DeviceContractError::RuntimeBudgetExceeded);
            }
            if let Some(configuration) = &instance.configuration {
                configuration.require_asset_free_processing()?;
            }
            if !instance.owner.is_valid() || instance.device_id == 0 || instance.sample_rate == 0 {
                return Err(DeviceContractError::RuntimeBudgetExceeded);
            }
            instance.device_type.validate()?;
            let powered_off = instance
                .definition
                .as_ref()
                .is_some_and(PreparedProcessorFactory::is_powered_off);
            if powered_off
                && (!matches!(
                    instance.owner,
                    ProcessorChainOwner::Main | ProcessorChainOwner::Return(_)
                ) || !instance.bypassed
                    || instance.device_type
                        != hex_lattice_contract::DeviceTypeId::from_static("hex.vst3")
                    || !instance.matrix.is_identity()
                    || !instance.automation.is_empty()
                    || prepared_modulation.is_some()
                    || instance
                        .configuration
                        .as_ref()
                        .is_none_or(|state| state.as_ref().type_id() != instance.device_type))
            {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            let (runtime_version, output_tail, latency) =
                if let Some(factory) = &instance.definition {
                    let pair = factory
                        .declared_pair_bytes()
                        .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                    let limit = if factory.is_owned() {
                        MAX_OWNED_PROCESSOR_PAIR_BYTES
                    } else {
                        MAX_PROCESSOR_RUNTIME_BYTES * 2
                    };
                    if factory.runtime_version() == 0 || pair > limit {
                        return Err(DeviceContractError::RuntimeBudgetExceeded);
                    }
                    if !powered_off {
                        stateful_count = stateful_count
                            .checked_add(1)
                            .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                    }
                    declared_pair_bytes = declared_pair_bytes
                        .checked_add(pair)
                        .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                    if stateful_count > MAX_STATEFUL_PROCESSORS
                        || declared_pair_bytes
                            .checked_add(automation_bytes)
                            .is_none_or(|bytes| bytes > MAX_PROCESSOR_BANK_RUNTIME_BYTES * 2)
                    {
                        return Err(DeviceContractError::RuntimeBudgetExceeded);
                    }
                    (
                        factory.runtime_version(),
                        factory.output_tail(),
                        factory.algorithmic_latency_frames(),
                    )
                } else {
                    (0, OutputTail::Finite { max_tail_frames: 0 }, Some(0))
                };
            if matches!(
                instance.definition,
                Some(PreparedProcessorFactory::Foreign(_))
            ) && (instance
                .configuration
                .as_ref()
                .is_none_or(|state| state.as_ref().type_id() != instance.device_type)
                || !instance.matrix.is_identity())
            {
                return Err(DeviceContractError::ForeignProcessingUnavailable);
            }
            if let Some(automation) = instance.automation.foreign()
                && !matches!(
                    instance.definition,
                    Some(PreparedProcessorFactory::Foreign(processor))
                        if !processor.is_instrument_profile()
                            && processor.binding() == Some(automation.borrow().binding)
                )
            {
                return Err(DeviceContractError::ForeignProcessingUnavailable);
            }
            if instance.automation.has_host_targets()
                && (matches!(
                    instance.definition,
                    Some(PreparedProcessorFactory::Foreign(_))
                ) || instance.configuration.is_none())
            {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            let supports_smoothed_bypass =
                instance
                    .definition
                    .as_ref()
                    .is_none_or(|factory| match factory {
                        PreparedProcessorFactory::Legacy(value) => value.supports_smoothed_bypass(),
                        PreparedProcessorFactory::Owned(value) => value.supports_smoothed_bypass(),
                        PreparedProcessorFactory::Foreign(_)
                        | PreparedProcessorFactory::PoweredOff => false,
                    });
            let reserved_frames = instance
                .definition
                .as_ref()
                .map(PreparedProcessorFactory::reserved_bypass_frames)
                .transpose()?
                .flatten();
            let zero_latency_smoothing = supports_smoothed_bypass && latency == Some(0);
            let (committed_aligned_dry, scratch_aligned_dry) = if let Some(frames) = reserved_frames
            {
                (
                    Some(bypass_ramp::AlignedDry::reserved(frames)?),
                    Some(bypass_ramp::AlignedDry::reserved(frames)?),
                )
            } else if instance
                .definition
                .as_ref()
                .is_some_and(PreparedProcessorFactory::supports_latency_aligned_bypass)
            {
                (
                    Some(
                        bypass_ramp::AlignedDry::new(latency)
                            .ok_or(DeviceContractError::InvalidDescriptor)?,
                    ),
                    Some(
                        bypass_ramp::AlignedDry::new(latency)
                            .ok_or(DeviceContractError::InvalidDescriptor)?,
                    ),
                )
            } else {
                (None, None)
            };
            let supports_power_off = match &instance.definition {
                Some(PreparedProcessorFactory::Legacy(value)) => {
                    (zero_latency_smoothing || committed_aligned_dry.is_some())
                        && value.supports_power_off()
                }
                Some(PreparedProcessorFactory::Owned(value)) => {
                    (zero_latency_smoothing || reserved_frames.is_some())
                        && value.supports_power_off()
                }
                Some(
                    PreparedProcessorFactory::Foreign(_) | PreparedProcessorFactory::PoweredOff,
                )
                | None => false,
            };
            let mut runtime = RuntimeOwnership::prepare(instance.definition.as_ref())?;
            let modulation = match prepared_modulation {
                Some(prepared) if prepared.foreign_effect().is_some() => {
                    let (RuntimeOwnership::Foreign(effect), Some(target)) =
                        (&mut runtime, prepared.foreign_effect())
                    else {
                        return Err(DeviceContractError::ForeignProcessingUnavailable);
                    };
                    if effect.prepared.binding() != Some(target.binding)
                        || target.binding.instance != instance.device_id
                        || matches!(instance.owner, ProcessorChainOwner::Source(_))
                        || (effect.prepared.is_instrument_profile() && !target.allows_note_events)
                        || (!effect.prepared.is_instrument_profile() && target.allows_note_events)
                        || usize::from(effect.prepared.quantum_frames())
                            > foreign_modulation::MAX_FOREIGN_MODULATION_CHUNK
                        || !instance.automation.parameter_ids().is_empty()
                    {
                        return Err(DeviceContractError::ForeignProcessingUnavailable);
                    }
                    let modulation = prepared.prepare_foreign_effect()?;
                    automation_bytes = automation_bytes
                        .checked_add(
                            modulation
                                .storage_bytes()
                                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?,
                        )
                        .filter(|bytes| *bytes <= MAX_PROCESSOR_BANK_RUNTIME_BYTES * 2)
                        .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                    effect.modulation = Some(Box::new(modulation));
                    None
                }
                Some(prepared) => Some(prepared.prepare()?),
                None => None,
            };
            if let Some(modulation) = &modulation {
                let input = modulation.program().host_input_base();
                let output = modulation.program().host_output_base();
                if (input.is_some() || output.is_some())
                    && (latency != Some(0)
                        || instance.configuration.as_ref().is_none_or(|state| {
                            input.is_some_and(|base| state.input_gain_millidb() != base.millidb())
                                || output.is_some_and(|base| {
                                    state.output_gain_millidb() != base.millidb()
                                })
                        }))
                {
                    return Err(DeviceContractError::UnsupportedProcessingClass);
                }
                if (!modulation.program().parameter_ids().is_empty()
                    && !instance.matrix.is_identity())
                    || matches!(
                        instance.definition,
                        Some(PreparedProcessorFactory::Foreign(_))
                    )
                {
                    return Err(DeviceContractError::UnsupportedProcessingClass);
                }
                for &parameter in modulation.program().parameter_ids() {
                    let program = modulation.program();
                    if instance.definition.as_ref().is_none_or(|factory| {
                        !factory.supports_parameter_automation(parameter)
                            || factory.parameter_automation_descriptor(parameter)
                                != program.descriptor(parameter)
                            || (program.has_routes(parameter)
                                && factory.parameter_modulation_semantics(parameter).filter(
                                    |semantics| {
                                        semantics.modulation
                                            == hex_lattice_contract::ParameterModulation::SampleRate
                                    },
                                ) != program.semantics(parameter))
                    }) || !runtime.supports_parameter_automation(parameter)
                    {
                        return Err(DeviceContractError::UnsupportedProcessingClass);
                    }
                }
                if instance.automation.parameter_ids().iter().any(|id| {
                    modulation
                        .program()
                        .parameter_ids()
                        .binary_search(id)
                        .is_err()
                }) {
                    return Err(DeviceContractError::UnknownParameter);
                }
                automation_bytes = automation_bytes
                    .checked_add(
                        modulation
                            .heap_storage_bytes()
                            .ok_or(DeviceContractError::RuntimeBudgetExceeded)?,
                    )
                    .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                if automation_bytes > MAX_PROCESSOR_BANK_RUNTIME_BYTES * 2 {
                    return Err(DeviceContractError::RuntimeBudgetExceeded);
                }
            }
            for &parameter in instance.automation.parameter_ids() {
                if instance.definition.as_ref().is_none_or(|factory| {
                    !factory.supports_parameter_automation(parameter)
                        || factory.parameter_automation_descriptor(parameter)
                            != instance.automation.descriptor(parameter)
                }) || !runtime.supports_parameter_automation(parameter)
                {
                    return Err(DeviceContractError::UnsupportedProcessingClass);
                }
            }
            entries.push(RuntimeEntry {
                modulation,
                automation: instance.automation,
                automation_frozen: false,
                committed_host_gains: instance.configuration.as_ref().map_or([1.0; 2], |state| {
                    [state.input_gain_linear(), state.output_gain_linear()]
                }),
                scratch_host_gains: instance.configuration.as_ref().map_or([1.0; 2], |state| {
                    [state.input_gain_linear(), state.output_gain_linear()]
                }),
                committed_output_gain_ramp: host_output_gain_ramp::OutputGainRamp::INACTIVE,
                scratch_output_gain_ramp: host_output_gain_ramp::OutputGainRamp::INACTIVE,
                committed_transient_output_gain: Default::default(),
                scratch_transient_output_gain: Default::default(),
                committed_matrix_ramp: matrix_ramp::MatrixRamp::INACTIVE,
                scratch_matrix_ramp: matrix_ramp::MatrixRamp::INACTIVE,
                meter: instance.meter,
                metering: false,
                input_gain: instance
                    .configuration
                    .as_ref()
                    .map_or(1.0, |state| state.input_gain_linear()),
                output_gain: instance
                    .configuration
                    .as_ref()
                    .map_or(1.0, |state| state.output_gain_linear()),
                owner: instance.owner,
                device_id: instance.device_id,
                device_type: instance.device_type,
                matrix: instance.matrix,
                bypassed: instance.bypassed,
                powered_off,
                removal_bypassing: false,
                runtime_version,
                sample_rate: instance.sample_rate,
                configuration: instance.configuration,
                output_tail,
                algorithmic_latency_frames: latency,
                supports_smoothed_bypass,
                supports_power_off,
                committed_bypass_ramp: bypass_ramp::BypassRamp::INACTIVE,
                scratch_bypass_ramp: bypass_ramp::BypassRamp::INACTIVE,
                committed_aligned_dry,
                scratch_aligned_dry,
                runtime,
            });
        }
        Ok(Self {
            entries,
            stateful_count,
            declared_pair_bytes,
        })
    }

    pub fn set_metering(&mut self, enabled: bool) {
        for entry in &mut self.entries {
            entry.metering = enabled;
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Effective latency for a prepared entry, including its current bypass.
    pub fn algorithmic_latency_frames_for_entry(
        &self,
        index: usize,
    ) -> Result<Option<u32>, DeviceContractError> {
        let entry = self
            .entries
            .get(index)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        Ok(
            if entry.bypassed
                && entry.committed_aligned_dry.is_none()
                && !matches!(entry.runtime, RuntimeOwnership::Foreign(_))
            {
                Some(0)
            } else {
                entry.algorithmic_latency_frames
            },
        )
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn has_stateful_processors(&self) -> bool {
        self.entries.iter().any(|entry| {
            entry.runtime.is_stateful()
                || !entry.automation.is_empty()
                || entry.modulation.is_some()
        })
    }

    /// Preflight and apply a transient audition to the exact current processor
    /// in both transactional twins. No mutation happens when either twin
    /// refuses the request. The target never resolves against retained tails.
    pub fn select_processor_audition(
        &mut self,
        owner: ProcessorChainOwner,
        device_id: u64,
        device_type: DeviceTypeId,
        selection: Option<ProcessorAuditionSelection>,
    ) -> bool {
        self.entries
            .iter_mut()
            .find(|entry| {
                entry.owner == owner
                    && entry.device_id == device_id
                    && entry.device_type == device_type
            })
            .is_some_and(|entry| entry.runtime.select_audition(selection, entry.bypassed))
    }

    /// Copy only a held transient audition selector from a matching old chain
    /// entry after a successful ownership handoff. Normal DSP history remains
    /// with the previous chain's release tail. The selector is applied only
    /// after both old twins agree and the candidate pair proves compatible
    /// parameter retargeting; `select_processor_audition` preflights both
    /// candidate twins before either is changed.
    pub fn transfer_processor_audition_from(
        &mut self,
        previous: &Self,
        owner: ProcessorChainOwner,
        device_id: u64,
        device_type: DeviceTypeId,
        minimum_entry: usize,
        expected_selection: ProcessorAuditionSelection,
    ) -> bool {
        let Some(index) = previous.entries.iter().position(|entry| {
            entry.owner == owner && entry.device_id == device_id && entry.device_type == device_type
        }) else {
            return false;
        };
        let Some(old) = previous.entries.get(index) else {
            return false;
        };
        if index < minimum_entry
            || old.runtime.requested_audition() != Some(expected_selection)
            || index
                .checked_add(1)
                .is_none_or(|end| !self.can_retarget_parameters_from_range(previous, index..end))
        {
            return false;
        }
        self.select_processor_audition(owner, device_id, device_type, Some(expected_selection))
    }

    /// Immediately remove transient audition state from the exact current
    /// processor, regardless of bypass or current band enablement.
    pub fn cancel_processor_audition(
        &mut self,
        owner: ProcessorChainOwner,
        device_id: u64,
        device_type: DeviceTypeId,
    ) -> bool {
        self.entries
            .iter_mut()
            .find(|entry| {
                entry.owner == owner
                    && entry.device_id == device_id
                    && entry.device_type == device_type
            })
            .is_some_and(|entry| entry.runtime.cancel_audition())
    }

    /// Immediately clear one exact current processor's transient state.
    pub fn cancel_processor_audition_for_identity(
        &mut self,
        owner: ProcessorChainOwner,
        device_id: u64,
        device_type: DeviceTypeId,
    ) -> bool {
        self.cancel_processor_audition(owner, device_id, device_type)
    }

    /// Clear every transient audition in this chain. Used when a prepared
    /// graph becomes a residual tail, which no longer has a control target.
    pub fn cancel_all_processor_auditions(&mut self) {
        for entry in &mut self.entries {
            entry.runtime.cancel_all_auditions();
        }
    }

    /// Whether both twins currently retain the requested transient selection.
    /// This is a bounded read-only lease check for the audio callback.
    pub fn processor_audition_is_active(
        &mut self,
        owner: ProcessorChainOwner,
        device_id: u64,
        device_type: DeviceTypeId,
        selection: ProcessorAuditionSelection,
    ) -> bool {
        self.entries
            .iter_mut()
            .find(|entry| {
                entry.owner == owner
                    && entry.device_id == device_id
                    && entry.device_type == device_type
            })
            .is_some_and(|entry| !entry.bypassed && entry.runtime.audition_is_active(selection))
    }

    pub const fn stateful_count(&self) -> usize {
        self.stateful_count
    }

    /// Total owned runtime payload, counting legacy twins exactly once each.
    pub const fn declared_pair_bytes(&self) -> usize {
        self.declared_pair_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_chain_construction_reports_no_entries_or_stateful_processors() {
        let mut chain = NativeRuntimeChain::new(Vec::new()).expect("empty chain");
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
        assert!(!chain.has_stateful_processors());
        assert_eq!(chain.foreign_bindings().count(), 0);
        chain.set_metering(true);
        chain.cancel_all_processor_auditions();
    }
}
