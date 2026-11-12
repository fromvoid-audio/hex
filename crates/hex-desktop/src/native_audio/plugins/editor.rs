//! Serialize editor operations on the exact hosted instance. A committed
//! replacement can reopen an attached editor after its predecessor detaches.
//! Only view intent and placement cross the handoff. Root owns processors and state.
use super::*;
use hex_audio::plugin_worker::{
    ControllerRequest, NativeWorkerControllerProgress, NativeWorkerControllerQuery,
    PluginEditorPosition, PluginEditorState,
};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditorAction {
    Toggle,
    Close,
    Detach,
    Reopen(Option<PluginEditorPosition>),
}

impl EditorAction {
    fn request(self) -> ControllerRequest {
        match self {
            Self::Toggle => ControllerRequest::ToggleEditor,
            Self::Close => ControllerRequest::CloseEditor,
            Self::Detach => ControllerRequest::DetachEditor,
            Self::Reopen(position) => ControllerRequest::ReopenEditor(position),
        }
    }
}

pub(super) enum EditorOperation {
    Pending {
        binding: Binding,
        generation: u64,
        action: EditorAction,
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
            || self.sources.iter().any(|source| {
                source.editor_transfer
                    && source.device == device
                    && source.project_generation == generation
            })
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
        let binding = source.binding;
        if let Some(inventory) = self.parameters.as_mut() {
            inventory.request_yield();
        }
        self.queue_editor(binding, generation, EditorAction::Toggle);
        Ok(())
    }

    fn queue_editor(&mut self, binding: Binding, generation: u64, action: EditorAction) {
        self.editor = Some(EditorOperation::Pending {
            binding,
            generation,
            action,
            request: None,
            admission_deadline: Instant::now() + Duration::from_millis(250),
        });
    }

    pub(super) fn editor_holds(&self, binding: Binding) -> bool {
        matches!(
            self.editor,
            Some(EditorOperation::Pending { binding: current, .. }) if current == binding
        )
    }

    fn editor_successor(&self, binding: Binding) -> Option<Binding> {
        let old = self
            .sources
            .iter()
            .find(|source| source.binding == binding)?;
        self.sources
            .iter()
            .rev()
            .find(|source| {
                source.accepted
                    && !source.retiring
                    && source.binding.root == binding.root
                    && source.project_generation == old.project_generation
                    && source.owner == old.owner
                    && source.device == old.device
            })
            .map(|source| source.binding)
    }

    fn queue_next_editor_transfer(&mut self, current: Option<AudioSourceContext>) {
        if self.editor.is_some() {
            return;
        }
        for index in 0..self.sources.len() {
            if !self.sources[index].editor_transfer {
                continue;
            }
            let source = &mut self.sources[index];
            source.editor_transfer = false;
            let (binding, generation) = (source.binding, source.project_generation);
            if current.is_some_and(|context| context.project_generation == generation)
                && self.editor_successor(binding).is_some()
            {
                if let Some(inventory) = self.parameters.as_mut() {
                    inventory.request_yield();
                }
                self.queue_editor(binding, generation, EditorAction::Detach);
                return;
            }
        }
    }

