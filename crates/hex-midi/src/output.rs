//! Control-side hardware MIDI output owner, bounded nonblocking timestamped
//! event handoff, active-note ledger, and deterministic terminal cleanup.
//!
//! # Contract
//! - Endpoint discovery and selection run strictly on the control-side
//!   [`OutputWorker`] thread, keyed by stable platform endpoint identity
//!   ([`OutputPort::id`], never port index or name fallback).
//! - Real-time / audio-callback senders interact only with [`OutputProducer`],
//!   whose operations are wait-free, allocation-free, and lock-free.
//! - Every producer send carries an `activation: u64` generation token; stale,
//!   zero, or disconnected activations are refused without mutating endpoint
//!   state or touching the wire.
//! - Saturation policy: ordinary messages (`NoteOn`, CC, expression, program
//!   change) use a fixed [`OUTPUT_NORMAL_CAPACITY`] SPSC ring. When saturated,
//!   ordinary messages return [`OutputEnqueueResult::Overflowed`], while
//!   `NoteOff` and channel-release messages spill into a dedicated
//!   [`OUTPUT_PRIORITY_CAPACITY`] ring so releases are not lost behind bursts.
//!   If even the priority ring saturates, an atomic panic-cleanup latch forces
//!   deterministic active-note cleanup on the worker.
//! - Stop priority: [`OutputProducer::request_stop`] bumps an atomic epoch
//!   that works under 100% queue saturation, discarding pending queued notes
//!   and immediately releasing all sounding notes and held sustain pedals.
//! - Failure preservation: invalid or failed `Connect` commands and stale
//!   `Disconnect` commands leave any currently active connection and sounding
//!   notes untouched.

use crate::output_native;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};
use std::thread::{JoinHandle, Thread};
use std::time::Duration;

/// Maximum number of hardware MIDI output ports enumerated by one discovery call.
pub const MAX_OUTPUT_PORTS: usize = 127;
/// Maximum byte length of a validated MIDI output port display name.
pub const MAX_OUTPUT_PORT_NAME_BYTES: usize = 1024;
/// Fixed power-of-two capacity of the normal timestamped output SPSC ring.
pub const OUTPUT_NORMAL_CAPACITY: usize = 512;
/// Fixed power-of-two capacity of the priority NoteOff/release SPSC ring.
pub const OUTPUT_PRIORITY_CAPACITY: usize = 256;
/// Maximum queued output events drained per worker wakeup before re-checking
/// control commands, stop requests, and endpoint presence.
pub const OUTPUT_MAX_EVENTS_PER_DRAIN: usize = 256;

const OUTPUT_COMMAND_CAPACITY: usize = 4;
const OUTPUT_EVENT_CAPACITY: usize = 4;
const OUTPUT_RETRY_INTERVAL: Duration = Duration::from_millis(20);
const OUTPUT_IDLE_INTERVAL: Duration = Duration::from_millis(500);
const CHANNEL_COUNT: usize = 16;
const PITCH_COUNT: usize = 128;

/// Stable hardware MIDI output endpoint descriptor.
///
/// Selection is strictly by [`Self::id`] (CoreMIDI `kMIDIPropertyUniqueID` on
/// macOS), never by list index or display name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OutputPort {
    pub id: u32,
    pub name: String,
}

/// Validation errors for outbound MIDI 1.0 channel messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputError {
    InvalidChannel,
    InvalidDataByte,
    InvalidVelocity,
    InvalidPitchBend,
}

/// Validated 2- or 3-byte MIDI 1.0 wire message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncodedMidiMessage {
    pub bytes: [u8; 3],
    pub len: u8,
}

impl EncodedMidiMessage {
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len).min(3)]
    }

    fn pack(self) -> u32 {
        u32::from(self.bytes[0])
            | (u32::from(self.bytes[1]) << 8)
            | (u32::from(self.bytes[2]) << 16)
            | (u32::from(self.len) << 24)
    }

    fn unpack(word: u32) -> Self {
        Self {
            bytes: [
                (word & 0xff) as u8,
                ((word >> 8) & 0xff) as u8,
                ((word >> 16) & 0xff) as u8,
            ],
            len: ((word >> 24) & 0xff).min(3) as u8,
        }
    }
}

/// Typed outbound MIDI 1.0 channel voice and channel mode messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMessage {
    NoteOn {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    PolyPressure {
        channel: u8,
        pitch: u8,
        value: u8,
    },
    ControlChange {
        channel: u8,
        cc: u8,
        value: u8,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
    ChannelPressure {
        channel: u8,
        value: u8,
    },
    /// 14-bit unsigned MIDI pitch bend (`0..=16383`, center `8192`).
    PitchBend {
        channel: u8,
        value: u16,
    },
    AllNotesOff {
        channel: u8,
    },
    AllSoundOff {
        channel: u8,
    },
    ResetAllControllers {
        channel: u8,
    },
}

impl OutputMessage {
    pub fn channel(self) -> u8 {
        match self {
            Self::NoteOn { channel, .. }
            | Self::NoteOff { channel, .. }
            | Self::PolyPressure { channel, .. }
            | Self::ControlChange { channel, .. }
            | Self::ProgramChange { channel, .. }
            | Self::ChannelPressure { channel, .. }
            | Self::PitchBend { channel, .. }
            | Self::AllNotesOff { channel }
            | Self::AllSoundOff { channel }
            | Self::ResetAllControllers { channel } => channel,
        }
    }

    pub fn is_note_off_or_release(self) -> bool {
        match self {
            Self::NoteOff { .. } | Self::AllNotesOff { .. } | Self::AllSoundOff { .. } => true,
            Self::ControlChange { cc, value, .. } => {
                (cc == 64 && value < 64) || matches!(cc, 120 | 123..=127)
            }
            _ => false,
        }
    }

    pub fn encode(self) -> Result<EncodedMidiMessage, OutputError> {
        let ch = self.channel();
        if ch > 15 {
            return Err(OutputError::InvalidChannel);
        }
        match self {
            Self::NoteOn {
                pitch, velocity, ..
            } => {
                if pitch > 127 {
                    return Err(OutputError::InvalidDataByte);
                }
                if velocity == 0 || velocity > 127 {
                    return Err(OutputError::InvalidVelocity);
                }
                Ok(EncodedMidiMessage {
                    bytes: [0x90 | ch, pitch, velocity],
                    len: 3,
                })
            }
            Self::NoteOff {
                pitch, velocity, ..
            } => {
                if pitch > 127 || velocity > 127 {
                    return Err(OutputError::InvalidDataByte);
                }
                Ok(EncodedMidiMessage {
                    bytes: [0x80 | ch, pitch, velocity],
                    len: 3,
                })
            }
            Self::PolyPressure { pitch, value, .. } => {
                if pitch > 127 || value > 127 {
                    return Err(OutputError::InvalidDataByte);
                }
                Ok(EncodedMidiMessage {
                    bytes: [0xa0 | ch, pitch, value],
                    len: 3,
                })
            }
            Self::ControlChange { cc, value, .. } => {
                if cc > 127 || value > 127 {
                    return Err(OutputError::InvalidDataByte);
                }
                Ok(EncodedMidiMessage {
                    bytes: [0xb0 | ch, cc, value],
                    len: 3,
                })
            }
            Self::ProgramChange { program, .. } => {
                if program > 127 {
                    return Err(OutputError::InvalidDataByte);
                }
                Ok(EncodedMidiMessage {
                    bytes: [0xc0 | ch, program, 0],
                    len: 2,
                })
            }
            Self::ChannelPressure { value, .. } => {
                if value > 127 {
                    return Err(OutputError::InvalidDataByte);
                }
                Ok(EncodedMidiMessage {
                    bytes: [0xd0 | ch, value, 0],
                    len: 2,
                })
            }
            Self::PitchBend { value, .. } => {
                if value > 16_383 {
                    return Err(OutputError::InvalidPitchBend);
                }
                let lsb = (value & 0x7f) as u8;
                let msb = ((value >> 7) & 0x7f) as u8;
                Ok(EncodedMidiMessage {
                    bytes: [0xe0 | ch, lsb, msb],
                    len: 3,
                })
            }
            Self::AllNotesOff { .. } => Ok(EncodedMidiMessage {
                bytes: [0xb0 | ch, 123, 0],
                len: 3,
            }),
            Self::AllSoundOff { .. } => Ok(EncodedMidiMessage {
                bytes: [0xb0 | ch, 120, 0],
                len: 3,
            }),
            Self::ResetAllControllers { .. } => Ok(EncodedMidiMessage {
                bytes: [0xb0 | ch, 121, 0],
                len: 3,
            }),
        }
    }
}

/// Outbound MIDI message paired with its monotonic nanosecond delivery timestamp
/// (`0` requests immediate delivery).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimestampedOutputMessage {
    pub timestamp_ns: u64,
    pub message: OutputMessage,
}

