//! # Purpose
//! Own validated native processor/instrument registries, borrowed resource contracts and bounded
//! runtime-history transactions and explicit borrowed foreign processing.
//!
//! # Dependencies
//! Uses hex-lattice-contract, hex-dsp and hex-tuning; providers supply original DSP through
//! explicit preparation capabilities.
//!
//! # Does not own
//! Does not own canonical edits, device I/O, host source leases or foreign-code containment. A
//! resource view or declaration never grants host publication or lifetime authority.
mod aux_input;
mod bypass_ramp;
#[cfg(any(test, feature = "conformance"))]
pub mod conformance;
pub use aux_input::{AuxiliaryBlockError, StereoAuxiliaryBlock};
mod foreign_processor;
mod host_input_gain_modulation;
mod host_output_gain_ramp;
mod matrix_ramp;
mod meter;
mod modulation;
#[cfg(test)]
mod modulation_graph_tests;
mod modulation_runtime;
mod modulation_source_routes;
mod modulation_sources;
mod owned_processor;
mod parameter_automation;
mod prepared_modulation;
pub use prepared_modulation::{MAX_PREPARED_MODULATION_BYTES, PreparedModulationState};
#[cfg(test)]
mod static_processing_tests;
mod tail_split;
pub use foreign_processor::{
    ForeignAutomation, ForeignAutomationProgram, ForeignAutomationSource, ForeignNote,
    ForeignParameterCapability, ForeignProcessorNode, ForeignProcessors,
    MAX_RETAINED_FOREIGN_PROGRAMS, PreparedForeignAutomation, PreparedForeignProcessor,
    RefuseForeign,
};
pub use parameter_automation::{
    ExternalParameterAutomation, HostAutomationLane, NativeParameterAutomation,
    NativeTransientOperation, NativeTransientValue, NormalizedAutomationValue,
    ParameterAutomationBlock, ParameterAutomationLane, ParameterAutomationPoint,
    ParameterAutomationTouchSpan, PreparedParameterAutomation, PreparedParameterAutomationLane,
    PreparedParameterAutomationPoint,
};
pub use tail_split::{TailSplit, TailSplitObserver};
mod resources;
mod runtime_ownership;
pub use hex_dsp::LfoWaveform;
pub use hex_lattice_contract::ModulationSourceId;
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, MAX_NATIVE_DEVICE_TYPES, NativeDeviceMetadata,
    NativeDeviceState,
};
pub use hex_tuning::{TuningTable, Waveform as TuningWaveform};
pub use meter::{DeviceMeter, DevicePeaks, DynamicsMeter, RenderWorkMeter, RenderWorkSample};
pub use modulation::{
    HostGainModulationRoute, MAX_MODULATION_BLOCK_FRAMES, MAX_MODULATION_ROUTES,
    MAX_MODULATION_SOURCES, ModulationBlock, ModulationRoute, ModulationTarget, PreparedModulation,
};
pub use modulation_runtime::ModulationRuntime;
pub use modulation_source_routes::SourceModulationRoute;
pub use modulation_sources::{ModulationSourceBank, PreparedModulationSource};
pub use owned_processor::{
    MAX_RESERVED_BYPASS_FRAMES, OwnedProcessorRuntime, PreparedOwnedProcessorRuntime,
    PreparedProcessorFactory, PreparedRuntimeDevice, PreparedRuntimeInstance,
};
pub use resources::{
    BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity, DeviceResourcePcm,
    DeviceResourceView, DeviceResources, EMPTY_DEVICE_RESOURCES,
};
use runtime_ownership::RuntimeOwnership;
use std::any::Any as StdAny;
use std::fmt;

mod analytic_continuation;
pub use analytic_continuation::{
    ANALYTIC_CONTINUATION_WORDS, AnalyticContinuation, AnalyticContinuationError,
    AnalyticContinuationFormat, AnalyticContinuationId, AnalyticParameterFrame,
    MAX_ANALYTIC_CONTINUATION_FRAMES, PreparedAnalyticContinuation,
};

/// A registered memoryless linear effect definition. Implementations own their
/// parameter codec in the contract trait and prepare a validated matrix here.
pub trait NativeDeviceDefinition: NativeDeviceMetadata {
    /// Bound for the immutable factory of an asset-free source effect with no
    /// host automation or modulation. Include its Arc backing and owned/shared
    /// children, but not the separately charged state, meter or runtime.
    /// Unknown providers cannot publish as compiled source effects.
    fn source_effect_factory_bytes(&self) -> Option<usize> {
        None
    }
    /// Control-only explicit shared-envelope capability, independent of older
    /// device-local envelope metadata. Default providers do not opt in.
    fn supports_parameter_automation(
        &self,
        _state: &NativeDeviceState,
        _parameter: hex_lattice_contract::ParameterId,
    ) -> Result<bool, DeviceContractError> {
        Ok(false)
    }
    /// Prepare a full identity-matrix processing law for shared automation.
    /// Factories/runtimes must independently confirm every current target;
    /// immutable program ownership remains with the host, never the provider.
    fn prepare_automated_runtime_instance(
        &self,
        _identity: ProcessorInstanceIdentity,
        _state: &NativeDeviceState,
        _context: &DevicePreparationContext<'_>,
        _parameters: &[hex_lattice_contract::ParameterId],
    ) -> Result<PreparedRuntimeDevice, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }

    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError>;

    /// Linear contribution for an admitted full device effect. Definitions
    /// with a stateful companion can override this while `prepare_matrix`
    /// continues to refuse matrix-only compilation of that state.
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        self.prepare_matrix(state)
    }

    /// General factory selection. Existing definitions retain the legacy path.
    /// Owned-only definitions override this and explicitly refuse their legacy
    /// prepare_processor interface rather than silently omit required DSP.
    fn prepare_runtime_factory(
        &self,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        self.prepare_processor(state, context).map(|processor| {
            processor
                .map(|processor| PreparedProcessorFactory::Legacy(std::sync::Arc::from(processor)))
        })
    }

    /// Prepare runtime-only state for the exact device instance. The provider
    /// may attach nonowning provenance from its actual prepared source; copied
    /// identities alone grant no endpoint or publication custody. Canonical
    /// state and save/history remain owned by the caller.
    fn prepare_instance_state(
        &self,
        _identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        _context: &DevicePreparationContext<'_>,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        self.migrate_state(state)
    }

    /// Control-side selection for one canonical chain/device. Instance-bound
    /// providers must validate this identity against their already prepared
    /// configuration and refuse missing instances. The identity alone grants
    /// no source, endpoint, graph publication or retirement custody.
    fn prepare_instance_runtime_factory(
        &self,
        _identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        self.prepare_runtime_factory(state, context)
    }

    /// Borrow the authored capabilities armed on the exact instance-bound
    /// source. This performs no SDK exchange or resource transfer.
    fn foreign_automation_source(
        &self,
        _identity: ProcessorInstanceIdentity,
        _state: &NativeDeviceState,
        _context: &DevicePreparationContext<'_>,
    ) -> Result<Option<ForeignAutomationSource<'_>>, DeviceContractError> {
        Ok(None)
    }

    /// Prepare an optional stateful processor using the active sample rate.
    /// Memoryless definitions keep the default `None` implementation.
    fn prepare_processor(
        &self,
        _state: &NativeDeviceState,
        _context: &crate::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        Ok(None)
    }
}

mod instrument;
mod instrument_gain;
mod instrument_pair;
mod instrument_runtime;
pub use instrument::{
    ANALYTIC_NOTE_PAYLOAD_WORDS, AnalyticNoteContinuation, MAX_SHARED_ANALYTIC_CONTEXT_BYTES,
    NativeInstrumentDefinition, NativeInstrumentRegistry, PreparedAnalyticNote,
    PreparedNativeInstrument, PreparedSharedAnalyticContext, PreparedSharedAnalyticNote,
    PreparedStorageDeclaration, SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS,
};
pub use instrument_gain::{
    INSTRUMENT_GAIN_UNITY, MAX_INSTRUMENT_OCCURRENCE_GAIN, NativeInstrumentOccurrenceGain,
};
pub use instrument_pair::{
    NativeOwnedInstrumentRuntimePair, OwnedInstrumentRuntime, PreparedOwnedInstrumentRuntime,
};
pub use instrument_runtime::{
    AnalyticInstrumentHistory, InstrumentHistoryRetargetPreparation, InstrumentParameterRuntime,
    MAX_INSTRUMENT_AUTOMATION_FRAMES, MutableInstrumentRuntime, NativeInstrumentAnalyticSeed,
    NativeInstrumentClock, NativeInstrumentEvent, NativeInstrumentExpression,
    NativeInstrumentRuntimeError, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
    PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime,
};

/// Immutable, control-prepared configuration for one stateful native effect.
/// Runtime construction happens before publication and is covered by the
/// definition's checked per-instance byte declaration. The optional latency
/// declaration is algorithmic delay in frames from fixed buffering or
/// lookahead; `None` means the latency is unknown. Filter phase/group delay,
/// audio-interface latency and intended effect tails are not declared here.
pub trait PreparedNativeProcessor: Send + Sync {
    /// Opt in only when continuing active processing during a brief wet/dry
    /// bypass transition preserves the device's history and tail semantics.
    fn supports_smoothed_bypass(&self) -> bool {
        false
    }
    /// Opt in to host-aligned bypass for an exact, fixed latency from 1 to
    /// 16 frames. The host keeps untrimmed dry history and continues ordinary
    /// wet processing and automation while bypassed. A bypass edit changes
    /// only the wet/dry ramp, never the graph latency or processor owner.
    fn supports_latency_aligned_bypass(&self) -> bool {
        false
    }
    fn parameter_automation_descriptor(
        &self,
        _parameter: hex_lattice_contract::ParameterId,
    ) -> Option<hex_lattice_contract::ParameterDescriptor> {
        None
    }
    fn supports_parameter_automation(&self, _parameter: hex_lattice_contract::ParameterId) -> bool {
        false
    }
    /// Exact semantics for sample-rate modulation through this prepared factory.
    /// Absence refuses delivery, independently of automation support.
    fn parameter_modulation_semantics(
        &self,
        _parameter: hex_lattice_contract::ParameterId,
    ) -> Option<hex_lattice_contract::ParameterSemantics> {
        None
    }
    fn runtime_version(&self) -> u32;
    /// Total bytes owned by one runtime: object plus every heap allocation,
    /// including reserved capacities, buffers and nested owned allocations.
    /// `size_of` alone is sufficient only for an entirely inline runtime.
    /// Immutable shared prepared configuration is accounted separately. The
    /// host charges this declaration for both primary and scratch twins.
    fn declared_runtime_bytes(&self) -> usize;
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime>;

    /// Return prepared linear magnitudes for the processor's two working lanes
    /// at one frequency, when available. Lane meaning is processor-defined
    /// (for example, mid/side); this is not necessarily a physical L/R transfer
    /// matrix. This control-side visualization query must not run in callbacks.
    fn frequency_response_at_hz(&self, _frequency_hz: f32) -> Option<[f32; 2]> {
        None
    }

    /// Describe residual output duration, excluding fixed algorithmic latency.
    /// Finite values are sample frames at this prepared
    /// processor's sample rate. A host must validate the value against its own
    /// finite work budget before admission, using checked frame arithmetic;
    /// in particular, `u64::MAX` is not an automatic acceptance. Unknown
    /// implementations conservatively make no finite reclamation-time promise.
    /// Reclamation must include both this residual and known fixed latency;
    /// use `NativeRuntimeChain::zero_input_drain_for_range` for that bound.
    fn output_tail(&self) -> OutputTail {
        OutputTail::Unbounded
    }

