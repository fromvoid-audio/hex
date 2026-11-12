//! Release-to-ACK display custody for clip moves and copies. Service FIFO
//! completion ordinals are the same contract used by trim and group-copy intents.
//! This owns metadata paint only; the worker validates and commits the real edit.
use super::pending_edit::{Disposition, Receipt};
use super::{EditorView, drag::ClipDrag, edit_cursor::DropCursor, viewport::Viewport};
use hex_project::{Command, Created, Project};
use hex_session::service::{Event, Request};
use std::sync::{Arc, Weak};

pub(super) struct Pending {
    pub(super) drag: ClipDrag,
    command: Command,
    receipt: Receipt,
    source: Arc<Project>,
    members: Vec<hex_project::ClipId>,
    validated_source: Option<Weak<Project>>,
    cursor_epoch: Option<u64>,
}

impl Pending {
    pub(super) fn new(
        drag: ClipDrag,
        command: Command,
        source: Arc<Project>,
        cursor_epoch: Option<u64>,
    ) -> Self {
        let receipt = Receipt::new(drag.project_generation);
        let mut members = drag.group.borrow().ids.clone();
        members.sort_unstable();
        Self {
            drag,
            command,
            receipt,
            validated_source: Some(Arc::downgrade(&source)),
            source,
            members,
            cursor_epoch,
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
            (
                Command::DuplicateAliasClipsAt {
                    clips,
                    anchor,
                    track,
                    position,
                },
                Command::DuplicateAliasClipsAt {
                    clips: c,
                    anchor: a,
                    track: t,
                    position: p,
                },
            ) => (clips, anchor, track, position) == (c, a, t, p),
            (
                Command::MoveClipToNewTrack {
                    clip,
                    name,
                    before_track,
                    start,
                    initial_gain,
                },
                Command::MoveClipToNewTrack {
                    clip: c,
                    name: n,
                    before_track: b,
                    start: s,
                    initial_gain: g,
                },
            )
            | (
                Command::CopyClipToNewTrack {
                    clip,
                    name,
                    before_track,
                    start,
                    initial_gain,
                },
                Command::CopyClipToNewTrack {
                    clip: c,
                    name: n,
                    before_track: b,
                    start: s,
                    initial_gain: g,
                },
            ) => (clip, name, before_track, start, initial_gain) == (c, n, b, s, g),
            (
                Command::MoveMidiClipToNewTrack {
                    clip,
                    name,
                    before_track,
                    start,
                    initial_gain,
                },
                Command::MoveMidiClipToNewTrack {
                    clip: c,
                    name: n,
                    before_track: b,
                    start: s,
                    initial_gain: g,
                },
            )
            | (
                Command::CopyMidiClipToNewTrack {
                    clip,
                    name,
                    before_track,
                    start,
                    initial_gain,
                },
                Command::CopyMidiClipToNewTrack {
                    clip: c,
                    name: n,
                    before_track: b,
                    start: s,
                    initial_gain: g,
                },
            ) => (clip, name, before_track, start, initial_gain) == (c, n, b, s, g),
            (
                Command::DuplicateAliasClipToNewTrack {
                    clip,
                    name,
                    before_track,
                    position,
                    initial_gain,
                },
                Command::DuplicateAliasClipToNewTrack {
                    clip: c,
                    name: n,
                    before_track: b,
                    position: p,
                    initial_gain: g,
                },
            ) => (clip, name, before_track, position, initial_gain) == (c, n, b, p, g),
            _ => false,
        }
    }

    // Selection/focus changes after release cannot cancel an already submitted
    // canonical edit. Only source replacement or its own terminal retires paint.
    fn current(&self, view: &EditorView) -> bool {
        self.drag.project_generation == view.project_generation
            && view.state.as_ref().is_some_and(|s| {
                s.revision >= self.drag.revision
                    && self
                        .validated_source
                        .as_ref()
                        .is_some_and(|project| project.as_ptr() == Arc::as_ptr(&s.project))
            })
    }

