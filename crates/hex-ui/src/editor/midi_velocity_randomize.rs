//! Selected-note velocity variation. The UI captures a seed once; the existing
//! validated project batch owns the result and history, so redo never rerolls.
use super::{
    EditorView,
    midi_group_drag::{GroupNote, capture_selected_notes},
};
use hex_project::{BeatTime, ClipId, Command, MidiNoteUpdate};
use hex_session::service::Request;

pub(super) struct State {
    pub(super) amount: u8,
    seed: u64,
}
impl Default for State {
    fn default() -> Self {
        Self {
            amount: 16,
            seed: 0,
        }
    }
}
#[derive(Clone, Copy)]
pub(super) struct Context {
    generation: u64,
    revision: u64,
    clip: ClipId,
    selection_epoch: u64,
    amount: u8,
}
impl EditorView {
    pub(super) fn velocity_random_context(&self) -> Option<Context> {
        Some(Context {
            generation: self.project_generation,
            revision: self.state.as_ref()?.revision,
            clip: self.detail_midi_clip_data()?,
            selection_epoch: self.midi_selection.epoch(),
            amount: self.midi_velocity_randomize.amount,
        })
    }
    pub(super) fn randomize_selected_velocities(&mut self, captured: Context) {
        if self.edit_pending()
            || self.project_generation != captured.generation
            || self
                .state
                .as_ref()
                .is_none_or(|s| s.revision != captured.revision)
            || self.detail_midi_clip_data() != Some(captured.clip)
            || self.midi_selection.epoch() != captured.selection_epoch
            || self.midi_velocity_randomize.amount != captured.amount
            || self.midi_selection.is_empty()
        {
            return;
        }
        let Some(seed) = self.midi_velocity_randomize.seed.checked_add(1) else {
            self.error = Some("velocity randomizer seed exhausted".into());
            return;
        };
        let result = self
            .midi_clip()
            .ok_or("selected midi source is unavailable")
            .and_then(|source| {
                let originals = capture_selected_notes(
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
                randomized(
                    &originals,
                    source.source_length().raw(),
                    captured.amount,
                    seed,
                )
            });
        match result {
            Ok(Some(notes)) => {
                self.finish_live_midi_marquees();
                self.cancel_midi_note_gesture();
                if self.enqueue(Request::Edit(Command::UpdateMidiNotes {
                    clip: captured.clip,
                    notes,
                })) {
                    self.midi_velocity_randomize.seed = seed;
                }
            }
            Ok(None) => self.midi_velocity_randomize.seed = seed,
            Err(error) => self.error = Some(error.into()),
        }
    }
}

fn randomized(
    originals: &[GroupNote],
    length: u64,
    amount: u8,
    seed: u64,
) -> Result<Option<Vec<MidiNoteUpdate>>, &'static str> {
    if amount > 126
        || originals.is_empty()
        || originals.len() > hex_project::MAX_NOTES
        || length == 0
    {
        return Err("invalid velocity randomizer input");
    }
    let mut ordered = Vec::new();
    ordered
        .try_reserve_exact(originals.len())
        .map_err(|_| "could not reserve velocity randomizer")?;
    ordered.extend_from_slice(originals);
    ordered.sort_unstable_by_key(|note| note.id);
    if ordered.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err("duplicate velocity randomizer note");
    }
    let mut updates = Vec::new();
    updates
        .try_reserve_exact(ordered.len())
        .map_err(|_| "could not reserve randomized notes")?;
    let mut entropy = seed;
    let mut changed = false;
    for note in ordered {
        if note.pitch > 127
            || note.channel > 15
            || !(1..=127).contains(&note.velocity)
            || note.duration == 0
            || note
                .start
                .checked_add(note.duration)
                .is_none_or(|end| end > length)
        {
            return Err("invalid velocity randomizer note");
        }
        // SplitMix64, local to this action. Sample the legal interval directly;
        // clamping a wider draw would pile up results at velocity 1 and 127.
        entropy = entropy.wrapping_add(0x9e3779b97f4a7c15);
        let mut word = entropy;
        word = (word ^ (word >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        word = (word ^ (word >> 27)).wrapping_mul(0x94d049bb133111eb);
        word ^= word >> 31;
        let low = note.velocity.saturating_sub(amount).max(1);
        let high = note.velocity.saturating_add(amount).min(127);
        let count = u64::from(high - low) + 1;
        let velocity = low + ((u128::from(word) * u128::from(count)) >> 64) as u8;
        changed |= velocity != note.velocity;
        updates.push(MidiNoteUpdate {
            note: note.id,
            start: BeatTime::from_raw(note.start),
            duration: BeatTime::from_raw(note.duration),
            pitch: note.pitch,
            channel: note.channel,
            velocity,
        });
    }
    Ok(changed.then_some(updates))
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_velocity_randomize_ui_tests.rs"]
mod tests;
