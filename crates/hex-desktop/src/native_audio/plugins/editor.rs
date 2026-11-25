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
    Open,
    Observe,
    Close,
    Detach,
    Reopen(Option<PluginEditorPosition>),
}

impl EditorAction {
    fn request(self) -> ControllerRequest {
        match self {
            Self::Toggle => ControllerRequest::ToggleEditor,
            Self::Open => ControllerRequest::ReopenEditor(None),
            Self::Observe => ControllerRequest::EditorVisibility,
            Self::Close => ControllerRequest::CloseEditor,
            Self::Detach => ControllerRequest::DetachEditorWhenIdle,
            Self::Reopen(position) => ControllerRequest::ReopenEditor(position),
        }
    }
}

pub(super) struct EditorPending {
    pub(super) binding: Binding,
    pub(super) generation: u64,
    pub(super) action: EditorAction,
    pub(super) request: Option<NativeWorkerControllerQuery>,
    pub(super) admission_deadline: Instant,
    pub(super) retry_after: Option<Instant>,
}

enum EditorLane {
    Explicit,
    Visibility,
}

impl Plugins {
    pub(in crate::native_audio) fn toggle_editor(
        &mut self,
        generation: u64,
        device: u64,
    ) -> Result<(), String> {
        if self.restore.is_some()
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
        let binding = self
            .sources
            .iter()
            .rev()
            .find(|source| {
                source.accepted
                    && !source.retiring
                    && source.device == device
                    && source.project_generation == generation
            })
            .ok_or("vst3 editor needs an active hosted instance")?
            .binding;
        if self
            .queued_editor_toggles
            .iter()
            .any(|(queued, _)| *queued == binding)
            || self
                .editor_ops
                .iter()
                .any(|op| op.binding == binding && !matches!(op.action, EditorAction::Open))
        {
            return Err("vst3 editor is busy · try again after preparation finishes".into());
        }
        if let Some(source) = self
            .sources
            .iter_mut()
            .rev()
            .find(|source| source.binding == binding)
        {
            source.editor_open_on_insert = false;
        }
        if let Some(inventory) = self.parameters.as_mut()
            && inventory.binding() == binding
        {
            inventory.request_yield();
        }
        if self.editor_holds(binding) {
            self.queued_editor_toggles.push((binding, generation));
        } else {
            self.queue_editor(binding, generation, EditorAction::Toggle);
        }
        Ok(())
    }

    fn queue_editor(&mut self, binding: Binding, generation: u64, action: EditorAction) {
        if action == EditorAction::Open {
            record_insertion_stage(
                self.diagnostics.as_ref(),
                binding,
                generation,
                "automatic_editor_queued",
                format_args!(""),
            );
        }
        let pending = EditorPending {
            binding,
            generation,
            action,
            request: None,
            admission_deadline: Instant::now() + Duration::from_millis(250),
            retry_after: None,
        };
        if action == EditorAction::Observe {
            self.editor_visibility.push(pending);
        } else {
            self.editor_ops.push(pending);
        }
    }

    pub(super) fn editor_holds(&self, binding: Binding) -> bool {
        self.editor_ops.iter().any(|op| op.binding == binding)
            || self
                .editor_visibility
                .iter()
                .any(|op| op.binding == binding)
    }

    /// Maximum queued stale Space bytes drained from one ineligible editor socket per poll.
    const MAX_STALE_TRANSPORT_SPACE_DRAIN: usize = 64;

    fn transport_space_eligible(&self, source: &Source, generation: u64) -> bool {
        self.transport_space
            && source.project_generation == generation
            && source.accepted
            && !source.retiring
            && source.editor_open
    }

    fn drain_transport_space_with(binding: Binding, take: &mut impl FnMut(Binding) -> bool) {
        for _ in 0..Self::MAX_STALE_TRANSPORT_SPACE_DRAIN {
            if !take(binding) {
                break;
            }
        }
    }

