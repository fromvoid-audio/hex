//! Paired transactional native runtime chain, handoff, bypass/gain ramps and block processing.
use crate::adapters::{ExternalSegment, OffsetClock, OffsetParameterValues};
use crate::runtime_ownership::RuntimeOwnership;
use crate::{
    DeviceMeter, ExternalParameterAutomation, ForeignConfigurationContinuation,
    ForeignProcessorNode, ForeignProcessors, MAX_OWNED_PROCESSOR_PAIR_BYTES,
    MAX_PROCESSOR_BANK_RUNTIME_BYTES, MAX_PROCESSOR_RUNTIME_BYTES, MAX_STATEFUL_PROCESSORS,
    MainPassDelay, ModulationRuntime, NativeExternalAutomation, NativeExternalAutomationSet,
    NativeInstrumentClock, NativeParameterAutomation, NativeProcessorRuntime,
    NativeTransientApplication, NativeTransientInput, NativeTransientOperation,
    NativeTransientProcessing, NativeTransientValue, NodeSignalPhase, OutputTail,
    OwnedProcessorRuntime, PreparedForeignProcessor, PreparedModulationState,
    PreparedParameterAutomation, PreparedProcessorFactory, PreparedProcessorInstance,
    PreparedRuntimeInstance, ProcessorAuditionSelection, ProcessorChainOwner,
    ProcessorInstanceIdentity, RefuseForeign, StereoAuxiliaryBlock, StereoFrame, StereoMatrix,
    bypass_ramp, foreign_modulation, host_input_gain_modulation, host_output_gain_ramp,
    matrix_ramp,
};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};
use std::num::NonZeroU32;

/// Borrowed per-block processing inputs for tapped and auxiliary chain stages.
pub struct StageProcessContext<'a> {
    pub frame_offset: usize,
    pub clock: Option<&'a dyn NativeInstrumentClock>,
    pub output_clock: Option<&'a dyn NativeInstrumentClock>,
    pub foreign: Option<&'a mut dyn ForeignProcessors>,
    pub transient: Option<&'a mut NativeTransientProcessing>,
    pub external: Option<&'a dyn NativeExternalAutomationSet>,
}

impl<'a> StageProcessContext<'a> {
    /// Construct a stage processing context with clocks and no optional inputs.
    pub fn new(
        frame_offset: usize,
        clock: Option<&'a dyn NativeInstrumentClock>,
        output_clock: Option<&'a dyn NativeInstrumentClock>,
    ) -> Self {
        Self {
            frame_offset,
            clock,
            output_clock,
            foreign: None,
            transient: None,
            external: None,
        }
    }

    /// Attach a borrowed foreign processor bank for this stage call.
    pub fn with_foreign(mut self, foreign: &'a mut dyn ForeignProcessors) -> Self {
        self.foreign = Some(foreign);
        self
    }

    /// Attach optional live transient parameter input for this stage call.
    pub fn with_transient(mut self, transient: Option<&'a mut NativeTransientProcessing>) -> Self {
        self.transient = transient;
        self
    }

    /// Attach optional materialized external automation for this stage call.
    pub fn with_external(mut self, external: Option<&'a dyn NativeExternalAutomationSet>) -> Self {
        self.external = external;
        self
    }
}

pub(crate) struct EntryProcessContext<'a> {
    pub(crate) key: Option<&'a [StereoFrame]>,
    pub(crate) frame_offset: usize,
    pub(crate) clock: Option<&'a dyn NativeInstrumentClock>,
    pub(crate) output_clock: Option<&'a dyn NativeInstrumentClock>,
    pub(crate) foreign: &'a mut dyn ForeignProcessors,
    pub(crate) input: Option<NativeTransientValue>,
    pub(crate) external: Option<NativeExternalAutomation<'a>>,
}

pub(crate) struct RuntimeEntry {
    pub(crate) modulation: Option<ModulationRuntime>,
    pub(crate) automation: PreparedParameterAutomation,
    pub(crate) automation_frozen: bool,
    // Host effective trims are transactional too: a late render refusal cannot
    // freeze speculative controls into a retained output suffix.
    pub(crate) committed_host_gains: [f32; 2],
    pub(crate) scratch_host_gains: [f32; 2],
    pub(crate) committed_output_gain_ramp: host_output_gain_ramp::OutputGainRamp,
    pub(crate) scratch_output_gain_ramp: host_output_gain_ramp::OutputGainRamp,
    pub(crate) committed_transient_output_gain: host_output_gain_ramp::TransientGain,
    pub(crate) scratch_transient_output_gain: host_output_gain_ramp::TransientGain,
    // Static matrix ramp twins; see `matrix_ramp` for the handoff law.
    pub(crate) committed_matrix_ramp: matrix_ramp::MatrixRamp,
    pub(crate) scratch_matrix_ramp: matrix_ramp::MatrixRamp,
    pub(crate) meter: std::sync::Arc<DeviceMeter>,
    pub(crate) metering: bool,
    pub(crate) input_gain: f32,
    pub(crate) output_gain: f32,
    pub(crate) owner: ProcessorChainOwner,
    pub(crate) device_id: u64,
    pub(crate) device_type: DeviceTypeId,
    pub(crate) matrix: StereoMatrix,
    pub(crate) bypassed: bool,
    pub(crate) powered_off: bool,
    // A pending removal may still be cancelled after the dry ramp commits.
    // Keep its runtime reversible until publication decides its custody.
    pub(crate) removal_bypassing: bool,
    pub(crate) runtime_version: u32,
    pub(crate) sample_rate: u32,
    pub(crate) configuration: Option<std::sync::Arc<NativeDeviceState>>,
    pub(crate) output_tail: OutputTail,
    pub(crate) algorithmic_latency_frames: Option<u32>,
    pub(crate) supports_smoothed_bypass: bool,
    pub(crate) supports_power_off: bool,
    pub(crate) committed_bypass_ramp: bypass_ramp::BypassRamp,
    pub(crate) scratch_bypass_ramp: bypass_ramp::BypassRamp,
    pub(crate) committed_aligned_dry: Option<bypass_ramp::AlignedDry>,
    pub(crate) scratch_aligned_dry: Option<bypass_ramp::AlignedDry>,
    pub(crate) runtime: RuntimeOwnership,
}

impl RuntimeEntry {
    pub(crate) fn powered_off(&self, ramp: bypass_ramp::BypassRamp) -> bool {
        self.supports_power_off && self.bypassed && !self.removal_bypassing && !ramp.is_active()
    }

    fn committed_is_quiescent(&self) -> bool {
        (self.powered_off(self.committed_bypass_ramp) || self.runtime.committed_is_quiescent())
            && self
                .committed_aligned_dry
                .as_ref()
                .is_none_or(bypass_ramp::AlignedDry::is_quiet)
    }

    fn modulates_host_input(&self) -> bool {
        !self.bypassed
            && !self.automation_frozen
            && self
                .modulation
                .as_ref()
                .is_some_and(|value| value.program().host_input_base().is_some())
    }

    fn modulates_host_output(&self) -> bool {
        !self.bypassed
            && !self.automation_frozen
            && self
                .modulation
                .as_ref()
                .is_some_and(|value| value.program().host_output_base().is_some())
    }

    fn matches_memoryless_modulation_handoff(&self, old: &Self) -> bool {
        self.owner == old.owner
            && self.device_id == old.device_id
            && self.device_type == old.device_type
            && self.sample_rate == old.sample_rate
            && self.algorithmic_latency_frames == Some(0)
            && old.algorithmic_latency_frames == Some(0)
            && !old.committed_matrix_ramp.is_active()
            && !old.committed_output_gain_ramp.active()
            && !old.committed_bypass_ramp.is_active()
            && self
                .runtime
                .permits_memoryless_modulation_handoff_from(&old.runtime)
            && self
                .configuration
                .as_ref()
                .zip(old.configuration.as_ref())
                .is_some_and(|(new, old)| {
                    std::sync::Arc::ptr_eq(new, old) || new.processing_eq_on_callback(old)
                })
    }

    fn can_smooth_bypass_from(&self, old: &Self) -> bool {
        if self.algorithmic_latency_frames == Some(0) && old.algorithmic_latency_frames == Some(0) {
            self.supports_smoothed_bypass && old.supports_smoothed_bypass
        } else {
            self.algorithmic_latency_frames == old.algorithmic_latency_frames
                && self
                    .committed_aligned_dry
                    .as_ref()
                    .zip(old.committed_aligned_dry.as_ref())
                    .is_some_and(|(new, old)| new.can_copy_from(old))
                && self.runtime.can_inherit_from(&old.runtime)
        }
    }
}

/// Paired audio-owned runtime state. `primary` is committed state; processing
/// operates on `scratch` and `commit_block` is called only after the entire
/// containing block succeeds. Construction and final destruction are control
/// side responsibilities.
/// Modulation advances per processed frame, including bypass. Detached output
/// tails hold their last effective controls. `reset` retriggers sources at their
/// prepared initial phase/rate; graph handoff matches source IDs within a stable
/// device owner/id/type/rate independently of the provider's DSP layout version.
pub struct NativeRuntimeChain {
    pub(crate) entries: Vec<RuntimeEntry>,
    pub(crate) stateful_count: usize,
    pub(crate) declared_pair_bytes: usize,
}

