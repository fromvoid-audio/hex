//! SDK observations feed the same bounded writer and adoption owner as native
//! receipts. The caller retains the original observation until its admission
//! is acknowledged; this adapter owns no audio endpoint or second recorder.
use super::*;
use crate::service::{PluginParameterDelivery, PluginParameterObservation as Observation};

impl DeviceAutomationTakeScope {
    /// Derive copied provenance from the actual Ready observation. The caller
    /// must create the draft successfully before submitting any plug-in input.
    /// These data confer no SDK authority; Begin still validates the live source.
    pub fn from_plugin_ready(
        current: AudioSourceContext,
        observation: &Observation,
    ) -> Result<Self, &'static str> {
        let Observation::Ready {
            key,
            context,
            target,
            source,
        } = *observation
        else {
            return Err("plugin recording requires its ready observation");
        };
        if context != current
            || key.generation != current.project_generation
            || key.token == 0
            || source.target != DeviceCaptureTarget::Vst3(target)
            || !target.binding.is_valid()
            || target.binding.root != source.output_identity
            || source.sample_rate == 0
        {
            return Err("plugin recording admission source changed");
        }
        Ok(Self {
            target: source.target,
            stamp: DraftProvenance {
                sample_rate: source.sample_rate,
                output_identity: source.output_identity,
                project_generation: context.project_generation,
                editor_revision: context.editor_revision,
                audio_revision: source.audio_revision,
                transport_epoch: source.transport_epoch,
                gesture_serial: key.token,
            },
        })
    }
}

impl Active {
    fn matches_plugin(
        &self,
        key: crate::device_gesture::GestureKey,
        context: AudioSourceContext,
        target: Vst3CaptureTarget,
        source: DeviceAutomationSource,
    ) -> bool {
        key.generation == self.scope.stamp.project_generation
            && key.token == self.scope.stamp.gesture_serial
            && context == self.scope.context()
            && self.scope.target == DeviceCaptureTarget::Vst3(target)
            && source.matches(self.scope)
            && source.section_loop == self.source.section_loop
    }

    fn validate_plugin_delivery(&self, point: &PluginParameterDelivery) -> Result<(), String> {
        if !self.matches_plugin(point.key, point.context, point.target, point.source)
            || point.delivery_performance_frame < point.application.origin.performance_frame
        {
            return Err("plugin recording observation belongs to another delivery".into());
        }
        Ok(())
    }

    pub(super) fn plugin_action(
        &self,
        observation: &Observation,
    ) -> Result<DeviceAutomationTakeAction, String> {
        use super::super::automation_draft::Vst3ApplicationValue;
        use DeviceAutomationTakeAction as Action;
        match observation {
            Observation::Delivered(point) => {
                self.validate_plugin_delivery(point)?;
                if !matches!(point.application.value, Vst3ApplicationValue::Value(_)) {
                    return Err("plugin point cannot finish its recording".into());
                }
                Ok(Action::AppendVst3(Arc::from([(
                    point.sequence,
                    point.application,
                )])))
            }
            Observation::Restored(point) => {
                self.validate_plugin_delivery(point)?;
                if point.application.value != Vst3ApplicationValue::Restored {
                    return Err("plugin recording requires actual restoration".into());
                }
                Ok(Action::FinishVst3(point.application))
            }
            Observation::Interrupted {
                key,
                context,
                target,
                source,
                ..
            } => {
                if !self.matches_plugin(*key, *context, *target, *source) {
                    return Err("plugin interruption belongs to another recording".into());
                }
                Ok(Action::Abort(DraftAbortReason::OutputFailed))
            }
            Observation::Unchanged { key } => {
                if key.generation != self.scope.stamp.project_generation
                    || key.token != self.scope.stamp.gesture_serial
                    || !matches!(self.scope.target, DeviceCaptureTarget::Vst3(_))
                    || self.input.progress().accepted_events != 0
                {
                    return Err("plugin recording has delivered changes".into());
                }
                Ok(Action::Abort(DraftAbortReason::Cancelled))
            }
            Observation::Ready { .. } => Err("plugin recording is already admitted".into()),
        }
    }
}

#[cfg(test)]
mod tests;
