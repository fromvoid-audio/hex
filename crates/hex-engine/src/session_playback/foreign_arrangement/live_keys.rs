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
}

impl Gates {
    pub(super) fn held(&self) -> bool {
        self.keys.iter().any(Option::is_some)
    }

    pub(super) fn disconnect(&mut self) {
        self.epoch = None;
    }

    pub(super) fn stage(
        &mut self,
        input: Option<&Input>,
        next_id: &mut u64,
        events: &mut Vec<ForeignNote>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let snapshot = input.map_or(NoteSnapshot::Inactive, |input| input.root.note_snapshot());
        let (epoch, words) = match snapshot {
            NoteSnapshot::Active(epoch, words) => (Some(epoch), words),
            NoteSnapshot::Deferred(epoch) if self.epoch == Some(epoch) => return Ok(()),
            _ => (None, [0; COMPUTER_KEY_COUNT]),
        };
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
                let id = next_id
                    .checked_add(1)
                    .and_then(|id| i32::try_from(id).ok())
                    .and_then(|id| NonZeroU32::new(id as u32))
                    .ok_or(MutableInstrumentRuntimeError::IdentityExhausted)?;
                *next_id = u64::from(id.get());
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
