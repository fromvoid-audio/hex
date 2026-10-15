//! Validated stereo DSP primitives and the registered native processor seam.
mod meter;
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, MAX_NATIVE_DEVICE_TYPES, MAX_PARAMETERS_PER_DEVICE,
    NativeDeviceMetadata, NativeDeviceState,
};
pub use meter::DeviceMeter;
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

    /// Prepare an optional stateful processor using the active sample rate.
    /// Memoryless definitions keep the default `None` implementation.
    fn prepare_processor(
        &self,
        _state: &NativeDeviceState,
        _sample_rate: u32,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        Ok(None)
    }
}

mod instrument;
mod instrument_runtime;
pub use instrument::{
    ANALYTIC_NOTE_PAYLOAD_WORDS, MAX_SHARED_ANALYTIC_CONTEXT_BYTES, NativeInstrumentDefinition,
    NativeInstrumentRegistry, PreparedAnalyticNote, PreparedNativeInstrument,
    PreparedSharedAnalyticContext, PreparedSharedAnalyticNote, PreparedStorageDeclaration,
    SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS,
};
pub use instrument_runtime::{
    InstrumentHistoryRetargetPreparation, MutableInstrumentRuntime, NativeInstrumentAnalyticSeed,
    NativeInstrumentClock, NativeInstrumentEvent, NativeInstrumentRuntimeError,
    NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair, PreparedInstrumentHistoryRetarget,
    PreparedMutableInstrumentRuntime,
};

/// Immutable, control-prepared configuration for one stateful native effect.
/// Runtime construction happens before publication and is covered by the
/// definition's checked per-instance byte declaration. The optional latency
/// declaration is algorithmic delay in frames from fixed buffering or
/// lookahead; `None` means the latency is unknown. Filter phase/group delay,
/// audio-interface latency and intended effect tails are not declared here.
pub trait PreparedNativeProcessor: Send + Sync {
    fn runtime_version(&self) -> u32;
    fn declared_runtime_bytes(&self) -> usize;
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime>;

    /// Return prepared linear magnitudes for the processor's two working lanes
    /// at one frequency, when available. Lane meaning is processor-defined
    /// (for example, mid/side); this is not necessarily a physical L/R transfer
    /// matrix. This control-side visualization query must not run in callbacks.
    fn frequency_response_at_hz(&self, _frequency_hz: f32) -> Option<[f32; 2]> {
        None
    }

    /// Describe the processor's residual-output contract after its final
    /// nonzero input frame. Finite values are sample frames at this prepared
    /// processor's sample rate. A host must validate the value against its own
    /// finite work budget before admission, using checked frame arithmetic;
    /// in particular, `u64::MAX` is not an automatic acceptance. Unknown
    /// implementations conservatively make no finite reclamation-time promise.
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
/// A `Finite` declaration promises that, after at most the declared number of
/// subsequent zero-input frames, `NativeProcessorRuntime::is_quiescent` is
/// true. The promise covers all supported finite input histories and every
/// stored state that can affect future output. It does not include fixed
/// algorithmic latency, host/device buffering, or external signal tails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputTail {
    /// After at most this many subsequent zero-input frames, the runtime must
    /// report quiescent. This bound includes every internal state capable of
    /// producing later output; zero means quiescent immediately after the last
    /// nonzero input frame.
    Finite { max_tail_frames: u64 },
    /// No finite tail bound is guaranteed. A runtime may still become
    /// quiescent, but callers must not infer a deadline from observed samples.
    Unbounded,
}

/// Audio-owned state with allocation-free, infallible block processing and
/// history operations once the prepared configurations have been admitted.
pub trait NativeProcessorRuntime: StdAny + Send {
    fn process_block(&mut self, frames: &mut [StereoFrame]);
    /// Advance private state while preserving a bypassed signal.
    fn process_bypassed_block(&mut self, frames: &mut [StereoFrame]) {
        for frame in frames {
            let input = *frame;
            self.process_block(std::slice::from_mut(frame));
            *frame = input;
        }
    }
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
}

/// Device-neutral selector for a transient processor audition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessorAuditionSelection {
    pub index: u8,
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
pub const MAX_PROCESSOR_RUNTIME_BYTES: usize = 4_096;

/// Typed owner identity for a prepared effect chain. Numeric IDs share the
/// canonical project allocator, while this tag keeps track and return owners
/// distinct during runtime history handoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessorChainOwner {
    Track(u64),
    Return(u64),
}

