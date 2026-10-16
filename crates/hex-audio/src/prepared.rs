//! Bounded ownership handoff for compiled arrangement playback.
//! Control constructs/drops graphs; callback swaps at most one per block and returns
//! old ownership through a reserved retirement queue. Used by the native prepared output.
use crate::{
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
mod admission;
mod eligible_publication;
mod endpoints;
pub use endpoints::{Client, WorkerControl};
pub(crate) use endpoints::{ClientEndpoints, EndpointTerminals};
mod runtime_view;
mod streaming;
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
    pub active_launch_beat_raw: Option<u64>,
    pub pending: Option<SessionPendingRoute>,
}

impl Default for SessionRouteEntry {
    fn default() -> Self {
        Self {
            track_id: 0,
            owner: SessionRouteOwner::Silence,
            active_launch_beat_raw: None,
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
}

struct AtomicRouteSlot {
    track_id: AtomicU64,
    owner_tag: AtomicU32,
    owner_clip_id: AtomicU64,
    active_launch_tag: AtomicU32,
    active_launch_beat_raw: AtomicU64,
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
            active_launch_tag: AtomicU32::new(0),
            active_launch_beat_raw: AtomicU64::new(0),
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
        self.active_launch_tag.store(0, SeqCst);
        self.active_launch_beat_raw.store(0, SeqCst);
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

pub(crate) struct Publication {
    revision: u64,
    /// UI/control project context this compiled playback belongs to. This is
    /// audio publication metadata, not project schema or engine state.
    control_context_generation: u64,
    policy: PublicationPolicy,
    fresh_runtime_cancellation: Option<Arc<FreshRuntimeCancellation>>,
    playback: Box<Playback>,
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
    Domain(crate::candidate_custody::DomainEnvelope),
    Eligible(crate::candidate_custody::QueuedEligibleCandidate),
    Legacy(Publication),
    Candidate(crate::candidate_custody::QueuedCandidate),
}
pub(crate) enum ReturnedPublication {
    Domain(crate::candidate_custody::DomainEnvelope),
    Eligible(crate::candidate_custody::ReturnedEligibleCandidate),
    Legacy(Publication),
    Candidate(crate::candidate_custody::ReturnedCandidate),
}
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
}

pub struct Status {
    pending_stop_marker: AtomicU64,
    admitted_ticket: AtomicU64,
    closing: AtomicBool,
    publication_pending: admission::PublicationAdmission,
    preview_used: AtomicBool,
    constructor_mode: AtomicBool,
    split_endpoints: AtomicBool,
    client_exposed: AtomicBool,
    submitted_revision: AtomicU64,
    pub device_fault: AtomicBool,
    pub render_fault: AtomicBool,
    render_fault_record: crate::render_fault::RenderFaultRecord,
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
    /// Latest streamed launch accepted into actual PendingRoute scheduling.
    pub stream_scheduled_command: AtomicU64,
    /// Latest scheduled launch whose actual due route/render chunk committed.
    pub stream_activated_command: AtomicU64,
    /// Latest schedule observation cancelled or not retained by actual handoff.
    /// This does not claim that an already committed musical route was undone.
    pub stream_invalidated_schedule: AtomicU64,
    pub stream_readiness_refused: AtomicBool,
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
    pub position: AtomicU64,
    pub performance_frame: AtomicU64,
    pub transport_epoch: AtomicU64,
    pub gain_recording_fault: AtomicBool,
    pub gain_recording_ready: AtomicBool,
    pub playing: AtomicBool,
    /// Last callback-admitted interactive click state.
    pub metronome_enabled: AtomicBool,
    pub session_capability: AtomicU32,
    pub shutdown_complete: AtomicBool,
    route_sequence: AtomicU64,
    route_revision: AtomicU64,
    route_count: AtomicU32,
    route_capability: AtomicU32,
    route_generation: AtomicU64,
    route_performance_at_update: AtomicU64,
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
        !self.constructor_mode.load(Acquire) && !self.closing.load(Acquire)
    }
    pub(crate) fn mark_preview_used(&self) {
        self.preview_used.store(true, Release);
    }
    pub(crate) fn preview_unused(&self) -> bool {
        !self.preview_used.load(Acquire)
    }
    pub(crate) fn reserve_gain_begin(&self) -> Result<bool, &'static str> {
        if self.closing.load(Acquire) {
            return Err("gain recording output is closing");
        }
        if self.constructor_mode.load(Acquire) {
            return Err("gain recording is unavailable for streamed documents");
        }
        if self.split_endpoints.load(Acquire) {
            if !self.publication_pending.reserve_gain() {
                return Err("playback publication or gain recording is pending");
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub(crate) fn gain_begin_committed(&self) {
        self.publication_pending.legacy_gain_started();
    }
    pub(crate) fn gain_end(&self) {
        self.publication_pending.gain_finished();
    }
    pub(crate) fn candidate_boundary_data_bytes(&self) -> Option<usize> {
        self.route_slots
            .len()
            .checked_mul(std::mem::size_of::<AtomicRouteSlot>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of_val(self)))
    }
    fn with_route_slots(route_slots: Box<[AtomicRouteSlot]>) -> Self {
        Self {
            pending_stop_marker: AtomicU64::new(0),
            admitted_ticket: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            publication_pending: admission::PublicationAdmission::default(),
            preview_used: AtomicBool::new(false),
            constructor_mode: AtomicBool::new(false),
            split_endpoints: AtomicBool::new(false),
            client_exposed: AtomicBool::new(false),
            submitted_revision: AtomicU64::new(0),
            device_fault: AtomicBool::new(false),
            render_fault: AtomicBool::new(false),
            render_fault_record: crate::render_fault::RenderFaultRecord::default(),
            callback_max_ns: AtomicU64::new(0),
            deadline_misses: AtomicU64::new(0),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            applied_revision: AtomicU64::new(0),
            consumed_command: AtomicU64::new(0),
            stream_applied_command_prefix: AtomicU64::new(0),
            stream_buffering: AtomicBool::new(false),
            stream_scheduled_command: AtomicU64::new(0),
            stream_activated_command: AtomicU64::new(0),
            stream_invalidated_schedule: AtomicU64::new(0),
            stream_readiness_refused: AtomicBool::new(false),
            stream_committed_frames: AtomicU64::new(0),
            rejected_action_ticket: AtomicU64::new(0),
            rejected_action_reason: AtomicU32::new(0),
            rejected_action_sequence: AtomicU64::new(0),
            rejected_revision: AtomicU64::new(0),
            rejected_revision_reason: AtomicU32::new(0),
            rejected_revision_sequence: AtomicU64::new(0),
            position: AtomicU64::new(0),
            performance_frame: AtomicU64::new(0),
            transport_epoch: AtomicU64::new(1),
            gain_recording_fault: AtomicBool::new(false),
            gain_recording_ready: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            metronome_enabled: AtomicBool::new(false),
            session_capability: AtomicU32::new(SessionCapabilityStatus::Legacy as u32),
            shutdown_complete: AtomicBool::new(false),
            route_sequence: AtomicU64::new(0),
            route_revision: AtomicU64::new(0),
            route_count: AtomicU32::new(0),
            route_capability: AtomicU32::new(SessionCapabilityStatus::Legacy as u32),
            route_generation: AtomicU64::new(0),
            route_performance_at_update: AtomicU64::new(0),
            route_slots,
        }
    }
}

impl Status {
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
            if count > output.len() || count > self.route_slots.len() {
                return None;
            }
            for (index, destination) in output.iter_mut().take(count).enumerate() {
                let slot = self.route_slots.get(index)?;
                let track_id = slot.track_id.load(SeqCst);
                let owner_tag = slot.owner_tag.load(SeqCst);
                let owner_clip_id = slot.owner_clip_id.load(SeqCst);
                let active_launch_tag = slot.active_launch_tag.load(SeqCst);
                let active_launch_beat_raw = slot.active_launch_beat_raw.load(SeqCst);
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
                let active_launch_beat_raw = match active_launch_tag {
                    0 => None,
                    1 => Some(active_launch_beat_raw),
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
                    active_launch_beat_raw,
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
                });
            }
        }
        None
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
    ) {
        self.route_revision.store(revision, SeqCst);
        self.route_count.store(count as u32, SeqCst);
        self.route_capability.store(capability as u32, SeqCst);
        self.route_generation.store(generation, SeqCst);
        self.route_performance_at_update
            .store(performance_at_update, SeqCst);
    }

    fn publish_empty_routes(&self) {
        self.metronome_enabled.store(false, Relaxed);
        self.begin_route_snapshot();
        for slot in self.route_slots.iter() {
            slot.clear();
        }
        self.publish_route_header(0, 0, SessionCapabilityStatus::Legacy, 0, 0);
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
        if let Some(beat) = route.active_launch {
            slot.active_launch_beat_raw.store(beat.raw(), SeqCst);
            slot.active_launch_tag.store(1, SeqCst);
        } else {
            slot.active_launch_beat_raw.store(0, SeqCst);
            slot.active_launch_tag.store(0, SeqCst);
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

/// Failed publication retains the candidate so the caller can retry or dispose
/// of it off the callback. Revisions advance only after successful publication.
pub struct PublishError {
    pub reason: &'static str,
    pub playback: Playback,
}

impl std::fmt::Debug for PublishError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublishError")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Legacy unsplit control. Native worker composition consumes this exactly once
/// before delivering its nonowning client. Resident callers retain these APIs.
pub struct Control {
    pub sample_preview: crate::sample_preview::Control,
    commands: Producer<QueuedCommand>,
    pub spectrum_target: spectrum_target::SpectrumTargetControl,
    pub processor_audition: processor_audition::ProcessorAuditionTargetControl,
    pub gain_recording: crate::gain_recording::Control,
    command_ticket: u64,
    publication: PublicationControl,
}
/// The unique worker-side publication/return endpoint; never a UI command client.
pub struct PublicationControl {
    publications: Producer<QueuedPublication>,
    retired: Option<Consumer<ReturnedPublication>>,
    candidate_root: crate::candidate_custody::CandidateRoot,
    native_custody: bool,
    held_control_return: Option<ReturnedPublication>,
    pub status: Arc<Status>,
    rate: u32,
    revision: u64,
    command_storage_bytes: usize,
    #[cfg(test)]
    endpoint_probe: Option<crate::native_callback_owner::DropProbe>,
    latest_fresh_runtime_cancellation: Option<Arc<FreshRuntimeCancellation>>,
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

pub struct Callback {
    sample_preview: crate::sample_preview::Callback,
    publications: Consumer<QueuedPublication>,
    retired: Producer<ReturnedPublication>,
    candidate_root: crate::candidate_custody::CandidateRoot,
    commands: Consumer<QueuedCommand>,
    status: Arc<Status>,
    rate: u32,
    spectrum_target: spectrum_target::SpectrumTargetCallback,
    processor_audition: processor_audition::ProcessorAuditionTargetCallback,
    processor_audition_lease: Option<processor_audition::ProcessorAuditionTarget>,
    processor_audition_rejected_epoch: u64,
    gain_recording: crate::gain_recording::Callback,
    spectrum_capture: Option<SpectrumCapture>,
    spectrum_stage: SpectrumStage,
    active: Option<ActivePublication>,
    #[cfg(test)]
    test_admit_candidate: bool,
    deferred: Option<Publication>,
    deferred_eligible: Option<crate::candidate_custody::QueuedEligibleCandidate>,
    domain_pages: [Option<crate::candidate_custody::DomainEnvelope>; 2],
    stream_pending_play: Option<u64>,
    stream_prefix: u64,
    // Defensive retention if a push unexpectedly fails; never drop graph ownership.
    held_retirement: Option<ReturnedPublication>,
    terminal_disposal_pending: bool,
    published_route_key: Option<(u64, u64)>,
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
    if !(8_000..=192_000).contains(&rate) {
        return Err(Invalid("unsupported playback sample rate"));
    }
    let (mut sample_preview, callback_preview) = crate::sample_preview::channel(rate);
    let (publications, incoming) = RingBuffer::new(1);
    let (returning, retired) = RingBuffer::new(RETIRED);
    let (commands, events) = RingBuffer::new(COMMANDS);
    let status = Arc::new(Status::try_new()?);
    let (spectrum_target, callback_target) = spectrum_target::channel();
    let (processor_audition, callback_audition) = processor_audition::channel();
    let (mut gain_recording, callback_gain_recording) = crate::gain_recording::channel();
    sample_preview.bind_status(status.clone());
    gain_recording.bind_status(status.clone());
    let candidate_root = crate::candidate_custody::CandidateRoot::new(RETIRED).map_err(Invalid)?;
    let command_storage_bytes = commands
        .buffer()
        .capacity()
        .checked_mul(std::mem::size_of::<QueuedCommand>())
        .and_then(|n| n.checked_add(std::mem::size_of_val(commands.buffer())))
        .ok_or(Invalid("command queue storage overflow"))?;
    Ok((
        Control {
            sample_preview,
            commands,
            spectrum_target,
            processor_audition,
            gain_recording,
            command_ticket: 0,
            publication: PublicationControl {
                candidate_root: candidate_root.clone(),
                native_custody: false,
                held_control_return: None,
                publications,
                retired: Some(retired),
                status: status.clone(),
                rate,
                revision: 0,
                command_storage_bytes,
                #[cfg(test)]
                endpoint_probe: None,
                latest_fresh_runtime_cancellation: None,
            },
        },
        Callback {
            sample_preview: callback_preview,
            candidate_root,
            publications: incoming,
            retired: returning,
            commands: events,
            status,
            rate,
            spectrum_target: callback_target,
            processor_audition: callback_audition,
            processor_audition_lease: None,
            processor_audition_rejected_epoch: 0,
            gain_recording: callback_gain_recording,
            spectrum_capture: capture,
            spectrum_stage: SpectrumStage::default(),
            active: None,
            #[cfg(test)]
            test_admit_candidate: false,
            deferred: None,
            deferred_eligible: None,
            domain_pages: std::array::from_fn(|_| None),
            stream_pending_play: None,
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
        },
    ))
}

struct SpectrumStage {
    frames: [[f32; 2]; 256],
    seen: [u64; 4],
    expected: usize,
    observed: bool,
    valid: bool,
    target: Option<spectrum_target::SpectrumTarget>,
    epoch: u64,
}

impl Default for SpectrumStage {
    fn default() -> Self {
        Self {
            frames: [[0.0; 2]; 256],
            seen: [0; 4],
            expected: 0,
            observed: false,
            valid: false,
            target: None,
            epoch: 0,
        }
    }
}

impl SpectrumStage {
    fn begin(&mut self, target: spectrum_target::SpectrumTarget, epoch: u64, expected: usize) {
        self.frames.fill([0.0; 2]);
        self.seen.fill(0);
        self.expected = expected;
        self.observed = false;
        self.valid = expected <= self.frames.len();
        self.target = Some(target);
        self.epoch = epoch;
    }

    fn observe(&mut self, identity: PostNodeIdentity, offset: usize, frames: &[[f32; 2]]) {
        let Some(target) = self.target else {
            return;
        };
        if identity != target.identity {
            return;
        }
        self.observed = true;
        let Some(end) = offset.checked_add(frames.len()) else {
            self.valid = false;
            return;
        };
        if end > self.expected || end > self.frames.len() {
            self.valid = false;
            return;
        }
        for (relative, frame) in frames.iter().enumerate() {
            let index = offset + relative;
            let word = index / 64;
            let mask = 1_u64 << (index % 64);
            let Some(bits) = self.seen.get_mut(word) else {
                self.valid = false;
                return;
            };
            if *bits & mask != 0 {
                self.valid = false;
                return;
            }
            *bits |= mask;
            self.frames[index] = *frame;
        }
    }

    fn complete(&self) -> bool {
        self.valid
            && self.observed
            && self.target.is_some()
            && (0..self.expected).all(|index| self.seen[index / 64] & (1_u64 << (index % 64)) != 0)
    }

    fn clear(&mut self) {
        self.target = None;
        self.expected = 0;
        self.observed = false;
        self.valid = false;
        self.seen.fill(0);
    }
}

impl PublicationControl {
    /// Capture the current route header under its sequence lock, then acquire
    /// the admitted-command high-water mark. Returns `None` on a contended or
    /// invalid route snapshot so callers can retry before preparing a graph.
    pub fn capture_fresh_runtime_fence(&self) -> Option<FreshRuntimeFence> {
        let mut routes = [SessionRouteEntry::default(); SESSION_ROUTE_CAPACITY];
        let snapshot = self.status.session_routes(&mut routes)?;
        Some(FreshRuntimeFence {
            expected_active_revision: snapshot.audio_revision,
            route_state_generation: snapshot.route_state_generation,
            admitted_command_ticket: self.status.admitted_ticket.load(Acquire),
        })
    }

    pub fn publish(&mut self, playback: Playback) -> Result<u64, PublishError> {
        self.publish_with_policy(playback, PublicationPolicy::InheritRuntime)
    }

    pub fn publish_with_policy(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
    ) -> Result<u64, PublishError> {
        self.publish_with_policy_and_context_generation(playback, policy, 0)
    }

    /// Publish a prepared graph tagged with the control-side project context
    /// that produced it. Existing callers retain generation zero through
    /// `publish` and `publish_with_policy`.
    pub fn publish_with_policy_and_context_generation(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
    ) -> Result<u64, PublishError> {
        let reason = if self.candidate_root.domains_configured() {
            Some("constructor publication does not adopt legacy playback")
        } else if self.status.publication_pending.gain_active() {
            Some("finish gain recording before replacing playback")
        } else if self.status.closing.load(Acquire) {
            Some("playback is shutting down")
        } else if playback.sample_rate() != self.rate {
            Some("playback rate differs from device")
        } else if playback.contains_streamed_sources() {
            Some("streamed playback requires a native media page endpoint")
        } else if self.revision == u64::MAX {
            Some("playback revision exhausted")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(PublishError { reason, playback });
        }
        if !self.status.publication_pending.reserve_publication() {
            return Err(PublishError {
                reason: "playback publication already pending",
                playback,
            });
        }
        let revision = self.revision + 1;
        let cancellation = matches!(policy, PublicationPolicy::InstallFreshRuntimeIfUnchanged(_))
            .then(|| {
                Arc::new(FreshRuntimeCancellation {
                    revision,
                    decision: AtomicU32::new(FRESH_PUBLICATION_PENDING),
                })
            });
        let mut playback = playback;
        playback.set_device_metering(true);
        let publication = Publication {
            revision,
            control_context_generation,
            policy,
            fresh_runtime_cancellation: cancellation.as_ref().map(Arc::clone),
            playback: Box::new(playback),
        };
        if let Err(publication) = enqueue_legacy(&mut self.publications, publication) {
            self.status.publication_pending.store(false, Release);
            return Err(PublishError {
                reason: "playback publication queue full",
                playback: *publication.playback,
            });
        }
        self.revision = revision;
        self.status.submitted_revision.store(revision, Release);
        self.latest_fresh_runtime_cancellation = cancellation;
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
                ReturnedPublication::Legacy(publication) => drop(publication),
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
        if self.status.client_exposed.load(Acquire)
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
    pub fn stop(&self) {
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
        if self.status.split_endpoints.load(Acquire) {
            self.request_shutdown();
        }
        if !self.native_custody {
            return;
        }
        self.candidate_root
            .abandon_control(self.retired.take(), self.held_control_return.take());
        #[cfg(test)]
        drop(self.endpoint_probe.take());
    }
}

impl Callback {
    pub(crate) fn constructor_channel_storage(
        &self,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        self.sample_preview
            .constructor_channel_storage()?
            .plus(self.gain_recording.constructor_channel_storage()?)?
            .plus(self.spectrum_target.constructor_channel_storage()?)?
            .plus(self.processor_audition.constructor_channel_storage()?)
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
    pub(crate) fn matches_native_status(&self, status: &Arc<Status>) -> bool {
        Arc::ptr_eq(&self.status, status)
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
        if let Some(ActivePublication::Candidate(active)) = self.active.as_mut() {
            if active.owner.test_apply_action(action).is_err() {
                self.status.device_fault.store(true, Relaxed);
            }
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
                    .or_else(|| {
                        self.publications.pop().ok().map(|queued| match queued {
                            QueuedPublication::Domain(pages) => ReturnedPublication::Domain(pages),
                            QueuedPublication::Eligible(value) => {
                                ReturnedPublication::Eligible(value.reject_before_install())
                            }
                            QueuedPublication::Legacy(value) => ReturnedPublication::Legacy(value),
                            QueuedPublication::Candidate(value) => {
                                ReturnedPublication::Candidate(value.reject_before_install())
                            }
                        })
                    });
            }
            if matches!(
                self.held_retirement.as_ref(),
                Some(ReturnedPublication::Legacy(_))
            ) {
                self.terminal_disposal_pending = true;
                drop(self.held_retirement.take());
                self.terminal_disposal_pending = false;
            } else if let Some(value) = self.held_retirement.take() {
                let refused = match value {
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
        self.return_publication(ReturnedPublication::Legacy(publication));
    }
    fn return_publication(&mut self, publication: ReturnedPublication) {
        if let Err(PushError::Full(publication)) = self.retired.push(publication) {
            self.held_retirement = Some(publication);
        }
    }
    /// At most one replacement and 32 commands; no graph disposal or allocation.
    pub fn begin_block(&mut self) {
        self.sample_preview
            .begin_block(self.status.closing.load(Acquire));
        if self.status.closing.load(Acquire) {
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
            if !self.retired.is_full() {
                if let Some(publication) = self.deferred.take() {
                    if let Some(cancellation) = publication.fresh_runtime_cancellation.as_ref() {
                        if cancellation.finish_for_shutdown() {
                            self.status.publish_rejected_revision(
                                publication.revision,
                                PlaybackHandoffRejection::FreshRuntimeCancelled,
                            );
                        }
                    }
                    self.return_graph(publication);
                    self.status.publication_pending.store(false, Release);
                }
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
            if !self.retired.is_full() {
                if let Ok(publication) = self.publications.pop() {
                    let publication = match publication {
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
                    if let Some(cancellation) = publication.fresh_runtime_cancellation.as_ref() {
                        if cancellation.finish_for_shutdown() {
                            self.status.publish_rejected_revision(
                                publication.revision,
                                PlaybackHandoffRejection::FreshRuntimeCancelled,
                            );
                        }
                    }
                    self.return_graph(publication);
                    self.status.publication_pending.store(false, Release);
                }
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
                QueuedPublication::Domain(pages) => {
                    if let Some(index) = pages.index_for(self.candidate_root.root_id())
                        && self.domain_pages[index].is_none()
                    {
                        self.domain_pages[index] = Some(pages);
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

        // A candidate with a transient release-capacity refusal remains owned
        // here while the previous runtime continues to render and drain tails.
        // The queue gate prevents a second graph from being admitted meanwhile.
        if (self.active.is_none() || !self.retired.is_full())
            && let Some(mut next) = self.deferred.take()
        {
            #[cfg(test)]
            let forced_refusal = self.test_transient_handoff_refusals > 0;
            #[cfg(test)]
            if forced_refusal {
                self.test_transient_handoff_refusals -= 1;
            }
            #[cfg(not(test))]
            let forced_refusal = false;
            let stale_fence = match next.policy {
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence) => {
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
                        || stop_pending)
                        .then_some(hex_engine::PlaybackHandoffError::RuntimeFenceChanged)
                }
                PublicationPolicy::InheritRuntime | PublicationPolicy::InstallFreshRuntime => None,
            };
            #[cfg(test)]
            if stale_fence.is_none()
                && matches!(
                    next.policy,
                    PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                )
                && let Some(barrier) = self.test_after_fresh_fence_validation.take()
            {
                barrier.wait();
                barrier.wait();
            }
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
            ) {
                None
            } else {
                self.active
                    .as_mut()
                    .and_then(ActivePublication::legacy_mut)
                    .map(|previous| {
                        next.playback
                            .try_inherit_runtime_from(&mut previous.playback)
                    })
            };
            match handoff {
                Some(Err(error)) if error.is_transient() => {
                    self.deferred = Some(next);
                }
                Some(Err(error)) => {
                    if let Some(cancellation) = next.fresh_runtime_cancellation.as_ref() {
                        cancellation.finish();
                    }
                    if let Some(reason) = error.rejection() {
                        self.status.publish_rejected_revision(next.revision, reason);
                    }
                    self.return_graph(next);
                    self.status.publication_pending.store(false, Release);
                }
                _ => {
                    #[cfg(test)]
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
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
                        if let Some(cancellation) = next.fresh_runtime_cancellation.as_ref() {
                            cancellation.finish();
                        }
                        self.status.publish_rejected_revision(
                            next.revision,
                            PlaybackHandoffRejection::FreshRuntimeCancelled,
                        );
                        self.return_graph(next);
                        self.status.publication_pending.store(false, Release);
                        self.consume_commands();
                        return;
                    }
                    #[cfg(test)]
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                    ) && let Some(barrier) = self.test_after_fresh_runtime_apply_decision.take()
                    {
                        barrier.wait();
                        barrier.wait();
                    }
                    // A fresh seek keeps its reconstructed transport/DSP
                    // history. Carry only the transient click toggle after
                    // every fence/cancellation refusal has been excluded.
                    if matches!(
                        next.policy,
                        PublicationPolicy::InstallFreshRuntimeIfUnchanged(_)
                    ) && let Some(previous) =
                        self.active.as_ref().and_then(ActivePublication::legacy)
                        && next.control_context_generation == previous.control_context_generation
                    {
                        next.playback
                            .inherit_metronome_for_fresh_seek(&previous.playback);
                    }
                    if let Some(previous) =
                        self.active.as_mut().and_then(ActivePublication::legacy_mut)
                    {
                        previous.playback.cancel_computer_keyboard();
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
                        self.consume_commands();
                        return;
                    }
                    if let Some(ActivePublication::Legacy(previous)) =
                        self.active.replace(ActivePublication::Legacy(next))
                    {
                        self.return_graph(previous);
                    }
                    if let Some(cancellation) = self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::legacy)
                        .and_then(|active| active.fresh_runtime_cancellation.as_ref())
                    {
                        cancellation.finish();
                    }
                    self.publish_active_routes(true);
                    self.status.render_fault_record.clear();
                    self.status.render_fault.store(false, Release);
                    self.status.applied_revision.store(revision, Release);
                    self.status.publication_pending.store(false, Release);
                }
            }
        }
        self.consume_commands();
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
            return;
        }
        // Capture the published queue prefix before clearing the marker. Later
        // producer admissions stay queued for the next block; a Stop arriving
        // after this swap also remains pending for the next block.
        let entry_ticket = self.status.admitted_ticket.load(Acquire);
        let stop_marker = self.status.pending_stop_marker.swap(0, AcqRel);
        self.consume_commands_through(command_cut(entry_ticket, stop_marker));
    }

    fn consume_commands_through(&mut self, cut: CommandCut) {
        if cut.stop_through.is_some() {
            #[cfg(all(test, feature = "allocation-probe"))]
            self.apply_candidate_action(PlaybackAction::Transport(PlaybackCommand::Stop));
            self.invalidate_gain_recording(crate::gain_recording::AbortReason::TransportChanged);
            if let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
            {
                active.playback.apply(PlaybackCommand::Stop);
            }
            self.invalidate_processor_audition_request();
        }
        let mut consumed = None;
        for _ in 0..COMMANDS {
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
            if cut
                .stop_through
                .is_none_or(|stop_through| queued.ticket > stop_through)
            {
                #[cfg(all(test, feature = "allocation-probe"))]
                self.apply_candidate_action(queued.action);
                let mut invalidate_audition = false;
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::current_mut)
                {
                    match active.playback.try_apply_action(queued.action) {
                        Ok(()) => {
                            invalidate_audition =
                                action_invalidates_processor_audition(queued.action);
                        }
                        Err(error) => self
                            .status
                            .publish_rejected_action(queued.ticket, error.telemetry_rejection()),
                    }
                }
                if invalidate_audition
                    || matches!(
                        queued.action,
                        PlaybackAction::Transport(PlaybackCommand::Pause)
                            | PlaybackAction::Session(hex_engine::SessionPlaybackCommand::Pause)
                    )
                {
                    self.invalidate_gain_recording(
                        crate::gain_recording::AbortReason::TransportChanged,
                    );
                }
                if invalidate_audition {
                    self.invalidate_processor_audition_request();
                }
            }
        }
        self.publish_transport();
        self.publish_active_routes(false);
        if let Some(ticket) = consumed {
            self.status.consumed_command.store(ticket, Release);
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
        if !self.status.closing.load(Acquire)
            && self
                .deferred_eligible
                .as_ref()
                .is_some_and(|next| next.streaming_enabled())
        {
            self.advance_eligible_publication(Some(output.len()));
        }
        self.render_project(output)?;
        if !self.status.closing.load(Acquire) {
            self.sample_preview.mix(output);
        }
        Ok(())
    }

    fn render_project(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
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
        }
        self.reconcile_processor_audition();
        let Some(target) = self
            .spectrum_capture
            .as_ref()
            .and(self.spectrum_target.current())
        else {
            self.spectrum_stage.clear();
            return self.render_unobserved(output);
        };
        let epoch = self.spectrum_target.current_epoch();
        let active_matches_target = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .is_some_and(|active| {
                active.control_context_generation == target.project_generation
                    && active.revision >= target.activation_floor
                    && active.playback.sample_rate() == target.sample_rate_hz
            });
        if target.sample_rate_hz != self.rate || !active_matches_target {
            self.spectrum_stage.begin(target, epoch, output.len());
            self.spectrum_stage.valid = false;
            return self.render_unobserved(output);
        }
        self.spectrum_stage.begin(target, epoch, output.len());
        if self.status.closing.load(Acquire) {
            output.fill([0.0; 2]);
            return Ok(());
        }
        let stage = &mut self.spectrum_stage;
        let mut observe =
            |identity, offset, frames: &[Frame]| stage.observe(identity, offset, frames);
        if let Some(ActivePublication::Eligible(active)) = self.active.as_mut()
            && active.contains_streamed_sources()
        {
            let outcome = active.render_streamed(
                &mut self.domain_pages,
                self.stream_pending_play.is_some(),
                self.stream_prefix,
                output,
                Some(&mut observe),
            );
            streaming::record_outcome(
                &self.status,
                &mut self.stream_pending_play,
                self.stream_prefix,
                active.has_stream_action(),
                outcome,
            )?;
        } else if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active
                .playback
                .render_with_post_node_observer(output, Some(&mut observe))?;
        } else {
            output.fill([0.0; 2]);
        }
        self.publish_transport();
        self.publish_active_routes(false);
        Ok(())
    }

    fn render_unobserved(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        if self.status.closing.load(Acquire) {
            output.fill([0.0; 2]);
            return Ok(());
        }
        if let Some(ActivePublication::Eligible(active)) = self.active.as_mut()
            && active.contains_streamed_sources()
        {
            let outcome = active.render_streamed(
                &mut self.domain_pages,
                self.stream_pending_play.is_some(),
                self.stream_prefix,
                output,
                None,
            );
            streaming::record_outcome(
                &self.status,
                &mut self.stream_pending_play,
                self.stream_prefix,
                active.has_stream_action(),
                outcome,
            )?;
            self.publish_transport();
            self.publish_active_routes(false);
            return Ok(());
        }
        if self.active.is_none()
            && self
                .deferred_eligible
                .as_ref()
                .is_some_and(|next| next.streaming_enabled())
        {
            output.fill([0.0; 2]);
            streaming::record_outcome(
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
                    fault: None,
                    action_rejection: None,
                    activated: None,
                },
            )?;
            return Ok(());
        }
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.render(output)?;
        } else {
            output.fill([0.0; 2]);
        }
        self.publish_transport();
        self.publish_active_routes(false);
        Ok(())
    }

    /// Accept or reject the staged post-node signal after the caller has
    /// validated the complete final output block. A skipped attempt consumes a
    /// capture sequence so the next accepted window cannot bridge the gap.
    pub fn finish_spectrum_block(
        &mut self,
        captured_start: Instant,
        output_valid: bool,
    ) -> Option<crate::spectrum_worker::CaptureOutcome> {
        let target = self.spectrum_stage.target?;
        let complete = self.spectrum_stage.complete();
        let current = self.spectrum_target.current();
        let current_epoch = self.spectrum_target.current_epoch();
        let valid = output_valid
            && complete
            && current == Some(target)
            && current_epoch == self.spectrum_stage.epoch
            && self
                .active
                .as_ref()
                .and_then(ActivePublication::current)
                .is_some_and(|active| {
                    active.control_context_generation == target.project_generation
                        && active.revision >= target.activation_floor
                        && active.playback.sample_rate() == target.sample_rate_hz
                })
            && target.sample_rate_hz == self.rate;
        let outcome = if let Some(capture) = self.spectrum_capture.as_mut() {
            if valid {
                let stamp = SpectrumStamp {
                    target: target.token,
                    generation: target.project_generation,
                    sample_rate_hz: target.sample_rate_hz,
                    applied_revision: self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::current)
                        .map_or(0, |active| active.revision),
                };
                Some(capture.submit_at(
                    stamp,
                    &self.spectrum_stage.frames[..self.spectrum_stage.expected],
                    captured_start,
                ))
            } else {
                Some(capture.skip())
            }
        } else {
            None
        };
        self.spectrum_stage.clear();
        outcome
    }

    fn invalidate_gain_recording(&mut self, reason: crate::gain_recording::AbortReason) {
        self.gain_recording.abort(reason);
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.clear_gain_recording_override();
        }
        if self
            .status
            .transport_epoch
            .fetch_update(AcqRel, Acquire, |epoch| epoch.checked_add(1))
            .is_err()
        {
            self.status.gain_recording_fault.store(true, Release);
        }
    }
    fn prepare_gain_recording_quantum(&mut self) {
        use crate::gain_recording::{AbortReason, Clock};
        let transport_epoch = self.status.transport_epoch.load(Acquire);
        let active = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut);
        let live = self.gain_recording.prepare(|target| {
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
        if let Some(mut active) = active {
            active.playback.clear_gain_recording_override();
            if let Some((track, gain)) = live {
                if active
                    .playback
                    .set_gain_recording_override(track, gain)
                    .is_err()
                {
                    self.gain_recording.abort(AbortReason::SourceChanged);
                }
            }
        }
    }
    /// Confirm the exact converted output quantum; this is sink delivery, not
    /// physical loudspeaker playback. Failure discards the entire pending take.
    pub fn finish_gain_recording_quantum(&mut self, output_valid: bool) {
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
        let key = (revision, generation);
        if !force && self.published_route_key == Some(key) {
            return;
        }

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
        );
        self.status.end_route_snapshot();
        self.published_route_key = Some(key);
    }
    fn publish_transport(&self) {
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(ActivePublication::Candidate(active)) = self.active.as_ref() {
            if let Some((position, frame, playing)) = active.owner.test_transport() {
                self.status.position.store(position, Relaxed);
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
            self.status
                .position
                .store(active.playback.position(), Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, Weak};

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

    fn spectrum_eq8_graph_with_params(
        playing: bool,
        mode: u32,
        gain_db: f32,
        q: f32,
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
        let project = CanonicalProject::from_snapshot(Snapshot {
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            next_id: DEVICE_ID + 2,
            assets: vec![AssetSnapshot {
                id: ASSET_ID,
                path: "analyzer-test.wav".into(),
                source_sample_rate: 48_000,
                source_frames: FRAMES,
                playback_frames: FRAMES,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            }],
            tracks: vec![TrackSnapshot {
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
                    id: DEVICE_ID,
                    bypassed: false,
                    state,
                }],
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
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
                sends: Vec::new(),
            }],
            return_tracks: vec![ReturnTrackSnapshot {
                id: 3,
                name: "return fixture".into(),
                gain: 1.0,
                muted: false,
                devices: vec![DeviceSnapshot {
                    id: DEVICE_ID + 1,
                    bypassed: false,
                    state: return_state,
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
                DeviceTypeId::from_static("hex.eq8"),
            ),
        )
    }

    #[cfg(feature = "allocation-probe")]
    fn saturator_track_and_return_graph(drive_db: f32) -> Playback {
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
        let project = CanonicalProject::from_snapshot(Snapshot {
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            next_id: RETURN_DEVICE_ID + 1,
            assets: vec![AssetSnapshot {
                id: ASSET_ID,
                path: "saturator-callback-test.wav".into(),
                source_sample_rate: 48_000,
                source_frames: FRAMES,
                playback_frames: FRAMES,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            }],
            tracks: vec![TrackSnapshot {
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
                    id: TRACK_DEVICE_ID,
                    bypassed: false,
                    state: make_state(),
                }],
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
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
        let project = CanonicalProject::from_snapshot(Snapshot {
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            next_id: RETURN_DEVICE_ID + 1,
            assets: vec![AssetSnapshot {
                id: ASSET_ID,
                path: "compressor-callback-test.wav".into(),
                source_sample_rate: 48_000,
                source_frames: FRAMES,
                playback_frames: FRAMES,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            }],
            tracks: vec![TrackSnapshot {
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
                    id: TRACK_DEVICE_ID,
                    bypassed: false,
                    state: make_state(),
                }],
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
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
        let project = CanonicalProject::from_snapshot(Snapshot {
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            next_id: RETURN_DEVICE_ID + 1,
            assets: vec![AssetSnapshot {
                id: ASSET_ID,
                path: "gate-callback-test.wav".into(),
                source_sample_rate: 48_000,
                source_frames: FRAMES,
                playback_frames: FRAMES,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            }],
            tracks: vec![TrackSnapshot {
                gain_envelope: Vec::new(),
                gain_touch_spans: Vec::new(),
                gain_touch_epoch: 0,
                id: TRACK_ID,
                name: "gate callback fixture".into(),
                gain: 1.0,
                balance: 0.0,
                muted: false,
                soloed: false,
                devices: vec![DeviceSnapshot {
                    id: TRACK_DEVICE_ID,
                    bypassed: false,
                    state: make_state(),
                }],
                instrument: None,
                instrument_position: None,
                content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
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
                devices: vec![DeviceSnapshot {
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

    fn metronome_graph() -> Playback {
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
            assert_eq!(control.status.playing.load(Relaxed), cancelled);
            control.request_shutdown();
            callback.begin_block();
            control.reclaim();
        }
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn clip_gain_with_fades_and_replacement_allocates_and_frees_nothing_in_callback() {
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
        editor
            .execute(hex_project::Command::SetClipFades {
                clip,
                fade_in: 64,
                fade_out: 64,
            })
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
        assert_eq!(prefix[64], [0.5 * gain, -0.25 * gain]);
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
        assert_eq!(output[0], [0.16; 2]);
        control.command(PlaybackCommand::Pause).unwrap();
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
        assert_eq!(output, [[0.0; 2]; 128]);
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
        assert!(
            original_output.iter().all(|frame| {
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
        assert!(output.iter().all(|frame| {
            (frame[0] - 0.34375).abs() < 1.0e-6 && (frame[1] + 0.55).abs() < 1.0e-6
        }));
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
        let fault = control.status.render_fault_snapshot().expect("fault");
        assert_eq!(fault.message, "ongoing midi query exceeds 256 edges");
        assert_eq!((fault.audio_revision, fault.frame), (revision, 128));
        callback.report_render_fault(None, 0);
        assert_eq!(control.status.render_fault_snapshot(), Some(fault));
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
    fn spectrum_stage_accumulates_exact_offsets_and_rejects_overlap_or_wrong_owner() {
        let target = spectrum_test_target(hex_engine::ProcessorChainOwner::Track(3), 0, 1, 48_000);
        let mut stage = SpectrumStage::default();
        stage.begin(target, 1, 4);
        stage.observe(target.identity, 0, &[[0.1, 0.2], [0.3, 0.4]]);
        let wrong_owner =
            spectrum_test_target(hex_engine::ProcessorChainOwner::Return(3), 0, 1, 48_000);
        stage.observe(wrong_owner.identity, 2, &[[0.8, 0.9]]);
        assert!(!stage.complete());
        stage.observe(target.identity, 2, &[[0.5, 0.6], [0.7, 0.8]]);
        assert!(stage.complete());
        assert_eq!(
            stage.frames[..4],
            [[0.1, 0.2], [0.3, 0.4], [0.5, 0.6], [0.7, 0.8]]
        );

        stage.begin(target, 2, 4);
        stage.observe(target.identity, 0, &[[0.1, 0.2], [0.3, 0.4]]);
        stage.observe(target.identity, 1, &[[0.5, 0.6]]);
        assert!(!stage.complete());

        stage.begin(target, 3, 4);
        stage.observe(target.identity, usize::MAX, &[[0.1, 0.2]]);
        assert!(!stage.complete());
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
        assert!(callback.spectrum_stage.complete());

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
            !callback.spectrum_stage.valid,
            "activation floor is not reached"
        );
        assert_eq!(callback.spectrum_stage.target, Some(target_b));
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
        assert!(callback.spectrum_stage.complete());
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
        assert!(!callback.spectrum_stage.valid);
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
        assert!(!callback.spectrum_stage.valid);
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
        assert!(!callback.spectrum_stage.valid);
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
        assert!(callback.spectrum_stage.complete());
        assert_eq!(callback.spectrum_stage.target, Some(target_b));
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
        assert!(!callback.spectrum_stage.valid);
        assert_eq!(callback.spectrum_stage.target, Some(target_a));
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
        assert!(callback.spectrum_stage.complete());
        assert_eq!(callback.spectrum_stage.target, Some(target_b));
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
                revision: 99,
                control_context_generation: 0,
                policy: PublicationPolicy::InstallFreshRuntime,
                fresh_runtime_cancellation: None,
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
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let revision = control
            .publish_with_policy(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            )
            .unwrap();
        let ticket = control
            .command_with_ticket(PlaybackCommand::Seek(13))
            .unwrap();
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), revision - 1);
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);
        assert!(control.status.publication_pending.load(Acquire));
        assert_eq!(control.reclaim(), RETIRED);
        callback.begin_block();
        assert_eq!(control.status.applied_revision.load(Acquire), revision - 1);
        assert_fresh_fence_rejection(&mut control, revision);
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
                    revision,
                    control_context_generation: 0,
                    policy: PublicationPolicy::InstallFreshRuntime,
                    fresh_runtime_cancellation: None,
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

    fn graph() -> (Playback, Weak<hex_engine::Project>) {
        graph_with_frame([0.25; 2])
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
        let project = CanonicalProject::from_snapshot(Snapshot {
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            next_id: 5,
            assets: vec![AssetSnapshot {
                id: 4,
                path: "session.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 2,
                playback_frames: 2,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            }],
            tracks: vec![TrackSnapshot {
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
    fn retired_session_audio_pcm_stays_owned_until_control_reclaims_publication() {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (playback, weak_revision, _, _) = session_audio_graph();
        callback.held_retirement = Some(ReturnedPublication::Legacy(Publication {
            revision: 1,
            control_context_generation: 0,
            policy: PublicationPolicy::InheritRuntime,
            fresh_runtime_cancellation: None,
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
                        revision,
                        control_context_generation: 0,
                        policy: PublicationPolicy::InheritRuntime,
                        fresh_runtime_cancellation: None,
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
                        revision,
                        control_context_generation: 0,
                        policy: PublicationPolicy::InheritRuntime,
                        fresh_runtime_cancellation: None,
                        playback: Box::new(playback),
                    }))
                    .is_ok()
            );
            retired_weak.push(weak);
        }
        let (held, held_weak) = graph();
        callback.held_retirement = Some(ReturnedPublication::Legacy(Publication {
            revision: 4,
            control_context_generation: 0,
            policy: PublicationPolicy::InheritRuntime,
            fresh_runtime_cancellation: None,
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
                    revision: 3,
                    control_context_generation: 0,
                    policy: PublicationPolicy::InheritRuntime,
                    fresh_runtime_cancellation: None,
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
        let project = CanonicalProject::from_snapshot(Snapshot {
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            return_feeds: Vec::new(),
            import_provenance: None,
            sample_rate: 48_000,
            next_id: track_id + 1,
            assets: Vec::new(),
            tracks: vec![TrackSnapshot {
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
                active_launch_beat_raw: None,
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
    fn computer_keyboard_callback_publication_polyphony_release_pressure_stop_and_retirement_allocate_nothing()
     {
        use hex_engine::computer_keyboard::ComputerKeyboardInput;
        use hex_project::{Command, Created, Editor, Project};
        for (routed, mutable) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack {
                    name: "keys".into(),
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
