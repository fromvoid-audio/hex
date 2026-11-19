//! Copied, bounded transport for an isolated processor worker.
//!
//! Control constructs both rings and retains their endpoints until callback
//! quiescence. Only the worker may perform IPC or native processing. This owner
//! does not load plugins, admit devices/graphs, or promise worker deadlines.
//! A binding is an equality fence, not proof of native resource admission.
//!
//! One immutable quantum Q (at most 256 stereo frames) has exactly 2Q frames of
//! pipeline latency. A physical callback must contain at most Q frames. Input is
//! staged across contiguous graph segments until the complete physical callback
//! passes final host conversion; only then may it reach a worker.
//! Malformed output or saturation revokes the entire one-shot exchange.
//! Native PCM workers may conceal up to six consecutive missing replies and
//! discard their late output. The audio host uses aligned dry input for effects
//! and silence for instruments. No stale-buffer replay, waiting, waking,
//! allocation or DSP rollback occurs; persistent misses still revoke the epoch.
//! A one-slot atomic observation reports the first accepted concealed quantum
//! and worker phase; a boolean records additional gaps while the slot is busy.
//! The callback performs one bounded publication attempt only after host-format
//! acceptance. Polling or source retirement may lose diagnostic observations.

type Frame = [f32; 2];
use crate::protocol::{EditorPerforms, EditorRestorations};
use hex_lattice_contract::ForeignProcessorBinding as Binding;
use rtrb::{Consumer, Producer, RingBuffer};
use std::num::NonZeroU64;
use std::sync::{
    Arc,
    atomic::{
        AtomicBool, AtomicU8, AtomicU64,
        Ordering::{AcqRel, Acquire, Relaxed, Release},
    },
};
mod authored;
use authored::AuthoredOrigins;
pub use authored::{AuthoredParameterProgram, AuthoredSegment};
mod dense;
pub use dense::{
    DenseEffectCallback, DenseParameterRequest, dense_effect_channel_from_completed_prefix,
};
mod parameter_fence;
pub use parameter_fence::{ParameterInputAdmission, ParameterInputFence, ParameterInputSource};
mod parameters;
mod storage;
pub use parameters::{MAX_PARAMETER_POINTS, ParameterPoint, Parameters};
#[cfg(test)]
mod editor_tests;
#[cfg(test)]
mod parameter_tests;
pub mod wire;
pub use storage::{ConstructorStorage, channel_storage_bound, dense_channel_storage_bound};

pub const MAX_FRAMES: usize = 256;
pub const MAX_EVENTS: usize = 64;
/// Reserve 32 hardware MIDI events beside the existing 128 identified notes
/// for authored playback and computer keys. Byte-only MIDI and replies keep
/// the smaller generic event bound.
pub const MAX_IDENTIFIED_INPUT_NOTES: usize = 160;
pub const QUEUE_CAPACITY: usize = 8;
// Two pipeline quanta and six concealed quanta fit in the bounded input ring.
const MAX_CONCEALED_OUTPUT_GAPS: u8 = 6;

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

struct Revocation {
    binding: Binding,
    callback_fault: AtomicU8,
    received_prefix: AtomicU64,
    received_programs: [AtomicU64; 2],
    processed_prefix: AtomicU64,
    missed_reply: AtomicU64,
    missed_phase: AtomicU8,
    concealed_slot: AtomicU8,
    concealed_reply: AtomicU64,
    concealed_phase: AtomicU8,
    concealed_overflow: AtomicBool,
    quantum_serial: AtomicU64,
    quantum_program: AtomicU64,
    quantum_state: AtomicU8,
    // Scope in high bits; low bit means armed. Keep the last serial after end
    // so a stale caller cannot reopen an earlier physical capture scope.
    editor_scope: AtomicU64,
    // The low bit reserves the admitted input, and the high bit records
    // revocation atomically against a callback's handoff decision.
    parameter_input_generation: AtomicU64,
}
impl Revocation {
    const REPORTED: u8 = 1 << 7;

    fn new(binding: Binding) -> Self {
        Self {
            binding,
            callback_fault: AtomicU8::new(0),
            received_prefix: AtomicU64::new(0),
            received_programs: std::array::from_fn(|_| AtomicU64::new(0)),
            processed_prefix: AtomicU64::new(0),
            missed_reply: AtomicU64::new(0),
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
        }
    }

    fn revoke(&self) {
        self.parameter_input_generation
            .fetch_or(parameter_fence::REVOKED_BIT, AcqRel);
    }

    fn is_revoked(&self) -> bool {
        self.parameter_input_generation.load(Acquire) & parameter_fence::REVOKED_BIT != 0
    }

