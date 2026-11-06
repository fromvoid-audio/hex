//! Borrowed, fallible foreign graph processing, separate from rollbackable DSP.
//! The host retains actual source/endpoint/worker custody and performs final
//! host-format acceptance. A scalar declaration or state equality grants none.
use crate::{NativeInstrumentClock, ProcessorChainOwner, StereoFrame};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, ForeignParameterPoint, ForeignProcessorBinding,
    NativeDeviceState,
};

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
        if parameters.is_empty() {
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