    /// Return known algorithmic latency in frames; `None` means unknown.
    /// Implementations may report zero when processing introduces no fixed
    /// buffering or lookahead delay.
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        None
    }
}

/// Prepared bound on residual output after the last nonzero input sample.
/// A `Finite` declaration promises quiescence after at most fixed algorithmic
/// latency plus the declared residual zero-input frames. The promise covers all
/// supported finite histories and stored states that can affect future output.
/// Host/device buffering and external signal tails are outside this contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputTail {
    /// After fixed algorithmic latency plus this many subsequent zero-input
    /// frames, the runtime must report quiescent. Zero residual duration can
    /// still leave samples pending inside fixed buffering.
    Finite { max_tail_frames: u64 },
    /// No finite tail bound is guaranteed. A runtime may still become
    /// quiescent, but callers must not infer a deadline from observed samples.
    Unbounded,
}

/// Audio-owned state with allocation-free ordinary processing/history after
/// admission. Prepared automation normally succeeds; malformed borrowed delivery
/// or a derived-control failure returns through the host's existing rollback.
pub trait NativeProcessorRuntime: StdAny + Send {
    /// Opt in when the host can ramp changed input matrix coefficients while
    /// the processor retains its committed history and authored settings.
    fn supports_manual_host_matrix_retarget(&self) -> bool {
        false
    }

    /// This native provider can replace an equivalent static matrix at a
    /// source modulation handoff without losing any filter or signal history.
    /// The host still requires the exact same authored configuration and rate.
    fn permits_memoryless_modulation_handoff(&self) -> bool {
        false
    }

    /// Opt in when processing a source release without an external clock,
    /// auxiliary input or automation is equivalent to ordinary processing,
    /// and ordinary history copy resumes that exact signal history.
    fn supports_detached_source_rejoin(&self) -> bool {
        false
    }

    /// Process an admitted stereo auxiliary block. `automation == None` uses
    /// the immutable prepared parameters. When `bypassed`, advance the same
    /// private history while leaving the main signal dry. The clock, automation,
    /// resources and block are borrowed for this call and must not be retained.
    /// Providers that declare an auxiliary input must implement this bounded,
    /// allocation-free entry; the default explicitly refuses it.
    fn process_block_with_auxiliary(
        &mut self,
        _block: StereoAuxiliaryBlock<'_>,
        _clock: Option<&dyn NativeInstrumentClock>,
        _automation: Option<&dyn NativeParameterAutomation>,
        _bypassed: bool,
        _resources: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn supports_parameter_automation(&self, _parameter: hex_lattice_contract::ParameterId) -> bool {
        false
    }
    fn process_block_with_automation(
        &mut self,
        _frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        _automation: &dyn NativeParameterAutomation,
        _resources: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn process_bypassed_block_with_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        automation: &dyn NativeParameterAutomation,
        resources: &dyn DeviceResources,
    ) -> Result<(), DeviceContractError> {
        for (offset, frame) in frames.iter_mut().enumerate() {
            let dry = *frame;
            let shifted = clock.map(|clock| OffsetClock { clock, offset });
            let values = OffsetAutomation {
                values: automation,
                offset,
            };
            let result = self.process_block_with_automation(
                std::slice::from_mut(frame),
                shifted
                    .as_ref()
                    .map(|clock| clock as &dyn NativeInstrumentClock),
                &values,
                resources,
            );
            *frame = dry;
            result?;
        }
        Ok(())
    }
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    );
    /// Process with the content-origin clock at this node's input. The host
    /// aligns this borrowed clock with compensated audio; processors must not
    /// retain it. Time-independent implementations preserve the existing path.
    fn process_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn crate::DeviceResources,
    ) {
        self.process_block(frames, _resources);
    }
    /// Advance private state while preserving a bypassed signal.
    fn process_bypassed_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        for frame in frames {
            let input = *frame;
            self.process_block(std::slice::from_mut(frame), _resources);
            *frame = input;
        }
    }
    /// Clock-aware counterpart preserving the existing specialized bypass
    /// operation by default. Time-aware processors should override this too
    /// when private bypass history depends on the content-origin clock.
    fn process_bypassed_block_with_clock(
        &mut self,
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        _resources: &dyn crate::DeviceResources,
    ) {
        self.process_bypassed_block(frames, _resources);
    }
    /// Enter a residual-output tail after the host's complete admission
    /// preflight. Both transaction twins receive this notification before
    /// clock-free processing. Freeze any last effective controls needed by
    /// that path without resetting signal history or transferring custody.
    /// No allocation, blocking, I/O or potentially final resource drops.
    /// Providers that already retain their effective controls need no action.
    fn begin_output_tail(&mut self) {}
    fn reset(&mut self);
    /// True only when this runtime cannot produce any future output under
    /// zero input and its current prepared configuration, including output
    /// from dormant delay lines or recursive state. A currently silent block
    /// alone is not proof. This query must be bounded, allocation-free,
    /// infallible, lock-free and read-only. Implementations may be conservative
    /// and keep returning false. The default is false so an older runtime never
    /// implies silence from uninspected state. Callers must inspect committed
    /// audio-owned state only; speculative twins are not evidence that the
    /// committed processor has drained.
    fn is_quiescent(&self) -> bool {
        false
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool;
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime);
    /// Whether this candidate runtime can keep the previous runtime history
    /// across an authored parameter change. Implementations must only return
    /// true when the existing history remains meaningful and the new
    /// configuration can transition without a reset. This does not authorize
    /// transferring history across identity, routing, bypass, rate, or runtime
    /// version changes; those are checked by `NativeRuntimeChain` first.
    fn can_retarget_parameters_from(&self, _previous: &dyn NativeProcessorRuntime) -> bool {
        false
    }
    /// Bounded live-transition admission (Pulse #4178). True when this
    /// candidate can take over `previous` (its committed runtime) only by
    /// capturing more still-audible transitions, such as outgoing routing or
    /// audition branches, than its fixed inline capacity holds. It is not an
    /// incompatibility: the host must then refuse the WHOLE handoff before
    /// any ownership mutation with a transient busy result and retry after
    /// rendering advances `previous`. Never answer this by reporting a
    /// history copy or retarget as unsupported, because the host treats those
    /// as reset, detached-tail or fresh-runtime handoffs instead of a refusal.
    /// It must become false once `previous` has rendered past its oldest
    /// captured transition, and after `reset`. The query must be bounded,
    /// allocation-free, infallible, lock-free and read-only. The default
    /// (false) is correct for runtimes that never hold live transitions.
    fn transition_busy_from(&self, _previous: &dyn NativeProcessorRuntime) -> bool {
        false
    }
    /// Idle-predecessor settlement (Pulse #4558). The host calls this on a
    /// committed runtime only while its graph renders no device chains (a
    /// paused transport without monitoring, keyboard or tail input), when a
    /// handoff would otherwise be `transition_busy_from` this runtime forever
    /// because nothing renders it. Its output is silent then and a resume
    /// ramps in from silence, so complete every pending live transition at
    /// once: discard only the outgoing, inaudible branches and keep the
    /// current routing and its history unchanged, as if the transitions had
    /// finished. Afterwards no candidate may be `transition_busy_from` this
    /// runtime. Must be bounded, allocation-free, infallible and lock-free.
    /// The host calls it only on a runtime that a candidate reports busy
    /// from, so the default `reset` (coarser: it also clears the current
    /// routing's frozen history) is correct for any runtime; providers with
    /// live transitions may override it to keep that history.
    fn settle_idle_transitions(&mut self) {
        self.reset();
    }
    fn as_any(&self) -> &dyn StdAny;
    /// Optional, transient live-audition controls. These are audio-owned and
    /// deliberately excluded from persisted device state.
    fn transient_audition(&mut self) -> Option<&mut dyn NativeProcessorAudition> {
        None
    }
    /// Read the currently requested transient selector without borrowing a
    /// mutable processor capability. Used only to transfer a held audition
    /// across a successful graph handoff; implementations must not expose
    /// authored state or mutate processing history here.
    fn transient_audition_requested_selection(&self) -> Option<ProcessorAuditionSelection> {
        None
    }
    /// Drain bounded provider telemetry after the enclosing render commits.
    /// Must not allocate, block, retain resources or change audible history.
    fn take_dynamics_meter(&mut self) -> Option<DynamicsMeter> {
        None
    }
}

/// Device-neutral selector for a transient processor audition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessorAuditionSelection {
    pub index: u32,
    pub lane: u8,
}

/// Optional capability implemented by processors that can audition a
/// sub-component without changing their authored state.
pub trait NativeProcessorAudition: Send {
    /// Check whether this selector can be activated in the current runtime.
    fn can_select(&self, selection: ProcessorAuditionSelection) -> bool;
    /// Return the last accepted selector (which may be fading out).
    fn requested_selection(&self) -> Option<ProcessorAuditionSelection>;
    /// Apply a previously preflighted selection or a smooth release.
    /// `can_select` is the complete admission check: after both twins pass it,
    /// this operation must be infallible and `None` must always be accepted.
    fn select(&mut self, selection: Option<ProcessorAuditionSelection>);
    /// Immediately clear transient state for lifecycle invalidation.
    fn cancel_immediately(&mut self);
}

/// Maximum admitted stateful instances in one runtime bank.
pub const MAX_STATEFUL_PROCESSORS: usize = 256;
/// Maximum control-declared storage for one effect runtime twin. Mutable
/// instrument limits are supplied separately by the engine's aggregate policy.
pub const MAX_PROCESSOR_RUNTIME_BYTES: usize = 262_144;
/// Maximum TOTAL owned transaction storage (both histories and all capacities).
/// Owned providers synchronize written intervals, unlike full-copy legacy twins.
/// Sixteen seconds of stereo history at 192 kHz fits below this 64-MiB limit.
/// Production large providers transfer banks on a retiring-donor handoff.
pub const MAX_OWNED_PROCESSOR_PAIR_BYTES: usize = 64 * 1024 * 1024;
/// Aggregate declared bytes in one runtime bank, counting one twin per instance.
/// The actual paired runtime storage doubles this bound (128 MiB). Engine candidate
/// and retained-owner budgets still account the entire declared allocation.
pub const MAX_PROCESSOR_BANK_RUNTIME_BYTES: usize = 64 * 1024 * 1024;

/// Borrowed signal boundary around an entire native effect entry.
/// Input precedes host input trim; Output follows effect, routing and output trim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeSignalPhase {
    Input,
    Output,
}

/// Typed owner identity for a prepared effect chain. Numeric IDs share the
/// canonical project allocator, while this tag keeps track, return and source
/// owners distinct during runtime history handoff. A source retains its identity
/// when the receiving track changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessorChainOwner {
    Main,
    Track(u64),
    Return(u64),
    Source(u64),
}

impl ProcessorChainOwner {
    fn is_valid(self) -> bool {
        match self {
            Self::Track(id) | Self::Return(id) | Self::Source(id) => id != 0,
            Self::Main => true,
        }
    }
}

/// Validated canonical effect placement for control-side preparation. Distinct
/// devices with equal type/state still have distinct preparation identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessorInstanceIdentity {
    owner: ProcessorChainOwner,
    device_id: u64,
}
impl ProcessorInstanceIdentity {
    pub fn new(owner: ProcessorChainOwner, device_id: u64) -> Result<Self, DeviceContractError> {
        if !owner.is_valid() || device_id == 0 {
            return Err(DeviceContractError::InvalidId);
        }
        Ok(Self { owner, device_id })
    }
    pub fn owner(self) -> ProcessorChainOwner {
        self.owner
    }
    pub fn device_id(self) -> u64 {
        self.device_id
    }
}

