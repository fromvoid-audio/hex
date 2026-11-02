//! Exact channel-note messages from the existing byte decoder. This is an input
//! contract, not a timestamp, scheduler, audio lease or recording authority.
use crate::HeldNoteFault;

/// Messages retain wire order, pitch, channel and the original MIDI velocity.
/// Zero-velocity NoteOn has NoteOff semantics; release velocity is preserved.
/// AllNotesOff also represents MIDI reset and dropped burst input. InputLost
/// (a held-note fault) requires terminal host
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

    fn deliver<'a>(
        decoder: &mut Decoder,
        held: &HeldNoteInput,
        events: &RefCell<Vec<NoteEvent>>,
        packets: impl IntoIterator<Item = &'a [u8]>,
    ) {
        deliver_packets_with_notes(
            decoder,
            0,
            Controllers::default(),
            packets,
            held,
            &|_| {},
            &|event| events.borrow_mut().push(event),
        );
    }

    #[test]
    fn exact_packet_and_byte_budgets_deliver_without_loss() {
        let mut decoder = Decoder::default();
        let held = HeldNoteInput::default();
        let events = RefCell::new(Vec::new());
        // The final permitted packet carries the note; nothing exceeds the limit.
        let mut packets = vec![&[0xf8][..]; crate::CALLBACK_PACKETS - 1];
        packets.push(&[0x90, 60, 100]);
        deliver(&mut decoder, &held, &events, packets);
        // A single packet that exactly fills the byte budget ends with a note.
        let mut bytes = vec![0xf8; crate::CALLBACK_BYTES - 3];
        bytes.extend([0x90, 62, 90]);
        deliver(&mut decoder, &held, &events, [bytes.as_slice()]);
        assert!(!held.is_cancelled());
        assert_eq!(
            *events.borrow(),
            vec![
                NoteEvent::On {
                    channel: 0,
                    pitch: 60,
                    velocity: 100
                },
                NoteEvent::On {
                    channel: 0,
                    pitch: 62,
                    velocity: 90
                },
            ]
        );
        let HeldNotesRead::Snapshot(snapshot) = held.read() else {
            panic!("expected snapshot")
        };
        assert_eq!(
            snapshot.after(0).map(|n| n.pitch).collect::<Vec<_>>(),
            [60, 62]
        );
    }

    #[test]
    fn dense_burst_releases_held_notes_and_input_keeps_working() {
        for mut burst in [
            vec![vec![0xf8]; crate::CALLBACK_PACKETS + 1],
            vec![vec![0xf8; crate::CALLBACK_BYTES + 1]],
        ] {
            let mut decoder = Decoder::default();
            let held = HeldNoteInput::default();
            let events = RefCell::new(Vec::new());
            deliver(&mut decoder, &held, &events, [&[0x90, 60, 100][..]]);
            // The excess carries a running-status fragment; it must not
            // combine with the next call's bytes into a phantom note.
            if let Some(packet) = burst.last_mut() {
                packet.push(0x90);
            }
            deliver(
                &mut decoder,
                &held,
                &events,
                burst.iter().map(Vec::as_slice),
            );
            let HeldNotesRead::Snapshot(released) = held.read() else {
                panic!("expected snapshot after a burst")
            };
            assert_eq!(released.after(0).count(), 0);
            deliver(&mut decoder, &held, &events, [&[64, 70, 0x90, 67, 80][..]]);
            assert!(!held.is_cancelled());
            assert_eq!(
                *events.borrow(),
                vec![
                    NoteEvent::On {
                        channel: 0,
                        pitch: 60,
                        velocity: 100
                    },
                    NoteEvent::AllNotesOff,
                    NoteEvent::On {
                        channel: 0,
                        pitch: 67,
                        velocity: 80
                    },
                ]
            );
            let HeldNotesRead::Snapshot(snapshot) = held.read() else {
                panic!("expected snapshot")
            };
            // Tokens stay monotonic so a control-side floor cannot hide new keys.
            assert_eq!(
                snapshot
                    .after(released.through())
                    .map(|n| n.pitch)
                    .collect::<Vec<_>>(),
                [67]
            );
        }
    }
}
