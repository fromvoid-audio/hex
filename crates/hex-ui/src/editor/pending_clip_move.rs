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
    members: Vec<hex_project::ClipId>,
}

impl Pending {
    pub(super) fn new(drag: ClipDrag, command: Command, source: Arc<Project>) -> Self {
        let receipt = Receipt::new(drag.project_generation);
        let mut members = drag.group.borrow().ids.clone();
        members.sort_unstable();
        Self {
            drag,
            command,
            receipt,
            source,
            members,
        }
    }

    fn matches(&self, request: &Request) -> bool {
        if !self.receipt.is_queued() {
            return false;
        }
        let command = match request {
            Request::Edit(command) => command,
            Request::MoveExistingClips {
                project_generation,
                revision,
                command,
            } if *project_generation == self.drag.project_generation
                && *revision == self.drag.revision =>
            {
                command
            }
            _ => return false,
        };
        match (&self.command, command) {
            (
                Command::MoveClipWithOverwrite { clip, track, start },
                Command::MoveClipWithOverwrite {
                    clip: c,
                    track: t,
                    start: s,
                },
            ) => (clip, track, start) == (c, t, s),
            (
                Command::CopyClipTo { clip, track, start },
                Command::CopyClipTo {
                    clip: c,
                    track: t,
                    start: s,
                },
            ) => (clip, track, start) == (c, t, s),
            (
                Command::CopyMidiClipTo { clip, track, start },
                Command::CopyMidiClipTo {
                    clip: c,
                    track: t,
                    start: s,
                },
            ) => (clip, track, start) == (c, t, s),
            (
                Command::MoveMidiClipWithOverwrite { clip, track, start },
                Command::MoveMidiClipWithOverwrite {
                    clip: c,
                    track: t,
                    start: s,
                },
            ) => (clip, track, start) == (c, t, s),
            (
                Command::MoveClipsWithOverwrite {
                    clips,
                    anchor,
                    track,
                    position,
                    copy,
                },
                Command::MoveClipsWithOverwrite {
                    clips: c,
                    anchor: a,
                    track: t,
                    position: p,
                    copy: other_copy,
                },
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
                        || selected_sources_unchanged(&self.source, &s.project, &self.members))
            })
    }
}

// Scan each track's clip lists once, rather than looking up each selected clip
// across the whole project on every pending frame. IDs are sorted at capture;
// unrelated mixer edits and clip insertions do not invalidate this projection.
fn selected_sources_unchanged(
    source: &Project,
    current: &Project,
    members: &[hex_project::ClipId],
) -> bool {
    source.tracks().iter().all(|original| {
        let selected = |id| members.binary_search(&id).is_ok();
        if !original
            .clips()
            .iter()
            .map(|clip| clip.id())
            .chain(original.midi_clips().iter().map(|clip| clip.id()))
            .any(selected)
        {
            return true;
        }
        let Some(current_track) = current.track(original.id()) else {
            return false;
        };
        original
            .clips()
            .iter()
            .filter(|clip| selected(clip.id()))
            .eq(current_track
                .clips()
                .iter()
                .filter(|clip| selected(clip.id())))
            && original
                .clips()
                .iter()
                .filter(|clip| selected(clip.id()))
                .all(|clip| source.asset(clip.asset()) == current.asset(clip.asset()))
            && original
                .midi_clips()
                .iter()
                .filter(|clip| selected(clip.id()))
                .eq(current_track
                    .midi_clips()
                    .iter()
                    .filter(|clip| selected(clip.id())))
    })
}

impl EditorView {
    /// Rendering can borrow the held capture or its released successor. Pending
    /// display never passes the held-drag admission check (work is now in flight).
    pub(super) fn displayed_clip_drag(&self) -> Option<&ClipDrag> {
        self.pending_clip_move
            .as_ref()
            .filter(|p| p.current(self))
            .map(|p| &p.drag)
            .or_else(|| self.pending_bounce_drag())
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

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Created, Editor};

    #[test]
    fn unrelated_revisions_keep_selected_sources_but_member_edits_invalidate_them() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let mut members = Vec::new();
        for start in [0, 2] {
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::from_quarters(start).unwrap(),
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip");
            };
            members.push(clip);
        }
        members.sort_unstable();
        let source = editor.project().clone();
        editor.execute(Command::Gain { track, gain: 0.5 }).unwrap();
        editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(4).unwrap(),
                length: BeatTime::ONE,
            })
            .unwrap();
        assert!(selected_sources_unchanged(
            &source,
            editor.project(),
            &members
        ));
        editor
            .execute(Command::MoveMidiClipWithOverwrite {
                clip: members[0],
                track,
                start: BeatTime::ONE,
            })
            .unwrap();
        assert!(!selected_sources_unchanged(
            &source,
            editor.project(),
            &members
        ));
        assert!(editor.undo());
        assert!(selected_sources_unchanged(
            &source,
            editor.project(),
            &members
        ));
        editor.execute(Command::RemoveTrack(track)).unwrap();
        assert!(!selected_sources_unchanged(
            &source,
            editor.project(),
            &members
        ));
    }
}
