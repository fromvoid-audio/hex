//! Immutable note-drag/creation display custody from release to the matching Service
//! terminal. Held input ends on release; this owner retains only view geometry,
//! its exact typed command and the original source identity. Canonical note edits,
//! overwrite resolution and history remain project/Session responsibilities.
//!
//! A bounded FIFO queue (`PendingQueue`) lets back-to-back piano-roll edits
//! remain in flight simultaneously without dropping follow-up clicks or
//! clobbering earlier frozen previews before their exact worker terminals arrive.
use super::{
    EditorView,
    midi_drag::{GestureKind, HitKind, NoteGesture, NotePreview},
    midi_group_drag::{GroupGesture, GroupNote},
    pending_edit::{Disposition, Receipt},
};
use hex_project::{ClipId, Command, MidiNoteInsert, MidiSourceIdentity, NoteId, Project};
use hex_session::service::{Event, Request};
use std::collections::VecDeque;
use std::sync::Arc;

pub(super) const MAX_PENDING_MIDI_DRAGS: usize = 16;

#[derive(Default)]
pub(super) struct PendingQueue {
    entries: VecDeque<Pending>,
}

impl PendingQueue {
    #[allow(dead_code)]
    pub(super) fn is_none(&self) -> bool {
        self.entries.is_empty()
    }

    #[allow(dead_code)]
    pub(super) fn is_some(&self) -> bool {
        !self.entries.is_empty()
    }

    pub(super) fn is_full(&self) -> bool {
        self.entries.len() >= MAX_PENDING_MIDI_DRAGS
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

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
        self.submit_midi_drag_preview_with_audition(clip, preview, command, false);
    }

    /// Submit a drag/create/nudge edit, skipping post-receipt audition when the
    /// final preview note(s) already sounded live during the gesture.
    pub(super) fn submit_midi_drag_preview_with_audition(
        &mut self,
        clip: ClipId,
        preview: Frozen,
        command: Command,
        already_auditioned: bool,
    ) {
        let Some(source) = self
            .state
            .as_ref()
            .and_then(|s| source_identity(&s.project, clip))
        else {
            return;
        };
        let audition = if self.midi_editor.audition.enabled && !already_auditioned {
            preview.audition(clip, &command).map(|intent| {
                self.cancel_midi_audition();
                (self.midi_editor.selection.epoch(), intent)
            })
        } else {
            None
        };
        let entry = Pending {
            generation: self.project_generation,
            clip,
            source,
            command: command.clone(),
            receipt: Receipt::new(self.project_generation),
            preview,
            audition,
        };
        self.midi_editor.pending_drag.entries.push_back(entry);
        let fallback_command = command.clone();
        if !self.enqueue(Request::Edit(command)) {
            self.retire_unsubmitted_midi_drag(&Request::Edit(fallback_command));
        }
    }

    pub(super) fn displayed_midi_note_gesture(&self) -> Option<NoteGesture> {
        self.midi_editor.note_gesture.or_else(|| {
            self.midi_editor
                .pending_drag
                .entries
                .iter()
                .rev()
                .find_map(|pending| {
                    let Frozen::Single(note) = &pending.preview else {
                        return None;
                    };
                    pending.displayed(self).then_some(*note)
                })
        })
    }

    /// All displayed single-note previews in chronological order (pending queue
    /// followed by any held gesture), deduplicated by note ID so the latest
    /// edit wins for each note without flickering earlier in-flight notes.
    pub(super) fn displayed_midi_single_previews(&self) -> Vec<(NoteId, NotePreview, u64)> {
        let mut previews: Vec<(NoteId, NotePreview, u64)> = Vec::new();
        for pending in &self.midi_editor.pending_drag.entries {
            if let Frozen::Single(note) = &pending.preview
                && pending.displayed(self)
            {
                if let Some(existing) = previews.iter_mut().find(|(id, _, _)| *id == note.note) {
                    *existing = (note.note, note.preview, note.source_prefix);
                } else {
                    previews.push((note.note, note.preview, note.source_prefix));
                }
            }
        }
        if let Some(note) = self.midi_editor.note_gesture {
            if let Some(existing) = previews.iter_mut().find(|(id, _, _)| *id == note.note) {
                *existing = (note.note, note.preview, note.source_prefix);
            } else {
                previews.push((note.note, note.preview, note.source_prefix));
            }
        }
        previews
    }

