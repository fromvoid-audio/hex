//! Bounded MIDI 1.0 byte-stream framing, channel filtering, controller snapshots,
//! and packet delivery shared across single-port and multi-port inputs.

use crate::held_notes::{HELD_NOTE_COUNT, HeldNoteFault, HeldNoteInput, HeldNotes};
use crate::mpe::{MpeDecoder, MpeExpressionEvent};
use crate::note_events::{self, ControlChange, NoteEvent};
use crate::timebase::{MachTimebase, MonotonicTimestampClock, TimedPacket, TimestampedNoteEvent};

pub(crate) const CALLBACK_BYTES: usize = 65_536;
pub(crate) const CALLBACK_PACKETS: usize = 1024;

/// Channel filter applied to decoded MIDI 1.0 channel-voice messages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChannelFilter {
    /// Accept all 16 MIDI channels (`0..=15`), retaining each message's wire channel.
    #[default]
    Omni,
    /// Accept only one zero-based MIDI channel (`0..=15`).
    Single(u8),
}

impl ChannelFilter {
    /// Construct a validated single-channel filter (`0..=15`).
    pub const fn single(channel: u8) -> Option<Self> {
        if channel < 16 {
            Some(Self::Single(channel))
        } else {
            None
        }
    }

    /// Construct from an optional single channel (`None` => `Omni`, `Some(0..=15)` => `Single`).
    pub const fn from_option(channel: Option<u8>) -> Option<Self> {
        match channel {
            None => Some(Self::Omni),
            Some(ch) => Self::single(ch),
        }
    }

    /// True when the filter configuration itself is valid (`Omni` or `Single(0..=15)`).
    pub const fn is_valid(self) -> bool {
        match self {
            Self::Omni => true,
            Self::Single(channel) => channel < 16,
        }
    }

