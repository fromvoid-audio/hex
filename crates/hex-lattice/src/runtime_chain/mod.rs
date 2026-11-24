//! Paired transactional native runtime chain, handoff, bypass/gain ramps and block processing.
mod construction;
mod process;
mod storage;
mod transitions;

pub use storage::{NativeRuntimeEntryStorage, NativeRuntimeStorage};
pub use transitions::{NativeInsertTransition, NativeRemovalTransition};

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
