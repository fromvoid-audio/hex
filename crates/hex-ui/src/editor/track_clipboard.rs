//! Header/menu intents only. The worker owns copied metadata and admission;
//! ordinary receipts gate selection of authoritative track births after install.
use super::{
    EditorView,
    pending_edit::{Disposition, Receipt},
};
use hex_project::{Created, TrackId, TrackSelectionIds};
use hex_session::service::{Event, Request, TrackClipboardAction};

#[derive(Clone, Copy)]
pub(super) enum Shortcut {
    Copy,
    Cut,
    Paste,
    Duplicate,
}

pub(super) struct Pending {
    mode: super::EditorMode,
    generation: u64,
    revision: u64,
    epoch: Option<u64>,
    action: TrackClipboardAction,
    receipt: Receipt,
}
pub(super) struct Completion {
    clean: bool,
    pending: Pending,
    created: Option<Created>,
    revision: u64,
}
impl EditorView {
    pub(super) fn track_membership(&self, target: Option<TrackId>) -> Option<TrackSelectionIds> {
        TrackSelectionIds::new(&self.track_members(target)).ok()
    }

    pub(super) fn track_group_membership(
        &self,
        target: Option<TrackId>,
    ) -> Option<TrackSelectionIds> {
        let mut ids = self.track_members(target);
        // The active selected track supplies the group destination. A
        // deselected active track falls back to the last remaining row.
        if let Some(index) = ids.iter().position(|id| Some(*id) == self.selected_track) {
            ids[index..].rotate_left(1);
        }
        TrackSelectionIds::new(&ids).ok()
    }

