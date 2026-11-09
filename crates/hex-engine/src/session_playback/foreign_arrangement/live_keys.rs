//! The selected physical key snapshot feeds the same copied worker as authored
//! MIDI. At most one snapshot per worker quantum admits 17 releases and 17
//! onsets. The owning ledger commits them with the accepted render.
use crate::computer_keyboard::{COMPUTER_KEY_COUNT, ComputerKeyboardInput, NoteSnapshot};
use crate::mutable_instrument::MutableInstrumentRuntimeError;
use hex_lattice::ForeignNote;
use hex_project::TrackId;
use std::{num::NonZeroU32, sync::Arc};

pub(super) struct Input {
    pub(super) track: TrackId,
    pub(super) root: Arc<ComputerKeyboardInput>,
    pub(super) detached: bool,
}

#[derive(Clone, Copy)]
struct Key {
    word: u64,
    id: NonZeroU32,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Gates {
    keys: [Option<Key>; COMPUTER_KEY_COUNT],
    epoch: Option<u64>,
    audition_age: u64,
    audition_gate_frames: u64,
    audition_started: bool,
    audition_finished: bool,
}

impl Gates {
    pub(super) fn held(&self) -> bool {
        self.keys.iter().any(Option::is_some)
    }

    pub(super) fn disconnect(&mut self) {
        self.epoch = None;
        self.audition_age = 0;
        self.audition_gate_frames = 0;
        self.audition_started = false;
        self.audition_finished = false;
    }

    /// Called once per rendered frame on scratch state. Offsets are quantized to
    /// the next worker quantum; rollback discards all accumulated gate time.
    pub(super) fn advance_audition(&mut self) {
        if self.audition_started && !self.audition_finished {
            self.audition_age = self.audition_age.saturating_add(1);
            self.audition_finished = self.audition_age >= self.audition_gate_frames;
        }
    }

    pub(super) fn commit_audition(&self, input: &Input) {
        if let Some((_, previously_started, _)) = input.root.foreign_audition_gate() {
            input.root.commit_foreign_audition_gate(
                self.audition_started,
                self.audition_finished || (previously_started && !self.audition_started),
            );
        }
    }

    pub(super) fn stage(
        &mut self,
        input: Option<&Input>,
        next_id: &mut u64,
        events: &mut Vec<ForeignNote>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let snapshot = input.map_or(NoteSnapshot::Inactive, |input| input.root.note_snapshot());
        let (epoch, mut words) = match snapshot {
            NoteSnapshot::Active(epoch, words) => (Some(epoch), words),
            NoteSnapshot::Deferred(epoch) if self.epoch == Some(epoch) => return Ok(()),
            _ => (None, [0; COMPUTER_KEY_COUNT]),
        };
        if let Some((gate_frames, started, finished)) =
            input.and_then(|input| input.root.foreign_audition_gate())
        {
            self.audition_gate_frames = gate_frames;
            if finished || self.audition_finished || (started && !self.audition_started) {
                words.fill(0);
            } else if words.iter().any(|word| *word != 0) {
                self.audition_started = true;
            }
        }
        for (held, word) in self.keys.iter_mut().zip(words) {
            if let Some(key) = *held
                && (self.epoch != epoch || key.word != word)
            {
                events.push(key.event(false));
                *held = None;
            }
        }
        for (held, word) in self.keys.iter_mut().zip(words) {
            if held.is_none() && word != 0 {
                let id = super::next_note_id(next_id)?;
                let key = Key { word, id };
                events.push(key.event(true));
                *held = Some(key);
            }
        }
        self.epoch = epoch;
        Ok(())
    }
}

impl Key {
    fn event(self, on: bool) -> ForeignNote {
        ForeignNote {
            frame: 0,
            channel: 0,
            // ComputerKeyboardInput only publishes words with pitch + 1 in 1..=128.
            pitch: (self.word as u8).saturating_sub(1),
            velocity: if on { (self.word >> 8) as u8 } else { 0 },
            on,
            note_id: Some(self.id),
        }
    }
}
