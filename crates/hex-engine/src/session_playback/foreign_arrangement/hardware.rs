//! Ordered hardware MIDI at one event per rendered sample. A quantum carries
//! at most 32 hardware events, in addition to its authored and physical keys.
//! The render transaction advances only the prefix that it staged.
use super::{Input, next_note_id};
use crate::computer_keyboard::{LiveNoteActivation, LiveNoteBatch, LiveNoteEvent};
use crate::mutable_instrument::MutableInstrumentRuntimeError;
use hex_lattice::ForeignNote;
use std::num::NonZeroU32;

pub(super) const NOTE_BUDGET: usize = 32;

#[derive(Default)]
pub(super) struct Batch {
    notes: Option<LiveNoteBatch>,
    consumed: usize,
}

impl Batch {
    pub(super) fn begin(&mut self, input: Option<&Input>) {
        self.notes = input
            .filter(|input| !input.detached)
            .and_then(|input| input.root.live_monitor_input())
            .and_then(|input| input.snapshot().ok());
        self.consumed = 0;
    }

    pub(super) fn has_events(&self) -> bool {
        self.notes
            .as_ref()
            .is_some_and(|notes| self.consumed < notes.len)
    }

    pub(super) fn commit(&self, input: Option<&Input>) {
        if self.consumed != 0
            && let Some((queue, notes)) = input
                .and_then(|input| input.root.live_monitor_input())
                .zip(self.notes.as_ref())
        {
            let _ = queue.commit(notes, self.consumed);
        }
    }
}

#[derive(Clone, Copy)]
struct Gate {
    channel: u8,
    pitch: u8,
    id: NonZeroU32,
}

impl Gate {
    fn event(self, on: bool, velocity: u8) -> ForeignNote {
        ForeignNote {
            frame: 0,
            channel: self.channel,
            pitch: self.pitch,
            velocity,
            on,
            note_id: Some(self.id),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Gates {
    activation: Option<LiveNoteActivation>,
    notes: [Option<Gate>; NOTE_BUDGET],
    held: usize,
    remaining: usize,
}

impl Default for Gates {
    fn default() -> Self {
        Self {
            activation: None,
            notes: [None; NOTE_BUDGET],
            held: 0,
            remaining: NOTE_BUDGET,
        }
    }
}

impl Gates {
    pub(super) fn held(&self) -> bool {
        self.held != 0
    }

    pub(super) fn disconnect(&mut self) {
        self.activation = None;
    }

    pub(super) fn reset_budget(&mut self) {
        self.remaining = NOTE_BUDGET;
    }

    pub(super) fn inherit_budget(&mut self, old: &Self) {
        self.remaining = old.remaining;
    }

    fn release_all(&mut self, events: &mut Vec<ForeignNote>) -> bool {
        if self.held > self.remaining {
            return false;
        }
        self.remaining -= self.held;
        self.held = 0;
        for gate in &mut self.notes {
            if let Some(gate) = gate.take() {
                events.push(gate.event(false, 0));
            }
        }
        true
    }

    pub(super) fn stage(
        &mut self,
        input: Option<&Input>,
        batch: &mut Batch,
        next_id: &mut u64,
        events: &mut Vec<ForeignNote>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let current = input
            .and_then(|input| input.root.live_monitor_input())
            .zip(batch.notes.as_ref())
            .filter(|(queue, notes)| queue.same_current_activation(notes.activation));
        let activation = current.map(|(_, notes)| notes.activation);
        if self.activation != activation || (activation.is_none() && self.held()) {
            if !self.release_all(events) {
                return Ok(());
            }
            self.activation = activation;
        }
        let Some((queue, notes)) = current else {
            return Ok(());
        };
        let Some(event) = notes.events().get(batch.consumed).copied() else {
            return Ok(());
        };

        let (channel, pitch, velocity, on) = match event {
            LiveNoteEvent::NoteOn {
                channel,
                pitch,
                velocity,
            } => (channel, pitch, velocity, true),
            LiveNoteEvent::NoteOff {
                channel,
                pitch,
                velocity,
            } => (channel, pitch, velocity, false),
            LiveNoteEvent::AllNotesOff => {
                if self.release_all(events) {
                    batch.consumed += 1;
                }
                return Ok(());
            }
        };
        let previous = self.notes.iter().position(|gate| {
            gate.is_some_and(|gate| gate.channel == channel && gate.pitch == pitch)
        });
        let cost = usize::from(previous.is_some()) + usize::from(on);
        if cost > self.remaining {
            return Ok(());
        }
        if let Some(gate) = previous.and_then(|index| self.notes[index].take()) {
            events.push(gate.event(false, if on { 0 } else { velocity }));
            self.held -= 1;
        }
        if on {
            let Some(slot) = self.notes.iter_mut().find(|slot| slot.is_none()) else {
                // A bounded refusal faults only this hardware activation. Its
                // existing gates are released now or at the next quantum.
                let _ = queue.fail_activation(notes.activation);
                self.release_all(events);
                return Ok(());
            };
            let gate = Gate {
                channel,
                pitch,
                id: next_note_id(next_id)?,
            };
            *slot = Some(gate);
            self.held += 1;
            events.push(gate.event(true, velocity));
        }
        self.remaining -= cost;
        batch.consumed += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer_keyboard::ComputerKeyboardInput;
    use std::sync::Arc;

    #[test]
    fn a_full_quantum_defers_releases_without_acknowledging_them() {
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
            root: Arc::new(ComputerKeyboardInput::default().with_live_notes().unwrap()),
            detached: false,
        };
        let queue = input.root.live_notes().unwrap();
        let activation = input.root.begin_live_notes_activation_on_control().unwrap();
        for pitch in 0..NOTE_BUDGET as u8 {
            queue
                .publish(
                    activation,
                    LiveNoteEvent::NoteOn {
                        channel: 0,
                        pitch,
                        velocity: 90,
                    },
                )
                .unwrap();
        }
        let off = LiveNoteEvent::NoteOff {
            channel: 0,
            pitch: 0,
            velocity: 11,
        };
        queue.publish(activation, off).unwrap();
        queue
            .publish(activation, LiveNoteEvent::AllNotesOff)
            .unwrap();
        let mut gates = Gates::default();
        let mut batch = Batch::default();
        let mut events = Vec::with_capacity(NOTE_BUDGET);
        let mut id = 0;
        batch.begin(Some(&input));
        for _ in 0..(NOTE_BUDGET + 2) {
            gates
                .stage(Some(&input), &mut batch, &mut id, &mut events)
                .unwrap();
        }
        batch.commit(Some(&input));
        assert_eq!(events.len(), NOTE_BUDGET);
        assert!(events.iter().all(|event| event.on));
        assert_eq!(
            queue.snapshot().unwrap().events(),
            &[off, LiveNoteEvent::AllNotesOff]
        );

        let expected = events.iter().map(|event| event.note_id).collect::<Vec<_>>();
        events.clear();
        gates.reset_budget();
        batch.begin(Some(&input));
        for _ in 0..2 {
            gates
                .stage(Some(&input), &mut batch, &mut id, &mut events)
                .unwrap();
        }
        batch.commit(Some(&input));
        assert!(queue.snapshot().unwrap().is_empty());
        assert!(!gates.held());
        assert!(events.iter().all(|event| !event.on));
        assert_eq!(events[0].velocity, 11);
        assert_eq!(
            events.iter().map(|event| event.note_id).collect::<Vec<_>>(),
            expected
        );
    }
}