/// Result of a wait-free producer enqueue or stop request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputEnqueueResult {
    Queued,
    QueuedPriority,
    InvalidMessage(OutputError),
    StaleActivation,
    Disconnected,
    Overflowed,
    PriorityOverflowCleanupLatched,
}

/// Backend send error distinguishing endpoint disappearance from transient I/O errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputSendError {
    Disconnected(String),
    Transient(String),
}

/// Connection state emitted in [`OutputEvent::Connection`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputConnectionState {
    Connected(OutputPort),
    Disconnected,
}

/// Control-side commands accepted by [`OutputHandle`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputCommand {
    Discover { request_id: u64 },
    Connect { activation: u64, port: OutputPort },
    Disconnect { activation: u64 },
    Stop { activation: u64 },
}

/// Replies emitted by the control-side [`OutputWorker`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputEvent {
    Ports {
        request_id: u64,
        result: Result<Vec<OutputPort>, String>,
    },
    Connection {
        activation: u64,
        result: Result<OutputConnectionState, String>,
    },
}

/// Platform or synthetic MIDI output endpoint backend driven strictly on the
/// control worker thread.
pub trait OutputBackend: Send + 'static {
    type Endpoint: Send;

    /// Called once on the worker thread with a notification callback that
    /// backends may invoke on asynchronous OS setup/endpoint changes.
    fn install_setup_notifier(&mut self, _notify: Arc<dyn Fn() + Send + Sync>) {}

    fn discover(&mut self) -> Result<Vec<OutputPort>, String>;

    fn connect(&mut self, port: &OutputPort) -> Result<Self::Endpoint, String>;

    fn is_present(&mut self, endpoint: &Self::Endpoint) -> bool;

    fn send(
        &mut self,
        endpoint: &mut Self::Endpoint,
        timestamp_ns: u64,
        message: EncodedMidiMessage,
    ) -> Result<(), OutputSendError>;

    fn close(&mut self, endpoint: Self::Endpoint);
}

/// Lock-free wake primitive shared between control handles, CoreMIDI setup
/// notifications, and wait-free audio-callback producers.
pub(crate) struct OutputWake {
    parked: AtomicBool,
    thread: OnceLock<Thread>,
}

impl OutputWake {
    fn new() -> Self {
        Self {
            parked: AtomicBool::new(false),
            thread: OnceLock::new(),
        }
    }

    fn register_current(&self) {
        let _ = self.thread.set(std::thread::current());
    }

    pub(crate) fn unpark_if_parked(&self) {
        if self.parked.swap(false, Ordering::AcqRel)
            && let Some(thread) = self.thread.get()
        {
            thread.unpark();
        }
    }

    fn force_unpark(&self) {
        self.parked.store(false, Ordering::Release);
        if let Some(thread) = self.thread.get() {
            thread.unpark();
        }
    }
}

struct OutputSlot {
    sequence: AtomicU64,
    timestamp_ns: AtomicU64,
    packed: AtomicU32,
}

