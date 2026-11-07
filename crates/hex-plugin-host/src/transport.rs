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
//! Native instruments may conceal one missing reply with silence and discard
//! its late output. No stale-buffer replay, waiting, waking, allocation or
//! DSP rollback occurs.

type Frame = [f32; 2];
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
mod parameters;
mod storage;
pub use parameters::{MAX_PARAMETER_POINTS, ParameterPoint, Parameters};
#[cfg(test)]
mod parameter_tests;
pub mod wire;
pub use storage::{ConstructorStorage, channel_storage_bound};

pub const MAX_FRAMES: usize = 256;
pub const MAX_EVENTS: usize = 64;
/// Reserve 32 hardware MIDI events beside the existing 128 identified notes
/// for authored playback and computer keys. Byte-only MIDI and replies keep
/// the smaller generic event bound.
pub const MAX_IDENTIFIED_INPUT_NOTES: usize = 160;
pub const QUEUE_CAPACITY: usize = 8;

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

#[derive(Default)]
struct Revocation {
    revoked: AtomicBool,
    callback_fault: AtomicU8,
    received_prefix: AtomicU64,
    processed_prefix: AtomicU64,
    missed_reply: AtomicU64,
    missed_phase: AtomicU8,
}
impl Revocation {
    const REPORTED: u8 = 1 << 7;

    fn revoke(&self) {
        self.revoked.store(true, Release);
    }

    fn is_revoked(&self) -> bool {
        self.revoked.load(Acquire)
    }

    fn record_callback_fault(&self, fault: Fault) {
        let _ = self
            .callback_fault
            .compare_exchange(0, fault as u8, Release, Relaxed);
        self.revoke();
    }

