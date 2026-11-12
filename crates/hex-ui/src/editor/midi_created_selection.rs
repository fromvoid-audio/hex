//! View-only selection custody for canonical note-creating edit replies.
//! Input is serialized by the existing empty edit queue. An intent binds the
//! next ordinary terminal to its source revision/clip and selection epoch;
//! only the worker's returned canonical identities may replace the baseline.
use super::EditorView;
use hex_project::{ClipId, Command, Created, MidiSourceIdentity, NoteId, Project};
use hex_session::{
    service::{Event, Request},
    tuning_gesture::{TuningGestureKey, TuningGesturePhase},
};

pub(super) struct Pending {
    generation: u64,
    revision: u64,
    clip: ClipId,
    selection_epoch: u64,
    tuning_barrier: Option<TuningGestureKey>,
    tuning_key: Option<TuningGestureKey>,
    source: MidiSourceIdentity,
    target: Target,
    remaining: Option<usize>,
}
#[derive(Clone, Copy)]
enum Target {
    Notes,
    TimeRange { start: u64, end: u64, epoch: u64 },
}
impl Target {
    fn is_current(self, view: &EditorView) -> bool {
        match self {
            Self::Notes => true,
            Self::TimeRange { epoch, .. } => view.midi_time_selection_epoch_matches(epoch),
        }
    }
}
pub(super) enum Completion {
    Notes {
        clip: ClipId,
        first: NoteId,
        last: NoteId,
    },
    TimeRange {
        clip: ClipId,
        start: u64,
        end: u64,
    },
}
fn source_identity(project: &Project, clip: ClipId) -> Option<MidiSourceIdentity> {
    project
        .midi_clip(clip)
        .map(|c| c.source_identity())
        .or_else(|| project.session_midi_clip(clip).map(|c| c.source_identity()))
}
impl EditorView {
    pub(super) fn created_note_selection_intent(&self, request: &Request) -> Option<Pending> {
        if self.edit_pending() {
            return None;
        }
        let clip = match request {
            Request::Edit(Command::DuplicateMidiSourceRange { clip, .. }) => *clip,
            Request::Edit(Command::AddMidiNote { clip, .. }) => *clip,
            Request::Edit(Command::AddMidiNotes { clip, notes }) if !notes.is_empty() => *clip,
            Request::Edit(
                Command::EditMidiNotes { clip, insert, .. }
                | Command::EditMidiNotesWithSourceExtent { clip, insert, .. },
            ) if !insert.is_empty() => *clip,
            _ => return None,
        };
        if self.detail_midi_clip_data() != Some(clip) {
            return None;
        }
        let target = match request {
            Request::Edit(Command::DuplicateMidiSourceRange { clip, start, end }) => {
                let epoch = self.midi_time_copy_epoch(*clip, start.raw(), end.raw())?;
                end.raw().checked_add(end.raw().checked_sub(start.raw())?)?;
                Target::TimeRange {
                    start: start.raw(),
                    end: end.raw(),
                    epoch,
                }
            }
            _ => Target::Notes,
        };
        let state = self.state.as_ref()?;
        let source = source_identity(&state.project, clip)?;
        Some(Pending {
            generation: self.project_generation,
            revision: state.revision,
            source,
            clip,
            selection_epoch: self.midi_selection.epoch(),
            target,
            remaining: None,
            tuning_key: self.tuning.client.snapshot().map(|s| s.key),
            tuning_barrier: self
                .tuning
                .client
                .snapshot()
                .filter(|_| self.tuning.client.has_pending_terminal())
                .map(|s| s.key),
        })
    }
    pub(super) fn bind_created_selection_terminal(&self, pending: &mut Pending) {
        // enqueue_request may finalize an active gesture after intent capture.
        // Bind only the terminal actually reserved for that exact captured key.
        pending.tuning_barrier = self
            .tuning
            .client
            .snapshot()
            .filter(|_| self.tuning.client.has_pending_terminal())
            .map(|s| s.key)
            .filter(|key| Some(*key) == pending.tuning_key);
    }
    pub(super) fn pending_time_copy_matches(&self, request: &Request) -> bool {
        let Some(pending) = self.pending_created_note_selection.as_ref() else {
            return false;
        };
        if pending.remaining.is_some() {
            return false;
        }
        matches!(
            (pending.target, request),
            (
                Target::TimeRange { start, end, .. },
                Request::Edit(Command::DuplicateMidiSourceRange { clip, start: a, end: b })
            ) if pending.clip == *clip && start == a.raw() && end == b.raw()
        )
    }
    pub(super) fn time_copy_submitted(&mut self) {
        if let Some(pending) = self.pending_created_note_selection.as_mut()
            && matches!(pending.target, Target::TimeRange { .. })
        {
            pending.remaining = Some(self.worker_requests_pending);
        }
    }
    /// Revision reconciliation cancels pointer previews, but a just-validated
    /// tuning terminal may have rebased this exact pending creation to that revision.
    pub(super) fn cancel_stale_midi_creation_gestures(&mut self, revision: u64) {
        let rebased = self
            .pending_created_note_selection
            .take()
            .filter(|p| p.revision == revision);
        self.cancel_midi_note_gesture();
        self.pending_created_note_selection = rebased;
    }
    pub(super) fn take_created_note_selection(&mut self, event: &Event) -> Option<Completion> {
        if event.project_replaced {
            self.pending_created_note_selection = None;
            return None;
        }
        // Only the exact already-reserved terminal may advance this intent's
        // source revision. Nonterminal overlay updates do not change canonical MIDI.
        if event.tuning_gesture_request {
            let ack = event.tuning_gesture_ack.as_ref()?;
            let pending = self.pending_created_note_selection.as_ref()?;
            if pending.tuning_barrier != Some(ack.key)
                || !matches!(
                    ack.phase,
                    TuningGesturePhase::Commit | TuningGesturePhase::Cancel
                )
            {
                return None;
            }
            let current = self.state.as_ref();
            let revision_matches = match ack.phase {
                TuningGesturePhase::Commit => {
                    event.state.revision == pending.revision
                        || pending.revision.checked_add(1) == Some(event.state.revision)
                }
                TuningGesturePhase::Cancel => event.state.revision == pending.revision,
                _ => false,
            };
            let valid = ack.accepted
                && revision_matches
                && event.error.is_none()
                && self.project_generation == pending.generation
                && self.detail_clip_id() == Some(pending.clip)
                && self.midi_selection.epoch() == pending.selection_epoch
                && pending.target.is_current(self)
                && current.is_some_and(|s| {
                    s.revision == pending.revision
                        && source_identity(&s.project, pending.clip) == Some(pending.source.clone())
                        && source_identity(&event.state.project, pending.clip)
                            == Some(pending.source.clone())
                });
            if valid {
                if let Some(pending) = self.pending_created_note_selection.as_mut() {
                    pending.revision = event.state.revision;
                    pending.tuning_barrier = None;
                }
            } else {
                self.pending_created_note_selection = None;
            }
            return None;
        }
        if !event.request_complete || event.gesture_request {
            return None;
        }
        let time_terminal = if self
            .pending_created_note_selection
            .as_ref()
            .is_some_and(|pending| matches!(pending.target, Target::TimeRange { .. }))
        {
            let pending = self.pending_created_note_selection.as_mut()?;
            let remaining = pending.remaining.as_mut()?;
            *remaining = remaining.saturating_sub(1);
            if *remaining != 0 {
                return None;
            }
            true
        } else {
            false
        };
        let pending = self.pending_created_note_selection.as_ref()?;
        // Metadata/duplicate old terminals cannot consume the outstanding edit.
        if !time_terminal && event.error.is_none() && event.state.revision <= pending.revision {
            return None;
        }
        let pending = self.pending_created_note_selection.take()?;
        if pending.tuning_barrier.is_some()
            || event.error.is_some()
            || !(pending.revision.checked_add(1) == Some(event.state.revision)
                || (time_terminal && event.state.revision == pending.revision))
            || !pending.target.is_current(self)
            || self.state.as_ref().is_none_or(|s| {
                s.revision != pending.revision
                    || source_identity(&s.project, pending.clip) != Some(pending.source.clone())
            })
            || self.project_generation != pending.generation
            || self.detail_clip_id() != Some(pending.clip)
            || self.midi_selection.epoch() != pending.selection_epoch
            || !self.clip_view_open
            || self.opening_project
            || self.close_prompt_open
        {
            return None;
        }
        match pending.target {
            Target::Notes => {
                let (first, last) = match event.created {
                    Some(Created::Note(note)) => (note, note),
                    Some(Created::Notes { clip, first, last }) if clip == pending.clip => {
                        (first, last)
                    }
                    _ => return None,
                };
                Some(Completion::Notes {
                    clip: pending.clip,
                    first,
                    last,
                })
            }
            Target::TimeRange { start, end, .. } => {
                if !matches!(event.created, None | Some(Created::None))
                    && !matches!(event.created,Some(Created::Notes{clip,..}) if clip==pending.clip)
                {
                    return None;
                }
                let destination_end = end.checked_add(end.checked_sub(start)?)?;
                if event
                    .state
                    .project
                    .midi_source(pending.clip)?
                    .source_length()
                    .raw()
                    < destination_end
                {
                    return None;
                }
                Some(Completion::TimeRange {
                    clip: pending.clip,
                    start: end,
                    end: destination_end,
                })
            }
        }
    }
    pub(super) fn apply_created_note_selection(
        &mut self,
        completion: Option<Completion>,
        audition: bool,
    ) {
        let Some(completion) = completion else {
            return;
        };
        let (clip, first, last) = match completion {
            Completion::TimeRange { clip, start, end } => {
                self.apply_midi_time_copy(clip, start, end);
                return;
            }
            Completion::Notes { clip, first, last } => (clip, first, last),
        };
        let Some(source) = self
            .state
            .as_ref()
            .and_then(|s| s.project.midi_source(clip))
        else {
            return;
        };
        let result = super::midi_draw::created_range_ids(
            self.detail_clip_id(),
            clip,
            source.notes(),
            first,
            last,
        );
        match result {
            Ok(Some(ids)) => {
                if let Err(error) = self.midi_selection.select_created(ids, last) {
                    self.error = Some(error.into());
                } else {
                    self.clear_midi_time_selection();
                    if audition {
                        self.audition_midi_selection();
                    }
                }
            }
            Ok(None) => {}
            Err(error) => self.error = Some(error.into()),
        }
    }
}
