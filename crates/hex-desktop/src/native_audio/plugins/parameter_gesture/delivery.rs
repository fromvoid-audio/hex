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
    transport_epoch: u64,
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
        let (applied, transport_epoch) = control
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
            transport_epoch,
            clock,
        })
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Point,
    Release,
}
#[derive(Clone, Copy)]
pub(super) struct PendingDelivery {
    pub(super) target: hex_audio::foreign_parameter_delivery::Target,
    pub(super) context: Context,
    pub(super) receipt: Option<Receipt>,
    pub(super) kind: Kind,
}
impl Context {
    pub(super) fn ready(self, key: GestureKey) -> Observation {
        Observation::Ready {
            key,
            context: self.applied.context,
            target: self.target,
            source: DeviceAutomationSource {
                target: DeviceCaptureTarget::Vst3(self.target),
                sample_rate: self.sample_rate,
                output_identity: self.applied.output_identity,
                audio_revision: self.applied.audio_revision,
                transport_epoch: self.transport_epoch,
                section_loop: self.clock.section_loop,
            },
        }
    }
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
        let delivered = matches!(
            (delivery.kind, receipt),
            (Kind::Point, Some(Receipt::Delivered { .. }))
                | (Kind::Release, Some(Receipt::Released { .. }))
        );
        if pending.processed || delivered {
            self.last = pending.value;
            self.sequence = pending.sequence;
            if pending.opening_hold
                && let Some(live) = self.live.as_mut()
            {
                live.hold = Some(pending.token);
            }
            // SDK release custody ends at its ACK even if final Main later
            // aborts. That abort still cannot become a successful take end.
            if delivery.kind == Kind::Release
                && let Some(live) = self.live.as_mut()
            {
                live.hold = None;
            }
        }
        let source = delivery.source();
        let payload = match (delivery.kind, receipt) {
            (
                Kind::Point,
                Some(Receipt::Delivered {
                    normalized,
                    origin,
                    delivery_performance_frame,
                    ..
                }),
            ) => Some((
                Vst3ApplicationValue::Value(normalized),
                origin,
                delivery_performance_frame,
            )),
            (
                Kind::Release,
                Some(Receipt::Released {
                    origin,
                    delivery_performance_frame,
                    ..
                }),
            ) => {
                if let Some(live) = self.live.as_mut() {
                    live.hold = None;
                }
                self.recorded_release = true;
                Some((
                    Vst3ApplicationValue::Restored,
                    origin,
                    delivery_performance_frame,
                ))
            }
            _ => None,
        };
        let observation = if let Some((value, origin, delivery_performance_frame)) = payload {
            let point = PluginParameterDelivery {
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
                    value,
                },
                delivery_performance_frame,
            };
            if delivery.kind == Kind::Release {
                Observation::Restored(point)
            } else {
                Observation::Delivered(point)
            }
        } else {
            Observation::Interrupted {
                key: self.key,
                context: delivery.target.source.context,
                target: delivery.context.target,
                source,
                sequence: pending.sequence,
            }
        };
        if matches!(observation, Observation::Interrupted { .. }) {
            self.recording_interrupted = true;
        }
        self.deliveries.push(observation);
        self.finish_closed_recording();
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
        #[cfg(all(test, feature = "allocation-probe"))]
        let ack_ready = !pending.hold_sdk_ack;
        #[cfg(not(all(test, feature = "allocation-probe")))]
        let ack_ready = true;
        if !pending.sdk_finished && ack_ready {
            match poll_input(control, self.binding, &pending.token) {
                Ok(
                    NativeWorkerParameterProgress::Processed(_)
                    | NativeWorkerParameterProgress::Flushed,
                ) => {
                    pending.processed = true;
                    pending.sdk_finished = true;
                }
                Ok(NativeWorkerParameterProgress::Released { .. })
                    if delivery.kind == Kind::Release =>
                {
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
        #[cfg(all(test, feature = "allocation-probe"))]
        let receipt_ready = std::env::var("HEX_DESKTOP_INSTRUMENT_TAKEOVER_CASE").as_deref()
            != Ok("touch_close_queued");
        #[cfg(not(all(test, feature = "allocation-probe")))]
        let receipt_ready = true;
        if receipt_ready
            && delivery.receipt.is_none()
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
    pub(super) fn release_recorded(
        &mut self,
        control: &mut PublicationControl,
        context: AudioSourceContext,
        clock: Option<(u32, ResidentRuntimeLoop)>,
    ) -> Result<Prepared, Error> {
        self.drain_point(control)?;
        if self.recorded_release {
            return Ok(Prepared::Applied);
        }
        let live = self.live.as_ref().ok_or_else(lost)?;
        if live.hold.is_none() {
            self.recorded_release = true;
            self.deliveries
                .push(Observation::Unchanged { key: self.key });
            return Ok(Prepared::Applied);
        }
        if control.status.native_parameter_source(true) != Some((live.source, live.transport_epoch))
        {
            return Err(lost());
        }
        let capture = Context::new(self, control, context, clock)?;
        let hold = self
            .live
            .as_ref()
            .and_then(|live| live.hold.as_ref())
            .ok_or_else(lost)?;
        let command = match self.recording_base {
            Some(base) => NativeParameterHold::RestoreAndRelease {
                hold,
                normalized: self.sdk_value(base),
            },
            None => NativeParameterHold::Release { hold },
        };
        let (token, target) = admit(control, |control| {
            control.native_source_parameter_hold_with_delivery_on_control(
                capture.applied,
                self.binding,
                command,
            )
        })?;
        self.pending = Some(PendingPoint {
            token,
            // A release is a terminal, not a new requested gesture value.
            value: self.last,
            sequence: self.sequence,
            opening_hold: false,
            delivery: Some(PendingDelivery {
                target,
                context: capture,
                receipt: None,
                kind: Kind::Release,
            }),
            processed: false,
            sdk_finished: false,
            #[cfg(all(test, feature = "allocation-probe"))]
            hold_sdk_ack: false,
        });
        self.drain_point(control)?;
        if self.recorded_release {
            Ok(Prepared::Applied)
        } else {
            Err(lost())
        }
    }
    /// Output closure or invalidation of the original source/epoch is a
    /// terminal without an invented application clock.
    /// Preserve the original pending point first; a point interruption already
    /// terminates the take. The queue holds at most that point plus this end.
    pub(in crate::native_audio::plugins) fn recording_output_closed(&mut self) {
        self.recording_closed = true;
        self.finish_closed_recording();
    }
    fn finish_closed_recording(&mut self) {
        if !self.recording_closed
            || self.recorded_release
            || self.recording_interrupted
            || self.pending.is_some()
            || self.retired_delivery.is_some()
        {
            return;
        }
        let Some(capture) = self.recording_context else {
            return;
        };
        self.recording_interrupted = true;
        self.deliveries.push(Observation::Interrupted {
            key: self.key,
            context: capture.applied.context,
            target: capture.target,
            source: DeviceAutomationSource {
                target: DeviceCaptureTarget::Vst3(capture.target),
                sample_rate: capture.sample_rate,
                output_identity: capture.applied.output_identity,
                audio_revision: capture.applied.audio_revision,
                transport_epoch: capture.transport_epoch,
                section_loop: capture.clock.section_loop,
            },
            sequence: self.sequence,
        });
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
        // A queued witness can be consumed during retirement itself. Mark
        // closure first so that immediate drain cannot retire the gesture
        // before emitting its terminal after the original point.
        gesture.recording_output_closed();
        if gesture
            .pending
            .as_ref()
            .is_some_and(|pending| pending.delivery.is_some())
        {
            gesture.retired_delivery = control.retire_native_source_parameter_delivery_on_control();
            if gesture.retired_delivery.is_none()
                && let Some(pending) = gesture.pending.take()
            {
                // The witness may already be held here while the SDK ACK is
                // busy. Its original channel is then empty, not interrupted.
                let receipt = pending.delivery.and_then(|delivery| delivery.receipt);
                let _ = gesture.observe_delivery(pending, receipt);
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
        self.confirmed_parameters
            .pop_front()
            .or_else(|| self.take_editor_recording_observation())
    }
}
