//! SDK observations feed the same bounded writer and adoption owner as native
//! receipts. The caller retains the original observation until its admission
//! is acknowledged; this adapter owns no audio endpoint or second recorder.
use super::super::automation_draft::{AppliedEvent, Vst3ApplicationValue};
use super::*;
use crate::service::{PluginParameterDelivery, PluginParameterObservation as Observation};
use hex_storage::automation_draft::DraftClock;

const MAX_FAILED_OBSERVATIONS: usize = 3;

/// Validated, bounded continuation; original SDK observations remain with their owner.
pub struct FailedPluginContinuation {
    events: [Option<AppliedEvent>; MAX_FAILED_OBSERVATIONS],
    terminal: Option<DraftTerminal>,
}
impl FailedPluginContinuation {
    pub fn events(&self) -> impl Iterator<Item = AppliedEvent> + '_ {
        self.events.iter().flatten().copied()
    }
    /// Only an actual restoration supplies a Finished clock.
    pub fn terminal(&self) -> Option<DraftTerminal> {
        self.terminal
    }
}

/// Validate retained SDK observations against the failed take and its verified predecessor.
/// This does not consume observations, write a file, release custody, or authorize adoption.
pub fn prepare_failed_plugin_continuation(
    scope: DeviceAutomationTakeScope,
    source: DeviceAutomationSource,
    predecessor: Option<AppliedEvent>,
    observations: &[Observation],
) -> Result<FailedPluginContinuation, &'static str> {
    let DeviceCaptureTarget::Vst3(target) = scope.target else {
        return Err("failed plugin continuation requires a vst3 target");
    };
    if observations.len() > MAX_FAILED_OBSERVATIONS
        || !source.matches(scope)
        || !target.binding.is_valid()
        || target.binding.root != scope.stamp.output_identity
        || predecessor.is_some_and(|event| event.sequence == 0)
    {
        return Err("failed plugin continuation has invalid scope or capacity");
    }
    let mut result = FailedPluginContinuation {
        events: [None; MAX_FAILED_OBSERVATIONS],
        terminal: None,
    };
    let mut previous = predecessor;
    let mut index = 0;
    for observation in observations {
        if result.terminal.is_some() {
            return Err("plugin observation follows its terminal");
        }
        match observation {
            Observation::Delivered(point) | Observation::Restored(point) => {
                if !matches_plugin_scope(
                    scope,
                    source,
                    point.key,
                    point.context,
                    point.target,
                    point.source,
                ) || point.delivery_performance_frame
                    < point.application.origin.performance_frame
                    || previous.is_some_and(|event| !clock_follows(event, point.application.origin))
                {
                    return Err("plugin recovery observation belongs to another delivery");
                }
                let value = target.value(point.application)?;
                match observation {
                    Observation::Delivered(_) => {
                        let value = value.ok_or("plugin recovery point restored the lane")?;
                        if point.sequence == 0
                            || previous.is_some_and(|event| point.sequence <= event.sequence)
                        {
                            return Err("plugin recovery sequence did not advance");
                        }
                        let event = AppliedEvent {
                            sequence: point.sequence,
                            performance_frame: point.application.origin.performance_frame,
                            content_frame: point.application.origin.content_frame,
                            loop_pass: point.application.origin.loop_pass,
                            value,
                        };
                        *result
                            .events
                            .get_mut(index)
                            .ok_or("plugin recovery event budget exceeded")? = Some(event);
                        index += 1;
                        previous = Some(event);
                    }
                    Observation::Restored(_) => {
                        if value.is_some()
                            || previous.is_some_and(|event| point.sequence < event.sequence)
                        {
                            return Err("plugin recovery terminal is invalid");
                        }
                        result.terminal = Some(DraftTerminal::Finished(point.application.origin));
                    }
                    _ => return Err("invalid plugin recovery observation"),
                }
            }
            Observation::Interrupted {
                key,
                context,
                target,
                source: actual,
                sequence,
            } => {
                if !matches_plugin_scope(scope, source, *key, *context, *target, *actual)
                    || previous.is_some_and(|event| *sequence < event.sequence)
                {
                    return Err("plugin recovery interruption belongs to another recording");
                }
                result.terminal = Some(DraftTerminal::Interrupted);
            }
            Observation::Unchanged { key } => {
                if key.generation != scope.stamp.project_generation
                    || key.token != scope.stamp.gesture_serial
                    || previous.is_some()
                {
                    return Err("plugin recovery no-op has delivered changes");
                }
                result.terminal = Some(DraftTerminal::Aborted(DraftAbortReason::Cancelled));
            }
            Observation::Ready { .. } => return Err("plugin recovery is already admitted"),
        }
    }
    Ok(result)
}

fn clock_follows(previous: AppliedEvent, next: DraftClock) -> bool {
    next.performance_frame >= previous.performance_frame
        && next.loop_pass >= previous.loop_pass
        && (next.loop_pass > previous.loop_pass || next.content_frame >= previous.content_frame)
}

fn matches_plugin_scope(
    scope: DeviceAutomationTakeScope,
    expected_source: DeviceAutomationSource,
    key: crate::device_gesture::GestureKey,
    context: AudioSourceContext,
    target: Vst3CaptureTarget,
    source: DeviceAutomationSource,
) -> bool {
    key.generation == scope.stamp.project_generation
        && key.token == scope.stamp.gesture_serial
        && context == scope.context()
        && scope.target == DeviceCaptureTarget::Vst3(target)
        && source.matches(scope)
        && source.section_loop == expected_source.section_loop
}

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
        matches_plugin_scope(self.scope, self.source, key, context, target, source)
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