    /// Latest displayed preview override for a note on `clip`, used so
    /// back-to-back clicks hit a note at its moved preview position before the
    /// earlier worker terminal arrives.
    pub(super) fn displayed_midi_note_override(
        &self,
        clip: ClipId,
        note_id: NoteId,
    ) -> Option<(NotePreview, u64)> {
        for pending in self.midi_editor.pending_drag.entries.iter().rev() {
            if pending.clip != clip || !pending.displayed(self) {
                continue;
            }
            match &pending.preview {
                Frozen::Single(note) if note.note == note_id => {
                    return Some((note.preview, note.source_prefix));
                }
                Frozen::Group(group) if !group.is_copy() => {
                    if let Some(preview) = group.preview(note_id) {
                        return Some((
                            NotePreview {
                                start: preview.start,
                                duration: preview.duration,
                                pitch: preview.pitch,
                            },
                            group.source_prefix(),
                        ));
                    }
                }
                _ => {}
            }
        }
        None
    }

    pub(super) fn displayed_midi_group_gesture(&self) -> Option<&GroupGesture> {
        self.midi_editor.group_gesture.as_ref().or_else(|| {
            self.midi_editor
                .pending_drag
                .entries
                .iter()
                .rev()
                .find_map(|pending| {
                    let Frozen::Group(group) = &pending.preview else {
                        return None;
                    };
                    pending.displayed(self).then_some(group)
                })
        })
    }

    pub(super) fn displayed_midi_group_previews(&self) -> Vec<(Arc<[GroupNote]>, GroupGesture)> {
        let mut groups = Vec::new();
        for pending in &self.midi_editor.pending_drag.entries {
            if let Frozen::Group(group) = &pending.preview
                && pending.displayed(self)
            {
                groups.push((group.shared_originals(), group.clone()));
            }
        }
        if let Some(group) = &self.midi_editor.group_gesture {
            groups.push((group.shared_originals(), group.clone()));
        }
        groups
    }

    /// Held input and accepted display custody are distinct. Only the existing
    /// ordinary receipt/source owner can retire a released creation preview.
    pub(super) fn displayed_midi_creation(&self) -> Option<MidiNoteInsert> {
        self.midi_editor
            .creation
            .as_ref()
            .map(|c| c.preview)
            .or_else(|| {
                self.midi_editor
                    .pending_drag
                    .entries
                    .iter()
                    .rev()
                    .find_map(|pending| {
                        let Frozen::Creation(note) = &pending.preview else {
                            return None;
                        };
                        pending.displayed(self).then_some(*note)
                    })
            })
    }

    /// Released velocity values follow source/receipt custody, independent of
    /// later selection, focus or viewport changes. This never owns input.
    pub(super) fn pending_midi_velocity(
        &self,
        clip: ClipId,
    ) -> Option<&super::midi_velocity::Gesture> {
        self.midi_editor
            .pending_drag
            .entries
            .iter()
            .rev()
            .find_map(|pending| {
                let Frozen::Velocity(gesture) = &pending.preview else {
                    return None;
                };
                (pending.clip == clip && pending.displayed(self)).then_some(gesture)
            })
    }

    /// Freeze only the immutable display payload, not the working stroke's
    /// baseline index, traversal caches or editable action map.
    pub(super) fn displayed_midi_stroke_preview(
        &self,
    ) -> Option<Arc<super::midi_stroke::StrokePreview>> {
        self.midi_editor
            .stroke
            .as_ref()
            .map(|stroke| stroke.shared_preview())
            .or_else(|| {
                self.midi_editor
                    .pending_drag
                    .entries
                    .iter()
                    .rev()
                    .find_map(|pending| {
                        let Frozen::Stroke(preview) = &pending.preview else {
                            return None;
                        };
                        pending.displayed(self).then(|| Arc::clone(preview))
                    })
            })
    }

    pub(super) fn pending_midi_drag_matches(&self, request: &Request) -> bool {
        self.midi_editor
            .pending_drag
            .entries
            .iter()
            .any(|p| p.matches(request))
    }

