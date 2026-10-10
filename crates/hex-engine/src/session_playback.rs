//! Headless, bounded mixed-route Session playback over one prepared revision.
//!
//! Prepared native `playback::Playback` delegates mixed routing to this kernel;
//! legacy finite Arrangement playback and offline export remain independent.
use crate::prepared_tempo::{
    AnchoredSessionTempoView, PreparedPeriodicSessionTempo, SessionBaseTimingView,
    SessionClockAnchor, SessionTimingView,
};
use crate::{
    Frame, PreparedRevision,
    midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK,
    midi_template::{MidiOccurrenceSuppression, MidiReleaseVoice, MidiSpanBuffer},
    mutable_instrument::{
        MutableInstrumentTail, MutableInstrumentTrackRuntime, MutableOnsetBuffer,
    },
};
use hex_project::{BeatTime, ClipId, MAX_TRACKS, NoteId, SceneId, TrackId};
use std::{fmt, sync::Arc};

const QUARTER_RAW: u64 = 1_u64 << 32;
const FOUR_QUARTERS_RAW: u64 = QUARTER_RAW * 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchQuantization {
    Immediate,
    FourQuarter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPlaybackCommand {
    Play,
    Pause,
    Stop,
    Seek(u64),
    LaunchClip {
        clip: ClipId,
        quantization: LaunchQuantization,
    },
    LaunchScene {
        scene: SceneId,
        quantization: LaunchQuantization,
    },
    StopTrack {
        track: TrackId,
        quantization: LaunchQuantization,
    },
    StopAllClips {
        quantization: LaunchQuantization,
    },
    BackToArrangement {
        quantization: LaunchQuantization,
    },
    CancelPending {
        track: Option<TrackId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPlaybackOwner {
    Arrangement,
    Session { clip: ClipId },
    Silence,
}

/// The target selected by a pending Session transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRouteTarget {
    Arrangement,
    Session { clip: ClipId },
    Silence,
}

/// One stable-track route as displayed at a Session route snapshot cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionRouteStatus {
    pub track: TrackId,
    pub owner: SessionPlaybackOwner,
    pub active_launch: Option<BeatTime>,
    pub pending: Option<SessionPendingRouteStatus>,
}

/// Quantized target waiting to take ownership of a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPendingRouteStatus {
    pub target: SessionRouteTarget,
    pub performance_frame: u64,
    pub beat: BeatTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPlaybackError {
    RevisionTrackMismatch,
    RevisionSampleRateMismatch,
    Allocation,
    UnknownTrack,
    UnknownScene,
    UnknownClip,
    ClipTrackMismatch,
    InvalidCommand,
    ArithmeticOverflow,
    TempoConversion,
    SessionTempoUnavailable(crate::PreparedTempoError),
    ReleaseBankFull,
    DeviceRuntime,
    MutableInstrumentRuntime,
    MutableInstrumentEvents,
    MutableInstrumentLoopUnsupported,
    MutableInstrumentActive,
    MutableInstrumentRouteBoundary,
    MutableInstrumentChase,
    SharedAnalyticContext,
    MediaPagesUnavailable,
    CandidateAlreadyUsed,
    Midi(crate::MidiTemplateError),
    Arrangement(&'static str),
}

impl fmt::Display for SessionPlaybackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionTempoUnavailable(error) => {
                write!(f, "session tempo is unavailable: {error}")
            }
            _ => f.write_str(match self {
                Self::RevisionTrackMismatch => "prepared arrangement and session tracks differ",
                Self::RevisionSampleRateMismatch => "session playback revision sample rates differ",
                Self::Allocation => "could not reserve session playback runtime storage",
                Self::UnknownTrack => "session playback track no longer exists",
                Self::UnknownScene => "session playback scene no longer exists",
                Self::UnknownClip => "session playback clip no longer exists",
                Self::ClipTrackMismatch => "session playback clip belongs to another track",
                Self::InvalidCommand => "session playback command is invalid for this operation",
                Self::ArithmeticOverflow => "session playback frame or beat overflow",
                Self::TempoConversion => "session playback timing conversion failed",
                Self::ReleaseBankFull => "session playback outgoing release bank is full",
                Self::DeviceRuntime => "session playback native device runtime is unavailable",
                Self::MutableInstrumentRuntime => {
                    "session playback mutable instrument runtime is unavailable"
                }
                Self::MutableInstrumentEvents => {
                    "session playback mutable instrument event query is invalid"
                }
                Self::MutableInstrumentLoopUnsupported => {
                    "mutable instrument loops are not admitted by this playback runtime"
                }
                Self::MutableInstrumentActive => {
                    "mutable instrument must become quiescent before this route change"
                }
                Self::MutableInstrumentRouteBoundary => {
                    "quantized route changes are not admitted for mutable instruments"
                }
                Self::MutableInstrumentChase => {
                    "mutable instrument seek or revision requires unsupported note history"
                }
                Self::SharedAnalyticContext => {
                    "shared analytic instrument context lease is unavailable"
                }
                Self::MediaPagesUnavailable => {
                    "streamed session playback requires a media page provider"
                }
                Self::CandidateAlreadyUsed => "session playback candidate has already been used",
                Self::Midi(_) => "session playback midi query failed",
                Self::Arrangement(_) => "session playback arrangement source failed",
                Self::SessionTempoUnavailable(_) => "session tempo is unavailable",
            }),
        }
    }
}

