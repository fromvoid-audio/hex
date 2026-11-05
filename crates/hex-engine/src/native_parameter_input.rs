//! Borrowed native input processing and speculative content-origin observations.
//! The audio owner fences requests and confirms final sink delivery. These Copy
//! observations never acknowledge a value merely because a command was queued.
use crate::{Frame, Invalid, NodeSignalObserver, PostNodeIdentity};
use hex_lattice::{
    ForeignProcessors, NativeInstrumentClock, NativeRuntimeChain, NativeTransientInput,
    NativeTransientOperation, NativeTransientProcessing, ProcessorInstanceIdentity,
};
use hex_lattice_contract::DeviceAutomationTarget;

/// Shared input transaction with an optional fully materialized scalar window.
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_range_with_external(
    input: Option<&mut Processing>,
    external: Option<hex_lattice::NativeExternalAutomation<'_>>,
    chain: &mut NativeRuntimeChain,
    range: std::ops::Range<usize>,
    frames: &mut [Frame],
    offset: usize,
    clock: Option<&dyn NativeInstrumentClock>,
    output_clock: Option<&dyn NativeInstrumentClock>,
    input_arrival: u32,
    output_arrival: u32,
    foreign: &mut dyn ForeignProcessors,
    observer: &mut Option<&mut NodeSignalObserver<'_>>,
) -> Result<(), Invalid> {
    process_range_with_key(
        input,
        external,
        chain,
        range,
        frames,
        offset,
        clock,
        output_clock,
        input_arrival,
        output_arrival,
        foreign,
        None,
        observer,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn process_range_with_key(
    input: Option<&mut Processing>,
    external: Option<hex_lattice::NativeExternalAutomation<'_>>,
    chain: &mut NativeRuntimeChain,
    range: std::ops::Range<usize>,
    frames: &mut [Frame],
    offset: usize,
    clock: Option<&dyn NativeInstrumentClock>,
    output_clock: Option<&dyn NativeInstrumentClock>,
    input_arrival: u32,
    output_arrival: u32,
    foreign: &mut dyn ForeignProcessors,
    key: Option<Frame>,
    observer: &mut Option<&mut NodeSignalObserver<'_>>,
) -> Result<(), Invalid> {
    if let Some(input) = input {
        input.process_with_external(
            external,
            chain,
            range,
            frames,
            offset,
            clock,
            output_clock,
            input_arrival,
            output_arrival,
            foreign,
            key,
            observer,
        )
    } else {
        dispatch_stage(
            chain,
            range,
            frames,
            offset,
            clock,
            output_clock,
            foreign,
            None,
            external,
            key,
            observer,
        )
        .map_err(|_| Invalid("native effect processing failed"))
    }
}

#[allow(clippy::too_many_arguments)]
fn dispatch_stage(
    chain: &mut NativeRuntimeChain,
    range: std::ops::Range<usize>,
    frames: &mut [Frame],
    offset: usize,
    clock: Option<&dyn NativeInstrumentClock>,
    output_clock: Option<&dyn NativeInstrumentClock>,
    foreign: &mut dyn ForeignProcessors,
    transient: Option<&mut NativeTransientProcessing>,
    external: Option<hex_lattice::NativeExternalAutomation<'_>>,
    key: Option<Frame>,
    observer: &mut Option<&mut NodeSignalObserver<'_>>,
) -> Result<(), hex_lattice_contract::DeviceContractError> {
    let mut observe = |owner, device, kind, phase, offset, frames: &[Frame]| {
        if let Some(observer) = observer.as_mut() {
            observer(
                PostNodeIdentity::new(owner, device, kind),
                phase,
                offset,
                frames,
            );
        }
    };
    if let Some(key) = key {
        if range.len() != 1 {
            return Err(hex_lattice_contract::DeviceContractError::InvalidDescriptor);
        }
        let block = hex_lattice::StereoAuxiliaryBlock::new(
            frames,
            Some(std::slice::from_ref(&key)),
            &mut [],
            1,
        )
        .map_err(|_| hex_lattice_contract::DeviceContractError::InvalidDescriptor)?;
        chain.process_auxiliary_stage(
            range.start,
            block,
            offset,
            clock,
            output_clock,
            transient,
            external,
            &mut observe,
        )
    } else {
        chain.process_range_tapped_with_foreign_input_and_external(
            range,
            frames,
            offset,
            clock,
            output_clock,
            foreign,
            transient,
            external,
            &mut observe,
        )
    }
}

/// First successful target application in a complete engine render transaction.
/// `performance_frame` is the actual processing counter, independent of the
/// delayed musical content origin. Delivery remains the output owner's concern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeParameterApplication {
    pub identity: ProcessorInstanceIdentity,
    pub operation: NativeTransientOperation,
    pub arrangement_frame: u64,
    pub loop_pass: u64,
    pub performance_frame: u64,
    pub remaining_latency_frames: u32,
}

/// One inline request, borrowed by the current chains only. Frozen tails never
/// receive this context. Discard the observation on any later render refusal.
#[derive(Clone, Copy)]
pub(crate) struct Processing {
    input: NativeTransientInput,
    performance_start: u64,
    final_arrival: u32,
    pub(crate) applied: Option<NativeParameterApplication>,
}
impl Processing {
    pub(crate) fn new(
        input: NativeTransientInput,
        performance_start: u64,
        final_arrival: u32,
    ) -> Self {
        Self {
            input,
            performance_start,
            final_arrival,
            applied: None,
        }
    }

    /// Different device arrivals require separate calls. Missing origins defer
    /// both Set and Clear; they cannot produce fabricated recording timestamps.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_with_external(
        &mut self,
        external: Option<hex_lattice::NativeExternalAutomation<'_>>,
        chain: &mut NativeRuntimeChain,
        range: std::ops::Range<usize>,
        frames: &mut [Frame],
        offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        input_arrival: u32,
        output_arrival: u32,
        foreign: &mut dyn ForeignProcessors,
        key: Option<Frame>,
        observer: &mut Option<&mut NodeSignalObserver<'_>>,
    ) -> Result<(), Invalid> {
        let (origin, arrival) =
            if self.input.operation.target() == DeviceAutomationTarget::HostOutputGain {
                (output_clock, output_arrival)
            } else {
                (clock, input_arrival)
            };
        let stamp = origin.and_then(|origin| {
            Some((
                origin.arrangement_frame_at_offset(0)?,
                origin.loop_pass_at_offset(0)?,
            ))
        });
        let mut transient = stamp.map(|_| NativeTransientProcessing::new(self.input));
        let performance_frame = self
            .performance_start
            .checked_add(
                u64::try_from(offset).map_err(|_| Invalid("native input frame offset overflow"))?,
            )
            .ok_or(Invalid("native input processing clock overflow"))?;
        let remaining = self
            .final_arrival
            .checked_sub(arrival)
            .ok_or(Invalid("native input stage exceeds final arrival"))?;
        let result = dispatch_stage(
            chain,
            range,
            frames,
            offset,
            clock,
            output_clock,
            foreign,
            transient.as_mut(),
            external,
            key,
            observer,
        );
        if result.is_err() {
            self.applied = None;
            return Err(Invalid("native parameter input processing failed"));
        }
        if self.applied.is_none()
            && let Some(application) = transient.and_then(|context| context.applied)
            && let Some((arrangement_frame, loop_pass)) = stamp
        {
            self.applied = Some(NativeParameterApplication {
                identity: application.identity,
                operation: application.operation,
                arrangement_frame,
                loop_pass,
                performance_frame,
                remaining_latency_frames: remaining,
            });
        }
        Ok(())
    }
}