    /// Return the zero-based wire channel when `status` matches this filter.
    pub(crate) const fn matches(self, status: u8) -> Option<u8> {
        let wire_channel = status & 15;
        match self {
            Self::Omni => Some(wire_channel),
            Self::Single(channel) if channel < 16 && wire_channel == channel => Some(wire_channel),
            Self::Single(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controllers {
    pub pressure: u8,
    pub bend: i16,
    pub wheel: u8,
    /// Sustain Pedal (CC64) 7-bit controller value (`>= 64` means pedal down).
    pub sustain: u8,
    /// Expression (CC11) 7-bit controller value.
    pub expression: u8,
    /// Most recent non-reset Control Change received on a matched channel.
    pub last_cc: Option<ControlChange>,
}

/// Exact channel-voice messages only; malformed input never changes the snapshot.
pub fn parse(message: &[u8], channel: u8, current: Controllers) -> Option<Controllers> {
    parse_filtered(message, ChannelFilter::Single(channel), current)
}

/// Channel-filtered counterpart of [`parse`], supporting both [`ChannelFilter::Omni`]
/// (all 16 channels) and [`ChannelFilter::Single`].
pub fn parse_filtered(
    message: &[u8],
    filter: ChannelFilter,
    current: Controllers,
) -> Option<Controllers> {
    if !(2..=3).contains(&message.len()) {
        return None;
    }
    let (&status, data) = message.split_first()?;
    let channel = filter.matches(status)?;
    if data.iter().any(|value| *value > 127) {
        return None;
    }
    let mut next = current;
    match (status & 0xf0, data) {
        (0xd0, [pressure]) => next.pressure = *pressure,
        (0xe0, [low, high]) => next.bend = (i16::from(*high) * 128 + i16::from(*low)) - 8192,
        (0xb0, [cc, value]) if !note_events::is_all_notes_off_cc(*cc) => {
            if *cc == ControlChange::RESET_ALL_CONTROLLERS {
                next = Controllers::default();
            } else {
                match *cc {
                    ControlChange::MOD_WHEEL => next.wheel = *value,
                    ControlChange::EXPRESSION => next.expression = *value,
                    ControlChange::SUSTAIN => next.sustain = *value,
                    _ => {}
                }
                next.last_cc = Some(ControlChange {
                    channel,
                    cc: *cc,
                    value: *value,
                });
            }
        }
        _ => return None,
    }
    Some(next)
}

enum FramedByte {
    Reset,
    Incomplete,
    Complete { message: [u8; 3], len: usize },
}

/// Fixed MIDI 1 byte-stream decoder, including running status across packets.
/// Realtime bytes may interleave anywhere. SysEx/system-common cancel channel
/// running status; unsupported channel messages are consumed at their exact width.
pub struct Decoder {
    status: u8,
    data: [u8; 2],
    used: usize,
    sysex: bool,
    pub(crate) held: HeldNotes,
    held_channels: [u16; HELD_NOTE_COUNT],
    held_changed: bool,
    held_fault: Option<HeldNoteFault>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            status: 0,
            data: [0; 2],
            used: 0,
            sysex: false,
            held: HeldNotes::default(),
            held_channels: [0; HELD_NOTE_COUNT],
            held_changed: false,
            held_fault: None,
        }
    }
}

impl Decoder {
    /// Coalesced note changes from the same decoded stream as controllers.
    pub fn take_held_notes(&mut self) -> Option<HeldNotes> {
        if self.held_changed {
            self.held_changed = false;
            Some(self.held)
        } else {
            None
        }
    }

    /// Borrow the decoder's current held-note snapshot without consuming its dirty flag.
    pub fn held_notes(&self) -> &HeldNotes {
        &self.held
    }

    pub fn held_note_fault(&self) -> Option<HeldNoteFault> {
        self.held_fault
    }

    fn clear_framing(&mut self) {
        self.status = 0;
        self.used = 0;
        self.sysex = false;
    }

    pub(crate) fn clear_held_and_framing(&mut self) {
        self.held.clear();
        self.held_channels.fill(0);
        self.held_changed = true;
        self.clear_framing();
    }

    /// Releases every held key and drops partial/running status, as MIDI reset
    /// does. Note tokens stay monotonic for the activation's control-side floor.
    pub(crate) fn reset(&mut self, note: &dyn Fn(NoteEvent)) {
        self.clear_held_and_framing();
        note(NoteEvent::AllNotesOff);
    }

    /// Releases only the keys held on this isolated port (emitting per-note `Off`
    /// events instead of global `AllNotesOff`) so burst overflow on one port in a
    /// multi-port setup never kills healthy notes held on other ports.
    pub(crate) fn reset_port_isolated(&mut self, filter: ChannelFilter, note: &dyn Fn(NoteEvent)) {
        match filter {
            ChannelFilter::Single(channel) => {
                for held in self.held.after(0) {
                    note(NoteEvent::Off {
                        channel,
                        pitch: held.pitch,
                        velocity: 0,
                    });
                }
            }
            ChannelFilter::Omni => {
                for (pitch, &mask) in self.held_channels.iter().enumerate() {
                    if mask == 0 {
                        continue;
                    }
                    for channel in 0_u8..16 {
                        if mask & (1_u16 << channel) != 0 {
                            note(NoteEvent::Off {
                                channel,
                                pitch: pitch as u8,
                                velocity: 0,
                            });
                        }
                    }
                }
            }
        }
        self.clear_held_and_framing();
    }

    fn advance_byte(&mut self, byte: u8) -> FramedByte {
        if byte == 0xff {
            return FramedByte::Reset;
        }
        if byte >= 0xf8 {
            return FramedByte::Incomplete;
        }
        if byte >= 0x80 {
            self.used = 0;
            if byte >= 0xf0 {
                self.status = 0;
                self.sysex = byte == 0xf0;
            } else {
                self.status = byte;
                self.sysex = false;
            }
            return FramedByte::Incomplete;
        }
        if self.sysex || self.status == 0 {
            return FramedByte::Incomplete;
        }
        let width = if matches!(self.status & 0xf0, 0xc0 | 0xd0) {
            1
        } else {
            2
        };
        self.data[self.used] = byte;
        self.used += 1;
        if self.used < width {
            return FramedByte::Incomplete;
        }
        self.used = 0;
        FramedByte::Complete {
            message: [self.status, self.data[0], self.data[1]],
            len: width + 1,
        }
    }

    pub fn byte(&mut self, byte: u8, channel: u8, current: Controllers) -> Option<Controllers> {
        self.byte_with_notes_filtered(byte, ChannelFilter::Single(channel), current, &|_| {})
    }

    pub fn byte_filtered(
        &mut self,
        byte: u8,
        filter: ChannelFilter,
        current: Controllers,
    ) -> Option<Controllers> {
        self.byte_with_notes_filtered(byte, filter, current, &|_| {})
    }

    /// Delivers each complete note message synchronously, in wire order. Unlike
    /// `take_held_notes`, this preserves an on/off pair within one packet. The
    /// sink must perform bounded nonblocking work; it never owns this decoder.
    pub fn byte_with_notes(
        &mut self,
        byte: u8,
        channel: u8,
        current: Controllers,
        note: &dyn Fn(NoteEvent),
    ) -> Option<Controllers> {
        self.byte_with_notes_filtered(byte, ChannelFilter::Single(channel), current, note)
    }

    /// Channel-filtered counterpart of [`Self::byte_with_notes`], supporting Omni
    /// (all 16 channels) and single-channel modes.
    pub fn byte_with_notes_filtered(
        &mut self,
        byte: u8,
        filter: ChannelFilter,
        current: Controllers,
        note: &dyn Fn(NoteEvent),
    ) -> Option<Controllers> {
        let mut counter = self.held.through();
        self.byte_with_notes_filtered_with_token(byte, filter, current, &mut counter, note)
    }

    pub(crate) fn byte_with_notes_filtered_with_token(
        &mut self,
        byte: u8,
        filter: ChannelFilter,
        current: Controllers,
        token_counter: &mut u64,
        note: &dyn Fn(NoteEvent),
    ) -> Option<Controllers> {
        let slice_buf;
        let slice = match self.advance_byte(byte) {
            FramedByte::Reset => {
                self.reset(note);
                return Some(Controllers::default());
            }
            FramedByte::Incomplete => return None,
            FramedByte::Complete { message, len } => {
                slice_buf = message;
                &slice_buf[..len]
            }
        };
        if self.held_fault.is_none() {
            let is_omni_all_notes_off = matches!(filter, ChannelFilter::Omni)
                && matches!(slice, &[status, cc, value] if (status & 0xf0) == 0xb0
                    && value <= 127
                    && note_events::is_all_notes_off_cc(cc));
            match self.held.observe_omni_with_channels_and_token(
                &mut self.held_channels,
                slice,
                filter,
                token_counter,
                |channel, pitch| {
                    note(NoteEvent::Off {
                        channel,
                        pitch,
                        velocity: 0,
                    });
                },
            ) {
                Ok(changed) => self.held_changed |= changed,
                Err(fault) => {
                    self.held.clear();
                    self.held_channels.fill(0);
                    self.held_fault = Some(fault);
                    self.held_changed = true;
                    note(NoteEvent::InputLost(fault));
                }
            }
            if self.held_fault.is_none()
                && !is_omni_all_notes_off
                && let Some(event) = NoteEvent::parse_filtered(slice, filter)
            {
                note(event);
            }
        }
        parse_filtered(slice, filter, current)
    }

    /// Decode one MIDI 1.0 byte through the existing byte-stream state machine
    /// (preserving running status, interleaved realtime bytes, SysEx cancellation,
    /// and `0xFF` reset) and route completed channel-voice messages through `mpe`.
    pub fn byte_with_mpe(
        &mut self,
        byte: u8,
        mpe: &mut MpeDecoder,
        current: Controllers,
        emit: &dyn Fn(MpeExpressionEvent),
    ) -> Option<Controllers> {
        let mut counter = self.held.through();
        self.byte_with_mpe_with_token(byte, mpe, current, &mut counter, emit)
    }

    pub(crate) fn byte_with_mpe_with_token(
        &mut self,
        byte: u8,
        mpe: &mut MpeDecoder,
        current: Controllers,
        token_counter: &mut u64,
        emit: &dyn Fn(MpeExpressionEvent),
    ) -> Option<Controllers> {
        let slice_buf;
        let slice = match self.advance_byte(byte) {
            FramedByte::Reset => {
                self.clear_held_and_framing();
                mpe.reset();
                emit(MpeExpressionEvent::AllNotesOff);
                return Some(Controllers::default());
            }
            FramedByte::Incomplete => return None,
            FramedByte::Complete { message, len } => {
                slice_buf = message;
                &slice_buf[..len]
            }
        };
        let mut out = [MpeExpressionEvent::AllNotesOff; 16];
        let count = mpe.decode_message_into(slice, &mut out);
        for event in &out[..count] {
            if self.held_fault.is_none() {
                match self.held.observe_mpe_event_with_token(
                    &mut self.held_channels,
                    *event,
                    token_counter,
                ) {
                    Ok(changed) => self.held_changed |= changed,
                    Err(fault) => {
                        self.held.clear();
                        self.held_channels.fill(0);
                        self.held_fault = Some(fault);
                        self.held_changed = true;
                    }
                }
            }
            emit(*event);
        }
        // In MPE mode, master pitch bend is combined directly into per-note
        // `NotePitchBend` cents via `MpeDecoder`, so exclude `0xe0` from the
        // global `Controllers.bend` snapshot to prevent double-applying bend.
        if (slice[0] & 0xf0) == 0xe0 {
            return None;
        }
        let master = ChannelFilter::Single(mpe.config().zone.master_channel());
        parse_filtered(slice, master, current)
    }
}

/// Shared bounded packet-loop driver enforcing `CALLBACK_PACKETS` and
/// `CALLBACK_BYTES` across all single-port and multi-port delivery paths.
///
/// Invokes `on_packet(timestamp_ticks, accepted_bytes, overflow)` for each
/// packet in `packets`, stopping immediately once `on_packet` returns `false`
/// or an overflow packet has been processed.
pub(crate) fn for_each_bounded_packet<'a>(
    packets: impl IntoIterator<Item = TimedPacket<'a>>,
    mut on_packet: impl FnMut(u64, &'a [u8], bool) -> bool,
) {
    let mut remaining = CALLBACK_BYTES;
    // One packet past the limit is inspected only to detect overflow.
    for (index, packet) in packets.into_iter().take(CALLBACK_PACKETS + 1).enumerate() {
        let bytes = packet.data;
        let overflow = index == CALLBACK_PACKETS || bytes.len() > remaining;
        let accepted = if index == CALLBACK_PACKETS {
            &[][..]
        } else {
            bytes.get(..remaining).unwrap_or(bytes)
        };
        let keep_going = on_packet(packet.timestamp_ticks, accepted, overflow);
        if !keep_going || overflow {
            break;
        }
        remaining -= accepted.len();
    }
}

/// Bounded packet-stream MPE decoder reusing [`Decoder`]'s framing, running
/// status, interleaved realtime handling, and `CALLBACK_PACKETS`/`CALLBACK_BYTES`
/// burst-overflow reset without adding a separate input worker.
pub fn deliver_packets_with_mpe<'a>(
    decoder: &mut Decoder,
    mpe: &mut MpeDecoder,
    mut current: Controllers,
    packets: impl IntoIterator<Item = &'a [u8]>,
    publish: &dyn Fn(Controllers),
    emit: &dyn Fn(MpeExpressionEvent),
) -> Controllers {
    for_each_bounded_packet(
        packets.into_iter().map(|data| TimedPacket {
            timestamp_ticks: 0,
            data,
        }),
        |_, accepted, overflow| {
            for &byte in accepted {
                if let Some(next) = decoder.byte_with_mpe(byte, mpe, current, emit) {
                    current = next;
                    publish(current);
                }
            }
            if overflow {
                decoder.clear_held_and_framing();
                mpe.reset();
                current = Controllers::default();
                publish(current);
                emit(MpeExpressionEvent::AllNotesOff);
            }
            true
        },
    );
    current
}

/// Same bounded packet loop for the native port and deterministic byte fixtures.
/// Controllers retain their existing per-message delivery; physical notes coalesce
/// once per invocation. At most `CALLBACK_PACKETS` packets and `CALLBACK_BYTES`
/// bytes are decoded per invocation; reaching a limit exactly is not a loss.
/// Input beyond either limit is dropped and handled as a MIDI reset: held notes
/// are released (`NoteEvent::AllNotesOff` plus an empty snapshot) and the decoder
/// resynchronizes, so a dense burst cannot stick notes or end the connection.
/// The connection owner must provide one serialized writer and one decoder per
/// endpoint; never share a decoder/endpoint across concurrent input ports.
pub fn deliver_packets<'a>(
    decoder: &mut Decoder,
    channel: u8,
    current: Controllers,
    packets: impl IntoIterator<Item = &'a [u8]>,
    notes: &HeldNoteInput,
    publish: &dyn Fn(Controllers),
) -> Controllers {
    deliver_packets_with_notes_filtered(
        decoder,
        ChannelFilter::Single(channel),
        current,
        packets,
        notes,
        publish,
        &|_| {},
    )
}

