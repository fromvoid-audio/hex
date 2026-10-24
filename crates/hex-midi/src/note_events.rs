//! Exact channel-note messages from the existing byte decoder. This is an input
//! contract, not a timestamp, scheduler, audio lease or recording authority.
use crate::HeldNoteFault;

/// Messages retain wire order, pitch, channel and the original MIDI velocity.
/// Zero-velocity NoteOn has NoteOff semantics; release velocity is preserved.
/// AllNotesOff also represents MIDI reset. InputLost requires terminal host
/// cancellation outside any bounded event queue, so overflow cannot stick notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteEvent {
    On {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    Off {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    AllNotesOff,
    InputLost(HeldNoteFault),
}

impl NoteEvent {
    pub(crate) fn parse(message: &[u8], channel: u8) -> Option<Self> {
        let &[status, pitch, velocity] = message else {
            return None;
        };
        if channel >= 16 || status & 15 != channel || pitch > 127 || velocity > 127 {
            return None;
        }
        match status & 0xf0 {
            0x90 if velocity != 0 => Some(Self::On {
                channel,
                pitch,
                velocity,
            }),
            0x80 | 0x90 => Some(Self::Off {
                channel,
                pitch,
                velocity,
            }),
            0xb0 if matches!(pitch, 120 | 123) => Some(Self::AllNotesOff),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Controllers, Decoder, HeldNoteInput, HeldNotesRead, deliver_packets_with_notes};
    use std::cell::RefCell;

    #[test]
    fn short_notes_survive_packet_coalescing_with_exact_channel_velocity_and_order() {
        let mut decoder = Decoder::default();
        let held = HeldNoteInput::default();
        let events = RefCell::new(Vec::new());
        // Running status spans packets; a realtime clock interleaves the note.
        // The other channel and controller messages cannot become note events.
        deliver_packets_with_notes(
            &mut decoder,
            2,
            Controllers::default(),
            [
                &[0x92, 60, 101, 64, 0xf8][..],
                &[79, 60, 0, 0x82, 64, 42, 0x91, 61, 100, 0xb2, 1, 80][..],
            ],
            &held,
            &|_| {},
            &|event| events.borrow_mut().push(event),
        );
        assert_eq!(
            *events.borrow(),
            vec![
                NoteEvent::On {
                    channel: 2,
                    pitch: 60,
                    velocity: 101
                },
                NoteEvent::On {
                    channel: 2,
                    pitch: 64,
                    velocity: 79
                },
                NoteEvent::Off {
                    channel: 2,
                    pitch: 60,
                    velocity: 0
                },
                NoteEvent::Off {
                    channel: 2,
                    pitch: 64,
                    velocity: 42
                },
            ]
        );
        let HeldNotesRead::Snapshot(snapshot) = held.read() else {
            panic!("expected snapshot")
        };
        assert_eq!(snapshot.after(0).count(), 0);
        assert_eq!(snapshot.through(), 2);
    }

    #[test]
    fn reset_and_channel_scoped_all_notes_off_preserve_wire_order() {
        let mut decoder = Decoder::default();
        let held = HeldNoteInput::default();
        let events = RefCell::new(Vec::new());
        deliver_packets_with_notes(
            &mut decoder,
            0,
            Controllers::default(),
            [&[0x90, 60, 1, 0xb1, 123, 0, 0xb0, 120, 0, 0xff, 60, 1][..]],
            &held,
            &|_| {},
            &|event| events.borrow_mut().push(event),
        );
        assert_eq!(
            *events.borrow(),
            vec![
                NoteEvent::On {
                    channel: 0,
                    pitch: 60,
                    velocity: 1
                },
                NoteEvent::AllNotesOff,
                NoteEvent::AllNotesOff,
            ]
        );
    }

    #[test]
    fn packet_loss_is_explicit_and_terminal_even_if_more_input_arrives() {
        let mut decoder = Decoder::default();
        let held = HeldNoteInput::default();
        let events = RefCell::new(Vec::new());
        let bytes = vec![0xf8; super::super::CALLBACK_BYTES];
        deliver_packets_with_notes(
            &mut decoder,
            0,
            Controllers::default(),
            [bytes.as_slice()],
            &held,
            &|_| {},
            &|event| events.borrow_mut().push(event),
        );
        deliver_packets_with_notes(
            &mut decoder,
            0,
            Controllers::default(),
            [&[0x90, 60, 127][..]],
            &held,
            &|_| {},
            &|event| events.borrow_mut().push(event),
        );
        assert!(held.is_cancelled());
        assert_eq!(
            *events.borrow(),
            vec![NoteEvent::InputLost(HeldNoteFault::PacketLimit)]
        );
    }
}
