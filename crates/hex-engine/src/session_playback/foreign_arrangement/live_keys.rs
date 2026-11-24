//! The selected physical key snapshot feeds the same copied worker as authored
//! MIDI. At most one snapshot per worker quantum admits 17 releases and 17
//! onsets. The owning ledger commits them with the accepted render.
use crate::computer_keyboard::{
    COMPUTER_KEY_COUNT, ComputerKeyboardInput, NoteSnapshot, note_audition::AuditionGate,
};
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
    audition: bool,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Gates {
    keys: [Option<Key>; COMPUTER_KEY_COUNT],
    epoch: Option<u64>,
    audition: AuditionGate,
}

impl Gates {
    pub(super) fn held(&self) -> bool {
        self.keys.iter().any(Option::is_some)
    }

    pub(super) fn disconnect(&mut self) {
        self.epoch = None;
        self.audition = AuditionGate::default();
    }

    /// Called once per rendered frame on scratch state. Offsets are quantized to
    /// the next worker quantum; rollback discards all accumulated gate time.
    pub(super) fn advance_audition(&mut self) {
        self.audition.advance(1);
    }

    pub(super) fn commit_audition(&self, input: &Input) {
        self.audition.commit(&input.root);
    }

    pub(super) fn stage(
        &mut self,
        input: Option<&Input>,
        next_id: &mut u64,
        events: &mut Vec<ForeignNote>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let snapshot = input.map_or(NoteSnapshot::Inactive, |input| input.root.note_snapshot());
        let (epoch, mut words) = match snapshot {
            NoteSnapshot::Active(epoch, words, request) => {
                self.audition
                    .observe(request, input.map(|input| input.root.as_ref()));
                (Some(epoch), words)
            }
            NoteSnapshot::Deferred(epoch) if self.epoch == Some(epoch) => {
                if !self.audition.expired() {
                    return Ok(());
                }
                (Some(epoch), [0; COMPUTER_KEY_COUNT])
            }
            _ => {
                self.audition = AuditionGate::default();
                (None, [0; COMPUTER_KEY_COUNT])
            }
        };
        let audition = self.audition.active();
        if self.audition.expired() {
            words.fill(0);
        }
        for (held, word) in self.keys.iter_mut().zip(words) {
            if let Some(key) = *held
                && (self.epoch != epoch || key.word != word || key.audition != audition)
            {
                events.push(key.event(false));
                *held = None;
            }
        }
        for (held, word) in self.keys.iter_mut().zip(words) {
            if held.is_none() && word != 0 {
                let id = super::next_note_id(next_id)?;
                let key = Key { word, id, audition };
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
            is_live: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer_keyboard::ComputerKeyboardInput;
    use std::sync::Arc;

    #[test]
    fn computer_keyboard_audition_press_and_release_carry_is_live() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("expected a MIDI track");
        };
        let input = Input {
            track,
            root: Arc::new(ComputerKeyboardInput::default()),
            detached: false,
        };
        let mut keys = Gates::default();
        let mut events = Vec::new();
        let mut next_id = 0;

        input.root.set_key(0, Some((60, 100, 1))).unwrap();
        keys.stage(Some(&input), &mut next_id, &mut events).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].on && events[0].is_live && events[0].pitch == 60);
        let press_id = events[0].note_id;

        events.clear();
        input.root.set_key(0, None).unwrap();
        keys.stage(Some(&input), &mut next_id, &mut events).unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].on && events[0].is_live && events[0].pitch == 60);
        assert_eq!(events[0].note_id, press_id);
    }
}
