//! Immutable note-drag/creation display custody from release to the matching Service
//! terminal. Held input ends on release; this owner retains only view geometry,
//! its exact typed command and the original source identity. Canonical note edits,
//! overwrite resolution and history remain project/Session responsibilities.
use super::{
    EditorView,
    midi_drag::{GestureKind, HitKind, NoteGesture},
    midi_group_drag::GroupGesture,
    pending_edit::{Disposition, Receipt},
};
use hex_project::{ClipId, Command, MidiNoteInsert, MidiSourceIdentity, Project};
use hex_session::service::{Event, Request};
use std::sync::Arc;

pub(super) enum Frozen {
    Single(NoteGesture),
    /// Keyboard moves retain canonical paint and use the receipt for source
    /// origin changes and audition.
    Nudge,
    Group(GroupGesture),
    Creation(MidiNoteInsert),
    Velocity(super::midi_velocity::Gesture),
    Stroke(Arc<super::midi_stroke::StrokePreview>),
}

#[derive(Clone, Copy)]
pub(super) enum EditAudition {
    Created(ClipId),
    Moved(ClipId),
}

impl Frozen {
    fn audition(&self, clip: ClipId, command: &Command) -> Option<EditAudition> {
        match self {
            Self::Creation(_) => Some(EditAudition::Created(clip)),
            Self::Stroke(_)
                if matches!(
                    command,
                    Command::EditMidiNotes { insert, .. }
                        | Command::EditMidiNotesWithSourceExtent { insert, .. }
                        if !insert.is_empty()
                ) =>
            {
                Some(EditAudition::Created(clip))
            }
            Self::Single(note) if matches!(note.kind, GestureKind::Move { .. }) => {
                Some(EditAudition::Moved(clip))
            }
            Self::Group(group) if group.kind() == HitKind::Move => Some(if group.is_copy() {
                EditAudition::Created(clip)
            } else {
                EditAudition::Moved(clip)
            }),
            Self::Nudge => Some(EditAudition::Moved(clip)),
            _ => None,
        }
    }
}

pub(super) struct Pending {
    generation: u64,
    clip: ClipId,
    source: MidiSourceIdentity,
    command: Command,
    receipt: Receipt,
    preview: Frozen,
    audition: Option<(u64, EditAudition)>,
}