    /// Consume at most one transport Space shortcut from an accepted, open editor
    /// in `generation`.
    ///
    /// Close policy: once an editor is closed (or while opening/reopening before
    /// the helper confirms it is open), retiring, unaccepted, or in another
    /// project generation, any unread bytes on its source-lifetime socket are
    /// drained and discarded so reopening the editor cannot replay an earlier
    /// keypress.
    pub(in crate::native_audio) fn take_transport_space(
        &self,
        control: &hex_audio::prepared::PublicationControl,
        generation: u64,
    ) -> bool {
        self.take_transport_space_with(generation, |binding| {
            control.take_native_source_transport_space(binding)
        })
    }

    pub(in crate::native_audio) fn drain_ineligible_transport_space(
        &self,
        control: &hex_audio::prepared::PublicationControl,
        generation: u64,
    ) {
        self.drain_ineligible_transport_space_with(generation, |binding| {
            control.take_native_source_transport_space(binding)
        });
    }

    fn take_transport_space_with(
        &self,
        generation: u64,
        mut take: impl FnMut(Binding) -> bool,
    ) -> bool {
        let mut triggered = false;
        for source in &self.sources {
            if self.transport_space_eligible(source, generation) {
                if !triggered && take(source.binding) {
                    triggered = true;
                }
            } else {
                Self::drain_transport_space_with(source.binding, &mut take);
            }
        }
        triggered
    }

