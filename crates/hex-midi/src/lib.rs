//! # Purpose
//! Own optional control-side MIDI controller input and bounded scalar delivery.
//!
//! # Dependencies
//! Uses the standard library and the configured platform MIDI dependency on supported targets.
//!
//! # Does not own
//! Does not schedule musical notes, mutate project state or perform audio callback work. Hardware
//! note hosting is outside this input-capture boundary. Physical held snapshots
//! support control-side step entry without scheduling or monitoring sound.
mod held_notes;
mod mpe;
mod note_events;
mod timebase;
pub use held_notes::{
    HELD_NOTE_COUNT, HeldNote, HeldNoteFault, HeldNoteInput, HeldNotes, HeldNotesRead,
};
pub use mpe::{
    DEFAULT_MPE_PITCH_BEND_SEMITONES, MAX_MPE_PITCH_BEND_SEMITONES, MpeConfig, MpeDecoder,
    MpeExpressionEvent, MpeZone,
};
pub use note_events::{ControlChange, NoteEvent};
pub use timebase::{MachTimebase, MonotonicTimestampClock, TimedPacket, TimestampedNoteEvent};

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};
use std::thread::JoinHandle;
use std::time::Duration;

/// Maximum number of MIDI input ports aggregated into one logical live input stream.
pub const MAX_AGGREGATED_PORTS: usize = 16;

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
/// Fixed MIDI 1 byte-stream decoder, including running status across packets.
/// Realtime bytes may interleave anywhere. SysEx/system-common cancel channel
/// running status; unsupported channel messages are consumed at their exact width.
pub struct Decoder {
    status: u8,
    data: [u8; 2],
    used: usize,
    sysex: bool,
    held: HeldNotes,
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
    /// Releases every held key and drops partial/running status, as MIDI reset
    /// does. Note tokens stay monotonic for the activation's control-side floor.
    fn reset(&mut self, note: &dyn Fn(NoteEvent)) {
        self.held.clear();
        self.held_channels.fill(0);
        self.held_changed = true;
        self.status = 0;
        self.used = 0;
        self.sysex = false;
        note(NoteEvent::AllNotesOff);
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
        if byte == 0xff {
            self.reset(note);
            return Some(Controllers::default());
        }
        if byte >= 0xf8 {
            return None;
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
            return None;
        }
        if self.sysex || self.status == 0 {
            return None;
        }
        let width = if matches!(self.status & 0xf0, 0xc0 | 0xd0) {
            1
        } else {
            2
        };
        self.data[self.used] = byte;
        self.used += 1;
        if self.used < width {
            return None;
        }
        self.used = 0;
        let message = [self.status, self.data[0], self.data[1]];
        if self.held_fault.is_none() {
            match self.held.observe_omni_with_channels(
                &mut self.held_channels,
                &message[..width + 1],
                filter,
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
                && let Some(event) = NoteEvent::parse_filtered(&message[..width + 1], filter)
            {
                note(event);
            }
        }
        parse_filtered(&message[..width + 1], filter, current)
    }
}
const CALLBACK_BYTES: usize = 65_536;
const CALLBACK_PACKETS: usize = 1024;

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
    let mut remaining = CALLBACK_BYTES;
    // One packet past the limit is inspected only to detect overflow.
    for (index, packet) in packets.into_iter().take(CALLBACK_PACKETS + 1).enumerate() {
        let timestamp_ns = clock.advance(timebase, packet.timestamp_ticks);
        let bytes = packet.data;
        let overflow = index == CALLBACK_PACKETS || bytes.len() > remaining;
        let accepted = if index == CALLBACK_PACKETS {
            &[][..]
        } else {
            bytes.get(..remaining).unwrap_or(bytes)
        };
        let emit = |event| {
            note(TimestampedNoteEvent {
                timestamp_ns,
                event,
            });
        };
        for byte in accepted {
            if let Some(next) = decoder.byte_with_notes_filtered(*byte, filter, current, &emit) {
                current = next;
                publish(current);
            }
            if let Some(fault) = decoder.held_note_fault() {
                notes.fail(fault);
                return current;
            }
        }
        if overflow {
            decoder.reset(&emit);
            break;
        }
        remaining -= accepted.len();
    }
    if let Some(fault) = decoder.held_note_fault() {
        notes.fail(fault);
    } else if let Some(snapshot) = decoder.take_held_notes() {
        notes.publish(snapshot);
    }
    current
}

