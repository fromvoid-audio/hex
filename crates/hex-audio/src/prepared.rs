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
const RETIRED: usize = 2;
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
        | PlaybackAction::LaunchAndPlay(_) => false,
    }
}

struct Publication {
    revision: u64,
    /// UI/control project context this compiled playback belongs to. This is
    /// audio publication metadata, not project schema or engine state.
    control_context_generation: u64,
    policy: PublicationPolicy,
    fresh_runtime_cancellation: Option<Arc<FreshRuntimeCancellation>>,
    playback: Box<Playback>,
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
    pub device_fault: AtomicBool,
    pub render_fault: AtomicBool,
    pub callback_max_ns: AtomicU64,
    pub deadline_misses: AtomicU64,
    pub peak_left: AtomicU32,
    pub peak_right: AtomicU32,
    pub applied_revision: AtomicU64,
    /// Highest admitted command consumed, including commands discarded by Stop.
    /// Release publication follows transport state; readers acquire before reading it.
    pub consumed_command: AtomicU64,
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
    pub playing: AtomicBool,
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
    fn with_route_slots(route_slots: Box<[AtomicRouteSlot]>) -> Self {
        Self {
            device_fault: AtomicBool::new(false),
            render_fault: AtomicBool::new(false),
            callback_max_ns: AtomicU64::new(0),
            deadline_misses: AtomicU64::new(0),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            applied_revision: AtomicU64::new(0),
            consumed_command: AtomicU64::new(0),
            rejected_action_ticket: AtomicU64::new(0),
            rejected_action_reason: AtomicU32::new(0),
            rejected_action_sequence: AtomicU64::new(0),
            rejected_revision: AtomicU64::new(0),
            rejected_revision_reason: AtomicU32::new(0),
            rejected_revision_sequence: AtomicU64::new(0),
            position: AtomicU64::new(0),
            performance_frame: AtomicU64::new(0),
            playing: AtomicBool::new(false),
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

pub struct Control {
    publications: Producer<Publication>,
    retired: Consumer<Publication>,
    commands: Producer<QueuedCommand>,
    pending_stop_marker: Arc<AtomicU64>,
    admitted_ticket: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
    publication_pending: Arc<AtomicBool>,
    pub status: Arc<Status>,
    pub spectrum_target: spectrum_target::SpectrumTargetControl,
    pub processor_audition: processor_audition::ProcessorAuditionTargetControl,
    rate: u32,
    revision: u64,
    command_ticket: u64,
    latest_fresh_runtime_cancellation: Option<Arc<FreshRuntimeCancellation>>,
}

pub struct Callback {
    publications: Consumer<Publication>,
    retired: Producer<Publication>,
    commands: Consumer<QueuedCommand>,
    pending_stop_marker: Arc<AtomicU64>,
    admitted_ticket: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
    publication_pending: Arc<AtomicBool>,
    status: Arc<Status>,
    rate: u32,
    spectrum_target: spectrum_target::SpectrumTargetCallback,
    processor_audition: processor_audition::ProcessorAuditionTargetCallback,
    processor_audition_lease: Option<processor_audition::ProcessorAuditionTarget>,
    processor_audition_rejected_epoch: u64,
    spectrum_capture: Option<SpectrumCapture>,
    spectrum_stage: SpectrumStage,
    active: Option<Publication>,
    deferred: Option<Publication>,
    // Defensive retention if a push unexpectedly fails; never drop graph ownership.
    held_retirement: Option<Publication>,
    published_route_key: Option<(u64, u64)>,
    #[cfg(test)]
    route_slot_visits: usize,
    #[cfg(test)]
    test_transient_handoff_refusals: usize,
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
    let (publications, incoming) = RingBuffer::new(1);
    let (returning, retired) = RingBuffer::new(RETIRED);
    let (commands, events) = RingBuffer::new(COMMANDS);
    let pending_stop_marker = Arc::new(AtomicU64::new(0));
    let admitted_ticket = Arc::new(AtomicU64::new(0));
    let closing = Arc::new(AtomicBool::new(false));
    let publication_pending = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Status::try_new()?);
    let (spectrum_target, callback_target) = spectrum_target::channel();
    let (processor_audition, callback_audition) = processor_audition::channel();
    Ok((
        Control {
            publications,
            retired,
            commands,
            pending_stop_marker: pending_stop_marker.clone(),
            admitted_ticket: admitted_ticket.clone(),
            closing: closing.clone(),
            publication_pending: publication_pending.clone(),
            status: status.clone(),
            spectrum_target,
            processor_audition,
            rate,
            revision: 0,
            command_ticket: 0,
            latest_fresh_runtime_cancellation: None,
        },
        Callback {
            publications: incoming,
            retired: returning,
            commands: events,
            pending_stop_marker,
            admitted_ticket,
            closing,
            publication_pending,
            status,
            rate,
            spectrum_target: callback_target,
            processor_audition: callback_audition,
            processor_audition_lease: None,
            processor_audition_rejected_epoch: 0,
            spectrum_capture: capture,
            spectrum_stage: SpectrumStage::default(),
            active: None,
            deferred: None,
            held_retirement: None,
            published_route_key: None,
            #[cfg(test)]
            route_slot_visits: 0,
            #[cfg(test)]
            test_transient_handoff_refusals: 0,
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

impl Control {
    /// Capture the current route header under its sequence lock, then acquire
    /// the admitted-command high-water mark. Returns `None` on a contended or
    /// invalid route snapshot so callers can retry before preparing a graph.
    pub fn capture_fresh_runtime_fence(&self) -> Option<FreshRuntimeFence> {
        let mut routes = [SessionRouteEntry::default(); SESSION_ROUTE_CAPACITY];
        let snapshot = self.status.session_routes(&mut routes)?;
        Some(FreshRuntimeFence {
            expected_active_revision: snapshot.audio_revision,
            route_state_generation: snapshot.route_state_generation,
            admitted_command_ticket: self.admitted_ticket.load(Acquire),
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
        let reason = if self.closing.load(Acquire) {
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
        if self
            .publication_pending
            .compare_exchange(false, true, AcqRel, Acquire)
            .is_err()
        {
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
        let publication = Publication {
            revision,
            control_context_generation,
            policy,
            fresh_runtime_cancellation: cancellation.as_ref().map(Arc::clone),
            playback: Box::new(playback),
        };
        if let Err(PushError::Full(publication)) = self.publications.push(publication) {
            self.publication_pending.store(false, Release);
            return Err(PublishError {
                reason: "playback publication queue full",
                playback: *publication.playback,
            });
        }
        self.revision = revision;
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
                PlaybackAction::Session(_) | PlaybackAction::LaunchAndPlay(_) => command,
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
                PlaybackAction::Transport(_) | PlaybackAction::LaunchAndPlay(_) => command,
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
                PlaybackAction::Transport(_) => command,
            })
    }

    pub fn action_with_ticket(&mut self, action: PlaybackAction) -> Result<u64, PlaybackAction> {
        if self.closing.load(Acquire) || self.revision == 0 {
            return Err(action);
        }
        let Some(ticket) = self
            .command_ticket
            .checked_add(1)
            .filter(|ticket| *ticket != u64::MAX)
        else {
            return Err(action);
        };
        self.commands
            .push(QueuedCommand { ticket, action })
            .map_err(|PushError::Full(queued)| queued.action)?;
        self.command_ticket = ticket;
        self.admitted_ticket.store(ticket, Release);
        if action_invalidates_processor_audition(action) {
            let _ = self.processor_audition.cancel_immediately();
        }
        Ok(ticket)
    }
    pub fn stop(&self) {
        // Ticket MAX is refused for ordinary commands, so the supported marker
        // encoding (ticket + 1) is always nonzero and cannot wrap.
        self.pending_stop_marker
            .store(self.command_ticket.saturating_add(1), Release);
        self.processor_audition.invalidate_current_immediately();
    }
    /// Bounded reclamation on the control thread, including final PCM drops.
    pub fn reclaim(&mut self) -> usize {
        let mut count = 0;
        for _ in 0..RETIRED {
            let Ok(publication) = self.retired.pop() else {
                break;
            };
            drop(publication);
            count += 1;
        }
        count
    }
    /// Freeze publication, return active/queued graphs over subsequent callbacks.
    /// Control must continue reclaiming until shutdown_complete before stream teardown.
    pub fn request_shutdown(&mut self) {
        self.closing.store(true, Release);
        self.stop();
    }
}

impl Callback {
    fn return_graph(&mut self, publication: Publication) {
        if let Err(PushError::Full(publication)) = self.retired.push(publication) {
            self.held_retirement = Some(publication);
        }
    }
    /// At most one replacement and 32 commands; no graph disposal or allocation.
    pub fn begin_block(&mut self) {
        self.processor_audition.begin_block();
        self.apply_processor_audition_boundary();
        self.spectrum_target.begin_block();
        #[cfg(test)]
        if let Some(barrier) = self.test_after_spectrum_target_boundary.take() {
            barrier.wait();
            barrier.wait();
        }
        if let Some(publication) = self.held_retirement.take() {
            self.return_graph(publication);
            if self.held_retirement.is_some() {
                // Ownership backpressure must not delay the independent Stop
                // path. Keep the graph retained while consuming bounded control.
                if self.closing.load(Acquire) {
                    self.status.playing.store(false, Relaxed);
                } else {
                    self.consume_commands();
                }
                return;
            }
        }
        if self.closing.load(Acquire) {
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
                    self.publication_pending.store(false, Release);
                }
            }
            if !self.retired.is_full() {
                if let Some(active) = self.active.as_mut() {
                    active.playback.cancel_all_processor_auditions();
                }
                if let Some(publication) = self.active.take() {
                    self.return_graph(publication);
                }
            }
            if !self.retired.is_full() {
                if let Ok(publication) = self.publications.pop() {
                    if let Some(cancellation) = publication.fresh_runtime_cancellation.as_ref() {
                        if cancellation.finish_for_shutdown() {
                            self.status.publish_rejected_revision(
                                publication.revision,
                                PlaybackHandoffRejection::FreshRuntimeCancelled,
                            );
                        }
                    }
                    self.return_graph(publication);
                    self.publication_pending.store(false, Release);
                }
            }
            self.status.playing.store(false, Relaxed);
            if self.active.is_none()
                && self.published_route_key.is_some()
                && self.deferred.is_none()
                && self.publications.is_empty()
            {
                self.status.publish_empty_routes();
                self.published_route_key = None;
            }
            if self.active.is_none()
                && self.deferred.is_none()
                && self.publications.is_empty()
                && self.held_retirement.is_none()
            {
                self.status.shutdown_complete.store(true, Release);
            }
            return;
        }
        if self.deferred.is_none()
            && (self.active.is_none() || !self.retired.is_full())
            && let Ok(publication) = self.publications.pop()
        {
            self.deferred = Some(publication);
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
                    let active_revision = self.active.as_ref().map_or(0, |active| active.revision);
                    let active_route_generation = self
                        .active
                        .as_ref()
                        .map_or(0, |active| active.playback.session_route_state_generation());
                    let admitted_ticket = self.admitted_ticket.load(Acquire);
                    let stop_pending = self.pending_stop_marker.load(Acquire) != 0;
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
                self.active.as_mut().map(|previous| {
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
                    self.publication_pending.store(false, Release);
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
                        self.publication_pending.store(false, Release);
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
                        let previous_matches_target = self.active.as_ref().is_some_and(|active| {
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
                    if let Some(previous) = self.active.replace(next) {
                        self.return_graph(previous);
                    }
                    if let Some(cancellation) = self
                        .active
                        .as_ref()
                        .and_then(|active| active.fresh_runtime_cancellation.as_ref())
                    {
                        cancellation.finish();
                    }
                    self.publish_active_routes(true);
                    self.status.render_fault.store(false, Relaxed);
                    self.status.applied_revision.store(revision, Release);
                    self.publication_pending.store(false, Release);
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
            && let Some(active) = self.active.as_mut()
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
            && let Some(active) = self.active.as_mut()
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
        let Some(active) = self.active.as_mut() else {
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
        // Capture the published queue prefix before clearing the marker. Later
        // producer admissions stay queued for the next block; a Stop arriving
        // after this swap also remains pending for the next block.
        let entry_ticket = self.admitted_ticket.load(Acquire);
        let stop_marker = self.pending_stop_marker.swap(0, AcqRel);
        self.consume_commands_through(command_cut(entry_ticket, stop_marker));
    }

    fn consume_commands_through(&mut self, cut: CommandCut) {
        if cut.stop_through.is_some() {
            if let Some(active) = self.active.as_mut() {
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
            if !cut
                .stop_through
                .is_some_and(|stop_through| queued.ticket <= stop_through)
            {
                let mut invalidate_audition = false;
                if let Some(active) = self.active.as_mut() {
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
    /// Stop immediately after a render fault; does not dispose graph ownership.
    pub fn stop_after_fault(&mut self) {
        if let Some(active) = self.active.as_mut() {
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
        if let Some(active) = self.active.as_mut() {
            active.playback.cancel_all_processor_auditions();
        }
    }
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
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
        let active_matches_target = self.active.as_ref().is_some_and(|active| {
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
        if self.closing.load(Acquire) {
            output.fill([0.0; 2]);
            return Ok(());
        }
        if let Some(active) = self.active.as_mut() {
            let stage = &mut self.spectrum_stage;
            let mut observe = |identity, offset, frames: &[Frame]| {
                stage.observe(identity, offset, frames);
            };
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
        if self.closing.load(Acquire) {
            output.fill([0.0; 2]);
            return Ok(());
        }
        if let Some(active) = self.active.as_mut() {
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
            && self.active.as_ref().is_some_and(|active| {
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
                    applied_revision: self.active.as_ref().map_or(0, |active| active.revision),
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

    fn publish_active_routes(&mut self, force: bool) {
        let Some(active) = self.active.as_ref() else {
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
        if let Some(active) = self.active.as_ref() {
            self.status
                .position
                .store(active.playback.position(), Relaxed);
            self.status
                .performance_frame
                .store(active.playback.performance_frame(), Relaxed);
            self.status
                .playing
                .store(active.playback.playing(), Relaxed);
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
        assert_eq!(callback.active.as_ref().unwrap().revision, first_revision);

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
            callback.active.as_ref().unwrap().revision,
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
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );

        assert_eq!(callback.processor_audition_lease, None);
        assert!(
            !callback
                .active
                .as_mut()
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
        assert_eq!(callback.active.as_ref().unwrap().revision, first_revision);

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
        assert_eq!(callback.active.as_ref().unwrap().revision, second_revision);
        assert_eq!(
            callback.active.as_ref().unwrap().control_context_generation,
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
            assert_eq!(callback.active.as_ref().unwrap().revision, revision);
            assert_eq!(
                callback.processor_audition_lease,
                control.processor_audition.desired()
            );
            assert!(
                callback
                    .active
                    .as_mut()
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
        assert_eq!(callback.active.as_ref().unwrap().revision, second_revision);
        assert_eq!(callback.processor_audition_lease, None);
        assert_eq!(callback.processor_audition_rejected_epoch, epoch);
        assert!(
            !callback
                .active
                .as_mut()
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

        assert!(!callback.active.as_ref().unwrap().playback.playing());
        assert_eq!(
            callback.processor_audition_lease,
            control.processor_audition.desired()
        );
        assert!(
            callback
                .active
                .as_mut()
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
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
        assert_eq!(callback.active.as_ref().unwrap().revision, second_revision);
        assert_eq!(callback.processor_audition_lease, None);
        assert_eq!(callback.processor_audition.current(), None);
        assert!(
            !callback
                .active
                .as_mut()
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
        assert!(!control.publication_pending.load(Acquire));
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
        assert_eq!(control.admitted_ticket.load(Acquire), 0);
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
        assert!(!control.publication_pending.load(Acquire));
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
        assert!(!control.publication_pending.load(Acquire));
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
            .push(Publication {
                revision: 99,
                control_context_generation: 0,
                policy: PublicationPolicy::InstallFreshRuntime,
                fresh_runtime_cancellation: None,
                playback: Box::new(graph().0),
            })
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
        let ticket_before = control.admitted_ticket.load(Acquire);
        callback
            .active
            .as_mut()
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
        assert_eq!(control.admitted_ticket.load(Acquire), ticket_before);
        assert_ne!(
            callback
                .active
                .as_ref()
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
        assert_eq!(control.pending_stop_marker.load(Acquire), 0);
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
        assert_eq!(control.pending_stop_marker.load(Acquire), 0);
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
        assert!(control.publication_pending.load(Acquire));
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
                .push(Publication {
                    revision,
                    control_context_generation: 0,
                    policy: PublicationPolicy::InstallFreshRuntime,
                    fresh_runtime_cancellation: None,
                    playback: Box::new(graph().0),
                })
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
        assert_eq!(control.admitted_ticket.load(Acquire), COMMANDS as u64);
        callback.begin_block();
        assert_eq!(
            control.status.consumed_command.load(Acquire),
            COMMANDS as u64
        );

        control.command_ticket = u64::MAX - 1;
        control.admitted_ticket.store(u64::MAX - 1, Release);
        assert!(control.command_with_ticket(PlaybackCommand::Play).is_err());
        control.stop();
        assert_eq!(control.pending_stop_marker.load(Acquire), u64::MAX);
        callback.begin_block();
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(control.pending_stop_marker.load(Acquire), 0);
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
        let entry_ticket = control.admitted_ticket.load(Acquire);
        let marker = control.pending_stop_marker.swap(0, AcqRel);
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
        assert_eq!(control.pending_stop_marker.load(Acquire), post_cut_play);
        assert_eq!(callback.commands.peek().unwrap().ticket, post_cut_play);

        // The next real block captures Stop + Play and applies them in order.
        callback.begin_block();
        assert!(control.status.playing.load(Acquire));
        assert_eq!(control.status.consumed_command.load(Acquire), post_cut_play);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
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
        callback.held_retirement = Some(Publication {
            revision: 1,
            control_context_generation: 0,
            policy: PublicationPolicy::InheritRuntime,
            fresh_runtime_cancellation: None,
            playback: Box::new(playback),
        });
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
                    .push(Publication {
                        revision,
                        control_context_generation: 0,
                        policy: PublicationPolicy::InheritRuntime,
                        fresh_runtime_cancellation: None,
                        playback: Box::new(playback),
                    })
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
        assert!(control.publication_pending.load(Acquire));
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
        assert!(!control.publication_pending.load(Acquire));
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
                    .push(Publication {
                        revision,
                        control_context_generation: 0,
                        policy: PublicationPolicy::InheritRuntime,
                        fresh_runtime_cancellation: None,
                        playback: Box::new(playback),
                    })
                    .is_ok()
            );
            retired_weak.push(weak);
        }
        let (held, held_weak) = graph();
        callback.held_retirement = Some(Publication {
            revision: 4,
            control_context_generation: 0,
            policy: PublicationPolicy::InheritRuntime,
            fresh_runtime_cancellation: None,
            playback: Box::new(held),
        });
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
                .push(Publication {
                    revision: 3,
                    control_context_generation: 0,
                    policy: PublicationPolicy::InheritRuntime,
                    fresh_runtime_cancellation: None,
                    playback: Box::new(queued),
                })
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
            sample_rate: 48_000,
            next_id: track_id + 1,
            assets: Vec::new(),
            tracks: vec![TrackSnapshot {
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
}
