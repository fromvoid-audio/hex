//! Copy payloads retain musical geometry; the project allocates fresh identities.
//! Canonical overwrite, source growth and history remain in the existing edit.
use super::{
    EditorView,
    midi_group_drag::{GroupNote, capture_selected_notes},
};
use hex_project::{BeatTime, Command, MidiNoteInsert};
use hex_session::service::Request;

pub(super) fn insertions(
    notes: &[GroupNote],
    delta: u64,
) -> Result<Vec<MidiNoteInsert>, &'static str> {
    if notes.is_empty() || notes.len() > hex_project::MAX_NOTES {
        return Err("invalid midi copy selection");
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(notes.len())
        .map_err(|_| "could not reserve midi copies")?;
    for note in notes {
        let start = note
            .start
            .checked_add(delta)
            .ok_or("midi copy exceeds beat range")?;
        start
            .checked_add(note.duration)
            .filter(|_| note.duration > 0)
            .ok_or("midi copy exceeds beat range")?;
        if note.pitch > 127 || note.channel > 15 || !(1..=127).contains(&note.velocity) {
            return Err("invalid midi copy note");
        }
        result.push(MidiNoteInsert {
            active: note.active,
            start: BeatTime::from_raw(start),
            duration: BeatTime::from_raw(note.duration),
            pitch: note.pitch,
            channel: note.channel,
            velocity: note.velocity,
        });
    }
    Ok(result)
}

fn keyboard_delta(notes: &[GroupNote], step: Option<u64>) -> Result<u64, &'static str> {
    let start = notes
        .iter()
        .map(|n| n.start)
        .min()
        .ok_or("select notes to duplicate")?;
    let end = notes
        .iter()
        .try_fold(0_u64, |end, note| {
            note.start
                .checked_add(note.duration)
                .map(|next| end.max(next))
        })
        .ok_or("midi copy exceeds beat range")?;
    let (start, end) = if let Some(step) = step {
        if step == 0 {
            return Err("invalid midi copy grid");
        }
        let end = end
            .checked_add(step - 1)
            .and_then(|n| (n / step).checked_mul(step))
            .ok_or("midi copy exceeds beat range")?;
        (start / step * step, end)
    } else {
        (start, end)
    };
    end.checked_sub(start)
        .filter(|delta| *delta > 0)
        .ok_or("invalid midi copy span")
}

impl EditorView {
    pub(super) fn duplicate_piano_notes(&mut self) {
        if self.edit_pending() {
            return;
        }
        if self.midi_selection.is_empty() {
            self.error = Some("select notes to duplicate".into());
            return;
        }
        let Some(clip) = self.detail_midi_clip_data() else {
            return;
        };
        let result = self
            .state
            .as_ref()
            .and_then(|state| state.project.midi_source(clip))
            .ok_or("midi copy source is unavailable")
            .and_then(|source| {
                let mut notes = capture_selected_notes(
                    source.notes().iter().map(|note| GroupNote {
                        id: note.id(),
                        active: note.active(),
                        start: note.start().raw(),
                        duration: note.duration().raw(),
                        pitch: note.pitch(),
                        channel: note.channel(),
                        velocity: note.velocity(),
                    }),
                    self.midi_selection.ids(),
                )?;
                notes.sort_unstable_by_key(|note| note.id);
                let delta = keyboard_delta(&notes, self.midi_snap_grid.step_raw())?;
                insertions(&notes, delta)
            });
        match result {
            Ok(insert) => {
                self.finish_live_midi_marquees();
                self.cancel_midi_note_gesture();
                self.enqueue(Request::Edit(Command::EditMidiNotesWithSourceExtent {
                    clip,
                    remove: Vec::new(),
                    insert,
                }));
            }
            Err(error) => self.error = Some(error.into()),
        }
    }
}
