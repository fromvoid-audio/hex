//! Bounded prepared-playback ownership handoff for native output.
//!
//! Control constructs and destroys graphs. Each callback accepts at most one graph and the bounded
//! command prefix; retirement capacity is checked before ownership moves. A full queue retains
//! owners in the existing held slots, preserving current audio and priority Stop. Stop's
//! independent ticket cutoff discards older commands while retaining later admitted FIFO commands.
//! InheritRuntime preserves compatible transport/route/history. Fresh policies use caller-prepared
//! runtime state; a conditional fresh fence additionally matches the actual predecessor, route
//! generation and admitted-command prefix at the final swap. Canonical editor revision and applied
//! audio revision remain distinct until that acceptance. Queue acceptance, source readiness and
//! physical disposal are separate facts; callback code must never finally destroy graphs, PCM or
//! endpoints.
//! Resident Clients stamp ordinary queued actions with the bound project generation. Compare it
//! with the actual active graph before an action changes that graph. Same-document revisions
//! share the binding. The existing output-wide priority Stop retains its independent cutoff.
//!
//! Ordinary computer keyboard inputs retain only this output's scalar activation clock. Control
//! Stop revokes its reserved prefix before publishing the priority marker; callback consumption
//! preserves only an input bound to that exact clock, allowing newer physical epochs without a
//! graph replacement. Queued explicit Stop, foreign/unbound inputs and lifecycle shutdown remain
//! terminal. Epoch transitions and note snapshots perform no callback allocation or reclamation.
use crate::{
    audio_diagnostics::{self, AudioDiagnosticEvent, AudioDiagnosticKind, AudioDiagnosticsFlags},
    processor_audition, spectrum_target,
    spectrum_worker::{SpectrumCapture, SpectrumStamp},
};
use hex_engine::{
    Frame, Invalid, MAX_SESSION_ROUTE_TRACKS, PlaybackAction, PlaybackActionRejection,
    PlaybackCommand, PlaybackHandoffRejection, PostNodeIdentity, SessionCapabilityStatus,
    playback::Playback,
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicBool, AtomicU32, AtomicU64,
        Ordering::{AcqRel, Acquire, Relaxed, Release, SeqCst},
    },
};
use std::time::Instant;

// Queue capacity and per-block consumption budget must stay equal: every
// pre-cut prefix fits in one callback, so no persistent Stop-discard watermark
// is needed when a later post-cut ticket remains queued.
const COMMANDS: usize = 32;

#[cfg(feature = "allocation-probe")]
impl Callback {
    #[cfg(feature = "stream-test-support")]
    pub(crate) fn test_stream_successor_yielded(&self) -> bool {
        self.yield_stream_successor_once
    }
    /// Read-only headless control observation while the callback is quiescent.
    /// Empty is queue state, not graph/source application or native readiness.
    pub(crate) fn publication_queue_empty_on_control(&self) -> bool {
        self.publications.is_empty()
    }
    /// Quiescent headless observation of the ordinary handoff, including a
    /// consumed publication still awaiting installation. This grants no ACK.
    pub(crate) fn ordinary_publication_idle_on_control(&self) -> bool {
        self.publications.is_empty()
            && self.deferred.is_none()
            && self.deferred_eligible.is_none()
            && !self.status.publication_pending.load(Acquire)
    }
}
mod action_outcomes;
mod audible_clock;
pub use action_outcomes::{
    ACTION_OUTCOME_CAPACITY, Failure as ActionOutcomeFailure, Outcome as ActionOutcome,
    Reader as ActionOutcomeReader, Receipt as ActionOutcomeReceipt,
};
mod admission;
mod eligible_publication;
mod endpoints;
pub(crate) mod retired_scalar_receipts;
pub use retired_scalar_receipts::{
    RetiredReceiptFailure, RetiredScalarReceiptBatch, RetiredScalarReceipts, ScalarReceipt,
    ScalarReceiptTarget, ScalarReceiptTerminal,
};
mod native_effects;
pub(crate) use native_effects::NativeEffectWindow;
mod midi_recording;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod native_sources;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use hex_lattice::ProcessorInstanceIdentity as NativeProcessorIdentity;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use native_sources::NativeParameterContinuationRequest;
mod resident_forwarding;
pub use endpoints::{Client, WorkerControl};
pub(crate) use endpoints::{ClientEndpoints, EndpointTerminals};
pub use resident_forwarding::{ResidentPublicationResponse, ResidentPublicationToken};
#[cfg(all(test, feature = "allocation-probe"))]
mod alias_tests;
mod applied_source;
#[cfg(all(test, feature = "allocation-probe"))]
mod delay_tests;
#[cfg(all(test, feature = "allocation-probe"))]
mod device_resources_tests;
#[cfg(all(test, feature = "allocation-probe"))]
mod keyboard_activation_tests;
#[cfg(all(test, feature = "allocation-probe"))]
mod loop_audio_tests;
#[cfg(test)]
mod project_spectrum_tests;
mod runtime_view;
mod sampler_edit;
mod sampler_keyboard;
mod section_launch;
#[cfg(test)]
mod spectrum_multi_tests;
#[cfg(test)]
mod spectrum_profile;
mod spectrum_stages;
#[cfg(all(test, feature = "allocation-probe"))]
mod tempo_tests;
pub use applied_source::AppliedAudioSource;
pub use sampler_edit::{StoppedEditFence, StoppedEditFenceState};
mod ordinary_streaming;
#[cfg(test)]
use spectrum_stages::SpectrumStage;
use spectrum_stages::SpectrumStages;
mod streaming;
mod transport_fade;
mod transport_observation;
pub use transport_observation::TransportObservation;
pub(crate) const RETIRED: usize = 2;
#[cfg(test)]
pub(crate) mod candidate_tests;
#[cfg(all(test, feature = "allocation-probe"))]
mod final_active_tests;
const ROUTE_READER_ATTEMPTS: usize = 3;
const MAX_TRACKS: usize = MAX_SESSION_ROUTE_TRACKS;
/// Fixed number of route entries accepted by `Status::session_routes`.
pub const SESSION_ROUTE_CAPACITY: usize = MAX_TRACKS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRouteOwner {
    Arrangement,
    Session { clip_id: u64 },
    Silence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRouteTarget {
    Arrangement,
    Session { clip_id: u64 },
    Silence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPendingRoute {
    pub target: SessionRouteTarget,
    pub performance_frame: u64,
    pub beat_raw: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionRouteEntry {
    pub track_id: u64,
    pub owner: SessionRouteOwner,
    /// Accepted source origin, published with the route snapshot sequence.
    pub active_origin: Option<hex_engine::SessionMidiOrigin>,
    /// Accepted Session audio launch on the performance clock, published with
    /// the same sequence. Elapsed source time is `performance_frame - launch`.
    pub audio_launch_frame: Option<u64>,
    pub pending: Option<SessionPendingRoute>,
}

impl Default for SessionRouteEntry {
    fn default() -> Self {
        Self {
            track_id: 0,
            owner: SessionRouteOwner::Silence,
            active_origin: None,
            audio_launch_frame: None,
            pending: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionRouteSnapshot {
    pub audio_revision: u64,
    pub count: usize,
    pub capability: SessionCapabilityStatus,
    pub route_state_generation: u64,
    /// The transport frame at which this route snapshot was published.
    /// It is not a live cursor; read `Status::performance_frame` for that.
    pub performance_at_update: u64,
    /// Effective renderer clock translation from the same accepted revision.
    pub clock_anchor: Option<hex_engine::SessionClockAnchor>,
}

struct AtomicRouteSlot {
    track_id: AtomicU64,
    owner_tag: AtomicU32,
    owner_clip_id: AtomicU64,
    active_origin_tag: AtomicU32,
    active_performance_anchor_raw: AtomicU64,
    active_elapsed_at_anchor_raw: AtomicU64,
    audio_launch_tag: AtomicU32,
    audio_launch_frame: AtomicU64,
    pending_tag: AtomicU32,
    pending_target_tag: AtomicU32,
    pending_clip_id: AtomicU64,
    pending_frame: AtomicU64,
    pending_beat_raw: AtomicU64,
}

impl AtomicRouteSlot {
    fn new() -> Self {
        Self {
            track_id: AtomicU64::new(0),
            owner_tag: AtomicU32::new(0),
            owner_clip_id: AtomicU64::new(0),
            active_origin_tag: AtomicU32::new(0),
            active_performance_anchor_raw: AtomicU64::new(0),
            active_elapsed_at_anchor_raw: AtomicU64::new(0),
            audio_launch_tag: AtomicU32::new(0),
            audio_launch_frame: AtomicU64::new(0),
            pending_tag: AtomicU32::new(0),
            pending_target_tag: AtomicU32::new(0),
            pending_clip_id: AtomicU64::new(0),
            pending_frame: AtomicU64::new(0),
            pending_beat_raw: AtomicU64::new(0),
        }
    }

    fn clear(&self) {
        self.track_id.store(0, SeqCst);
        self.owner_tag.store(0, SeqCst);
        self.owner_clip_id.store(0, SeqCst);
        self.active_origin_tag.store(0, SeqCst);
        self.active_performance_anchor_raw.store(0, SeqCst);
        self.active_elapsed_at_anchor_raw.store(0, SeqCst);
        self.audio_launch_tag.store(0, SeqCst);
        self.audio_launch_frame.store(0, SeqCst);
        self.pending_tag.store(0, SeqCst);
        self.pending_target_tag.store(0, SeqCst);
        self.pending_clip_id.store(0, SeqCst);
        self.pending_frame.store(0, SeqCst);
        self.pending_beat_raw.store(0, SeqCst);
    }
}

#[derive(Clone, Copy)]
struct QueuedCommand {
    ticket: u64,
    action: PlaybackAction,
    project_generation: Option<u64>,
    expected_source: Option<AppliedAudioSource>,
    sampler_edit_gate: u64,
    transport_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CommandCut {
    stop_through: Option<u64>,
    process_through: u64,
}

fn command_cut(entry_ticket: u64, stop_marker: u64) -> CommandCut {
    let stop_through = (stop_marker != 0).then_some(stop_marker.saturating_sub(1));
    CommandCut {
        stop_through,
        process_through: stop_through.map_or(entry_ticket, |stop| stop.max(entry_ticket)),
    }
}

fn action_invalidates_processor_audition(action: PlaybackAction) -> bool {
    match action {
        PlaybackAction::Transport(
            PlaybackCommand::Stop | PlaybackCommand::Seek(_) | PlaybackCommand::PlayFrom(_),
        )
        | PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::Stop | hex_engine::SessionPlaybackCommand::Seek(_),
        ) => true,
        PlaybackAction::Transport(PlaybackCommand::Play | PlaybackCommand::Pause)
        | PlaybackAction::Session(_)
        | PlaybackAction::LaunchAndPlay(_)
        | PlaybackAction::SetMetronome(_) => false,
    }
}

fn is_pause_action(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(PlaybackCommand::Pause)
            | PlaybackAction::Session(hex_engine::SessionPlaybackCommand::Pause)
    )
}

fn starts_playback(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(PlaybackCommand::Play | PlaybackCommand::PlayFrom(_))
            | PlaybackAction::Session(
                hex_engine::SessionPlaybackCommand::Play
                    | hex_engine::SessionPlaybackCommand::LaunchClip { .. }
                    | hex_engine::SessionPlaybackCommand::LaunchScene { .. }
                    | hex_engine::SessionPlaybackCommand::CueMidiClip { .. }
            )
            | PlaybackAction::LaunchAndPlay(_)
    )
}

// A start queued before an output fault must not revive that output, even
// if graph recovery clears the fault before the command reaches the callback.
fn stale_output_start(status: &Status, queued: QueuedCommand) -> bool {
    starts_playback(queued.action)
        && queued.transport_epoch <= status.last_output_fault_epoch.load(Acquire)
}

// A refused PlayFrom keeps its frame as recovery cursor intent. Seek is not
// a start: it still applies to the stopped graph while a render fault is latched.
fn fault_cursor_intent(action: PlaybackAction) -> Option<u64> {
    match action {
        PlaybackAction::Transport(PlaybackCommand::PlayFrom(frame)) => Some(frame),
        _ => None,
    }
}

fn recovery_cursor_action(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(
            PlaybackCommand::Stop | PlaybackCommand::Seek(_) | PlaybackCommand::PlayFrom(_)
        ) | PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::Stop | hex_engine::SessionPlaybackCommand::Seek(_)
        )
    )
}

pub(crate) struct Publication {
    pcm_frames: u64,
    forwarded_pcm: bool,
    revision: u64,
    /// UI/control project context this compiled playback belongs to. This is
    /// audio publication metadata, not project schema or engine state.
    control_context_generation: u64,
    // A legacy default zero is not proof of a real project generation.
    explicit_control_context: bool,
    source_context: Option<hex_engine::AudioSourceContext>,
    policy: PublicationPolicy,
    fresh_runtime_cancellation: Option<Arc<FreshRuntimeCancellation>>,
    quantized_seek_frame: Option<u64>,
    streams: Option<Box<crate::candidate_custody::OrdinaryStreams>>,
    playback: Box<Playback>,
    // Drop the source observation on control before releasing the native
    // source reference, which keeps its actual paired allocation alive.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    native_continuation: Option<hex_plugin_host::transport::ParameterInputSource>,
    // Field order matters: destroy the actual ordinary graph before releasing
    // its native source references. This publication is destroyed on control.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    native_references: Option<crate::candidate_custody::OrdinaryNativeReferences>,
}

struct RenderOutcome {
    audible_recorded: bool,
    start: Option<(u64, u64, bool)>,
}
// Exactly one active musical runtime, including the constructor-owned path.
enum ActivePublication {
    Eligible(crate::candidate_custody::ActiveEligibleCandidate),
    Legacy(Publication),
    Candidate(crate::candidate_custody::ActiveCandidate),
}
impl ActivePublication {
    fn legacy(&self) -> Option<&Publication> {
        match self {
            Self::Legacy(value) => Some(value),
            Self::Candidate(_) | Self::Eligible(_) => None,
        }
    }
    fn legacy_mut(&mut self) -> Option<&mut Publication> {
        match self {
            Self::Legacy(value) => Some(value),
            Self::Candidate(_) | Self::Eligible(_) => None,
        }
    }
}
enum QueuedPublication {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    NativeEpoch {
        target: native_sources::NativeEpochRuntimeTarget,
        fence: StoppedEditFence,
    },
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    Native(crate::candidate_custody::NativeEndpointEnvelope),
    Domain(crate::candidate_custody::DomainEnvelope),
    Eligible(crate::candidate_custody::QueuedEligibleCandidate),
    Legacy(Publication),
    Candidate(crate::candidate_custody::QueuedCandidate),
}
pub(crate) enum ReturnedPublication {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    Native(crate::candidate_custody::NativeEndpointEnvelope),
    Domain(crate::candidate_custody::DomainEnvelope),
    Eligible(crate::candidate_custody::ReturnedEligibleCandidate),
    Legacy(Publication),
    Candidate(crate::candidate_custody::ReturnedCandidate),
}
// A refused queue returns the same fixed 128-byte carrier unchanged. Boxing
// the error would add a separate failure allocation and ownership container.
#[allow(clippy::result_large_err)]
fn enqueue_legacy(
    queue: &mut Producer<QueuedPublication>,
    value: Publication,
) -> Result<(), Publication> {
    let Ok(chunk) = queue.write_chunk_uninit(1) else {
        return Err(value);
    };
    // Safe rtrb API, exactly one reserved slot and std::iter::Once. Refusal
    // precedes moving the owner; no enum fallback can invent queue success.
    chunk.fill_from_iter(std::iter::once(QueuedPublication::Legacy(value)));
    Ok(())
}

/// Controls whether a newly compiled runtime inherits transport and Session
/// route state from the currently active project revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationPolicy {
    /// Preserve transport, launch ownership, and compatible voice state.
    InheritRuntime,
    /// Install a caller-prepared fresh runtime without inheriting from the active one.
    /// The policy does not sanitize a pre-mutated candidate itself.
    InstallFreshRuntime,
    /// Install only if the active runtime, route generation, and command prefix
    /// still match the caller's control-side capture.
    InstallFreshRuntimeIfUnchanged(FreshRuntimeFence),
    /// A prepared quantized seek waits on the predecessor's musical clock.
    /// The first ready callback selects the next grid boundary. Installation
    /// uses ordinary seek/reset semantics, retaining the candidate's DSP clock.
    InstallQuantizedSeekWhenReady {
        fence: FreshRuntimeFence,
        quantization: hex_engine::LaunchQuantization,
    },
    /// Authored structural edit: the shared stopped gate remains held until exact
    /// installation. Stop keeps priority; Play cannot run on the old graph.
    InstallUnderStoppedArrangement(StoppedEditFence),
    /// Same-source native renewal after actual stopped graph/endpoint return.
    /// Only its ready native gate admits this ordinary successor; it does not
    /// inherit an old revoked runtime or authorize a different source context.
    InstallUnderNativeEpoch(StoppedEditFence),
    /// Inherit the exact still-active ordinary native graph while its stopped
    /// fence remains held. This does not renew or return its original sources.
    InheritUnderStoppedNative {
        source: AppliedAudioSource,
        fence: StoppedEditFence,
    },
    /// Inherit only while the native parameter gesture's exact ordinary source
    /// and transport are still current. This grants no native worker custody.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    InheritUnderNativeParameter {
        source: AppliedAudioSource,
        transport_epoch: u64,
        playing: bool,
    },
    /// Structural live edit under the same source/command gate; inherit the actual
    /// installation-boundary transport, routes and voice/release state.
    InheritUnderArrangement(StoppedEditFence),
}

impl PublicationPolicy {
    fn inherits_runtime(self) -> bool {
        match self {
            Self::InheritRuntime
            | Self::InheritUnderArrangement(_)
            | Self::InheritUnderStoppedNative { .. } => true,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::InheritUnderNativeParameter { .. } => true,
            _ => false,
        }
    }

    fn fresh_fence(self) -> Option<FreshRuntimeFence> {
        match self {
            Self::InstallFreshRuntimeIfUnchanged(fence)
            | Self::InstallQuantizedSeekWhenReady { fence, .. } => Some(fence),
            _ => None,
        }
    }
}

const FRESH_PUBLICATION_PENDING: u32 = 0;
const FRESH_PUBLICATION_APPLYING: u32 = 1;
const FRESH_PUBLICATION_CANCELLED: u32 = 2;
const FRESH_PUBLICATION_COMPLETE: u32 = 3;

/// Control-side result of cancelling a conditionally published fresh runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshRuntimeCancellationResult {
    /// Cancellation won before the callback began installing the candidate.
    Cancelled,
    /// The callback had already crossed its final installation decision.
    Applying,
    /// The callback has already returned or installed this candidate.
    Complete,
    /// The nonblocking Client view is contended; retain and retry the request.
    Busy,
    /// There is no currently retained cancellation decision for this revision.
    UnknownRevision,
}

/// A per-publication decision cell binds a full revision to its own atomic
/// state. The immutable key and CAS live in the same allocation, so there is
/// no split key/state ABA window and no truncated-revision encoding.
struct FreshRuntimeCancellation {
    revision: u64,
    decision: AtomicU32,
}

impl FreshRuntimeCancellation {
    fn request_cancel(&self) -> FreshRuntimeCancellationResult {
        loop {
            match self.decision.load(Acquire) {
                FRESH_PUBLICATION_PENDING => {
                    if self
                        .decision
                        .compare_exchange(
                            FRESH_PUBLICATION_PENDING,
                            FRESH_PUBLICATION_CANCELLED,
                            AcqRel,
                            Acquire,
                        )
                        .is_ok()
                    {
                        return FreshRuntimeCancellationResult::Cancelled;
                    }
                }
                FRESH_PUBLICATION_APPLYING => {
                    return FreshRuntimeCancellationResult::Applying;
                }
                FRESH_PUBLICATION_CANCELLED => {
                    return FreshRuntimeCancellationResult::Cancelled;
                }
                FRESH_PUBLICATION_COMPLETE => {
                    return FreshRuntimeCancellationResult::Complete;
                }
                _ => return FreshRuntimeCancellationResult::Complete,
            }
        }
    }

    fn finish(&self) {
        self.decision.store(FRESH_PUBLICATION_COMPLETE, Release);
    }

    fn finish_for_shutdown(&self) -> bool {
        self.decision.swap(FRESH_PUBLICATION_COMPLETE, AcqRel) == FRESH_PUBLICATION_CANCELLED
    }

    fn begin_apply(&self) -> bool {
        self.decision
            .compare_exchange(
                FRESH_PUBLICATION_PENDING,
                FRESH_PUBLICATION_APPLYING,
                AcqRel,
                Acquire,
            )
            .is_ok()
    }
}

/// Control-side freshness proof for a candidate prepared from an observed
/// runtime state. Revision zero represents an empty audio owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshRuntimeFence {
    expected_active_revision: u64,
    route_state_generation: u64,
    admitted_command_ticket: u64,
    output_fault_epoch: u64,
    consumed_stop_generation: u64,
}

impl FreshRuntimeFence {
    pub const fn expected_active_revision(self) -> u64 {
        self.expected_active_revision
    }

    pub const fn route_state_generation(self) -> u64 {
        self.route_state_generation
    }

    pub const fn admitted_command_ticket(self) -> u64 {
        self.admitted_command_ticket
    }

    pub const fn output_fault_epoch(self) -> u64 {
        self.output_fault_epoch
    }

    /// Callback-owned Stop sequence. Odd values are in flight; exhaustion refuses new fences.
    pub const fn consumed_stop_generation(self) -> u64 {
        self.consumed_stop_generation
    }
}

pub struct Status {
    interface_sample_rate: Option<u32>,
    diagnostics_flags: AtomicU32,
    diagnostics_dropped: AtomicU64,
    sampler_edit: sampler_edit::Fence,
    sampler_keyboard: sampler_keyboard::Mailbox,
    keyboard_activation: Arc<hex_engine::computer_keyboard::KeyboardActivationClock>,
    project_spectrum: crate::spectrum_worker::ProjectSelectionMailbox,
    pending_stop_marker: AtomicU64,
    // Only the callback writes this: even when idle, odd while handling Stop.
    consumed_stop_generation: AtomicU64,
    admitted_ticket: AtomicU64,
    closing: AtomicBool,
    publication_pending: admission::PublicationAdmission,
    preview_used: AtomicBool,
    constructor_mode: AtomicBool,
    verified_streaming_mode: AtomicBool,
    applied_source: applied_source::AppliedSourceStatus,
    recording_source: applied_source::AppliedSourceStatus,
    ordinary_native_revision: AtomicU64,
    pub midi_recording_fault: AtomicBool,
    split_endpoints: AtomicBool,
    client_exposed: AtomicBool,
    submitted_revision: AtomicU64,
    /// Latched for this output after a device error; recovery opens a new output.
    pub device_fault: AtomicBool,
    pub render_fault: AtomicBool,
    render_fault_record: crate::render_fault::RenderFaultRecord,
    pub cpu_meter: crate::cpu_meter::CpuMeter,
    pub callback_max_ns: AtomicU64,
    pub deadline_misses: AtomicU64,
    pub peak_left: AtomicU32,
    pub peak_right: AtomicU32,
    pub applied_revision: AtomicU64,
    /// Highest admitted command consumed, including commands discarded by Stop.
    /// Release publication follows transport state; readers acquire before reading it.
    pub consumed_command: AtomicU64,
    /// Opt-in streamed prefix resolved by ready Play or real Pause/Stop/rejection;
    /// dequeuing a pending Play does not advance this observation.
    pub stream_applied_command_prefix: AtomicU64,
    /// Best-effort render telemetry, never a readiness/ownership receipt.
    pub stream_buffering: AtomicBool,
    /// Monotonic count of callbacks that held streamed playback silent while
    /// the transport was already playing with no Play/action pre-roll pending:
    /// disk pages arrived too late for audible output. Counted per output;
    /// a replacement output starts from zero. Best-effort telemetry.
    pub stream_starved_callbacks: AtomicU64,
    /// Latest streamed launch accepted into actual PendingRoute scheduling.
    pub stream_scheduled_command: AtomicU64,
    /// Latest scheduled launch whose actual due route/render chunk committed.
    pub stream_activated_command: AtomicU64,
    /// Latest schedule observation cancelled or not retained by actual handoff.
    /// This does not claim that an already committed musical route was undone.
    pub stream_invalidated_schedule: AtomicU64,
    pub stream_readiness_refused: AtomicBool,
    /// A demanded active stream reported changed media; retained through Stop
    /// until a replacement graph is installed or the output is replaced;
    /// cached audio alone does not reverify the file.
    pub stream_source_changed: AtomicBool,
    /// Coalescing notification of a demanded audio-page decode/read failure.
    /// The sole control notification observer takes it through
    /// `take_stream_read_failure`; it is not persistent unavailability state.
    pub stream_read_failed: AtomicBool,
    /// Frames committed before the latest BUFFERING gap; zero when complete.
    pub stream_committed_frames: AtomicU64,
    /// Latest rejected command is read through `rejected_action` for a coherent pair.
    rejected_action_ticket: AtomicU64,
    rejected_action_reason: AtomicU32,
    rejected_action_sequence: AtomicU64,
    /// Latest permanently rejected publication; transient handoffs are retried.
    /// Latest rejected graph is read through `rejected_revision` for a coherent pair.
    rejected_revision: AtomicU64,
    rejected_revision_reason: AtomicU32,
    rejected_revision_sequence: AtomicU64,
    /// Scheduled Arrangement cursor: the next content frame the graph renders.
    /// Recording, scheduling and edits use this position.
    pub position: AtomicU64,
    /// Arrangement frame now leaving Main output: `position` delayed by the
    /// graph's Main output latency, mapped back across loop wraps. Use it only
    /// to display the playhead; see `prepared::audible_clock`.
    pub audible_position: AtomicU64,
    // Last callback-published cursor, frozen before fault Stop rewinds the graph.
    recovery_position: AtomicU64,
    pub performance_frame: AtomicU64,
    pub transport_epoch: AtomicU64,
    // A capture admitted at or before this epoch crossed an output fault,
    // even if a queued graph subsequently clears the render-fault flag.
    last_output_fault_epoch: AtomicU64,
    pub gain_recording_fault: AtomicBool,
    pub native_parameter_input_fault: AtomicBool,
    native_parameter_revoked_gesture: AtomicU64,
    pub gain_recording_ready: AtomicBool,
    pub playing: AtomicBool,
    /// Last callback-admitted interactive click state.
    pub metronome_enabled: AtomicBool,
    pub session_capability: AtomicU32,
    sampler_console: AtomicBool,
    pub shutdown_complete: AtomicBool,
    transport_observation: transport_observation::TransportObservationSlot,
    route_sequence: AtomicU64,
    route_revision: AtomicU64,
    route_count: AtomicU32,
    route_capability: AtomicU32,
    route_generation: AtomicU64,
    route_performance_at_update: AtomicU64,
    route_clock_anchor_tag: AtomicU32,
    route_clock_cut_frame: AtomicU64,
    route_clock_old_beat_raw: AtomicU64,
    route_clock_new_beat_raw: AtomicU64,
    route_slots: Box<[AtomicRouteSlot]>,
}

impl Default for Status {
    fn default() -> Self {
        let route_slots = (0..MAX_TRACKS)
            .map(|_| AtomicRouteSlot::new())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self::with_route_slots(route_slots)
    }
}

impl Status {
    /// Nominal rate of this native output's interface, captured at open.
    /// Headless endpoints do not have an interface.
    pub fn interface_sample_rate(&self) -> Option<u32> {
        self.interface_sample_rate
    }

    /// Snapshot of the publication reservation, not a guarantee that admission will succeed:
    /// other actors may own admission, and the state can change immediately after this read.
    pub fn publication_in_flight(&self) -> bool {
        self.publication_pending.load(Acquire)
    }

    #[cfg(feature = "allocation-probe")]
    pub(crate) fn headless_shutdown_requested(&self) -> bool {
        self.closing.load(Acquire)
    }
    #[cfg(test)]
    pub(crate) fn set_split_for_test(&self) {
        self.split_endpoints.store(true, Release);
    }
    #[cfg(test)]
    pub(crate) fn publication_reserve_for_test(&self) -> bool {
        self.publication_pending.reserve_publication()
    }
    #[cfg(test)]
    pub(crate) fn publication_clear_for_test(&self) {
        self.publication_pending.store(false, Release);
    }
    pub(crate) fn preview_permitted(&self) -> bool {
        !self.constructor_mode.load(Acquire)
            && !self.closing.load(Acquire)
            && !self.device_fault.load(Acquire)
            && !self.render_fault.load(Acquire)
    }
    pub(crate) fn mark_preview_used(&self) {
        self.preview_used.store(true, Release);
    }
    pub(crate) fn preview_unused(&self) -> bool {
        !self.preview_used.load(Acquire)
    }
    pub(crate) fn reserve_midi_begin(&self) -> Result<(), &'static str> {
        if self.device_fault.load(Acquire) || self.render_fault.load(Acquire) {
            return Err("midi recording output has failed");
        }
        if self.closing.load(Acquire) {
            return Err("midi recording output is closing");
        }
        if self.verified_streaming_mode.load(Acquire) {
            return Err("midi recording is unavailable for streamed documents");
        }
        if !self.publication_pending.reserve_midi() {
            return Err("playback publication or recording is pending");
        }
        Ok(())
    }
    pub(crate) fn midi_end(&self) {
        self.publication_pending.midi_finished();
    }
    pub(crate) fn reserve_gain_begin(&self) -> Result<bool, &'static str> {
        if self.device_fault.load(Acquire) || self.render_fault.load(Acquire) {
            return Err("gain recording output has failed");
        }
        if self.closing.load(Acquire) {
            return Err("gain recording output is closing");
        }
        if self.constructor_mode.load(Acquire) {
            return Err("gain recording is unavailable for streamed documents");
        }
        if self.publication_pending.midi_active() {
            return Err("finish midi recording before gain recording");
        }
        if self.split_endpoints.load(Acquire) {
            if !self.publication_pending.reserve_gain() {
                return Err("playback publication or gain recording is pending");
            }
            Ok(true)
        } else if self.publication_pending.reserve_legacy_gain() {
            Ok(true)
        } else {
            Err("playback publication or recording is pending")
        }
    }
    pub(crate) fn gain_begin_committed(&self) {
        self.publication_pending.legacy_gain_started();
    }
    pub(crate) fn reserve_native_parameter_input(&self) -> Result<(), &'static str> {
        if self.device_fault.load(Acquire) || self.render_fault.load(Acquire) {
            return Err("native parameter input output has failed");
        }
        if self.closing.load(Acquire) {
            return Err("native parameter input output is closing");
        }
        if !self.publication_pending.reserve_parameter_input() {
            return Err("playback publication or live input is pending");
        }
        Ok(())
    }
    pub(crate) fn native_parameter_input_end(&self) {
        self.publication_pending.parameter_input_finished();
    }
    pub(crate) fn revoke_native_parameter_input(&self, gesture: u64) {
        self.native_parameter_revoked_gesture
            .fetch_max(gesture, AcqRel);
    }
    fn native_parameter_input_revoked(&self, gesture: u64) -> bool {
        self.native_parameter_revoked_gesture.load(Acquire) >= gesture
    }
    pub(crate) fn gain_end(&self) {
        self.publication_pending.gain_finished();
    }
    pub(crate) fn candidate_boundary_data_bytes(&self) -> Option<usize> {
        self.route_slots
            .len()
            .checked_mul(std::mem::size_of::<AtomicRouteSlot>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of_val(self)))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    hex_engine::computer_keyboard::KeyboardActivationClock,
                >())
            })
            .and_then(|bytes| bytes.checked_add(2 * std::mem::size_of::<usize>()))
    }
    fn with_route_slots(route_slots: Box<[AtomicRouteSlot]>) -> Self {
        Self {
            sampler_edit: Default::default(),
            sampler_keyboard: Default::default(),
            keyboard_activation: Arc::default(),
            project_spectrum: Default::default(),
            pending_stop_marker: AtomicU64::new(0),
            consumed_stop_generation: AtomicU64::new(0),
            admitted_ticket: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            publication_pending: admission::PublicationAdmission::default(),
            preview_used: AtomicBool::new(false),
            constructor_mode: AtomicBool::new(false),
            verified_streaming_mode: AtomicBool::new(false),
            applied_source: applied_source::AppliedSourceStatus::default(),
            recording_source: applied_source::AppliedSourceStatus::default(),
            ordinary_native_revision: AtomicU64::new(0),
            midi_recording_fault: AtomicBool::new(false),
            split_endpoints: AtomicBool::new(false),
            client_exposed: AtomicBool::new(false),
            submitted_revision: AtomicU64::new(0),
            device_fault: AtomicBool::new(false),
            render_fault: AtomicBool::new(false),
            render_fault_record: crate::render_fault::RenderFaultRecord::default(),
            interface_sample_rate: None,
            diagnostics_flags: AtomicU32::new(0),
            diagnostics_dropped: AtomicU64::new(0),
            cpu_meter: crate::cpu_meter::CpuMeter::default(),
            callback_max_ns: AtomicU64::new(0),
            deadline_misses: AtomicU64::new(0),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            applied_revision: AtomicU64::new(0),
            consumed_command: AtomicU64::new(0),
            stream_applied_command_prefix: AtomicU64::new(0),
            stream_buffering: AtomicBool::new(false),
            stream_starved_callbacks: AtomicU64::new(0),
            stream_scheduled_command: AtomicU64::new(0),
            stream_activated_command: AtomicU64::new(0),
            stream_invalidated_schedule: AtomicU64::new(0),
            stream_readiness_refused: AtomicBool::new(false),
            stream_source_changed: AtomicBool::new(false),
            stream_read_failed: AtomicBool::new(false),
            stream_committed_frames: AtomicU64::new(0),
            rejected_action_ticket: AtomicU64::new(0),
            rejected_action_reason: AtomicU32::new(0),
            rejected_action_sequence: AtomicU64::new(0),
            rejected_revision: AtomicU64::new(0),
            rejected_revision_reason: AtomicU32::new(0),
            rejected_revision_sequence: AtomicU64::new(0),
            position: AtomicU64::new(0),
            audible_position: AtomicU64::new(0),
            recovery_position: AtomicU64::new(0),
            performance_frame: AtomicU64::new(0),
            transport_epoch: AtomicU64::new(1),
            last_output_fault_epoch: AtomicU64::new(0),
            gain_recording_fault: AtomicBool::new(false),
            native_parameter_input_fault: AtomicBool::new(false),
            native_parameter_revoked_gesture: AtomicU64::new(0),
            gain_recording_ready: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            metronome_enabled: AtomicBool::new(false),
            session_capability: AtomicU32::new(SessionCapabilityStatus::Legacy as u32),
            sampler_console: AtomicBool::new(false),
            shutdown_complete: AtomicBool::new(false),
            transport_observation: Default::default(),
            route_sequence: AtomicU64::new(0),
            route_revision: AtomicU64::new(0),
            route_count: AtomicU32::new(0),
            route_capability: AtomicU32::new(SessionCapabilityStatus::Legacy as u32),
            route_generation: AtomicU64::new(0),
            route_performance_at_update: AtomicU64::new(0),
            route_clock_anchor_tag: AtomicU32::new(0),
            route_clock_cut_frame: AtomicU64::new(0),
            route_clock_old_beat_raw: AtomicU64::new(0),
            route_clock_new_beat_raw: AtomicU64::new(0),
            route_slots,
        }
    }
}

impl Status {
    /// Last callback-published arrangement cursor, retained across a fault Stop.
    /// A pending explicit priority Stop takes precedence and returns zero even
    /// before the callback consumes it.
    /// This is a recovery observation, not the current transport position or a
    /// claim that the corresponding audio reached the physical device.
    pub fn recovery_position(&self) -> u64 {
        if !self.closing.load(Acquire) && self.pending_stop_marker.load(Acquire) != 0 {
            return 0;
        }
        self.recovery_position.load(Acquire)
    }
    /// Current diagnostic configuration; this observation grants no authority.
    pub fn audio_diagnostics_flags(&self) -> AudioDiagnosticsFlags {
        AudioDiagnosticsFlags::from_bits(self.diagnostics_flags.load(Relaxed))
    }
    /// Number of callback trace records lost to the bounded diagnostic queue.
    pub fn audio_diagnostics_dropped(&self) -> u64 {
        self.diagnostics_dropped.load(Relaxed)
    }
    /// Read bounded diagnostic telemetry on control/UI, never in a callback.
    pub fn render_fault_snapshot(&self) -> Option<crate::RenderFaultSnapshot> {
        if !self.render_fault.load(Acquire) {
            return None;
        }
        let snapshot = self.render_fault_record.read();
        if self.render_fault.load(Acquire) {
            snapshot
        } else {
            None
        }
    }
    fn try_new() -> Result<Self, Invalid> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(MAX_TRACKS)
            .map_err(|_| Invalid("could not allocate Session route telemetry"))?;
        slots.extend((0..MAX_TRACKS).map(|_| AtomicRouteSlot::new()));
        Ok(Self::with_route_slots(slots.into_boxed_slice()))
    }

    /// Read one coherent bounded route snapshot into caller-owned storage.
    /// Returns `None` after three contended attempts or when `output` is too
    /// short. Ignore all output entries when it returns `None`.
    pub fn session_routes(&self, output: &mut [SessionRouteEntry]) -> Option<SessionRouteSnapshot> {
        for _ in 0..ROUTE_READER_ATTEMPTS {
            let before = self.route_sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let audio_revision = self.route_revision.load(SeqCst);
            let count = usize::try_from(self.route_count.load(SeqCst)).ok()?;
            let capability =
                SessionCapabilityStatus::from_code(self.route_capability.load(SeqCst))?;
            let route_state_generation = self.route_generation.load(SeqCst);
            let performance_at_update = self.route_performance_at_update.load(SeqCst);
            let clock_anchor = match self.route_clock_anchor_tag.load(SeqCst) {
                0 => None,
                1 => Some(hex_engine::SessionClockAnchor::from_raw(
                    self.route_clock_cut_frame.load(SeqCst),
                    self.route_clock_old_beat_raw.load(SeqCst),
                    self.route_clock_new_beat_raw.load(SeqCst),
                )),
                _ => return None,
            };
            if count > output.len() || count > self.route_slots.len() {
                return None;
            }
            for (index, destination) in output.iter_mut().take(count).enumerate() {
                let slot = self.route_slots.get(index)?;
                let track_id = slot.track_id.load(SeqCst);
                let owner_tag = slot.owner_tag.load(SeqCst);
                let owner_clip_id = slot.owner_clip_id.load(SeqCst);
                let active_origin_tag = slot.active_origin_tag.load(SeqCst);
                let active_performance_anchor_raw = slot.active_performance_anchor_raw.load(SeqCst);
                let active_elapsed_at_anchor_raw = slot.active_elapsed_at_anchor_raw.load(SeqCst);
                let audio_launch_tag = slot.audio_launch_tag.load(SeqCst);
                let audio_launch_frame = slot.audio_launch_frame.load(SeqCst);
                let pending_tag = slot.pending_tag.load(SeqCst);
                let pending_target_tag = slot.pending_target_tag.load(SeqCst);
                let pending_clip_id = slot.pending_clip_id.load(SeqCst);
                let pending_frame = slot.pending_frame.load(SeqCst);
                let pending_beat_raw = slot.pending_beat_raw.load(SeqCst);
                let owner = match owner_tag {
                    1 => SessionRouteOwner::Arrangement,
                    2 => SessionRouteOwner::Session {
                        clip_id: owner_clip_id,
                    },
                    3 => SessionRouteOwner::Silence,
                    _ => return None,
                };
                let active_origin = match active_origin_tag {
                    0 => None,
                    1 => Some(hex_engine::SessionMidiOrigin::from_raw(
                        active_performance_anchor_raw,
                        active_elapsed_at_anchor_raw,
                    )),
                    _ => return None,
                };
                let audio_launch_frame = match audio_launch_tag {
                    0 => None,
                    1 => Some(audio_launch_frame),
                    _ => return None,
                };
                let pending = match pending_tag {
                    0 => None,
                    1 => {
                        let target = match pending_target_tag {
                            1 => SessionRouteTarget::Arrangement,
                            2 => SessionRouteTarget::Session {
                                clip_id: pending_clip_id,
                            },
                            3 => SessionRouteTarget::Silence,
                            _ => return None,
                        };
                        Some(SessionPendingRoute {
                            target,
                            performance_frame: pending_frame,
                            beat_raw: pending_beat_raw,
                        })
                    }
                    _ => return None,
                };
                *destination = SessionRouteEntry {
                    track_id,
                    owner,
                    active_origin,
                    audio_launch_frame,
                    pending,
                };
            }
            let after = self.route_sequence.load(SeqCst);
            if before == after {
                return Some(SessionRouteSnapshot {
                    audio_revision,
                    count,
                    capability,
                    route_state_generation,
                    performance_at_update,
                    clock_anchor,
                });
            }
        }
        None
    }

    /// Read one coherent callback-published transport observation, or `None`
    /// when no supported active runtime is published, the output is closing,
    /// or a bounded read conflicts with a concurrent callback. The result is
    /// telemetry only and can become stale immediately.
    pub fn transport_observation(&self) -> Option<TransportObservation> {
        if self.closing.load(Acquire) {
            return None;
        }
        let value = self.transport_observation.read()?;
        (!self.closing.load(Acquire)).then_some(value)
    }

    fn begin_route_snapshot(&self) {
        self.route_sequence.fetch_add(1, SeqCst);
    }

    fn end_route_snapshot(&self) {
        self.route_sequence.fetch_add(1, SeqCst);
    }

    fn publish_route_header(
        &self,
        revision: u64,
        count: usize,
        capability: SessionCapabilityStatus,
        generation: u64,
        performance_at_update: u64,
        clock_anchor: Option<hex_engine::SessionClockAnchor>,
    ) {
        self.route_revision.store(revision, SeqCst);
        self.route_count.store(count as u32, SeqCst);
        self.route_capability.store(capability as u32, SeqCst);
        self.route_generation.store(generation, SeqCst);
        self.route_performance_at_update
            .store(performance_at_update, SeqCst);
        if let Some(anchor) = clock_anchor {
            self.route_clock_cut_frame.store(anchor.cut_frame, SeqCst);
            self.route_clock_old_beat_raw
                .store(anchor.old_beat.raw(), SeqCst);
            self.route_clock_new_beat_raw
                .store(anchor.new_beat.raw(), SeqCst);
            self.route_clock_anchor_tag.store(1, SeqCst);
        } else {
            self.route_clock_cut_frame.store(0, SeqCst);
            self.route_clock_old_beat_raw.store(0, SeqCst);
            self.route_clock_new_beat_raw.store(0, SeqCst);
            self.route_clock_anchor_tag.store(0, SeqCst);
        }
    }

    fn publish_empty_routes(&self) {
        self.metronome_enabled.store(false, Relaxed);
        self.begin_route_snapshot();
        for slot in self.route_slots.iter() {
            slot.clear();
        }
        self.publish_route_header(0, 0, SessionCapabilityStatus::Legacy, 0, 0, None);
        self.sampler_console.store(false, Release);
        self.session_capability
            .store(SessionCapabilityStatus::Legacy as u32, Relaxed);
        self.end_route_snapshot();
    }

    fn publish_route_slot(&self, index: usize, route: hex_engine::SessionRouteStatus) {
        let Some(slot) = self.route_slots.get(index) else {
            return;
        };
        slot.track_id.store(route.track.value(), SeqCst);
        match route.owner {
            hex_engine::SessionPlaybackOwner::Arrangement => {
                slot.owner_tag.store(1, SeqCst);
                slot.owner_clip_id.store(0, SeqCst);
            }
            hex_engine::SessionPlaybackOwner::Session { clip } => {
                slot.owner_tag.store(2, SeqCst);
                slot.owner_clip_id.store(clip.value(), SeqCst);
            }
            hex_engine::SessionPlaybackOwner::Silence => {
                slot.owner_tag.store(3, SeqCst);
                slot.owner_clip_id.store(0, SeqCst);
            }
        }
        if let Some(origin) = route.active_origin {
            slot.active_performance_anchor_raw
                .store(origin.performance_anchor.raw(), SeqCst);
            slot.active_elapsed_at_anchor_raw
                .store(origin.elapsed_at_anchor.raw(), SeqCst);
            slot.active_origin_tag.store(1, SeqCst);
        } else {
            slot.active_performance_anchor_raw.store(0, SeqCst);
            slot.active_elapsed_at_anchor_raw.store(0, SeqCst);
            slot.active_origin_tag.store(0, SeqCst);
        }
        if let Some(launch) = route.audio_launch_frame {
            slot.audio_launch_frame.store(launch, SeqCst);
            slot.audio_launch_tag.store(1, SeqCst);
        } else {
            slot.audio_launch_frame.store(0, SeqCst);
            slot.audio_launch_tag.store(0, SeqCst);
        }
        if let Some(pending) = route.pending {
            let (tag, clip_id) = match pending.target {
                hex_engine::SessionRouteTarget::Arrangement => (1, 0),
                hex_engine::SessionRouteTarget::Session { clip } => (2, clip.value()),
                hex_engine::SessionRouteTarget::Silence => (3, 0),
            };
            slot.pending_target_tag.store(tag, SeqCst);
            slot.pending_clip_id.store(clip_id, SeqCst);
            slot.pending_frame.store(pending.performance_frame, SeqCst);
            slot.pending_beat_raw.store(pending.beat.raw(), SeqCst);
            slot.pending_tag.store(1, SeqCst);
        } else {
            slot.pending_tag.store(0, SeqCst);
            slot.pending_target_tag.store(0, SeqCst);
            slot.pending_clip_id.store(0, SeqCst);
            slot.pending_frame.store(0, SeqCst);
            slot.pending_beat_raw.store(0, SeqCst);
        }
    }
}

impl Status {
    pub fn rejected_action(&self) -> Option<(u64, PlaybackActionRejection)> {
        for _ in 0..3 {
            let before = self.rejected_action_sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let ticket = self.rejected_action_ticket.load(SeqCst);
            let reason = self.rejected_action_reason.load(SeqCst);
            let after = self.rejected_action_sequence.load(SeqCst);
            if before == after {
                return (ticket != 0)
                    .then(|| PlaybackActionRejection::from_code(reason))
                    .flatten()
                    .map(|reason| (ticket, reason));
            }
        }
        None
    }

    pub fn rejected_revision(&self) -> Option<(u64, PlaybackHandoffRejection)> {
        for _ in 0..3 {
            let before = self.rejected_revision_sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let revision = self.rejected_revision.load(SeqCst);
            let reason = self.rejected_revision_reason.load(SeqCst);
            let after = self.rejected_revision_sequence.load(SeqCst);
            if before == after {
                return (revision != 0)
                    .then(|| PlaybackHandoffRejection::from_code(reason))
                    .flatten()
                    .map(|reason| (revision, reason));
            }
        }
        None
    }

    pub fn sampler_console_available(&self) -> bool {
        self.sampler_console.load(Acquire)
    }
    pub fn session_capability(&self) -> Option<SessionCapabilityStatus> {
        SessionCapabilityStatus::from_code(self.session_capability.load(Acquire))
    }

    fn publish_rejected_action(&self, ticket: u64, reason: PlaybackActionRejection) {
        self.rejected_action_sequence.fetch_add(1, SeqCst);
        self.rejected_action_reason.store(reason as u32, SeqCst);
        self.rejected_action_ticket.store(ticket, SeqCst);
        self.rejected_action_sequence.fetch_add(1, SeqCst);
    }

    fn publish_rejected_revision(&self, revision: u64, reason: PlaybackHandoffRejection) {
        self.rejected_revision_sequence.fetch_add(1, SeqCst);
        self.rejected_revision_reason.store(reason as u32, SeqCst);
        self.rejected_revision.store(revision, SeqCst);
        self.rejected_revision_sequence.fetch_add(1, SeqCst);
    }
}

/// Classification of a refused ordinary graph publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishErrorKind {
    /// An original native source was unavailable during reference admission.
    /// The reason for its unavailability is not implied by this classification.
    NativeSourceUnavailable,
    /// Any other publication refusal; consult the diagnostic reason.
    Other,
}

/// Failed publication retains the candidate so the caller can retry or dispose
/// of it off the callback. Revisions advance only after successful publication.
pub struct PublishError {
    pub kind: PublishErrorKind,
    pub reason: &'static str,
    pub playback: Playback,
}

/// A failed full control-union admission retains the same unpublished graph, independently of
/// a queue/resource refusal. Neither outcome changes canonical history or acknowledges playback.
pub enum AdmittedPublishError<E> {
    Admission { reason: E, playback: Playback },
    Publication(PublishError),
}
impl<E: std::fmt::Debug> std::fmt::Debug for AdmittedPublishError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admission { reason, .. } => f.debug_tuple("Admission").field(reason).finish(),
            Self::Publication(error) => std::fmt::Debug::fmt(error, f),
        }
    }
}

impl std::fmt::Debug for PublishError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublishError")
            .field("kind", &self.kind)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Legacy unsplit control. Native worker composition consumes this exactly once
/// before delivering its nonowning client. Resident callers retain these APIs.
pub struct Control {
    diagnostics: audio_diagnostics::TraceReader,
    action_admission: Option<action_outcomes::Admission>,
    pub sample_preview: crate::sample_preview::Control,
    pub sample_recording: crate::sample_recording::Control,
    commands: Producer<QueuedCommand>,
    pub spectrum_target: spectrum_target::SpectrumTargetControl,
    pub processor_audition: processor_audition::ProcessorAuditionTargetControl,
    pub gain_recording: crate::gain_recording::Control,
    pub native_parameter_input: crate::native_parameter_input::Control,
    pub midi_recording: crate::midi_recording::Control,
    command_ticket: u64,
    publication: PublicationControl,
}
/// The unique worker-side publication/return endpoint; never a UI command client.
pub struct PublicationControl {
    foreign_parameter_delivery: Option<crate::foreign_parameter_delivery::Control>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    foreign_editor_delivery: Option<crate::foreign_parameter_delivery::editor::Control>,
    plugin_callback_quantum: Option<u16>,
    output_transport_reaper: Option<crate::plugin_worker::Reaper>,
    publications: Producer<QueuedPublication>,
    retired: Option<Consumer<ReturnedPublication>>,
    candidate_root: crate::candidate_custody::CandidateRoot,
    native_custody: bool,
    held_control_return: Option<ReturnedPublication>,
    pub status: Arc<Status>,
    rate: u32,
    revision: u64,
    command_storage_bytes: usize,
    legacy_pcm_frames: u64,
    device_resources: Option<hex_engine::native_device_resources::DeviceResourcePublication>,
    resident_forwarding: Option<Arc<resident_forwarding::ResidentForwarding>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    endpoint_probe: Option<crate::native_callback_owner::DropProbe>,
    latest_fresh_runtime_cancellation: Option<Arc<FreshRuntimeCancellation>>,
}
impl PublicationControl {
    /// Configure optional tracing on control. Default is off; old records may
    /// still be drained after disabling. The music/fault queues are independent.
    pub fn set_audio_diagnostics(&self, flags: AudioDiagnosticsFlags) {
        self.status.diagnostics_flags.store(flags.bits(), Relaxed);
    }
}
impl Control {
    /// Opt in on the original vacant constructor before callback or client
    /// exposure. Admission has 32 credits until the reader consumes outcomes;
    /// priority Stop uses its separate path. Streamed actions retain their
    /// credit through preparation; scheduled launches complete at scheduling,
    /// while activation remains a separate status. Admission may also refuse
    /// when the next ticket's slot is still occupied; consume
    /// available outcomes and retry. After shutdown, retain the reader until
    /// native retirement or retire the unsplit constructor on control.
    pub fn enable_action_outcomes_on_control(
        &mut self,
        callback: &mut Callback,
    ) -> Result<ActionOutcomeReader, &'static str> {
        if self.action_admission.is_some()
            || callback.action_writer.is_some()
            || self.command_ticket != 0
            || self.publication.revision != 0
            || self.publication.native_custody
            || self.status.split_endpoints.load(Acquire)
            || self.status.closing.load(Acquire)
            || !callback.transport_setup_open
            || !callback.matches_native_status(&self.status)
            || !callback.belongs_to_candidate_root(&self.publication.candidate_root)
        {
            return Err("action outcomes require a vacant paired constructor");
        }
        let (admission, writer, reader) =
            action_outcomes::channel(self.publication.candidate_root.root_id());
        self.action_admission = Some(admission);
        callback.action_writer = Some(writer);
        Ok(reader)
    }

    /// Consume an unsplit, nonnative pair after shutdown, when exclusive access
    /// to the callback establishes that it cannot execute concurrently.
    pub fn retire_action_outcomes_on_control(
        &mut self,
        callback: &mut Callback,
    ) -> Result<(), &'static str> {
        if !self.status.closing.load(Acquire)
            || self.publication.native_custody
            || self.status.split_endpoints.load(Acquire)
            || !callback.matches_native_status(&self.status)
            || !callback.belongs_to_candidate_root(&self.publication.candidate_root)
        {
            return Err("action outcomes require a quiescent paired constructor");
        }
        let Some(admission) = self.action_admission.take() else {
            return Err("action outcome admission unavailable");
        };
        let Some(writer) = callback.action_writer.take() else {
            self.action_admission = Some(admission);
            return Err("action outcome writer unavailable");
        };
        match writer.interrupt_after_quiescence(admission) {
            Ok(()) => Ok(()),
            Err((writer, admission)) => {
                callback.action_writer = Some(writer);
                self.action_admission = Some(admission);
                Err("action outcome endpoints do not match")
            }
        }
    }

    /// Take one non-owning Copy event; callers bound each drain.
    pub fn take_audio_diagnostic(&mut self) -> Option<AudioDiagnosticEvent> {
        self.diagnostics.pop()
    }
}

impl std::ops::Deref for Control {
    type Target = PublicationControl;
    fn deref(&self) -> &Self::Target {
        &self.publication
    }
}
impl std::ops::DerefMut for Control {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.publication
    }
}

/// Final worker-input disposition, distinct from host delivery. Cancellation
/// must replace an already copied host chunk with silence, never dispatch it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputTransportDelivery {
    Accepted,
    Cancelled,
    Refused,
}

pub struct Callback {
    diagnostics: audio_diagnostics::TraceWriter,
    action_writer: Option<action_outcomes::Writer>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    native_sources: crate::candidate_custody::NativeEndpointBank,
    plugin_callback_quantum: Option<u16>,
    output_transport: Option<Box<hex_plugin_host::transport::Callback>>,
    output_transport_cancelled: bool,
    transport_setup_open: bool,
    sample_preview: crate::sample_preview::Callback,
    sample_recording: crate::sample_recording::Callback,
    publications: Consumer<QueuedPublication>,
    retired: Producer<ReturnedPublication>,
    candidate_root: crate::candidate_custody::CandidateRoot,
    commands: Consumer<QueuedCommand>,
    status: Arc<Status>,
    audible_clock: audible_clock::AudibleClock,
    recovery_position_frozen: bool,
    recovery_seek_pending: Option<u64>,
    device_fault_stopped: bool,
    device_fault_fenced_in_block: bool,
    rate: u32,
    spectrum_target: spectrum_target::SpectrumTargetCallback,
    processor_audition: processor_audition::ProcessorAuditionTargetCallback,
    processor_audition_lease: Option<processor_audition::ProcessorAuditionTarget>,
    processor_audition_rejected_epoch: u64,
    gain_recording: crate::gain_recording::Callback,
    native_parameter_input: Box<crate::native_parameter_input::Callback>,
    foreign_parameter_delivery: crate::foreign_parameter_delivery::Callback,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    foreign_editor_delivery: crate::foreign_parameter_delivery::editor::Callback,
    midi_recording: crate::midi_recording::Callback,
    spectrum_capture: Option<SpectrumCapture>,
    spectrum_stages: SpectrumStages,
    active: Option<ActivePublication>,
    #[cfg(test)]
    test_admit_candidate: bool,
    deferred: Option<Publication>,
    deferred_eligible: Option<crate::candidate_custody::QueuedEligibleCandidate>,
    domain_pages: [Option<crate::candidate_custody::DomainEnvelope>; 2],
    stream_pending_play: Option<u64>,
    // Credits remain reserved until publication, so no more than the ledger
    // capacity can accumulate before this callback publishes and flushes them.
    stream_outcomes: [Option<(u64, action_outcomes::Outcome)>; ACTION_OUTCOME_CAPACITY],
    stream_outcome_count: usize,
    transport_fade: Option<transport_fade::PendingFade>,
    yield_stream_successor_once: bool,
    current_quantum_pins: Option<crate::candidate_custody::QuantumPins>,
    stream_prefix: u64,
    // Defensive retention if a push unexpectedly fails; never drop graph ownership.
    held_retirement: Option<ReturnedPublication>,
    terminal_disposal_pending: bool,
    published_route_key: Option<(u64, u64, Option<hex_engine::SessionClockAnchor>)>,
    #[cfg(test)]
    route_slot_visits: usize,
    #[cfg(test)]
    test_transient_handoff_refusals: usize,
    #[cfg(all(test, feature = "allocation-probe"))]
    test_eligible_transfer_allowance: Option<usize>,
    #[cfg(test)]
    test_after_fresh_fence_validation: Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    test_before_fresh_runtime_apply: Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    test_after_fresh_runtime_apply_decision: Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    test_after_spectrum_target_boundary: Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    test_before_action_outcome_status: Option<std::sync::Arc<std::sync::Barrier>>,
}

pub(crate) fn native_endpoint_bank_storage_quote() -> Option<usize> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        crate::candidate_custody::NativeEndpointBank::data_quote()
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        Some(0)
    }
}

/// Allocate channels/telemetry on the control side. Initial graph must be
/// published before playback; an empty provider emits silence.
pub fn channel(rate: u32) -> Result<(Control, Callback), Invalid> {
    channel_with_spectrum(rate, None)
}

/// Construct the prepared callback with an optional prebuilt bounded spectrum
/// producer. The worker and queue are created off the audio thread.
pub fn channel_with_spectrum(
    rate: u32,
    capture: Option<SpectrumCapture>,
) -> Result<(Control, Callback), Invalid> {
    channel_with_interface(rate, capture, None)
}

pub(crate) fn channel_with_interface(
    rate: u32,
    capture: Option<SpectrumCapture>,
    interface_sample_rate: Option<u32>,
) -> Result<(Control, Callback), Invalid> {
    if !(8_000..=192_000).contains(&rate) {
        return Err(Invalid("unsupported playback sample rate"));
    }
    let (mut sample_preview, callback_preview) = crate::sample_preview::channel(rate);
    let (sample_recording, callback_recording) = crate::sample_recording::channel(rate);
    let (publications, incoming) = RingBuffer::new(1);
    let (returning, retired) = RingBuffer::new(RETIRED);
    let (commands, events) = RingBuffer::new(COMMANDS);
    let (trace_writer, trace_reader) = audio_diagnostics::channel();
    let mut status = Status::try_new()?;
    status.interface_sample_rate = interface_sample_rate;
    let status = Arc::new(status);
    let (spectrum_target, callback_target) = spectrum_target::channel();
    let (processor_audition, callback_audition) = processor_audition::channel();
    let (mut gain_recording, callback_gain_recording) = crate::gain_recording::channel();
    let (mut native_parameter_input, callback_native_parameter_input) =
        crate::native_parameter_input::channel();
    let (foreign_parameter_delivery, callback_foreign_parameter_delivery) =
        crate::foreign_parameter_delivery::channel();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let (foreign_editor_delivery, callback_foreign_editor_delivery) =
        crate::foreign_parameter_delivery::editor::channel();
    let (mut midi_recording, callback_midi_recording) = crate::midi_recording::channel();
    sample_preview.bind_status(status.clone());
    gain_recording.bind_status(status.clone());
    native_parameter_input.bind_status(status.clone());
    midi_recording.bind_status(status.clone());
    let candidate_root = crate::candidate_custody::CandidateRoot::new(RETIRED).map_err(Invalid)?;
    let command_storage_bytes = commands
        .buffer()
        .capacity()
        .checked_mul(std::mem::size_of::<QueuedCommand>())
        .and_then(|n| n.checked_add(std::mem::size_of_val(commands.buffer())))
        .and_then(|n| n.checked_add(trace_writer.storage_bytes()?))
        .ok_or(Invalid("command queue storage overflow"))?;
    Ok((
        Control {
            diagnostics: trace_reader,
            action_admission: None,
            sample_preview,
            sample_recording,
            commands,
            spectrum_target,
            processor_audition,
            gain_recording,
            native_parameter_input,
            midi_recording,
            command_ticket: 0,
            publication: PublicationControl {
                foreign_parameter_delivery: Some(foreign_parameter_delivery),
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                foreign_editor_delivery: Some(foreign_editor_delivery),
                plugin_callback_quantum: None,
                output_transport_reaper: None,
                candidate_root: candidate_root.clone(),
                native_custody: false,
                held_control_return: None,
                publications,
                retired: Some(retired),
                status: status.clone(),
                rate,
                revision: 0,
                command_storage_bytes,
                legacy_pcm_frames: 0,
                device_resources: None,
                resident_forwarding: None,
                #[cfg(all(test, feature = "allocation-probe"))]
                endpoint_probe: None,
                latest_fresh_runtime_cancellation: None,
            },
        },
        Callback {
            diagnostics: trace_writer,
            action_writer: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_sources: crate::candidate_custody::NativeEndpointBank::new().map_err(Invalid)?,
            plugin_callback_quantum: None,
            output_transport: None,
            output_transport_cancelled: false,
            transport_setup_open: true,
            sample_preview: callback_preview,
            sample_recording: callback_recording,
            candidate_root,
            publications: incoming,
            retired: returning,
            commands: events,
            status,
            audible_clock: audible_clock::AudibleClock::new(),
            recovery_position_frozen: false,
            recovery_seek_pending: None,
            device_fault_stopped: false,
            device_fault_fenced_in_block: false,
            rate,
            spectrum_target: callback_target,
            processor_audition: callback_audition,
            processor_audition_lease: None,
            processor_audition_rejected_epoch: 0,
            gain_recording: callback_gain_recording,
            native_parameter_input: callback_native_parameter_input,
            foreign_parameter_delivery: callback_foreign_parameter_delivery,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            foreign_editor_delivery: callback_foreign_editor_delivery,
            midi_recording: callback_midi_recording,
            spectrum_capture: capture,
            spectrum_stages: SpectrumStages::new()?,
            active: None,
            #[cfg(test)]
            test_admit_candidate: false,
            deferred: None,
            deferred_eligible: None,
            domain_pages: std::array::from_fn(|_| None),
            stream_pending_play: None,
            stream_outcomes: [None; ACTION_OUTCOME_CAPACITY],
            stream_outcome_count: 0,
            transport_fade: None,
            yield_stream_successor_once: false,
            current_quantum_pins: None,
            stream_prefix: 0,
            held_retirement: None,
            terminal_disposal_pending: false,
            published_route_key: None,
            #[cfg(test)]
            route_slot_visits: 0,
            #[cfg(test)]
            test_transient_handoff_refusals: 0,
            #[cfg(all(test, feature = "allocation-probe"))]
            test_eligible_transfer_allowance: None,
            #[cfg(test)]
            test_after_fresh_fence_validation: None,
            #[cfg(test)]
            test_before_fresh_runtime_apply: None,
            #[cfg(test)]
            test_after_fresh_runtime_apply_decision: None,
            #[cfg(test)]
            test_after_spectrum_target_boundary: None,
            #[cfg(test)]
            test_before_action_outcome_status: None,
        },
    ))
}

fn capture_runtime_fence(status: &Status) -> Option<FreshRuntimeFence> {
    let consumed_stop_generation = status.consumed_stop_generation.load(Acquire);
    if consumed_stop_generation & 1 != 0
        || status.pending_stop_marker.load(Acquire) != 0
        || status.render_fault.load(Acquire)
        || status.device_fault.load(Acquire)
    {
        return None;
    }
    let output_fault_epoch = status.last_output_fault_epoch.load(Acquire);
    let mut routes = [SessionRouteEntry::default(); SESSION_ROUTE_CAPACITY];
    let snapshot = status.session_routes(&mut routes)?;
    if status.pending_stop_marker.load(Acquire) != 0
        || status.consumed_stop_generation.load(Acquire) != consumed_stop_generation
        || status.render_fault.load(Acquire)
        || status.device_fault.load(Acquire)
        || status.last_output_fault_epoch.load(Acquire) != output_fault_epoch
    {
        return None;
    }
    Some(FreshRuntimeFence {
        expected_active_revision: snapshot.audio_revision,
        route_state_generation: snapshot.route_state_generation,
        admitted_command_ticket: status.admitted_ticket.load(Acquire),
        output_fault_epoch,
        consumed_stop_generation,
    })
}

#[allow(clippy::result_large_err)] // Refusals return the actual graph inline without allocating on failure.
impl PublicationControl {
    /// Correlation for nonowning client/view metadata, never custody authority.
    pub fn output_identity(&self) -> Option<u64> {
        Some(self.candidate_root.root_id())
    }
    /// Immutable VST3 transport quantum in project frames. Same-rate output
    /// requests this many hardware frames; a converting backend verifies its
    /// maximum client slice against this quantum before starting. The callback
    /// still validates its actual extent, including after a route change.
    pub fn plugin_callback_quantum_on_control(&self) -> Option<u16> {
        self.plugin_callback_quantum
    }
    /// Bind only while both exact constructor endpoints remain unexposed.
    /// The backend or headless fixture owns the corresponding delivery bound.
    pub(crate) fn configure_plugin_callback_quantum(
        &mut self,
        callback: &mut Callback,
        frames: u16,
    ) -> Result<(), Invalid> {
        if frames == 0
            || usize::from(frames) > hex_plugin_host::transport::MAX_FRAMES
            || self.plugin_callback_quantum.is_some()
            || callback.plugin_callback_quantum.is_some()
            || self.revision != 0
            || self.native_custody
            || self.status.client_exposed.load(Acquire)
            || self.status.closing.load(Acquire)
            || !callback.transport_setup_open
            || !callback.belongs_to_candidate_root(&self.candidate_root)
            || !callback.matches_native_status(&self.status)
        {
            return Err(Invalid(
                "plugin callback quantum requires its vacant constructor",
            ));
        }
        self.plugin_callback_quantum = Some(frames);
        callback.plugin_callback_quantum = Some(frames);
        Ok(())
    }
    /// Capture the current route header under its sequence lock, then acquire
    /// the admitted-command high-water mark. Returns `None` on a contended or
    /// invalid route snapshot so callers can retry before preparing a graph.
    pub fn capture_fresh_runtime_fence(&self) -> Option<FreshRuntimeFence> {
        capture_runtime_fence(&self.status)
    }

    /// Pair a running Session observation with the matching command/source
    /// fence for conditional control-side preparation. A command batch can
    /// make the observation lag until a later callback publication. The pair
    /// can become stale immediately and does not authorize installation; the
    /// callback must recheck the fence, exact graph and predicted clock. A
    /// zero control generation is conservatively refused, even if explicit.
    pub fn capture_running_transport_fence(
        &self,
    ) -> Option<(TransportObservation, FreshRuntimeFence)> {
        let fence = capture_runtime_fence(&self.status)?;
        let observation = self.status.transport_observation()?;
        (observation.playing
            && observation.loop_state.is_some()
            && observation.control_generation != 0
            && observation.audio_revision != 0
            && observation.audio_revision == fence.expected_active_revision
            && observation.consumed_command_ticket == fence.admitted_command_ticket
            && self.status.admitted_ticket.load(Acquire) == fence.admitted_command_ticket
            && self.status.consumed_command.load(Acquire) == fence.admitted_command_ticket
            && self.status.last_output_fault_epoch.load(Acquire) == fence.output_fault_epoch
            && self.status.consumed_stop_generation.load(Acquire) == fence.consumed_stop_generation
            && !self.status.render_fault.load(Acquire)
            && !self.status.device_fault.load(Acquire)
            && self.status.pending_stop_marker.load(Acquire) == 0
            && !self.status.closing.load(Acquire))
        .then_some((observation, fence))
    }

    /// Prepare a conditional future anchor from this output's observed running
    /// Session transport. `None` means the observation is unavailable or does
    /// not match the requested source revision and control generation. The
    /// caller must retain the real revisions independently and account for and
    /// reclaim the returned token on control. This is not installation authority:
    /// the callback must recheck the exact runtime graphs, clocks and host fence.
    pub fn prepare_fenced_tempo_anchor(
        &self,
        expected_audio_revision: u64,
        expected_control_generation: u64,
        previous: Arc<hex_engine::PreparedRevision>,
        candidate: Arc<hex_engine::PreparedRevision>,
        cut_performance_frame: u64,
    ) -> Result<
        Option<(
            hex_engine::session_playback::PreparedTempoAnchor,
            FreshRuntimeFence,
        )>,
        hex_engine::session_playback::SessionPlaybackError,
    > {
        let Some((observation, fence)) = self.capture_running_transport_fence() else {
            return Ok(None);
        };
        if observation.audio_revision != expected_audio_revision
            || observation.control_generation != expected_control_generation
        {
            return Ok(None);
        }
        let Some(loop_state) = observation.loop_state else {
            return Ok(None);
        };
        let anchor =
            hex_engine::session_playback::PreparedTempoAnchor::prepare_from_running_observation(
                previous,
                candidate,
                observation.arrangement_frame,
                observation.performance_frame,
                loop_state,
                cut_performance_frame,
            )?;
        if !self
            .capture_running_transport_fence()
            .is_some_and(|(latest, current)| {
                current == fence
                    && latest.control_generation == expected_control_generation
                    && latest.performance_frame <= cut_performance_frame
            })
        {
            return Ok(None);
        }
        Ok(Some((anchor, fence)))
    }

    /// Reserve the ordinary registered-resource subtree before any publication/client exposure.
    /// Configure this capability even for an initially resource-free project so a later sampler
    /// does not reopen the output. Limits grant no custody or complete graph provenance. Legacy
    /// clip PCM uses exact control-owned backing records shared across publications. Each record
    /// releases after its matching graph is destroyed on control. Runtime leases may migrate into tails and
    /// retain their source obligations across returns of the publication which introduced them.
    /// Graph publication for this entire output is permanently worker-owned. Client transport,
    /// Stop and observation remain available; its legacy raw graph-forwarding bridge refuses.
    /// Consumers must route every canonical graph edit, including the resource-free initial graph,
    /// through the worker sink before enabling this capability, as the native resident adapter does.
    pub fn configure_device_resource_publication_on_control(
        &mut self,
        reservation: hex_engine::AudioStorageLedgerReservation,
    ) -> Result<(), &'static str> {
        self.configure_device_resource_publication(reservation, false)
    }

    /// Reserve both resource books on a fresh native output. The verified
    /// constructor owns the shared source root and domain reservation; the
    /// ordinary book is separate and does not include that root. `combined`
    /// must cover both declared limits, with checked arithmetic. These are
    /// caller limits, not an RSS or allocator bound. The combined ceiling
    /// includes one ordinary demand workspace for every permitted graph.
    /// This grants no ordinary publication or cross-kind switching authority;
    /// the exposed client retains its verified publication policy.
    /// If construction fails after configuring the
    /// verified root, the output is closed and must be discarded rather than
    /// exposed.
    pub fn configure_combined_publication_on_control(
        &mut self,
        verified: crate::AudioCandidatePublicationReservation,
        ordinary: hex_engine::AudioStorageLedgerReservation,
        ordinary_extents: usize,
        ordinary_metadata_bytes: usize,
        combined: hex_engine::AudioStorageLedgerReservation,
    ) -> Result<(), &'static str> {
        let fits =
            |a: usize, b: usize, limit: usize| a.checked_add(b).is_some_and(|sum| sum <= limit);
        let graph_count = self
            .publications
            .buffer()
            .capacity()
            .checked_add(RETIRED)
            .and_then(|count| count.checked_add(5))
            .ok_or("combined audio reservation exceeded")?;
        let workspace_bytes = ordinary_metadata_bytes
            .checked_mul(graph_count)
            .ok_or("combined audio reservation exceeded")?;
        // One boxed ordinary stream owner plus four extent vectors per graph.
        let workspace_rows = graph_count
            .checked_mul(5)
            .ok_or("combined audio reservation exceeded")?;
        let ordinary_data_rows = ordinary
            .data_rows
            .checked_add(workspace_rows)
            .ok_or("combined audio reservation exceeded")?;
        let ordinary_retained = ordinary
            .max_retained_bytes
            .checked_add(workspace_bytes)
            .ok_or("combined audio reservation exceeded")?;
        let ordinary_peak = ordinary
            .max_construction_peak_bytes
            .checked_add(workspace_bytes)
            .ok_or("combined audio reservation exceeded")?;
        if combined.max_construction_peak_bytes < combined.max_retained_bytes
            || !fits(verified.data_rows, ordinary_data_rows, combined.data_rows)
            || !fits(
                verified.handle_rows,
                ordinary.handle_rows,
                combined.handle_rows,
            )
            || !fits(1, ordinary.root_rows, combined.root_rows)
            || !fits(
                verified.max_retained_data_bytes,
                ordinary_retained,
                combined.max_retained_bytes,
            )
            || !fits(
                verified.max_construction_peak_data_bytes,
                ordinary_peak,
                combined.max_construction_peak_bytes,
            )
        {
            return Err("combined audio reservation exceeded");
        }
        if let Some(bridge) = self.resident_forwarding.as_ref() {
            bridge.begin_combined_configuration(&self.status)?;
        } else {
            return Err("combined reservation requires paired native worker endpoints");
        }
        let result = match self.configure_audio_candidate_publication(verified) {
            Err(error) => Err(error),
            Ok(()) => match self
                .configure_device_resource_publication(ordinary, true)
                .and_then(|()| {
                    self.candidate_root.configure_combined_ordinary_streams(
                        ordinary_extents,
                        ordinary_metadata_bytes,
                    )
                }) {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.request_shutdown();
                    Err(error)
                }
            },
        };
        if let Some(bridge) = self.resident_forwarding.as_ref() {
            bridge.finish_combined_configuration();
        }
        result
    }

    fn configure_device_resource_publication(
        &mut self,
        reservation: hex_engine::AudioStorageLedgerReservation,
        shared_verified_root: bool,
    ) -> Result<(), &'static str> {
        if self.device_resources.is_some()
            || self.revision != 0
            || self.status.client_exposed.load(Acquire)
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || (self.candidate_root.domains_configured() && !shared_verified_root)
            || (shared_verified_root && !self.status.constructor_mode.load(Acquire))
        {
            return Err("device resource publication requires a vacant unexposed ordinary output");
        }
        // One queue, active, deferred, callback-held, control-held and construction owner, plus
        // the existing retirement queue. Each Playback's track/tail/keyboard bound is unchanged.
        let max_graphs = self
            .publications
            .buffer()
            .capacity()
            .checked_add(RETIRED)
            .and_then(|n| n.checked_add(5))
            .ok_or("device resource publication capacity overflow")?;
        let max_obligations = max_graphs
            .checked_mul(hex_engine::native_device_resources::MAX_PLAYBACK_DEVICE_RESOURCE_LEASES)
            .ok_or("device resource publication capacity overflow")?;
        let resources =
            hex_engine::native_device_resources::DeviceResourcePublication::reserve_on_control(
                reservation,
                max_obligations,
                max_graphs,
            )
            .map_err(|_| "device resource publication reservation refused")?;
        let bridge = self
            .resident_forwarding
            .as_ref()
            .map(Arc::clone)
            .unwrap_or_else(resident_forwarding::ResidentForwarding::new);
        if !shared_verified_root {
            bridge.claim_worker_publication_on_control(&self.status)?;
        }
        self.resident_forwarding = Some(bridge);
        self.device_resources = Some(resources);
        Ok(())
    }

    /// Synchronous worker-side inventory of the active resource book. Select this actual root
    /// once in a control union; a prospective publication view already includes the same book.
    pub fn device_resource_admission_on_control(
        &self,
    ) -> Option<hex_engine::native_device_resources::DeviceResourceAdmission<'_>> {
        self.device_resources
            .as_ref()
            .map(|book| book.admission_on_control())
    }

    /// Freeze all graph publication and move the actual resource book into the caller's one
    /// retirement hold before dropping its output. Actual runtimes still own their leases.
    /// Retain this book and the separate legacy allowance until a fresh output acquires the
    /// existing exclusive native lease; endpoint destruction alone does not prove that frontier.
    pub fn take_device_resource_publication_on_shutdown(
        &mut self,
    ) -> Option<hex_engine::native_device_resources::DeviceResourcePublication> {
        self.request_shutdown();
        self.reclaim();
        self.device_resources.take()
    }

    /// Admit the actual current/prospective resource union, then enqueue as the final fallible
    /// prepared-transition step. The caller must reserve canonical commit storage beforehand and
    /// must not report cancellation/rollback after success. Submitted is not callback-applied.
    pub fn publish_with_admission_on_control<E>(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
        admit: impl FnOnce(
            hex_engine::native_device_resources::DeviceResourceAdmission<'_>,
        ) -> Result<(), E>,
    ) -> Result<u64, AdmittedPublishError<E>> {
        self.publish_admitted_with_source(
            playback,
            policy,
            control_context_generation,
            None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            None,
            admit,
        )
    }
    /// Tag the actual admitted ordinary graph with its captured canonical source.
    /// Existing generation-only methods remain unknown to MIDI recording.
    pub fn publish_with_admission_and_source_on_control<E>(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        source_context: hex_engine::AudioSourceContext,
        admit: impl FnOnce(
            hex_engine::native_device_resources::DeviceResourceAdmission<'_>,
        ) -> Result<(), E>,
    ) -> Result<u64, AdmittedPublishError<E>> {
        self.publish_admitted_with_source(
            playback,
            policy,
            source_context.project_generation,
            Some(source_context),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            None,
            admit,
        )
    }
    fn publish_admitted_with_source<E>(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
        source_context: Option<hex_engine::AudioSourceContext>,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))] native_continuation: Option<
            hex_plugin_host::transport::ParameterInputSource,
        >,
        admit: impl FnOnce(
            hex_engine::native_device_resources::DeviceResourceAdmission<'_>,
        ) -> Result<(), E>,
    ) -> Result<u64, AdmittedPublishError<E>> {
        let Some(mut resources) = self.device_resources.take() else {
            return Err(AdmittedPublishError::Publication(PublishError {
                kind: PublishErrorKind::Other,
                reason: "ordinary worker publication is not configured",
                playback,
            }));
        };
        let legacy_pcm = playback
            .resident_pcm_capacity_frames()
            .and_then(|frames| self.legacy_pcm_frames.checked_add(frames));
        let result = if legacy_pcm.is_some() {
            resources.publish_admitted_on_control(playback, |playback, view| {
                if let Err(reason) = admit(view) {
                    return Err(AdmittedPublishError::Admission { reason, playback });
                }
                self.publish_unaccounted_with_native_continuation(
                    playback,
                    policy,
                    control_context_generation,
                    true,
                    false,
                    source_context,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation,
                )
                .map_err(AdmittedPublishError::Publication)
            }).map_err(|error| match error {
                hex_engine::native_device_resources::DeviceResourcePublishError::Resources { playback, .. } =>
                    AdmittedPublishError::Publication(PublishError { kind: PublishErrorKind::Other, reason: "device resource publication storage reservation exceeded", playback }),
                hex_engine::native_device_resources::DeviceResourcePublishError::Publication(error) => error,
            })
        } else {
            Err(AdmittedPublishError::Publication(PublishError {
                kind: PublishErrorKind::Other,
                reason: "resident pcm obligation unavailable or overflowing",
                playback,
            }))
        };
        self.device_resources = Some(resources);
        result
    }

    /// Actual accepted ordinary graph owners still requiring control destruction. Resource-free
    /// graphs count too; a migrated original-source lease can outlive its introducing graph.
    pub fn unreturned_resident_graphs_on_control(&self) -> Option<usize> {
        self.device_resources
            .as_ref()
            .map(|book| book.unreturned_graphs_on_control())
    }
    pub fn outstanding_device_resource_leases(&self) -> usize {
        self.device_resources
            .as_ref()
            .map_or(0, |resources| resources.outstanding_leases())
    }

    pub fn publish(&mut self, playback: Playback) -> Result<u64, PublishError> {
        self.publish_with_policy(playback, PublicationPolicy::InheritRuntime)
    }

    pub fn publish_with_policy(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
    ) -> Result<u64, PublishError> {
        self.publish_with_pcm_obligation(playback, policy, 0, false, false)
    }

    /// Publish a prepared graph tagged with the control-side project context
    /// that produced it. Legacy `publish`/`publish_with_policy` retain unknown context,
    /// even though their scalar generation remains zero for existing telemetry.
    pub fn publish_with_policy_and_context_generation(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
    ) -> Result<u64, PublishError> {
        self.publish_with_pcm_obligation(playback, policy, control_context_generation, true, false)
    }
    fn publish_with_pcm_obligation(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
        explicit_control_context: bool,
        forwarded_pcm: bool,
    ) -> Result<u64, PublishError> {
        let Some(mut resources) = self.device_resources.take() else {
            if playback.contains_device_resource_leases()
                || playback.has_foreign_continuation_states()
            {
                return Err(PublishError {
                    kind: PublishErrorKind::Other,
                    reason: "device resource publication was not reserved for this output",
                    playback,
                });
            }
            return self.publish_unaccounted_pcm_obligation(
                playback,
                policy,
                control_context_generation,
                explicit_control_context,
                forwarded_pcm,
                None,
            );
        };
        if forwarded_pcm {
            self.device_resources = Some(resources);
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "graph publication belongs to the audio worker for this output",
                playback,
            });
        }
        let legacy_pcm = playback
            .resident_pcm_capacity_frames()
            .and_then(|frames| self.legacy_pcm_frames.checked_add(frames));
        let result = if legacy_pcm.is_some() {
            resources.publish_on_control(playback, |playback| {
                self.publish_unaccounted_pcm_obligation(playback, policy, control_context_generation, explicit_control_context, forwarded_pcm, None)
            }).map_err(|error| match error {
                hex_engine::native_device_resources::DeviceResourcePublishError::Resources { playback, .. } => PublishError { kind: PublishErrorKind::Other,
                    reason: "device resource publication storage reservation exceeded", playback,
                },
                hex_engine::native_device_resources::DeviceResourcePublishError::Publication(error) => error,
            })
        } else {
            Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "resident pcm obligation unavailable or overflowing",
                playback,
            })
        };
        self.device_resources = Some(resources);
        result
    }
    fn publish_unaccounted_pcm_obligation(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
        explicit_control_context: bool,
        forwarded_pcm: bool,
        source_context: Option<hex_engine::AudioSourceContext>,
    ) -> Result<u64, PublishError> {
        self.publish_unaccounted_with_native_continuation(
            playback,
            policy,
            control_context_generation,
            explicit_control_context,
            forwarded_pcm,
            source_context,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // Existing ownership plus the exact move-only source observation.
    fn publish_unaccounted_with_native_continuation(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
        explicit_control_context: bool,
        forwarded_pcm: bool,
        source_context: Option<hex_engine::AudioSourceContext>,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))] native_continuation: Option<
            hex_plugin_host::transport::ParameterInputSource,
        >,
    ) -> Result<u64, PublishError> {
        let native_epoch = match policy {
            PublicationPolicy::InstallUnderNativeEpoch(token)
            | PublicationPolicy::InheritUnderStoppedNative { fence: token, .. } => Some(token),
            _ => None,
        };
        let native_inheritance = match policy {
            PublicationPolicy::InheritUnderStoppedNative { source, fence } => Some((source, fence)),
            _ => None,
        };
        let reason = if native_inheritance.is_some_and(|(source, fence)| {
            !self
                .status
                .stopped_native_source_fence_matches(source, fence)
                || self.status.applied_audio_source().is_some()
                || source_context.is_none_or(|context| {
                    context.project_generation != source.context.project_generation
                })
        }) {
            Some("native inheritance requires its exact stopped active source")
        } else if native_inheritance.is_none()
            && native_epoch.is_some_and(|token| {
                self.status
                    .sampler_edit
                    .native_epoch_context(token)
                    .is_none()
                    || self.status.sampler_edit.native_epoch_context(token) != source_context
                    || self.status.native_effect_source().is_some()
            })
        {
            Some("native successor requires its matching stopped returned source")
        } else if self.status.sampler_edit.native_epoch_active() && native_epoch.is_none() {
            Some("native epoch excludes unrelated graph publication")
        } else if self.status.sampler_edit.active()
            && !matches!(policy, PublicationPolicy::InstallUnderStoppedArrangement(token) | PublicationPolicy::InheritUnderArrangement(token) | PublicationPolicy::InstallUnderNativeEpoch(token) | PublicationPolicy::InheritUnderStoppedNative { fence: token, .. }
                if self.stopped_edit_fence_state(token) == StoppedEditFenceState::Ready)
        {
            Some("stopped edit excludes unrelated graph publication")
        } else if matches!(policy, PublicationPolicy::InstallUnderStoppedArrangement(token) | PublicationPolicy::InheritUnderArrangement(token) | PublicationPolicy::InstallUnderNativeEpoch(token) | PublicationPolicy::InheritUnderStoppedNative { fence: token, .. }
            if self.stopped_edit_fence_state(token) != StoppedEditFenceState::Ready)
        {
            Some("arrangement stopped fence is no longer ready")
        } else if self.status.constructor_mode.load(Acquire)
            || (self.candidate_root.domains_configured()
                && !self.candidate_root.ordinary_streams_configured())
        {
            Some("constructor publication does not adopt legacy playback")
        } else if self.status.publication_pending.recording_active() {
            Some("finish recording before replacing playback")
        } else if self.status.closing.load(Acquire) {
            Some("playback is shutting down")
        } else if playback.sample_rate() != self.rate {
            Some("playback rate differs from device")
        } else if playback.contains_streamed_sources()
            && (!self.candidate_root.ordinary_streams_configured() || source_context.is_none())
        {
            Some("streamed playback requires a native media page endpoint")
        } else if self.revision == u64::MAX {
            Some("playback revision exhausted")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason,
                playback,
            });
        }
        let Some(pcm_frames) = playback.resident_pcm_capacity_frames() else {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "resident pcm capacity overflow",
                playback,
            });
        };
        let Some(next_pcm) =
            self.legacy_pcm_frames
                .checked_add(if forwarded_pcm { 0 } else { pcm_frames })
        else {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "resident pcm obligation overflow",
                playback,
            });
        };
        let reserved = if native_epoch.is_some() {
            self.status
                .publication_pending
                .reserve_native_epoch_publication()
        } else {
            self.status.publication_pending.reserve_publication()
        };
        if !reserved {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "playback publication already pending",
                playback,
            });
        }
        let revision = self.revision + 1;
        let cancellation = policy.fresh_fence().is_some().then(|| {
            Arc::new(FreshRuntimeCancellation {
                revision,
                decision: AtomicU32::new(FRESH_PUBLICATION_PENDING),
            })
        });
        let mut playback = playback;
        playback.set_device_metering(true);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let native_references = match self
            .candidate_root
            .retain_ordinary_native_sources(&playback)
        {
            Ok(Some(_)) if source_context.is_none() => {
                self.status.publication_pending.store(false, Release);
                return Err(PublishError {
                    kind: PublishErrorKind::Other,
                    reason: "native graph publication requires a canonical source context",
                    playback,
                });
            }
            Ok(references) => references,
            Err(failure) => {
                self.status.publication_pending.store(false, Release);
                return Err(PublishError {
                    kind: if failure.is_unavailable() {
                        PublishErrorKind::NativeSourceUnavailable
                    } else {
                        PublishErrorKind::Other
                    },
                    reason: failure.message(),
                    playback,
                });
            }
        };
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if native_continuation.is_some() && native_references.is_none() {
            self.status.publication_pending.store(false, Release);
            return Err(PublishError {
                kind: PublishErrorKind::NativeSourceUnavailable,
                reason: "native continuation requires its original graph source",
                playback,
            });
        }
        let streams = if self.candidate_root.ordinary_streams_configured() {
            let Some(context) = source_context else {
                self.status.publication_pending.store(false, Release);
                return Err(PublishError {
                    kind: PublishErrorKind::Other,
                    reason: "ordinary stream context unavailable",
                    playback,
                });
            };
            match self
                .candidate_root
                .prepare_ordinary_streams(&playback, revision, context)
            {
                Ok(streams) => Some(streams),
                Err(reason) => {
                    self.status.publication_pending.store(false, Release);
                    return Err(PublishError {
                        kind: PublishErrorKind::Other,
                        reason,
                        playback,
                    });
                }
            }
        } else {
            None
        };
        let publication = Publication {
            pcm_frames,
            forwarded_pcm,
            revision,
            control_context_generation,
            explicit_control_context,
            source_context,
            policy,
            fresh_runtime_cancellation: cancellation.as_ref().map(Arc::clone),
            quantized_seek_frame: None,
            streams,
            playback: Box::new(playback),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_continuation,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_references,
        };
        if let Err(mut publication) = enqueue_legacy(&mut self.publications, publication) {
            publication.release_streams_on_control(&self.candidate_root);
            self.status.publication_pending.store(false, Release);
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "playback publication queue full",
                playback: *publication.playback,
            });
        }
        self.legacy_pcm_frames = next_pcm;
        self.revision = revision;
        self.status.submitted_revision.store(revision, Release);
        self.latest_fresh_runtime_cancellation = cancellation;
        self.update_resident_cancellation();
        Ok(revision)
    }

    /// Cancel a conditional fresh-runtime publication until the callback's
    /// final Pending-to-Applying CAS. A successful result means the candidate
    /// will be returned through the normal retirement queue without becoming
    /// active. Once Applying wins, callers must wait for its exact terminal
    /// revision and publish any successor from the resulting canonical state.
    pub fn cancel_fresh_runtime_publication(
        &self,
        revision: u64,
    ) -> FreshRuntimeCancellationResult {
        let Some(cancellation) = self.latest_fresh_runtime_cancellation.as_ref() else {
            return FreshRuntimeCancellationResult::UnknownRevision;
        };
        if cancellation.revision != revision {
            return FreshRuntimeCancellationResult::UnknownRevision;
        }
        cancellation.request_cancel()
    }
    /// Bounded reclamation on the control thread, including final PCM drops.
    pub fn reclaim(&mut self) -> usize {
        self.status.retire_stopped_edit_on_control();
        let mut count = 0;
        for _ in 0..RETIRED {
            let publication = self
                .held_control_return
                .take()
                .or_else(|| self.retired.as_mut().and_then(|r| r.pop().ok()));
            let Some(publication) = publication else {
                break;
            };
            match publication {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                ReturnedPublication::Native(endpoint) => {
                    if let Err(endpoint) = self.candidate_root.ingest_native_endpoint(endpoint) {
                        self.held_control_return = Some(ReturnedPublication::Native(endpoint));
                        break;
                    }
                }
                ReturnedPublication::Domain(pages) => {
                    if let Err(pages) = self.candidate_root.ingest_pages(pages) {
                        self.held_control_return = Some(ReturnedPublication::Domain(pages));
                        break;
                    }
                }
                ReturnedPublication::Eligible(value) => {
                    if let Err(value) = self.candidate_root.ingest_eligible(value) {
                        self.held_control_return = Some(ReturnedPublication::Eligible(value));
                        break;
                    }
                }
                ReturnedPublication::Legacy(mut publication) => {
                    publication.release_streams_on_control(&self.candidate_root);
                    let frames = publication.pcm_frames;
                    let forwarded = publication.forwarded_pcm;
                    if let Some(resources) = self.device_resources.as_mut() {
                        if let Err(playback) = resources.retire_on_control(publication.playback) {
                            publication.playback = playback;
                            self.held_control_return =
                                Some(ReturnedPublication::Legacy(publication));
                            break;
                        }
                    } else {
                        drop(publication.playback);
                    } // actual owner destruction precedes obligation release
                    if forwarded {
                        if let Some(bridge) = &self.resident_forwarding {
                            bridge.release_pcm_after_drop(frames);
                        }
                    } else {
                        self.legacy_pcm_frames = self
                            .legacy_pcm_frames
                            .checked_sub(frames)
                            .unwrap_or(u64::MAX);
                    }
                }
                ReturnedPublication::Candidate(returned) => {
                    if let Err(returned) = self.candidate_root.ingest(returned) {
                        self.held_control_return = Some(ReturnedPublication::Candidate(returned));
                        break;
                    }
                }
            }
            count += 1;
        }
        crate::lifecycle::request_candidate_cleanup(&self.candidate_root);
        count
    }
    pub(crate) fn enable_native_custody(&mut self) -> Result<(), String> {
        crate::lifecycle::prepare_candidate_cleanup()?;
        self.native_custody = true;
        Ok(())
    }
    pub(crate) fn native_candidate_root(&self) -> crate::candidate_custody::CandidateRoot {
        self.candidate_root.clone()
    }
    /// Opt in only on a vacant native channel. This reserves retained domain
    /// DATA, not native installation, page readiness or complete scalar costs.
    pub fn configure_audio_candidate_domains(
        &mut self,
        reservation: crate::AudioCandidateDomainReservation,
    ) -> Result<(), &'static str> {
        if self.device_resources.is_some()
            || self.status.client_exposed.load(Acquire)
            || !self.native_custody
            || self.revision != 0
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || self.held_control_return.is_some()
            || self.latest_fresh_runtime_cancellation.is_some()
        {
            return Err("constructor publication requires a vacant native channel");
        }
        self.candidate_root.configure_domains(reservation)
    }
    /// Retains one actual factory pair in a fixed Root cell. Successful custody
    /// may already be closing; the token does not promise publication/readiness.
    pub fn register_audio_candidate_domain(
        &mut self,
        domain: crate::CandidateDomain,
    ) -> Result<crate::AudioCandidateDomainToken, Box<crate::AudioCandidateDomainRefusal>> {
        self.candidate_root.register_domain(domain)
    }
    pub fn retire_audio_candidate_domain(
        &mut self,
        token: crate::AudioCandidateDomainToken,
    ) -> bool {
        self.candidate_root.retire_domain(token)
    }
    /// Moves an already READY scalar report; never joins or destroys endpoints.
    pub fn recover_audio_candidate_domain(
        &mut self,
        token: crate::AudioCandidateDomainToken,
    ) -> Option<hex_stream::PagePoolReport> {
        self.candidate_root.recover_domain(token)
    }
    /// Nonblocking observation. None includes temporary lifecycle contention;
    /// it does not establish zero occupancy or release any reservation.
    pub fn audio_candidate_domain_usage(&self) -> Option<crate::AudioCandidateDomainUsage> {
        self.candidate_root.domain_usage()
    }
    /// Accept disposal-only custody. Queue acceptance is not installation or a
    /// complete-cost claim; the callback currently refuses this path before use.
    pub fn stage_audio_candidate(
        &mut self,
        owner: hex_engine::AudioCandidateOwner,
        domains: Vec<crate::CandidateDomain>,
    ) -> Result<crate::AudioCandidateTicket, crate::AudioCandidateStageRefusal> {
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || owner.sample_rate() != self.rate
            || owner.phase() != hex_engine::AudioCandidateDisposalPhase::Owned
        {
            return Err(crate::AudioCandidateStageRefusal {
                reason: "candidate native custody unavailable",
                owner,
                domains,
            });
        }
        let publications = &mut self.publications;
        self.candidate_root
            .stage(owner, domains, |queued| {
                let Ok(chunk) = publications.write_chunk_uninit(1) else {
                    return Err(queued);
                };
                chunk.fill_from_iter(std::iter::once(QueuedPublication::Candidate(queued)));
                Ok(())
            })
            .map(|(_, ticket)| ticket)
    }
    pub fn recover_audio_candidate(
        &mut self,
        ticket: &crate::AudioCandidateTicket,
    ) -> Option<crate::AudioCandidateRecovery> {
        // Caller polls ordinary return ingestion with reclaim(); this method
        // only moves a READY original bank/receipt, with no destructor or join.
        self.candidate_root.recover(ticket)
    }
    pub fn audio_candidate_custody(&self) -> crate::AudioCandidateCustodyReport {
        self.candidate_root.report()
    }
    pub fn request_shutdown(&mut self) {
        self.status.closing.store(true, Release);
        self.status.keyboard_activation.revoke_reserved_prefix();
        self.status.sampler_keyboard.revoke_all();
        if let Some(reaper) = self.output_transport_reaper.as_ref() {
            reaper.revoke();
        }
        self.status.pending_stop_marker.store(
            self.status.admitted_ticket.load(Acquire).saturating_add(1),
            Release,
        );
    }
}
impl Control {
    /// Caller retains a rejected command; Stop uses the independent priority path.
    pub fn command(&mut self, command: PlaybackCommand) -> Result<(), PlaybackCommand> {
        self.command_with_ticket(command).map(|_| ())
    }
    /// Enqueue without waiting and return a monotonic acknowledgment ticket.
    /// Failed admission does not advance the ticket. `consumed_command` denotes
    /// consumption, not guaranteed application: priority Stop discards commands admitted
    /// before its ticket marker while later commands in the captured prefix follow FIFO.
    /// Playback starts admitted before an output fault that reach consumption
    /// receive `OutputFailed`, even if a replacement has cleared the fault.
    /// Priority Stop can supersede them; later user commands and explicit
    /// seek/stop retain their ordinary ordering.
    /// While a render fault remains latched, starts receive `OutputFailed`
    /// and the failed graph is not rendered; a refused PlayFrom still updates
    /// the recovery cursor unless a later priority Stop supersedes it. Seek
    /// and Stop keep their ordinary semantics. A replacement must clear the
    /// fault before a new start can run the graph.
    pub fn command_with_ticket(
        &mut self,
        command: PlaybackCommand,
    ) -> Result<u64, PlaybackCommand> {
        self.action_with_ticket(PlaybackAction::Transport(command))
            .map_err(|action| match action {
                PlaybackAction::Transport(command) => command,
                PlaybackAction::Session(_)
                | PlaybackAction::LaunchAndPlay(_)
                | PlaybackAction::SetMetronome(_) => command,
            })
    }

    /// Enqueue a fixed-size Session route action without waiting. Actions are
    /// rejected at the callback boundary with a separate ticket/reason status.
    pub fn session_command_with_ticket(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
    ) -> Result<u64, hex_engine::SessionPlaybackCommand> {
        self.action_with_ticket(PlaybackAction::Session(command))
            .map_err(|action| match action {
                PlaybackAction::Session(command) => command,
                PlaybackAction::Transport(_)
                | PlaybackAction::LaunchAndPlay(_)
                | PlaybackAction::SetMetronome(_) => command,
            })
    }

    /// Atomically enqueue a valid Session clip/scene launch followed by Play.
    pub fn launch_and_play_with_ticket(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
    ) -> Result<u64, hex_engine::SessionPlaybackCommand> {
        self.action_with_ticket(PlaybackAction::LaunchAndPlay(command))
            .map_err(|action| match action {
                PlaybackAction::LaunchAndPlay(command) | PlaybackAction::Session(command) => {
                    command
                }
                PlaybackAction::Transport(_) | PlaybackAction::SetMetronome(_) => command,
            })
    }

    pub fn action_with_ticket(&mut self, action: PlaybackAction) -> Result<u64, PlaybackAction> {
        endpoints::legacy_enqueue(self, action)
    }
    /// Control-only input allocation; retains only the scalar output activation clock.
    pub fn new_computer_keyboard_input_on_control(
        &self,
    ) -> Result<hex_engine::computer_keyboard::ComputerKeyboardInput, hex_engine::Invalid> {
        hex_engine::computer_keyboard::ComputerKeyboardInput::with_output_activation(Arc::clone(
            &self.status.keyboard_activation,
        ))
    }
    pub fn stop(&self) {
        self.midi_recording.finish();
        self.status.keyboard_activation.revoke_reserved_prefix();
        self.status.sampler_keyboard.revoke_all();
        // Ticket MAX is refused for ordinary commands, so the supported marker
        // encoding (ticket + 1) is always nonzero and cannot wrap.
        self.status
            .pending_stop_marker
            .store(self.command_ticket.saturating_add(1), Release);
        self.processor_audition.invalidate_current_immediately();
    }
    /// Freeze publication, return active/queued graphs over subsequent callbacks.
    /// Control must continue reclaiming until shutdown_complete before stream teardown.
    pub fn request_shutdown(&mut self) {
        self.midi_recording.cancel();
        self.status.keyboard_activation.revoke_reserved_prefix();
        self.status.sampler_keyboard.revoke_all();
        self.status.closing.store(true, Release);
        self.sample_preview.shutdown();
        self.stop();
    }
    /// Resident compatibility reclamation; Verified clients never own preview PCM.
    pub fn reclaim(&mut self) -> usize {
        self.sample_preview.reclaim();
        self.publication.reclaim()
    }
}

impl Drop for PublicationControl {
    fn drop(&mut self) {
        // The exact worker guard retains its endpoints independently of
        // callback terminal order. Join/final disposal stay on control.
        if let Some(reaper) = self.output_transport_reaper.as_mut() {
            self.status.closing.store(true, Release);
            let _ = reaper.join_on_control();
        }
        if self.status.split_endpoints.load(Acquire) {
            self.request_shutdown();
        }
        if !self.native_custody {
            return;
        }
        self.candidate_root
            .abandon_control(self.retired.take(), self.held_control_return.take());
        #[cfg(all(test, feature = "allocation-probe"))]
        drop(self.endpoint_probe.take());
    }
}

impl PublicationControl {
    /// Original whole-output exchange, with its actual joining worker retained
    /// by this existing control owner. Native device/graph admission remains
    /// unavailable until device insertion and delayed delivery clocks exist.
    /// `process` runs only on the worker and must bound its own IPC/supervision;
    /// it must never load native plugin ABI into Hex.
    pub fn prepare_output_transport_worker_on_control(
        &mut self,
        callback: &mut Callback,
        binding: hex_lattice_contract::ForeignProcessorBinding,
        quantum: u16,
        process: impl FnMut(
            hex_plugin_host::transport::Block,
        ) -> Result<
            hex_plugin_host::transport::Block,
            hex_plugin_host::transport::Fault,
        > + Send
        + 'static,
    ) -> Result<(), Invalid> {
        if self.native_custody
            || self.output_transport_reaper.is_some()
            || binding.root != self.candidate_root.root_id()
            || !callback.belongs_to_candidate_root(&self.candidate_root)
            || !callback.matches_native_status(&self.status)
        {
            return Err(Invalid(
                "output transport owner mismatch or already exposed",
            ));
        }
        let (control, worker) = callback.prepare_output_transport_on_control(
            binding.instance,
            binding.graph,
            binding.epoch,
            quantum,
        )?;
        match crate::plugin_worker::spawn(control, worker, process) {
            Ok(reaper) => {
                self.output_transport_reaper = Some(reaper);
                Ok(())
            }
            Err(_) => {
                // Spawn failed before exposure/dispatch; all endpoint disposal
                // is still on this constructing control lane.
                callback.output_transport = None;
                Err(Invalid("output transport worker could not start"))
            }
        }
    }
    /// Diagnostic/control-fixture observation, never readiness or custody proof.
    pub fn output_transport_completed_blocks_on_control(&self) -> Option<u64> {
        self.output_transport_reaper
            .as_ref()
            .map(crate::plugin_worker::Reaper::completed_blocks)
    }
}

impl Callback {
    /// Construct an original whole-output transport profile before callback
    /// exposure. This does not admit a device, graph node or native worker.
    /// Native resource admission remains unavailable until device insertion
    /// and delayed delivery clocks exist. Spectrum/gain capture and native
    /// parameter input are unsupported in this original whole-output profile;
    /// ordinary output is unchanged.
    pub(crate) fn prepare_output_transport_on_control(
        &mut self,
        instance: u64,
        graph_revision: u64,
        epoch: u64,
        quantum: u16,
    ) -> Result<
        (
            hex_plugin_host::transport::Control,
            hex_plugin_host::transport::Worker,
        ),
        Invalid,
    > {
        if self.output_transport.is_some()
            || !self.transport_setup_open
            || self.active.is_some()
            || self.spectrum_capture.is_some()
            || self.status.client_exposed.load(Acquire)
            || self.status.closing.load(Acquire)
            || epoch != self.status.transport_epoch.load(Acquire)
        {
            return Err(Invalid(
                "output transport requires an unexposed empty callback",
            ));
        }
        let binding = hex_lattice_contract::ForeignProcessorBinding {
            root: self.candidate_root.root_id(),
            instance,
            graph: graph_revision,
            epoch,
        };
        let (control, callback, worker) = hex_plugin_host::transport::channel(binding, quantum)
            .map_err(|_| Invalid("invalid output transport configuration"))?;
        self.output_transport = Some(Box::new(callback));
        Ok((control, worker))
    }

    pub(crate) fn output_transport_quantum(&self) -> Option<usize> {
        self.output_transport
            .as_ref()
            .map(|transport| transport.quantum_frames())
    }

    pub(crate) fn revoke_output_transport(&mut self) {
        self.revoke_native_effects();
        self.revoke_forwarded_output_transport();
    }

    fn revoke_forwarded_output_transport(&mut self) {
        if let Some(transport) = self.output_transport.as_mut() {
            transport.revoke();
            self.output_transport_cancelled = true;
        }
    }

    pub(crate) fn stage_output_transport(&mut self, frames: &mut [hex_engine::Frame]) -> bool {
        if self.status.closing.load(Acquire) {
            self.revoke_output_transport();
        }
        if self.output_transport_cancelled {
            frames.fill([0.0; 2]);
            if self.status.playing.load(Relaxed) {
                if let Some(transport) = self.output_transport.as_mut() {
                    transport.reject(hex_plugin_host::transport::Fault::Revoked);
                }
                return false;
            }
            return true;
        }
        let Some(transport) = self.output_transport.as_mut() else {
            return true;
        };
        if transport.binding().graph != self.status.applied_revision.load(Acquire)
            || transport.binding().epoch != self.status.transport_epoch.load(Acquire)
        {
            transport.reject(hex_plugin_host::transport::Fault::Identity);
            return false;
        }
        if self.gain_recording.has_take()
            || self.native_parameter_input.has_input()
            || frames.len() > hex_plugin_host::transport::MAX_FRAMES
        {
            transport.reject(hex_plugin_host::transport::Fault::Protocol);
            return false;
        }
        let mut input = [[0.0; 2]; hex_plugin_host::transport::MAX_FRAMES];
        input[..frames.len()].copy_from_slice(frames);
        // This finite stereo-output profile has no MIDI destination. Refuse
        // output events rather than silently claiming they reached a device.
        transport
            .stage(&input[..frames.len()], &[], frames)
            .is_ok_and(|events| events.as_slice().is_empty())
    }

    pub(crate) fn finish_output_transport(&mut self, accepted: bool) -> OutputTransportDelivery {
        use OutputTransportDelivery::{Accepted, Cancelled, Refused};
        let Some(transport) = self.output_transport.as_ref() else {
            return Accepted;
        };
        if transport
            .fault()
            .is_some_and(|fault| fault != hex_plugin_host::transport::Fault::Revoked)
        {
            return Refused;
        }
        // Control may cancel after staging/conversion. Recheck its real owner
        // signals at the last boundary before any request can reach the worker.
        if self.status.closing.load(Acquire) || self.status.pending_stop_marker.load(Acquire) != 0 {
            self.revoke_output_transport();
            return Cancelled;
        }
        if self.output_transport_cancelled {
            return if self.status.playing.load(Relaxed) {
                Refused
            } else {
                Cancelled
            };
        }
        match self.output_transport.as_mut() {
            Some(transport) => {
                if transport.finish(accepted).is_ok() {
                    Accepted
                } else {
                    Refused
                }
            }
            _ => Refused,
        }
    }
    pub(crate) fn output_transport_error(&self) -> Option<Invalid> {
        self.output_transport
            .as_ref()
            .and_then(|transport| transport.fault())
            .map(|fault| Invalid(fault.message()))
            .or_else(|| {
                self.native_effects_revoked()
                    .then_some(Invalid("native effect callback refused"))
            })
    }

    pub(crate) fn native_spectrum_receipt(
        &self,
    ) -> Option<Option<crate::spectrum_worker::NativeSpectrumReceipt>> {
        self.spectrum_capture
            .as_ref()
            .map_or(Some(None), |capture| capture.native_receipt().map(Some))
    }
    pub(crate) fn constructor_channel_storage(
        &self,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        // Worker endpoints retire through the existing guards/control owner.
        // Ring custody still does not grant native graph/device admission or
        // supply the delayed delivery clocks required for real device insertion.
        if self.output_transport.is_some() {
            return None;
        }
        let outcome_storage = if self.action_writer.is_some() {
            crate::native_callback_owner::ChannelStorage {
                data_bytes: action_outcomes::storage_bytes(),
                shared_groups: 1,
                shared_handles: 3,
            }
        } else {
            crate::native_callback_owner::ChannelStorage::default()
        };
        let storage = self
            .sample_preview
            .constructor_channel_storage()?
            .plus(crate::sample_recording::Control::callback_storage(
                &self.sample_recording,
            )?)?
            .plus(self.gain_recording.constructor_channel_storage()?)?
            .plus(self.native_parameter_input.constructor_channel_storage()?)?
            .plus(
                self.foreign_parameter_delivery
                    .constructor_channel_storage()?,
            )?
            .plus(self.midi_recording.constructor_channel_storage()?)?
            .plus(self.spectrum_target.constructor_channel_storage()?)?
            .plus(self.processor_audition.constructor_channel_storage()?)?
            .plus(outcome_storage)?
            .plus(crate::native_callback_owner::ChannelStorage {
                data_bytes: self.spectrum_stages.heap_data_bytes()?,
                shared_groups: 0,
                shared_handles: 0,
            })?;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let storage = storage.plus(self.foreign_editor_delivery.constructor_channel_storage()?)?;
        Some(storage)
    }

    fn install_candidate(
        &mut self,
        value: crate::candidate_custody::QueuedCandidate,
    ) -> Result<(), crate::candidate_custody::QueuedCandidate> {
        #[cfg(test)]
        let allowed = self.test_admit_candidate;
        #[cfg(not(test))]
        let allowed = false;
        if !allowed
            || self.active.is_some()
            || self.deferred.is_some()
            || self.status.closing.load(Acquire)
        {
            return Err(value);
        }
        let active =
            crate::candidate_custody::ActiveCandidate::install(value, &self.candidate_root)?;
        self.active = Some(ActivePublication::Candidate(active));
        self.status.publication_pending.store(false, Release);
        Ok(())
    }
    pub(crate) fn native_endpoint_bank_data_bytes(&self) -> Option<usize> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.native_sources.data_bytes()
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            Some(0)
        }
    }
    pub(crate) fn matches_native_status(&self, status: &Arc<Status>) -> bool {
        Arc::ptr_eq(&self.status, status)
    }
    pub(crate) fn has_action_writer(&self) -> bool {
        self.action_writer.is_some()
    }
    pub(crate) fn action_writer_identity(&self) -> Option<usize> {
        self.action_writer
            .as_ref()
            .map(action_outcomes::Writer::identity)
    }
    /// Consume the matched producer and writer only under the exact native
    /// root's post-guard, post-stream-handle release authority.
    pub(crate) fn retire_action_outcomes_on_control(
        &mut self,
        client: &mut ClientEndpoints,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
    ) -> Result<(), ()> {
        if !authority.matches(self.candidate_root.root_id())
            || !client.matches(&self.status)
            || !client.matches_retired_scalar_output(
                Arc::as_ptr(&self.status) as usize,
                self.candidate_root.root_id(),
            )
        {
            return Err(());
        }
        let Some(writer) = self.action_writer.take() else {
            return Err(());
        };
        let Some(admission) = client.take_action_admission() else {
            self.action_writer = Some(writer);
            return Err(());
        };
        match writer.interrupt_after_quiescence(admission) {
            Ok(()) => Ok(()),
            Err((writer, admission)) => {
                self.action_writer = Some(writer);
                client.restore_action_admission(admission);
                Err(())
            }
        }
    }
    pub(crate) fn candidate_root_for_native_boundary(
        &self,
    ) -> crate::candidate_custody::CandidateRoot {
        self.candidate_root.clone()
    }
    pub(crate) fn belongs_to_candidate_root(
        &self,
        root: &crate::candidate_custody::CandidateRoot,
    ) -> bool {
        self.candidate_root.same_root(root)
    }
    pub(crate) fn has_active_ordinary_streams(&self) -> bool {
        matches!(&self.active, Some(ActivePublication::Legacy(publication)) if publication.streams.is_some())
    }
    /// Both callback carriers and the actual stream handle are terminal-owned.
    /// Release only the ordinary graph's source associations here; its actual
    /// Playback/resource-book owners still follow their existing destructors.
    pub(crate) fn release_final_ordinary_streams_on_control(
        &mut self,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
    ) -> bool {
        if !authority.matches(self.candidate_root.root_id()) {
            return false;
        }
        if let Some(ActivePublication::Legacy(publication)) = self.active.as_mut() {
            publication.release_streams_on_control(&self.candidate_root);
            // Both carriers and the actual backend are terminal here. The
            // remaining graph is inert metadata; releasing native references
            // cannot admit reuse because this Root is already closing. Keep
            // the original callback Box/resources in their terminal custody.
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            drop(publication.native_references.take());
        }
        true
    }
    pub(crate) fn has_active_candidate(&self) -> bool {
        matches!(
            self.active,
            Some(ActivePublication::Candidate(_) | ActivePublication::Eligible(_))
        )
    }
    pub(crate) fn extract_final_candidate_on_control(
        &mut self,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
        destination: &mut Option<crate::candidate_custody::FinalActiveReturn>,
    ) -> bool {
        if destination.is_some() || !authority.matches(self.candidate_root.root_id()) {
            return false;
        }
        if !self.has_active_candidate() {
            return true;
        }
        // Prevalidated destination and authority; no provider/destructor occurs
        // between taking the active owner and securing its persistent terminal slot.
        match self.active.take() {
            Some(ActivePublication::Candidate(active)) => match active.return_final(authority) {
                Ok(returned) => *destination = Some(returned),
                Err(active) => {
                    self.active = Some(ActivePublication::Candidate(active));
                    return false;
                }
            },
            Some(ActivePublication::Eligible(active)) => match active.return_final(authority) {
                Ok(returned) => {
                    *destination = Some(crate::candidate_custody::FinalActiveReturn::Eligible(
                        returned,
                    ))
                }
                Err(active) => {
                    self.active = Some(ActivePublication::Eligible(active));
                    return false;
                }
            },
            other => {
                self.active = other;
                return false;
            }
        }
        true
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    fn apply_candidate_action(&mut self, action: PlaybackAction) {
        if let Some(ActivePublication::Candidate(active)) = self.active.as_mut()
            && active.owner.test_apply_action(action).is_err()
        {
            self.status.device_fault.store(true, Relaxed);
        }
    }
    pub(crate) fn close_candidate_custody_on_control(&mut self) {
        self.status.closing.store(true, Release);
        self.candidate_root.close();
    }
    pub(crate) fn has_domain_pages(&self) -> bool {
        self.domain_pages.iter().any(Option::is_some)
    }
    pub(crate) fn extract_domain_pages_on_control(
        &mut self,
        destination: &mut [Option<crate::candidate_custody::DomainEnvelope>; 2],
    ) -> bool {
        if destination.iter().any(Option::is_some)
            || self.domain_pages.iter().enumerate().any(|(index, pages)| {
                pages.as_ref().is_some_and(|pages| {
                    pages.index_for(self.candidate_root.root_id()) != Some(index)
                })
            })
        {
            return false;
        }
        for (from, to) in self.domain_pages.iter_mut().zip(destination) {
            *to = from.take();
            if let Some(pages) = to.as_ref() {
                pages.departed_callback();
            }
        }
        true
    }
    pub(crate) fn drain_candidate_custody_on_control(&mut self) -> bool {
        self.close_candidate_custody_on_control();
        if self.terminal_disposal_pending {
            return false;
        }
        // Persistent held custody precedes any destructor. A failed legacy
        // destructor latches Pending, just like the outer callback carrier.
        // Candidate carriers only move into the exact root/slot association.
        for _ in 0..2 {
            // one existing held return plus the one-slot input ring
            if self.held_retirement.is_none() {
                self.held_retirement = self
                    .deferred_eligible
                    .take()
                    .map(|value| ReturnedPublication::Eligible(value.reject_before_install()))
                    .or_else(|| match self.publications.pop().ok()? {
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        QueuedPublication::NativeEpoch { .. } => None,
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        QueuedPublication::Native(endpoint) => {
                            Some(ReturnedPublication::Native(endpoint))
                        }
                        QueuedPublication::Domain(pages) => {
                            Some(ReturnedPublication::Domain(pages))
                        }
                        QueuedPublication::Eligible(value) => {
                            Some(ReturnedPublication::Eligible(value.reject_before_install()))
                        }
                        QueuedPublication::Legacy(value) => {
                            Some(ReturnedPublication::Legacy(value))
                        }
                        QueuedPublication::Candidate(value) => Some(
                            ReturnedPublication::Candidate(value.reject_before_install()),
                        ),
                    });
            }
            if matches!(
                self.held_retirement.as_ref(),
                Some(ReturnedPublication::Legacy(_))
            ) {
                if let Some(ReturnedPublication::Legacy(publication)) =
                    self.held_retirement.as_mut()
                {
                    publication.release_streams_on_control(&self.candidate_root);
                }
                self.terminal_disposal_pending = true;
                drop(self.held_retirement.take());
                self.terminal_disposal_pending = false;
            } else if let Some(value) = self.held_retirement.take() {
                let refused = match value {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    ReturnedPublication::Native(endpoint) => self
                        .candidate_root
                        .ingest_native_endpoint(endpoint)
                        .err()
                        .map(ReturnedPublication::Native),
                    ReturnedPublication::Domain(pages) => {
                        if let Some(index) = pages.index_for(self.candidate_root.root_id())
                            && self.domain_pages[index].is_none()
                        {
                            self.domain_pages[index] = Some(pages);
                            None
                        } else {
                            Some(ReturnedPublication::Domain(pages))
                        }
                    }
                    ReturnedPublication::Candidate(value) => self
                        .candidate_root
                        .ingest(value)
                        .err()
                        .map(ReturnedPublication::Candidate),
                    ReturnedPublication::Eligible(value) => self
                        .candidate_root
                        .ingest_eligible(value)
                        .err()
                        .map(ReturnedPublication::Eligible),
                    // Handled above without holding any owner across destruction.
                    ReturnedPublication::Legacy(value) => Some(ReturnedPublication::Legacy(value)),
                };
                if let Some(value) = refused {
                    self.held_retirement = Some(value);
                    return false;
                }
            }
        }
        self.held_retirement.is_none() && self.publications.is_empty()
    }

    fn return_graph(&mut self, publication: Publication) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.restore_parameter_predecessor_prewarm(&publication);
        self.return_publication(ReturnedPublication::Legacy(publication));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn restore_parameter_predecessor_prewarm(&mut self, publication: &Publication) {
        let PublicationPolicy::InheritUnderNativeParameter { source, .. } = publication.policy
        else {
            return;
        };
        if self.status.closing.load(Acquire) {
            return;
        }
        if let Some(active) = self.active.as_mut().and_then(ActivePublication::legacy_mut)
            && active.revision == source.audio_revision
            && active.source_context == Some(source.context)
            && let Some(mut view) = active.stream_view()
        {
            view.publication_abandoned();
        }
    }
    fn return_publication(&mut self, publication: ReturnedPublication) {
        if let ReturnedPublication::Domain(pages) = &publication {
            pages.departed_callback();
        }
        if let Err(PushError::Full(publication)) = self.retired.push(publication) {
            self.held_retirement = Some(publication);
        }
    }
    /// At most one replacement and 32 commands; no graph disposal or allocation.
    pub fn begin_block(&mut self) {
        self.begin_block_inner();
        self.flush_stream_outcomes();
    }
    fn begin_block_inner(&mut self) {
        self.transport_setup_open = false;
        self.device_fault_fenced_in_block = false;
        if self.status.device_fault.load(Acquire) && !self.status.closing.load(Acquire) {
            self.fence_device_fault();
            return;
        }
        if self.status.closing.load(Acquire) || self.status.pending_stop_marker.load(Acquire) != 0 {
            self.revoke_output_transport();
        }
        self.sample_preview
            .begin_block(self.status.closing.load(Acquire));
        if self.status.closing.load(Acquire) {
            self.invalidate_midi_recording(crate::midi_recording::AbortReason::Cancelled);
            self.invalidate_gain_recording(crate::gain_recording::AbortReason::Cancelled);
        }
        self.processor_audition.begin_block();
        self.apply_processor_audition_boundary();
        self.spectrum_target.begin_block();
        #[cfg(test)]
        if let Some(barrier) = self.test_after_spectrum_target_boundary.take() {
            barrier.wait();
            barrier.wait();
        }
        if let Some(publication) = self.held_retirement.take() {
            self.return_publication(publication);
            if self.held_retirement.is_some() {
                // Ownership backpressure must not delay the independent Stop
                // path. Keep the graph retained while consuming bounded control.
                if self.status.closing.load(Acquire) {
                    self.status.playing.store(false, Relaxed);
                } else {
                    self.consume_commands();
                }
                return;
            }
        }
        self.return_requested_domain_pages();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.native_sources.refresh_runtime_requirements(
            self.active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|active| active.playback.as_ref()),
        );
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.return_requested_native_sources();
        if self.status.closing.load(Acquire) {
            // A candidate return may end this block early; closing is already
            // silent and must expose stopped status at every such frontier.
            self.status.playing.store(false, Relaxed);
            if !self.retired.is_full()
                && let Some(value) = self.deferred_eligible.take()
            {
                self.return_publication(ReturnedPublication::Eligible(
                    value.reject_before_install(),
                ));
                self.status.publication_pending.store(false, Release);
            }
            if !self.retired.is_full()
                && let Some(publication) = self.deferred.take()
            {
                if let Some(cancellation) = publication.fresh_runtime_cancellation.as_ref()
                    && cancellation.finish_for_shutdown()
                {
                    self.reject_publication(
                        publication.revision,
                        PlaybackHandoffRejection::FreshRuntimeCancelled,
                    );
                }
                self.return_graph(publication);
                self.status.publication_pending.store(false, Release);
            }
            if !self.retired.is_full() {
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::current_mut)
                {
                    active.playback.cancel_all_processor_auditions();
                }
                if matches!(self.active, Some(ActivePublication::Legacy(_)))
                    && let Some(ActivePublication::Legacy(publication)) = self.active.take()
                {
                    self.return_graph(publication);
                }
            }
            if !self.retired.is_full()
                && let Ok(publication) = self.publications.pop()
            {
                let publication = match publication {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    QueuedPublication::NativeEpoch { .. } => return,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    QueuedPublication::Native(endpoint) => {
                        self.return_publication(ReturnedPublication::Native(endpoint));
                        return;
                    }
                    QueuedPublication::Domain(pages) => {
                        self.return_publication(ReturnedPublication::Domain(pages));
                        return;
                    }
                    QueuedPublication::Eligible(value) => {
                        self.return_publication(ReturnedPublication::Eligible(
                            value.reject_before_install(),
                        ));
                        self.status.publication_pending.store(false, Release);
                        return;
                    }
                    QueuedPublication::Legacy(publication) => publication,
                    QueuedPublication::Candidate(candidate) => {
                        self.return_publication(ReturnedPublication::Candidate(
                            candidate.reject_before_install(),
                        ));
                        return;
                    }
                };
                if let Some(cancellation) = publication.fresh_runtime_cancellation.as_ref()
                    && cancellation.finish_for_shutdown()
                {
                    self.reject_publication(
                        publication.revision,
                        PlaybackHandoffRejection::FreshRuntimeCancelled,
                    );
                }
                self.return_graph(publication);
                self.status.publication_pending.store(false, Release);
            }
            self.status.playing.store(false, Relaxed);
            if self.active.is_none()
                && self.published_route_key.is_some()
                && self.deferred.is_none()
                && self.deferred_eligible.is_none()
                && self.publications.is_empty()
            {
                self.status.publish_empty_routes();
                self.published_route_key = None;
            }
            if self.active.is_none()
                && self.deferred.is_none()
                && self.deferred_eligible.is_none()
                && self.publications.is_empty()
                && self.held_retirement.is_none()
                && self.sample_preview.is_empty()
            {
                self.status.shutdown_complete.store(true, Release);
            }
            return;
        }
        if self.deferred.is_none()
            && self.deferred_eligible.is_none()
            && (self.active.is_none() || !self.retired.is_full())
            && let Ok(publication) = self.publications.pop()
        {
            match publication {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                QueuedPublication::NativeEpoch { target, fence } => {
                    self.consume_commands();
                    self.return_native_epoch_runtime(target, fence);
                    return;
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                QueuedPublication::Native(endpoint) => {
                    if let Err(endpoint) = self
                        .native_sources
                        .install(self.candidate_root.root_id(), endpoint)
                    {
                        self.return_publication(ReturnedPublication::Native(endpoint));
                    }
                    self.consume_commands();
                    return;
                }
                QueuedPublication::Domain(pages) => {
                    if let Some(index) = pages.index_for(self.candidate_root.root_id())
                        && self.domain_pages[index].is_none()
                    {
                        self.domain_pages[index] = Some(pages);
                        if let Some(pages) = self.domain_pages[index].as_ref() {
                            pages.installed_on_callback();
                        }
                    } else {
                        self.return_publication(ReturnedPublication::Domain(pages));
                    }
                }
                QueuedPublication::Eligible(value) => {
                    self.deferred_eligible = Some(value);
                }
                QueuedPublication::Legacy(publication) => {
                    if matches!(
                        self.active,
                        Some(ActivePublication::Candidate(_) | ActivePublication::Eligible(_))
                    ) {
                        self.return_graph(publication);
                        self.consume_commands();
                        return;
                    }
                    self.deferred = Some(publication);
                }
                QueuedPublication::Candidate(candidate) => {
                    if let Err(candidate) = self.install_candidate(candidate) {
                        // Default production admission remains an unconditional
                        // refusal before active installation or render.
                        self.return_publication(ReturnedPublication::Candidate(
                            candidate.reject_before_install(),
                        ));
                    }
                    // Rejection must not starve priority Stop or the bounded
                    // command prefix belonging to the still-active graph.
                    self.consume_commands();
                    return;
                }
            }
        }

        if self.deferred_eligible.is_some() {
            self.advance_eligible_publication(None);
            self.consume_commands();
            return;
        }

        self.advance_legacy_publication(None);
        self.consume_commands();
    }

    fn reject_publication(&mut self, revision: u64, reason: PlaybackHandoffRejection) {
        self.status.publish_rejected_revision(revision, reason);
        self.trace_audio(
            AudioDiagnosticKind::PublicationRejected { reason },
            revision,
            None,
            0,
        );
    }

    fn trace_audio(
        &mut self,
        kind: AudioDiagnosticKind,
        revision: u64,
        context: Option<hex_engine::AudioSourceContext>,
        frames: usize,
    ) {
        if !self
            .status
            .audio_diagnostics_flags()
            .contains(kind.category())
        {
            return;
        }
        let event = AudioDiagnosticEvent {
            sequence: 0,
            output: self.candidate_root.root_id(),
            revision,
            context,
            applied_revision: self.status.applied_revision.load(Relaxed),
            consumed_command: self.status.consumed_command.load(Relaxed),
            performance_frame: self.status.performance_frame.load(Relaxed),
            arrangement_frame: self.status.position.load(Relaxed),
            rate: self.rate,
            frames,
            commands_pending: self.commands.slots(),
            retirement_slots: self.retired.slots(),
            kind,
        };
        self.diagnostics
            .push(event, &self.status.diagnostics_dropped);
    }
    /// Native adapter supplies its already measured whole-host-callback time.
    /// Reading the clock and formatting remain outside this record operation.
    pub(crate) fn trace_callback_timing(
        &mut self,
        elapsed_ns: u64,
        deadline_ns: u64,
        frames: usize,
    ) {
        self.trace_audio(
            AudioDiagnosticKind::CallbackTiming {
                elapsed_ns,
                deadline_ns,
            },
            self.status.applied_revision.load(Relaxed),
            None,
            frames,
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn native_parameter_fence_matches(
        &self,
        source: AppliedAudioSource,
        transport_epoch: u64,
        playing: bool,
        successor: Option<hex_engine::AudioSourceContext>,
    ) -> bool {
        let predecessor = self.active.as_ref().and_then(ActivePublication::legacy);
        self.status.native_parameter_source(playing) == Some((source, transport_epoch))
            && source.output_identity == self.candidate_root.root_id()
            && predecessor.is_some_and(|active| {
                active.revision == source.audio_revision
                    && active.source_context == Some(source.context)
            })
            && successor.is_some_and(|context| {
                context.project_generation == source.context.project_generation
                    && source.context.editor_revision.checked_add(1)
                        == Some(context.editor_revision)
            })
    }

    fn advance_legacy_publication(&mut self, frames: Option<usize>) {
        if frames.is_none()
            && self
                .deferred
                .as_ref()
                .is_some_and(|next| next.streams.is_some())
        {
            // An already-prepared candidate must release its page role before
            // an admitted command can schedule work on the predecessor.
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if self.status.admitted_ticket.load(Acquire)
                != self.status.consumed_command.load(Acquire)
                && let Some(mut next) = self.deferred.take()
            {
                if matches!(
                    next.policy,
                    PublicationPolicy::InheritUnderNativeParameter { .. }
                ) {
                    if let Some(streams) = next.streams.as_mut() {
                        streams.cancel_install(&mut self.domain_pages);
                    }
                    self.restore_parameter_predecessor_prewarm(&next);
                }
                self.deferred = Some(next);
            }
            return;
        }
        if !self.quantized_seek_publication_due() {
            return;
        }
        // A candidate with a transient release-capacity refusal remains owned
        // here while the previous runtime continues to render and drain tails.
        // The queue gate prevents a second graph from being admitted meanwhile.
        if (self.active.is_none() || !self.retired.is_full())
            && let Some(mut next) = self.deferred.take()
        {
            self.trace_audio(
                AudioDiagnosticKind::PublicationAttempt,
                next.revision,
                next.source_context,
                frames.unwrap_or(0),
            );
            if next
                .fresh_runtime_cancellation
                .as_ref()
                .is_some_and(|c| c.decision.load(Acquire) == FRESH_PUBLICATION_CANCELLED)
            {
                if let Some(streams) = next.streams.as_mut() {
                    streams.cancel_install(&mut self.domain_pages);
                }
                if let Some(cancellation) = next.fresh_runtime_cancellation.as_ref() {
                    cancellation.finish();
                }
                self.reject_publication(
                    next.revision,
                    PlaybackHandoffRejection::FreshRuntimeCancelled,
                );
                self.return_graph(next);
                self.status.publication_pending.store(false, Release);
                return;
            }
            // Do not overtake an accepted transport command or cancel a
            // streamed action while deciding a gesture-bound successor.
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if matches!(
                next.policy,
                PublicationPolicy::InheritUnderNativeParameter { .. }
            ) && (self.status.admitted_ticket.load(Acquire)
                != self.status.consumed_command.load(Acquire)
                || self.stream_pending_play.is_some()
                || self.stream_action_pending()
                || self.transport_fade.is_some())
            {
                if let Some(streams) = next.streams.as_mut() {
                    streams.cancel_install(&mut self.domain_pages);
                }
                self.restore_parameter_predecessor_prewarm(&next);
                self.deferred = Some(next);
                return;
            }
            #[cfg(test)]
            let forced_refusal = self.test_transient_handoff_refusals > 0;
            #[cfg(test)]
            if forced_refusal {
                self.test_transient_handoff_refusals -= 1;
            }
            #[cfg(not(test))]
            let forced_refusal = false;
            let stale_fence = match next.policy {
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence)
                | PublicationPolicy::InstallQuantizedSeekWhenReady { fence, .. } => {
                    let active_revision = self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::legacy)
                        .map_or(0, |active| active.revision);
                    let active_route_generation = self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::legacy)
                        .map_or(0, |active| active.playback.session_route_state_generation());
                    let admitted_ticket = self.status.admitted_ticket.load(Acquire);
                    let stop_pending = self.status.pending_stop_marker.load(Acquire) != 0;
                    (active_revision != fence.expected_active_revision
                        || active_route_generation != fence.route_state_generation
                        || admitted_ticket != fence.admitted_command_ticket
                        || self.status.last_output_fault_epoch.load(Acquire)
                            != fence.output_fault_epoch
                        || self.status.consumed_stop_generation.load(Acquire)
                            != fence.consumed_stop_generation
                        || self.status.render_fault.load(Acquire)
                        || self.status.device_fault.load(Acquire)
                        || stop_pending)
                        .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                PublicationPolicy::InstallUnderStoppedArrangement(token)
                | PublicationPolicy::InheritUnderArrangement(token) => {
                    (self.status.sampler_edit.state(token) != StoppedEditFenceState::Ready)
                        .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                PublicationPolicy::InstallUnderNativeEpoch(token) => {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let live = self
                        .native_sources
                        .runtime_transports_are_live(&next.playback);
                    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                    let live = false;
                    (self
                        .status
                        .sampler_edit
                        .native_epoch_context(token)
                        .is_none()
                        || self.status.sampler_edit.native_epoch_context(token)
                            != next.source_context
                        || self.active.is_some()
                        || !live)
                        .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                PublicationPolicy::InheritUnderStoppedNative { source, fence } => {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let live = self
                        .native_sources
                        .runtime_transports_are_live(&next.playback)
                        && self.current_native_epoch_source() == Some(source);
                    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                    let live = false;
                    (!live
                        || !self
                            .status
                            .stopped_native_source_fence_matches(source, fence)
                        || self
                            .active
                            .as_ref()
                            .and_then(ActivePublication::legacy)
                            .is_none_or(|active| active.playback.playing())
                        || next.source_context.is_none_or(|context| {
                            context.project_generation != source.context.project_generation
                        }))
                    .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                PublicationPolicy::InheritUnderNativeParameter {
                    source,
                    transport_epoch,
                    playing,
                } => {
                    let predecessor_matches = next.native_continuation.is_none()
                        || self
                            .active
                            .as_ref()
                            .and_then(ActivePublication::legacy)
                            .is_some_and(|active| {
                                next.playback
                                    .foreign_continuation_matches_predecessor_on_callback(
                                        &active.playback,
                                    )
                            });
                    (!predecessor_matches
                        || !self.native_parameter_fence_matches(
                            source,
                            transport_epoch,
                            playing,
                            next.source_context,
                        ))
                    .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                PublicationPolicy::InheritRuntime | PublicationPolicy::InstallFreshRuntime => None,
            };
            #[cfg(test)]
            if stale_fence.is_none()
                && matches!(
                    next.policy,
                    PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                        | PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                )
                && let Some(barrier) = self.test_after_fresh_fence_validation.take()
            {
                barrier.wait();
                barrier.wait();
            }
            let mut promotions = None;
            if !forced_refusal && stale_fence.is_none() && next.streams.is_some() {
                let pending_window = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::stream_view)
                    .is_some_and(|active| active.has_pending_render_window());
                if pending_window {
                    if next.policy.inherits_runtime() {
                        // Give an inheriting successor's predecessor a chance
                        // to commit its exact pending window first, while
                        // bounding a current demand that can never fit.
                        if let Some(frames) = frames {
                            let mut reservation = self.reserve_current_stream_window(frames);
                            if matches!(
                                reservation,
                                Err(crate::candidate_custody::QuantumFailure::Capacity)
                            ) {
                                if let Some(streams) = next.streams.as_mut() {
                                    streams.cancel_install(&mut self.domain_pages);
                                }
                                reservation = self.reserve_current_stream_window(frames);
                            }
                            match reservation {
                                Ok(Some(pins)) => self.current_quantum_pins = Some(pins),
                                Ok(None)
                                | Err(crate::candidate_custody::QuantumFailure::PendingAction) => {}
                                Err(crate::candidate_custody::QuantumFailure::Capacity) => {
                                    if !next
                                        .streams
                                        .as_mut()
                                        .is_some_and(|streams| streams.retry_capacity_conflict())
                                    {
                                        if let Some(streams) = next.streams.as_mut() {
                                            streams.cancel_install(&mut self.domain_pages);
                                        }
                                        self.reject_publication(
                                            next.revision,
                                            PlaybackHandoffRejection::StreamedPageCapacity,
                                        );
                                        self.return_graph(next);
                                        self.status.publication_pending.store(false, Release);
                                        return;
                                    }
                                }
                                Err(crate::candidate_custody::QuantumFailure::Invalid) => {
                                    if let Some(streams) = next.streams.as_mut() {
                                        streams.cancel_install(&mut self.domain_pages);
                                    }
                                    self.reject_publication(
                                        next.revision,
                                        PlaybackHandoffRejection::StreamedPagesNotReady,
                                    );
                                    self.return_graph(next);
                                    self.status.publication_pending.store(false, Release);
                                    return;
                                }
                            }
                        }
                        self.deferred = Some(next);
                        return;
                    }
                    // A fresh replacement is a recovery path when the current
                    // source cannot complete; it must not wait on that source.
                    if let Some(mut active) = self
                        .active
                        .as_mut()
                        .and_then(ActivePublication::stream_view)
                    {
                        active.cancel_pending_render_window(&mut self.domain_pages);
                    }
                }
                self.cancel_active_stream_action();
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::stream_view)
                {
                    active.publication_replaces_prewarm(&mut self.domain_pages);
                }
                if next.policy.inherits_runtime()
                    && self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::legacy)
                        .is_some_and(|old| {
                            old.source_context.is_none_or(|context| {
                                context.project_generation != next.control_context_generation
                            })
                        })
                {
                    self.reject_publication(
                        next.revision,
                        PlaybackHandoffRejection::RuntimeFenceChanged,
                    );
                    self.return_graph(next);
                    self.status.publication_pending.store(false, Release);
                    return;
                }
                if let Some(frames) = frames {
                    let mut reservation = self.reserve_current_stream_window(frames);
                    if matches!(
                        reservation,
                        Err(crate::candidate_custody::QuantumFailure::Capacity)
                    ) {
                        if let Some(streams) = next.streams.as_mut() {
                            streams.cancel_install(&mut self.domain_pages);
                        }
                        reservation = self.reserve_current_stream_window(frames);
                    }
                    let refusal = match reservation {
                        Ok(Some(pins)) => {
                            let complete = pins.complete();
                            self.current_quantum_pins = Some(pins);
                            if !complete && next.policy.inherits_runtime() {
                                if let Some(streams) = next.streams.as_mut() {
                                    streams.cancel_install(&mut self.domain_pages);
                                }
                                self.deferred = Some(next);
                                return;
                            }
                            None
                        }
                        Ok(None) => None,
                        Err(crate::candidate_custody::QuantumFailure::PendingAction) => {
                            self.deferred = Some(next);
                            return;
                        }
                        Err(crate::candidate_custody::QuantumFailure::Capacity) => {
                            if next
                                .streams
                                .as_mut()
                                .is_some_and(|streams| streams.retry_capacity_conflict())
                            {
                                self.deferred = Some(next);
                                return;
                            }
                            Some(PlaybackHandoffRejection::StreamedPageCapacity)
                        }
                        Err(crate::candidate_custody::QuantumFailure::Invalid) => {
                            Some(PlaybackHandoffRejection::StreamedPagesNotReady)
                        }
                    };
                    if let Some(reason) = refusal {
                        if let Some(streams) = next.streams.as_mut() {
                            streams.cancel_install(&mut self.domain_pages);
                        }
                        if let Some(cancellation) = next.fresh_runtime_cancellation.as_ref() {
                            cancellation.finish();
                        }
                        self.reject_publication(next.revision, reason);
                        self.return_graph(next);
                        self.status.publication_pending.store(false, Release);
                        return;
                    }
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let foreign_continuation = next.native_continuation.is_some();
                #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                let foreign_continuation = false;
                let (prepared, preview_refused) = {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let guard = next.native_continuation.as_ref().and_then(|source| {
                        next.playback
                            .foreign_continuation_binding()
                            .and_then(|binding| {
                                native_sources::reserve_parameter_observation(
                                    &self.native_sources,
                                    &self.status,
                                    binding,
                                    source,
                                )
                            })
                    });
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let preview_refused =
                        foreign_continuation && guard.as_ref().is_none_or(|guard| !guard.is_live());
                    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                    let preview_refused = false;
                    if preview_refused {
                        (None, true)
                    } else {
                        let previous = if next.policy.inherits_runtime() {
                            self.active
                                .as_mut()
                                .and_then(ActivePublication::legacy_mut)
                                .and_then(|old| {
                                    old.source_context
                                        .map(|context| (&mut *old.playback, context, old.revision))
                                })
                        } else {
                            None
                        };
                        let fenced_restart =
                            matches!(next.policy, PublicationPolicy::InheritUnderArrangement(_))
                                && next.playback.live_restart_requested();
                        let prepared = next.streams.as_mut().map(|streams| {
                            streams.prepare_install(
                                &mut next.playback,
                                previous,
                                &mut self.domain_pages,
                                self.stream_pending_play.is_some(),
                                self.stream_prefix,
                                frames.unwrap_or(0),
                                fenced_restart,
                                foreign_continuation,
                            )
                        });
                        (prepared, false)
                    }
                };
                if preview_refused {
                    if let Some(streams) = next.streams.as_mut() {
                        streams.cancel_install(&mut self.domain_pages);
                    }
                    self.reject_publication(
                        next.revision,
                        PlaybackHandoffRejection::RuntimeFenceChanged,
                    );
                    self.return_graph(next);
                    self.status.publication_pending.store(false, Release);
                    return;
                }
                match prepared {
                    Some(Ok(Some(_))) => {
                        // Page planning may span callbacks. Revalidate the
                        // actual fence before promoting its protected pages.
                        if let PublicationPolicy::InheritUnderArrangement(token) = next.policy
                            && self.status.sampler_edit.state(token) != StoppedEditFenceState::Ready
                        {
                            if let Some(streams) = next.streams.as_mut() {
                                streams.cancel_install(&mut self.domain_pages);
                            }
                            self.reject_publication(
                                next.revision,
                                PlaybackHandoffRejection::RuntimeFenceChanged,
                            );
                            self.return_graph(next);
                            self.status.publication_pending.store(false, Release);
                            return;
                        }
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        if let PublicationPolicy::InheritUnderNativeParameter {
                            source,
                            transport_epoch,
                            playing,
                        } = next.policy
                        {
                            let changed = !self.native_parameter_fence_matches(
                                source,
                                transport_epoch,
                                playing,
                                next.source_context,
                            );
                            let commands_pending = self.status.admitted_ticket.load(Acquire)
                                != self.status.consumed_command.load(Acquire);
                            if changed || commands_pending {
                                if let Some(streams) = next.streams.as_mut() {
                                    streams.cancel_install(&mut self.domain_pages);
                                }
                                if changed {
                                    self.reject_publication(
                                        next.revision,
                                        PlaybackHandoffRejection::RuntimeFenceChanged,
                                    );
                                    self.return_graph(next);
                                    self.status.publication_pending.store(false, Release);
                                } else {
                                    self.restore_parameter_predecessor_prewarm(&next);
                                    self.deferred = Some(next);
                                }
                                return;
                            }
                        }
                        self.trace_audio(
                            AudioDiagnosticKind::PublicationPrepared,
                            next.revision,
                            next.source_context,
                            frames.unwrap_or(0),
                        );
                        let committed = next
                            .streams
                            .as_mut()
                            .map(|streams| streams.prepare_commit(&mut self.domain_pages));
                        match committed {
                            Some(Ok(value)) => promotions = Some(value),
                            _ => {
                                if let Some(streams) = next.streams.as_mut() {
                                    streams.cancel_install(&mut self.domain_pages);
                                }
                                self.deferred = Some(next);
                                return;
                            }
                        }
                    }
                    Some(Ok(None)) => {
                        self.trace_audio(
                            AudioDiagnosticKind::PublicationWaiting,
                            next.revision,
                            next.source_context,
                            frames.unwrap_or(0),
                        );
                        self.deferred = Some(next);
                        return;
                    }
                    refused => {
                        let capacity_conflict = next
                            .streams
                            .as_ref()
                            .is_some_and(|streams| streams.capacity_conflict());
                        if capacity_conflict
                            && next
                                .streams
                                .as_mut()
                                .is_some_and(|streams| streams.retry_candidate_capacity_conflict())
                        {
                            if let Some(streams) = next.streams.as_mut() {
                                streams.cancel_install(&mut self.domain_pages);
                            }
                            self.deferred = Some(next);
                            return;
                        }
                        let source_changed = next
                            .streams
                            .as_mut()
                            .is_some_and(|streams| streams.take_source_changed());
                        let handoff = next
                            .streams
                            .as_mut()
                            .and_then(|streams| streams.take_handoff_diagnostic());
                        let detail = match refused {
                            Some(Err(error)) => error.0,
                            _ => "stream preparation state unavailable",
                        };
                        self.trace_audio(
                            AudioDiagnosticKind::StreamPreflightFailed { detail, handoff },
                            next.revision,
                            next.source_context,
                            frames.unwrap_or(0),
                        );
                        if let Some(streams) = next.streams.as_mut() {
                            streams.cancel_install(&mut self.domain_pages);
                        }
                        if let Some(cancellation) = next.fresh_runtime_cancellation.as_ref() {
                            cancellation.finish();
                        }
                        self.reject_publication(
                            next.revision,
                            if source_changed {
                                PlaybackHandoffRejection::StreamSourceChanged
                            } else {
                                handoff.map_or(
                                    if capacity_conflict {
                                        PlaybackHandoffRejection::StreamedPageCapacity
                                    } else {
                                        PlaybackHandoffRejection::StreamedPagesNotReady
                                    },
                                    PlaybackHandoffRejection::from_error,
                                )
                            },
                        );
                        self.return_graph(next);
                        self.status.publication_pending.store(false, Release);
                        return;
                    }
                }
            }
            next.playback
                .bind_browser_audition_transport(self.status.transport_epoch.load(Acquire));
            let handoff = if forced_refusal {
                Some(Err(hex_engine::PlaybackHandoffError::Session(
                    hex_engine::SessionPlaybackError::ReleaseBankFull,
                )))
            } else if let Some(error) = stale_fence {
                Some(Err(error))
            } else if matches!(
                next.policy,
                PublicationPolicy::InstallFreshRuntime
                    | PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                    | PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                    | PublicationPolicy::InstallUnderStoppedArrangement(_)
                    | PublicationPolicy::InstallUnderNativeEpoch(_)
            ) {
                None
            } else {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let foreign_continuation = next.native_continuation.is_some();
                #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                let foreign_continuation = false;
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let mut guard = next.native_continuation.as_ref().and_then(|source| {
                    next.playback
                        .foreign_continuation_binding()
                        .and_then(|binding| {
                            native_sources::reserve_parameter_observation(
                                &self.native_sources,
                                &self.status,
                                binding,
                                source,
                            )
                        })
                });
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let refused = foreign_continuation
                    && guard.as_mut().is_none_or(|guard| {
                        !guard.try_order_before_revocation()
                            || self
                                .status
                                .device_fault
                                .compare_exchange(false, false, AcqRel, Acquire)
                                .is_err()
                            || self
                                .status
                                .render_fault
                                .compare_exchange(false, false, AcqRel, Acquire)
                                .is_err()
                            || self
                                .status
                                .pending_stop_marker
                                .compare_exchange(0, 0, AcqRel, Acquire)
                                .is_err()
                            || {
                                let consumed = self.status.consumed_command.load(Acquire);
                                self.status
                                    .admitted_ticket
                                    .compare_exchange(consumed, consumed, AcqRel, Acquire)
                                    .is_err()
                            }
                    });
                #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                let refused = false;
                if refused {
                    Some(Err(hex_engine::PlaybackHandoffError::RuntimeFenceChanged))
                } else {
                    self.active
                        .as_mut()
                        .and_then(ActivePublication::legacy_mut)
                        .map(|previous| {
                            let generation = next
                                .explicit_control_context
                                .then_some(next.control_context_generation);
                            let previous_generation = previous
                                .explicit_control_context
                                .then_some(previous.control_context_generation);
                            if matches!(next.policy, PublicationPolicy::InheritUnderArrangement(_))
                                && next.playback.live_restart_requested()
                            {
                                next.playback.try_restart_runtime_under_arrangement_fence(
                                    &mut previous.playback,
                                    generation,
                                    previous_generation,
                                )
                            } else if foreign_continuation {
                                next.playback
                                    .try_inherit_runtime_with_stored_foreign_continuation(
                                        &mut previous.playback,
                                        generation,
                                        previous_generation,
                                    )
                            } else {
                                next.playback.try_inherit_runtime_in_control_context(
                                    &mut previous.playback,
                                    generation,
                                    previous_generation,
                                )
                            }
                        })
                }
            };
            match handoff {
                Some(Err(error)) if error.is_transient() => {
                    let _promotions = promotions;
                    if let Some(streams) = next.streams.as_mut() {
                        streams.cancel_install(&mut self.domain_pages);
                    }
                    self.deferred = Some(next);
                }
                Some(Err(error)) => {
                    let _promotions = promotions;
                    if let Some(streams) = next.streams.as_mut() {
                        streams.cancel_install(&mut self.domain_pages);
                    }
                    if let Some(cancellation) = next.fresh_runtime_cancellation.as_ref() {
                        cancellation.finish();
                    }
                    if let Some(reason) = error.rejection() {
                        self.reject_publication(next.revision, reason);
                    }
                    self.return_graph(next);
                    self.status.publication_pending.store(false, Release);
                }
                _ => {
                    #[cfg(test)]
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                            | PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                    ) && let Some(barrier) = self.test_before_fresh_runtime_apply.take()
                    {
                        barrier.wait();
                        barrier.wait();
                    }
                    let applying = next
                        .fresh_runtime_cancellation
                        .as_ref()
                        .is_none_or(|cancellation| cancellation.begin_apply());
                    if !applying {
                        let _promotions = promotions;
                        if let Some(streams) = next.streams.as_mut() {
                            streams.cancel_install(&mut self.domain_pages);
                        }
                        if let Some(cancellation) = next.fresh_runtime_cancellation.as_ref() {
                            cancellation.finish();
                        }
                        self.reject_publication(
                            next.revision,
                            PlaybackHandoffRejection::FreshRuntimeCancelled,
                        );
                        self.return_graph(next);
                        self.status.publication_pending.store(false, Release);
                        return;
                    }
                    #[cfg(test)]
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                            | PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                    ) && let Some(barrier) = self.test_after_fresh_runtime_apply_decision.take()
                    {
                        barrier.wait();
                        barrier.wait();
                    }
                    for promotion in promotions.into_iter().flatten().flatten() {
                        promotion.commit();
                    }
                    if let Some(streams) = next.streams.as_mut() {
                        streams.record_install();
                    }
                    if let Some(mut previous) = self
                        .active
                        .as_mut()
                        .and_then(ActivePublication::stream_view)
                    {
                        if let Some(mut successor) = next.stream_view()
                            && let Some(ticket) =
                                successor.inherit_schedule_observation(&mut previous)
                        {
                            self.status
                                .stream_invalidated_schedule
                                .store(ticket, Release);
                        }
                        previous.clear_stream_roles(&mut self.domain_pages);
                    }
                    // A fresh seek keeps its reconstructed transport and
                    // internal DSP. Retained workers still need their gate
                    // identities after the final fence/cancellation decision.
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                            | PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                    ) && let Some(previous) =
                        self.active.as_mut().and_then(ActivePublication::legacy_mut)
                        && next.control_context_generation == previous.control_context_generation
                    {
                        next.playback
                            .inherit_metronome_for_fresh_seek(&previous.playback);
                        next.playback
                            .cut_retained_foreign_gates_for_fresh_seek(&mut previous.playback);
                    }
                    if let Some(previous) =
                        self.active.as_mut().and_then(ActivePublication::legacy_mut)
                    {
                        next.playback.detach_rebound_inputs_from(
                            &mut previous.playback,
                            next.explicit_control_context
                                .then_some(next.control_context_generation),
                            previous
                                .explicit_control_context
                                .then_some(previous.control_context_generation),
                        );
                        previous.playback.cancel_computer_keyboard();
                        previous.playback.cancel_tuning_audition();
                    }
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallUnderStoppedArrangement(_)
                    ) && self.status.transport_epoch.load(Acquire)
                        != self.status.sampler_edit.ready_transport_epoch.load(Acquire)
                    {
                        // Priority Stop after the ready witness wins over the
                        // prepared stopped cursor, even during graph construction.
                        next.playback.apply(PlaybackCommand::Stop);
                    }
                    let revision = next.revision;
                    // A held EQ8 selector normally follows the current pair
                    // through certified parameter retargets. If an active
                    // instrument note forces its old processor chain into a
                    // release tail, transfer only the transient selector to
                    // the matching candidate pair at this no-more-refusals
                    // point. The tail retains its ordinary filter history.
                    if let Some(target) = self
                        .processor_audition_lease
                        .filter(|target| self.processor_audition.current() == Some(*target))
                    {
                        let previous_matches_target = self
                            .active
                            .as_ref()
                            .and_then(ActivePublication::legacy)
                            .is_some_and(|active| {
                                active.control_context_generation == target.project_generation
                                    && active.revision >= target.activation_floor
                                    && active.playback.sample_rate() == target.sample_rate_hz
                                    && active.playback.sample_rate() == self.rate
                            });
                        let candidate_matches_target = next.control_context_generation
                            == target.project_generation
                            && next.revision >= target.activation_floor
                            && next.playback.sample_rate() == target.sample_rate_hz
                            && next.playback.sample_rate() == self.rate;
                        if previous_matches_target && candidate_matches_target {
                            let _ = next.playback.transfer_processor_audition_from_release_tail(
                                target.identity,
                                target.selection,
                            );
                        }
                    }
                    // This is beyond every fallible handoff decision. Tail
                    // chains transferred into the candidate have no current
                    // command target, so remove transient auditions before
                    // the candidate's first callback render.
                    next.playback.cancel_retired_processor_auditions();
                    self.status.sampler_console.store(false, Release);
                    self.status
                        .session_capability
                        .store(next.playback.session_capability_status() as u32, Relaxed);
                    // Candidate ownership cannot enter this legacy replacement
                    // path; the queue decision above retains it in the same slot.
                    if matches!(
                        self.active,
                        Some(ActivePublication::Candidate(_) | ActivePublication::Eligible(_))
                    ) {
                        self.return_graph(next);
                        return;
                    }
                    let diagnostic_context = next.source_context;
                    let reset_audible = matches!(
                        next.policy,
                        PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                    );
                    let previous = self.active.replace(ActivePublication::Legacy(next));
                    if reset_audible {
                        self.audible_clock.clear();
                    }
                    self.release_current_quantum_pins();
                    // Publish actual inherited-tail requirements before the
                    // old graph can be destroyed on control and release its
                    // native references. No lock/allocation/owner drop occurs.
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    self.native_sources.refresh_runtime_requirements(
                        self.active
                            .as_ref()
                            .and_then(ActivePublication::legacy)
                            .map(|active| active.playback.as_ref()),
                    );
                    if let Some(ActivePublication::Legacy(previous)) = previous {
                        self.return_graph(previous);
                    }
                    self.finish_pending_stream_play();
                    if let Some(cancellation) = self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::legacy)
                        .and_then(|active| active.fresh_runtime_cancellation.as_ref())
                    {
                        cancellation.finish();
                    }
                    self.publish_active_routes(true);
                    self.recovery_seek_pending = None;
                    if self.recovery_position_frozen && !self.status.device_fault.load(Acquire) {
                        self.recovery_position_frozen = false;
                        self.publish_transport();
                    }
                    self.status.render_fault_record.clear();
                    self.status.render_fault.store(false, Release);
                    self.status.stream_source_changed.store(false, Release);
                    self.status.stream_read_failed.store(false, Release);
                    self.status.applied_source.publish(None);
                    self.status.applied_revision.store(revision, Release);
                    self.status.publication_pending.store(false, Release);
                    self.trace_audio(
                        AudioDiagnosticKind::PublicationApplied,
                        revision,
                        diagnostic_context,
                        frames.unwrap_or(0),
                    );
                }
            }
        }
    }

    fn apply_processor_audition_boundary(&mut self) {
        let Some(boundary) = self.processor_audition.take_boundary() else {
            return;
        };
        if let Some(previous) = boundary.previous
            && let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
        {
            if active.control_context_generation == previous.project_generation {
                if boundary.clear_mode == processor_audition::ClearMode::Immediate {
                    let _ = active.playback.cancel_processor_audition(previous.identity);
                } else {
                    let _ = active
                        .playback
                        .select_processor_audition(previous.identity, None);
                }
            } else if boundary.clear_mode == processor_audition::ClearMode::Immediate {
                active.playback.cancel_all_processor_auditions();
            }
        } else if boundary.clear_mode == processor_audition::ClearMode::Immediate
            && let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
        {
            // A prior smooth release already cleared its lease target, but its
            // fade may still be audible. A later hard lifecycle cancel must
            // clear that residual transient state too.
            active.playback.cancel_all_processor_auditions();
        }
        self.processor_audition_lease = None;
        self.processor_audition_rejected_epoch = 0;
    }

    fn reconcile_processor_audition(&mut self) {
        let epoch = self.processor_audition.current_epoch();
        let Some(target) = self.processor_audition.current() else {
            return;
        };
        if self.processor_audition_rejected_epoch == epoch {
            return;
        }
        let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        else {
            return;
        };
        if active.playback.sample_rate() != target.sample_rate_hz
            || target.sample_rate_hz != self.rate
        {
            active.playback.cancel_all_processor_auditions();
            self.processor_audition.reject_current(epoch);
            self.processor_audition_rejected_epoch = epoch;
            self.processor_audition_lease = None;
            return;
        }
        if active.control_context_generation < target.project_generation
            || active.revision < target.activation_floor
        {
            return;
        }
        if active.control_context_generation != target.project_generation {
            active.playback.cancel_all_processor_auditions();
            self.processor_audition.reject_current(epoch);
            self.processor_audition_rejected_epoch = epoch;
            self.processor_audition_lease = None;
            return;
        }
        if self.processor_audition_lease != Some(target) {
            if !active
                .playback
                .select_processor_audition(target.identity, Some(target.selection))
            {
                self.processor_audition.reject_current(epoch);
                self.processor_audition_rejected_epoch = epoch;
                self.processor_audition_lease = None;
                return;
            }
            self.processor_audition_lease = Some(target);
        }
        if !active
            .playback
            .processor_audition_is_active(target.identity, target.selection)
        {
            let _ = active.playback.cancel_processor_audition(target.identity);
            self.processor_audition.reject_current(epoch);
            self.processor_audition_rejected_epoch = epoch;
            self.processor_audition_lease = None;
        }
    }

    fn consume_commands(&mut self) {
        if self.streamed_transport_mode() {
            self.consume_stream_commands();
            self.acknowledge_sampler_edit();
            return;
        }
        // Capture the published queue prefix before clearing the marker. Later
        // producer admissions stay queued for the next block; a Stop arriving
        // after this swap also remains pending for the next block.
        let entry_ticket = self.status.admitted_ticket.load(Acquire);
        let stop_marker = self.take_stop_marker();
        self.consume_commands_through(command_cut(entry_ticket, stop_marker));
        self.finish_stop_marker(stop_marker);
        self.acknowledge_sampler_edit();
    }

    // Publish the user Stop's recovery cursor before clearing its marker.
    // A racing newer marker stays pending for the next block; no callback retry.
    fn take_stop_marker(&self) -> u64 {
        let marker = self.status.pending_stop_marker.load(Acquire);
        if marker != 0 {
            // This callback is the sole writer. The odd in-flight generation
            // is visible before clearing the marker, until Stop's transport
            // and route observations have been published. Saturation refuses
            // new fences permanently while priority Stop remains effective.
            let generation = self.status.consumed_stop_generation.load(Relaxed);
            self.status
                .consumed_stop_generation
                .store(generation.saturating_add(1), Release);
            // A reader may observe the cleared marker before Stop finishes.
            // Refuse any existing stopped witness before opening that window.
            self.status.sampler_edit.interrupt_for_stop();
            if !self.status.closing.load(Acquire) {
                self.status.recovery_position.store(0, Release);
            }
            let _ = self
                .status
                .pending_stop_marker
                .compare_exchange(marker, 0, AcqRel, Acquire);
        }
        marker
    }

    fn finish_stop_marker(&self, marker: u64) {
        if marker != 0 {
            let generation = self.status.consumed_stop_generation.load(Relaxed);
            self.status
                .consumed_stop_generation
                .store(generation.saturating_add(1), Release);
        }
    }

    fn trace_command_dequeued(&mut self, queued: QueuedCommand, cut: CommandCut) {
        self.trace_audio(
            AudioDiagnosticKind::CommandDequeued {
                ticket: queued.ticket,
                action: queued.action,
                discarded_by_stop: cut.stop_through.is_some_and(|stop| queued.ticket <= stop),
            },
            self.status.applied_revision.load(Relaxed),
            None,
            0,
        );
    }

    fn consume_commands_through(&mut self, cut: CommandCut) {
        self.consume_commands_through_inner(cut, false);
    }

    fn finish_action_outcome(&mut self, ticket: u64, outcome: action_outcomes::Outcome) {
        if let Some(writer) = self.action_writer.as_mut() {
            // A mismatch latches an observable failure and refuses further admission.
            let _ = writer.finish(ticket, outcome);
        }
    }

    fn stage_stream_outcome(&mut self, ticket: u64, outcome: action_outcomes::Outcome) {
        let Some(writer) = self.action_writer.as_ref() else {
            return;
        };
        let Some(slot) = self.stream_outcomes.get_mut(self.stream_outcome_count) else {
            writer.fail_invariant();
            return;
        };
        *slot = Some((ticket, outcome));
        self.stream_outcome_count += 1;
    }

    fn flush_stream_outcomes(&mut self) {
        if self.stream_outcome_count == 0 {
            return;
        }
        #[cfg(test)]
        if let Some(barrier) = self.test_before_action_outcome_status.take() {
            barrier.wait();
            barrier.wait();
        }
        self.publish_transport();
        self.publish_active_routes(false);
        let count = std::mem::take(&mut self.stream_outcome_count);
        for index in 0..count {
            if let Some((ticket, outcome)) = self.stream_outcomes[index].take() {
                self.finish_action_outcome(ticket, outcome);
            }
        }
    }

    fn consume_commands_through_inner(&mut self, cut: CommandCut, fault: bool) {
        // A publication can complete a streamed terminal before handing the
        // same block to resident consumption. Preserve completion order.
        self.flush_stream_outcomes();
        if cut.stop_through.is_some() {
            // The immediate priority Stop follows an interrupted fade's effects.
            self.settle_transport_fade();
        }
        if let Some(through_ticket) = cut.stop_through {
            if !fault && !self.status.closing.load(Acquire) {
                self.recovery_position_frozen = false;
                self.status.recovery_position.store(0, Relaxed);
            }
            if !fault {
                self.trace_audio(
                    AudioDiagnosticKind::PriorityStop { through_ticket },
                    self.status.applied_revision.load(Relaxed),
                    None,
                    0,
                );
                self.finish_midi_before_stop();
                self.revoke_output_transport();
                self.revoke_current_sampler_keyboard();
                #[cfg(all(test, feature = "allocation-probe"))]
                self.apply_candidate_action(PlaybackAction::Transport(PlaybackCommand::Stop));
                self.invalidate_gain_recording(
                    crate::gain_recording::AbortReason::TransportChanged,
                );
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::current_mut)
                {
                    active
                        .playback
                        .apply_priority_stop(&self.status.keyboard_activation);
                }
                self.invalidate_processor_audition_request();
            }
        }
        let mut consumed = None;
        let mut outcomes = [None; COMMANDS];
        for staged in &mut outcomes {
            if !self
                .commands
                .peek()
                .is_ok_and(|queued| queued.ticket <= cut.process_through)
            {
                break;
            }
            let Ok(queued) = self.commands.pop() else {
                break;
            };
            consumed = Some(queued.ticket);
            if !fault {
                self.trace_command_dequeued(queued, cut);
            }
            if !fault
                && cut.stop_through.is_none_or(|stop| queued.ticket > stop)
                && queued.project_generation.is_some_and(|generation| {
                    self.recording_source()
                        .is_none_or(|source| source.context.project_generation != generation)
                })
            {
                let reason = PlaybackActionRejection::ProjectChanged;
                self.status.publish_rejected_action(queued.ticket, reason);
                *staged = Some((queued.ticket, action_outcomes::Outcome::Rejected(reason)));
                continue;
            }
            if !fault
                && cut.stop_through.is_none_or(|stop| queued.ticket > stop)
                && queued
                    .expected_source
                    .is_some_and(|expected| self.recording_source() != Some(expected))
            {
                let reason = PlaybackActionRejection::AudioSourceChanged;
                self.status.publish_rejected_action(queued.ticket, reason);
                *staged = Some((queued.ticket, action_outcomes::Outcome::Rejected(reason)));
                continue;
            }
            // Each command follows the audible end of an earlier fade.
            self.settle_transport_fade();
            if fault && sampler_edit::is_stop(queued.action) {
                self.status.recovery_position.store(0, Release);
                *staged = Some((queued.ticket, action_outcomes::Outcome::Applied));
            } else if fault {
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                ));
            }
            if fault {
                continue;
            }
            if stale_output_start(&self.status, queued) {
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                ));
                continue;
            }
            if self.status.render_fault.load(Acquire) && starts_playback(queued.action) {
                if cut.stop_through.is_none_or(|stop| queued.ticket > stop)
                    && let Some(frame) = fault_cursor_intent(queued.action)
                {
                    self.status.recovery_position.store(frame, Release);
                    // An earlier applied Seek unfroze the cursor; keep this intent
                    // from being overwritten by the stopped graph's position.
                    self.recovery_position_frozen = true;
                }
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                ));
                continue;
            }
            if !self.status.sampler_edit.permits(queued) {
                self.status.publish_rejected_action(
                    queued.ticket,
                    PlaybackActionRejection::SamplerEditBusy,
                );
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::SamplerEditBusy),
                ));
                continue;
            }
            if cut
                .stop_through
                .is_none_or(|stop_through| queued.ticket > stop_through)
            {
                #[cfg(all(test, feature = "allocation-probe"))]
                self.apply_candidate_action(queued.action);
                let mut invalidate_audition = false;
                let mut fading = false;
                let outcome;
                let closing = self.status.closing.load(Acquire);
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::current_mut)
                {
                    let applied = if closing {
                        active.playback.try_apply_action(queued.action)
                    } else {
                        active.playback.try_apply_declicked_action(queued.action)
                    };
                    match applied {
                        Ok(()) => {
                            fading =
                                active.playback.pending_transport_fade() == Some(queued.action);
                            outcome = action_outcomes::Outcome::Applied;
                            if recovery_cursor_action(queued.action) {
                                self.recovery_position_frozen = false;
                            }
                            invalidate_audition =
                                action_invalidates_processor_audition(queued.action);
                        }
                        Err(error) => {
                            let reason = error.telemetry_rejection();
                            self.status.publish_rejected_action(queued.ticket, reason);
                            outcome = action_outcomes::Outcome::Rejected(reason);
                        }
                    }
                } else {
                    self.status.publish_rejected_action(
                        queued.ticket,
                        PlaybackActionRejection::NoActivePlayback,
                    );
                    outcome = action_outcomes::Outcome::Rejected(
                        PlaybackActionRejection::NoActivePlayback,
                    );
                }
                *staged = Some((queued.ticket, outcome));
                let stop = matches!(
                    queued.action,
                    PlaybackAction::Transport(PlaybackCommand::Stop)
                );
                if fading {
                    // The output transport, epoch and sampler keyboard stay
                    // live until the fade's audible end.
                    self.begin_transport_fade(queued.ticket, queued.action, false);
                } else if stop {
                    self.revoke_current_sampler_keyboard();
                }
                if invalidate_audition
                    || matches!(
                        queued.action,
                        PlaybackAction::Transport(PlaybackCommand::Pause)
                            | PlaybackAction::Session(hex_engine::SessionPlaybackCommand::Pause)
                    )
                {
                    self.invalidate_midi_recording(
                        crate::midi_recording::AbortReason::TransportChanged,
                    );
                    if fading {
                        self.abort_transport_inputs(
                            crate::gain_recording::AbortReason::TransportChanged,
                        );
                    } else {
                        self.invalidate_for_transport_change(queued.action);
                    }
                }
                if invalidate_audition {
                    self.invalidate_processor_audition_request();
                }
            } else {
                *staged = Some((queued.ticket, action_outcomes::Outcome::Superseded));
            }
        }
        #[cfg(test)]
        if let Some(barrier) = self.test_before_action_outcome_status.take() {
            barrier.wait();
            barrier.wait();
        }
        self.publish_transport();
        self.publish_active_routes(false);
        if let Some(ticket) = consumed {
            self.status.consumed_command.store(ticket, Release);
        }
        for (ticket, outcome) in outcomes.into_iter().flatten() {
            self.finish_action_outcome(ticket, outcome);
        }
    }
    /// Latch the first actual render cause before Stop changes the cursor.
    /// None denotes nonfinite output after successful rendering.
    /// Native adapters call this on the callback; no formatting, I/O or ownership drops.
    pub fn report_render_fault(&mut self, error: Option<&Invalid>, block_start_frame: u64) {
        if !self.status.render_fault.load(Acquire) {
            self.status.render_fault_record.publish(
                error.map_or("audio output contains nonfinite samples", |error| error.0),
                self.status.applied_revision.load(Relaxed),
                block_start_frame,
            );
            self.status.render_fault.store(true, Release);
        }
        self.stop_after_fault();
    }
    /// Stop immediately after a render fault; does not dispose graph ownership.
    pub fn stop_after_fault(&mut self) {
        // The fault Stop below revokes every transport effect immediately.
        self.transport_fade = None;
        self.recovery_position_frozen = true;
        if self.streamed_transport_mode() {
            self.cancel_stream_for_fault();
        }
        self.recovery_seek_pending = None;
        self.sample_preview.invalidate_on_fault();
        self.sample_recording
            .end(crate::sample_recording::End::OutputFailed);
        self.invalidate_midi_recording(crate::midi_recording::AbortReason::OutputFailed);
        self.revoke_output_transport();
        self.status.sampler_keyboard.revoke_on_callback();
        #[cfg(all(test, feature = "allocation-probe"))]
        self.apply_candidate_action(PlaybackAction::Transport(PlaybackCommand::Stop));
        self.invalidate_gain_recording(crate::gain_recording::AbortReason::OutputFailed);
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.apply(PlaybackCommand::Stop);
        }
        self.processor_audition.invalidate_current_immediately();
        self.invalidate_processor_audition_request();
        self.publish_transport();
        self.publish_active_routes(false);
        self.flush_stream_outcomes();
    }

    // The internal cut is never published as a priority Stop: that marker is
    // reserved for the user's explicit cursor-reset intent. Drain at most one
    // bounded command prefix per callback and leave graph custody unchanged.
    pub(crate) fn fence_device_fault(&mut self) {
        if self.device_fault_fenced_in_block || self.status.closing.load(Acquire) {
            return;
        }
        self.device_fault_fenced_in_block = true;
        let entry = self.status.admitted_ticket.load(Acquire);
        let user_stop = self.take_stop_marker();
        if !self.device_fault_stopped {
            self.device_fault_stopped = true;
            self.stop_after_fault();
        } else {
            // A begin that raced the error still receives its reserved terminal.
            self.sample_recording
                .end(crate::sample_recording::End::OutputFailed);
            self.invalidate_midi_recording(crate::midi_recording::AbortReason::OutputFailed);
            self.gain_recording
                .abort(crate::gain_recording::AbortReason::OutputFailed);
            self.invalidate_native_parameter_input(
                crate::native_parameter_input::AbortReason::OutputFailed,
            );
        }
        let through = entry.max(user_stop.saturating_sub(1));
        let cut = CommandCut {
            stop_through: Some(through),
            process_through: through,
        };
        if self.streamed_transport_mode() {
            self.consume_stream_commands_through(cut, true);
        } else {
            self.consume_commands_through_inner(cut, true);
        }
        self.finish_stop_marker(user_stop);
        self.acknowledge_sampler_edit();
        self.flush_stream_outcomes();
    }

    fn invalidate_processor_audition_request(&mut self) {
        if self.processor_audition.has_request_after_hard_cancel() {
            return;
        }
        let epoch = self.processor_audition.current_epoch();
        self.processor_audition.reject_current(epoch);
        self.processor_audition_rejected_epoch = epoch;
        self.processor_audition_lease = None;
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.cancel_all_processor_auditions();
        }
    }
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        let block = self.scheduled_transport();
        let result = self.render_inner(output, block);
        self.release_current_quantum_pins();
        if self.transport_fade.is_some() && !self.transport_fade_pending() {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if result.is_ok()
                && self.transport_fade_is_pause()
                && let Some((_, before, true)) = block
                && let Some((_, after, false)) = self.scheduled_transport()
                && let Ok(frame) = usize::try_from(after.saturating_sub(before))
                && frame < output.len()
            {
                self.native_sources.end_staging_at_pause(frame);
            }
            self.settle_transport_fade();
        }
        if let Ok(outcome) = &result
            && !outcome.audible_recorded
        {
            self.record_audible_run(outcome.start, output.len());
        }
        self.publish_audible_position();
        self.flush_stream_outcomes();
        result.map(|_| ())
    }

    fn record_audible_run(&mut self, block: Option<(u64, u64, bool)>, frames: usize) {
        if let Some((position, performance, true)) = block
            && frames != 0
        {
            // A declicked Pause/Stop can end inside the block; only the frames
            // its performance clock advanced were played (#4151).
            let frames = match self.scheduled_transport() {
                Some((_, after, false)) => usize::try_from(after.saturating_sub(performance))
                    .map_or(frames, |played| played.min(frames)),
                _ => frames,
            };
            if frames > 0 {
                self.audible_clock.push(position, performance, frames);
            }
        }
    }

    /// Scheduled position, performance frame and playing state of the current graph.
    fn scheduled_transport(&self) -> Option<(u64, u64, bool)> {
        self.active
            .as_ref()
            .and_then(ActivePublication::current)
            .map(|active| {
                (
                    active.playback.position(),
                    active.playback.performance_frame(),
                    active.playback.playing(),
                )
            })
    }

    fn publish_audible_position(&self) {
        if let Some(active) = self.active.as_ref().and_then(ActivePublication::current) {
            let playback = &active.playback;
            let (latency, region) = playback.audible_timing();
            self.status.audible_position.store(
                self.audible_clock.audible(
                    playback.position(),
                    playback.performance_frame(),
                    latency,
                    region,
                ),
                Relaxed,
            );
        }
    }
    /// Returns whether a split seek recorded its own suffix and the clock used
    /// for ordinary bookkeeping, adjusted for a pre-render streamed section.
    fn render_inner(
        &mut self,
        output: &mut [Frame],
        initial: Option<(u64, u64, bool)>,
    ) -> Result<RenderOutcome, Invalid> {
        if self.status.device_fault.load(Acquire) {
            if !self.status.closing.load(Acquire) && !self.device_fault_fenced_in_block {
                self.fence_device_fault();
            }
            output.fill([0.0; 2]);
            return Ok(RenderOutcome {
                audible_recorded: false,
                start: initial,
            });
        }
        let yield_successor = self.yield_stream_successor_once;
        let quantized_revision = self.deferred.as_ref().and_then(|next| {
            matches!(
                next.policy,
                PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
            )
            .then_some(next.revision)
        });
        if !output.is_empty() {
            self.yield_stream_successor_once = false;
        }
        if !yield_successor
            && !self.status.closing.load(Acquire)
            && self
                .deferred
                .as_ref()
                .is_some_and(|next| next.streams.is_some())
        {
            self.advance_legacy_publication(Some(output.len()));
        }

        if !yield_successor
            && !self.status.closing.load(Acquire)
            && self
                .deferred_eligible
                .as_ref()
                .is_some_and(|next| next.streaming_enabled())
        {
            self.advance_eligible_publication(Some(output.len()));
        }
        if !output.is_empty() {
            self.prepare_midi_recording_quantum(output.len());
        }
        // A replacement may clear the fault above. Until then, no failed graph
        // or its device chain is rendered, including while transport is stopped.
        // Take and live-input quanta still run so a racing take or gesture
        // receives its `OutputFailed` terminal instead of waiting on silence.
        if self.status.render_fault.load(Acquire) {
            self.sample_recording
                .end(crate::sample_recording::End::OutputFailed);
            if !output.is_empty() {
                self.prepare_gain_recording_quantum();
                self.prepare_native_parameter_quantum();
            }
            output.fill([0.0; 2]);
            return Ok(RenderOutcome {
                audible_recorded: false,
                start: initial,
            });
        }
        if self.output_transport.is_some() {
            self.sample_recording
                .end(crate::sample_recording::End::UnsupportedOutput);
        } else {
            self.sample_recording.begin(output.len());
        }
        // A ready streamed section can install above, before any frames
        // render. Only that transition replaces the original block start.
        let render_start = if quantized_revision.is_some_and(|revision| {
            self.active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .is_some_and(|active| active.revision == revision)
        }) {
            self.scheduled_transport()
        } else {
            initial
        };
        let quantized = if self.status.closing.load(Acquire) {
            section_launch::QuantizedSeekRender::NotHandled
        } else {
            self.render_quantized_seek(output)?
        };
        if matches!(quantized, section_launch::QuantizedSeekRender::NotHandled) {
            self.render_project(output, 0)?;
        }
        self.report_sampler_keyboard_rejection();
        if !self.status.closing.load(Acquire) {
            self.sample_preview.mix(output);
        }
        Ok(RenderOutcome {
            audible_recorded: matches!(quantized, section_launch::QuantizedSeekRender::Switched),
            start: render_start,
        })
    }

    fn render_project(
        &mut self,
        output: &mut [Frame],
        capture_offset: usize,
    ) -> Result<(), Invalid> {
        if self.native_effects_revoked() {
            output.fill([0.0; 2]);
            return if self.status.playing.load(Relaxed) {
                Err(Invalid("native effect callback is revoked"))
            } else {
                Ok(())
            };
        }
        self.read_sampler_keyboard();
        if let Some(ActivePublication::Candidate(active)) = self.active.as_mut() {
            if self.status.closing.load(Acquire) {
                output.fill([0.0; 2]);
                return Ok(());
            }
            #[cfg(all(test, feature = "allocation-probe"))]
            {
                active.owner.test_render(output)?;
                self.publish_transport();
                return Ok(());
            }
            #[cfg(not(all(test, feature = "allocation-probe")))]
            {
                let _ = active;
                return Err(Invalid("candidate admission is unavailable"));
            }
        }

        if !output.is_empty() {
            self.prepare_gain_recording_quantum();
            self.prepare_native_parameter_quantum();
        }
        self.reconcile_processor_audition();
        self.begin_project_spectrum(output.len());
        let active = self.active.as_ref().and_then(ActivePublication::current);
        let targets = self
            .spectrum_capture
            .as_ref()
            .and(self.spectrum_target.current_targets());
        self.spectrum_stages.begin(
            targets,
            self.spectrum_target.current_epoch(),
            output.len(),
            |target| {
                active.as_ref().is_some_and(|active| {
                    active.control_context_generation == target.project_generation
                        && active.revision >= target.activation_floor
                        && active.playback.sample_rate() == target.sample_rate_hz
                        && target.sample_rate_hz == self.rate
                })
            },
        );
        let node_observed = self.spectrum_stages.observing();
        let project_observed = self
            .spectrum_capture
            .as_ref()
            .and_then(|c| c.project.as_ref())
            .is_some_and(|p| p.observing());
        let recording_source = self.recording_source();
        if self.sample_recording.selection().is_some_and(|layout| {
            self.status.closing.load(Acquire)
                || layout
                    .expected_source
                    .is_some_and(|source| Some(source) != recording_source)
                || active.as_ref().is_none_or(|active| {
                    active.control_context_generation != layout.generation
                        || active.playback.sample_rate() != self.rate
                        || !active.playback.project_sample_sources_supported(layout)
                })
        }) {
            self.sample_recording
                .end(crate::sample_recording::End::SourceChanged);
        }
        let mut recording = self.sample_recording.observation();
        if !node_observed && !project_observed && recording.is_none() {
            return self.render_unobserved(output, capture_offset);
        }
        if self.status.closing.load(Acquire) {
            output.fill([0.0; 2]);
            return Ok(());
        }
        let stages = &mut self.spectrum_stages;
        let targets = self.spectrum_target.current_targets();
        let mut observe = |identity, phase, offset, frames: &[Frame]| {
            stages.observe_signal(targets, identity, phase, offset, frames)
        };
        let mut project_capture = self
            .spectrum_capture
            .as_mut()
            .and_then(|c| c.project.as_mut());
        let selection = project_capture
            .as_ref()
            .and_then(|p| p.selection())
            .filter(|_| project_observed);
        let (record_tracks, returns) = recording
            .as_ref()
            .map_or((&[][..], &[][..]), |(layout, _)| {
                (layout.tracks.as_ref(), layout.returns.as_ref())
            });
        let spectrum_tracks = selection.as_ref().map_or(&[][..], |s| s.tracks());
        let mut joined = [0; crate::sample_recording::TRACKS];
        let tracks =
            crate::sample_recording::merge_tracks(spectrum_tracks, record_tracks, &mut joined);
        // A failed selection merge must invalidate recording even when its
        // timing-only layout no longer relies on a Main PCM observation.
        if project_observed
            && tracks.is_none()
            && let Some((layout, stage)) = recording.as_mut()
        {
            stage.observe(layout, hex_engine::ProjectSignal::Unavailable, 0, &[]);
        }
        let recording_observed = recording.is_some();
        let timing_only_recording = recording
            .as_ref()
            .is_some_and(|(layout, _)| layout.timing_only);
        let recording = std::cell::RefCell::new(recording);
        let mut observe_project = |signal, offset, frames: &[Frame]| {
            if let Some(project) = project_capture.as_mut()
                && match signal {
                    hex_engine::ProjectSignal::Track(id) => {
                        spectrum_tracks.binary_search(&id).is_ok()
                    }
                    hex_engine::ProjectSignal::Return(_) => false,
                    _ => true,
                }
            {
                project.observe(signal, offset, frames);
            }
            if recording_observed
                && let Ok(mut recording) = recording.try_borrow_mut()
                && let Some((layout, stage)) = recording.as_mut()
            {
                stage.observe(layout, signal, capture_offset + offset, frames);
            }
        };
        let mut observe_timing = |offset, start, frames, map: &hex_engine::PreparedTempoMap| {
            if let Ok(mut recording) = recording.try_borrow_mut()
                && let Some((_, stage)) = recording.as_mut()
            {
                stage.observe_timing(capture_offset + offset, start, frames, map);
            }
        };
        let mut project = if timing_only_recording && !project_observed {
            Some(hex_engine::ProjectOutputTap::timing_only(
                &mut observe_timing,
                &mut observe_project,
            ))
        } else {
            tracks
                .filter(|_| project_observed || recording_observed)
                .and_then(|tracks| {
                    let tap = hex_engine::ProjectOutputTap::with_returns(
                        tracks,
                        returns,
                        &mut observe_project,
                    )?;
                    Some(if recording_observed {
                        tap.with_timing(&mut observe_timing)
                    } else {
                        tap
                    })
                })
        };
        let node: Option<&mut hex_engine::NodeSignalObserver<'_>> = if node_observed {
            Some(&mut observe)
        } else {
            None
        };
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let has_endpoints = self.native_sources.has_endpoints();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let mut foreign_window = NativeEffectWindow::new(&mut self.native_sources, capture_offset);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let foreign: Option<&mut dyn hex_lattice::ForeignProcessors> =
            has_endpoints.then_some(&mut foreign_window);
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let foreign = None;
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
        {
            let mut outcome = active.render_streamed_with_foreign(
                &mut self.domain_pages,
                self.stream_pending_play.is_some(),
                self.stream_prefix,
                output,
                node,
                project.as_mut(),
                foreign,
            );
            let action_pending = active.has_stream_action();
            let capacity_conflict = active.capacity_conflict();
            streaming::count_starvation(
                &self.status,
                self.stream_pending_play.is_some(),
                action_pending,
                &outcome,
            );
            self.resolve_recovery_seek(action_pending, outcome.action_rejection);
            if capacity_conflict && self.yield_pending_stream_successor() {
                outcome.readiness_refused = false;
            }
            let streaming::RecordedStreamOutcome {
                applied_play,
                applied_action,
                rejected_action,
                fault,
            } = streaming::record_outcome(
                &self.status,
                &mut self.stream_pending_play,
                self.stream_prefix,
                action_pending,
                outcome,
            );
            if let Some((ticket, reason)) = rejected_action {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Rejected(reason));
            }
            if let Some(ticket) = applied_action {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            if let Some(ticket) = applied_play {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            if let Some(error) = fault {
                return Err(error);
            }
        } else if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.render_with_signal_observers(
                output,
                node,
                project.as_mut(),
                foreign,
            )?;
        } else {
            output.fill([0.0; 2]);
        }
        self.publish_transport();
        self.publish_active_routes(false);
        Ok(())
    }

    fn render_unobserved(
        &mut self,
        output: &mut [Frame],
        capture_offset: usize,
    ) -> Result<(), Invalid> {
        if self.status.closing.load(Acquire) {
            output.fill([0.0; 2]);
            return Ok(());
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let has_endpoints = self.native_sources.has_endpoints();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let mut foreign_window = NativeEffectWindow::new(&mut self.native_sources, capture_offset);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let foreign: Option<&mut dyn hex_lattice::ForeignProcessors> =
            has_endpoints.then_some(&mut foreign_window);
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let foreign = {
            let _ = capture_offset;
            None
        };
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
        {
            let mut outcome = active.render_streamed_with_foreign(
                &mut self.domain_pages,
                self.stream_pending_play.is_some(),
                self.stream_prefix,
                output,
                None,
                None,
                foreign,
            );
            let action_pending = active.has_stream_action();
            let capacity_conflict = active.capacity_conflict();
            streaming::count_starvation(
                &self.status,
                self.stream_pending_play.is_some(),
                action_pending,
                &outcome,
            );
            self.resolve_recovery_seek(action_pending, outcome.action_rejection);
            if capacity_conflict && self.yield_pending_stream_successor() {
                outcome.readiness_refused = false;
            }
            let streaming::RecordedStreamOutcome {
                applied_play,
                applied_action,
                rejected_action,
                fault,
            } = streaming::record_outcome(
                &self.status,
                &mut self.stream_pending_play,
                self.stream_prefix,
                action_pending,
                outcome,
            );
            self.publish_transport();
            self.publish_active_routes(false);
            if let Some((ticket, reason)) = rejected_action {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Rejected(reason));
            }
            if let Some(ticket) = applied_action {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            if let Some(ticket) = applied_play {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            if let Some(error) = fault {
                return Err(error);
            }
            return Ok(());
        }
        if self.active.is_none()
            && (self
                .deferred
                .as_ref()
                .is_some_and(|next| next.streams.is_some())
                || self
                    .deferred_eligible
                    .as_ref()
                    .is_some_and(|next| next.streaming_enabled()))
        {
            output.fill([0.0; 2]);
            let fault = streaming::record_outcome(
                &self.status,
                &mut self.stream_pending_play,
                self.stream_prefix,
                false,
                crate::candidate_custody::StreamOutcome {
                    progress: crate::candidate_custody::StreamProgress::Buffering {
                        committed_frames: 0,
                    },
                    play_applied: false,
                    readiness_refused: false,
                    source_changed: false,
                    read_failed: false,
                    fault: None,
                    action_rejection: None,
                    action_applied: None,
                    activated: None,
                },
            )
            .fault;
            if let Some(error) = fault {
                return Err(error);
            }
            return Ok(());
        }
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active
                .playback
                .render_with_signal_observers(output, None, None, foreign)?;
        } else {
            output.fill([0.0; 2]);
        }
        self.publish_transport();
        self.publish_active_routes(false);
        Ok(())
    }

    fn begin_project_spectrum(&mut self, frames: usize) {
        let selection = self.status.project_spectrum.read().filter(|s| s.active);
        let active = self.active.as_ref().and_then(ActivePublication::current);
        let Some(project) = self
            .spectrum_capture
            .as_mut()
            .and_then(|c| c.project.as_mut())
        else {
            return;
        };
        let valid = match (selection, active) {
            (Some(selection), Some(active))
                if selection.sample_rate_hz == self.rate
                    && active.playback.sample_rate() == self.rate
                    && active.control_context_generation == selection.generation
                    && active.revision >= selection.activation_floor =>
            {
                if project.needs_source_validation(selection, active.revision) {
                    if active.playback.project_output_supported(selection.tracks()) {
                        project.source_validated(selection, active.revision);
                        true
                    } else {
                        false
                    }
                } else {
                    true
                }
            }
            _ => false,
        };
        project.begin(selection, frames, valid);
    }
    fn finish_project_spectrum(
        &mut self,
        captured_at: Instant,
        output_valid: bool,
    ) -> Option<crate::spectrum_worker::CaptureOutcome> {
        let selection = self
            .spectrum_capture
            .as_ref()
            .and_then(|c| c.project.as_ref())
            .and_then(|p| p.selection())?;
        let active = self.active.as_ref().and_then(ActivePublication::current);
        let revision = active.as_ref().map_or(0, |active| active.revision);
        let valid = output_valid
            && !self.status.closing.load(Acquire)
            && self.status.project_spectrum.token() == selection.token
            && selection.sample_rate_hz == self.rate
            && active.is_some_and(|active| {
                active.control_context_generation == selection.generation
                    && active.playback.sample_rate() == self.rate
                    && active.revision >= selection.activation_floor
            });
        self.spectrum_capture
            .as_mut()?
            .project
            .as_mut()?
            .finish(captured_at, revision, valid)
    }
    fn finish_analysis_block(
        &mut self,
        captured_start: Instant,
        output_valid: bool,
    ) -> (
        Option<crate::spectrum_worker::CaptureOutcome>,
        Option<crate::spectrum_worker::CaptureOutcome>,
    ) {
        let project = self.finish_project_spectrum(captured_start, output_valid);
        let node = self.finish_eq_spectrum(captured_start, output_valid);
        (node, project)
    }

    /// Accept or reject the staged post-node signal after the caller has
    /// validated the complete final output block. A skipped attempt consumes a
    /// per-target sequence so the next accepted window cannot bridge the gap.
    /// Finalizes every target; the return value describes the first active slot
    /// for single-target callers. Native delivery does not depend on this value.
    pub fn finish_spectrum_block(
        &mut self,
        captured_start: Instant,
        output_valid: bool,
    ) -> Option<crate::spectrum_worker::CaptureOutcome> {
        self.finish_analysis_block(captured_start, output_valid).0
    }

    pub(crate) fn set_sample_recording_output_clock(
        &mut self,
        mut clock: Option<crate::sample_recording::OutputClock>,
    ) {
        if let Some(clock) = &mut clock {
            clock.graph_latency_frames = self
                .active
                .as_ref()
                .and_then(ActivePublication::current)
                .and_then(|active| active.playback.main_output_latency_frames());
        }
        self.sample_recording.set_output_clock(clock);
    }

    pub(crate) fn finish_sample_recording(&mut self, output_valid: bool) {
        let recording_source = self.recording_source();
        if output_valid
            && self.sample_recording.selection().is_some_and(|layout| {
                self.status.closing.load(Acquire)
                    || layout
                        .expected_source
                        .is_some_and(|source| Some(source) != recording_source)
                    || self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::current)
                        .is_none_or(|active| {
                            active.control_context_generation != layout.generation
                                || !active.playback.project_sample_sources_supported(layout)
                        })
            })
        {
            self.sample_recording
                .end(crate::sample_recording::End::SourceChanged);
            return;
        }
        self.sample_recording.finish(output_valid);
    }
    fn finish_eq_spectrum(
        &mut self,
        captured_start: Instant,
        output_valid: bool,
    ) -> Option<crate::spectrum_worker::CaptureOutcome> {
        let active = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .map(|active| {
                (
                    active.control_context_generation,
                    active.revision,
                    active.playback.sample_rate(),
                )
            });
        let mut first = None;
        for (slot, stage) in self.spectrum_stages.iter() {
            let Some(target) = stage.target else {
                continue;
            };
            let valid = output_valid
                && stage.complete()
                && self
                    .spectrum_target
                    .current_targets()
                    .and_then(|targets| targets.get(slot))
                    == Some(target)
                && self.spectrum_target.current_epoch() == stage.epoch
                && active.is_some_and(|(generation, revision, rate)| {
                    generation == target.project_generation
                        && revision >= target.activation_floor
                        && rate == target.sample_rate_hz
                })
                && target.sample_rate_hz == self.rate;
            let outcome = self.spectrum_capture.as_mut().map(|capture| {
                if valid {
                    capture.submit_pair_for_slot_at(
                        slot,
                        SpectrumStamp {
                            target: target.token,
                            generation: target.project_generation,
                            sample_rate_hz: target.sample_rate_hz,
                            applied_revision: active.map_or(0, |(_, revision, _)| revision),
                        },
                        &stage.input_frames[..stage.expected],
                        &stage.frames[..stage.expected],
                        captured_start,
                    )
                } else {
                    capture.skip_slot(slot)
                }
            });
            if first.is_none() {
                first = outcome;
            }
        }
        self.spectrum_stages.clear();
        first
    }

    fn invalidate_gain_recording(&mut self, reason: crate::gain_recording::AbortReason) {
        self.revoke_native_effects();
        self.invalidate_transport_recording(reason);
    }

    fn invalidate_for_transport_change(&mut self, action: PlaybackAction) {
        let reason = crate::gain_recording::AbortReason::TransportChanged;
        if is_pause_action(action) {
            // Pause retains the active graph for monitoring, release tails
            // and resume. The native endpoints belong to that same graph.
            self.invalidate_transport_recording(reason);
        } else {
            self.invalidate_gain_recording(reason);
        }
    }

    fn invalidate_transport_recording(&mut self, reason: crate::gain_recording::AbortReason) {
        // Reuse the existing transport-discontinuity owner/epoch; asynchronous
        // output must not expose pre-seek/pause/stop buffers in a new context.
        self.revoke_forwarded_output_transport();
        self.abort_transport_inputs(reason);
        let prior_epoch = self
            .status
            .transport_epoch
            .fetch_update(AcqRel, Acquire, |epoch| epoch.checked_add(1));
        if matches!(reason, crate::gain_recording::AbortReason::OutputFailed) {
            self.status
                .last_output_fault_epoch
                .store(prior_epoch.unwrap_or_else(|epoch| epoch), Release);
        }
        if prior_epoch.is_err() {
            self.status.gain_recording_fault.store(true, Release);
        }
    }
    /// Abort the gain take and native parameter gesture without revoking the
    /// output transport or advancing its epoch. A declicked Pause/Stop does
    /// this when consumed; the rest of `invalidate_gain_recording` follows at
    /// its fade end.
    fn abort_transport_inputs(&mut self, reason: crate::gain_recording::AbortReason) {
        if matches!(reason, crate::gain_recording::AbortReason::TransportChanged) {
            self.sample_recording.end_bound_for_transport_change();
        }
        self.gain_recording.abort(reason);
        self.invalidate_native_parameter_input(match reason {
            crate::gain_recording::AbortReason::SourceChanged => {
                crate::native_parameter_input::AbortReason::SourceChanged
            }
            crate::gain_recording::AbortReason::TransportChanged => {
                crate::native_parameter_input::AbortReason::TransportChanged
            }
            crate::gain_recording::AbortReason::OutputFailed => {
                crate::native_parameter_input::AbortReason::OutputFailed
            }
            crate::gain_recording::AbortReason::Cancelled => {
                crate::native_parameter_input::AbortReason::Cancelled
            }
            crate::gain_recording::AbortReason::ClockExhausted => {
                crate::native_parameter_input::AbortReason::ClockExhausted
            }
        });
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.clear_gain_recording_override();
        }
    }
    fn invalidate_native_parameter_input(
        &mut self,
        reason: crate::native_parameter_input::AbortReason,
    ) {
        self.native_parameter_input.abort(reason);
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.invalidate_native_parameter_input();
        }
    }
    fn prepare_native_parameter_quantum(&mut self) {
        use crate::native_parameter_input::AbortReason;
        let source = self.status.recording_source();
        let transport_epoch = self.status.transport_epoch.load(Acquire);
        let unsupported_output = self.output_transport.is_some();
        let status = &self.status;
        let active = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut);
        let had_input = self.native_parameter_input.has_input();
        let live = self.native_parameter_input.prepare(|target| {
            if status.render_fault.load(Acquire)
                || target.transport_epoch <= status.last_output_fault_epoch.load(Acquire)
            {
                return Err(AbortReason::OutputFailed);
            }
            if status.native_parameter_input_revoked(target.gesture_serial) {
                return Err(AbortReason::Cancelled);
            }
            if unsupported_output {
                return Err(AbortReason::UnsupportedOutput);
            }
            let active = active.as_ref().ok_or(AbortReason::SourceChanged)?;
            if source != Some(target.source)
                || active.revision != target.source.audio_revision
                || active.control_context_generation != target.source.context.project_generation
                || active.playback.native_parameter_input_target() != Some(target.input)
            {
                return Err(AbortReason::SourceChanged);
            }
            if transport_epoch != target.transport_epoch || !active.playback.playing() {
                return Err(AbortReason::TransportChanged);
            }
            Ok(())
        });
        if let Some(mut active) = active {
            if let Some(operation) = live {
                if active
                    .playback
                    .set_native_parameter_input(operation)
                    .is_err()
                {
                    self.native_parameter_input
                        .abort(AbortReason::TargetUnavailable);
                    active.playback.invalidate_native_parameter_input();
                }
            } else if had_input {
                active.playback.invalidate_native_parameter_input();
            }
        }
    }
    /// Final checked host write, not physical loudspeaker delivery. Both Set
    /// and Clear wait for the actual processed sample plus remaining Main delay.
    pub fn finish_native_parameter_quantum(&mut self, valid: bool) {
        use crate::native_parameter_input::AbortReason;
        let invalidated = self.native_parameter_input.owner().and_then(|target| {
            if self.status.closing.load(Acquire)
                || self
                    .status
                    .native_parameter_input_revoked(target.gesture_serial)
            {
                Some(AbortReason::Cancelled)
            } else if self.status.pending_stop_marker.load(Acquire) != 0
                || self.status.transport_epoch.load(Acquire) != target.transport_epoch
            {
                Some(AbortReason::TransportChanged)
            } else if self.status.recording_source() != Some(target.source) {
                Some(AbortReason::SourceChanged)
            } else {
                None
            }
        });
        if let Some(reason) = invalidated {
            self.invalidate_native_parameter_input(reason);
            return;
        }
        let cancelled = self.native_parameter_input.owner().is_some_and(|target| {
            self.status
                .native_parameter_input_revoked(target.gesture_serial)
        });
        let mut active = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut);
        let application = active
            .as_mut()
            .and_then(|active| active.playback.take_native_parameter_application());
        let end = active
            .as_ref()
            .map_or(0, |active| active.playback.performance_frame());
        let had_input = self.native_parameter_input.has_input();
        let cleared =
            self.native_parameter_input
                .finish_quantum(application, end, valid, cancelled);
        if had_input
            && !self.native_parameter_input.has_input()
            && let Some(active) = &mut active
        {
            if cleared {
                active.playback.finish_native_parameter_input();
            } else {
                active.playback.invalidate_native_parameter_input();
            }
        }
        if self.native_parameter_input.receipt_fault {
            self.status
                .native_parameter_input_fault
                .store(true, Release);
        }
    }
    fn prepare_gain_recording_quantum(&mut self) {
        use crate::gain_recording::{AbortReason, Clock};
        let transport_epoch = self.status.transport_epoch.load(Acquire);
        let status = &self.status;
        let active = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut);
        let live = self.gain_recording.prepare(|target| {
            if status.render_fault.load(Acquire)
                || target.transport_epoch <= status.last_output_fault_epoch.load(Acquire)
            {
                return Err(AbortReason::OutputFailed);
            }
            let active = active.as_ref().ok_or(AbortReason::SourceChanged)?;
            if active.revision != target.audio_revision
                || active.control_context_generation != target.project_generation
                || active.playback.sample_rate() != target.sample_rate
                || !active.playback.gain_recording_ready(target.track_id)
            {
                return Err(AbortReason::SourceChanged);
            }
            if transport_epoch != target.transport_epoch || !active.playback.playing() {
                return Err(AbortReason::TransportChanged);
            }
            let (content_frame, delivery_delay, loop_pass) = active
                .playback
                .gain_recording_clock(target.track_id)
                .ok_or(AbortReason::SourceChanged)?;
            Ok(Clock {
                performance_frame: active.playback.performance_frame(),
                content_frame,
                delivery_delay,
                loop_pass,
            })
        });
        if let Some(mut active) = active
            && active.playback.set_gain_recording_input(live).is_err()
        {
            active.playback.clear_gain_recording_override();
            self.gain_recording.abort(AbortReason::SourceChanged);
        }
    }
    /// Confirm the exact converted output quantum; this is sink delivery, not
    /// physical loudspeaker playback. Failure discards the entire pending take.
    pub fn finish_gain_recording_quantum(&mut self, output_valid: bool) {
        if !output_valid
            && let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
        {
            active.playback.clear_gain_recording_override();
        }
        let end = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .map_or(0, |active| active.playback.performance_frame());
        self.gain_recording.finish_quantum(end, output_valid);
        if self.gain_recording.receipt_fault {
            self.status.gain_recording_fault.store(true, Release);
        }
    }

    fn publish_active_routes(&mut self, force: bool) {
        let Some(active) = self.active.as_ref().and_then(ActivePublication::current) else {
            return;
        };
        let revision = active.revision;
        let playback = &active.playback;
        let generation = playback.session_route_state_generation();
        let key = (revision, generation, playback.session_clock_anchor());
        if !force && self.published_route_key == Some(key) {
            return;
        }

        self.status
            .sampler_console
            .store(playback.sampler_console_available(), Release);
        let capability = playback.session_capability_status();
        self.status
            .session_capability
            .store(capability as u32, Relaxed);
        let count = if capability == SessionCapabilityStatus::Available {
            playback.session_route_count()
        } else {
            0
        };
        self.status.begin_route_snapshot();
        for index in 0..MAX_TRACKS {
            #[cfg(test)]
            {
                self.route_slot_visits = self.route_slot_visits.saturating_add(1);
            }
            if index < count {
                if let Some(route) = playback.session_route_status(index) {
                    self.status.publish_route_slot(index, route);
                } else if let Some(slot) = self.status.route_slots.get(index) {
                    slot.clear();
                }
            } else if let Some(slot) = self.status.route_slots.get(index) {
                slot.clear();
            }
        }
        self.status.publish_route_header(
            revision,
            count,
            capability,
            generation,
            playback.performance_frame(),
            playback.session_clock_anchor(),
        );
        self.status.end_route_snapshot();
        self.published_route_key = Some(key);
    }
    fn publish_transport(&self) {
        let observation =
            self.active
                .as_ref()
                .and_then(ActivePublication::current)
                .and_then(|active| {
                    active.playback.transport().map(
                        |(position, performance, playing, loop_state)| TransportObservation {
                            audio_revision: active.revision,
                            control_generation: active.control_context_generation,
                            consumed_command_ticket: self.status.consumed_command.load(Acquire),
                            arrangement_frame: position,
                            performance_frame: performance,
                            playing,
                            loop_state,
                        },
                    )
                });
        self.status.transport_observation.publish(observation);
        self.status
            .recording_source
            .publish(self.recording_source());
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let native_revision = self
            .native_sources
            .ordinary_runtime_requires_sources()
            .then(|| {
                self.active
                    .as_ref()
                    .and_then(ActivePublication::legacy)
                    .map(|active| active.revision)
            })
            .flatten()
            .unwrap_or(0);
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let native_revision = 0;
        self.status
            .ordinary_native_revision
            .store(native_revision, Release);
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(ActivePublication::Candidate(active)) = self.active.as_ref() {
            if let Some((position, frame, playing)) = active.owner.test_transport() {
                self.status.position.store(position, Relaxed);
                if !self.recovery_position_frozen {
                    self.status.recovery_position.store(position, Relaxed);
                }
                self.status.performance_frame.store(frame, Relaxed);
                self.status.playing.store(playing, Relaxed);
            }
            return;
        }
        let ready =
            self.active
                .as_ref()
                .and_then(ActivePublication::current)
                .is_some_and(|active| {
                    active.playback.playing()
                        && active.playback.gain_recording_track().is_some_and(|track| {
                            active.playback.gain_recording_clock(track).is_some()
                        })
                });
        self.status.gain_recording_ready.store(ready, Release);
        if let Some(active) = self.active.as_ref().and_then(ActivePublication::current) {
            let position = active.playback.position();
            self.status.position.store(position, Relaxed);
            self.publish_audible_position();
            if !self.recovery_position_frozen {
                self.status.recovery_position.store(position, Relaxed);
            }
            self.status
                .performance_frame
                .store(active.playback.performance_frame(), Relaxed);
            self.status
                .playing
                .store(active.playback.playing(), Relaxed);
            self.status
                .metronome_enabled
                .store(active.playback.metronome_enabled(), Relaxed);
        }
    }
}

/// Known channel payload/capacity quote for the pinned worker factory.
pub(crate) fn worker_boundary_storage_quote() -> Option<usize> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let editor_bytes = crate::foreign_parameter_delivery::editor::constructor_storage_quote()?;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    let editor_bytes = 0;
    std::mem::size_of::<Status>()
        .checked_add(action_outcomes::storage_bytes())?
        .checked_add(SESSION_ROUTE_CAPACITY.checked_mul(std::mem::size_of::<AtomicRouteSlot>())?)?
        .checked_add(crate::sample_preview::constructor_storage_quote()?)?
        .checked_add(crate::sample_recording::storage_quote()?)?
        .checked_add(crate::gain_recording::constructor_storage_quote()?)?
        .checked_add(crate::native_parameter_input::constructor_storage_quote()?)?
        .checked_add(crate::foreign_parameter_delivery::constructor_storage_quote()?)?
        .checked_add(editor_bytes)?
        .checked_add(crate::midi_recording::constructor_storage_quote()?)?
        .checked_add(spectrum_target::constructor_storage_quote()?)?
        .checked_add(processor_audition::constructor_storage_quote()?)?
        .checked_add(endpoints::constructor_endpoint_storage_quote()?)?
        .checked_add(SpectrumStages::storage_quote()?)
}
pub(crate) fn worker_publication_queue_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    ChannelStorage::ring_bytes::<QueuedPublication>(1)?
        .checked_add(ChannelStorage::ring_bytes::<ReturnedPublication>(RETIRED)?)?
        .checked_add(ChannelStorage::ring_bytes::<QueuedCommand>(COMMANDS)?)?
        .checked_add(ChannelStorage::ring_bytes::<AudioDiagnosticEvent>(
            audio_diagnostics::TRACE_CAPACITY,
        )?)?
        .checked_add(crate::native_callback_owner::worker_boundary_storage_quote()?)
}

#[cfg(all(test, feature = "allocation-probe"))]
#[path = "prepared/diagnostics_tests.rs"]
mod diagnostics_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, Weak};

    #[test]
    fn prepared_channel_accepts_only_supported_sample_rates() {
        for rate in [0, 7_999, 192_001, u32::MAX] {
            assert!(channel(rate).is_err(), "accepted {rate} hz");
        }
        for rate in [8_000, 192_000] {
            assert!(channel(rate).is_ok(), "rejected {rate} hz");
        }
    }

    fn spectrum_test_target(
        owner: hex_engine::ProcessorChainOwner,
        project_generation: u64,
        activation_floor: u64,
        sample_rate_hz: u32,
    ) -> spectrum_target::SpectrumTarget {
        spectrum_target::SpectrumTarget {
            token: crate::spectrum_worker::SpectrumTargetId(1),
            identity: PostNodeIdentity::new(
                owner,
                11,
                hex_engine::DeviceTypeId::from_static("hex.eq8"),
            ),
            project_generation,
            activation_floor,
            sample_rate_hz,
        }
    }

    fn spectrum_eq8_graph() -> (Playback, PostNodeIdentity) {
        spectrum_eq8_graph_with_playing(true)
    }

    fn fresh_spectrum_eq8_graph() -> (Playback, PostNodeIdentity) {
        spectrum_eq8_graph_with_playing(false)
    }

    fn spectrum_eq8_graph_with_playing(playing: bool) -> (Playback, PostNodeIdentity) {
        spectrum_eq8_graph_with_mode(playing, 0)
    }

    fn spectrum_eq8_graph_with_mode(playing: bool, mode: u32) -> (Playback, PostNodeIdentity) {
        spectrum_eq8_graph_with_params(playing, mode, 0.0, 1.0)
    }

    pub(super) fn spectrum_eq8_graph_with_params(
        playing: bool,
        mode: u32,
        gain_db: f32,
        q: f32,
    ) -> (Playback, PostNodeIdentity) {
        spectrum_eq_graph_with_params(playing, mode, gain_db, q, false, false)
    }

    pub(super) fn spectrum_bus_graph() -> (Playback, [PostNodeIdentity; 3]) {
        let (playback, track) = spectrum_eq_graph_with_params(true, 0, 12.0, 1.0, false, true);
        (
            playback,
            [
                track,
                PostNodeIdentity::new(
                    hex_engine::ProcessorChainOwner::Return(3),
                    12,
                    track.device_type,
                ),
                PostNodeIdentity::new(hex_engine::ProcessorChainOwner::Main, 13, track.device_type),
            ],
        )
    }

    fn spectrum_eq_graph_with_params(
        playing: bool,
        mode: u32,
        gain_db: f32,
        q: f32,
        flexible: bool,
        effect_buses: bool,
    ) -> (Playback, PostNodeIdentity) {
        use hex_lattice_contract::DeviceTypeId;
        use hex_project::{
            AssetSnapshot, ClipFade, ClipSnapshot, DeviceSnapshot, MeterPoint,
            Project as CanonicalProject, ReturnTrackSnapshot, SessionSnapshot, Snapshot, Tempo,
            TempoPoint, TrackContentSnapshot, TrackSnapshot,
        };

        const DEVICE_ID: u64 = 11;
        const TRACK_ID: u64 = 1;
        const ASSET_ID: u64 = 4;
        const CLIP_ID: u64 = 5;
        const FRAMES: u64 = 512;
        let mut state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.eq8"),
        )
        .unwrap();
        // The audition contract intentionally refuses disabled bands. Enable
        // band zero in both stereo banks so this prepared callback fixture
        // exercises the admitted path rather than only waiting/rejection.
        for parameter in [1, 41] {
            state = hex_engine::patch_native_device_parameter(
                &state,
                hex_lattice_contract::ParameterId::from_static(parameter),
                hex_lattice_contract::ParameterValue::Bool(true),
            )
            .unwrap();
        }
        state = hex_engine::patch_native_device_parameter(
            &state,
            hex_lattice_contract::ParameterId::from_static(201),
            hex_lattice_contract::ParameterValue::Choice(mode),
        )
        .unwrap();
        state = hex_engine::patch_native_device_parameter(
            &state,
            hex_lattice_contract::ParameterId::from_static(4),
            hex_lattice_contract::ParameterValue::Float(gain_db),
        )
        .unwrap();
        state = hex_engine::patch_native_device_parameter(
            &state,
            hex_lattice_contract::ParameterId::from_static(5),
            hex_lattice_contract::ParameterValue::Float(q),
        )
        .unwrap();
        let mut return_state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.eq8"),
        )
        .unwrap();
        for parameter in [1, 41] {
            return_state = hex_engine::patch_native_device_parameter(
                &return_state,
                hex_lattice_contract::ParameterId::from_static(parameter),
                hex_lattice_contract::ParameterValue::Bool(true),
            )
            .unwrap();
        }
        return_state = hex_engine::patch_native_device_parameter(
            &return_state,
            hex_lattice_contract::ParameterId::from_static(201),
            hex_lattice_contract::ParameterValue::Choice(mode),
        )
        .unwrap();
        if flexible {
            use hex_engine::eq::{
                AutomationLane, AutomationPoint, EqState, Node, NodeId, NodeParameter,
            };
            let mut settings = EqState::default();
            for index in 0..64 {
                let id = NodeId::new(1000 + index).unwrap();
                let mut node =
                    Node::new(id, 500.0, if index == 0 { gain_db } else { 0.0 }, 0).unwrap();
                node.q = q;
                settings.add(node).unwrap();
                settings
                    .set_lane(AutomationLane {
                        node: id,
                        parameter: NodeParameter::Frequency,
                        points: vec![
                            AutomationPoint {
                                beat: 0,
                                normalized: 0.2,
                            },
                            AutomationPoint {
                                beat: 4 << 32,
                                normalized: 0.8,
                            },
                        ],
                    })
                    .unwrap();
            }
            state = settings.encode().unwrap();
            return_state = state.clone();
        }
        let project = CanonicalProject::from_legacy_snapshot(Snapshot {
            automatic_track_names: Vec::new(),
            retained_source_paths: Vec::new(),
            aliases: Default::default(),
            native_sources: Vec::new(),
            arrangement_sections: Vec::new(),
            arrangement_locators: Vec::new(),
            main_devices: if effect_buses {
                vec![DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: DEVICE_ID + 2,
                    bypassed: false,
                    state: state.clone(),
                }]
            } else {
                Vec::new()
            },
            tuning: Default::default(),
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            sidechains: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            sample_rate_mode: Default::default(),
            next_id: DEVICE_ID + if effect_buses { 3 } else { 2 },
            assets: vec![AssetSnapshot {
                original_source: None,
                id: ASSET_ID,
                path: "analyzer-test.wav".into(),
                source_sample_rate: 48_000,
                source_frames: FRAMES,
                playback_frames: FRAMES,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            }],
            tracks: vec![TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: TRACK_ID,
                name: "analyzer fixture".into(),
                gain: 1.0,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: vec![DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: DEVICE_ID,
                    bypassed: false,
                    state,
                }],
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                    active: true,
                    name: "clip".into(),
                    flavor: hex_project::ClipFlavor::default(),
                    edit: None,
                    gain: hex_project::ClipGain::UNITY,
                    id: CLIP_ID,
                    asset: ASSET_ID,
                    start: 0,
                    duration: FRAMES,
                    source_offset: 0,
                    source_length: FRAMES,
                    looping: true,
                    source_phase: 0,
                    fade: ClipFade::neutral(FRAMES).unwrap(),
                }]),
                sends: if effect_buses {
                    vec![hex_project::SendLevelSnapshot {
                        return_track: 3,
                        gain: 0.5,
                    }]
                } else {
                    Vec::new()
                },
            }],
            return_tracks: vec![ReturnTrackSnapshot {
                id: 3,
                name: "return fixture".into(),
                gain: 1.0,
                muted: false,
                devices: vec![DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: DEVICE_ID + 1,
                    bypassed: false,
                    state: return_state,
                }],
            }],
            session: SessionSnapshot::default(),
            loop_region: if effect_buses {
                Some(hex_project::LoopRegion::new(0, FRAMES, true).unwrap())
            } else {
                None
            },
            master_gain: 1.0,
            tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
            meter_points: vec![MeterPoint::new(
                hex_project::BeatTime::ZERO,
                hex_project::Meter::COMMON_TIME,
            )],
        })
        .unwrap();
        let asset = project.assets().next().unwrap().id();
        let audio = hex_engine::Audio::new(48_000, vec![[0.25, -0.4]; FRAMES as usize]).unwrap();
        let media = [(asset, audio)].into_iter().collect();
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &media,
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        if playing {
            playback.apply(PlaybackCommand::Play);
        }
        (
            playback,
            PostNodeIdentity::new(
                hex_engine::ProcessorChainOwner::Track(TRACK_ID),
                DEVICE_ID,
                DeviceTypeId::from_static(if flexible { "hex.eq" } else { "hex.eq8" }),
            ),
        )
    }

    #[cfg(feature = "allocation-probe")]
    fn saturator_track_and_return_graph(drive_db: f32) -> Playback {
        use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
        let make_state = || {
            let mut state = hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                DeviceTypeId::from_static("hex.saturator"),
            )
            .expect("registered saturator state");
            state = hex_engine::patch_native_device_parameter(
                &state,
                ParameterId::from_static(1),
                ParameterValue::Float(drive_db),
            )
            .expect("saturator drive");
            hex_engine::patch_native_device_parameter(
                &state,
                ParameterId::from_static(4),
                ParameterValue::Choice(1),
            )
            .expect("saturator curve")
        };
        native_effect_track_and_return_graph(make_state())
    }

    #[cfg(feature = "allocation-probe")]
    fn native_effect_track_and_return_graph(
        state: hex_lattice_contract::NativeDeviceState,
    ) -> Playback {
        use hex_project::{
            AssetSnapshot, ClipFade, ClipSnapshot, DeviceSnapshot, MeterPoint,
            Project as CanonicalProject, ReturnTrackSnapshot, SendLevelSnapshot, SessionSnapshot,
            Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
        };

        const TRACK_ID: u64 = 1;
        const ASSET_ID: u64 = 4;
        const CLIP_ID: u64 = 5;
        const TRACK_DEVICE_ID: u64 = 6;
        const RETURN_ID: u64 = 7;
        const RETURN_DEVICE_ID: u64 = 8;
        const FRAMES: u64 = 512;
        let make_state = || state.clone();
        let project = CanonicalProject::from_legacy_snapshot(Snapshot {
            automatic_track_names: Vec::new(),
            retained_source_paths: Vec::new(),
            aliases: Default::default(),
            native_sources: Vec::new(),
            arrangement_sections: Vec::new(),
            arrangement_locators: Vec::new(),
            main_devices: Vec::new(),
            tuning: Default::default(),
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            sidechains: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            sample_rate_mode: Default::default(),
            next_id: RETURN_DEVICE_ID + 1,
            assets: vec![AssetSnapshot {
                original_source: None,
                id: ASSET_ID,
                path: "saturator-callback-test.wav".into(),
                source_sample_rate: 48_000,
                source_frames: FRAMES,
                playback_frames: FRAMES,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            }],
            tracks: vec![TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: TRACK_ID,
                name: "saturator callback fixture".into(),
                gain: 1.0,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: vec![DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: TRACK_DEVICE_ID,
                    bypassed: false,
                    state: make_state(),
                }],
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                    active: true,
                    name: "clip".into(),
                    flavor: hex_project::ClipFlavor::default(),
                    edit: None,
                    gain: hex_project::ClipGain::UNITY,
                    id: CLIP_ID,
                    asset: ASSET_ID,
                    start: 0,
                    duration: FRAMES,
                    source_offset: 0,
                    source_length: FRAMES,
                    looping: true,
                    source_phase: 0,
                    fade: ClipFade::neutral(FRAMES).expect("neutral fade"),
                }]),
                sends: vec![SendLevelSnapshot {
                    return_track: RETURN_ID,
                    gain: 0.5,
                }],
            }],
            return_tracks: vec![ReturnTrackSnapshot {
                id: RETURN_ID,
                name: "saturator return fixture".into(),
                gain: 0.75,
                muted: false,
                devices: vec![DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: RETURN_DEVICE_ID,
                    bypassed: false,
                    state: make_state(),
                }],
            }],
            session: SessionSnapshot::default(),
            loop_region: None,
            master_gain: 1.0,
            tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
            meter_points: vec![MeterPoint::new(
                hex_project::BeatTime::ZERO,
                hex_project::Meter::COMMON_TIME,
            )],
        })
        .expect("valid track and Return graph");
        let audio = hex_engine::Audio::new(48_000, vec![[0.25, -0.4]; FRAMES as usize])
            .expect("audio fixture");
        let media = [(project.assets().next().expect("asset").id(), audio)]
            .into_iter()
            .collect();
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &media,
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .expect("prepared track and Return chain"),
        );
        Playback::try_from_prepared_with_session(revision).expect("playback")
    }

    #[cfg(feature = "allocation-probe")]
    fn compressor_track_and_return_graph(threshold_db: f32) -> Playback {
        use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
        use hex_project::{
            AssetSnapshot, ClipFade, ClipSnapshot, DeviceSnapshot, MeterPoint,
            Project as CanonicalProject, ReturnTrackSnapshot, SendLevelSnapshot, SessionSnapshot,
            Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
        };

        const TRACK_ID: u64 = 1;
        const ASSET_ID: u64 = 4;
        const CLIP_ID: u64 = 5;
        const TRACK_DEVICE_ID: u64 = 6;
        const RETURN_ID: u64 = 7;
        const RETURN_DEVICE_ID: u64 = 8;
        const FRAMES: u64 = 512;
        let make_state = || {
            let state = hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                DeviceTypeId::from_static("hex.compressor"),
            )
            .expect("registered compressor state");
            hex_engine::patch_native_device_parameter(
                &state,
                ParameterId::from_static(1),
                ParameterValue::Float(threshold_db),
            )
            .expect("compressor threshold")
        };
        let project = CanonicalProject::from_legacy_snapshot(Snapshot {
            automatic_track_names: Vec::new(),
            retained_source_paths: Vec::new(),
            aliases: Default::default(),
            native_sources: Vec::new(),
            arrangement_sections: Vec::new(),
            arrangement_locators: Vec::new(),
            main_devices: Vec::new(),
            tuning: Default::default(),
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            sidechains: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            sample_rate_mode: Default::default(),
            next_id: RETURN_DEVICE_ID + 1,
            assets: vec![AssetSnapshot {
                original_source: None,
                id: ASSET_ID,
                path: "compressor-callback-test.wav".into(),
                source_sample_rate: 48_000,
                source_frames: FRAMES,
                playback_frames: FRAMES,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            }],
            tracks: vec![TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: TRACK_ID,
                name: "compressor callback fixture".into(),
                gain: 1.0,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: vec![DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: TRACK_DEVICE_ID,
                    bypassed: false,
                    state: make_state(),
                }],
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                    active: true,
                    name: "clip".into(),
                    flavor: hex_project::ClipFlavor::default(),
                    edit: None,
                    gain: hex_project::ClipGain::UNITY,
                    id: CLIP_ID,
                    asset: ASSET_ID,
                    start: 0,
                    duration: FRAMES,
                    source_offset: 0,
                    source_length: FRAMES,
                    looping: true,
                    source_phase: 0,
                    fade: ClipFade::neutral(FRAMES).expect("neutral fade"),
                }]),
                sends: vec![SendLevelSnapshot {
                    return_track: RETURN_ID,
                    gain: 0.5,
                }],
            }],
            return_tracks: vec![ReturnTrackSnapshot {
                id: RETURN_ID,
                name: "compressor return fixture".into(),
                gain: 0.75,
                muted: false,
                devices: vec![DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: RETURN_DEVICE_ID,
                    bypassed: false,
                    state: make_state(),
                }],
            }],
            session: SessionSnapshot::default(),
            loop_region: None,
            master_gain: 1.0,
            tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
            meter_points: vec![MeterPoint::new(
                hex_project::BeatTime::ZERO,
                hex_project::Meter::COMMON_TIME,
            )],
        })
        .expect("valid compressor track and Return graph");
        let audio = hex_engine::Audio::new(48_000, vec![[0.25, -0.4]; FRAMES as usize])
            .expect("audio fixture");
        let media = [(project.assets().next().expect("asset").id(), audio)]
            .into_iter()
            .collect();
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &media,
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .expect("prepared compressor track and Return chain"),
        );
        Playback::try_from_prepared_with_session(revision).expect("playback")
    }

    #[cfg(feature = "allocation-probe")]
    fn gate_track_and_return_graph(floor_db: f32) -> Playback {
        use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
        let make_state = || {
            let mut state = hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                DeviceTypeId::from_static("hex.gate"),
            )
            .expect("registered gate state");
            state = hex_engine::patch_native_device_parameter(
                &state,
                ParameterId::from_static(1),
                ParameterValue::Float(0.0),
            )
            .expect("gate threshold");
            state = hex_engine::patch_native_device_parameter(
                &state,
                ParameterId::from_static(2),
                ParameterValue::Float(-6.0),
            )
            .expect("gate return");
            hex_engine::patch_native_device_parameter(
                &state,
                ParameterId::from_static(3),
                ParameterValue::Float(floor_db),
            )
            .expect("gate floor")
        };
        native_track_and_return_graph(Some(make_state()))
    }

    #[cfg(feature = "allocation-probe")]
    fn native_track_and_return_graph(
        state: Option<hex_lattice_contract::NativeDeviceState>,
    ) -> Playback {
        use hex_project::{
            AssetSnapshot, ClipFade, ClipSnapshot, DeviceSnapshot, MeterPoint,
            Project as CanonicalProject, ReturnTrackSnapshot, SendLevelSnapshot, SessionSnapshot,
            Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
        };

        const TRACK_ID: u64 = 1;
        const ASSET_ID: u64 = 4;
        const CLIP_ID: u64 = 5;
        const TRACK_DEVICE_ID: u64 = 6;
        const RETURN_ID: u64 = 7;
        const RETURN_DEVICE_ID: u64 = 8;
        const FRAMES: u64 = 512;
        let project = CanonicalProject::from_legacy_snapshot(Snapshot {
            automatic_track_names: Vec::new(),
            retained_source_paths: Vec::new(),
            aliases: Default::default(),
            native_sources: Vec::new(),
            arrangement_sections: Vec::new(),
            arrangement_locators: Vec::new(),
            main_devices: Vec::new(),
            tuning: Default::default(),
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            sidechains: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            sample_rate_mode: Default::default(),
            next_id: RETURN_DEVICE_ID + 1,
            assets: vec![AssetSnapshot {
                original_source: None,
                id: ASSET_ID,
                path: "gate-callback-test.wav".into(),
                source_sample_rate: 48_000,
                source_frames: FRAMES,
                playback_frames: FRAMES,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            }],
            tracks: vec![TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: TRACK_ID,
                name: "gate callback fixture".into(),
                gain: 1.0,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: state
                    .as_ref()
                    .map(|state| DeviceSnapshot {
                        modulation: Default::default(),
                        parent: None,
                        automation: Vec::new(),
                        comparison: None,
                        id: TRACK_DEVICE_ID,
                        bypassed: false,
                        state: state.clone(),
                    })
                    .into_iter()
                    .collect(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                    active: true,
                    name: "clip".into(),
                    flavor: hex_project::ClipFlavor::default(),
                    edit: None,
                    gain: hex_project::ClipGain::UNITY,
                    id: CLIP_ID,
                    asset: ASSET_ID,
                    start: 0,
                    duration: FRAMES,
                    source_offset: 0,
                    source_length: FRAMES,
                    looping: true,
                    source_phase: 0,
                    fade: ClipFade::neutral(FRAMES).expect("neutral fade"),
                }]),
                sends: vec![SendLevelSnapshot {
                    return_track: RETURN_ID,
                    gain: 0.5,
                }],
            }],
            return_tracks: vec![ReturnTrackSnapshot {
                id: RETURN_ID,
                name: "gate return fixture".into(),
                gain: 0.75,
                muted: false,
                devices: state
                    .as_ref()
                    .map(|state| DeviceSnapshot {
                        modulation: Default::default(),
                        parent: None,
                        automation: Vec::new(),
                        comparison: None,
                        id: RETURN_DEVICE_ID,
                        bypassed: false,
                        state: state.clone(),
                    })
                    .into_iter()
                    .collect(),
            }],
            session: SessionSnapshot::default(),
            loop_region: None,
            master_gain: 1.0,
            tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
            meter_points: vec![MeterPoint::new(
                hex_project::BeatTime::ZERO,
                hex_project::Meter::COMMON_TIME,
            )],
        })
        .expect("valid track and Return Gate graph");
        let audio = hex_engine::Audio::new(48_000, vec![[0.25, -0.4]; FRAMES as usize])
            .expect("audio fixture");
        let media = [(project.assets().next().expect("asset").id(), audio)]
            .into_iter()
            .collect();
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &media,
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .expect("prepared Gate track and Return chain"),
        );
        Playback::try_from_prepared_with_session(revision).expect("playback")
    }

    #[cfg(feature = "allocation-probe")]
    fn compressor_routed_reference(
        track_threshold_db: f64,
        return_threshold_db: f64,
        track_history: &mut CompressorReferenceHistory,
        return_history: &mut CompressorReferenceHistory,
        input: &[[f32; 2]],
    ) -> Vec<[f32; 2]> {
        let process =
            |frame: [f32; 2], threshold_db: f64, history: &mut CompressorReferenceHistory| {
                let peak = f64::from(frame[0]).abs().max(f64::from(frame[1]).abs());
                let alpha = (-1.0_f64 / (0.010 * 48_000.0)).exp();
                history.envelope = alpha * history.envelope + (1.0 - alpha) * peak;
                let level_db = if history.envelope == 0.0 {
                    -120.0
                } else {
                    20.0 * history.envelope.log10()
                };
                let over = level_db - threshold_db;
                let slope = 0.25 - 1.0;
                let reduction_db = if over <= -3.0 {
                    0.0
                } else if over >= 3.0 {
                    slope * over
                } else {
                    slope * (over + 3.0).powi(2) / 12.0
                };
                let gain_alpha = (-1.0_f64 / (0.005 * 48_000.0)).exp();
                history.gain_db = gain_alpha * history.gain_db + (1.0 - gain_alpha) * reduction_db;
                let gain = 10.0_f64.powf(history.gain_db / 20.0);
                [
                    (f64::from(frame[0]) * gain) as f32,
                    (f64::from(frame[1]) * gain) as f32,
                ]
            };
        input
            .iter()
            .copied()
            .map(|frame| {
                let direct = process(frame, track_threshold_db, track_history);
                let send_input = [direct[0] * 0.5, direct[1] * 0.5];
                let returned = process(send_input, return_threshold_db, return_history);
                [
                    direct[0] + returned[0] * 0.75,
                    direct[1] + returned[1] * 0.75,
                ]
            })
            .collect()
    }

    #[cfg(feature = "allocation-probe")]
    #[derive(Default)]
    struct CompressorReferenceHistory {
        envelope: f64,
        gain_db: f64,
    }

    pub(super) fn metronome_graph() -> Playback {
        let mut project = hex_project::Project::new(8_000).unwrap().snapshot();
        project.loop_region = Some(hex_project::LoopRegion::new(0, 4_050, true).unwrap());
        let revision = hex_engine::PreparedRevision::prepare(
            &hex_project::Project::from_snapshot(project).unwrap(),
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap();
        Playback::from_prepared(Arc::new(revision))
    }

    #[test]
    fn metronome_admission_is_reported_and_priority_stop_discards_older_toggle() {
        let (mut control, mut callback) = channel(8_000).unwrap();
        control.publish(metronome_graph()).unwrap();
        callback.begin_block();
        let ticket = control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        assert!(!control.status.metronome_enabled.load(Relaxed));
        callback.begin_block();
        assert!(control.status.metronome_enabled.load(Relaxed));
        assert!(control.status.consumed_command.load(Acquire) >= ticket);
        control
            .action_with_ticket(PlaybackAction::SetMetronome(false))
            .unwrap();
        control.stop();
        callback.begin_block();
        assert!(control.status.metronome_enabled.load(Relaxed));
        assert!(!control.status.playing.load(Relaxed));
        control
            .action_with_ticket(PlaybackAction::SetMetronome(false))
            .unwrap();
        callback.begin_block();
        assert!(!control.status.metronome_enabled.load(Relaxed));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    fn metronome_prerolled_seek(
        revision: Arc<hex_engine::PreparedRevision>,
        destination: u64,
    ) -> Playback {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let runtime = hex_engine::SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                    revision, destination, destination, true, &AtomicBool::new(false),
                ).unwrap();
                Playback::from_interactive_arrangement_preroll(runtime).unwrap()
            }).unwrap().join().unwrap()
    }

    #[test]
    fn metronome_fresh_seek_preserves_only_monitoring_after_accepted_exact_source_fence() {
        for (same_source, same_context, conditional, enabled) in [
            (true, true, true, true),
            (false, true, true, false),
            (true, false, true, false),
            (true, true, false, false),
        ] {
            let (old, _) = fresh_spectrum_eq8_graph();
            let old_source = Arc::clone(old.prepared_revision().unwrap());
            assert!(old_source.arrangement_seek_requires_preroll());
            let source = if same_source {
                Arc::clone(&old_source)
            } else {
                Arc::clone(fresh_spectrum_eq8_graph().0.prepared_revision().unwrap())
            };
            let mut oracle = metronome_prerolled_seek(Arc::clone(&source), 200);
            let mut expected = [[0.0; 2]; 128];
            oracle.render(&mut expected).unwrap();
            let candidate = metronome_prerolled_seek(source, 200);
            assert_eq!(
                Arc::ptr_eq(candidate.prepared_revision().unwrap(), &old_source),
                same_source
            );
            assert!(!candidate.metronome_enabled());
            let (mut control, mut callback) = channel(48_000).unwrap();
            control
                .publish_with_policy_and_context_generation(
                    old,
                    PublicationPolicy::InstallFreshRuntime,
                    12,
                )
                .unwrap();
            control
                .action_with_ticket(PlaybackAction::SetMetronome(true))
                .unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 37]).unwrap();
            assert!(control.status.metronome_enabled.load(Relaxed));
            let fence = control.capture_fresh_runtime_fence().unwrap();
            let policy = if conditional {
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence)
            } else {
                PublicationPolicy::InstallFreshRuntime
            };
            let revision = control
                .publish_with_policy_and_context_generation(
                    candidate,
                    policy,
                    if same_context { 12 } else { 13 },
                )
                .unwrap();
            let mut output = [[0.0; 2]; 128];
            let render = || {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            };
            #[cfg(feature = "allocation-probe")]
            {
                let allocations = allocation_counter::measure(render);
                assert_eq!(
                    (
                        allocations.count_total,
                        allocations.bytes_total,
                        allocations.count_current,
                        allocations.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
            }
            #[cfg(not(feature = "allocation-probe"))]
            {
                let mut render = render;
                render();
            }
            assert_eq!(control.status.applied_revision.load(Acquire), revision);
            assert_eq!(control.status.metronome_enabled.load(Relaxed), enabled);
            assert_eq!(control.status.position.load(Relaxed), 328);
            assert_eq!(control.status.performance_frame.load(Relaxed), 328);
            assert_eq!(
                output, expected,
                "fresh DSP/transport history and a reset click tail must match the preroll oracle"
            );
            control.request_shutdown();
            callback.begin_block();
            control.reclaim();
        }
    }

    #[test]
    fn metronome_rejected_fresh_seek_keeps_active_toggle_and_position() {
        for cancelled in [false, true] {
            let (old, _) = fresh_spectrum_eq8_graph();
            let source = Arc::clone(old.prepared_revision().unwrap());
            let candidate = metronome_prerolled_seek(source, 200);
            let (mut control, mut callback) = channel(48_000).unwrap();
            let active_revision = control
                .publish_with_policy_and_context_generation(
                    old,
                    PublicationPolicy::InstallFreshRuntime,
                    12,
                )
                .unwrap();
            control
                .action_with_ticket(PlaybackAction::SetMetronome(true))
                .unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 37]).unwrap();
            let fence = control.capture_fresh_runtime_fence().unwrap();
            let rejected = control
                .publish_with_policy_and_context_generation(
                    candidate,
                    PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                    12,
                )
                .unwrap();
            if cancelled {
                assert_eq!(
                    control.cancel_fresh_runtime_publication(rejected),
                    FreshRuntimeCancellationResult::Cancelled
                );
            } else {
                control.command(PlaybackCommand::Pause).unwrap();
            }
            callback.begin_block();
            assert_eq!(
                control.status.applied_revision.load(Acquire),
                active_revision
            );
            assert_eq!(
                control.status.rejected_revision(),
                Some((
                    rejected,
                    if cancelled {
                        PlaybackHandoffRejection::FreshRuntimeCancelled
                    } else {
                        PlaybackHandoffRejection::RuntimeFenceChanged
                    }
                ))
            );
            assert!(control.status.metronome_enabled.load(Relaxed));
            assert_eq!(control.status.position.load(Relaxed), 37);
            // A queued Pause is Applied when consumed but keeps playing
            // through its fade until a render reaches its audible end (#4151).
            assert!(control.status.playing.load(Relaxed));
            control.request_shutdown();
            callback.begin_block();
            control.reclaim();
        }
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn clip_gain_with_shaped_fades_and_replacement_allocates_and_frees_nothing_in_callback() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let hex_project::Created::Imported { asset, clip, .. } = editor
            .execute(hex_project::Command::ImportAudioTrack {
                name: "clip gain".into(),
                path: "gain.wav".into(),
                sample_rate: 48_000,
                frames: 1_024,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let fade = hex_project::ClipFade::new(1_024, 0, 64, 64)
            .unwrap()
            .with_shape(4, 4, 700, -700)
            .unwrap()
            .with_curve_positions(250, 750)
            .unwrap()
            .with_curve_laws([
                hex_project::ClipFade::DEFAULT_S_CURVE,
                hex_project::FadeCurveLaw::Linear,
            ]);
        editor
            .execute(hex_project::Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        let audio = hex_engine::Audio::new(48_000, vec![[0.5, -0.25]; 1_024]).unwrap();
        let media = std::collections::BTreeMap::from([(asset, audio)]);
        let prepare = |editor: &hex_project::Editor| {
            let revision = hex_engine::PreparedRevision::prepare(
                editor.project(),
                &media,
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap();
            Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap()
        };
        editor
            .execute(hex_project::Command::SetAudioClipGain {
                clip,
                millidb: -6_000,
            })
            .unwrap();
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(prepare(&editor)).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 128];
        let allocations = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut prefix).unwrap();
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let gain = hex_project::ClipGain::new(-6_000).unwrap().linear();
        assert_eq!(prefix[0], [0.0; 2]);
        assert_eq!(
            prefix[32],
            [
                0.5 * fade.gain_at(32) * gain,
                -0.25 * fade.gain_at(32) * gain
            ]
        );
        assert_eq!(prefix[68], [0.5 * gain, -0.25 * gain]);
        editor
            .execute(hex_project::Command::SetAudioClipGain {
                clip,
                millidb: 6_000,
            })
            .unwrap();
        control.publish(prepare(&editor)).unwrap();
        let mut output = [[0.0; 2]; 128];
        let allocations = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let gain = hex_project::ClipGain::new(6_000).unwrap().linear();
        assert_eq!(output, [[0.5 * gain, -0.25 * gain]; 128]);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn metronome_commands_loops_and_handoff_allocate_and_free_nothing_in_callback() {
        let (mut control, mut callback) = channel(8_000).unwrap();
        control.publish(metronome_graph()).unwrap();
        callback.begin_block();
        control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 128];
        let allocations = allocation_counter::measure(|| {
            callback.begin_block();
            for _ in 0..65 {
                callback.render(&mut output).unwrap();
            }
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(control.status.metronome_enabled.load(Relaxed));
        control.publish(metronome_graph()).unwrap();
        control.command(PlaybackCommand::Seek(0)).unwrap();
        let allocations = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        // The Seek applies after its 21-frame fade at 8 kHz, where the
        // undamped metronome click at frame zero follows (#4140).
        assert_eq!(output[21], [0.16; 2]);
        control.command(PlaybackCommand::Pause).unwrap();
        let allocations = allocation_counter::measure(|| {
            // The first block is the queued Pause's 128-frame fade (#4151).
            for _ in 0..2 {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            }
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(output, [[0.0; 2]; 128]);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn held_mutable_arrangement_loop_handoff_preserves_pcm_without_callback_allocation() {
        for (old_enabled, new_enabled, block_frames) in
            [(true, true, 256), (false, true, 17), (true, false, 1)]
        {
            check_held_arrangement_loop_handoff(old_enabled, new_enabled, block_frames);
        }
    }

    #[cfg(feature = "allocation-probe")]
    fn check_held_arrangement_loop_handoff(
        old_enabled: bool,
        new_enabled: bool,
        block_frames: usize,
    ) {
        use hex_project::{BeatTime, Command, Created, Editor, Project};
        let class = hex_engine::NativeDeviceClass::Instrument;
        let state = hex_engine::default_native_device_state_for_class(
            class,
            hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        let state = hex_engine::patch_native_device_parameter_for_class(
            class,
            &state,
            hex_lattice_contract::ParameterId::from_static(78),
            hex_lattice_contract::ParameterValue::Float(0.4),
        )
        .unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "loop probe".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state,
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(10).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ONE,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        // Leave room for the native instrument's conservative release bound.
        editor
            .execute(Command::SetLoopRegion {
                start: 48_000,
                end: 96_000_000,
                enabled: old_enabled,
            })
            .unwrap();
        let prepare = |project: &Project| {
            Playback::from_prepared(Arc::new(
                hex_engine::PreparedRevision::prepare(
                    project,
                    &std::collections::BTreeMap::new(),
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                )
                .unwrap(),
            ))
        };
        let mut reference = prepare(editor.project());
        reference.apply(PlaybackCommand::Play);
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(prepare(editor.project())).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut prefix = vec![[0.0; 2]; 30_000];
        for chunk in prefix.chunks_mut(256) {
            callback.begin_block();
            callback.render(chunk).unwrap();
        }
        let mut expected_prefix = prefix.clone();
        reference.render(&mut expected_prefix).unwrap();
        assert_eq!(prefix, expected_prefix);
        editor
            .execute(Command::SetLoopRegion {
                start: 24_000,
                end: 120_000_000,
                enabled: new_enabled,
            })
            .unwrap();
        let revision = control.publish(prepare(editor.project())).unwrap();
        let mut actual = vec![[0.0; 2]; block_frames * 3];
        let measured = allocation_counter::measure(|| {
            for chunk in actual.chunks_mut(block_frames) {
                callback.begin_block();
                callback.render(chunk).unwrap();
            }
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.applied_revision.load(Relaxed), revision);
        assert_eq!(control.status.rejected_revision(), None);
        let mut expected = vec![[0.0; 2]; actual.len()];
        for chunk in expected.chunks_mut(block_frames) {
            reference.render(chunk).unwrap();
        }
        assert!(expected.iter().flatten().any(|sample| sample.abs() > 1e-6));
        assert_eq!(actual, expected, "held source must keep its DSP history");
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn saturator_track_and_return_revision_admission_and_render_allocate_nothing() {
        let (mut control, mut callback) = channel(48_000).expect("channel");
        let playback = saturator_track_and_return_graph(36.0);
        control.publish(playback).expect("publish");
        control.command(PlaybackCommand::Play).expect("play");
        callback.begin_block();
        let mut original_output = [[0.0; 2]; 128];
        callback
            .render(&mut original_output)
            .expect("initial callback render");
        // Each oversampled Saturator answers a step with exact DC once its
        // declared latency and finite tail have elapsed. Main waits for the
        // return path, two Saturators deep; pre-epoch output is silent.
        let saturator = hex_devices::saturator::SaturatorDefinition::prepare_owned(
            &hex_lattice_contract::NativeDeviceMetadata::default_state(
                &hex_devices::saturator::SATURATOR_DEFINITION,
            )
            .expect("default saturator"),
            48_000,
            &[],
        )
        .expect("standalone saturator");
        let latency = hex_lattice::PreparedNativeProcessor::algorithmic_latency_frames(&saturator)
            .expect("declared latency") as usize;
        let hex_lattice::OutputTail::Finite { max_tail_frames } =
            hex_lattice::PreparedNativeProcessor::output_tail(&saturator)
        else {
            panic!("finite saturator tail");
        };
        let settled = 2 * (latency + max_tail_frames as usize);
        assert_eq!((latency, settled), (11, 72));
        assert!(
            original_output[..2 * latency]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert!(
            original_output[settled..].iter().all(|frame| {
                (frame[0] - 1.75).abs() < 1.0e-6 && (frame[1] + 1.75).abs() < 1.0e-6
            })
        );

        control
            .publish(saturator_track_and_return_graph(0.0))
            .expect("publish edited prepared revision");
        let mut output = [[0.0; 2]; 128];
        let allocations = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).expect("callback render");
            callback.begin_block();
            callback
                .render(&mut output)
                .expect("callback render during ramp");
            callback.begin_block();
            callback
                .render(&mut output)
                .expect("callback render after ramp");
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0),
        );
        assert_ne!(output, original_output);
        // At 48 kHz the 5 ms ramp from the edit at frame 128 ends at input
        // frame 368; Main settles on the exact target `settled` frames later,
        // inside the third 128-frame block (frames 384..512).
        let target = 128 + 240 + settled - 384;
        assert_eq!(target, 56);
        assert!(output[target..].iter().all(|frame| {
            (frame[0] - 0.34375).abs() < 1.0e-6 && (frame[1] + 0.55).abs() < 1.0e-6
        }));
        assert!(output[..target].iter().any(|frame| {
            (frame[0] - 0.34375).abs() >= 1.0e-6 || (frame[1] + 0.55).abs() >= 1.0e-6
        }));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn overdrive_track_return_handoff_and_render_allocate_nothing() {
        use hex_devices::overdrive::{OVERDRIVE_DEFINITION as d, parameter};
        use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
        let (mut control, mut callback) = channel(48_000).unwrap();
        let original = d.default_state().unwrap();
        control
            .publish(native_effect_track_and_return_graph(original.clone()))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 128];
        callback.begin_block();
        callback.render(&mut output).unwrap();
        let changed = d
            .patch_parameter(&original, parameter::DRIVE, ParameterValue::Float(1.0))
            .unwrap();
        control
            .publish(native_effect_track_and_return_graph(changed))
            .unwrap();
        let allocations = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(output.iter().flatten().all(|sample| sample.is_finite()));
        assert!(output.iter().flatten().any(|sample| *sample != 0.0));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn rattle_track_return_handoff_and_render_allocate_nothing() {
        use hex_devices::rattle::{RATTLE_DEFINITION as d, parameter};
        use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
        let (mut control, mut callback) = channel(48_000).unwrap();
        let original = d.default_state().unwrap();
        control
            .publish(native_effect_track_and_return_graph(original.clone()))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 128];
        callback.begin_block();
        callback.render(&mut output).unwrap();
        // The edited revision inherits oversampler/noise history and glides drive.
        let changed = d
            .patch_parameter(&original, parameter::DRIVE, ParameterValue::Float(1.0))
            .unwrap();
        control
            .publish(native_effect_track_and_return_graph(changed))
            .unwrap();
        let allocations = allocation_counter::measure(|| {
            for _ in 0..4 {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            }
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(output.iter().flatten().all(|sample| sample.is_finite()));
        assert!(output.iter().flatten().any(|sample| *sample != 0.0));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn compressor_track_and_return_revision_admission_and_render_allocate_nothing() {
        let (mut control, mut callback) = channel(48_000).expect("channel");
        control
            .publish(compressor_track_and_return_graph(-18.0))
            .expect("publish compressor graph");
        control.command(PlaybackCommand::Play).expect("play");
        callback.begin_block();

        let input = [[0.25, -0.4]; 128];
        let mut reference_track = CompressorReferenceHistory::default();
        let mut reference_return = CompressorReferenceHistory::default();
        let expected_before = compressor_routed_reference(
            -18.0,
            -18.0,
            &mut reference_track,
            &mut reference_return,
            &input,
        );
        let mut original_output = [[0.0; 2]; 128];
        callback
            .render(&mut original_output)
            .expect("initial callback render");
        assert_eq!(original_output.as_slice(), expected_before.as_slice());

        let mut changed_track = CompressorReferenceHistory {
            envelope: reference_track.envelope,
            gain_db: reference_track.gain_db,
        };
        let mut changed_return = CompressorReferenceHistory {
            envelope: reference_return.envelope,
            gain_db: reference_return.gain_db,
        };
        let expected_after = compressor_routed_reference(
            -42.0,
            -42.0,
            &mut changed_track,
            &mut changed_return,
            &input,
        );
        control
            .publish(compressor_track_and_return_graph(-42.0))
            .expect("publish edited compressor revision");
        let mut output = [[0.0; 2]; 128];
        let allocations = allocation_counter::measure(|| {
            callback.begin_block();
            callback
                .render(&mut output)
                .expect("edited callback render");
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0),
        );
        assert_eq!(output.as_slice(), expected_after.as_slice());
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn reverb_track_return_render_edit_removal_stop_seek_and_shutdown_allocate_nothing() {
        use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
        let state =
            hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.reverb"))
                .unwrap();
        let changed = hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(1),
            ParameterValue::Float(2.5),
        )
        .unwrap();
        let frozen = hex_engine::patch_native_device_parameter(
            &changed,
            ParameterId::from_static(26),
            ParameterValue::Bool(true),
        )
        .unwrap();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut block = [[0.0; 2]; 127];
        for state in [state, changed, frozen] {
            control
                .publish(native_track_and_return_graph(Some(state)))
                .unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let counts = allocation_counter::measure(|| {
                for _ in 0..40 {
                    callback.begin_block();
                    callback.render(&mut block).unwrap();
                }
            });
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert!(block.iter().flatten().any(|v| v.abs() > 0.0));
            control.reclaim();
        }
        control
            .publish(native_track_and_return_graph(None))
            .unwrap();
        let counts = allocation_counter::measure(|| {
            for _ in 0..8 {
                callback.begin_block();
                callback.render(&mut block).unwrap();
            }
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(
            block.iter().flatten().any(|v| v.abs() > 0.0),
            "removed effects drain their real boxed histories"
        );
        control.reclaim();
        for command in [PlaybackCommand::Seek(0), PlaybackCommand::Stop] {
            control.command(command).unwrap();
            let counts = allocation_counter::measure(|| {
                // A queued Stop fades over 128 frames, past one block (#4151).
                for _ in 0..1 + usize::from(command == PlaybackCommand::Stop) {
                    callback.begin_block();
                    callback.render(&mut block).unwrap();
                }
            });
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        assert_eq!(block, [[0.0; 2]; 127]);
        control.request_shutdown();
        let counts = allocation_counter::measure(|| {
            callback.begin_block();
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn gate_track_and_return_initial_and_edited_callback_render_allocate_nothing() {
        let (mut control, mut callback) = channel(48_000).expect("channel");
        control
            .publish(gate_track_and_return_graph(-60.0))
            .expect("publish gate graph");
        control.command(PlaybackCommand::Play).expect("play");
        let mut initial = [[0.0; 2]; 128];
        let initial_allocations = allocation_counter::measure(|| {
            callback.begin_block();
            callback
                .render(&mut initial)
                .expect("initial callback render");
        });
        assert_eq!(
            (
                initial_allocations.count_total,
                initial_allocations.bytes_total,
                initial_allocations.count_current,
                initial_allocations.bytes_current
            ),
            (0, 0, 0, 0),
        );
        assert!(initial.iter().all(|frame| {
            (frame[0] - 0.00025009375).abs() < 1.0e-8 && (frame[1] + 0.00040015).abs() < 1.0e-8
        }));

        control
            .publish(gate_track_and_return_graph(-20.0))
            .expect("publish floor-edited Gate graph");
        let mut edited = [[0.0; 2]; 128];
        let allocations = allocation_counter::measure(|| {
            callback.begin_block();
            callback
                .render(&mut edited)
                .expect("edited callback render");
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0),
        );
        let start = 10.0_f64.powf(-60.0 / 20.0);
        let target = 10.0_f64.powf(-20.0 / 20.0);
        let step = (target - start) / 4_800.0;
        for (index, frame) in edited.iter().enumerate() {
            let gain = start + step * (index as f64 + 1.0);
            let direct_left = 0.25 * gain;
            let direct_right = -0.4 * gain;
            let expected = [
                direct_left + direct_left * 0.5 * gain * 0.75,
                direct_right + direct_right * 0.5 * gain * 0.75,
            ];
            assert!((f64::from(frame[0]) - expected[0]).abs() < 2.0e-8);
            assert!((f64::from(frame[1]) - expected[1]).abs() < 2.0e-8);
        }
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn seek_cancels_a_waiting_begin_without_blocking_a_later_fresh_press() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, identity) = spectrum_eq8_graph();
        let first_revision = control
            .publish_with_policy_and_context_generation(
                playback,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        callback.begin_block();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .revision,
            first_revision
        );

        let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
        control
            .processor_audition
            .select(identity, 3, 2, 48_000, selection)
            .unwrap();
        callback.begin_block();
        let mut output = [[0.0; 2]; 64];
        callback.render(&mut output).unwrap();
        assert_eq!(callback.processor_audition_lease, None);
        assert!(callback.processor_audition.current().is_some());

        control.command(PlaybackCommand::Seek(0)).unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(callback.processor_audition_lease, None);
        assert_eq!(callback.processor_audition.current(), None);

        let (replacement, replacement_identity) = fresh_spectrum_eq8_graph();
        assert_eq!(replacement_identity, identity);
        let second_revision = control
            .publish_with_policy_and_context_generation(
                replacement,
                PublicationPolicy::InheritRuntime,
                3,
            )
            .unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .revision,
            second_revision,
            "publication rejection: {:?}",
            callback.status.rejected_revision()
        );
        assert_eq!(callback.status.rejected_revision(), None);
        assert_eq!(callback.processor_audition_lease, None);
        assert_eq!(callback.processor_audition.current(), None);
        assert!(
            !callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );

        assert_eq!(callback.processor_audition_lease, None);
        assert!(
            !callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );

        control
            .processor_audition
            .select(identity, 3, second_revision, 48_000, selection)
            .unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(
            callback.processor_audition_lease,
            control.processor_audition.desired()
        );
        assert!(
            callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );

        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn audition_waits_for_actual_generation_and_revision_floor_then_activates() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, identity) = spectrum_eq8_graph();
        let first_revision = control
            .publish_with_policy_and_context_generation(
                playback,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        callback.begin_block();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .revision,
            first_revision
        );

        let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
        control
            .processor_audition
            .select(identity, 3, first_revision + 1, 48_000, selection)
            .unwrap();
        let mut output = [[0.0; 2]; 64];
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(callback.processor_audition_lease, None);
        assert!(callback.processor_audition.current().is_some());

        let (replacement, replacement_identity) = fresh_spectrum_eq8_graph();
        assert_eq!(replacement_identity, identity);
        let second_revision = control
            .publish_with_policy_and_context_generation(
                replacement,
                PublicationPolicy::InheritRuntime,
                3,
            )
            .unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .revision,
            second_revision
        );
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .control_context_generation,
            3
        );
        assert_eq!(
            callback.processor_audition_lease,
            control.processor_audition.desired()
        );
        assert!(
            callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );
        assert!(output.iter().flatten().any(|value| value.abs() > 1.0e-6));

        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn sparse_eq_callback_publication_held_stable_audition_release_and_retirement_allocate_nothing()
    {
        let (mut control, mut callback) = channel(48000).unwrap();
        let (playback, identity) = spectrum_eq_graph_with_params(true, 0, 3.0, 1.0, true, false);
        let first = control
            .publish_with_policy_and_context_generation(
                playback,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        let mut prefix = [[0.0; 2]; 17];
        let admission = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut prefix).unwrap();
        });
        assert_eq!(
            (
                admission.count_total,
                admission.bytes_total,
                admission.count_current,
                admission.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let selection = hex_engine::ProcessorAuditionSelection {
            index: 1000,
            lane: 0,
        };
        control
            .processor_audition
            .select(identity, 2, first, 48000, selection)
            .unwrap();
        let mut output = [[0.0; 2]; 64];
        let held = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                held.count_total,
                held.bytes_total,
                held.count_current,
                held.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let (candidate, _) = spectrum_eq_graph_with_params(false, 0, 6.0, 2.0, true, false);
        let revision = control
            .publish_with_policy_and_context_generation(
                candidate,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        let publication = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                publication.count_total,
                publication.bytes_total,
                publication.count_current,
                publication.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert_eq!(
            callback.processor_audition_lease,
            control.processor_audition.desired()
        );
        assert!(
            callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );
        control.processor_audition.release().unwrap();
        let mut fade = [[0.0; 2]; 256];
        let release = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut fade).unwrap();
        });
        assert_eq!(
            (
                release.count_total,
                release.bytes_total,
                release.count_current,
                release.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(callback.processor_audition_lease.is_none());
        control
            .processor_audition
            .select(identity, 2, revision, 48000, selection)
            .unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        control.processor_audition.cancel_immediately().unwrap();
        let cancel = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                cancel.count_total,
                cancel.bytes_total,
                cancel.count_current,
                cancel.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(callback.processor_audition_lease.is_none());
        control.request_shutdown();
        let retire = allocation_counter::measure(|| {
            callback.begin_block();
        });
        assert_eq!(
            (
                retire.count_total,
                retire.bytes_total,
                retire.count_current,
                retire.bytes_current
            ),
            (0, 0, 0, 0)
        );
        control.reclaim();
    }

    #[test]
    fn compatible_parameter_publications_preserve_the_held_audition() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, identity) = spectrum_eq8_graph();
        let first_revision = control
            .publish_with_policy_and_context_generation(
                playback,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        callback.begin_block();
        let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
        control
            .processor_audition
            .select(identity, 2, first_revision, 48_000, selection)
            .unwrap();
        let mut output = [[0.0; 2]; 64];
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert!(
            callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );

        for (gain, q) in [(6.0, 1.0), (6.0, 2.0)] {
            let (candidate, candidate_identity) = spectrum_eq8_graph_with_params(false, 0, gain, q);
            assert_eq!(candidate_identity, identity);
            let revision = control
                .publish_with_policy_and_context_generation(
                    candidate,
                    PublicationPolicy::InheritRuntime,
                    2,
                )
                .unwrap();
            callback.begin_block();
            callback.render(&mut output).unwrap();
            assert_eq!(
                callback
                    .active
                    .as_ref()
                    .and_then(ActivePublication::legacy)
                    .unwrap()
                    .revision,
                revision
            );
            assert_eq!(
                callback.processor_audition_lease,
                control.processor_audition.desired()
            );
            assert!(
                callback
                    .active
                    .as_mut()
                    .and_then(ActivePublication::legacy_mut)
                    .unwrap()
                    .playback
                    .processor_audition_is_active(identity, selection)
            );
        }

        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn wrong_sample_rate_target_is_rejected_without_replaying() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, identity) = spectrum_eq8_graph();
        let revision = control
            .publish_with_policy_and_context_generation(
                playback,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        callback.begin_block();

        let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
        control
            .processor_audition
            .select(identity, 2, revision, 44_100, selection)
            .unwrap();
        let mut output = [[0.0; 2]; 64];
        callback.begin_block();
        let epoch = callback.processor_audition.current_epoch();
        callback.render(&mut output).unwrap();
        assert_eq!(callback.processor_audition_lease, None);
        assert_eq!(callback.processor_audition_rejected_epoch, epoch);
        assert!(
            !callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );

        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn mode_change_invalidates_the_old_audition_epoch_and_needs_a_fresh_press() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, identity) = spectrum_eq8_graph();
        let first_revision = control
            .publish_with_policy_and_context_generation(
                playback,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        callback.begin_block();
        let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
        control
            .processor_audition
            .select(identity, 2, first_revision, 48_000, selection)
            .unwrap();
        let mut output = [[0.0; 2]; 64];
        callback.begin_block();
        let epoch = callback.processor_audition.current_epoch();
        callback.render(&mut output).unwrap();
        assert_eq!(
            callback.processor_audition_lease,
            control.processor_audition.desired()
        );

        // Stereo -> left/right invalidates the prior transient selection while
        // keeping the stable device ID. The old held epoch must stay rejected.
        let (replacement, replacement_identity) = spectrum_eq8_graph_with_mode(false, 1);
        assert_eq!(replacement_identity, identity);
        let second_revision = control
            .publish_with_policy_and_context_generation(
                replacement,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .revision,
            second_revision
        );
        assert_eq!(callback.processor_audition_lease, None);
        assert_eq!(callback.processor_audition_rejected_epoch, epoch);
        assert!(
            !callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(callback.processor_audition_lease, None);
        assert_eq!(callback.processor_audition_rejected_epoch, epoch);

        control
            .processor_audition
            .select(identity, 2, second_revision, 48_000, selection)
            .unwrap();
        callback.begin_block();
        let fresh_epoch = callback.processor_audition.current_epoch();
        assert!(fresh_epoch > epoch);
        callback.render(&mut output).unwrap();
        assert!(
            callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );

        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn a_fresh_audition_admitted_after_priority_stop_survives_same_block_consumption() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, identity) = spectrum_eq8_graph();
        let revision = control
            .publish_with_policy_and_context_generation(
                playback,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        callback.begin_block();

        control.stop();
        let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
        control
            .processor_audition
            .select(identity, 2, revision, 48_000, selection)
            .unwrap();
        callback.begin_block();
        let mut output = [[0.0; 2]; 64];
        callback.render(&mut output).unwrap();

        assert!(
            !callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .playback
                .playing()
        );
        assert_eq!(
            callback.processor_audition_lease,
            control.processor_audition.desired()
        );
        assert!(
            callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );

        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn render_fault_refuses_new_capture_admissions() {
        let (control, mut callback) = channel(48_000).unwrap();
        callback.report_render_fault(Some(&Invalid("injected failure")), 0);
        assert!(control.status.reserve_midi_begin().is_err());
        assert!(control.status.reserve_gain_begin().is_err());
        assert!(control.status.reserve_native_parameter_input().is_err());
    }

    #[test]
    fn stale_play_queued_before_fault_does_not_restart_resident_graph() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, _) = spectrum_eq8_graph();
        control.publish(playback).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 32]).unwrap();
        let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        callback.report_render_fault(Some(&Invalid("injected failure")), 32);
        let mut output = [[0.0; 2]; 16];
        #[cfg(feature = "allocation-probe")]
        let allocations = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        #[cfg(not(feature = "allocation-probe"))]
        {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        }
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(
            !control.status.playing.load(Acquire),
            "pre-fault queued Play restarted the faulted graph"
        );
        assert_eq!(
            control.status.rejected_action(),
            Some((ticket, PlaybackActionRejection::OutputFailed))
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn admission_delayed_across_fault_cannot_restart_playback() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, _) = spectrum_eq8_graph();
        control.publish(playback).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 32]).unwrap();
        // Model a producer suspended after reading the actual admission fields
        // but before publishing its fixed-size command into the ring.
        let epoch = control.status.transport_epoch.load(Acquire);
        let gate = control.status.sampler_edit.gate();
        callback.report_render_fault(Some(&Invalid("injected failure")), 32);
        let ticket = control.command_ticket + 1;
        assert!(
            control
                .commands
                .push(QueuedCommand {
                    ticket,
                    action: PlaybackAction::Transport(PlaybackCommand::PlayFrom(45)),
                    project_generation: None,
                    expected_source: None,
                    sampler_edit_gate: gate,
                    transport_epoch: epoch,
                })
                .is_ok()
        );
        control.command_ticket = ticket;
        control.status.admitted_ticket.store(ticket, Release);
        callback.begin_block();
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.recovery_position(), 32);
        assert_eq!(
            control.status.rejected_action(),
            Some((ticket, PlaybackActionRejection::OutputFailed))
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn stale_play_is_rejected_after_replacement_and_new_play_is_allowed() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = control
            .enable_action_outcomes_on_control(&mut callback)
            .unwrap();
        let (playback, _) = spectrum_eq8_graph();
        control.publish(playback).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 32]).unwrap();
        let stale = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        let stale_second = control
            .command_with_ticket(PlaybackCommand::PlayFrom(45))
            .unwrap();
        let (replacement, _) = fresh_spectrum_eq8_graph();
        let revision = control
            .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
            .unwrap();
        callback.report_render_fault(Some(&Invalid("injected failure")), 32);
        #[cfg(feature = "allocation-probe")]
        let allocations = allocation_counter::measure(|| callback.begin_block());
        #[cfg(not(feature = "allocation-probe"))]
        callback.begin_block();
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert_eq!(
            control.status.rejected_action(),
            Some((stale_second, PlaybackActionRejection::OutputFailed))
        );
        for ticket in [stale, stale_second] {
            assert_eq!(
                reader.try_take(),
                Some(ActionOutcomeReceipt {
                    ticket,
                    outcome: ActionOutcome::Rejected(PlaybackActionRejection::OutputFailed),
                })
            );
        }
        assert!(!control.status.playing.load(Acquire));
        let fresh = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 16]).unwrap();
        assert!(control.status.playing.load(Acquire));
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: fresh,
                outcome: ActionOutcome::Applied,
            })
        );
        control.request_shutdown();
        control
            .retire_action_outcomes_on_control(&mut callback)
            .unwrap();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn fresh_play_from_after_fault_retains_explicit_cursor() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, _) = spectrum_eq8_graph();
        control.publish(playback).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 32]).unwrap();
        callback.report_render_fault(Some(&Invalid("injected failure")), 32);
        let ticket = control
            .command_with_ticket(PlaybackCommand::PlayFrom(45))
            .unwrap();
        let mut output = [[1.0; 2]; 16];
        #[cfg(feature = "allocation-probe")]
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        #[cfg(not(feature = "allocation-probe"))]
        {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        }
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.recovery_position(), 45);
        assert_eq!(
            control.status.rejected_action(),
            Some((ticket, PlaybackActionRejection::OutputFailed))
        );
        assert_eq!(output, [[0.0; 2]; 16]);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn priority_stop_preserves_zero_cursor_over_post_fault_play_from() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, _) = spectrum_eq8_graph();
        control.publish(playback).unwrap();
        callback.begin_block();
        callback.report_render_fault(Some(&Invalid("injected failure")), 32);
        control.command(PlaybackCommand::PlayFrom(45)).unwrap();
        control.stop();
        callback.begin_block();
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.recovery_position(), 0);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn multiple_rejected_actions_are_each_observable_after_one_callback() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = control
            .enable_action_outcomes_on_control(&mut callback)
            .unwrap();
        let (playback, _, _, _) = session_audio_graph();
        control.publish(playback).unwrap();
        callback.begin_block();
        let first = control
            .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::Stop)
            .unwrap();
        let second = control
            .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::Seek(4))
            .unwrap();
        callback.begin_block();
        assert_eq!(control.status.consumed_command.load(Acquire), second);
        assert_eq!(
            control.status.rejected_action(),
            Some((second, PlaybackActionRejection::InvalidSessionAction)),
        );
        for ticket in [first, second] {
            assert_eq!(
                reader.try_take(),
                Some(ActionOutcomeReceipt {
                    ticket,
                    outcome: ActionOutcome::Rejected(PlaybackActionRejection::InvalidSessionAction),
                })
            );
        }
        control.request_shutdown();
        control
            .retire_action_outcomes_on_control(&mut callback)
            .unwrap();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn first_render_cause_is_latched_before_stop_and_new_admission_clears_it() {
        let (mut control, mut callback) = channel(48_000).expect("channel");
        let (playback, _) = spectrum_eq8_graph();
        let revision = control.publish(playback).expect("publish");
        control.command(PlaybackCommand::Play).expect("play");
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 128]).expect("prefix");
        #[cfg(feature = "allocation-probe")]
        let allocations = allocation_counter::measure(|| {
            callback
                .report_render_fault(Some(&Invalid("ongoing midi query exceeds 256 edges")), 128);
        });
        #[cfg(not(feature = "allocation-probe"))]
        callback.report_render_fault(Some(&Invalid("ongoing midi query exceeds 256 edges")), 128);
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0),
        );
        assert!(!control.status.playing.load(Relaxed));
        assert_eq!(control.status.position.load(Relaxed), 0);
        assert_eq!(control.status.recovery_position(), 128);
        let fault = control.status.render_fault_snapshot().expect("fault");
        assert_eq!(fault.message, "ongoing midi query exceeds 256 edges");
        assert_eq!((fault.audio_revision, fault.frame), (revision, 128));
        callback.report_render_fault(None, 0);
        assert_eq!(control.status.render_fault_snapshot(), Some(fault));
        assert_eq!(control.status.recovery_position(), 128);
        control.command(PlaybackCommand::Seek(37)).expect("seek");
        callback.begin_block();
        assert_eq!(control.status.position.load(Relaxed), 37);
        assert_eq!(control.status.recovery_position(), 37);
        callback.report_render_fault(None, 37);
        assert_eq!(control.status.recovery_position(), 37);
        control.stop();
        assert_eq!(control.status.recovery_position(), 0);
        control
            .command(PlaybackCommand::Seek(45))
            .expect("later seek");
        callback.begin_block();
        assert_eq!(control.status.recovery_position(), 45);
        let (replacement, _) = spectrum_eq8_graph();
        let replacement_revision = control
            .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
            .expect("replacement");
        callback.begin_block();
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            replacement_revision
        );
        assert!(!control.status.render_fault.load(Acquire));
        assert!(control.status.render_fault_snapshot().is_none());
        assert_eq!(control.status.recovery_position(), 0);
        callback.report_render_fault(None, 256);
        assert_eq!(
            control
                .status
                .render_fault_snapshot()
                .expect("nonfinite")
                .message,
            "audio output contains nonfinite samples"
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn device_fault_keeps_recovery_cursor_across_graph_admission() {
        let (mut control, mut callback) = channel(48_000).expect("channel");
        let (playback, _) = spectrum_eq8_graph();
        control.publish(playback).expect("publish");
        control.command(PlaybackCommand::Play).expect("play");
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 64]).expect("prefix");
        control.status.device_fault.store(true, Release);
        callback.stop_after_fault();
        assert_eq!(control.status.position.load(Relaxed), 0);
        assert_eq!(control.status.recovery_position(), 64);
        let (replacement, _) = spectrum_eq8_graph();
        control
            .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
            .expect("replacement");
        callback.begin_block();
        assert_eq!(control.status.recovery_position(), 64);
        control.stop();
        assert_eq!(control.status.recovery_position(), 0);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn render_fault_fences_a_waiting_begin_before_a_future_graph_is_published() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, identity) = spectrum_eq8_graph();
        let first_revision = control
            .publish_with_policy_and_context_generation(
                playback,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        callback.begin_block();

        let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
        control
            .processor_audition
            .select(identity, 3, first_revision + 1, 48_000, selection)
            .unwrap();
        callback.begin_block();
        let mut output = [[0.0; 2]; 64];
        callback.render(&mut output).unwrap();
        assert_eq!(callback.processor_audition_lease, None);
        assert!(callback.processor_audition.current().is_some());

        callback.stop_after_fault();
        assert_eq!(callback.processor_audition.current(), None);
        let (replacement, _) = spectrum_eq8_graph();
        let second_revision = control
            .publish_with_policy_and_context_generation(
                replacement,
                PublicationPolicy::InstallFreshRuntime,
                3,
            )
            .unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();

        assert_eq!(second_revision, first_revision + 1);
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .revision,
            second_revision
        );
        assert_eq!(callback.processor_audition_lease, None);
        assert_eq!(callback.processor_audition.current(), None);
        assert!(
            !callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );

        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn playback_audition_routes_to_arrangement_session_runtime_and_return_owner() {
        let (mut playback, _) = spectrum_eq8_graph();
        let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
        let return_identity = PostNodeIdentity::new(
            hex_engine::ProcessorChainOwner::Return(3),
            12,
            hex_engine::DeviceTypeId::from_static("hex.eq8"),
        );
        assert!(playback.select_processor_audition(return_identity, Some(selection)));
        assert!(playback.processor_audition_is_active(return_identity, selection));
        let wrong_owner = PostNodeIdentity::new(
            hex_engine::ProcessorChainOwner::Track(3),
            12,
            hex_engine::DeviceTypeId::from_static("hex.eq8"),
        );
        assert!(!playback.select_processor_audition(wrong_owner, Some(selection)));
        playback.cancel_all_processor_auditions();
        assert!(!playback.processor_audition_is_active(return_identity, selection));
    }

    fn spectrum_test_channel() -> (
        Control,
        Callback,
        crate::spectrum_worker::SpectrumReader,
        crate::spectrum_worker::SpectrumReaper,
    ) {
        let stamp = SpectrumStamp {
            target: crate::spectrum_worker::SpectrumTargetId(0),
            generation: 0,
            sample_rate_hz: 48_000,
            applied_revision: 0,
        };
        let (capture, reader, reaper) =
            crate::spectrum_worker::spawn_spectrum_worker(stamp).unwrap();
        let (control, callback) = channel_with_spectrum(48_000, Some(capture)).unwrap();
        (control, callback, reader, reaper)
    }

    fn shutdown_spectrum_test_channel(
        mut control: Control,
        mut callback: Callback,
        reaper: crate::spectrum_worker::SpectrumReaper,
    ) {
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
        drop(callback);
        drop(control);
        reaper.shutdown_and_join().unwrap();
    }

    #[test]
    fn spectrum_stage_requires_independent_exact_phase_coverage() {
        use hex_engine::NodeSignalPhase::{Input, Output};
        let target = spectrum_test_target(hex_engine::ProcessorChainOwner::Track(3), 0, 1, 48_000);
        let wrong_owner =
            spectrum_test_target(hex_engine::ProcessorChainOwner::Return(3), 0, 1, 48_000);
        let input = [[0.1, -0.2], [0.3, -0.4], [0.5, -0.6], [0.7, -0.8]];
        let output = [[0.2, -0.4], [0.6, -0.8], [1.0, -1.2], [1.4, -1.6]];
        let mut stage = SpectrumStage::default();
        stage.begin(target, 1, 4);
        stage.observe_signal(target.identity, Input, 0, &input);
        assert!(!stage.complete(), "input alone cannot publish");
        stage.observe_signal(target.identity, Output, 2, &output[2..]);
        stage.observe_signal(wrong_owner.identity, Output, 0, &output[..2]);
        assert!(
            !stage.complete(),
            "the other owner's samples cannot complete a pair"
        );
        stage.observe_signal(target.identity, Output, 0, &output[..2]);
        assert!(stage.complete());
        assert_eq!(stage.input_frames[..4], input);
        assert_eq!(stage.frames[..4], output);
        stage.clear();
        assert!(!stage.complete());

        for phase in [Input, Output] {
            for (offset, count) in [(1, 1), (4, 1), (usize::MAX, 1)] {
                stage.begin(target, 2, 4);
                stage.observe_signal(target.identity, Input, 0, &input);
                stage.observe_signal(target.identity, Output, 0, &output);
                assert!(stage.complete());
                stage.observe_signal(target.identity, phase, offset, &input[..count]);
                assert!(
                    !stage.complete(),
                    "overlap or out-of-bounds in either phase invalidates the pair"
                );
            }
        }
        stage.begin(target, 3, 257);
        assert!(!stage.complete());
    }

    #[test]
    fn prepared_eq_captures_actual_input_and_processed_output_without_changing_pcm() {
        let (mut control, mut callback, mut reader, reaper) = spectrum_test_channel();
        let (playback, identity) = spectrum_eq8_graph_with_params(true, 0, 12.0, 1.0);
        let (mut oracle, _) = spectrum_eq8_graph_with_params(true, 0, 12.0, 1.0);
        let revision = control.publish(playback).unwrap();
        control
            .spectrum_target
            .select(identity, 0, revision, 48_000)
            .unwrap();
        let mut actual = [[0.0; 2]; 256];
        let mut expected = actual;
        oracle.render(&mut expected).unwrap();
        let mut render = || {
            callback.begin_block();
            callback.render(&mut actual).unwrap();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(&mut render);
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        render();
        assert_eq!(actual, expected);
        assert!(callback.spectrum_stages.get(0).unwrap().complete());
        assert_eq!(
            callback.spectrum_stages.get(0).unwrap().input_frames,
            [[0.25, -0.4]; 256]
        );
        assert_eq!(callback.spectrum_stages.get(0).unwrap().frames, actual);
        assert!(actual.iter().any(|frame| (frame[0] - 0.25).abs() > 0.001));
        // Even a fully covered pair must be discarded if final caller validation fails.
        assert_eq!(
            callback.finish_spectrum_block(Instant::now(), false),
            Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
        );
        assert!(!callback.spectrum_stages.get(0).unwrap().complete());
        assert!(reader.try_latest_pair().is_none());
        shutdown_spectrum_test_channel(control, callback, reaper);
    }

    #[test]
    fn existing_publication_methods_use_control_context_generation_zero() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(graph().0).unwrap();
        callback.begin_block();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|publication| publication.control_context_generation),
            Some(0)
        );
        control
            .publish_with_policy(graph().0, PublicationPolicy::InstallFreshRuntime)
            .unwrap();
        callback.begin_block();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|publication| publication.control_context_generation),
            Some(0)
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn analyzer_waits_for_exact_publication_generation_and_activation_floor() {
        let (mut control, mut callback, _reader, reaper) = spectrum_test_channel();
        let generation_a = 41;
        let generation_b = 42;
        let (playback_a, identity) = spectrum_eq8_graph();
        let revision_a = control
            .publish_with_policy_and_context_generation(
                playback_a,
                PublicationPolicy::InstallFreshRuntime,
                generation_a,
            )
            .unwrap();
        callback.begin_block();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|publication| publication.control_context_generation),
            Some(generation_a)
        );

        let target_a = spectrum_test_target(identity.owner, generation_a, revision_a, 48_000);
        control
            .spectrum_target
            .select(identity, generation_a, revision_a, 48_000)
            .unwrap();
        callback.begin_block();
        assert_eq!(callback.spectrum_target.current(), Some(target_a));

        let mut output = [[0.0; 2]; 16];
        callback.render(&mut output).unwrap();
        assert!(callback.spectrum_stages.get(0).unwrap().complete());

        let (playback_b, identity_b) = spectrum_eq8_graph();
        assert_eq!(identity_b, identity);
        let revision_b = control
            .publish_with_policy_and_context_generation(
                playback_b,
                PublicationPolicy::InstallFreshRuntime,
                generation_b,
            )
            .unwrap();
        assert!(revision_b > target_a.activation_floor);
        callback.begin_block();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|publication| publication.control_context_generation),
            Some(generation_b)
        );

        // A project can switch after render but before the adapter submits its
        // block. The final admission check must reject the old project's tap.
        assert_eq!(
            callback.finish_spectrum_block(Instant::now(), true),
            Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
        );

        let (target_b, _) = control
            .spectrum_target
            .select(identity, generation_b, revision_b + 1, 48_000)
            .unwrap();
        callback.begin_block();
        assert_eq!(callback.spectrum_target.current(), Some(target_b));
        callback.render(&mut output).unwrap();
        assert!(
            !callback.spectrum_stages.get(0).unwrap().valid,
            "activation floor is not reached"
        );
        assert_eq!(
            callback.spectrum_stages.get(0).unwrap().target,
            Some(target_b)
        );
        assert_eq!(
            callback.finish_spectrum_block(Instant::now(), true),
            Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 1 })
        );

        let (playback_c, identity_c) = spectrum_eq8_graph();
        assert_eq!(identity_c, identity);
        let revision_c = control
            .publish_with_policy_and_context_generation(
                playback_c,
                PublicationPolicy::InstallFreshRuntime,
                generation_b,
            )
            .unwrap();
        assert_eq!(revision_c, revision_b + 1);
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert!(callback.spectrum_stages.get(0).unwrap().complete());
        assert!(matches!(
            callback.finish_spectrum_block(Instant::now(), true),
            Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 2 })
                | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 2 })
        ));

        // A later ordinary publication in the same project must keep the
        // selected token and its original lower-bound activation revision.
        let revision_d = control
            .publish_with_policy_and_context_generation(
                spectrum_eq8_graph().0,
                PublicationPolicy::InheritRuntime,
                generation_b,
            )
            .unwrap();
        assert!(revision_d > revision_c);
        callback.begin_block();
        assert_eq!(callback.spectrum_target.current(), Some(target_b));
        callback.render(&mut output).unwrap();
        assert!(matches!(
            callback.finish_spectrum_block(Instant::now(), true),
            Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 3 })
                | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 3 })
        ));

        let (target_wrong_rate, _) = control
            .spectrum_target
            .select(identity, generation_b, revision_d, 44_100)
            .unwrap();
        callback.begin_block();
        assert_eq!(callback.spectrum_target.current(), Some(target_wrong_rate));
        callback.render(&mut output).unwrap();
        assert!(!callback.spectrum_stages.get(0).unwrap().valid);
        assert_eq!(
            callback.finish_spectrum_block(Instant::now(), true),
            Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 4 })
        );
        shutdown_spectrum_test_channel(control, callback, reaper);
    }

    #[test]
    fn generation_context_survives_full_queue_retry_and_deferred_handoff() {
        let (mut control, mut callback, _reader, reaper) = spectrum_test_channel();
        let generation_a = 71;
        let generation_b = 72;
        let (playback_a, identity) = spectrum_eq8_graph();
        let revision_a = control
            .publish_with_policy_and_context_generation(
                playback_a,
                PublicationPolicy::InstallFreshRuntime,
                generation_a,
            )
            .unwrap();
        let rejected = control
            .publish_with_policy_and_context_generation(
                spectrum_eq8_graph().0,
                PublicationPolicy::InstallFreshRuntime,
                generation_b,
            )
            .unwrap_err();
        assert_eq!(rejected.reason, "playback publication already pending");
        assert_eq!(control.revision, revision_a);

        let (target_b, _) = control
            .spectrum_target
            .select(identity, generation_b, revision_a + 1, 48_000)
            .unwrap();
        callback.begin_block();
        assert_eq!(callback.spectrum_target.current(), Some(target_b));
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|publication| publication.control_context_generation),
            Some(generation_a)
        );
        let mut output = [[0.0; 2]; 16];
        callback.render(&mut output).unwrap();
        assert!(!callback.spectrum_stages.get(0).unwrap().valid);
        assert_eq!(
            callback.finish_spectrum_block(Instant::now(), true),
            Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
        );

        control
            .publish_with_policy_and_context_generation(
                rejected.playback,
                PublicationPolicy::InstallFreshRuntime,
                generation_b,
            )
            .unwrap();
        callback.test_transient_handoff_refusals = 1;
        callback.begin_block();
        assert_eq!(
            callback
                .deferred
                .as_ref()
                .map(|publication| publication.control_context_generation),
            Some(generation_b)
        );
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|publication| publication.control_context_generation),
            Some(generation_a)
        );
        callback.render(&mut output).unwrap();
        assert!(!callback.spectrum_stages.get(0).unwrap().valid);
        assert_eq!(
            callback.finish_spectrum_block(Instant::now(), true),
            Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 1 })
        );

        callback.begin_block();
        assert!(callback.deferred.is_none());
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|publication| publication.control_context_generation),
            Some(generation_b)
        );
        callback.render(&mut output).unwrap();
        assert!(callback.spectrum_stages.get(0).unwrap().complete());
        assert_eq!(
            callback.spectrum_stages.get(0).unwrap().target,
            Some(target_b)
        );
        assert!(matches!(
            callback.finish_spectrum_block(Instant::now(), true),
            Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 2 })
                | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 2 })
        ));
        shutdown_spectrum_test_channel(control, callback, reaper);
    }

    #[test]
    fn selected_target_is_rejected_when_project_switches_at_block_boundary() {
        let (mut control, mut callback, _reader, reaper) = spectrum_test_channel();
        let (playback_a, identity) = spectrum_eq8_graph();
        control
            .publish_with_policy_and_context_generation(
                playback_a,
                PublicationPolicy::InstallFreshRuntime,
                0,
            )
            .unwrap();
        callback.begin_block();
        let (target_a, _) = control
            .spectrum_target
            .select(identity, 0, 1, 48_000)
            .unwrap();
        callback.begin_block();
        assert_eq!(callback.spectrum_target.current(), Some(target_a));

        let after_target_selection = Arc::new(Barrier::new(2));
        callback.test_after_spectrum_target_boundary = Some(Arc::clone(&after_target_selection));
        std::thread::scope(|scope| {
            let callback_thread = scope.spawn(|| callback.begin_block());
            after_target_selection.wait();
            let (playback_b, identity_b) = spectrum_eq8_graph();
            assert_eq!(identity_b, identity);
            let revision_b = control
                .publish_with_policy_and_context_generation(
                    playback_b,
                    PublicationPolicy::InstallFreshRuntime,
                    1,
                )
                .unwrap();
            after_target_selection.wait();
            callback_thread.join().unwrap();
            assert_eq!(revision_b, 2);
        });
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|publication| publication.control_context_generation),
            Some(1)
        );
        let mut output = [[0.0; 2]; 8];
        assert_eq!(callback.spectrum_target.current(), Some(target_a));
        assert_eq!(
            callback.spectrum_target.current(),
            control.spectrum_target.desired()
        );
        callback.render(&mut output).unwrap();
        assert!(!callback.spectrum_stages.get(0).unwrap().valid);
        assert_eq!(
            callback.spectrum_stages.get(0).unwrap().target,
            Some(target_a)
        );
        assert_eq!(
            callback.finish_spectrum_block(Instant::now(), true),
            Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
        );

        let (target_b, _) = control
            .spectrum_target
            .select(identity, 1, 2, 48_000)
            .unwrap();
        callback.begin_block();
        assert_eq!(callback.spectrum_target.current(), Some(target_b));
        assert_eq!(target_b.project_generation, 1);
        callback.render(&mut output).unwrap();
        assert!(callback.spectrum_stages.get(0).unwrap().complete());
        assert_eq!(
            callback.spectrum_stages.get(0).unwrap().target,
            Some(target_b)
        );
        assert!(matches!(
            callback.finish_spectrum_block(Instant::now(), true),
            Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 1 })
                | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 1 })
        ));
        shutdown_spectrum_test_channel(control, callback, reaper);
    }

    fn assert_fresh_fence_rejection(control: &mut Control, expected_revision: u64) {
        assert_eq!(
            control.status.rejected_revision(),
            Some((
                expected_revision,
                PlaybackHandoffRejection::RuntimeFenceChanged
            ))
        );
        assert!(!control.status.publication_pending.load(Acquire));
    }

    #[test]
    fn running_transport_fence_rejects_stale_and_pending_command_observations() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        assert!(control.capture_running_transport_fence().is_none());
        control
            .publish_with_policy_and_context_generation(
                session_playback(1),
                PublicationPolicy::InstallFreshRuntime,
                7,
            )
            .unwrap();
        callback.begin_block();
        assert!(control.capture_running_transport_fence().is_none());
        let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        assert!(control.capture_running_transport_fence().is_none());
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 8]).unwrap();
        let (observed, fence) = control.capture_running_transport_fence().unwrap();
        assert_eq!(observed.consumed_command_ticket, play);
        assert_eq!(fence.admitted_command_ticket(), play);

        let seek = control
            .command_with_ticket(PlaybackCommand::Seek(0))
            .unwrap();
        assert!(control.capture_running_transport_fence().is_none());
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 8]).unwrap();
        let current = control.status.transport_observation().unwrap();
        assert_eq!(current.consumed_command_ticket, seek);
        control.status.transport_observation.publish(Some(observed));
        assert!(control.capture_running_transport_fence().is_none());
        control.status.transport_observation.publish(Some(current));
        assert!(control.capture_running_transport_fence().is_some());

        control
            .status
            .transport_observation
            .publish(Some(TransportObservation {
                audio_revision: current.audio_revision + 1,
                ..current
            }));
        assert!(control.capture_running_transport_fence().is_none());
        control
            .status
            .transport_observation
            .publish(Some(TransportObservation {
                control_generation: 0,
                ..current
            }));
        assert!(control.capture_running_transport_fence().is_none());
        control.status.transport_observation.publish(Some(current));
        control.status.device_fault.store(true, Release);
        assert!(control.capture_running_transport_fence().is_none());
        control.status.device_fault.store(false, Release);
        assert!(control.capture_running_transport_fence().is_some());
    }

    #[test]
    fn fresh_runtime_fence_accepts_empty_and_exact_active_snapshots() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let empty = control.capture_fresh_runtime_fence().unwrap();
        assert_eq!(empty.expected_active_revision(), 0);
        assert_eq!(empty.route_state_generation(), 0);
        assert_eq!(empty.admitted_command_ticket(), 0);
        let first_revision = control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(empty),
            )
            .unwrap();
        assert_eq!(control.status.admitted_ticket.load(Acquire), 0);
        callback.begin_block();
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            first_revision
        );

        let current = control.capture_fresh_runtime_fence().unwrap();
        let second_revision = control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(current),
            )
            .unwrap();
        callback.begin_block();
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            second_revision
        );
        assert_eq!(control.status.rejected_revision(), None);
        assert!(!control.status.publication_pending.load(Acquire));
        assert_eq!(control.reclaim(), 1);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn cancelled_fresh_publication_returns_candidate_and_allows_successor() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(graph().0).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        let active_revision = control.status.applied_revision.load(Acquire);
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let candidate_revision = control
            .publish_with_policy(
                graph_with_frame([0.91, -0.83]).0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();

        assert_eq!(
            control.cancel_fresh_runtime_publication(candidate_revision + 1),
            FreshRuntimeCancellationResult::UnknownRevision
        );
        assert_eq!(
            control.cancel_fresh_runtime_publication(candidate_revision),
            FreshRuntimeCancellationResult::Cancelled
        );
        callback.begin_block();
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            active_revision
        );
        assert_eq!(
            control.status.rejected_revision(),
            Some((
                candidate_revision,
                PlaybackHandoffRejection::FreshRuntimeCancelled
            ))
        );
        assert!(!control.status.publication_pending.load(Acquire));
        let mut output = [[0.0; 2]; 1];
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.25; 2]], "the old runtime remains active");
        assert_eq!(control.reclaim(), 1, "the candidate retires to control");

        let successor = control.publish(graph_with_frame([0.5, -0.4]).0).unwrap();
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), successor);
        assert_eq!(
            control.cancel_fresh_runtime_publication(candidate_revision),
            FreshRuntimeCancellationResult::UnknownRevision
        );
        assert_eq!(
            control.reclaim(),
            1,
            "the replaced graph retires to control"
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn cancellation_and_final_apply_cas_have_one_winner() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let active_revision = control.status.applied_revision.load(Acquire);
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let cancelled_revision = control
            .publish_with_policy(
                graph_with_frame([0.91, -0.83]).0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        let before_apply = Arc::new(Barrier::new(2));
        callback.test_before_fresh_runtime_apply = Some(Arc::clone(&before_apply));
        std::thread::scope(|scope| {
            let callback_thread = scope.spawn(|| callback.begin_block());
            before_apply.wait();
            assert_eq!(
                control.cancel_fresh_runtime_publication(cancelled_revision),
                FreshRuntimeCancellationResult::Cancelled
            );
            control.stop();
            before_apply.wait();
            callback_thread.join().unwrap();
        });
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            active_revision
        );
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(
            control.status.rejected_revision(),
            Some((
                cancelled_revision,
                PlaybackHandoffRejection::FreshRuntimeCancelled
            ))
        );
        assert_eq!(control.reclaim(), 1);

        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        assert!(control.status.playing.load(Acquire));
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let applying_revision = control
            .publish_with_policy(
                graph_with_frame([0.5, -0.4]).0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        let after_apply_decision = Arc::new(Barrier::new(2));
        callback.test_after_fresh_runtime_apply_decision = Some(Arc::clone(&after_apply_decision));
        std::thread::scope(|scope| {
            let callback_thread = scope.spawn(|| callback.begin_block());
            after_apply_decision.wait();
            assert_eq!(
                control.cancel_fresh_runtime_publication(applying_revision),
                FreshRuntimeCancellationResult::Applying
            );
            control.stop();
            after_apply_decision.wait();
            callback_thread.join().unwrap();
        });
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            applying_revision
        );
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(
            control.status.rejected_revision().unwrap().0,
            cancelled_revision
        );
        assert_eq!(
            control.cancel_fresh_runtime_publication(applying_revision),
            FreshRuntimeCancellationResult::Complete
        );
        assert_eq!(control.reclaim(), 1);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn failed_enqueue_does_not_leave_a_cancellable_revision_or_block_next_publish() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publications
            .push(QueuedPublication::Legacy(Publication {
                pcm_frames: 0,
                forwarded_pcm: false,
                revision: 99,
                control_context_generation: 0,
                explicit_control_context: false,
                source_context: None,
                policy: PublicationPolicy::InstallFreshRuntime,
                fresh_runtime_cancellation: None,
                quantized_seek_frame: None,
                streams: None,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                native_references: None,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                native_continuation: None,
                playback: Box::new(graph().0),
            }))
            .unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let rejected = control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap_err();
        assert_eq!(rejected.reason, "playback publication queue full");
        assert_eq!(control.revision, 0);
        assert_eq!(
            control.cancel_fresh_runtime_publication(1),
            FreshRuntimeCancellationResult::UnknownRevision
        );
        let queued = callback.publications.pop().unwrap();
        drop(queued);
        assert_eq!(control.publish(rejected.playback).unwrap(), 1);
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn ordinary_inherited_publications_have_no_fresh_cancellation_decision() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let first = control.publish(graph().0).unwrap();
        callback.begin_block();
        let next = control.publish(graph_with_frame([0.5, -0.4]).0).unwrap();
        assert_eq!(
            control.cancel_fresh_runtime_publication(next),
            FreshRuntimeCancellationResult::UnknownRevision
        );
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), next);
        assert_eq!(
            control.cancel_fresh_runtime_publication(first),
            FreshRuntimeCancellationResult::UnknownRevision
        );
        control.reclaim();
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn fault_invalidates_prepared_playing_runtime_fence() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let (mut prepared, _) = fresh_spectrum_eq8_graph();
        prepared.apply(PlaybackCommand::Play);
        let revision = control
            .publish_with_policy(
                prepared,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        callback.report_render_fault(Some(&Invalid("injected failure")), 32);
        callback.begin_block();
        assert_eq!(
            control.status.rejected_revision(),
            Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged))
        );
        assert!(!control.status.playing.load(Acquire));
    }

    #[test]
    fn latched_fault_refuses_prepared_runtime_even_after_epoch_exhaustion() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(graph().0).unwrap();
        callback.begin_block();
        control.status.transport_epoch.store(u64::MAX, Release);
        control
            .status
            .last_output_fault_epoch
            .store(u64::MAX, Release);
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let (mut prepared, _) = fresh_spectrum_eq8_graph();
        prepared.apply(PlaybackCommand::Play);
        let revision = control
            .publish_with_policy(
                prepared,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        callback.report_render_fault(Some(&Invalid("injected failure")), 32);
        callback.begin_block();
        assert_eq!(
            control.status.rejected_revision(),
            Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged))
        );
        assert!(!control.status.playing.load(Acquire));
    }

    #[test]
    fn fresh_runtime_fence_rejects_stale_active_revision() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish(graph().0)
            .unwrap_or_else(|error| panic!("{}", error.reason));
        callback.begin_block();
        let stale = control.capture_fresh_runtime_fence().unwrap();
        let intervening = control
            .publish_with_policy(graph().0, PublicationPolicy::InstallFreshRuntime)
            .unwrap_or_else(|error| panic!("{}", error.reason));
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), intervening);

        let rejected = control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
            )
            .unwrap_or_else(|error| panic!("{}", error.reason));
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), intervening);
        assert_fresh_fence_rejection(&mut control, rejected);
        assert_eq!(control.reclaim(), 2);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn fresh_runtime_fence_rejects_stale_route_generation_independently() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (old, _, clip, _) = session_audio_graph();
        let (mut oracle, _, oracle_clip, _) = session_audio_graph();
        assert_eq!(clip, oracle_clip);
        control
            .publish(old)
            .unwrap_or_else(|error| panic!("{}", error.reason));
        callback.begin_block();
        control
            .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            })
            .unwrap();
        callback.begin_block();
        oracle
            .try_apply_action(hex_engine::PlaybackAction::Session(
                hex_engine::SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: hex_engine::LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        oracle.apply(PlaybackCommand::Play);

        let stale = control.capture_fresh_runtime_fence().unwrap();
        let ticket_before = control.status.admitted_ticket.load(Acquire);
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .try_apply_action(hex_engine::PlaybackAction::Session(
                hex_engine::SessionPlaybackCommand::Seek(1),
            ))
            .unwrap();
        oracle
            .try_apply_action(hex_engine::PlaybackAction::Session(
                hex_engine::SessionPlaybackCommand::Seek(1),
            ))
            .unwrap();
        callback.publish_active_routes(true);
        assert_eq!(control.status.admitted_ticket.load(Acquire), ticket_before);
        assert_ne!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .playback
                .session_route_state_generation(),
            stale.route_state_generation()
        );

        let (candidate, _) = graph_with_frame([0.91, -0.83]);
        let rejected = control
            .publish_with_policy(
                candidate,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
            )
            .unwrap();
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        assert_fresh_fence_rejection(&mut control, rejected);
        let mut actual = [[0.0; 2]; 2];
        let mut expected = [[0.0; 2]; 2];
        callback.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(
            actual, expected,
            "stale fresh candidate replaced active PCM"
        );
        assert_ne!(actual[0], [0.91, -0.83]);
        let mut routes = [SessionRouteEntry::default(); 1];
        control.status.session_routes(&mut routes).unwrap();
        assert_eq!(
            routes[0].owner,
            SessionRouteOwner::Session {
                clip_id: clip.value()
            },
            "stale rejection must preserve the active Session route owner"
        );
        assert_eq!(control.reclaim(), 1);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn fresh_runtime_fence_rejects_stale_command_ticket_and_applies_prefix_to_old_graph() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish(graph().0)
            .unwrap_or_else(|error| panic!("{}", error.reason));
        callback.begin_block();
        let stale = control.capture_fresh_runtime_fence().unwrap();
        let ticket = control
            .command_with_ticket(PlaybackCommand::Seek(19))
            .unwrap();
        let rejected = control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
            )
            .unwrap();
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        assert_eq!(control.status.position.load(Acquire), 19);
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);
        assert_fresh_fence_rejection(&mut control, rejected);
        assert_eq!(control.reclaim(), 1);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn fresh_runtime_fence_rejects_pending_stop_and_stops_the_old_graph() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish(graph().0)
            .unwrap_or_else(|error| panic!("{}", error.reason));
        callback.begin_block();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        assert!(control.status.playing.load(Acquire));
        let stale = control.capture_fresh_runtime_fence().unwrap();
        control.stop();
        let rejected = control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
            )
            .unwrap_or_else(|error| panic!("{}", error.reason));
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
        assert_fresh_fence_rejection(&mut control, rejected);
        assert_eq!(control.reclaim(), 1);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn fresh_runtime_fence_rejects_stop_consumed_before_publication() {
        assert_consumed_stop_invalidates_fresh_fence(false);
    }

    #[test]
    fn resident_fresh_runtime_fence_rejects_stop_consumed_before_publication() {
        assert_consumed_stop_invalidates_fresh_fence(true);
    }

    #[test]
    fn fresh_fence_refuses_stop_between_marker_clear_and_transport_ack() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(session_audio_graph().0).unwrap();
        callback.begin_block();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        assert!(control.status.playing.load(Acquire));
        control.stop();
        let barrier = Arc::new(Barrier::new(2));
        callback.test_before_action_outcome_status = Some(Arc::clone(&barrier));
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| callback.begin_block());
            barrier.wait();
            assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
            assert!(control.capture_fresh_runtime_fence().is_none());
            barrier.wait();
            worker.join().unwrap();
        });
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(
            control
                .capture_fresh_runtime_fence()
                .unwrap()
                .consumed_stop_generation(),
            2
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    fn assert_consumed_stop_invalidates_fresh_fence(resident: bool) {
        let make_graph = || {
            if resident {
                session_audio_graph().0
            } else {
                graph().0
            }
        };
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(make_graph()).unwrap();
        begin_outcome_test_block(&mut callback);
        control.command(PlaybackCommand::Play).unwrap();
        begin_outcome_test_block(&mut callback);
        assert!(control.status.playing.load(Acquire));
        let stale = control.capture_fresh_runtime_fence().unwrap();
        control.stop();
        assert!(control.capture_fresh_runtime_fence().is_none());
        begin_outcome_test_block(&mut callback);
        assert!(!control.status.playing.load(Acquire));
        let mut candidate = make_graph();
        candidate.apply(PlaybackCommand::Play);
        let rejected = control
            .publish_with_policy(
                candidate,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
            )
            .unwrap();
        begin_outcome_test_block(&mut callback);
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        assert!(!control.status.playing.load(Acquire));
        assert_fresh_fence_rejection(&mut control, rejected);
        assert_eq!(control.reclaim(), 1);
        let fresh = control.capture_fresh_runtime_fence().unwrap();
        let mut candidate = make_graph();
        candidate.apply(PlaybackCommand::Play);
        let accepted = control
            .publish_with_policy(
                candidate,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fresh),
            )
            .unwrap();
        begin_outcome_test_block(&mut callback);
        assert_eq!(control.status.applied_revision.load(Acquire), accepted);
        assert!(control.status.playing.load(Acquire));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn exhausted_stop_generation_refuses_new_fences_without_blocking_stop() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(graph().0).unwrap();
        callback.begin_block();
        control
            .status
            .consumed_stop_generation
            .store(u64::MAX - 1, Release);
        control.stop();
        callback.begin_block();
        assert_eq!(
            control.status.consumed_stop_generation.load(Acquire),
            u64::MAX
        );
        assert!(control.capture_fresh_runtime_fence().is_none());
        control.stop();
        callback.begin_block();
        assert_eq!(
            control.status.consumed_stop_generation.load(Acquire),
            u64::MAX
        );
        assert!(!control.status.playing.load(Acquire));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn command_admitted_after_fresh_fence_validation_applies_to_new_runtime() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish(graph().0)
            .unwrap_or_else(|error| panic!("{}", error.reason));
        callback.begin_block();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let revision = control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        let barrier = std::sync::Arc::new(Barrier::new(2));
        callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
        std::thread::scope(|scope| {
            let callback_thread = scope.spawn(|| callback.begin_block());
            barrier.wait();
            let ticket = control
                .command_with_ticket(PlaybackCommand::Seek(47))
                .unwrap();
            barrier.wait();
            callback_thread.join().unwrap();
            assert_eq!(control.status.consumed_command.load(Acquire), ticket);
        });
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert_eq!(control.status.position.load(Acquire), 47);
        assert_eq!(control.status.rejected_revision(), None);
        assert_eq!(control.reclaim(), 1);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn bound_timing_only_capture_tracks_loop_spans_and_output_confirmation() {
        use crate::sample_recording::{OutputClock, OutputClockIndex};
        use hex_project::{Command, Editor, Project};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(Command::AddTrack {
                name: "input".into(),
            })
            .unwrap();
        editor
            .execute(Command::SetLoopRegion {
                start: 32,
                end: 160,
                enabled: true,
            })
            .unwrap();
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let playback = Playback::try_from_prepared_with_session(revision).unwrap();
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish_unaccounted_pcm_obligation(
                playback,
                PublicationPolicy::InstallFreshRuntime,
                7,
                true,
                false,
                Some(hex_engine::AudioSourceContext {
                    project_generation: 7,
                    editor_revision: 3,
                }),
            )
            .unwrap();
        callback.begin_block();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        let source = callback.recording_source().unwrap();
        let mut reader = control.sample_recording.start_bound(source).unwrap();
        assert!(reader.sources().is_empty());
        let mut index = OutputClockIndex::new(&reader);
        assert!(reader.ready());
        for block in 0..4 {
            let mut output = [[0.0; 2]; 64];
            let measured = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
                callback.set_sample_recording_output_clock(Some(OutputClock {
                    playback: cpal::StreamInstant::new(10, 0),
                    offset_frames: block * 64,
                    graph_latency_frames: None,
                    origin: crate::sample_recording::OutputClockOrigin::Unknown,
                }));
                callback.finish_sample_recording(true);
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(index.poll(&mut reader), Ok(None));
        }
        let starts: Vec<_> = (0..index.len())
            .map(|n| {
                index
                    .packet(n)
                    .unwrap()
                    .timing
                    .iter()
                    .map(|span| (span.take_start, span.take_end, span.timeline_start))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(
            starts,
            vec![
                vec![(0, 64, 0)],
                vec![(0, 64, 64)],
                vec![(0, 32, 128), (32, 64, 32)],
                vec![(0, 64, 64)]
            ]
        );
        control.sample_recording.stop();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 1]).unwrap();
        assert_eq!(
            index.poll(&mut reader),
            Ok(Some(crate::sample_recording::End::Stopped))
        );
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn recorded_audible_clock_accounts_for_actual_main_limiter_delay() {
        use crate::input_capture::{
            InputConfig, InputTake, InputTakeEnd, InputTiming, SharedClock, align_audible_take,
        };
        use crate::sample_recording::{End, OutputClock, OutputClockIndex};
        use hex_engine::AudioSourceContext;
        use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
        use hex_project::{Command, Created, Editor, Project};
        use std::collections::BTreeMap;

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, clip } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "transient".into(),
                path: "memory/transient.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 4_096,
                playback_frames: 4_096,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor
            .execute(Command::MoveClip {
                clip,
                track,
                start: 4_800,
            })
            .unwrap();
        let definition = &hex_devices::limiter::DEFINITION;
        let state = definition
            .patch_parameter(
                &definition.default_state().unwrap(),
                hex_devices::limiter::parameter::LOOKAHEAD,
                ParameterValue::Choice(2),
            )
            .unwrap();
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap();
        let mut pcm = vec![[0.0; 2]; 4_096];
        pcm[0] = [0.25; 2];
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::from([(asset, hex_engine::Audio::new(48_000, pcm).unwrap())]),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        assert_eq!(revision.arrangement().main_latency_frames(), 288);

        for block_size in [63, 256] {
            let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
            let (mut control, mut callback) = channel(48_000).unwrap();
            control
                .publish_unaccounted_pcm_obligation(
                    playback,
                    PublicationPolicy::InstallFreshRuntime,
                    7,
                    true,
                    false,
                    Some(AudioSourceContext {
                        project_generation: 7,
                        editor_revision: 3,
                    }),
                )
                .unwrap();
            callback.begin_block();
            control.command(PlaybackCommand::Play).unwrap();
            callback.begin_block();
            let source = callback.recording_source().unwrap();
            let mut reader = control.sample_recording.start_bound(source).unwrap();
            assert!(
                reader.sources().is_empty(),
                "source-bound capture retains timing only"
            );
            let mut index = OutputClockIndex::new(&reader);
            assert!(reader.ready());
            let origin = cpal::StreamInstant::new(10, 0);
            let mut output = vec![[0.0; 2]; 6_000];
            for (number, block) in output.chunks_mut(block_size).enumerate() {
                let measured = allocation_counter::measure(|| {
                    callback.begin_block();
                    callback.render(block).unwrap();
                    callback.set_sample_recording_output_clock(Some(OutputClock {
                        playback: origin,
                        offset_frames: (number * block_size) as u32,
                        graph_latency_frames: None,
                        origin: crate::sample_recording::OutputClockOrigin::Unknown,
                    }));
                    callback.finish_sample_recording(true);
                });
                assert_eq!(
                    (
                        measured.count_total,
                        measured.bytes_total,
                        measured.count_current,
                        measured.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
                assert_eq!(index.poll(&mut reader), Ok(None));
            }
            assert!(output[5_088][0] > 0.01);
            control.sample_recording.stop();
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 1]).unwrap();
            assert_eq!(index.poll(&mut reader), Ok(Some(End::Stopped)));
            let take = InputTake {
                path: std::path::PathBuf::new(),
                input_device: format!("{}:test-input", cpal::default_host().id())
                    .parse()
                    .unwrap(),
                input_config: InputConfig {
                    sample_rate: 48_000,
                    channels: 1,
                },
                sample_rate: 48_000,
                frames: 16,
                preroll_frames: 0,
                timing: vec![InputTiming {
                    take_start: 0,
                    frames: 16,
                    captured_at: origin,
                    offset_frames: 5_088,
                }],
                end: InputTakeEnd::Stopped,
                open_error: None,
            };
            assert_eq!(
                align_audible_take(SharedClock, &take, &index)
                    .unwrap()
                    .timeline_start,
                4_800
            );
            assert_eq!(
                index.packet(0).unwrap().clock.graph_latency_frames,
                Some(288)
            );
            control.sample_recording.reclaim();
            let mut seek_take = control.sample_recording.start_bound(source).unwrap();
            assert!(seek_take.ready());
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 1]).unwrap();
            callback.finish_sample_recording(true);
            let position = control.status.position.load(Acquire);
            control.command(PlaybackCommand::Seek(position)).unwrap();
            let measured = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut [[0.0; 2]; 1]).unwrap();
                callback.finish_sample_recording(true);
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(seek_take.drain(|_, _, _| {}), Some(End::TransportChanged));
        }
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn bound_output_recording_rejects_replaced_source() {
        use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
        use crate::sample_recording::{End, OutputClock, OutputClockIndex};
        use hex_engine::AudioSourceContext;

        let _serial = super::candidate_tests::serial();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let context = AudioSourceContext {
            project_generation: 0,
            editor_revision: 4,
        };
        let (graph, _, _, _) = session_audio_graph();
        control
            .publish_unaccounted_pcm_obligation(
                graph,
                PublicationPolicy::InstallFreshRuntime,
                0,
                true,
                false,
                Some(context),
            )
            .unwrap();
        callback.begin_block();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        client.bind_resident_transport(0);
        let source = client.status().unwrap().recording_source().unwrap();
        let wrong = AppliedAudioSource {
            audio_revision: source.audio_revision + 1,
            ..source
        };
        assert!(client.start_bound_output_recording(wrong).is_err());
        let mut take = client.start_bound_output_recording(source).unwrap();
        let mut index = OutputClockIndex::new(&take);
        assert_eq!(index.source_binding(), Some(source));
        assert!(take.ready());
        let mut output = [[0.0; 2]; 8];
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
                c.set_sample_recording_output_clock(Some(OutputClock {
                    playback: cpal::StreamInstant::new(10, 0),
                    offset_frames: 0,
                    graph_latency_frames: Some(0),
                    origin: crate::sample_recording::OutputClockOrigin::Unknown,
                }));
                c.finish_sample_recording(true);
            });
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(index.poll(&mut take), Ok(None));
        assert_eq!(index.len(), 1);

        // Hold a rendered packet until after the next source has been applied.
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
            c.set_sample_recording_output_clock(Some(OutputClock {
                playback: cpal::StreamInstant::new(11, 0),
                offset_frames: 0,
                graph_latency_frames: Some(0),
                origin: crate::sample_recording::OutputClockOrigin::Unknown,
            }));
        });
        let control = publication.control().unwrap();
        let (graph, _, _, _) = session_audio_graph();
        control
            .publish_unaccounted_pcm_obligation(
                graph,
                PublicationPolicy::InheritRuntime,
                0,
                true,
                false,
                Some(context),
            )
            .unwrap();
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.finish_sample_recording(true);
            });
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(index.poll(&mut take), Ok(Some(End::SourceChanged)));
        assert_eq!(index.len(), 1);
        assert!(client.start_bound_output_recording(source).is_err());
        client.reclaim_sample_recording();
        let current = client.status().unwrap().recording_source().unwrap();
        let mut next = client.start_bound_output_recording(current).unwrap();
        assert!(next.ready());
        let control = publication.control().unwrap();
        let (graph, _, _, _) = session_audio_graph();
        control
            .publish_unaccounted_pcm_obligation(
                graph,
                PublicationPolicy::InheritRuntime,
                0,
                true,
                false,
                Some(context),
            )
            .unwrap();
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
                c.finish_sample_recording(true);
            });
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            next.drain(|_, _, _| panic!("changed source published a packet")),
            Some(End::SourceChanged)
        );
        client.close();
        publication.close();
        drop(data);
        drop(errors);
        assert!(
            crate::lifecycle::test_dispose_backend(
                &mut Some(()),
                &mut false,
                &mut false,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        assert!(retirement.advance().unwrap());
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn source_bound_seek_preserves_playback_and_rejects_replaced_source() {
        use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
        use hex_engine::AudioSourceContext;

        let _serial = super::candidate_tests::serial();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        let context = AudioSourceContext {
            project_generation: 0,
            editor_revision: 4,
        };
        let (graph, _, _, _) = session_audio_graph();
        control
            .publish_unaccounted_pcm_obligation(
                graph,
                PublicationPolicy::InstallFreshRuntime,
                0,
                true,
                false,
                Some(context),
            )
            .unwrap();
        callback.begin_block();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        client.bind_resident_transport(0);
        let source = client.status().unwrap().recording_source().unwrap();
        let wrong_output = AppliedAudioSource {
            output_identity: source.output_identity + 1,
            ..source
        };
        assert!(
            client
                .seek_from_source_with_ticket(1, wrong_output)
                .is_err()
        );
        let wrong_context = AppliedAudioSource {
            context: AudioSourceContext {
                project_generation: 1,
                ..context
            },
            ..source
        };
        assert!(
            client
                .seek_from_source_with_ticket(1, wrong_context)
                .is_err()
        );

        let ticket = client.seek_from_source_with_ticket(17, source).unwrap();
        let measured =
            allocation_counter::measure(|| data.owner_mut().unwrap().run(|c, _| c.begin_block()));
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Applied
            })
        );
        assert!(!client.status().unwrap().playing.load(Acquire));
        assert_eq!(client.status().unwrap().position.load(Acquire), 17);

        let start = client.play_from_source_with_ticket(17, source).unwrap();
        data.owner_mut().unwrap().run(|c, _| c.begin_block());
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: start,
                outcome: ActionOutcome::Applied
            })
        );
        let ticket = client.seek_from_source_with_ticket(29, source).unwrap();
        let mut output = [[0.0; 2]; 129];
        // Running transport seeks complete after the 128-frame declick fade.
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Applied
            })
        );
        assert!(client.status().unwrap().playing.load(Acquire));
        assert_eq!(client.status().unwrap().position.load(Acquire), 30);
        client.stop();
        data.owner_mut().unwrap().run(|c, _| c.begin_block());

        let control = publication.control().unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let (graph, _, _, _) = session_audio_graph();
        control
            .publish_unaccounted_pcm_obligation(
                graph,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                0,
                true,
                false,
                Some(context),
            )
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        data.owner_mut()
            .unwrap()
            .run(|c, _| c.test_after_fresh_fence_validation = Some(Arc::clone(&barrier)));
        let ticket = std::thread::scope(|scope| {
            let callback = scope.spawn(|| {
                allocation_counter::measure(|| {
                    data.owner_mut().unwrap().run(|c, _| c.begin_block())
                })
            });
            barrier.wait();
            let ticket = client.seek_from_source_with_ticket(47, source).unwrap();
            barrier.wait();
            let measured = callback.join().unwrap();
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            ticket
        });
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::AudioSourceChanged)
            })
        );
        assert!(!client.status().unwrap().playing.load(Acquire));
        assert_ne!(client.status().unwrap().position.load(Acquire), 47);
        assert!(client.seek_from_source_with_ticket(47, source).is_err());
        let current = client.status().unwrap().recording_source().unwrap();
        for _ in 0..COMMANDS {
            client.seek_from_source_with_ticket(2, current).unwrap();
        }
        assert!(client.seek_from_source_with_ticket(2, current).is_err());
        client.stop();
        data.owner_mut().unwrap().run(|c, _| c.begin_block());
        assert!(!client.status().unwrap().playing.load(Acquire));
        client.close();
        publication.close();
        drop(data);
        drop(errors);
        assert!(
            crate::lifecycle::test_dispose_backend(
                &mut Some(()),
                &mut false,
                &mut false,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        assert!(retirement.advance().unwrap());
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn source_bound_start_rejects_same_project_replacement_and_preserves_stop() {
        use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
        use hex_engine::AudioSourceContext;

        let _serial = super::candidate_tests::serial();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        let old = AudioSourceContext {
            project_generation: 0,
            editor_revision: 4,
        };
        let (graph, _, _, _) = session_audio_graph();
        control
            .publish_unaccounted_pcm_obligation(
                graph,
                PublicationPolicy::InstallFreshRuntime,
                0,
                true,
                false,
                Some(old),
            )
            .unwrap();
        callback.begin_block();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        client.bind_resident_transport(0);
        let source = client.status().unwrap().recording_source().unwrap();
        let wrong_output = AppliedAudioSource {
            output_identity: source.output_identity + 1,
            ..source
        };
        assert!(
            client
                .play_from_source_with_ticket(1, wrong_output)
                .is_err()
        );
        let wrong_context = AppliedAudioSource {
            context: AudioSourceContext {
                project_generation: 1,
                ..old
            },
            ..source
        };
        assert!(
            client
                .play_from_source_with_ticket(1, wrong_context)
                .is_err()
        );

        let ticket = client.play_from_source_with_ticket(17, source).unwrap();
        let measured =
            allocation_counter::measure(|| data.owner_mut().unwrap().run(|c, _| c.begin_block()));
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Applied
            })
        );
        assert!(client.status().unwrap().playing.load(Acquire));
        client.stop();
        data.owner_mut().unwrap().run(|c, _| c.begin_block());

        let control = publication.control().unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        // A graph can be replaced even while its canonical context is unchanged.
        let next = old;
        let (graph, _, _, _) = session_audio_graph();
        control
            .publish_unaccounted_pcm_obligation(
                graph,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                0,
                true,
                false,
                Some(next),
            )
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        data.owner_mut()
            .unwrap()
            .run(|c, _| c.test_after_fresh_fence_validation = Some(Arc::clone(&barrier)));
        let ticket = std::thread::scope(|scope| {
            let callback = scope.spawn(|| {
                allocation_counter::measure(|| {
                    data.owner_mut().unwrap().run(|c, _| c.begin_block())
                })
            });
            barrier.wait();
            let ticket = client.play_from_source_with_ticket(47, source).unwrap();
            barrier.wait();
            let measured = callback.join().unwrap();
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            ticket
        });
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::AudioSourceChanged)
            })
        );
        assert!(!client.status().unwrap().playing.load(Acquire));
        assert_eq!(
            client.status().unwrap().recording_source().unwrap().context,
            next
        );
        assert_ne!(
            client
                .status()
                .unwrap()
                .recording_source()
                .unwrap()
                .audio_revision,
            source.audio_revision
        );
        assert!(client.play_from_source_with_ticket(47, source).is_err());
        let source = client.status().unwrap().recording_source().unwrap();
        for _ in 0..COMMANDS {
            client.play_from_source_with_ticket(2, source).unwrap();
        }
        assert!(client.play_from_source_with_ticket(2, source).is_err());
        client.stop();
        data.owner_mut().unwrap().run(|c, _| c.begin_block());
        assert!(!client.status().unwrap().playing.load(Acquire));
        client.close();
        publication.close();
        drop(data);
        drop(errors);
        assert!(
            crate::lifecycle::test_dispose_backend(
                &mut Some(()),
                &mut false,
                &mut false,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        assert!(retirement.advance().unwrap());
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn document_bound_client_rejects_commands_racing_a_successor_graph() {
        use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
        use hex_engine::{AudioSourceContext, LaunchQuantization, SessionPlaybackCommand};

        let _serial = super::candidate_tests::serial();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        let old = AudioSourceContext {
            project_generation: 0,
            editor_revision: 4,
        };
        let (old_graph, _, clip, _) = session_audio_graph();
        control
            .publish_unaccounted_pcm_obligation(
                old_graph,
                PublicationPolicy::InstallFreshRuntime,
                old.project_generation,
                true,
                false,
                Some(old),
            )
            .unwrap();
        callback.begin_block();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        client.bind_resident_transport(old.project_generation);

        let control = publication.control().unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let new = AudioSourceContext {
            project_generation: 2,
            editor_revision: 1,
        };
        let (new_graph, _, new_clip, _) = session_audio_graph();
        assert_eq!(clip, new_clip);
        control
            .publish_unaccounted_pcm_obligation(
                new_graph,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                2,
                true,
                false,
                Some(new),
            )
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        data.owner_mut().unwrap().run(|callback, _| {
            callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
        });
        let actions = [
            PlaybackAction::Transport(PlaybackCommand::PlayFrom(47)),
            PlaybackAction::Transport(PlaybackCommand::Seek(61)),
            PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            }),
        ];
        let tickets = std::thread::scope(|scope| {
            let callback = scope.spawn(|| {
                allocation_counter::measure(|| {
                    data.owner_mut()
                        .unwrap()
                        .run(|callback, _| callback.begin_block());
                })
            });
            barrier.wait();
            let tickets = actions.map(|action| client.action_with_ticket(action));
            barrier.wait();
            let count = callback.join().unwrap();
            assert_eq!(
                (
                    count.count_total,
                    count.bytes_total,
                    count.count_current,
                    count.bytes_current
                ),
                (0, 0, 0, 0)
            );
            tickets.map(Result::unwrap)
        });
        for ticket in tickets {
            assert_eq!(
                reader.try_take(),
                Some(ActionOutcomeReceipt {
                    ticket,
                    outcome: ActionOutcome::Rejected(PlaybackActionRejection::ProjectChanged),
                })
            );
        }
        let status = client.status().unwrap();
        assert_eq!(
            status.rejected_action(),
            Some((tickets[2], PlaybackActionRejection::ProjectChanged))
        );
        assert_eq!(status.recording_source().unwrap().context, new);
        assert_eq!(status.position.load(Acquire), 0);
        assert!(!status.playing.load(Acquire));
        assert!(client.command_with_ticket(PlaybackCommand::Play).is_err());

        client.bind_resident_transport(new.project_generation);
        let control = publication.control().unwrap();
        assert_eq!(control.reclaim(), 1);
        let edited = AudioSourceContext {
            editor_revision: 2,
            ..new
        };
        let (edited_graph, _, _, _) = session_audio_graph();
        control
            .publish_unaccounted_pcm_obligation(
                edited_graph,
                PublicationPolicy::InstallFreshRuntime,
                2,
                true,
                false,
                Some(edited),
            )
            .unwrap();
        let next = client
            .command_with_ticket(PlaybackCommand::Seek(29))
            .unwrap();
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: next,
                outcome: ActionOutcome::Applied
            })
        );
        assert_eq!(
            client.status().unwrap().recording_source().unwrap().context,
            edited
        );
        assert_eq!(client.status().unwrap().position.load(Acquire), 29);
        for _ in 0..COMMANDS {
            client.command_with_ticket(PlaybackCommand::Play).unwrap();
        }
        assert!(client.command_with_ticket(PlaybackCommand::Play).is_err());
        client.stop();
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        assert!(!client.status().unwrap().playing.load(Acquire));
        assert_eq!(client.status().unwrap().position.load(Acquire), 0);

        client.close();
        publication.close();
        drop(data);
        drop(errors);
        assert!(
            crate::lifecycle::test_dispose_backend(
                &mut Some(()),
                &mut false,
                &mut false,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        assert!(retirement.advance().unwrap());
    }

    #[test]
    fn stop_admitted_after_fresh_fence_validation_stops_new_runtime() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let revision = control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap_or_else(|error| panic!("{}", error.reason));
        let barrier = std::sync::Arc::new(Barrier::new(2));
        callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
        std::thread::scope(|scope| {
            let callback_thread = scope.spawn(|| callback.begin_block());
            barrier.wait();
            control.stop();
            barrier.wait();
            callback_thread.join().unwrap();
        });
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
        assert_eq!(control.status.rejected_revision(), None);
        assert_eq!(control.reclaim(), 1);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn stale_fresh_runtime_fence_is_rechecked_after_retirement_backpressure() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(graph().0).unwrap();
        callback.begin_block();
        for _ in 0..RETIRED {
            control
                .publish_with_policy(graph().0, PublicationPolicy::InstallFreshRuntime)
                .unwrap();
            callback.begin_block();
        }
        assert!(callback.retired.is_full());
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let revision = control
            .publish_with_policy(
                graph_with_frame([0.91, -0.83]).0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        let ticket = control
            .command_with_ticket(PlaybackCommand::Seek(13))
            .unwrap();
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), revision - 1);
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);
        // A Seek while playing fades out, jumps and ramps in (#4140).
        assert_eq!(control.status.position.load(Acquire), 0);
        assert!(control.status.publication_pending.load(Acquire));
        let mut fade_out = [[0.0; 2]; 128];
        callback.render(&mut fade_out).unwrap();
        assert_eq!(control.status.position.load(Acquire), 13);
        let ramp = |frame: usize| [0.25 * (frame + 1) as f32 / 128.0; 2];
        let mut before_retry = [[0.0; 2]; 8];
        callback.render(&mut before_retry).unwrap();
        assert_eq!(before_retry, std::array::from_fn(ramp));
        assert_eq!(control.status.position.load(Acquire), 21);
        assert_eq!(control.reclaim(), RETIRED);
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), revision - 1);
        assert_fresh_fence_rejection(&mut control, revision);
        let mut after_retry = [[0.0; 2]; 8];
        callback.render(&mut after_retry).unwrap();
        assert_eq!(after_retry, std::array::from_fn(|frame| ramp(frame + 8)));
        assert_eq!(control.status.position.load(Acquire), 29);
        assert_eq!(control.reclaim(), 1);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn rejected_candidate_is_held_until_control_reclaims_retirement_space() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        for revision in 10..10 + RETIRED as u64 {
            callback
                .retired
                .push(ReturnedPublication::Legacy(Publication {
                    pcm_frames: 0,
                    forwarded_pcm: false,
                    revision,
                    control_context_generation: 0,
                    explicit_control_context: false,
                    source_context: None,
                    policy: PublicationPolicy::InstallFreshRuntime,
                    fresh_runtime_cancellation: None,
                    quantized_seek_frame: None,
                    streams: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_references: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation: None,
                    playback: Box::new(graph().0),
                }))
                .unwrap();
        }
        assert!(callback.retired.is_full());

        let fence = control.capture_fresh_runtime_fence().unwrap();
        let (candidate, weak_candidate, _, _) = session_audio_graph();
        let rejected = control
            .publish_with_policy(
                candidate,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        control.stop();
        callback.begin_block();

        assert_eq!(control.status.applied_revision.load(Acquire), 0);
        assert_fresh_fence_rejection(&mut control, rejected);
        assert!(callback.held_retirement.is_some());
        assert!(weak_candidate.upgrade().is_some());

        assert_eq!(control.reclaim(), RETIRED);
        callback.begin_block();
        assert!(callback.held_retirement.is_none());
        assert!(weak_candidate.upgrade().is_some());
        assert_eq!(control.reclaim(), 1);
        assert!(weak_candidate.upgrade().is_none());

        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn command_cut_distinguishes_captured_prefix_and_stop_boundary() {
        assert_eq!(
            command_cut(7, 4),
            CommandCut {
                stop_through: Some(3),
                process_through: 7,
            }
        );
        assert_eq!(
            command_cut(2, 8),
            CommandCut {
                stop_through: Some(7),
                process_through: 7,
            }
        );
        assert_eq!(
            command_cut(7, 0),
            CommandCut {
                stop_through: None,
                process_through: 7,
            }
        );
    }

    #[test]
    fn stop_cut_suppresses_tickets_at_or_before_boundary_only() {
        for (stop_through, process_through, expected_position) in [
            (Some(2), 1, 0),
            (Some(2), 2, 0),
            (Some(2), 3, 33),
            (None, 3, 33),
        ] {
            let (mut control, mut callback) = channel(48_000).unwrap();
            assert!(control.publish(graph().0).is_ok());
            callback.begin_block();
            for frame in [11, 22, 33] {
                control.command(PlaybackCommand::Seek(frame)).unwrap();
            }
            callback.consume_commands_through(CommandCut {
                stop_through,
                process_through,
            });
            assert_eq!(control.status.position.load(Acquire), expected_position);
            assert_eq!(
                control.status.consumed_command.load(Acquire),
                process_through
            );
            control.request_shutdown();
            callback.begin_block();
            control.reclaim();
        }
    }

    #[test]
    fn full_queue_does_not_advance_admitted_ticket_and_stop_never_wraps() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        assert!(control.publish(graph().0).is_ok());
        for expected in 1..=COMMANDS as u64 {
            assert_eq!(
                control
                    .command_with_ticket(PlaybackCommand::Seek(expected))
                    .unwrap(),
                expected
            );
        }
        assert!(
            control
                .command_with_ticket(PlaybackCommand::Seek(99))
                .is_err()
        );
        assert_eq!(
            control.status.admitted_ticket.load(Acquire),
            COMMANDS as u64
        );
        callback.begin_block();
        assert_eq!(
            control.status.consumed_command.load(Acquire),
            COMMANDS as u64
        );

        control.command_ticket = u64::MAX - 1;
        control.status.admitted_ticket.store(u64::MAX - 1, Release);
        assert!(control.command_with_ticket(PlaybackCommand::Play).is_err());
        control.stop();
        assert_eq!(control.status.pending_stop_marker.load(Acquire), u64::MAX);
        callback.begin_block();
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn play_from_uses_one_ticket_and_priority_stop_cuts_it_off_under_full_queue() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        assert!(control.publish(graph().0).is_ok());
        callback.begin_block();

        let ticket = control
            .command_with_ticket(PlaybackCommand::PlayFrom(37))
            .unwrap();
        assert_eq!(ticket, 1, "seek and start are admitted as one command");
        callback.begin_block();
        assert!(control.status.playing.load(Acquire));
        assert_eq!(control.status.position.load(Acquire), 37);
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);

        control.command(PlaybackCommand::Pause).unwrap();
        callback.begin_block();
        for frame in 1..COMMANDS as u64 {
            control.command(PlaybackCommand::Seek(frame)).unwrap();
        }
        let play_from_ticket = control
            .command_with_ticket(PlaybackCommand::PlayFrom(91))
            .unwrap();
        assert_eq!(play_from_ticket, COMMANDS as u64 + 2);
        assert!(control.command(PlaybackCommand::Play).is_err());
        control.stop();
        callback.begin_block();
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.position.load(Acquire), 0);
        assert_eq!(
            control.status.consumed_command.load(Acquire),
            play_from_ticket,
            "priority Stop consumes the queued prefix while suppressing playback"
        );

        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn post_cut_admissions_remain_queued_until_the_next_actual_block() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        assert!(control.publish(graph().0).is_ok());
        callback.begin_block();
        assert!(!control.status.playing.load(Acquire));

        // Capture the actual callback cut and marker, then admit Stop twice
        // and Play after that cut. Both Stops coalesce to the same cutoff.
        let entry_ticket = control.status.admitted_ticket.load(Acquire);
        let marker = control.status.pending_stop_marker.swap(0, AcqRel);
        let cut = command_cut(entry_ticket, marker);
        assert_eq!(cut.stop_through, None);
        control.stop();
        control.stop();
        let post_cut_play = control.command_with_ticket(PlaybackCommand::Play).unwrap();

        // Run the same captured cut production uses after the swap. The later
        // Play remains queued and the post-entry Stop marker is still pending.
        callback.consume_commands_through(cut);
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.consumed_command.load(Acquire), 0);
        assert_eq!(
            control.status.pending_stop_marker.load(Acquire),
            post_cut_play
        );
        assert_eq!(callback.commands.peek().unwrap().ticket, post_cut_play);

        // The next real block captures Stop + Play and applies them in order.
        callback.begin_block();
        assert!(control.status.playing.load(Acquire));
        assert_eq!(control.status.consumed_command.load(Acquire), post_cut_play);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn derived_pcm_render_replace_and_retire_never_allocate_or_drop_on_callback() {
        fn transformed(cents: i16) -> (Playback, Weak<hex_engine::PreparedRevision>) {
            use hex_project::{Command, Created, Editor, Project};
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Imported { clip, asset, track } = editor
                .execute(Command::ImportAudioTrack {
                    name: "audio".into(),
                    path: "derived.wav".into(),
                    sample_rate: 48_000,
                    frames: 2_048,
                })
                .unwrap()
            else {
                panic!("import")
            };
            editor
                .execute(Command::SetAudioClipPitch { clip, cents })
                .unwrap();
            let Created::Track(target) = editor
                .execute(Command::AddTrack {
                    name: "routed".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            editor
                .execute(Command::SetAudioFeed {
                    source: hex_project::RoutingOwner::Track(track),
                    destination: target,
                    gain: 0.5,
                })
                .unwrap();
            editor
                .execute(Command::ReplaceTrackGainEnvelope {
                    track,
                    points: vec![hex_project::GainPoint {
                        incoming_gain: None,
                        beat: hex_project::BeatTime::ZERO,
                        gain: 0.5,
                        outgoing: hex_project::GainInterpolation::Step,
                    }],
                })
                .unwrap();
            let audio = hex_engine::Audio::new(
                48_000,
                (0..2_048)
                    .map(|i| {
                        let x = (i as f32 * 0.173).sin() * 0.2;
                        [x, -x]
                    })
                    .collect(),
            )
            .unwrap();
            // The temporary default preparation cache is destroyed here. Only the
            // returned graph owns derived Audio; no UI/source/cache masks retirement.
            let revision = Arc::new(
                hex_engine::PreparedRevision::prepare(
                    editor.project(),
                    &std::collections::BTreeMap::from([(asset, audio)]),
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let weak = Arc::downgrade(&revision);
            (
                Playback::try_from_prepared_with_session(revision).unwrap(),
                weak,
            )
        }
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut roots = Vec::new();
        let mut output = [[0.0; 2]; 31];
        for cents in [137, 211, 337, 411] {
            let (graph, weak) = transformed(cents);
            roots.push(weak);
            control.publish(graph).unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            let allocation = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            });
            assert_eq!(
                (
                    allocation.count_total,
                    allocation.bytes_total,
                    allocation.count_current,
                    allocation.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert!(
                roots.iter().all(|root| root.strong_count() > 0),
                "retired derived roots remain owned until control reclamation"
            );
        }
        control.request_shutdown();
        for _ in 0..3 {
            let allocation = allocation_counter::measure(|| {
                callback.begin_block();
            });
            assert_eq!(
                (
                    allocation.count_total,
                    allocation.bytes_total,
                    allocation.count_current,
                    allocation.bytes_current
                ),
                (0, 0, 0, 0)
            );
            control.reclaim();
        }
        assert!(roots.iter().all(|root| root.strong_count() == 0));
    }

    fn admission_gain_target() -> crate::gain_recording::Target {
        crate::gain_recording::Target {
            track_id: 1,
            project_generation: 1,
            canonical_revision: 1,
            audio_revision: 1,
            recording_epoch: 1,
            transport_epoch: 1,
            gesture_serial: 1,
            sample_rate: 48_000,
        }
    }
    #[test]
    fn split_gain_and_publication_each_reserve_before_the_other_actor_mutates() {
        for gain_first in [false, true] {
            let (mut control, mut callback) = channel(48_000).unwrap();
            control.status.split_endpoints.store(true, Release);
            if gain_first {
                control
                    .gain_recording
                    .begin(admission_gain_target(), 0.5)
                    .unwrap();
                let (playback, weak) = graph();
                let refused = control.publish(playback).unwrap_err();
                assert_eq!(control.revision, 0);
                assert_eq!(
                    control.publications.slots(),
                    control.publications.buffer().capacity()
                );
                assert!(weak.upgrade().is_some());
                drop(refused);
                assert!(weak.upgrade().is_none());
            } else {
                control.publish(graph().0).unwrap();
                assert!(
                    control
                        .gain_recording
                        .begin(admission_gain_target(), 0.5)
                        .is_err()
                );
                assert!(control.gain_recording.target().is_none());
                // Callback completion clears only publication; the next Begin
                // succeeds without any UI publication receipt collection.
                callback.begin_block();
                control
                    .gain_recording
                    .begin(admission_gain_target(), 0.5)
                    .unwrap();
            }
        }
    }
    #[test]
    fn unsplit_legacy_control_keeps_publish_then_begin_ordering() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(graph().0).unwrap();
        control
            .gain_recording
            .begin(admission_gain_target(), 0.5)
            .unwrap();
        callback.begin_block();
        assert!(!control.status.publication_pending.load(Acquire));
        assert!(!control.status.publication_pending.reserve_publication());
        // Publication completion did not clear gain's independent bit.
        assert_eq!(
            control.gain_recording.target(),
            Some(admission_gain_target())
        );
    }

    #[test]
    fn arrangement_stopped_gate_holds_play_through_install_and_stop_keeps_priority() {
        for stop_stage in 0..3 {
            let (mut control, mut callback) = channel(48_000).unwrap();
            let mut old = graph().0;
            old.apply(PlaybackCommand::Seek(8));
            let revision = control
                .publish_with_policy_and_context_generation(
                    old,
                    PublicationPolicy::InstallFreshRuntime,
                    7,
                )
                .unwrap();
            callback.begin_block();
            let gate = control.request_arrangement_edit_fence(7, revision).unwrap();
            callback.begin_block();
            assert_eq!(control.arrangement_edit_position(gate), Some(8));
            assert!(
                control.publish(graph().0).is_err(),
                "unrelated publication cannot cross gate"
            );
            let _ = control.command(PlaybackCommand::Play);
            callback.begin_block();
            assert!(!control.status.playing.load(Acquire));
            if stop_stage == 1 {
                control.stop();
                callback.begin_block();
            }
            let mut next = graph().0;
            next.apply(PlaybackCommand::Seek(16));
            let installed = control
                .publish_with_policy_and_context_generation(
                    next,
                    PublicationPolicy::InstallUnderStoppedArrangement(gate),
                    7,
                )
                .unwrap();
            let _ = control.command(PlaybackCommand::Play);
            if stop_stage == 2 {
                control.stop();
            }
            #[cfg(feature = "allocation-probe")]
            let measured = allocation_counter::measure(|| callback.begin_block());
            #[cfg(not(feature = "allocation-probe"))]
            callback.begin_block();
            #[cfg(feature = "allocation-probe")]
            {
                assert_eq!(measured.count_total, 0);
                assert_eq!(measured.bytes_total, 0);
                assert_eq!(measured.count_current, 0);
                assert_eq!(measured.bytes_current, 0);
            }
            assert_eq!(control.status.applied_revision.load(Acquire), installed);
            assert!(!control.status.playing.load(Acquire));
            let position = callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .playback
                .position();
            assert_eq!(position, if stop_stage == 0 { 16 } else { 0 });
            let _ = control.command(PlaybackCommand::Play);
            callback.begin_block();
            assert!(
                !control.status.playing.load(Acquire),
                "accepted candidate retains gate until ACK owner retires it"
            );
            control.release_stopped_edit_fence(gate);
            control.command(PlaybackCommand::Play).unwrap();
            callback.begin_block();
            assert!(control.status.playing.load(Acquire));
            control.request_shutdown();
            callback.begin_block();
            control.reclaim();
        }
    }

    #[test]
    fn live_arrangement_gate_inherits_installation_cursor_and_priority_stop() {
        for stop in [false, true] {
            let (mut control, mut callback) = channel(48_000).unwrap();
            let revision = control
                .publish_with_policy_and_context_generation(
                    graph().0,
                    PublicationPolicy::InstallFreshRuntime,
                    7,
                )
                .unwrap();
            callback.begin_block();
            control.command(PlaybackCommand::Play).unwrap();
            let gate = control
                .request_live_arrangement_edit_fence(7, revision)
                .unwrap();
            let mut pcm = [[0.0; 2]; 8];
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
            assert_eq!(control.arrangement_edit_playing(gate), Some(true));
            let captured = control.arrangement_edit_position(gate).unwrap();
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
            assert!(control.publish(graph().0).is_err());
            let installed = control
                .publish_with_policy_and_context_generation(
                    graph_with_frame([0.5; 2]).0,
                    PublicationPolicy::InheritUnderArrangement(gate),
                    7,
                )
                .unwrap();
            if stop {
                control.stop();
            }
            #[cfg(feature = "allocation-probe")]
            let measured = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut pcm).unwrap();
            });
            #[cfg(not(feature = "allocation-probe"))]
            {
                callback.begin_block();
                callback.render(&mut pcm).unwrap();
            }
            #[cfg(feature = "allocation-probe")]
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(control.status.applied_revision.load(Acquire), installed);
            assert_eq!(control.status.playing.load(Acquire), !stop);
            let active = callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap();
            if stop {
                assert_eq!(active.playback.position(), 0);
                assert_eq!(pcm, [[0.0; 2]; 8]);
            } else {
                assert!(active.playback.position() > captured + 8);
                assert_eq!(pcm, [[0.5; 2]; 8]);
            }
            control.release_stopped_edit_fence(gate);
            control.request_shutdown();
            callback.begin_block();
            control.reclaim();
        }
    }

    #[test]
    fn arrangement_stopped_gate_refuses_an_earlier_play_without_changing_runtime() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let revision = control
            .publish_with_policy_and_context_generation(
                graph().0,
                PublicationPolicy::InstallFreshRuntime,
                7,
            )
            .unwrap();
        callback.begin_block();
        control.command(PlaybackCommand::Play).unwrap();
        let gate = control.request_arrangement_edit_fence(7, revision).unwrap();
        callback.begin_block();
        assert_eq!(
            control.stopped_edit_fence_state(gate),
            StoppedEditFenceState::Refused
        );
        assert!(control.status.playing.load(Acquire));
        control.release_stopped_edit_fence(gate);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    fn graph() -> (Playback, Weak<hex_engine::Project>) {
        graph_with_frame([0.25; 2])
    }

    fn wire_action_outcomes(
        control: &mut Control,
        callback: &mut Callback,
    ) -> action_outcomes::Reader {
        control.enable_action_outcomes_on_control(callback).unwrap()
    }

    fn begin_outcome_test_block(callback: &mut Callback) {
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(|| callback.begin_block());
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        callback.begin_block();
    }

    #[test]
    fn action_outcomes_require_a_vacant_matching_constructor_and_account_storage() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (_, mut foreign) = channel(48_000).unwrap();
        assert!(
            control
                .enable_action_outcomes_on_control(&mut foreign)
                .is_err()
        );
        let before = callback.constructor_channel_storage().unwrap();
        let reader = control
            .enable_action_outcomes_on_control(&mut callback)
            .unwrap();
        let after = callback.constructor_channel_storage().unwrap();
        assert_eq!(
            after.data_bytes - before.data_bytes,
            reader.retained_data_bytes()
        );
        assert_eq!(after.shared_groups - before.shared_groups, 1);
        assert_eq!(after.shared_handles - before.shared_handles, 3);
        assert!(
            control
                .enable_action_outcomes_on_control(&mut callback)
                .is_err()
        );
        let (mut late, mut late_callback) = channel(48_000).unwrap();
        late_callback.begin_block();
        assert!(
            late.enable_action_outcomes_on_control(&mut late_callback)
                .is_err()
        );
    }

    #[test]
    fn legacy_quiescent_retirement_refuses_foreign_callback_and_interrupts_pending() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (_, mut foreign) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        control.request_shutdown();
        assert!(
            control
                .retire_action_outcomes_on_control(&mut foreign)
                .is_err()
        );
        assert_eq!(reader.try_take(), None);
        control
            .retire_action_outcomes_on_control(&mut callback)
            .unwrap();
        assert_eq!(reader.failure(), None);
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket,
                outcome: action_outcomes::Outcome::Interrupted,
            })
        );
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn abandoning_legacy_endpoint_reports_unknown_instead_of_a_terminal() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        control.command_with_ticket(PlaybackCommand::Play).unwrap();
        drop(control);
        assert_eq!(
            reader.failure(),
            Some(ActionOutcomeFailure::EndpointAbandoned)
        );
        assert_eq!(reader.try_take(), None);
        drop(callback);
    }

    #[test]
    fn resident_action_outcomes_preserve_distinct_rejections_before_polling() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        let fence = control.capture_fresh_runtime_fence().unwrap();
        control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        let no_graph = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        begin_outcome_test_block(&mut callback);
        control.reclaim();
        control.publish(graph().0).unwrap();
        let metronome = control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        begin_outcome_test_block(&mut callback);
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: no_graph,
                outcome: action_outcomes::Outcome::Rejected(
                    PlaybackActionRejection::NoActivePlayback
                )
            })
        );
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: metronome,
                outcome: action_outcomes::Outcome::Rejected(
                    PlaybackActionRejection::MetronomeUnavailable
                )
            })
        );
        assert_eq!(reader.try_take(), None);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn resident_outcomes_cross_concurrent_admission_and_callback_handoff() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
        std::thread::scope(|scope| {
            let audio = scope.spawn(|| callback.begin_block());
            barrier.wait();
            let session = control
                .session_command_with_ticket(hex_engine::SessionPlaybackCommand::Play)
                .unwrap();
            let metronome = control
                .action_with_ticket(PlaybackAction::SetMetronome(true))
                .unwrap();
            barrier.wait();
            audio.join().unwrap();
            assert_eq!(
                reader.try_take(),
                Some(action_outcomes::Receipt {
                    ticket: session,
                    outcome: action_outcomes::Outcome::Rejected(
                        PlaybackActionRejection::SessionNotPrepared
                    ),
                })
            );
            assert_eq!(
                reader.try_take(),
                Some(action_outcomes::Receipt {
                    ticket: metronome,
                    outcome: action_outcomes::Outcome::Rejected(
                        PlaybackActionRejection::MetronomeUnavailable
                    ),
                })
            );
        });
        control.request_shutdown();
        control
            .retire_action_outcomes_on_control(&mut callback)
            .unwrap();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn action_receipt_waits_for_transport_and_consumed_status() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        callback.test_before_action_outcome_status = Some(Arc::clone(&barrier));
        std::thread::scope(|scope| {
            let audio = scope.spawn(|| callback.begin_block());
            barrier.wait();
            let early = reader.try_take();
            let playing = control.status.playing.load(Acquire);
            let consumed = control.status.consumed_command.load(Acquire);
            barrier.wait();
            audio.join().unwrap();
            assert_eq!(early, None);
            assert!(!playing);
            assert!(consumed < ticket);
        });
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket,
                outcome: action_outcomes::Outcome::Applied,
            })
        );
        assert!(control.status.playing.load(Acquire));
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);
        control.request_shutdown();
        control
            .retire_action_outcomes_on_control(&mut callback)
            .unwrap();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn resident_outcome_backpressure_preserves_priority_stop_and_retries() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let mut tickets = Vec::new();
        for _ in 0..COMMANDS {
            tickets.push(control.command_with_ticket(PlaybackCommand::Play).unwrap());
        }
        assert!(control.command_with_ticket(PlaybackCommand::Play).is_err());
        control.stop();
        begin_outcome_test_block(&mut callback);
        assert!(!control.status.playing.load(Acquire));
        assert!(control.command_with_ticket(PlaybackCommand::Play).is_err());
        for ticket in tickets {
            assert_eq!(
                reader.try_take(),
                Some(action_outcomes::Receipt {
                    ticket,
                    outcome: action_outcomes::Outcome::Superseded
                })
            );
        }
        let retried = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        begin_outcome_test_block(&mut callback);
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: retried,
                outcome: action_outcomes::Outcome::Applied
            })
        );
        assert!(control.status.playing.load(Acquire));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn resident_outcomes_report_each_pre_fault_action_and_stop() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        let pause = control.command_with_ticket(PlaybackCommand::Pause).unwrap();
        let stop = control.command_with_ticket(PlaybackCommand::Stop).unwrap();
        control.status.device_fault.store(true, Release);
        begin_outcome_test_block(&mut callback);
        for ticket in [play, pause] {
            assert_eq!(
                reader.try_take(),
                Some(action_outcomes::Receipt {
                    ticket,
                    outcome: action_outcomes::Outcome::Rejected(
                        PlaybackActionRejection::OutputFailed
                    ),
                })
            );
        }
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: stop,
                outcome: action_outcomes::Outcome::Applied,
            })
        );
        assert!(!control.status.playing.load(Acquire));
        control.request_shutdown();
        control
            .retire_action_outcomes_on_control(&mut callback)
            .unwrap();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn action_outcomes_survive_graph_and_output_replacement_without_ticket_aliasing() {
        let (mut old, mut old_callback) = channel(48_000).unwrap();
        let mut old_reader = wire_action_outcomes(&mut old, &mut old_callback);
        old.publish(graph().0).unwrap();
        old_callback.begin_block();
        let first = old.command_with_ticket(PlaybackCommand::Play).unwrap();
        old.publish(graph().0).unwrap();
        old_callback.begin_block();
        let pending = old.command_with_ticket(PlaybackCommand::Pause).unwrap();
        assert!(!old_reader.is_retired());
        old.request_shutdown();
        old.retire_action_outcomes_on_control(&mut old_callback)
            .unwrap();
        assert!(old_reader.is_retired());

        let (mut replacement, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut replacement, &mut callback);
        assert_ne!(old_reader.output_identity(), reader.output_identity());
        replacement.publish(graph().0).unwrap();
        callback.begin_block();
        let replacement_ticket = replacement
            .command_with_ticket(PlaybackCommand::Play)
            .unwrap();
        callback.begin_block();
        assert_eq!(replacement_ticket, first);
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: replacement_ticket,
                outcome: action_outcomes::Outcome::Applied,
            })
        );
        assert_eq!(
            old_reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: first,
                outcome: action_outcomes::Outcome::Applied,
            })
        );
        assert_eq!(
            old_reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: pending,
                outcome: action_outcomes::Outcome::Interrupted,
            })
        );
        old_callback.begin_block();
        old.reclaim();
        replacement.request_shutdown();
        replacement
            .retire_action_outcomes_on_control(&mut callback)
            .unwrap();
        callback.begin_block();
        replacement.reclaim();
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn split_client_admission_reaches_the_actual_callback_writer() {
        use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};

        let _serial = super::candidate_tests::serial();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        let session = client
            .session_command_with_ticket(hex_engine::SessionPlaybackCommand::Play)
            .unwrap();
        let metronome = client
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        let ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
        {
            let measured = allocation_counter::measure(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block());
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: session,
                outcome: action_outcomes::Outcome::Rejected(
                    PlaybackActionRejection::SessionNotPrepared
                ),
            })
        );
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: metronome,
                outcome: action_outcomes::Outcome::Rejected(
                    PlaybackActionRejection::MetronomeUnavailable
                ),
            })
        );
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket,
                outcome: action_outcomes::Outcome::Applied
            })
        );
        let pending = client.command_with_ticket(PlaybackCommand::Pause).unwrap();
        client.close();
        publication.close();
        assert!(!retirement.advance().unwrap());
        assert_eq!(reader.try_take(), None);
        drop(data);
        assert!(!retirement.advance().unwrap());
        assert_eq!(reader.try_take(), None);
        drop(errors);
        // Both guard carriers are back, but the stream handle is not yet disposed.
        assert!(!retirement.advance().unwrap());
        assert_eq!(reader.try_take(), None);
        assert!(
            crate::lifecycle::test_dispose_backend(
                &mut Some(()),
                &mut false,
                &mut false,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        assert!(reader.is_retired());
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: pending,
                outcome: action_outcomes::Outcome::Interrupted,
            })
        );
        assert_eq!(reader.try_take(), None);
        assert_eq!(reader.failure(), None);
        super::candidate_tests::finish(&mut retirement);
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn action_outcomes_wait_for_client_after_stream_and_callback_retirement() {
        use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
        let _serial = super::candidate_tests::serial();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let root = control.native_candidate_root();
        let (data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        let ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
        publication.close();
        drop(errors);
        drop(data);
        assert!(
            !crate::lifecycle::test_dispose_backend(
                &mut Some(()),
                &mut false,
                &mut false,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        assert_eq!(reader.try_take(), None);
        assert!(
            !reader.is_retired(),
            "callback return alone is not quiescence"
        );
        client.close();
        assert!(retirement.advance().unwrap());
        assert!(
            reader.is_retired(),
            "retirement frontier must follow exact endpoint return"
        );
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket,
                outcome: action_outcomes::Outcome::Interrupted,
            })
        );
        super::candidate_tests::finish(&mut retirement);
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn retired_client_reader_stays_bound_to_its_output_after_replacement() {
        use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
        let _serial = super::candidate_tests::serial();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut old_reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let root = control.native_candidate_root();
        let (data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        let old_ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
        client.close();
        publication.close();
        drop(data);
        drop(errors);
        assert!(
            crate::lifecycle::test_dispose_backend(
                &mut Some(()),
                &mut false,
                &mut false,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        super::candidate_tests::finish(&mut retirement);

        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut new_reader = wire_action_outcomes(&mut control, &mut callback);
        assert_ne!(old_reader.output_identity(), new_reader.output_identity());
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        let new_ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
        assert_eq!(new_ticket, old_ticket);
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        assert_eq!(
            new_reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: new_ticket,
                outcome: action_outcomes::Outcome::Applied,
            })
        );
        assert_eq!(
            old_reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: old_ticket,
                outcome: action_outcomes::Outcome::Interrupted,
            })
        );
        client.close();
        publication.close();
        drop(data);
        drop(errors);
        assert!(
            crate::lifecycle::test_dispose_backend(
                &mut Some(()),
                &mut false,
                &mut false,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        super::candidate_tests::finish(&mut retirement);
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn action_outcomes_report_retirement_failure_without_inventing_a_terminal() {
        use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
        let _serial = super::candidate_tests::serial();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let root = control.native_candidate_root();
        let (data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        let ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
        retirement.fail_retained_receipts();
        assert!(!reader.is_retired());
        assert_eq!(
            reader.failure(),
            Some(ActionOutcomeFailure::RetirementUnavailable)
        );
        assert_eq!(reader.try_take(), None);
        assert!(client.command_with_ticket(PlaybackCommand::Pause).is_err());
        client.close();
        publication.close();
        drop(data);
        drop(errors);
        assert!(
            crate::lifecycle::test_dispose_backend(
                &mut Some(()),
                &mut false,
                &mut false,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        assert!(reader.is_retired());
        assert_eq!(
            reader.failure(),
            Some(ActionOutcomeFailure::RetirementUnavailable)
        );
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket,
                outcome: action_outcomes::Outcome::Interrupted,
            })
        );
        super::candidate_tests::finish(&mut retirement);
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn abandoned_action_reader_does_not_destroy_ledger_on_callback_guard_drop() {
        use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
        let _serial = super::candidate_tests::serial();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let reader = wire_action_outcomes(&mut control, &mut callback);
        control.publish(graph().0).unwrap();
        callback.begin_block();
        let root = control.native_candidate_root();
        let (data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        client.command_with_ticket(PlaybackCommand::Play).unwrap();
        drop(reader);
        client.close();
        publication.close();
        let measured = allocation_counter::measure(|| drop(data));
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        drop(errors);
        assert!(
            crate::lifecycle::test_dispose_backend(
                &mut Some(()),
                &mut false,
                &mut false,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        super::candidate_tests::finish(&mut retirement);
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn reliable_output_accepts_ordinary_stream_configuration() {
        let _serial = super::candidate_tests::serial();
        let (mut control, mut callback) = channel(48_000).unwrap();
        let _reader = wire_action_outcomes(&mut control, &mut callback);
        control.enable_native_custody().unwrap();
        control
            .configure_device_resource_publication_on_control(
                hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 256,
                    root_rows: 128,
                    max_retained_bytes: 20_000_000,
                    max_construction_peak_bytes: 20_000_000,
                },
            )
            .unwrap();
        assert!(
            control
                .configure_ordinary_streaming_on_control(
                    crate::AudioCandidateDomainReservation {
                        max_retained_data_bytes: 8 * 1024 * 1024,
                        max_construction_peak_data_bytes: 8 * 1024 * 1024,
                    },
                    3,
                    4096,
                )
                .is_ok()
        );
        assert!(control.candidate_root.ordinary_streams_configured());
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn verified_streaming_accepts_both_outcome_modes() {
        use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
        let _serial = super::candidate_tests::serial();
        for reliable in [false, true] {
            let (mut control, mut callback) = channel(48_000).unwrap();
            let _reader = reliable.then(|| wire_action_outcomes(&mut control, &mut callback));
            let root = control.native_candidate_root();
            let (data, errors, terminals, mut client, mut publication) =
                reserve_native_worker(control, callback, None);
            let mut retirement = NativeRetirement::new(terminals, root);
            let worker = publication.control().unwrap();
            worker.enable_native_custody().unwrap();
            worker
                .configure_audio_candidate_publication(
                    crate::AudioCandidatePublicationReservation {
                        runtime_slots: 2,
                        archive_slots: 2,
                        member_rows: 256,
                        transfer_rows: 256,
                        receipt_rows: 3,
                        data_rows: 4096,
                        handle_rows: 2048,
                        max_retained_data_bytes: 96 * 1024 * 1024,
                        max_construction_peak_data_bytes: 96 * 1024 * 1024,
                    },
                )
                .unwrap();
            assert!(worker.configure_audio_candidate_streaming(3).is_ok());
            client.close();
            publication.close();
            drop(data);
            drop(errors);
            assert!(
                crate::lifecycle::test_dispose_backend(
                    &mut Some(()),
                    &mut false,
                    &mut false,
                    &mut retirement,
                    |_| Ok(())
                )
                .unwrap()
            );
            super::candidate_tests::finish(&mut retirement);
        }
    }

    #[test]
    fn initial_stale_fence_rejects_play_without_active_playback() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let revision = control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        #[cfg(not(feature = "allocation-probe"))]
        callback.begin_block();
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(|| callback.begin_block());
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        assert_eq!(control.status.applied_revision.load(Acquire), 0);
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);
        assert_eq!(
            control.status.rejected_revision().map(|r| r.0),
            Some(revision)
        );
        assert_eq!(
            control.status.rejected_action(),
            Some((ticket, PlaybackActionRejection::NoActivePlayback))
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn priority_stop_discards_play_without_active_playback() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        control.stop();
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), 0);
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);
        assert_eq!(control.status.rejected_action(), None);
        assert!(!control.status.playing.load(Acquire));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    fn graph_with_frame(frame: hex_engine::Frame) -> (Playback, Weak<hex_engine::Project>) {
        let audio = hex_engine::Audio::new(48_000, vec![frame]).unwrap();
        let clip = hex_engine::Clip::new(audio, 0, 100, true).unwrap();
        let track = hex_engine::Track::new(vec![clip], 1.0, false).unwrap();
        let project = Arc::new(hex_engine::Project::new(48_000, vec![track]).unwrap());
        let weak = Arc::downgrade(&project);
        (Playback::from_shared(project), weak)
    }

    fn session_audio_graph() -> (
        Playback,
        Weak<hex_engine::PreparedRevision>,
        hex_project::ClipId,
        hex_project::TrackId,
    ) {
        use hex_project::{
            AssetSnapshot, Project as CanonicalProject, SceneSnapshot, SessionAudioClipSnapshot,
            SessionCellContentSnapshot, SessionCellSnapshot, SessionSnapshot, Snapshot, Tempo,
            TempoPoint, TrackContentSnapshot, TrackSnapshot,
        };
        let project = CanonicalProject::from_legacy_snapshot(Snapshot {
            automatic_track_names: Vec::new(),
            retained_source_paths: Vec::new(),
            aliases: Default::default(),
            native_sources: Vec::new(),
            arrangement_sections: Vec::new(),
            arrangement_locators: Vec::new(),
            main_devices: Vec::new(),
            tuning: Default::default(),
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            sidechains: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            sample_rate_mode: Default::default(),
            next_id: 5,
            assets: vec![AssetSnapshot {
                original_source: None,
                id: 4,
                path: "session.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 2,
                playback_frames: 2,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            }],
            tracks: vec![TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: 1,
                name: "audio".into(),
                gain: 1.0,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: Vec::new(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(Vec::new()),
                sends: Vec::new(),
            }],
            session: SessionSnapshot {
                scenes: vec![SceneSnapshot {
                    id: 2,
                    name: "scene".into(),
                }],
                cells: vec![SessionCellSnapshot {
                    track: 1,
                    scene: 2,
                    content: SessionCellContentSnapshot::Audio(SessionAudioClipSnapshot {
                        loop_range: None,
                        active: true,
                        flavor: hex_project::ClipFlavor::default(),
                        gain: hex_project::ClipGain::UNITY,
                        id: 3,
                        name: "one shot".into(),
                        asset: 4,
                        source_start: 0,
                        source_end: 2,
                    }),
                }],
            },
            loop_region: None,
            master_gain: 1.0,
            tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
            meter_points: vec![hex_project::MeterPoint::new(
                hex_project::BeatTime::ZERO,
                hex_project::Meter::COMMON_TIME,
            )],
            return_tracks: Vec::new(),
        })
        .unwrap();
        let audio = hex_engine::Audio::new(48_000, vec![[0.25, -0.5], [0.75, -1.0]]).unwrap();
        let media = [(project.assets().next().unwrap().id(), audio)]
            .into_iter()
            .collect();
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &media,
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let weak = Arc::downgrade(&revision);
        let clip = revision.session().audio_clips()[0].id;
        let track = revision.session().audio_clips()[0].track;
        (
            Playback::try_from_prepared_with_session(revision).unwrap(),
            weak,
            clip,
            track,
        )
    }

    #[test]
    fn pre_fault_session_launch_cannot_restart_or_change_route() {
        use hex_engine::{LaunchQuantization, PlaybackAction, SessionPlaybackCommand};
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, _, clip, _) = session_audio_graph();
        control.publish(playback).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 16]).unwrap();
        let ticket = control
            .action_with_ticket(PlaybackAction::LaunchAndPlay(
                SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        callback.report_render_fault(Some(&Invalid("injected failure")), 16);
        let mut output = [[0.0; 2]; 16];
        #[cfg(feature = "allocation-probe")]
        let allocations = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        #[cfg(not(feature = "allocation-probe"))]
        {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        }
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(
            control.status.rejected_action(),
            Some((ticket, PlaybackActionRejection::OutputFailed))
        );
        let mut routes = [SessionRouteEntry::default(); 1];
        control.status.session_routes(&mut routes).unwrap();
        assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
        assert!(routes[0].pending.is_none());
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn priority_stop_clears_pending_and_queued_session_launches_before_later_play() {
        use hex_engine::{LaunchQuantization, PlaybackAction, SessionPlaybackCommand};
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, weak, clip, track) = session_audio_graph();
        control.publish(playback).unwrap();
        callback.begin_block();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 1]).unwrap();
        let quantized = SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        };
        control
            .action_with_ticket(PlaybackAction::Session(quantized))
            .unwrap();
        callback.begin_block();
        let mut routes = [SessionRouteEntry::default(); 1];
        control.status.session_routes(&mut routes).unwrap();
        assert_eq!(routes[0].track_id, track.value());
        assert!(routes[0].pending.is_some());
        assert_eq!(
            routes[0].pending.unwrap().target,
            SessionRouteTarget::Session {
                clip_id: clip.value()
            }
        );
        let old_due = routes[0].pending.unwrap().performance_frame;
        assert_eq!(old_due, 96_000);

        let old_ticket = control
            .action_with_ticket(PlaybackAction::Session(
                SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        control.stop();
        let play_ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        assert!(play_ticket > old_ticket);
        let mut output = [[0.0; 2]; 128];
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(output, [[0.0; 2]; 128]);
        assert!(control.status.playing.load(Acquire));
        assert_eq!(control.status.consumed_command.load(Acquire), play_ticket);
        control.status.session_routes(&mut routes).unwrap();
        assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
        assert_eq!(routes[0].pending, None);

        // Cross the old due frame with real callback-sized blocks: no stale
        // Session audio or deferred launch may reappear after the Stop.
        let mut rendered = output.len() as u64;
        while rendered <= old_due {
            let n = ((old_due + 1 - rendered) as usize).min(output.len());
            let measured = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut output[..n]).unwrap();
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert!(output[..n].iter().all(|frame| *frame == [0.0; 2]));
            rendered += n as u64;
        }
        assert_eq!(control.status.position.load(Acquire), old_due + 1);
        assert_eq!(control.status.performance_frame.load(Acquire), old_due + 1);
        control.status.session_routes(&mut routes).unwrap();
        assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
        assert_eq!(routes[0].pending, None);
        control
            .action_with_ticket(PlaybackAction::Session(
                SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output[..4]).unwrap();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            &output[..4],
            &[[0.25, -0.5], [0.75, -1.0], [0.0; 2], [0.0; 2]]
        );
        control.status.session_routes(&mut routes).unwrap();
        assert_eq!(
            routes[0].owner,
            SessionRouteOwner::Session {
                clip_id: clip.value()
            }
        );
        control.stop();
        let stopped = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output[..4]).unwrap();
        });
        assert_eq!(
            (
                stopped.count_total,
                stopped.bytes_total,
                stopped.count_current,
                stopped.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(&output[..4], &[[0.0; 2]; 4]);
        control.request_shutdown();
        let shutdown = allocation_counter::measure(|| callback.begin_block());
        assert_eq!(
            (
                shutdown.count_total,
                shutdown.bytes_total,
                shutdown.count_current,
                shutdown.bytes_current
            ),
            (0, 0, 0, 0)
        );
        control.reclaim();
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn retired_session_audio_pcm_stays_owned_until_control_reclaims_publication() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, weak_revision, _, _) = session_audio_graph();
        callback.held_retirement = Some(ReturnedPublication::Legacy(Publication {
            pcm_frames: 0,
            forwarded_pcm: false,
            revision: 1,
            control_context_generation: 0,
            explicit_control_context: false,
            source_context: None,
            policy: PublicationPolicy::InheritRuntime,
            fresh_runtime_cancellation: None,
            quantized_seek_frame: None,
            streams: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_references: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_continuation: None,
            playback: Box::new(playback),
        }));
        assert!(weak_revision.upgrade().is_some());
        callback.begin_block();
        assert!(weak_revision.upgrade().is_some());
        assert_eq!(control.reclaim(), 1);
        assert!(weak_revision.upgrade().is_none());
    }

    #[test]
    fn reset_publication_drops_same_id_session_owner_after_stop_and_reclaim() {
        use hex_engine::{LaunchQuantization, SessionPlaybackCommand};

        let (mut control, mut callback) = channel(48_000).unwrap();
        let (old, old_weak, clip, track) = session_audio_graph();
        assert!(control.publish(old).is_ok());
        callback.begin_block();
        control
            .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        callback.begin_block();
        let mut first_audio = [[0.0; 2]; 8];
        callback.render(&mut first_audio).unwrap();
        assert_eq!(control.status.performance_frame.load(Acquire), 8);
        let mut old_routes = [SessionRouteEntry::default(); 1];
        let old_snapshot = control
            .status
            .session_routes(&mut old_routes)
            .expect("old active route");
        assert_eq!(old_routes[0].track_id, track.value());
        assert_eq!(
            old_routes[0].owner,
            SessionRouteOwner::Session {
                clip_id: clip.value()
            }
        );

        let (replacement, _, same_clip, same_track) = session_audio_graph();
        assert_eq!(same_clip, clip);
        assert_eq!(same_track, track);
        assert!(
            control
                .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
                .is_ok()
        );
        callback.test_transient_handoff_refusals = 1;
        callback.begin_block();
        assert!(callback.deferred.is_some());
        assert!(old_weak.upgrade().is_some());
        assert_eq!(
            old_routes[0].owner,
            SessionRouteOwner::Session {
                clip_id: clip.value()
            }
        );
        control.stop();
        callback.begin_block();
        assert!(callback.deferred.is_none());
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.performance_frame.load(Acquire), 0);
        let mut new_routes = [SessionRouteEntry::default(); 1];
        let new_snapshot = control
            .status
            .session_routes(&mut new_routes)
            .expect("replacement active route");
        assert!(new_snapshot.audio_revision > old_snapshot.audio_revision);
        assert_eq!(new_routes[0].owner, SessionRouteOwner::Arrangement);
        assert_eq!(new_routes[0].pending, None);
        assert!(old_weak.upgrade().is_some());

        let mut paused = [[1.0; 2]; 4];
        callback.render(&mut paused).unwrap();
        assert_eq!(paused, [[0.0; 2]; 4]);
        assert_eq!(control.reclaim(), 1);
        assert!(old_weak.upgrade().is_none());
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn fresh_runtime_waits_for_retirement_space_without_delaying_stop() {
        use hex_engine::{LaunchQuantization, SessionPlaybackCommand};

        let (mut control, mut callback) = channel(48_000).unwrap();
        let (old, old_weak, clip, _) = session_audio_graph();
        assert!(control.publish(old).is_ok());
        callback.begin_block();
        control
            .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 8]).unwrap();

        // Fill the real retirement ring: publication must wait for control-side
        // reclamation rather than dropping the active graph on the callback.
        let mut retired_weak = Vec::new();
        for revision in 10..10 + RETIRED as u64 {
            let (playback, weak, _, _) = session_audio_graph();
            assert!(
                callback
                    .retired
                    .push(ReturnedPublication::Legacy(Publication {
                        pcm_frames: 0,
                        forwarded_pcm: false,
                        revision,
                        control_context_generation: 0,
                        explicit_control_context: false,
                        source_context: None,
                        policy: PublicationPolicy::InheritRuntime,
                        fresh_runtime_cancellation: None,
                        quantized_seek_frame: None,
                        streams: None,
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        native_references: None,
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        native_continuation: None,
                        playback: Box::new(playback),
                    }))
                    .is_ok()
            );
            retired_weak.push(weak);
        }
        let (replacement, replacement_weak, _, _) = session_audio_graph();
        assert!(
            control
                .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
                .is_ok()
        );
        control.stop();
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        assert!(control.status.publication_pending.load(Acquire));
        assert!(!control.status.playing.load(Acquire));
        let mut routes = [SessionRouteEntry::default(); 1];
        control.status.session_routes(&mut routes).unwrap();
        assert_eq!(
            routes[0].owner,
            SessionRouteOwner::Session {
                clip_id: clip.value()
            }
        );
        let mut output = [[9.0; 2]; 8];
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 8]);
        assert!(old_weak.upgrade().is_some());
        assert!(replacement_weak.upgrade().is_some());
        assert!(retired_weak.iter().all(|weak| weak.upgrade().is_some()));

        assert_eq!(control.reclaim(), RETIRED);
        assert!(retired_weak.iter().all(|weak| weak.upgrade().is_none()));
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), 2);
        assert!(!control.status.publication_pending.load(Acquire));
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.performance_frame.load(Acquire), 0);
        control.status.session_routes(&mut routes).unwrap();
        assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
        assert_eq!(routes[0].pending, None);
        assert!(old_weak.upgrade().is_some());
        assert_eq!(control.reclaim(), 1);
        assert!(old_weak.upgrade().is_none());
        control.request_shutdown();
        callback.begin_block();
        assert!(replacement_weak.upgrade().is_some());
        assert_eq!(control.reclaim(), 1);
        assert!(replacement_weak.upgrade().is_none());
    }

    #[test]
    fn held_retirement_does_not_block_stop_or_drop_ownership() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        assert!(control.publish(graph().0).is_ok());
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        assert!(control.status.playing.load(Acquire));

        // Force the defensive retention path. Normal reserved swaps cannot
        // fill it, but it must retain ownership and honor Stop if exercised.
        let mut retired_weak = Vec::new();
        for revision in 2..=3 {
            let (playback, weak) = graph();
            assert!(
                callback
                    .retired
                    .push(ReturnedPublication::Legacy(Publication {
                        pcm_frames: 0,
                        forwarded_pcm: false,
                        revision,
                        control_context_generation: 0,
                        explicit_control_context: false,
                        source_context: None,
                        policy: PublicationPolicy::InheritRuntime,
                        fresh_runtime_cancellation: None,
                        quantized_seek_frame: None,
                        streams: None,
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        native_references: None,
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        native_continuation: None,
                        playback: Box::new(playback),
                    }))
                    .is_ok()
            );
            retired_weak.push(weak);
        }
        let (held, held_weak) = graph();
        callback.held_retirement = Some(ReturnedPublication::Legacy(Publication {
            pcm_frames: 0,
            forwarded_pcm: false,
            revision: 4,
            control_context_generation: 0,
            explicit_control_context: false,
            source_context: None,
            policy: PublicationPolicy::InheritRuntime,
            fresh_runtime_cancellation: None,
            quantized_seek_frame: None,
            streams: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_references: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_continuation: None,
            playback: Box::new(held),
        }));
        let mut last_ticket = 0;
        for _ in 0..COMMANDS {
            last_ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        }
        control.stop();
        callback.begin_block();
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.status.consumed_command.load(Acquire), last_ticket);
        assert!(callback.held_retirement.is_some());
        assert!(held_weak.upgrade().is_some());
        assert!(retired_weak.iter().all(|weak| weak.upgrade().is_some()));
        let mut output = [[9.0; 2]; 8];
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 8]);

        assert_eq!(control.reclaim(), RETIRED);
        assert!(retired_weak.iter().all(|weak| weak.upgrade().is_none()));
        control.request_shutdown();
        callback.begin_block();
        assert!(control.status.shutdown_complete.load(Acquire));
        assert!(held_weak.upgrade().is_some());
        assert_eq!(control.reclaim(), RETIRED);
        assert!(held_weak.upgrade().is_none());
    }

    #[test]
    fn transient_handoff_keeps_candidate_gated_retries_and_old_graph_renders() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (old, _) = graph();
        assert_eq!(
            control
                .publish(old)
                .unwrap_or_else(|error| panic!("{}", error.reason)),
            1
        );
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        let mut output = [[0.0; 2]; 16];
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.25; 2]; 16]);

        let (candidate, _) = graph();
        assert_eq!(
            control
                .publish(candidate)
                .unwrap_or_else(|error| panic!("{}", error.reason)),
            2
        );
        callback.test_transient_handoff_refusals = 2;
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        assert!(callback.deferred.is_some());
        assert_eq!(
            control.publish(graph().0).unwrap_err().reason,
            "playback publication already pending"
        );
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.25; 2]; 16]);

        control.stop();
        callback.begin_block();
        assert!(callback.deferred.is_some());
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        assert!(!control.status.playing.load(Acquire));
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 16]);

        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), 2);
        assert!(callback.deferred.is_none());
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.reclaim(), 1);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn rejection_snapshots_never_pair_a_new_reason_with_an_old_ticket() {
        let status = Arc::new(Status::default());
        let writer = Arc::clone(&status);
        let thread = std::thread::spawn(move || {
            for ticket in 1..=50_000 {
                let reason = if ticket & 1 == 0 {
                    PlaybackActionRejection::SessionNotPrepared
                } else {
                    PlaybackActionRejection::SessionUnavailable
                };
                writer.publish_rejected_action(ticket, reason);
            }
        });
        while !thread.is_finished() {
            if let Some((ticket, reason)) = status.rejected_action() {
                let expected = if ticket & 1 == 0 {
                    PlaybackActionRejection::SessionNotPrepared
                } else {
                    PlaybackActionRejection::SessionUnavailable
                };
                assert_eq!(reason, expected);
            }
        }
        thread.join().unwrap();
        assert_eq!(
            status.rejected_action(),
            Some((50_000, PlaybackActionRejection::SessionNotPrepared))
        );
    }

    #[test]
    fn shutdown_returns_active_deferred_and_queued_graphs_for_control_drops() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (active, active_weak) = graph();
        assert_eq!(
            control
                .publish(active)
                .unwrap_or_else(|error| panic!("{}", error.reason)),
            1
        );
        callback.begin_block();

        let (deferred, deferred_weak) = graph();
        assert_eq!(
            control
                .publish(deferred)
                .unwrap_or_else(|error| panic!("{}", error.reason)),
            2
        );
        callback.test_transient_handoff_refusals = 1;
        callback.begin_block();
        assert_eq!(
            callback.deferred.as_ref().map(|value| value.revision),
            Some(2)
        );

        // Model an already queued final owner while shutdown admission closes.
        let (queued, queued_weak) = graph();
        assert!(
            control
                .publications
                .push(QueuedPublication::Legacy(Publication {
                    pcm_frames: 0,
                    forwarded_pcm: false,
                    revision: 3,
                    control_context_generation: 0,
                    explicit_control_context: false,
                    source_context: None,
                    policy: PublicationPolicy::InheritRuntime,
                    fresh_runtime_cancellation: None,
                    quantized_seek_frame: None,
                    streams: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_references: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation: None,
                    playback: Box::new(queued),
                }))
                .is_ok()
        );
        control.request_shutdown();
        callback.begin_block();
        assert!(active_weak.upgrade().is_some());
        assert!(deferred_weak.upgrade().is_some());
        assert!(queued_weak.upgrade().is_some());
        while !control.status.shutdown_complete.load(Acquire) {
            control.reclaim();
            callback.begin_block();
        }
        control.reclaim();
        assert!(active_weak.upgrade().is_none());
        assert!(deferred_weak.upgrade().is_none());
        assert!(queued_weak.upgrade().is_none());
    }

    fn session_playback(track_id: u64) -> Playback {
        use hex_project::{
            Project as CanonicalProject, SessionSnapshot, Snapshot, Tempo, TempoPoint,
            TrackContentSnapshot, TrackSnapshot,
        };
        let project = CanonicalProject::from_legacy_snapshot(Snapshot {
            automatic_track_names: Vec::new(),
            retained_source_paths: Vec::new(),
            aliases: Default::default(),
            native_sources: Vec::new(),
            arrangement_sections: Vec::new(),
            arrangement_locators: Vec::new(),
            main_devices: Vec::new(),
            tuning: Default::default(),
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            sidechains: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            sample_rate_mode: Default::default(),
            next_id: track_id + 1,
            assets: Vec::new(),
            tracks: vec![TrackSnapshot {
                group_output_mutes: Vec::new(),
                tags: Vec::new(),
                kind: hex_project::TrackKind::Ordinary,
                parent_group: None,
                color: hex_project::ElementColor::default(),
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: track_id,
                name: "keys".into(),
                gain: 1.0,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: Vec::new(),
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Instrument(Vec::new()),
                sends: Vec::new(),
            }],
            session: SessionSnapshot {
                scenes: Vec::new(),
                cells: Vec::new(),
            },
            loop_region: None,
            master_gain: 1.0,
            tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
            meter_points: vec![hex_project::MeterPoint::new(
                hex_project::BeatTime::ZERO,
                hex_project::Meter::COMMON_TIME,
            )],
            return_tracks: Vec::new(),
        })
        .unwrap();
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        Playback::try_from_prepared_with_session(revision).unwrap()
    }

    #[test]
    fn unchanged_blocks_do_not_rescan_or_rewrite_route_slots() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        assert!(control.publish(session_playback(101)).is_ok());
        callback.begin_block();
        let sequence = control.status.route_sequence.load(SeqCst);
        let slot_visits = callback.route_slot_visits;
        let mut output = [[0.0; 2]; 128];
        callback.render(&mut output).unwrap();
        assert_eq!(control.status.route_sequence.load(SeqCst), sequence);
        assert_eq!(callback.route_slot_visits, slot_visits);
        assert_eq!(slot_visits, MAX_TRACKS);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn deferred_route_candidate_keeps_active_snapshot_until_acceptance() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        assert!(control.publish(session_playback(101)).is_ok());
        callback.begin_block();
        let mut routes = [SessionRouteEntry::default(); 1];
        let active = control.status.session_routes(&mut routes).unwrap();
        assert_eq!(active.audio_revision, 1);
        assert_eq!(routes[0].track_id, 101);

        assert!(control.publish(session_playback(202)).is_ok());
        callback.test_transient_handoff_refusals = 1;
        callback.begin_block();
        let deferred = control.status.session_routes(&mut routes).unwrap();
        assert_eq!(deferred.audio_revision, 1);
        assert_eq!(routes[0].track_id, 101);
        assert_eq!(control.status.applied_revision.load(Acquire), 1);

        callback.begin_block();
        let accepted = control.status.session_routes(&mut routes).unwrap();
        assert_eq!(accepted.audio_revision, 2);
        assert_eq!(routes[0].track_id, 202);
        assert_eq!(control.status.applied_revision.load(Acquire), 2);
        control.reclaim();
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }

    #[test]
    fn concurrent_route_reader_never_observes_mixed_revision_or_track_list() {
        use std::sync::atomic::AtomicBool;
        use std::thread;

        let (mut control, mut callback) = channel(48_000).unwrap();
        let status = Arc::clone(&control.status);
        let done = Arc::new(AtomicBool::new(false));
        let reader_done = Arc::clone(&done);
        let reader = thread::spawn(move || {
            let mut entries = [SessionRouteEntry {
                track_id: 0,
                owner: SessionRouteOwner::Silence,
                active_origin: None,
                audio_launch_frame: None,
                pending: None,
            }; 1];
            let mut observed = 0;
            while !reader_done.load(Acquire) {
                let Some(snapshot) = status.session_routes(&mut entries) else {
                    continue;
                };
                if snapshot.audio_revision == 0 {
                    assert_eq!(snapshot.count, 0);
                    assert_eq!(snapshot.capability, SessionCapabilityStatus::Legacy);
                } else {
                    assert_eq!(snapshot.count, 1);
                    assert_eq!(snapshot.capability, SessionCapabilityStatus::Available);
                    let expected_track = if snapshot.audio_revision % 2 == 1 {
                        101
                    } else {
                        202
                    };
                    assert_eq!(entries[0].track_id, expected_track);
                }
                observed += 1;
            }
            observed
        });

        for revision in 1..=64 {
            let track_id = if revision % 2 == 1 { 101 } else { 202 };
            assert_eq!(
                control
                    .publish(session_playback(track_id))
                    .unwrap_or_else(|error| panic!("{}", error.reason)),
                revision
            );
            callback.begin_block();
            control.reclaim();
        }
        done.store(true, Release);
        assert!(reader.join().unwrap() > 0);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn contextual_monitor_input_rebind_keeps_keys_across_edits_and_fresh_seek_without_history() {
        use hex_engine::computer_keyboard::ComputerKeyboardInput;
        use hex_project::{Command, Created, Editor, Project};
        // An explicit generation zero is legitimate; the legacy default zero is unknown.
        for (old_context, next_context, fresh, retained) in [
            (Some(0), Some(0), false, true),
            (Some(7), Some(7), true, true),
            (None, None, false, false),
            (Some(7), Some(8), false, false),
        ] {
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Track(track) = editor
                .execute(Command::InsertTrackWithDevice {
                    name: "keys".into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            let input = Arc::new(ComputerKeyboardInput::default());
            input.set_key(0, Some((60, 100, 1))).unwrap();
            let audition = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(
                old_context.unwrap_or(0),
            ));
            audition.set_key(0, Some((64, 100, 2))).unwrap();
            let graph = |project: &Project| {
                let revision = Arc::new(
                    hex_engine::PreparedRevision::prepare(
                        project,
                        &std::collections::BTreeMap::new(),
                        Arc::new(hex_engine::SineTable::new()),
                        None,
                    )
                    .unwrap(),
                );
                let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
                playback
                    .prepare_computer_keyboard(project, track, Arc::clone(&input))
                    .unwrap();
                playback
                    .prepare_tuning_audition(project, Arc::clone(&audition))
                    .unwrap();
                playback
            };
            let (mut control, mut callback) = channel(48_000).unwrap();
            let first = graph(editor.project());
            match old_context {
                Some(generation) => control
                    .publish_with_policy_and_context_generation(
                        first,
                        PublicationPolicy::InheritRuntime,
                        generation,
                    )
                    .unwrap(),
                None => control.publish(first).unwrap(),
            };
            let mut output = [[0.0; 2]; 64];
            callback.begin_block();
            callback.render(&mut output).unwrap();
            assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
            editor.execute(Command::Gain { track, gain: 0.7 }).unwrap();
            let next = graph(editor.project());
            let policy = if fresh {
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(
                    control.capture_fresh_runtime_fence().unwrap(),
                )
            } else {
                PublicationPolicy::InheritRuntime
            };
            let revision = match next_context {
                Some(generation) => control
                    .publish_with_policy_and_context_generation(next, policy, generation)
                    .unwrap(),
                None => control.publish_with_policy(next, policy).unwrap(),
            };
            // Queue pressure returns the candidate and cannot detach the audible input.
            let refused = control.publish(graph(editor.project())).unwrap_err();
            drop(refused);
            assert!(!input.is_cancelled());
            let measured = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            if retained {
                assert_eq!(control.status.applied_revision.load(Acquire), revision);
                assert!(!input.is_cancelled());
            } else {
                // Without a matching input-context generation, the callback
                // cannot transfer source-owned live IDs safely. Keep the old
                // graph and input installed rather than retiring their only
                // shared DSP owner or pretending the new revision applied.
                assert_eq!(control.status.applied_revision.load(Acquire), 1);
                assert_eq!(
                    control.status.rejected_revision(),
                    Some((revision, PlaybackHandoffRejection::SessionRuntime)),
                    "context={old_context:?}->{next_context:?}, fresh={fresh}"
                );
                assert!(!input.is_cancelled());
            }
            // Normal inheritance retains the existing certified audition continuation. Fresh
            // runtime installation instead rebinds only its input after the final fence cut.
            if fresh {
                assert!(!audition.is_cancelled());
            }
            assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
            control.reclaim();
            control.stop();
            callback.begin_block();
            assert!(input.is_cancelled());
            control.request_shutdown();
            callback.begin_block();
            control.reclaim();
        }
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn computer_keyboard_callback_publication_polyphony_release_pressure_stop_and_retirement_allocate_nothing()
     {
        use hex_engine::computer_keyboard::ComputerKeyboardInput;
        use hex_project::{Command, Created, Editor, Project};
        for (routed, mutable) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Track(track) = editor
                .execute(Command::InsertTrackWithDevice {
                    name: "keys".into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state()
                        .expect("fixture instrument state"),
                })
                .unwrap()
            else {
                panic!("track")
            };
            if mutable {
                let class = hex_engine::NativeDeviceClass::Instrument;
                let state = hex_engine::default_native_device_state_for_class(
                    class,
                    hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
                )
                .unwrap();
                let state = hex_engine::patch_native_device_parameter_for_class(
                    class,
                    &state,
                    hex_lattice_contract::ParameterId::from_static(78),
                    hex_lattice_contract::ParameterValue::Float(0.4),
                )
                .unwrap();
                let device = editor
                    .project()
                    .track(track)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .id();
                editor
                    .execute(Command::ReplaceDeviceState { device, state })
                    .unwrap();
            }
            if routed {
                let Created::Track(bus) = editor
                    .execute(Command::AddTrack { name: "bus".into() })
                    .unwrap()
                else {
                    panic!("bus")
                };
                editor
                    .execute(Command::SetAudioFeed {
                        source: hex_project::RoutingOwner::Track(track),
                        destination: bus,
                        gain: 0.5,
                    })
                    .unwrap();
            }
            let revision = Arc::new(
                hex_engine::PreparedRevision::prepare(
                    editor.project(),
                    &std::collections::BTreeMap::new(),
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let input = Arc::new(ComputerKeyboardInput::default());
            input.set_key(0, Some((60, 100, 1))).unwrap();
            input.set_key(1, Some((64, 80, 2))).unwrap();
            let mut playback = Playback::from_prepared(Arc::clone(&revision));
            playback
                .prepare_computer_keyboard(editor.project(), track, Arc::clone(&input))
                .unwrap();
            let (mut control, mut callback) = channel(48_000).unwrap();
            control.publish(playback).unwrap();
            let mut output = [[0.0; 2]; 256];
            let measure = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            });
            assert_eq!(
                (
                    measure.count_total,
                    measure.bytes_total,
                    measure.count_current,
                    measure.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
            input.set_key(0, None).unwrap();
            input.set_key(1, None).unwrap();
            let measure = allocation_counter::measure(|| {
                for _ in 0..if mutable { 2000 } else { 12 } {
                    callback.begin_block();
                    callback.render(&mut output).unwrap();
                }
            });
            assert_eq!(
                (
                    measure.count_total,
                    measure.bytes_total,
                    measure.count_current,
                    measure.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(output, [[0.0; 2]; 256]);
            input.set_key(0, Some((60, 100, 3))).unwrap();
            callback.begin_block();
            callback.render(&mut output).unwrap();
            for _ in 0..COMMANDS {
                control.command(PlaybackCommand::Pause).unwrap();
            }
            assert!(control.command(PlaybackCommand::Pause).is_err());
            control.stop();
            let measure = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            });
            assert_eq!(
                (
                    measure.count_total,
                    measure.bytes_total,
                    measure.count_current,
                    measure.bytes_current
                ),
                (0, 0, 0, 0)
            );
            assert!(input.is_cancelled());
            assert_eq!(output, [[0.0; 2]; 256]);
            control.publish(Playback::from_prepared(revision)).unwrap();
            let measure = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            });
            assert_eq!(
                (
                    measure.count_total,
                    measure.bytes_total,
                    measure.count_current,
                    measure.bytes_current
                ),
                (0, 0, 0, 0)
            );
            control.reclaim();
            control.request_shutdown();
            callback.begin_block();
            control.reclaim();
        }
    }
}