struct AggregatedState {
    decoders: Vec<Decoder>,
    current: Controllers,
    clock: MonotonicTimestampClock,
    merged_through: u64,
    publish: Box<dyn Fn(Controllers) + Send>,
    note: Box<dyn Fn(TimestampedNoteEvent) + Send>,
}

/// Bounded multi-port MIDI input aggregator (up to [`MAX_AGGREGATED_PORTS`] ports).
///
/// Each port retains its own isolated [`Decoder`] so running status, SysEx framing,
/// and per-port burst overflow never bleed across physical inputs. One control-side
/// mutex serializes merged controller snapshots, monotonic cross-port timestamps,
/// ordered note/CC delivery, and sole-writer publication into [`HeldNoteInput`].
pub struct MultiPortAggregator {
    filter: ChannelFilter,
    timebase: MachTimebase,
    notes: Arc<HeldNoteInput>,
    state: std::sync::Mutex<AggregatedState>,
}

impl MultiPortAggregator {
    /// Construct a bounded multi-port aggregator for `port_count` distinct input ports
    /// (`1..=MAX_AGGREGATED_PORTS`) using `filter` and `timebase`.
    pub fn new(
        port_count: usize,
        filter: ChannelFilter,
        timebase: MachTimebase,
        notes: Arc<HeldNoteInput>,
        publish: Box<dyn Fn(Controllers) + Send>,
        note: Box<dyn Fn(TimestampedNoteEvent) + Send>,
    ) -> Result<Self, &'static str> {
        if port_count == 0 || port_count > MAX_AGGREGATED_PORTS {
            return Err("controller port count must be between 1 and 16");
        }
        if !filter.is_valid() {
            return Err("controller channel is invalid");
        }
        let mut decoders = Vec::new();
        decoders
            .try_reserve_exact(port_count)
            .map_err(|_| "could not allocate midi port decoders")?;
        for _ in 0..port_count {
            decoders.push(Decoder::default());
        }
        Ok(Self {
            filter,
            timebase,
            notes,
            state: std::sync::Mutex::new(AggregatedState {
                decoders,
                current: Controllers::default(),
                clock: MonotonicTimestampClock::default(),
                merged_through: 0,
                publish,
                note,
            }),
        })
    }

    /// Deliver one batch of timestamped packets from `port_index` into its isolated
    /// decoder and publish merged held-note / controller / ordered-event updates.
    pub fn deliver_timed_packets<'a>(
        &self,
        port_index: usize,
        packets: impl IntoIterator<Item = TimedPacket<'a>>,
    ) {
        if self.notes.is_cancelled() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if self.notes.is_cancelled() || port_index >= state.decoders.len() {
            return;
        }
        let AggregatedState {
            decoders,
            current,
            clock,
            merged_through,
            publish,
            note,
        } = &mut *state;
        let decoder = &mut decoders[port_index];
        let mut remaining = CALLBACK_BYTES;
        let mut any_held_change = false;
        for (index, packet) in packets.into_iter().take(CALLBACK_PACKETS + 1).enumerate() {
            let timestamp_ns = clock.advance(self.timebase, packet.timestamp_ticks);
            let bytes = packet.data;
            let overflow = index == CALLBACK_PACKETS || bytes.len() > remaining;
            let accepted = if index == CALLBACK_PACKETS {
                &[][..]
            } else {
                bytes.get(..remaining).unwrap_or(bytes)
            };
            let emit = |event| {
                note(TimestampedNoteEvent {
                    timestamp_ns,
                    event,
                });
            };
            for byte in accepted {
                if let Some(next) =
                    decoder.byte_with_notes_filtered(*byte, self.filter, *current, &emit)
                {
                    *current = next;
                    publish(*current);
                }
                if let Some(fault) = decoder.held_note_fault() {
                    self.notes.fail(fault);
                    return;
                }
            }
            if overflow {
                decoder.reset(&emit);
                break;
            }
            remaining -= accepted.len();
        }
        if let Some(fault) = decoder.held_note_fault() {
            self.notes.fail(fault);
            return;
        }
        if let Some(port_snapshot) = decoder.take_held_notes() {
            *merged_through = (*merged_through).max(port_snapshot.through());
            any_held_change = true;
        }
        if any_held_change {
            let merged =
                HeldNotes::merge_from(decoders.iter().map(Decoder::held_notes), *merged_through);
            self.notes.publish(merged);
        }
    }

    pub fn controllers(&self) -> Controllers {
        self.state
            .lock()
            .map(|state| state.current)
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub id: u32,
    pub name: String,
}
/// Constructed on control; the native callback only invokes this bounded sink.
pub struct Sink {
    pub publish: Box<dyn Fn(Controllers) + Send>,
    /// Ordered native input messages with monotonic hardware timestamps,
    /// independent of the step-entry snapshot.
    pub note: Box<dyn Fn(TimestampedNoteEvent) + Send>,
    pub notes: Arc<HeldNoteInput>,
    pub cancel: Box<dyn Fn() + Send>,
    pub is_cancelled: Box<dyn Fn() -> bool + Send>,
}
impl Sink {
    fn cancel(&self) {
        self.notes.cancel();
        (self.cancel)();
    }
}
pub enum Command {
    Discover {
        request_id: u64,
    },
    Connect {
        activation: u64,
        port: Port,
        channel: u8,
        sink: Sink,
    },
    /// Connect one or more ports (up to [`MAX_AGGREGATED_PORTS`]) with a
    /// [`ChannelFilter`] (single channel or Omni), aggregating them through one
    /// serialized [`MultiPortAggregator`].
    ConnectPorts {
        activation: u64,
        ports: Vec<Port>,
        channel_filter: ChannelFilter,
        sink: Sink,
    },
    Disconnect {
        activation: u64,
    },
}
#[derive(Debug)]
pub enum Event {
    Ports {
        request_id: u64,
        result: Result<Vec<Port>, String>,
    },
    Connection {
        activation: u64,
        result: Result<bool, String>,
    },
}
/// Command queue capacity shared by every handle; a full queue refuses the command.
const COMMAND_CAPACITY: usize = 4;
/// Event queue capacity; replies that do not fit wait in the worker's [`Outbox`].
const EVENT_CAPACITY: usize = 4;
/// Retry interval for a retained reply while the event queue stays full.
const RETRY_INTERVAL: Duration = Duration::from_millis(20);
/// Idle wait between connected-endpoint presence checks.
const IDLE_INTERVAL: Duration = Duration::from_millis(500);