/// Borrowed physical storage observations, never a provider declaration or
/// ownership/retirement capability. Private chain fields remain inaccessible.
pub struct NativeRuntimeStorage<'a> {
    chain: &'a NativeRuntimeChain,
}
pub struct NativeRuntimeEntryStorage<'a> {
    entry: &'a RuntimeEntry,
}
impl<'a> NativeRuntimeStorage<'a> {
    pub fn entry_capacity_bytes(&self) -> Option<usize> {
        self.chain
            .entries
            .capacity()
            .checked_mul(std::mem::size_of::<RuntimeEntry>())
    }
    pub fn entries(&self) -> impl Iterator<Item = NativeRuntimeEntryStorage<'a>> + use<'a> {
        self.chain
            .entries
            .iter()
            .map(NativeRuntimeEntryStorage::new)
    }
}
impl<'a> NativeRuntimeEntryStorage<'a> {
    fn new(entry: &RuntimeEntry) -> NativeRuntimeEntryStorage<'_> {
        let RuntimeEntry {
            modulation: _,
            automation: _,
            automation_frozen: _,
            committed_host_gains: _,
            scratch_host_gains: _,
            committed_output_gain_ramp: _,
            scratch_output_gain_ramp: _,
            committed_transient_output_gain: _,
            scratch_transient_output_gain: _,
            committed_matrix_ramp: _,
            scratch_matrix_ramp: _,
            meter: _,
            metering: _,
            input_gain: _,
            output_gain: _,
            owner: _,
            device_id: _,
            device_type: _,
            matrix: _,
            bypassed: _,
            powered_off: _,
            removal_bypassing: _,
            runtime_version: _,
            sample_rate: _,
            configuration: _,
            output_tail: _,
            algorithmic_latency_frames: _,
            supports_smoothed_bypass: _,
            supports_power_off: _,
            committed_bypass_ramp: _,
            scratch_bypass_ramp: _,
            committed_aligned_dry: _,
            scratch_aligned_dry: _,
            runtime: _,
        } = entry;
        NativeRuntimeEntryStorage { entry }
    }
    /// Only scalar addresses leave this view. They are observations, not origin
    /// receipts; equal bytes or addresses alone never establish provenance.
    pub fn automation(&self) -> &'a PreparedParameterAutomation {
        &self.entry.automation
    }
    pub fn modulation_storage_bytes(&self) -> Option<usize> {
        self.entry
            .modulation
            .as_ref()
            .map_or(Some(0), ModulationRuntime::heap_storage_bytes)?
            .checked_add(self.entry.runtime.foreign_modulation_bytes()?)
    }
    pub fn runtime_box_identities(&self) -> (Option<usize>, Option<usize>) {
        self.entry
            .runtime
            .legacy()
            .map_or((None, None), |(primary, scratch)| {
                (
                    Some(primary as *const dyn NativeProcessorRuntime as *const () as usize),
                    Some(scratch as *const dyn NativeProcessorRuntime as *const () as usize),
                )
            })
    }
    /// Explicit prepared Off metadata, not absence of a processing source.
    pub fn is_powered_off(&self) -> bool {
        self.entry.powered_off
    }

    /// Owned transactions have a different physical proof and never masquerade
    /// as a pair of legacy boxes. A declaration is not a native custody proof.
    pub fn is_owned_transaction(&self) -> bool {
        matches!(self.entry.runtime, RuntimeOwnership::Owned(_))
    }
    /// Read-only typed view of the actual owned transaction. The caller retains
    /// the closed constructor's independent factory and storage custody proofs.
    pub fn owned_runtime<T: OwnedProcessorRuntime>(&self) -> Option<&T> {
        let RuntimeOwnership::Owned(runtime) = &self.entry.runtime else {
            return None;
        };
        let concrete: &dyn std::any::Any = runtime.as_ref();
        concrete.downcast_ref()
    }

    /// Borrow only actual scalar foreign facts. This transfers no endpoint or
    /// source custody and cannot classify another runtime family as foreign.
    pub fn foreign_processor(&self) -> Option<PreparedForeignProcessor> {
        match &self.entry.runtime {
            RuntimeOwnership::Foreign(value) => Some(value.prepared),
            _ => None,
        }
    }
    /// Borrow actual reserved host histories. This grants no constructor,
    /// source, publication or retirement authority; the chain retains custody.
    pub fn reserved_dry_storage(&self) -> [Option<&'a Vec<StereoFrame>>; 2] {
        [
            self.entry
                .committed_aligned_dry
                .as_ref()
                .and_then(bypass_ramp::AlignedDry::reserved_storage),
            self.entry
                .scratch_aligned_dry
                .as_ref()
                .and_then(bypass_ramp::AlignedDry::reserved_storage),
        ]
    }
    pub fn primary_payload_bytes(&self) -> Option<usize> {
        self.entry
            .runtime
            .legacy()
            .map(|(v, _)| std::mem::size_of_val(v))
    }
    pub fn scratch_payload_bytes(&self) -> Option<usize> {
        self.entry
            .runtime
            .legacy()
            .map(|(_, v)| std::mem::size_of_val(v))
    }
    /// Read-only typed borrow of both actual legacy runtime owners. This never
    /// adopts a declaration/provider, clones history, or establishes native
    /// source custody; closed constructors retain that independent obligation.
    pub fn runtime_pair<T: NativeProcessorRuntime>(&self) -> Option<(&T, &T)> {
        let (primary, scratch) = self.entry.runtime.legacy()?;
        Some((
            primary.as_any().downcast_ref::<T>()?,
            scratch.as_any().downcast_ref::<T>()?,
        ))
    }
    pub fn runtime_types_are<T: NativeProcessorRuntime>(&self) -> bool {
        self.entry
            .runtime
            .legacy()
            .is_some_and(|(a, b)| a.as_any().is::<T>() && b.as_any().is::<T>())
    }
    pub fn matches_prepared_runtime_storage(&self, prepared: &PreparedRuntimeInstance) -> bool {
        (match (
            &self.entry.modulation,
            self.entry.runtime.foreign_modulation(),
            &prepared.modulation,
        ) {
            (Some(runtime), None, Some(declaration)) => {
                declaration.foreign_effect().is_none() && declaration.matches_runtime(runtime)
            }
            (None, Some(runtime), Some(declaration)) => {
                declaration.foreign_effect().is_some_and(|target| {
                    self.foreign_processor()
                        .and_then(PreparedForeignProcessor::binding)
                        == Some(target.binding)
                }) && runtime.matches_prepared(declaration)
            }
            (None, None, None) => true,
            _ => false,
        }) && self.entry.powered_off
            == prepared
                .definition
                .as_ref()
                .is_some_and(PreparedProcessorFactory::is_powered_off)
            && self.entry.automation == prepared.automation
            && self.entry.owner == prepared.owner
            && self.entry.device_id == prepared.device_id
            && self.entry.device_type == prepared.device_type
            && self.entry.sample_rate == prepared.sample_rate
            && std::sync::Arc::ptr_eq(&self.entry.meter, &prepared.meter)
            && match (&self.entry.configuration, &prepared.configuration) {
                (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
    /// Compare against an already borrowed prepared owner. The view never
    /// returns or clones either shared owning field.
    pub fn matches_prepared_storage(&self, prepared: &PreparedProcessorInstance) -> bool {
        self.entry.modulation.is_none()
            && self.entry.runtime.foreign_modulation().is_none()
            && self.entry.owner == prepared.owner
            && self.entry.device_id == prepared.device_id
            && self.entry.device_type == prepared.device_type
            && self.entry.sample_rate == prepared.sample_rate
            && std::sync::Arc::ptr_eq(&self.entry.meter, &prepared.meter)
            && match (&self.entry.configuration, &prepared.configuration) {
                (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

/// One staged native insertion at a proved position in a pair of chains.
/// Constructed by `NativeRuntimeChain::native_insert_transition_from`.
#[derive(Clone, Copy)]
pub struct NativeInsertTransition {
    position: usize,
}

/// The exact stage on a retiring predecessor. An authored bypass retains its
/// existing path to dry and needs no provisional mix.
#[derive(Clone, Copy)]
pub struct NativeRemovalTransition {
    position: usize,
    identity: ProcessorInstanceIdentity,
    authored_bypass: bool,
    foreign_pass: Option<(hex_lattice_contract::ForeignProcessorBinding, NonZeroU32)>,
}

impl NativeRemovalTransition {
    pub fn identity(self) -> ProcessorInstanceIdentity {
        self.identity
    }

    /// The exact endpoint and frame count of this Main stage's aligned history.
    pub fn foreign_main_pass(self) -> Option<(hex_lattice_contract::ForeignProcessorBinding, u32)> {
        let (binding, frames) = self.foreign_pass?;
        (self.identity.owner() == ProcessorChainOwner::Main).then_some((binding, frames.get()))
    }

    /// The exact original binding for the sole 512-frame hosted Return going Off.
    pub fn foreign_return_pass(self) -> Option<hex_lattice_contract::ForeignProcessorBinding> {
        let (binding, frames) = self.foreign_pass?;
        (matches!(self.identity.owner(), ProcessorChainOwner::Return(_)) && frames.get() == 512)
            .then_some(binding)
    }

    fn matches(self, entry: &RuntimeEntry) -> bool {
        entry.owner == self.identity.owner() && entry.device_id == self.identity.device_id()
            && self.foreign_pass.is_none_or(|(binding, frames)| {
                matches!(&entry.runtime,
                    RuntimeOwnership::Foreign(runtime) if runtime.prepared.binding() == Some(binding)
                        && runtime.prepared.total_latency_frames() == frames.get())
            })
    }
}

/// The retiring chain can keep a whole tail range, or leave only completed
/// foreign processors behind while other entries transfer normally.
enum HistoryExclusion {
    RetainedTail(std::ops::Range<usize>),
    CompletedForeign(std::ops::Range<usize>),
}

impl HistoryExclusion {
    fn range(&self) -> &std::ops::Range<usize> {
        match self {
            Self::RetainedTail(range) | Self::CompletedForeign(range) => range,
        }
    }

    fn valid_for(&self, len: usize) -> bool {
        let range = self.range();
        range.start <= range.end && range.end <= len
    }

    fn skips(&self, index: usize, old: &RuntimeEntry) -> bool {
        self.range().contains(&index)
            && (matches!(self, Self::RetainedTail(_))
                || (matches!(old.runtime, RuntimeOwnership::Foreign(_))
                    && old.runtime.committed_is_quiescent()
                    && old.committed_is_quiescent()))
    }
}

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
    /// Control-side inventory or bounded scalar test observation only. This
    /// does not authorize concurrent access to an active runtime.
    pub fn storage_view(&self) -> NativeRuntimeStorage<'_> {
        let Self {
            entries: _,
            stateful_count: _,
            declared_pair_bytes: _,
        } = self;
        NativeRuntimeStorage { chain: self }
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
                        || effect.prepared.is_instrument_profile()
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

    /// Account the chain's owned control/audio runtime storage, excluding
    /// allocator bookkeeping and shared prepared metadata (processor
    /// definitions and immutable `NativeDeviceState` payloads). Stateful DSP
    /// payload bytes use each processor's declared fixed storage. The
    /// configuration Arc handle stored in each runtime entry is included in
    /// the entry's `size_of` charge; its separately shared allocation is not.
    pub fn accounted_storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(
                self.entries
                    .capacity()
                    .checked_mul(std::mem::size_of::<RuntimeEntry>())?,
            )?
            .checked_add(self.declared_pair_bytes)?
            .checked_add(self.entries.iter().try_fold(0usize, |sum, entry| {
                sum.checked_add(entry.automation.storage_bytes()?)?
                    .checked_add(
                        entry
                            .modulation
                            .as_ref()
                            .map_or(Some(0), ModulationRuntime::heap_storage_bytes)?,
                    )?
                    .checked_add(entry.runtime.foreign_modulation_bytes()?)
            })?)
    }

    /// Declared payload preflight estimate for building a chain from instances.
    /// Call before runtime factories to reject an aggregate owner over budget.
    /// This assumes the requested entry capacity; allocator bookkeeping and
    /// shared prepared definitions are excluded. Check actual vector capacities
    /// and fixed runtime payload declarations after construction.
    pub fn declared_storage_preflight_bytes_for_instances(
        instances: &[PreparedProcessorInstance],
    ) -> Option<usize> {
        let payloads = instances.iter().try_fold(0_usize, |total, instance| {
            total.checked_add(
                instance
                    .definition
                    .as_ref()
                    .map_or(Some(0), |v| v.declared_runtime_bytes().checked_mul(2))?,
            )
        })?;
        Self::preflight_bytes(instances.len(), payloads)
    }
    pub fn declared_storage_preflight_bytes_for_prepared(
        instances: &[PreparedRuntimeInstance],
    ) -> Option<usize> {
        let payloads = instances.iter().try_fold(0_usize, |total, instance| {
            total.checked_add(
                instance
                    .definition
                    .as_ref()
                    .map_or(Some(0), PreparedProcessorFactory::declared_pair_bytes)?,
            )
        })?;
        let programs = instances.iter().try_fold(0usize, |sum, instance| {
            sum.checked_add(instance.automation.storage_bytes()?)?
                .checked_add(
                    instance
                        .modulation
                        .as_ref()
                        .map_or(Some(0), PreparedModulationState::quoted_runtime_heap_bytes)?,
                )
        })?;
        Self::preflight_bytes(instances.len(), payloads)?.checked_add(programs)
    }
    fn preflight_bytes(count: usize, payloads: usize) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(count.checked_mul(std::mem::size_of::<RuntimeEntry>())?)?
            .checked_add(payloads)
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

    /// Whether two complete chains perform the same static processing without
    /// signal, automation or unfinished host-ramp history. This observes only
    /// committed state; callers retain transport/source and ownership proofs.
    pub fn static_processing_matches(&self, other: &Self) -> bool {
        self.source_rejoin_processing_matches(other, |runtime| {
            matches!(runtime, RuntimeOwnership::Memoryless)
        })
    }

    /// Whether the whole post-instrument chain can copy committed history
    /// from a detached source. It must process the same stream without a clock
    /// or auxiliary signal, emit nothing autonomously, and explicitly admit
    /// this use. No ownership moves during preflight.
    pub fn detached_source_history_matches(&self, previous: &Self) -> bool {
        self.source_rejoin_processing_matches(previous, |runtime| match runtime {
            RuntimeOwnership::Memoryless => true,
            RuntimeOwnership::Legacy { primary, .. } => primary.supports_detached_source_rejoin(),
            RuntimeOwnership::Owned(_) | RuntimeOwnership::Foreign(_) => false,
        }) && self.can_inherit_from(previous)
    }

    fn source_rejoin_processing_matches(
        &self,
        other: &Self,
        supports: impl Fn(&RuntimeOwnership) -> bool,
    ) -> bool {
        let settled = |entry: &RuntimeEntry| {
            supports(&entry.runtime)
                && entry.algorithmic_latency_frames == Some(0)
                && entry.output_tail == (OutputTail::Finite { max_tail_frames: 0 })
                && entry.automation.is_empty()
                && entry.modulation.is_none()
                && !entry.committed_matrix_ramp.is_active()
                && !entry.committed_bypass_ramp.is_active()
                && !entry.committed_output_gain_ramp.active()
                && entry.committed_transient_output_gain
                    == host_output_gain_ramp::TransientGain::Follow
                && entry.committed_host_gains == [entry.input_gain, entry.output_gain]
        };
        self.len() == other.len()
            && self.configuration_matches_range(other, 0..self.len())
            && self
                .entries
                .iter()
                .zip(&other.entries)
                .all(|(left, right)| {
                    left.bypassed == right.bypassed
                        && left.input_gain == right.input_gain
                        && left.output_gain == right.output_gain
                        && settled(left)
                        && settled(right)
                })
    }

    /// Whether a same-position range has exact immutable processor
    /// configuration identity, in addition to its prepared processing
    /// semantics. Missing synthetic identities compare conservatively false.
    /// Native processors explicitly supporting a compatible smoothed bypass
    /// may differ in bypass state; their history stays with the live chain. The
    /// same rule applies to memoryless matrix entries. Foreign sources retain
    /// their separate aligned-bypass identity contract. Different foreign
    /// program revisions can match only through exact retained program-owner
    /// correspondence, with unchanged native/host envelope metadata. The host
    /// still validates the current frozen quantum and actual source custody.
    /// Revision handoff can call this on the audio callback; it can inspect up to
    /// `range.len() * MAX_NATIVE_DEVICE_STATE_BYTES` payload bytes in the
    /// worst case. Larger VST3 state must share its immutable payload allocation.
    pub fn configuration_matches_range(&self, other: &Self, range: std::ops::Range<usize>) -> bool {
        self.configuration_matches_range_inner(other, range, None)
    }

    /// Compare a single host-authenticated foreign continuation alongside the
    /// ordinary immutable graph rules. This is comparison only: it neither
    /// validates the host's witness nor transfers source or tail custody.
    pub fn configuration_matches_range_with_foreign_continuation(
        &self,
        other: &Self,
        range: std::ops::Range<usize>,
        continuation: &ForeignConfigurationContinuation<'_>,
    ) -> bool {
        self.configuration_matches_range_inner(other, range, Some(continuation))
    }

    fn configuration_matches_range_inner(
        &self,
        other: &Self,
        range: std::ops::Range<usize>,
        continuation: Option<&ForeignConfigurationContinuation<'_>>,
    ) -> bool {
        if range.start > range.end
            || range.end > self.entries.len()
            || range.end > other.entries.len()
        {
            return false;
        }
        self.entries[range.clone()]
            .iter()
            .zip(&other.entries[range])
            .all(|(left, right)| Self::entry_configuration_matches(left, right, continuation))
    }

    pub(crate) fn entry_configuration_matches(
        left: &RuntimeEntry,
        right: &RuntimeEntry,
        continuation: Option<&ForeignConfigurationContinuation<'_>>,
    ) -> bool {
        left.owner == right.owner
        && left.device_id == right.device_id
        && left.device_type == right.device_type
        && left.runtime_version == right.runtime_version
        && left.sample_rate == right.sample_rate
        && left.runtime.foreign_configuration_matches(&right.runtime)
        // Foreign bypass is the host's aligned wet/dry selection:
        // the same source still processes, at the same latency.
        // This compares graph views only. The host must separately
        // retain/admit the actual endpoint and its dry history.
        && (left.bypassed == right.bypassed
            || (left.can_smooth_bypass_from(right)
                && left.input_gain == right.input_gain
                && left.output_gain == right.output_gain
                && !matches!(left.runtime, RuntimeOwnership::Foreign(_))
                && !matches!(right.runtime, RuntimeOwnership::Foreign(_)))
            || matches!((&left.runtime, &right.runtime),
                (RuntimeOwnership::Foreign(left), RuntimeOwnership::Foreign(right))
                    if left.prepared == right.prepared))
        && (left.automation == right.automation
            || match (&left.runtime, &right.runtime) {
                (RuntimeOwnership::Foreign(a), RuntimeOwnership::Foreign(b))
                    if a.prepared == b.prepared => a.prepared.binding().is_some_and(|binding|
                        left.automation.shares_foreign_program_history_with(&right.automation, binding)),
                _ => false,
            })
        && left.matrix == right.matrix
        && match (&left.configuration, &right.configuration) {
            (Some(previous_state), Some(candidate_state)) => {
                std::sync::Arc::ptr_eq(previous_state, candidate_state)
                    || previous_state.processing_eq_on_callback(candidate_state)
                    || continuation.is_some_and(|proof| {
                        proof.matches(left, right, previous_state, candidate_state)
                    })
            }
            _ => false,
        }
    }

    /// Preflight a retiring-donor transfer for a source effect recipe. The
    /// native provider, trims and topology must still match. A live edit may
    /// replace a static matrix with a provider that explicitly proves it has
    /// no signal history to carry, or retarget a matching unautomated,
    /// memoryless source matrix through the existing host ramp. A stateful
    /// provider may also admit a changed host matrix if both candidate twins
    /// explicitly prove that their own processing configuration is unchanged
    /// and the ordinary history and retarget preflights succeed. Compatible
    /// automation changes keep the existing DSP history under the same
    /// configuration and tail bound.
    /// A live continuation may append one new, memoryless identity matrix
    /// after an exactly matching nonempty prefix. The new stage has neutral
    /// trims, no automation/modulation, latency or tail, and a distinct source
    /// device identity. Strict continuation compares the entire declaration.
    pub fn can_transfer_source_recipe_history_from(
        &self,
        previous: &Self,
        allow_control_edit: bool,
    ) -> bool {
        self.can_transfer_source_history_from(previous, allow_control_edit, false)
    }

    /// Exact source recipe continuation with an authored power edit on one
    /// or more opted-in native stages. The caller proves live playback and
    /// the same receiving track, in addition to its normal generator proof.
    pub fn can_transfer_source_power_edit_from(&self, previous: &Self) -> bool {
        self.can_transfer_source_history_from(previous, false, true)
    }

    fn can_transfer_source_history_from(
        &self,
        previous: &Self,
        allow_control_edit: bool,
        allow_power_edit: bool,
    ) -> bool {
        // An identity-only final stage adds no signal or control history. Keep
        // the old prefix exact and transfer it to the candidate before use.
        // The candidate's new identity must not alias any donor entry.
        let neutral_append = allow_control_edit
            && !previous.is_empty()
            && previous.len().checked_add(1) == Some(self.len())
            && self.entries.last().is_some_and(|appended| {
                matches!(appended.owner, ProcessorChainOwner::Source(_))
                    && previous.entries.iter().all(|old| {
                        old.owner == appended.owner
                            && old.sample_rate == appended.sample_rate
                            && old.device_id != appended.device_id
                    })
                    && matches!(appended.runtime, RuntimeOwnership::Memoryless)
                    && appended.configuration.is_some()
                    && !appended.bypassed
                    && appended.input_gain == 1.0
                    && appended.output_gain == 1.0
                    && appended.matrix == StereoMatrix::identity()
                    && appended.algorithmic_latency_frames == Some(0)
                    && appended.output_tail == (OutputTail::Finite { max_tail_frames: 0 })
                    && appended.automation.is_empty()
                    && appended.modulation.is_none()
                    && appended.committed_transient_output_gain
                        == host_output_gain_ramp::TransientGain::Follow
                    && appended.scratch_transient_output_gain
                        == host_output_gain_ramp::TransientGain::Follow
                    && appended.committed_host_gains == [1.0, 1.0]
                    && appended.scratch_host_gains == [1.0, 1.0]
                    && !appended.scratch_matrix_ramp.is_active()
                    && !appended.scratch_output_gain_ramp.active()
                    && !appended.scratch_bypass_ramp.is_active()
                    && !appended.committed_matrix_ramp.is_active()
                    && !appended.committed_output_gain_ramp.active()
                    && !appended.committed_bypass_ramp.is_active()
            });
        if neutral_append {
            return self.entries.iter().zip(&previous.entries).enumerate().all(
                |(index, (left, right))| {
                    !matches!(left.runtime, RuntimeOwnership::Foreign(_))
                        && !matches!(right.runtime, RuntimeOwnership::Foreign(_))
                        && left.bypassed == right.bypassed
                        && left.input_gain == right.input_gain
                        && left.output_gain == right.output_gain
                        && left.algorithmic_latency_frames == right.algorithmic_latency_frames
                        && left.output_tail == right.output_tail
                        && self.configuration_matches_range(previous, index..index + 1)
                        && left.runtime.same_foreign_modulation(&right.runtime)
                        && match (&left.modulation, &right.modulation) {
                            (None, None) => true,
                            (Some(left), Some(right)) => left.same_declaration(right),
                            _ => false,
                        }
                },
            ) && !self.transition_busy_from(previous)
                && self.can_inherit_from(previous);
        }
        self.len() == previous.len()
            && self.entries.iter().zip(&previous.entries).enumerate().all(
                |(index, (left, right))| {
                    let power_edit = allow_power_edit
                        && matches!(left.owner, ProcessorChainOwner::Source(_))
                        && left.supports_power_off
                        && right.supports_power_off;
                    !matches!(left.runtime, RuntimeOwnership::Foreign(_))
                        && !matches!(right.runtime, RuntimeOwnership::Foreign(_))
                        && (left.bypassed == right.bypassed || power_edit)
                        && left.input_gain == right.input_gain
                        && left.output_gain == right.output_gain
                        && left.algorithmic_latency_frames == right.algorithmic_latency_frames
                        && ((left.output_tail == right.output_tail
                            && self.configuration_matches_range(previous, index..index + 1))
                            || allow_control_edit
                                && left.matches_memoryless_modulation_handoff(right)
                            // A static, zero-latency source matrix has no DSP
                            // history to retarget, even when different authored
                            // settings resolve to the same matrix. The existing identity preflight
                            // and inherited matrix ramp preserve its live signal.
                            || allow_control_edit
                                && matches!(left.owner, ProcessorChainOwner::Source(_))
                                && matches!(left.runtime, RuntimeOwnership::Memoryless)
                                && matches!(right.runtime, RuntimeOwnership::Memoryless)
                                && left.configuration.is_some()
                                && right.configuration.is_some()
                                && left.algorithmic_latency_frames == Some(0)
                                && right.algorithmic_latency_frames == Some(0)
                                && left.output_tail == (OutputTail::Finite { max_tail_frames: 0 })
                                && right.output_tail == (OutputTail::Finite { max_tail_frames: 0 })
                                && left.automation.is_empty()
                                && right.automation.is_empty()
                                && left.modulation.is_none()
                                && right.modulation.is_none()
                                && self.can_retarget_parameters_from_range(previous, index..index + 1)
                            || allow_control_edit
                                && matches!(left.owner, ProcessorChainOwner::Source(_))
                                && left.configuration.is_some()
                                && right.configuration.is_some()
                                && left.output_tail == right.output_tail
                                && left.automation.is_empty()
                                && right.automation.is_empty()
                                && left.modulation.is_none()
                                && right.modulation.is_none()
                                && left.runtime.can_retarget_source_host_matrix_from(&right.runtime)
                                && self.can_retarget_parameters_from_range(previous, index..index + 1)
                            || allow_control_edit
                                && matches!(left.owner, ProcessorChainOwner::Source(_))
                                && !left.bypassed
                                && left.configuration.is_some()
                                && right.configuration.is_some()
                                && left.output_tail == right.output_tail
                                && left.matrix == right.matrix
                                && left.automation.is_empty()
                                && right.automation.is_empty()
                                && left.modulation.is_none()
                                && right.modulation.is_none()
                                && left.runtime.can_retarget_source_parameters_from(&right.runtime)
                                && self.can_retarget_parameters_from_range(previous, index..index + 1)
                            || allow_control_edit
                                && left.output_tail == right.output_tail
                                && left.matrix == right.matrix
                                && left
                                    .configuration
                                    .as_ref()
                                    .zip(right.configuration.as_ref())
                                    .is_some_and(|(new, old)| {
                                        std::sync::Arc::ptr_eq(new, old)
                                            || new.processing_eq_on_callback(old)
                                    })
                                && self
                                    .can_retarget_parameters_from_range(previous, index..index + 1))
                        && (allow_control_edit
                            || left.runtime.same_foreign_modulation(&right.runtime))
                        && (allow_control_edit
                            || match (&left.modulation, &right.modulation) {
                                (None, None) => true,
                                (Some(left), Some(right)) => left.same_declaration(right),
                                _ => false,
                            })
                },
            )
            && !self.transition_busy_from(previous)
            && self.can_inherit_from(previous)
    }

    /// Whether a same-position processor range can keep history across an
    /// authored parameter change. Entry identity and processing topology must
    /// remain exact; a changed configuration is accepted only when both
    /// candidate twins explicitly support retargeting from the old committed
    /// runtime, or when a compatible unbypassed, unautomated entry changes
    /// its static host matrix. The host ramps that matrix; a stateful entry
    /// must also explicitly admit history and parameter retargeting. A simultaneous bypass change also requires explicit
    /// smoothed bypass support, transferable history and unchanged host
    /// gains, matrix and automation. Empty and malformed ranges are refused.
    pub fn can_retarget_parameters_from_range(
        &self,
        previous: &Self,
        range: std::ops::Range<usize>,
    ) -> bool {
        if range.start >= range.end
            || range.end > self.entries.len()
            || range.end > previous.entries.len()
        {
            return false;
        }
        self.entries[range.clone()]
            .iter()
            .zip(&previous.entries[range])
            .all(|(candidate, old)| {
                // Static host coefficients ramp independently of processor
                // history. A stateful provider still has to admit the retarget.
                let stateful_matrix_retarget =
                    matches!(
                        &candidate.runtime,
                        RuntimeOwnership::Legacy { primary, scratch }
                            if primary.supports_manual_host_matrix_retarget()
                                && scratch.supports_manual_host_matrix_retarget()
                    ) && candidate.runtime.can_inherit_from(&old.runtime)
                        && candidate.runtime.can_retarget_from(&old.runtime);
                let static_matrix_retarget = !candidate.bypassed
                    && candidate.automation.is_empty()
                    && old.automation.is_empty()
                    && ((!candidate.runtime.is_stateful() && !old.runtime.is_stateful())
                        || stateful_matrix_retarget);
                let bypass_retarget = candidate.bypassed != old.bypassed
                    && candidate.configuration.is_some()
                    && old.configuration.is_some()
                    && candidate.can_smooth_bypass_from(old)
                    && candidate.input_gain == old.input_gain
                    && candidate.output_gain == old.output_gain
                    && candidate.matrix == old.matrix
                    && candidate.automation == old.automation
                    && candidate.runtime.can_inherit_from(&old.runtime)
                    && candidate.runtime.can_retarget_from(&old.runtime);
                let stable_processing_identity = Self::can_preserve_aligned_dry(candidate, old)
                    && candidate.owner == old.owner
                    && candidate.device_id == old.device_id
                    && candidate.device_type == old.device_type
                    && candidate.runtime_version == old.runtime_version
                    && candidate.sample_rate == old.sample_rate
                    && candidate
                        .runtime
                        .foreign_configuration_matches(&old.runtime)
                    && (candidate.bypassed == old.bypassed || bypass_retarget)
                    && (candidate.matrix == old.matrix
                        || static_matrix_retarget
                        || ((!candidate.automation.is_empty() || !old.automation.is_empty())
                            && candidate.runtime.can_retarget_from(&old.runtime)));
                if !stable_processing_identity {
                    return false;
                }
                let configuration_matches = match (&candidate.configuration, &old.configuration) {
                    (Some(candidate), Some(old)) => {
                        std::sync::Arc::ptr_eq(candidate, old)
                            || candidate.processing_eq_on_callback(old)
                    }
                    _ => false,
                };
                if configuration_matches || static_matrix_retarget {
                    return true;
                }
                candidate.runtime.can_retarget_from(&old.runtime)
            })
    }

    /// Combine residual output-tail declarations for a serial range, excluding
    /// fixed algorithmic latency. Reclamation uses `zero_input_drain_for_range`.
    /// A range
    /// with any unbounded processor is unbounded; finite declarations add with
    /// checked arithmetic. Invalid ranges return `None`.
    pub fn output_tail_for_range(&self, range: std::ops::Range<usize>) -> Option<OutputTail> {
        if range.start > range.end || range.end > self.entries.len() {
            return None;
        }
        let entries = &self.entries[range];
        if entries
            .iter()
            .any(|entry| entry.output_tail == OutputTail::Unbounded)
        {
            return Some(OutputTail::Unbounded);
        }
        let finite_frames = entries.iter().try_fold(0_u64, |total, entry| {
            let OutputTail::Finite { max_tail_frames } = entry.output_tail else {
                return None;
            };
            total.checked_add(max_tail_frames)
        })?;
        Some(OutputTail::Finite {
            max_tail_frames: finite_frames,
        })
    }

    /// A foreign endpoint without a finite source/host drain bound cannot be
    /// admitted into a finite retained-tail transition. Invalid ranges refuse
    /// conservatively. Built-in unbounded runtimes retain their existing policy.
    pub fn range_has_unbounded_foreign_tail(&self, range: std::ops::Range<usize>) -> bool {
        self.entries.get(range).is_none_or(|entries| {
            entries.iter().any(|entry| {
                matches!(&entry.runtime, RuntimeOwnership::Foreign(value)
                if value.prepared.output_tail() == OutputTail::Unbounded)
            })
        })
    }

    /// Total zero-input drain bound for a serial range: fixed intrinsic latency
    /// plus residual output duration for every entry. Bypassed processors still
    /// advance private state, so this uses intrinsic rather than audible latency.
    /// Unknown latency or residual duration is unbounded; invalid ranges and
    /// checked sum overflow return `None`. Reclamation must also test quiescence.
    pub fn zero_input_drain_for_range(&self, range: std::ops::Range<usize>) -> Option<OutputTail> {
        let entries = self.entries.get(range)?;
        if entries.iter().any(|entry| {
            entry.algorithmic_latency_frames.is_none() || entry.output_tail == OutputTail::Unbounded
        }) {
            return Some(OutputTail::Unbounded);
        }
        let frames = entries.iter().try_fold(0_u64, |total, entry| {
            let OutputTail::Finite { max_tail_frames } = entry.output_tail else {
                return None;
            };
            total
                .checked_add(u64::from(entry.algorithmic_latency_frames?))?
                .checked_add(max_tail_frames)
        })?;
        Some(OutputTail::Finite {
            max_tail_frames: frames,
        })
    }

    /// True only when every committed processor in a valid range has no
    /// future output under zero input. Invalid ranges conservatively return
    /// false. Stateless matrix entries are immediately quiescent.
    pub fn committed_range_is_quiescent(&self, range: std::ops::Range<usize>) -> bool {
        self.entries
            .get(range)
            .is_some_and(|entries| entries.iter().all(RuntimeEntry::committed_is_quiescent))
    }

    /// Render silent input while a bypass mix still needs to reach its target,
    /// even when there is no remaining processor signal history.
    pub fn needs_zero_input_render(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.scratch_bypass_ramp.is_active() || !entry.committed_is_quiescent())
    }

    /// Speculative counterpart used only after processing scratch state in the
    /// current uncommitted render block.
    pub fn speculative_range_is_quiescent(&self, range: std::ops::Range<usize>) -> bool {
        self.entries.get(range).is_some_and(|entries| {
            entries.iter().all(|entry| {
                (entry.powered_off(entry.scratch_bypass_ramp)
                    || entry.runtime.speculative_is_quiescent())
                    && entry
                        .scratch_aligned_dry
                        .as_ref()
                        .is_none_or(bypass_ramp::AlignedDry::is_quiet)
            })
        })
    }

    /// Preflight every twin before copying any state. Matching identities whose
    /// runtime compatibility changed remain freshly reset on the candidate.
    pub fn can_inherit_from(&self, previous: &Self) -> bool {
        self.can_inherit_from_excluding_old_range(previous, 0..0)
    }

    /// Whether any stateful entry must defer a handoff from the same-identity
    /// entry of `previous` until that committed runtime's live transitions
    /// drain (Pulse #4178; see `NativeProcessorRuntime::transition_busy_from`).
    /// Hosts check this during pure handoff preflight, before inheritance,
    /// tail capture or reset classification, and refuse the whole handoff as
    /// transient instead of choosing another history path. It is conservative
    /// for entries a retained tail would keep. Bounded and allocation-free:
    /// one identity scan per candidate entry, like `can_inherit_from`.
    /// Settle the committed runtimes that `candidate` is
    /// `transition_busy_from`, while this (previous) chain's graph renders
    /// nothing (Pulse #4558). Pairs entries exactly as
    /// `transition_busy_from`; bounded and allocation-free. See
    /// `NativeProcessorRuntime::settle_idle_transitions`.
    pub fn settle_idle_transitions_for(&mut self, candidate: &Self) {
        for entry in candidate
            .entries
            .iter()
            .filter(|entry| entry.runtime.is_stateful())
        {
            if let Some(old) = self
                .entries
                .iter_mut()
                .find(|old| Self::same_runtime_identity(entry, old) && old.runtime.is_stateful())
                && entry.runtime.transition_busy_from(&old.runtime)
            {
                old.runtime.settle_idle_transitions();
            }
        }
    }
    pub fn transition_busy_from(&self, previous: &Self) -> bool {
        self.entries.iter().any(|entry| {
            entry.runtime.is_stateful()
                && previous
                    .entries
                    .iter()
                    .find(|old| {
                        Self::same_runtime_identity(entry, old) && old.runtime.is_stateful()
                    })
                    .is_some_and(|old| entry.runtime.transition_busy_from(&old.runtime))
        })
    }

    /// Preflight history inheritance while leaving a range of old chain entries
    /// with a retained tail owner. Exclusion is resolved by old stable device
    /// identity, so candidate reordering cannot accidentally inherit a tail's
    /// history into the new signal path.
    pub fn can_inherit_from_excluding_old_range(
        &self,
        previous: &Self,
        excluded_old_range: std::ops::Range<usize>,
    ) -> bool {
        self.can_inherit_preflight(previous, HistoryExclusion::RetainedTail(excluded_old_range))
    }

    /// Preflight a completed foreign suffix without retaining its old endpoint.
    /// The range is only a hint: each excluded entry must be an actual foreign
    /// runtime whose committed provider observation is quiescent. Speculative
    /// silence, owned DSP and incompatible nonforeign histories remain subject
    /// to the ordinary inheritance checks. This grants no host resource return
    /// or native source/admission authority.
    pub fn can_inherit_with_completed_foreign_range(
        &self,
        previous: &Self,
        completed_old_range: std::ops::Range<usize>,
    ) -> bool {
        self.can_inherit_preflight(
            previous,
            HistoryExclusion::CompletedForeign(completed_old_range),
        )
    }

    fn can_inherit_preflight(&self, previous: &Self, excluded: HistoryExclusion) -> bool {
        if !excluded.valid_for(previous.len()) {
            return false;
        }
        self.entries.iter().all(|entry| {
            if !entry.runtime.is_stateful() {
                return true;
            }
            let Some((index, old)) = previous.entries.iter().enumerate().find(|(_, old)| {
                Self::same_runtime_identity(entry, old) && old.runtime.is_stateful()
            }) else {
                return true;
            };
            excluded.skips(index, old)
                || (entry.runtime.can_inherit_from(&old.runtime)
                    && Self::can_preserve_aligned_dry(entry, old))
        })
    }
    fn can_preserve_aligned_dry(candidate: &RuntimeEntry, old: &RuntimeEntry) -> bool {
        match (&candidate.committed_aligned_dry, &old.committed_aligned_dry) {
            (Some(candidate), Some(old)) => candidate.can_copy_from(old) || old.is_quiet(),
            (None, Some(old)) => old.is_quiet(),
            _ => true,
        }
    }
    pub(crate) fn same_runtime_identity(a: &RuntimeEntry, b: &RuntimeEntry) -> bool {
        a.owner == b.owner
            && a.powered_off == b.powered_off
            && a.device_id == b.device_id
            && a.device_type == b.device_type
            && a.runtime_version == b.runtime_version
            && a.sample_rate == b.sample_rate
    }

    /// Check whether one exact processor can seed a pristine successor with
    /// timing/control state while its donor retains all signal history. This
    /// does not transfer custody and unsupported providers refuse.
    pub fn can_fork_timing_without_signal_history_at(&self, previous: &Self, index: usize) -> bool {
        let Some((new, old)) = self.entries.get(index).zip(previous.entries.get(index)) else {
            return false;
        };
        Self::same_runtime_identity(new, old)
            // A timing-only fork has no control-trajectory transfer contract.
            // Keep its existing fallback until an active output ramp is proved.
            && !old.committed_output_gain_ramp.active()
            && new.input_gain == old.input_gain
            && new.output_gain == old.output_gain
            && new.matrix == old.matrix
            && new.automation == old.automation
            && match (&new.configuration, &old.configuration) {
                (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b) || a.processing_eq_on_callback(b),
                _ => false,
            }
            && new
                .runtime
                .can_fork_timing_without_signal_history_from(&old.runtime)
    }

    /// Seed the checked successor without moving or copying donor signal history.
    /// Refusal leaves both runtime owners untouched; all work is bounded and
    /// provider implementations must not allocate, block or destroy storage.
    pub fn fork_timing_without_signal_history_at(
        &mut self,
        previous: &Self,
        index: usize,
    ) -> Result<(), DeviceContractError> {
        if !self.can_fork_timing_without_signal_history_at(previous, index) {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        let Some((new, old)) = self.entries.get_mut(index).zip(previous.entries.get(index)) else {
            return Err(DeviceContractError::InvalidDescriptor);
        };
        if !new
            .runtime
            .fork_timing_without_signal_history_from(&old.runtime)
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        Ok(())
    }

    /// Copy compatible histories to both twins after a successful preflight.
    /// Identity also includes the stable device type, runtime version and rate;
    /// persisted state versions may change while physical history is retained.
    pub fn inherit_from(&mut self, previous: &Self) {
        self.inherit_from_excluding_old_range_unchecked(previous, 0..0);
    }

    /// Copy compatible histories except entries retained by an old tail owner.
    /// Callers must preflight the same range before entering their commit phase.
    pub fn inherit_from_excluding_old_range(
        &mut self,
        previous: &Self,
        excluded_old_range: std::ops::Range<usize>,
    ) -> Result<(), DeviceContractError> {
        if excluded_old_range.start > excluded_old_range.end
            || excluded_old_range.end > previous.entries.len()
        {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        self.inherit_from_excluding_old_range_unchecked(previous, excluded_old_range);
        Ok(())
    }

    fn inherit_from_excluding_old_range_unchecked(
        &mut self,
        previous: &Self,
        excluded_old_range: std::ops::Range<usize>,
    ) {
        let excluded = HistoryExclusion::RetainedTail(excluded_old_range);
        self.inherit_host_stage_ramps(previous, &excluded);
        self.inherit_bypass_ramps(previous, &excluded);
        self.inherit_modulation(previous, &excluded);
        for entry in &mut self.entries {
            if entry.powered_off(entry.committed_bypass_ramp) {
                entry.runtime.reset();
            } else if let Some((index, old)) =
                previous.entries.iter().enumerate().find(|(_, old)| {
                    Self::same_runtime_identity(entry, old) && old.runtime.is_stateful()
                })
                && !excluded.skips(index, old)
            {
                entry.runtime.inherit_from(&old.runtime);
            }
        }
    }

    /// Commit-phase history transfer from an exclusively borrowed retiring
    /// chain, after the same identity/history preflight as ordinary inheritance.
    /// Owned providers may exchange their preallocated banks in constant time;
    /// legacy and foreign entries retain their existing copy laws. Neither
    /// chain nor any resource is destroyed; the host still retires the donor.
    pub fn transfer_history_from(&mut self, previous: &mut Self) {
        self.transfer_history_excluding_unchecked(previous, HistoryExclusion::RetainedTail(0..0));
    }

    /// Find a single new native stage that can ramp in at the same sample
    /// origin. Every other stage must retain identity, order and compatible
    /// configuration and history. The host separately proves routing and timing.
    pub fn native_insert_transition_from(
        &self,
        previous: &Self,
        continuation: Option<&ForeignConfigurationContinuation<'_>>,
    ) -> Option<NativeInsertTransition> {
        let position = self.extra_native_stage_from(
            previous,
            |added| !added.bypassed,
            |old, current| Self::entry_configuration_matches(old, current, continuation),
        )?;
        self.can_inherit_from(previous)
            .then_some(NativeInsertTransition { position })
    }

    /// Prove one smoothed zero-latency removal, including a memoryless matrix,
    /// with all surviving stages in order and history-compatible.
    pub fn native_removal_transition_from(
        &self,
        previous: &Self,
    ) -> Option<NativeRemovalTransition> {
        let position = previous.extra_native_stage_from(
            self,
            |removed| {
                !removed.removal_bypassing
                    && (!removed.bypassed
                        || removed.powered_off(removed.committed_bypass_ramp)
                        || matches!(removed.runtime, RuntimeOwnership::Memoryless))
            },
            |current, old| Self::entry_configuration_matches(old, current, None),
        )?;
        let removed = previous.entries.get(position)?;
        let identity = ProcessorInstanceIdentity::new(removed.owner, removed.device_id).ok()?;
        self.can_inherit_from(previous)
            .then_some(NativeRemovalTransition {
                position,
                identity,
                authored_bypass: removed.bypassed,
                foreign_pass: None,
            })
    }

    /// One outgoing Main stage whose delayed dry history can be copied by
    /// value. The host separately proves the unchanged upstream signal clock.
    pub fn main_pass_removal_from(&self, previous: &Self) -> Option<NativeRemovalTransition> {
        self.pass_removal_from(previous, ProcessorChainOwner::Main)
    }

    /// Exactly one unchanged hosted Return stage becomes resource-free Off.
    /// The host separately proves the original binding and both route clocks.
    pub fn return_power_off_from(
        &self,
        previous: &Self,
        return_id: u64,
    ) -> Option<NativeRemovalTransition> {
        self.pass_removal_from(previous, ProcessorChainOwner::Return(return_id))
    }

    fn pass_removal_from(
        &self,
        previous: &Self,
        owner: ProcessorChainOwner,
    ) -> Option<NativeRemovalTransition> {
        let [entry] = previous.entries.as_slice() else {
            return None;
        };
        if entry.owner != owner {
            return None;
        }
        let main = owner == ProcessorChainOwner::Main;
        let (latency_frames, foreign_pass) = match &entry.runtime {
            RuntimeOwnership::Memoryless => return None,
            RuntimeOwnership::Foreign(runtime) => {
                let prepared = runtime.prepared;
                if entry.device_type != DeviceTypeId::from_static("hex.vst3")
                    || prepared.is_instrument_profile()
                    || prepared.quantum_frames() != 256
                    || prepared.bridge_latency_frames() != 512
                {
                    return None;
                }
                match prepared.processor_latency_frames() {
                    0 => {}
                    // Only deletion has been admitted for the seven-frame profile.
                    7 if main && self.is_empty() => {}
                    _ => return None,
                }
                let frames = NonZeroU32::new(prepared.total_latency_frames())?;
                (frames.get(), Some((prepared.binding()?, frames)))
            }
            _ if main => (
                entry
                    .committed_aligned_dry
                    .as_ref()?
                    .main_pass_latency_frames()?,
                None,
            ),
            _ => return None,
        };
        // A genuine hosted Off keeps authored identity but owns no runtime.
        // Its live-pass timing is the same separately copied passive delay as
        // deletion; the old foreign processor remains only in the donor graph.
        let off = match self.entries.as_slice() {
            [off] => {
                matches!(entry.runtime, RuntimeOwnership::Foreign(_))
                    && off.powered_off
                    && off.bypassed
                    && off.owner == entry.owner
                    && off.device_id == entry.device_id
                    && off.device_type == entry.device_type
                    && off.sample_rate == entry.sample_rate
                    && off.algorithmic_latency_frames == Some(0)
                    && off.input_gain == entry.input_gain
                    && off.output_gain == entry.output_gain
                    && off
                        .configuration
                        .as_deref()
                        .zip(entry.configuration.as_deref())
                        .is_some_and(|(new, old)| new.processing_eq_on_callback(old))
            }
            _ => false,
        };
        if !(off || (main && self.is_empty()))
            || entry.algorithmic_latency_frames != Some(latency_frames)
            || entry.bypassed
            || entry.removal_bypassing
            || !entry.automation.is_empty()
            || entry.modulation.is_some()
            || entry.input_gain != 1.0
            || entry.output_gain != 1.0
            || entry.matrix != StereoMatrix::identity()
            || entry.committed_bypass_ramp.is_active()
            || entry.committed_matrix_ramp.is_active()
            || entry.committed_output_gain_ramp.active()
            || entry.committed_transient_output_gain != host_output_gain_ramp::TransientGain::Follow
        {
            return None;
        }
        Some(NativeRemovalTransition {
            position: 0,
            identity: ProcessorInstanceIdentity::new(entry.owner, entry.device_id).ok()?,
            authored_bypass: false,
            foreign_pass,
        })
    }

    /// Copy the exact settled Main delay without transferring its processor.
    pub fn settled_main_pass_delay(
        &self,
        transition: NativeRemovalTransition,
    ) -> Option<MainPassDelay> {
        if transition.identity().owner() != ProcessorChainOwner::Main
            || !self.native_removal_ready(transition)
        {
            return None;
        }
        self.entries
            .get(transition.position)?
            .committed_aligned_dry
            .as_ref()?
            .main_pass_delay()
    }

    fn extra_native_stage_from(
        &self,
        shorter: &Self,
        accepts_extra: impl Fn(&RuntimeEntry) -> bool,
        compatible: impl Fn(&RuntimeEntry, &RuntimeEntry) -> bool,
    ) -> Option<usize> {
        if shorter.len().checked_add(1) != Some(self.len()) {
            return None;
        }
        let position = shorter
            .entries
            .iter()
            .zip(&self.entries)
            .position(|(short, long)| !Self::same_runtime_identity(short, long))
            .unwrap_or(shorter.len());
        let extra = self.entries.get(position)?;
        let retained = self
            .entries
            .iter()
            .take(position)
            .chain(self.entries.iter().skip(position + 1));
        (accepts_extra(extra)
            && extra.algorithmic_latency_frames == Some(0)
            && extra.supports_smoothed_bypass
            && !matches!(extra.runtime, RuntimeOwnership::Foreign(_))
            && shorter.entries.iter().zip(retained).all(|(short, long)| {
                short.device_id != extra.device_id
                    && short.algorithmic_latency_frames == long.algorithmic_latency_frames
                    && compatible(short, long)
            }))
        .then_some(position)
    }

    /// Start a proved removal on its predecessor. Only the affected stage's
    /// wet/dry mix changes. Its DSP stays owned by the predecessor.
    pub fn begin_native_removal(&mut self, transition: NativeRemovalTransition) -> bool {
        if transition.authored_bypass {
            self.entries.get(transition.position).is_some_and(|entry| {
                transition.matches(entry) && entry.bypassed && !entry.removal_bypassing
            })
        } else {
            self.set_native_removal_bypass(transition, true)
        }
    }

    /// Restore the same stage if the pending edit is cancelled or refused.
    pub fn cancel_native_removal(&mut self, transition: NativeRemovalTransition) {
        if !transition.authored_bypass {
            self.set_native_removal_bypass(transition, false);
        }
    }

    /// Either the authored bypass or the provisional removal must reach dry.
    pub fn native_removal_ready(&self, transition: NativeRemovalTransition) -> bool {
        self.entries.get(transition.position).is_some_and(|entry| {
            transition.matches(entry)
                && entry.bypassed
                && entry.removal_bypassing != transition.authored_bypass
                && !entry.committed_bypass_ramp.is_active()
        })
    }

    fn set_native_removal_bypass(
        &mut self,
        transition: NativeRemovalTransition,
        bypassed: bool,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(transition.position) else {
            return false;
        };
        if !transition.matches(entry)
            || entry.bypassed == bypassed
            || entry.removal_bypassing == bypassed
        {
            return false;
        }
        // The foreign endpoint already mixes against its aligned dry ring.
        let ramp = if transition.foreign_pass.is_some() {
            bypass_ramp::BypassRamp::INACTIVE
        } else {
            bypass_ramp::BypassRamp::inherited(
                entry.committed_bypass_ramp,
                entry.bypassed,
                bypassed,
                entry.sample_rate,
            )
        };
        entry.bypassed = bypassed;
        entry.removal_bypassing = bypassed;
        entry.committed_bypass_ramp = ramp;
        entry.scratch_bypass_ramp = ramp;
        true
    }

    /// Commit the preflighted transition against the same two chains. The new
    /// stage ramps from dry to wet on the same signal. Only the new graph runs.
    pub fn transfer_history_with_native_insert_from(
        &mut self,
        previous: &mut Self,
        transition: NativeInsertTransition,
    ) {
        self.transfer_history_from(previous);
        if let Some(entry) = self.entries.get_mut(transition.position) {
            let ramp = bypass_ramp::BypassRamp::inherited(
                bypass_ramp::BypassRamp::INACTIVE,
                true,
                false,
                entry.sample_rate,
            );
            entry.committed_bypass_ramp = ramp;
            entry.scratch_bypass_ramp = ramp;
        }
    }

    /// Transfer only histories not retained by the donor's old output suffix.
    pub fn transfer_history_excluding_old_range(
        &mut self,
        previous: &mut Self,
        excluded: std::ops::Range<usize>,
    ) -> Result<(), DeviceContractError> {
        self.transfer_history_excluding(previous, HistoryExclusion::RetainedTail(excluded))
    }

    /// Transfer history after preflighting `can_inherit_with_completed_foreign_range`
    /// for the same range. Only quiescent foreign entries in it stay with the
    /// retiring chain. The caller supplies the separate endpoint-custody proof.
    pub fn transfer_history_with_completed_foreign_range(
        &mut self,
        previous: &mut Self,
        completed: std::ops::Range<usize>,
    ) -> Result<(), DeviceContractError> {
        self.transfer_history_excluding(previous, HistoryExclusion::CompletedForeign(completed))
    }

    fn transfer_history_excluding(
        &mut self,
        previous: &mut Self,
        excluded: HistoryExclusion,
    ) -> Result<(), DeviceContractError> {
        if !excluded.valid_for(previous.len()) {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        self.transfer_history_excluding_unchecked(previous, excluded);
        Ok(())
    }
    fn transfer_history_excluding_unchecked(
        &mut self,
        previous: &mut Self,
        excluded: HistoryExclusion,
    ) {
        self.inherit_host_stage_ramps(previous, &excluded);
        self.inherit_bypass_ramps(previous, &excluded);
        self.inherit_modulation(previous, &excluded);
        for entry in &mut self.entries {
            if entry.powered_off(entry.committed_bypass_ramp) {
                entry.runtime.reset();
            } else if let Some((index, old)) =
                previous.entries.iter_mut().enumerate().find(|(_, old)| {
                    Self::same_runtime_identity(entry, old) && old.runtime.is_stateful()
                })
                && !excluded.skips(index, old)
            {
                entry.runtime.transfer_history_from(&mut old.runtime);
            }
        }
    }

    fn inherit_modulation(&mut self, previous: &Self, excluded: &HistoryExclusion) {
        for entry in &mut self.entries {
            if let Some((index, old)) = previous.entries.iter().enumerate().find(|(_, old)| {
                entry.owner == old.owner
                    && entry.device_id == old.device_id
                    && entry.device_type == old.device_type
                    && entry.sample_rate == old.sample_rate
            }) && !excluded.skips(index, old)
                && let (Some(current), Some(old)) = (&mut entry.modulation, &old.modulation)
            {
                current.inherit_graph_committed(old);
            }
        }
    }

    /// Reuse one same-identity donor lookup for the independent output-trim
    /// and static-matrix trajectories. Excluded donors, bypass and automation
    /// keep the existing matrix law; the scalar proof lives with its owner.
    fn inherit_host_stage_ramps(&mut self, previous: &Self, excluded: &HistoryExclusion) {
        for entry in &mut self.entries {
            let old = previous
                .entries
                .iter()
                .enumerate()
                .find(|(_, old)| Self::same_runtime_identity(entry, old));
            host_output_gain_ramp::inherit(
                entry,
                old.filter(|&(index, old)| !excluded.skips(index, old))
                    .map(|(_, old)| old),
            );
            let ramp = match old {
                Some((index, old)) => matrix_ramp::MatrixRamp::inherited(
                    old.committed_matrix_ramp,
                    old.matrix,
                    entry.matrix,
                    entry.sample_rate,
                    !excluded.skips(index, old)
                        && !entry.bypassed
                        && !old.bypassed
                        && entry.automation.is_empty()
                        && old.automation.is_empty()
                        && !matches!(entry.runtime, RuntimeOwnership::Foreign(_)),
                ),
                None => matrix_ramp::MatrixRamp::INACTIVE,
            };
            entry.committed_matrix_ramp = ramp;
            entry.scratch_matrix_ramp = ramp;
        }
    }

    fn inherit_bypass_ramps(&mut self, previous: &Self, excluded: &HistoryExclusion) {
        for entry in &mut self.entries {
            let old = previous
                .entries
                .iter()
                .enumerate()
                .find(|(_, old)| Self::same_runtime_identity(entry, old));
            let ramp = match old {
                Some((index, old))
                    if !excluded.skips(index, old)
                        && entry.can_smooth_bypass_from(old)
                        && entry.input_gain == old.input_gain
                        && entry.output_gain == old.output_gain
                        && !matches!(entry.runtime, RuntimeOwnership::Foreign(_))
                        && !matches!(old.runtime, RuntimeOwnership::Foreign(_))
                        && entry.matrix == old.matrix
                        && entry.automation == old.automation
                        && match (&entry.configuration, &old.configuration) {
                            (Some(a), Some(b)) => {
                                std::sync::Arc::ptr_eq(a, b)
                                    || a.processing_eq_on_callback(b)
                                    || (entry.runtime.can_inherit_from(&old.runtime)
                                        && entry.runtime.can_retarget_from(&old.runtime))
                            }
                            _ => false,
                        } =>
                {
                    let ramp = bypass_ramp::BypassRamp::inherited(
                        old.committed_bypass_ramp,
                        old.bypassed,
                        entry.bypassed,
                        entry.sample_rate,
                    );
                    if entry.supports_power_off
                        && !entry.bypassed
                        && old.powered_off(old.committed_bypass_ramp)
                    {
                        bypass_ramp::BypassRamp::fresh_power_on(
                            entry.algorithmic_latency_frames.unwrap_or(0),
                            entry.sample_rate,
                        )
                    } else {
                        ramp
                    }
                }
                _ => bypass_ramp::BypassRamp::INACTIVE,
            };
            let inherited_dry = if let Some((index, old)) = old
                && !excluded.skips(index, old)
                && entry.committed_aligned_dry.is_some()
                && old.committed_aligned_dry.is_some()
                && entry.algorithmic_latency_frames == old.algorithmic_latency_frames
            {
                let committed = bypass_ramp::copy_dry(
                    &mut entry.committed_aligned_dry,
                    &old.committed_aligned_dry,
                );
                let scratch = bypass_ramp::copy_dry(
                    &mut entry.scratch_aligned_dry,
                    &old.committed_aligned_dry,
                );
                committed && scratch
            } else {
                false
            };
            if !inherited_dry {
                // A proved quiet layout change starts fresh, even if this
                // prepared candidate previously rendered another signal.
                // Reset in place; never replace or retire storage on audio.
                if let Some(history) = &mut entry.committed_aligned_dry {
                    history.reset();
                }
                if let Some(history) = &mut entry.scratch_aligned_dry {
                    history.reset();
                }
            }
            entry.committed_bypass_ramp = ramp;
            entry.scratch_bypass_ramp = ramp;
        }
    }

    /// Enter a residual-output owner after complete admission preflight. Owned
    /// sustain/freeze providers can release their held input-independent cycle;
    /// history and all storage stay with this chain until control retirement.
    pub fn begin_output_tail(&mut self) {
        for entry in &mut self.entries {
            entry.automation_frozen = true;
            entry.runtime.begin_output_tail();
        }
    }

    /// Start a transactional block by copying committed histories into twins.
    pub fn begin_block(&mut self) {
        for entry in &mut self.entries {
            if let Some(modulation) = &mut entry.modulation {
                modulation.begin();
            }
            entry.scratch_host_gains = entry.committed_host_gains;
            entry.scratch_output_gain_ramp = entry.committed_output_gain_ramp;
            entry.scratch_transient_output_gain = entry.committed_transient_output_gain;
            entry.scratch_matrix_ramp = entry.committed_matrix_ramp;
            entry.scratch_bypass_ramp = entry.committed_bypass_ramp;
            bypass_ramp::copy_dry(&mut entry.scratch_aligned_dry, &entry.committed_aligned_dry);
            if !entry.powered_off(entry.committed_bypass_ramp) {
                entry.runtime.begin();
            }
        }
    }

    /// Process the summed track signal through all registered stateful entries.
    pub fn process_block(&mut self, frames: &mut [StereoFrame]) -> Result<(), DeviceContractError> {
        Self::process_entries(&mut self.entries, frames, None, None)
    }

    /// Process the full chain while observing each entry's output in order.
    /// The observer borrows the current audio block and must not retain it.
    /// This is intended for bounded, allocation-free diagnostics such as an
    /// analyzer tap; musical behavior remains unchanged.
    pub fn process_block_observed(
        &mut self,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        observer: &mut impl FnMut(ProcessorChainOwner, u64, DeviceTypeId, usize, &[StereoFrame]),
    ) -> Result<(), DeviceContractError> {
        Self::process_entries_observed(
            &mut self.entries,
            frames,
            frame_offset,
            None,
            None,
            observer,
        )
    }

    /// Process a control-prevalidated effect segment without ending the
    /// surrounding transactional block. The range indexes the prepared effect
    /// order; callers must begin once and commit only after the full render block.
    pub fn process_range(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
    ) -> Result<(), DeviceContractError> {
        self.process_range_with_clock(range, frames, None, None)
    }

    /// Process a segment with explicit content origins for both host stages.
    /// `clock` describes entering content: provider controls and host input gain
    /// use it. `output_clock` describes emitted content after each processor's
    /// own algorithmic latency and drives host output gain. Each unavailable
    /// origin remains independently None; neither stage borrows the other's clock.
    /// The clocks must describe every entry in the range. Different arrivals
    /// require single-entry calls; zero-latency ranges explicitly reuse a clock.
    /// This method never shifts clocks or grants foreign automation support.
    pub fn process_range_with_clock(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        let entries = self
            .entries
            .get_mut(range)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        Self::process_entries(entries, frames, clock, output_clock)?;
        Ok(())
    }

    /// Process a prepared segment and observe each matching post-node signal.
    /// The callback receives the stable device ID and a borrowed view of the
    /// rendered segment. It must do bounded, nonblocking work and cannot retain
    /// the samples after returning.
    pub fn process_range_observed(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        observer: &mut impl FnMut(ProcessorChainOwner, u64, DeviceTypeId, usize, &[StereoFrame]),
    ) -> Result<(), DeviceContractError> {
        self.process_range_observed_with_clock(range, frames, frame_offset, None, None, observer)
    }

    /// Clock-aware segment processing with the same borrowed post-node tap.
    /// Stage origins follow `process_range_with_clock`; output observation does
    /// not change which content origin drives provider/input or output controls.
    pub fn process_range_observed_with_clock(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        observer: &mut impl FnMut(ProcessorChainOwner, u64, DeviceTypeId, usize, &[StereoFrame]),
    ) -> Result<(), DeviceContractError> {
        let entries = self
            .entries
            .get_mut(range)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        Self::process_entries_observed(
            entries,
            frames,
            frame_offset,
            clock,
            output_clock,
            observer,
        )?;
        Ok(())
    }

    /// Observe synchronous input/output borrows around each complete entry.
    /// Samples must not be retained, allocated from, or used for blocking work.
    pub fn process_block_tapped(
        &mut self,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        Self::process_entries_tapped(
            &mut self.entries,
            frames,
            frame_offset,
            None,
            None,
            observer,
        )
    }

    /// Process a prepared range with input/output taps and no content clock.
    pub fn process_range_tapped(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        self.process_range_tapped_with_clock(range, frames, frame_offset, None, None, observer)
    }

    /// Clock-aware paired taps. Input is before host trim, Output after the
    /// entire entry. Both phases have the same identity, offset and frame count.
    /// The independent stage-origin contract is `process_range_with_clock`.
    pub fn process_range_tapped_with_clock(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        let entries = self
            .entries
            .get_mut(range)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        Self::process_entries_tapped(entries, frames, frame_offset, clock, output_clock, observer)?;
        Ok(())
    }

    /// Process a segment without observation through the same borrowed path.
    /// Stage origins follow `process_range_with_clock`; foreign DSP continues
    /// to receive only its entering-content clock and existing host trim law.
    pub fn process_range_with_foreign(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        foreign: &mut dyn ForeignProcessors,
    ) -> Result<(), DeviceContractError> {
        self.process_range_tapped_with_context(
            range,
            frames,
            StageProcessContext::new(frame_offset, clock, output_clock).with_foreign(foreign),
            &mut |_, _, _, _, _, _| {},
        )
    }

    /// Validate an inline request against the exact prepared current target.
    /// A missing or frozen instance returns false without mutating the chain.
    /// Host trims cannot be applied while bypassed. Provider controls may
    /// advance under warm bypass; a powered-off entry cannot apply input.
    pub fn validate_transient_input(
        &self,
        input: NativeTransientInput,
    ) -> Result<bool, DeviceContractError> {
        let mut matching = self.entries.iter().filter(|entry| {
            !entry.automation_frozen
                && entry.owner == input.identity.owner()
                && entry.device_id == input.identity.device_id()
        });
        let Some(entry) = matching.next() else {
            return Ok(false);
        };
        if matching.next().is_some()
            || entry.automation.input_target() != Some(input.operation.target())
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        if entry.powered_off(entry.committed_bypass_ramp)
            || (entry.bypassed && input.operation.target().is_host())
        {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        if input.operation.target() == hex_lattice_contract::DeviceAutomationTarget::HostOutputGain
            && matches!(entry.runtime, RuntimeOwnership::Foreign(_))
        {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        if let Some(value) = input.operation.value() {
            entry.automation.validate_input(value)?;
        }
        if let hex_lattice_contract::DeviceAutomationTarget::Parameter(id) =
            input.operation.target()
            && (!entry.matrix.is_identity() || !entry.runtime.supports_parameter_automation(id))
        {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        Ok(true)
    }

    /// Drop an armed output override and its trajectory at a lifecycle fence.
    /// Call between render transactions. Missing input during rendering does
    /// not cross this fence; a delivered Clear can settle without its request.
    pub fn invalidate_transient_output_gain(&mut self, identity: ProcessorInstanceIdentity) {
        for entry in self.entries.iter_mut().filter(|entry| {
            !entry.automation_frozen
                && entry.owner == identity.owner()
                && entry.device_id == identity.device_id()
        }) {
            host_output_gain_ramp::invalidate_transient(entry);
        }
    }

    /// Stage a graph segment through borrowed clocks, optional foreign endpoints,
    /// transient input, and materialized external automation with per-node taps.
    /// Successful processing still grants no dispatch: final host-format
    /// acceptance remains exclusively with the audio owner. On error all
    /// foreign work for this callback is revoked; owned DSP can then roll back.
    pub fn process_range_tapped_with_context(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        context: StageProcessContext<'_>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        self.process_tapped(range, frames, None, context, observer)
    }

    /// Run one exact effect stage with its admitted key in the ordinary
    /// begin/commit/rollback transaction. Main and key are borrowed only for
    /// this call. A stereo key requires a stateful native identity-matrix stage.
    /// The caller rolls back the complete block on any stage refusal.
    pub fn process_auxiliary_stage_with_context(
        &mut self,
        stage: usize,
        block: StereoAuxiliaryBlock<'_>,
        mut context: StageProcessContext<'_>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        let end = stage
            .checked_add(1)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        let (main, key) = block.into_parts();
        context.foreign = None;
        self.process_tapped(stage..end, main, Some(key), context, observer)
    }

    fn process_tapped(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        key: Option<&[StereoFrame]>,
        context: StageProcessContext<'_>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        let StageProcessContext {
            frame_offset,
            clock,
            output_clock,
            foreign,
            mut transient,
            external,
        } = context;
        let mut refuse_foreign = RefuseForeign;
        let foreign: &mut dyn ForeignProcessors = foreign.unwrap_or(&mut refuse_foreign);
        let requested = transient.as_ref().map(|context| context.input);
        let mut applied = None;
        let result =
            (|| {
                let entries = self
                    .entries
                    .get_mut(range)
                    .ok_or(DeviceContractError::InvalidDescriptor)?;
                if let Some(requested) = requested
                    && entries
                        .iter()
                        .filter(|entry| {
                            entry.owner == requested.identity.owner()
                                && entry.device_id == requested.identity.device_id()
                        })
                        .take(2)
                        .count()
                        > 1
                {
                    return Err(DeviceContractError::InvalidDescriptor);
                }
                if let Some(set) = external {
                    for (index, entry) in entries
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| !entry.automation_frozen)
                    {
                        let identity = ProcessorInstanceIdentity {
                            owner: entry.owner,
                            device_id: entry.device_id,
                        };
                        if let Some(window) = set.window(identity)
                            && (window.identity != identity
                                || entries[..index].iter().any(|previous| {
                                    !previous.automation_frozen
                                        && previous.owner == entry.owner
                                        && previous.device_id == entry.device_id
                                }))
                        {
                            return Err(DeviceContractError::InvalidDescriptor);
                        }
                    }
                }
                for entry in entries {
                    let input = requested
                        .filter(|requested| {
                            !entry.automation_frozen
                                && entry.owner == requested.identity.owner()
                                && entry.device_id == requested.identity.device_id()
                        })
                        .map(|requested| requested.operation);
                    if let Some(operation) = input {
                        if entry.automation.input_target() != Some(operation.target()) {
                            return Err(DeviceContractError::InvalidDescriptor);
                        }
                        if let Some(value) = operation.value() {
                            entry.automation.validate_input(value)?;
                        }
                        if let hex_lattice_contract::DeviceAutomationTarget::Parameter(parameter) =
                            operation.target()
                            && (!entry.matrix.is_identity()
                                || !entry.runtime.supports_parameter_automation(parameter))
                        {
                            return Err(DeviceContractError::UnsupportedProcessingClass);
                        }
                    }

                    if !frames.is_empty()
                        && !entry.bypassed
                        && let Some(operation) = input
                    {
                        host_output_gain_ramp::stage_transient(entry, operation)?;
                    }
                    observer(
                        entry.owner,
                        entry.device_id,
                        entry.device_type,
                        NodeSignalPhase::Input,
                        frame_offset,
                        frames,
                    );
                    let identity = ProcessorInstanceIdentity {
                        owner: entry.owner,
                        device_id: entry.device_id,
                    };
                    let window = if entry.automation_frozen {
                        None
                    } else {
                        external.and_then(|set| set.window(identity))
                    };
                    if window.is_some_and(|window| window.identity != identity) {
                        return Err(DeviceContractError::InvalidDescriptor);
                    }
                    let started_powered_off =
                        input.is_some() && entry.powered_off(entry.scratch_bypass_ramp);
                    Self::process_entry_with_foreign(
                        entry,
                        frames,
                        EntryProcessContext {
                            key,
                            frame_offset,
                            clock,
                            output_clock,
                            foreign,
                            input: input.and_then(NativeTransientOperation::value).filter(
                                |value| !matches!(value, NativeTransientValue::HostOutputGain(_)),
                            ),
                            external: window,
                        },
                    )?;
                    if !frames.is_empty()
                        && !started_powered_off
                        && let Some(operation) = input
                        && (!entry.bypassed
                            || matches!(
                                operation.target(),
                                hex_lattice_contract::DeviceAutomationTarget::Parameter(_)
                            ))
                    {
                        applied = Some(NativeTransientApplication {
                            identity: ProcessorInstanceIdentity {
                                owner: entry.owner,
                                device_id: entry.device_id,
                            },
                            operation,
                            offset: frame_offset,
                        });
                    }

                    observer(
                        entry.owner,
                        entry.device_id,
                        entry.device_type,
                        NodeSignalPhase::Output,
                        frame_offset,
                        frames,
                    );
                }
                Ok(())
            })();
        if result.is_err() {
            foreign.reject_callback();
            if let Some(context) = &mut transient {
                context.applied = None;
            }
        } else if let Some(context) = &mut transient
            && context.applied.is_none()
        {
            context.applied = applied;
        }
        result
    }

    fn process_entries_tapped(
        entries: &mut [RuntimeEntry],
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        for entry in entries {
            observer(
                entry.owner,
                entry.device_id,
                entry.device_type,
                NodeSignalPhase::Input,
                frame_offset,
                frames,
            );
            Self::process_entry(entry, frames, clock, output_clock)?;
            observer(
                entry.owner,
                entry.device_id,
                entry.device_type,
                NodeSignalPhase::Output,
                frame_offset,
                frames,
            );
        }
        Ok(())
    }

    fn process_entries(
        entries: &mut [RuntimeEntry],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        for entry in entries {
            Self::process_entry(entry, frames, clock, output_clock)?;
        }
        Ok(())
    }

    fn process_entries_observed(
        entries: &mut [RuntimeEntry],
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        observer: &mut impl FnMut(ProcessorChainOwner, u64, DeviceTypeId, usize, &[StereoFrame]),
    ) -> Result<(), DeviceContractError> {
        for entry in entries {
            Self::process_entry(entry, frames, clock, output_clock)?;
            observer(
                entry.owner,
                entry.device_id,
                entry.device_type,
                frame_offset,
                frames,
            );
        }
        Ok(())
    }

    fn process_entry(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        Self::process_entry_with_foreign(
            entry,
            frames,
            EntryProcessContext {
                key: None,
                frame_offset: 0,
                clock,
                output_clock,
                foreign: &mut RefuseForeign,
                input: None,
                external: None,
            },
        )
    }

    fn process_entry_with_foreign(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        context: EntryProcessContext<'_>,
    ) -> Result<(), DeviceContractError> {
        let EntryProcessContext {
            key,
            frame_offset,
            clock,
            output_clock,
            foreign,
            input,
            external,
        } = context;
        let aligned = entry.scratch_aligned_dry.is_some();
        let aligned_bypass =
            aligned && entry.bypassed && (!entry.supports_power_off || entry.removal_bypassing);
        if !entry.scratch_bypass_ramp.is_active() && !aligned_bypass {
            if !entry.powered_off(entry.scratch_bypass_ramp)
                && let Some(history) = &mut entry.scratch_aligned_dry
            {
                history.advance_discard(frames);
            }
            return Self::process_entry_with_foreign_inner(
                entry,
                frames,
                EntryProcessContext {
                    key,
                    frame_offset,
                    clock,
                    output_clock,
                    foreign,
                    input,
                    external,
                },
            );
        }
        let mut offset = 0;
        while (aligned_bypass || entry.scratch_bypass_ramp.is_active()) && offset < frames.len() {
            const CHUNK: usize = 64;
            let mut count = (frames.len() - offset).min(CHUNK);
            if !aligned || entry.supports_power_off && !aligned_bypass {
                count = count.min(entry.scratch_bypass_ramp.remaining());
            }
            let end = offset + count;
            let mut dry = [[0.0; 2]; CHUNK];
            dry[..count].copy_from_slice(&frames[offset..end]);
            if let Some(history) = &mut entry.scratch_aligned_dry {
                for frame in &mut dry[..count] {
                    *frame = history.advance(*frame);
                }
            }
            let shifted_clock = clock.map(|clock| OffsetClock { clock, offset });
            let shifted_output_clock = output_clock.map(|clock| OffsetClock { clock, offset });
            let shifted_frame_offset = frame_offset
                .checked_add(offset)
                .ok_or(DeviceContractError::InvalidDescriptor)?;
            let bypassed = entry.bypassed;
            let aligned_metering = aligned && entry.metering;
            if aligned_metering {
                entry.meter.observe_input(&frames[offset..end]);
                entry.metering = false;
            }
            entry.bypassed = false;
            let result = Self::process_entry_with_foreign_inner(
                entry,
                &mut frames[offset..end],
                EntryProcessContext {
                    key: key.map(|key| &key[offset..end]),
                    frame_offset: shifted_frame_offset,
                    clock: shifted_clock
                        .as_ref()
                        .map(|c| c as &dyn NativeInstrumentClock),
                    output_clock: shifted_output_clock
                        .as_ref()
                        .map(|c| c as &dyn NativeInstrumentClock),
                    foreign,
                    input,
                    external,
                },
            );
            entry.bypassed = bypassed;
            entry.metering |= aligned_metering;
            result?;
            for (wet, dry) in frames[offset..end].iter_mut().zip(&dry[..count]) {
                let mix = entry.scratch_bypass_ramp.next(bypassed);
                if aligned && mix == 0.0 {
                    *wet = *dry;
                } else if !aligned || mix != 1.0 {
                    for channel in 0..2 {
                        wet[channel] = dry[channel] + (wet[channel] - dry[channel]) * mix;
                    }
                }
            }
            if aligned_metering {
                entry.meter.observe_output(&frames[offset..end]);
            }
            offset = end;
        }
        if offset == frames.len() {
            return Ok(());
        }
        if !entry.powered_off(entry.scratch_bypass_ramp)
            && let Some(history) = &mut entry.scratch_aligned_dry
        {
            history.advance_discard(&frames[offset..]);
        }
        let shifted_clock = clock.map(|clock| OffsetClock { clock, offset });
        let shifted_output_clock = output_clock.map(|clock| OffsetClock { clock, offset });
        Self::process_entry_with_foreign_inner(
            entry,
            &mut frames[offset..],
            EntryProcessContext {
                key: key.map(|key| &key[offset..]),
                frame_offset: frame_offset
                    .checked_add(offset)
                    .ok_or(DeviceContractError::InvalidDescriptor)?,
                clock: shifted_clock
                    .as_ref()
                    .map(|c| c as &dyn NativeInstrumentClock),
                output_clock: shifted_output_clock
                    .as_ref()
                    .map(|c| c as &dyn NativeInstrumentClock),
                foreign,
                input,
                external,
            },
        )
    }

    pub(crate) fn process_entry_with_foreign_inner(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        context: EntryProcessContext<'_>,
    ) -> Result<(), DeviceContractError> {
        let EntryProcessContext {
            key,
            frame_offset,
            clock,
            output_clock,
            foreign,
            input,
            external,
        } = context;
        let external = external.filter(|window| {
            !entry.automation_frozen
                && window.identity.owner() == entry.owner
                && window.identity.device_id() == entry.device_id
        });
        let segment = if let Some(window) = external {
            let start = frame_offset
                .checked_sub(window.frame_offset)
                .ok_or(DeviceContractError::InvalidDescriptor)?;
            let end = start
                .checked_add(frames.len())
                .ok_or(DeviceContractError::InvalidDescriptor)?;
            if end > window.values.frames() {
                return Err(DeviceContractError::InvalidDescriptor);
            }
            Some(ExternalSegment {
                values: window.values,
                start,
                frames: frames.len(),
            })
        } else {
            None
        };
        let external = segment
            .as_ref()
            .map(|segment| segment as &dyn ExternalParameterAutomation);
        let is_foreign = matches!(entry.runtime, RuntimeOwnership::Foreign(_));
        if key.is_some()
            && (is_foreign || !entry.runtime.is_stateful() || !entry.matrix.is_identity())
        {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        if entry.powered_off(entry.scratch_bypass_ramp) {
            if entry.metering {
                entry.meter.observe_input(frames);
            }
            if let Some(history) = &mut entry.scratch_aligned_dry {
                for frame in frames.iter_mut() {
                    *frame = history.advance(*frame);
                }
            }
            if entry.metering {
                entry.meter.observe_output(frames);
            }
            return Ok(());
        }
        let modulates_input = entry.modulates_host_input();
        if !entry.bypassed && !is_foreign && !modulates_input {
            Self::apply_host_gain(entry, frames, clock, true, input, external)?;
        }
        if entry.metering && !modulates_input {
            if is_foreign {
                entry
                    .meter
                    .observe_input_with_gain(frames, entry.input_gain);
            } else {
                entry.meter.observe_input(frames);
            }
        }
        let mut foreign_applies_output_gain = false;
        if let Some(key) = key {
            if !entry.bypassed && !modulates_input {
                entry.scratch_matrix_ramp.apply_block(entry.matrix, frames);
            }
            Self::process_native_provider(
                entry,
                frames,
                Some(key),
                clock,
                output_clock,
                input,
                external,
            )?;
        } else if let RuntimeOwnership::Foreign(runtime) = &mut entry.runtime {
            let configuration = entry
                .configuration
                .as_deref()
                .ok_or(DeviceContractError::ForeignProcessingUnavailable)?;
            let node = ForeignProcessorNode {
                owner: entry.owner,
                device_id: entry.device_id,
                device_type: entry.device_type,
                sample_rate: entry.sample_rate,
                configuration,
                prepared: runtime.prepared,
                automation: entry
                    .automation
                    .foreign()
                    .map(|program| {
                        program.borrow_for_effect_clock(if entry.automation_frozen {
                            None
                        } else {
                            clock.and_then(|clock| clock.authored_clock_epoch_at_offset(0))
                        })
                    })
                    .transpose()?,
                bypassed: entry.bypassed,
                input_gain: entry.input_gain,
                output_gain: if entry.automation_frozen {
                    entry.committed_host_gains[1]
                } else {
                    entry.output_gain
                },
            };
            foreign_applies_output_gain = foreign.returns_post_trim(node);
            // A failed stage supplies no new observation. Invalidate scratch
            // first so an earlier quiet state cannot mask renewed input.
            runtime.scratch_drained = false;
            // A detached tail retains its old program key while finishing any
            // partial PCM quantum. Its new input has no authored origin.
            let clock = if entry.automation_frozen && node.automation.is_some() {
                None
            } else {
                clock
            };
            if let Some(modulation) = &mut runtime.modulation {
                modulation.process_effect_segment(
                    node,
                    frame_offset,
                    frames,
                    clock,
                    foreign,
                    entry.automation_frozen,
                )?;
            } else {
                foreign.process_segment(node, frame_offset, frames, clock)?;
            }
            runtime.scratch_drained =
                matches!(runtime.prepared.output_tail(), OutputTail::Finite { .. })
                    && foreign.committed_tail_is_drained(node);
        } else if entry.bypassed {
            if entry.runtime.is_stateful() || entry.modulation.is_some() {
                if entry.matrix.is_identity() {
                    Self::process_native_provider(
                        entry,
                        frames,
                        None,
                        clock,
                        output_clock,
                        input,
                        external,
                    )?;
                } else {
                    for (frame_index, frame) in frames.iter_mut().enumerate() {
                        let audible = *frame;
                        let mut internal = entry.matrix.apply(audible);
                        // Preserve the existing per-frame matrix/bypass path.
                        // A shifted view keeps clock offset zero meaningful.
                        let shifted = clock.map(|clock| OffsetClock {
                            clock,
                            offset: frame_index,
                        });
                        let shifted = shifted
                            .as_ref()
                            .map(|clock| clock as &dyn NativeInstrumentClock);
                        if entry.modulation.is_some() {
                            // A nonidentity matrix can coexist only with an
                            // unassigned bank. Advance it while the processor
                            // sees the same private input it would without it.
                            Self::process_native_provider(
                                entry,
                                std::slice::from_mut(&mut internal),
                                None,
                                shifted,
                                None,
                                None,
                                None,
                            )?;
                        } else {
                            entry.runtime.process_bypassed_with_clock(
                                std::slice::from_mut(&mut internal),
                                shifted,
                            )?;
                        }
                        *frame = audible;
                    }
                }
            }
        } else {
            if !modulates_input {
                entry.scratch_matrix_ramp.apply_block(entry.matrix, frames);
            }
            Self::process_native_provider(
                entry,
                frames,
                None,
                clock,
                output_clock,
                input,
                external,
            )?;
        }
        if !entry.bypassed && !foreign_applies_output_gain && !entry.modulates_host_output() {
            Self::apply_host_gain(entry, frames, output_clock, false, input, external)?;
        } else if !entry.bypassed && foreign_applies_output_gain && !frames.is_empty() {
            entry.scratch_host_gains[1] = if entry.automation_frozen {
                entry.committed_host_gains[1]
            } else {
                entry.output_gain
            };
        }
        if entry.metering {
            entry.meter.observe_output(frames);
        }
        Ok(())
    }

    fn process_native_provider(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        key: Option<&[StereoFrame]>,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        input: Option<NativeTransientValue>,
        external: Option<&dyn ExternalParameterAutomation>,
    ) -> Result<(), DeviceContractError> {
        let modulates_input = entry.modulates_host_input();
        let values = (modulates_input
            || (!entry.automation.parameter_ids().is_empty() && !entry.automation_frozen))
            .then(|| {
                entry
                    .automation
                    .block_with_optional_external(clock, frames.len(), input, external)
            })
            .transpose()?;
        let input_stage = values.as_ref().filter(|_| modulates_input);
        let upstream = values
            .as_ref()
            .map(|value| value as &dyn NativeParameterAutomation);
        use host_output_gain_ramp::TransientGain;
        let host_values = entry
            .modulates_host_output()
            .then(|| {
                entry.automation.block_with_optional_external(
                    output_clock,
                    frames.len(),
                    input,
                    external,
                )
            })
            .transpose()?;
        let transient = entry.scratch_transient_output_gain;
        let host_target = hex_lattice_contract::DeviceAutomationTarget::HostOutputGain;
        if host_values.is_some()
            && !frames.is_empty()
            && transient == TransientGain::Follow
            && (entry.automation.has_host_target(host_target)
                || input.is_some_and(|input| input.target() == host_target))
        {
            entry.scratch_output_gain_ramp = host_output_gain_ramp::OutputGainRamp::INACTIVE;
        }
        let runtime = &mut entry.runtime;
        if let Some(modulation) = entry
            .modulation
            .as_mut()
            .filter(|_| !entry.automation_frozen)
        {
            let tap_required = input_stage.is_some() && modulation.follows_input();
            let chunk_frames = if tap_required {
                modulation
                    .chunk_frames()
                    .min(host_input_gain_modulation::TAP_FRAMES)
            } else {
                modulation.chunk_frames()
            };
            let mut offset = 0usize;
            for chunk in frames.chunks_mut(chunk_frames) {
                let end = offset
                    .checked_add(chunk.len())
                    .ok_or(DeviceContractError::InvalidDescriptor)?;
                let shifted = clock.map(|clock| OffsetClock { clock, offset });
                let chunk_clock = shifted
                    .as_ref()
                    .map(|clock| clock as &dyn NativeInstrumentClock);
                let shifted_values =
                    upstream.map(|values| OffsetParameterValues { values, offset });
                let chunk_values = shifted_values
                    .as_ref()
                    .map(|values| values as &dyn NativeParameterAutomation);
                let chunk_key = key
                    .map(|key| {
                        key.get(offset..end)
                            .ok_or(DeviceContractError::InvalidDescriptor)
                    })
                    .transpose()?;
                let mut tap;
                let follower_input = if let Some(input) = input_stage.filter(|_| tap_required) {
                    tap = [[0.0; 2]; host_input_gain_modulation::TAP_FRAMES];
                    let tap = &mut tap[..chunk.len()];
                    host_input_gain_modulation::prepare_tap(
                        tap,
                        chunk,
                        entry.input_gain,
                        input,
                        offset,
                        entry.scratch_matrix_ramp,
                        entry.matrix,
                    );
                    Some(&*tap)
                } else {
                    None
                };
                modulation.render_audio_chunk(
                    chunk,
                    follower_input,
                    chunk_values,
                    chunk_clock,
                    |chunk, values| {
                        if let Some(input) = input_stage {
                            if let Some(gain) =
                                host_input_gain_modulation::apply(input, values, offset, chunk)?
                            {
                                entry.scratch_host_gains[0] = gain;
                            }
                            if entry.metering {
                                entry.meter.observe_input(chunk);
                            }
                            entry.scratch_matrix_ramp.apply_block(entry.matrix, chunk);
                        }
                        if let Some(key) = chunk_key {
                            runtime.process_auxiliary(
                                StereoAuxiliaryBlock::paired(chunk, key),
                                chunk_clock,
                                (!values.parameter_ids().is_empty()).then_some(values),
                                entry.bypassed,
                            )
                        } else if values.parameter_ids().is_empty() {
                            if entry.bypassed {
                                runtime.process_bypassed_with_clock(chunk, chunk_clock)
                            } else {
                                runtime.process_with_clock(chunk, chunk_clock)
                            }
                        } else {
                            runtime.process_automation(chunk, chunk_clock, values, entry.bypassed)
                        }?;
                        if let Some(host) = host_values.as_ref()
                            && let Some(gain) = host_output_gain_ramp::apply_modulated(
                                &mut entry.scratch_output_gain_ramp,
                                transient,
                                host,
                                values,
                                offset,
                                chunk,
                            )?
                        {
                            entry.scratch_host_gains[1] = gain;
                        }
                        Ok(())
                    },
                )?;
                offset = end;
            }
            if host_values.is_some() && !frames.is_empty() {
                host_output_gain_ramp::finish_release(entry);
            }
            Ok(())
        } else if let Some(key) = key {
            runtime.process_auxiliary(
                StereoAuxiliaryBlock::paired(frames, key),
                clock,
                upstream,
                entry.bypassed,
            )
        } else if let Some(values) = upstream {
            runtime.process_automation(frames, clock, values, entry.bypassed)
        } else if entry.bypassed {
            runtime.process_bypassed_with_clock(frames, clock)
        } else {
            runtime.process_with_clock(frames, clock)
        }
    }

    fn apply_host_gain(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        input: bool,
        transient: Option<NativeTransientValue>,
        external: Option<&dyn ExternalParameterAutomation>,
    ) -> Result<(), DeviceContractError> {
        let index = if input { 0 } else { 1 };
        let base = if input {
            entry.input_gain
        } else {
            entry.output_gain
        };
        let target = if input {
            hex_lattice_contract::DeviceAutomationTarget::HostInputGain
        } else {
            hex_lattice_contract::DeviceAutomationTarget::HostOutputGain
        };
        use host_output_gain_ramp::TransientGain;
        if !input
            && !entry.automation_frozen
            && let TransientGain::Held(gain) = entry.scratch_transient_output_gain
        {
            if let Some(external) = external {
                entry
                    .automation
                    .block_with_external(clock, frames.len(), None, external)?;
            }
            if let Some(applied) = entry.scratch_output_gain_ramp.apply(gain.linear(), frames) {
                entry.scratch_host_gains[index] = applied;
            }
            return Ok(());
        }
        if entry.automation_frozen
            || (!entry.automation.has_host_target(target)
                && transient.is_none_or(|value| value.target() != target))
        {
            let gain = if entry.automation_frozen {
                entry.committed_host_gains[index]
            } else {
                base
            };
            if !input && !entry.automation_frozen {
                if let Some(gain) = entry.scratch_output_gain_ramp.apply(gain, frames) {
                    entry.scratch_host_gains[index] = gain;
                    host_output_gain_ramp::finish_release(entry);
                }
            } else {
                if gain != 1.0 {
                    for frame in frames.iter_mut() {
                        frame[0] *= gain;
                        frame[1] *= gain;
                    }
                }
                if !frames.is_empty() {
                    entry.scratch_host_gains[index] = gain;
                }
            }
            return Ok(());
        }
        let values = entry.automation.block_with_optional_external(
            clock,
            frames.len(),
            transient,
            external,
        )?;
        let releasing = !input && entry.scratch_transient_output_gain == TransientGain::Releasing;
        if !input && !releasing && !frames.is_empty() {
            entry.scratch_output_gain_ramp = host_output_gain_ramp::OutputGainRamp::INACTIVE;
        }
        for (offset, frame) in frames.iter_mut().enumerate() {
            let mut gain = values.host_gain_at(target, offset).unwrap_or(base);
            if releasing {
                gain = entry.scratch_output_gain_ramp.next(gain);
            }
            if gain != 1.0 {
                frame[0] *= gain;
                frame[1] *= gain;
            }
            entry.scratch_host_gains[index] = gain;
        }
        if releasing && !frames.is_empty() {
            host_output_gain_ramp::finish_release(entry);
        }
        Ok(())
    }

    /// Commit histories after all render chunks and source reads succeeded.
    pub fn commit_block(&mut self) {
        for entry in &mut self.entries {
            if let Some(modulation) = &mut entry.modulation {
                // Commit follows successful rendering of every provider; a
                // refused render aborts the enclosing transaction instead.
                let _ = modulation.commit();
            }
            entry.committed_host_gains = entry.scratch_host_gains;
            entry.committed_output_gain_ramp = entry.scratch_output_gain_ramp;
            entry.committed_transient_output_gain = entry.scratch_transient_output_gain;
            entry.committed_matrix_ramp = entry.scratch_matrix_ramp;
            let was_powered_off = entry.powered_off(entry.committed_bypass_ramp);
            entry.committed_bypass_ramp = entry.scratch_bypass_ramp;
            bypass_ramp::copy_dry(&mut entry.committed_aligned_dry, &entry.scratch_aligned_dry);
            if !was_powered_off {
                entry.runtime.commit();
                if entry.powered_off(entry.committed_bypass_ramp) {
                    entry.runtime.reset();
                }
                let reading = entry.runtime.take_dynamics_meter();
                if let Some(reading) = reading.filter(|_| entry.metering && !entry.bypassed) {
                    entry.meter.observe_dynamics(reading);
                }
            }
        }
    }
    /// Restore every speculative effect after a later host render stage refuses.
    pub fn rollback_block(&mut self) {
        for entry in &mut self.entries {
            if let Some(modulation) = &mut entry.modulation {
                modulation.rollback();
            }
            entry.scratch_host_gains = entry.committed_host_gains;
            entry.scratch_output_gain_ramp = entry.committed_output_gain_ramp;
            entry.scratch_transient_output_gain = entry.committed_transient_output_gain;
            entry.scratch_matrix_ramp = entry.committed_matrix_ramp;
            entry.scratch_bypass_ramp = entry.committed_bypass_ramp;
            bypass_ramp::copy_dry(&mut entry.scratch_aligned_dry, &entry.committed_aligned_dry);
            if !entry.powered_off(entry.committed_bypass_ramp) {
                entry.runtime.rollback();
            }
        }
    }

    /// Reset both committed and speculative state at a hard discontinuity.
    pub fn reset(&mut self) {
        for entry in &mut self.entries {
            if let Some(modulation) = &mut entry.modulation {
                modulation.reset_all();
            }
            entry.committed_host_gains = [entry.input_gain, entry.output_gain];
            entry.scratch_host_gains = entry.committed_host_gains;
            entry.committed_output_gain_ramp = host_output_gain_ramp::OutputGainRamp::INACTIVE;
            entry.scratch_output_gain_ramp = host_output_gain_ramp::OutputGainRamp::INACTIVE;
            entry.committed_transient_output_gain = Default::default();
            entry.scratch_transient_output_gain = Default::default();
            entry.committed_matrix_ramp = matrix_ramp::MatrixRamp::INACTIVE;
            entry.scratch_matrix_ramp = matrix_ramp::MatrixRamp::INACTIVE;
            entry.committed_bypass_ramp = bypass_ramp::BypassRamp::INACTIVE;
            entry.scratch_bypass_ramp = bypass_ramp::BypassRamp::INACTIVE;
            if let Some(history) = &mut entry.committed_aligned_dry {
                history.reset();
            }
            bypass_ramp::copy_dry(&mut entry.scratch_aligned_dry, &entry.committed_aligned_dry);
            entry.runtime.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_runtime_chain_reports_zero_storage_and_is_quiescent() {
        let mut chain = NativeRuntimeChain::empty();
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
        assert_eq!(chain.stateful_count(), 0);
        assert_eq!(chain.declared_pair_bytes(), 0);
        assert!(chain.committed_range_is_quiescent(0..0));
        assert_eq!(chain.storage_view().entry_capacity_bytes(), Some(0));
        assert_eq!(chain.storage_view().entries().count(), 0);
        chain.reset();
        chain.commit_block();
    }
}