/// One processor prepared for a stable typed chain/device identity.
#[derive(Clone)]
pub struct PreparedProcessorInstance {
    pub meter: std::sync::Arc<DeviceMeter>,
    pub owner: ProcessorChainOwner,
    pub device_id: u64,
    pub device_type: DeviceTypeId,
    pub matrix: StereoMatrix,
    pub bypassed: bool,
    pub sample_rate: u32,
    /// Exact migrated configuration used to prepare this processor. Production
    /// compilation supplies this shared immutable state so revision
    /// handoff can distinguish unchanged settings from compatible history
    /// edits. Synthetic instances may omit it; callers must then conservatively
    /// treat configuration identity as unknown.
    pub configuration: Option<std::sync::Arc<NativeDeviceState>>,
    pub definition: Option<std::sync::Arc<dyn PreparedNativeProcessor>>,
}

impl PreparedProcessorInstance {
    /// Effective audible algorithmic latency from the actual prepared processor.
    /// A memoryless matrix and ordinary bypass have zero latency. An opted-in
    /// processor keeps its declared latency through bypass.
    pub fn algorithmic_latency_frames(&self) -> Option<u32> {
        if self.bypassed
            && self
                .definition
                .as_ref()
                .is_none_or(|definition| !definition.supports_latency_aligned_bypass())
        {
            Some(0)
        } else {
            self.definition.as_ref().map_or(Some(0), |definition| {
                definition.algorithmic_latency_frames()
            })
        }
    }
}

pub struct PreparedNativeDevice {
    pub matrix: StereoMatrix,
    pub processor: Option<Box<dyn PreparedNativeProcessor>>,
}
impl fmt::Debug for PreparedProcessorInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedProcessorInstance")
            .field("owner", &self.owner)
            .field("device_id", &self.device_id)
            .field("device_type", &self.device_type)
            .field("matrix", &self.matrix)
            .field("bypassed", &self.bypassed)
            .field("sample_rate", &self.sample_rate)
            .field("has_configuration_identity", &self.configuration.is_some())
            .field("stateful", &self.definition.is_some())
            .finish()
    }
}

/// One current prepared instance and its requested canonical value. The engine
/// owns source/epoch fencing and supplies this only with a valid stage origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeTransientInput {
    pub identity: ProcessorInstanceIdentity,
    pub operation: NativeTransientOperation,
}
/// Successful speculative processing, not physical delivery or a DSP settling
/// claim. The engine/audio owner commits or discards this Copy observation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeTransientApplication {
    pub identity: ProcessorInstanceIdentity,
    pub operation: NativeTransientOperation,
    pub offset: usize,
}
/// Borrowed segment context. Retains at most the first application until its
/// owner consumes it; any processing refusal clears the speculative observation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeTransientProcessing {
    pub input: NativeTransientInput,
    pub applied: Option<NativeTransientApplication>,
}
impl NativeTransientProcessing {
    pub fn new(input: NativeTransientInput) -> Self {
        Self {
            input,
            applied: None,
        }
    }
}

/// A fully available normalized window for one exact current processor.
#[derive(Clone, Copy)]
pub struct NativeExternalAutomation<'a> {
    pub identity: ProcessorInstanceIdentity,
    pub frame_offset: usize,
    pub values: &'a dyn ExternalParameterAutomation,
}

/// A stable, already-materialized set of per-processor windows. Implementors
/// must return the same identity/window throughout one processing transaction
/// without allocation, blocking, I/O or changing the underlying values. Each
/// window may contain several sorted distinct targets for that processor.
pub trait NativeExternalAutomationSet {
    fn window(&self, identity: ProcessorInstanceIdentity) -> Option<NativeExternalAutomation<'_>>;
}
impl NativeExternalAutomationSet for NativeExternalAutomation<'_> {
    fn window(&self, identity: ProcessorInstanceIdentity) -> Option<NativeExternalAutomation<'_>> {
        (self.identity == identity).then_some(*self)
    }
}
struct ExternalSegment<'a> {
    values: &'a dyn ExternalParameterAutomation,
    start: usize,
    frames: usize,
}
impl ExternalParameterAutomation for ExternalSegment<'_> {
    fn frames(&self) -> usize {
        self.frames
    }
    fn targets(&self) -> &[hex_lattice_contract::DeviceAutomationTarget] {
        self.values.targets()
    }
    fn value_at(
        &self,
        target: hex_lattice_contract::DeviceAutomationTarget,
        offset: usize,
    ) -> Option<NormalizedAutomationValue> {
        if offset >= self.frames {
            return None;
        }
        self.values
            .value_at(target, self.start.checked_add(offset)?)
    }
}

struct OffsetParameterValues<'a> {
    values: &'a dyn NativeParameterAutomation,
    offset: usize,
}
impl NativeParameterAutomation for OffsetParameterValues<'_> {
    fn parameter_ids(&self) -> &[hex_lattice_contract::ParameterId] {
        self.values.parameter_ids()
    }
    fn value_at(
        &self,
        id: hex_lattice_contract::ParameterId,
        offset: usize,
    ) -> Option<hex_lattice_contract::ParameterValue> {
        self.values.value_at(id, self.offset.checked_add(offset)?)
    }
    fn values_at(
        &self,
        offset: usize,
        output: &mut [Option<hex_lattice_contract::ParameterValue>],
    ) -> Result<(), DeviceContractError> {
        self.values.values_at(
            self.offset
                .checked_add(offset)
                .ok_or(DeviceContractError::InvalidParameterValue)?,
            output,
        )
    }
}

struct RuntimeEntry {
    modulation: Option<ModulationRuntime>,
    automation: PreparedParameterAutomation,
    automation_frozen: bool,
    // Host effective trims are transactional too: a late render refusal cannot
    // freeze speculative controls into a retained output suffix.
    committed_host_gains: [f32; 2],
    scratch_host_gains: [f32; 2],
    committed_output_gain_ramp: host_output_gain_ramp::OutputGainRamp,
    scratch_output_gain_ramp: host_output_gain_ramp::OutputGainRamp,
    committed_transient_output_gain: host_output_gain_ramp::TransientGain,
    scratch_transient_output_gain: host_output_gain_ramp::TransientGain,
    // Static matrix ramp twins; see `matrix_ramp` for the handoff law.
    committed_matrix_ramp: matrix_ramp::MatrixRamp,
    scratch_matrix_ramp: matrix_ramp::MatrixRamp,
    meter: std::sync::Arc<DeviceMeter>,
    metering: bool,
    input_gain: f32,
    output_gain: f32,
    owner: ProcessorChainOwner,
    device_id: u64,
    device_type: DeviceTypeId,
    matrix: StereoMatrix,
    bypassed: bool,
    runtime_version: u32,
    sample_rate: u32,
    configuration: Option<std::sync::Arc<NativeDeviceState>>,
    output_tail: OutputTail,
    algorithmic_latency_frames: Option<u32>,
    supports_smoothed_bypass: bool,
    committed_bypass_ramp: bypass_ramp::BypassRamp,
    scratch_bypass_ramp: bypass_ramp::BypassRamp,
    committed_aligned_dry: Option<bypass_ramp::AlignedDry>,
    scratch_aligned_dry: Option<bypass_ramp::AlignedDry>,
    runtime: RuntimeOwnership,
}

impl RuntimeEntry {
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

struct OffsetAutomation<'a> {
    values: &'a dyn NativeParameterAutomation,
    offset: usize,
}
impl NativeParameterAutomation for OffsetAutomation<'_> {
    fn parameter_ids(&self) -> &[hex_lattice_contract::ParameterId] {
        self.values.parameter_ids()
    }
    fn value_at(
        &self,
        id: hex_lattice_contract::ParameterId,
        offset: usize,
    ) -> Option<hex_lattice_contract::ParameterValue> {
        self.values.value_at(id, self.offset.checked_add(offset)?)
    }
    fn values_at(
        &self,
        offset: usize,
        output: &mut [Option<hex_lattice_contract::ParameterValue>],
    ) -> Result<(), DeviceContractError> {
        self.values.values_at(
            self.offset
                .checked_add(offset)
                .ok_or(DeviceContractError::InvalidParameterValue)?,
            output,
        )
    }
}
struct OffsetClock<'a> {
    clock: &'a dyn NativeInstrumentClock,
    offset: usize,
}