/// Channel-filtered counterpart of [`deliver_packets`], supporting Omni and single-channel modes.
pub fn deliver_packets_filtered<'a>(
    decoder: &mut Decoder,
    filter: ChannelFilter,
    current: Controllers,
    packets: impl IntoIterator<Item = &'a [u8]>,
    notes: &HeldNoteInput,
    publish: &dyn Fn(Controllers),
) -> Controllers {
    deliver_packets_with_notes_filtered(decoder, filter, current, packets, notes, publish, &|_| {})
}

/// Ordered-note counterpart of `deliver_packets`, sharing its byte/packet and
/// held-snapshot limits. Burst overflow publishes `AllNotesOff` and continues on
/// the next invocation. Held-note faults are terminal: publish one explicit loss
/// event and stop delivery. The sink must cancel its host activation on loss; it
/// must never silently drop a note-off when its own handoff is full.
pub fn deliver_packets_with_notes<'a>(
    decoder: &mut Decoder,
    channel: u8,
    current: Controllers,
    packets: impl IntoIterator<Item = &'a [u8]>,
    notes: &HeldNoteInput,
    publish: &dyn Fn(Controllers),
    note: &dyn Fn(NoteEvent),
) -> Controllers {
    deliver_packets_with_notes_filtered(
        decoder,
        ChannelFilter::Single(channel),
        current,
        packets,
        notes,
        publish,
        note,
    )
}

