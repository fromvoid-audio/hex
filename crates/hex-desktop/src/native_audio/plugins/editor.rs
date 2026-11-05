//! Route one device-chain editor toggle to its original hosted instance.
use super::*;
use hex_audio::plugin_worker::{
    ControllerRequest, NativeWorkerControllerProgress, NativeWorkerControllerQuery,
    PluginEditorState,
};
use std::time::{Duration, Instant};

pub(super) enum EditorOperation {
    Pending {
        binding: Binding,
        generation: u64,
        report: bool,
        request: Option<NativeWorkerControllerQuery>,
        admission_deadline: Instant,
    },
    Complete {
        generation: u64,
        result: Result<bool, String>,
    },
}

impl Plugins {
    pub(in crate::native_audio) fn toggle_editor(
        &mut self,
        generation: u64,
        device: u64,
    ) -> Result<(), String> {
        if self.editor.is_some()
            || self.restore.is_some()
            || self.reopen.is_some()
            || self.edit.is_some()
        {
            return Err("vst3 editor is busy · try again after preparation finishes".into());
        }
        let source = self
            .sources
            .iter()
            .rev()
            .find(|source| {
                source.accepted
                    && !source.retiring
                    && source.device == device
                    && source.project_generation == generation
            })
            .ok_or("vst3 editor needs an active hosted instance")?;
        self.queue_editor(source.binding, generation, true);
        Ok(())
    }

    fn queue_editor(&mut self, binding: Binding, generation: u64, report: bool) {
        self.editor = Some(EditorOperation::Pending {
            binding,
            generation,
            report,
            request: None,
            admission_deadline: Instant::now() + Duration::from_millis(250),
        });
    }

    pub(super) fn poll_editor(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        current: Option<AudioSourceContext>,
    ) {
        let Some(EditorOperation::Pending {
            binding,
            generation,
            report,
            request,
            admission_deadline,
        }) = self.editor.as_mut()
        else {
            return;
        };
        let progress = if let Some(request) = request {
            control.poll_native_source_controller_query_on_control(*binding, request)
        } else if current.is_none_or(|context| context.project_generation != *generation) {
            Err("vst3 editor target changed")
        } else {
            let action = if *report {
                ControllerRequest::ToggleEditor
            } else {
                ControllerRequest::CloseEditor
            };
            match control.begin_native_source_controller_query_on_control(*binding, action) {
                Ok(token) => {
                    *request = Some(token);
                    return;
                }
                Err("native preparation is busy" | "native controller mailbox is busy")
                    if Instant::now() < *admission_deadline =>
                {
                    return;
                }
                Err("native preparation is busy" | "native controller mailbox is busy") => {
                    Err("vst3 editor is busy")
                }
                Err(error) => Err(error),
            }
        };
        let result = match progress {
            Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                return;
            }
            Ok(NativeWorkerControllerProgress::Editor(PluginEditorState::Open)) => Ok(true),
            Ok(NativeWorkerControllerProgress::Editor(PluginEditorState::Closed)) => Ok(false),
            Ok(NativeWorkerControllerProgress::Editor(PluginEditorState::Unavailable)) => {
                Err("native editor unavailable for this vst3".into())
            }
            Err(error) => Err(error.into()),
            Ok(_) => Err("vst3 editor operation failed".into()),
        };
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.binding == *binding)
        {
            if let Ok(open) = result {
                source.editor_open = open;
                if open {
                    self.editor_checked = None;
                }
            } else if !*report {
                // The exact source disappeared or failed. Its own retirement
                // closes any remaining native window.
                source.editor_open = false;
            }
        }
        self.editor = report.then_some(EditorOperation::Complete {
            generation: *generation,
            result,
        });
    }

    /// Removed devices can retain a warm worker for Undo. Close its editor
    /// after the canonical device disappears, even if the helper stays alive.
    pub(in crate::native_audio) fn close_removed_editor(
        &mut self,
        desired: &CanonicalAudioSourceSnapshot,
    ) {
        let context = desired.context();
        if self.editor.is_some() || self.editor_checked == Some(context) {
            return;
        }
        let removed = self.sources.iter().find(|source| {
            if !source.editor_open
                || source.retiring
                || source.project_generation != context.project_generation
            {
                return false;
            }
            let mut present = false;
            let checked = desired
                .effect_preparation()
                .visit_plugin_effects(|_, device, _| {
                    present |= device == source.device;
                    Ok(())
                });
            checked.is_ok() && !present
        });
        if let Some(source) = removed {
            self.queue_editor(source.binding, context.project_generation, false);
        } else {
            self.editor_checked = Some(context);
        }
    }

    pub(in crate::native_audio) fn editor_ready(&self) -> bool {
        matches!(self.editor, Some(EditorOperation::Complete { .. }))
    }

    pub(in crate::native_audio) fn take_editor_result(
        &mut self,
        current_generation: u64,
    ) -> Option<Result<bool, String>> {
        if !self.editor_ready() {
            return None;
        }
        match self.editor.take()? {
            EditorOperation::Complete { generation, result }
                if generation == current_generation =>
            {
                Some(result)
            }
            _ => None,
        }
    }
}
