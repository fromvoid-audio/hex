//! Two bounded control observations survive SDK errors and output retirement.
//! Only the original private final-output witness can produce a timed point.
use super::*;
use hex_audio::foreign_parameter_delivery::{Receipt, RetiredOutcome};
use hex_session::service::automation_draft::{
    DraftClock, Vst3ApplicationValue, Vst3CaptureTarget, Vst3ParameterApplication,
};
use hex_session::service::{
    DeviceAutomationSource, DeviceCaptureTarget, PluginParameterDelivery,
    PluginParameterObservation as Observation, ResidentRuntimeLoop,
};
#[derive(Clone, Copy)]
pub(super) struct Context {
    pub(super) applied: AppliedAudioSource,
    target: Vst3CaptureTarget,
    sample_rate: u32,
    clock: ResidentRuntimeLoop,
}
impl Context {
    pub(super) fn new(
        gesture: &Gesture,
        control: &PublicationControl,
        context: AudioSourceContext,
        clock: Option<(u32, ResidentRuntimeLoop)>,
    ) -> Result<Self, Error> {
        let class = gesture
            .capture_class
            .ok_or_else(|| refused("vst3 parameter is not automatable"))?;
        if gesture.live.as_ref().is_none_or(|live| !live.playing) {
            return Err(refused(
                "confirmed vst3 input requires the current playing source",
            ));
        }
        let (sample_rate, clock) = clock.filter(|(rate, _)| *rate != 0).ok_or_else(lost)?;
        let (applied, _) = control
            .status
            .native_parameter_source(true)
            .filter(|(source, _)| source.context == context)
            .ok_or_else(lost)?;
        if clock.source_context != Some(applied.context)
            || clock.audio_revision != applied.audio_revision
            || clock.output_identity != applied.output_identity
        {
            return Err(lost());
        }
        Ok(Self {
            applied,
            target: Vst3CaptureTarget {
                device: gesture.origin.device,
                class,
                parameter: gesture.origin.parameter,
                binding: gesture.binding,
            },
            sample_rate,
            clock,
        })
    }
}
#[derive(Clone, Copy)]
pub(super) struct PendingDelivery {
    pub(super) target: hex_audio::foreign_parameter_delivery::Target,
    pub(super) context: Context,
    pub(super) receipt: Option<Receipt>,
}
impl PendingDelivery {
    fn source(self) -> DeviceAutomationSource {
        DeviceAutomationSource {
            target: DeviceCaptureTarget::Vst3(self.context.target),
            sample_rate: self.context.sample_rate,
            output_identity: self.target.source.output_identity,
            audio_revision: self.target.source.audio_revision,
            transport_epoch: self.target.transport_epoch,
            section_loop: self.context.clock.section_loop,
        }
    }
}
impl Gesture {
    pub(in crate::native_audio::plugins) fn has_retired_delivery(&self) -> bool {
        self.retired_delivery.is_some()
    }
    fn observe_delivery(
        &mut self,
        pending: PendingPoint,
        receipt: Option<Receipt>,
    ) -> Result<(), Error> {
        let delivery = pending.delivery.ok_or_else(lost)?;
        let delivered = matches!(receipt, Some(Receipt::Delivered { .. }));
        if pending.processed || delivered {
            self.last = pending.value;
            self.sequence = pending.sequence;
            if pending.opening_hold
                && let Some(live) = self.live.as_mut()
            {
                live.hold = Some(pending.token);
            }
        }
        let source = delivery.source();
        let observation = if let Some(Receipt::Delivered {
            normalized,
            origin,
            delivery_performance_frame,
            ..
        }) = receipt
        {
            Observation::Delivered(PluginParameterDelivery {
                key: self.key,
                context: delivery.target.source.context,
                target: delivery.context.target,
                source,
                sequence: pending.sequence,
                application: Vst3ParameterApplication {
                    binding: delivery.target.binding,
                    parameter: delivery.target.parameter,
                    origin: DraftClock {
                        performance_frame: origin.performance_frame,
                        content_frame: origin.arrangement_frame,
                        loop_pass: origin.loop_pass,
                    },
                    value: Vst3ApplicationValue::Value(normalized),
                },
                delivery_performance_frame,
            })
        } else {
            Observation::Interrupted {
                key: self.key,
                context: delivery.target.source.context,
                target: delivery.context.target,
                source,
                sequence: pending.sequence,
            }
        };
        self.deliveries.push(observation);
        if delivered { Ok(()) } else { Err(lost()) }
    }
    pub(super) fn poll_delivered_point(
        &mut self,
        control: &mut PublicationControl,
    ) -> Result<bool, Error> {
        let pending = self.pending.as_mut().ok_or_else(lost)?;
        let delivery = pending.delivery.as_mut().ok_or_else(lost)?;
        let expected = delivery.target;
        // The final-output witness and SDK mailbox ACK have separate custody.
        // Drain the ACK even if output was already confirmed, before Release
        // can reuse that mailbox. Retain an early witness while ACK is busy.
        if !pending.sdk_finished {
            match poll_input(control, self.binding, &pending.token) {
                Ok(
                    NativeWorkerParameterProgress::Processed(_)
                    | NativeWorkerParameterProgress::Flushed,
                ) => {
                    pending.processed = true;
                    pending.sdk_finished = true;
                }
                Ok(NativeWorkerParameterProgress::Pending) | Err("native preparation is busy") => {}
                _ => {
                    pending.sdk_finished = true;
                    control.cancel_native_source_parameter_delivery_on_control(expected);
                }
            }
        }
        if delivery.receipt.is_none()
            && let Some(receipt) = control.try_native_source_parameter_delivery_on_control()
        {
            let actual = match receipt {
                Receipt::Delivered { target, .. }
                | Receipt::Released { target, .. }
                | Receipt::Aborted { target, .. } => target,
            };
            if actual != expected {
                control.cancel_native_source_parameter_delivery_on_control(expected);
                return Err(lost());
            }
            delivery.receipt = Some(receipt);
        }
        if !pending.sdk_finished || delivery.receipt.is_none() {
            return Ok(false);
        }
        let receipt = delivery.receipt;
        let pending = self.pending.take().ok_or_else(lost)?;
        self.observe_delivery(pending, receipt)?;
        Ok(true)
    }
    fn poll_retired(&mut self) {
        let Some(outcome) = self
            .retired_delivery
            .as_mut()
            .and_then(|reader| reader.try_take_on_control())
        else {
            return;
        };
        self.retired_delivery = None;
        if let Some(pending) = self.pending.take() {
            let receipt = match outcome {
                RetiredOutcome::Observed(receipt) => Some(receipt),
                RetiredOutcome::Interrupted { .. } => None,
            };
            let expected = pending.delivery.map(|delivery| delivery.target);
            let actual = receipt.map(|receipt| match receipt {
                Receipt::Delivered { target, .. }
                | Receipt::Released { target, .. }
                | Receipt::Aborted { target, .. } => target,
            });
            let _ = self.observe_delivery(pending, if actual == expected { receipt } else { None });
        }
    }
}
impl Plugins {
    pub(in crate::native_audio) fn retire_parameter_delivery(
        &mut self,
        control: &mut PublicationControl,
    ) {
        let Some(gesture) = self.parameter_gesture.as_mut() else {
            return;
        };
        if gesture
            .pending
            .as_ref()
            .is_some_and(|pending| pending.delivery.is_some())
        {
            gesture.retired_delivery = control.retire_native_source_parameter_delivery_on_control();
            if gesture.retired_delivery.is_none()
                && let Some(pending) = gesture.pending.take()
            {
                let _ = gesture.observe_delivery(pending, None);
            }
        }
        self.collect_parameter_observations();
    }
    pub(in crate::native_audio::plugins) fn collect_parameter_observations(&mut self) {
        if let Some(gesture) = self.parameter_gesture.as_mut() {
            let was_retired = gesture.has_retired_delivery();
            gesture.poll_retired();
            self.confirmed_parameters
                .extend(gesture.deliveries.drain(..));
            if was_retired && !gesture.has_retired_delivery() {
                self.parameter_gesture = None;
            }
        }
    }
    pub(in crate::native_audio) fn take_parameter_observation(&mut self) -> Option<Observation> {
        self.collect_parameter_observations();
        self.confirmed_parameters.pop_front()
    }
}