impl ProcessorChainOwner {
    fn is_valid(self) -> bool {
        match self {
            Self::Track(id) | Self::Return(id) => id != 0,
        }
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

struct RuntimeEntry {
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
    primary: Option<Box<dyn NativeProcessorRuntime>>,
    scratch: Option<Box<dyn NativeProcessorRuntime>>,
}

/// Paired audio-owned runtime state. `primary` is committed state; processing
/// operates on `scratch` and `commit_block` is called only after the entire
/// containing block succeeds. Construction and final destruction are control
/// side responsibilities.
pub struct NativeRuntimeChain {
    entries: Vec<RuntimeEntry>,
    stateful_count: usize,
    declared_runtime_bytes: usize,
}

impl NativeRuntimeChain {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            stateful_count: 0,
            declared_runtime_bytes: 0,
        }
    }

    pub fn new(instances: Vec<PreparedProcessorInstance>) -> Result<Self, DeviceContractError> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(instances.len())
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        let mut declared_bytes = 0_usize;
        let mut stateful_count = 0_usize;
        for instance in instances {
            if !instance.owner.is_valid() || instance.device_id == 0 || instance.sample_rate == 0 {
                return Err(DeviceContractError::RuntimeBudgetExceeded);
            }
            instance.device_type.validate()?;
            let (runtime_version, output_tail, primary, scratch) =
                if let Some(definition) = &instance.definition {
                    let bytes = definition.declared_runtime_bytes();
                    if definition.runtime_version() == 0 || bytes > MAX_PROCESSOR_RUNTIME_BYTES {
                        return Err(DeviceContractError::RuntimeBudgetExceeded);
                    }
                    stateful_count = stateful_count
                        .checked_add(1)
                        .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                    declared_bytes = declared_bytes
                        .checked_add(bytes)
                        .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                    if stateful_count > MAX_STATEFUL_PROCESSORS
                        || declared_bytes > MAX_STATEFUL_PROCESSORS * MAX_PROCESSOR_RUNTIME_BYTES
                    {
                        return Err(DeviceContractError::RuntimeBudgetExceeded);
                    }
                    (
                        definition.runtime_version(),
                        definition.output_tail(),
                        Some(definition.create_runtime()),
                        Some(definition.create_runtime()),
                    )
                } else {
                    (0, OutputTail::Finite { max_tail_frames: 0 }, None, None)
                };
            entries.push(RuntimeEntry {
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
                primary,
                scratch,
            });
        }
        Ok(Self {
            entries,
            stateful_count,
            declared_runtime_bytes: declared_bytes,
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
            .checked_add(self.declared_runtime_bytes.checked_mul(2)?)
    }

    /// Declared payload preflight estimate for building a chain from instances.
    /// Call before runtime factories to reject an aggregate owner over budget.
    /// This assumes the requested entry capacity; allocator bookkeeping and
    /// shared prepared definitions are excluded. Check actual vector capacities
    /// and fixed runtime payload declarations after construction.
    pub fn declared_storage_preflight_bytes_for_instances(
        instances: &[PreparedProcessorInstance],
    ) -> Option<usize> {
        let entries = instances
            .len()
            .checked_mul(std::mem::size_of::<RuntimeEntry>())?;
        let twin_payloads = instances.iter().try_fold(0_usize, |total, instance| {
            let bytes = instance
                .definition
                .as_ref()
                .map_or(0, |definition| definition.declared_runtime_bytes());
            total.checked_add(bytes.checked_mul(2)?)
        })?;
        std::mem::size_of::<Self>()
            .checked_add(entries)?
            .checked_add(twin_payloads)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn has_stateful_processors(&self) -> bool {
        self.entries.iter().any(|entry| entry.primary.is_some())
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
        let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.owner == owner && entry.device_id == device_id && entry.device_type == device_type
        }) else {
            return false;
        };
        let (Some(primary), Some(scratch)) =
            (entry.primary.as_deref_mut(), entry.scratch.as_deref_mut())
        else {
            return false;
        };
        let (Some(primary_audition), Some(scratch_audition)) =
            (primary.transient_audition(), scratch.transient_audition())
        else {
            return false;
        };
        if selection.is_some_and(|selection| {
            entry.bypassed
                || !primary_audition.can_select(selection)
                || !scratch_audition.can_select(selection)
        }) {
            return false;
        }
        // A release is deliberately valid for a bypassed or now-disabled
        // device so an existing fade cannot get stuck.
        primary_audition.select(selection);
        scratch_audition.select(selection);
        true
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
        if index < minimum_entry {
            return false;
        }
        let Some(old) = previous.entries.get(index) else {
            return false;
        };
        let (Some(old_primary), Some(old_scratch)) =
            (old.primary.as_deref(), old.scratch.as_deref())
        else {
            return false;
        };
        if old_primary.transient_audition_requested_selection() != Some(expected_selection)
            || old_scratch.transient_audition_requested_selection() != Some(expected_selection)
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
        let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.owner == owner && entry.device_id == device_id && entry.device_type == device_type
        }) else {
            return false;
        };
        let (Some(primary), Some(scratch)) =
            (entry.primary.as_deref_mut(), entry.scratch.as_deref_mut())
        else {
            return false;
        };
        let (Some(primary_audition), Some(scratch_audition)) =
            (primary.transient_audition(), scratch.transient_audition())
        else {
            return false;
        };
        primary_audition.cancel_immediately();
        scratch_audition.cancel_immediately();
        true
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
            if let Some(runtime) = entry.primary.as_deref_mut()
                && let Some(audition) = runtime.transient_audition()
            {
                audition.cancel_immediately();
            }
            if let Some(runtime) = entry.scratch.as_deref_mut()
                && let Some(audition) = runtime.transient_audition()
            {
                audition.cancel_immediately();
            }
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
        let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.owner == owner && entry.device_id == device_id && entry.device_type == device_type
        }) else {
            return false;
        };
        if entry.bypassed {
            return false;
        }
        let (Some(primary), Some(scratch)) =
            (entry.primary.as_deref_mut(), entry.scratch.as_deref_mut())
        else {
            return false;
        };
        matches!(
            (primary.transient_audition(), scratch.transient_audition()),
            (Some(primary), Some(scratch))
                if primary.requested_selection() == Some(selection)
                    && scratch.requested_selection() == Some(selection)
                    && primary.can_select(selection)
                    && scratch.can_select(selection)
        )
    }

    pub const fn stateful_count(&self) -> usize {
        self.stateful_count
    }

    pub const fn declared_runtime_bytes(&self) -> usize {
        self.declared_runtime_bytes
    }

    /// Whether a same-position range has exact immutable processor
    /// configuration identity, in addition to its prepared processing
    /// semantics. Missing synthetic identities compare conservatively false.
    /// Revision handoff can call this on the audio callback; it can inspect up to
    /// `range.len() * MAX_NATIVE_DEVICE_STATE_BYTES` payload bytes in the
    /// worst case; the shared payload allocations themselves are not copied.
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
                    && left.bypassed == right.bypassed
                    && left.matrix == right.matrix
                    && match (&left.configuration, &right.configuration) {
                        (Some(left), Some(right)) => {
                            std::sync::Arc::ptr_eq(left, right) || left.processing_eq(right)
                        }
                        _ => false,
                    }
            })
    }

    /// Whether a same-position processor range can keep history across an
    /// authored parameter change. Entry identity and processing topology must
    /// remain exact; a changed configuration is accepted only when both
    /// candidate twins explicitly support retargeting from the old committed
    /// runtime. Empty ranges and malformed ranges are refused.
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
                let stable_processing_identity = candidate.owner == old.owner
                    && candidate.device_id == old.device_id
                    && candidate.device_type == old.device_type
                    && candidate.runtime_version == old.runtime_version
                    && candidate.sample_rate == old.sample_rate
                    && candidate.bypassed == old.bypassed
                    && candidate.matrix == old.matrix;
                if !stable_processing_identity {
                    return false;
                }
                let configuration_matches = match (&candidate.configuration, &old.configuration) {
                    (Some(candidate), Some(old)) => {
                        std::sync::Arc::ptr_eq(candidate, old) || candidate.processing_eq(old)
                    }
                    _ => false,
                };
                if configuration_matches {
                    return true;
                }
                let (Some(candidate_primary), Some(candidate_scratch), Some(old_primary)) = (
                    candidate.primary.as_deref(),
                    candidate.scratch.as_deref(),
                    old.primary.as_deref(),
                ) else {
                    return false;
                };
                candidate_primary.can_retarget_parameters_from(old_primary)
                    && candidate_scratch.can_retarget_parameters_from(old_primary)
            })
    }

    /// Combine prepared output-tail declarations for a serial range. A range
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

    /// True only when every committed processor in a valid range has no
    /// future output under zero input. Invalid ranges conservatively return
    /// false. Stateless matrix entries are immediately quiescent.
    pub fn committed_range_is_quiescent(&self, range: std::ops::Range<usize>) -> bool {
        range.start <= range.end
            && range.end <= self.entries.len()
            && self.entries[range].iter().all(|entry| {
                entry
                    .primary
                    .as_deref()
                    .is_none_or(|runtime| runtime.is_quiescent())
            })
    }

    /// Speculative counterpart used only after processing scratch state in the
    /// current uncommitted render block.
    pub fn speculative_range_is_quiescent(&self, range: std::ops::Range<usize>) -> bool {
        range.start <= range.end
            && range.end <= self.entries.len()
            && self.entries[range].iter().all(|entry| {
                entry
                    .scratch
                    .as_deref()
                    .is_none_or(|runtime| runtime.is_quiescent())
            })
    }

    /// Preflight every twin before copying any state. Matching identities whose
    /// runtime compatibility changed remain freshly reset on the candidate.
    pub fn can_inherit_from(&self, previous: &Self) -> bool {
        self.can_inherit_from_excluding_old_range(previous, 0..0)
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
        if excluded_old_range.start > excluded_old_range.end
            || excluded_old_range.end > previous.entries.len()
        {
            return false;
        }
        self.entries.iter().all(|entry| {
            let Some(new_runtime) = entry.primary.as_ref() else {
                return true;
            };
            let Some((old_index, old)) = previous.entries.iter().enumerate().find(|(_, old)| {
                old.owner == entry.owner
                    && old.device_id == entry.device_id
                    && old.device_type == entry.device_type
                    && old.runtime_version == entry.runtime_version
                    && old.sample_rate == entry.sample_rate
                    && old.primary.is_some()
            }) else {
                return true;
            };
            if excluded_old_range.contains(&old_index) {
                return true;
            }
            let Some(old_runtime) = old.primary.as_deref() else {
                return true;
            };
            new_runtime.can_copy_history_from(old_runtime)
                && entry
                    .scratch
                    .as_ref()
                    .is_some_and(|scratch| scratch.can_copy_history_from(old_runtime))
        })
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
        for entry in &mut self.entries {
            if let Some((old_index, old)) = previous.entries.iter().enumerate().find(|(_, old)| {
                old.owner == entry.owner
                    && old.device_id == entry.device_id
                    && old.device_type == entry.device_type
                    && old.runtime_version == entry.runtime_version
                    && old.sample_rate == entry.sample_rate
                    && old.primary.is_some()
            }) && !excluded_old_range.contains(&old_index)
                && let (Some(primary), Some(scratch), Some(old_primary)) = (
                    entry.primary.as_mut(),
                    entry.scratch.as_mut(),
                    old.primary.as_deref(),
                )
                && primary.can_copy_history_from(old_primary)
            {
                primary.copy_history_from(old_primary);
                scratch.copy_history_from(old_primary);
            }
        }
    }

    /// Start a transactional block by copying committed histories into twins.
    pub fn begin_block(&mut self) {
        for entry in &mut self.entries {
            if let (Some(primary), Some(scratch)) =
                (entry.primary.as_deref(), entry.scratch.as_mut())
            {
                if scratch.can_copy_history_from(primary) {
                    scratch.copy_history_from(primary);
                } else {
                    scratch.reset();
                }
            }
        }
    }

    /// Process the summed track signal through all registered stateful entries.
    pub fn process_block(&mut self, frames: &mut [StereoFrame]) {
        Self::process_entries(&mut self.entries, frames);
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
    ) {
        Self::process_entries_observed(&mut self.entries, frames, frame_offset, observer);
    }

    /// Process a control-prevalidated effect segment without ending the
    /// surrounding transactional block. The range indexes the prepared effect
    /// order; callers must begin once and commit only after the full render block.
    pub fn process_range(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
    ) -> Result<(), DeviceContractError> {
        let entries = self
            .entries
            .get_mut(range)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        Self::process_entries(entries, frames);
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
        let entries = self
            .entries
            .get_mut(range)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        Self::process_entries_observed(entries, frames, frame_offset, observer);
        Ok(())
    }

    fn process_entries(entries: &mut [RuntimeEntry], frames: &mut [StereoFrame]) {
        for entry in entries {
            Self::process_entry(entry, frames);
        }
    }

    fn process_entries_observed(
        entries: &mut [RuntimeEntry],
        frames: &mut [StereoFrame],
        frame_offset: usize,
        observer: &mut impl FnMut(ProcessorChainOwner, u64, DeviceTypeId, usize, &[StereoFrame]),
    ) {
        for entry in entries {
            Self::process_entry(entry, frames);
            observer(
                entry.owner,
                entry.device_id,
                entry.device_type,
                frame_offset,
                frames,
            );
        }
    }

    fn process_entry(entry: &mut RuntimeEntry, frames: &mut [StereoFrame]) {
        if !entry.bypassed && entry.input_gain != 1.0 {
            for frame in frames.iter_mut() {
                frame[0] *= entry.input_gain;
                frame[1] *= entry.input_gain;
            }
        }
        if entry.metering {
            entry.meter.observe_input(frames);
        }
        if entry.bypassed {
            if let Some(runtime) = entry.scratch.as_mut() {
                if entry.matrix.is_identity() {
                    runtime.process_bypassed_block(frames);
                } else {
                    for frame in frames.iter_mut() {
                        let audible = *frame;
                        let mut internal = entry.matrix.apply(audible);
                        runtime.process_bypassed_block(std::slice::from_mut(&mut internal));
                        *frame = audible;
                    }
                }
            }
        } else {
            entry.matrix.apply_block(frames);
            if let Some(runtime) = entry.scratch.as_mut() {
                runtime.process_block(frames);
            }
        }
        if !entry.bypassed && entry.output_gain != 1.0 {
            for frame in frames.iter_mut() {
                frame[0] *= entry.output_gain;
                frame[1] *= entry.output_gain;
            }
        }
        if entry.metering {
            entry.meter.observe_output(frames);
        }
    }

    /// Commit histories after all render chunks and source reads succeeded.
    pub fn commit_block(&mut self) {
        for entry in &mut self.entries {
            std::mem::swap(&mut entry.primary, &mut entry.scratch);
        }
    }

    /// Reset both committed and speculative state at a hard discontinuity.
    pub fn reset(&mut self) {
        for entry in &mut self.entries {
            if let Some(primary) = entry.primary.as_mut() {
                primary.reset();
            }
            if let Some(scratch) = entry.scratch.as_mut() {
                scratch.reset();
            }
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
            if definition.descriptors().len() > MAX_PARAMETERS_PER_DEVICE {
                return Err(DeviceContractError::InvalidDescriptor);
            }
            for (parameter_index, descriptor) in definition.descriptors().iter().enumerate() {
                descriptor.validate_descriptor()?;
                if descriptor.id.value() >= u32::MAX - 1 {
                    return Err(DeviceContractError::InvalidDescriptor);
                }
                if definition.descriptors()[..parameter_index]
                    .iter()
                    .any(|prior| prior.id == descriptor.id)
                {
                    return Err(DeviceContractError::InvalidDescriptor);
                }
            }
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
        self.validate_current(definition, definition.migrate_state(&state)?)
    }
    pub fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        self.validate_current(definition, definition.migrate_state(state)?)?
            .with_input_gain_millidb(state.input_gain_millidb())?
            .with_output_gain_millidb(state.output_gain_millidb())
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
        let current = self
            .validate_current(definition, definition.migrate_state(state)?)?
            .with_input_gain_millidb(state.input_gain_millidb())?
            .with_output_gain_millidb(state.output_gain_millidb())?;
        let descriptor = definition
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.id == parameter)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(value)?;
        let patched = definition.patch_parameter(&current, parameter, value)?;
        self.validate_current(definition, definition.migrate_state(&patched)?)?
            .with_input_gain_millidb(state.input_gain_millidb())?
            .with_output_gain_millidb(state.output_gain_millidb())
    }
    pub fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self
            .validate_current(definition, definition.migrate_state(state)?)?
            .with_input_gain_millidb(state.input_gain_millidb())?
            .with_output_gain_millidb(state.output_gain_millidb())?;
        definition.prepare_matrix(&current)
    }
    pub fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self
            .validate_current(definition, definition.migrate_state(state)?)?
            .with_input_gain_millidb(state.input_gain_millidb())?
            .with_output_gain_millidb(state.output_gain_millidb())?;
        definition.prepare_processor(&current, sample_rate)
    }
    pub fn prepare_device(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<PreparedNativeDevice, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self
            .validate_current(definition, definition.migrate_state(state)?)?
            .with_input_gain_millidb(state.input_gain_millidb())?
            .with_output_gain_millidb(state.output_gain_millidb())?;
        Ok(PreparedNativeDevice {
            matrix: definition.prepare_effect_matrix(&current)?,
            processor: definition.prepare_processor(&current, sample_rate)?,
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
        Ok(state)
    }
}

/// One interleaved stereo frame represented as left and right samples.
pub type StereoFrame = [f32; 2];

/// A coefficient or composition failed finite single-precision validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMatrix(pub &'static str);

impl fmt::Display for InvalidMatrix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for InvalidMatrix {}

/// An immutable 2×2 transform whose coefficients are finite `f32` values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StereoMatrix {
    coefficients: [[f32; 2]; 2],
}

