//! Local source selection only. Session owns draft admission, original
//! plug-in editor custody and publication. The view never consumes SDK facts.
use super::*;
use hex_session::service::{
    DeviceAutomationTakeCompletion, MAX_VST3_EDITOR_GROUP_MEMBERS as MEMBERS,
    PluginEditorRecordingInput, PluginEditorRecordingTarget, automation_draft::Vst3GroupTarget,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct EditorArm {
    generation: u64,
    revision: u64,
    target: PluginEditorRecordingTarget,
}

#[derive(Clone, Copy)]
pub(super) struct EditorGroupSelection {
    generation: u64,
    revision: u64,
    device: DeviceId,
    class: [u8; 16],
    parameters: [u32; MEMBERS],
    len: usize,
}

impl EditorGroupSelection {
    fn ids(&self) -> &[u32] {
        &self.parameters[..self.len]
    }

    fn group(&self) -> Option<Vst3GroupTarget> {
        Vst3GroupTarget::new(self.device, self.class, self.ids()).ok()
    }

    fn toggle(&mut self, parameter: u32) -> Result<(), &'static str> {
        match self.ids().binary_search(&parameter) {
            Ok(index) => {
                self.parameters.copy_within(index + 1..self.len, index);
                self.len -= 1;
            }
            Err(_) if self.len == MEMBERS => return Err("vst3 editor group is full"),
            Err(index) => {
                self.parameters.copy_within(index..self.len, index + 1);
                self.parameters[index] = parameter;
                self.len += 1;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) struct EditorCapture {
    request_id: u64,
    key: GestureKey,
    arm: EditorArm,
    cancelling: bool,
}

impl Recorder {
    pub(in crate::editor) fn permits_original_editor_window(&self, request: &Request) -> bool {
        let Request::TogglePluginEditor {
            project_generation,
            device,
        } = request
        else {
            return false;
        };
        self.editor_take.is_some_and(|capture| {
            capture.arm.generation == *project_generation && capture.arm.target.device() == *device
        })
    }
}

impl EditorView {
    pub(in crate::editor) fn vst3_editor_arm_selected(
        &self,
        device: DeviceId,
        class: [u8; 16],
        parameter: u32,
    ) -> bool {
        self.editor_arm_matches(PluginEditorRecordingTarget::Single {
            device,
            class,
            parameter,
            reading: false,
        })
    }

    fn editor_arm_matches(&self, target: PluginEditorRecordingTarget) -> bool {
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
        self.toggle_editor_arm(PluginEditorRecordingTarget::Single {
            device,
            class,
            parameter,
            reading: false,
        });
    }

    fn editor_group_for(&self, device: DeviceId, class: [u8; 16]) -> Option<EditorGroupSelection> {
        self.native_automation_recorder
            .editor_group
            .filter(|group| {
                group.device == device
                    && group.class == class
                    && group.generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == group.revision)
            })
    }

    pub(in crate::editor) fn vst3_editor_group_menu(
        &self,
        device: DeviceId,
        class: [u8; 16],
        parameter: u32,
    ) -> Option<(&'static str, Option<&'static str>)> {
        let assigned = self
            .state
            .as_ref()?
            .project
            .device(device)?
            .automation_target(DeviceAutomationTarget::Vst3 { parameter })?;
        if assigned.vst3_class != Some(class) {
            return None;
        }
        let selection = self.editor_group_for(device, class);
        let included = selection.is_some_and(|group| group.ids().binary_search(&parameter).is_ok());
        let membership = if included {
            "remove from plug-in editor group"
        } else {
            "add to plug-in editor group"
        };
        let arm = selection
            .filter(|_| included)
            .and_then(|group| group.group())
            .map(|group| {
                if self.editor_arm_matches(PluginEditorRecordingTarget::Group(group)) {
                    "disarm plug-in editor group"
                } else {
                    "arm plug-in editor group"
                }
            });
        Some((membership, arm))
    }

    pub(in crate::editor) fn toggle_vst3_editor_group_member(
        &mut self,
        device: DeviceId,
        class: [u8; 16],
        parameter: u32,
    ) {
        if self.automation_close_preflight.frozen()
            || self
                .vst3_editor_group_menu(device, class, parameter)
                .is_none()
        {
            return;
        }
        if self.native_automation_recorder.busy() {
            self.error =
                Some("finish the current automation take before changing its group".into());
            return;
        }
        let Some(revision) = self.state.as_ref().map(|state| state.revision) else {
            return;
        };
        let mut group = self
            .editor_group_for(device, class)
            .unwrap_or(EditorGroupSelection {
                generation: self.project_generation,
                revision,
                device,
                class,
                parameters: [0; MEMBERS],
                len: 0,
            });
        if let Err(reason) = group.toggle(parameter) {
            self.error = Some(reason.into());
            return;
        }
        if self
            .native_automation_recorder
            .editor_arm
            .is_some_and(|arm| matches!(arm.target, PluginEditorRecordingTarget::Group(_)))
        {
            self.native_automation_recorder.editor_arm = None;
        }
        self.native_automation_recorder.editor_group = (group.len != 0).then_some(group);
        self.wake_poll();
    }

    pub(in crate::editor) fn toggle_vst3_editor_group_arm(
        &mut self,
        device: DeviceId,
        class: [u8; 16],
    ) {
        if let Some(group) = self
            .editor_group_for(device, class)
            .and_then(|group| group.group())
        {
            self.toggle_editor_arm(PluginEditorRecordingTarget::Group(group));
        }
    }

    fn toggle_editor_arm(&mut self, target: PluginEditorRecordingTarget) {
        if self.automation_close_preflight.frozen() {
            return;
        }
        if self.editor_arm_matches(target) {
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
        let arm = EditorArm {
            generation: self.project_generation,
            revision: state.revision,
            target,
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
                target: arm.target,
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
                    if let Some(group) = self.native_automation_recorder.editor_group.as_mut()
                        && group.generation == capture.arm.generation
                        && group.revision == capture.arm.revision
                        && group.group().is_some_and(|target| {
                            next.target == PluginEditorRecordingTarget::Group(target)
                        })
                    {
                        group.revision = next.revision;
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_armed_editor_can_open_its_exact_window_without_unblocking_other_edits() {
        let effect = hex_engine::NativeDeviceClass::Effect;
        let utility = hex_engine::native_device_catalog_for_class(effect)
            .unwrap()
            .into_iter()
            .find(|entry| entry.type_id.as_str() == Ok("hex.utility"))
            .unwrap()
            .type_id;
        let state = hex_engine::default_native_device_state_for_class(effect, utility).unwrap();
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
        let mut add = || {
            let hex_project::Created::Device(device) = editor
                .execute(hex_project::Command::InsertMainDevice {
                    before: None,
                    state: state.clone(),
                })
                .unwrap()
            else {
                panic!("fixture device")
            };
            device
        };
        let selected = add();
        let other = add();
        let recorder = Recorder {
            editor_take: Some(EditorCapture {
                request_id: 1,
                key: GestureKey {
                    generation: 0,
                    token: 1,
                },
                arm: EditorArm {
                    generation: 0,
                    revision: 7,
                    target: PluginEditorRecordingTarget::Single {
                        device: selected,
                        class: [1; 16],
                        parameter: 0,
                        reading: false,
                    },
                },
                cancelling: false,
            }),
            ..Recorder::default()
        };
        assert!(recorder.busy());
        for (project_generation, device, allowed) in
            [(0, selected, true), (0, other, false), (1, selected, false)]
        {
            assert_eq!(
                recorder.permits_original_editor_window(&Request::TogglePluginEditor {
                    project_generation,
                    device,
                }),
                allowed
            );
        }
        assert!(!recorder.permits_original_editor_window(&Request::Undo));

        let mut recorder = recorder;
        let capture = recorder.editor_take.as_mut().unwrap();
        let mut selection = EditorGroupSelection {
            generation: 0,
            revision: 7,
            device: selected,
            class: [1; 16],
            parameters: [0; MEMBERS],
            len: 0,
        };
        selection.toggle(11).unwrap();
        assert!(selection.group().is_none());
        selection.toggle(0).unwrap();
        let group = selection.group().unwrap();
        assert_eq!(group.ids(), &[0, 11]);
        capture.arm.target = PluginEditorRecordingTarget::Group(group);
        assert!(
            recorder.permits_original_editor_window(&Request::TogglePluginEditor {
                project_generation: 0,
                device: selected,
            })
        );
        assert!(
            !recorder.permits_original_editor_window(&Request::TogglePluginEditor {
                project_generation: 0,
                device: other,
            })
        );
    }
}
