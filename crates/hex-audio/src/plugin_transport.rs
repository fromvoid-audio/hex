//! Copied, bounded transport for an isolated processor worker.
//!
//! Control constructs both rings and retains their endpoints until callback
//! quiescence. Only the worker may perform IPC or native processing. This owner
//! does not load plugins, admit devices/graphs, or promise worker deadlines.
//! A binding is an equality fence, not proof of native resource admission.
//!
//! One immutable quantum Q (at most 256 stereo frames) has exactly 2Q frames of
//! pipeline latency. A physical callback must contain at most Q frames. Input is
//! staged until final host conversion succeeds; only then may it reach a worker.
//! Missing/malformed output or saturation revokes the entire one-shot exchange.
//! No stale-buffer replay, waiting, waking, allocation or DSP rollback occurs.

use hex_engine::Frame;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicBool,
        Ordering::{Acquire, Release},
    },
};
pub mod wire;
mod worker;
pub(crate) use worker::Reaper;
pub(crate) use worker::spawn;

pub const MAX_FRAMES: usize = 256;
pub const MAX_EVENTS: usize = 64;
pub const QUEUE_CAPACITY: usize = 8;

/// Exact instance/root/graph/transport epoch. Recreate on any discontinuity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub root: u64,
    pub instance: u64,
    pub graph: u64,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Configuration,
    Revoked,
    Protocol,
    InvalidSamples,
    InvalidEvents,
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
}

/// Callback endpoint. Install before native callback reservation; return the
/// complete owner through the existing terminal guards before dropping it.
pub struct Callback {
    binding: Binding,
    quantum: u16,
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
    if [binding.root, binding.instance, binding.graph, binding.epoch].contains(&0)
        || quantum == 0
        || usize::from(quantum) > MAX_FRAMES
    {
        return Err(Fault::Configuration);
    }
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
            requests: request_tx,
            replies: reply_rx,
            revoked: Arc::clone(&revoked),
            state: State {
                position: 0,
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
            sequence: 0,
            in_flight: false,
        },
    ))
}

impl Callback {
    pub(crate) fn reject(&mut self, fault: Fault) {
        let _ = self.fail::<()>(fault);
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }
    pub fn quantum_frames(&self) -> usize {
        usize::from(self.quantum)
    }
    pub fn revoke(&self) {
        self.revoked.store(true, Release);
    }
    pub fn latency_frames(&self) -> u32 {
        u32::from(self.quantum) * 2
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
    /// Stage one actual physical callback (not one chunk of a larger callback).
    /// Output events use offsets within this callback. The first 2Q frames are
    /// explicit pipeline warmup silence, not an audible-start guarantee.
    pub fn stage(
        &mut self,
        input: &[Frame],
        events: &[MidiEvent],
        output: &mut [Frame],
    ) -> Result<Events, Fault> {
        output.fill([0.0; 2]);
        let result = self.stage_inner(input, events, output);
        if let Err(fault) = result {
            output.fill([0.0; 2]);
            return self.fail(fault);
        }
        result
    }
    fn stage_inner(
        &mut self,
        input: &[Frame],
        events: &[MidiEvent],
        output: &mut [Frame],
    ) -> Result<Events, Fault> {
        if self.revoked.load(Acquire) {
            return Err(Fault::Revoked);
        }
        if self.staged.is_some()
            || input.is_empty()
            || input.len() != output.len()
            || input.len() > usize::from(self.quantum)
        {
            return Err(Fault::Protocol);
        }
        if !input.iter().flatten().all(|x| x.is_finite()) {
            return Err(Fault::InvalidSamples);
        }
        if events.len() > MAX_EVENTS
            || !events.iter().all(|e| e.valid(input.len()))
            || events.windows(2).any(|w| w[0].frame > w[1].frame)
        {
            return Err(Fault::InvalidEvents);
        }
        let mut stage = Stage {
            state: self.state,
            request: None,
        };
        let q = u64::from(self.quantum);
        let mut result = Events::default();
        let mut event_index = 0;
        for (index, (source, destination)) in input.iter().zip(output).enumerate() {
            let position = stage.state.position;
            let offset = (position % q) as usize;
            let sequence = position / q;
            if offset == 0 {
                stage.state.input = Block::empty(self.binding, self.quantum);
                stage.state.input.sequence = sequence;
                stage.state.input.first_frame = position;
                if sequence >= 2 {
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
                    result.push(event)?;
                }
            }
            stage.state.position = position.checked_add(1).ok_or(Fault::Exhausted)?;
            if offset + 1 == usize::from(self.quantum) {
                stage.request = Some(stage.state.input);
            }
        }
        self.staged = Some(stage);
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
        if let Some(request) = stage.request {
            if self.requests.push(request).is_err() {
                return self.fail(Fault::Saturated);
            }
        }
        self.state = stage.state;
        Ok(())
    }
}

impl Worker {
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
