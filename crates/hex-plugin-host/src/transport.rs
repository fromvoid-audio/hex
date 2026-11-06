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
//! Missing/malformed output or saturation revokes the entire one-shot exchange.
//! No stale-buffer replay, waiting, waking, allocation or DSP rollback occurs.

type Frame = [f32; 2];
use hex_lattice_contract::ForeignProcessorBinding as Binding;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicBool,
        Ordering::{Acquire, Release},
    },
};
mod parameters;
mod storage;
pub use parameters::{MAX_PARAMETER_POINTS, ParameterPoint, Parameters};
#[cfg(test)]
mod parameter_tests;
pub mod wire;
pub use storage::{ConstructorStorage, channel_storage_bound};

pub const MAX_FRAMES: usize = 256;
pub const MAX_EVENTS: usize = 64;
pub const QUEUE_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Configuration,
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
pub struct Events {
    values: [MidiEvent; MAX_EVENTS],
    len: usize,
}
impl Default for Events {
    fn default() -> Self {
        Self {
            values: [MidiEvent::default(); MAX_EVENTS],
            len: 0,
        }
    }
}
impl Events {
    pub fn as_slice(&self) -> &[MidiEvent] {
        &self.values[..self.len]
    }
    pub fn push(&mut self, event: MidiEvent) -> Result<(), Fault> {
        if self.len == MAX_EVENTS
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

/// Inline wire payload; dropping it never reclaims a resource.
#[derive(Clone, Copy)]
pub struct Block {
    pub binding: Binding,
    pub sequence: u64,
    pub first_frame: u64,
    pub frames: u16,
    pub pcm: [Frame; MAX_FRAMES],
    pub events: Events,
    pub parameters: Parameters,
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
        }
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
        Ok(())
    }
}

/// Queue-independent revocation. Retain this control lease through worker join
/// and callback terminal return; neither endpoint may be destroyed on callback.
pub struct Control {
    revoked: Arc<AtomicBool>,
}
impl Control {
    pub fn revoke(&self) {
        self.revoked.store(true, Release);
    }
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(Acquire)
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.revoke();
    }
}

#[derive(Clone, Copy)]
struct State {
    position: u64,
    input: Block,
    output: Block,
}
#[derive(Clone, Copy)]
struct Stage {
    state: State,
    request: Option<Block>,
    expected_frames: usize,
    staged_frames: usize,
    input_events: usize,
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
    requests: Producer<Block>,
    replies: Consumer<Block>,
    revoked: Arc<AtomicBool>,
    state: State,
    staged: Option<Stage>,
    fault: Option<Fault>,
}

/// Serialized worker endpoint; process at most one received block at a time.
/// IPC, waiting and worker shutdown belong to its joining control owner.
pub struct Worker {
    binding: Binding,
    quantum: u16,
    requests: Consumer<Block>,
    replies: Producer<Block>,
    revoked: Arc<AtomicBool>,
    sequence: u64,
    in_flight: bool,
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
    if !binding.is_valid() || quantum == 0 || usize::from(quantum) > MAX_FRAMES {
        return Err(Fault::Configuration);
    }
    let position = completed_blocks
        .checked_mul(u64::from(quantum))
        .ok_or(Fault::Exhausted)?;
    let first_reply_sequence = completed_blocks.checked_add(2).ok_or(Fault::Exhausted)?;
    let (request_tx, request_rx) = RingBuffer::new(QUEUE_CAPACITY);
    let (reply_tx, reply_rx) = RingBuffer::new(QUEUE_CAPACITY);
    let revoked = Arc::new(AtomicBool::new(false));
    let empty = Block::empty(binding, quantum);
    Ok((
        Control {
            revoked: Arc::clone(&revoked),
        },
        Callback {
            binding,
            quantum,
            first_reply_sequence,
            requests: request_tx,
            replies: reply_rx,
            revoked: Arc::clone(&revoked),
            state: State {
                position,
                input: empty,
                output: empty,
            },
            staged: None,
            fault: None,
        },
        Worker {
            binding,
            quantum,
            requests: request_rx,
            replies: reply_tx,
            revoked,
            sequence: completed_blocks,
            in_flight: false,
        },
    ))
}

