//! Frozen clip-local event geometry crosses release through the ordinary FIFO
//! receipt. Inspector changes may hide it, but never relinquish admitted custody.
use super::*;
use crate::editor::pending_edit::{Disposition, Receipt};
use hex_project::{AudioEvents, Command};
use hex_session::service::{Event, Request};

pub(super) struct Pending {
    pub(super) owner: (ClipId, u64),
    pub(super) operation: AudioEventEdit,
    pub(super) preview: AudioEvents,
    pub(super) placement: Option<std::sync::Arc<placement::Preview>>,
    receipt: Receipt,
    intent: Option<u64>,
    before: Selection,
    pub(super) clipboard: Option<Clipboard>,
    pub(super) waveform: Option<waveform_preview::Preview>,
}
#[derive(Clone, Copy)]
struct Selection {
    cursor: u64,
    selected: Option<u64>,
    range: Option<(u64, u64)>,
}
impl EditorView {
    pub(in crate::editor) fn event_commit(&mut self, operation: AudioEventEdit) {
        if self.edit_pending()
            || self.audio_event_editor.pending.is_some()
            || !self.event_context_current()
        {
            return;
        }
        let Some((clip, root, _)) = self.event_context() else {
            return;
        };
        let Some(source) = self.state.as_ref().and_then(|s| s.project.clip(clip)) else {
            return;
        };
        let placement = self.placement_preview(&operation);
        let preview = if let Some(placement) = &placement {
            placement.clip().audio_events()
        } else {
            match source
                .editable_audio_events()
                .and_then(|events| events.edited(operation.clone(), root.domain.clock.length()))
            {
                Ok(events) => events,
                Err(error) => {
                    self.error = Some(error.0.into());
                    return;
                }
            }
        };
        self.advance_selection_intent();
        let before = Selection {
            cursor: self.audio_event_editor.cursor,
            selected: self.audio_event_editor.selected,
            range: self.audio_event_editor.range,
        };
        let destination = match &operation {
            AudioEventEdit::Paste { at, length, .. }
            | AudioEventEdit::Insert { at, length, .. } => {
                at.checked_add(*length).map(|end| (*at, end))
            }
            AudioEventEdit::MoveRange {
                start,
                end,
                destination,
            } => end
                .checked_sub(*start)
                .and_then(|length| destination.checked_add(length))
                .map(|end| (*destination, end)),
            _ => None,
        };
        if let Some((start, end)) = destination {
            self.audio_event_editor.range = Some((start, end));
            self.audio_event_editor.cursor = start;
            self.audio_event_editor.selected = None;
            self.audio_event_editor.retained_selection = None;
        } else if let AudioEventEdit::Delete { start, .. } = &operation {
            self.audio_event_editor.range = None;
            self.audio_event_editor.selected = None;
            self.audio_event_editor.retained_selection = None;
            self.audio_event_editor.cursor = *start;
        }
        self.audio_event_editor.gesture = None;
        self.audio_event_editor.pending = Some(Pending {
            owner: (clip, self.project_generation),
            operation: operation.clone(),
            preview,
            placement,
            receipt: Receipt::new(self.project_generation),
            intent: self.selection_intent_epoch,
            before,
            clipboard: None,
            waveform: None,
        });
        if !self.enqueue(Request::Edit(Command::EditAudioEvents { clip, operation })) {
            self.retire_pending_audio_events();
        }
    }
    pub(in crate::editor) fn pending_audio_events_matches(&self, request: &Request) -> bool {
        let Some(p) = self
            .audio_event_editor
            .pending
            .as_ref()
            .filter(|p| p.receipt.is_queued())
        else {
            return false;
        };
        matches!(request, Request::Edit(Command::EditAudioEvents { clip, operation })
            if *clip == p.owner.0 && *operation == p.operation)
    }
    pub(in crate::editor) fn audio_events_submitted(&mut self) {
        if let Some(p) = &mut self.audio_event_editor.pending {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(in crate::editor) fn retire_pending_audio_events(&mut self) -> bool {
        let Some(p) = self.audio_event_editor.pending.take() else {
            return false;
        };
        if self.audio_event_editor.owner == Some(p.owner)
            && p.intent.is_some()
            && p.intent == self.selection_intent_epoch
        {
            self.audio_event_editor.cursor = p.before.cursor;
            self.audio_event_editor.selected = p.before.selected;
            self.audio_event_editor.range = p.before.range;
        }
        true
    }
    pub(in crate::editor) fn audio_events_request_complete(&mut self, event: &Event) {
        let Some(p) = &mut self.audio_event_editor.pending else {
            return;
        };
        match p.receipt.observe(
            self.project_generation,
            event.project_replaced,
            event.request_complete,
        ) {
            Disposition::Waiting => {}
            Disposition::Terminal if event.error.is_some() => {
                self.retire_pending_audio_events();
            }
            Disposition::Terminal => {
                if let Some(clipboard) = self
                    .audio_event_editor
                    .pending
                    .take()
                    .and_then(|p| p.clipboard)
                {
                    self.audio_event_editor.clipboard = Some(clipboard);
                }
            }
            Disposition::Invalidated => {
                self.audio_event_editor.pending = None;
            }
        }
    }
}