impl StereoMatrix {
    /// Identity preserves each input sample bit-for-bit in [`Self::apply`].
    pub const fn identity() -> Self {
        Self {
            coefficients: [[1.0, 0.0], [0.0, 1.0]],
        }
    }

    /// Construct from output-row/input-column coefficients after finite checks.
    pub fn new(coefficients: [[f32; 2]; 2]) -> Result<Self, InvalidMatrix> {
        if coefficients
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(InvalidMatrix("stereo matrix coefficients must be finite"));
        }
        Ok(Self { coefficients })
    }

    /// Return the immutable output-row/input-column coefficient array.
    pub const fn coefficients(self) -> [[f32; 2]; 2] {
        self.coefficients
    }

    /// Report the fixed delay of this memoryless matrix transform.
    pub const fn latency_frames(self) -> u32 {
        0
    }

    /// Apply this matrix to one stereo frame.
    pub fn apply(self, input: StereoFrame) -> StereoFrame {
        if self.is_identity() {
            return input;
        }
        let matrix = self.coefficients;
        [
            matrix[0][0] * input[0] + matrix[0][1] * input[1],
            matrix[1][0] * input[0] + matrix[1][1] * input[1],
        ]
    }

    /// Apply this matrix in place to a caller-owned block, without allocation.
    pub fn apply_block(self, frames: &mut [StereoFrame]) {
        if self.is_identity() {
            return;
        }
        for frame in frames {
            *frame = self.apply(*frame);
        }
    }

    /// Compose in signal order: `first.then(next)` applies `first`, then `next`.
    /// The result is refused if any composed `f32` coefficient is non-finite.
    pub fn then(self, next: Self) -> Result<Self, InvalidMatrix> {
        if self.is_identity() {
            return Ok(next);
        }
        if next.is_identity() {
            return Ok(self);
        }
        let a = self.coefficients;
        let b = next.coefficients;
        let mut result = [[0.0; 2]; 2];
        for (row, output) in result.iter_mut().enumerate() {
            for (column, coefficient) in output.iter_mut().enumerate() {
                let value = f64::from(b[row][0]) * f64::from(a[0][column])
                    + f64::from(b[row][1]) * f64::from(a[1][column]);
                if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
                    return Err(InvalidMatrix("stereo matrix composition overflowed"));
                }
                *coefficient = value as f32;
                if !coefficient.is_finite() {
                    return Err(InvalidMatrix("stereo matrix composition overflowed"));
                }
            }
        }
        Self::new(result)
    }

    fn is_identity(self) -> bool {
        self.coefficients == [[1.0, 0.0], [0.0, 1.0]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{ParameterDescriptor, ParameterId, ParameterValue};

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
        fn process_block(&mut self, _frames: &mut [StereoFrame]) {}

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
        let primary = entry.primary.as_deref_mut().expect("primary runtime");
        let primary_audition = primary.transient_audition().expect("primary capability");
        assert_eq!(primary_audition.requested_selection(), None);
        let scratch = entry.scratch.as_deref_mut().expect("scratch runtime");
        let scratch_audition = scratch.transient_audition().expect("scratch capability");
        assert_eq!(scratch_audition.requested_selection(), None);
    }

    #[test]
    fn identity_is_a_zero_latency_bit_preserving_fast_path() {
        let identity = StereoMatrix::identity();
        assert_eq!(identity.latency_frames(), 0);
        let input = [f32::from_bits(0x8000_0000), f32::from_bits(0x7fc0_0123)];
        let output = identity.apply(input);
        assert_eq!(output[0].to_bits(), input[0].to_bits());
        assert_eq!(output[1].to_bits(), input[1].to_bits());
        let mut block = [input, [1.0, -1.0]];
        identity.apply_block(&mut block);
        assert_eq!(block[0][1].to_bits(), input[1].to_bits());
    }

    #[test]
    fn coefficients_are_finite_and_composition_overflow_is_refused() {
        assert!(StereoMatrix::new([[f32::NAN, 0.0], [0.0, 1.0]]).is_err());
        assert!(StereoMatrix::new([[f32::INFINITY, 0.0], [0.0, 1.0]]).is_err());
        let large = StereoMatrix::new([[f32::MAX, 0.0], [0.0, f32::MAX]]).unwrap();
        let scale = StereoMatrix::new([[2.0, 0.0], [0.0, 2.0]]).unwrap();
        assert!(large.then(scale).is_err());
    }

    #[test]
    fn composition_preserves_serial_noncommutative_order() {
        let swap = StereoMatrix::new([[0.0, 1.0], [1.0, 0.0]]).unwrap();
        let left_gain = StereoMatrix::new([[2.0, 0.0], [0.0, 1.0]]).unwrap();
        let swapped_then_gain = swap.then(left_gain).unwrap();
        let gain_then_swap = left_gain.then(swap).unwrap();
        assert_eq!(swapped_then_gain.apply([1.0, 3.0]), [6.0, 1.0]);
        assert_eq!(gain_then_swap.apply([1.0, 3.0]), [3.0, 2.0]);
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
            _sample_rate: u32,
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
        fn process_block(&mut self, frames: &mut [StereoFrame]) {
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
        fn process_block(&mut self, frames: &mut [StereoFrame]) {
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
            runtime.process_block(&mut impulse);
            assert_eq!(impulse, [[0.0, 0.0]]);
            assert!(!runtime.is_quiescent());

            let mut silence = [[0.0, 0.0]; 3];
            let mut processed = 0;
            for block in silence.chunks_mut(partition) {
                runtime.process_block(block);
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
        assert!(!old.configuration_matches_range(&bypassed, 0..1));
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
        fn process_block(&mut self, _frames: &mut [StereoFrame]) {}
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
        fn process_block(&mut self, _frames: &mut [StereoFrame]) {}
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
                48_000,
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
        feedback.process_block(&mut excitation);
        assert!(!feedback.is_quiescent());
        for _ in 0..32 {
            let mut silence = [[0.0, 0.0]; 7];
            feedback.process_block(&mut silence);
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
        chain.process_block(&mut committed);
        chain.commit_block();

        chain.begin_block();
        let mut failed_block = [[2.0, 2.0]];
        chain.process_block(&mut failed_block);
        // No commit models a source/render failure after this processor ran.
        chain.begin_block();
        let mut retry = [[2.0, 2.0]];
        chain.process_block(&mut retry);
        chain.commit_block();
        assert_eq!(retry, [[3.0, 3.0]]);

        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                48_000,
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
        successor.process_block(&mut inherited);
        assert_eq!(inherited, [[4.0, 4.0]]);

        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                44_100,
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
        incompatible_rate.process_block(&mut reset_history);
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
                48_000,
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
        previous.process_block(&mut old_signal);
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
        candidate.process_block(&mut fresh_audio);
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
                48_000,
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
        entry.primary = Some(Box::new(IncompatibleRuntime));
        entry.scratch = Some(Box::new(IncompatibleRuntime));
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
                48_000,
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
                48_000,
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
        bypassed.process_block(&mut dry[1..]);
        bypassed.commit_block();

        active_reference.begin_block();
        let mut audible_reference = warmup;
        active_reference.process_block(&mut audible_reference);
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
        bypassed.process_block(&mut resumed_bypassed[1..]);
        bypassed.commit_block();
        active_reference.begin_block();
        active_reference.process_block(&mut resumed_reference);
        active_reference.commit_block();

        assert_eq!(resumed_bypassed, resumed_reference);
    }
}