    fn track_members(&self, target: Option<TrackId>) -> Vec<TrackId> {
        match target {
            Some(id) if !self.track_selection.contains(id) => vec![id],
            _ => self.selected_track_ids(),
        }
    }
    pub(super) fn track_paste_anchor(&self) -> Option<TrackId> {
        let state = self.state.as_ref()?;
        let ids = self.selected_track_ids();
        let last = state
            .project
            .tracks()
            .iter()
            .rposition(|t| ids.contains(&t.id()))?;
        state.project.tracks().get(last + 1).map(|t| t.id())
    }
    pub(super) fn start_track_clipboard(
        &mut self,
        generation: u64,
        revision: u64,
        action: TrackClipboardAction,
    ) {
        if self.edit_pending()
            || self.pending_track_clipboard.is_some()
            || self.track_name.is_editing()
            || self.clip_name_edit.is_some()
            || self.track_reorder_active()
            || self.track_resize_active()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.time_selection_gesture.is_some()
        {
            self.error = Some("finish the current edit before copying tracks".into());
            return;
        }
        if generation != self.project_generation
            || self.state.as_ref().is_none_or(|s| s.revision != revision)
        {
            self.error = Some("track clipboard canceled · project changed".into());
            return;
        }
        self.pending_track_clipboard = Some(Pending {
            mode: self.mode,
            generation,
            revision,
            epoch: self.selection_intent_epoch,
            action: action.clone(),
            receipt: Receipt::new(generation),
        });
        if !self.enqueue(Request::TrackClipboard {
            project_generation: generation,
            revision,
            action,
        }) {
            self.pending_track_clipboard = None;
        }
    }
    pub(super) fn track_clipboard_shortcut(
        &mut self,
        window: &gpui::Window,
        kind: Shortcut,
    ) -> bool {
        let retained_context = self.mode == super::EditorMode::Arrangement
            && self.focus.is_focused(window)
            && self.selection_intent_epoch.is_some_and(|epoch| {
                self.track_clipboard_context == Some((self.project_generation, epoch))
            });
        if !self.track_header_focused(window) && !retained_context {
            return false;
        }
        let Some(state) = &self.state else {
            return true;
        };
        let revision = state.revision;
        let tracks = self.track_membership(None);
        let action = match kind {
            Shortcut::Copy => tracks.map(TrackClipboardAction::Copy),
            Shortcut::Cut => tracks.map(TrackClipboardAction::Cut),
            Shortcut::Paste => Some(TrackClipboardAction::Paste {
                before: self.track_paste_anchor(),
            }),
            Shortcut::Duplicate => tracks.map(|tracks| TrackClipboardAction::Duplicate {
                tracks,
                without_clips: false,
            }),
        };
        let Some(action) = action else {
            return true;
        };
        self.start_track_clipboard(self.project_generation, revision, action);
        true
    }
    pub(super) fn pending_track_clipboard_matches(&self, request: &Request) -> bool {
        self.pending_track_clipboard.as_ref().is_some_and(|p| {
            matches!(request, Request::TrackClipboard { project_generation, revision, action }
                if p.receipt.is_queued() && *project_generation == p.generation && *revision == p.revision && *action == p.action)
        })
    }
    pub(super) fn track_clipboard_submitted(&mut self) {
        if let Some(p) = &mut self.pending_track_clipboard {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn retire_abandoned_track_clipboard(&mut self) -> bool {
        if self
            .pending_track_clipboard
            .as_ref()
            .is_some_and(|p| p.receipt.is_queued())
            && !self
                .pending_requests
                .iter()
                .chain(self.device_gesture_barriers.iter())
                .any(|r| self.pending_track_clipboard_matches(r))
        {
            return self.pending_track_clipboard.take().is_some();
        }
        false
    }
    pub(super) fn take_track_clipboard_completion(
        &mut self,
        event: &mut Event,
    ) -> Option<Completion> {
        let disposition = self.pending_track_clipboard.as_mut()?.receipt.observe(
            self.project_generation,
            event.project_replaced,
            event.request_complete,
        );
        match disposition {
            Disposition::Waiting => None,
            Disposition::Invalidated => {
                self.pending_track_clipboard = None;
                None
            }
            Disposition::Terminal => Some(Completion {
                clean: event.error.is_none(),
                pending: self.pending_track_clipboard.take()?,
                created: event.created.take(),
                revision: event.state.revision,
            }),
        }
    }
    pub(super) fn complete_track_clipboard(&mut self, completion: Completion) {
        let p = completion.pending;
        if self.mode != p.mode
            || self.opening_project
            || self.close_prompt_open
            || self.recovery.generation().is_some()
            || p.generation != self.project_generation
            || p.epoch.is_none()
            || p.epoch != self.selection_intent_epoch
        {
            return;
        }
        let copied = matches!(p.action, TrackClipboardAction::Copy(_))
            && completion.clean
            && p.revision == completion.revision;
        if !copied && p.revision.checked_add(1) != Some(completion.revision) {
            return;
        }
        // Keep the track input scope when canonical removal retires its header.
        // This is not clipboard metadata: any subsequent real selection (even
        // an ABA selection) or document replacement invalidates the epoch stamp.
        if copied
            || matches!(completion.created, Some(Created::Tracks { .. }))
            || (matches!(completion.created, Some(Created::None))
                && matches!(p.action, TrackClipboardAction::Cut(_)))
        {
            self.track_clipboard_context = p.epoch.map(|epoch| (p.generation, epoch));
        }
        let Some(Created::Tracks { first, last }) = completion.created else {
            return;
        };
        let Some(state) = &self.state else {
            return;
        };
        let ids: Vec<_> = state
            .project
            .tracks()
            .iter()
            .map(|t| t.id())
            .filter(|id| *id >= first && *id <= last)
            .collect();
        if ids.is_empty() {
            return;
        }
        self.track_selection.replace(&ids);
        self.selected_track = ids.first().copied();
        self.selected_clip = None;
        self.selected_cell = None;
        self.selected_scene = None;
        self.focused_return = None;
        self.device_panel.main_selected = None;
        self.midi_selection.clear();
        self.midi_note_gesture = None;
        self.midi_group_gesture = None;
        self.update_track_selection_status();
    }
}