impl std::error::Error for SessionPlaybackError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteTarget {
    Arrangement,
    Session(ClipId),
    Silence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteOwner {
    Arrangement,
    Session { clip: ClipId, launch: BeatTime },
    SessionAudio { clip: ClipId, launch_frame: u64 },
    Silence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingRoute {
    frame: u64,
    beat: BeatTime,
    target: RouteTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackRoute {
    id: TrackId,
    owner: RouteOwner,
    pending: Option<PendingRoute>,
}

#[derive(Clone, Copy)]
struct TransitionMoment {
    target_frame: u64,
    target_beat: BeatTime,
    immediate: bool,
    source_frame: u64,
    performance_frame: u64,
}

#[derive(Clone, Copy, Debug)]
struct ReleaseEntry {
    track_index: usize,
    track_id: TrackId,
    order: u64,
    mix: crate::PreparedTrackMix,
    tail_group: Option<u16>,
    continuation: Option<SessionContinuation>,
    voice: MidiReleaseVoice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContinuationKey {
    clip: ClipId,
    note: NoteId,
    iteration: u64,
}

#[derive(Clone, Copy, Debug)]
struct SessionContinuation {
    key: ContinuationKey,
    onset_beat: BeatTime,
    gate_end_beat: BeatTime,
    pitch: u8,
    channel: u8,
    velocity: u8,
}

const SESSION_RELEASE_CAPACITY: usize = MAX_TRACKS * MAX_SIMULTANEOUS_VOICES_PER_TRACK;
const SESSION_TAIL_GROUP_CAPACITY: usize = hex_lattice::MAX_STATEFUL_PROCESSORS;

/// Bounded result for targets canceled because their stable track or clip was
/// removed by the candidate revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionHandoffReport {
    cancelled_pending: [Option<TrackId>; MAX_TRACKS],
    cancelled_pending_len: usize,
}

impl SessionHandoffReport {
    pub fn cancelled_pending_tracks(&self) -> &[Option<TrackId>] {
        &self.cancelled_pending[..self.cancelled_pending_len]
    }
}

/// Independent, headless mixed Arrangement/Session render kernel.
///
/// Routes and release storage are allocated at construction. Rendering uses
/// copied route state and commits clocks/routes/releases only after the entire
/// requested block succeeds. The existing native `Playback` remains unchanged.
pub struct SessionPlayback {
    revision: Arc<PreparedRevision>,
    periodic_tempo: Option<PreparedPeriodicSessionTempo>,
    anchor: Option<SessionClockAnchor>,
    historical_floor: Option<BeatTime>,
    route_historical_floors: [Option<BeatTime>; MAX_TRACKS],
    routes: Vec<TrackRoute>,
    route_scratch: Vec<TrackRoute>,
    device_chains: Vec<hex_lattice::NativeRuntimeChain>,
    mutable_instruments: Vec<Option<MutableInstrumentTrackRuntime>>,
    mutable_onsets: MutableOnsetBuffer,
    mutable_events: Vec<hex_lattice::NativeInstrumentEvent>,
    tail_groups: Vec<Option<hex_lattice::NativeRuntimeChain>>,
    tail_mutable_instruments: Vec<Option<MutableInstrumentTail>>,
    tail_mixes: Vec<crate::PreparedTrackMix>,
    tail_suffix_starts: Vec<usize>,
    tail_suffix_matrices: Vec<Option<hex_lattice::StereoMatrix>>,
    releases: Vec<ReleaseEntry>,
    release_scratch: Vec<ReleaseEntry>,
    retained_contexts: Vec<Option<Arc<crate::shared_analytic::SharedContextOwner>>>,
    context_lease_plan: Vec<crate::shared_analytic::ContextLeasePlan>,
    release_capacity: usize,
    release_order: u64,
    release_order_scratch: u64,
    suppressions: Vec<MidiOccurrenceSuppression>,
    suppression_scratch: Vec<MidiOccurrenceSuppression>,
    spans: MidiSpanBuffer,
    performance_frame: u64,
    arrangement_frame: u64,
    playing: bool,
    linear_arrangement_export: bool,
    route_state_generation: u64,
    candidate_admission_open: bool,
}

impl fmt::Debug for SessionPlayback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionPlayback")
            .field("track_count", &self.routes.len())
            .field("release_count", &self.releases.len())
            .field("release_capacity", &self.release_capacity)
            .field("performance_frame", &self.performance_frame)
            .field("arrangement_frame", &self.arrangement_frame)
            .field("playing", &self.playing)
            .field("linear_arrangement_export", &self.linear_arrangement_export)
            .field("route_state_generation", &self.route_state_generation)
            .field("candidate_admission_open", &self.candidate_admission_open)
            .finish()
    }
}

impl SessionPlayback {
    pub(super) fn historical_floor_for(&self, route_index: usize) -> Option<BeatTime> {
        self.historical_floor
            .into_iter()
            .chain(
                self.route_historical_floors
                    .get(route_index)
                    .copied()
                    .flatten(),
            )
            .max()
    }