impl OutputSlot {
    fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            timestamp_ns: AtomicU64::new(0),
            packed: AtomicU32::new(0),
        }
    }

    fn write(&self, sequence: u64, timestamp_ns: u64, message: EncodedMidiMessage) {
        self.sequence.store(sequence, Ordering::Relaxed);
        self.timestamp_ns.store(timestamp_ns, Ordering::Relaxed);
        self.packed.store(message.pack(), Ordering::Relaxed);
    }

    fn read(&self) -> QueuedOutputItem {
        QueuedOutputItem {
            sequence: self.sequence.load(Ordering::Relaxed),
            timestamp_ns: self.timestamp_ns.load(Ordering::Relaxed),
            message: EncodedMidiMessage::unpack(self.packed.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QueuedOutputItem {
    sequence: u64,
    timestamp_ns: u64,
    message: EncodedMidiMessage,
}

struct OutputRing<const N: usize> {
    slots: Box<[OutputSlot; N]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl<const N: usize> OutputRing<N> {
    fn new() -> Self {
        debug_assert!(N.is_power_of_two());
        Self {
            slots: Box::new(std::array::from_fn(|_| OutputSlot::new())),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    fn push(&self, sequence: u64, timestamp_ns: u64, message: EncodedMidiMessage) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) >= N {
            return false;
        }
        self.slots[tail & (N - 1)].write(sequence, timestamp_ns, message);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        true
    }

    fn peek(&self) -> Option<QueuedOutputItem> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        Some(self.slots[head & (N - 1)].read())
    }

    fn pop(&self) -> Option<QueuedOutputItem> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let item = self.slots[head & (N - 1)].read();
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(item)
    }

    fn discard_all(&self) {
        let tail = self.tail.load(Ordering::Acquire);
        self.head.store(tail, Ordering::Release);
    }

    fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        head == tail
    }
}

struct SharedOutputHandoff {
    normal: OutputRing<OUTPUT_NORMAL_CAPACITY>,
    priority: OutputRing<OUTPUT_PRIORITY_CAPACITY>,
    active_activation: AtomicU64,
    next_sequence: AtomicU64,
    stop_epoch: AtomicU64,
    panic_cleanup_latched: AtomicBool,
    normal_overflow_count: AtomicU64,
    closed: AtomicBool,
    wake: Arc<OutputWake>,
}

impl SharedOutputHandoff {
    fn new(wake: Arc<OutputWake>) -> Self {
        Self {
            normal: OutputRing::new(),
            priority: OutputRing::new(),
            active_activation: AtomicU64::new(0),
            next_sequence: AtomicU64::new(0),
            stop_epoch: AtomicU64::new(0),
            panic_cleanup_latched: AtomicBool::new(false),
            normal_overflow_count: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            wake,
        }
    }

    fn has_pending_work(&self, handled_stop_epoch: u64) -> bool {
        self.stop_epoch.load(Ordering::Acquire) != handled_stop_epoch
            || self.panic_cleanup_latched.load(Ordering::Acquire)
            || !self.normal.is_empty()
            || !self.priority.is_empty()
    }
}

/// Wait-free, allocation-free, single-producer handle for sending timestamped
/// MIDI events and priority Stop requests to the control [`OutputWorker`].
///
/// Safe to hold and call inside real-time audio callbacks: performs only atomic
/// operations and never allocates, locks a mutex, or performs OS I/O.
pub struct OutputProducer {
    shared: Arc<SharedOutputHandoff>,
}

impl OutputProducer {
    /// Returns the currently active output activation generation (`0` if
    /// disconnected or shut down).
    pub fn active_activation(&self) -> u64 {
        if self.shared.closed.load(Ordering::Acquire) {
            return 0;
        }
        self.shared.active_activation.load(Ordering::Acquire)
    }

    /// Cumulative count of ordinary messages dropped because the normal output
    /// ring was full.
    pub fn overflow_count(&self) -> u64 {
        self.shared.normal_overflow_count.load(Ordering::Relaxed)
    }

    /// Wait-free, allocation-free enqueue of one timestamped MIDI message for
    /// `activation`.
    pub fn try_send(
        &self,
        activation: u64,
        event: TimestampedOutputMessage,
    ) -> OutputEnqueueResult {
        let encoded = match event.message.encode() {
            Ok(encoded) => encoded,
            Err(error) => return OutputEnqueueResult::InvalidMessage(error),
        };
        if self.shared.closed.load(Ordering::Acquire) {
            return OutputEnqueueResult::Disconnected;
        }
        let current = self.shared.active_activation.load(Ordering::Acquire);
        if current == 0 {
            return OutputEnqueueResult::Disconnected;
        }
        if activation == 0 || activation != current {
            return OutputEnqueueResult::StaleActivation;
        }
        let seq = self.shared.next_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        if self.shared.normal.push(seq, event.timestamp_ns, encoded) {
            self.shared.wake.unpark_if_parked();
            return OutputEnqueueResult::Queued;
        }
        if event.message.is_note_off_or_release() {
            if self.shared.priority.push(seq, event.timestamp_ns, encoded) {
                self.shared.wake.unpark_if_parked();
                return OutputEnqueueResult::QueuedPriority;
            }
            self.shared
                .panic_cleanup_latched
                .store(true, Ordering::Release);
            self.shared.wake.unpark_if_parked();
            return OutputEnqueueResult::PriorityOverflowCleanupLatched;
        }
        self.shared
            .normal_overflow_count
            .fetch_add(1, Ordering::Relaxed);
        self.shared.wake.unpark_if_parked();
        OutputEnqueueResult::Overflowed
    }

    /// Wait-free, allocation-free priority Stop / panic cleanup request.
    ///
    /// Remains effective even when both event rings are 100% full: the worker
    /// immediately discards queued pending messages and releases every sounding
    /// note and held sustain pedal on the active endpoint.
    pub fn request_stop(&self, activation: u64) -> OutputEnqueueResult {
        if self.shared.closed.load(Ordering::Acquire) {
            return OutputEnqueueResult::Disconnected;
        }
        let current = self.shared.active_activation.load(Ordering::Acquire);
        if current == 0 {
            return OutputEnqueueResult::Disconnected;
        }
        if activation == 0 || activation != current {
            return OutputEnqueueResult::StaleActivation;
        }
        self.shared.stop_epoch.fetch_add(1, Ordering::AcqRel);
        self.shared.wake.unpark_if_parked();
        OutputEnqueueResult::QueuedPriority
    }
}

/// Fixed-size active-note and sustain-pedal tracker owned solely by the output
/// worker thread for deterministic terminal cleanup (zero heap allocation).
struct ActiveNoteLedger {
    counts: [u8; CHANNEL_COUNT * PITCH_COUNT],
    sustain_held: [bool; CHANNEL_COUNT],
    active_channels: u16,
}

impl Default for ActiveNoteLedger {
    fn default() -> Self {
        Self {
            counts: [0; CHANNEL_COUNT * PITCH_COUNT],
            sustain_held: [false; CHANNEL_COUNT],
            active_channels: 0,
        }
    }
}

impl ActiveNoteLedger {
    fn observe_sent(&mut self, msg: EncodedMidiMessage) {
        if msg.len < 3 {
            return;
        }
        let status = msg.bytes[0];
        let channel = usize::from(status & 0x0f);
        let d1 = msg.bytes[1];
        let d2 = msg.bytes[2];
        if d1 > 127 || d2 > 127 {
            return;
        }
        let row = channel * PITCH_COUNT;
        match status & 0xf0 {
            0x90 if d2 != 0 => {
                let count = &mut self.counts[row + usize::from(d1)];
                *count = count.saturating_add(1);
                self.active_channels |= 1_u16 << channel;
            }
            0x80 | 0x90 => {
                let count = &mut self.counts[row + usize::from(d1)];
                *count = count.saturating_sub(1);
            }
            0xb0 if d1 == 64 => {
                if d2 >= 64 {
                    self.sustain_held[channel] = true;
                    self.active_channels |= 1_u16 << channel;
                } else {
                    self.sustain_held[channel] = false;
                }
            }
            0xb0 if matches!(d1, 120 | 123..=127) => {
                self.counts[row..row + PITCH_COUNT].fill(0);
            }
            0xb0 if d1 == 121 => {
                self.sustain_held[channel] = false;
            }
            _ => {}
        }
    }

    fn drain_cleanup(&mut self, mut emit: impl FnMut(EncodedMidiMessage)) {
        for channel in 0..CHANNEL_COUNT {
            let row = channel * PITCH_COUNT;
            let mut channel_had_notes = false;
            for pitch in 0..PITCH_COUNT {
                if self.counts[row + pitch] != 0 {
                    self.counts[row + pitch] = 0;
                    channel_had_notes = true;
                    emit(EncodedMidiMessage {
                        bytes: [0x80 | (channel as u8), pitch as u8, 0],
                        len: 3,
                    });
                }
            }
            let had_sustain = self.sustain_held[channel];
            if had_sustain {
                self.sustain_held[channel] = false;
                emit(EncodedMidiMessage {
                    bytes: [0xb0 | (channel as u8), 64, 0],
                    len: 3,
                });
            }
            if channel_had_notes || had_sustain || (self.active_channels & (1_u16 << channel) != 0)
            {
                emit(EncodedMidiMessage {
                    bytes: [0xb0 | (channel as u8), 123, 0],
                    len: 3,
                });
                emit(EncodedMidiMessage {
                    bytes: [0xb0 | (channel as u8), 120, 0],
                    len: 3,
                });
            }
        }
        self.active_channels = 0;
    }
}

#[derive(Default)]
struct OutputOutbox {
    ports: Option<(u64, Result<Vec<OutputPort>, String>)>,
    connection: Option<(u64, Result<OutputConnectionState, String>)>,
}

impl OutputOutbox {
    fn retain(&mut self, event: OutputEvent) {
        match event {
            OutputEvent::Ports { request_id, result } => {
                if self
                    .ports
                    .as_ref()
                    .is_none_or(|(held, _)| *held <= request_id)
                {
                    self.ports = Some((request_id, result));
                }
            }
            OutputEvent::Connection { activation, result } => {
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

    fn flush(&mut self, events: &SyncSender<OutputEvent>) {
        let ports = self
            .ports
            .take()
            .map(|(request_id, result)| OutputEvent::Ports { request_id, result });
        let connection = self
            .connection
            .take()
            .map(|(activation, result)| OutputEvent::Connection { activation, result });
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

    fn send(&mut self, events: &SyncSender<OutputEvent>, event: OutputEvent) {
        self.retain(event);
        self.flush(events);
    }

    fn wait(&self) -> Duration {
        if self.ports.is_some() || self.connection.is_some() {
            OUTPUT_RETRY_INTERVAL
        } else {
            OUTPUT_IDLE_INTERVAL
        }
    }
}

enum OutputRequest {
    Command(OutputCommand),
    Shutdown,
}

/// Cloneable control-side command/event handle to one [`OutputWorker`].
#[derive(Clone)]
pub struct OutputHandle {
    commands: SyncSender<OutputRequest>,
    events: Arc<std::sync::Mutex<Receiver<OutputEvent>>>,
    wake: Arc<OutputWake>,
}

impl OutputHandle {
    pub fn send(&self, command: OutputCommand) -> Result<(), &'static str> {
        match self.commands.try_send(OutputRequest::Command(command)) {
            Ok(()) => {
                self.wake.unpark_if_parked();
                Ok(())
            }
            Err(_) => Err("midi output worker is busy or unavailable"),
        }
    }

    pub fn try_event(&self) -> Result<OutputEvent, TryRecvError> {
        self.events
            .lock()
            .map_err(|_| TryRecvError::Disconnected)?
            .try_recv()
    }
}

struct ActiveOutput<E> {
    activation: u64,
    endpoint: E,
    ledger: ActiveNoteLedger,
}

fn validate_port(port: &OutputPort) -> Result<(), String> {
    if port.name.is_empty() || port.name.len() > MAX_OUTPUT_PORT_NAME_BYTES {
        return Err("midi output port name is invalid".into());
    }
    Ok(())
}

fn validate_discovered_ports(ports: Vec<OutputPort>) -> Result<Vec<OutputPort>, String> {
    if ports.len() > MAX_OUTPUT_PORTS {
        return Err("midi output exceeds 127 ports".into());
    }
    for port in &ports {
        validate_port(port)?;
    }
    Ok(ports)
}

fn emit_cleanup<B: OutputBackend>(backend: &mut B, active: &mut ActiveOutput<B::Endpoint>) {
    let mut disconnected = false;
    active.ledger.drain_cleanup(|msg| {
        if !disconnected
            && matches!(
                backend.send(&mut active.endpoint, 0, msg),
                Err(OutputSendError::Disconnected(_))
            )
        {
            disconnected = true;
        }
    });
}

fn pop_next_in_order(handoff: &SharedOutputHandoff) -> Option<QueuedOutputItem> {
    match (handoff.normal.peek(), handoff.priority.peek()) {
        (Some(n), Some(p)) => {
            if n.sequence <= p.sequence {
                handoff.normal.pop()
            } else {
                handoff.priority.pop()
            }
        }
        (Some(_), None) => handoff.normal.pop(),
        (None, Some(_)) => handoff.priority.pop(),
        (None, None) => None,
    }
}

fn run_output_worker<B: OutputBackend>(
    mut backend: B,
    requests: Receiver<OutputRequest>,
    events: SyncSender<OutputEvent>,
    handoff: Arc<SharedOutputHandoff>,
    stopping: Arc<AtomicBool>,
    wake: Arc<OutputWake>,
) {
    wake.register_current();
    let setup_changed = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&setup_changed);
        let notify_wake = Arc::clone(&wake);
        backend.install_setup_notifier(Arc::new(move || {
            flag.store(true, Ordering::Release);
            notify_wake.unpark_if_parked();
        }));
    }

    let mut outbox = OutputOutbox::default();
    let mut active: Option<ActiveOutput<B::Endpoint>> = None;
    let mut highest_activation: u64 = 0;
    let mut handled_stop_epoch: u64 = handoff.stop_epoch.load(Ordering::Acquire);

    while !stopping.load(Ordering::Acquire) {
        outbox.flush(&events);

        // 1. Drain bounded control commands without blocking.
        let mut shutdown_requested = false;
        for _ in 0..=OUTPUT_COMMAND_CAPACITY {
            match requests.try_recv() {
                Ok(OutputRequest::Shutdown) => {
                    shutdown_requested = true;
                    break;
                }
                Ok(OutputRequest::Command(OutputCommand::Discover { request_id })) => {
                    let result = backend.discover().and_then(validate_discovered_ports);
                    outbox.send(&events, OutputEvent::Ports { request_id, result });
                }
                Ok(OutputRequest::Command(OutputCommand::Connect { activation, port })) => {
                    let result = if activation == 0 || activation <= highest_activation {
                        Err("stale midi output activation".into())
                    } else if let Err(error) = validate_port(&port) {
                        Err(error)
                    } else {
                        match backend.connect(&port) {
                            Ok(new_endpoint) => {
                                // Suspend producer writes before retiring the previous endpoint
                                // so old-activation events cannot bleed into the new endpoint.
                                handoff.active_activation.store(0, Ordering::Release);
                                handoff.normal.discard_all();
                                handoff.priority.discard_all();
                                handoff
                                    .panic_cleanup_latched
                                    .store(false, Ordering::Release);
                                if let Some(mut old) = active.take() {
                                    emit_cleanup(&mut backend, &mut old);
                                    backend.close(old.endpoint);
                                }
                                highest_activation = activation;
                                handled_stop_epoch = handoff.stop_epoch.load(Ordering::Acquire);
                                active = Some(ActiveOutput {
                                    activation,
                                    endpoint: new_endpoint,
                                    ledger: ActiveNoteLedger::default(),
                                });
                                handoff
                                    .active_activation
                                    .store(activation, Ordering::Release);
                                Ok(OutputConnectionState::Connected(port))
                            }
                            Err(error) => Err(error),
                        }
                    };
                    outbox.send(&events, OutputEvent::Connection { activation, result });
                }
                Ok(OutputRequest::Command(OutputCommand::Disconnect { activation })) => {
                    if active
                        .as_ref()
                        .is_some_and(|current| current.activation == activation)
                    {
                        handoff.active_activation.store(0, Ordering::Release);
                        handoff.normal.discard_all();
                        handoff.priority.discard_all();
                        handoff
                            .panic_cleanup_latched
                            .store(false, Ordering::Release);
                        if let Some(mut old) = active.take() {
                            emit_cleanup(&mut backend, &mut old);
                            backend.close(old.endpoint);
                        }
                    }
                    outbox.send(
                        &events,
                        OutputEvent::Connection {
                            activation,
                            result: Ok(OutputConnectionState::Disconnected),
                        },
                    );
                }
                Ok(OutputRequest::Command(OutputCommand::Stop { activation })) => {
                    if let Some(current) = active
                        .as_mut()
                        .filter(|current| current.activation == activation)
                    {
                        handoff.normal.discard_all();
                        handoff.priority.discard_all();
                        handoff
                            .panic_cleanup_latched
                            .store(false, Ordering::Release);
                        handled_stop_epoch = handoff.stop_epoch.load(Ordering::Acquire);
                        emit_cleanup(&mut backend, current);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    shutdown_requested = true;
                    break;
                }
            }
        }
        if shutdown_requested || stopping.load(Ordering::Acquire) {
            break;
        }

        // 2. Handle priority Stop / priority-overflow cleanup and drain bounded events.
        let mut lost_endpoint: Option<(u64, String)> = None;
        if let Some(current) = active.as_mut() {
            let current_stop = handoff.stop_epoch.load(Ordering::Acquire);
            let panic_cleanup = handoff.panic_cleanup_latched.swap(false, Ordering::AcqRel);
            if current_stop != handled_stop_epoch || panic_cleanup {
                handled_stop_epoch = current_stop;
                handoff.normal.discard_all();
                handoff.priority.discard_all();
                emit_cleanup(&mut backend, current);
            } else {
                for _ in 0..OUTPUT_MAX_EVENTS_PER_DRAIN {
                    // Re-check Stop before every event so Stop preempts even mid-drain.
                    let latest_stop = handoff.stop_epoch.load(Ordering::Acquire);
                    if latest_stop != handled_stop_epoch
                        || handoff.panic_cleanup_latched.swap(false, Ordering::AcqRel)
                    {
                        handled_stop_epoch = latest_stop;
                        handoff.normal.discard_all();
                        handoff.priority.discard_all();
                        emit_cleanup(&mut backend, current);
                        break;
                    }
                    let Some(item) = pop_next_in_order(&handoff) else {
                        break;
                    };
                    match backend.send(&mut current.endpoint, item.timestamp_ns, item.message) {
                        Ok(()) => current.ledger.observe_sent(item.message),
                        Err(OutputSendError::Disconnected(reason)) => {
                            lost_endpoint = Some((current.activation, reason));
                            break;
                        }
                        Err(OutputSendError::Transient(_)) => {}
                    }
                }
            }
        } else {
            // No active connection; keep queues clean.
            handoff.normal.discard_all();
            handoff.priority.discard_all();
            handoff
                .panic_cleanup_latched
                .store(false, Ordering::Release);
            handled_stop_epoch = handoff.stop_epoch.load(Ordering::Acquire);
        }

        // 3. Check OS setup changes or endpoint presence.
        if lost_endpoint.is_none()
            && let Some(current) = active.as_ref()
        {
            let _ = setup_changed.swap(false, Ordering::AcqRel);
            if !backend.is_present(&current.endpoint) {
                lost_endpoint = Some((current.activation, "midi output disconnected".into()));
            }
        }

        if let Some((activation, reason)) = lost_endpoint {
            handoff.active_activation.store(0, Ordering::Release);
            handoff.normal.discard_all();
            handoff.priority.discard_all();
            handoff
                .panic_cleanup_latched
                .store(false, Ordering::Release);
            if let Some(old) = active.take() {
                backend.close(old.endpoint);
            }
            outbox.send(
                &events,
                OutputEvent::Connection {
                    activation,
                    result: Err(reason),
                },
            );
        }

        outbox.flush(&events);

        // 4. Park only when no pending producer work, command, or setup change exists.
        if handoff.has_pending_work(handled_stop_epoch)
            || setup_changed.load(Ordering::Acquire)
            || stopping.load(Ordering::Acquire)
        {
            continue;
        }
        wake.parked.store(true, Ordering::Release);
        if handoff.has_pending_work(handled_stop_epoch)
            || setup_changed.load(Ordering::Acquire)
            || stopping.load(Ordering::Acquire)
        {
            wake.parked.store(false, Ordering::Release);
            continue;
        }
        std::thread::park_timeout(outbox.wait());
        wake.parked.store(false, Ordering::Release);
    }

    // Control-side shutdown: close producer, perform deterministic terminal note
    // cleanup on the active endpoint, and close endpoint before returning.
    handoff.closed.store(true, Ordering::Release);
    handoff.active_activation.store(0, Ordering::Release);
    handoff.normal.discard_all();
    handoff.priority.discard_all();
    if let Some(mut old) = active.take() {
        if backend.is_present(&old.endpoint) {
            emit_cleanup(&mut backend, &mut old);
        }
        backend.close(old.endpoint);
    }
}

/// Owner of the control-side hardware MIDI output thread.
///
/// Dropping [`OutputWorker`] signals shutdown, wakes the worker promptly,
/// releases all sounding notes and held sustain pedals on any active endpoint,
/// closes the endpoint, and joins the worker thread.
pub struct OutputWorker {
    handle: OutputHandle,
    producer: Option<OutputProducer>,
    stopping: Arc<AtomicBool>,
    wake: Arc<OutputWake>,
    thread: Option<JoinHandle<()>>,
}

impl OutputWorker {
    /// Starts an idle hardware MIDI output worker using the host platform
    /// backend (CoreMIDI destinations on macOS; explicit unsupported refusal on
    /// other targets).
    pub fn start() -> Result<Self, String> {
        Self::start_with_backend(output_native::PlatformOutputBackend::new())
    }

    /// Starts an idle MIDI output worker driven by an explicit [`OutputBackend`]
    /// (used for deterministic synthetic endpoint fixtures and integration tests).
    pub fn start_with_backend<B: OutputBackend>(backend: B) -> Result<Self, String> {
        let (commands, requests) = mpsc::sync_channel(OUTPUT_COMMAND_CAPACITY);
        let (events, results) = mpsc::sync_channel(OUTPUT_EVENT_CAPACITY);
        let stopping = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(OutputWake::new());
        let handoff = Arc::new(SharedOutputHandoff::new(Arc::clone(&wake)));

        let thread_handoff = Arc::clone(&handoff);
        let thread_stop = Arc::clone(&stopping);
        let thread_wake = Arc::clone(&wake);
        let thread = std::thread::Builder::new()
            .name("hex-midi-out".into())
            .spawn(move || {
                run_output_worker(
                    backend,
                    requests,
                    events,
                    thread_handoff,
                    thread_stop,
                    thread_wake,
                )
            })
            .map_err(|error| error.to_string())?;

        Ok(Self {
            handle: OutputHandle {
                commands,
                events: Arc::new(std::sync::Mutex::new(results)),
                wake: Arc::clone(&wake),
            },
            producer: Some(OutputProducer { shared: handoff }),
            stopping,
            wake,
            thread: Some(thread),
        })
    }

    /// Cloneable control-side command/event handle.
    pub fn handle(&self) -> OutputHandle {
        self.handle.clone()
    }

    /// Takes sole ownership of the single-producer wait-free handoff handle.
    ///
    /// Returns `None` if the single producer has already been taken.
    pub fn take_producer(&mut self) -> Option<OutputProducer> {
        self.producer.take()
    }
}

impl Drop for OutputWorker {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.handle.commands.try_send(OutputRequest::Shutdown);
        self.wake.force_unpark();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::{Condvar, Mutex};

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct RecordedPacket {
        port_id: u32,
        timestamp_ns: u64,
        bytes: Vec<u8>,
    }

    #[derive(Default)]
    struct SyntheticState {
        ports: Vec<OutputPort>,
        connected_ids: BTreeMap<u32, usize>,
        closed_ports: Vec<u32>,
        packets: Vec<RecordedPacket>,
        paused: bool,
        fail_connect_ids: Vec<u32>,
        notifier: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    #[derive(Clone)]
    struct SyntheticFixture {
        inner: Arc<(Mutex<SyntheticState>, Condvar)>,
    }

    impl SyntheticFixture {
        fn new(ports: Vec<OutputPort>) -> Self {
            Self {
                inner: Arc::new((
                    Mutex::new(SyntheticState {
                        ports,
                        ..SyntheticState::default()
                    }),
                    Condvar::new(),
                )),
            }
        }

        fn backend(&self) -> SyntheticBackend {
            SyntheticBackend {
                inner: Arc::clone(&self.inner),
            }
        }

        fn set_ports(&self, ports: Vec<OutputPort>) {
            let notifier = {
                let mut state = self.inner.0.lock().unwrap();
                state.ports = ports;
                state.notifier.clone()
            };
            if let Some(notify) = notifier {
                notify();
            }
        }

        fn set_paused(&self, paused: bool) {
            let mut state = self.inner.0.lock().unwrap();
            state.paused = paused;
            if !paused {
                self.inner.1.notify_all();
            }
        }

        fn fail_connect(&self, port_id: u32) {
            self.inner.0.lock().unwrap().fail_connect_ids.push(port_id);
        }

        fn packets(&self) -> Vec<RecordedPacket> {
            self.inner.0.lock().unwrap().packets.clone()
        }

        fn closed_ports(&self) -> Vec<u32> {
            self.inner.0.lock().unwrap().closed_ports.clone()
        }

        fn wait_for_packets(
            &self,
            limit: Duration,
            predicate: impl Fn(&[RecordedPacket]) -> bool,
        ) -> Vec<RecordedPacket> {
            let deadline = std::time::Instant::now() + limit;
            let mut state = self.inner.0.lock().unwrap();
            while !predicate(&state.packets) {
                let Some(rem) = deadline.checked_duration_since(std::time::Instant::now()) else {
                    break;
                };
                let (next, _) = self.inner.1.wait_timeout(state, rem).unwrap();
                state = next;
            }
            state.packets.clone()
        }
    }

    struct SyntheticBackend {
        inner: Arc<(Mutex<SyntheticState>, Condvar)>,
    }

    impl OutputBackend for SyntheticBackend {
        type Endpoint = u32;

        fn install_setup_notifier(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
            self.inner.0.lock().unwrap().notifier = Some(notify);
        }

        fn discover(&mut self) -> Result<Vec<OutputPort>, String> {
            Ok(self.inner.0.lock().unwrap().ports.clone())
        }

        fn connect(&mut self, port: &OutputPort) -> Result<Self::Endpoint, String> {
            let mut state = self.inner.0.lock().unwrap();
            if state.fail_connect_ids.contains(&port.id) {
                return Err("synthetic connect failure".into());
            }
            if !state.ports.iter().any(|p| p.id == port.id) {
                return Err("midi output port is unavailable".into());
            }
            *state.connected_ids.entry(port.id).or_default() += 1;
            Ok(port.id)
        }

        fn is_present(&mut self, endpoint: &Self::Endpoint) -> bool {
            self.inner
                .0
                .lock()
                .unwrap()
                .ports
                .iter()
                .any(|p| p.id == *endpoint)
        }

        fn send(
            &mut self,
            endpoint: &mut Self::Endpoint,
            timestamp_ns: u64,
            message: EncodedMidiMessage,
        ) -> Result<(), OutputSendError> {
            let mut state = self.inner.0.lock().unwrap();
            while state.paused {
                let (next, _) = self
                    .inner
                    .1
                    .wait_timeout(state, Duration::from_millis(5))
                    .unwrap();
                state = next;
            }
            if !state.ports.iter().any(|p| p.id == *endpoint) {
                return Err(OutputSendError::Disconnected(
                    "midi output disconnected".into(),
                ));
            }
            state.packets.push(RecordedPacket {
                port_id: *endpoint,
                timestamp_ns,
                bytes: message.as_slice().to_vec(),
            });
            self.inner.1.notify_all();
            Ok(())
        }

        fn close(&mut self, endpoint: Self::Endpoint) {
            let mut state = self.inner.0.lock().unwrap();
            if let Some(count) = state.connected_ids.get_mut(&endpoint) {
                *count = count.saturating_sub(1);
            }
            state.closed_ports.push(endpoint);
            self.inner.1.notify_all();
        }
    }

    fn next_event(handle: &OutputHandle) -> OutputEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match handle.try_event() {
                Ok(event) => return event,
                Err(TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("expected output worker event: {error:?}"),
            }
        }
    }

    fn port(id: u32, name: &str) -> OutputPort {
        OutputPort {
            id,
            name: name.into(),
        }
    }

    #[test]
    fn stable_id_selection_ignores_port_reordering_and_rejects_invalid_messages() {
        let fixture = SyntheticFixture::new(vec![port(101, "synth a"), port(202, "synth b")]);
        let mut worker = OutputWorker::start_with_backend(fixture.backend()).unwrap();
        let handle = worker.handle();
        let producer = worker.take_producer().unwrap();
        assert!(worker.take_producer().is_none());

        handle
            .send(OutputCommand::Discover { request_id: 1 })
            .unwrap();
        assert_eq!(
            next_event(&handle),
            OutputEvent::Ports {
                request_id: 1,
                result: Ok(vec![port(101, "synth a"), port(202, "synth b")]),
            }
        );

        // Reorder endpoints so port 202 moves to index 0; selection must still bind to 202.
        fixture.set_ports(vec![port(202, "synth b"), port(101, "synth a")]);
        handle
            .send(OutputCommand::Connect {
                activation: 1,
                port: port(202, "synth b"),
            })
            .unwrap();
        assert_eq!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 1,
                result: Ok(OutputConnectionState::Connected(port(202, "synth b"))),
            }
        );
        assert_eq!(producer.active_activation(), 1);

        // Invalid messages are rejected wait-free without reaching the wire.
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 10,
                    message: OutputMessage::NoteOn {
                        channel: 16,
                        pitch: 60,
                        velocity: 100,
                    },
                },
            ),
            OutputEnqueueResult::InvalidMessage(OutputError::InvalidChannel)
        );
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 10,
                    message: OutputMessage::NoteOn {
                        channel: 0,
                        pitch: 60,
                        velocity: 0,
                    },
                },
            ),
            OutputEnqueueResult::InvalidMessage(OutputError::InvalidVelocity)
        );
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 10,
                    message: OutputMessage::PitchBend {
                        channel: 0,
                        value: 16_384,
                    },
                },
            ),
            OutputEnqueueResult::InvalidMessage(OutputError::InvalidPitchBend)
        );

        // Valid ordered sequence arrives on port 202 with exact timestamps.
        let events = [
            TimestampedOutputMessage {
                timestamp_ns: 1_000,
                message: OutputMessage::NoteOn {
                    channel: 2,
                    pitch: 60,
                    velocity: 105,
                },
            },
            TimestampedOutputMessage {
                timestamp_ns: 1_500,
                message: OutputMessage::PolyPressure {
                    channel: 2,
                    pitch: 60,
                    value: 80,
                },
            },
            TimestampedOutputMessage {
                timestamp_ns: 2_000,
                message: OutputMessage::PitchBend {
                    channel: 2,
                    value: 12_000,
                },
            },
            TimestampedOutputMessage {
                timestamp_ns: 2_500,
                message: OutputMessage::NoteOff {
                    channel: 2,
                    pitch: 60,
                    velocity: 33,
                },
            },
        ];
        for ev in events {
            assert_eq!(producer.try_send(1, ev), OutputEnqueueResult::Queued);
        }

        let packets = fixture.wait_for_packets(Duration::from_secs(2), |p| p.len() >= 4);
        assert_eq!(
            packets,
            vec![
                RecordedPacket {
                    port_id: 202,
                    timestamp_ns: 1_000,
                    bytes: vec![0x92, 60, 105],
                },
                RecordedPacket {
                    port_id: 202,
                    timestamp_ns: 1_500,
                    bytes: vec![0xa2, 60, 80],
                },
                RecordedPacket {
                    port_id: 202,
                    timestamp_ns: 2_000,
                    bytes: vec![0xe2, (12_000 & 0x7f) as u8, ((12_000 >> 7) & 0x7f) as u8],
                },
                RecordedPacket {
                    port_id: 202,
                    timestamp_ns: 2_500,
                    bytes: vec![0x82, 60, 33],
                },
            ]
        );
    }

    #[test]
    fn queue_saturation_prioritizes_note_offs_and_stop_cleans_sounding_notes() {
        let fixture = SyntheticFixture::new(vec![port(10, "hw")]);
        let mut worker = OutputWorker::start_with_backend(fixture.backend()).unwrap();
        let handle = worker.handle();
        let producer = worker.take_producer().unwrap();

        handle
            .send(OutputCommand::Connect {
                activation: 1,
                port: port(10, "hw"),
            })
            .unwrap();
        assert!(matches!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 1,
                result: Ok(OutputConnectionState::Connected(_)),
            }
        ));

        // Play one sounding note and wait until the endpoint has received it.
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 100,
                    message: OutputMessage::NoteOn {
                        channel: 1,
                        pitch: 64,
                        velocity: 100,
                    },
                },
            ),
            OutputEnqueueResult::Queued
        );
        fixture.wait_for_packets(Duration::from_secs(2), |p| p.len() == 1);

        // Pause the backend on the next message, then fill the entire normal queue.
        fixture.set_paused(true);
        // Send one sentinel CC so the worker blocks inside backend.send.
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 200,
                    message: OutputMessage::ControlChange {
                        channel: 1,
                        cc: 1,
                        value: 1,
                    },
                },
            ),
            OutputEnqueueResult::Queued
        );
        // Give the worker a brief moment to pick up the sentinel and pause inside send.
        std::thread::sleep(Duration::from_millis(10));

        // Saturate the normal ring completely with CCs.
        let mut queued_normal = 0;
        let mut saw_overflow = false;
        for i in 0..(OUTPUT_NORMAL_CAPACITY + 4) {
            let res = producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 300 + i as u64,
                    message: OutputMessage::ControlChange {
                        channel: 1,
                        cc: 74,
                        value: (i % 128) as u8,
                    },
                },
            );
            match res {
                OutputEnqueueResult::Queued => queued_normal += 1,
                OutputEnqueueResult::Overflowed => {
                    saw_overflow = true;
                    break;
                }
                other => panic!("unexpected enqueue result while saturating: {other:?}"),
            }
        }
        assert!(queued_normal >= OUTPUT_NORMAL_CAPACITY - 1);
        assert!(saw_overflow);
        assert!(producer.overflow_count() >= 1);

        // Even while the normal queue is 100% saturated, NoteOff spills into the priority lane.
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 9_000,
                    message: OutputMessage::NoteOff {
                        channel: 1,
                        pitch: 64,
                        velocity: 0,
                    },
                },
            ),
            OutputEnqueueResult::QueuedPriority
        );

        // Resume backend and verify the priority NoteOff arrives without being lost.
        fixture.set_paused(false);
        let packets = fixture.wait_for_packets(Duration::from_secs(2), |p| {
            p.iter().any(|pkt| pkt.bytes == [0x81, 64, 0])
        });
        assert!(packets.iter().any(|pkt| pkt.bytes == [0x81, 64, 0]));

        // Now start two notes + sustain, pause, fill queue with new NoteOns, and issue Stop.
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 10_000,
                    message: OutputMessage::NoteOn {
                        channel: 3,
                        pitch: 48,
                        velocity: 90,
                    },
                },
            ),
            OutputEnqueueResult::Queued
        );
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 10_001,
                    message: OutputMessage::ControlChange {
                        channel: 3,
                        cc: 64,
                        value: 127,
                    },
                },
            ),
            OutputEnqueueResult::Queued
        );
        fixture.wait_for_packets(Duration::from_secs(2), |p| {
            p.iter().any(|pkt| pkt.bytes == [0xb3, 64, 127])
        });

        fixture.set_paused(true);
        // Trigger worker into paused send with a harmless CC, then pack queued NoteOn(72).
        let _ = producer.try_send(
            1,
            TimestampedOutputMessage {
                timestamp_ns: 10_100,
                message: OutputMessage::ControlChange {
                    channel: 3,
                    cc: 1,
                    value: 5,
                },
            },
        );
        std::thread::sleep(Duration::from_millis(10));
        for _ in 0..(OUTPUT_NORMAL_CAPACITY + 2) {
            let _ = producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 10_200,
                    message: OutputMessage::NoteOn {
                        channel: 3,
                        pitch: 72,
                        velocity: 110,
                    },
                },
            );
        }
        // Stop succeeds even under 100% queue saturation.
        assert_eq!(
            producer.request_stop(1),
            OutputEnqueueResult::QueuedPriority
        );
        fixture.set_paused(false);

        let after_stop = fixture.wait_for_packets(Duration::from_secs(2), |p| {
            p.iter().any(|pkt| pkt.bytes == [0xb3, 120, 0])
        });
        // Queued NoteOn(72) must have been discarded by Stop.
        assert!(
            !after_stop.iter().any(|pkt| pkt.bytes == [0x93, 72, 110]),
            "Stop must discard queued NoteOns"
        );
        // Terminal cleanup must release sounding pitch 48, sustain CC64, AllNotesOff, AllSoundOff.
        let tail: Vec<Vec<u8>> = after_stop
            .iter()
            .rev()
            .take(4)
            .map(|p| p.bytes.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        assert_eq!(
            tail,
            vec![
                vec![0x83, 48, 0],
                vec![0xb3, 64, 0],
                vec![0xb3, 123, 0],
                vec![0xb3, 120, 0],
            ]
        );
    }

    #[test]
    fn stale_activation_and_failed_connect_preserve_active_endpoint() {
        let fixture = SyntheticFixture::new(vec![port(1, "alpha"), port(2, "broken")]);
        fixture.fail_connect(2);
        let mut worker = OutputWorker::start_with_backend(fixture.backend()).unwrap();
        let handle = worker.handle();
        let producer = worker.take_producer().unwrap();

        handle
            .send(OutputCommand::Connect {
                activation: 5,
                port: port(1, "alpha"),
            })
            .unwrap();
        assert_eq!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 5,
                result: Ok(OutputConnectionState::Connected(port(1, "alpha"))),
            }
        );

        // Stale producer activation is refused without touching the wire.
        assert_eq!(
            producer.try_send(
                4,
                TimestampedOutputMessage {
                    timestamp_ns: 1,
                    message: OutputMessage::NoteOn {
                        channel: 0,
                        pitch: 60,
                        velocity: 100,
                    },
                },
            ),
            OutputEnqueueResult::StaleActivation
        );
        assert_eq!(
            producer.request_stop(4),
            OutputEnqueueResult::StaleActivation
        );

        // Failed connect and stale connect/disconnect keep activation 5 intact.
        handle
            .send(OutputCommand::Connect {
                activation: 6,
                port: port(2, "broken"),
            })
            .unwrap();
        assert!(matches!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 6,
                result: Err(_),
            }
        ));
        handle
            .send(OutputCommand::Connect {
                activation: 5,
                port: port(1, "alpha"),
            })
            .unwrap();
        assert!(matches!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 5,
                result: Err(_),
            }
        ));
        handle
            .send(OutputCommand::Disconnect { activation: 4 })
            .unwrap();
        assert_eq!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 4,
                result: Ok(OutputConnectionState::Disconnected),
            }
        );

        // Activation 5 still owns port 1 and delivers normally.
        assert_eq!(producer.active_activation(), 5);
        assert_eq!(
            producer.try_send(
                5,
                TimestampedOutputMessage {
                    timestamp_ns: 42,
                    message: OutputMessage::NoteOn {
                        channel: 0,
                        pitch: 62,
                        velocity: 88,
                    },
                },
            ),
            OutputEnqueueResult::Queued
        );
        let packets = fixture.wait_for_packets(Duration::from_secs(2), |p| !p.is_empty());
        assert_eq!(
            packets,
            vec![RecordedPacket {
                port_id: 1,
                timestamp_ns: 42,
                bytes: vec![0x90, 62, 88],
            }]
        );
    }

    #[test]
    fn device_removal_invalidates_producer_and_allows_clean_reconnect() {
        let fixture = SyntheticFixture::new(vec![port(77, "ext")]);
        let mut worker = OutputWorker::start_with_backend(fixture.backend()).unwrap();
        let handle = worker.handle();
        let producer = worker.take_producer().unwrap();

        handle
            .send(OutputCommand::Connect {
                activation: 1,
                port: port(77, "ext"),
            })
            .unwrap();
        assert_eq!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 1,
                result: Ok(OutputConnectionState::Connected(port(77, "ext"))),
            }
        );

        // Remove endpoint asynchronously via setup notification.
        fixture.set_ports(Vec::new());
        assert!(matches!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 1,
                result: Err(_),
            }
        ));
        assert_eq!(producer.active_activation(), 0);
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 1,
                    message: OutputMessage::NoteOn {
                        channel: 0,
                        pitch: 60,
                        velocity: 100,
                    },
                },
            ),
            OutputEnqueueResult::Disconnected
        );

        // Restore endpoint and reconnect cleanly with a fresh activation.
        fixture.set_ports(vec![port(77, "ext")]);
        handle
            .send(OutputCommand::Connect {
                activation: 2,
                port: port(77, "ext"),
            })
            .unwrap();
        assert_eq!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 2,
                result: Ok(OutputConnectionState::Connected(port(77, "ext"))),
            }
        );
        assert_eq!(producer.active_activation(), 2);
        assert_eq!(
            producer.try_send(
                2,
                TimestampedOutputMessage {
                    timestamp_ns: 55,
                    message: OutputMessage::NoteOn {
                        channel: 0,
                        pitch: 67,
                        velocity: 99,
                    },
                },
            ),
            OutputEnqueueResult::Queued
        );
        let packets = fixture.wait_for_packets(Duration::from_secs(2), |p| !p.is_empty());
        assert_eq!(packets[0].bytes, vec![0x90, 67, 99]);
    }

    #[test]
    fn shutdown_emits_terminal_cleanup_for_overlapping_notes_and_returns_promptly() {
        let fixture = SyntheticFixture::new(vec![port(55, "poly")]);
        let mut worker = OutputWorker::start_with_backend(fixture.backend()).unwrap();
        let handle = worker.handle();
        let producer = worker.take_producer().unwrap();

        handle
            .send(OutputCommand::Connect {
                activation: 1,
                port: port(55, "poly"),
            })
            .unwrap();
        assert!(matches!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 1,
                result: Ok(OutputConnectionState::Connected(_)),
            }
        ));

        // Send overlapping same-pitch NoteOn twice, one NoteOff (still held), plus another pitch & sustain.
        for msg in [
            OutputMessage::NoteOn {
                channel: 0,
                pitch: 60,
                velocity: 100,
            },
            OutputMessage::NoteOn {
                channel: 0,
                pitch: 60,
                velocity: 110,
            },
            OutputMessage::NoteOff {
                channel: 0,
                pitch: 60,
                velocity: 0,
            },
            OutputMessage::NoteOn {
                channel: 4,
                pitch: 67,
                velocity: 90,
            },
            OutputMessage::ControlChange {
                channel: 4,
                cc: 64,
                value: 127,
            },
        ] {
            assert_eq!(
                producer.try_send(
                    1,
                    TimestampedOutputMessage {
                        timestamp_ns: 10,
                        message: msg,
                    },
                ),
                OutputEnqueueResult::Queued
            );
        }
        fixture.wait_for_packets(Duration::from_secs(2), |p| p.len() == 5);

        let started = std::time::Instant::now();
        drop(worker);
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_millis(100), "{elapsed:?}");

        // Producer is permanently disconnected after shutdown.
        assert_eq!(producer.active_activation(), 0);
        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 20,
                    message: OutputMessage::NoteOff {
                        channel: 0,
                        pitch: 60,
                        velocity: 0,
                    },
                },
            ),
            OutputEnqueueResult::Disconnected
        );
        assert_eq!(fixture.closed_ports(), vec![55]);

        let cleanup: Vec<Vec<u8>> = fixture
            .packets()
            .into_iter()
            .skip(5)
            .map(|p| p.bytes)
            .collect();
        assert_eq!(
            cleanup,
            vec![
                vec![0x80, 60, 0],
                vec![0xb0, 123, 0],
                vec![0xb0, 120, 0],
                vec![0x84, 67, 0],
                vec![0xb4, 64, 0],
                vec![0xb4, 123, 0],
                vec![0xb4, 120, 0],
            ]
        );
    }

    #[test]
    fn priority_queue_overflow_latches_deterministic_cleanup() {
        let fixture = SyntheticFixture::new(vec![port(9, "burst")]);
        let mut worker = OutputWorker::start_with_backend(fixture.backend()).unwrap();
        let handle = worker.handle();
        let producer = worker.take_producer().unwrap();

        handle
            .send(OutputCommand::Connect {
                activation: 1,
                port: port(9, "burst"),
            })
            .unwrap();
        assert!(matches!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 1,
                result: Ok(OutputConnectionState::Connected(_)),
            }
        ));

        assert_eq!(
            producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 1,
                    message: OutputMessage::NoteOn {
                        channel: 2,
                        pitch: 60,
                        velocity: 100,
                    },
                },
            ),
            OutputEnqueueResult::Queued
        );
        fixture.wait_for_packets(Duration::from_secs(2), |p| p.len() == 1);

        fixture.set_paused(true);
        let _ = producer.try_send(
            1,
            TimestampedOutputMessage {
                timestamp_ns: 2,
                message: OutputMessage::ControlChange {
                    channel: 2,
                    cc: 1,
                    value: 1,
                },
            },
        );
        std::thread::sleep(Duration::from_millis(10));

        // Fill normal queue completely.
        for _ in 0..=OUTPUT_NORMAL_CAPACITY {
            let _ = producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 3,
                    message: OutputMessage::ControlChange {
                        channel: 2,
                        cc: 1,
                        value: 2,
                    },
                },
            );
        }
        // Fill priority queue completely with NoteOffs, then trigger priority overflow latch.
        let mut latched = false;
        for i in 0..=(OUTPUT_PRIORITY_CAPACITY + 2) {
            let res = producer.try_send(
                1,
                TimestampedOutputMessage {
                    timestamp_ns: 10 + i as u64,
                    message: OutputMessage::NoteOff {
                        channel: 2,
                        pitch: (i % 128) as u8,
                        velocity: 0,
                    },
                },
            );
            if res == OutputEnqueueResult::PriorityOverflowCleanupLatched {
                latched = true;
                break;
            }
        }
        assert!(latched);

        fixture.set_paused(false);
        let packets = fixture.wait_for_packets(Duration::from_secs(2), |p| {
            p.iter().any(|pkt| pkt.bytes == [0xb2, 120, 0])
        });
        assert!(packets.iter().any(|pkt| pkt.bytes == [0x82, 60, 0]));
        assert!(packets.iter().any(|pkt| pkt.bytes == [0xb2, 123, 0]));
        assert!(packets.iter().any(|pkt| pkt.bytes == [0xb2, 120, 0]));
    }
}