    fn observe_source(&mut self, project: &Arc<Project>) {
        if self
            .validated_source
            .as_ref()
            .is_some_and(|last| last.as_ptr() == Arc::as_ptr(project))
        {
            return;
        }
        // Source validation belongs to canonical installation. Painting can
        // then check the validated identity in constant time on every frame.
        self.validated_source = (Arc::ptr_eq(project, &self.source)
            || selected_sources_unchanged(&self.source, project, &self.members))
        .then(|| Arc::downgrade(project));
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

    /// Position of the captured anchor clip in the displayed move preview.
    /// The same borrowed payload survives release until its own receipt settles.
    pub(super) fn displayed_clip_move_guide_frame(&self, viewport: Viewport) -> Option<u64> {
        let drag = self
            .pending_clip_move
            .as_ref()
            .filter(|p| p.current(self))
            .map(|p| &p.drag)
            .or_else(|| self.pending_bounce_drag())
            .or_else(|| {
                let state = self.state.as_ref()?;
                self.active_clip_drag.as_ref().filter(|drag| {
                    let group = drag.group.borrow();
                    !self.opening_project
                        && !self.close_prompt_open
                        && drag.project_generation == self.project_generation
                        && drag.revision == state.revision
                        && drag.viewport() == viewport
                        && drag.snap_grid == self.snap_grid
                        && (group.ids.len() <= 1
                            || group.selection_epoch == Some(self.clip_selection.epoch))
                })
            })?;
        drag.cursor_position().map(|(_, frame)| frame)
    }

    /// The callback-confirmed anchor is shown separately only while it differs
    /// from the held pointer target. Never infer a position from submission.
    pub(super) fn audible_clip_move_anchor(&self) -> Option<(hex_project::TrackId, u64)> {
        let drag = self.active_clip_drag.as_ref()?;
        let (audio_revision, applied) = self.applied_clip_preview?;
        let desired = self.expected_clip_preview?;
        let state = self.state.as_ref()?;
        if !self.drag_start_is_current(drag)
            || self
                .audio_status()?
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire)
                != audio_revision
            || applied.context != desired.context
            || applied.context.project_generation != self.project_generation
            || applied.context.editor_revision != state.revision
            || applied.gesture != desired.gesture
            || applied.sequence > desired.sequence
            || (applied.track, applied.position, applied.mode)
                == (desired.track, desired.position, desired.mode)
        {
            return None;
        }
        let frame = match applied.position {
            hex_project::ClipGroupPosition::Audio(frame) => frame,
            hex_project::ClipGroupPosition::Midi(beat) => {
                state.project.tempo_map().beat_to_frame(beat).ok()?
            }
        };
        Some((applied.track, frame))
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

    pub(super) fn clip_move_request_complete(&mut self, event: &Event) -> Option<DropCursor> {
        let p = self.pending_clip_move.as_mut()?;
        let disposition = p.receipt.observe(
            self.project_generation,
            event.project_replaced,
            event.request_complete,
        );
        if disposition == Disposition::Waiting {
            p.observe_source(&event.state.project);
            return None;
        }
        let pending = self.pending_clip_move.take()?;
        self.service.retire_arrangement_move_audition();
        self.expected_clip_preview = None;
        self.applied_clip_preview = None;
        let cursor = (disposition == Disposition::Terminal)
            .then(|| committed_drop_cursor(&pending.drag, pending.cursor_epoch, event))
            .flatten();
        pending.drag.retire();
        cursor
    }

    pub(super) fn retire_pending_clip_move(&mut self) -> bool {
        let Some(pending) = self.pending_clip_move.take() else {
            return false;
        };
        self.service.retire_arrangement_move_audition();
        self.expected_clip_preview = None;
        self.applied_clip_preview = None;
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

/// Use only the exact successful terminal. A new-track alias can name the
/// copied clip rather than its owner, so resolve that owner from the receipt.
pub(super) fn committed_drop_cursor(
    drag: &ClipDrag,
    epoch: Option<u64>,
    event: &Event,
) -> Option<DropCursor> {
    if event.error.is_some() || event.project_replaced {
        return None;
    }
    let created = event.created.as_ref()?;
    let (track, frame) = drag.cursor_position()?;
    let track = match (track, created) {
        (Some(track), _) => track,
        (None, Created::Track(track)) => *track,
        (None, Created::Clip(clip) | Created::Clips { first: clip, .. }) => event
            .state
            .project
            .tracks()
            .iter()
            .find(|track| {
                track.clips().iter().any(|item| item.id() == *clip)
                    || track.midi_clips().iter().any(|item| item.id() == *clip)
            })?
            .id(),
        _ => return None,
    };
    Some(DropCursor {
        track,
        frame,
        generation: drag.project_generation,
        epoch: epoch?,
    })
}

#[cfg(all(test, not(feature = "ui-test-support")))]
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