    pub(super) fn advance_route_state_generation(&mut self) {
        self.route_state_generation = self.route_state_generation.wrapping_add(1);
    }

    pub fn new(revision: Arc<PreparedRevision>) -> Result<Self, SessionPlaybackError> {
        Self::new_with_policy(revision, false)
    }

    /// Prepare the shared renderer for finite, linear Arrangement export.
    /// Session route actions are refused, Session-only tempo/loop inventory
    /// does not affect this owner, and the authored Arrangement tempo map is
    /// used without transport-loop wrapping. Mutable Arrangement source loops
    /// and seek destinations that require note chase remain unsupported.
    pub fn new_linear_arrangement_export(
        revision: Arc<PreparedRevision>,
    ) -> Result<Self, SessionPlaybackError> {
        Self::new_with_policy(revision, true)
    }

    fn new_with_policy(
        revision: Arc<PreparedRevision>,
        linear_arrangement_export: bool,
    ) -> Result<Self, SessionPlaybackError> {
        let arrangement_tracks = revision.arrangement().tracks();
        let session_tracks = revision.session().tracks();
        if arrangement_tracks.len() != session_tracks.len()
            || arrangement_tracks
                .iter()
                .zip(session_tracks)
                .any(|(arrangement, session)| arrangement.canonical_track_id() != Some(session.id))
        {
            return Err(SessionPlaybackError::RevisionTrackMismatch);
        }
        if !linear_arrangement_export && let Some(error) = revision.session().periodic_tempo_error()
        {
            return Err(SessionPlaybackError::SessionTempoUnavailable(error));
        }
        let periodic_tempo = if linear_arrangement_export {
            None
        } else {
            revision.session().periodic_tempo()
        };

        if revision.session().has_mutable_instruments()
            && ((!linear_arrangement_export
                && (revision
                    .arrangement()
                    .loop_region()
                    .is_some_and(|region| region.enabled())
                    || revision.session().has_mutable_looping_source()))
                || arrangement_tracks
                    .iter()
                    .any(|track| track.has_mutable_looping_source()))
        {
            return Err(SessionPlaybackError::MutableInstrumentLoopUnsupported);
        }

        let release_capacity = SESSION_RELEASE_CAPACITY;
        let mut routes = Vec::new();
        routes
            .try_reserve_exact(arrangement_tracks.len())
            .map_err(|_| SessionPlaybackError::Allocation)?;
        for track in arrangement_tracks {
            routes.push(TrackRoute {
                id: track
                    .canonical_track_id()
                    .ok_or(SessionPlaybackError::RevisionTrackMismatch)?,
                owner: RouteOwner::Arrangement,
                pending: None,
            });
        }
        let mut mutable_count = 0_usize;
        let mut mutable_bytes = 0_usize;
        let mut existing_stateful_count = 0_usize;
        let mut existing_stateful_bytes = 0_usize;
        for track in arrangement_tracks {
            existing_stateful_count = existing_stateful_count
                .checked_add(track.stateful_device_count())
                .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
            existing_stateful_bytes = existing_stateful_bytes
                .checked_add(
                    track
                        .stateful_device_declared_bytes()
                        .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?,
                )
                .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
            if let Some(factory) = revision.session().mutable_instrument_factory(
                track
                    .canonical_track_id()
                    .ok_or(SessionPlaybackError::RevisionTrackMismatch)?,
            ) {
                mutable_count = mutable_count
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
                mutable_bytes = mutable_bytes
                    .checked_add(
                        factory
                            .declared_twin_bytes()
                            .checked_mul(2)
                            .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?,
                    )
                    .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
            }
        }
        crate::mutable_instrument::validate_mutable_graph_budget(
            existing_stateful_count,
            existing_stateful_bytes,
            mutable_count,
            mutable_bytes,
        )
        .map_err(SessionPlaybackError::from)?;
        if mutable_count > 0 {
            crate::mutable_instrument::host_storage_upper_bound(MAX_TRACKS)
                .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
        }
        let mut mutable_instruments = Vec::new();
        mutable_instruments
            .try_reserve_exact(arrangement_tracks.len())
            .map_err(|_| SessionPlaybackError::Allocation)?;
        if mutable_instruments.capacity() > MAX_TRACKS {
            return Err(SessionPlaybackError::MutableInstrumentRuntime);
        }
        for track in arrangement_tracks {
            let id = track
                .canonical_track_id()
                .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
            let runtime = revision
                .session()
                .mutable_instrument_factory(id)
                .map(|factory| {
                    MutableInstrumentTrackRuntime::new(
                        factory,
                        revision.arrangement().sample_rate(),
                    )
                })
                .transpose()
                .map_err(SessionPlaybackError::from)?;
            mutable_instruments.push(runtime);
        }
        let mut mutable_events = Vec::new();
        mutable_events
            .try_reserve_exact(crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        if mutable_events.capacity() > crate::mutable_instrument::MAX_MUTABLE_EVENTS_PER_QUERY {
            return Err(SessionPlaybackError::MutableInstrumentRuntime);
        }
        let mut route_scratch = Vec::new();
        route_scratch
            .try_reserve_exact(routes.len())
            .map_err(|_| SessionPlaybackError::Allocation)?;
        route_scratch.extend_from_slice(&routes);

        let mut device_chains = Vec::new();
        device_chains
            .try_reserve_exact(arrangement_tracks.len())
            .map_err(|_| SessionPlaybackError::Allocation)?;
        for track in arrangement_tracks {
            device_chains.push(
                track
                    .new_device_runtime()
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?,
            );
        }
        let mut tail_groups = Vec::new();
        tail_groups
            .try_reserve_exact(SESSION_TAIL_GROUP_CAPACITY)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        tail_groups.resize_with(SESSION_TAIL_GROUP_CAPACITY, || None);
        let mut tail_mutable_instruments = Vec::new();
        tail_mutable_instruments
            .try_reserve_exact(SESSION_TAIL_GROUP_CAPACITY)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        tail_mutable_instruments.resize_with(SESSION_TAIL_GROUP_CAPACITY, || None);
        let mut tail_suffix_starts = Vec::new();
        tail_suffix_starts
            .try_reserve_exact(SESSION_TAIL_GROUP_CAPACITY)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        tail_suffix_starts.resize(SESSION_TAIL_GROUP_CAPACITY, 0);
        let mut tail_suffix_matrices = Vec::new();
        tail_suffix_matrices
            .try_reserve_exact(SESSION_TAIL_GROUP_CAPACITY)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        tail_suffix_matrices.resize(SESSION_TAIL_GROUP_CAPACITY, None);
        let mut tail_mixes = Vec::new();
        tail_mixes
            .try_reserve_exact(SESSION_TAIL_GROUP_CAPACITY)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        tail_mixes.resize(
            SESSION_TAIL_GROUP_CAPACITY,
            crate::PreparedTrackMix::identity(),
        );

        let mut releases = Vec::new();
        releases
            .try_reserve_exact(release_capacity)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        let mut release_scratch = Vec::new();
        release_scratch
            .try_reserve_exact(release_capacity)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        let mut retained_contexts = Vec::new();
        retained_contexts
            .try_reserve_exact(crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        retained_contexts.resize_with(crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS, || {
            None
        });
        let mut context_lease_plan = Vec::new();
        context_lease_plan
            .try_reserve_exact(crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS)
            .map_err(|_| SessionPlaybackError::Allocation)?;

        let mut suppressions = Vec::new();
        suppressions
            .try_reserve_exact(release_capacity)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        let mut suppression_scratch = Vec::new();
        suppression_scratch
            .try_reserve_exact(release_capacity)
            .map_err(|_| SessionPlaybackError::Allocation)?;

        Ok(Self {
            revision,
            periodic_tempo,
            anchor: None,
            historical_floor: None,
            route_historical_floors: [None; MAX_TRACKS],
            routes,
            route_scratch,
            device_chains,
            mutable_instruments,
            mutable_onsets: MutableOnsetBuffer::default(),
            mutable_events,
            tail_groups,
            tail_mutable_instruments,
            tail_mixes,
            tail_suffix_starts,
            tail_suffix_matrices,
            releases,
            release_scratch,
            retained_contexts,
            context_lease_plan,
            release_capacity,
            release_order: 0,
            release_order_scratch: 0,
            suppressions,
            suppression_scratch,
            spans: MidiSpanBuffer::default(),
            performance_frame: 0,
            arrangement_frame: 0,
            playing: false,
            linear_arrangement_export,
            route_state_generation: 0,
            candidate_admission_open: true,
        })
    }

    pub fn performance_frame(&self) -> u64 {
        self.performance_frame
    }

    pub fn arrangement_frame(&self) -> u64 {
        self.arrangement_frame
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Opaque change stamp for the displayed route state. It wraps on change;
    /// callers compare for equality and must not use it for musical arithmetic.
    pub fn route_state_generation(&self) -> u64 {
        self.route_state_generation
    }

    /// Number of stable-track route entries in project order.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Read one route without allocation. `None` means the index is out of range.
    pub fn route_status(&self, index: usize) -> Option<SessionRouteStatus> {
        let route = self.routes.get(index)?;
        let (owner, active_launch) = match route.owner {
            RouteOwner::Arrangement => (SessionPlaybackOwner::Arrangement, None),
            RouteOwner::Session { clip, launch } => {
                (SessionPlaybackOwner::Session { clip }, Some(launch))
            }
            RouteOwner::SessionAudio { clip, .. } => (SessionPlaybackOwner::Session { clip }, None),
            RouteOwner::Silence => (SessionPlaybackOwner::Silence, None),
        };
        let pending = route.pending.map(|pending| SessionPendingRouteStatus {
            target: match pending.target {
                RouteTarget::Arrangement => SessionRouteTarget::Arrangement,
                RouteTarget::Session(clip) => SessionRouteTarget::Session { clip },
                RouteTarget::Silence => SessionRouteTarget::Silence,
            },
            performance_frame: pending.frame,
            beat: pending.beat,
        });
        Some(SessionRouteStatus {
            track: route.id,
            owner,
            active_launch,
            pending,
        })
    }

    /// Seed a fresh all-Arrangement runtime from legacy finite transport state.
    /// This is used only when a graph first gains Session capability; no Session
    /// route, pending action, or release bank is copied from the legacy graph.
    pub(crate) fn seed_legacy_transport(
        &mut self,
        arrangement_frame: u64,
        performance_frame: u64,
        playing: bool,
    ) -> Result<(), SessionPlaybackError> {
        self.require_fresh_candidate()?;
        self.validate_legacy_transport(performance_frame)?;
        self.candidate_admission_open = false;
        self.arrangement_frame = arrangement_frame;
        self.performance_frame = performance_frame;
        self.playing = playing;
        Ok(())
    }

    pub(crate) fn validate_legacy_transport(
        &self,
        performance_frame: u64,
    ) -> Result<(), SessionPlaybackError> {
        use crate::MidiTiming;
        // Legacy finite playback accepts frame positions beyond the Session
        // beat domain. Refuse promotion before modifying the candidate rather
        // than accepting a graph that will fail at its first render/launch.
        base_timing_for(self.revision.session(), self.periodic_tempo)
            .frame_to_beat(performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        Ok(())
    }

    /// A Session runtime can fall back to the legacy finite renderer only when
    /// doing so cannot discard a selected route or any carried control state.
    pub(crate) fn can_demote_to_arrangement(&self) -> bool {
        self.routes.iter().enumerate().all(|(index, route)| {
            if route.owner != RouteOwner::Arrangement || route.pending.is_some() {
                return false;
            }
            let Some(track) = self.revision.arrangement().tracks().get(index) else {
                return false;
            };
            if !track.instrument_enabled() {
                return true;
            }
            let mut active = false;
            track
                .for_each_active_midi_span(self.arrangement_frame, |_, _| active = true)
                .is_ok()
                && !active
        }) && self.releases.is_empty()
            && self.suppressions.is_empty()
            && (self.performance_frame == 0
                || !self
                    .revision
                    .arrangement()
                    .tracks()
                    .iter()
                    .any(crate::Track::has_stateful_devices))
    }

    pub(crate) fn has_active_arrangement_voices(&self) -> bool {
        self.routes.iter().enumerate().any(|(index, route)| {
            if route.owner != RouteOwner::Arrangement {
                return false;
            }
            let Some(track) = self.revision.arrangement().tracks().get(index) else {
                return false;
            };
            if !track.instrument_enabled() {
                return false;
            }
            let mut active = false;
            track
                .for_each_active_midi_span(self.arrangement_frame, |_, _| active = true)
                .is_err()
                || active
        })
    }

    pub fn release_voice_count(&self) -> usize {
        self.releases.len()
    }

    pub fn release_voice_capacity(&self) -> usize {
        self.release_capacity
    }

    /// Preallocated shared capacity for carried and outgoing voices.
    pub fn release_storage_capacity(&self) -> usize {
        self.release_capacity
    }

    /// Reconcile this unused prepared runtime with an accepted predecessor.
    /// Preflight refusals, including `ReleaseBankFull`, leave both committed
    /// runtimes unchanged and keep this candidate available for retry. Once
    /// runtime inheritance starts, admission closes before any possibly partial
    /// mutation; a later error consumes the candidate. Any public apply/render
    /// attempt also consumes candidate admission, even if rendering is refused;
    /// an accepted legacy transport seed does as well. Handoff uses preallocated
    /// route/release storage and retains at most one
    /// `Arc` per unique carried shared context; it never retains an old playback
    /// graph or allocates an owner per voice.
    pub fn try_inherit_state_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        // Linear export is a deliberately fresh Arrangement-only owner. It
        // cannot inherit route state, Session launches, or a live transport
        // clock from the interactive playback graph.
        if self.linear_arrangement_export || previous.linear_arrangement_export {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.require_fresh_candidate()?;
        self.inherit_state(previous)
    }

    pub(super) fn require_fresh_candidate(&self) -> Result<(), SessionPlaybackError> {
        if self.candidate_admission_open {
            Ok(())
        } else {
            Err(SessionPlaybackError::CandidateAlreadyUsed)
        }
    }

    pub fn owner(&self, track: TrackId) -> Option<SessionPlaybackOwner> {
        self.routes
            .iter()
            .find(|route| route.id == track)
            .map(|route| match route.owner {
                RouteOwner::Arrangement => SessionPlaybackOwner::Arrangement,
                RouteOwner::Session { clip, .. } => SessionPlaybackOwner::Session { clip },
                RouteOwner::SessionAudio { clip, .. } => SessionPlaybackOwner::Session { clip },
                RouteOwner::Silence => SessionPlaybackOwner::Silence,
            })
    }

    pub(super) fn timing_view(&self) -> SessionTimingView<'_> {
        timing_view_for(self.revision.session(), self.periodic_tempo, self.anchor)
    }

    fn owner_for_target(
        &self,
        target: RouteTarget,
        beat: BeatTime,
        frame: u64,
    ) -> Result<RouteOwner, SessionPlaybackError> {
        Ok(match target {
            RouteTarget::Arrangement => RouteOwner::Arrangement,
            RouteTarget::Silence => RouteOwner::Silence,
            RouteTarget::Session(clip) if self.revision.session().audio_clip(clip).is_some() => {
                RouteOwner::SessionAudio {
                    clip,
                    launch_frame: frame,
                }
            }
            RouteTarget::Session(clip) if self.revision.session().midi_clip(clip).is_some() => {
                RouteOwner::Session { clip, launch: beat }
            }
            RouteTarget::Session(_) => return Err(SessionPlaybackError::UnknownClip),
        })
    }
}

pub(super) fn timing_view_for(
    session: &crate::PreparedSessionInventory,
    periodic_tempo: Option<PreparedPeriodicSessionTempo>,
    anchor: Option<SessionClockAnchor>,
) -> SessionTimingView<'_> {
    let base = base_timing_for(session, periodic_tempo);
    anchor.map_or_else(
        || SessionTimingView::from_base(base),
        |anchor| SessionTimingView::Anchored(AnchoredSessionTempoView::new(base, anchor)),
    )
}

pub(super) fn base_timing_for(
    session: &crate::PreparedSessionInventory,
    periodic_tempo: Option<PreparedPeriodicSessionTempo>,
) -> SessionBaseTimingView<'_> {
    let canonical = session.timing();
    periodic_tempo.map_or(SessionBaseTimingView::Canonical(canonical), |periodic| {
        SessionBaseTimingView::Periodic(periodic.view(canonical))
    })
}

mod commands;
mod handoff;
mod render;
#[cfg(test)]
mod tests;

fn append_release(
    releases: &mut Vec<ReleaseEntry>,
    capacity: usize,
    next_order: &mut u64,
    mut entry: ReleaseEntry,
) -> Result<(), SessionPlaybackError> {
    if releases.len() >= capacity {
        return Err(SessionPlaybackError::ReleaseBankFull);
    }
    let order = *next_order;
    *next_order = next_order
        .checked_add(1)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    entry.order = order;
    releases.push(entry);
    Ok(())
}