    // The callback is the sole publisher. Exhaustion permanently makes this
    // optional observation unavailable; it never changes transport behavior.
    fn publish_quantum(&self, quantum: CommittedQuantumState) {
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

    fn record_callback_fault(&self, fault: Fault) {
        let _ = self
            .callback_fault
            .compare_exchange(0, fault as u8, Release, Relaxed);
        self.revoke();
    }

    fn record_received_program(
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

    fn received_program(&self) -> Option<(u64, Option<NonZeroU64>)> {
        let prefix = self.received_prefix.load(Acquire);
        let sequence = prefix.checked_sub(1)?;
        // The next request writes the other slot. Reusing this slot requires
        // publishing an intermediate prefix first. Acquire on the slot makes
        // that prefix visible to the final prefix check, rejecting a torn read.
        let key = self.received_programs[(sequence & 1) as usize].load(Acquire);
        (prefix == self.received_prefix.load(Acquire)).then_some((sequence, NonZeroU64::new(key)))
    }

    fn worker_phase(&self, sequence: u64) -> DeadlineWorkerPhase {
        if self.processed_prefix.load(Acquire) > sequence {
            DeadlineWorkerPhase::Publishing
        } else if self.received_prefix.load(Acquire) > sequence {
            DeadlineWorkerPhase::Processing
        } else {
            DeadlineWorkerPhase::AwaitingPickup
        }
    }

    fn record_deadline_miss(&self, sequence: u64) {
        let phase = self.worker_phase(sequence);
        self.missed_reply.store(sequence, Relaxed);
        self.missed_phase.store(phase as u8, Release);
    }

    // Single callback producer and single control consumer. Neither side
    // retries or waits when the other owns the slot.
    fn publish_concealment(&self, miss: DeadlineMiss) {
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

    fn decode_fault(code: u8) -> Option<Fault> {
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
    pub note_id: Option<std::num::NonZeroU32>,
}
impl MidiEvent {
    fn valid(self, frames: usize) -> bool {
        let expected = match self.bytes[0] & 0xf0 {
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
                matches!(self.bytes[0] & 0xf0, 0x80 | 0x90) && i32::try_from(id.get()).is_ok()
            })
    }
}

#[derive(Clone, Copy)]
pub struct Events<const CAPACITY: usize = MAX_EVENTS> {
    values: [MidiEvent; CAPACITY],
    len: usize,
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
    fn valid(&self, frames: usize) -> bool {
        self.as_slice().iter().all(|event| event.valid(frames))
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
    values: [TimingChange; MAX_TIMING_CHANGES],
    len: usize,
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
    fn is_valid(self) -> bool {
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
    fn empty(binding: Binding, quantum: u16) -> Self {
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

    fn latest_timing(&self) -> Option<ProcessTiming> {
        self.timing_changes
            .as_slice()
            .last()
            .map(|change| change.timing)
            .or(self.timing)
    }

    fn add_timing_change(&mut self, frame: u16, timing: ProcessTiming) -> Result<(), Fault> {
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
    fn valid(&self, binding: Binding, quantum: u16, sequence: u64) -> Result<(), Fault> {
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
        if !self.events.valid(usize::from(quantum)) {
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

    fn valid_reply(&self, binding: Binding, quantum: u16, sequence: u64) -> Result<(), Fault> {
        if self.events.len > MAX_EVENTS {
            return Err(Fault::InvalidEvents);
        }
        if !self.timing_changes.as_slice().is_empty() {
            return Err(Fault::Protocol);
        }
        self.valid(binding, quantum, sequence)
    }
}

/// Queue-independent revocation. Retain this control lease through worker join
/// and callback terminal return; neither endpoint may be destroyed on callback.
pub struct Control {
    revoked: Arc<Revocation>,
}

/// Actual pickup of one accepted original transport request. This is neither
/// successful SDK processing, final Main delivery nor publication authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceivedProgram {
    pub binding: Binding,
    pub sequence: u64,
    pub program: Option<NonZeroU64>,
}
/// Control-side custody of the original pair's opt-in. This retains only
/// copied transport signals, not a worker, endpoint, SDK or output lease.
/// Drop on control after the last wanted marker is drained.
pub struct EditorScope {
    signal: Arc<Revocation>,
    serial: NonZeroU64,
}

impl Drop for EditorScope {
    fn drop(&mut self) {
        let ended = self.serial.get() << 1;
        let _ = self
            .signal
            .editor_scope
            .compare_exchange(ended | 1, ended, AcqRel, Acquire);
    }
}

impl Control {
    /// Enable observation on new physical quantum starts for this exact pair.
    /// Serial numbers must increase per pair. Ending discards unconsumed
    /// markers; drain wanted observations before ending an orderly scope.
    pub fn arm_editor_pcm(&mut self, serial: NonZeroU64) -> Result<EditorScope, Fault> {
        self.arm_editor_pcm_after(serial, || {})
    }

    /// The unique control owner installs its exact worker plan before the
    /// release publication can make this serial visible to the callback.
    pub fn arm_editor_pcm_after(
        &mut self,
        serial: NonZeroU64,
        prepare: impl FnOnce(),
    ) -> Result<EditorScope, Fault> {
        if self.is_revoked() {
            return Err(Fault::Revoked);
        }
        let value = serial.get().checked_mul(2).ok_or(Fault::Exhausted)? | 1;
        let previous = self.revoked.editor_scope.load(Acquire);
        if previous & 1 != 0 || serial.get() <= previous >> 1 {
            return Err(Fault::Protocol);
        }
        prepare();
        self.revoked.editor_scope.store(value, Release);
        Ok(EditorScope {
            signal: Arc::clone(&self.revoked),
            serial,
        })
    }

    /// Distinguish an untouched paired callback from a committed quantum.
    /// None includes a concurrent first acceptance, contention, revocation,
    /// and exhausted observation serial. Neither result stays valid by itself.
    pub fn quantum_observation(&self) -> Option<QuantumObservation> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.revoked.is_revoked() {
            return None;
        }
        let serial = self.revoked.quantum_serial.load(SeqCst);
        if serial == 0 {
            return (self.revoked.quantum_serial.load(SeqCst) == 0 && !self.revoked.is_revoked())
                .then_some(QuantumObservation::Untouched(self.revoked.binding));
        }
        self.committed_quantum().map(QuantumObservation::Committed)
    }

    /// Read one bounded snapshot of the exact paired callback's most recently
    /// accepted quantum. None means no observation, contention, exhaustion, or
    /// revocation. A copied binding is not proof of any graph installation.
    pub fn committed_quantum(&self) -> Option<CommittedQuantum> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.revoked.is_revoked() {
            return None;
        }
        let before = self.revoked.quantum_serial.load(SeqCst);
        if before == 0 || before & 1 != 0 {
            return None;
        }
        let key = self.revoked.quantum_program.load(SeqCst);
        let state = match self.revoked.quantum_state.load(SeqCst) {
            1 => CommittedQuantumState::Closed,
            2 => CommittedQuantumState::OpenUnprogrammed,
            3 => CommittedQuantumState::OpenProgram(NonZeroU64::new(key)?),
            _ => return None,
        };
        (before == self.revoked.quantum_serial.load(SeqCst) && !self.revoked.is_revoked())
            .then_some(CommittedQuantum {
                binding: self.revoked.binding,
                state,
            })
    }
    pub fn revoke(&self) {
        self.revoked.revoke();
    }
    pub fn is_revoked(&self) -> bool {
        self.revoked.is_revoked()
    }
    /// One bounded coherent observation from this constructor-paired worker.
    /// A concurrent publication or revocation returns no observation; retry on
    /// a later control poll. No input/current-graph key is substituted.
    pub fn received_program(&self) -> Option<ReceivedProgram> {
        if self.is_revoked() {
            return None;
        }
        let (sequence, program) = self.revoked.received_program()?;
        (!self.is_revoked()).then_some(ReceivedProgram {
            binding: self.revoked.binding,
            sequence,
            program,
        })
    }

    /// This live pair has published no worker pickup yet. A failed coherent
    /// read of a nonzero prefix is a different observation. The callback may
    /// still be assembling or sending a quantum.
    pub fn has_no_pcm_pickup(&self) -> bool {
        self.revoked.received_prefix.load(Acquire) == 0 && !self.is_revoked()
    }

    /// Retained observation for the terminal MissingOutput of this exact pair.
    pub fn deadline_miss(&self) -> Option<DeadlineMiss> {
        let worker_phase = match self.revoked.missed_phase.load(Acquire) {
            1 => DeadlineWorkerPhase::AwaitingPickup,
            2 => DeadlineWorkerPhase::Processing,
            3 => DeadlineWorkerPhase::Publishing,
            _ => return None,
        };
        Some(DeadlineMiss {
            reply_sequence: self.revoked.missed_reply.load(Relaxed),
            worker_phase,
        })
    }
    /// Drain at most one accepted concealment from this exact pair. Contention
    /// defers the read; an absent value is not evidence that no gap occurred.
    pub fn take_concealed_output(&self) -> Option<ConcealedOutput> {
        self.revoked
            .concealed_slot
            .compare_exchange(2, 1, Acquire, Relaxed)
            .ok()?;
        let sequence = self.revoked.concealed_reply.load(Relaxed);
        let phase = self.revoked.concealed_phase.load(Relaxed);
        let overflowed = self.revoked.concealed_overflow.swap(false, Relaxed);
        self.revoked.concealed_slot.store(0, Release);
        let worker_phase = match phase {
            1 => DeadlineWorkerPhase::AwaitingPickup,
            2 => DeadlineWorkerPhase::Processing,
            3 => DeadlineWorkerPhase::Publishing,
            _ => return None,
        };
        Some(ConcealedOutput {
            binding: self.revoked.binding,
            miss: DeadlineMiss {
                reply_sequence: sequence,
                worker_phase,
            },
            overflowed,
        })
    }
    /// The exact callback's first actionable fault, once. A control/worker
    /// revocation alone and a later callback observing it have no new cause.
    pub fn take_callback_fault(&self) -> Option<Fault> {
        let code = self.revoked.callback_fault.load(Acquire);
        if code == 0 || code & Revocation::REPORTED != 0 {
            return None;
        }
        let previous = self
            .revoked
            .callback_fault
            .fetch_or(Revocation::REPORTED, AcqRel);
        if previous & Revocation::REPORTED != 0 {
            return None;
        }
        Revocation::decode_fault(previous).filter(|&fault| fault != Fault::Revoked)
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.revoke();
    }
}

/// Input content at the exact physical quantum start. This stays inside
/// the copied transport and has no meaning when the stage clock is absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManualParameterOrigin {
    pub arrangement_frame: u64,
    pub performance_frame: u64,
    pub loop_pass: u64,
}

/// Borrow the compensated graph origins for one segment. Authored playback
/// needs the per-sample view. Manual correlation only reads the quantum start.
#[derive(Default)]
pub struct EffectSegmentOrigins<'a> {
    pub authored: Option<AuthoredSegment<'a>>,
    pub manual: Option<&'a dyn Fn(usize) -> Option<ManualParameterOrigin>>,
}

#[derive(Clone, Copy)]
struct InputRequest {
    block: Block,
    // Minimum spare capacity of every physical callback contributing to this
    // quantum. Control-originated input may consume it only after acceptance.
    parameter_capacity: usize,
    authored_program: Option<NonZeroU64>,
    manual_origin: Option<ManualParameterOrigin>,
    editor_scope: Option<NonZeroU64>,
}

pub struct WorkerRequest {
    pub block: Block,
    pub parameter_capacity: usize,
    pub dense_parameter: Option<DenseParameterRequest>,
    /// Opening program, also the conservative frontier for retained history
    /// when a later clock region changed programs inside the quantum.
    pub authored_program: Option<NonZeroU64>,
    /// The exact accepted quantum starts on a known compensated Arrangement
    /// origin. The copied clock stays with the transport's in-flight request.
    /// This observation grants no publication or parameter input authority.
    pub manual_origin_available: bool,
    /// The control scope active at the original accepted physical quantum's
    /// first sample. No request already staged at arm time is retroactively tagged.
    pub editor_scope: Option<NonZeroU64>,
}

/// The one exact host operation applied at offset zero in a successful SDK
/// exchange. This metadata is attached by the joining worker after native
/// validation and never enters the native request/reply wire.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ManualParameterInput {
    pub operation: NonZeroU64,
    pub action: ManualParameterAction,
}

/// Host input applied at the successful SDK quantum's first frame. A release
/// restores the authored target and has no invented normalized point value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ManualParameterAction {
    Point(ParameterPoint),
    Release { parameter: u32 },
}
impl ManualParameterInput {
    pub fn parameter(self) -> u32 {
        match self.action {
            ManualParameterAction::Point(point) => point.id,
            ManualParameterAction::Release { parameter } => parameter,
        }
    }
    pub fn point(self) -> Option<ParameterPoint> {
        match self.action {
            ManualParameterAction::Point(point) => Some(point),
            ManualParameterAction::Release { .. } => None,
        }
    }
    fn valid(self, quantum: u16) -> bool {
        match self.action {
            ManualParameterAction::Point(point) => {
                point.frame == 0 && parameters::valid(point, usize::from(quantum))
            }
            ManualParameterAction::Release { parameter } => parameters::valid_id(parameter),
        }
    }
}

/// Manual input carried by a sample in the accepted physical endpoint
/// callback. The mix owner still decides final output/transport acceptance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AcceptedManualParameter {
    pub binding: Binding,
    pub sequence: u64,
    pub first_frame: u64,
    pub callback_offset: u16,
    pub input: ManualParameterInput,
    pub origin: Option<ManualParameterOrigin>,
}

#[derive(Clone, Copy)]
struct ManualExchange {
    input: ManualParameterInput,
    origin: Option<ManualParameterOrigin>,
}

/// Successful editor input carried with this exact endpoint's accepted sample.
/// Only the Main owner can determine its final delivery and transport validity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AcceptedEditorQuantum {
    pub binding: Binding,
    pub sequence: u64,
    pub first_frame: u64,
    pub callback_offset: u16,
    pub scope: NonZeroU64,
    pub performs: EditorPerforms,
    pub restorations: EditorRestorations,
    pub lost_origin: bool,
    pub origin: Option<ManualParameterOrigin>,
}