impl Callback {
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
        self.revoked.store(true, Release);
        self.staged = None;
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
    /// Current epoch revocation, including a worker/control request that the
    /// callback has not yet visited. This read grants no future liveness.
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(Acquire)
    }
    pub fn fault(&self) -> Option<Fault> {
        self.fault
    }
    fn fail<T>(&mut self, fault: Fault) -> Result<T, Fault> {
        self.revoked.store(true, Release);
        self.fault.get_or_insert(fault);
        self.staged = None;
        Err(fault)
    }
    /// Declare one actual physical callback, even when the graph renders it in
    /// several ordered segments. The complete extent remains at most Q; graph
    /// chunking never grants a second dispatch or same-callback worker wait.
    pub fn begin_physical_callback(&mut self, frames: usize) -> Result<(), Fault> {
        if self.revoked.load(Acquire) {
            return self.fail(Fault::Revoked);
        }
        if self.staged.is_some() || frames == 0 || frames > usize::from(self.quantum) {
            return self.fail(Fault::Protocol);
        }
        self.staged = Some(Stage {
            state: self.state,
            request: None,
            expected_frames: frames,
            staged_frames: 0,
            input_events: 0,
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
        output.fill([0.0; 2]);
        let result = self.stage_segment_inner(frame_offset, input, events, parameters, output);
        if let Err(fault) = result {
            output.fill([0.0; 2]);
            return self.fail(fault);
        }
        result
    }
    fn stage_segment_inner(
        &mut self,
        frame_offset: usize,
        input: &[Frame],
        events: &[MidiEvent],
        parameters: &[ParameterPoint],
        output: &mut [Frame],
    ) -> Result<SegmentReply, Fault> {
        if self.revoked.load(Acquire) {
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
        if events.len() > MAX_EVENTS - stage.input_events
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
        let mut result = SegmentReply {
            events: Events::default(),
            parameters: Parameters::default(),
        };
        let mut event_index = 0;
        let mut parameter_index = 0;
        for (index, (source, destination)) in input.iter().zip(output).enumerate() {
            let position = stage.state.position;
            let offset = (position % q) as usize;
            let sequence = position / q;
            if offset == 0 {
                stage.state.input = Block::empty(self.binding, self.quantum);
                stage.state.input.sequence = sequence;
                stage.state.input.first_frame = position;
                if sequence >= self.first_reply_sequence {
                    let reply = self.replies.pop().map_err(|_| Fault::MissingOutput)?;
                    reply.valid(self.binding, self.quantum, sequence - 2)?;
                    stage.state.output = reply;
                }
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
                *destination = stage.state.output.pcm[offset];
                for event in stage
                    .state
                    .output
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
            stage.state.position = position.checked_add(1).ok_or(Fault::Exhausted)?;
            if offset + 1 == usize::from(self.quantum) {
                stage.request = Some(stage.state.input);
            }
        }
        if result.events.as_slice().len() > MAX_EVENTS - stage.output_events {
            return Err(Fault::InvalidEvents);
        }
        if result.parameters.as_slice().len() > MAX_PARAMETER_POINTS - stage.output_parameters {
            return Err(Fault::InvalidParameters);
        }
        stage.input_events += events.len();
        stage.output_events += result.events.as_slice().len();
        stage.input_parameters += parameters.len();
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
        if self.revoked.load(Acquire) {
            return self.fail(Fault::Revoked);
        }
        let Some(stage) = self.staged.take() else {
            return self.fail(Fault::Protocol);
        };
        if stage.staged_frames != stage.expected_frames {
            return self.fail(Fault::Protocol);
        }
        if let Some(request) = stage.request
            && self.requests.push(request).is_err()
        {
            return self.fail(Fault::Saturated);
        }
        self.state = stage.state;
        Ok(())
    }
}

impl Worker {
    /// Queue-independent revocation by the joining worker owner. Does not
    /// dispose either endpoint or grant native/canonical acceptance.
    pub fn revoke(&self) {
        self.revoked.store(true, Release);
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
        self.revoked.store(true, Release);
        Err(fault)
    }
    /// One nonblocking receive. A revoked lease wins even under queue pressure.
    pub fn receive(&mut self) -> Result<Option<Block>, Fault> {
        if self.revoked.load(Acquire) {
            return Err(Fault::Revoked);
        }
        if self.in_flight {
            return self.fail(Fault::Protocol);
        }
        let Ok(block) = self.requests.pop() else {
            return Ok(None);
        };
        if let Err(fault) = block.valid(self.binding, self.quantum, self.sequence) {
            return self.fail(fault);
        }
        self.in_flight = true;
        Ok(Some(block))
    }
    /// Publish exactly one matching reply after the isolated processor finishes.
    /// Revocation discards a late completion; it cannot undo DSP already run.
    pub fn complete(&mut self, reply: Block) -> Result<(), Fault> {
        if self.revoked.load(Acquire) {
            return Err(Fault::Revoked);
        }
        if !self.in_flight {
            return self.fail(Fault::Protocol);
        }
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
        self.in_flight = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
