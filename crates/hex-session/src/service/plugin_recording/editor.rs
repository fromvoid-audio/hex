//! Original editor input shares the existing VST3 draft and failed custody.
//! Session creates the draft before enabling the SDK scope and schedules the
//! same canonical publication only after the writer and actual terminal agree.
use super::*;
use crate::device_gesture::GestureKey;

pub(in crate::service) struct EditorOutcome {
    pub(in crate::service) scope: DeviceAutomationTakeScope,
    pub(in crate::service) result: Result<Step, String>,
}

impl State {
    pub(in crate::service) fn editor_active(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.editor_request.is_some())
    }

    pub(in crate::service) fn editor_ready(&self) -> Option<u64> {
        let active = self.active.as_ref()?;
        (active.ended && active.outcome.is_some())
            .then_some(active.editor_request)
            .flatten()
    }

    pub(in crate::service) fn finish_editor(
        &mut self,
        request_id: u64,
        lane: &Lane,
    ) -> Option<EditorOutcome> {
        if self.editor_ready() != Some(request_id) {
            return None;
        }
        let mut active = self.active.take()?;
        let scope = active.scope;
        let mut result = active.outcome.take()?;
        if let Some(reason) = active.editor_start_error.take() {
            result = Err(reason);
        } else if !active.has_points
            && !lane.has_failed_custody()
            && matches!(
                result,
                Ok(Step::Progress(DeviceAutomationTakeProgress::Retained(_)))
            )
        {
            result = Ok(Step::Progress(DeviceAutomationTakeProgress::Unchanged));
        }
        if let Err(reason) = result.as_mut() {
            *reason = native_automation_recording::retained_error(&active.path, reason);
        }
        if lane.has_failed_custody()
            || lane.busy()
            || active.pending.is_some()
            || active.has_failed_tail()
        {
            self.failed = Some(active);
        }
        Some(EditorOutcome { scope, result })
    }
}

impl Preparation<'_> {
    pub(in crate::service) fn begin_editor(
        &mut self,
        session: &Session,
        input: PluginEditorRecordingInput,
        request_id: u64,
        sink: &mut dyn WorkerAudioSink,
    ) -> Result<DeviceAutomationTakeProgress, String> {
        if request_id == 0
            || self.state.pending()
            || self.state.failed.is_some()
            || self.lane.busy()
        {
            return Err("recover or finish the previous automation take first".into());
        }
        let observation = sink.prepare_plugin_editor_recording(input)?;
        if let Err(reason) = self.admit_ready(session, input.context, input.key, observation) {
            sink.cancel_plugin_editor_recording(input.key);
            return Err(reason);
        }
        let Some(active) = self.state.active.as_mut() else {
            sink.cancel_plugin_editor_recording(input.key);
            return Err("the original editor take was not admitted".into());
        };
        active.editor_request = Some(request_id);
        if let Err(reason) = sink.start_plugin_editor_recording(input.key) {
            // Start did not activate the prepared scope. This is an empty
            // aborted take; no SDK point or release clock can exist.
            sink.cancel_plugin_editor_recording(input.key);
            active.pending = Some(DeviceAutomationTakeAction::PluginObservation(Box::new(
                PluginParameterObservation::Unchanged { key: input.key },
            )));
            active.editor_start_error = Some(reason);
            return Ok(DeviceAutomationTakeProgress::Pending);
        }
        Ok(DeviceAutomationTakeProgress::Started)
    }

    pub(in crate::service) fn stop_editor(
        &mut self,
        key: GestureKey,
        request_id: u64,
        sink: &mut dyn WorkerAudioSink,
    ) -> bool {
        self.state
            .active
            .as_ref()
            .is_some_and(|active| active.editor_request == Some(request_id) && active.key == key)
            && sink.cancel_plugin_editor_recording(key)
    }
}
