//! Copied transport value types, fault vocabulary, revocation, MIDI events, and PCM blocks.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Fault {
    Configuration = 1,
    Revoked,
    Protocol,
    InvalidSamples,
    InvalidEvents,
    InvalidParameters,
    MissingOutput,
    Identity,
    Saturated,
    Exhausted,
}

/// Worker progress observed when the callback found a required reply absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DeadlineWorkerPhase {
    AwaitingPickup = 1,
    Processing,
    Publishing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineMiss {
    pub reply_sequence: u64,
    pub worker_phase: DeadlineWorkerPhase,
}

/// Worker progress sampled by the callback that declared MissingOutput.
/// The last sequences describe this transport pair only. A restored pair
/// can begin at a nonzero sequence without having picked up a new request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalDeadlineMiss {
    pub miss: DeadlineMiss,
    pub last_received_sequence: Option<u64>,
    pub last_processed_sequence: Option<u64>,
}

/// Best-effort observation of an accepted concealed quantum. An overflow flag
/// means at least one other accepted gap could not occupy the single slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConcealedOutput {
    pub binding: Binding,
    pub miss: DeadlineMiss,
    pub overflowed: bool,
}

/// The callback's last accepted state within its copied transport quantum.
/// This observation grants no graph, worker, quiescence or reclamation authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommittedQuantumState {
    Closed,
    OpenUnprogrammed,
    OpenProgram(NonZeroU64),
}

/// A bounded snapshot from the exact callback pair. Untouched means no
/// callback stage was accepted at observation time; a later callback can change
/// it immediately. This grants no worker, graph or publication authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuantumObservation {
    Untouched(Binding),
    Committed(CommittedQuantum),
}

/// A coherent diagnostic snapshot from the original constructor-paired callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommittedQuantum {
    pub binding: Binding,
    pub state: CommittedQuantumState,
}

const WAKE_AWAKE: u32 = 0;
const WAKE_NOTIFIED: u32 = 1;
const WAKE_PARKED: u32 = 2;

#[cfg(unix)]
struct WorkerWakeup {
    inner: hex_shm_ring::RtWakeup,
}

#[cfg(unix)]
impl WorkerWakeup {
    fn new() -> Result<Self, Fault> {
        hex_shm_ring::RtWakeup::new()
            .map(|inner| Self { inner })
            .map_err(|_| Fault::Configuration)
    }

    #[inline]
    fn signal(&self) {
        self.inner.signal();
    }

    #[inline]
    fn wait_timeout(&self, timeout: std::time::Duration) {
        let _ = self.inner.wait_timeout(timeout);
    }
}

#[cfg(not(unix))]
struct WorkerWakeup {
    state: std::sync::Mutex<u64>,
    cvar: std::sync::Condvar,
}

#[cfg(not(unix))]
impl WorkerWakeup {
    fn new() -> Result<Self, Fault> {
        Ok(Self {
            state: std::sync::Mutex::new(0),
            cvar: std::sync::Condvar::new(),
        })
    }

    #[inline]
    fn signal(&self) {
        if let Ok(mut tokens) = self.state.lock() {
            *tokens = tokens.saturating_add(1);
            self.cvar.notify_one();
        }
    }

    fn wait_timeout(&self, timeout: std::time::Duration) {
        if let Ok(mut tokens) = self.state.lock() {
            if *tokens > 0 {
                *tokens -= 1;
                return;
            }
            if let Ok((mut guard, _)) = self.cvar.wait_timeout(tokens, timeout)
                && *guard > 0
            {
                *guard -= 1;
            }
        }
    }
}

pub(super) struct Revocation {
    pub(super) binding: Binding,
    pub(super) callback_fault: AtomicU8,
    pub(super) received_prefix: AtomicU64,
    pub(super) received_programs: [AtomicU64; 2],
    pub(super) processed_prefix: AtomicU64,
    pub(super) missed_reply: AtomicU64,
    pub(super) missed_received_prefix: AtomicU64,
    pub(super) missed_processed_prefix: AtomicU64,
    pub(super) missed_phase: AtomicU8,
    pub(super) concealed_slot: AtomicU8,
    pub(super) concealed_reply: AtomicU64,
    pub(super) concealed_phase: AtomicU8,
    pub(super) concealed_overflow: AtomicBool,
    pub(super) quantum_serial: AtomicU64,
    pub(super) quantum_program: AtomicU64,
    pub(super) quantum_state: AtomicU8,
    // Scope in high bits; low bit means armed. Keep the last serial after end
    // so a stale caller cannot reopen an earlier physical capture scope.
    pub(super) editor_scope: AtomicU64,
    // The low bit reserves the admitted input, and the high bit records
    // revocation atomically against a callback's handoff decision.
    pub(super) parameter_input_generation: AtomicU64,
    // Wait-free real-time wakeup for the joining transport worker.
    // Uses a 3-state single-atomic protocol (AWAKE/NOTIFIED/PARKED) with
    // Release-Acquire ordering (no SeqCst fence, no Thread::unpark() mutex).
    worker_wakeup: WorkerWakeup,
    worker_wake_state: AtomicU32,
}
impl Revocation {
    pub(super) const REPORTED: u8 = 1 << 7;

