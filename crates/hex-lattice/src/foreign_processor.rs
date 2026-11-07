//! Borrowed, fallible foreign graph processing, separate from rollbackable DSP.
//! The host retains actual source/endpoint/worker custody and performs final
//! host-format acceptance. A scalar declaration or state equality grants none.
use crate::{NativeInstrumentClock, ProcessorChainOwner, StereoFrame};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, ForeignParameterPoint, ForeignProcessorBinding,
    NativeDeviceState,
};
use std::{alloc::Layout, num::NonZeroU64, sync::Arc};

/// Control-validated facts for a copied foreign endpoint. No runtime factory,
/// native pointer or resource owner lives here. The host must bind these facts
/// and the node's exact current configuration to its actual admitted source,
/// output root, graph and transport epoch before exposing a borrowed endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedForeignProcessor {
    quantum: u16,
    processor_latency: u32,
    bridge_latency: u32,
    total_latency: u32,
    binding: Option<ForeignProcessorBinding>,
    output_tail: crate::OutputTail,
    instrument_profile: bool,
}
impl PreparedForeignProcessor {
    pub fn new(
        quantum: u16,
        processor_latency: u32,
        bridge_latency: u32,
    ) -> Result<Self, DeviceContractError> {
        if quantum == 0 || bridge_latency == 0 {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let total_latency = processor_latency
            .checked_add(bridge_latency)
            .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        Ok(Self {
            quantum,
            processor_latency,
            bridge_latency,
            total_latency,
            binding: None,
            output_tail: crate::OutputTail::Unbounded,
            instrument_profile: false,
        })
    }
    /// Retain the exact existing source/endpoint binding in immutable graph
    /// facts. Native providers derive this from their actual prepared lease.
    /// The value itself grants no source or graph admission authority.
    pub fn with_binding(
        mut self,
        binding: ForeignProcessorBinding,
    ) -> Result<Self, DeviceContractError> {
        if !binding.is_valid() {
            return Err(DeviceContractError::InvalidId);
        }
        self.binding = Some(binding);
        Ok(self)
    }
    /// Source-validated residual bound, excluding the fixed latency above.
    /// This declaration alone cannot establish runtime quiescence or native
    /// custody. Hosts must still match the actual source and accepted drain.
    pub fn with_output_tail(mut self, tail: crate::OutputTail) -> Self {
        self.output_tail = tail;
        self
    }
    pub fn output_tail(self) -> crate::OutputTail {
        self.output_tail
    }
    /// Declare the zero-audio-input configuration used for instrument
    /// workers. The host still owns the actual source binding.
    pub fn with_instrument_profile(mut self) -> Self {
        self.instrument_profile = true;
        self
    }
    pub fn is_instrument_profile(self) -> bool {
        self.instrument_profile
    }
    pub fn binding(self) -> Option<ForeignProcessorBinding> {
        self.binding
    }
    pub fn quantum_frames(self) -> u16 {
        self.quantum
    }
    pub fn processor_latency_frames(self) -> u32 {
        self.processor_latency
    }
    pub fn bridge_latency_frames(self) -> u32 {
        self.bridge_latency
    }
    pub fn total_latency_frames(self) -> u32 {
        self.total_latency
    }
}

/// One SDK parameter observed on the exact selected source. Zero steps means
/// continuous. Raw parameter ID zero is valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForeignParameterCapability {
    pub id: u32,
    pub steps: u32,
    /// Exact restored SDK base as finite normalized f64 bits. This is the
    /// canonical value, independent of subsequent authored/manual DSP input.
    pub normalized_base_bits: u64,
}

/// Control-only authored capabilities from the original worker's preparation.
/// This view owns no worker, endpoint or authority to publish a graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForeignAutomationSource<'a> {
    pub binding: ForeignProcessorBinding,
    /// Unique immutable program revision issued by this original source owner.
    pub revision: NonZeroU64,
    pub class: [u8; 16],
    pub sample_rate: u32,
    pub quantum_frames: u16,
    /// Strictly increasing raw parameter IDs.
    pub parameters: &'a [ForeignParameterCapability],
}

/// An immutable, control-prepared program that fills bounded storage for a
/// complete physical foreign processing quantum. This call borrows the program
/// and must perform no allocation, waiting or I/O.
pub trait ForeignAutomationProgram: Sync {
    fn render_quantum(
        &self,
        origins: &[Option<u64>],
        slice_cuts: &[u16],
        output: &mut [ForeignParameterPoint],
    ) -> Result<usize, DeviceContractError>;
}