    fn drain_ineligible_transport_space_with(
        &self,
        generation: u64,
        mut take: impl FnMut(Binding) -> bool,
    ) {
        for source in &self.sources {
            if !self.transport_space_eligible(source, generation) {
                Self::drain_transport_space_with(source.binding, &mut take);
            }
        }
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
                    && source.device == old.device
            })
            .map(|source| source.binding)
    }

    fn queue_next_editor_transfer(&mut self, current: Option<AudioSourceContext>) {
        let mut to_queue = Vec::new();
        for index in 0..self.sources.len() {
            if !self.sources[index].editor_transfer {
                continue;
            }
            let binding = self.sources[index].binding;
            if self.editor_holds(binding) {
                continue;
            }
            let generation = self.sources[index].project_generation;
            self.sources[index].editor_transfer = false;
            if current.is_some_and(|context| context.project_generation == generation)
                && self.editor_successor(binding).is_some()
            {
                to_queue.push((binding, generation));
            }
        }
        for (binding, generation) in to_queue {
            if let Some(inventory) = self.parameters.as_mut()
                && inventory.binding() == binding
            {
                inventory.request_yield();
            }
            self.queue_editor(binding, generation, EditorAction::Detach);
        }
    }

    pub(in crate::native_audio) fn open_editor_devices(
        &self,
        generation: u64,
    ) -> impl Iterator<Item = u64> + '_ {
        self.sources.iter().filter_map(move |source| {
            let reopening = self.editor_ops.iter().any(|op| {
                op.binding == source.binding && matches!(op.action, EditorAction::Reopen(_))
            });
            (source.project_generation == generation && (source.editor_open || reopening))
                .then_some(source.device)
        })
    }

    fn queue_next_editor_work(&mut self, desired: &CanonicalAudioSourceSnapshot) {
        let context = desired.context();
        let generation = context.project_generation;
        let mut ready_toggles = Vec::new();
        let mut index = 0;
        while index < self.queued_editor_toggles.len() {
            let (binding, expected) = self.queued_editor_toggles[index];
            if expected != generation {
                self.queued_editor_toggles.remove(index);
            } else if !self.editor_holds(binding) {
                self.queued_editor_toggles.remove(index);
                ready_toggles.push(binding);
            } else {
                index += 1;
            }
        }
        for binding in ready_toggles {
            self.queue_editor(binding, generation, EditorAction::Toggle);
        }
        if self.restore.is_some() || self.reopen.is_some() || self.edit.is_some() {
            return;
        }
        let mut auto_opens = Vec::new();
        for index in 0..self.sources.len() {
            if !self.sources[index].editor_open_on_insert {
                continue;
            }
            let binding = self.sources[index].binding;
            if self.sources[index].project_generation == generation
                && context.editor_revision < binding.graph
            {
                continue;
            }
            if self.editor_holds(binding) {
                continue;
            }
            let same_generation = self.sources[index].project_generation == generation;
            self.sources[index].editor_open_on_insert = false;
            if same_generation && self.editor_source_present(binding, desired) {
                auto_opens.push(binding);
            }
        }
        for binding in auto_opens {
            if let Some(inventory) = self.parameters.as_mut()
                && inventory.binding() == binding
            {
                inventory.request_yield();
            }
            self.queue_editor(binding, generation, EditorAction::Open);
        }
        let now = Instant::now();
        if self.next_editor_visibility.is_some_and(|next| now < next) {
            return;
        }
        let count = self.sources.len();
        for offset in 0..count {
            let index = (self.editor_visibility_cursor + offset) % count;
            let source = &self.sources[index];
            if source.project_generation == generation
                && source.accepted
                && !source.retiring
                && source.editor_open
                && !self.editor_holds(source.binding)
                && self
                    .parameters
                    .as_ref()
                    .is_none_or(|inventory| inventory.binding() != source.binding)
                && self
                    .parameter_conversion
                    .as_ref()
                    .is_none_or(|conversion| conversion.binding() != source.binding)
            {
                self.editor_visibility_cursor = (index + 1) % count;
                self.next_editor_visibility = Some(now + Duration::from_millis(100));
                self.queue_editor(source.binding, generation, EditorAction::Observe);
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
        if let Some(desired) = desired {
            self.queue_next_editor_transfer(current);
            self.queue_next_editor_work(desired);
        }
        self.poll_editor_lane(EditorLane::Explicit, control, current, desired);
        self.poll_editor_lane(EditorLane::Visibility, control, current, desired);
    }

    fn poll_editor_lane(
        &mut self,
        lane: EditorLane,
        control: &mut hex_audio::prepared::PublicationControl,
        current: Option<AudioSourceContext>,
        desired: Option<&CanonicalAudioSourceSnapshot>,
    ) {
        let len = match lane {
            EditorLane::Explicit => self.editor_ops.len(),
            EditorLane::Visibility => self.editor_visibility.len(),
        };
        let mut completed = Vec::new();
        let mut index = 0;
        let mut remaining = len;
        while index < remaining {
            let (binding, action, has_request) = {
                let op = match lane {
                    EditorLane::Explicit => &self.editor_ops[index],
                    EditorLane::Visibility => &self.editor_visibility[index],
                };
                (op.binding, op.action, op.request.is_some())
            };
            if desired.is_none()
                && !has_request
                && matches!(
                    action,
                    EditorAction::Toggle
                        | EditorAction::Open
                        | EditorAction::Observe
                        | EditorAction::Reopen(_)
                )
            {
                index += 1;
                continue;
            }
            // An intervening accepted edit can supersede the queued target.
            // Resolve it before admission while the previous binding is still held.
            let can_admit = if !has_request {
                match action {
                    EditorAction::Reopen(_) => {
                        let target = self.editor_successor(binding).filter(|&target| {
                            desired.is_some_and(|snapshot| {
                                self.editor_source_present(target, snapshot)
                            })
                        });
                        if let Some(target) = target {
                            let op = match lane {
                                EditorLane::Explicit => &mut self.editor_ops[index],
                                EditorLane::Visibility => &mut self.editor_visibility[index],
                            };
                            op.binding = target;
                        }
                        target.is_some()
                    }
                    EditorAction::Toggle | EditorAction::Open | EditorAction::Observe => desired
                        .is_some_and(|snapshot| self.editor_source_present(binding, snapshot)),
                    EditorAction::Close | EditorAction::Detach => true,
                }
            } else {
                true
            };
            let op = match lane {
                EditorLane::Explicit => &mut self.editor_ops[index],
                EditorLane::Visibility => &mut self.editor_visibility[index],
            };
            if op.request.is_none()
                && (self.restore.is_some() || op.retry_after.is_some_and(|at| Instant::now() < at))
            {
                index += 1;
                continue;
            }
            let progress = if let Some(request) = &op.request {
                control.poll_native_source_controller_query_on_control(op.binding, request)
            } else if !can_admit
                || current.is_none_or(|context| context.project_generation != op.generation)
            {
                Err("vst3 editor target changed")
            } else {
                match control.begin_native_source_controller_query_on_control(
                    op.binding,
                    op.action.request(),
                ) {
                    Ok(token) => {
                        op.request = Some(token);
                        index += 1;
                        continue;
                    }
                    Err(
                        "native preparation is busy"
                        | "native controller mailbox is busy"
                        | "native controller query is already pending",
                    ) if Instant::now() < op.admission_deadline => {
                        index += 1;
                        continue;
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
                    index += 1;
                    continue;
                }
                Ok(NativeWorkerControllerProgress::Editor(PluginEditorState::Unavailable)) => {
                    Err("native editor unavailable for this vst3".into())
                }
                Ok(NativeWorkerControllerProgress::Editor(PluginEditorState::GestureActive))
                    if op.action == EditorAction::Detach =>
                {
                    op.request = None;
                    op.admission_deadline = Instant::now() + Duration::from_millis(250);
                    op.retry_after = Instant::now().checked_add(Duration::from_millis(10));
                    index += 1;
                    continue;
                }
                Ok(NativeWorkerControllerProgress::Editor(state)) => Ok(state),
                Err(error) => Err(error.into()),
                Ok(_) => Err("vst3 editor operation failed".into()),
            };
            let removed = match lane {
                EditorLane::Explicit => self.editor_ops.remove(index),
                EditorLane::Visibility => self.editor_visibility.remove(index),
            };
            remaining -= 1;
            completed.push((removed.binding, removed.generation, removed.action, result));
        }
        for (binding, generation, action, result) in completed {
            self.finish_editor_action(binding, generation, action, result, current);
            if self
                .sources
                .iter()
                .find(|source| source.binding == binding)
                .is_some_and(|source| !source.editor_open)
            {
                Self::drain_transport_space_with(binding, &mut |b| {
                    control.take_native_source_transport_space(b)
                });
            }
        }
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
        if action == EditorAction::Open {
            record_insertion_stage(
                self.diagnostics.as_ref(),
                binding,
                generation,
                "automatic_editor_terminal",
                format_args!(" open={open}"),
            );
        }
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.binding == binding)
        {
            match action {
                EditorAction::Detach | EditorAction::Close => source.editor_open = false,
                EditorAction::Toggle | EditorAction::Open | EditorAction::Reopen(_)
                    if result.is_ok() =>
                {
                    source.editor_open = open;
                    if open {
                        self.editor_checked = None;
                        // A later commit can supersede the target while the
                        // original mailbox operation is still in flight.
                        source.editor_transfer |= source.retiring;
                    }
                }
                EditorAction::Observe if result.is_ok() => source.editor_open = open,
                _ => {}
            }
        }
        if action == EditorAction::Observe {
            self.editor_visibility.retain(|op| op.binding != binding);
        } else {
            self.editor_ops
                .retain(|op| !(op.binding == binding && op.action == action));
        }
        if action == EditorAction::Toggle {
            self.editor_results
                .push_back((generation, result.map(|_| open)));
        }
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

    /// Close the editor of an absent device whose worker is still retained.
    pub(in crate::native_audio) fn close_removed_editor(
        &mut self,
        desired: &CanonicalAudioSourceSnapshot,
    ) {
        let context = desired.context();
        if self.editor_checked == Some(context) {
            return;
        }
        let mut defer_checked = false;
        let mut to_close = Vec::new();
        for source in &self.sources {
            if !source.editor_open || source.project_generation != context.project_generation {
                continue;
            }
            if source.editor_transfer || self.editor_holds(source.binding) {
                defer_checked = true;
                continue;
            }
            let mut present = false;
            let checked = desired
                .effect_preparation()
                .visit_plugin_devices(|_, device, _| {
                    present |= device == source.device;
                    Ok(())
                });
            if checked.is_ok() && !present {
                to_close.push(source.binding);
            }
        }
        for binding in to_close {
            self.queue_editor(binding, context.project_generation, EditorAction::Close);
        }
        if !defer_checked
            && self
                .editor_ops
                .iter()
                .all(|op| op.action != EditorAction::Close)
        {
            self.editor_checked = Some(context);
        }
    }

    pub(in crate::native_audio) fn editor_ready(&self) -> bool {
        !self.editor_results.is_empty()
    }

    pub(in crate::native_audio) fn take_editor_result(
        &mut self,
        current_generation: u64,
    ) -> Option<Result<bool, String>> {
        while let Some((generation, result)) = self.editor_results.pop_front() {
            if generation == current_generation {
                return Some(result);
            }
        }
        None
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
            operation: SourceOperation::Edit {
                context,
                predecessor: None,
            },
            authored: None,
            accepted: false,
            retiring: false,
            queued: true,
            observes_latency: false,
            editor_open: open,
            editor_transfer: false,
            editor_open_on_insert: false,
            edit_backlog: false,
            edit_recovery: None,
            open_editor_group: None,
            provider: None,
            editor_retirement: Default::default(),
            pending_touch: None,
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        }
    }

    #[test]
    fn interactive_insert_requests_open_only_after_acceptance_and_reports_observed_close() {
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 2,
        };
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 2,
            epoch: 1,
        };
        for (requested, accepted) in [(true, true), (true, false), (false, true)] {
            let mut plugins = Plugins::default();
            let operation = Arc::new(if requested {
                PluginEffectControl::opening_editor()
            } else {
                PluginEffectControl::default()
            });
            let mut source = source(binding, context, false);
            source.operation = SourceOperation::Insertion(Arc::clone(&operation));
            plugins.sources.push(source);
            plugins.pending = Some(Pending {
                operation: Arc::clone(&operation),
                binding,
                context,
                canonical_context: context,
                result: Some(Ok(NativeDeviceState::new(
                    hex_engine::DeviceTypeId::from_static("hex.vst3"),
                    1,
                    vec![0],
                )
                .unwrap())),
                facts_ready_for_session: true,
                facts_observed: true,
                #[cfg(feature = "allocation-probe")]
                facts_attempt_from_session: false,
            });
            plugins.finish(&operation, accepted);
            assert_eq!(
                plugins.sources[0].editor_open_on_insert,
                requested && accepted
            );
            assert_eq!(plugins.open_editor_devices(1).next(), None);
            if requested && accepted {
                plugins.finish_editor_action(
                    binding,
                    1,
                    EditorAction::Open,
                    Ok(PluginEditorState::Open),
                    Some(context),
                );
                assert_eq!(
                    plugins.open_editor_devices(1).next(),
                    Some(binding.instance)
                );
                assert_eq!(plugins.open_editor_devices(2).next(), None);
                plugins.finish_editor_action(
                    binding,
                    1,
                    EditorAction::Observe,
                    Ok(PluginEditorState::Closed),
                    Some(context),
                );
                assert_eq!(plugins.open_editor_devices(1).next(), None);
            }
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
                assert!(plugins.editor_ops.is_empty());
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
                assert!(plugins.editor_ops.iter().any(|op| {
                    op.binding == new
                        && matches!(op.action, EditorAction::Reopen(Some(p)) if p == position)
                }));
                plugins.finish_editor_action(
                    new,
                    1,
                    EditorAction::Reopen(Some(position)),
                    Ok(PluginEditorState::Open),
                    Some(after),
                );
                assert!(plugins.sources[1].editor_open);
                assert!(plugins.editor_ops.is_empty());
            }
        }
    }

    #[test]
    fn cross_track_move_transfers_open_editor_window_and_supersedes_reopen_across_owners() {
        let before = AudioSourceContext {
            project_generation: 1,
            editor_revision: 2,
        };
        let after_move = AudioSourceContext {
            editor_revision: 3,
            ..before
        };
        let after_undo_or_second_move = AudioSourceContext {
            editor_revision: 4,
            ..before
        };
        let donor_binding = Binding {
            root: 1,
            instance: 7,
            graph: 2,
            epoch: 1,
        };
        let receiver_binding = Binding {
            graph: 3,
            epoch: 2,
            ..donor_binding
        };
        let third_binding = Binding {
            graph: 4,
            epoch: 3,
            ..donor_binding
        };
        let position = PluginEditorPosition::with_size(240.0, 180.0, 540, 360).unwrap();

        let mut plugins = Plugins {
            edit: Some(after_move),
            ..Plugins::default()
        };
        let mut donor = source(donor_binding, before, true);
        donor.owner = ProcessorChainOwner::Track(10);
        donor.accepted = true;
        let mut receiver = source(receiver_binding, after_move, false);
        receiver.owner = ProcessorChainOwner::Track(20);
        plugins.sources.extend([donor, receiver]);

        // Cross-track move keeps DeviceId (7) while changing owner Track(10) -> Track(20).
        plugins.finish_edit(after_move, true);
        plugins.queue_next_editor_transfer(Some(after_move));
        assert!(
            plugins.editor_holds(donor_binding),
            "cross-track move must queue Detach on donor binding even though owner changed"
        );
        plugins.finish_editor_action(
            donor_binding,
            1,
            EditorAction::Detach,
            Ok(PluginEditorState::Detached(position)),
            Some(after_move),
        );
        assert!(!plugins.sources[0].editor_open);
        assert!(
            plugins.editor_ops.iter().any(|op| {
                op.binding == receiver_binding
                    && matches!(op.action, EditorAction::Reopen(Some(p)) if p == position && p.content_size() == Some((540, 360)))
            }),
            "detach must queue Reopen with prior position and resized content size on destination track's successor"
        );

        // Complete Reopen on receiver, then Undo back to Track(10) and Redo to Track(20),
        // preserving the resized placement on both transitions.
        plugins.finish_editor_action(
            receiver_binding,
            1,
            EditorAction::Reopen(Some(position)),
            Ok(PluginEditorState::Open),
            Some(after_move),
        );
        assert!(plugins.sources[1].editor_open);
        let mut undo_target = source(third_binding, after_undo_or_second_move, false);
        undo_target.owner = ProcessorChainOwner::Track(10);
        plugins.sources.push(undo_target);
        plugins.edit = Some(after_undo_or_second_move);
        plugins.finish_edit(after_undo_or_second_move, true);
        plugins.queue_next_editor_transfer(Some(after_undo_or_second_move));
        assert!(plugins.editor_holds(receiver_binding));
        plugins.finish_editor_action(
            receiver_binding,
            1,
            EditorAction::Detach,
            Ok(PluginEditorState::Detached(position)),
            Some(after_undo_or_second_move),
        );
        assert!(
            plugins.editor_ops.iter().any(|op| {
                op.binding == third_binding
                    && matches!(op.action, EditorAction::Reopen(Some(p)) if p == position && p.content_size() == Some((540, 360)))
            }),
            "undo must preserve resized placement on restored track successor"
        );
        plugins.finish_editor_action(
            third_binding,
            1,
            EditorAction::Reopen(Some(position)),
            Ok(PluginEditorState::Open),
            Some(after_undo_or_second_move),
        );
        assert!(plugins.sources[2].editor_open);
        let after_redo = AudioSourceContext {
            editor_revision: 5,
            ..before
        };
        let redo_binding = Binding {
            graph: 5,
            epoch: 4,
            ..donor_binding
        };
        let mut redo_target = source(redo_binding, after_redo, false);
        redo_target.owner = ProcessorChainOwner::Track(20);
        plugins.sources.push(redo_target);
        plugins.edit = Some(after_redo);
        plugins.finish_edit(after_redo, true);
        plugins.queue_next_editor_transfer(Some(after_redo));
        assert!(plugins.editor_holds(third_binding));
        plugins.finish_editor_action(
            third_binding,
            1,
            EditorAction::Detach,
            Ok(PluginEditorState::Detached(position)),
            Some(after_redo),
        );
        assert!(
            plugins.editor_ops.iter().any(|op| {
                op.binding == redo_binding
                    && matches!(op.action, EditorAction::Reopen(Some(p)) if p == position && p.content_size() == Some((540, 360)))
            }),
            "redo must preserve resized placement on destination track successor"
        );
        assert_eq!(
            plugins.editor_successor(receiver_binding),
            Some(redo_binding)
        );
    }

    #[test]
    fn deleted_draining_retiring_plugin_closes_orphaned_editor_and_clears_open_editor_devices() {
        let service = hex_session::service::Service::new(48_000, 8, 0).unwrap();
        let initial = loop {
            match service.try_event() {
                Ok(event) => break event,
                Err(std::sync::mpsc::TryRecvError::Empty) => std::thread::yield_now(),
                Err(error) => panic!("service event unavailable: {error}"),
            }
        };
        let desired = initial
            .state
            .canonical_audio_source
            .as_ref()
            .expect("empty project canonical audio source snapshot");
        let context = desired.context();
        let binding = Binding {
            root: 1,
            instance: 99,
            graph: context.editor_revision,
            epoch: 1,
        };

        let mut plugins = Plugins::default();
        let mut deleted = source(binding, context, true);
        deleted.accepted = true;
        // Simulates `retire_absent_catalog_sources` marking the deleted source retiring
        // before `close_removed_editor` runs while outgoing tails drain.
        deleted.retiring = true;
        // First verify an active transfer is not prematurely closed or cached.
        deleted.editor_transfer = true;
        plugins.sources.push(deleted);
        plugins.close_removed_editor(desired);
        assert!(plugins.editor_ops.is_empty());
        assert_eq!(plugins.editor_checked, None);

        // Once no transfer is pending, `close_removed_editor` must close the retiring source.
        plugins.sources[0].editor_transfer = false;
        assert_eq!(
            plugins
                .open_editor_devices(context.project_generation)
                .next(),
            Some(binding.instance)
        );
        plugins.close_removed_editor(desired);
        assert!(
            plugins
                .editor_ops
                .iter()
                .any(|op| op.binding == binding && op.action == EditorAction::Close),
            "close_removed_editor must queue Close for a retiring deleted source with an open editor"
        );
        plugins.finish_editor_action(
            binding,
            context.project_generation,
            EditorAction::Close,
            Ok(PluginEditorState::Closed),
            Some(context),
        );
        assert!(!plugins.sources[0].editor_open);
        assert_eq!(
            plugins
                .open_editor_devices(context.project_generation)
                .next(),
            None
        );
    }

    #[test]
    fn closed_and_reopened_editor_discards_stale_transport_space_and_accepts_fresh_press() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;

        let (mut helper_tx, parent_rx) = UnixStream::pair().expect("unix stream pair");
        parent_rx
            .set_nonblocking(true)
            .expect("nonblocking shortcut receiver");
        let rx = &parent_rx;
        let read_socket = |expected: Binding| {
            move |binding: Binding| {
                if binding != expected {
                    return false;
                }
                let mut byte = [0_u8; 1];
                (&*rx).read(&mut byte).ok() == Some(1) && byte == [1]
            }
        };

        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 4,
        };
        let binding = Binding {
            root: 1,
            instance: 7,
            graph: context.editor_revision,
            epoch: 1,
        };

        let mut plugins = Plugins {
            transport_space: true,
            ..Plugins::default()
        };
        let mut active = source(binding, context, true);
        active.accepted = true;
        plugins.sources.push(active);

        // 1. User presses Space while open, then closes the editor before desktop drains.
        helper_tx.write_all(&[1]).expect("queue pre-close space");
        plugins.finish_editor_action(
            binding,
            context.project_generation,
            EditorAction::Observe,
            Ok(PluginEditorState::Closed),
            Some(context),
        );
        assert!(!plugins.sources[0].editor_open);
        assert!(
            !plugins.take_transport_space_with(context.project_generation, read_socket(binding)),
            "closed editor must discard pre-close Space and emit zero toggles"
        );

        // 2. Reopening (both pending and confirmed open) without a new keypress emits zero toggles.
        plugins.queue_editor(binding, context.project_generation, EditorAction::Open);
        assert!(plugins.editor_holds(binding));
        assert!(
            !plugins.take_transport_space_with(context.project_generation, read_socket(binding)),
            "pending reopen must not replay earlier Space"
        );
        // Even a late byte arriving before Open completes is discarded rather than replayed.
        helper_tx.write_all(&[1, 1]).expect("queue pre-open bytes");
        assert!(
            !plugins.take_transport_space_with(context.project_generation, read_socket(binding)),
            "pending reopen must drain bytes queued while still closed"
        );
        plugins.finish_editor_action(
            binding,
            context.project_generation,
            EditorAction::Open,
            Ok(PluginEditorState::Open),
            Some(context),
        );
        assert!(plugins.sources[0].editor_open);
        assert!(
            !plugins.take_transport_space_with(context.project_generation, read_socket(binding)),
            "reopened editor without a new keypress must emit zero toggles"
        );

        // 3. A fresh Space press in the reopened editor toggles exactly once.
        helper_tx.write_all(&[1]).expect("queue fresh space");
        assert!(
            plugins.take_transport_space_with(context.project_generation, read_socket(binding)),
            "fresh Space press in reopened editor must toggle once"
        );
        assert!(
            !plugins.take_transport_space_with(context.project_generation, read_socket(binding)),
            "second poll without another press must not toggle again"
        );

        // 4. Previous project generation cannot toggle current transport and drains stale bytes.
        helper_tx.write_all(&[1]).expect("queue old-gen space");
        assert!(
            !plugins
                .take_transport_space_with(context.project_generation + 1, read_socket(binding)),
            "previous project generation must not toggle current transport"
        );

        // 5. Retiring device cannot toggle current transport and drains stale bytes.
        plugins.sources[0].retiring = true;
        helper_tx.write_all(&[1]).expect("queue retiring space");
        assert!(
            !plugins.take_transport_space_with(context.project_generation, read_socket(binding)),
            "retired device must not toggle current transport"
        );
    }

    #[test]
    fn visibility_probe_and_other_plugin_editor_ops_do_not_reject_toggle_editor_as_busy() {
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 4,
        };
        let binding_a = Binding {
            root: 1,
            instance: 10,
            graph: 4,
            epoch: 1,
        };
        let binding_b = Binding {
            root: 1,
            instance: 20,
            graph: 4,
            epoch: 2,
        };
        let mut plugins = Plugins::default();
        let mut src_a = source(binding_a, context, true);
        src_a.accepted = true;
        let mut src_b = source(binding_b, context, false);
        src_b.accepted = true;
        plugins.sources.extend([src_a, src_b]);

        // 1. Active background visibility probe on Plugin A must not block toggle_editor on Plugin B.
        plugins.queue_editor(binding_a, 1, EditorAction::Observe);
        assert!(plugins.editor_holds(binding_a));
        assert!(plugins.editor_visibility.len() == 1);
        plugins
            .toggle_editor(1, binding_b.instance)
            .expect("visibility probe on plugin A must not reject plugin B toggle");
        assert!(
            plugins
                .editor_ops
                .iter()
                .any(|op| op.binding == binding_b && op.action == EditorAction::Toggle)
        );

        // 2. Active Toggle on Plugin B must not block queueing a toggle on Plugin A while Observe finishes.
        plugins
            .toggle_editor(1, binding_a.instance)
            .expect("toggle on plugin A while Observe is in flight must queue cleanly");
        assert_eq!(plugins.queued_editor_toggles, vec![(binding_a, 1)]);

        // 3. A second Toggle on the exact same binding B is still rejected while B's Toggle is in flight.
        let err = plugins
            .toggle_editor(1, binding_b.instance)
            .expect_err("duplicate toggle on same binding must still be rejected");
        assert!(err.contains("vst3 editor is busy"), "unexpected err: {err}");

        // 4. Completing Observe on A releases A so queued Toggle can advance without affecting B.
        plugins.finish_editor_action(
            binding_a,
            1,
            EditorAction::Observe,
            Ok(PluginEditorState::Open),
            Some(context),
        );
        assert!(!plugins.editor_holds(binding_a));
        assert!(plugins.editor_holds(binding_b));
        plugins.finish_editor_action(
            binding_b,
            1,
            EditorAction::Toggle,
            Ok(PluginEditorState::Open),
            Some(context),
        );
        assert!(plugins.editor_ready());
        assert_eq!(plugins.take_editor_result(1), Some(Ok(true)));
        assert!(!plugins.editor_ready());
    }
}
