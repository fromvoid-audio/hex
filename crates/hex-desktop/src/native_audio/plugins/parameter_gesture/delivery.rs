//! A pending point keeps both its SDK token and final-output witness. SDK
//! failure cancels the witness but does not drop its late terminal custody.
use super::*;
use hex_audio::foreign_parameter_delivery::Receipt;
use hex_session::service::{DeviceAutomationSource, DeviceCaptureTarget, PluginParameterDelivery};
use hex_session::service::automation_draft::{Vst3ApplicationValue, Vst3CaptureTarget, Vst3ParameterApplication};
use hex_storage::automation_draft::DraftClock;
impl Gesture {
    pub(super) fn poll_delivered_point(&mut self,control:&mut PublicationControl)->Result<bool,Error> {
        let pending=self.pending.as_mut().ok_or_else(lost)?;
        let expected=pending.delivery.ok_or_else(lost)?;
        let receipt=control.try_native_source_parameter_delivery_on_control();
        if receipt.is_none() {
            match poll_input(control,self.binding,&pending.token) {
                Ok(NativeWorkerParameterProgress::Processed(_) | NativeWorkerParameterProgress::Flushed) => pending.processed=true,
                Ok(NativeWorkerParameterProgress::Pending) | Err("native preparation is busy") => {},
                _ => { control.cancel_native_source_parameter_delivery_on_control(expected); }
            }
            return Ok(false);
        }
        let receipt=receipt.ok_or_else(lost)?;
        let actual=match receipt {Receipt::Delivered{target,..}|Receipt::Released{target,..}|Receipt::Aborted{target,..}=>target};
        if actual!=expected {
            // Never convert an unrelated private witness into this gesture.
            control.cancel_native_source_parameter_delivery_on_control(expected);
            return Err(lost());
        }
        let pending=self.pending.take().ok_or_else(lost)?;
        if pending.processed || matches!(receipt,Receipt::Delivered{..}) {
            self.last=pending.value;
            self.sequence=pending.sequence;
            if pending.opening_hold && let Some(live)=self.live.as_mut() {live.hold=Some(pending.token);}
        }
        let Receipt::Delivered {normalized,origin,delivery_performance_frame,..}=receipt else {return Err(lost())};
        let target=Vst3CaptureTarget {device:self.origin.device,class:self.capture_class.ok_or_else(lost)?,parameter:expected.parameter,binding:expected.binding};
        self.deliveries.push(PluginParameterDelivery {
            target,
            source:DeviceAutomationSource {target:DeviceCaptureTarget::Vst3(target),sample_rate:control.sample_rate(),output_identity:expected.source.output_identity,audio_revision:expected.source.audio_revision,transport_epoch:expected.transport_epoch,section_loop:None},
            sequence:pending.sequence,
            application:Vst3ParameterApplication {binding:expected.binding,parameter:expected.parameter,origin:DraftClock {performance_frame:origin.performance_frame,content_frame:origin.arrangement_frame,loop_pass:origin.loop_pass},value:Vst3ApplicationValue::Value(normalized)},
            delivery_performance_frame,
        });
        Ok(true)
    }
}
