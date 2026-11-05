//! One accepted group-copy or clip-paste completion; actual Service FIFO owns
//! its terminal.
use super::{EditorMode, EditorView};
use hex_project::{ClipGroupPosition, ClipId, Command, Project, TrackId};
use hex_session::service::{Event, Request};

/// The request whose acknowledged copy births become the selection.
enum Kind {
    Move {
        anchor: ClipId,
        track: TrackId,
        position: ClipGroupPosition,
        count: usize,
        alias: bool,
    },
    NewTrack {
        clip: ClipId,
        before_track: Option<TrackId>,
        position: ClipGroupPosition,
    },
    /// A clip-clipboard paste or insert. Its reported `first..=last` range
    /// holds only pasted clips (hex-project's paste contract), so the expected
    /// count is the number of clips in that range.
    Paste {
        track: TrackId,
        frame: u64,
        insert: bool,
    },
}

pub(super) struct Pending {
    generation: u64,
    revision: u64,
    epoch: u64,
    primary: Option<ClipId>,
    kind: Kind,
    remaining: Option<usize>,
}
impl Pending {
    fn new(view: &EditorView, kind: Kind) -> Option<Self> {
        Some(Self {
            generation: view.project_generation,
            revision: view.state.as_ref()?.revision,
            epoch: view.clip_selection.epoch,
            primary: view.selected_clip,
            kind,
            remaining: None,
        })
    }
    pub(super) fn capture(view: &EditorView, command: &Command) -> Option<Self> {
        let kind = match command {
            Command::MoveClipsWithOverwrite {
                clips,
                anchor,
                track,
                position,
                copy: true,
            } => Kind::Move {
                anchor: *anchor,
                track: *track,
                position: *position,
                count: clips.len(),
                alias: false,
            },
            Command::DuplicateAliasClipsAt {
                clips,
                anchor,
                track,
                position,
            } => Kind::Move {
                anchor: *anchor,
                track: *track,
                position: *position,
                count: clips.len(),
                alias: true,
            },
            Command::DuplicateAliasClipToNewTrack {
                clip,
                before_track,
                position,
                ..
            } => Kind::NewTrack {
                clip: *clip,
                before_track: *before_track,
                position: *position,
            },
            _ => return None,
        };
        Self::new(view, kind)
    }
    /// Capture a time-selection paste/insert request. A range clipboard paste
    /// reports no clip births, so its completion selects nothing.
    pub(super) fn capture_paste(view: &EditorView, request: &Request) -> Option<Self> {
        let (track, frame, insert) = match request {
            Request::PasteTimeRange { track, frame } => (*track, *frame, false),
            Request::InsertTimeRange { track, frame } => (*track, *frame, true),
            _ => return None,
        };
        Self::new(
            view,
            Kind::Paste {
                track,
                frame,
                insert,
            },
        )
    }
    pub(super) fn submitted(&mut self, remaining: usize) {
        self.remaining = Some(remaining);
    }
    /// The number of committed births the acknowledged range must contain.
    pub(super) fn expected_count(&self, project: &Project, first: ClipId, last: ClipId) -> usize {
        match self.kind {
            Kind::Move { count, .. } => count,
            Kind::NewTrack { .. } => 1,
            Kind::Paste { .. } => project
                .tracks()
                .iter()
                .flat_map(|track| {
                    track
                        .clips()
                        .iter()
                        .map(|c| c.id())
                        .chain(track.midi_clips().iter().map(|c| c.id()))
                })
                .filter(|id| *id >= first && *id <= last)
                .count(),
        }
    }
    fn matches(&self, request: &Request) -> bool {
        if self.remaining.is_some() {
            return false;
        }
        match self.kind {
            Kind::Paste {
                track,
                frame,
                insert,
            } => {
                return match request {
                    Request::PasteTimeRange { track: t, frame: f } => {
                        !insert && *t == track && *f == frame
                    }
                    Request::InsertTimeRange { track: t, frame: f } => {
                        insert && *t == track && *f == frame
                    }
                    _ => false,
                };
            }
            Kind::Move { .. } | Kind::NewTrack { .. } => {}
        }
        let command = match request {
            Request::Edit(command) => command,
            Request::MoveExistingClips {
                project_generation,
                revision,
                command,
            } if *project_generation == self.generation && *revision == self.revision => command,
            _ => return false,
        };
        match self.kind {
            Kind::Move {
                anchor,
                track,
                position,
                count,
                alias: false,
            } => {
                matches!(command, Command::MoveClipsWithOverwrite { clips, anchor: a, track: t, position: p, copy: true }
                    if *a == anchor && *t == track && *p == position && clips.len() == count)
            }
            Kind::Move {
                anchor,
                track,
                position,
                count,
                alias: true,
            } => {
                matches!(command, Command::DuplicateAliasClipsAt { clips, anchor: a, track: t, position: p }
                    if *a == anchor && *t == track && *p == position && clips.len() == count)
            }
            Kind::NewTrack {
                clip,
                before_track,
                position,
            } => {
                matches!(command, Command::DuplicateAliasClipToNewTrack { clip: c, before_track: b, position: p, .. }
                    if *c == clip && *b == before_track && *p == position)
            }
            Kind::Paste { .. } => false,
        }
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
