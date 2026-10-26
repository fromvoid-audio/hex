//! A delivered take stays owned until its exact submitted edit terminal and
//! canonical installation arrive together. Refusal never consumes its events.
use super::*;
use crate::editor::pending_edit::{Disposition, Receipt as EditReceipt};
use hex_session::service::{Event, GainLaneEdit, ViewState};
use std::sync::Arc;

pub(super) struct SealedTake {
    target: Target,
    end: Option<GainTakeEnd>,
    proposal: Option<Proposal>,
    pending: Option<EditReceipt>,
    accepted_state: Option<Arc<ViewState>>,
    outcome_unknown: bool,
}
struct Proposal {
    spans: Vec<hex_project::GainTouchSpan>,
    lane: hex_project::GainLane,
}
impl SealedTake {
    fn matches(&self, request: &Request) -> bool {
        let Some(proposal) = &self.proposal else {
            return false;
        };
        self.pending.as_ref().is_some_and(EditReceipt::is_queued)
            && matches!(request, Request::GainLane { project_generation, revision, track, edit: GainLaneEdit::SpliceTake { expected_epoch, spans } }
                if *project_generation == self.target.project_generation
                && *revision == self.target.canonical_revision
                && track.value() == self.target.track_id
                && Some(*expected_epoch) == self.target.recording_epoch.checked_sub(1)
                && *spans == proposal.spans)
    }
    pub(super) fn complete(&self) -> bool {
        self.end.is_some()
    }
    pub(super) fn awaiting_commit(&self) -> bool {
        self.pending.is_some() || self.accepted_state.is_some()
    }
}
impl Recorder {
    pub(in crate::editor) fn has_retained_take(&self) -> bool {
        self.sealed.is_some() || self.retired.is_some()
    }
    /// Lifecycle loss can retire capture/bindings, never a sealed delivered take.
    pub(in crate::editor) fn audio_lost(&mut self) {
        self.pending_gain = None;
        self.binding = None;
        if self.retired.is_some() {
            self.mode = Mode::Retained;
        } else if self.sealed.is_some() {
            self.target = None;
        } else if let Some(target) = self.target.take() {
            if self.cancelled {
                self.discard();
                return;
            }
            // Delivered events without an actual Finished have no truthful end
            // clock. Keep the draft, but never offer a canonical commit for it.
            self.mode = Mode::Retained;
            self.sealed = Some(SealedTake {
                target,
                end: None,
                proposal: None,
                pending: None,
                accepted_state: None,
                outcome_unknown: false,
            });
        } else {
            self.discard();
        }
    }
}
impl EditorView {
    pub(super) fn seal_gain_take(&mut self, target: Target, end: GainTakeEnd) {
        if self.gain_recorder.events.is_empty() {
            // A touch wholly superseded before rendering has no delivered work.
            self.gain_recorder.discard();
            self.status = "empty gain touch · project unchanged".into();
            return;
        }
        self.gain_recorder.target = None;
        self.gain_recorder.pending_gain = None;
        self.gain_recorder.binding = None;
        self.gain_recorder.mode = Mode::Retained;
        self.gain_recorder.sealed = Some(SealedTake {
            target,
            end: Some(end),
            proposal: None,
            pending: None,
            accepted_state: None,
            outcome_unknown: false,
        });
        self.retry_gain_take();
    }
    pub(super) fn gain_take_retry_available(&self) -> bool {
        self.gain_recorder.sealed.as_ref().is_some_and(|take| {
            take.complete()
                && !take.awaiting_commit()
                && !take.outcome_unknown
                && self.project_generation == take.target.project_generation
                && self.state.as_ref().is_some_and(|state| {
                    state.revision == take.target.canonical_revision
                        && state
                            .project
                            .tracks()
                            .iter()
                            .find(|track| track.id().value() == take.target.track_id)
                            .is_some_and(|track| {
                                Some(track.gain_lane().recording_epoch())
                                    == take.target.recording_epoch.checked_sub(1)
                            })
                })
        })
    }
    pub(super) fn retry_gain_take(&mut self) {
        let Some(take) = self.gain_recorder.sealed.as_ref() else {
            return;
        };
        let Some(end) = take.end else {
            self.error = Some(
                "incomplete gain take retained · no delivered end clock · discard explicitly"
                    .into(),
            );
            return;
        };
        if take.outcome_unknown {
            self.error =
                Some("gain commit outcome unknown · take retained · discard explicitly".into());
            return;
        }
        if take.awaiting_commit() {
            self.error = Some("gain take is applying · wait for its acknowledgment".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if self.project_generation != take.target.project_generation
            || state.revision != take.target.canonical_revision
        {
            self.error =
                Some("gain take retained · recorded project changed · discard explicitly".into());
            return;
        }
        let Some(track) = self
            .gain_recorder
            .track
            .filter(|track| track.value() == take.target.track_id)
        else {
            return;
        };
        let Some(expected_epoch) = take.target.recording_epoch.checked_sub(1) else {
            self.error = Some("gain take retained · invalid recording epoch".into());
            return;
        };
        let context = hex_project::GainTouchContext {
            recording_epoch: take.target.recording_epoch,
            project_generation: take.target.project_generation,
            canonical_revision: take.target.canonical_revision,
            audio_revision: take.target.audio_revision,
            transport_epoch: take.target.transport_epoch,
            gesture_epoch: take.target.transport_epoch,
            gesture_serial: take.target.gesture_serial,
            sample_rate: take.target.sample_rate,
            loop_pass: self
                .gain_recorder
                .events
                .first()
                .map_or(end.loop_pass, |event| event.loop_pass),
        };
        let spans = match hex_session::gain_recording::finish_take(
            &state.project,
            track,
            expected_epoch,
            context,
            &self.gain_recorder.events,
            end,
        ) {
            Ok(spans) if !spans.is_empty() => spans,
            Ok(_) => {
                self.error = Some("empty gain take retained · discard explicitly".into());
                return;
            }
            Err(reason) => {
                self.error = Some(format!("gain take retained · {reason}").into());
                return;
            }
        };
        // Build the exact expected lane from the immutable source. Equality is
        // only an installation check AFTER our typed request's exact terminal.
        let Some(owner) = state.project.track(track) else {
            return;
        };
        let mut lane = owner.gain_lane().clone();
        let mut epoch = expected_epoch;
        for span in &spans {
            let next = span.context.recording_epoch;
            match lane.with_touch(epoch, span.clone()) {
                Ok(next_lane) => lane = next_lane,
                Err(reason) => {
                    self.error = Some(format!("gain take retained · {reason}").into());
                    return;
                }
            }
            epoch = next;
        }
        let request = Request::GainLane {
            project_generation: take.target.project_generation,
            revision: take.target.canonical_revision,
            track,
            edit: GainLaneEdit::SpliceTake {
                expected_epoch,
                spans: spans.clone(),
            },
        };
        let generation = take.target.project_generation;
        let Some(take) = self.gain_recorder.sealed.as_mut() else {
            return;
        };
        take.proposal = Some(Proposal { spans, lane });
        take.pending = Some(EditReceipt::new(generation));
        self.gain_recorder.mode = Mode::Committing;
        if !self.enqueue(request) {
            if let Some(take) = self.gain_recorder.sealed.as_mut() {
                take.pending = None;
            }
            self.gain_recorder.mode = Mode::Retained;
            self.status = "gain take retained · retry or discard".into();
        }
    }
    pub(in crate::editor) fn pending_gain_take_matches(&self, request: &Request) -> bool {
        self.gain_recorder
            .sealed
            .as_ref()
            .is_some_and(|take| take.matches(request))
    }
    pub(in crate::editor) fn gain_take_submitted(&mut self) {
        if let Some(receipt) = self
            .gain_recorder
            .sealed
            .as_mut()
            .and_then(|take| take.pending.as_mut())
        {
            receipt.submitted(self.worker_requests_pending);
        }
    }
    pub(in crate::editor) fn gain_take_not_submitted(&mut self, request: &Request) {
        if self.pending_gain_take_matches(request) {
            if let Some(take) = self.gain_recorder.sealed.as_mut() {
                take.pending = None;
            }
            self.gain_recorder.mode = Mode::Retained;
            self.status = "gain take retained · retry or discard".into();
        }
    }
    pub(in crate::editor) fn gain_take_service_disconnected(&mut self) {
        if let Some(take) = self.gain_recorder.sealed.as_mut() {
            if let Some(receipt) = take.pending.take() {
                take.outcome_unknown = !receipt.is_queued();
            }
            take.accepted_state = None;
            self.gain_recorder.mode = Mode::Retained;
            self.status = "project worker stopped · gain take retained".into();
        }
    }
    pub(in crate::editor) fn gain_take_request_complete(&mut self, event: &Event) {
        let Some(take) = self.gain_recorder.sealed.as_mut() else {
            return;
        };
        let Some(receipt) = take.pending.as_mut() else {
            return;
        };
        // Data custody survives a source change. Do not use display-preview
        // invalidation to forget an admitted edit or restamp its retry.
        if receipt.observe(
            take.target.project_generation,
            false,
            event.request_complete,
        ) != Disposition::Terminal
        {
            return;
        }
        take.pending = None;
        if event.error.is_none()
            && !event.project_replaced
            && self.project_generation == take.target.project_generation
            && Some(event.state.revision) == take.target.canonical_revision.checked_add(1)
            && event
                .state
                .project
                .tracks()
                .iter()
                .find(|track| track.id().value() == take.target.track_id)
                .zip(take.proposal.as_ref())
                .is_some_and(|(track, proposal)| track.gain_lane() == &proposal.lane)
        {
            take.accepted_state = Some(Arc::clone(&event.state));
        } else {
            self.gain_recorder.mode = Mode::Retained;
            self.status = "gain take retained · retry or discard".into();
        }
    }
    pub(in crate::editor) fn gain_take_state_installed(&mut self) {
        let committed = self.gain_recorder.sealed.as_ref().is_some_and(|take| {
            self.project_generation == take.target.project_generation
                && take
                    .accepted_state
                    .as_ref()
                    .zip(self.state.as_ref())
                    .is_some_and(|(accepted, installed)| Arc::ptr_eq(accepted, installed))
        });
        if committed {
            self.gain_recorder.discard();
            self.status = "gain take committed".into();
        }
    }
    pub(super) fn discard_gain_take(&mut self) {
        if self
            .gain_recorder
            .sealed
            .as_ref()
            .is_some_and(SealedTake::awaiting_commit)
        {
            self.error =
                Some("gain take is applying · discarding cannot cancel an admitted edit".into());
            return;
        }
        if self.gain_recorder.has_retained_take() {
            self.gain_recorder.discard();
            self.status = "retained gain take discarded".into();
        }
    }
}
