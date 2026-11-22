//! Atomic double-checked route snapshot mailbox for prepared Session playback.
use super::*;

pub(super) const ROUTE_READER_ATTEMPTS: usize = 3;
pub(super) const MAX_TRACKS: usize = MAX_SESSION_ROUTE_TRACKS;
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

pub(super) struct AtomicRouteSlot {
    pub(super) track_id: AtomicU64,
    pub(super) owner_tag: AtomicU32,
    pub(super) owner_clip_id: AtomicU64,
    pub(super) active_origin_tag: AtomicU32,
    pub(super) active_performance_anchor_raw: AtomicU64,
    pub(super) active_elapsed_at_anchor_raw: AtomicU64,
    pub(super) audio_launch_tag: AtomicU32,
    pub(super) audio_launch_frame: AtomicU64,
    pub(super) pending_tag: AtomicU32,
    pub(super) pending_target_tag: AtomicU32,
    pub(super) pending_clip_id: AtomicU64,
    pub(super) pending_frame: AtomicU64,
    pub(super) pending_beat_raw: AtomicU64,
}

impl AtomicRouteSlot {
    pub(super) fn new() -> Self {
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

    pub(super) fn clear(&self) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_route_slot_clear_resets_all_fields() {
        let slot = AtomicRouteSlot::new();
        slot.track_id.store(42, SeqCst);
        slot.owner_tag.store(1, SeqCst);
        slot.owner_clip_id.store(99, SeqCst);
        slot.pending_tag.store(1, SeqCst);
        slot.clear();
        assert_eq!(slot.track_id.load(SeqCst), 0);
        assert_eq!(slot.owner_tag.load(SeqCst), 0);
        assert_eq!(slot.owner_clip_id.load(SeqCst), 0);
        assert_eq!(slot.pending_tag.load(SeqCst), 0);
        assert_eq!(
            SessionRouteEntry::default().owner,
            SessionRouteOwner::Silence
        );
    }
}
