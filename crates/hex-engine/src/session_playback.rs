//! Headless, bounded mixed-route playback over one prepared revision.
//!
//! Each track has one Arrangement, Session or Silence owner. Arrangement position and the
//! monotonic performance clock are distinct frame domains; route transitions use canonical
//! prepared musical timing. An exhausted Session one-shot keeps its route until an explicit
//! change. Compatible handoffs inherit actual runtime history; cut voices retain their original
//! sampler, suffix/mix and bounded tail owner. Tail banks are preallocated, and ownership moves
//! only after complete preflight; retired runtime/context destruction belongs on control. The
//! private loaded-sampler constructor retains its narrower transport/source class and must not be
//! treated as ordinary mixed-instrument publication.
use crate::prepared_tempo::{
    AnchoredSessionTempoView, PreparedPeriodicSessionTempo, SessionBaseTimingView,
    SessionClockAnchor, SessionTimingView,
};
use crate::{
    Frame, PreparedRevision,
    midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK,
    midi_template::{
        MidiOccurrenceSuppression, MidiReleaseVoice, MidiSpanBuffer, SessionMidiOrigin,
    },
    mutable_instrument::{
        MutableInstrumentTail, MutableInstrumentTrackRuntime, MutableOnsetBuffer,
    },
};
use hex_project::{BeatTime, ClipId, MAX_TRACKS, NoteId, SceneId, TrackId};
use std::{fmt, sync::Arc};

mod browser_audition;
mod candidate_storage;
mod foreign_arrangement;
mod midi_capture;
mod mutable_checkpoint;
mod render_work;
mod track_fader;
mod transport_loop_admission;
pub use mutable_checkpoint::{
    MAX_MUTABLE_RUNTIME_CHECKPOINT_STORE_BYTES, MAX_MUTABLE_RUNTIME_CHECKPOINTS,
    MutableRuntimeCheckpointError, MutableRuntimeCheckpointStore,
};