    pub(super) fn new(binding: Binding) -> Result<Self, Fault> {
        let worker_wakeup = WorkerWakeup::new()?;
        Ok(Self {
            binding,
            callback_fault: AtomicU8::new(0),
            received_prefix: AtomicU64::new(0),
            received_programs: std::array::from_fn(|_| AtomicU64::new(0)),
            processed_prefix: AtomicU64::new(0),
            missed_reply: AtomicU64::new(0),
            missed_received_prefix: AtomicU64::new(0),
            missed_processed_prefix: AtomicU64::new(0),
            missed_phase: AtomicU8::new(0),
            concealed_slot: AtomicU8::new(0),
            concealed_reply: AtomicU64::new(0),
            concealed_phase: AtomicU8::new(0),
            concealed_overflow: AtomicBool::new(false),
            quantum_serial: AtomicU64::new(0),
            quantum_program: AtomicU64::new(0),
            quantum_state: AtomicU8::new(0),
            editor_scope: AtomicU64::new(0),
            parameter_input_generation: AtomicU64::new(0),
            worker_wakeup,
            worker_wake_state: AtomicU32::new(WAKE_AWAKE),
        })
    }

    pub(super) fn wake_worker(&self) {
        if self.worker_wake_state.swap(WAKE_NOTIFIED, AcqRel) == WAKE_PARKED {
            self.worker_wakeup.signal();
        }
    }

    pub(super) fn park_worker_timeout(
        &self,
        timeout: std::time::Duration,
        should_stay_awake: impl FnOnce() -> bool,
    ) {
        self.worker_wake_state.swap(WAKE_AWAKE, AcqRel);
        if should_stay_awake() {
            return;
        }
        if self
            .worker_wake_state
            .compare_exchange(WAKE_AWAKE, WAKE_PARKED, AcqRel, Acquire)
            .is_err()
        {
            return;
        }
        self.worker_wakeup.wait_timeout(timeout);
        self.worker_wake_state.swap(WAKE_AWAKE, AcqRel);
    }

    pub(super) fn revoke(&self) {
        self.parameter_input_generation
            .fetch_or(parameter_fence::REVOKED_BIT, AcqRel);
        self.wake_worker();
    }

    pub(super) fn is_revoked(&self) -> bool {
        self.parameter_input_generation.load(Acquire) & parameter_fence::REVOKED_BIT != 0
    }

    // The callback is the sole publisher. Exhaustion permanently makes this
    // optional observation unavailable; it never changes transport behavior.
    pub(super) fn publish_quantum(&self, quantum: CommittedQuantumState) {
        use std::sync::atomic::Ordering::SeqCst;
        let serial = self.quantum_serial.load(SeqCst);
        // Serial 1 is reserved for the first accepted-stage transition. It
        // was marked before callback acceptance, so an observer cannot mistake
        // the acceptance/publication gap for an untouched callback.
        let next = if serial == 1 {
            2
        } else {
            if serial & 1 != 0 {
                return;
            }
            let Some(next) = serial.checked_add(2) else {
                self.quantum_serial.store(u64::MAX, SeqCst);
                return;
            };
            self.quantum_serial.store(serial + 1, SeqCst);
            next
        };
        let (state, key) = match quantum {
            CommittedQuantumState::Closed => (1, 0),
            CommittedQuantumState::OpenUnprogrammed => (2, 0),
            CommittedQuantumState::OpenProgram(key) => (3, key.get()),
        };
        self.quantum_program.store(key, SeqCst);
        self.quantum_state.store(state, SeqCst);
        self.quantum_serial.store(next, SeqCst);
    }

    pub(super) fn record_callback_fault(&self, fault: Fault) {
        let _ = self
            .callback_fault
            .compare_exchange(0, fault as u8, Release, Relaxed);
        self.revoke();
    }

    pub(super) fn record_received_program(
        &self,
        sequence: u64,
        program: Option<NonZeroU64>,
    ) -> Result<(), Fault> {
        let prefix = sequence.checked_add(1).ok_or(Fault::Exhausted)?;
        self.received_programs[(sequence & 1) as usize]
            .store(program.map_or(0, NonZeroU64::get), Release);
        self.received_prefix.store(prefix, Release);
        Ok(())
    }