impl NativeInstrumentClock for OffsetClock<'_> {
    fn arrangement_meter_at_offset(&self, offset: usize) -> Option<(u8, u8)> {
        self.clock
            .arrangement_meter_at_offset(self.offset.checked_add(offset)?)
    }

    fn arrangement_cycle_at_offset(&self, offset: usize) -> Option<(u64, u64)> {
        self.clock
            .arrangement_cycle_at_offset(self.offset.checked_add(offset)?)
    }

    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        self.clock
            .tempo_micro_bpm_at_offset(self.offset.checked_add(offset)?)
    }

    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .arrangement_beat_at_offset(self.offset.checked_add(offset)?)
    }
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock.beat_at_offset(self.offset.checked_add(offset)?)
    }

    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .arrangement_frame_at_offset(self.offset.checked_add(offset)?)
    }

    fn authored_arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .authored_arrangement_frame_at_offset(self.offset.checked_add(offset)?)
    }

    fn loop_pass_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .loop_pass_at_offset(self.offset.checked_add(offset)?)
    }
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.clock
            .performance_frame_at_offset(self.offset.checked_add(offset)?)
    }

    fn transport_is_running(&self) -> Option<bool> {
        self.clock.transport_is_running()
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
    entries: Vec<RuntimeEntry>,
    stateful_count: usize,
    declared_pair_bytes: usize,
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
            runtime_version: _,
            sample_rate: _,
            configuration: _,
            output_tail: _,
            algorithmic_latency_frames: _,
            supports_smoothed_bypass: _,
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
            .map_or(Some(0), ModulationRuntime::heap_storage_bytes)
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
    /// Owned transactions have a different physical proof and never masquerade
    /// as a pair of legacy boxes. A declaration is not a native custody proof.
    pub fn is_owned_transaction(&self) -> bool {
        matches!(self.entry.runtime, RuntimeOwnership::Owned(_))
    }
    /// Borrow only actual scalar foreign facts. This transfers no endpoint or
    /// source custody and cannot classify another runtime family as foreign.
    pub fn foreign_processor(&self) -> Option<PreparedForeignProcessor> {
        match &self.entry.runtime {
            RuntimeOwnership::Foreign(value) => Some(value.prepared),
            _ => None,
        }
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
        (match (&self.entry.modulation, &prepared.modulation) {
            (Some(runtime), Some(declaration)) => declaration.matches_runtime(runtime),
            (None, None) => true,
            _ => false,
        }) && self.entry.automation == prepared.automation
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
    /// recipe. The recipe keeps its immutable declaration. Only the runtime
    /// allocates private modulation history. Source recipes have no automation.
    pub fn new_from_source_recipe(
        instances: &[PreparedRuntimeInstance],
    ) -> Result<Self, DeviceContractError> {
        if instances.iter().any(|instance| {
            !matches!(instance.owner, ProcessorChainOwner::Source(_))
                || !instance.automation.is_empty()
        }) {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        Self::from_prepared(instances.iter().map(|instance| {
            let entry = PreparedRuntimeInstance {
                modulation: None,
                automation: Default::default(),
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
                    stateful_count = stateful_count
                        .checked_add(1)
                        .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
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
                        PreparedProcessorFactory::Foreign(_) => false,
                    });
            let reserved_frames = instance
                .definition
                .as_ref()
                .map(PreparedProcessorFactory::reserved_bypass_frames)
                .transpose()?
                .flatten();
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
            let runtime = RuntimeOwnership::prepare(instance.definition.as_ref())?;
            let modulation = prepared_modulation
                .map(PreparedModulationState::prepare)
                .transpose()?;
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
                runtime_version,
                sample_rate: instance.sample_rate,
                configuration: instance.configuration,
                output_tail,
                algorithmic_latency_frames: latency,
                supports_smoothed_bypass,
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
                    )
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
        if range.start > range.end
            || range.end > self.entries.len()
            || range.end > other.entries.len()
        {
            return false;
        }
        self.entries[range.clone()]
            .iter()
            .zip(&other.entries[range])
            .all(|(left, right)| {
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
                        (Some(left), Some(right)) => {
                            std::sync::Arc::ptr_eq(left, right) || left.processing_eq_on_callback(right)
                        }
                        _ => false,
                    }
            })
    }

    /// Preflight a retiring-donor transfer for a source effect recipe. The
    /// native provider, trims and topology must still match. A live edit may
    /// replace a static matrix with a provider that explicitly proves it has
    /// no signal history to carry. Other live edits preserve committed DSP
    /// history. Strict continuation also compares the prepared declaration.
    pub fn can_transfer_source_recipe_history_from(
        &self,
        previous: &Self,
        edit_modulation: bool,
    ) -> bool {
        self.len() == previous.len()
            && self.entries.iter().zip(&previous.entries).enumerate().all(
                |(index, (left, right))| {
                    !matches!(left.runtime, RuntimeOwnership::Foreign(_))
                        && !matches!(right.runtime, RuntimeOwnership::Foreign(_))
                        && left.bypassed == right.bypassed
                        && left.input_gain == right.input_gain
                        && left.output_gain == right.output_gain
                        && left.algorithmic_latency_frames == right.algorithmic_latency_frames
                        && left.automation.is_empty()
                        && right.automation.is_empty()
                        && ((left.output_tail == right.output_tail
                            && self.configuration_matches_range(previous, index..index + 1))
                            || edit_modulation && left.matches_memoryless_modulation_handoff(right))
                        && (edit_modulation
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
        self.entries.get(range).is_some_and(|entries| {
            entries.iter().all(|entry| {
                entry.runtime.committed_is_quiescent()
                    && entry
                        .committed_aligned_dry
                        .as_ref()
                        .is_none_or(bypass_ramp::AlignedDry::is_quiet)
            })
        })
    }

    /// Speculative counterpart used only after processing scratch state in the
    /// current uncommitted render block.
    pub fn speculative_range_is_quiescent(&self, range: std::ops::Range<usize>) -> bool {
        self.entries.get(range).is_some_and(|entries| {
            entries.iter().all(|entry| {
                entry.runtime.speculative_is_quiescent()
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
        self.can_inherit_preflight(previous, excluded_old_range, false)
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
        self.can_inherit_preflight(previous, completed_old_range, true)
    }

    fn can_inherit_preflight(
        &self,
        previous: &Self,
        excluded_old_range: std::ops::Range<usize>,
        completed_foreign_only: bool,
    ) -> bool {
        if excluded_old_range.start > excluded_old_range.end
            || excluded_old_range.end > previous.entries.len()
        {
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
            (excluded_old_range.contains(&index)
                && (!completed_foreign_only
                    || (matches!(&old.runtime, RuntimeOwnership::Foreign(_))
                        && old.runtime.committed_is_quiescent())))
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
    fn same_runtime_identity(a: &RuntimeEntry, b: &RuntimeEntry) -> bool {
        a.owner == b.owner
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
        self.inherit_host_stage_ramps(previous, &excluded_old_range);
        self.inherit_bypass_ramps(previous, &excluded_old_range);
        self.inherit_modulation(previous, &excluded_old_range);
        for entry in &mut self.entries {
            if let Some((index, old)) = previous.entries.iter().enumerate().find(|(_, old)| {
                Self::same_runtime_identity(entry, old) && old.runtime.is_stateful()
            }) && !excluded_old_range.contains(&index)
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
        self.transfer_history_excluding_unchecked(previous, 0..0);
    }

    /// Transfer only histories not retained by the donor's old output suffix.
    pub fn transfer_history_excluding_old_range(
        &mut self,
        previous: &mut Self,
        excluded: std::ops::Range<usize>,
    ) -> Result<(), DeviceContractError> {
        if excluded.start > excluded.end || excluded.end > previous.entries.len() {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        self.transfer_history_excluding_unchecked(previous, excluded);
        Ok(())
    }
    fn transfer_history_excluding_unchecked(
        &mut self,
        previous: &mut Self,
        excluded: std::ops::Range<usize>,
    ) {
        self.inherit_host_stage_ramps(previous, &excluded);
        self.inherit_bypass_ramps(previous, &excluded);
        self.inherit_modulation(previous, &excluded);
        for entry in &mut self.entries {
            if let Some((index, old)) = previous.entries.iter_mut().enumerate().find(|(_, old)| {
                Self::same_runtime_identity(entry, old) && old.runtime.is_stateful()
            }) && !excluded.contains(&index)
            {
                entry.runtime.transfer_history_from(&mut old.runtime);
            }
        }
    }

    fn inherit_modulation(&mut self, previous: &Self, excluded: &std::ops::Range<usize>) {
        for entry in &mut self.entries {
            if let Some((index, old)) = previous.entries.iter().enumerate().find(|(_, old)| {
                entry.owner == old.owner
                    && entry.device_id == old.device_id
                    && entry.device_type == old.device_type
                    && entry.sample_rate == old.sample_rate
            }) && !excluded.contains(&index)
                && let (Some(current), Some(old)) = (&mut entry.modulation, &old.modulation)
            {
                current.inherit_graph_committed(old);
            }
        }
    }

    /// Reuse one same-identity donor lookup for the independent output-trim
    /// and static-matrix trajectories. Excluded donors, bypass and automation
    /// keep the existing matrix law; the scalar proof lives with its owner.
    fn inherit_host_stage_ramps(&mut self, previous: &Self, excluded: &std::ops::Range<usize>) {
        for entry in &mut self.entries {
            let old = previous
                .entries
                .iter()
                .enumerate()
                .find(|(_, old)| Self::same_runtime_identity(entry, old));
            host_output_gain_ramp::inherit(entry, old, excluded);
            let ramp = match old {
                Some((index, old)) => matrix_ramp::MatrixRamp::inherited(
                    old.committed_matrix_ramp,
                    old.matrix,
                    entry.matrix,
                    entry.sample_rate,
                    !excluded.contains(&index)
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

    fn inherit_bypass_ramps(&mut self, previous: &Self, excluded: &std::ops::Range<usize>) {
        for entry in &mut self.entries {
            let old = previous
                .entries
                .iter()
                .enumerate()
                .find(|(_, old)| Self::same_runtime_identity(entry, old));
            let ramp = match old {
                Some((index, old))
                    if !excluded.contains(&index)
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
                    bypass_ramp::BypassRamp::inherited(
                        old.committed_bypass_ramp,
                        old.bypassed,
                        entry.bypassed,
                        entry.sample_rate,
                    )
                }
                _ => bypass_ramp::BypassRamp::INACTIVE,
            };
            let inherited_dry = if let Some((index, old)) = old
                && !excluded.contains(&index)
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
            entry.runtime.begin();
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
        self.process_range_tapped_with_foreign(
            range,
            frames,
            frame_offset,
            clock,
            output_clock,
            foreign,
            &mut |_, _, _, _, _, _| {},
        )
    }

    /// Stage a graph segment through the host's actual borrowed endpoints.
    /// Successful processing still grants no dispatch: final host-format
    /// acceptance remains exclusively with the audio owner. On error all
    /// foreign work for this callback is revoked; owned DSP can then roll back.
    /// Independent stage origins follow `process_range_with_clock`.
    #[allow(clippy::too_many_arguments)] // Two explicit borrowed stage clocks and existing foreign/tap endpoints.
    pub fn process_range_tapped_with_foreign(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        foreign: &mut dyn ForeignProcessors,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        self.process_range_tapped_with_foreign_and_input(
            range,
            frames,
            frame_offset,
            clock,
            output_clock,
            foreign,
            None,
            observer,
        )
    }

    /// Validate an inline request against the exact prepared current target.
    /// A missing or frozen instance returns false without mutating the chain.
    /// Host trims cannot be applied while bypassed; provider controls retain
    /// their existing internal-history advancement under bypass.
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
        if entry.bypassed && input.operation.target().is_host() {
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

    /// Same transaction and taps with one prepared transient input. Only the
    /// exact current, non-frozen instance can consume it. A successful range
    /// stages its first nonempty application; final host delivery is not owned
    /// here. Provider values advance under bypass; skipped host trims do not.
    #[allow(clippy::too_many_arguments)] // Existing borrowed endpoints plus one inline transient context.
    pub fn process_range_tapped_with_foreign_and_input(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        foreign: &mut dyn ForeignProcessors,
        transient: Option<&mut NativeTransientProcessing>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        self.process_range_tapped_with_foreign_input_and_external(
            range,
            frames,
            frame_offset,
            clock,
            output_clock,
            foreign,
            transient,
            None,
            observer,
        )
    }

    /// Same processing transaction with available normalized recorded samples.
    /// Frozen retained processors do not read a current external source.
    #[allow(clippy::too_many_arguments)]
    pub fn process_range_tapped_with_foreign_input_and_external(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        foreign: &mut dyn ForeignProcessors,
        transient: Option<&mut NativeTransientProcessing>,
        external: Option<NativeExternalAutomation<'_>>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        let set = external
            .as_ref()
            .map(|window| window as &dyn NativeExternalAutomationSet);
        self.process_range_tapped_with_foreign_input_and_external_set(
            range,
            frames,
            frame_offset,
            clock,
            output_clock,
            foreign,
            transient,
            set,
            observer,
        )
    }

    /// The same transaction with at most one materialized target group per
    /// exact processor. Frozen tails cannot consume current groups.
    #[allow(clippy::too_many_arguments)]
    pub fn process_range_tapped_with_foreign_input_and_external_set(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        foreign: &mut dyn ForeignProcessors,
        transient: Option<&mut NativeTransientProcessing>,
        external: Option<&dyn NativeExternalAutomationSet>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        self.process_tapped(
            range,
            frames,
            None,
            frame_offset,
            clock,
            output_clock,
            foreign,
            transient,
            external,
            observer,
        )
    }

    /// Run one exact effect stage with its admitted key in the ordinary
    /// begin/commit/rollback transaction. Main and key are borrowed only for
    /// this call. A stereo key requires a stateful native identity-matrix stage.
    /// The caller rolls back the complete block on any stage refusal.
    #[allow(clippy::too_many_arguments)]
    pub fn process_auxiliary_stage(
        &mut self,
        stage: usize,
        block: StereoAuxiliaryBlock<'_>,
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        transient: Option<&mut NativeTransientProcessing>,
        external: Option<NativeExternalAutomation<'_>>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        let set = external
            .as_ref()
            .map(|window| window as &dyn NativeExternalAutomationSet);
        self.process_auxiliary_stage_with_external_set(
            stage,
            block,
            frame_offset,
            clock,
            output_clock,
            transient,
            set,
            observer,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process_auxiliary_stage_with_external_set(
        &mut self,
        stage: usize,
        block: StereoAuxiliaryBlock<'_>,
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        transient: Option<&mut NativeTransientProcessing>,
        external: Option<&dyn NativeExternalAutomationSet>,
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
        self.process_tapped(
            stage..end,
            main,
            Some(key),
            frame_offset,
            clock,
            output_clock,
            &mut RefuseForeign,
            transient,
            external,
            observer,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn process_tapped(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
        key: Option<&[StereoFrame]>,
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        foreign: &mut dyn ForeignProcessors,
        mut transient: Option<&mut NativeTransientProcessing>,
        external: Option<&dyn NativeExternalAutomationSet>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        let requested = transient.as_ref().map(|context| context.input);
        let mut applied = None;
        let result = (|| {
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
                Self::process_entry_with_foreign(
                    entry,
                    frames,
                    key,
                    frame_offset,
                    clock,
                    output_clock,
                    foreign,
                    input
                        .and_then(NativeTransientOperation::value)
                        .filter(|value| !matches!(value, NativeTransientValue::HostOutputGain(_))),
                    window,
                )?;
                if !frames.is_empty()
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
            None,
            0,
            clock,
            output_clock,
            &mut RefuseForeign,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // Borrowed stages plus independent live/recorded inputs.
    fn process_entry_with_foreign(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        key: Option<&[StereoFrame]>,
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        foreign: &mut dyn ForeignProcessors,
        input: Option<NativeTransientValue>,
        external: Option<NativeExternalAutomation<'_>>,
    ) -> Result<(), DeviceContractError> {
        let aligned = entry.scratch_aligned_dry.is_some();
        let aligned_bypass = aligned && entry.bypassed;
        if !entry.scratch_bypass_ramp.is_active() && !aligned_bypass {
            if let Some(history) = &mut entry.scratch_aligned_dry {
                for &frame in frames.iter() {
                    history.advance(frame);
                }
            }
            return Self::process_entry_with_foreign_inner(
                entry,
                frames,
                key,
                frame_offset,
                clock,
                output_clock,
                foreign,
                input,
                external,
            );
        }
        let mut offset = 0;
        while (aligned_bypass || entry.scratch_bypass_ramp.is_active()) && offset < frames.len() {
            const CHUNK: usize = 64;
            let mut count = (frames.len() - offset).min(CHUNK);
            if !aligned {
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
                key.map(|key| &key[offset..end]),
                shifted_frame_offset,
                shifted_clock
                    .as_ref()
                    .map(|c| c as &dyn NativeInstrumentClock),
                shifted_output_clock
                    .as_ref()
                    .map(|c| c as &dyn NativeInstrumentClock),
                foreign,
                input,
                external,
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
        if let Some(history) = &mut entry.scratch_aligned_dry {
            for &frame in &frames[offset..] {
                history.advance(frame);
            }
        }
        let shifted_clock = clock.map(|clock| OffsetClock { clock, offset });
        let shifted_output_clock = output_clock.map(|clock| OffsetClock { clock, offset });
        Self::process_entry_with_foreign_inner(
            entry,
            &mut frames[offset..],
            key.map(|key| &key[offset..]),
            frame_offset
                .checked_add(offset)
                .ok_or(DeviceContractError::InvalidDescriptor)?,
            shifted_clock
                .as_ref()
                .map(|c| c as &dyn NativeInstrumentClock),
            shifted_output_clock
                .as_ref()
                .map(|c| c as &dyn NativeInstrumentClock),
            foreign,
            input,
            external,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn process_entry_with_foreign_inner(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        key: Option<&[StereoFrame]>,
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        foreign: &mut dyn ForeignProcessors,
        input: Option<NativeTransientValue>,
        external: Option<NativeExternalAutomation<'_>>,
    ) -> Result<(), DeviceContractError> {
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
                    .map(PreparedForeignAutomation::borrow),
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
            foreign.process_segment(node, frame_offset, frames, clock)?;
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
            entry.committed_bypass_ramp = entry.scratch_bypass_ramp;
            bypass_ramp::copy_dry(&mut entry.committed_aligned_dry, &entry.scratch_aligned_dry);
            entry.runtime.commit();
            let reading = entry.runtime.take_dynamics_meter();
            if let Some(reading) = reading.filter(|_| entry.metering && !entry.bypassed) {
                entry.meter.observe_dynamics(reading);
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
            entry.runtime.rollback();
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

pub use hex_dsp::{InvalidMatrix, StereoFrame, StereoMatrix};

#[cfg(test)]
mod compensation_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{ParameterDescriptor, ParameterId, ParameterValue};

    #[test]
    fn source_processor_identity_is_nonzero_and_independent_of_track_and_return() {
        let source = ProcessorInstanceIdentity::new(ProcessorChainOwner::Source(4), 7).unwrap();
        assert_eq!(source.owner(), ProcessorChainOwner::Source(4));
        assert_ne!(
            source,
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(4), 7).unwrap()
        );
        assert_ne!(
            source,
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Return(4), 7).unwrap()
        );
        assert!(ProcessorInstanceIdentity::new(ProcessorChainOwner::Source(0), 7).is_err());
    }
    const DYNAMICS_METER_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.dynamics-meter");

    struct DynamicsMeterPrepared;
    struct DynamicsMeterRuntime {
        pending: DynamicsMeter,
    }

    impl PreparedNativeProcessor for DynamicsMeterPrepared {
        fn runtime_version(&self) -> u32 {
            1
        }

        fn declared_runtime_bytes(&self) -> usize {
            std::mem::size_of::<DynamicsMeterRuntime>()
        }

        fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
            Box::new(DynamicsMeterRuntime {
                pending: DynamicsMeter::default(),
            })
        }
    }

    impl NativeProcessorRuntime for DynamicsMeterRuntime {
        fn process_block(
            &mut self,
            frames: &mut [StereoFrame],
            _resources: &dyn crate::DeviceResources,
        ) {
            self.pending = DynamicsMeter {
                reduction_db: [
                    frames
                        .iter()
                        .map(|frame| frame[0].abs())
                        .fold(0.0, f32::max),
                    frames
                        .iter()
                        .map(|frame| frame[1].abs())
                        .fold(0.0, f32::max),
                ],
                clipped: frames.iter().any(|frame| frame[0].abs() > 1.0),
                post_input_peak: frames
                    .iter()
                    .flatten()
                    .map(|sample| sample.abs())
                    .fold(0.0, f32::max),
            };
        }

        fn reset(&mut self) {
            self.pending = DynamicsMeter::default();
        }

        fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
            previous.as_any().is::<Self>()
        }

        fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
            if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
                self.pending = previous.pending;
            }
        }

        fn as_any(&self) -> &dyn StdAny {
            self
        }

        fn take_dynamics_meter(&mut self) -> Option<DynamicsMeter> {
            Some(std::mem::take(&mut self.pending))
        }
    }

    fn dynamics_meter_chain(bypassed: bool) -> NativeRuntimeChain {
        NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id: 9,
            device_type: DYNAMICS_METER_TYPE,
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(std::sync::Arc::new(DynamicsMeterPrepared)),
        }])
        .expect("prepared dynamics meter runtime")
    }

    #[test]
    fn dynamics_meter_publishes_only_the_committed_active_runtime_reading() {
        let mut chain = dynamics_meter_chain(false);
        chain.set_metering(true);

        chain.begin_block();
        let mut speculative = [[2.5, 4.0]];
        chain.process_block(&mut speculative).unwrap();
        chain.rollback_block();
        assert_eq!(
            chain.entries[0].meter.take_dynamics(),
            DynamicsMeter::default()
        );
        // The input tap combines both channels; the output tap keeps left and
        // right apart. Neither depends on committed provider dynamics readings.
        assert_eq!(
            chain.entries[0].meter.take_peaks(),
            crate::DevicePeaks {
                input: 4.0,
                output: [2.5, 4.0],
            }
        );

        chain.begin_block();
        let mut committed = [[0.25, 0.75]];
        chain.process_block(&mut committed).unwrap();
        chain.commit_block();

        assert_eq!(
            chain.entries[0].meter.take_dynamics(),
            DynamicsMeter {
                reduction_db: [0.25, 0.75],
                clipped: false,
                post_input_peak: 0.75,
            }
        );
        assert_eq!(
            chain.entries[0].meter.take_peaks(),
            crate::DevicePeaks {
                input: 0.75,
                output: [0.25, 0.75],
            }
        );
    }

    #[test]
    fn dynamics_meter_drains_readings_while_metering_is_disabled_or_device_bypassed() {
        let mut disabled = dynamics_meter_chain(false);
        disabled.begin_block();
        let mut frames = [[2.0, 3.0]];
        disabled.process_block(&mut frames).unwrap();
        disabled.commit_block();
        assert_eq!(
            disabled.entries[0].meter.take_dynamics(),
            DynamicsMeter::default()
        );

        disabled.set_metering(true);
        disabled.begin_block();
        let mut quiet = [[0.0, 0.0]];
        disabled.process_block(&mut quiet).unwrap();
        disabled.commit_block();
        assert_eq!(
            disabled.entries[0].meter.take_dynamics(),
            DynamicsMeter::default()
        );

        let mut bypassed = dynamics_meter_chain(true);
        bypassed.set_metering(true);
        bypassed.begin_block();
        let mut bypassed_frames = [[2.0, 3.0]];
        bypassed.process_block(&mut bypassed_frames).unwrap();
        bypassed.commit_block();
        assert_eq!(
            bypassed.entries[0].meter.take_dynamics(),
            DynamicsMeter::default()
        );
        assert_eq!(
            bypassed.entries[0].meter.take_peaks(),
            crate::DevicePeaks {
                input: 3.0,
                output: [2.0, 3.0],
            }
        );
    }

    const SNAPSHOT_PATCH_PARAMETER: ParameterId = ParameterId::from_static(1);
    const SNAPSHOT_PATCH_DESCRIPTORS: &[ParameterDescriptor] = &[ParameterDescriptor {
        id: SNAPSHOT_PATCH_PARAMETER,
        label: "level",
        kind: hex_lattice_contract::ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.25),
        presentation: hex_lattice_contract::ParameterPresentation::Linear,
    }];
    const SNAPSHOT_PATCH_INPUT_GAIN: i32 = -12_345;
    const SNAPSHOT_PATCH_OUTPUT_GAIN: i32 = 6_789;

    struct SnapshotPatchEffectDefinition;

    impl NativeDeviceMetadata for SnapshotPatchEffectDefinition {
        fn type_id(&self) -> DeviceTypeId {
            DeviceTypeId::from_static("test.effect-snapshot-patch")
        }

        fn state_version(&self) -> u32 {
            1
        }

        fn descriptors(&self) -> &'static [ParameterDescriptor] {
            SNAPSHOT_PATCH_DESCRIPTORS
        }

        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(NativeDeviceMetadata::type_id(self), 1, vec![64])
        }

        fn migrate_state(
            &self,
            state: &NativeDeviceState,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if state.type_id() != NativeDeviceMetadata::type_id(self) || state.state_version() != 1
            {
                return Err(DeviceContractError::MalformedState);
            }
            NativeDeviceState::new(
                NativeDeviceMetadata::type_id(self),
                1,
                state.payload().to_vec(),
            )
        }

        fn read_parameter(
            &self,
            state: &NativeDeviceState,
            parameter: ParameterId,
        ) -> Result<ParameterValue, DeviceContractError> {
            if parameter != SNAPSHOT_PATCH_PARAMETER {
                return Err(DeviceContractError::UnknownParameter);
            }
            let value = *state
                .payload()
                .first()
                .ok_or(DeviceContractError::MalformedState)?;
            Ok(ParameterValue::Float(f32::from(value) / 255.0))
        }

        fn patch_parameter(
            &self,
            _state: &NativeDeviceState,
            parameter: ParameterId,
            value: ParameterValue,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if parameter != SNAPSHOT_PATCH_PARAMETER {
                return Err(DeviceContractError::UnknownParameter);
            }
            let ParameterValue::Float(value) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            NativeDeviceState::new(
                NativeDeviceMetadata::type_id(self),
                1,
                vec![(value * 255.0).round() as u8],
            )
        }

        fn patch_parameters(
            &self,
            state: &NativeDeviceState,
            snapshot: &hex_lattice_contract::DeviceParameterSnapshot,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if state.input_gain_millidb() != SNAPSHOT_PATCH_INPUT_GAIN
                || state.output_gain_millidb() != SNAPSHOT_PATCH_OUTPUT_GAIN
            {
                return Err(DeviceContractError::MalformedState);
            }
            let mut patched = self.migrate_state(state)?;
            for &(parameter, value) in snapshot.entries() {
                patched = self.patch_parameter(&patched, parameter, value)?;
            }
            Ok(patched)
        }
    }

    impl NativeDeviceDefinition for SnapshotPatchEffectDefinition {
        fn prepare_matrix(
            &self,
            _state: &NativeDeviceState,
        ) -> Result<StereoMatrix, DeviceContractError> {
            Ok(StereoMatrix::identity())
        }
    }

    #[test]
    fn effect_snapshot_patch_preserves_host_gains_across_migrations() {
        let definition = SnapshotPatchEffectDefinition;
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let state = registry
            .default_state(NativeDeviceMetadata::type_id(&definition))
            .unwrap()
            .with_input_gain_millidb(SNAPSHOT_PATCH_INPUT_GAIN)
            .unwrap()
            .with_output_gain_millidb(SNAPSHOT_PATCH_OUTPUT_GAIN)
            .unwrap()
            .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
                slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
                asset: hex_lattice_contract::DeviceAssetKey::new(42).unwrap(),
            }])
            .unwrap();
        let snapshot = hex_lattice_contract::DeviceParameterSnapshot::new(&[(
            SNAPSHOT_PATCH_PARAMETER,
            ParameterValue::Float(1.0),
        )])
        .unwrap();

        let patched = registry.patch_parameters(&state, &snapshot).unwrap();

        assert_eq!(patched.input_gain_millidb(), SNAPSHOT_PATCH_INPUT_GAIN);
        assert_eq!(patched.output_gain_millidb(), SNAPSHOT_PATCH_OUTPUT_GAIN);
        assert_eq!(patched.asset_references(), state.asset_references());
        assert_eq!(
            definition
                .read_parameter(&patched, SNAPSHOT_PATCH_PARAMETER)
                .unwrap(),
            ParameterValue::Float(1.0)
        );
    }

    struct SplitAuditionPrepared(std::sync::atomic::AtomicUsize);

    struct SplitAuditionRuntime {
        accepts: bool,
        requested: Option<ProcessorAuditionSelection>,
    }

    impl PreparedNativeProcessor for SplitAuditionPrepared {
        fn runtime_version(&self) -> u32 {
            1
        }

        fn declared_runtime_bytes(&self) -> usize {
            std::mem::size_of::<SplitAuditionRuntime>()
        }

        fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
            let index = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Box::new(SplitAuditionRuntime {
                accepts: index == 0,
                requested: None,
            })
        }
    }

    impl NativeProcessorRuntime for SplitAuditionRuntime {
        fn process_block(
            &mut self,
            _frames: &mut [StereoFrame],
            _resources: &dyn crate::DeviceResources,
        ) {
        }

        fn reset(&mut self) {
            self.requested = None;
        }

        fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
            previous.as_any().is::<Self>()
        }

        fn copy_history_from(&mut self, _previous: &dyn NativeProcessorRuntime) {}

        fn as_any(&self) -> &dyn StdAny {
            self
        }

        fn transient_audition(&mut self) -> Option<&mut dyn NativeProcessorAudition> {
            Some(self)
        }
    }

    impl NativeProcessorAudition for SplitAuditionRuntime {
        fn can_select(&self, _selection: ProcessorAuditionSelection) -> bool {
            self.accepts
        }

        fn requested_selection(&self) -> Option<ProcessorAuditionSelection> {
            self.requested
        }

        fn select(&mut self, selection: Option<ProcessorAuditionSelection>) {
            self.requested = selection;
        }

        fn cancel_immediately(&mut self) {
            self.requested = None;
        }
    }

    #[test]
    fn audition_preflights_both_twins_before_mutating_either() {
        use std::sync::atomic::AtomicUsize;

        let device_type = DeviceTypeId::from_static("test.split-audition");
        let definition: std::sync::Arc<dyn PreparedNativeProcessor> =
            std::sync::Arc::new(SplitAuditionPrepared(AtomicUsize::new(0)));
        let mut chain = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id: 9,
            device_type,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(definition),
        }])
        .expect("prepared twins");
        let selection = ProcessorAuditionSelection { index: 2, lane: 0 };

        assert!(!chain.select_processor_audition(
            ProcessorChainOwner::Track(3),
            9,
            device_type,
            Some(selection),
        ));
        let entry = &mut chain.entries[0];
        let (primary, scratch) = entry.runtime.legacy_mut().expect("legacy runtimes");
        let primary_audition = primary.transient_audition().expect("primary capability");
        assert_eq!(primary_audition.requested_selection(), None);
        let scratch_audition = scratch.transient_audition().expect("scratch capability");
        assert_eq!(scratch_audition.requested_selection(), None);
    }

    struct IndependentDefinition;
    struct IndependentPrepared(u32);
    #[derive(Default)]
    struct IndependentRuntime {
        history: f32,
    }
    struct IncompatibleRuntime;

    impl NativeDeviceMetadata for IndependentDefinition {
        fn type_id(&self) -> DeviceTypeId {
            DeviceTypeId::from_static("test.independent")
        }
        fn state_version(&self) -> u32 {
            1
        }
        fn descriptors(&self) -> &'static [ParameterDescriptor] {
            &[]
        }
        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(NativeDeviceMetadata::type_id(self), 1, vec![1])
        }
        fn migrate_state(
            &self,
            state: &NativeDeviceState,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if state.type_id() != NativeDeviceMetadata::type_id(self)
                || state.state_version() != 1
                || state.payload().len() != 1
            {
                return Err(DeviceContractError::MalformedState);
            }
            Ok(state.clone())
        }
        fn read_parameter(
            &self,
            _state: &NativeDeviceState,
            _parameter: ParameterId,
        ) -> Result<ParameterValue, DeviceContractError> {
            Err(DeviceContractError::UnknownParameter)
        }
        fn patch_parameter(
            &self,
            _state: &NativeDeviceState,
            _parameter: ParameterId,
            _value: ParameterValue,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            Err(DeviceContractError::UnknownParameter)
        }
    }
    impl NativeDeviceDefinition for IndependentDefinition {
        fn prepare_matrix(
            &self,
            _state: &NativeDeviceState,
        ) -> Result<StereoMatrix, DeviceContractError> {
            Ok(StereoMatrix::identity())
        }
        fn prepare_processor(
            &self,
            state: &NativeDeviceState,
            _context: &crate::DevicePreparationContext<'_>,
        ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
            if state.payload() != [1] {
                return Err(DeviceContractError::MalformedState);
            }
            Ok(Some(Box::new(IndependentPrepared(1))))
        }
    }
    impl PreparedNativeProcessor for IndependentPrepared {
        fn runtime_version(&self) -> u32 {
            self.0
        }
        fn declared_runtime_bytes(&self) -> usize {
            std::mem::size_of::<IndependentRuntime>()
        }
        fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
            Box::new(IndependentRuntime::default())
        }
    }
    impl NativeProcessorRuntime for IndependentRuntime {
        fn process_block(
            &mut self,
            frames: &mut [StereoFrame],
            _resources: &dyn crate::DeviceResources,
        ) {
            for frame in frames {
                frame[0] += self.history;
                frame[1] += self.history;
                self.history = frame[0];
            }
        }
        fn reset(&mut self) {
            self.history = 0.0;
        }
        fn is_quiescent(&self) -> bool {
            self.history == 0.0
        }
        fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
            previous.as_any().is::<Self>()
        }
        fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
            if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
                self.history = previous.history;
            }
        }
        fn as_any(&self) -> &dyn StdAny {
            self
        }
    }

    struct DelayedTailPrepared;

    impl PreparedNativeProcessor for DelayedTailPrepared {
        fn runtime_version(&self) -> u32 {
            1
        }
        fn declared_runtime_bytes(&self) -> usize {
            std::mem::size_of::<DelayedTailRuntime>()
        }
        fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
            Box::new(DelayedTailRuntime::default())
        }
        fn output_tail(&self) -> OutputTail {
            OutputTail::Finite { max_tail_frames: 3 }
        }
    }

    #[derive(Default)]
    struct DelayedTailRuntime {
        delay: [StereoFrame; 3],
    }

    impl NativeProcessorRuntime for DelayedTailRuntime {
        fn process_block(
            &mut self,
            frames: &mut [StereoFrame],
            _resources: &dyn crate::DeviceResources,
        ) {
            for frame in frames {
                let delayed = self.delay[0];
                self.delay.rotate_left(1);
                self.delay[2] = *frame;
                *frame = delayed;
            }
        }
        fn reset(&mut self) {
            self.delay = [[0.0; 2]; 3];
        }
        fn is_quiescent(&self) -> bool {
            self.delay.iter().all(|frame| *frame == [0.0; 2])
        }
        fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
            previous.as_any().is::<Self>()
        }
        fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
            if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
                self.delay = previous.delay;
            }
        }
        fn as_any(&self) -> &dyn StdAny {
            self
        }
    }

    #[test]
    fn finite_output_tail_keeps_delayed_residual_until_bound_and_reset() {
        let prepared = DelayedTailPrepared;
        assert_eq!(
            prepared.output_tail(),
            OutputTail::Finite { max_tail_frames: 3 }
        );
        for partition in [1, 2, 7] {
            let mut runtime = prepared.create_runtime();
            assert!(runtime.is_quiescent());
            let mut impulse = [[1.0, -0.5]];
            runtime.process_block(&mut impulse, &crate::EMPTY_DEVICE_RESOURCES);
            assert_eq!(impulse, [[0.0, 0.0]]);
            assert!(!runtime.is_quiescent());

            let mut silence = [[0.0, 0.0]; 3];
            let mut processed = 0;
            for block in silence.chunks_mut(partition) {
                runtime.process_block(block, &crate::EMPTY_DEVICE_RESOURCES);
                processed += block.len();
                assert_eq!(runtime.is_quiescent(), processed == 3);
            }
            assert_eq!(silence, [[0.0, 0.0], [0.0, 0.0], [1.0, -0.5]]);
            assert!(runtime.is_quiescent());

            runtime.reset();
            assert!(runtime.is_quiescent());
        }
    }

    #[test]
    fn chain_combines_tail_ranges_and_queries_committed_and_speculative_quiescence() {
        let finite: std::sync::Arc<dyn PreparedNativeProcessor> =
            std::sync::Arc::new(DelayedTailPrepared);
        let unbounded: std::sync::Arc<dyn PreparedNativeProcessor> =
            std::sync::Arc::new(IndependentPrepared(1));
        let instance = |device_id, definition| PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id,
            device_type: DeviceTypeId::from_static("test.tail"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(definition),
        };
        let matrix_only = PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id: 1,
            device_type: DeviceTypeId::from_static("test.matrix"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: None,
            definition: None,
        };
        let mut chain = NativeRuntimeChain::new(vec![
            matrix_only,
            instance(2, std::sync::Arc::clone(&finite)),
            instance(3, std::sync::Arc::clone(&finite)),
        ])
        .unwrap();
        assert!(!chain.select_processor_audition(
            ProcessorChainOwner::Track(3),
            1,
            DeviceTypeId::from_static("test.matrix"),
            Some(ProcessorAuditionSelection { index: 0, lane: 0 }),
        ));
        assert_eq!(
            chain.output_tail_for_range(0..3),
            Some(OutputTail::Finite { max_tail_frames: 6 })
        );
        assert_eq!(
            chain.output_tail_for_range(1..2),
            Some(OutputTail::Finite { max_tail_frames: 3 })
        );
        assert_eq!(
            chain.output_tail_for_range(3..3),
            Some(OutputTail::Finite { max_tail_frames: 0 })
        );
        assert_eq!(chain.output_tail_for_range(0..4), None);
        assert_eq!(
            chain.output_tail_for_range(std::ops::Range { start: 2, end: 1 }),
            None
        );
        assert!(chain.committed_range_is_quiescent(1..3));
        chain.begin_block();
        let mut impulse = [[1.0, -1.0]];
        chain.process_range(1..3, &mut impulse).unwrap();
        assert!(!chain.speculative_range_is_quiescent(1..3));
        assert!(chain.committed_range_is_quiescent(1..3));
        chain.commit_block();
        assert!(!chain.committed_range_is_quiescent(1..3));

        let unbounded_chain = NativeRuntimeChain::new(vec![instance(4, unbounded)]).unwrap();
        assert_eq!(
            unbounded_chain.output_tail_for_range(0..1),
            Some(OutputTail::Unbounded)
        );
        assert_eq!(
            unbounded_chain.output_tail_for_range(0..0),
            Some(OutputTail::Finite { max_tail_frames: 0 })
        );
    }

    #[test]
    fn configuration_range_requires_exact_shared_state_and_prepared_semantics() {
        let device_type = DeviceTypeId::from_static("test.config");
        let configuration =
            || std::sync::Arc::new(NativeDeviceState::new(device_type, 1, vec![1, 2, 3]).unwrap());
        let instance = |configuration, bypassed| PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id: 9,
            device_type,
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 48_000,
            configuration,
            definition: None,
        };
        let old = NativeRuntimeChain::new(vec![instance(Some(configuration()), false)]).unwrap();
        let same_content =
            NativeRuntimeChain::new(vec![instance(Some(configuration()), false)]).unwrap();
        let changed = NativeRuntimeChain::new(vec![instance(
            Some(std::sync::Arc::new(
                NativeDeviceState::new(device_type, 1, vec![1, 2, 4]).unwrap(),
            )),
            false,
        )])
        .unwrap();
        let missing = NativeRuntimeChain::new(vec![instance(None, false)]).unwrap();
        let bypassed =
            NativeRuntimeChain::new(vec![instance(Some(configuration()), true)]).unwrap();

        assert!(old.configuration_matches_range(&same_content, 0..1));
        assert!(!old.configuration_matches_range(&changed, 0..1));
        assert!(!old.configuration_matches_range(&missing, 0..1));
        assert!(
            old.configuration_matches_range(&bypassed, 0..1),
            "zero-latency bypass retains its live chain"
        );
        assert!(!old.configuration_matches_range(&same_content, 0..2));
        assert!(
            !old.configuration_matches_range(&same_content, std::ops::Range { start: 1, end: 0 })
        );
    }

    struct RetargetPrepared {
        allow: bool,
        scratch_allow: bool,
        version: u32,
        runtime_count: std::sync::atomic::AtomicUsize,
    }

    #[derive(Default)]
    struct RetargetRuntime {
        allow: bool,
    }

    impl PreparedNativeProcessor for RetargetPrepared {
        fn runtime_version(&self) -> u32 {
            self.version
        }

        fn declared_runtime_bytes(&self) -> usize {
            std::mem::size_of::<RetargetRuntime>()
        }

        fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
            let twin = self
                .runtime_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Box::new(RetargetRuntime {
                allow: if twin == 0 {
                    self.allow
                } else {
                    self.scratch_allow
                },
            })
        }
    }

    impl NativeProcessorRuntime for RetargetRuntime {
        fn process_block(
            &mut self,
            _frames: &mut [StereoFrame],
            _resources: &dyn crate::DeviceResources,
        ) {
        }
        fn reset(&mut self) {}
        fn is_quiescent(&self) -> bool {
            true
        }
        fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
            previous.as_any().is::<Self>()
        }
        fn copy_history_from(&mut self, _previous: &dyn NativeProcessorRuntime) {}
        fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
            self.allow && previous.as_any().is::<Self>()
        }
        fn as_any(&self) -> &dyn StdAny {
            self
        }
    }

    struct BypassRetargetPrepared {
        smooth: bool,
        retarget: bool,
        latency: u32,
    }

    impl PreparedNativeProcessor for BypassRetargetPrepared {
        fn supports_smoothed_bypass(&self) -> bool {
            self.smooth
        }
        fn runtime_version(&self) -> u32 {
            1
        }
        fn declared_runtime_bytes(&self) -> usize {
            std::mem::size_of::<RetargetRuntime>()
        }
        fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
            Box::new(RetargetRuntime {
                allow: self.retarget,
            })
        }
        fn algorithmic_latency_frames(&self) -> Option<u32> {
            Some(self.latency)
        }
    }

    #[test]
    fn combined_bypass_retarget_requires_both_capabilities_and_stable_host_contract() {
        let kind = DeviceTypeId::from_static("test.bypass-retarget");
        let chain = |payload, bypassed, smooth, retarget, latency, gain| {
            let state = NativeDeviceState::new(kind, 1, vec![payload])
                .unwrap()
                .with_output_gain_db(gain)
                .unwrap();
            NativeRuntimeChain::new(vec![PreparedProcessorInstance {
                meter: Default::default(),
                owner: ProcessorChainOwner::Track(1),
                device_id: 3,
                device_type: kind,
                matrix: StereoMatrix::identity(),
                bypassed,
                sample_rate: 48_000,
                configuration: Some(std::sync::Arc::new(state)),
                definition: Some(std::sync::Arc::new(BypassRetargetPrepared {
                    smooth,
                    retarget,
                    latency,
                })),
            }])
            .unwrap()
        };
        let old = chain(1, false, true, true, 0, 0.0);
        let mut unsupported = chain(1, true, true, true, 0, 0.0);
        assert!(!unsupported.can_fork_timing_without_signal_history_at(&old, 0));
        assert!(
            unsupported
                .fork_timing_without_signal_history_at(&old, 0)
                .is_err()
        );
        let mut missing_configuration = chain(2, true, true, true, 0, 0.0);
        missing_configuration.entries[0].configuration = None;
        assert!(!missing_configuration.can_retarget_parameters_from_range(&old, 0..1));
        assert!(chain(2, true, true, true, 0, 0.0).can_retarget_parameters_from_range(&old, 0..1));
        let old_without_opt_in = chain(1, false, false, true, 0, 0.0);
        assert!(
            !chain(2, true, true, true, 0, 0.0)
                .can_retarget_parameters_from_range(&old_without_opt_in, 0..1)
        );
        assert!(
            !chain(2, true, false, true, 0, 0.0).can_retarget_parameters_from_range(&old, 0..1)
        );
        assert!(
            !chain(2, true, true, false, 0, 0.0).can_retarget_parameters_from_range(&old, 0..1)
        );
        assert!(!chain(2, true, true, true, 1, 0.0).can_retarget_parameters_from_range(&old, 0..1));
        assert!(!chain(2, true, true, true, 0, 6.0).can_retarget_parameters_from_range(&old, 0..1));
    }

    #[test]
    fn parameter_retarget_requires_stable_chain_topology_and_candidate_twin_opt_in() {
        let device_type = DeviceTypeId::from_static("test.parameter-retarget");
        let instance = |payload: u8,
                        allow: bool,
                        scratch_allow: bool,
                        owner,
                        device_id,
                        device_type,
                        version,
                        bypassed,
                        matrix,
                        sample_rate| {
            let configuration =
                std::sync::Arc::new(NativeDeviceState::new(device_type, 1, vec![payload]).unwrap());
            let definition: std::sync::Arc<dyn PreparedNativeProcessor> =
                std::sync::Arc::new(RetargetPrepared {
                    allow,
                    scratch_allow,
                    version,
                    runtime_count: std::sync::atomic::AtomicUsize::new(0),
                });
            PreparedProcessorInstance {
                meter: Default::default(),
                owner,
                device_id,
                device_type,
                matrix,
                bypassed,
                sample_rate,
                configuration: Some(configuration),
                definition: Some(definition),
            }
        };
        let old = NativeRuntimeChain::new(vec![instance(
            1,
            false,
            false,
            ProcessorChainOwner::Track(3),
            9,
            device_type,
            1,
            false,
            StereoMatrix::identity(),
            48_000,
        )])
        .unwrap();
        let candidate = NativeRuntimeChain::new(vec![instance(
            2,
            true,
            true,
            ProcessorChainOwner::Track(3),
            9,
            device_type,
            1,
            false,
            StereoMatrix::identity(),
            48_000,
        )])
        .unwrap();
        assert!(candidate.can_retarget_parameters_from_range(&old, 0..1));
        assert!(!candidate.configuration_matches_range(&old, 0..1));

        let one_twin_opted_in = NativeRuntimeChain::new(vec![instance(
            2,
            true,
            false,
            ProcessorChainOwner::Track(3),
            9,
            device_type,
            1,
            false,
            StereoMatrix::identity(),
            48_000,
        )])
        .unwrap();
        assert!(!one_twin_opted_in.can_retarget_parameters_from_range(&old, 0..1));

        let no_opt_in = NativeRuntimeChain::new(vec![instance(
            2,
            false,
            false,
            ProcessorChainOwner::Track(3),
            9,
            device_type,
            1,
            false,
            StereoMatrix::identity(),
            48_000,
        )])
        .unwrap();
        assert!(!no_opt_in.can_retarget_parameters_from_range(&old, 0..1));

        let bypassed = NativeRuntimeChain::new(vec![instance(
            2,
            true,
            true,
            ProcessorChainOwner::Track(3),
            9,
            device_type,
            1,
            true,
            StereoMatrix::identity(),
            48_000,
        )])
        .unwrap();
        assert!(!bypassed.can_retarget_parameters_from_range(&old, 0..1));

        let wrong_rate = NativeRuntimeChain::new(vec![instance(
            2,
            true,
            true,
            ProcessorChainOwner::Track(3),
            9,
            device_type,
            1,
            false,
            StereoMatrix::identity(),
            44_100,
        )])
        .unwrap();
        assert!(!wrong_rate.can_retarget_parameters_from_range(&old, 0..1));
        assert!(!candidate.can_retarget_parameters_from_range(&old, 0..0));
        assert!(!candidate.can_retarget_parameters_from_range(&old, 0..2));

        let wrong_owner = NativeRuntimeChain::new(vec![instance(
            2,
            true,
            true,
            ProcessorChainOwner::Return(3),
            9,
            device_type,
            1,
            false,
            StereoMatrix::identity(),
            48_000,
        )])
        .unwrap();
        assert!(!wrong_owner.can_retarget_parameters_from_range(&old, 0..1));

        let wrong_id = NativeRuntimeChain::new(vec![instance(
            2,
            true,
            true,
            ProcessorChainOwner::Track(3),
            10,
            device_type,
            1,
            false,
            StereoMatrix::identity(),
            48_000,
        )])
        .unwrap();
        assert!(!wrong_id.can_retarget_parameters_from_range(&old, 0..1));

        let other_type = DeviceTypeId::from_static("test.other-parameter-retarget");
        let wrong_type = NativeRuntimeChain::new(vec![instance(
            2,
            true,
            true,
            ProcessorChainOwner::Track(3),
            9,
            other_type,
            1,
            false,
            StereoMatrix::identity(),
            48_000,
        )])
        .unwrap();
        assert!(!wrong_type.can_retarget_parameters_from_range(&old, 0..1));

        let wrong_version = NativeRuntimeChain::new(vec![instance(
            2,
            true,
            true,
            ProcessorChainOwner::Track(3),
            9,
            device_type,
            2,
            false,
            StereoMatrix::identity(),
            48_000,
        )])
        .unwrap();
        assert!(!wrong_version.can_retarget_parameters_from_range(&old, 0..1));

        let wrong_matrix = NativeRuntimeChain::new(vec![instance(
            2,
            true,
            true,
            ProcessorChainOwner::Track(3),
            9,
            device_type,
            1,
            false,
            StereoMatrix::new([[0.5, 0.0], [0.0, 1.0]]).unwrap(),
            48_000,
        )])
        .unwrap();
        assert!(!wrong_matrix.can_retarget_parameters_from_range(&old, 0..1));
    }

    impl NativeProcessorRuntime for IncompatibleRuntime {
        fn process_block(
            &mut self,
            _frames: &mut [StereoFrame],
            _resources: &dyn crate::DeviceResources,
        ) {
        }
        fn reset(&mut self) {}
        fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
            previous.as_any().is::<Self>()
        }
        fn copy_history_from(&mut self, _previous: &dyn NativeProcessorRuntime) {}
        fn as_any(&self) -> &dyn StdAny {
            self
        }
    }

    #[test]
    fn independently_registered_stateful_definition_uses_paired_transactional_runtime() {
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                &crate::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &crate::TuningTable::default(),
                    resources: &crate::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(prepared.algorithmic_latency_frames(), None);
        assert_eq!(prepared.output_tail(), OutputTail::Unbounded);
        let legacy_runtime: Box<dyn NativeProcessorRuntime> = Box::new(IncompatibleRuntime);
        assert!(!legacy_runtime.is_quiescent());
        let mut feedback = prepared.create_runtime();
        assert!(feedback.is_quiescent());
        let mut excitation = [[1.0, 1.0]];
        feedback.process_block(&mut excitation, &crate::EMPTY_DEVICE_RESOURCES);
        assert!(!feedback.is_quiescent());
        for _ in 0..32 {
            let mut silence = [[0.0, 0.0]; 7];
            feedback.process_block(&mut silence, &crate::EMPTY_DEVICE_RESOURCES);
            assert!(silence.iter().all(|frame| *frame != [0.0, 0.0]));
            assert!(!feedback.is_quiescent());
        }
        feedback.reset();
        assert!(feedback.is_quiescent());
        let mut chain = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id: 9,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(std::sync::Arc::from(prepared)),
        }])
        .unwrap();
        let mut committed = [[1.0, 1.0]];
        chain.begin_block();
        chain.process_block(&mut committed).unwrap();
        chain.commit_block();

        chain.begin_block();
        let mut failed_block = [[2.0, 2.0]];
        chain.process_block(&mut failed_block).unwrap();
        // No commit models a source/render failure after this processor ran.
        chain.begin_block();
        let mut retry = [[2.0, 2.0]];
        chain.process_block(&mut retry).unwrap();
        chain.commit_block();
        assert_eq!(retry, [[3.0, 3.0]]);

        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                &crate::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &crate::TuningTable::default(),
                    resources: &crate::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        let mut successor = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id: 9,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(std::sync::Arc::from(prepared)),
        }])
        .unwrap();
        assert!(successor.can_inherit_from(&chain));
        successor.inherit_from(&chain);
        successor.begin_block();
        let mut inherited = [[1.0, 1.0]];
        successor.process_block(&mut inherited).unwrap();
        assert_eq!(inherited, [[4.0, 4.0]]);

        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                &crate::DevicePreparationContext {
                    sample_rate: 44_100,
                    tuning: &crate::TuningTable::default(),
                    resources: &crate::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        let mut incompatible_rate = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id: 9,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 44_100,
            configuration: None,
            definition: Some(std::sync::Arc::from(prepared)),
        }])
        .unwrap();
        assert!(incompatible_rate.can_inherit_from(&chain));
        incompatible_rate.inherit_from(&chain);
        incompatible_rate.begin_block();
        let mut reset_history = [[1.0, 1.0]];
        incompatible_rate.process_block(&mut reset_history).unwrap();
        assert_eq!(reset_history, [[1.0, 1.0]]);
    }

    #[test]
    fn selective_inheritance_excludes_old_suffix_by_identity_after_reorder() {
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                &crate::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &crate::TuningTable::default(),
                    resources: &crate::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        let prepared: std::sync::Arc<dyn PreparedNativeProcessor> = std::sync::Arc::from(prepared);
        let instance = |device_id| PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(std::sync::Arc::clone(&prepared)),
        };
        let mut previous = NativeRuntimeChain::new(vec![instance(9), instance(10)]).unwrap();
        previous.begin_block();
        let mut old_signal = [[1.0, 1.0]];
        previous.process_block(&mut old_signal).unwrap();
        previous.commit_block();
        assert_eq!(old_signal, [[1.0, 1.0]]);

        // Device 10 is the retained old suffix but moves to index 0 in the
        // candidate; device 9 is an inherited processor despite moving to 1.
        let mut candidate = NativeRuntimeChain::new(vec![instance(10), instance(9)]).unwrap();
        assert!(!candidate.can_inherit_from_excluding_old_range(&previous, 1..3));
        assert!(candidate.can_inherit_from_excluding_old_range(&previous, 1..2));
        candidate
            .inherit_from_excluding_old_range(&previous, 1..2)
            .unwrap();
        candidate.begin_block();
        let mut fresh_audio = [[0.0, 0.0]];
        candidate.process_block(&mut fresh_audio).unwrap();
        // The new suffix starts fresh (device 10 contributes zero); device 9
        // retains its prefix history and contributes one.
        assert_eq!(fresh_audio, [[1.0, 1.0]]);
    }

    #[test]
    fn full_chain_preflight_rejects_an_incompatible_retained_suffix() {
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                &crate::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &crate::TuningTable::default(),
                    resources: &crate::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        let prepared: std::sync::Arc<dyn PreparedNativeProcessor> = std::sync::Arc::from(prepared);
        let instance = || PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id: 10,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(std::sync::Arc::clone(&prepared)),
        };
        let previous = NativeRuntimeChain::new(vec![instance()]).unwrap();
        let mut candidate = NativeRuntimeChain::new(vec![instance()]).unwrap();

        // Model a factory/runtime compatibility change behind unchanged
        // identity metadata. Excluding this old suffix is valid for a tail
        // owner, but exact full-chain carry must refuse before inherit_from.
        let entry = candidate.entries.first_mut().unwrap();
        entry.runtime = RuntimeOwnership::Legacy {
            primary: Box::new(IncompatibleRuntime),
            scratch: Box::new(IncompatibleRuntime),
        };
        assert!(candidate.can_inherit_from_excluding_old_range(&previous, 0..1));
        assert!(!candidate.can_inherit_from(&previous));
    }

    #[test]
    fn segmented_processing_shares_one_transaction_and_rolls_back_all_ranges() {
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                &crate::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &crate::TuningTable::default(),
                    resources: &crate::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        let prepared: std::sync::Arc<dyn PreparedNativeProcessor> = std::sync::Arc::from(prepared);
        let make_instance = |device_id| PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(3),
            device_id,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(std::sync::Arc::clone(&prepared)),
        };
        let mut chain = NativeRuntimeChain::new(vec![make_instance(9), make_instance(10)]).unwrap();

        chain.begin_block();
        let mut first = [[1.0, 1.0]];
        chain.process_range(0..1, &mut first).unwrap();
        chain.process_range(1..2, &mut first).unwrap();
        chain.commit_block();

        chain.begin_block();
        let mut speculative = [[2.0, 2.0]];
        chain.process_range(0..1, &mut speculative).unwrap();
        chain.process_range(1..2, &mut speculative).unwrap();
        // A later render-stage failure leaves both segment histories uncommitted.
        chain.begin_block();
        let mut retry = [[2.0, 2.0]];
        chain.process_range(0..1, &mut retry).unwrap();
        chain.process_range(1..2, &mut retry).unwrap();
        chain.commit_block();
        assert_eq!(retry, [[4.0, 4.0]]);

        chain.begin_block();
        let mut next = [[3.0, 3.0]];
        chain.process_range(0..1, &mut next).unwrap();
        chain.process_range(1..2, &mut next).unwrap();
        chain.commit_block();
        assert_eq!(next, [[10.0, 10.0]]);

        chain.begin_block();
        let mut untouched = [[7.0, 7.0]];
        assert_eq!(
            chain.process_range(0..3, &mut untouched),
            Err(DeviceContractError::InvalidDescriptor)
        );
        assert_eq!(untouched, [[7.0, 7.0]]);
    }

    #[test]
    fn bypass_advances_history_after_matrix_without_changing_dry_pcm() {
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                &crate::DevicePreparationContext {
                    sample_rate: 48_000,
                    tuning: &crate::TuningTable::default(),
                    resources: &crate::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .unwrap();
        let prepared: std::sync::Arc<dyn PreparedNativeProcessor> = std::sync::Arc::from(prepared);
        let matrix = StereoMatrix::new([[2.0, 0.0], [0.0, 2.0]]).unwrap();
        let make_chain = |bypassed| {
            NativeRuntimeChain::new(vec![PreparedProcessorInstance {
                meter: Default::default(),
                owner: ProcessorChainOwner::Track(3),
                device_id: 9,
                device_type: DeviceTypeId::from_static("test.independent"),
                matrix,
                bypassed,
                sample_rate: 48_000,
                configuration: None,
                definition: Some(std::sync::Arc::clone(&prepared)),
            }])
            .unwrap()
        };
        let mut bypassed = make_chain(true);
        let mut active_reference = make_chain(false);
        let warmup = [[0.25, -0.125], [0.5, 0.75], [-0.25, 0.375]];
        let mut dry = warmup;

        bypassed.begin_block();
        // Different chunking must not change the state advanced while bypassed.
        bypassed.process_range(0..1, &mut dry[..1]).unwrap();
        bypassed.process_block(&mut dry[1..]).unwrap();
        bypassed.commit_block();

        active_reference.begin_block();
        let mut audible_reference = warmup;
        active_reference
            .process_block(&mut audible_reference)
            .unwrap();
        active_reference.commit_block();

        for (actual, original) in dry.iter().zip(warmup) {
            assert_eq!(
                [actual[0].to_bits(), actual[1].to_bits()],
                [original[0].to_bits(), original[1].to_bits()],
                "bypass must preserve the original samples exactly"
            );
        }

        // Re-enable both processors and compare their output. Their hidden
        // histories must match the active reference despite bypass chunking.
        bypassed.entries[0].bypassed = false;
        let resumed = [[0.125, -0.125], [0.0, 0.25]];
        let mut resumed_bypassed = resumed;
        let mut resumed_reference = resumed;
        bypassed.begin_block();
        bypassed
            .process_range(0..1, &mut resumed_bypassed[..1])
            .unwrap();
        bypassed.process_block(&mut resumed_bypassed[1..]).unwrap();
        bypassed.commit_block();
        active_reference.begin_block();
        active_reference
            .process_block(&mut resumed_reference)
            .unwrap();
        active_reference.commit_block();

        assert_eq!(resumed_bypassed, resumed_reference);
    }
}