/// Channel-filtered counterpart of [`deliver_packets_with_notes`].
pub fn deliver_packets_with_notes_filtered<'a>(
    decoder: &mut Decoder,
    filter: ChannelFilter,
    current: Controllers,
    packets: impl IntoIterator<Item = &'a [u8]>,
    notes: &HeldNoteInput,
    publish: &dyn Fn(Controllers),
    note: &dyn Fn(NoteEvent),
) -> Controllers {
    let mut clock = MonotonicTimestampClock::default();
    deliver_timed_packets_with_notes_filtered(
        decoder,
        filter,
        current,
        MachTimebase::NANOSECONDS,
        &mut clock,
        packets.into_iter().map(|data| TimedPacket {
            timestamp_ticks: 0,
            data,
        }),
        notes,
        publish,
        &|timed| note(timed.event),
    )
}

/// Hardware-timestamped counterpart of `deliver_packets_with_notes`. Each packet's
/// raw Mach host timestamp is converted via `timebase` and clamped to monotonic
/// non-decreasing nanoseconds via `clock` before attaching to every emitted
/// [`TimestampedNoteEvent`] (including `NoteEvent::ControlChange` and overflow resets).
#[allow(clippy::too_many_arguments)]
pub fn deliver_timed_packets_with_notes<'a>(
    decoder: &mut Decoder,
    channel: u8,
    current: Controllers,
    timebase: MachTimebase,
    clock: &mut MonotonicTimestampClock,
    packets: impl IntoIterator<Item = TimedPacket<'a>>,
    notes: &HeldNoteInput,
    publish: &dyn Fn(Controllers),
    note: &dyn Fn(TimestampedNoteEvent),
) -> Controllers {
    deliver_timed_packets_with_notes_filtered(
        decoder,
        ChannelFilter::Single(channel),
        current,
        timebase,
        clock,
        packets,
        notes,
        publish,
        note,
    )
}