/// Worker-private queue entry: public commands plus the owner's prompt shutdown wake.
enum Request {
    Command(Command),
    Shutdown,
}
/// Cloneable command/event access to one worker.
///
/// Commands use a bounded queue of `COMMAND_CAPACITY`; a full or closed queue refuses
/// immediately and cancels a refused connection sink. Port and connection replies are
/// never silently lost while the receiver exists: see `Outbox` for the overflow policy.
#[derive(Clone)]
pub struct Handle {
    commands: SyncSender<Request>,
    events: Arc<std::sync::Mutex<Receiver<Event>>>,
}
impl Handle {
    pub fn send(&self, command: Command) -> Result<(), &'static str> {
        match self.commands.try_send(Request::Command(command)) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(request) | mpsc::TrySendError::Disconnected(request)) => {
                if let Request::Command(
                    Command::Connect { sink, .. } | Command::ConnectPorts { sink, .. },
                ) = request
                {
                    sink.cancel();
                }
                Err("controller input worker is busy or unavailable")
            }
        }
    }
    pub fn try_event(&self) -> Result<Event, TryRecvError> {
        self.events
            .lock()
            .map_err(|_| TryRecvError::Disconnected)?
            .try_recv()
    }
}
/// Bounded worker-side custody for replies that do not fit the event queue.
///
/// At most one undelivered reply per kind (ports, connection) is retained and retried every
/// [`RETRY_INTERVAL`] until the receiver drains. A newer reply of the same kind replaces a
/// retained one unless it answers an older discovery id or activation: the UI issues both
/// monotonically and ignores stale answers, so only the newest reply per kind is observable.
/// A connection is never torn down because its reply is waiting. Replies are discarded only
/// once the receiver is gone. Note input bypasses this queue; held-note, note-off and
/// all-notes-off delivery are unaffected.
#[derive(Default)]
struct Outbox {
    ports: Option<(u64, Result<Vec<Port>, String>)>,
    connection: Option<(u64, Result<bool, String>)>,
    #[cfg(test)]
    on_full_retain: Option<SyncSender<()>>,
}
impl Outbox {
    fn retain(&mut self, event: Event) {
        match event {
            Event::Ports { request_id, result } => {
                if self
                    .ports
                    .as_ref()
                    .is_none_or(|(held, _)| *held <= request_id)
                {
                    self.ports = Some((request_id, result));
                }
            }
            Event::Connection { activation, result } => {
                if self
                    .connection
                    .as_ref()
                    .is_none_or(|(held, _)| *held <= activation)
                {
                    self.connection = Some((activation, result));
                }
            }
        }
    }
    /// Delivers retained replies that fit; full-queue replies stay retained.
    fn flush(&mut self, events: &SyncSender<Event>) {
        let ports = self
            .ports
            .take()
            .map(|(request_id, result)| Event::Ports { request_id, result });
        let connection = self
            .connection
            .take()
            .map(|(activation, result)| Event::Connection { activation, result });
        for event in [ports, connection].into_iter().flatten() {
            match events.try_send(event) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(event)) => {
                    self.retain(event);
                    #[cfg(test)]
                    if let Some(signal) = &self.on_full_retain {
                        let _ = signal.try_send(());
                    }
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    *self = Self::default();
                    return;
                }
            }
        }
    }
    fn send(&mut self, events: &SyncSender<Event>, event: Event) {
        self.retain(event);
        self.flush(events);
    }
    fn wait(&self) -> Duration {
        if self.ports.is_some() || self.connection.is_some() {
            RETRY_INTERVAL
        } else {
            IDLE_INTERVAL
        }
    }
}
fn cancel_queued(requests: &Receiver<Request>) {
    for _ in 0..=COMMAND_CAPACITY {
        match requests.try_recv() {
            Ok(Request::Command(
                Command::Connect { sink, .. } | Command::ConnectPorts { sink, .. },
            )) => sink.cancel(),
            Ok(_) => {}
            Err(_) => break,
        }
    }
}
/// Owner of the control-side MIDI thread.
///
/// Dropping the worker sets the stop flag and queues a shutdown wake, so an idle worker
/// exits without waiting for its idle interval. When the command queue is full the worker
/// is already draining it and observes the flag after the current command.
pub struct Worker {
    handle: Handle,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    /// Starts an idle worker; discovery/native access requires an explicit command.
    pub fn start() -> Result<Self, String> {
        let (commands, requests) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (events, results) = mpsc::sync_channel(EVENT_CAPACITY);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let thread = std::thread::Builder::new()
            .name("hex-midi".into())
            .spawn(move || native::run(requests, events, stop))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            handle: Handle {
                commands,
                events: Arc::new(std::sync::Mutex::new(results)),
            },
            stopping,
            thread: Some(thread),
        })
    }
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        // A full queue is already being drained; the flag remains the fallback.
        let _ = self.handle.commands.try_send(Request::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use coremidi::{Client, InputPort, Source, Sources};
    struct Active {
        // Rust drops fields in declaration order: input ports close before client.
        activation: u64,
        _ports: Vec<InputPort>,
        _client: Client,
        sources: Vec<Source>,
        cancel: Box<dyn Fn() + Send>,
        is_cancelled: Box<dyn Fn() -> bool + Send>,
        notes: Arc<HeldNoteInput>,
    }
    impl Drop for Active {
        fn drop(&mut self) {
            self.notes.cancel();
            (self.cancel)();
        }
    }
    fn source_by_id(id: u32) -> Option<Source> {
        Sources
            .into_iter()
            .take(128)
            .find(|source| source.unique_id() == Some(id))
    }
    fn discover() -> Result<Vec<Port>, String> {
        if Sources::count() > 127 {
            return Err("controller input exceeds 127 ports".into());
        }
        let result: Vec<_> = Sources
            .into_iter()
            .take(128)
            .map(|source| {
                let id = source
                    .unique_id()
                    .ok_or("controller port identity unavailable")?;
                let name = source
                    .display_name()
                    .ok_or("controller port name unavailable")?;
                if name.len() > 1024 {
                    return Err("controller port name exceeds limit".into());
                }
                Ok(Port { id, name })
            })
            .collect::<Result<_, String>>()?;
        if result.len() > 127 {
            return Err("controller input exceeds 127 ports".into());
        }
        Ok(result)
    }
    fn connect_ports(
        activation: u64,
        ports: Vec<Port>,
        filter: ChannelFilter,
        sink: Sink,
    ) -> Result<Active, String> {
        let result = (|| {
            if sink.notes.is_cancelled() || (sink.is_cancelled)() {
                return Err("controller activation was cancelled".into());
            }
            if ports.is_empty() || ports.len() > MAX_AGGREGATED_PORTS {
                return Err("controller port count must be between 1 and 16".into());
            }
            if !filter.is_valid() {
                return Err("controller channel is invalid".into());
            }
            let mut seen_ids = std::collections::HashSet::with_capacity(ports.len());
            let mut sources = Vec::with_capacity(ports.len());
            for port in &ports {
                if !seen_ids.insert(port.id) {
                    return Err("controller port list contains duplicate port ids".into());
                }
                let source = source_by_id(port.id).ok_or("controller port is unavailable")?;
                sources.push(source);
            }
            let client = Client::new("hex controller input")
                .map_err(|status| format!("controller client failed: {status}"))?;
            let aggregator = Arc::new(
                MultiPortAggregator::new(
                    sources.len(),
                    filter,
                    MachTimebase::host(),
                    Arc::clone(&sink.notes),
                    sink.publish,
                    sink.note,
                )
                .map_err(str::to_owned)?,
            );
            let mut inputs = Vec::with_capacity(sources.len());
            for (port_index, source) in sources.iter().enumerate() {
                let agg = Arc::clone(&aggregator);
                let port_name = format!("hex controller input {port_index}");
                let input = client
                    .input_port(&port_name, move |packets| {
                        agg.deliver_timed_packets(
                            port_index,
                            packets.iter().map(|packet| TimedPacket {
                                timestamp_ticks: packet.timestamp(),
                                data: packet.data(),
                            }),
                        );
                    })
                    .map_err(|status| format!("controller port creation failed: {status}"))?;
                input
                    .connect_source(source)
                    .map_err(|status| format!("controller connection failed: {status}"))?;
                inputs.push(input);
            }
            Ok((inputs, client, sources))
        })();
        match result {
            Ok((inputs, client, sources)) => Ok(Active {
                activation,
                _ports: inputs,
                _client: client,
                sources,
                cancel: sink.cancel,
                is_cancelled: sink.is_cancelled,
                notes: sink.notes,
            }),
            Err(error) => {
                // publish/note may have moved into the aggregator; cancel remaining owners.
                sink.notes.cancel();
                (sink.cancel)();
                Err(error)
            }
        }
    }
    pub(super) fn run(
        requests: Receiver<Request>,
        events: SyncSender<Event>,
        stop: Arc<AtomicBool>,
    ) {
        run_with_outbox(requests, events, stop, Outbox::default());
    }
    #[cfg(test)]
    pub(super) fn run_with_retain_signal(
        requests: Receiver<Request>,
        events: SyncSender<Event>,
        stop: Arc<AtomicBool>,
        on_full_retain: SyncSender<()>,
    ) {
        run_with_outbox(
            requests,
            events,
            stop,
            Outbox {
                on_full_retain: Some(on_full_retain),
                ..Outbox::default()
            },
        );
    }
    fn run_with_outbox(
        requests: Receiver<Request>,
        events: SyncSender<Event>,
        stop: Arc<AtomicBool>,
        mut outbox: Outbox,
    ) {
        let mut active: Option<Active> = None;
        while !stop.load(Ordering::Acquire) {
            outbox.flush(&events);
            match requests.recv_timeout(outbox.wait()) {
                Ok(Request::Shutdown) => break,
                Ok(Request::Command(Command::Discover { request_id })) => {
                    outbox.send(
                        &events,
                        Event::Ports {
                            request_id,
                            result: discover(),
                        },
                    );
                }
                Ok(Request::Command(Command::Connect {
                    activation,
                    port,
                    channel,
                    sink,
                })) => {
                    active.take();
                    let result = match connect_ports(
                        activation,
                        vec![port],
                        ChannelFilter::Single(channel),
                        sink,
                    ) {
                        Ok(connection) => {
                            active = Some(connection);
                            Ok(true)
                        }
                        Err(error) => Err(error),
                    };
                    outbox.send(&events, Event::Connection { activation, result });
                }
                Ok(Request::Command(Command::ConnectPorts {
                    activation,
                    ports,
                    channel_filter,
                    sink,
                })) => {
                    active.take();
                    let result = match connect_ports(activation, ports, channel_filter, sink) {
                        Ok(connection) => {
                            active = Some(connection);
                            Ok(true)
                        }
                        Err(error) => Err(error),
                    };
                    outbox.send(&events, Event::Connection { activation, result });
                }
                Ok(Request::Command(Command::Disconnect { activation })) => {
                    if active
                        .as_ref()
                        .is_some_and(|active| active.activation == activation)
                    {
                        active.take();
                    }
                    outbox.send(
                        &events,
                        Event::Connection {
                            activation,
                            result: Ok(false),
                        },
                    );
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Some(connection) = &active {
                // Check endpoint identity/presence, never infer loss from silence.
                let present = connection.sources.iter().all(|connected| {
                    connected
                        .unique_id()
                        .and_then(source_by_id)
                        .is_some_and(|source| source == *connected)
                });
                if !present || connection.notes.is_cancelled() || (connection.is_cancelled)() {
                    let activation = connection.activation;
                    active.take();
                    outbox.send(
                        &events,
                        Event::Connection {
                            activation,
                            result: Err("controller input disconnected".into()),
                        },
                    );
                }
            }
        }
        active.take();
        cancel_queued(&requests);
    }
}
#[cfg(not(target_os = "macos"))]
mod native {
    use super::*;
    pub(super) fn run(
        requests: Receiver<Request>,
        events: SyncSender<Event>,
        stop: Arc<AtomicBool>,
    ) {
        run_with_outbox(requests, events, stop, Outbox::default());
    }
    #[cfg(test)]
    pub(super) fn run_with_retain_signal(
        requests: Receiver<Request>,
        events: SyncSender<Event>,
        stop: Arc<AtomicBool>,
        on_full_retain: SyncSender<()>,
    ) {
        run_with_outbox(
            requests,
            events,
            stop,
            Outbox {
                on_full_retain: Some(on_full_retain),
                ..Outbox::default()
            },
        );
    }
    fn run_with_outbox(
        requests: Receiver<Request>,
        events: SyncSender<Event>,
        stop: Arc<AtomicBool>,
        mut outbox: Outbox,
    ) {
        while !stop.load(Ordering::Acquire) {
            outbox.flush(&events);
            match requests.recv_timeout(outbox.wait()) {
                Ok(Request::Shutdown) => break,
                Ok(Request::Command(
                    Command::Connect {
                        activation, sink, ..
                    }
                    | Command::ConnectPorts {
                        activation, sink, ..
                    },
                )) => {
                    sink.cancel();
                    outbox.send(
                        &events,
                        Event::Connection {
                            activation,
                            result: Err("controller input currently supports macos only".into()),
                        },
                    );
                }
                Ok(Request::Command(Command::Discover { request_id })) => {
                    outbox.send(
                        &events,
                        Event::Ports {
                            request_id,
                            result: Err("controller input currently supports macos only".into()),
                        },
                    );
                }
                Ok(Request::Command(Command::Disconnect { .. })) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        cancel_queued(&requests);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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

        // Set bend +4096 (0x00, 0x60 => 96*128 - 8192 = +4096), wheel 90, pressure 64,
        // hold a note, and interleave a MIDI clock inside a partial channel message.
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

        // System Reset ending the packet publishes and returns Controllers::default()
        // and clears held notes.
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

        // Same packet: non-neutral controllers + held note + 0xFF + later mod wheel starts
        // from the reset state, while an interleaved clock preserves partial state.
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
    fn full_or_closed_worker_queue_cancels_connection_sink_immediately() {
        let (commands, requests) = mpsc::sync_channel(1);
        let (_, results) = mpsc::sync_channel(1);
        let handle = Handle {
            commands,
            events: Arc::new(std::sync::Mutex::new(results)),
        };
        handle.send(Command::Discover { request_id: 1 }).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let notes = Arc::new(HeldNoteInput::default());
        assert!(
            handle
                .send(Command::Connect {
                    activation: 2,
                    port: Port {
                        id: 1,
                        name: "fake".into()
                    },
                    channel: 0,
                    sink: Sink {
                        notes: Arc::clone(&notes),
                        publish: Box::new(|_| {}),
                        note: Box::new(|_| {}),
                        cancel: Box::new(move || flag.store(true, Ordering::Release)),
                        is_cancelled: Box::new(|| false),
                    }
                })
                .is_err()
        );
        assert!(cancelled.load(Ordering::Acquire));
        assert!(notes.is_cancelled());
        drop(requests);
        cancelled.store(false, Ordering::Release);
        let flag = Arc::clone(&cancelled);
        let notes = Arc::new(HeldNoteInput::default());
        assert!(
            handle
                .send(Command::Connect {
                    activation: 3,
                    port: Port {
                        id: 1,
                        name: "fake".into()
                    },
                    channel: 0,
                    sink: Sink {
                        notes: Arc::clone(&notes),
                        publish: Box::new(|_| {}),
                        note: Box::new(|_| {}),
                        cancel: Box::new(move || flag.store(true, Ordering::Release)),
                        is_cancelled: Box::new(|| false),
                    }
                })
                .is_err()
        );
        assert!(cancelled.load(Ordering::Acquire));
        assert!(notes.is_cancelled());
    }
    #[test]
    fn worker_shutdown_cancels_pending_connections_without_native_access() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let notes = Arc::new(HeldNoteInput::default());
        assert!(
            sender
                .try_send(Request::Command(Command::Connect {
                    activation: 7,
                    port: Port {
                        id: 1,
                        name: "fake".into()
                    },
                    channel: 0,
                    sink: Sink {
                        notes: Arc::clone(&notes),
                        publish: Box::new(|_| {}),
                        note: Box::new(|_| {}),
                        cancel: Box::new(move || flag.store(true, Ordering::Release)),
                        is_cancelled: Box::new(|| false)
                    }
                }))
                .is_ok()
        );
        let (events, _results) = mpsc::sync_channel(4);
        native::run(receiver, events, Arc::new(AtomicBool::new(true)));
        assert!(cancelled.load(Ordering::Acquire));
        assert!(notes.is_cancelled());
    }
    /// Drains events until one matches or the bounded deadline passes.
    fn receive_until(
        results: &Receiver<Event>,
        limit: Duration,
        done: impl Fn(&Event) -> bool,
    ) -> Vec<Event> {
        let deadline = std::time::Instant::now() + limit;
        let mut received = Vec::new();
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match results.recv_timeout(left) {
                Ok(event) => {
                    let finished = done(&event);
                    received.push(event);
                    if finished {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        received
    }
    #[test]
    fn full_event_queue_retains_discovery_reply_until_drained() {
        let (commands, requests) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (filler, results) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let (retained_tx, retained_rx) = mpsc::sync_channel(1);
        let worker = {
            let (events, stop) = (filler.clone(), Arc::clone(&stop));
            std::thread::spawn(move || {
                native::run_with_retain_signal(requests, events, stop, retained_tx)
            })
        };
        filler
            .try_send(Event::Connection {
                activation: 0,
                result: Ok(false),
            })
            .unwrap();
        commands
            .try_send(Request::Command(Command::Discover { request_id: 9 }))
            .unwrap();
        // Wait deterministically until the worker hits the full queue and retains the reply.
        retained_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let received = receive_until(&results, Duration::from_secs(2), |event| {
            matches!(event, Event::Ports { request_id: 9, .. })
        });
        stop.store(true, Ordering::Release);
        let _ = commands.try_send(Request::Shutdown);
        worker.join().unwrap();
        assert!(
            received
                .iter()
                .any(|event| matches!(event, Event::Ports { request_id: 9, .. })),
            "{received:?}"
        );
    }
    #[test]
    fn retained_replies_keep_the_newest_answer_per_kind() {
        let (sender, results) = mpsc::sync_channel(1);
        sender
            .try_send(Event::Connection {
                activation: 0,
                result: Ok(false),
            })
            .unwrap();
        let mut outbox = Outbox::default();
        for event in [
            Event::Ports {
                request_id: 2,
                result: Ok(Vec::new()),
            },
            Event::Ports {
                request_id: 1,
                result: Err("stale".into()),
            },
            Event::Connection {
                activation: 5,
                result: Ok(true),
            },
            Event::Connection {
                activation: 4,
                result: Ok(false),
            },
        ] {
            outbox.send(&sender, event);
        }
        assert_eq!(outbox.wait(), RETRY_INTERVAL);
        let mut delivered = Vec::new();
        for _ in 0..3 {
            delivered.extend(results.try_recv());
            outbox.flush(&sender);
        }
        delivered.extend(results.try_recv());
        assert_eq!(outbox.wait(), IDLE_INTERVAL);
        assert!(matches!(
            delivered.as_slice(),
            [
                Event::Connection { activation: 0, .. },
                Event::Ports {
                    request_id: 2,
                    result: Ok(_)
                },
                Event::Connection {
                    activation: 5,
                    result: Ok(true)
                },
            ]
        ));
    }
    #[test]
    fn dropping_an_idle_worker_returns_promptly() {
        let worker = Worker::start().unwrap();
        // Ensure the worker is parked in its idle wait before shutdown.
        std::thread::sleep(Duration::from_millis(20));
        let started = std::time::Instant::now();
        drop(worker);
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_millis(50), "{elapsed:?}");
    }

    #[test]
    fn omni_channel_mode_delivers_all_sixteen_channels_with_wire_channel_preserved() {
        let mut decoder = Decoder::default();
        let held = HeldNoteInput::default();
        let notes = std::cell::RefCell::new(Vec::new());
        let controllers = std::cell::RefCell::new(Vec::new());
        // Notes, poly pressure, sustain, expression, and general CCs across channels 0, 5, and 15.
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
        // Pitch 60 must remain held because channel 5 has not released it yet.
        let HeldNotesRead::Snapshot(snapshot) = held.read() else {
            panic!("expected snapshot");
        };
        let mut pitches: Vec<u8> = snapshot.after(0).map(|n| n.pitch).collect();
        pitches.sort_unstable();
        assert_eq!(pitches, vec![60, 67]);

        // Releasing channel 5 drops pitch 60 while keeping channel 15's pitch 67.
        deliver_packets_with_notes_filtered(
            &mut decoder,
            ChannelFilter::Omni,
            final_ctrl,
            [&[0x85, 60, 0][..]],
            &held,
            &|_| {},
            &|_| {},
        );
        let HeldNotesRead::Snapshot(after_ch5_off) = held.read() else {
            panic!("expected snapshot");
        };
        let remaining: Vec<u8> = after_ch5_off.after(0).map(|n| n.pitch).collect();
        assert_eq!(remaining, vec![67]);
    }

    #[test]
    fn multi_port_aggregator_isolates_running_status_merges_held_notes_and_enforces_bounds() {
        let held = Arc::new(HeldNoteInput::default());
        // Bound checks: 0 ports, > MAX_AGGREGATED_PORTS, invalid single channel are rejected.
        assert!(
            MultiPortAggregator::new(
                0,
                ChannelFilter::Omni,
                MachTimebase::NANOSECONDS,
                Arc::clone(&held),
                Box::new(|_| {}),
                Box::new(|_| {}),
            )
            .is_err()
        );
        assert!(
            MultiPortAggregator::new(
                MAX_AGGREGATED_PORTS + 1,
                ChannelFilter::Omni,
                MachTimebase::NANOSECONDS,
                Arc::clone(&held),
                Box::new(|_| {}),
                Box::new(|_| {}),
            )
            .is_err()
        );
        assert!(
            MultiPortAggregator::new(
                2,
                ChannelFilter::Single(16),
                MachTimebase::NANOSECONDS,
                Arc::clone(&held),
                Box::new(|_| {}),
                Box::new(|_| {}),
            )
            .is_err()
        );

        let delivered = Arc::new(std::sync::Mutex::new(Vec::new()));
        let delivered_sink = Arc::clone(&delivered);
        let agg = MultiPortAggregator::new(
            2,
            ChannelFilter::Omni,
            MachTimebase::NANOSECONDS,
            Arc::clone(&held),
            Box::new(|_| {}),
            Box::new(move |ev| {
                delivered_sink.lock().unwrap().push(ev);
            }),
        )
        .unwrap();

        // Port 0 establishes running status 0x90 (ch 0 NoteOn) and presses pitch 60 at t=100.
        agg.deliver_timed_packets(
            0,
            [TimedPacket {
                timestamp_ticks: 100,
                data: &[0x90, 60, 100],
            }],
        );
        // Port 1 establishes its own independent running status 0x92 (ch 2 NoteOn) at t=90 (clamped monotonic to 100!)
        // and presses the same pitch 60 + pitch 64.
        agg.deliver_timed_packets(
            1,
            [TimedPacket {
                timestamp_ticks: 90,
                data: &[0x92, 60, 95, 64, 80],
            }],
        );
        // Port 0 uses its preserved running status 0x90 (without status byte!) to send note-off (vel 0) for pitch 60.
        agg.deliver_timed_packets(
            0,
            [TimedPacket {
                timestamp_ticks: 200,
                data: &[60, 0],
            }],
        );

        let events = delivered.lock().unwrap();
        assert_eq!(
            *events,
            vec![
                TimestampedNoteEvent {
                    timestamp_ns: 100,
                    event: NoteEvent::On {
                        channel: 0,
                        pitch: 60,
                        velocity: 100
                    }
                },
                TimestampedNoteEvent {
                    timestamp_ns: 100, // clamped monotonic against port 0's t=100
                    event: NoteEvent::On {
                        channel: 2,
                        pitch: 60,
                        velocity: 95
                    }
                },
                TimestampedNoteEvent {
                    timestamp_ns: 100,
                    event: NoteEvent::On {
                        channel: 2,
                        pitch: 64,
                        velocity: 80
                    }
                },
                TimestampedNoteEvent {
                    timestamp_ns: 200,
                    event: NoteEvent::Off {
                        channel: 0,
                        pitch: 60,
                        velocity: 0
                    }
                },
            ]
        );
        // Pitch 60 must remain held in the merged snapshot because port 1 still holds it!
        let HeldNotesRead::Snapshot(merged) = held.read() else {
            panic!("expected merged snapshot");
        };
        let mut pitches: Vec<u8> = merged.after(0).map(|n| n.pitch).collect();
        pitches.sort_unstable();
        assert_eq!(pitches, vec![60, 64]);
    }
}