/// A graph's prepared automation for one exact foreign endpoint. The graph
/// owner must admit the program and reserve a unique revision for it within
/// the binding's lifetime. Equal scalar bindings alone grant no SDK capability.
#[derive(Clone, Copy)]
pub struct ForeignAutomation<'a> {
    pub binding: ForeignProcessorBinding,
    pub revision: std::num::NonZeroU64,
    pub program: &'a dyn ForeignAutomationProgram,
    retained: &'a [Option<RetainedProgram>],
}

impl<'a> ForeignAutomation<'a> {
    /// Borrow one current program without retained versions.
    pub fn new(
        binding: ForeignProcessorBinding,
        revision: NonZeroU64,
        program: &'a dyn ForeignAutomationProgram,
    ) -> Self {
        Self {
            binding,
            revision,
            program,
            retained: &[],
        }
    }

    pub fn contains_revision(self, revision: NonZeroU64) -> bool {
        revision == self.revision
            || self
                .retained
                .iter()
                .flatten()
                .any(|entry| entry.revision == revision)
    }

    /// Resolve a complete quantum without cloning or releasing an owner.
    pub fn render_revision(
        self,
        revision: NonZeroU64,
        origins: &[Option<u64>],
        cuts: &[u16],
        output: &mut [ForeignParameterPoint],
    ) -> Result<usize, DeviceContractError> {
        let program = if revision == self.revision {
            self.program
        } else {
            self.retained
                .iter()
                .flatten()
                .find(|entry| entry.revision == revision)
                .map(|entry| entry.program.as_ref())
                .ok_or(DeviceContractError::ForeignProcessingFailed)?
        };
        program.render_quantum(origins, cuts, output)
    }
}

/// Maximum historical programs retained beside a graph's current version.
/// A full set refuses on control; publication progress must permit pruning
/// before further versions can be admitted. This grants no native custody.
pub const MAX_RETAINED_FOREIGN_PROGRAMS: usize = 8;

#[derive(Clone)]
struct RetainedProgram {
    revision: NonZeroU64,
    program: Arc<dyn ForeignAutomationProgram + Send>,
    bytes: usize,
}

/// Immutable graph data. Prepared and runtime entries share the allocation
/// on control. The callback borrows it without acquiring or releasing custody.
#[derive(Clone)]
pub struct PreparedForeignAutomation {
    binding: ForeignProcessorBinding,
    revision: NonZeroU64,
    program: Arc<dyn ForeignAutomationProgram + Send>,
    program_bytes: usize,
    storage_bytes: usize,
    retained: [Option<RetainedProgram>; MAX_RETAINED_FOREIGN_PROGRAMS],
}

