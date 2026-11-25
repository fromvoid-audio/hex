//! Released gain-point display follows the shared ordered edit receipt. Escape
//! cancels held input only; an admitted edit keeps its final marker until its
//! own terminal and canonical state arrive together in EditorView::receive.
use super::*;
use crate::editor::pending_edit::{Disposition, Receipt};
use hex_session::service::Event;

pub(in crate::editor) struct PendingGain {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) track: TrackId,
    pub(super) draft: Option<Draft>,
    pub(super) group_points: Option<Vec<GainPoint>>,
    pub(super) edit: GainLaneEdit,
    receipt: Receipt,
}
impl PendingGain {
    pub(super) fn new(draft: Draft, edit: GainLaneEdit) -> Self {
        Self {
            generation: draft.generation,
            revision: draft.revision,
            track: draft.track,
            draft: Some(draft),
            group_points: None,
            edit,
            receipt: Receipt::new(draft.generation),
        }
    }
    pub(super) fn new_group(
        generation: u64,
        revision: u64,
        track: TrackId,
        points: Vec<GainPoint>,
        edit: GainLaneEdit,
    ) -> Self {
        Self {
            generation,
            revision,
            track,
            draft: None,
            group_points: Some(points),
            edit,
            receipt: Receipt::new(generation),
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
        *project_generation == self.generation
            && *revision == self.revision
            && *track == self.track
            && match (&self.edit, edit) {
                (GainLaneEdit::Upsert(a), GainLaneEdit::Upsert(b)) => a == b,
                (GainLaneEdit::Move { from: a, to: b }, GainLaneEdit::Move { from: c, to: d }) => {
                    (a, b) == (c, d)
                }
                (GainLaneEdit::Replace(a), GainLaneEdit::Replace(b)) => a == b,
                (GainLaneEdit::Clear, GainLaneEdit::Clear) => true,
                _ => false,
            }
    }
}
impl GainLane {
    pub(super) fn display_draft(&self, track: TrackId) -> Option<Draft> {
        self.draft.filter(|draft| draft.track == track).or_else(|| {
            self.pending
                .as_ref()
                .and_then(|p| p.draft)
                .filter(|draft| draft.track == track)
        })
    }

    pub(super) fn display_group_points(&self, track: TrackId) -> Option<&[GainPoint]> {
        self.group_draft
            .as_ref()
            .filter(|draft| draft.track == track)
            .map(|draft| draft.preview.as_slice())
            .or_else(|| {
                self.pending
                    .as_ref()
                    .filter(|pending| pending.track == track)
                    .and_then(|pending| pending.group_points.as_deref())
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