/// Channel-filtered counterpart of [`deliver_timed_packets_with_notes`].
#[allow(clippy::too_many_arguments)]
pub fn deliver_timed_packets_with_notes_filtered<'a>(
    decoder: &mut Decoder,
    filter: ChannelFilter,
    mut current: Controllers,
    timebase: MachTimebase,
    clock: &mut MonotonicTimestampClock,
    packets: impl IntoIterator<Item = TimedPacket<'a>>,
    notes: &HeldNoteInput,
    publish: &dyn Fn(Controllers),
    note: &dyn Fn(TimestampedNoteEvent),
) -> Controllers {
    if notes.is_cancelled() {
        return current;
    }
    let mut faulted = false;
    for_each_bounded_packet(packets, |timestamp_ticks, accepted, overflow| {
        let timestamp_ns = clock.advance(timebase, timestamp_ticks);
        let emit = |event| {
            note(TimestampedNoteEvent {
                timestamp_ns,
                event,
            });
        };
        for &byte in accepted {
            if let Some(next) = decoder.byte_with_notes_filtered(byte, filter, current, &emit) {
                current = next;
                publish(current);
            }
            if let Some(fault) = decoder.held_note_fault() {
                notes.fail(fault);
                faulted = true;
                return false;
            }
        }
        if overflow {
            decoder.reset(&emit);
        }
        true
    });
    if faulted {
        return current;
    }
    if let Some(fault) = decoder.held_note_fault() {
        notes.fail(fault);
    } else if let Some(snapshot) = decoder.take_held_notes() {
        notes.publish(snapshot);
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::held_notes::HeldNotesRead;

    #[test]
    fn exact_messages_channel_endpoints_and_reset() {
        let initial = Controllers::default();
        let pressure = parse(&[0xd2, 127], 2, initial).unwrap();
        assert_eq!(pressure.pressure, 127);
        assert_eq!(parse(&[0xe2, 0, 0], 2, pressure).unwrap().bend, -8192);
        assert_eq!(parse(&[0xe2, 127, 127], 2, pressure).unwrap().bend, 8191);
        assert_eq!(parse(&[0xe2, 0, 64], 2, pressure).unwrap().bend, 0);
        let wheel = parse(&[0xb2, 1, 127], 2, pressure).unwrap();
        assert_eq!(wheel.wheel, 127);
        assert_eq!(
            wheel.last_cc,
            Some(ControlChange {
                channel: 2,
                cc: 1,
                value: 127,
            })
        );
        let sustain = parse(&[0xb2, 64, 127], 2, wheel).unwrap();
        assert_eq!(sustain.sustain, 127);
        assert_eq!(
            sustain.last_cc,
            Some(ControlChange {
                channel: 2,
                cc: 64,
                value: 127,
            })
        );
        let expression = parse(&[0xb2, 11, 96], 2, sustain).unwrap();
        assert_eq!(expression.expression, 96);
        assert_eq!(
            expression.last_cc,
            Some(ControlChange {
                channel: 2,
                cc: 11,
                value: 96,
            })
        );
        let general = parse(&[0xb2, 74, 88], 2, expression).unwrap();
        assert_eq!(
            general.last_cc,
            Some(ControlChange {
                channel: 2,
                cc: 74,
                value: 88,
            })
        );
        assert_eq!(parse(&[0xb2, 121, 0], 2, general), Some(initial));
        for malformed in [
            &[0xd2][..],
            &[0xd2, 128],
            &[0xd2, 1, 2],
            &[0xe2, 0],
            &[0xb2, 1],
            &[0x92, 60, 127],
            &[0xb2, 123, 0],
            &[0xd1, 127],
            &[0xf8],
            &[0xf0, 1, 0xf7],
        ] {
            assert_eq!(parse(malformed, 2, general), None, "{malformed:?}");
        }
        assert_eq!(parse(&[0xd0, 1], 16, general), None);
    }

    #[test]
    fn flood_coalesces_to_latest_without_message_storage() {
        let mut state = Controllers::default();
        for index in 0..100_000 {
            state = parse(&[0xb0, 1, (index % 128) as u8], 0, state).unwrap();
        }
        assert_eq!(state.wheel, (99_999 % 128) as u8);
    }

    #[test]
    fn decoder_keeps_running_status_across_packets_and_skips_realtime_notes_and_sysex() {
        let mut decoder = Decoder::default();
        let mut state = Controllers::default();
        let mut outputs = Vec::new();
        for packet in [
            &[0xb0, 1][..],
            &[0xf8, 100, 1, 20, 0xd0, 70, 0xfa, 90],
            &[0x90, 60, 100, 61, 110, 0xb0, 1, 30],
            &[0xf0, 0xd, 1, 0xf8, 30, 0xf7, 1, 100, 0xe0, 0, 64],
        ] {
            for byte in packet {
                if let Some(next) = decoder.byte(*byte, 0, state) {
                    state = next;
                    outputs.push(state);
                }
            }
        }
        assert_eq!(outputs.len(), 6);
        assert_eq!(
            outputs.iter().map(|state| state.wheel).collect::<Vec<_>>(),
            [100, 20, 20, 20, 30, 30]
        );
        assert_eq!(
            state,
            Controllers {
                pressure: 90,
                bend: 0,
                wheel: 30,
                sustain: 0,
                expression: 0,
                last_cc: Some(ControlChange {
                    channel: 0,
                    cc: 1,
                    value: 30,
                }),
            }
        );
        decoder.byte(0xb0, 0, state);
        decoder.byte(1, 0, state);
        decoder.byte(0xf2, 0, state); // system-common cancels partial running status
        assert_eq!(decoder.byte(100, 0, state), None);
        assert_eq!(decoder.byte(1, 0, state), None);
        assert_eq!(decoder.byte(100, 0, state), None);
    }

    #[test]
    fn system_reset_publishes_default_controllers_releases_notes_and_clock_stays_transparent() {
        let mut decoder = Decoder::default();
        let held = HeldNoteInput::default();
        let published = std::cell::RefCell::new(Vec::new());
        let events = std::cell::RefCell::new(Vec::new());

        let before_reset = deliver_packets_with_notes(
            &mut decoder,
            0,
            Controllers::default(),
            [&[
                0x90, 60, 100, // Note On 60
                0xe0, 0x00, 0xf8, 0x60, // Pitch bend +4096 with interleaved clock
                0xb0, 1, 90, // Mod wheel 90
                0xd0, 64, // Channel pressure 64
            ][..]],
            &held,
            &|snapshot| published.borrow_mut().push(snapshot),
            &|event| events.borrow_mut().push(event),
        );
        assert_eq!(
            before_reset,
            Controllers {
                pressure: 64,
                bend: 4096,
                wheel: 90,
                sustain: 0,
                expression: 0,
                last_cc: Some(ControlChange {
                    channel: 0,
                    cc: 1,
                    value: 90,
                }),
            }
        );

        published.borrow_mut().clear();
        events.borrow_mut().clear();
        let after_reset = deliver_packets_with_notes(
            &mut decoder,
            0,
            before_reset,
            [&[0xff][..]],
            &held,
            &|snapshot| published.borrow_mut().push(snapshot),
            &|event| events.borrow_mut().push(event),
        );
        assert_eq!(after_reset, Controllers::default());
        assert_eq!(*published.borrow(), vec![Controllers::default()]);
        assert_eq!(*events.borrow(), vec![NoteEvent::AllNotesOff]);
        let HeldNotesRead::Snapshot(snapshot) = held.read() else {
            panic!("expected snapshot after system reset")
        };
        assert_eq!(snapshot.after(0).count(), 0);

        published.borrow_mut().clear();
        events.borrow_mut().clear();
        let resumed = deliver_packets_with_notes(
            &mut decoder,
            0,
            Controllers::default(),
            [&[
                0x90, 62, 110, // Note On 62
                0xe0, 0x00, 0x60, // Bend +4096
                0xd0, 64,   // Pressure 64
                0xff, // System Reset
                0xb0, 1, 0xf8, 42, // Mod wheel 42 with interleaved clock
            ][..]],
            &held,
            &|snapshot| published.borrow_mut().push(snapshot),
            &|event| events.borrow_mut().push(event),
        );
        assert_eq!(
            resumed,
            Controllers {
                pressure: 0,
                bend: 0,
                wheel: 42,
                sustain: 0,
                expression: 0,
                last_cc: Some(ControlChange {
                    channel: 0,
                    cc: 1,
                    value: 42,
                }),
            }
        );
        assert_eq!(
            *published.borrow(),
            vec![
                Controllers {
                    pressure: 0,
                    bend: 4096,
                    wheel: 0,
                    sustain: 0,
                    expression: 0,
                    last_cc: None,
                },
                Controllers {
                    pressure: 64,
                    bend: 4096,
                    wheel: 0,
                    sustain: 0,
                    expression: 0,
                    last_cc: None,
                },
                Controllers::default(),
                Controllers {
                    pressure: 0,
                    bend: 0,
                    wheel: 42,
                    sustain: 0,
                    expression: 0,
                    last_cc: Some(ControlChange {
                        channel: 0,
                        cc: 1,
                        value: 42,
                    }),
                },
            ]
        );
        assert_eq!(
            *events.borrow(),
            vec![
                NoteEvent::On {
                    channel: 0,
                    pitch: 62,
                    velocity: 110,
                },
                NoteEvent::AllNotesOff,
                NoteEvent::ControlChange {
                    channel: 0,
                    cc: 1,
                    value: 42,
                },
            ]
        );
        let HeldNotesRead::Snapshot(after_resumed) = held.read() else {
            panic!("expected snapshot after resumed packet")
        };
        assert_eq!(after_resumed.after(0).count(), 0);
    }

    #[test]
    fn omni_channel_mode_delivers_all_sixteen_channels_with_wire_channel_preserved() {
        let mut decoder = Decoder::default();
        let held = HeldNoteInput::default();
        let notes = std::cell::RefCell::new(Vec::new());
        let controllers = std::cell::RefCell::new(Vec::new());
        let final_ctrl = deliver_packets_with_notes_filtered(
            &mut decoder,
            ChannelFilter::Omni,
            Controllers::default(),
            [&[
                0x90, 60, 100, // ch 0 note on C4
                0x95, 60, 88, // ch 5 same pitch C4 (must stay held when ch 0 releases)
                0x9f, 67, 120, // ch 15 note on G4
                0xa5, 60, 77, // ch 5 poly pressure
                0xb0, 64, 127, // ch 0 sustain down
                0xb5, 11, 90, // ch 5 expression
                0xbf, 74, 63, // ch 15 cutoff CC74
                0x80, 60, 10, // ch 0 note off C4 (ch 5 still holds C4)
            ][..]],
            &held,
            &|ctrl| controllers.borrow_mut().push(ctrl),
            &|event| notes.borrow_mut().push(event),
        );
        assert_eq!(final_ctrl.sustain, 127);
        assert_eq!(final_ctrl.expression, 90);
        assert_eq!(
            final_ctrl.last_cc,
            Some(ControlChange {
                channel: 15,
                cc: 74,
                value: 63,
            })
        );
        assert_eq!(
            *notes.borrow(),
            vec![
                NoteEvent::On {
                    channel: 0,
                    pitch: 60,
                    velocity: 100
                },
                NoteEvent::On {
                    channel: 5,
                    pitch: 60,
                    velocity: 88
                },
                NoteEvent::On {
                    channel: 15,
                    pitch: 67,
                    velocity: 120
                },
                NoteEvent::PolyPressure {
                    channel: 5,
                    pitch: 60,
                    value: 77
                },
                NoteEvent::ControlChange {
                    channel: 0,
                    cc: 64,
                    value: 127
                },
                NoteEvent::ControlChange {
                    channel: 5,
                    cc: 11,
                    value: 90
                },
                NoteEvent::ControlChange {
                    channel: 15,
                    cc: 74,
                    value: 63
                },
                NoteEvent::Off {
                    channel: 0,
                    pitch: 60,
                    velocity: 10
                },
            ]
        );
        let HeldNotesRead::Snapshot(snapshot) = held.read() else {
            panic!("expected snapshot");
        };
        let mut pitches: Vec<u8> = snapshot.after(0).map(|n| n.pitch).collect();
        pitches.sort_unstable();
        assert_eq!(pitches, vec![60, 67]);

        // Omni CC123 on channel 5 releases ONLY channel 5's pitch 60 without killing channel 15's pitch 67.
        notes.borrow_mut().clear();
        deliver_packets_with_notes_filtered(
            &mut decoder,
            ChannelFilter::Omni,
            final_ctrl,
            [&[0xb5, 123, 0][..]],
            &held,
            &|_| {},
            &|event| notes.borrow_mut().push(event),
        );
        assert_eq!(
            *notes.borrow(),
            vec![NoteEvent::Off {
                channel: 5,
                pitch: 60,
                velocity: 0
            }]
        );
        let HeldNotesRead::Snapshot(after_ch5_cc123) = held.read() else {
            panic!("expected snapshot");
        };
        let remaining: Vec<u8> = after_ch5_cc123.after(0).map(|n| n.pitch).collect();
        assert_eq!(remaining, vec![67]);
    }
}
