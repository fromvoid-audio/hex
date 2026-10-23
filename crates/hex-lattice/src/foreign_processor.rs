//! Borrowed, fallible foreign graph processing, separate from rollbackable DSP.
//! The host retains actual source/endpoint/worker custody and performs final
//! host-format acceptance. A scalar declaration or state equality grants none.
use crate::{NativeInstrumentClock, ProcessorChainOwner, StereoFrame};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};

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
        })
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
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError>;
    /// Queue-independent, idempotent revocation of all staged foreign work for
    /// this physical callback. No native teardown/join may run on this lane.
    fn reject_callback(&mut self);
}

pub(crate) struct RefuseForeign;
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
