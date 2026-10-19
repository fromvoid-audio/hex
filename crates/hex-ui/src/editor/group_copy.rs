//! One accepted group-copy completion; actual Service FIFO owns its terminal.
use super::{EditorMode, EditorView};
use hex_project::{ClipGroupPosition, ClipId, Command, TrackId};
use hex_session::service::{Event, Request};

pub(super) struct Pending {
    generation: u64,
    revision: u64,
    epoch: u64,
    primary: Option<ClipId>,
    anchor: ClipId,
    track: TrackId,
    position: ClipGroupPosition,
    pub(super) count: usize,
    remaining: Option<usize>,
}
impl Pending {
    pub(super) fn capture(view: &EditorView, command: &Command) -> Option<Self> {
        let Command::MoveClipsWithOverwrite {
            clips,
            anchor,
            track,
            position,
            copy: true,
        } = command
        else {
            return None;
        };
        Some(Self {
            generation: view.project_generation,
            revision: view.state.as_ref()?.revision,
            epoch: view.clip_selection.epoch,
            primary: view.selected_clip,
            anchor: *anchor,
            track: *track,
            position: *position,
            count: clips.len(),
            remaining: None,
        })
    }
    pub(super) fn submitted(&mut self, remaining: usize) {
        self.remaining = Some(remaining);
    }
    fn matches(&self, request: &Request) -> bool {
        matches!(request, Request::Edit(Command::MoveClipsWithOverwrite { clips, anchor, track, position, copy: true })
            if self.remaining.is_none() && *anchor == self.anchor && *track == self.track
            && *position == self.position && clips.len() == self.count)
    }
    pub(super) fn is_current(&self, view: &EditorView) -> bool {
        self.generation == view.project_generation
            && self.epoch == view.clip_selection.epoch
            && self.primary == view.selected_clip
            && view
                .state
                .as_ref()
                .is_some_and(|s| self.revision.checked_add(1) == Some(s.revision))
            && view.mode == EditorMode::Arrangement
            && !view.opening_project
            && !view.close_prompt_open
            && view.recovery.generation().is_none()
    }
}
impl EditorView {
    pub(super) fn retire_abandoned_group_copy(&mut self) -> bool {
        let abandoned = self.pending_group_copy.as_ref().is_some_and(|p| {
            p.generation != self.project_generation
                || p.epoch != self.clip_selection.epoch
                || p.primary != self.selected_clip
                || self.opening_project
                || self.close_prompt_open
                || self.mode != EditorMode::Arrangement
                || self.recovery.generation().is_some()
                || (p.remaining.is_none()
                    && !self.pending_requests.iter().any(|r| p.matches(r))
                    && !self.device_gesture_barriers.iter().any(|r| p.matches(r)))
        });
        if abandoned {
            self.pending_group_copy = None;
        }
        abandoned
    }
    pub(super) fn pending_group_copy_matches(&self, request: &Request) -> bool {
        self.pending_group_copy
            .as_ref()
            .is_some_and(|p| p.generation == self.project_generation && p.matches(request))
    }
    pub(super) fn group_copy_submitted(&mut self) {
        if let Some(pending) = &mut self.pending_group_copy {
            pending.submitted(self.worker_requests_pending);
        }
    }
    pub(super) fn group_copy_request_complete(&mut self, event: &Event) -> Option<Pending> {
        let pending = self.pending_group_copy.as_mut()?;
        if event.project_replaced || pending.generation != self.project_generation {
            self.pending_group_copy = None;
            return None;
        }
        if event.request_complete
            && let Some(remaining) = &mut pending.remaining
        {
            *remaining = remaining.saturating_sub(1);
            if *remaining == 0 {
                return self.pending_group_copy.take();
            }
        }
        None
    }
}
