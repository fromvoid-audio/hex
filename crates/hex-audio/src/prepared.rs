//! Bounded ownership handoff for compiled arrangement playback.
//! Control constructs/drops graphs; callback swaps at most one per block and returns
//! old ownership through a reserved retirement queue. Used by the native prepared output.
use hex_engine::{
    Frame, Invalid, MAX_SESSION_ROUTE_TRACKS, PlaybackAction, PlaybackActionRejection,
    PlaybackCommand, PlaybackHandoffRejection, SessionCapabilityStatus, playback::Playback,
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicBool, AtomicU32, AtomicU64,
        Ordering::{AcqRel, Acquire, Relaxed, Release, SeqCst},
    },
};

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

struct Publication {
    revision: u64,
    policy: PublicationPolicy,
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

pub struct Control {
    publications: Producer<Publication>,
    retired: Consumer<Publication>,
    commands: Producer<QueuedCommand>,
    pending_stop_marker: Arc<AtomicU64>,
    admitted_ticket: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
    publication_pending: Arc<AtomicBool>,
    pub status: Arc<Status>,
    rate: u32,
    revision: u64,
    command_ticket: u64,
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
    active: Option<Publication>,
    deferred: Option<Publication>,
    // Defensive retention if a push unexpectedly fails; never drop graph ownership.
    held_retirement: Option<Publication>,
    published_route_key: Option<(u64, u64)>,
    #[cfg(test)]
    route_slot_visits: usize,
    #[cfg(test)]
    test_transient_handoff_refusals: usize,
}

/// Allocate channels/telemetry on the control side. Initial graph must be
/// published before playback; an empty provider emits silence.
pub fn channel(rate: u32) -> Result<(Control, Callback), Invalid> {
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
            rate,
            revision: 0,
            command_ticket: 0,
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
            active: None,
            deferred: None,
            held_retirement: None,
            published_route_key: None,
            #[cfg(test)]
            route_slot_visits: 0,
            #[cfg(test)]
            test_transient_handoff_refusals: 0,
        },
    ))
}

impl Control {
    pub fn publish(&mut self, playback: Playback) -> Result<u64, PublishError> {
        self.publish_with_policy(playback, PublicationPolicy::InheritRuntime)
    }

    pub fn publish_with_policy(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
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
        let publication = Publication {
            revision,
            policy,
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
        Ok(revision)
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
        Ok(ticket)
    }
    pub fn stop(&self) {
        // Ticket MAX is refused for ordinary commands, so the supported marker
        // encoding (ticket + 1) is always nonzero and cannot wrap.
        self.pending_stop_marker
            .store(self.command_ticket.saturating_add(1), Release);
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
                    self.return_graph(publication);
                    self.publication_pending.store(false, Release);
                }
            }
            if !self.retired.is_full() {
                if let Some(publication) = self.active.take() {
                    self.return_graph(publication);
                }
            }
            if !self.retired.is_full() {
                if let Ok(publication) = self.publications.pop() {
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
            let handoff = if forced_refusal {
                Some(Err(hex_engine::PlaybackHandoffError::Session(
                    hex_engine::SessionPlaybackError::ReleaseBankFull,
                )))
            } else if next.policy == PublicationPolicy::InstallFreshRuntime {
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
                    if let Some(reason) = error.rejection() {
                        self.status.publish_rejected_revision(next.revision, reason);
                    }
                    self.return_graph(next);
                    self.publication_pending.store(false, Release);
                }
                _ => {
                    let revision = next.revision;
                    self.status
                        .session_capability
                        .store(next.playback.session_capability_status() as u32, Relaxed);
                    if let Some(previous) = self.active.replace(next) {
                        self.return_graph(previous);
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

    fn consume_commands(&mut self) {
        // Capture the published queue prefix before clearing the marker. Later
        // producer admissions stay queued for the next block; a Stop arriving
        // after this swap also remains pending for the next block.
        let entry_ticket = self.admitted_ticket.load(Acquire);
        let stop_marker = self.pending_stop_marker.swap(0, AcqRel);
        self.consume_commands_through(command_cut(entry_ticket, stop_marker));
    }

    fn consume_commands_through(&mut self, cut: CommandCut) {
        if cut.stop_through.is_some()
            && let Some(active) = self.active.as_mut()
        {
            active.playback.apply(PlaybackCommand::Stop);
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
                && let Some(active) = self.active.as_mut()
                && let Err(error) = active.playback.try_apply_action(queued.action)
            {
                self.status
                    .publish_rejected_action(queued.ticket, error.telemetry_rejection());
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
        self.publish_transport();
        self.publish_active_routes(false);
    }
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
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
    use std::sync::Weak;

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
        let audio = hex_engine::Audio::new(48_000, vec![[0.25; 2]]).unwrap();
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
                content: TrackContentSnapshot::Audio(Vec::new()),
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
            policy: PublicationPolicy::InheritRuntime,
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
                        policy: PublicationPolicy::InheritRuntime,
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
                        policy: PublicationPolicy::InheritRuntime,
                        playback: Box::new(playback),
                    })
                    .is_ok()
            );
            retired_weak.push(weak);
        }
        let (held, held_weak) = graph();
        callback.held_retirement = Some(Publication {
            revision: 4,
            policy: PublicationPolicy::InheritRuntime,
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
                    policy: PublicationPolicy::InheritRuntime,
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
                content: TrackContentSnapshot::Instrument(Vec::new()),
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