    fn record_deadline_miss(&self, sequence: u64) {
        let phase = if self.processed_prefix.load(Acquire) > sequence {
            DeadlineWorkerPhase::Publishing
        } else if self.received_prefix.load(Acquire) > sequence {
            DeadlineWorkerPhase::Processing
        } else {
            DeadlineWorkerPhase::AwaitingPickup
        };
        self.missed_reply.store(sequence, Relaxed);
        self.missed_phase.store(phase as u8, Release);
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

pub const MAX_TIMING_CHANGES: usize = 7;

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
impl Control {
    pub fn revoke(&self) {
        self.revoked.revoke();
    }
    pub fn is_revoked(&self) -> bool {
        self.revoked.is_revoked()
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
    authored: bool,
    manual_origin: Option<ManualParameterOrigin>,
}

pub struct WorkerRequest {
    pub block: Block,
    pub parameter_capacity: usize,
    pub authored: bool,
}

/// The one exact host operation applied at offset zero in a successful SDK
/// exchange. This metadata is attached by the joining worker after native
/// validation and never enters the native request/reply wire.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ManualParameterInput {
    pub operation: NonZeroU64,
    pub point: ParameterPoint,
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

#[derive(Clone, Copy)]
struct OutputReply {
    block: Block,
    manual: Option<ManualExchange>,
}

impl OutputReply {
    fn valid(&self, binding: Binding, quantum: u16, sequence: u64) -> Result<(), Fault> {
        self.block.valid_reply(binding, quantum, sequence)?;
        if self.manual.is_some_and(|manual| {
            manual.input.point.frame != 0
                || !parameters::valid(manual.input.point, usize::from(quantum))
        }) {
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
    output: OutputReply,
    output_gap: bool,
    parameter_capacity: usize,
}
#[derive(Clone, Copy)]
struct Stage {
    state: State,
    request: Option<InputRequest>,
    manual: Option<AcceptedManualParameter>,
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
}

/// Callback endpoint. Install before native callback reservation; return the
/// complete owner through the existing terminal guards before dropping it.
pub struct Callback {
    binding: Binding,
    quantum: u16,
    first_reply_sequence: u64,
    conceal_instrument_gap: bool,
    requests: Producer<InputRequest>,
    replies: Consumer<OutputReply>,
    revoked: Arc<Revocation>,
    state: State,
    staged: Option<Stage>,
    accepted_manual: Option<AcceptedManualParameter>,
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
}

struct InFlight {
    manual_origin: Option<ManualParameterOrigin>,
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

/// The native instrument wire carries PCM and parameter observations but no
/// output MIDI. A single deadline miss can leave one quantum silent while
/// preserving ordered input and the fixed 2Q alignment of subsequent replies.
pub fn instrument_channel_from_completed_prefix(
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
    conceal_instrument_gap: bool,
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
    let revoked = Arc::new(Revocation::default());
    let empty = Block::empty(binding, quantum);
    Ok((
        Control {
            revoked: Arc::clone(&revoked),
        },
        Callback {
            binding,
            quantum,
            first_reply_sequence,
            conceal_instrument_gap,
            requests: request_tx,
            replies: reply_rx,
            revoked: Arc::clone(&revoked),
            state: State {
                position,
                input: empty,
                input_manual_origin: None,
                output: OutputReply {
                    block: empty,
                    manual: None,
                },
                output_gap: false,
                parameter_capacity: MAX_PARAMETER_POINTS,
            },
            staged: None,
            accepted_manual: None,
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
    /// An audible Pause can end inside the physical callback. The caller
    /// proves the exact end of playback; a contiguous staged prefix ending
    /// there may omit the unvisited, paused suffix. A fully staged monitoring
    /// path keeps its original extent. Final host acceptance is still required.
    pub fn end_staging_at_pause(&mut self, frame: usize) {
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
        self.staged = Some(Stage {
            state: self.state,
            request: None,
            manual: None,
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
        output.fill([0.0; 2]);
        let result = self.stage_segment_inner(
            frame_offset,
            input,
            events,
            parameters,
            output,
            timing,
            origins,
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
    ) -> Result<SegmentReply, Fault> {
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
        let EffectSegmentOrigins { authored, manual } = origins;
        if authored.is_some() && !parameters.is_empty() {
            return Err(Fault::InvalidParameters);
        }
        let program = authored
            .as_ref()
            .map(|segment| segment.program.quantum_program_key());
        if stage.state.position % q != 0 && self.authored_origins.program != program {
            return Err(Fault::InvalidParameters);
        }
        let mut result = SegmentReply {
            events: Events::default(),
            parameters: Parameters::default(),
        };
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
                stage.state.parameter_capacity = MAX_PARAMETER_POINTS;
                self.authored_origins.program = program;
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
                        self.conceal_instrument_gap,
                        &mut stage.state,
                        sequence - 2,
                    )?;
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
                }
            } else if origin.is_some() && self.authored_origins.frames[offset - 1].is_none() {
                // Starting a new authored run after unavailable history needs
                // an SDK call of its own. Otherwise the first parameter point
                // interpolates backward from the previous call's held value.
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
                    let mut event = *event;
                    event.frame = index as u16;
                    result.events.push(event)?;
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
                    result.parameters.push(ParameterPoint {
                        frame: index as u16,
                        ..*point
                    })?;
                }
            }
            if authored.is_some() {
                self.authored_origins.frames[offset] = origin;
            }
            stage.state.position = position.checked_add(1).ok_or(Fault::Exhausted)?;
            if offset + 1 == usize::from(self.quantum) {
                if let Some(authored) = &authored {
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
                    authored: authored.is_some(),
                    manual_origin: stage.state.input_manual_origin,
                });
            }
        }
        if result.events.as_slice().len() > MAX_EVENTS - stage.output_events {
            return Err(Fault::InvalidEvents);
        }
        if result.parameters.as_slice().len() > MAX_PARAMETER_POINTS - stage.output_parameters {
            return Err(Fault::InvalidParameters);
        }
        stage.input_events = input_event_count;
        stage.legacy_input_events = legacy_input_events;
        stage.output_events += result.events.as_slice().len();
        stage.input_parameters += input_parameters;
        stage.output_parameters += result.parameters.as_slice().len();
        stage.staged_frames = end;
        Ok(result)
    }
    /// Call only after final host-format validation. Refusal terminates this
    /// epoch; consumed replies and foreign DSP are never rolled back/replayed.
    pub fn finish(&mut self, host_accepted: bool) -> Result<(), Fault> {
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
        Ok(())
    }

    /// Consume the exact accepted point once, before the next physical
    /// callback. An unaccepted or revoked endpoint never exposes a marker.
    pub fn take_accepted_manual_parameter(&mut self) -> Option<AcceptedManualParameter> {
        self.accepted_manual
            .take()
            .filter(|_| !self.revoked.is_revoked())
    }
}

fn stage_reply(
    replies: &mut Consumer<OutputReply>,
    revoked: &Revocation,
    binding: Binding,
    quantum: u16,
    conceal_instrument_gap: bool,
    state: &mut State,
    expected: u64,
) -> Result<(), Fault> {
    let validate = |reply: &OutputReply, sequence| {
        if conceal_instrument_gap && !reply.block.events.as_slice().is_empty() {
            return Err(Fault::InvalidEvents);
        }
        reply.valid(binding, quantum, sequence)
    };
    let mut reply = replies.pop();
    if state.output_gap
        && let Ok(late) = &reply
        && expected.checked_sub(1) == Some(late.block.sequence)
    {
        // At most one concealed block may precede the sample-aligned reply.
        validate(late, late.block.sequence)?;
        reply = replies.pop();
    }
    match reply {
        Ok(reply) => {
            validate(&reply, expected)?;
            state.output = reply;
            state.output_gap = false;
        }
        Err(_) if conceal_instrument_gap && !state.output_gap => {
            state.output = OutputReply {
                block: Block::empty(binding, quantum),
                manual: None,
            };
            state.output_gap = true;
        }
        Err(_) => {
            revoked.record_deadline_miss(expected);
            return Err(Fault::MissingOutput);
        }
    }
    Ok(())
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
            Some(request) if request.authored => self.fail(Fault::InvalidParameters),
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
        self.revoked
            .received_prefix
            .store(self.sequence.saturating_add(1), Release);
        self.in_flight = Some(InFlight {
            manual_origin: request.manual_origin,
        });
        Ok(Some(WorkerRequest {
            block,
            parameter_capacity: request.parameter_capacity,
            authored: request.authored,
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
        if self.revoked.is_revoked() {
            return Err(Fault::Revoked);
        }
        let Some(in_flight) = self.in_flight.take() else {
            return self.fail(Fault::Protocol);
        };
        self.revoked
            .processed_prefix
            .store(self.sequence.saturating_add(1), Release);
        let reply = OutputReply {
            block: reply,
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