#[derive(Clone, Copy, Default)]
pub struct EditorPcmFacts {
    pub performs: EditorPerforms,
    pub restorations: EditorRestorations,
    pub lost_origin: bool,
}

#[derive(Clone, Copy)]
struct EditorExchange {
    scope: NonZeroU64,
    facts: EditorPcmFacts,
    origin: Option<ManualParameterOrigin>,
}

#[derive(Clone, Copy)]
struct OutputReply {
    block: Block,
    manual: Option<ManualExchange>,
    editor: Option<EditorExchange>,
}

impl OutputReply {
    fn valid(&self, binding: Binding, quantum: u16, sequence: u64) -> Result<(), Fault> {
        self.block.valid_reply(binding, quantum, sequence)?;
        if self
            .manual
            .is_some_and(|manual| !manual.input.valid(quantum))
        {
            return Err(Fault::InvalidParameters);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct State {
    position: u64,
    input: Block,
    input_manual_origin: Option<ManualParameterOrigin>,
    input_editor_scope: Option<NonZeroU64>,
    output: OutputReply,
    output_gap: bool,
    consecutive_output_gaps: u8,
    next_output_reply: u64,
    parameter_capacity: usize,
}
#[derive(Clone, Copy)]
struct Stage {
    state: State,
    request: Option<InputRequest>,
    manual: Option<AcceptedManualParameter>,
    editor: Option<AcceptedEditorQuantum>,
    concealed: Option<DeadlineMiss>,
    expected_frames: usize,
    staged_frames: usize,
    input_events: usize,
    legacy_input_events: bool,
    output_events: usize,
    input_parameters: usize,
    output_parameters: usize,
}

/// Copied responses for one staged graph segment. Like PCM, this is not a
/// final host-delivery acknowledgement. Offsets are relative to the segment.
#[derive(Clone, Copy)]
pub struct SegmentReply {
    pub events: Events,
    pub parameters: Parameters,
    /// Half-open segment-relative span with a concealed PCM reply, if any.
    /// It carries no accepted parameter or editor metadata.
    pub concealed_output: Option<(usize, usize)>,
}

/// Callback endpoint. Install before native callback reservation; return the
/// complete owner through the existing terminal guards before dropping it.
pub struct Callback {
    binding: Binding,
    quantum: u16,
    first_reply_sequence: u64,
    conceal_output_gap: bool,
    requests: Producer<InputRequest>,
    replies: Consumer<OutputReply>,
    revoked: Arc<Revocation>,
    state: State,
    staged: Option<Stage>,
    accepted_manual: Option<AcceptedManualParameter>,
    accepted_editor: Option<AcceptedEditorQuantum>,
    committed_quantum: Option<CommittedQuantumState>,
    // Only the prefix below the staged position is read. Rejection permanently
    // revokes the endpoint, so origin scratch does not need a rollback copy.
    authored_origins: AuthoredOrigins,
}

/// Serialized worker endpoint; process at most one received block at a time.
/// IPC, waiting and worker shutdown belong to its joining control owner.
pub struct Worker {
    binding: Binding,
    quantum: u16,
    requests: Consumer<InputRequest>,
    replies: Producer<OutputReply>,
    revoked: Arc<Revocation>,
    sequence: u64,
    in_flight: Option<InFlight>,
    dense_requests: Option<Consumer<DenseParameterRequest>>,
}

struct InFlight {
    manual_origin: Option<ManualParameterOrigin>,
    editor_scope: Option<NonZeroU64>,
}

/// Allocate only on control. The two rings have eight fixed inline slots each.
pub fn channel(binding: Binding, quantum: u16) -> Result<(Control, Callback, Worker), Fault> {
    channel_from_completed_prefix(binding, quantum, 0)
}

/// Fresh queues after an actual native quiescence/resume acknowledgement. The
/// caller retains the unique native source and uses its exact completed block
/// prefix; these copied scalars grant no native/root/callback custody. Old queues
/// must be returned/disposed on control, never retagged or replayed. The fresh
/// pipeline still primes exactly 2Q frames, independent of its absolute origin.
pub fn channel_from_completed_prefix(
    binding: Binding,
    quantum: u16,
    completed_blocks: u64,
) -> Result<(Control, Callback, Worker), Fault> {
    channel_with_policy(binding, quantum, completed_blocks, false)
}

/// The native instance wire carries PCM and parameter observations, with no
/// output MIDI. Up to six consecutive deadline misses conceal their quanta
/// while preserving ordered input and the fixed 2Q alignment of later replies.
/// A late reply never acknowledges audio, editor gestures or manual parameters.
pub fn native_pcm_channel_from_completed_prefix(
    binding: Binding,
    quantum: u16,
    completed_blocks: u64,
) -> Result<(Control, Callback, Worker), Fault> {
    channel_with_policy(binding, quantum, completed_blocks, true)
}

fn channel_with_policy(
    binding: Binding,
    quantum: u16,
    completed_blocks: u64,
    conceal_output_gap: bool,
) -> Result<(Control, Callback, Worker), Fault> {
    if !binding.is_valid() || quantum == 0 || usize::from(quantum) > MAX_FRAMES {
        return Err(Fault::Configuration);
    }
    let position = completed_blocks
        .checked_mul(u64::from(quantum))
        .ok_or(Fault::Exhausted)?;
    let first_reply_sequence = completed_blocks.checked_add(2).ok_or(Fault::Exhausted)?;
    let (request_tx, request_rx) = RingBuffer::new(QUEUE_CAPACITY);
    let (reply_tx, reply_rx) = RingBuffer::new(QUEUE_CAPACITY);
    let revoked = Arc::new(Revocation::new(binding));
    let empty = Block::empty(binding, quantum);
    Ok((
        Control {
            revoked: Arc::clone(&revoked),
        },
        Callback {
            binding,
            quantum,
            first_reply_sequence,
            conceal_output_gap,
            requests: request_tx,
            replies: reply_rx,
            revoked: Arc::clone(&revoked),
            state: State {
                position,
                input: empty,
                input_manual_origin: None,
                input_editor_scope: None,
                output: OutputReply {
                    block: empty,
                    manual: None,
                    editor: None,
                },
                output_gap: false,
                consecutive_output_gaps: 0,
                next_output_reply: completed_blocks,
                parameter_capacity: MAX_PARAMETER_POINTS,
            },
            staged: None,
            accepted_manual: None,
            accepted_editor: None,
            committed_quantum: None,
            authored_origins: AuthoredOrigins::default(),
        },
        Worker {
            binding,
            quantum,
            requests: request_rx,
            replies: reply_tx,
            revoked,
            sequence: completed_blocks,
            in_flight: None,
            dense_requests: None,
        },
    ))
}

impl Callback {
    /// Committed request prefix for an exclusively owned offline endpoint.
    /// No observation or borrow of the device callback is permitted.
    pub fn dispatched_blocks_on_control(&self) -> u64 {
        self.state.position / u64::from(self.quantum)
    }
    /// Permanently refuse this exchange at its actual host validation boundary.
    /// Rejection neither replays work nor destroys either endpoint.
    pub fn reject(&mut self, fault: Fault) {
        let _ = self.fail::<()>(fault);
    }
    /// Constructor pairing only; never native/root publication authority.
    pub fn matches_control(&self, control: &Control) -> bool {
        Arc::ptr_eq(&self.revoked, &control.revoked)
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }
    /// Diagnostic observation of actual committed transport progress. Staging
    /// alone does not advance it; this grants no worker/readiness authority.
    #[cfg(any(test, feature = "test-support", feature = "allocation-probe"))]
    pub fn committed_position_for_test(&self) -> u64 {
        self.state.position
    }
    /// Snapshot the original producer's pending requests. This is diagnostic
    /// progress only, not processing completion or native/publication custody.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pending_requests_for_test(&self) -> usize {
        QUEUE_CAPACITY - self.requests.slots()
    }
    pub fn quantum_frames(&self) -> usize {
        usize::from(self.quantum)
    }
    pub fn revoke(&mut self) {
        self.revoked.revoke();
        self.staged = None;
        self.accepted_manual = None;
        self.accepted_editor = None;
    }
    pub fn latency_frames(&self) -> u32 {
        u32::from(self.quantum) * 2
    }
    /// Whether the visited physical callback has complete staged coverage.
    /// This is not final host acceptance and grants no dispatch authority.
    pub fn has_complete_staged_callback(&self) -> bool {
        self.staged
            .as_ref()
            .is_some_and(|stage| stage.staged_frames == stage.expected_frames)
    }
    /// End at a proven transport hold, such as an audible Pause or a stream
    /// waiting for pages. A contiguous stage ending exactly there may omit the
    /// unvisited suffix. A fully staged monitoring path keeps its extent.
    /// Final host acceptance is still required.
    pub fn end_staging_at_rendered_prefix(&mut self, frame: usize) {
        if let Some(stage) = self.staged.as_mut()
            && frame != 0
            && stage.staged_frames == frame
            && frame < stage.expected_frames
        {
            stage.expected_frames = frame;
        }
    }
    /// Current epoch revocation, including a worker/control request that the
    /// callback has not yet visited. This read grants no future liveness.
    pub fn is_revoked(&self) -> bool {
        self.revoked.is_revoked()
    }
    pub fn fault(&self) -> Option<Fault> {
        Revocation::decode_fault(self.revoked.callback_fault.load(Relaxed))
    }
    fn fail<T>(&mut self, fault: Fault) -> Result<T, Fault> {
        self.revoked.record_callback_fault(fault);
        self.staged = None;
        self.accepted_manual = None;
        self.accepted_editor = None;
        Err(fault)
    }
    /// Declare one actual physical callback, even when the graph renders it in
    /// several ordered segments. The complete extent remains at most Q; graph
    /// chunking never grants a second dispatch or same-callback worker wait.
    pub fn begin_physical_callback(&mut self, frames: usize) -> Result<(), Fault> {
        if self.revoked.is_revoked() {
            return self.fail(Fault::Revoked);
        }
        if self.staged.is_some() || frames == 0 || frames > usize::from(self.quantum) {
            return self.fail(Fault::Protocol);
        }
        self.accepted_manual = None;
        self.accepted_editor = None;
        self.staged = Some(Stage {
            state: self.state,
            request: None,
            manual: None,
            editor: None,
            concealed: None,
            expected_frames: frames,
            staged_frames: 0,
            input_events: 0,
            legacy_input_events: false,
            output_events: 0,
            input_parameters: 0,
            output_parameters: 0,
        });
        Ok(())
    }
    /// Stage one whole physical callback. Output events use offsets within it.
    /// The first 2Q frames are explicit pipeline warmup silence, not an
    /// audible-start guarantee. Segment callers declare the full extent first.
    pub fn stage(
        &mut self,
        input: &[Frame],
        events: &[MidiEvent],
        output: &mut [Frame],
    ) -> Result<Events, Fault> {
        output.fill([0.0; 2]);
        self.begin_physical_callback(input.len())?;
        self.stage_segment(0, input, events, output)
    }
    /// Append the next contiguous graph segment. Input and output event offsets
    /// are relative to this segment. Replies may be consumed, but no request is
    /// published before the single final host-format acceptance in `finish`.
    /// Any gap, overlap, incomplete callback or refusal revokes the whole epoch;
    /// already consumed replies/foreign history are never rolled back/replayed.
    pub fn stage_segment(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        output: &mut [Frame],
    ) -> Result<Events, Fault> {
        let reply = self.stage_segment_with_parameters(frame_offset, input, events, &[], output)?;
        if !reply.parameters.as_slice().is_empty() {
            // Legacy event-only consumers cannot silently discard processor
            // feedback. Parameter-capable consumers use the complete reply.
            output.fill([0.0; 2]);
            return self.fail(Fault::InvalidParameters);
        }
        Ok(reply.events)
    }
    /// Stage input and collect processor feedback using the same physical
    /// callback/quantum acceptance as PCM and MIDI. At most 64 input and output
    /// points may cross one callback and at most 64 remain in either quantum.
    /// Malformed/overflowing input revokes before any staged request is sent.
    pub fn stage_segment_with_parameters(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
    ) -> Result<SegmentReply, Fault> {
        self.stage_segment_with_timing(
            frame_offset,
            input,
            events,
            parameters,
            output,
            SegmentTiming {
                playing: None,
                at: |_| None,
            },
        )
    }

    /// A partial quantum retains the exact first origin and any opted-in
    /// transport changes across physical callbacks. Copy a new origin only
    /// when the segment changes the most recent valid playing state.
    pub fn stage_segment_with_timing(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
    ) -> Result<SegmentReply, Fault> {
        self.stage_effect_segment(
            frame_offset,
            input,
            events,
            parameters,
            output,
            timing,
            EffectSegmentOrigins::default(),
        )
    }

    /// Accumulate compensated origins through graph and physical callback
    /// partitions. Render the authored points once for the actual complete
    /// processing quantum. Ordinary parameter input has a separate path.
    pub fn stage_segment_with_authored_parameters(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        authored: AuthoredSegment<'_>,
    ) -> Result<SegmentReply, Fault> {
        self.stage_effect_segment(
            frame_offset,
            input,
            events,
            &[],
            output,
            timing,
            EffectSegmentOrigins {
                authored: Some(authored),
                manual: None,
            },
        )
    }

    /// Stage the actual effect's compensated clock alongside the normal
    /// parameter and PCM input. The copied marker is never sent to the SDK.
    #[allow(clippy::too_many_arguments)] // Borrowed PCM, events and exact clock views.
    pub fn stage_effect_segment(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
    ) -> Result<SegmentReply, Fault> {
        let mut reply = SegmentReply {
            events: Events::default(),
            parameters: Parameters::default(),
            concealed_output: None,
        };
        reply.concealed_output = self.stage_effect_segment_with_reply(
            frame_offset,
            input,
            events,
            parameters,
            output,
            timing,
            origins,
            Some(&mut reply),
        )?;
        Ok(reply)
    }

    /// Stage native PCM for a consumer that observes reply metadata on control.
    /// Validate the same output bounds without copying unused events and points.
    #[allow(clippy::too_many_arguments)] // Same borrowed windows as ordinary effect staging.
    pub fn stage_effect_pcm_segment(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
    ) -> Result<Option<(usize, usize)>, Fault> {
        self.stage_effect_segment_with_reply(
            frame_offset,
            input,
            events,
            parameters,
            output,
            timing,
            origins,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // Borrowed windows share one staging transaction.
    fn stage_effect_segment_with_reply(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
        reply: Option<&mut SegmentReply>,
    ) -> Result<Option<(usize, usize)>, Fault> {
        output.fill([0.0; 2]);
        let result = self.stage_segment_inner(
            frame_offset,
            input,
            events,
            parameters,
            output,
            timing,
            origins,
            reply,
        );
        if let Err(fault) = result {
            output.fill([0.0; 2]);
            return self.fail(fault);
        }
        result
    }

    #[allow(clippy::too_many_arguments)] // Same borrowed windows, with a single staging transaction.
    fn stage_segment_inner(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
        mut timing: SegmentTiming<impl FnMut(usize) -> Option<ProcessTiming>>,
        origins: EffectSegmentOrigins<'_>,
        mut reply: Option<&mut SegmentReply>,
    ) -> Result<Option<(usize, usize)>, Fault> {
        if self.revoked.is_revoked() {
            return Err(Fault::Revoked);
        }
        let stage = self.staged.as_mut().ok_or(Fault::Protocol)?;
        let end = frame_offset
            .checked_add(input.len())
            .ok_or(Fault::Protocol)?;
        if frame_offset != stage.staged_frames
            || input.is_empty()
            || input.len() != output.len()
            || end > stage.expected_frames
        {
            return Err(Fault::Protocol);
        }
        if !input.iter().flatten().all(|x| x.is_finite()) {
            return Err(Fault::InvalidSamples);
        }
        let input_event_count = stage
            .input_events
            .checked_add(events.len())
            .filter(|count| *count <= MAX_IDENTIFIED_INPUT_NOTES)
            .ok_or(Fault::InvalidEvents)?;
        let legacy_input_events =
            stage.legacy_input_events || events.iter().any(|event| event.note_id.is_none());
        if (input_event_count > MAX_EVENTS && legacy_input_events)
            || !events.iter().all(|e| e.valid(input.len()))
            || events.windows(2).any(|w| w[0].frame > w[1].frame)
        {
            return Err(Fault::InvalidEvents);
        }
        if parameters.len() > MAX_PARAMETER_POINTS - stage.input_parameters
            || !parameters
                .iter()
                .all(|point| parameters::valid(*point, input.len()))
            || parameters
                .windows(2)
                .any(|pair| pair[0].frame > pair[1].frame)
        {
            return Err(Fault::InvalidParameters);
        }
        let q = u64::from(self.quantum);
        let quantum_frames = usize::from(self.quantum);
        let initial_offset = (stage.state.position % q) as usize;
        let EffectSegmentOrigins { authored, manual } = origins;
        if authored.is_some() && !parameters.is_empty() {
            return Err(Fault::InvalidParameters);
        }
        let program = authored
            .as_ref()
            .map(|segment| segment.program.quantum_program_key());
        let clock_epoch = authored
            .as_ref()
            .and_then(|segment| segment.program.authored_clock_epoch());
        if initial_offset != 0
            && self.authored_origins.program.is_some()
            && self.authored_origins.program != program
        {
            let retained = self
                .authored_origins
                .program
                .zip(authored.as_ref())
                .is_some_and(|(key, segment)| segment.program.contains_quantum_program(key));
            if !retained {
                return Err(Fault::InvalidParameters);
            }
        }
        let mut concealed_output = (initial_offset != 0 && stage.state.output_gap)
            .then_some((0, input.len().min(quantum_frames - initial_offset)));
        let mut output_events = 0;
        let mut output_parameters = 0;
        let mut event_index = 0;
        let mut parameter_index = 0;
        let mut input_parameters = parameters.len();
        for (index, (source, destination)) in input.iter().zip(output).enumerate() {
            let position = stage.state.position;
            let offset = (position % q) as usize;
            let sequence = position / q;
            let origin = authored
                .as_ref()
                .and_then(|segment| (segment.origin)(index));
            if offset == 0 {
                stage.state.input = Block::empty(self.binding, self.quantum);
                stage.state.input.sequence = sequence;
                stage.state.input.first_frame = position;
                stage.state.input_manual_origin = manual.and_then(|origin| origin(index));
                let editor_scope = self.revoked.editor_scope.load(Acquire);
                stage.state.input_editor_scope = if editor_scope & 1 != 0 {
                    NonZeroU64::new(editor_scope >> 1)
                } else {
                    None
                };
                stage.state.parameter_capacity = MAX_PARAMETER_POINTS;
                self.authored_origins.open(program, clock_epoch);
                stage.state.input.timing = (timing.at)(index);
                if stage.state.input.timing.is_some_and(|origin| {
                    !origin.is_valid()
                        || timing
                            .playing
                            .is_some_and(|playing| origin.playing != playing)
                }) {
                    return Err(Fault::Protocol);
                }
                if sequence >= self.first_reply_sequence {
                    stage_reply(
                        &mut self.replies,
                        &self.revoked,
                        self.binding,
                        self.quantum,
                        self.conceal_output_gap,
                        &mut stage.state,
                        sequence - 2,
                    )?;
                    if stage.state.output_gap {
                        stage.concealed = Some(DeadlineMiss {
                            reply_sequence: sequence - 2,
                            worker_phase: self.revoked.worker_phase(sequence - 2),
                        });
                        let start = concealed_output.map_or(index, |span| span.0);
                        concealed_output = Some((start, input.len().min(index + quantum_frames)));
                    }
                    if let Some(manual) = stage.state.output.manual {
                        // Extent is at most one quantum, so only one reply
                        // start can fall in the complete physical callback.
                        stage.manual = Some(AcceptedManualParameter {
                            binding: self.binding,
                            sequence: stage.state.output.block.sequence,
                            first_frame: stage.state.output.block.first_frame,
                            callback_offset: (frame_offset + index) as u16,
                            input: manual.input,
                            origin: manual.origin,
                        });
                    }
                    if let Some(editor) = stage.state.output.editor {
                        stage.editor = Some(AcceptedEditorQuantum {
                            binding: self.binding,
                            sequence: stage.state.output.block.sequence,
                            first_frame: stage.state.output.block.first_frame,
                            callback_offset: (frame_offset + index) as u16,
                            scope: editor.scope,
                            performs: editor.facts.performs,
                            restorations: editor.facts.restorations,
                            lost_origin: editor.facts.lost_origin,
                            origin: editor.origin,
                        });
                    }
                }
            } else if origin.is_some()
                && (self
                    .authored_origins
                    .switch_clock(offset, program, clock_epoch)?
                    || self.authored_origins.frames[offset - 1].is_none())
            {
                // A new clock region or resumed history gets its own SDK call.
                // Its first parameter point must not interpolate backward into
                // the preceding region.
                let change = (timing.at)(index)
                    .filter(|origin| origin.playing)
                    .ok_or(Fault::Protocol)?;
                stage.state.input.add_timing_change(offset as u16, change)?;
            } else if index == 0
                && let Some(playing) = timing.playing
                && stage
                    .state
                    .input
                    .latest_timing()
                    .is_some_and(|prior| prior.playing != playing)
            {
                let change = (timing.at)(index)
                    .filter(|origin| origin.playing == playing)
                    .ok_or(Fault::Protocol)?;
                stage.state.input.add_timing_change(offset as u16, change)?;
            }
            stage.state.input.pcm[offset] = *source;
            while let Some(event) = events
                .get(event_index)
                .filter(|e| usize::from(e.frame) == index)
            {
                let mut event = *event;
                event.frame = offset as u16;
                stage.state.input.events.push(event)?;
                event_index += 1;
            }
            while let Some(point) = parameters
                .get(parameter_index)
                .filter(|point| usize::from(point.frame) == index)
            {
                stage.state.input.parameters.push(ParameterPoint {
                    frame: offset as u16,
                    ..*point
                })?;
                parameter_index += 1;
            }
            if sequence >= 2 {
                *destination = stage.state.output.block.pcm[offset];
                for event in stage
                    .state
                    .output
                    .block
                    .events
                    .as_slice()
                    .iter()
                    .filter(|e| usize::from(e.frame) == offset)
                {
                    output_events += 1;
                    if let Some(reply) = reply.as_deref_mut() {
                        let mut event = *event;
                        event.frame = index as u16;
                        reply.events.push(event)?;
                    }
                }
                for point in stage
                    .state
                    .output
                    .block
                    .parameters
                    .as_slice()
                    .iter()
                    .filter(|point| usize::from(point.frame) == offset)
                {
                    output_parameters += 1;
                    if let Some(reply) = reply.as_deref_mut() {
                        reply.parameters.push(ParameterPoint {
                            frame: index as u16,
                            ..*point
                        })?;
                    }
                }
            }
            if authored.is_some() {
                self.authored_origins.frames[offset] = origin;
            }
            stage.state.position = position.checked_add(1).ok_or(Fault::Exhausted)?;
            if offset + 1 == quantum_frames {
                // Absence is also frozen at the opening frame. A programme
                // first supplied mid-quantum starts at the next boundary;
                // it cannot replace this quantum's ordinary parameter input.
                if let Some(authored) = &authored
                    && self.authored_origins.program.is_some()
                {
                    let points = self.authored_origins.render(
                        authored,
                        usize::from(self.quantum),
                        &stage.state.input.timing_changes,
                    )?;
                    input_parameters += points.as_slice().len();
                    if input_parameters > MAX_PARAMETER_POINTS - stage.input_parameters {
                        return Err(Fault::InvalidParameters);
                    }
                    stage.state.input.parameters = points;
                }
                stage.request = Some(InputRequest {
                    block: stage.state.input,
                    parameter_capacity: stage.state.parameter_capacity,
                    authored_program: self.authored_origins.program,
                    manual_origin: stage.state.input_manual_origin,
                    editor_scope: stage.state.input_editor_scope,
                });
            }
        }
        if output_events > MAX_EVENTS - stage.output_events {
            return Err(Fault::InvalidEvents);
        }
        if output_parameters > MAX_PARAMETER_POINTS - stage.output_parameters {
            return Err(Fault::InvalidParameters);
        }
        stage.input_events = input_event_count;
        stage.legacy_input_events = legacy_input_events;
        stage.output_events += output_events;
        stage.input_parameters += input_parameters;
        stage.output_parameters += output_parameters;
        stage.staged_frames = end;
        Ok(concealed_output)
    }
    /// Call only after final host-format validation. Refusal terminates this
    /// epoch; consumed replies and foreign DSP are never rolled back/replayed.
    pub fn finish(&mut self, host_accepted: bool) -> Result<(), Fault> {
        if self.committed_quantum.is_none() {
            // Mark the first transition before any state can be accepted.
            self.revoked
                .quantum_serial
                .store(1, std::sync::atomic::Ordering::SeqCst);
        }
        if !host_accepted {
            return self.fail(Fault::Protocol);
        }
        if self.revoked.is_revoked() {
            return self.fail(Fault::Revoked);
        }
        let Some(mut stage) = self.staged.take() else {
            return self.fail(Fault::Protocol);
        };
        if stage.staged_frames != stage.expected_frames {
            return self.fail(Fault::Protocol);
        }
        let remaining = MAX_PARAMETER_POINTS - stage.input_parameters;
        if let Some(mut request) = stage.request {
            request.parameter_capacity = request.parameter_capacity.min(remaining);
            if self.requests.push(request).is_err() {
                return self.fail(Fault::Saturated);
            }
        }
        if stage.state.position % u64::from(self.quantum) != 0 {
            stage.state.parameter_capacity = stage.state.parameter_capacity.min(remaining);
        } else {
            stage.state.parameter_capacity = MAX_PARAMETER_POINTS;
        }
        self.state = stage.state;
        self.accepted_manual = stage.manual;
        self.accepted_editor = stage.editor;
        let quantum = if self.state.position.is_multiple_of(u64::from(self.quantum)) {
            CommittedQuantumState::Closed
        } else if let Some(key) = self.authored_origins.program {
            CommittedQuantumState::OpenProgram(key)
        } else {
            CommittedQuantumState::OpenUnprogrammed
        };
        // Identical observations carry no additional receipt or authority.
        if self.committed_quantum != Some(quantum) {
            self.revoked.publish_quantum(quantum);
            self.committed_quantum = Some(quantum);
        }
        if let Some(miss) = stage.concealed {
            self.revoked.publish_concealment(miss);
        }
        Ok(())
    }

    /// Consume the exact accepted point once, before the next physical
    /// callback. An unaccepted or revoked endpoint never exposes a marker.
    pub fn take_accepted_manual_parameter(&mut self) -> Option<AcceptedManualParameter> {
        self.accepted_manual
            .take()
            .filter(|_| !self.revoked.is_revoked())
    }

    /// Consume once before the next physical callback. Rejection, revocation
    /// and ending/replacing the control scope never return an old marker.
    pub fn take_accepted_editor_quantum(&mut self) -> Option<AcceptedEditorQuantum> {
        self.accepted_editor.take().filter(|marker| {
            self.revoked.editor_scope.load(Acquire) == ((marker.scope.get() << 1) | 1)
                && !self.revoked.is_revoked()
        })
    }
}

fn stage_reply(
    replies: &mut Consumer<OutputReply>,
    revoked: &Revocation,
    binding: Binding,
    quantum: u16,
    conceal_output_gap: bool,
    state: &mut State,
    expected: u64,
) -> Result<(), Fault> {
    let validate = |reply: &OutputReply, sequence| {
        if conceal_output_gap && !reply.block.events.as_slice().is_empty() {
            return Err(Fault::InvalidEvents);
        }
        reply.valid(binding, quantum, sequence)
    };
    if state.next_output_reply > expected {
        return Err(Fault::Protocol);
    }
    // At most six stale replies and the current reply can be consumed in one
    // callback. Validate every sequence before discarding its PCM and facts.
    for _ in 0..=MAX_CONCEALED_OUTPUT_GAPS {
        match replies.pop() {
            Ok(reply) => {
                validate(&reply, state.next_output_reply)?;
                let sequence = state.next_output_reply;
                state.next_output_reply = sequence.checked_add(1).ok_or(Fault::Exhausted)?;
                if sequence == expected {
                    state.output = reply;
                    state.output_gap = false;
                    state.consecutive_output_gaps = 0;
                    return Ok(());
                }
            }
            Err(_)
                if conceal_output_gap
                    && state.consecutive_output_gaps < MAX_CONCEALED_OUTPUT_GAPS =>
            {
                state.output = OutputReply {
                    block: Block::empty(binding, quantum),
                    manual: None,
                    editor: None,
                };
                state.output_gap = true;
                state.consecutive_output_gaps += 1;
                return Ok(());
            }
            Err(_) => {
                revoked.record_deadline_miss(expected);
                return Err(Fault::MissingOutput);
            }
        }
    }
    Err(Fault::Protocol)
}

impl Worker {
    /// Queue-independent revocation by the joining worker owner. Does not
    /// dispose either endpoint or grant native/canonical acceptance.
    pub fn revoke(&self) {
        self.revoked.revoke();
    }
    /// Diagnostic count after matching completion publication, never custody.
    pub fn completed_blocks(&self) -> u64 {
        self.sequence
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn pending_requests_for_test(&self) -> usize {
        self.requests.slots()
    }
    fn fail<T>(&self, fault: Fault) -> Result<T, Fault> {
        self.revoked.revoke();
        Err(fault)
    }
    /// One nonblocking receive. A revoked lease wins even under queue pressure.
    pub fn receive(&mut self) -> Result<Option<Block>, Fault> {
        self.receive_with_parameter_capacity()
            .map(|request| request.map(|(block, _)| block))
    }
    /// Same original accepted request, with conservative spare input capacity
    /// across its contributing physical callbacks. This budget permits bounded
    /// control input only; it grants no source/publication/canonical authority.
    /// The quantum's Parameters capacity is an independent bound and must
    /// still be checked before adding input.
    pub fn receive_with_parameter_capacity(&mut self) -> Result<Option<(Block, usize)>, Fault> {
        match self.receive_request()? {
            Some(request)
                if request.authored_program.is_some() || request.dense_parameter.is_some() =>
            {
                self.fail(Fault::InvalidParameters)
            }
            Some(request) => Ok(Some((request.block, request.parameter_capacity))),
            None => Ok(None),
        }
    }

    /// Take the original accepted request with its input provenance. An
    /// authored request requires the guarded SDK exchange for this exact source.
    pub fn receive_request(&mut self) -> Result<Option<WorkerRequest>, Fault> {
        if self.revoked.is_revoked() {
            return Err(Fault::Revoked);
        }
        if self.in_flight.is_some() {
            return self.fail(Fault::Protocol);
        }
        let Ok(request) = self.requests.pop() else {
            return Ok(None);
        };
        if request.parameter_capacity > MAX_PARAMETER_POINTS {
            return self.fail(Fault::InvalidParameters);
        }
        let block = request.block;
        if let Err(fault) = block.valid(self.binding, self.quantum, self.sequence) {
            return self.fail(fault);
        }
        let dense_parameter = match self.dense_requests.as_mut() {
            Some(requests) => match dense::receive(requests, &block) {
                Ok(lane) => lane,
                Err(fault) => return self.fail(fault),
            },
            None => None,
        };
        if let Err(fault) = self
            .revoked
            .record_received_program(self.sequence, request.authored_program)
        {
            return self.fail(fault);
        }
        self.in_flight = Some(InFlight {
            manual_origin: request.manual_origin,
            editor_scope: request.editor_scope,
        });
        Ok(Some(WorkerRequest {
            block,
            parameter_capacity: request.parameter_capacity,
            dense_parameter,
            authored_program: request.authored_program,
            manual_origin_available: request.manual_origin.is_some(),
            editor_scope: request.editor_scope,
        }))
    }
    /// Publish exactly one matching reply after the isolated processor finishes.
    /// Revocation discards a late completion; it cannot undo DSP already run.
    pub fn complete(&mut self, reply: Block) -> Result<(), Fault> {
        self.complete_with_manual_parameter(reply, None)
    }

    /// Attach a validated offset-zero host operation to this exact successful
    /// PCM reply. Callers use the original joining worker's input receipt.
    pub fn complete_with_manual_parameter(
        &mut self,
        reply: Block,
        manual: Option<ManualParameterInput>,
    ) -> Result<(), Fault> {
        self.complete_with_parameter_observations(reply, manual, None)
    }

    /// Attach the validated result of the opted-in SDK exchange for this
    /// in-flight request. An empty batch still witnesses its physical quantum;
    /// absence means the SDK exchange did not opt in.
    pub fn complete_with_parameter_observations(
        &mut self,
        reply: Block,
        manual: Option<ManualParameterInput>,
        performs: Option<EditorPerforms>,
    ) -> Result<(), Fault> {
        self.complete_with_editor_facts(
            reply,
            manual,
            performs.map(|performs| EditorPcmFacts {
                performs,
                restorations: Default::default(),
                lost_origin: false,
            }),
        )
    }

    /// Publish the distinct SDK restoration beside the same checked PCM.
    /// The later Main owner still controls final output acceptance.
    pub fn complete_with_editor_facts(
        &mut self,
        reply: Block,
        manual: Option<ManualParameterInput>,
        facts: Option<EditorPcmFacts>,
    ) -> Result<(), Fault> {
        if self.revoked.is_revoked() {
            return Err(Fault::Revoked);
        }
        let Some(in_flight) = self.in_flight.take() else {
            return self.fail(Fault::Protocol);
        };
        self.revoked
            .processed_prefix
            .store(self.sequence.saturating_add(1), Release);
        let editor = match (in_flight.editor_scope, facts) {
            (None, None) => None,
            (Some(scope), Some(facts)) => Some(EditorExchange {
                scope,
                facts,
                origin: in_flight.manual_origin,
            }),
            _ => return self.fail(Fault::Protocol),
        };
        let reply = OutputReply {
            block: reply,
            editor,
            manual: manual.map(|input| ManualExchange {
                input,
                origin: in_flight.manual_origin,
            }),
        };
        if let Err(fault) = reply.valid(self.binding, self.quantum, self.sequence) {
            return self.fail(fault);
        }
        let Some(next) = self.sequence.checked_add(1) else {
            return self.fail(Fault::Exhausted);
        };
        if self.replies.push(reply).is_err() {
            return self.fail(Fault::Saturated);
        }
        self.sequence = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