pub use crate::launch_quantization::LaunchQuantization;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPlaybackCommand {
    Play,
    Pause,
    Stop,
    /// Absolute first-pass cursor. At/after the loop end, retain the canonical
    /// linear tempo/source law instead of normalizing backwards into the loop.
    Seek(u64),
    LaunchClip {
        clip: ClipId,
        quantization: LaunchQuantization,
    },
    /// Immediately cue an already active Session MIDI source without seeking
    /// either transport clock or changing the saved source/play-start markers.
    CueMidiClip {
        track: TrackId,
        clip: ClipId,
        source_beat: BeatTime,
        expected_route_generation: u64,
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

/// The Session Arrangement loop law and its current iteration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrangementLoopState {
    pub pass: u64,
    /// Active half-open Arrangement frame interval, when looping applies.
    pub active_region: Option<(u64, u64)>,
}

/// One stable-track route as displayed at a Session route snapshot cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionRouteStatus {
    pub track: TrackId,
    pub owner: SessionPlaybackOwner,
    /// Present only for a Session MIDI owner; both coordinates remain global/cued as typed.
    pub active_origin: Option<SessionMidiOrigin>,
    /// Present only for a Session audio owner: the performance frame at which
    /// its source elapsed time is zero (a launch or legacy seed, not a cursor).
    pub audio_launch_frame: Option<u64>,
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
    Capture(capture::OutputCaptureError),
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
    /// An incompatible latency history cannot be transferred while transport
    /// retains its current position and processing state.
    LatencyLayoutChanged,
    /// An insertion cannot inherit the running clock and audio continuously.
    DeviceInsertNeedsConfirmation,
    /// Active source history cannot be adopted by the replacement provider.
    SourceHistoryUnavailable,
    /// The active detached receiver still owns effect history that must not
    /// be replaced by a live reset/chase. The prior graph keeps rendering.
    DetachedReceiverHistoryUnavailable,
    /// The exact foreign endpoint is still needed by an outgoing tail. A
    /// publication owner may retry pure handoff planning after real rendering
    /// advances that tail; it must not duplicate, reset or substitute the source.
    ForeignTailSourceBusy,
    /// The predecessor currently renders no device chains, so retrying the
    /// streamed handoff cannot advance the outgoing foreign effect tail.
    ForeignTailSourcePaused,
    /// The retained group path is still processing an outgoing tail. The
    /// publication owner can retry once the predecessor drains or stops.
    GroupTailRoutingBusy,
    /// A device runtime's fixed live-transition capacity (outgoing routing or
    /// audition branches) is full (Pulse #4178). The pure preflight refused
    /// before any ownership change; the publication owner retries after
    /// rendering advances the committed transitions. Never answered by a
    /// reset, detached tail or fresh runtime, which would drop or duplicate
    /// an audible branch. An idle predecessor that renders no device chains
    /// (paused, no monitoring) is never left busy: the planner settles its
    /// silent transitions instead (Pulse #4558).
    DeviceTransitionBusy,
    MutableInstrumentRuntime,
    /// Existing authored and live voices leave insufficient room for the launch.
    SharedSourceVoiceCapacity,
    MutableInstrumentDiagnostic(&'static str),
    MutableInstrumentEvents,
    MutableInstrumentLoopUnsupported,
    MutableInstrumentLoopDensity,
    MutableInstrumentActive,
    MutableInstrumentLoopBoundsActive,
    MutableInstrumentRouteBoundary,
    MutableInstrumentChase,
    SharedAnalyticContext,
    MediaPagesUnavailable,
    CandidateAlreadyUsed,
    Midi(crate::MidiTemplateError),
    Arrangement(&'static str),
}

impl SessionPlaybackError {
    /// Static render cause: safe to propagate without formatting or allocating
    /// in the audio callback. Detailed numeric timing formatting stays on control.
    pub(crate) fn diagnostic_message(self) -> &'static str {
        match self {
            Self::Capture(error) => error.message(),
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
            Self::LatencyLayoutChanged => {
                "latency layout could not change live · previous audio continues"
            }
            Self::DeviceInsertNeedsConfirmation => {
                "device insertion needs confirmation before interrupting playback"
            }
            Self::SourceHistoryUnavailable | Self::DetachedReceiverHistoryUnavailable => {
                "source history cannot transfer live · previous audio continues"
            }
            Self::ForeignTailSourceBusy => {
                "native effect endpoint is still owned by an outgoing tail"
            }
            Self::ForeignTailSourcePaused => {
                "native effect tail is paused · resume playback, then retry"
            }
            Self::GroupTailRoutingBusy => {
                "group routing change is waiting for an outgoing tail · stop or wait for it to finish"
            }
            Self::DeviceTransitionBusy => {
                "device change is waiting for an audible transition to finish"
            }
            Self::MutableInstrumentRuntime => {
                "session playback mutable instrument runtime is unavailable"
            }
            Self::SharedSourceVoiceCapacity => {
                "shared source voice capacity is full · release notes or wait for tails, then retry"
            }
            Self::MutableInstrumentDiagnostic(message) => message,
            Self::MutableInstrumentEvents => {
                "session playback mutable instrument event query is invalid"
            }
            Self::MutableInstrumentLoopUnsupported => {
                "mutable instrument source loops are unsupported by this playback mode"
            }
            Self::MutableInstrumentLoopDensity => {
                "mutable instrument transport loop exceeds bounded event or voice density"
            }
            Self::MutableInstrumentActive => {
                "mutable instrument must become quiescent before this route change"
            }
            Self::MutableInstrumentLoopBoundsActive => {
                "transport loop bounds cannot change while a mutable Arrangement gate is active"
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
            Self::Midi(error) => error.diagnostic_message(),
            Self::Arrangement(message) => message,
            Self::SessionTempoUnavailable(error) => match error {
                crate::PreparedTempoError::BeatToFrame(message)
                | crate::PreparedTempoError::FrameToBeat(message)
                | crate::PreparedTempoError::BeatLowerBound(message)
                | crate::PreparedTempoError::InvalidLoop(message) => message,
                crate::PreparedTempoError::SampleRateMismatch { .. } => {
                    "session tempo sample rates differ"
                }
                crate::PreparedTempoError::PointCountOutOfRange { .. } => {
                    "session tempo point count is out of range"
                }
            },
        }
    }
}

impl fmt::Display for SessionPlaybackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionTempoUnavailable(error) => {
                write!(f, "session tempo is unavailable: {error}")
            }
            _ => f.write_str(self.diagnostic_message()),
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
    Session {
        clip: ClipId,
        origin: SessionMidiOrigin,
    },
    SessionAudio {
        clip: ClipId,
        launch_frame: u64,
    },
    Silence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CurrentRouteReservation {
    incarnation: u64,
    survivors: usize,
    note_offs: usize,
    incoming_voices: usize,
    incoming_onsets: usize,
}
#[derive(Clone, Copy, Debug)]
struct CurrentPendingCut {
    current_sampler_cut: bool,
    owner: RouteOwner,
    incarnation: u64,
    frame: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingRoute {
    frame: u64,
    beat: BeatTime,
    target: RouteTarget,
    current_reservation: Option<CurrentRouteReservation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackRoute {
    id: TrackId,
    owner: RouteOwner,
    pending: Option<PendingRoute>,
    current_incarnation: u64,
    current_first_frame: bool,
    /// Outgoing Session audio clip still fading; see `audio_release`.
    audio_release: Option<audio_release::AudioRouteRelease>,
}

#[derive(Clone, Copy)]
struct TransitionMoment {
    target_frame: u64,
    target_beat: BeatTime,
    immediate: bool,
    source_frame: u64,
    performance_frame: u64,
}

/// Actual source/output custody recorded at analytic release birth. TrackId on
/// ReleaseEntry remains the contributor/note identity. A current source may
/// reroute its summed releases through a new receiver; detached releases retain
/// the captured receiver suffix group and never infer origin from track binding.
#[derive(Clone, Copy, Debug)]
struct ReleaseSourceOrigin {
    source: hex_project::SourceId,
    receiver: TrackId,
    follows_current_configuration: bool,
}
impl ReleaseSourceOrigin {
    fn capture(revision: &PreparedRevision, contributor: usize) -> Option<Self> {
        let routing = revision.source_routing();
        Some(Self {
            source: routing.source(contributor)?,
            receiver: revision
                .arrangement()
                .tracks()
                .get(routing.receiver_index(contributor)?)?
                .canonical_track_id()?,
            follows_current_configuration: true,
        })
    }
}

/// The timeline which created a retained MIDI voice. This remains fixed even
/// when its track is later routed to the other view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReleaseTimeline {
    Arrangement,
    Session,
}

#[derive(Clone, Copy, Debug)]
struct ReleaseEntry {
    source_origin: Option<ReleaseSourceOrigin>,
    timeline: ReleaseTimeline,
    // Only paused monitoring advances this offset. Keeping it on Play avoids
    // rebasing an old onset into an unrepresentable negative musical frame.
    monitor_offset: u64,
    location: ReleaseLocation,
    track_id: TrackId,
    order: u64,
    mix: crate::PreparedTrackMix,
    continuation: Option<SessionContinuation>,
    inline_tuning_owned: bool,
    clip_gain: f64,
    voice: MidiReleaseVoice,
}

impl ReleaseEntry {
    fn end_frame(&self) -> u64 {
        self.voice.end_frame().saturating_sub(self.monitor_offset)
    }
    fn sample_frame(&self, musical: u64) -> Result<u64, SessionPlaybackError> {
        musical
            .checked_add(self.monitor_offset)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)
    }
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

/// Shared conservative owning-location count for fresh closed constructors.
/// Playback contributes five outer locations and Session three singleton roots.
pub(crate) fn candidate_group_reservation(
    tracks: usize,
    returns: usize,
) -> Result<usize, crate::AudioStorageError> {
    tracks
        .checked_mul(2)
        .and_then(|n| n.checked_add(returns))
        .and_then(|n| n.checked_add(SESSION_TAIL_GROUP_CAPACITY.checked_mul(2)?))
        .and_then(|n| n.checked_add(return_effect_tails::RETURN_EFFECT_TAIL_CAPACITY))
        .and_then(|n| n.checked_add(crate::shared_analytic::MAX_RETAINED_SHARED_CONTEXTS))
        .and_then(|n| n.checked_add(8))
        .ok_or(crate::AudioStorageError::Overflow)
}

const SESSION_RELEASE_CAPACITY: usize = MAX_TRACKS * MAX_SIMULTANEOUS_VOICES_PER_TRACK;
const SESSION_TAIL_GROUP_CAPACITY: usize = hex_lattice::MAX_STATEFUL_PROCESSORS;
const MAX_RETAINED_SUFFIX_ENTRIES: usize = 4_096;

/// Control-owned source conversion ceiling, separate from general WAV export.
pub const MAX_FINITE_BOUNCE_FRAMES: u64 = hex_project::MAX_ARRANGEMENT_BOUNCE_FRAMES;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RenderConstruction {
    Interactive,
    /// The resident keeps every instrument runtime and lends its source
    /// slots for a paused render. This constructor owns downstream DSP only.
    BorrowedPauseSources,
    LinearExport,
    LinearForeignExport,
    FiniteBounce,
    FiniteForeignBounce,
}
/// Finite serial-effect drain admission horizon. Ten minutes admits useful
/// chains of long-decay rooms without weakening processor-count or byte bounds.
/// It is a reclamation deadline, not additional per-block work or a transport
/// delay; quiescence permits earlier release and Stop clears retained histories.
pub(crate) const MAX_FINITE_EFFECT_TAIL_SECONDS: u64 = 600;

/// Bounded result for targets canceled because their stable track or clip was
/// removed by the candidate revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionHandoffReport {
    cancelled_pending: [Option<TrackId>; MAX_TRACKS],
    cancelled_pending_len: usize,
    running_audio_restart: bool,
}

impl SessionHandoffReport {
    pub fn cancelled_pending_tracks(&self) -> &[Option<TrackId>] {
        &self.cancelled_pending[..self.cancelled_pending_len]
    }

    /// Whether this plan resets or relocates audio while the predecessor plays.
    /// A preview observes one predecessor and does not authorize a later commit.
    pub fn running_audio_restart(&self) -> bool {
        self.running_audio_restart
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static CURRENT_SAMPLER_RUNTIME_TEST_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES) };
}

/// Independent, headless mixed Arrangement/Session render kernel.
///
/// Routes and release storage are allocated at construction. Rendering uses
/// copied route state and commits clocks/routes/releases only after the entire
/// requested block succeeds. The existing native `Playback` remains unchanged.
#[derive(Clone, Copy)]
struct PendingSharedChase {
    signature: crate::mutable_instrument::MutableOccurrenceSignature,
    frame: u64,
}
#[derive(Clone, Copy)]
struct PendingSharedLive {
    input: crate::mutable_instrument::LiveInputId,
    token: u64,
    frame: u64,
}

pub struct SessionPlayback {
    pub(crate) live_expression: Option<Arc<crate::live_expression::LiveExpressionInput>>,
    pub(crate) computer_keyboard: Option<Box<crate::computer_keyboard::ComputerKeyboardRuntime>>,
    pub(crate) paused_source_monitor: Option<Box<PausedTrackMonitor>>,
    pub(crate) defer_paused_source_monitor: bool,
    pub(crate) current_sampler_keyboard_epoch: u64,
    pub(crate) current_sampler_keyboard_revoked: u64,
    // Nontransferable constructor-owned pairs; no UI input/source owners.
    pub(crate) current_sampler_keyboards:
        Vec<Box<crate::computer_keyboard::ComputerKeyboardRuntime>>,
    capture_runtime: u64,
    capture_epoch: u64,
    gain_recording_override: Option<f32>,
    live_gain: gain_recording::LiveGain,
    live_gain_scratch: gain_recording::LiveGain,
    // Optional control allocation; the callback mutates Copy contents only.
    native_parameter_state: Option<Box<native_parameter_input::State>>,
    compensation: Option<crate::signal_graph::SignalRuntime>,
    device_metering: bool,
    render_work: render_work::Sampler,
    revision: Arc<PreparedRevision>,
    /// Graph-local section audition; expires when the source graph is replaced.
    pub(crate) transient_loop: Option<hex_project::LoopRegion>,
    browser_audition: browser_audition::BrowserAudition,
    pub(crate) midi_admission: Option<crate::session_inventory::PreparedSessionMidiAdmission>,
    periodic_tempo: Option<PreparedPeriodicSessionTempo>,
    anchor: Option<SessionClockAnchor>,
    /// Reset-and-chase frontier, retained until a successful nonempty render.
    interactive_start_frame: Option<u64>,
    // One exact muted-onset chase, retained until a successful render.
    pending_shared_chase: Option<PendingSharedChase>,
    pending_shared_live: Option<PendingSharedLive>,
    historical_floor: Option<BeatTime>,
    route_historical_floors: [Option<BeatTime>; MAX_TRACKS],
    source_voice_envelopes: [u16; MAX_TRACKS],
    /// Per-voice streamed page-miss fades, lent to the page view for each
    /// render; see `crate::stream_miss_fade` for ownership and resets.
    stream_miss_fades: crate::StreamMissFades,
    routes: Vec<TrackRoute>,
    route_scratch: Vec<TrackRoute>,
    track_faders: [track_fader::TrackFader; MAX_TRACKS],
    track_fader_scratch: [track_fader::TrackFader; MAX_TRACKS],
    source_effects: crate::source_effect_stage::SourceEffectStages,
    device_chains: Vec<hex_lattice::NativeRuntimeChain>,
    return_chains: Vec<hex_lattice::NativeRuntimeChain>,
    main_chain: hex_lattice::NativeRuntimeChain,
    return_scratch: Vec<[Frame; 256]>,
    // Constructor-owned, transaction-local resident source windows.
    resident_source_scratch: Vec<[Frame; 256]>,
    meter_cells: Vec<meter_capture::Cell>,
    return_effect_tails: ReturnEffectTailBank,
    mutable_instruments: Vec<Option<MutableInstrumentTrackRuntime>>,
    foreign_arrangement: foreign_arrangement::ForeignArrangement,
    mutable_onsets: MutableOnsetBuffer,
    mutable_events: Vec<hex_lattice::NativeInstrumentEvent>,
    current_route_cut_scratch: Vec<Option<CurrentPendingCut>>,
    tail_groups: Vec<Option<hex_lattice::NativeRuntimeChain>>,
    // Immutable original ownership, including effect-only suffixes. Inline
    // slots are charged by size_of::<Self>() in actual and paired preflight.
    tail_track_ids: [Option<TrackId>; SESSION_TAIL_GROUP_CAPACITY],
    // Only tails admitted against the identical compensated group graph may
    // contribute to that graph's current send and feed edges.
    tail_route_sends: [bool; SESSION_TAIL_GROUP_CAPACITY],
    has_tail_route_sends: bool,
    // A pure foreign suffix admitted with an unchanged grouped clock and graph.
    tail_proven_grouped: [bool; SESSION_TAIL_GROUP_CAPACITY],
    tail_mutable_instruments: Vec<Option<MutableInstrumentTail>>,
    tail_mixes: Vec<crate::PreparedTrackMix>,
    tail_suffix_starts: Vec<usize>,
    tail_suffix_matrices: Vec<Option<hex_lattice::StereoMatrix>>,
    tail_output_active: [bool; SESSION_TAIL_GROUP_CAPACITY],
    tail_output_scratch: [bool; SESSION_TAIL_GROUP_CAPACITY],
    // Occupied tail prefix for the current render transaction.
    detached_tail_group_limit: usize,
    releases: Vec<ReleaseEntry>,
    release_scratch: Vec<ReleaseEntry>,
    retained_contexts: Vec<Option<Arc<crate::shared_analytic::SharedContextOwner>>>,
    context_lease_plan: Vec<crate::shared_analytic::ContextLeasePlan>,
    // Fixed MAX_TRACKS slots, allocated on control and indexed by immutable
    // revision context. Actual capacities are charged in preroll accounting.
    analytic_continuations: Vec<Option<hex_lattice::AnalyticContinuation>>,
    analytic_continuation_scratch: Vec<Option<hex_lattice::AnalyticContinuation>>,
    release_capacity: usize,
    release_order: u64,
    release_order_scratch: u64,
    suppressions: Vec<MidiOccurrenceSuppression>,
    suppression_scratch: Vec<MidiOccurrenceSuppression>,
    // Keep the fixed large span payload out of by-value constructor temporaries.
    // Its stable Box is allocated on control and retired with this runtime.
    spans: Box<MidiSpanBuffer>,
    analytic_import: analytic_import::ImportPlan,
    performance_frame: u64,
    arrangement_frame: u64,
    arrangement_loop_pass: u64,
    playing: bool,
    // Pause/live activation admits source/FX draining. That intent survives
    // handoffs, input removal and later Pause even after receiver removal;
    // Stop/seek/preroll reset it. A stopped seek keeps authored history frozen
    // until Play or live activation. False therefore permits Main-tail-only
    // draining without ticking a fresh replacement processor's input clock.
    monitor_source_releases: bool,
    // Present only for the independently prepared PCM monitor. A validated
    // rendered block keeps its private processor and delay history live.
    stopped_input: Option<StoppedInputHistory>,
    linear_arrangement_export: bool,
    current_sampler_console: bool,
    route_state_generation: u64,
    candidate_admission_open: bool,
    pub(crate) require_continuous_insert: bool,
    pub(crate) foreign_continuation: Option<Box<crate::playback::StoredForeignContinuation>>,
    // Control-side request that the live handoff restart at the cursor rather
    // than carry voices or detached tails the candidate already represents.
    live_restart: bool,
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

fn effective_midi_route(
    revision: &PreparedRevision,
    routes: &[TrackRoute],
    index: usize,
) -> Result<TrackRoute, SessionPlaybackError> {
    let mut route = *routes
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    let track = revision
        .arrangement()
        .tracks()
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    if let Some(source) = track.midi_source_index {
        let source = routes
            .get(source)
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        route.owner = source.owner;
        route.pending = source.pending;
    }
    Ok(route)
}

impl SessionPlayback {
    pub(super) fn source_voice_envelope(&self, index: usize) -> usize {
        self.source_voice_envelopes
            .get(index)
            .copied()
            .map_or(0, usize::from)
    }

    pub(crate) fn visit_foreign_runtime_bindings(
        &self,
        visit: &mut impl FnMut(Option<hex_lattice_contract::ForeignProcessorBinding>),
    ) {
        for node in self
            .revision
            .arrangement()
            .tracks()
            .iter()
            .filter_map(|track| track.foreign_instrument.as_ref())
            .filter(|node| !node.bypassed)
        {
            if let Some(hex_lattice::PreparedProcessorFactory::Foreign(facts)) = node.definition {
                visit(facts.binding());
            }
        }
        let chains = self
            .device_chains
            .iter()
            .chain(self.return_chains.iter())
            .chain(std::iter::once(&self.main_chain))
            .chain(
                self.tail_groups
                    .iter()
                    .enumerate()
                    .filter_map(|(index, chain)| {
                        self.tail_output_active
                            .get(index)
                            .copied()
                            .unwrap_or(false)
                            .then_some(chain.as_ref())
                            .flatten()
                    }),
            )
            .chain(
                self.return_effect_tails
                    .slots()
                    .iter()
                    .flatten()
                    .filter(|tail| tail.output_active)
                    .map(|tail| &tail.chain),
            );
        for chain in chains {
            for binding in chain.foreign_bindings() {
                visit(binding);
            }
        }
    }
    pub(crate) fn device_resource_lease_on_control(
        &self,
        track: TrackId,
    ) -> Result<Option<crate::native_device_resources::PreparedDeviceResourceLease>, crate::Invalid>
    {
        let index = self
            .routes
            .iter()
            .position(|route| route.id == track)
            .ok_or(crate::Invalid("device resource track is unavailable"))?;
        self.mutable_instruments
            .get(index)
            .and_then(Option::as_ref)
            .and_then(|runtime| runtime.resources.as_ref())
            .map(|lease| {
                lease
                    .capture_on_control()
                    .map_err(|_| crate::Invalid("device resource lease capture refused"))
            })
            .transpose()
    }
    pub(crate) fn device_resource_leases(
        &self,
    ) -> impl Iterator<Item = &crate::native_device_resources::PreparedDeviceResourceLease> {
        self.mutable_instruments
            .iter()
            .flatten()
            .filter_map(|runtime| runtime.resources.as_ref())
            .chain(
                self.tail_mutable_instruments
                    .iter()
                    .flatten()
                    .filter_map(|tail| tail.runtime.resources.as_ref()),
            )
            .chain(
                self.computer_keyboard
                    .iter()
                    .filter_map(|keyboard| keyboard.resources.as_ref()),
            )
    }
    pub(crate) fn new_audio_candidate(
        revision: Arc<PreparedRevision>,
    ) -> Result<Self, crate::AudioStorageError> {
        let bytes = preroll::arrangement_runtime_storage_preflight_bytes(&revision)
            .ok_or(crate::AudioStorageError::UnknownStorage)?;
        if bytes > MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES {
            return Err(crate::AudioStorageError::ReservationExceeded);
        }
        Self::new(revision).map_err(|_| crate::AudioStorageError::UnsupportedGraph)
    }
    pub(crate) fn new_current_sampler_candidate(
        revision: Arc<PreparedRevision>,
        sources: &crate::native_device_resources::current_sampler::CurrentSamplerSources,
    ) -> Result<Self, crate::AudioStorageError> {
        use crate::{
            AudioStorageError as Error, computer_keyboard::ComputerKeyboardRuntime as Keyboard,
        };
        let allowance = Self::current_sampler_runtime_preflight(&revision, sources)?;
        let runtime_limit = MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
        #[cfg(test)]
        let runtime_limit =
            CURRENT_SAMPLER_RUNTIME_TEST_LIMIT.with(|limit| limit.get().min(runtime_limit));
        if allowance > runtime_limit {
            return Err(Error::ReservationExceeded);
        }
        let mut playback = Self::new_with_policy_scoped(revision, false, None)
            .map_err(|_| Error::UnsupportedGraph)?;
        playback
            .current_sampler_keyboards
            .try_reserve_exact(sources.metadata.configurations().len())
            .map_err(|_| Error::Allocation)?;
        for config in sources.metadata.configurations() {
            let resources = sources
                .resources(config.track)
                .map_err(|_| Error::SourceMismatch)?;
            let keyboard = Keyboard::prepare_current_sampler(
                config.track,
                config.device,
                &config.prepared,
                playback.revision.arrangement().sample_rate(),
                &resources,
            )
            .map_err(|_| Error::UnsupportedGraph)?;
            playback.current_sampler_keyboards.push(keyboard);
        }
        if playback
            .accounted_runtime_storage_bytes()
            .is_none_or(|n| n > allowance)
        {
            return Err(Error::ReservationExceeded);
        }
        playback
            .current_route_cut_scratch
            .try_reserve_exact(playback.routes.len())
            .map_err(|_| Error::Allocation)?;
        playback
            .current_route_cut_scratch
            .resize(playback.routes.len(), None);
        if playback
            .accounted_runtime_storage_bytes()
            .is_none_or(|n| n > allowance)
        {
            return Err(Error::ReservationExceeded);
        }
        playback.current_sampler_console = true;
        Ok(playback)
    }
    pub(crate) fn current_sampler_console_available(&self) -> bool {
        self.current_sampler_console
    }
    pub(crate) fn current_sampler_runtime_preflight(
        revision: &PreparedRevision,
        sources: &crate::native_device_resources::current_sampler::CurrentSamplerSources,
    ) -> Result<usize, crate::AudioStorageError> {
        use crate::{
            AudioStorageError as Error, computer_keyboard::ComputerKeyboardRuntime as Keyboard,
        };
        if revision.resource_scope != Some(sources.metadata.id) {
            return Err(Error::SourceMismatch);
        }
        let keyboard_bytes = sources
            .metadata
            .configurations()
            .len()
            .checked_mul(
                Keyboard::current_sampler_preflight_bytes()
                    .and_then(|n| n.checked_add(std::mem::size_of::<Box<Keyboard>>()))
                    .ok_or(Error::Overflow)?,
            )
            .ok_or(Error::Overflow)?;
        let allowance = Self::interactive_runtime_allowance(revision)
            .map_err(|_| Error::ReservationExceeded)?
            .checked_add(keyboard_bytes)
            .and_then(|n| {
                n.checked_add(
                    revision
                        .arrangement()
                        .tracks()
                        .len()
                        .checked_mul(std::mem::size_of::<Option<CurrentPendingCut>>())?,
                )
            })
            .ok_or(Error::Overflow)?;
        if allowance > MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES {
            return Err(Error::ReservationExceeded);
        }
        Ok(allowance)
    }
    pub(crate) fn validate_candidate_runtime_storage(
        &self,
        metadata: usize,
    ) -> Result<(), crate::AudioStorageError> {
        let bytes = self
            .accounted_runtime_storage_bytes()
            .and_then(|n| n.checked_add(metadata))
            .ok_or(crate::AudioStorageError::UnknownStorage)?;
        if bytes > MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES {
            return Err(crate::AudioStorageError::ReservationExceeded);
        }
        Ok(())
    }
    /// Observation for headless profiling only. The first value is the existing
    /// runtime ledger charge; the second is the charge for detached track-tail
    /// chains already included in that total. Shared revision and source owners,
    /// native resources, allocator overhead and process memory are not included.
    #[cfg(feature = "instrument-test-support")]
    pub fn profile_runtime_storage_bytes(&self) -> Option<(usize, usize)> {
        let total = self.accounted_runtime_storage_bytes()?;
        let tails = self
            .tail_groups
            .iter()
            .flatten()
            .try_fold(0_usize, |sum, chain| {
                sum.checked_add(chain.accounted_storage_bytes()?)
            })?;
        Some((total, tails))
    }

    #[cfg(any(test, feature = "instrument-test-support"))]
    pub(crate) fn test_track_boxes(
        &self,
        track: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        let index = self.routes.iter().position(|r| r.id == track)?;
        self.device_chains
            .get(index)?
            .storage_view()
            .entries()
            .next()
            .map(|entry| {
                let (a, b) = entry.runtime_box_identities();
                if a > b { (b, a) } else { (a, b) }
            })
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub(crate) fn test_tail_boxes(
        &self,
        track: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        let index = self
            .tail_track_ids
            .iter()
            .position(|id| *id == Some(track))?;
        self.tail_groups
            .get(index)?
            .as_ref()?
            .storage_view()
            .entries()
            .next()
            .map(|entry| {
                let (a, b) = entry.runtime_box_identities();
                if a > b { (b, a) } else { (a, b) }
            })
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub(crate) fn test_tail_activity(&self, track: hex_project::TrackId) -> Option<(bool, bool)> {
        let slot = self
            .tail_track_ids
            .iter()
            .position(|id| *id == Some(track))?;
        let chain = self.tail_groups.get(slot)?.as_ref()?;
        Some((
            !chain.committed_range_is_quiescent(0..chain.len()),
            self.tail_output_active[slot],
        ))
    }
    #[cfg(test)]
    pub(crate) fn test_populate_unsupported_tail(&mut self) {
        self.tail_groups[0] = Some(hex_lattice::NativeRuntimeChain::empty());
    }
    /// Constructor-bound inventory of independently movable runtime owner
    /// locations, adapted from original177 owner_groups.rs. No callback visitor
    /// or byte/handle completeness claim. The closed constructor class permits
    /// Utility Track/Tail transfers; other owning locations remain classified.
    pub(crate) fn visit_candidate_groups(
        &self,
        visit: &mut impl FnMut(u8, usize, bool) -> Result<(), crate::AudioStorageError>,
    ) -> Result<(), crate::AudioStorageError> {
        // Exhaustive owning-field match: other buffers belong to the Session
        // container group, not independent transferable device/context groups.
        let Self {
            live_expression,
            computer_keyboard,
            paused_source_monitor: _,
            defer_paused_source_monitor: _,
            current_sampler_keyboards: _,
            current_sampler_keyboard_epoch: _,
            current_sampler_keyboard_revoked: _,
            capture_runtime: _,
            capture_epoch: _,
            gain_recording_override: _,
            live_gain: _,
            live_gain_scratch: _,
            native_parameter_state: _,
            compensation,
            device_metering: _,
            render_work: _,
            revision: _,
            transient_loop: _,
            browser_audition: _,
            midi_admission: _,
            periodic_tempo: _,
            anchor: _,
            interactive_start_frame: _,
            pending_shared_chase: _,
            pending_shared_live: _,
            historical_floor: _,
            route_historical_floors: _,
            stream_miss_fades: _,
            source_voice_envelopes: _,
            routes: _,
            route_scratch: _,
            track_faders: _,
            track_fader_scratch: _,
            source_effects: _, // retained by the Session container, not separately transferred
            device_chains,
            return_chains,
            main_chain,
            return_scratch: _,
            resident_source_scratch: _,
            meter_cells: _,
            return_effect_tails,
            mutable_instruments,
            foreign_arrangement,
            mutable_onsets: _,
            mutable_events: _,
            current_route_cut_scratch: _,
            tail_groups,
            tail_track_ids: _,
            tail_route_sends: _,
            has_tail_route_sends: _,
            tail_proven_grouped: _,
            tail_mutable_instruments,
            tail_mixes: _,
            tail_suffix_starts: _,
            tail_suffix_matrices: _,
            tail_output_active: _,
            tail_output_scratch: _,
            detached_tail_group_limit: _,
            releases: _,
            release_scratch: _,
            retained_contexts,
            context_lease_plan: _,
            analytic_continuations: _,
            analytic_continuation_scratch: _,
            release_capacity: _,
            release_order: _,
            release_order_scratch: _,
            suppressions: _,
            suppression_scratch: _,
            spans: _,
            analytic_import: _,
            performance_frame: _,
            arrangement_frame: _,
            arrangement_loop_pass: _,
            playing: _,
            monitor_source_releases: _,
            stopped_input: _,
            linear_arrangement_export: _,
            current_sampler_console: _,
            route_state_generation: _,
            candidate_admission_open: _,
            require_continuous_insert: _,
            foreign_continuation,
            live_restart: _,
        } = self;
        // Fresh Main metadata must match before any location is emitted.
        // A real owning transfer can later leave an allocation-free empty
        // Main field; its now-vacant group is checked against the origin book.
        let prepared_main = &self.revision.arrangement().main_device_chain;
        let main_matches = main_chain.len() == prepared_main.len()
            && main_chain
                .storage_view()
                .entries()
                .zip(prepared_main.iter())
                .all(|(actual, prepared)| actual.matches_prepared_runtime_storage(prepared));
        let main_present = main_chain.storage_view().entry_capacity_bytes() != Some(0);
        let moved_empty_main =
            !self.candidate_admission_open && main_chain.is_empty() && !main_present;
        if (!main_matches && !moved_empty_main)
            || foreign_continuation.is_some()
            || !foreign_arrangement.is_empty()
            || live_expression.is_some()
            || computer_keyboard.is_some()
        {
            return Err(crate::AudioStorageError::UnsupportedGraph);
        }
        for index in 0..device_chains.len() {
            visit(0, index, true)?;
        }
        for (index, owner) in tail_groups.iter().enumerate() {
            visit(1, index, owner.is_some())?;
        }
        for (index, owner) in mutable_instruments.iter().enumerate() {
            if let Some(owner) = owner
                && (self.revision.resource_scope.is_none()
                    || !owner.is_concrete_current_sampler()
                    || self
                        .revision
                        .arrangement()
                        .tracks()
                        .get(index)
                        .is_none_or(|t| t.instrument_id != owner.expression_identity))
            {
                return Err(crate::AudioStorageError::UnsupportedGraph);
            }
            visit(2, index, owner.is_some())?;
        }
        for (index, owner) in tail_mutable_instruments.iter().enumerate() {
            visit(3, index, owner.is_some())?;
        }
        for index in 0..return_chains.len() {
            visit(4, index, true)?;
        }
        for (index, owner) in return_effect_tails.slots().iter().enumerate() {
            visit(5, index, owner.is_some())?;
        }
        for (index, owner) in retained_contexts.iter().enumerate() {
            visit(6, index, owner.is_some())?;
        }
        visit(7, 0, true)?; // this exact Session revision handle
        visit(8, 0, compensation.is_some())?;
        visit(14, 0, main_present)?; // actual Main backing; moved-out fields are vacant
        Ok(())
    }
    pub(crate) fn set_device_metering(&mut self, enabled: bool) {
        self.device_metering = enabled;
        for chain in self
            .device_chains
            .iter_mut()
            .chain(self.source_effects.chains_mut())
            .chain(self.return_chains.iter_mut())
            .chain(std::iter::once(&mut self.main_chain))
        {
            chain.set_metering(enabled);
        }
    }

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

    pub(crate) fn attach_live_expression(
        &mut self,
        input: Arc<crate::live_expression::LiveExpressionInput>,
    ) -> Result<(), crate::Invalid> {
        let index = self
            .revision
            .arrangement()
            .tracks()
            .iter()
            .position(|track| track.canonical_track_id() == Some(input.track))
            .ok_or(crate::Invalid("controller input track unavailable"))?;
        if !self
            .mutable_instruments
            .get(index)
            .and_then(Option::as_ref)
            .is_some_and(
                crate::mutable_instrument::MutableInstrumentTrackRuntime::supports_live_expression,
            )
        {
            return Err(crate::Invalid(
                "this instrument mode does not support controller input",
            ));
        }
        let previous = self.live_expression.replace(input);
        if self.validate_candidate_runtime_storage(0).is_err() {
            self.live_expression = previous;
            return Err(crate::Invalid(
                "controller input exceeds graph storage budget",
            ));
        }
        Ok(())
    }
    pub(crate) fn live_expression_supported_on_control(&self, track: TrackId) -> bool {
        self.revision
            .arrangement()
            .tracks()
            .iter()
            .position(|candidate| candidate.canonical_track_id() == Some(track))
            .and_then(|index| self.mutable_instruments.get(index))
            .and_then(Option::as_ref)
            .is_some_and(
                crate::mutable_instrument::MutableInstrumentTrackRuntime::supports_live_expression,
            )
    }
    pub fn new(revision: Arc<PreparedRevision>) -> Result<Self, SessionPlaybackError> {
        Self::new_with_policy(revision, false, None)
    }

    pub(crate) fn select_processor_audition(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: Option<hex_lattice::ProcessorAuditionSelection>,
    ) -> bool {
        let project = self.revision.arrangement();
        match identity.owner {
            hex_lattice::ProcessorChainOwner::Source(_) => false,
            hex_lattice::ProcessorChainOwner::Main => self.main_chain.select_processor_audition(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection,
            ),
            hex_lattice::ProcessorChainOwner::Track(owner) => project
                .tracks()
                .iter()
                .position(|track| track.canonical_track_id().map(|id| id.value()) == Some(owner))
                .and_then(|index| self.device_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.select_processor_audition(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
            hex_lattice::ProcessorChainOwner::Return(owner) => project
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == owner)
                .and_then(|index| self.return_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.select_processor_audition(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
        }
    }

    /// Move only the held selector from an instrument-release suffix into the
    /// matching current track chain. The old suffix keeps its normal processor
    /// history and is hard-cancelled by the caller before this graph renders.
    pub(crate) fn transfer_processor_audition_from_release_tail(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: hex_lattice::ProcessorAuditionSelection,
    ) -> bool {
        let hex_lattice::ProcessorChainOwner::Track(owner) = identity.owner else {
            return false;
        };
        let tracks = self.revision.arrangement().tracks();
        let Some(track_index) = tracks
            .iter()
            .position(|track| track.canonical_track_id().map(|id| id.value()) == Some(owner))
        else {
            return false;
        };
        let slots = self.tail_mutable_instruments.len();
        for slot in 0..slots {
            let matching_owner = self
                .tail_mutable_instruments
                .get(slot)
                .and_then(Option::as_ref)
                .is_some_and(|tail| tail.track_id.value() == owner);
            if !matching_owner {
                continue;
            }
            let Some(previous) = self.tail_groups.get(slot).and_then(Option::as_ref) else {
                continue;
            };
            let Some(suffix_start) = self.tail_suffix_starts.get(slot).copied() else {
                continue;
            };
            let Some(candidate) = self.device_chains.get_mut(track_index) else {
                return false;
            };
            if candidate.transfer_processor_audition_from(
                previous,
                identity.owner,
                identity.device_id,
                identity.device_type,
                suffix_start,
                selection,
            ) {
                return true;
            }
        }
        false
    }

    pub(crate) fn processor_audition_is_active(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: hex_lattice::ProcessorAuditionSelection,
    ) -> bool {
        let project = self.revision.arrangement();
        match identity.owner {
            hex_lattice::ProcessorChainOwner::Source(_) => false,
            hex_lattice::ProcessorChainOwner::Main => self.main_chain.processor_audition_is_active(
                identity.owner,
                identity.device_id,
                identity.device_type,
                selection,
            ),
            hex_lattice::ProcessorChainOwner::Track(owner) => project
                .tracks()
                .iter()
                .position(|track| track.canonical_track_id().map(|id| id.value()) == Some(owner))
                .and_then(|index| self.device_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.processor_audition_is_active(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
            hex_lattice::ProcessorChainOwner::Return(owner) => project
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == owner)
                .and_then(|index| self.return_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.processor_audition_is_active(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
        }
    }

    pub(crate) fn cancel_processor_audition(&mut self, identity: crate::PostNodeIdentity) -> bool {
        let project = self.revision.arrangement();
        match identity.owner {
            hex_lattice::ProcessorChainOwner::Source(_) => false,
            hex_lattice::ProcessorChainOwner::Main => {
                self.main_chain.cancel_processor_audition_for_identity(
                    identity.owner,
                    identity.device_id,
                    identity.device_type,
                )
            }
            hex_lattice::ProcessorChainOwner::Track(owner) => project
                .tracks()
                .iter()
                .position(|track| track.canonical_track_id().map(|id| id.value()) == Some(owner))
                .and_then(|index| self.device_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.cancel_processor_audition_for_identity(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                    )
                }),
            hex_lattice::ProcessorChainOwner::Return(owner) => project
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == owner)
                .and_then(|index| self.return_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.cancel_processor_audition_for_identity(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                    )
                }),
        }
    }

    pub(crate) fn cancel_all_processor_auditions(&mut self) {
        for chain in &mut self.device_chains {
            chain.cancel_all_processor_auditions();
        }
        for chain in &mut self.return_chains {
            chain.cancel_all_processor_auditions();
        }
        for chain in self.tail_groups.iter_mut().flatten() {
            chain.cancel_all_processor_auditions();
        }
        self.main_chain.cancel_all_processor_auditions();
        self.return_effect_tails.cancel_all_processor_auditions();
    }

    pub(crate) fn cancel_retired_processor_auditions(&mut self) {
        for chain in self.tail_groups.iter_mut().flatten() {
            chain.cancel_all_processor_auditions();
        }
        self.return_effect_tails.cancel_all_processor_auditions();
    }

    /// Prepare the shared renderer for finite, linear Arrangement export.
    /// Session route actions are refused, Session-only tempo/loop inventory
    /// does not affect this owner, and the authored Arrangement tempo map is
    /// used without transport-loop wrapping. Mutable Arrangement MIDI source loops
    /// and seek destinations that require note chase remain unsupported.
    /// Graphs without compiled Arrangement instruments omit unused analytic
    /// release/suppression banks; their immutable routing cannot create voices.
    pub fn new_linear_arrangement_export(
        revision: Arc<PreparedRevision>,
    ) -> Result<Self, SessionPlaybackError> {
        Self::new_with_policy(revision, true, None)
    }

    /// Construct finite linear Arrangement export with the exact set used by the ordinary
    /// registered compiler. Each mutable runtime owns its scoped lease; no live monitor inputs
    /// are attached. The caller separately admits/pins source/configuration/factory custody.
    /// Runtime construction and actual retained runtime bytes share the existing export ceiling.
    /// This is a thin entry: the owning constructor performs both checks around its single
    /// construction, avoiding additional large debug Result temporaries for synchronous callers.
    pub fn new_linear_arrangement_export_with_device_resources(
        revision: Arc<PreparedRevision>,
        resources: &crate::native_device_resources::PreparedDeviceResources,
    ) -> Result<Self, SessionPlaybackError> {
        Self::new_with_policy(revision, true, Some(resources))
    }

    /// Prepare linear export with independently owned foreign processors.
    /// The caller must supply those processors for every render, including
    /// preroll and latency drain. The output owner bounds the export window;
    /// the shorter source-bounce frame limit does not apply here.
    pub fn new_linear_arrangement_export_with_foreign(
        revision: Arc<PreparedRevision>,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
    ) -> Result<Self, SessionPlaybackError> {
        if revision.requires_control_sampler_sources() {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler requires private offline export resources",
            ));
        }
        Self::new_with_render_policy(
            revision,
            RenderConstruction::LinearForeignExport,
            resources,
            None,
        )
    }

    /// Private-control export capability: the graph contains only scalar sampler
    /// coefficients. Every block must borrow this exact sealed source sidecar.
    /// This closed owner accepts only Play, Stop and frame-zero Seek, never
    /// interactive publication or Console routing. It therefore omits analytic
    /// release and occurrence-suppression banks; ordinary playback retains them.
    pub fn new_control_sampler_export(
        revision: Arc<PreparedRevision>,
        sources: &crate::native_device_resources::ControlSamplerSources,
    ) -> Result<Self, SessionPlaybackError> {
        if revision.resource_scope != Some(sources.id()) {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler export source scope differs",
            ));
        }
        let allowance = Self::control_sampler_export_runtime_allowance(&revision, sources)?;
        let playback = Self::new_with_policy_scoped(revision, true, None)?;
        if playback
            .accounted_runtime_storage_bytes()
            .is_none_or(|n| n > allowance)
        {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler export runtime exceeds its checked allowance",
            ));
        }
        Ok(playback)
    }
    pub fn control_sampler_export_runtime_allowance(
        revision: &PreparedRevision,
        sources: &crate::native_device_resources::ControlSamplerSources,
    ) -> Result<usize, SessionPlaybackError> {
        if revision.resource_scope != Some(sources.id()) {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler export source scope differs",
            ));
        }
        Self::linear_export_runtime_allowance(revision)
    }
    /// Base runtime/scratch allowance for interactive playback and ordinary
    /// preroll. Includes full release/handoff banks even when linear export of
    /// this revision can omit them. Input owners and CURRENT's keyboard pool
    /// add their separately checked storage to this base.
    pub fn interactive_runtime_allowance(
        revision: &PreparedRevision,
    ) -> Result<usize, SessionPlaybackError> {
        Self::runtime_allowance_for(revision, RenderConstruction::Interactive)
    }
    /// Runtime/scratch allowance only for a fresh linear or finite-bounce owner.
    /// It cannot fund interactive construction, Console routing or inheritance.
    pub fn linear_export_runtime_allowance(
        revision: &PreparedRevision,
    ) -> Result<usize, SessionPlaybackError> {
        Self::runtime_allowance_for(revision, RenderConstruction::LinearExport)
    }
    fn runtime_allowance_for(
        revision: &PreparedRevision,
        policy: RenderConstruction,
    ) -> Result<usize, SessionPlaybackError> {
        let bytes = match policy {
            RenderConstruction::Interactive => {
                preroll::arrangement_runtime_storage_preflight_bytes(revision)
            }
            RenderConstruction::BorrowedPauseSources => {
                preroll::paused_source_runtime_storage_preflight_bytes(revision)
            }
            RenderConstruction::LinearExport
            | RenderConstruction::LinearForeignExport
            | RenderConstruction::FiniteBounce
            | RenderConstruction::FiniteForeignBounce => {
                preroll::linear_export_runtime_storage_preflight_bytes(revision)
            }
        };
        bytes
            .filter(|n| *n <= MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
            .ok_or(SessionPlaybackError::MutableInstrumentDiagnostic(
                match policy {
                    RenderConstruction::Interactive => {
                        "interactive runtime exceeds its independent runtime limit"
                    }
                    RenderConstruction::BorrowedPauseSources => {
                        "paused source monitor exceeds its independent runtime limit"
                    }
                    _ => "sampler export runtime exceeds its independent runtime limit",
                },
            ))
    }
    /// Linear owners cannot admit Console commands or predecessor history.
    /// Private CURRENT contains only mutable samplers; an Arrangement without
    /// any compiled generator also cannot create analytic release obligations.
    /// Nonreceivers cannot generate source audio. An unbound authoring bank
    /// is harmless only when its compiled spans are empty.
    /// Keep quote and allocation on the same rule. Interactive and other MIDI
    /// render classes retain their full release and suppression banks.
    fn release_capacity_for(revision: &PreparedRevision, linear_export: bool) -> usize {
        if linear_export
            && (revision.requires_control_sampler_sources()
                || revision.arrangement().tracks().iter().all(|track| {
                    !track.receives_source_audio()
                        || (!track.has_mutable_instrument()
                            && track.instrument.as_ref().is_none_or(|bank| {
                                !track.instrument_slot_enabled() && bank.spans.is_empty()
                            }))
                }))
        {
            0
        } else {
            SESSION_RELEASE_CAPACITY
        }
    }
    fn new_with_policy(
        revision: Arc<PreparedRevision>,
        linear_arrangement_export: bool,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
    ) -> Result<Self, SessionPlaybackError> {
        if revision.requires_control_sampler_sources() {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler requires private offline export resources",
            ));
        }
        Self::new_with_policy_scoped(revision, linear_arrangement_export, resources)
    }
    /// Construct ordinary mixed playback with the exact resource set used by its compiler.
    /// Leases are allocated on control and then owned by each actual mutable runtime.
    pub fn new_with_device_resources(
        revision: Arc<PreparedRevision>,
        resources: &crate::native_device_resources::PreparedDeviceResources,
    ) -> Result<Self, SessionPlaybackError> {
        if revision.resource_scope.is_some() {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "private sampler scope is not an ordinary resource set",
            ));
        }
        Self::new_with_policy_scoped(revision, false, Some(resources))
    }

    /// Construct an unused interactive candidate with an existing launched loop.
    /// Control owns construction/admission; callback inheritance supplies its actual clock.
    pub(crate) fn new_section_candidate(
        revision: Arc<PreparedRevision>,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        region: hex_project::LoopRegion,
    ) -> Result<Self, SessionPlaybackError> {
        Self::section_runtime_allowance(&revision, region)?;
        Self::new_with_policy_and_loop(revision, false, Some(resources), Some(region))
    }

    fn new_with_policy_scoped(
        revision: Arc<PreparedRevision>,
        linear_arrangement_export: bool,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
    ) -> Result<Self, SessionPlaybackError> {
        Self::new_with_policy_scoped_and_loop(revision, linear_arrangement_export, resources, None)
    }

    fn new_with_policy_and_loop(
        revision: Arc<PreparedRevision>,
        linear_arrangement_export: bool,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        transient_loop: Option<hex_project::LoopRegion>,
    ) -> Result<Self, SessionPlaybackError> {
        if revision.requires_control_sampler_sources() {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler requires private offline export resources",
            ));
        }
        Self::new_with_policy_scoped_and_loop(
            revision,
            linear_arrangement_export,
            resources,
            transient_loop,
        )
    }

    /// Construct an offline finite source bounce. Source-loop occurrences use
    /// the same bounded query/render path as Arrangement playback. Transport
    /// wrapping and Session commands remain disabled, as in linear export.
    /// The exporter owns the declared output window and finite publication;
    /// this constructor admits its size before any runtime allocation.
    pub fn new_finite_arrangement_bounce(
        revision: Arc<PreparedRevision>,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        frames: u64,
    ) -> Result<Self, SessionPlaybackError> {
        Self::new_finite_bounce(
            revision,
            resources,
            frames,
            RenderConstruction::FiniteBounce,
        )
    }

    /// Finite bounce whose caller supplies independently prepared foreign
    /// processors for every render, including preroll and latency drain.
    /// The processors receive authored automation on the Arrangement clock.
    pub fn new_finite_arrangement_bounce_with_foreign(
        revision: Arc<PreparedRevision>,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        frames: u64,
    ) -> Result<Self, SessionPlaybackError> {
        Self::new_finite_bounce(
            revision,
            resources,
            frames,
            RenderConstruction::FiniteForeignBounce,
        )
    }

    fn new_finite_bounce(
        revision: Arc<PreparedRevision>,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        frames: u64,
        policy: RenderConstruction,
    ) -> Result<Self, SessionPlaybackError> {
        if frames == 0 || frames > MAX_FINITE_BOUNCE_FRAMES {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "source bounce exceeds its finite frame limit",
            ));
        }
        if revision.requires_control_sampler_sources() {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler requires private offline export resources",
            ));
        }
        Self::new_with_render_policy(revision, policy, resources, None)
    }

    fn new_with_policy_scoped_and_loop(
        revision: Arc<PreparedRevision>,
        linear_arrangement_export: bool,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        transient_loop: Option<hex_project::LoopRegion>,
    ) -> Result<Self, SessionPlaybackError> {
        Self::new_with_render_policy(
            revision,
            if linear_arrangement_export {
                RenderConstruction::LinearExport
            } else {
                RenderConstruction::Interactive
            },
            resources,
            transient_loop,
        )
    }

    fn new_with_render_policy(
        revision: Arc<PreparedRevision>,
        policy: RenderConstruction,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        transient_loop: Option<hex_project::LoopRegion>,
    ) -> Result<Self, SessionPlaybackError> {
        Self::new_with_render_policy_and_source_effects(
            revision,
            policy,
            resources,
            transient_loop,
            None,
            &[],
        )
    }

    #[cfg(test)]
    pub(crate) fn test_with_source_effects(
        revision: Arc<PreparedRevision>,
        recipes: &[crate::source_effect_stage::PreparedSourceEffects],
    ) -> Result<Self, SessionPlaybackError> {
        Self::new_with_render_policy_and_source_effects(
            revision,
            RenderConstruction::Interactive,
            None,
            None,
            Some(recipes),
            &[],
        )
    }

    fn new_with_render_policy_and_source_effects(
        revision: Arc<PreparedRevision>,
        policy: RenderConstruction,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
        transient_loop: Option<hex_project::LoopRegion>,
        recipe_override: Option<&[crate::source_effect_stage::PreparedSourceEffects]>,
        monitor_effects: &[PausedMonitorForeignBinding],
    ) -> Result<Self, SessionPlaybackError> {
        let arrangement = revision.arrangement();
        let recipes = recipe_override.unwrap_or(&arrangement.source_effects);
        let linear_arrangement_export = !matches!(
            policy,
            RenderConstruction::Interactive | RenderConstruction::BorrowedPauseSources
        );
        let lacks_foreign_renderer = linear_arrangement_export
            && !matches!(
                policy,
                RenderConstruction::FiniteForeignBounce | RenderConstruction::LinearForeignExport
            );
        let chain_has_foreign_automation = arrangement
            .tracks
            .iter()
            .flat_map(|track| track.device_chain.iter().chain(&track.foreign_instrument))
            .chain(
                arrangement
                    .return_buses
                    .iter()
                    .flat_map(|bus| bus.device_chain.iter()),
            )
            .chain(arrangement.main_device_chain.iter())
            .any(|device| device.automation.foreign().is_some());
        if lacks_foreign_renderer
            && (chain_has_foreign_automation || !revision.foreign_automation.is_empty())
        {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "offline vst3 automation requires a private bounce renderer",
            ));
        }
        if chain_has_foreign_automation
            && transient_loop.is_some_and(|region| Some(region) != arrangement.loop_region())
        {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "vst3 automation requires the prepared arrangement loop",
            ));
        }
        if transient_loop.is_some() && !revision.foreign_automation.admits_loop(transient_loop) {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "audition loop exceeds the prepared vst3 automation capacity",
            ));
        }
        let foreign_arrangement = if policy == RenderConstruction::BorrowedPauseSources {
            foreign_arrangement::ForeignArrangement::default()
        } else {
            foreign_arrangement::ForeignArrangement::prepare(&revision)?
        };
        if lacks_foreign_renderer && !foreign_arrangement.is_empty() {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "vst3 instruments require interactive arrangement playback",
            ));
        }
        if revision.device_resource_scope != resources.map(|resources| resources.id()) {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "device resource set differs from prepared revision",
            ));
        }
        // Linear export with scoped resources or source effects, and every finite bounce,
        // reserve before allocation and check actual runtime storage below.
        let linear_resource_allowance = if (linear_arrangement_export
            && (resources.is_some() || !recipes.is_empty()))
            || matches!(
                policy,
                RenderConstruction::FiniteBounce | RenderConstruction::FiniteForeignBounce
            ) {
            Some(Self::linear_export_runtime_allowance(&revision)?)
        } else {
            None
        };
        let source_effects =
            crate::source_effect_stage::SourceEffectStages::prepare(arrangement, recipes)
                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
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
        if !linear_arrangement_export
            && transient_loop.is_none()
            && let Some(error) = revision.session().periodic_tempo_error()
        {
            return Err(SessionPlaybackError::SessionTempoUnavailable(error));
        }
        let periodic_tempo = if let Some(region) = transient_loop {
            if linear_arrangement_export {
                return Err(SessionPlaybackError::InvalidCommand);
            }
            Some(
                PreparedPeriodicSessionTempo::prepare(revision.session().timing(), region)
                    .map_err(SessionPlaybackError::SessionTempoUnavailable)?,
            )
        } else if linear_arrangement_export {
            None
        } else {
            revision.session().periodic_tempo()
        };
        let midi_admission = if let Some(region) = transient_loop {
            let _ = Self::section_runtime_allowance(&revision, region)?;
            let base = preroll::arrangement_runtime_storage_preflight_bytes(&revision)
                .ok_or(SessionPlaybackError::Allocation)?;
            revision
                .session()
                .prepare_section_admission(
                    crate::MidiTiming::max_micro_bpm(&base_timing_for(
                        revision.session(),
                        periodic_tempo,
                    )),
                    MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES
                        .checked_sub(base)
                        .ok_or(SessionPlaybackError::Allocation)?,
                )
                .map_err(|error| SessionPlaybackError::MutableInstrumentDiagnostic(error.0))?
        } else {
            None
        };

        let mutable_arrangement_source_loop = matches!(
            policy,
            RenderConstruction::LinearExport | RenderConstruction::LinearForeignExport
        ) && arrangement_tracks
            .iter()
            .any(|track| track.has_mutable_looping_source());
        if mutable_arrangement_source_loop {
            return Err(SessionPlaybackError::MutableInstrumentLoopUnsupported);
        }
        let mut source_voice_envelopes = [0_u16; MAX_TRACKS];
        let source_routing = revision.source_routing();
        for (index, track) in arrangement_tracks.iter().enumerate() {
            if source_routing.source(index).is_none() {
                continue;
            }
            let id = track
                .canonical_track_id()
                .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
            let console_voices = midi_admission
                .as_ref()
                .map_or(0, |admission| admission.track_voice_envelope(id));
            let voices = source_routing
                .contributor_voice_envelope(index)
                .max(console_voices);
            source_voice_envelopes[index] = u16::try_from(voices)
                .map_err(|_| SessionPlaybackError::MutableInstrumentRuntime)?;
        }
        if !linear_arrangement_export
            && let Some(region) = transient_loop
                .or_else(|| revision.arrangement().loop_region())
                .filter(|region| region.enabled())
        {
            for (receiver_index, track) in arrangement_tracks.iter().enumerate() {
                let id = track
                    .canonical_track_id()
                    .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
                let Some(factory) = revision.session().mutable_instrument_factory(id) else {
                    continue;
                };
                if source_routing.is_shared(receiver_index) {
                    if source_routing.receiver_index(receiver_index) != Some(receiver_index) {
                        continue;
                    }
                    let contributor_indices = source_routing.contributors(receiver_index);
                    let mut contributors = Vec::new();
                    contributors
                        .try_reserve_exact(contributor_indices.len())
                        .map_err(|_| SessionPlaybackError::Allocation)?;
                    let mut contributor_voices = Vec::new();
                    contributor_voices
                        .try_reserve_exact(contributor_indices.len())
                        .map_err(|_| SessionPlaybackError::Allocation)?;
                    for &contributor in contributor_indices {
                        let index = usize::from(contributor);
                        let contributor_track = arrangement_tracks
                            .get(index)
                            .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
                        let voices = if contributor_track.source_input_enabled() {
                            contributors.push(contributor_track);
                            transport_loop_admission::admit_track(
                                contributor_track,
                                region,
                                factory.max_release_tail_frames(),
                                factory.max_events_per_block(),
                                factory.max_active_occurrences(),
                                false,
                            )?
                        } else {
                            0
                        };
                        contributor_voices.push((index, voices));
                    }
                    transport_loop_admission::admit_tracks(
                        &contributors,
                        region,
                        factory.max_release_tail_frames(),
                        factory.max_events_per_block(),
                        factory.max_active_occurrences(),
                    )?;
                    let mut reserved_voices = 0usize;
                    for (index, loop_voices) in contributor_voices {
                        let voices = usize::from(source_voice_envelopes[index]).max(loop_voices);
                        source_voice_envelopes[index] = u16::try_from(voices)
                            .map_err(|_| SessionPlaybackError::MutableInstrumentRuntime)?;
                        reserved_voices = reserved_voices
                            .checked_add(voices)
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    }
                    if reserved_voices > factory.max_active_occurrences() {
                        return Err(SessionPlaybackError::MutableInstrumentLoopDensity);
                    }
                } else if track.has_mutable_instrument() {
                    let loop_voices = transport_loop_admission::admit_track(
                        track,
                        region,
                        factory.max_release_tail_frames(),
                        factory.max_events_per_block(),
                        factory.max_active_occurrences(),
                        factory.steals_releasing_voices(),
                    )?;
                    source_voice_envelopes[receiver_index] = u16::try_from(
                        usize::from(source_voice_envelopes[receiver_index]).max(loop_voices),
                    )
                    .map_err(|_| SessionPlaybackError::MutableInstrumentRuntime)?;
                }
            }
        }

        let release_capacity = Self::release_capacity_for(&revision, linear_arrangement_export);
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
                current_incarnation: 0,
                current_first_frame: false,
                audio_release: None,
            });
        }
        let mut mutable_count = 0_usize;
        for track in arrangement_tracks {
            if revision
                .session()
                .mutable_instrument_factory(
                    track
                        .canonical_track_id()
                        .ok_or(SessionPlaybackError::RevisionTrackMismatch)?,
                )
                .is_some()
            {
                mutable_count = mutable_count
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
            }
        }
        crate::mutable_instrument::validate_prepared_graph_budget(
            revision.arrangement(),
            revision.session(),
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
                .filter(|_| policy != RenderConstruction::BorrowedPauseSources)
                .map(|factory| {
                    MutableInstrumentTrackRuntime::new(
                        factory,
                        revision.arrangement().sample_rate(),
                    )
                })
                .transpose()
                .map_err(SessionPlaybackError::from)?;
            let mut runtime = runtime;
            if let Some(runtime) = runtime.as_mut() {
                if let Some(modulation) = revision.session().instrument_modulation(id) {
                    runtime.install_modulation(modulation)?;
                }
                if track.source_id().is_some() && !revision.requires_control_sampler_sources() {
                    runtime.enable_source_live();
                }
                runtime.resources = resources
                    .map(|resources| resources.lease_on_control(id))
                    .transpose()
                    .map_err(|_| SessionPlaybackError::MutableInstrumentRuntime)?
                    .flatten();
                runtime.expression_identity = track.instrument_id;
            }
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
            let chain = if monitor_effects.is_empty() {
                track
                    .new_device_runtime()
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?
            } else {
                paused_track_monitor::build_monitor_chain(&track.device_chain, monitor_effects)?
            };
            device_chains.push(chain);
        }
        let return_buses = revision.arrangement().return_buses();
        let mut return_chains = Vec::new();
        return_chains
            .try_reserve_exact(return_buses.len())
            .map_err(|_| SessionPlaybackError::Allocation)?;
        let mut return_scratch = Vec::new();
        let mut meter_cells = Vec::new();
        let meter_count = arrangement_tracks
            .len()
            .checked_add(return_buses.len())
            .ok_or(SessionPlaybackError::Allocation)?;
        meter_cells
            .try_reserve_exact(meter_count)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        for track in arrangement_tracks {
            let id = track
                .canonical_track_id()
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            meter_cells.push(meter_capture::Cell::new(OutputCaptureId::Track(id)));
        }
        for bus in return_buses {
            meter_cells.push(meter_capture::Cell::new(OutputCaptureId::Return(bus.id)));
        }
        meter_capture::sort(&mut meter_cells);
        return_scratch
            .try_reserve_exact(return_buses.len())
            .map_err(|_| SessionPlaybackError::Allocation)?;
        for bus in return_buses {
            return_chains.push(paused_track_monitor::build_monitor_chain(
                &bus.device_chain,
                monitor_effects,
            )?);
            return_scratch.push([[0.0; 2]; 256]);
        }
        let main_chain = if monitor_effects.is_empty() {
            revision
                .arrangement()
                .new_main_device_runtime()
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?
        } else {
            paused_track_monitor::build_monitor_chain(
                &revision.arrangement().main_device_chain,
                monitor_effects,
            )?
        };
        let return_effect_tails = ReturnEffectTailBank::new()?;
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

        let analytic_continuations = reserve_analytic_continuations()?;
        let analytic_continuation_scratch = reserve_analytic_continuations()?;

        let mut suppressions = Vec::new();
        suppressions
            .try_reserve_exact(release_capacity)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        let mut suppression_scratch = Vec::new();
        suppression_scratch
            .try_reserve_exact(release_capacity)
            .map_err(|_| SessionPlaybackError::Allocation)?;

        let compensation = revision
            .arrangement()
            .signal_graph
            .as_ref()
            .filter(|graph| graph.needs_runtime())
            .map(|graph| crate::signal_graph::SignalRuntime::new(Arc::clone(graph)))
            .transpose()
            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
        if compensation.is_none() && !foreign_arrangement.is_empty() {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        // Resident Arrangement and eligible Session audio share this window.
        // Generic streamed providers retain their original per-frame lookups.
        let mut resident_source_scratch = Vec::new();
        if compensation
            .as_ref()
            .is_some_and(|runtime| runtime.graph.needs_upstream_frame_routing())
        {
            resident_source_scratch
                .try_reserve_exact(arrangement_tracks.len())
                .map_err(|_| SessionPlaybackError::Allocation)?;
            resident_source_scratch.resize(arrangement_tracks.len(), [[0.0; 2]; 256]);
        }
        let scalar_automation = revision
            .scalar_automation
            .as_deref()
            .map(crate::scalar_automation::StateSet::new)
            .transpose()
            .map_err(|_| SessionPlaybackError::Allocation)?
            .map(Box::new);
        let analytic_import = analytic_import::ImportPlan::new()?;
        let current_sampler_keyboards = Vec::new();
        let current_route_cut_scratch = Vec::new();
        let native_parameter_state = (revision.native_parameter_input_target().is_some()
            || scalar_automation.is_some())
        .then(|| {
            Box::new(native_parameter_input::State {
                scalar: scalar_automation,
                ..Default::default()
            })
        });
        if let Some(allowance) = linear_resource_allowance {
            let actual = preroll::RuntimeStorage {
                live_expression: false,
                computer_keyboard: None,
                native_parameter_state: native_parameter_state.is_some(),
                scalar_automation: native_parameter_state
                    .as_deref()
                    .is_some_and(|state| state.scalar.is_some()),
                compensation: compensation.as_ref(),
                main_chain: &main_chain,
                current_sampler_keyboards: &current_sampler_keyboards,
                revision: &revision,
                midi_admission: midi_admission.as_ref(),
                analytic_import: &analytic_import,
                source_effects: &source_effects,
                device_chains: &device_chains,
                return_chains: &return_chains,
                return_effect_tails: &return_effect_tails,
                mutable_instruments: &mutable_instruments,
                foreign_arrangement: &foreign_arrangement,
                tail_groups: &tail_groups,
                tail_mutable_instruments: &tail_mutable_instruments,
                routes: routes.capacity(),
                route_scratch: route_scratch.capacity(),
                current_route_cut_scratch: current_route_cut_scratch.capacity(),
                return_scratch: return_scratch.capacity(),
                resident_source_scratch: resident_source_scratch.capacity(),
                meter_cells: meter_cells.capacity(),
                mutable_events: mutable_events.capacity(),
                tail_mixes: tail_mixes.capacity(),
                tail_suffix_starts: tail_suffix_starts.capacity(),
                tail_suffix_matrices: tail_suffix_matrices.capacity(),
                releases: releases.capacity(),
                release_scratch: release_scratch.capacity(),
                retained_contexts: retained_contexts.capacity(),
                context_lease_plan: context_lease_plan.capacity(),
                analytic_continuations: analytic_continuations.capacity(),
                analytic_continuation_scratch: analytic_continuation_scratch.capacity(),
                suppressions: suppressions.capacity(),
                suppression_scratch: suppression_scratch.capacity(),
            }
            .bytes();
            if actual.is_none_or(|bytes| bytes > allowance) {
                return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                    "device resource export runtime exceeds its checked allowance",
                ));
            }
        }
        let stream_miss_fades = crate::StreamMissFades::new(revision.arrangement().sample_rate());
        let mut track_faders = [track_fader::TrackFader::new([1.0; 2]); MAX_TRACKS];
        for (fader, track) in track_faders.iter_mut().zip(arrangement_tracks) {
            *fader = track_fader::TrackFader::new(track.mix_at(0).channel_gain());
        }
        Ok(Self {
            live_expression: None,
            computer_keyboard: None,
            paused_source_monitor: None,
            defer_paused_source_monitor: false,
            current_sampler_keyboards,
            current_sampler_keyboard_epoch: 0,
            current_sampler_keyboard_revoked: 0,
            capture_runtime: capture::next_runtime()?,
            capture_epoch: 0,
            gain_recording_override: None,
            live_gain: gain_recording::LiveGain::default(),
            live_gain_scratch: gain_recording::LiveGain::default(),
            native_parameter_state,
            compensation,
            device_metering: false,
            render_work: Default::default(),
            revision,
            transient_loop,
            browser_audition: Default::default(),
            midi_admission,
            periodic_tempo,
            anchor: None,
            interactive_start_frame: None,
            pending_shared_chase: None,
            pending_shared_live: None,
            historical_floor: None,
            route_historical_floors: [None; MAX_TRACKS],
            source_voice_envelopes,
            stream_miss_fades,
            routes,
            route_scratch,
            track_faders,
            track_fader_scratch: track_faders,
            source_effects,
            device_chains,
            return_chains,
            main_chain,
            return_scratch,
            resident_source_scratch,
            meter_cells,
            return_effect_tails,
            mutable_instruments,
            foreign_arrangement,
            mutable_onsets: MutableOnsetBuffer::default(),
            mutable_events,
            current_route_cut_scratch,
            tail_groups,
            tail_track_ids: [None; SESSION_TAIL_GROUP_CAPACITY],
            tail_route_sends: [false; SESSION_TAIL_GROUP_CAPACITY],
            has_tail_route_sends: false,
            tail_proven_grouped: [false; SESSION_TAIL_GROUP_CAPACITY],
            tail_mutable_instruments,
            tail_mixes,
            tail_suffix_starts,
            tail_suffix_matrices,
            tail_output_active: [false; SESSION_TAIL_GROUP_CAPACITY],
            tail_output_scratch: [false; SESSION_TAIL_GROUP_CAPACITY],
            detached_tail_group_limit: 0,
            releases,
            release_scratch,
            retained_contexts,
            context_lease_plan,
            analytic_continuations,
            analytic_continuation_scratch,
            release_capacity,
            release_order: 0,
            release_order_scratch: 0,
            suppressions,
            suppression_scratch,
            spans: Box::new(MidiSpanBuffer::default()),
            analytic_import,
            performance_frame: 0,
            arrangement_frame: 0,
            arrangement_loop_pass: 0,
            playing: false,
            monitor_source_releases: false,
            stopped_input: None,
            linear_arrangement_export,
            current_sampler_console: false,
            route_state_generation: 0,
            candidate_admission_open: true,
            require_continuous_insert: false,
            foreign_continuation: None,
            live_restart: false,
        })
    }

    /// Structural work admitted for the actual runtime's Session MIDI templates.
    /// This is a guard envelope, not a measured physical CPU budget.
    pub fn session_work_envelope(&self) -> crate::SessionWorkEnvelope {
        crate::session_inventory::EffectiveSessionMidi::new(
            self.revision.session(),
            self.midi_admission.as_ref(),
        )
        .work()
    }

    pub(crate) fn periodic_tempo(&self) -> Option<PreparedPeriodicSessionTempo> {
        self.periodic_tempo_at(self.arrangement_frame)
    }

    fn periodic_tempo_at(&self, frame: u64) -> Option<PreparedPeriodicSessionTempo> {
        self.active_loop_region_at(frame).and(self.periodic_tempo)
    }

    pub fn performance_frame(&self) -> u64 {
        self.performance_frame
    }

    /// Configured loop law for capability/admission checks. Rendering and demand
    /// use `active_loop_region` to respect absolute cursors after the loop.
    /// A transient section loop never changes the immutable saved revision.
    pub(crate) fn loop_region(&self) -> Option<hex_project::LoopRegion> {
        if self.linear_arrangement_export {
            None
        } else {
            self.transient_loop
                .or_else(|| self.revision.arrangement().loop_region())
                .filter(|region| region.enabled())
        }
    }

    /// The saved/audition loop participates only while the cursor is before
    /// its end, on every pass. An absolute seek at or after the end begins a
    /// linear pass; a cursor before the start plays into the loop and wraps.
    /// A loop edited behind a cursor that already wrapped likewise lets
    /// playback continue linearly instead of jumping into the new region.
    /// Arrangement audio fades out immediately before a wrap, while the loop
    /// start keeps its authored level. Clip fades and transport action fades
    /// retain their independent contracts.
    /// This query does not change loop settings or introduce transport state.
    pub(crate) fn active_loop_region(&self) -> Option<hex_project::LoopRegion> {
        self.active_loop_region_at(self.arrangement_frame)
    }

    fn active_loop_region_at(&self, frame: u64) -> Option<hex_project::LoopRegion> {
        self.loop_region().filter(|region| frame < region.end())
    }

    /// Exact immutable graph retained by this runtime.
    pub fn prepared_revision(&self) -> &Arc<PreparedRevision> {
        &self.revision
    }

    pub(crate) const fn is_linear_arrangement_export(&self) -> bool {
        self.linear_arrangement_export
    }

    pub fn arrangement_frame(&self) -> u64 {
        self.arrangement_frame
    }

    /// Current pass and active loop law at the Arrangement cursor. The pass
    /// distinguishes loop wraps but can reset on a seek; this is not a fence.
    pub fn arrangement_loop_state(&self) -> ArrangementLoopState {
        ArrangementLoopState {
            pass: self.arrangement_loop_pass,
            active_region: self.active_loop_region().map(|r| (r.start(), r.end())),
        }
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Freeze or restore the clock at a silent publication boundary. Preserve
    /// route, voice and processor history for handoff or resumption.
    pub(crate) fn set_playing_at_device_insert_cut(&mut self, playing: bool) {
        self.playing = playing;
    }

    /// Scalar translation used by the effective performance clock. Observers
    /// must correlate it with this runtime's applied revision and route snapshot.
    pub fn clock_anchor(&self) -> Option<SessionClockAnchor> {
        self.anchor
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
        let (owner, active_origin, audio_launch_frame) = match route.owner {
            RouteOwner::Arrangement => (SessionPlaybackOwner::Arrangement, None, None),
            RouteOwner::Session { clip, origin } => {
                (SessionPlaybackOwner::Session { clip }, Some(origin), None)
            }
            RouteOwner::SessionAudio { clip, launch_frame } => (
                SessionPlaybackOwner::Session { clip },
                None,
                Some(launch_frame),
            ),
            RouteOwner::Silence => (SessionPlaybackOwner::Silence, None, None),
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
            active_origin,
            audio_launch_frame,
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
        self.validate_legacy_transport(arrangement_frame, performance_frame)?;
        self.candidate_admission_open = false;
        self.arrangement_frame = arrangement_frame;
        self.performance_frame = performance_frame;
        self.playing = playing;
        Ok(())
    }

    pub(crate) fn validate_legacy_transport(
        &self,
        arrangement_frame: u64,
        performance_frame: u64,
    ) -> Result<(), SessionPlaybackError> {
        use crate::MidiTiming;
        // Legacy finite playback accepts frame positions beyond the Session
        // beat domain. Refuse promotion before modifying the candidate rather
        // than accepting a graph that will fail at its first render/launch.
        base_timing_for(
            self.revision.session(),
            self.periodic_tempo_at(arrangement_frame),
        )
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
                    .any(crate::Track::has_device_chain))
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

    /// Offline bounce completion after the isolated source's final gate. Checks
    /// actual analytic releases and mutable processor state, never output silence.
    /// The caller must exclude future source events. This does not inspect
    /// compensation delay buffers: after an idle observation, callers must
    /// render at least the full output-path latency and recheck before stopping.
    pub fn arrangement_bounce_is_quiescent(&self) -> bool {
        !self.has_active_arrangement_voices()
            && !self.foreign_arrangement.has_held_notes()
            && self.releases.is_empty()
            && self
                .mutable_instruments
                .iter()
                .flatten()
                .all(|r| r.committed_is_quiescent())
            && self
                .device_chains
                .iter()
                .chain(self.return_chains.iter())
                .chain(std::iter::once(&self.main_chain))
                .all(|c| c.committed_range_is_quiescent(0..c.len()))
            && self.tail_groups.iter().all(Option::is_none)
            && self.tail_mutable_instruments.iter().all(Option::is_none)
    }

    /// Release an input scheduler only after its control-side replacement has
    /// passed storage admission. Every matching source/tail keeps DSP custody.
    pub(crate) fn release_source_input_on_control(
        &mut self,
        input: crate::mutable_instrument::LiveInputId,
    ) {
        for runtime in self.mutable_instruments.iter_mut().flatten().chain(
            self.tail_mutable_instruments
                .iter_mut()
                .flatten()
                .map(|tail| &mut tail.runtime),
        ) {
            runtime.request_live_input_release(input);
        }
    }

    /// Borrow the existing source runtime during control-side input preparation.
    /// This grants no DSP/resource custody and constructs no additional instance.
    pub(crate) fn source_runtime_on_control(
        &self,
        receiver: usize,
    ) -> Option<&MutableInstrumentTrackRuntime> {
        self.mutable_instruments
            .get(receiver)
            .and_then(Option::as_ref)
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
        if self.midi_capture_active()
            || previous.midi_capture_active()
            || self.linear_arrangement_export
            || previous.linear_arrangement_export
            || self.current_sampler_console
            || previous.current_sampler_console
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.require_fresh_candidate()?;
        self.inherit_state(previous)
    }

    /// The live publication handoff: as `try_inherit_state_from`, except an
    /// unproven mutable chase restarts audio at the cursor instead of
    /// refusing (Pulse #3770). An ordinary reset refuses while an active
    /// detached Return tail cannot be preserved. Allocation-free; nothing is
    /// committed before the strict planner's refusal.
    pub fn try_inherit_state_live_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.try_inherit_live_from(previous, false)
    }

    /// Rebuild at an output-owned silent cut without reanchoring either
    /// transport clock or already-playing Session clip origins.
    pub(crate) fn try_inherit_state_at_device_insert_cut(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        self.try_inherit_live_from(previous, true)
    }

    fn try_inherit_live_from(
        &mut self,
        previous: &mut Self,
        confirmed_insert: bool,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        if self.midi_capture_active()
            || previous.midi_capture_active()
            || self.linear_arrangement_export
            || previous.linear_arrangement_export
            || self.current_sampler_console
            || previous.current_sampler_console
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.require_fresh_candidate()?;
        if confirmed_insert {
            self.inherit_state_at_device_insert_cut(previous, None)
        } else {
            self.inherit_state_live(previous, None)
        }
    }

    /// The caller must authenticate and retain the source and state identities.
    pub(crate) fn try_inherit_state_live_with_foreign_continuation_from(
        &mut self,
        previous: &mut Self,
        continuation: &hex_lattice::ForeignConfigurationContinuation<'_>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        if self.midi_capture_active()
            || previous.midi_capture_active()
            || self.linear_arrangement_export
            || previous.linear_arrangement_export
            || self.current_sampler_console
            || previous.current_sampler_console
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.require_fresh_candidate()?;
        self.inherit_state_planned(previous, None, None, true, false, Some(continuation))
    }

    /// Reset a prepared bounce successor at an authenticated Arrangement fence.
    /// The audio publication owner must validate the live fence before calling;
    /// this does not grant permission to use an unfenced live handoff.
    pub(crate) fn try_inherit_state_fenced_restart_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        if !self.live_restart
            || self.revision.requires_control_sampler_sources()
            || self.midi_capture_active()
            || previous.midi_capture_active()
            || self.linear_arrangement_export
            || previous.linear_arrangement_export
            || self.current_sampler_console
            || previous.current_sampler_console
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.require_fresh_candidate()?;
        self.inherit_state_fenced_restart(previous)
    }

    /// Make a later live handoff restart at the cursor (voices and tails
    /// reset, gate-active notes chased) without first trying to carry state.
    /// In-place bounces need this: their baked PCM already contains the old
    /// instrument and effect output, so detached tails would sound twice.
    pub fn request_live_restart(&mut self) {
        self.live_restart = true;
    }
    pub fn live_restart_requested(&self) -> bool {
        self.live_restart
    }

    pub(crate) fn inherit_candidate_groups(
        &mut self,
        previous: &mut Self,
        groups: crate::audio_candidate::groups::Transfer<'_>,
    ) -> Result<SessionHandoffReport, SessionPlaybackError> {
        if self.midi_capture_active()
            || previous.midi_capture_active()
            || self.linear_arrangement_export
            || previous.linear_arrangement_export
            || self.current_sampler_console
            || previous.current_sampler_console
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.require_fresh_candidate()?;
        self.inherit_state_live(previous, Some(groups))
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
        timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor)
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
            RouteTarget::Session(clip)
                if crate::session_inventory::EffectiveSessionMidi::new(
                    self.revision.session(),
                    self.midi_admission.as_ref(),
                )
                .midi_clip(clip)
                .is_some() =>
            {
                RouteOwner::Session {
                    clip,
                    origin: SessionMidiOrigin::launch(beat),
                }
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

mod audio_release;
mod capture;
mod meter_capture;
mod release_location;
use release_location::ReleaseLocation;
/// These tables never grow, shrink, swap ownership or allocate in handoff or
/// rendering. Refusal/destruction of their storage belongs to control.
fn reserve_analytic_continuations()
-> Result<Vec<Option<hex_lattice::AnalyticContinuation>>, SessionPlaybackError> {
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(MAX_TRACKS)
        .map_err(|_| SessionPlaybackError::Allocation)?;
    if slots.capacity() != MAX_TRACKS {
        return Err(SessionPlaybackError::Allocation);
    }
    slots.resize(MAX_TRACKS, None);
    Ok(slots)
}

mod analytic_import;
mod clip_gain;
mod commands;
mod current_sampler_routes;
mod handoff;
pub use handoff::PreparedTempoAnchor;
mod harmonic_import;
mod paused_track_monitor;
use paused_track_monitor::StoppedInputHistory;
pub use paused_track_monitor::{
    PausedKeyboardMonitor, PausedMonitorAdmissionError, PausedMonitorForeignBinding,
    PausedMonitorStorageHold, PausedSourceMonitor, PausedTrackMonitor,
};
mod preroll;
mod render;
mod stream_actions;
mod stream_demand;
pub use capture::{
    MAX_OUTPUT_CAPTURE_FRAMES, MAX_OUTPUT_CAPTURES, OutputCapture, OutputCaptureError,
    OutputCaptureId, OutputCapturePlan, ProjectOutputTap, ProjectSignal, ProjectSignalObserver,
};
pub use stream_actions::PreparedStreamAction;
mod return_effect_tails;
pub use preroll::{
    ArrangementPrerollError, MAX_ARRANGEMENT_PREROLL_FRAMES, MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
    PreparedPrerollPages,
};
use return_effect_tails::{ReturnEffectTailBank, ReturnEffectTailPlan};
#[cfg(test)]
mod harmonic_accounting;
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

#[cfg(test)]
mod render_diagnostic_tests {
    use super::*;
    #[test]
    fn callback_messages_retain_nested_causes_instead_of_a_generic_render_failure() {
        assert_eq!(
            SessionPlaybackError::Midi(crate::MidiTemplateError::TooManyEdges).diagnostic_message(),
            "ongoing midi query exceeds 256 edges"
        );
        assert_eq!(
            SessionPlaybackError::Arrangement("missing source page").diagnostic_message(),
            "missing source page"
        );
        assert_eq!(
            SessionPlaybackError::SessionTempoUnavailable(crate::PreparedTempoError::BeatToFrame(
                "beat overflow"
            ))
            .diagnostic_message(),
            "beat overflow"
        );
        use crate::mutable_instrument::MutableInstrumentRuntimeError;
        assert_eq!(
            MutableInstrumentRuntimeError::Runtime(
                hex_lattice::NativeInstrumentRuntimeError::InvalidEventBatch
            )
            .render_error()
            .diagnostic_message(),
            "native instrument event batch is invalid"
        );
        assert_eq!(
            MutableInstrumentRuntimeError::OrphanedGate
                .render_error()
                .diagnostic_message(),
            "mutable instrument note-off has no occurrence"
        );
        assert_eq!(
            MutableInstrumentRuntimeError::EventCapacity
                .render_error()
                .diagnostic_message(),
            "mutable instrument event query capacity exceeded"
        );
        // Render diagnostics do not change the admission/retry classification.
        assert_eq!(
            SessionPlaybackError::from(MutableInstrumentRuntimeError::OrphanedGate),
            SessionPlaybackError::MutableInstrumentEvents
        );
        assert_eq!(
            SessionPlaybackError::ReleaseBankFull.diagnostic_message(),
            "session playback outgoing release bank is full"
        );
    }
}
mod gain_recording;
mod native_parameter_input;
#[cfg(test)]
mod return_feed_tests;

#[cfg(test)]
mod compensation_tests;
#[cfg(test)]
mod foreign_tests;
#[cfg(test)]
mod live_track_audio_tests;

#[cfg(test)]
mod routing_tests;

#[cfg(test)]
mod live_expression_tests;
