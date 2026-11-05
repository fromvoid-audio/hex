//! Explicit grid-step note insertion. Physical input stays in its input owners;
//! this view owner sends one canonical batch and advances only on its FIFO ACK.
//! No callback, audio resources, project history or hardware MIDI connection is owned here.
use super::EditorView;
use hex_project::{BeatTime, ClipId, Command, Created, MidiNoteInsert};
use hex_session::service::{Event, Request};

#[derive(Default)]
pub(super) struct StepEntry {
    pub(super) enabled: bool,
    pending: Option<Pending>,
}

pub(super) struct Pending {
    generation: u64,
    revision: u64,
    clip: ClipId,
    cursor: u64,
    next: u64,
    selection_epoch: u64,
}
impl StepEntry {
    pub(super) fn cancel_pending(&mut self) {
        self.pending = None;
    }
}
impl EditorView {
    /// Fixed-size, pitch-ordered chord shared by preview and canonical entry.
    /// Computer keys win duplicate pitches, retaining the existing velocity/channel policy.
    pub(super) fn midi_step_chord(
        &self,
        cursor: u64,
        duration: u64,
    ) -> Result<[Option<MidiNoteInsert>; 128], &'static str> {
        let hardware = self.hardware_step_notes()?;
        Ok(self.midi_held_chord(
            cursor,
            duration,
            hardware,
            self.computer_keyboard.physical_notes(),
        ))
    }

    fn midi_held_chord(
        &self,
        cursor: u64,
        duration: u64,
        hardware: Option<(hex_midi::HeldNotes, u64, u8)>,
        computer: impl Iterator<Item = super::computer_keyboard::HeldKey>,
    ) -> [Option<MidiNoteInsert>; 128] {
        let mut chord = [None; 128];
        for note in computer {
            chord[usize::from(note.pitch)].get_or_insert(MidiNoteInsert {
                active: true,
                start: BeatTime::from_raw(cursor),
                duration: BeatTime::from_raw(duration),
                pitch: note.pitch,
                channel: 0,
                velocity: note.velocity,
            });
        }
        if let Some((notes, floor, channel)) = hardware {
            for note in notes.after(floor) {
                chord[usize::from(note.pitch)].get_or_insert(MidiNoteInsert {
                    active: true,
                    start: BeatTime::from_raw(cursor),
                    duration: BeatTime::from_raw(duration),
                    pitch: note.pitch,
                    channel,
                    velocity: note.velocity,
                });
            }
        }
        chord
    }

    pub(super) fn midi_live_pitches(&self) -> [bool; 128] {
        if !self.live_midi_context() {
            return [false; 128];
        }
        self.midi_held_chord(
            0,
            1,
            self.hardware_live_notes(),
            self.computer_keyboard
                .physical_notes_after(self.live_computer_floor()),
        )
        .map(|note| note.is_some())
    }

    pub(super) fn midi_step_preview(&self) -> [Option<MidiNoteInsert>; 128] {
        let cursor = self
            .midi_marker
            .unwrap_or(self.piano_beat_start.max(0) as u64);
        let duration = self.midi_snap_grid.movement_step();
        if !self.live_midi_context()
            || !self.midi_step.enabled
            || self.edit_pending()
            || self.midi_step.pending.is_some()
            || self.tuning.client.snapshot().is_some()
            || self.tuning.client.has_pending_requests()
            || self.tuning.client.is_recovery_pending()
            || cursor
                .checked_add(duration)
                .is_none_or(|next| next == u64::MAX)
        {
            return [None; 128];
        }
        self.midi_step_chord(cursor, duration)
            .unwrap_or([None; 128])
    }

    pub(super) fn enter_midi_step(&mut self) {
        if !self.midi_step.enabled
            || self.edit_pending()
            || self.midi_step.pending.is_some()
            || self.tuning.client.snapshot().is_some()
            || self.tuning.client.has_pending_requests()
            || self.tuning.client.is_recovery_pending()
        {
            return;
        }
        let Some(clip) = self.detail_midi_clip_data() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let cursor = self
            .midi_marker
            .unwrap_or(self.piano_beat_start.max(0) as u64);
        let duration = self.midi_snap_grid.movement_step();
        let Some(next) = cursor.checked_add(duration).filter(|next| *next < u64::MAX) else {
            self.error = Some("step entry exceeds the musical time range".into());
            return;
        };
        let held = match self.midi_step_chord(cursor, duration) {
            Ok(notes) => notes,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        let mut insert = Vec::new();
        if insert.try_reserve_exact(hex_midi::HELD_NOTE_COUNT).is_err() {
            self.error = Some("could not reserve step-entry notes".into());
            return;
        }
        insert.extend(held.into_iter().flatten());
        if insert.is_empty() {
            self.midi_marker = Some(next);
            self.reveal_piano_cursor(next);
            return;
        }
        let pending = Pending {
            generation: self.project_generation,
            revision: state.revision,
            clip,
            cursor,
            next,
            selection_epoch: self.midi_selection.epoch(),
        };
        // edit_pending forbids earlier queued/worker/gesture requests. Service's
        // next ordinary terminal is this accepted edit, even if submission queues.
        if self.enqueue(Request::Edit(Command::EditMidiNotesWithSourceExtent {
            clip,
            remove: Vec::new(),
            insert,
        })) {
            self.midi_step.pending = Some(pending);
        }
    }

    pub(super) fn take_completed_midi_step(&mut self, event: &Event) -> Option<Pending> {
        if event.project_replaced {
            self.midi_step.cancel_pending();
            return None;
        }
        if !event.request_complete || event.gesture_request || event.tuning_gesture_request {
            return None;
        }
        let pending = self.midi_step.pending.take()?;
        (event.error.is_none()
            && matches!(event.created, Some(Created::Notes { clip, .. }) if clip == pending.clip)
            && pending.revision.checked_add(1) == Some(event.state.revision)
            && pending.generation == self.project_generation
            && self.detail_clip_id() == Some(pending.clip)
            && self.midi_marker == Some(pending.cursor)
            && self.midi_selection.epoch() == pending.selection_epoch
            && self.midi_step.enabled)
            .then_some(pending)
    }

    pub(super) fn apply_completed_midi_step(&mut self, pending: Option<Pending>) {
        if let Some(pending) = pending
            && self.project_generation == pending.generation
            && self.detail_clip_id() == Some(pending.clip)
            && self.midi_step.enabled
            && self.midi_marker == Some(pending.cursor)
        {
            self.midi_marker = Some(pending.next);
            self.reveal_piano_cursor(pending.next);
        }
    }
}