    pub(super) fn received_program(&self) -> Option<(u64, Option<NonZeroU64>)> {
        let prefix = self.received_prefix.load(Acquire);
        let sequence = prefix.checked_sub(1)?;
        // The next request writes the other slot. Reusing this slot requires
        // publishing an intermediate prefix first. Acquire on the slot makes
        // that prefix visible to the final prefix check, rejecting a torn read.
        let key = self.received_programs[(sequence & 1) as usize].load(Acquire);
        (prefix == self.received_prefix.load(Acquire)).then_some((sequence, NonZeroU64::new(key)))
    }

    pub(super) fn worker_progress(&self, sequence: u64) -> (DeadlineWorkerPhase, u64, u64) {
        let processed = self.processed_prefix.load(Acquire);
        let received = self.received_prefix.load(Acquire);
        let phase = if processed > sequence {
            DeadlineWorkerPhase::Publishing
        } else if received > sequence {
            DeadlineWorkerPhase::Processing
        } else {
            DeadlineWorkerPhase::AwaitingPickup
        };
        (phase, received, processed)
    }

    pub(super) fn record_deadline_miss(&self, sequence: u64) {
        let (phase, received, processed) = self.worker_progress(sequence);
        self.missed_received_prefix.store(received, Relaxed);
        self.missed_processed_prefix.store(processed, Relaxed);
        self.missed_reply.store(sequence, Relaxed);
        self.missed_phase.store(phase as u8, Release);
    }

    // Single callback producer and single control consumer. Neither side
    // retries or waits when the other owns the slot.
    pub(super) fn publish_concealment(&self, miss: DeadlineMiss) {
        if self
            .concealed_slot
            .compare_exchange(0, 1, Acquire, Relaxed)
            .is_err()
        {
            self.concealed_overflow.store(true, Relaxed);
            return;
        }
        self.concealed_reply.store(miss.reply_sequence, Relaxed);
        self.concealed_phase.store(miss.worker_phase as u8, Relaxed);
        self.concealed_slot.store(2, Release);
    }

    pub(super) fn decode_fault(code: u8) -> Option<Fault> {
        let faults = [
            Fault::Configuration,
            Fault::Revoked,
            Fault::Protocol,
            Fault::InvalidSamples,
            Fault::InvalidEvents,
            Fault::InvalidParameters,
            Fault::MissingOutput,
            Fault::Identity,
            Fault::Saturated,
            Fault::Exhausted,
        ];
        let index = (code & !Self::REPORTED).checked_sub(1)?;
        faults.get(usize::from(index)).copied()
    }
}
impl Fault {
    pub fn message(self) -> &'static str {
        match self {
            Self::Configuration => "invalid plugin transport configuration",
            Self::Revoked => "plugin exchange revoked",
            Self::Protocol => "plugin block or host delivery refused",
            Self::InvalidSamples => "plugin exchange contains nonfinite samples",
            Self::InvalidEvents => "plugin exchange contains invalid or excessive events",
            Self::InvalidParameters => "plugin exchange contains invalid or excessive parameters",
            Self::MissingOutput => "plugin output missed its fixed pipeline deadline",
            Self::Identity => "plugin output identity does not match the pending block",
            Self::Saturated => "plugin exchange queue is full",
            Self::Exhausted => "plugin exchange sample or sequence counter exhausted",
        }
    }
}

/// A bounded MIDI channel message, preserving source bytes and sample offset.
/// SysEx/system messages require another explicitly bounded protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MidiEvent {
    pub frame: u16,
    pub bytes: [u8; 3],
    pub len: u8,
    /// An optional scheduled note identity, bound to one native epoch.
    /// Identified input pitch bend spans ±4800 cents. Identified CC74
    /// carries brightness. Both target the preceding onset on this channel.
    pub note_id: Option<std::num::NonZeroU32>,
    /// True for monitored live input (VST3 `Event::kIsLive`); false for sequenced playback.
    pub is_live: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EventDirection {
    Input,
    Output,
}

impl MidiEvent {
    /// Decode the SDK identity on a validated VST3 processor output. Positive
    /// IDs retain their existing copied value. An absent ID represents SDK -1,
    /// the all-ones value represents SDK zero, and plug-in-reserved negative
    /// IDs preserve their two's-complement bits.
    pub fn vst3_output_note_id(self) -> Option<i32> {
        self.note_id.map(|id| {
            let raw = id.get();
            if raw == u32::MAX { 0 } else { raw as i32 }
        })
    }

