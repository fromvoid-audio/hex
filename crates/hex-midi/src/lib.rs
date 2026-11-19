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
mod note_events;
pub use held_notes::{
    HELD_NOTE_COUNT, HeldNote, HeldNoteFault, HeldNoteInput, HeldNotes, HeldNotesRead,
};
pub use note_events::NoteEvent;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controllers {
    pub pressure: u8,
    pub bend: i16,
    pub wheel: u8,
}
/// Exact channel-voice messages only; malformed input never changes the snapshot.
pub fn parse(message: &[u8], channel: u8, current: Controllers) -> Option<Controllers> {
    if !(2..=3).contains(&message.len()) {
        return None;
    }
    let (&status, data) = message.split_first()?;
    if channel >= 16 || status & 15 != channel || data.iter().any(|value| *value > 127) {
        return None;
    }
    let mut next = current;
    match (status & 0xf0, data) {
        (0xd0, [pressure]) => next.pressure = *pressure,
        (0xe0, [low, high]) => next.bend = (i16::from(*high) * 128 + i16::from(*low)) - 8192,
        (0xb0, [1, wheel]) => next.wheel = *wheel,
        (0xb0, [121, _]) => next = Controllers::default(),
        _ => return None,
    }
    Some(next)
}
/// Fixed MIDI 1 byte-stream decoder, including running status across packets.
/// Realtime bytes may interleave anywhere. SysEx/system-common cancel channel
/// running status; unsupported channel messages are consumed at their exact width.
#[derive(Default)]
pub struct Decoder {
    status: u8,
    data: [u8; 2],
    used: usize,
    sysex: bool,
    held: HeldNotes,
    held_changed: bool,
    held_fault: Option<HeldNoteFault>,
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
    pub fn held_note_fault(&self) -> Option<HeldNoteFault> {
        self.held_fault
    }
    /// Releases every held key and drops partial/running status, as MIDI reset
    /// does. Note tokens stay monotonic for the activation's control-side floor.
    fn reset(&mut self, note: &dyn Fn(NoteEvent)) {
        self.held.clear();
        self.held_changed = true;
        self.status = 0;
        self.used = 0;
        self.sysex = false;
        note(NoteEvent::AllNotesOff);
    }
    pub fn byte(&mut self, byte: u8, channel: u8, current: Controllers) -> Option<Controllers> {
        self.byte_with_notes(byte, channel, current, &|_| {})
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
        if byte == 0xff {
            self.reset(note);
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
            match self.held.observe(&message[..width + 1], channel) {
                Ok(changed) => self.held_changed |= changed,
                Err(fault) => {
                    self.held.clear();
                    self.held_fault = Some(fault);
                    self.held_changed = true;
                    note(NoteEvent::InputLost(fault));
                }
            }
            if self.held_fault.is_none()
                && let Some(event) = NoteEvent::parse(&message[..width + 1], channel)
            {
                note(event);
            }
        }
        parse(&message[..width + 1], channel, current)
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
    deliver_packets_with_notes(decoder, channel, current, packets, notes, publish, &|_| {})
}

/// Ordered-note counterpart of `deliver_packets`, sharing its byte/packet and
/// held-snapshot limits. Burst overflow publishes `AllNotesOff` and continues on
/// the next invocation. Held-note faults are terminal: publish one explicit loss
/// event and stop delivery. The sink must cancel its host activation on loss; it
/// must never silently drop a note-off when its own handoff is full.
pub fn deliver_packets_with_notes<'a>(
    decoder: &mut Decoder,
    channel: u8,
    mut current: Controllers,
    packets: impl IntoIterator<Item = &'a [u8]>,
    notes: &HeldNoteInput,
    publish: &dyn Fn(Controllers),
    note: &dyn Fn(NoteEvent),
) -> Controllers {
    if notes.is_cancelled() {
        return current;
    }
    let mut remaining = CALLBACK_BYTES;
    // One packet past the limit is inspected only to detect overflow.
    for (index, bytes) in packets.into_iter().take(CALLBACK_PACKETS + 1).enumerate() {
        let overflow = index == CALLBACK_PACKETS || bytes.len() > remaining;
        let accepted = if index == CALLBACK_PACKETS {
            &[][..]
        } else {
            bytes.get(..remaining).unwrap_or(bytes)
        };
        for byte in accepted {
            if let Some(next) = decoder.byte_with_notes(*byte, channel, current, note) {
                current = next;
                publish(current);
            }
            if let Some(fault) = decoder.held_note_fault() {
                notes.fail(fault);
                return current;
            }
        }
        if overflow {
            decoder.reset(note);
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub id: u32,
    pub name: String,
}
/// Constructed on control; the native callback only invokes this bounded sink.
pub struct Sink {
    pub publish: Box<dyn Fn(Controllers) + Send>,
    /// Ordered native input messages, independent of the step-entry snapshot.
    pub note: Box<dyn Fn(NoteEvent) + Send>,
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
                if let Request::Command(Command::Connect { sink, .. }) = request {
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
                Err(mpsc::TrySendError::Full(event)) => self.retain(event),
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
            Ok(Request::Command(Command::Connect { sink, .. })) => sink.cancel(),
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
        // Rust drops fields in declaration order: input port closes before client.
        activation: u64,
        _port: InputPort,
        _client: Client,
        source: Source,
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
    fn connect(activation: u64, port: Port, channel: u8, sink: Sink) -> Result<Active, String> {
        let result = (|| {
            if sink.notes.is_cancelled() || (sink.is_cancelled)() {
                return Err("controller activation was cancelled".into());
            }
            if channel >= 16 {
                return Err("controller channel is invalid".into());
            }
            let source = source_by_id(port.id).ok_or("controller port is unavailable")?;
            let client = Client::new("hex controller input")
                .map_err(|status| format!("controller client failed: {status}"))?;
            let mut decoder = Decoder::default();
            let mut current = Controllers::default();
            let publish = sink.publish;
            let note = sink.note;
            let notes = Arc::clone(&sink.notes);
            let input = client
                .input_port("hex controller input", move |packets| {
                    current = deliver_packets_with_notes(
                        &mut decoder,
                        channel,
                        current,
                        packets.iter().map(|packet| packet.data()),
                        &notes,
                        publish.as_ref(),
                        note.as_ref(),
                    );
                })
                .map_err(|status| format!("controller port creation failed: {status}"))?;
            input
                .connect_source(&source)
                .map_err(|status| format!("controller connection failed: {status}"))?;
            Ok((input, client, source))
        })();
        match result {
            Ok((input, client, source)) => Ok(Active {
                activation,
                _port: input,
                _client: client,
                source,
                cancel: sink.cancel,
                is_cancelled: sink.is_cancelled,
                notes: sink.notes,
            }),
            Err(error) => {
                // publish moved into the callback; cancel the remaining owners.
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
        let mut active: Option<Active> = None;
        let mut outbox = Outbox::default();
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
                    let result = match connect(activation, port, channel, sink) {
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
                let present = connection
                    .source
                    .unique_id()
                    .and_then(source_by_id)
                    .is_some_and(|source| source == connection.source);
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
        let mut outbox = Outbox::default();
        while !stop.load(Ordering::Acquire) {
            outbox.flush(&events);
            match requests.recv_timeout(outbox.wait()) {
                Ok(Request::Shutdown) => break,
                Ok(Request::Command(Command::Connect {
                    activation, sink, ..
                })) => {
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
        assert_eq!(parse(&[0xb2, 121, 0], 2, wheel), Some(initial));
        for malformed in [
            &[0xd2][..],
            &[0xd2, 128],
            &[0xd2, 1, 2],
            &[0xe2, 0],
            &[0xb2, 1],
            &[0x92, 60, 127],
            &[0xb2, 64, 127],
            &[0xd1, 127],
            &[0xf8],
            &[0xf0, 1, 0xf7],
        ] {
            assert_eq!(parse(malformed, 2, wheel), None, "{malformed:?}");
        }
        assert_eq!(parse(&[0xd0, 1], 16, wheel), None);
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
                wheel: 30
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
        let worker = {
            let (events, stop) = (filler.clone(), Arc::clone(&stop));
            std::thread::spawn(move || native::run(requests, events, stop))
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
        // Let the worker answer into the full queue before anything is drained.
        std::thread::sleep(Duration::from_millis(50));
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
}
