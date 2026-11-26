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
        AtomicBool, AtomicU8, AtomicU32, AtomicU64,
        Ordering::{AcqRel, Acquire, Relaxed, Release},
    },
};
mod authored;
mod callback;
mod control;
mod types;
mod worker;
use authored::AuthoredOrigins;
pub use authored::{AuthoredParameterProgram, AuthoredSegment};
pub use control::{Control, EditorScope, ReceivedProgram};
use types::Revocation;
pub use types::{
    Block, CommittedQuantum, CommittedQuantumState, ConcealedOutput, DeadlineMiss,
    DeadlineWorkerPhase, DenseParameterLane, Events, Fault, InputEvents, MAX_TIMING_CHANGES,
    MidiEvent, ProcessTiming, QuantumObservation, SegmentTiming, TerminalDeadlineMiss,
    TimingChange, TimingChanges,
};
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
    sidechain: Option<[Frame; MAX_FRAMES]>,
    // Minimum spare capacity of every physical callback contributing to this
    // quantum. Control-originated input may consume it only after acceptance.
    parameter_capacity: usize,
    authored_program: Option<NonZeroU64>,
    manual_origin: Option<ManualParameterOrigin>,
    editor_scope: Option<NonZeroU64>,
    /// Optional monotonic nanosecond timestamp stamped at exact queue commit
    /// (`Callback::finish_with_publish_timestamp`). `None` when the callback
    /// clock is unavailable or omitted; never fabricated from an earlier entry.
    published_at_ns: Option<u64>,
}

pub struct WorkerRequest {
    pub block: Block,
    /// Aligned stereo external sidechain key frames for this quantum, when
    /// routed to a sidechain-capable VST3 effect via `SignalGraph`'s `KeyStage`.
    pub sidechain: Option<[Frame; MAX_FRAMES]>,
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
    /// Optional monotonic nanosecond timestamp captured at exact queue commit,
    /// or `None` when unavailable.
    pub published_at_ns: Option<u64>,
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
    pub(crate) fn valid(&self, binding: Binding, quantum: u16, sequence: u64) -> Result<(), Fault> {
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
    input_sidechain: Option<[Frame; MAX_FRAMES]>,
    input_manual_origin: Option<ManualParameterOrigin>,
    input_editor_scope: Option<NonZeroU64>,
    output: OutputReply,
    output_event_cursor: usize,
    output_parameter_cursor: usize,
    output_gap: bool,
    consecutive_output_gaps: u8,
    next_output_reply: u64,
    parameter_capacity: usize,
}
/// Lightweight per-callback staging metadata. Because any callback staging
/// failure or refusal permanently revokes the endpoint, `Callback::state` and
/// `Callback::staged_request` are mutated in place without ~50 KiB rollback
/// copies on every physical callback.
#[derive(Clone, Copy)]
struct Stage {
    has_request: bool,
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

#[cfg(test)]
mod stage_layout_tests {
    use super::*;

    #[test]
    fn stage_metadata_does_not_embed_state_or_request_rollback_copies() {
        assert!(
            std::mem::size_of::<Stage>() < std::mem::size_of::<State>() / 4,
            "Stage ({} B) must stay lightweight relative to State ({} B)",
            std::mem::size_of::<Stage>(),
            std::mem::size_of::<State>(),
        );
        assert!(
            std::mem::size_of::<Stage>() <= 4096,
            "Stage ({} B) unexpectedly grew",
            std::mem::size_of::<Stage>(),
        );
    }
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
    /// Last committed sample position; updated only in `finish(true)` so
    /// diagnostic/control reads never observe uncommitted staged progress.
    committed_position: u64,
    state: State,
    staged_request: InputRequest,
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
    let revoked = Arc::new(Revocation::new(binding)?);
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
            committed_position: position,
            state: State {
                position,
                input: empty,
                input_sidechain: None,
                input_manual_origin: None,
                input_editor_scope: None,
                output: OutputReply {
                    block: empty,
                    manual: None,
                    editor: None,
                },
                output_event_cursor: 0,
                output_parameter_cursor: 0,
                output_gap: false,
                consecutive_output_gaps: 0,
                next_output_reply: completed_blocks,
                parameter_capacity: MAX_PARAMETER_POINTS,
            },
            staged_request: InputRequest {
                block: empty,
                sidechain: None,
                parameter_capacity: MAX_PARAMETER_POINTS,
                authored_program: None,
                manual_origin: None,
                editor_scope: None,
                published_at_ns: None,
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

#[cfg(test)]
mod tests;