impl PreparedForeignAutomation {
    /// The compiler supplies actual retained backing bytes beyond the inline
    /// program payload. Include both in the existing chain storage budget.
    pub fn new<P: ForeignAutomationProgram + Send + 'static>(
        binding: ForeignProcessorBinding,
        revision: NonZeroU64,
        program: P,
        backing_bytes: usize,
    ) -> Result<Self, DeviceContractError> {
        if !binding.is_valid() {
            return Err(DeviceContractError::InvalidId);
        }
        let layout = Layout::new::<[usize; 2]>()
            .extend(Layout::new::<P>())
            .map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?
            .0
            .pad_to_align();
        let storage_bytes = layout
            .size()
            .checked_add(backing_bytes)
            .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        Ok(Self {
            binding,
            revision,
            program: Arc::new(program),
            program_bytes: storage_bytes,
            storage_bytes,
            retained: std::array::from_fn(|_| None),
        })
    }

    /// Retain immutable versions from the exact previous binding. Call only
    /// while constructing a new graph on control. Keys may alias only the
    /// same actual allocation. Refusal leaves this owner unchanged. Retained
    /// fields use fixed inline storage; all Arc releases remain on control.
    pub fn retain_previous_on_control(
        &mut self,
        previous: &Self,
    ) -> Result<(), DeviceContractError> {
        self.retain_previous_since_on_control(previous, None)
    }

    /// Carry a source-monotonic history from its known received frontier.
    /// The caller must obtain `received` from the exact original worker, not
    /// from a current compiler key or a queued candidate. The received quantum
    /// may still be unfinished: retain that revision and every newer one.
    /// Native admission and observation validation remain the host's job.
    /// No existing graph is modified, and refusal leaves this owner unchanged.
    pub fn retain_received_on_control(
        &mut self,
        previous: &Self,
        received: NonZeroU64,
    ) -> Result<(), DeviceContractError> {
        if self.revision <= previous.revision
            || !previous.borrow().contains_revision(received)
            || previous
                .retained
                .iter()
                .flatten()
                .any(|entry| entry.revision >= previous.revision)
            || self
                .retained
                .iter()
                .flatten()
                .any(|entry| entry.revision >= self.revision)
        {
            return Err(DeviceContractError::ForeignProcessingFailed);
        }
        self.retain_previous_since_on_control(previous, Some(received))
    }

    fn retain_previous_since_on_control(
        &mut self,
        previous: &Self,
        received: Option<NonZeroU64>,
    ) -> Result<(), DeviceContractError> {
        if self.binding != previous.binding {
            return Err(DeviceContractError::ForeignProcessingFailed);
        }
        let mut candidate = self.clone();
        for slot in &mut candidate.retained {
            if slot
                .as_ref()
                .is_some_and(|entry| received.is_some_and(|received| entry.revision < received))
            {
                let bytes = slot.as_ref().map_or(0, |entry| entry.bytes);
                candidate.storage_bytes = candidate
                    .storage_bytes
                    .checked_sub(bytes)
                    .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                *slot = None;
            }
        }
        let versions =
            std::iter::once((previous.revision, &previous.program, previous.program_bytes)).chain(
                previous
                    .retained
                    .iter()
                    .flatten()
                    .map(|entry| (entry.revision, &entry.program, entry.bytes)),
            );
        for (revision, program, bytes) in versions {
            if received.is_some_and(|received| revision < received) {
                continue;
            }
            let existing = if revision == candidate.revision {
                Some((&candidate.program, candidate.program_bytes))
            } else {
                candidate
                    .retained
                    .iter()
                    .flatten()
                    .find(|entry| entry.revision == revision)
                    .map(|entry| (&entry.program, entry.bytes))
            };
            if let Some((existing, existing_bytes)) = existing {
                if !Arc::ptr_eq(existing, program) || existing_bytes != bytes {
                    return Err(DeviceContractError::ForeignProcessingFailed);
                }
                continue;
            }
            let slot = candidate
                .retained
                .iter_mut()
                .find(|entry| entry.is_none())
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            candidate.storage_bytes = candidate
                .storage_bytes
                .checked_add(bytes)
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            *slot = Some(RetainedProgram {
                revision,
                program: Arc::clone(program),
                bytes,
            });
        }
        *self = candidate;
        Ok(())
    }

    /// Every actual owning field and its immutable allocation declaration.
    /// Inline retained slots are already included in the runtime entry stride.
    pub fn storage_owners(
        &self,
    ) -> impl Iterator<Item = (&Arc<dyn ForeignAutomationProgram + Send>, usize)> {
        std::iter::once((&self.program, self.program_bytes)).chain(
            self.retained
                .iter()
                .flatten()
                .map(|entry| (&entry.program, entry.bytes)),
        )
    }

    /// Exact handle correspondence between prepared and runtime copies.
    pub fn shares_storage_with(&self, other: &Self) -> bool {
        self.binding == other.binding
            && self.revision == other.revision
            && self.program_bytes == other.program_bytes
            && Arc::ptr_eq(&self.program, &other.program)
            && self
                .retained
                .iter()
                .zip(&other.retained)
                .all(|(a, b)| match (a, b) {
                    (None, None) => true,
                    (Some(a), Some(b)) => {
                        a.revision == b.revision
                            && a.bytes == b.bytes
                            && Arc::ptr_eq(&a.program, &b.program)
                    }
                    _ => false,
                })
    }

    pub fn storage_bytes(&self) -> usize {
        self.storage_bytes
    }

    pub fn borrow(&self) -> ForeignAutomation<'_> {
        ForeignAutomation {
            binding: self.binding,
            revision: self.revision,
            program: self.program.as_ref(),
            retained: &self.retained,
        }
    }
}

impl PartialEq for PreparedForeignAutomation {
    fn eq(&self, other: &Self) -> bool {
        // The compiler keeps one immutable program revision per worker
        // binding. Independently prepared graphs may own equal programs.
        self.binding == other.binding && self.revision == other.revision
    }
}

impl std::fmt::Debug for PreparedForeignAutomation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedForeignAutomation")
            .field("binding", &self.binding)
            .field("revision", &self.revision)
            .field("storage_bytes", &self.storage_bytes)
            .finish_non_exhaustive()
    }
}

