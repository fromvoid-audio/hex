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
        let cursor = self.midi_marker.unwrap_or(self.piano_beat_start.raw());
        let duration = self.midi_snap_grid.movement_step();
        let Some(next) = cursor.checked_add(duration).filter(|next| *next < u64::MAX) else {
            self.error = Some("step entry exceeds the musical time range".into());
            return;
        };
        let hardware = match self.hardware_step_notes() {
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
        insert.extend(
            self.computer_keyboard
                .physical_notes()
                .map(|note| MidiNoteInsert {
                    start: BeatTime::from_raw(cursor),
                    duration: BeatTime::from_raw(duration),
                    pitch: note.pitch,
                    channel: 0,
                    velocity: note.velocity,
                }),
        );
        insert.sort_by_key(|note| note.pitch);
        insert.dedup_by_key(|note| note.pitch);
        // Computer keys have priority for duplicate pitches. Capture remains
        // separate from monitoring; exact hardware velocities/channel are edits.
        if let Some((notes, floor, channel)) = hardware {
            for note in notes.after(floor) {
                if !insert.iter().any(|existing| existing.pitch == note.pitch) {
                    insert.push(MidiNoteInsert {
                        start: BeatTime::from_raw(cursor),
                        duration: BeatTime::from_raw(duration),
                        pitch: note.pitch,
                        channel,
                        velocity: note.velocity,
                    });
                }
            }
        }
        insert.sort_by_key(|note| note.pitch);
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
