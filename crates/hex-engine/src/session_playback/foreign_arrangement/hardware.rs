//! Ordered hardware MIDI at one event per rendered sample. A quantum carries
//! at most 32 hardware events, in addition to its authored and physical keys.
//! The render transaction advances only the prefix that it staged.
use super::{Input, next_note_id};
use crate::computer_keyboard::{LiveNoteActivation, LiveNoteBatch, LiveNoteEvent};
use crate::mutable_instrument::MutableInstrumentRuntimeError;
use hex_lattice::{ForeignNote, ForeignNoteKind};
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
    sustained: bool,
}

impl Gate {
    fn event(self, kind: ForeignNoteKind, value: u8) -> ForeignNote {
        ForeignNote {
            frame: 0,
            channel: self.channel,
            pitch: self.pitch,
            value,
            kind,
            note_id: Some(self.id),
            is_live: true,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Gates {
    activation: Option<LiveNoteActivation>,
    notes: [Option<Gate>; NOTE_BUDGET],
    held: usize,
    remaining: usize,
    sustain_channels: u16,
}

impl Default for Gates {
    fn default() -> Self {
        Self {
            activation: None,
            notes: [None; NOTE_BUDGET],
            held: 0,
            remaining: NOTE_BUDGET,
            sustain_channels: 0,
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
        self.sustain_channels = 0;
        self.remaining -= self.held;
        self.held = 0;
        for gate in &mut self.notes {
            if let Some(gate) = gate.take() {
                events.push(gate.event(ForeignNoteKind::Off, 0));
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

        let (channel, pitch, value, kind) = match event {
            LiveNoteEvent::NoteOn {
                channel,
                pitch,
                velocity,
            } => (channel, pitch, velocity, ForeignNoteKind::On),
            LiveNoteEvent::NoteOff {
                channel,
                pitch,
                velocity,
            } => (channel, pitch, velocity, ForeignNoteKind::Off),
            LiveNoteEvent::PolyPressure {
                channel,
                pitch,
                value,
            } => (channel, pitch, value, ForeignNoteKind::PolyPressure),
            LiveNoteEvent::ControlChange { channel, cc, value } => {
                if cc == 64 && value >= 64 {
                    self.sustain_channels |= 1_u16 << channel;
                    batch.consumed += 1;
                    return Ok(());
                }
                if (cc == 64 && value < 64) || cc == 121 {
                    let sustained_count = self
                        .notes
                        .iter()
                        .flatten()
                        .filter(|gate| gate.channel == channel && gate.sustained)
                        .count();
                    if sustained_count > self.remaining {
                        return Ok(());
                    }
                    self.sustain_channels &= !(1_u16 << channel);
                    self.remaining -= sustained_count;
                    self.held -= sustained_count;
                    for slot in &mut self.notes {
                        if slot.is_some_and(|gate| gate.channel == channel && gate.sustained)
                            && let Some(gate) = slot.take()
                        {
                            events.push(gate.event(ForeignNoteKind::Off, 0));
                        }
                    }
                    batch.consumed += 1;
                    return Ok(());
                }
                batch.consumed += 1;
                return Ok(());
            }
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
        if kind == ForeignNoteKind::PolyPressure {
            if let Some(gate) = previous.and_then(|index| self.notes[index]) {
                if self.remaining == 0 {
                    return Ok(());
                }
                events.push(gate.event(kind, value));
                self.remaining -= 1;
            }
            batch.consumed += 1;
            return Ok(());
        }
        let on = kind == ForeignNoteKind::On;
        if !on && (self.sustain_channels & (1_u16 << channel)) != 0 {
            if let Some(gate) = previous.and_then(|index| self.notes[index].as_mut()) {
                gate.sustained = true;
            }
            batch.consumed += 1;
            return Ok(());
        }
        let cost = usize::from(previous.is_some()) + usize::from(on);
        if cost > self.remaining {
            return Ok(());
        }
        if let Some(gate) = previous.and_then(|index| self.notes[index].take()) {
            events.push(gate.event(ForeignNoteKind::Off, if on { 0 } else { value }));
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
                sustained: false,
            };
            *slot = Some(gate);
            self.held += 1;
            events.push(gate.event(ForeignNoteKind::On, value));
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
    fn a_full_quantum_defers_pressure_and_releases_without_acknowledging_them() {
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
        let pressure = LiveNoteEvent::PolyPressure {
            channel: 0,
            pitch: 0,
            value: 37,
        };
        queue.publish(activation, pressure).unwrap();
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
        assert!(
            events
                .iter()
                .all(|event| event.kind == ForeignNoteKind::On && event.is_live)
        );
        assert_eq!(
            queue.snapshot().unwrap().events(),
            &[pressure, off, LiveNoteEvent::AllNotesOff]
        );

        let expected = events.iter().map(|event| event.note_id).collect::<Vec<_>>();
        events.clear();
        gates.reset_budget();
        batch.begin(Some(&input));
        for _ in 0..3 {
            gates
                .stage(Some(&input), &mut batch, &mut id, &mut events)
                .unwrap();
        }
        batch.commit(Some(&input));
        assert_eq!(
            queue.snapshot().unwrap().events(),
            &[LiveNoteEvent::AllNotesOff]
        );
        let [pressed, released] = events.as_slice() else {
            panic!("expected pressure and release");
        };
        assert_eq!(
            (pressed.kind, pressed.value, pressed.note_id),
            (ForeignNoteKind::PolyPressure, 37, expected[0])
        );
        assert!(pressed.is_live);
        assert_eq!(
            (released.kind, released.value, released.note_id),
            (ForeignNoteKind::Off, 11, expected[0])
        );
        assert!(released.is_live);
        events.clear();
        gates.reset_budget();
        batch.begin(Some(&input));
        gates
            .stage(Some(&input), &mut batch, &mut id, &mut events)
            .unwrap();
        batch.commit(Some(&input));
        assert!(queue.snapshot().unwrap().is_empty());
        assert!(!gates.held());
        assert!(
            events
                .iter()
                .all(|event| event.kind == ForeignNoteKind::Off && event.is_live)
        );
        assert_eq!(
            events.iter().map(|event| event.note_id).collect::<Vec<_>>(),
            expected[1..]
        );
    }

    #[test]
    fn sustain_pedal_holds_hosted_vst3_gates_across_note_off_until_pedal_release() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "vst3-sustain".into(),
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

        // Hold pitch 60 and 64 on ch 0, press CC64 sustain, release pitch 60, send CC11 expression.
        queue
            .publish_at(
                activation,
                LiveNoteEvent::NoteOn {
                    channel: 0,
                    pitch: 60,
                    velocity: 100,
                },
                1_000,
            )
            .unwrap();
        queue
            .publish_at(
                activation,
                LiveNoteEvent::NoteOn {
                    channel: 0,
                    pitch: 64,
                    velocity: 90,
                },
                2_000,
            )
            .unwrap();
        queue
            .publish_at(
                activation,
                LiveNoteEvent::ControlChange {
                    channel: 0,
                    cc: 64,
                    value: 127,
                },
                3_000,
            )
            .unwrap();
        queue
            .publish_at(
                activation,
                LiveNoteEvent::NoteOff {
                    channel: 0,
                    pitch: 60,
                    velocity: 0,
                },
                4_000,
            )
            .unwrap();
        queue
            .publish_at(
                activation,
                LiveNoteEvent::ControlChange {
                    channel: 0,
                    cc: 11,
                    value: 80,
                },
                5_000,
            )
            .unwrap();

        let mut gates = Gates::default();
        let mut batch = Batch::default();
        let mut events = Vec::with_capacity(NOTE_BUDGET);
        let mut id = 0;
        batch.begin(Some(&input));
        for _ in 0..5 {
            gates
                .stage(Some(&input), &mut batch, &mut id, &mut events)
                .unwrap();
        }
        batch.commit(Some(&input));

        // Only the two NoteOns should have fired; NoteOff for 60 is held by CC64.
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.kind == ForeignNoteKind::On));
        let id_60 = events[0].note_id;
        let id_64 = events[1].note_id;
        assert!(gates.held());

        // Release CC64 sustain pedal: pitch 60 releases, while still-held pitch 64 stays sounding.
        events.clear();
        gates.reset_budget();
        queue
            .publish_at(
                activation,
                LiveNoteEvent::ControlChange {
                    channel: 0,
                    cc: 64,
                    value: 0,
                },
                6_000,
            )
            .unwrap();
        batch.begin(Some(&input));
        gates
            .stage(Some(&input), &mut batch, &mut id, &mut events)
            .unwrap();
        batch.commit(Some(&input));
        assert_eq!(events.len(), 1);
        assert_eq!(
            (events[0].kind, events[0].pitch, events[0].note_id),
            (ForeignNoteKind::Off, 60, id_60)
        );
        assert!(gates.held());

        // Now release pitch 64 normally without sustain.
        events.clear();
        gates.reset_budget();
        queue
            .publish_at(
                activation,
                LiveNoteEvent::NoteOff {
                    channel: 0,
                    pitch: 64,
                    velocity: 32,
                },
                7_000,
            )
            .unwrap();
        batch.begin(Some(&input));
        gates
            .stage(Some(&input), &mut batch, &mut id, &mut events)
            .unwrap();
        batch.commit(Some(&input));
        assert_eq!(events.len(), 1);
        assert_eq!(
            (
                events[0].kind,
                events[0].pitch,
                events[0].value,
                events[0].note_id
            ),
            (ForeignNoteKind::Off, 64, 32, id_64)
        );
        assert!(!gates.held());
    }
}
