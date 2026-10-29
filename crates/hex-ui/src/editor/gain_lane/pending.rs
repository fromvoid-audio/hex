//! Released gain-point display follows the shared ordered edit receipt. Escape
//! cancels held input only; an admitted edit keeps its final marker until its
//! own terminal and canonical state arrive together in EditorView::receive.
use super::*;
use crate::editor::pending_edit::{Disposition, Receipt};
use hex_session::service::Event;

pub(in crate::editor) struct PendingGain {
    pub(super) draft: Draft,
    pub(super) edit: GainLaneEdit,
    receipt: Receipt,
}
impl PendingGain {
    pub(super) fn new(draft: Draft, edit: GainLaneEdit) -> Self {
        Self {
            draft,
            edit,
            receipt: Receipt::new(draft.generation),
        }
    }
    fn matches(&self, request: &Request) -> bool {
        let Request::GainLane {
            project_generation,
            revision,
            track,
            edit,
        } = request
        else {
            return false;
        };
        *project_generation == self.draft.generation
            && *revision == self.draft.revision
            && *track == self.draft.track
            && match (&self.edit, edit) {
                (GainLaneEdit::Upsert(a), GainLaneEdit::Upsert(b)) => a == b,
                (GainLaneEdit::Move { from: a, to: b }, GainLaneEdit::Move { from: c, to: d }) => {
                    (a, b) == (c, d)
                }
                (GainLaneEdit::Replace(a), GainLaneEdit::Replace(b)) => a == b,
                _ => false,
            }
    }
}
impl GainLane {
    pub(super) fn display_draft(&self, track: TrackId) -> Option<Draft> {
        self.draft.filter(|draft| draft.track == track).or_else(|| {
            self.pending
                .as_ref()
                .map(|p| p.draft)
                .filter(|draft| draft.track == track)
        })
    }
}
impl EditorView {
    pub(in crate::editor) fn pending_gain_matches(&self, request: &Request) -> bool {
        self.gain_lane
            .pending
            .as_ref()
            .is_some_and(|pending| pending.matches(request))
    }
    pub(in crate::editor) fn gain_submitted(&mut self) {
        if let Some(p) = &mut self.gain_lane.pending {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(in crate::editor) fn retire_pending_gain(&mut self) -> bool {
        self.gain_lane.pending.take().is_some()
    }
    pub(in crate::editor) fn gain_request_complete(&mut self, event: &Event) {
        if let Some(p) = &mut self.gain_lane.pending
            && p.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        {
            self.retire_pending_gain();
        }
    }
}