    pub(super) fn midi_drag_submitted(&mut self) {
        if let Some(pending) = self
            .midi_editor
            .pending_drag
            .entries
            .iter_mut()
            .find(|p| p.receipt.is_queued())
        {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }

    /// Run before state installation in the same synchronous receive. Both
    /// success and refusal replace pending paint with the terminal's canonical
    /// snapshot atomically; earlier completions and background events do not.
    pub(super) fn midi_drag_request_complete(&mut self, event: &Event) -> Option<EditAudition> {
        let request_complete = event.request_complete && !event.plugin_parameter_observation;
        let mut remaining = VecDeque::with_capacity(self.midi_editor.pending_drag.entries.len());
        let mut completed = Vec::new();
        while let Some(mut pending) = self.midi_editor.pending_drag.entries.pop_front() {
            let disposition = pending.receipt.observe(
                self.project_generation,
                event.project_replaced,
                request_complete,
            );
            if disposition == Disposition::Waiting {
                remaining.push_back(pending);
            } else {
                completed.push((pending, disposition));
            }
        }
        self.midi_editor.pending_drag.entries = remaining;

        let mut completed_clips: Vec<ClipId> = Vec::new();
        let mut audition_result = None;
        for (pending, disposition) in completed {
            if disposition != Disposition::Terminal
                || event.error.is_some()
                || !pending.source_current(self)
            {
                continue;
            }
            if !completed_clips.contains(&pending.clip) {
                completed_clips.push(pending.clip);
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
                self.midi_editor.origin_history.record_prefix(
                    self.project_generation,
                    clip,
                    &pending.source,
                    &source,
                    prefix.raw(),
                );
            }
            if let Some((epoch, audition)) = pending.audition
                && self.midi_editor.audition.enabled
                && self.audio_detail.clip_view_open
                && !self.persistence.opening_project
                && !self.persistence.close_prompt_open
                && self.detail_clip_id() == Some(pending.clip)
                && self.midi_editor.selection.epoch() == epoch
                && self
                    .state
                    .as_ref()
                    .is_some_and(|s| s.revision.checked_add(1) == Some(event.state.revision))
            {
                audition_result = Some(audition);
            }
        }

        // Advance the tracked source identity on remaining queued/in-flight
        // MIDI edits for the same clip when our own earlier edit just landed,
        // so follow-up previews survive without flickering while genuine
        // external source replacements still retire them.
        if !event.project_replaced && event.error.is_none() {
            for clip in completed_clips {
                if let Some(new_source) = source_identity(&event.state.project, clip) {
                    for pending in &mut self.midi_editor.pending_drag.entries {
                        if pending.clip == clip && pending.generation == self.project_generation {
                            pending.source = new_source.clone();
                        }
                    }
                }
            }
            self.rebase_held_midi_gestures(&event.state.project, event.state.revision);
        }

        audition_result
    }

    /// Keep an in-progress held note drag alive when a non-conflicting canonical
    /// revision lands (e.g. an earlier note move or clip duplicate completes).
    fn rebase_held_midi_gestures(&mut self, project: &Project, revision: u64) {
        if let Some(gesture) = self.midi_editor.note_gesture.as_mut()
            && gesture.generation == self.project_generation
            && gesture.revision < revision
            && let Some(source) = project.midi_source(gesture.clip)
            && source.source_length().raw() == gesture.clip_length
            && source.notes().iter().any(|note| {
                note.id() == gesture.note
                    && note.start().raw() == gesture.original.start
                    && note.duration().raw() == gesture.original.duration
                    && note.pitch() == gesture.original.pitch
                    && note.channel() == gesture.channel
                    && note.velocity() == gesture.velocity
            })
        {
            gesture.revision = revision;
        }
        if let Some(gesture) = self.midi_editor.group_gesture.as_mut()
            && gesture.captured_context().generation == self.project_generation
            && gesture.captured_context().revision < revision
            && let Some(clip) = gesture.captured_context().clip
            && let Some(source) = project.midi_source(clip)
            && source.source_length().raw() == gesture.source_length()
            && gesture.shared_originals().iter().all(|orig| {
                source.notes().iter().any(|note| {
                    note.id() == orig.id
                        && note.start().raw() == orig.start
                        && note.duration().raw() == orig.duration
                        && note.pitch() == orig.pitch
                        && note.channel() == orig.channel
                        && note.velocity() == orig.velocity
                })
            })
        {
            gesture.rebase_revision(revision);
        }
    }

    pub(super) fn cancel_pending_midi_edit_audition(&mut self) {
        for pending in &mut self.midi_editor.pending_drag.entries {
            pending.audition = None;
        }
    }

    pub(super) fn retire_pending_midi_drag(&mut self) -> bool {
        let had = !self.midi_editor.pending_drag.entries.is_empty();
        self.midi_editor.pending_drag.entries.clear();
        had
    }

    pub(super) fn retire_unsubmitted_midi_drag(&mut self, request: &Request) -> bool {
        if let Some(index) = self
            .midi_editor
            .pending_drag
            .entries
            .iter()
            .position(|p| p.matches(request))
        {
            self.midi_editor.pending_drag.entries.remove(index);
            true
        } else {
            false
        }
    }

    pub(super) fn retire_abandoned_midi_drag(&mut self) -> bool {
        let before = self.midi_editor.pending_drag.entries.len();
        let mut kept = VecDeque::with_capacity(before);
        while let Some(pending) = self.midi_editor.pending_drag.entries.pop_front() {
            let abandoned = !pending.source_current(self)
                || (pending.receipt.is_queued()
                    && !self.queued_requests().any(|r| pending.matches(r)));
            if !abandoned {
                kept.push_back(pending);
            }
        }
        self.midi_editor.pending_drag.entries = kept;
        self.midi_editor.pending_drag.entries.len() != before
    }
}