    pub(super) fn valid(self, frames: usize) -> bool {
        self.valid_for(frames, EventDirection::Input)
    }

    fn valid_for(self, frames: usize, direction: EventDirection) -> bool {
        let status = self.bytes[0] & 0xf0;
        let carries_note_identity = matches!(status, 0x80 | 0x90 | 0xa0);
        let identified_expression = (direction == EventDirection::Input || !self.is_live)
            && (status == 0xe0 || (status == 0xb0 && self.bytes[1] == 74));
        let expected = match status {
            0x80 | 0x90 | 0xa0 | 0xb0 | 0xe0 => 3,
            0xc0 | 0xd0 => 2,
            _ => return false,
        };
        usize::from(self.frame) < frames
            && self.len == expected
            && self.bytes[1..usize::from(self.len)]
                .iter()
                .all(|b| *b < 128)
            && (self.len == 3 || self.bytes[2] == 0)
            && self.note_id.is_none_or(|id| {
                if i32::try_from(id.get()).is_ok() {
                    return carries_note_identity || identified_expression;
                }
                // Steinberg ivstevents.h reserves this range for the plug-in.
                direction == EventDirection::Output
                    && carries_note_identity
                    && (id.get() == u32::MAX || (-10_000..=-1_000).contains(&(id.get() as i32)))
            })
            && (!self.is_live
                || carries_note_identity
                || matches!(status, 0xb0 | 0xd0 | 0xe0)
                || (direction == EventDirection::Output && status == 0xc0))
    }
}

#[derive(Clone, Copy)]
pub struct Events<const CAPACITY: usize = MAX_EVENTS> {
    pub(super) values: [MidiEvent; CAPACITY],
    pub(super) len: usize,
}
pub type InputEvents = Events<MAX_IDENTIFIED_INPUT_NOTES>;

impl<const CAPACITY: usize> Default for Events<CAPACITY> {
    fn default() -> Self {
        Self {
            values: [MidiEvent::default(); CAPACITY],
            len: 0,
        }
    }
}
impl<const CAPACITY: usize> Events<CAPACITY> {
    pub fn as_slice(&self) -> &[MidiEvent] {
        &self.values[..self.len]
    }
    pub fn push(&mut self, event: MidiEvent) -> Result<(), Fault> {
        if self.len == CAPACITY
            || (self.len >= MAX_EVENTS
                && (event.note_id.is_none()
                    || (self.len == MAX_EVENTS
                        && self.as_slice().iter().any(|prior| prior.note_id.is_none()))))
            || self
                .as_slice()
                .last()
                .is_some_and(|last| last.frame > event.frame)
        {
            return Err(Fault::InvalidEvents);
        }
        self.values[self.len] = event;
        self.len += 1;
        Ok(())
    }
    fn valid(&self, frames: usize, direction: EventDirection) -> bool {
        self.as_slice()
            .iter()
            .all(|event| event.valid_for(frames, direction))
    }
}

/// Exact Arrangement origin for the first sample of an SDK processing slice.
/// Optional musical facts come from the same aligned input origin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessTiming {
    pub arrangement_frame: i64,
    pub playing: bool,
    pub arrangement_beat_raw: Option<u64>,
    pub tempo_micro_bpm: Option<std::num::NonZeroU32>,
    pub meter: Option<(u8, u8)>,
    pub cycle_beats_raw: Option<(u64, u64)>,
}

pub use hex_lattice_contract::MAX_FOREIGN_TIMING_CHANGES as MAX_TIMING_CHANGES;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimingChange {
    pub frame: u16,
    pub timing: ProcessTiming,
}

#[derive(Clone, Copy, Default)]
pub struct TimingChanges {
    pub(super) values: [TimingChange; MAX_TIMING_CHANGES],
    pub(super) len: usize,
}

impl TimingChanges {
    pub fn as_slice(&self) -> &[TimingChange] {
        &self.values[..self.len]
    }
}

/// The run state is constant throughout one graph segment. Instruments opt
/// into observing it. Full origin facts are read only at SDK slice starts.
pub struct SegmentTiming<F> {
    pub playing: Option<bool>,
    pub at: F,
}

impl ProcessTiming {
    pub(super) fn is_valid(self) -> bool {
        self.arrangement_frame >= 0
            && self.meter.is_none_or(|(numerator, denominator)| {
                (1..=99).contains(&numerator) && denominator.is_power_of_two() && denominator <= 64
            })
            && self.cycle_beats_raw.is_none_or(|(start, end)| start < end)
    }
}