    pub(super) fn poll_editor(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        current: Option<AudioSourceContext>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
    ) {
        if desired.is_some() {
            self.queue_next_editor_transfer(current);
        }
        if desired.is_none()
            && matches!(
                self.editor,
                Some(EditorOperation::Pending {
                    action: EditorAction::Reopen(_),
                    request: None,
                    ..
                })
            )
        {
            return;
        }
        // An intervening accepted edit can supersede the queued target.
        // Resolve it before admission while the previous binding is still held.
        let can_reopen = match &self.editor {
            Some(EditorOperation::Pending {
                binding,
                action: EditorAction::Reopen(_),
                request: None,
                ..
            }) => {
                let target = self.editor_successor(*binding).filter(|&target| {
                    desired.is_some_and(|snapshot| self.editor_source_present(target, snapshot))
                });
                if let Some(target) = target
                    && let Some(EditorOperation::Pending { binding, .. }) = &mut self.editor
                {
                    *binding = target;
                }
                target.is_some()
            }
            _ => true,
        };
        let Some(EditorOperation::Pending {
            binding,
            generation,
            action,
            request,
            admission_deadline,
        }) = self.editor.as_mut()
        else {
            return;
        };
        let progress = if let Some(request) = request {
            control.poll_native_source_controller_query_on_control(*binding, request)
        } else if !can_reopen
            || current.is_none_or(|context| context.project_generation != *generation)
        {
            Err("vst3 editor target changed")
        } else {
            match control
                .begin_native_source_controller_query_on_control(*binding, action.request())
            {
                Ok(token) => {
                    *request = Some(token);
                    return;
                }
                Err(
                    "native preparation is busy"
                    | "native controller mailbox is busy"
                    | "native controller query is already pending",
                ) if Instant::now() < *admission_deadline => {
                    return;
                }
                Err(
                    "native preparation is busy"
                    | "native controller mailbox is busy"
                    | "native controller query is already pending",
                ) => Err("vst3 editor is busy"),
                Err(error) => Err(error),
            }
        };
        let result = match progress {
            Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                return;
            }
            Ok(NativeWorkerControllerProgress::Editor(PluginEditorState::Unavailable)) => {
                Err("native editor unavailable for this vst3".into())
            }
            Ok(NativeWorkerControllerProgress::Editor(state)) => Ok(state),
            Err(error) => Err(error.into()),
            Ok(_) => Err("vst3 editor operation failed".into()),
        };
        let (binding, generation, action) = (*binding, *generation, *action);
        self.finish_editor_action(binding, generation, action, result, current);
    }

    fn finish_editor_action(
        &mut self,
        binding: Binding,
        generation: u64,
        action: EditorAction,
        result: Result<PluginEditorState, String>,
        current: Option<AudioSourceContext>,
    ) {
        if matches!(action, EditorAction::Detach | EditorAction::Reopen(_))
            && let Err(reason) = &result
            && let Some(log) = &self.diagnostics
        {
            let _ = log.record_typed(
                hex_diagnostics::Level::Warn,
                hex_diagnostics::Category::Audio,
                &format!("vst3_editor v=1 stage=handoff_refused action={action:?} binding={binding:?} reason={reason}"),
            );
        }
        let placement = match &result {
            Ok(PluginEditorState::Detached(position)) => Some(*position),
            _ => None,
        };
        let open = matches!(
            result,
            Ok(PluginEditorState::Open | PluginEditorState::Detached(_))
        );
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.binding == binding)
        {
            match action {
                EditorAction::Detach | EditorAction::Close => source.editor_open = false,
                EditorAction::Toggle | EditorAction::Reopen(_) if result.is_ok() => {
                    source.editor_open = open;
                    if open {
                        self.editor_checked = None;
                        // A later commit can supersede the target while the
                        // original mailbox operation is still in flight.
                        source.editor_transfer |= source.retiring;
                    }
                }
                _ => {}
            }
        }
        self.editor = (action == EditorAction::Toggle).then_some(EditorOperation::Complete {
            generation,
            result: result.map(|_| open),
        });
        if action == EditorAction::Detach
            && open
            && current.is_some_and(|context| context.project_generation == generation)
            && let Some(successor) = self.editor_successor(binding)
        {
            self.queue_editor(successor, generation, EditorAction::Reopen(placement));
        }
    }

    fn editor_source_present(
        &self,
        binding: Binding,
        desired: &CanonicalAudioSourceSnapshot,
    ) -> bool {
        let Some(source) = self
            .sources
            .iter()
            .find(|source| source.binding == binding && source.accepted && !source.retiring)
        else {
            return false;
        };
        let mut present = false;
        desired
            .effect_preparation()
            .visit_plugin_devices(|destination, device, _| {
                present |= device == source.device && destination.owner() == source.owner;
                Ok(())
            })
            .is_ok()
            && present
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
                .visit_plugin_devices(|_, device, _| {
                    present |= device == source.device;
                    Ok(())
                });
            checked.is_ok() && !present
        });
        if let Some(source) = removed {
            self.queue_editor(
                source.binding,
                context.project_generation,
                EditorAction::Close,
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn source(binding: Binding, context: AudioSourceContext, open: bool) -> Source {
        Source {
            binding,
            owner: ProcessorChainOwner::Main,
            device: binding.instance,
            project_generation: context.project_generation,
            operation: SourceOperation::Edit(context),
            authored: None,
            accepted: false,
            retiring: false,
            queued: true,
            observes_latency: false,
            editor_open: open,
            editor_transfer: false,
            edit_backlog: false,
            edit_recovery: None,
            pending_touch: None,
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        }
    }

    #[test]
    fn committed_replacement_hands_off_only_an_editor_the_worker_says_was_open() {
        let before = AudioSourceContext {
            project_generation: 1,
            editor_revision: 2,
        };
        let after = AudioSourceContext {
            editor_revision: 3,
            ..before
        };
        let old = Binding {
            root: 1,
            instance: 2,
            graph: 2,
            epoch: 1,
        };
        let new = Binding {
            graph: 3,
            epoch: 2,
            ..old
        };
        let position = PluginEditorPosition::new(-120.25, 905.5).unwrap();
        for (accepted, was_open) in [(false, true), (true, false), (true, true)] {
            let mut plugins = Plugins {
                edit: Some(after),
                ..Plugins::default()
            };
            let mut original = source(old, before, true);
            original.accepted = true;
            plugins
                .sources
                .extend([original, source(new, after, false)]);
            plugins.finish_edit(after, accepted);
            plugins.queue_next_editor_transfer(Some(after));
            if !accepted {
                assert!(plugins.editor.is_none());
                assert!(plugins.sources[0].accepted && plugins.sources[0].editor_open);
                continue;
            }
            assert!(plugins.sources[0].retiring);
            assert!(plugins.editor_holds(old));
            let state = if was_open {
                PluginEditorState::Detached(position)
            } else {
                PluginEditorState::Closed
            };
            plugins.finish_editor_action(old, 1, EditorAction::Detach, Ok(state), Some(after));
            assert!(!plugins.editor_holds(old));
            assert!(!plugins.sources[0].editor_open);
            assert_eq!(plugins.editor_holds(new), was_open);
            if was_open {
                assert!(matches!(
                    plugins.editor,
                    Some(EditorOperation::Pending { action: EditorAction::Reopen(Some(p)), .. }) if p == position
                ));
                plugins.finish_editor_action(
                    new,
                    1,
                    EditorAction::Reopen(Some(position)),
                    Ok(PluginEditorState::Open),
                    Some(after),
                );
                assert!(plugins.sources[1].editor_open);
                assert!(plugins.editor.is_none());
            }
        }
    }
}