/// One invocation of an existing graph node. Configuration borrows the actual
/// prepared state; providers do not invent a second source-ID/state namespace.
/// Equality is a matching fence, never native custody or publication authority.
#[derive(Clone, Copy)]
pub struct ForeignProcessorNode<'a> {
    pub owner: ProcessorChainOwner,
    pub device_id: u64,
    pub device_type: DeviceTypeId,
    pub sample_rate: u32,
    pub configuration: &'a NativeDeviceState,
    pub prepared: PreparedForeignProcessor,
    /// Borrowed authored input for this segment. An endpoint without
    /// authored support must refuse the segment before staging any work.
    pub automation: Option<ForeignAutomation<'a>>,
    pub bypassed: bool,
    /// Cached finite host input trim. Segments contain the original untrimmed
    /// signal so the host can preserve aligned dry history. Apply this gain to
    /// the copied wet input, including while bypassed. Output trim is applied
    /// by the chain unless the endpoint declares post-trim output.
    pub input_gain: f32,
    /// Host output trim for endpoints that explicitly return post-trim PCM.
    /// Legacy borrowed endpoints continue to return pre-trim PCM.
    pub output_gain: f32,
}

/// One MIDI 1.0 note event at an offset within the supplied render segment.
/// The caller owns note scheduling and sends release events through the same
/// endpoint as the matching note-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForeignNote {
    pub frame: u16,
    pub channel: u8,
    pub pitch: u8,
    pub velocity: u8,
    pub on: bool,
    /// Graph-assigned identity shared by this occurrence's on/off pair.
    pub note_id: Option<std::num::NonZeroU32>,
}

/// Unique borrowed view of the host's already retained foreign endpoints.
/// Each call stages the next contiguous graph segment of one actual physical
/// callback. It must be bounded and free of allocation, I/O, waiting, waking,
/// owning-handle destruction or native DSP. Returned PCM is copied output from
/// the fixed pipeline; a missing/malformed reply is a failure, never replay.
///
/// This interface deliberately has no commit/dispatch method. Engine success
/// is not host delivery: the audio owner alone accepts pending input after the
/// complete final host-format conversion. Refusal permanently revokes the
/// epoch; consumed replies and foreign history cannot be rolled back. Bypass
/// still advances the same endpoint and returns aligned dry samples under its
/// unchanged declared latency. The view cannot transfer resource custody.
pub trait ForeignProcessors {
    /// True only when this endpoint applies the supplied host output trim to
    /// active wet output itself and returns post-trim PCM. The host must not
    /// apply it a second time. Steady bypass output is untrimmed aligned dry;
    /// an endpoint may transition between the two while keeping them aligned.
    fn returns_post_trim(&self, _node: ForeignProcessorNode<'_>) -> bool {
        false
    }
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError>;
    /// Stage provider parameter points through the same segment/source and
    /// final host acceptance as PCM. Providers validate raw IDs and metadata;
    /// this data cannot grant resource custody or mutate canonical state.
    /// Hosts without this capability must refuse nonempty input explicitly.
    fn process_segment_with_parameters(
        &mut self,
        node: ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [StereoFrame],
        parameters: &[ForeignParameterPoint],
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        if parameters.is_empty() && node.automation.is_none() {
            self.process_segment(node, frame_offset, frames, clock)
        } else {
            self.reject_callback();
            frames.fill([0.0; 2]);
            Err(DeviceContractError::ForeignProcessingUnavailable)
        }
    }
    /// Stage ordered note events along with the segment. Like PCM, staged
    /// events become visible to the worker only after host acceptance.
    fn process_segment_with_notes(
        &mut self,
        node: ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [StereoFrame],
        parameters: &[ForeignParameterPoint],
        notes: &[ForeignNote],
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        if notes.is_empty() {
            self.process_segment_with_parameters(node, frame_offset, frames, parameters, clock)
        } else {
            self.reject_callback();
            frames.fill([0.0; 2]);
            Err(DeviceContractError::ForeignProcessingUnavailable)
        }
    }
    /// Read an accepted drain observation from this exact retained endpoint.
    /// Pending zeros must not advance the proof; pending renewed input must
    /// invalidate it. Missing/unknown/revoked endpoints return false. This query
    /// stages no work and grants no endpoint destruction/return authority.
    fn committed_tail_is_drained(&self, _node: ForeignProcessorNode<'_>) -> bool {
        false
    }
    /// Queue-independent, idempotent revocation of all staged foreign work for
    /// this physical callback. No native teardown/join may run on this lane.
    fn reject_callback(&mut self);
}

/// Explicit context for renderers without admitted foreign endpoints. A
/// foreign node refuses instead of silently bypassing its required processing.
pub struct RefuseForeign;
impl ForeignProcessors for RefuseForeign {
    fn process_segment(
        &mut self,
        _node: ForeignProcessorNode<'_>,
        _frame_offset: usize,
        _frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }
    fn reject_callback(&mut self) {}
}

#[cfg(test)]
mod tests;