pub(super) fn source_identity(project: &Project, clip: ClipId) -> Option<MidiSourceIdentity> {
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
                Command::AddMidiNote {
                    clip,
                    start,
                    duration,
                    pitch,
                    channel,
                    velocity,
                },
                Request::Edit(Command::AddMidiNote {
                    clip: c,
                    start: s,
                    duration: d,
                    pitch: p,
                    channel: ch,
                    velocity: v,
                }),
            ) => (clip, start, duration, pitch, channel, velocity) == (c, s, d, p, ch, v),
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
            )
            | (
                Command::UpdateMidiNotesWithSourceExtent { clip, notes },
                Request::Edit(Command::UpdateMidiNotesWithSourceExtent { clip: c, notes: n }),
            ) => (clip, notes) == (c, n),
            (
                Command::UpdateMidiNotesWithSourcePrefix {
                    clip,
                    prefix,
                    notes,
                },
                Request::Edit(Command::UpdateMidiNotesWithSourcePrefix {
                    clip: c,
                    prefix: p,
                    notes: n,
                }),
            ) => (clip, prefix, notes) == (c, p, n),
            (
                Command::EditMidiNotes {
                    clip,
                    remove,
                    insert,
                },
                Request::Edit(Command::EditMidiNotes {
                    clip: c,
                    remove: r,
                    insert: i,
                }),
            )
            | (
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
        let audition = if self.midi_audition.enabled {
            preview.audition(clip, &command).map(|intent| {
                self.cancel_midi_audition();
                (self.midi_selection.epoch(), intent)
            })
        } else {
            None
        };
        self.pending_midi_drag = Some(Pending {
            generation: self.project_generation,
            clip,
            source,
            command: command.clone(),
            receipt: Receipt::new(self.project_generation),
            preview,
            audition,
        });
        if !self.enqueue(Request::Edit(command)) {
            self.retire_pending_midi_drag();
        }
    }

    pub(super) fn displayed_midi_note_gesture(&self) -> Option<NoteGesture> {
        self.midi_note_gesture.or_else(|| {
            let pending = self.pending_midi_drag.as_ref()?;
            let Frozen::Single(note) = &pending.preview else {
                return None;
            };
            pending.displayed(self).then_some(*note)
        })
    }

    pub(super) fn displayed_midi_group_gesture(&self) -> Option<&GroupGesture> {
        self.midi_group_gesture.as_ref().or_else(|| {
            let pending = self.pending_midi_drag.as_ref()?;
            let Frozen::Group(group) = &pending.preview else {
                return None;
            };
            pending.displayed(self).then_some(group)
        })
    }

    /// Held input and accepted display custody are distinct. Only the existing
    /// ordinary receipt/source owner can retire a released creation preview.
    pub(super) fn displayed_midi_creation(&self) -> Option<MidiNoteInsert> {
        self.midi_creation.as_ref().map(|c| c.preview).or_else(|| {
            let pending = self.pending_midi_drag.as_ref()?;
            let Frozen::Creation(note) = &pending.preview else {
                return None;
            };
            pending.displayed(self).then_some(*note)
        })
    }

    /// Released velocity values follow source/receipt custody, independent of
    /// later selection, focus or viewport changes. This never owns input.
    pub(super) fn pending_midi_velocity(
        &self,
        clip: ClipId,
    ) -> Option<&super::midi_velocity::Gesture> {
        let pending = self.pending_midi_drag.as_ref()?;
        let Frozen::Velocity(gesture) = &pending.preview else {
            return None;
        };
        (pending.clip == clip && pending.displayed(self)).then_some(gesture)
    }

    /// Freeze only the immutable display payload, not the working stroke's
    /// baseline index, traversal caches or editable action map.
    pub(super) fn displayed_midi_stroke_preview(
        &self,
    ) -> Option<Arc<super::midi_stroke::StrokePreview>> {
        self.midi_stroke
            .as_ref()
            .map(|stroke| stroke.shared_preview())
            .or_else(|| {
                let pending = self.pending_midi_drag.as_ref()?;
                let Frozen::Stroke(preview) = &pending.preview else {
                    return None;
                };
                pending.displayed(self).then(|| Arc::clone(preview))
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
    pub(super) fn midi_drag_request_complete(&mut self, event: &Event) -> Option<EditAudition> {
        let disposition = self.pending_midi_drag.as_mut()?.receipt.observe(
            self.project_generation,
            event.project_replaced,
            event.request_complete && !event.plugin_parameter_observation,
        );
        if disposition == Disposition::Waiting {
            return None;
        }
        let pending = self.pending_midi_drag.take()?;
        if disposition != Disposition::Terminal
            || event.error.is_some()
            || !pending.source_current(self)
        {
            return None;
        }
        if let Command::UpdateMidiNotesWithSourcePrefix { clip, prefix, .. } = pending.command
            && self
                .state
                .as_ref()
                .and_then(|old| old.project.midi_source(clip))
                .zip(event.state.project.midi_source(clip))
                .is_some_and(|(old, new)| {
                    old.source_start().raw().checked_add(prefix.raw())
                        == Some(new.source_start().raw())
                })
            && let Some(source) = source_identity(&event.state.project, clip)
        {
            self.midi_origin_history.record_prefix(
                self.project_generation,
                clip,
                &pending.source,
                &source,
                prefix.raw(),
            );
        }
        let (epoch, audition) = pending.audition?;
        // The command's exact terminal supplies the edited source. The
        // controller will audition it after installing notes and selection.
        (self.midi_audition.enabled
            && self.clip_view_open
            && !self.opening_project
            && !self.close_prompt_open
            && self.detail_clip_id() == Some(pending.clip)
            && self.midi_selection.epoch() == epoch
            && self.state.as_ref()?.revision.checked_add(1) == Some(event.state.revision))
        .then_some(audition)
    }

    pub(super) fn cancel_pending_midi_edit_audition(&mut self) {
        if let Some(pending) = self.pending_midi_drag.as_mut() {
            pending.audition = None;
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
