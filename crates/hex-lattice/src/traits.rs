//! Core native device/processor traits, limits and prepared instance identity types.
use crate::{
    DeviceMeter, DevicePreparationContext, DeviceResources, DynamicsMeter, ForeignAutomationSource,
    NativeInstrumentClock, NativeParameterAutomation, OffsetAutomation, OffsetClock,
    PreparedProcessorFactory, PreparedRuntimeDevice, StereoAuxiliaryBlock, StereoFrame,
    StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
};
use std::any::Any as StdAny;
use std::fmt;

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

    /// Resolve an exact hosted effect receiver to its already prepared physical
    /// identity. Ordinary definitions retain the authored identity. A compiler
    /// borrow grants no endpoint, graph publication or retirement custody.
    fn retained_foreign_effect_identity(
        &self,
        identity: ProcessorInstanceIdentity,
        _state: &NativeDeviceState,
        _context: &DevicePreparationContext<'_>,
    ) -> Result<ProcessorInstanceIdentity, DeviceContractError> {
        Ok(identity)
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
    /// Opt into authored power-off for a zero-latency smoothed effect or an
    /// effect with validated host-aligned dry latency. Once the off ramp
    /// commits, the host resets both runtime twins in place and skips their
    /// block transactions until the stage turns on. The aligned dry delay
    /// remains active. Reset must be bounded, allocation-free, and restore
    /// fresh DSP and control history. Cold power-on keeps the aligned dry
    /// lead, then uses a half-millisecond de-click into fresh wet output.
    fn supports_power_off(&self) -> bool {
        false
    }
    /// Opt in to host-aligned bypass for an exact, fixed latency from 1 to
    /// 16 frames. The host keeps untrimmed dry history. Ordinary bypass
    /// continues wet processing and automation. An explicit power-off opt-in
    /// stops the wet runtime after the fade. The graph retains its latency.
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

    /// Prove that only the host input matrix changed while this processor's
    /// authored processing configuration remains identical. Both candidate
    /// twins must accept the same committed donor before a source handoff.
    /// This callback-side query must be bounded, allocation-free and read-only.
    fn can_retarget_source_host_matrix_from(&self, _previous: &dyn NativeProcessorRuntime) -> bool {
        false
    }

    /// Prove that an internal source parameter change can copy the donor's
    /// history and transition continuously. The host independently requires
    /// the same input matrix, identity and an unautomated source. Both twins
    /// must accept the committed donor. This query is bounded and read-only.
    fn can_retarget_source_parameters_from(&self, _previous: &dyn NativeProcessorRuntime) -> bool {
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
    pub(crate) fn is_valid(self) -> bool {
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
    pub(crate) owner: ProcessorChainOwner,
    pub(crate) device_id: u64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceMeter;
    use std::sync::Arc;

    #[test]
    fn processor_instance_identity_rejects_zero_ids_and_separates_owners() {
        assert_eq!(
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 0),
            Err(DeviceContractError::InvalidId)
        );
        assert_eq!(
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(0), 1),
            Err(DeviceContractError::InvalidId)
        );
        assert_eq!(
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Return(0), 1),
            Err(DeviceContractError::InvalidId)
        );
        assert_eq!(
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Source(0), 1),
            Err(DeviceContractError::InvalidId)
        );

        let track = ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(2), 9).unwrap();
        let ret = ProcessorInstanceIdentity::new(ProcessorChainOwner::Return(2), 9).unwrap();
        assert_eq!(track.owner(), ProcessorChainOwner::Track(2));
        assert_eq!(track.device_id(), 9);
        assert_ne!(track, ret);
    }

    #[test]
    fn memoryless_prepared_instance_reports_zero_algorithmic_latency() {
        let instance = PreparedProcessorInstance {
            meter: Arc::new(DeviceMeter::default()),
            owner: ProcessorChainOwner::Main,
            device_id: 1,
            device_type: DeviceTypeId::new("hex.test.memoryless").unwrap(),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            configuration: None,
            definition: None,
        };
        assert_eq!(instance.algorithmic_latency_frames(), Some(0));
    }
}