/// One authored raw SDK parameter with a normalized value at every sample.
#[derive(Clone, Copy)]
pub struct DenseParameterLane<'a> {
    pub id: u32,
    pub normalized: &'a [f64],
}
impl DenseParameterLane<'_> {
    pub(crate) fn valid(self, frames: usize) -> bool {
        parameters::valid_id(self.id)
            && self.normalized.len() == frames
            && self
                .normalized
                .iter()
                .all(|value| (0.0..=1.0).contains(value))
    }
}

/// Inline wire payload; dropping it never reclaims a resource.
#[derive(Clone, Copy)]
pub struct Block {
    pub binding: Binding,
    pub sequence: u64,
    pub first_frame: u64,
    pub frames: u16,
    pub pcm: [Frame; MAX_FRAMES],
    pub events: InputEvents,
    pub parameters: Parameters,
    pub timing: Option<ProcessTiming>,
    pub timing_changes: TimingChanges,
}
impl Block {
    pub(super) fn empty(binding: Binding, quantum: u16) -> Self {
        Self {
            binding,
            sequence: 0,
            first_frame: 0,
            frames: quantum,
            pcm: [[0.0; 2]; MAX_FRAMES],
            events: Events::default(),
            parameters: Parameters::default(),
            timing: None,
            timing_changes: TimingChanges::default(),
        }
    }

    pub(super) fn latest_timing_entry(&self) -> Option<(u16, ProcessTiming)> {
        self.timing_changes
            .as_slice()
            .last()
            .map(|change| (change.frame, change.timing))
            .or_else(|| self.timing.map(|timing| (0, timing)))
    }

    pub(super) fn latest_timing(&self) -> Option<ProcessTiming> {
        self.latest_timing_entry().map(|(_, timing)| timing)
    }

    pub(super) fn add_timing_change(
        &mut self,
        frame: u16,
        timing: ProcessTiming,
    ) -> Result<(), Fault> {
        // A playing cut can resume compensated history. A stopped cut must
        // end a playing run, so redundant stopped contexts stay malformed.
        let previous_frame = self
            .timing_changes
            .as_slice()
            .last()
            .map_or(0, |change| change.frame);
        if frame <= previous_frame
            || frame >= self.frames
            || !timing.is_valid()
            || (!timing.playing && !self.latest_timing().is_some_and(|prior| prior.playing))
        {
            return Err(Fault::Protocol);
        }
        *self
            .timing_changes
            .values
            .get_mut(self.timing_changes.len)
            .ok_or(Fault::Protocol)? = TimingChange { frame, timing };
        self.timing_changes.len += 1;
        Ok(())
    }
    pub(crate) fn valid(&self, binding: Binding, quantum: u16, sequence: u64) -> Result<(), Fault> {
        self.valid_for(binding, quantum, sequence, EventDirection::Input)
    }

    pub(super) fn valid_for(
        &self,
        binding: Binding,
        quantum: u16,
        sequence: u64,
        direction: EventDirection,
    ) -> Result<(), Fault> {
        if self.binding != binding
            || self.sequence != sequence
            || self.frames != quantum
            || sequence.checked_mul(u64::from(quantum)) != Some(self.first_frame)
        {
            return Err(Fault::Identity);
        }
        if !self.pcm[..usize::from(quantum)]
            .iter()
            .flatten()
            .all(|x| x.is_finite())
        {
            return Err(Fault::InvalidSamples);
        }
        if !self.events.valid(usize::from(quantum), direction) {
            return Err(Fault::InvalidEvents);
        }
        if !self.parameters.valid(usize::from(quantum)) {
            return Err(Fault::InvalidParameters);
        }
        if self.timing.is_some_and(|timing| !timing.is_valid()) {
            return Err(Fault::Protocol);
        }
        let mut previous = self.timing.map(|timing| TimingChange { frame: 0, timing });
        for &change in self.timing_changes.as_slice() {
            if change.frame == 0
                || change.frame >= quantum
                || !change.timing.is_valid()
                || previous.is_some_and(|prior| prior.frame >= change.frame)
                || (!change.timing.playing && !previous.is_some_and(|prior| prior.timing.playing))
            {
                return Err(Fault::Protocol);
            }
            previous = Some(change);
        }
        Ok(())
    }

    pub(super) fn valid_reply(
        &self,
        binding: Binding,
        quantum: u16,
        sequence: u64,
    ) -> Result<(), Fault> {
        if self.events.len > MAX_EVENTS {
            return Err(Fault::InvalidEvents);
        }
        if !self.timing_changes.as_slice().is_empty() {
            return Err(Fault::Protocol);
        }
        self.valid_for(binding, quantum, sequence, EventDirection::Output)
    }
}
