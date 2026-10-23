//! Release-to-ACK display custody for an existing-track move or copy. Service FIFO
//! completion ordinals are the same contract used by trim and group-copy intents.
//! This owns metadata paint only; the worker validates and commits the real edit.
use super::pending_edit::{Disposition, Receipt};
use super::{EditorView, drag::ClipDrag};
use hex_project::{Command, Project};
use hex_session::service::{Event, Request};
use std::sync::Arc;

pub(super) struct Pending {
    pub(super) drag: ClipDrag,
    command: Command,
    receipt: Receipt,
    source: Arc<Project>,
}

impl Pending {
    pub(super) fn new(drag: ClipDrag, command: Command, source: Arc<Project>) -> Self {
        let receipt = Receipt::new(drag.project_generation);
        Self {
            drag,
            command,
            receipt,
            source,
        }
    }

    fn matches(&self, request: &Request) -> bool {
        if !self.receipt.is_queued() {
            return false;
        }
        match (&self.command, request) {
            (
                Command::MoveClipWithOverwrite { clip, track, start },
                Request::Edit(Command::MoveClipWithOverwrite {
                    clip: c,
                    track: t,
                    start: s,
                }),
            ) => (clip, track, start) == (c, t, s),
            (
                Command::CopyClipTo { clip, track, start },
                Request::Edit(Command::CopyClipTo {
                    clip: c,
                    track: t,
                    start: s,
                }),
            ) => (clip, track, start) == (c, t, s),
            (
                Command::CopyMidiClipTo { clip, track, start },
                Request::Edit(Command::CopyMidiClipTo {
                    clip: c,
                    track: t,
                    start: s,
                }),
            ) => (clip, track, start) == (c, t, s),
            (
                Command::MoveMidiClipWithOverwrite { clip, track, start },
                Request::Edit(Command::MoveMidiClipWithOverwrite {
                    clip: c,
                    track: t,
                    start: s,
                }),
            ) => (clip, track, start) == (c, t, s),
            (
                Command::MoveClipsWithOverwrite {
                    clips,
                    anchor,
                    track,
                    position,
                    copy,
                },
                Request::Edit(Command::MoveClipsWithOverwrite {
                    clips: c,
                    anchor: a,
                    track: t,
                    position: p,
                    copy: other_copy,
                }),
            ) => (clips, anchor, track, position, copy) == (c, a, t, p, other_copy),
            _ => false,
        }
    }

    // Selection/focus changes after release cannot cancel an already submitted
    // canonical edit. Only source replacement or its own terminal retires paint.
    fn current(&self, view: &EditorView) -> bool {
        self.drag.project_generation == view.project_generation
            && view.state.as_ref().is_some_and(|s| {
                s.revision >= self.drag.revision
                    && (Arc::ptr_eq(&s.project, &self.source)
                        || self.drag.group.borrow().ids.iter().all(|id| {
                            match (self.source.clip(*id), self.source.midi_clip(*id)) {
                                (Some(original), _) => {
                                    s.project.clip(*id) == Some(original)
                                        && self.source.asset(original.asset())
                                            == s.project.asset(original.asset())
                                }
                                (_, Some(original)) => s.project.midi_clip(*id) == Some(original),
                                _ => false,
                            }
                        }))
            })
    }
}

impl EditorView {
    /// Rendering can borrow the held capture or its released successor. Pending
    /// display never passes the held-drag admission check (work is now in flight).
    pub(super) fn displayed_clip_drag(&self) -> Option<&ClipDrag> {
        self.pending_clip_move
            .as_ref()
            .filter(|p| p.current(self))
            .map(|p| &p.drag)
            .or_else(|| {
                self.active_clip_drag
                    .as_ref()
                    .filter(|d| self.drag_start_is_current(d))
            })
    }

    pub(super) fn pending_clip_move_matches(&self, request: &Request) -> bool {
        self.pending_clip_move
            .as_ref()
            .is_some_and(|p| p.matches(request))
    }

    pub(super) fn clip_move_submitted(&mut self) {
        if let Some(p) = self.pending_clip_move.as_mut() {
            p.receipt.submitted(self.worker_requests_pending);
        }
    }

    pub(super) fn clip_move_request_complete(&mut self, event: &Event) {
        let Some(p) = self.pending_clip_move.as_mut() else {
            return;
        };
        if p.receipt.observe(
            self.project_generation,
            event.project_replaced,
            event.request_complete,
        ) != Disposition::Waiting
        {
            // Accepted and refused terminals both retire before the event's
            // canonical state is installed, in the same synchronous receive.
            self.retire_pending_clip_move();
        }
    }

    pub(super) fn retire_pending_clip_move(&mut self) -> bool {
        let Some(pending) = self.pending_clip_move.take() else {
            return false;
        };
        pending.drag.retire();
        true
    }

    pub(super) fn retire_abandoned_clip_move(&mut self) -> bool {
        let abandoned = self.pending_clip_move.as_ref().is_some_and(|p| {
            !p.current(self)
                || (p.receipt.is_queued()
                    && !self.pending_requests.iter().any(|r| p.matches(r))
                    && !self.device_gesture_barriers.iter().any(|r| p.matches(r)))
        });
        if abandoned {
            self.retire_pending_clip_move();
        }
        abandoned
    }
}
