//! Local source selection only. Session owns draft admission, original
//! plug-in editor custody and publication. The view never consumes SDK facts.
use super::*;
use hex_session::service::{DeviceAutomationTakeCompletion, PluginEditorRecordingInput};

#[derive(Clone, Copy)]
pub(super) struct EditorCapture {
    request_id: u64,
    key: GestureKey,
    arm: Arm,
    cancelling: bool,
}

impl EditorView {
    pub(in crate::editor) fn vst3_editor_arm_selected(
        &self,
        device: DeviceId,
        class: [u8; 16],
        parameter: u32,
    ) -> bool {
        let target = ArmTarget::Vst3 {
            device,
            class,
            parameter,
        };
        self.native_automation_recorder
            .editor_arm
            .is_some_and(|arm| {
                arm.target == target
                    && arm.generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == arm.revision)
            })
    }

    pub(in crate::editor) fn toggle_vst3_editor_arm(
        &mut self,
        device: DeviceId,
        class: [u8; 16],
        parameter: u32,
    ) {
        if self.automation_close_preflight.frozen() {
            return;
        }
        if self.vst3_editor_arm_selected(device, class, parameter) {
            self.native_automation_recorder.editor_arm = None;
            self.wake_poll();
            return;
        }
        if self.automation_take_recovery.unresolved()
            || self.native_automation_recorder.busy()
            || self.gain_recorder.blocks_edits()
            || self.midi_recorder.blocks_edits()
            || self.device_gesture_pending()
            || self.edit_pending()
        {
            self.error = Some("finish the current edit or take before arming automation".into());
            return;
        }
        if self.audio.is_none()
            || self.document_audio_mode != hex_session::DocumentAudioMode::Resident
        {
            self.error = Some("automation recording requires ordinary playback".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let arm = Arm {
            generation: self.project_generation,
            revision: state.revision,
            target: ArmTarget::Vst3 {
                device,
                class,
                parameter,
            },
        };
        if self.native_automation_recorder.arm.is_some_and(|armed| {
            matches!(armed.target, ArmTarget::Native { .. })
                && armed.generation == arm.generation
                && armed.revision == arm.revision
        }) && !self.enqueue(Request::ArmNativeParameterRecording {
            project_generation: arm.generation,
            revision: arm.revision,
            target: None,
        }) {
            return;
        }
        self.native_automation_recorder.arm = None;
        self.native_automation_recorder.editor_arm = Some(arm);
        self.wake_poll();
    }

    pub(super) fn tick_plugin_editor_recording(&mut self) -> bool {
        let selected = self.native_automation_recorder.editor_arm;
        if let Some(capture) = self.native_automation_recorder.editor_take.as_mut() {
            if selected.is_none() && !capture.cancelling {
                capture.cancelling = true;
                let request = Request::StopPluginEditorRecording {
                    request_id: capture.request_id,
                    key: capture.key,
                };
                if self.enqueue(request) {
                    return true;
                }
                if let Some(capture) = self.native_automation_recorder.editor_take.as_mut() {
                    capture.cancelling = false;
                }
            }
            return false;
        }
        let Some(arm) = selected else { return false };
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if arm.generation != self.project_generation || arm.revision != state.revision {
            self.native_automation_recorder.editor_arm = None;
            return true;
        }
        if self.automation_close_preflight.frozen()
            || self.automation_take_recovery.unresolved()
            || self.gain_recorder.blocks_edits()
            || self.midi_recorder.blocks_edits()
            || self.device_gesture_pending()
            || self.edit_pending()
        {
            return false;
        }
        let context = hex_engine::AudioSourceContext {
            project_generation: arm.generation,
            editor_revision: arm.revision,
        };
        let playing = self
            .audio
            .as_ref()
            .and_then(|audio| audio.status())
            .and_then(|status| status.live_native_parameter_source())
            .is_some_and(|(source, _)| source.context == context);
        if !playing || !self.play_intent.desired_playing(playing) {
            return false;
        }
        let ArmTarget::Vst3 {
            device,
            class,
            parameter,
        } = arm.target
        else {
            return false;
        };
        let Ok(key) = self
            .device_gesture
            .reserve_plugin_editor_key(arm.generation)
        else {
            return false;
        };
        let Ok(request_id) = self.native_automation_recorder.reserve_request() else {
            self.native_automation_recorder.editor_arm = None;
            self.error = Some("automation request limit reached".into());
            return true;
        };
        self.native_automation_recorder.editor_take = Some(EditorCapture {
            request_id,
            key,
            arm,
            cancelling: false,
        });
        if !self.enqueue(Request::StartPluginEditorRecording {
            request_id,
            input: PluginEditorRecordingInput {
                key,
                context,
                device,
                class,
                parameter,
                reading: false,
            },
        }) {
            self.native_automation_recorder.editor_take = None;
            self.native_automation_recorder.editor_arm = None;
        }
        true
    }

    pub(super) fn plugin_editor_take_completion(
        &mut self,
        event: &Event,
        completion: &DeviceAutomationTakeCompletion,
    ) -> bool {
        let Some(capture) = self
            .native_automation_recorder
            .editor_take
            .filter(|capture| capture.request_id == completion.request_id)
        else {
            return false;
        };
        match &completion.progress {
            Progress::Started | Progress::Pending => {}
            Progress::Committed => {
                self.native_automation_recorder.editor_take = None;
                if self.native_automation_recorder.editor_arm == Some(capture.arm) {
                    let mut next = capture.arm;
                    next.revision = event.state.revision;
                    self.native_automation_recorder.editor_arm = Some(next);
                }
            }
            Progress::Unchanged => {
                self.native_automation_recorder.editor_take = None;
            }
            Progress::Retained(path) | Progress::CommittedWithRetainedCustody(path) => {
                self.native_automation_recorder.editor_take = None;
                self.native_automation_recorder.editor_arm = None;
                self.error = Some(format!("automation take retained at {}", path.display()).into());
            }
            _ => {
                self.native_automation_recorder.editor_take = None;
                self.native_automation_recorder.editor_arm = None;
            }
        }
        true
    }

    pub(super) fn plugin_editor_request_not_submitted(
        &mut self,
        request: &Request,
        worker_closed: bool,
    ) -> bool {
        let (request_id, start) = match request {
            Request::StartPluginEditorRecording { request_id, .. } => (*request_id, true),
            Request::StopPluginEditorRecording { request_id, .. } => (*request_id, false),
            _ => return false,
        };
        if let Some(capture) = self.native_automation_recorder.editor_take.as_mut()
            && capture.request_id == request_id
        {
            if start {
                self.native_automation_recorder.editor_take = None;
                self.native_automation_recorder.editor_arm = None;
            } else if !worker_closed {
                capture.cancelling = false;
            }
        }
        true
    }
}
