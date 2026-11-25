//! Exact note, per-key expression, and control-change messages from the existing byte
//! decoder. This is an input contract, not a scheduler, audio lease or recording authority.
use crate::HeldNoteFault;

/// One validated MIDI 1.0 Control Change message (`0xBn cc value`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlChange {
    pub channel: u8,
    pub cc: u8,
    pub value: u8,
}

impl ControlChange {
    pub const MOD_WHEEL: u8 = 1;
    pub const EXPRESSION: u8 = 11;
    pub const SUSTAIN: u8 = 64;
    /// MPE per-note Slide / Timbre (CC74, brightness/filter cutoff).
    pub const TIMBRE: u8 = 74;
    pub const RESET_ALL_CONTROLLERS: u8 = 121;

    pub fn new(channel: u8, cc: u8, value: u8) -> Option<Self> {
        (channel <= 15 && cc <= 127 && value <= 127).then_some(Self { channel, cc, value })
    }

    pub fn is_sustain_down(self) -> bool {
        self.cc == Self::SUSTAIN && self.value >= 64
    }
}

/// Messages retain wire order, pitch/controller, channel and seven-bit value.
/// Zero-velocity NoteOn has NoteOff semantics; release velocity is preserved.
/// ControlChange preserves Sustain Pedal (CC64), Expression (CC11), Mod Wheel (CC1),
/// Reset All Controllers (CC121), and all general MIDI CCs in wire order.
/// AllNotesOff represents channel-mode note-release messages (CC120 and CC123–127),
/// MIDI reset, and dropped burst input. InputLost (a held-note fault) requires
/// terminal host cancellation outside any bounded event queue, so overflow cannot stick notes.
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
    /// Polyphonic key pressure for one channel and pitch.
    PolyPressure {
        channel: u8,
        pitch: u8,
        value: u8,
    },
    /// Per-note pitch bend offset in cents (`-4800..=4800`) attributed to one
    /// active note on an MPE member channel.
    NotePitchBend {
        channel: u8,
        pitch: u8,
        cents: i16,
    },
    /// Per-note slide / timbre (`0..=127`, CC74) attributed to one active note
    /// on an MPE member channel.
    NoteTimbre {
        channel: u8,
        pitch: u8,
        value: u8,
    },
    /// Channel Control Change (including CC64 Sustain, CC11 Expression, and general CCs).
    ControlChange {
        channel: u8,
        cc: u8,
        value: u8,
    },
    AllNotesOff,
    InputLost(HeldNoteFault),
}

/// MIDI 1.0 channel-mode controllers that release all sounding notes on the
/// selected channel: All Sound Off (120), All Notes Off (123), Omni Off/On
/// (124/125), Mono On (126), and Poly On (127).
pub(crate) fn is_all_notes_off_cc(controller: u8) -> bool {
    matches!(controller, 120 | 123..=127)
}

