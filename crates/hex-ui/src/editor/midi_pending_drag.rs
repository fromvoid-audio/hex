//! Immutable note-drag display custody from release to the matching Service
//! terminal. Held input ends on release; this owner retains only view geometry,
//! its exact typed command and the original source identity. Canonical note edits,
//! overwrite resolution and history remain project/Session responsibilities.
use super::{
    EditorView,
    midi_drag::NoteGesture,
    midi_group_drag::GroupGesture,
    pending_edit::{Disposition, Receipt},
};
use hex_project::{ClipId, Command, MidiSourceIdentity, Project};
use hex_session::service::{Event, Request};

pub(super) enum Frozen {
    Single(NoteGesture),
    Group(GroupGesture),
}

pub(super) struct Pending {
    generation: u64,
    clip: ClipId,
    source: MidiSourceIdentity,
    command: Command,
    receipt: Receipt,
    preview: Frozen,
}

fn source_identity(project: &Project, clip: ClipId) -> Option<MidiSourceIdentity> {
    project
        .midi_clip(clip)
        .map(|clip| clip.source_identity())
        .or_else(|| {
            project
                .session_midi_clip(clip)
                .map(|clip| clip.source_identity())
        })
}

impl Pending {
    fn matches(&self, request: &Request) -> bool {
        if !self.receipt.is_queued() {
            return false;
        }
        match (&self.command, request) {
            (
                Command::UpdateMidiNote {
                    note,
                    start,
                    duration,
                    pitch,
                    channel,
                    velocity,
                },
                Request::Edit(Command::UpdateMidiNote {
                    note: n,
                    start: s,
                    duration: d,
                    pitch: p,
                    channel: c,
                    velocity: v,
                }),
            ) => (note, start, duration, pitch, channel, velocity) == (n, s, d, p, c, v),
            (
                Command::UpdateMidiNotes { clip, notes },
                Request::Edit(Command::UpdateMidiNotes { clip: c, notes: n }),
            ) => (clip, notes) == (c, n),
            (
                Command::EditMidiNotesWithSourceExtent {
                    clip,
                    remove,
                    insert,
                },
                Request::Edit(Command::EditMidiNotesWithSourceExtent {
                    clip: c,
                    remove: r,
                    insert: i,
                }),
            ) => (clip, remove, insert) == (c, r, i),
            _ => false,
        }
    }

    fn source_current(&self, view: &EditorView) -> bool {
        self.generation == view.project_generation
            && view
                .state
                .as_ref()
                .and_then(|s| source_identity(&s.project, self.clip))
                .is_some_and(|source| source == self.source)
    }

    fn displayed(&self, view: &EditorView) -> bool {
        view.detail_clip_id() == Some(self.clip) && self.source_current(view)
    }
}

impl EditorView {
    /// Capture before enqueue so direct submissions and deferred tuning/device
    /// barriers can stamp this same immutable intent exactly once.
    pub(super) fn submit_midi_drag_preview(
        &mut self,
        clip: ClipId,
        preview: Frozen,
        command: Command,
    ) {
        let Some(source) = self
            .state
            .as_ref()
            .and_then(|s| source_identity(&s.project, clip))
        else {
            return;
        };
        self.pending_midi_drag = Some(Pending {
            generation: self.project_generation,
            clip,
            source,
            command: command.clone(),
            receipt: Receipt::new(self.project_generation),
            preview,
        });
        if !self.enqueue(Request::Edit(command)) {
            self.retire_pending_midi_drag();
        }
    }

    pub(super) fn displayed_midi_note_gesture(&self) -> Option<NoteGesture> {
        self.midi_note_gesture.or_else(|| {
            self.pending_midi_drag
                .as_ref()
                .filter(|p| p.displayed(self))
                .and_then(|p| match &p.preview {
                    Frozen::Single(note) => Some(*note),
                    Frozen::Group(_) => None,
                })
        })
    }

    pub(super) fn displayed_midi_group_gesture(&self) -> Option<&GroupGesture> {
        self.midi_group_gesture.as_ref().or_else(|| {
            self.pending_midi_drag
                .as_ref()
                .filter(|p| p.displayed(self))
                .and_then(|p| match &p.preview {
                    Frozen::Group(group) => Some(group),
                    Frozen::Single(_) => None,
                })
        })
    }

    pub(super) fn pending_midi_drag_matches(&self, request: &Request) -> bool {
        self.pending_midi_drag
            .as_ref()
            .is_some_and(|p| p.matches(request))
    }

    pub(super) fn midi_drag_submitted(&mut self) {
        if let Some(pending) = self.pending_midi_drag.as_mut() {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }

    /// Run before state installation in the same synchronous receive. Both
    /// success and refusal replace pending paint with the terminal's canonical
    /// snapshot atomically; earlier completions and background events do not.
    pub(super) fn midi_drag_request_complete(&mut self, event: &Event) {
        if let Some(pending) = self.pending_midi_drag.as_mut()
            && pending.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            ) != Disposition::Waiting
        {
            self.retire_pending_midi_drag();
        }
    }

    pub(super) fn retire_pending_midi_drag(&mut self) -> bool {
        self.pending_midi_drag.take().is_some()
    }

    pub(super) fn retire_abandoned_midi_drag(&mut self) -> bool {
        let abandoned = self.pending_midi_drag.as_ref().is_some_and(|p| {
            !p.source_current(self)
                || (p.receipt.is_queued()
                    && !self.pending_requests.iter().any(|r| p.matches(r))
                    && !self.device_gesture_barriers.iter().any(|r| p.matches(r)))
        });
        abandoned && self.retire_pending_midi_drag()
    }
}
