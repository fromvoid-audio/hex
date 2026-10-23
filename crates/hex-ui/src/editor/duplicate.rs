//! One whole-clip keyboard duplication chain, owned through its exact terminal.
//!
//! Repeated presses retain bounded intentions, not copies of media or requests.
//! Selection changes cancel repeats/selection effects without abandoning admitted
//! work. Every ordinary terminal counts, including refusals before edit dispatch.
use super::{
    EditorMode, EditorView,
    pending_edit::{Disposition, Receipt},
};
use hex_project::{ClipId, Created};
use hex_session::service::{Event, Request};

const MAX_QUEUED_REPEATS: u8 = 32;

#[derive(Default)]
pub(super) struct DuplicateIntent {
    pending: Option<Pending>,
    queued_repeats: u8,
}

struct Pending {
    generation: u64,
    revision: u64,
    clips: Vec<ClipId>,
    primary: Option<ClipId>,
    selection_epoch: u64,
    active: bool,
    receipt: Receipt,
}

pub(super) struct Completion {
    pending: Pending,
    created: Option<Created>,
    committed_revision: bool,
    clean: bool,
    repeats: u8,
}

impl DuplicateIntent {
    fn queue_repeat(&mut self) -> Result<(), &'static str> {
        if !self.pending.as_ref().is_some_and(|p| p.active) {
            return Err("duplicate is still completing");
        }
        if self.queued_repeats >= MAX_QUEUED_REPEATS {
            return Err("duplicate queue full · maximum 32 queued repeats");
        }
        self.queued_repeats += 1;
        Ok(())
    }

    pub(super) fn cancel_chain(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.active = false;
        }
        self.queued_repeats = 0;
    }

    pub(super) fn reject(&mut self) -> bool {
        self.queued_repeats = 0;
        self.pending.take().is_some()
    }

    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

impl EditorView {
    fn duplicate_selection_current(&self, pending: &Pending) -> bool {
        pending.active
            && pending.generation == self.project_generation
            && self.selection_intent_epoch == Some(pending.selection_epoch)
            && self.selected_clip == pending.primary
            && self.selected_clip_ids() == pending.clips
            && self.mode == EditorMode::Arrangement
            && !self.opening_project
            && self.recovery.generation().is_none()
    }

    pub(super) fn duplicate_selected(&mut self) {
        // A modal sheet pauses new input, while repeats accepted before it
        // opened retain their existing continuation admission policy.
        if self.close_prompt_open {
            self.error = Some("finish the close action before editing".into());
            return;
        }
        if let Some(pending) = &self.duplicate_intent.pending {
            if !self.duplicate_selection_current(pending) {
                self.duplicate_intent.cancel_chain();
            }
            if let Err(reason) = self.duplicate_intent.queue_repeat() {
                self.error = Some(reason.into());
            }
            return;
        }
        let clips = self.selected_clip_ids();
        if clips.is_empty() {
            return;
        }
        if self.edit_pending() {
            self.error = Some("applying edit · wait before duplicating clips".into());
            return;
        }
        self.start_clip_duplicate(clips, 0, false);
    }

    fn start_clip_duplicate(&mut self, clips: Vec<ClipId>, repeats: u8, continuation: bool) {
        let Some(state) = &self.state else {
            return;
        };
        let Some(selection_epoch) = self.selection_intent_epoch else {
            self.error = Some(
                "selection intent exhausted · reopen the editor before duplicating clips".into(),
            );
            return;
        };
        let revision = state.revision;
        let request = Request::DuplicateClips {
            project_generation: self.project_generation,
            revision,
            clips: clips.clone(),
        };
        self.duplicate_intent.pending = Some(Pending {
            generation: self.project_generation,
            revision,
            clips,
            primary: self.selected_clip,
            selection_epoch,
            active: true,
            receipt: Receipt::new(self.project_generation),
        });
        self.duplicate_intent.queued_repeats = repeats;
        let accepted = if continuation {
            self.enqueue_duplicate_continuation(request)
        } else {
            self.enqueue(request)
        };
        if !accepted {
            self.duplicate_intent.reject();
        }
    }

    pub(super) fn pending_duplicate_matches(&self, request: &Request) -> bool {
        let Some(pending) = &self.duplicate_intent.pending else {
            return false;
        };
        matches!(request, Request::DuplicateClips { project_generation, revision, clips }
            if pending.receipt.is_queued() && *project_generation == pending.generation
                && *revision == pending.revision && *clips == pending.clips)
    }

    pub(super) fn retire_abandoned_duplicate(&mut self) -> bool {
        let queued = self
            .duplicate_intent
            .pending
            .as_ref()
            .is_some_and(|pending| pending.receipt.is_queued());
        if queued
            && !self
                .pending_requests
                .iter()
                .chain(self.device_gesture_barriers.iter())
                .any(|request| self.pending_duplicate_matches(request))
        {
            return self.duplicate_intent.reject();
        }
        false
    }

    pub(super) fn duplicate_submitted(&mut self) {
        if let Some(pending) = &mut self.duplicate_intent.pending {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }

    pub(super) fn take_duplicate_completion(&mut self, event: &mut Event) -> Option<Completion> {
        let disposition = self.duplicate_intent.pending.as_mut()?.receipt.observe(
            self.project_generation,
            event.project_replaced,
            event.request_complete,
        );
        match disposition {
            Disposition::Waiting => None,
            Disposition::Invalidated => {
                self.duplicate_intent.reject();
                None
            }
            Disposition::Terminal => {
                let pending = self.duplicate_intent.pending.take()?;
                let repeats = std::mem::take(&mut self.duplicate_intent.queued_repeats);
                let committed_revision =
                    pending.revision.checked_add(1) == Some(event.state.revision);
                Some(Completion {
                    pending,
                    repeats,
                    committed_revision,
                    clean: event.error.is_none(),
                    created: event.created.take(),
                })
            }
        }
    }

    pub(super) fn complete_clip_duplicate(&mut self, completion: Completion) {
        if !completion.committed_revision || !self.duplicate_selection_current(&completion.pending)
        {
            return;
        }
        let count = completion.pending.clips.len();
        let range = match completion.created {
            Some(Created::Clips { first, last }) => Some((first, last)),
            Some(Created::Clip(id)) if count == 1 => Some((id, id)),
            _ => None,
        };
        let selected =
            range.is_some_and(|(first, last)| self.select_copied_group(first, last, count));
        if !selected {
            self.error
                .get_or_insert_with(|| "copied clips could not be resolved".into());
            return;
        }
        if completion.clean && completion.repeats > 0 {
            self.start_clip_duplicate(self.selected_clip_ids(), completion.repeats - 1, true);
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "duplicate_ui_tests.rs"]
mod ui_tests;