impl NoteEvent {
    pub(crate) fn parse_filtered(message: &[u8], filter: crate::ChannelFilter) -> Option<Self> {
        let &[status, pitch, velocity] = message else {
            return None;
        };
        let channel = filter.matches(status)?;
        if pitch > 127 || velocity > 127 {
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
            0xa0 => Some(Self::PolyPressure {
                channel,
                pitch,
                value: velocity,
            }),
            0xb0 if is_all_notes_off_cc(pitch) => Some(Self::AllNotesOff),
            0xb0 => Some(Self::ControlChange {
                channel,
                cc: pitch,
                value: velocity,
            }),
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
        // The other channel is filtered out; selected-channel CC preserves wire order.
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
                NoteEvent::ControlChange {
                    channel: 2,
                    cc: 1,
                    value: 80
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
    fn poly_pressure_keeps_wire_order_without_changing_held_notes_or_channel_pressure() {
        let mut decoder = Decoder::default();
        let held = HeldNoteInput::default();
        let events = RefCell::new(Vec::new());
        let controllers = RefCell::new(Vec::new());
        let last = deliver_packets_with_notes(
            &mut decoder,
            2,
            Controllers::default(),
            [
                &[0x92, 60, 101, 64, 90, 0xa2, 60][..],
                &[0xf8, 127, 64, 0, 0xa1, 60, 100, 0xd2, 23, 0x82, 60, 11][..],
            ],
            &held,
            &|snapshot| controllers.borrow_mut().push(snapshot),
            &|event| events.borrow_mut().push(event),
        );
        assert_eq!(last.pressure, 23);
        assert_eq!(*controllers.borrow(), vec![last]);
        assert_eq!(
            *events.borrow(),
            [
                NoteEvent::On {
                    channel: 2,
                    pitch: 60,
                    velocity: 101
                },
                NoteEvent::On {
                    channel: 2,
                    pitch: 64,
                    velocity: 90
                },
                NoteEvent::PolyPressure {
                    channel: 2,
                    pitch: 60,
                    value: 127
                },
                NoteEvent::PolyPressure {
                    channel: 2,
                    pitch: 64,
                    value: 0
                },
                NoteEvent::Off {
                    channel: 2,
                    pitch: 60,
                    velocity: 11
                },
            ]
        );
        let HeldNotesRead::Snapshot(snapshot) = held.read() else {
            panic!("expected snapshot");
        };
        assert_eq!(snapshot.through(), 2);
        assert_eq!(
            snapshot
                .after(0)
                .map(|note| (note.pitch, note.velocity))
                .collect::<Vec<_>>(),
            [(64, 90)]
        );
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
    fn channel_mode_messages_release_held_notes_on_selected_channel_only() {
        for cc in [120u8, 123, 124, 125, 126, 127] {
            let mut decoder = Decoder::default();
            let held = HeldNoteInput::default();
            let events = RefCell::new(Vec::new());
            // Mode message on another channel must leave selected channel held notes intact;
            // the same mode message on the selected channel clears held notes and emits AllNotesOff.
            let mode_value = if cc == 126 { 4 } else { 0 };
            deliver_packets_with_notes(
                &mut decoder,
                2,
                Controllers::default(),
                [&[0x92, 60, 100, 0xb1, cc, mode_value][..]],
                &held,
                &|_| {},
                &|event| events.borrow_mut().push(event),
            );
            let HeldNotesRead::Snapshot(after_other_channel) = held.read() else {
                panic!("expected snapshot for other-channel CC {cc}")
            };
            assert_eq!(
                after_other_channel
                    .after(0)
                    .map(|note| (note.pitch, note.velocity))
                    .collect::<Vec<_>>(),
                [(60, 100)],
                "other-channel CC {cc} must not release selected channel"
            );
            assert_eq!(
                *events.borrow(),
                vec![NoteEvent::On {
                    channel: 2,
                    pitch: 60,
                    velocity: 100,
                }],
                "other-channel CC {cc} must not emit AllNotesOff"
            );

            deliver_packets_with_notes(
                &mut decoder,
                2,
                Controllers::default(),
                [&[0xb2, cc, mode_value][..]],
                &held,
                &|_| {},
                &|event| events.borrow_mut().push(event),
            );
            let HeldNotesRead::Snapshot(after_selected_channel) = held.read() else {
                panic!("expected snapshot after selected-channel CC {cc}")
            };
            assert_eq!(
                after_selected_channel.after(0).count(),
                0,
                "selected-channel CC {cc} must clear held notes"
            );

            deliver_packets_with_notes(
                &mut decoder,
                2,
                Controllers::default(),
                [&[0x92, 64, 90][..]],
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
                        velocity: 100,
                    },
                    NoteEvent::AllNotesOff,
                    NoteEvent::On {
                        channel: 2,
                        pitch: 64,
                        velocity: 90,
                    },
                ],
                "selected-channel CC {cc} must emit ordered AllNotesOff before next Note On"
            );
        }
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

    #[test]
    fn sustain_expression_and_general_cc_deliver_in_wire_order_with_monotonic_timestamps() {
        use crate::{
            MachTimebase, MonotonicTimestampClock, TimedPacket, TimestampedNoteEvent,
            deliver_timed_packets_with_notes,
        };
        let mut decoder = Decoder::default();
        let held = HeldNoteInput::default();
        let mut clock = MonotonicTimestampClock::default();
        let timebase = MachTimebase::APPLE_SILICON_24MHZ;
        let events = RefCell::new(Vec::new());
        let controllers = RefCell::new(Vec::new());

        // Packet 1 (t=24_000 ticks = 1_000_000 ns): NoteOn 60, Sustain Down (CC64=127), Expression (CC11=96).
        // Packet 2 (t=12_000 ticks, slightly regressive -> clamps to 1_000_000 ns): NoteOff 60, general CC74=80.
        // Packet 3 (t=48_000 ticks = 2_000_000 ns): Sustain Up (CC64=0), other-channel CC64 ignored.
        let last = deliver_timed_packets_with_notes(
            &mut decoder,
            2,
            Controllers::default(),
            timebase,
            &mut clock,
            [
                TimedPacket {
                    timestamp_ticks: 24_000,
                    data: &[0x92, 60, 100, 0xb2, 64, 127, 11, 96],
                },
                TimedPacket {
                    timestamp_ticks: 12_000,
                    data: &[0x82, 60, 0, 0xb2, 74, 80],
                },
                TimedPacket {
                    timestamp_ticks: 48_000,
                    data: &[0xb1, 64, 127, 0xb2, 64, 0],
                },
            ],
            &held,
            &|snapshot| controllers.borrow_mut().push(snapshot),
            &|timed| events.borrow_mut().push(timed),
        );
        assert_eq!(last.sustain, 0);
        assert_eq!(last.expression, 96);
        assert_eq!(
            last.last_cc,
            Some(ControlChange {
                channel: 2,
                cc: 64,
                value: 0,
            })
        );
        assert_eq!(
            *events.borrow(),
            vec![
                TimestampedNoteEvent {
                    timestamp_ns: 1_000_000,
                    event: NoteEvent::On {
                        channel: 2,
                        pitch: 60,
                        velocity: 100,
                    },
                },
                TimestampedNoteEvent {
                    timestamp_ns: 1_000_000,
                    event: NoteEvent::ControlChange {
                        channel: 2,
                        cc: 64,
                        value: 127,
                    },
                },
                TimestampedNoteEvent {
                    timestamp_ns: 1_000_000,
                    event: NoteEvent::ControlChange {
                        channel: 2,
                        cc: 11,
                        value: 96,
                    },
                },
                TimestampedNoteEvent {
                    timestamp_ns: 1_000_000,
                    event: NoteEvent::Off {
                        channel: 2,
                        pitch: 60,
                        velocity: 0,
                    },
                },
                TimestampedNoteEvent {
                    timestamp_ns: 1_000_000,
                    event: NoteEvent::ControlChange {
                        channel: 2,
                        cc: 74,
                        value: 80,
                    },
                },
                TimestampedNoteEvent {
                    timestamp_ns: 2_000_000,
                    event: NoteEvent::ControlChange {
                        channel: 2,
                        cc: 64,
                        value: 0,
                    },
                },
            ]
        );
        // Verify monotonic ordering under dense burst traffic.
        let burst_events = RefCell::new(Vec::new());
        let payloads: Vec<[u8; 3]> = (0..crate::CALLBACK_PACKETS + 1)
            .map(|i| [0xb2, 74, (i % 128) as u8])
            .collect();
        let packets: Vec<_> = payloads
            .iter()
            .enumerate()
            .map(|(i, data)| TimedPacket {
                timestamp_ticks: 48_000 + (i as u64) * 24,
                data: data.as_slice(),
            })
            .collect();
        deliver_timed_packets_with_notes(
            &mut decoder,
            2,
            last,
            timebase,
            &mut clock,
            packets,
            &held,
            &|_| {},
            &|timed| burst_events.borrow_mut().push(timed),
        );
        let recorded = burst_events.borrow();
        assert_eq!(recorded.len(), crate::CALLBACK_PACKETS + 1);
        assert_eq!(recorded.last().unwrap().event, NoteEvent::AllNotesOff);
        assert!(
            recorded
                .windows(2)
                .all(|pair| pair[0].timestamp_ns <= pair[1].timestamp_ns),
            "timestamps must stay monotonic across burst traffic and overflow reset"
        );
    }
}
