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
    external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
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
    external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
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
    external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
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
        chain.process_auxiliary_stage_with_external_set(
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
        chain.process_range_tapped_with_foreign_input_and_external_set(
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

/// Immutable control-side normalization for one armed native target.
/// The capture owner must fence the same project, output and transport
/// revision before passing a delivery-confirmed application here.
pub struct NativeParameterCapture {
    target: crate::NativeParameterInputTarget,
    mapping: Option<(
        hex_lattice_contract::ParameterDescriptor,
        hex_lattice_contract::ParameterNormalization,
    )>,
}

impl NativeParameterCapture {
    pub fn prepare(
        project: &hex_project::Project,
        target: crate::NativeParameterInputTarget,
    ) -> Result<Self, Invalid> {
        if crate::native_parameter_input_target(project, target.device(), target.target())?
            != target
        {
            return Err(Invalid("native capture target owner or type changed"));
        }
        let mapping = if let Some(id) = target.target().provider_parameter() {
            let device = project
                .device(target.device())
                .ok_or(Invalid("native capture target no longer exists"))?;
            let class = if matches!(
                target.identity().owner(),
                hex_lattice::ProcessorChainOwner::Source(_)
            ) {
                crate::NativeDeviceClass::Instrument
            } else {
                crate::NativeDeviceClass::Effect
            };
            let view = crate::native_device_view_for_class(class, device.state())?;
            let parameter = view
                .parameters
                .iter()
                .find(|parameter| parameter.descriptor.id == id)
                .ok_or(Invalid(
                    "native capture parameter descriptor is unavailable",
                ))?;
            let normalization = parameter
                .semantics
                .ok_or(Invalid(
                    "native capture parameter normalization is unavailable",
                ))?
                .normalization;
            Some((parameter.descriptor, normalization))
        } else {
            None
        };
        Ok(Self { target, mapping })
    }

    /// Normalize the delivered manual target, preserving the provider's
    /// smoothing for replay. None is the delivered restoration operation.
    pub fn value(
        &self,
        application: NativeParameterApplication,
    ) -> Result<Option<hex_project::AutomationValue>, Invalid> {
        use hex_lattice::NativeTransientValue;

        if application.identity != self.target.identity()
            || application.operation.target() != self.target.target()
        {
            return Err(Invalid(
                "native capture application targets another control",
            ));
        }
        let normalized = match application.operation {
            NativeTransientOperation::Clear(_) => return Ok(None),
            NativeTransientOperation::Set(NativeTransientValue::Parameter { value, .. }) => {
                let (descriptor, normalization) = self.mapping.ok_or(Invalid(
                    "native capture parameter normalization is unavailable",
                ))?;
                normalization
                    .normalize(descriptor, value)
                    .map_err(|_| Invalid("delivered native capture value is invalid"))?
            }
            NativeTransientOperation::Set(
                NativeTransientValue::HostInputGain(gain)
                | NativeTransientValue::HostOutputGain(gain),
            ) => gain.normalized(),
        };
        Ok(Some(hex_project::AutomationValue::new(normalized)?))
    }
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

    /// Input only for the current receiving instrument, never its MIDI
    /// contributors or a retained release processor.
    pub(crate) fn for_source(
        &self,
        source: Option<hex_project::SourceId>,
        device: Option<hex_project::DeviceId>,
    ) -> Option<NativeTransientInput> {
        let hex_lattice::ProcessorChainOwner::Source(owner) = self.input.identity.owner() else {
            return None;
        };
        (source.is_some_and(|source| source.value() == owner)
            && device.is_some_and(|device| device.value() == self.input.identity.device_id()))
        .then_some(self.input)
    }

    /// A successful source processor application precedes the full downstream
    /// compensation delay. The callback owner still confirms final delivery.
    pub(crate) fn source_applied(
        &mut self,
        arrangement_frame: u64,
        loop_pass: u64,
        performance_frame: u64,
    ) -> Result<(), Invalid> {
        if performance_frame < self.performance_start {
            return Err(Invalid("native instrument input processing clock differs"));
        }
        self.applied.get_or_insert(NativeParameterApplication {
            identity: self.input.identity,
            operation: self.input.operation,
            arrangement_frame,
            loop_pass,
            performance_frame,
            remaining_latency_frames: self.final_arrival,
        });
        Ok(())
    }

    /// Different device arrivals require separate calls. Missing origins defer
    /// both Set and Clear; they cannot produce fabricated recording timestamps.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_with_external(
        &mut self,
        external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
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
