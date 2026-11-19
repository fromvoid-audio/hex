//! Audio-owned playback of an immutable prepared project revision.
//! Construction and destruction belong outside the callback.
use crate::{
    Frame, Invalid, MediaPages, PreparedRevision, Project, RendererRuntime, SessionPlayback,
};
use std::sync::Arc;

mod candidate_storage;
#[cfg(test)]
mod idle_transition_tests;
#[cfg(test)]
mod live_track_audio_tests;
mod metronome;
pub(crate) mod transport_fade;
pub use transport_fade::TransportFadeCompletion;
pub(crate) use transport_fade::fade_frames;
mod timed_transport;
mod tuning_audition;
use metronome::Metronome;
pub use timed_transport::{
    MAX_TIMED_TRANSPORT_COMMANDS, TimedTransportCommand, TimedTransportError,
};

/// Why a bounded current-stream projection could not be completed.
/// Visitor refusal may leave partial caller-owned observations speculative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamQuantumProjectionError {
    Unsupported,
    ClockOverflow,
    Visitor(Invalid),
    Projection(Invalid),
}

impl From<StreamQuantumProjectionError> for Invalid {
    fn from(error: StreamQuantumProjectionError) -> Self {
        match error {
            StreamQuantumProjectionError::Unsupported => {
                Self("stream demand requires a closed audio runtime")
            }
            StreamQuantumProjectionError::ClockOverflow => {
                Self("stream callback projection overflow")
            }
            StreamQuantumProjectionError::Visitor(error)
            | StreamQuantumProjectionError::Projection(error) => error,
        }
    }
}
impl std::fmt::Display for StreamQuantumProjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&Invalid::from(self.clone()), f)
    }
}
impl std::error::Error for StreamQuantumProjectionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackCommand {
    Play,
    /// Start at this absolute frame with fresh effect/voice history. Mutable
    /// Arrangement notes crossing the cursor retrigger with their remaining
    /// authored gate; analytic notes retain their direct authored-age law.
    /// This interactive action never replays audio before the cursor.
    /// A cursor at/after the enabled loop end continues linearly beyond it;
    /// a cursor before the end enters/wraps the loop without changing its bounds.
    PlayFrom(u64),
    Pause,
    Stop,
    /// Absolute content cursor; loop participation resumes only before its end.
    Seek(u64),
}

/// Browser Enter freezes the grid and the running output's transport epoch.
/// The callback may install the authored edit after a seek or stop, but the
/// older press has no authority to audition in the new transport.
#[derive(Clone, Debug)]
pub struct BrowserAuditionTiming {
    pub grid: hex_project::ResolvedInputGrid,
    pub transport_epoch: u64,
}

/// Fixed-size action passed through the native callback command ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackAction {
    /// Transient interactive click; never part of the saved project or export.
    SetMetronome(bool),
    Transport(PlaybackCommand),
    Session(crate::SessionPlaybackCommand),
    /// Apply a valid clip or scene launch and start transport as one action.
    LaunchAndPlay(crate::SessionPlaybackCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCapability {
    Legacy,
    Available,
    Unavailable(crate::SessionPlaybackError),
}

/// Stable compact status published after a graph is accepted by the callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SessionCapabilityStatus {
    Legacy = 0,
    Available = 1,
    TimingUnavailable = 2,
    AllocationFailed = 3,
    RevisionMismatch = 4,
    RuntimeUnavailable = 5,
}

impl SessionCapabilityStatus {
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Legacy),
            1 => Some(Self::Available),
            2 => Some(Self::TimingUnavailable),
            3 => Some(Self::AllocationFailed),
            4 => Some(Self::RevisionMismatch),
            5 => Some(Self::RuntimeUnavailable),
            _ => None,
        }
    }

    pub const fn message(self) -> Option<&'static str> {
        match self {
            Self::Legacy | Self::Available => None,
            Self::TimingUnavailable => Some("session playback timing is unavailable"),
            Self::AllocationFailed => Some("session playback runtime storage is unavailable"),
            Self::RevisionMismatch => Some("session playback revision does not match arrangement"),
            Self::RuntimeUnavailable => Some("session playback runtime is unavailable"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackActionError {
    MetronomeUnavailable,
    SessionUnavailable(crate::SessionPlaybackError),
    SessionNotPrepared,
    Session(crate::SessionPlaybackError),
}

/// Truthful compact reason stored by the audio callback after it rejects an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PlaybackActionRejection {
    SessionUnavailable = 1,
    SessionNotPrepared = 2,
    UnknownTrack = 3,
    UnknownScene = 4,
    UnknownClip = 5,
    ClipTrackMismatch = 6,
    ReleaseCapacity = 7,
    Timing = 8,
    InvalidSessionAction = 9,
    MetronomeUnavailable = 10,
    StreamActionBusy = 11,
    StreamPagesNotReady = 12,
    StreamActionCancelled = 13,
    SamplerEditBusy = 14,
    StreamSourceChanged = 15,
    OutputFailed = 16,
    NoActivePlayback = 17,
    ProjectChanged = 18,
    AudioSourceChanged = 19,
    SharedSourceVoiceCapacity = 20,
}

impl PlaybackActionRejection {
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::SessionUnavailable),
            2 => Some(Self::SessionNotPrepared),
            3 => Some(Self::UnknownTrack),
            4 => Some(Self::UnknownScene),
            5 => Some(Self::UnknownClip),
            6 => Some(Self::ClipTrackMismatch),
            7 => Some(Self::ReleaseCapacity),
            8 => Some(Self::Timing),
            9 => Some(Self::InvalidSessionAction),
            10 => Some(Self::MetronomeUnavailable),
            11 => Some(Self::StreamActionBusy),
            12 => Some(Self::StreamPagesNotReady),
            13 => Some(Self::StreamActionCancelled),
            14 => Some(Self::SamplerEditBusy),
            15 => Some(Self::StreamSourceChanged),
            16 => Some(Self::OutputFailed),
            17 => Some(Self::NoActivePlayback),
            18 => Some(Self::ProjectChanged),
            19 => Some(Self::AudioSourceChanged),
            20 => Some(Self::SharedSourceVoiceCapacity),
            _ => None,
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::NoActivePlayback => "no active playback graph",
            Self::ProjectChanged => "project changed · playback action cancelled",
            Self::AudioSourceChanged => "audio source changed · playback action cancelled",
            Self::SharedSourceVoiceCapacity => {
                "shared source voice capacity is full · release notes or wait for tails, then retry"
            }
            Self::MetronomeUnavailable => {
                "metronome requires prepared musical timing and sample storage"
            }
            Self::SessionUnavailable => "session playback is unavailable for this revision",
            Self::SessionNotPrepared => "session playback was not prepared for this graph",
            Self::UnknownTrack => "session track is no longer available",
            Self::UnknownScene => "session scene is no longer available",
            Self::UnknownClip => "session clip is no longer available",
            Self::ClipTrackMismatch => "session clip belongs to another track",
            Self::ReleaseCapacity => {
                "session transition was refused because the release bank is full"
            }
            Self::Timing => "session playback timing conversion failed",
            Self::InvalidSessionAction => "session playback action was rejected",
            Self::StreamActionBusy => "stream action waits for an earlier command or publication",
            Self::StreamPagesNotReady => "stream action page protection is unavailable",
            Self::StreamSourceChanged => "original media changed · reopen or relink it",
            Self::OutputFailed => "audio output failed · restart audio before retrying",
            Self::SamplerEditBusy => "sampler edit is preparing; retry after audio is ready",
            Self::StreamActionCancelled => {
                "stream action was cancelled by transport or replacement"
            }
        }
    }
}

impl PlaybackActionError {
    /// Compact stable category for callback-to-control telemetry.
    pub const fn telemetry_rejection(self) -> PlaybackActionRejection {
        match self {
            Self::MetronomeUnavailable => PlaybackActionRejection::MetronomeUnavailable,
            Self::SessionUnavailable(_) => PlaybackActionRejection::SessionUnavailable,
            Self::SessionNotPrepared => PlaybackActionRejection::SessionNotPrepared,
            Self::Session(crate::SessionPlaybackError::UnknownTrack) => {
                PlaybackActionRejection::UnknownTrack
            }
            Self::Session(crate::SessionPlaybackError::UnknownScene) => {
                PlaybackActionRejection::UnknownScene
            }
            Self::Session(crate::SessionPlaybackError::UnknownClip) => {
                PlaybackActionRejection::UnknownClip
            }
            Self::Session(crate::SessionPlaybackError::ClipTrackMismatch) => {
                PlaybackActionRejection::ClipTrackMismatch
            }
            Self::Session(crate::SessionPlaybackError::ReleaseBankFull) => {
                PlaybackActionRejection::ReleaseCapacity
            }
            Self::Session(crate::SessionPlaybackError::SharedSourceVoiceCapacity) => {
                PlaybackActionRejection::SharedSourceVoiceCapacity
            }
            Self::Session(crate::SessionPlaybackError::TempoConversion)
            | Self::Session(crate::SessionPlaybackError::SessionTempoUnavailable(_)) => {
                PlaybackActionRejection::Timing
            }
            Self::Session(crate::SessionPlaybackError::CandidateAlreadyUsed) => {
                PlaybackActionRejection::InvalidSessionAction
            }
            Self::Session(_) => PlaybackActionRejection::InvalidSessionAction,
        }
    }

    pub fn message(self) -> &'static str {
        self.telemetry_rejection().message()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackHandoffError {
    Session(crate::SessionPlaybackError),
    ActiveSessionStateCannotBeDropped,
    ArrangementStateCannotBeCaptured,
    ActiveArrangementStateCannotBeDropped,
    PerformanceClockExhausted,
    RuntimeFenceChanged,
    FreshRuntimeCancelled,
}

/// Compact truthful category for asynchronous callback handoff refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PlaybackHandoffRejection {
    SessionRuntime = 1,
    ActiveSessionStateCannotBeDropped = 2,
    PerformanceClockExhausted = 3,
    ArrangementStateCannotBeCaptured = 4,
    ActiveArrangementStateCannotBeDropped = 5,
    RuntimeFenceChanged = 6,
    FreshRuntimeCancelled = 7,
    ConstructorClaimsUnavailable = 8,
    StreamedPagesNotReady = 9,
    LatencyLayoutChanged = 10,
    SourceHistoryUnavailable = 11,
    StreamSourceChanged = 12,
    SharedSourceVoiceCapacity = 13,
    StreamedPageCapacity = 14,
    ForeignTailSourcePaused = 15,
    DeviceInsertNeedsConfirmation = 16,
    TempoCutMissed = 17,
}

impl PlaybackHandoffRejection {
    pub const fn from_error(error: PlaybackHandoffError) -> Self {
        match error {
            PlaybackHandoffError::Session(crate::SessionPlaybackError::LatencyLayoutChanged) => {
                Self::LatencyLayoutChanged
            }
            PlaybackHandoffError::Session(
                crate::SessionPlaybackError::DeviceInsertNeedsConfirmation,
            ) => Self::DeviceInsertNeedsConfirmation,
            PlaybackHandoffError::Session(
                crate::SessionPlaybackError::SharedSourceVoiceCapacity,
            ) => Self::SharedSourceVoiceCapacity,
            PlaybackHandoffError::Session(
                crate::SessionPlaybackError::SourceHistoryUnavailable
                | crate::SessionPlaybackError::DetachedReceiverHistoryUnavailable,
            ) => Self::SourceHistoryUnavailable,
            PlaybackHandoffError::Session(crate::SessionPlaybackError::CandidateAlreadyUsed) => {
                Self::SessionRuntime
            }
            PlaybackHandoffError::Session(crate::SessionPlaybackError::ForeignTailSourcePaused) => {
                Self::ForeignTailSourcePaused
            }
            PlaybackHandoffError::Session(_) => Self::SessionRuntime,
            PlaybackHandoffError::ActiveSessionStateCannotBeDropped => {
                Self::ActiveSessionStateCannotBeDropped
            }
            PlaybackHandoffError::PerformanceClockExhausted => Self::PerformanceClockExhausted,
            PlaybackHandoffError::RuntimeFenceChanged => Self::RuntimeFenceChanged,
            PlaybackHandoffError::FreshRuntimeCancelled => Self::FreshRuntimeCancelled,
            PlaybackHandoffError::ArrangementStateCannotBeCaptured => {
                Self::ArrangementStateCannotBeCaptured
            }
            PlaybackHandoffError::ActiveArrangementStateCannotBeDropped => {
                Self::ActiveArrangementStateCannotBeDropped
            }
        }
    }

    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::SessionRuntime),
            2 => Some(Self::ActiveSessionStateCannotBeDropped),
            3 => Some(Self::PerformanceClockExhausted),
            4 => Some(Self::ArrangementStateCannotBeCaptured),
            5 => Some(Self::ActiveArrangementStateCannotBeDropped),
            6 => Some(Self::RuntimeFenceChanged),
            7 => Some(Self::FreshRuntimeCancelled),
            8 => Some(Self::ConstructorClaimsUnavailable),
            9 => Some(Self::StreamedPagesNotReady),
            10 => Some(Self::LatencyLayoutChanged),
            11 => Some(Self::SourceHistoryUnavailable),
            12 => Some(Self::StreamSourceChanged),
            13 => Some(Self::SharedSourceVoiceCapacity),
            14 => Some(Self::StreamedPageCapacity),
            15 => Some(Self::ForeignTailSourcePaused),
            16 => Some(Self::DeviceInsertNeedsConfirmation),
            17 => Some(Self::TempoCutMissed),
            _ => None,
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::SessionRuntime => {
                "session graph handoff was rejected; the prior graph remains active"
            }
            Self::ActiveSessionStateCannotBeDropped => {
                "session graph handoff was rejected because active session state cannot be dropped"
            }
            Self::PerformanceClockExhausted => {
                "session graph handoff was rejected because the legacy performance clock is exhausted"
            }
            Self::ArrangementStateCannotBeCaptured => {
                "session graph handoff was rejected because active arrangement state cannot be captured"
            }
            Self::ActiveArrangementStateCannotBeDropped => {
                "session graph handoff was rejected because active arrangement state cannot be dropped"
            }
            Self::RuntimeFenceChanged => {
                "fresh playback was rejected because the active runtime changed"
            }
            Self::FreshRuntimeCancelled => "playback was cancelled before it became active",
            Self::ConstructorClaimsUnavailable => "constructor playback claims are unavailable",
            Self::StreamedPagesNotReady => "streamed playback page readiness is unavailable",
            Self::StreamedPageCapacity => "stream cache capacity is full · stop playback or retry",
            Self::StreamSourceChanged => "original media changed · reopen or relink it",
            Self::ForeignTailSourcePaused => {
                "native effect tail is paused · resume playback, then retry"
            }
            Self::SharedSourceVoiceCapacity => {
                "shared source voice capacity is full · release notes or wait for tails, then retry"
            }
            Self::LatencyLayoutChanged => {
                "latency layout could not change live · previous audio continues"
            }
            Self::DeviceInsertNeedsConfirmation => {
                "device insertion needs confirmation before interrupting playback"
            }
            Self::TempoCutMissed => {
                "scheduled tempo edit missed its cut · previous audio continues"
            }
            Self::SourceHistoryUnavailable => {
                "source history cannot transfer live · previous audio continues"
            }
        }
    }
}

impl PlaybackHandoffError {
    pub const fn is_transient(self) -> bool {
        matches!(
            self,
            Self::Session(
                crate::SessionPlaybackError::ReleaseBankFull
                    | crate::SessionPlaybackError::GroupTailRoutingBusy
                    | crate::SessionPlaybackError::DeviceTransitionBusy
            )
        )
    }

    pub const fn rejection(self) -> Option<PlaybackHandoffRejection> {
        if self.is_transient() {
            None
        } else {
            Some(PlaybackHandoffRejection::from_error(self))
        }
    }
}

// Retained by exactly one ordinary graph and destroyed on control with that
// graph. This is comparison data, not native source or publication authority.
pub(crate) struct StoredForeignContinuation {
    identity: hex_lattice::ProcessorInstanceIdentity,
    binding: hex_lattice_contract::ForeignProcessorBinding,
    previous: hex_lattice_contract::NativeDeviceState,
    candidate: hex_lattice_contract::NativeDeviceState,
}
impl StoredForeignContinuation {
    fn matcher(
        &self,
    ) -> Result<
        hex_lattice::ForeignConfigurationContinuation<'_>,
        hex_lattice_contract::DeviceContractError,
    > {
        hex_lattice::ForeignConfigurationContinuation::new(
            self.identity,
            self.binding,
            &self.previous,
            &self.candidate,
        )
    }
    fn retained_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(self.previous.retained_control_metadata_bytes()?)?
            .checked_add(self.candidate.retained_control_metadata_bytes()?)
    }
}

/// Playback graph, with the legacy finite renderer kept as the compatibility path.
pub struct Playback {
    metronome: Option<Box<Metronome>>,
    tuning_audition: Option<Box<crate::tuning_audition::Runtime>>,
    project: Arc<Project>,
    prepared_revision: Option<Arc<PreparedRevision>>,
    // Control-minted publication identity; no callback owner or allocation.
    pub(crate) resident_pcm_publication: Option<std::num::NonZeroU64>,
    session_runtime: Option<Box<SessionPlayback>>,
    linear_runtime: Option<Box<RendererRuntime>>,
    session_unavailable: Option<crate::SessionPlaybackError>,
    position: u64,
    performance_frame: u64,
    performance_clock_exhausted: bool,
    // Boxed so publication results carrying a `Playback` stay small.
    transport_fade: Box<transport_fade::TransportFade>,
    playing: bool,
}
impl Playback {
    /// Quote resource-free observation metadata for this actual runtime, including a
    /// transient section's effective MIDI timing. Does not acquire graph resources.
    pub fn observation_storage_quote_on_control(&self) -> Result<usize, Invalid> {
        let revision = self
            .prepared_revision
            .as_deref()
            .ok_or(Invalid("playback revision unavailable"))?;
        crate::PreparedPlaybackObservation::storage_quote(
            revision,
            crate::session_inventory::EffectiveSessionMidi::new(
                revision.session(),
                self.session_runtime
                    .as_ref()
                    .and_then(|runtime| runtime.midi_admission.as_ref()),
            ),
        )
    }
    /// Capture gates from the installed candidate's immutable runtime metadata,
    /// preserving the original revision/resource identity and existing control custody.
    pub fn prepare_observation_on_control(
        &self,
    ) -> Result<crate::PreparedPlaybackObservation, Invalid> {
        let revision = self
            .prepared_revision
            .as_deref()
            .ok_or(Invalid("playback revision unavailable"))?;
        let runtime = self.session_runtime.as_ref();
        crate::PreparedPlaybackObservation::prepare_effective(
            revision,
            crate::session_inventory::EffectiveSessionMidi::new(
                revision.session(),
                runtime.and_then(|runtime| runtime.midi_admission.as_ref()),
            ),
            runtime.map_or_else(
                || revision.session().periodic_tempo(),
                |runtime| runtime.periodic_tempo(),
            ),
            runtime.is_none() && revision.session().periodic_tempo_error().is_some(),
        )
    }
    pub(crate) fn device_resource_leases(
        &self,
    ) -> impl Iterator<Item = &crate::native_device_resources::PreparedDeviceResourceLease> {
        self.session_runtime
            .iter()
            .flat_map(|runtime| runtime.device_resource_leases())
    }
    /// Control-side, before publication: a live handoff restarts this candidate
    /// at the cursor instead of inheriting voices and tails. Returns false when
    /// there is no Session runtime to restart.
    pub fn request_live_restart(&mut self) -> bool {
        self.session_runtime
            .as_mut()
            .map(|runtime| runtime.request_live_restart())
            .is_some()
    }
    /// Arm the resident clip in this exact candidate. Audio remains owned by
    /// the prepared revision; the gesture carries only track, placement and grid.
    pub fn arm_browser_audition(
        &mut self,
        track: hex_project::TrackId,
        start: u64,
        timing: BrowserAuditionTiming,
    ) {
        if let Some(runtime) = &mut self.session_runtime {
            runtime.arm_browser_audition(track, start, timing);
        }
    }
    pub fn bind_browser_audition_transport(&mut self, epoch: u64) {
        if let Some(runtime) = &mut self.session_runtime {
            runtime.bind_browser_audition_transport(epoch);
        }
    }
    pub fn browser_audition_retained_bytes_on_control(&self) -> Option<usize> {
        self.session_runtime
            .as_ref()
            .map_or(Some(0), |runtime| runtime.browser_audition_retained_bytes())
    }
    /// Whether `request_live_restart` applies to this candidate's live handoff.
    pub fn live_restart_requested(&self) -> bool {
        self.session_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.live_restart_requested())
    }
    /// Existing Session runtime-buffer accounting on control, excluding immutable prepared graph,
    /// source PCM and factory backing. This is the same bounded host/DSP inventory used by preroll;
    /// it is not whole-graph DATA identity or native constructor provenance.
    pub fn accounted_session_runtime_bytes_on_control(&self) -> Option<usize> {
        self.session_runtime
            .as_ref()?
            .accounted_runtime_storage_bytes()
    }
    /// Validated immutable inline resource-factory bytes retained by this graph. Publication may
    /// conservatively charge this per owner until its actual control-side destruction, separately
    /// from migrated source obligations. No revision owner is added to a source-tail anchor.
    pub fn resource_factory_inline_bytes_on_control(
        &self,
    ) -> Result<usize, crate::AudioStorageError> {
        match self.prepared_revision.as_deref() {
            Some(revision) if revision.device_resource_scope.is_some() => {
                revision.session().resource_factory_inline_bytes()
            }
            _ => Ok(0),
        }
    }
    /// Conservative resource-factory, ordinary handoff-evidence and source-program allowance.
    /// This follows the immutable graph's physical control-side destruction, not runtime leases.
    pub(crate) fn resource_control_metadata_bytes_on_control(
        &self,
    ) -> Result<usize, crate::AudioStorageError> {
        let continuation_bytes = self.foreign_continuation_metadata_bytes_on_control()?;
        self.resource_factory_inline_bytes_on_control()?
            .checked_add(
                self.prepared_revision
                    .as_deref()
                    .map_or(Ok(0), |revision| revision.device_handoff_metadata_bytes())?,
            )
            .and_then(|bytes| {
                bytes.checked_add(self.prepared_revision.as_deref().map_or(0, |revision| {
                    revision.session().instrument_automation_storage_bytes()
                }))
            })
            .and_then(|bytes| bytes.checked_add(continuation_bytes))
            .ok_or(crate::AudioStorageError::Overflow)
    }
    /// Conservative backing owned by this unpublished or admitted graph. The
    /// enclosing resource book or unpublished inventory must charge it once.
    pub(crate) fn foreign_continuation_metadata_bytes_on_control(
        &self,
    ) -> Result<usize, crate::AudioStorageError> {
        self.session_runtime
            .as_ref()
            .and_then(|runtime| runtime.foreign_continuation.as_ref())
            .map_or(Some(0), |states| states.retained_bytes())
            .ok_or(crate::AudioStorageError::Overflow)
    }
    /// Retain the exact two state allocations for a prospective ordinary
    /// continuation. This is comparison data only; the host must independently
    /// authenticate its worker, source, candidate and transport. The graph
    /// must use the ordinary resource book, and this owner is dropped on control.
    pub fn retain_foreign_continuation_states_on_control(
        &mut self,
        identity: hex_lattice::ProcessorInstanceIdentity,
        binding: hex_lattice_contract::ForeignProcessorBinding,
        previous: hex_lattice_contract::NativeDeviceState,
        candidate: hex_lattice_contract::NativeDeviceState,
    ) -> Result<(), Invalid> {
        if self.has_foreign_continuation_states()
            || self.resident_pcm_publication.is_some()
            || self
                .session_runtime
                .as_ref()
                .is_none_or(|runtime| runtime.require_fresh_candidate().is_err())
        {
            return Err(Invalid(
                "foreign continuation requires a fresh ordinary graph",
            ));
        }
        hex_lattice::ForeignConfigurationContinuation::new(
            identity, binding, &previous, &candidate,
        )
        .map_err(|_| Invalid("foreign continuation states differ"))?;
        let retained = StoredForeignContinuation {
            identity,
            binding,
            previous,
            candidate,
        };
        retained
            .retained_bytes()
            .ok_or(Invalid("foreign continuation storage overflow"))?;
        let Some(runtime) = self.session_runtime.as_mut() else {
            return Err(Invalid(
                "foreign continuation requires a fresh ordinary graph",
            ));
        };
        runtime.foreign_continuation = Some(Box::new(retained));
        Ok(())
    }
    /// Match a retry to the exact graph-owned comparison states. Equal bytes
    /// from another allocation are not the same candidate state.
    pub fn foreign_continuation_states_match_on_control(
        &self,
        identity: hex_lattice::ProcessorInstanceIdentity,
        binding: hex_lattice_contract::ForeignProcessorBinding,
        previous: &hex_lattice_contract::NativeDeviceState,
        candidate: &hex_lattice_contract::NativeDeviceState,
    ) -> bool {
        self.session_runtime
            .as_ref()
            .and_then(|runtime| runtime.foreign_continuation.as_ref())
            .is_some_and(|states| {
                states.identity == identity
                    && states.binding == binding
                    && states.previous.shares_payload_storage_with(previous)
                    && states.previous == *previous
                    && states.candidate.shares_payload_storage_with(candidate)
                    && states.candidate == *candidate
            })
    }

    /// Graph-owned comparison binding; this conveys no source or publication authority.
    pub fn foreign_continuation_binding(
        &self,
    ) -> Option<hex_lattice_contract::ForeignProcessorBinding> {
        self.session_runtime
            .as_ref()?
            .foreign_continuation
            .as_ref()
            .map(|states| states.binding)
    }

    /// Compare the retained predecessor against the named immutable graph node.
    /// This is bounded, allocation-free comparison data, not source or
    /// publication authority; the audio owner validates the actual source.
    pub fn foreign_continuation_matches_predecessor_on_callback(&self, previous: &Self) -> bool {
        let Some(states) = self
            .session_runtime
            .as_ref()
            .and_then(|runtime| runtime.foreign_continuation.as_ref())
        else {
            return false;
        };
        fn matches<'a>(
            nodes: impl Iterator<Item = &'a hex_lattice::PreparedRuntimeInstance>,
            states: &StoredForeignContinuation,
        ) -> bool {
            let mut nodes = nodes.filter(|node| {
                node.owner == states.identity.owner()
                    && node.device_id == states.identity.device_id()
            });
            let Some(node) = nodes.next() else {
                return false;
            };
            nodes.next().is_none()
                && matches!(&node.definition, Some(hex_lattice::PreparedProcessorFactory::Foreign(prepared)) if prepared.binding() == Some(states.binding) && !prepared.is_instrument_profile())
                && node.configuration.as_deref().is_some_and(|state| {
                    state.shares_payload_storage_with(&states.previous) && *state == states.previous
                })
        }
        match states.identity.owner() {
            hex_lattice::ProcessorChainOwner::Main => {
                matches(previous.project.main_device_chain.iter(), states)
            }
            hex_lattice::ProcessorChainOwner::Track(owner) => previous
                .project
                .tracks
                .iter()
                .find(|track| {
                    track
                        .canonical_track_id()
                        .is_some_and(|id| id.value() == owner)
                })
                .is_some_and(|track| matches(track.device_chain.iter(), states)),
            hex_lattice::ProcessorChainOwner::Return(owner) => previous
                .project
                .return_buses
                .iter()
                .find(|bus| bus.id.value() == owner)
                .is_some_and(|bus| matches(bus.device_chain.iter(), states)),
            // Source effect stages reject foreign processors at preparation.
            hex_lattice::ProcessorChainOwner::Source(_) => false,
        }
    }

    /// This graph retains extra state backing that needs ordinary resource accounting.
    pub fn has_foreign_continuation_states(&self) -> bool {
        self.session_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.foreign_continuation.is_some())
    }

    pub fn contains_device_resource_leases(&self) -> bool {
        self.device_resource_leases().next().is_some()
    }
    /// Borrow actual legacy PCM leaves on control. The iterator grants no callback
    /// or reclamation authority, and does not expose native device source banks.
    pub(crate) fn resident_pcm_sources(&self) -> impl Iterator<Item = &crate::Audio> + Clone {
        self.project
            .tracks()
            .iter()
            .flat_map(|t| t.clips.iter().filter_map(|c| c.audio.as_ref()))
            .chain(self.prepared_revision.iter().flat_map(|r| {
                r.session()
                    .audio_clips()
                    .iter()
                    .filter_map(|c| c.resident_audio())
            }))
    }
    /// Control-side PCM capacity of this graph, deduplicated by actual backing.
    pub fn resident_pcm_capacity_frames(&self) -> Option<u64> {
        self.resident_pcm_sources()
            .enumerate()
            .try_fold(0_u64, |n, (i, source)| {
                if self
                    .resident_pcm_sources()
                    .take(i)
                    .any(|old| old.shares_pcm_with(source))
                {
                    return Some(n);
                }
                n.checked_add(u64::try_from(source.pcm_capacity_frames()).ok()?)
            })
    }
    /// Control-side attachment validated against the exact compiled instrument instance.
    pub fn live_expression_supported_on_control(&self, track: hex_project::TrackId) -> bool {
        self.session_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.live_expression_supported_on_control(track))
    }
    /// Control-side attachment validated against the exact compiled instrument instance.
    pub fn prepare_live_expression(
        &mut self,
        project: &hex_project::Project,
        input: Arc<crate::live_expression::LiveExpressionInput>,
    ) -> Result<(), Invalid> {
        let canonical = project
            .track(input.track)
            .and_then(hex_project::Track::instrument)
            .ok_or(Invalid("controller input instrument unavailable"))?;
        let compiled = self
            .project
            .tracks()
            .iter()
            .find(|track| track.canonical_track_id() == Some(input.track))
            .ok_or(Invalid("controller input track unavailable"))?;
        if project.sample_rate() != self.sample_rate()
            || canonical.id() != input.instrument
            || compiled.instrument_id != Some(input.instrument)
            || compiled.instrument_state.as_ref()
                != Some(&canonical.state().clone().without_host_gains())
            || compiled.instrument_bypassed != canonical.bypassed()
            || canonical.bypassed()
        {
            return Err(Invalid("controller input source does not match playback"));
        }
        let runtime = self
            .session_runtime
            .as_mut()
            .ok_or(Invalid("controller input requires prepared playback"))?;
        runtime.attach_live_expression(input)?;
        Ok(())
    }
    /// Control-side only: attach a selected instrument's bounded audition pool.
    /// The ordinary graph publication owns preparation, inheritance and retirement.
    pub fn prepare_computer_keyboard(
        &mut self,
        project: &hex_project::Project,
        track: hex_project::TrackId,
        input: Arc<crate::computer_keyboard::ComputerKeyboardInput>,
    ) -> Result<(), Invalid> {
        self.prepare_computer_keyboard_impl(project, track, input, None)
    }

    /// Extra resident DATA for an independent paused source route. The
    /// ordinary candidate owns these bytes through callback return. Zero
    /// means the compiled graph uses the existing monitoring path.
    pub fn paused_source_monitor_allowance_on_control(
        revision: &PreparedRevision,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
    ) -> Result<usize, Invalid> {
        crate::PausedTrackMonitor::resident_allowance(revision, resources, &[])
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    /// Prepare independent pause processing under the candidate's existing
    /// pre-admitted DATA reservation. The same resident owns all note and
    /// keyboard history across both processing routes.
    pub fn prepare_resident_paused_source_monitor(
        &mut self,
        resources: Option<&crate::native_device_resources::PreparedDeviceResources>,
    ) -> Result<(), Invalid> {
        let revision = self.prepared_revision.as_ref().ok_or(Invalid(
            "paused source monitor requires a prepared revision",
        ))?;
        let allowance = Self::paused_source_monitor_allowance_on_control(revision, resources)?;
        if allowance == 0 {
            return Ok(());
        }
        self.validate_candidate_runtime_storage(allowance)
            .map_err(|_| Invalid("paused source monitor exceeds graph storage budget"))?;
        let monitor = crate::PausedTrackMonitor::prepare_resident_graph(
            Arc::clone(revision),
            resources,
            &[],
            allowance,
        )
        .map_err(|error| Invalid(error.diagnostic_message()))?;
        let runtime = self
            .session_runtime
            .as_mut()
            .ok_or(Invalid("paused source monitor requires a session runtime"))?;
        runtime.paused_source_monitor = Some(Box::new(monitor));
        Ok(())
    }
    /// Reuse a checked tuning-only family and its exact physical lease. The
    /// caller prepares that family through PreparedRevision::prepare_tuning_from.
    pub fn prepare_computer_keyboard_for_tuning(
        &mut self,
        project: &hex_project::Project,
        track: hex_project::TrackId,
        input: Arc<crate::computer_keyboard::ComputerKeyboardInput>,
        hint: &crate::computer_keyboard::ComputerKeyboardTuningHint,
    ) -> Result<(), Invalid> {
        self.prepare_computer_keyboard_impl(project, track, input, Some(hint))
    }
    /// Controller-only publication retains the same checked lease promotion
    /// hint after a tuned Operator has returned to neutral through Undo.
    pub fn prepare_computer_keyboard_for_controller_activation_with_tuning_hint(
        &mut self,
        project: &hex_project::Project,
        track: hex_project::TrackId,
        input: Arc<crate::computer_keyboard::ComputerKeyboardInput>,
        hint: &crate::computer_keyboard::ComputerKeyboardTuningHint,
    ) -> Result<(), Invalid> {
        self.prepare_computer_keyboard_for_tuning(project, track, input, hint)
    }
    pub fn computer_keyboard_tuning_hint(
        &self,
    ) -> Option<crate::computer_keyboard::ComputerKeyboardTuningHint> {
        self.session_runtime
            .as_ref()?
            .computer_keyboard
            .as_ref()?
            .tuning_hint()
    }
    fn prepare_computer_keyboard_impl(
        &mut self,
        project: &hex_project::Project,
        track: hex_project::TrackId,
        input: Arc<crate::computer_keyboard::ComputerKeyboardInput>,
        hint: Option<&crate::computer_keyboard::ComputerKeyboardTuningHint>,
    ) -> Result<(), Invalid> {
        let owner = project
            .tracks()
            .iter()
            .find(|candidate| candidate.id() == track)
            .ok_or(Invalid("computer keyboard track is unavailable"))?;
        if owner.retained_source_path() {
            return Err(Invalid(
                "computer keyboard cannot target a retained source path",
            ));
        }
        let canonical = owner
            .instrument()
            .ok_or(Invalid("computer keyboard track is unavailable"))?;
        let contributor = self
            .project
            .tracks()
            .iter()
            .position(|candidate| candidate.canonical_track_id() == Some(track))
            .ok_or(Invalid("computer keyboard track is unavailable"))?;
        let receiver = self
            .prepared_revision
            .as_ref()
            .and_then(|revision| revision.source_routing().receiver_index(contributor));
        let compiled = self
            .project
            .tracks()
            .get(receiver.unwrap_or(contributor))
            .ok_or(Invalid("computer keyboard receiver is unavailable"))?;
        if project.sample_rate() != self.sample_rate()
            || compiled.instrument_id != Some(canonical.id())
            || compiled.instrument_state.as_ref() != Some(canonical.state())
            || compiled.instrument_bypassed != canonical.bypassed()
        {
            return Err(Invalid("computer keyboard source does not match playback"));
        }
        let runtime = self.session_runtime.as_mut().ok_or(Invalid(
            "computer keyboard requires prepared session playback",
        ))?;
        if compiled.foreign_instrument.is_some() {
            if hint.is_some() {
                return Err(Invalid(
                    "vst3 computer keyboard tuning hints are unsupported",
                ));
            }
            return runtime.prepare_foreign_keyboard(track, input);
        }
        let family = self
            .prepared_revision
            .as_ref()
            .ok_or(Invalid(
                "computer keyboard requires prepared revision provenance",
            ))?
            .tuning_family();
        if self.project.shared_contexts().tuning()
            != &project.tuning().table().map_err(|e| Invalid(e.0))?
        {
            return Err(Invalid("computer keyboard tuning does not match playback"));
        }
        let source_input = receiver.filter(|_| !runtime.current_sampler_console_available());
        let resources = if source_input.is_none() {
            runtime.device_resource_lease_on_control(track)?
        } else {
            None
        };
        let mut candidate = Box::new(if let Some(receiver) = source_input {
            let revision = self
                .prepared_revision
                .as_ref()
                .ok_or(Invalid("source provenance is unavailable"))?;
            let source = runtime.source_runtime_on_control(receiver);
            let authored = revision
                .source_routing()
                .contributors(receiver)
                .iter()
                .try_fold(0usize, |sum, &index| {
                    sum.checked_add(runtime.source_voice_envelope(usize::from(index)))
                        .ok_or(Invalid("shared live voice capacity overflow"))
                })?;
            let capacity = source.map_or(64usize.saturating_sub(authored), |source| {
                source.live_voice_capacity(authored)
            });
            let contexts = self.project.shared_contexts();
            let context = contexts
                .index_for_device(canonical.id())
                .and_then(|index| contexts.clone_owner(index));
            crate::computer_keyboard::ComputerKeyboardRuntime::prepare_source_input(
                project, track, input, source, context, capacity,
            )?
        } else if let Some(resources) = resources {
            if hint.is_some() {
                return Err(Invalid("resource keyboard tuning hints are unsupported"));
            }
            let factory = self
                .prepared_revision
                .as_ref()
                .and_then(|revision| revision.session().mutable_instrument_factory(track))
                .ok_or(Invalid("resource keyboard mutable factory is unavailable"))?;
            let mut keyboard =
                crate::computer_keyboard::ComputerKeyboardRuntime::prepare_resource_mutable(
                    track,
                    input,
                    project.sample_rate(),
                    factory,
                    resources,
                )?;
            keyboard.set_resource_identity(
                canonical.id(),
                project.tuning().table().map_err(|error| Invalid(error.0))?,
            );
            keyboard
        } else if let Some(hint) = hint {
            crate::computer_keyboard::ComputerKeyboardRuntime::prepare_for_tuning(
                project, track, input, family, hint,
            )?
        } else {
            crate::computer_keyboard::ComputerKeyboardRuntime::prepare(project, track, input)?
        });
        candidate.set_tuning_family(family);
        if candidate.is_source_input()
            && let Some(hint) = hint
        {
            candidate.validate_source_tuning_lease(family, hint)?;
        }
        let previous = runtime.computer_keyboard.replace(candidate);
        if runtime.validate_candidate_runtime_storage(0).is_err() {
            runtime.computer_keyboard = previous;
            return Err(Invalid("computer keyboard exceeds graph storage budget"));
        }
        if runtime
            .computer_keyboard
            .as_ref()
            .is_some_and(|keyboard| keyboard.is_source_input())
            && runtime.activate_source_monitor_on_control().is_err()
        {
            runtime.computer_keyboard = previous;
            return Err(Invalid("source monitoring could not release clip notes"));
        }
        if let Some(input) = previous
            .as_ref()
            .and_then(|keyboard| keyboard.source_input_id())
        {
            runtime.release_source_input_on_control(input);
        }
        Ok(())
    }

    /// Reprepare an existing same-revision keyboard lease for a controller-only
    /// publication. Ordinary project publications keep their cancellation policy.
    pub fn prepare_computer_keyboard_for_controller_activation(
        &mut self,
        project: &hex_project::Project,
        track: hex_project::TrackId,
        input: Arc<crate::computer_keyboard::ComputerKeyboardInput>,
    ) -> Result<(), Invalid> {
        self.prepare_computer_keyboard(project, track, input)?;
        if let Some(keyboard) = self
            .session_runtime
            .as_mut()
            .and_then(|runtime| runtime.computer_keyboard.as_mut())
        {
            keyboard.request_controller_handoff_preservation();
        }
        Ok(())
    }

    pub fn cancel_computer_keyboard(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.cancel_current_sampler_keyboard();
            runtime.cancel_foreign_keyboard(None);
        }

        if let Some(keyboard) = self
            .session_runtime
            .as_mut()
            .and_then(|runtime| runtime.computer_keyboard.as_mut())
        {
            keyboard.cancel();
        }
    }
    pub(crate) fn new_audio_candidate(
        revision: Arc<PreparedRevision>,
    ) -> Result<Self, crate::AudioStorageError> {
        let runtime = SessionPlayback::new_audio_candidate(Arc::clone(&revision))?;
        Ok(Self::from_parts(revision, Some(Box::new(runtime)), None))
    }
    pub(crate) fn new_current_sampler_candidate(
        revision: Arc<PreparedRevision>,
        sources: &crate::native_device_resources::current_sampler::CurrentSamplerSources,
    ) -> Result<Self, crate::AudioStorageError> {
        let runtime =
            SessionPlayback::new_current_sampler_candidate(Arc::clone(&revision), sources)?;
        Ok(Self::from_parts(revision, Some(Box::new(runtime)), None))
    }
    pub(crate) fn current_sampler_console_available(&self) -> bool {
        self.session_runtime
            .as_ref()
            .is_some_and(|r| r.current_sampler_console_available())
    }
    #[cfg(test)]
    pub(crate) fn test_current_sampler_route_proofs(
        &self,
        index: usize,
    ) -> Option<(u64, usize, usize)> {
        self.session_runtime
            .as_ref()?
            .test_current_sampler_route_proofs(index)
    }
    pub(crate) fn current_sampler_keyboard_epoch(&self) -> Option<u64> {
        let epoch = self
            .session_runtime
            .as_ref()?
            .current_sampler_keyboard_epoch;
        (epoch != 0).then_some(epoch)
    }
    pub(crate) fn current_sampler_keyboard_rejected_epoch(&self) -> Option<u64> {
        let runtime = self.session_runtime.as_ref()?;
        (runtime.current_sampler_keyboard_epoch != 0
            && runtime
                .current_sampler_keyboards
                .iter()
                .any(|k| k.current_rejected()))
        .then_some(runtime.current_sampler_keyboard_epoch)
    }
    pub(crate) fn apply_current_sampler_keyboard(
        &mut self,
        frame: Option<crate::computer_keyboard::CurrentSamplerKeyboardFrame>,
    ) -> Result<(), Invalid> {
        self.session_runtime
            .as_mut()
            .ok_or(Invalid("sampler runtime unavailable"))?
            .apply_current_sampler_keyboard(frame)
    }
    pub(crate) fn render_current_sampler<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        sources: &crate::native_device_resources::current_sampler::CurrentSamplerSources,
    ) -> Result<(), Invalid> {
        self.render_current_sampler_with_foreign(
            output,
            pages,
            observer,
            project,
            sources,
            &mut hex_lattice::RefuseForeign,
        )
    }
    pub(crate) fn render_current_sampler_with_foreign<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        sources: &crate::native_device_resources::current_sampler::CurrentSamplerSources,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        let mut pages = pages;
        let mut observer = observer;
        let mut project = project;
        self.render_declicked(output, |playback, output, offset| {
            let runtime = playback
                .session_runtime
                .as_mut()
                .ok_or(Invalid("current sampler runtime missing"))?;
            let mut foreign = ShiftedForeign {
                inner: &mut *foreign,
                offset,
            };
            with_part_offset(project.as_deref_mut(), offset, |project| {
                with_shifted_observer(observer.as_deref_mut(), offset, |observer| {
                    runtime
                        .render_current_sampler_with_foreign(
                            output,
                            pages.as_deref_mut(),
                            observer,
                            project,
                            sources,
                            &mut foreign,
                        )
                        .map_err(|e| Invalid(e.diagnostic_message()))
                })
            })
        })
    }
    pub(crate) fn validate_candidate_runtime_storage(
        &self,
        metadata: usize,
    ) -> Result<(), crate::AudioStorageError> {
        self.session_runtime
            .as_ref()
            .ok_or(crate::AudioStorageError::UnsupportedGraph)?
            .validate_candidate_runtime_storage(metadata)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub(crate) fn test_track_boxes(
        &self,
        track: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        self.session_runtime.as_ref()?.test_track_boxes(track)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub(crate) fn test_tail_boxes(
        &self,
        track: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        self.session_runtime.as_ref()?.test_tail_boxes(track)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub(crate) fn test_tail_activity(&self, track: hex_project::TrackId) -> Option<(bool, bool)> {
        self.session_runtime.as_ref()?.test_tail_activity(track)
    }
    #[cfg(test)]
    pub(crate) fn test_populate_unsupported_tail(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.test_populate_unsupported_tail();
        }
    }
    pub(crate) fn visit_candidate_groups(
        &self,
        visit: &mut impl FnMut(u8, usize, bool) -> Result<(), crate::AudioStorageError>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self {
            metronome,
            tuning_audition,
            project: _,
            prepared_revision,
            resident_pcm_publication,
            session_runtime,
            linear_runtime,
            session_unavailable: _,
            position: _,
            performance_frame: _,
            performance_clock_exhausted: _,
            transport_fade: _,
            playing: _,
        } = self;
        if tuning_audition.is_some()
            || resident_pcm_publication.is_some()
            || self.has_foreign_continuation_states()
        {
            return Err(crate::AudioStorageError::UnsupportedGraph);
        }
        visit(9, 0, true)?; // actual Project handle
        visit(10, 0, prepared_revision.is_some())?;
        visit(11, 0, metronome.is_some())?;
        visit(12, 0, session_runtime.is_some())?;
        visit(13, 0, linear_runtime.is_some())?;
        session_runtime
            .as_ref()
            .ok_or(crate::AudioStorageError::UnsupportedGraph)?
            .visit_candidate_groups(visit)
    }
    pub fn new(project: Project) -> Self {
        Self::from_shared(Arc::new(project))
    }

    /// Share a control-side compiled project without recompiling or copying its graph.
    /// This constructor deliberately keeps legacy Arrangement-only behavior.
    pub fn from_shared(project: Arc<Project>) -> Self {
        let linear_runtime = Some(Box::new(RendererRuntime::new(&project)));
        let metronome = None;
        Self {
            metronome,
            tuning_audition: None,
            project,
            prepared_revision: None,
            resident_pcm_publication: None,
            session_runtime: None,
            linear_runtime,
            session_unavailable: None,
            position: 0,
            performance_frame: 0,
            performance_clock_exhausted: false,
            transport_fade: Box::default(),
            playing: false,
        }
    }

    /// Retain an accepted artifact with the shared prepared route runtime.
    /// If Session runtime admission fails, its capability error stays explicit;
    /// the existing finite fallback remains available where it is safe.
    pub fn from_prepared(prepared_revision: Arc<PreparedRevision>) -> Self {
        match SessionPlayback::new(Arc::clone(&prepared_revision)) {
            Ok(runtime) => Self::from_parts(prepared_revision, Some(Box::new(runtime)), None),
            Err(error) => Self::from_parts(prepared_revision, None, Some(error)),
        }
    }

    /// Fallibly prepare the bounded Session runtime on a control worker.
    /// A successful runtime begins with all tracks owned by Arrangement.
    pub fn try_from_prepared_with_session(
        prepared_revision: Arc<PreparedRevision>,
    ) -> Result<Self, crate::SessionPlaybackError> {
        let runtime = SessionPlayback::new(Arc::clone(&prepared_revision))?;
        Ok(Self::from_parts(
            prepared_revision,
            Some(Box::new(runtime)),
            None,
        ))
    }

    /// Build the ordinary Session/Arrangement runtime against its compiler's exact original-source
    /// set. Source custody stays in runtime leases and follows outgoing runtime tails.
    pub fn try_from_prepared_with_device_resources(
        prepared_revision: Arc<PreparedRevision>,
        resources: &crate::native_device_resources::PreparedDeviceResources,
    ) -> Result<Self, crate::SessionPlaybackError> {
        let runtime =
            SessionPlayback::new_with_device_resources(Arc::clone(&prepared_revision), resources)?;
        Ok(Self::from_parts(
            prepared_revision,
            Some(Box::new(runtime)),
            None,
        ))
    }

    /// Prepare an unused interactive candidate retaining a launched section loop.
    /// This performs no seek or replay; the ordinary callback handoff owns clock/state transfer.
    pub fn try_from_prepared_section_candidate(
        prepared_revision: Arc<PreparedRevision>,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        region: hex_project::LoopRegion,
    ) -> Result<Self, crate::SessionPlaybackError> {
        let runtime = SessionPlayback::new_section_candidate(
            Arc::clone(&prepared_revision),
            resources,
            region,
        )?;
        Ok(Self::from_parts(
            prepared_revision,
            Some(Box::new(runtime)),
            None,
        ))
    }

    /// Install a completed control-prepared Arrangement preroll without
    /// reconstructing or seeking the runtime. The runtime's exact prepared
    /// revision, clocks and play state are retained as-is. This is still the
    /// linear Arrangement-only policy: Session route actions and live native
    /// promotion are not supported by this constructor.
    pub fn from_arrangement_preroll(
        runtime: SessionPlayback,
    ) -> Result<Self, crate::ArrangementPrerollError> {
        if !runtime.is_linear_arrangement_export() {
            return Err(crate::ArrangementPrerollError::CandidatePositionMismatch);
        }
        Ok(Self::from_prerolled_session_runtime(runtime))
    }

    /// Wrap an interactive Session runtime without reconstructing, seeking, or
    /// changing its route policy. The caller must supply a runtime prepared by
    /// `prepare_interactive_arrangement_preroll_from_revision` and perform its
    /// own current-project/route admission checks; this wrapper does not prove
    /// the runtime's target frame or that it is ready for live publication.
    /// The wrapped runtime remains Session-capable and can accept route
    /// commands or serve as the predecessor for an ordinary graph-handoff
    /// candidate.
    pub fn from_interactive_arrangement_preroll(
        runtime: SessionPlayback,
    ) -> Result<Self, crate::ArrangementPrerollError> {
        if runtime.is_linear_arrangement_export() {
            return Err(crate::ArrangementPrerollError::CandidatePositionMismatch);
        }
        Ok(Self::from_prerolled_session_runtime(runtime))
    }

    fn from_prerolled_session_runtime(runtime: SessionPlayback) -> Self {
        let prepared_revision = Arc::clone(runtime.prepared_revision());
        let project = Arc::clone(prepared_revision.arrangement());
        let metronome = (!runtime.is_linear_arrangement_export())
            .then(|| {
                Metronome::prepare(project.sample_rate(), project.main_latency_frames())
                    .ok()
                    .map(Box::new)
            })
            .flatten();
        let position = runtime.arrangement_frame();
        let performance_frame = runtime.performance_frame();
        let playing = runtime.playing();
        Self {
            metronome,
            tuning_audition: None,
            project,
            prepared_revision: Some(prepared_revision),
            resident_pcm_publication: None,
            session_runtime: Some(Box::new(runtime)),
            linear_runtime: None,
            session_unavailable: None,
            position,
            performance_frame,
            performance_clock_exhausted: false,
            transport_fade: Box::default(),
            playing,
        }
    }

    /// Retain a valid Arrangement artifact when the optional Session runtime
    /// could not be prepared. The capability failure remains explicit.
    pub fn from_prepared_with_session_unavailable(
        prepared_revision: Arc<PreparedRevision>,
        error: crate::SessionPlaybackError,
    ) -> Self {
        Self::from_parts(prepared_revision, None, Some(error))
    }

    fn from_parts(
        prepared_revision: Arc<PreparedRevision>,
        session_runtime: Option<Box<SessionPlayback>>,
        session_unavailable: Option<crate::SessionPlaybackError>,
    ) -> Self {
        let project = Arc::clone(prepared_revision.arrangement());
        let metronome = Metronome::prepare(project.sample_rate(), project.main_latency_frames())
            .ok()
            .map(Box::new);
        let refuses_unowned_prepared_fallback = session_runtime.is_none()
            && session_unavailable.is_some()
            && (prepared_revision.session().has_mutable_instruments()
                || !project.main_device_chain.is_empty()
                || project
                    .tracks()
                    .iter()
                    .any(|track| track.has_device_chain() || track.has_instrument_source()));
        let linear_runtime = (session_runtime.is_none() && !refuses_unowned_prepared_fallback)
            .then(|| Box::new(RendererRuntime::new(&project)));
        Self {
            metronome,
            tuning_audition: None,
            project,
            prepared_revision: Some(prepared_revision),
            resident_pcm_publication: None,
            session_runtime,
            linear_runtime,
            session_unavailable,
            position: 0,
            performance_frame: 0,
            performance_clock_exhausted: false,
            transport_fade: Box::default(),
            playing: false,
        }
    }

    pub fn prepared_revision(&self) -> Option<&Arc<PreparedRevision>> {
        self.prepared_revision.as_ref()
    }

    /// Borrow the actual prepared foreign nodes on control. These immutable
    /// facts grant no native source or endpoint custody: publication must match
    /// them to the host's retained instances and protect their graph lifetime.
    pub fn foreign_nodes_on_control(
        &self,
    ) -> impl Iterator<Item = &hex_lattice::PreparedRuntimeInstance> {
        self.project
            .tracks
            .iter()
            .flat_map(|track| {
                track.device_chain.iter().chain(
                    track
                        .foreign_instrument
                        .iter()
                        .filter(|node| !node.bypassed),
                )
            })
            .chain(
                self.project
                    .return_buses
                    .iter()
                    .flat_map(|bus| bus.device_chain.iter()),
            )
            .chain(self.project.main_device_chain.iter())
            .filter(|node| {
                matches!(
                    node.definition,
                    Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
                )
            })
    }

    /// Compare a prepared node to a source binding on control. This is not
    /// native source custody or publication authority.
    pub fn has_foreign_binding_on_control(
        &self,
        owner: hex_lattice::ProcessorChainOwner,
        device: u64,
        binding: hex_lattice_contract::ForeignProcessorBinding,
    ) -> bool {
        self.foreign_nodes_on_control().any(|node| {
            node.owner == owner && node.device_id == device
                && matches!(&node.definition, Some(hex_lattice::PreparedProcessorFactory::Foreign(prepared)) if prepared.binding() == Some(binding))
        })
    }

    /// Observe actual current and committed release-tail foreign bindings.
    /// The audio owner uses this bounded scalar visit before exposing a graph
    /// return; copied immutable revision facts alone omit migrated tails.
    /// No allocation, source transfer or concurrent runtime access is allowed.
    pub fn visit_foreign_runtime_bindings(
        &self,
        visit: &mut impl FnMut(Option<hex_lattice_contract::ForeignProcessorBinding>),
    ) {
        if let Some(runtime) = self.session_runtime.as_ref() {
            runtime.visit_foreign_runtime_bindings(visit);
        }
    }

    pub fn native_parameter_input_target(&self) -> Option<crate::NativeParameterInputTarget> {
        self.session_runtime
            .as_ref()?
            .native_parameter_input_target()
    }
    pub fn set_native_parameter_input(
        &mut self,
        operation: hex_lattice::NativeTransientOperation,
    ) -> Result<(), Invalid> {
        self.session_runtime
            .as_mut()
            .ok_or(Invalid("native parameter input requires session playback"))?
            .set_native_parameter_input(operation)
    }
    pub fn invalidate_native_parameter_input(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.invalidate_native_parameter_input();
        }
    }
    pub fn finish_native_parameter_input(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.finish_native_parameter_input();
        }
    }
    pub fn take_native_parameter_application(
        &mut self,
    ) -> Option<crate::NativeParameterApplication> {
        self.session_runtime
            .as_mut()?
            .take_native_parameter_application()
    }

    /// True only for an explicitly armed, admitted Session-capable runtime.
    pub fn gain_recording_ready(&self, track: u64) -> bool {
        self.session_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.gain_recording_ready(track))
    }
    /// Apply an absolute fader override. Construction and validation never allocate here.
    pub fn set_gain_recording_override(&mut self, track: u64, gain: f32) -> Result<(), Invalid> {
        self.session_runtime
            .as_mut()
            .ok_or(Invalid("gain recording requires session playback"))?
            .set_gain_recording_override(track, gain)
    }
    pub fn clear_gain_recording_override(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.clear_gain_recording_override();
        }
    }
    pub fn release_gain_recording_override(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.release_gain_recording_override();
        }
    }
    /// Arrangement content frame, known Main delivery delay and actual content loop pass.
    /// Delivery delay excludes physical latency.
    pub fn gain_recording_clock(&self, track: u64) -> Option<(u64, u64, u64)> {
        self.session_runtime
            .as_ref()
            .and_then(|runtime| runtime.gain_recording_clock(track))
    }

    /// Set or smoothly release one transient post-node processor audition on
    /// the active graph. Retained effect tails are never command targets.
    pub fn select_processor_audition(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: Option<hex_lattice::ProcessorAuditionSelection>,
    ) -> bool {
        if let Some(runtime) = self.session_runtime.as_mut() {
            return runtime.select_processor_audition(identity, selection);
        }
        self.linear_runtime.as_mut().is_some_and(|runtime| {
            runtime.select_processor_audition(&self.project, identity, selection)
        })
    }

    /// Transfer only a held transient selector from an instrument-release
    /// suffix after this graph has inherited its runtime successfully.
    pub fn transfer_processor_audition_from_release_tail(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: hex_lattice::ProcessorAuditionSelection,
    ) -> bool {
        self.session_runtime.as_mut().is_some_and(|runtime| {
            runtime.transfer_processor_audition_from_release_tail(identity, selection)
        })
    }

    pub fn processor_audition_is_active(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: hex_lattice::ProcessorAuditionSelection,
    ) -> bool {
        if let Some(runtime) = self.session_runtime.as_mut() {
            return runtime.processor_audition_is_active(identity, selection);
        }
        self.linear_runtime.as_mut().is_some_and(|runtime| {
            runtime.processor_audition_is_active(&self.project, identity, selection)
        })
    }

    pub fn cancel_processor_audition(&mut self, identity: crate::PostNodeIdentity) -> bool {
        if let Some(runtime) = self.session_runtime.as_mut() {
            return runtime.cancel_processor_audition(identity);
        }
        self.linear_runtime
            .as_mut()
            .is_some_and(|runtime| runtime.cancel_processor_audition(&self.project, identity))
    }

    /// Hard-clear transient audition state at an ownership/lifecycle boundary.
    /// Session residual tails are included because they can continue processing
    /// after their original route is replaced.
    pub fn cancel_all_processor_auditions(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.cancel_all_processor_auditions();
        }
        if let Some(runtime) = self.linear_runtime.as_mut() {
            runtime.cancel_all_processor_auditions();
        }
    }

    /// Clear auditions from residual effect chains transferred into this
    /// candidate after a successful graph handoff. The active route chains are
    /// left intact so compatible live EQ8 requests survive parameter previews.
    pub fn cancel_retired_processor_auditions(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.cancel_retired_processor_auditions();
        }
    }

    pub fn session_capability(&self) -> SessionCapability {
        if self.session_runtime.is_some() {
            SessionCapability::Available
        } else if let Some(error) = self.session_unavailable {
            SessionCapability::Unavailable(error)
        } else {
            SessionCapability::Legacy
        }
    }

    pub const fn session_capability_status(&self) -> SessionCapabilityStatus {
        match self.session_unavailable {
            Some(crate::SessionPlaybackError::SessionTempoUnavailable(_))
            | Some(crate::SessionPlaybackError::TempoConversion) => {
                SessionCapabilityStatus::TimingUnavailable
            }
            Some(crate::SessionPlaybackError::Allocation) => {
                SessionCapabilityStatus::AllocationFailed
            }
            Some(crate::SessionPlaybackError::RevisionTrackMismatch)
            | Some(crate::SessionPlaybackError::RevisionSampleRateMismatch) => {
                SessionCapabilityStatus::RevisionMismatch
            }
            Some(_) => SessionCapabilityStatus::RuntimeUnavailable,
            None if self.session_runtime.is_some() => SessionCapabilityStatus::Available,
            None => SessionCapabilityStatus::Legacy,
        }
    }

    fn metronome_supported(&self) -> bool {
        self.prepared_revision.is_some()
            && self.metronome.is_some()
            && !self
                .session_runtime
                .as_ref()
                .is_some_and(|runtime| runtime.is_linear_arrangement_export())
    }

    /// Last admitted transient click state, independent of project history.
    pub fn metronome_enabled(&self) -> bool {
        self.metronome
            .as_ref()
            .is_some_and(|metronome| metronome.enabled())
    }

    /// Preserve only the transient click toggle across an accepted fresh seek
    /// of this exact prepared source, resetting its sample tail. The caller
    /// must separately prove the same project context and publication fence.
    /// Transport, route ownership and reconstructed DSP history are untouched.
    /// Unsupported or different-source recipients retain their own state.
    pub fn inherit_metronome_for_fresh_seek(&mut self, previous: &Self) {
        let same_source = match (&self.prepared_revision, &previous.prepared_revision) {
            (Some(next), Some(old)) => Arc::ptr_eq(next, old),
            _ => false,
        };
        if !same_source || !self.metronome_supported() || !previous.metronome_supported() {
            return;
        }
        if let Some(metronome) = self.metronome.as_mut() {
            metronome.set_enabled(previous.metronome_enabled());
            metronome.reset();
        }
    }

    /// At an accepted, same-project fresh seek, release old authored gates on
    /// any exact retained foreign instrument. The destination owns transport
    /// and all internal DSP history. Call only after the final publication
    /// decision, while the predecessor still owns its accepted note ledger.
    pub fn cut_retained_foreign_gates_for_fresh_seek(&mut self, previous: &mut Self) {
        if let (Some(next), Some(old)) = (&mut self.session_runtime, &mut previous.session_runtime)
        {
            next.cut_retained_foreign_gates_from(old);
        }
    }

    fn reset_metronome(&mut self) {
        if let Some(metronome) = self.metronome.as_mut() {
            metronome.reset();
        }
    }

    fn mix_metronome(&mut self, start: u64, playing: bool, output: &mut [Frame]) {
        if !playing {
            self.reset_metronome();
            return;
        }
        if let (Some(metronome), Some(revision)) = (&mut self.metronome, &self.prepared_revision) {
            metronome.mix(
                revision.session().timing(),
                revision.session().meter_map(),
                start,
                self.session_runtime.as_ref().map_or_else(
                    || self.project.loop_region(),
                    |runtime| runtime.active_loop_region(),
                ),
                output,
            );
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.project.sample_rate()
    }

    /// Control-side immutable prepared PCM source facts; these are not page or
    /// native publication authority. Legacy graphs without a complete prepared
    /// revision return an explicit refusal rather than incomplete dependencies.
    pub fn streamed_sources_on_control(
        &self,
    ) -> Result<impl Iterator<Item = &crate::StreamedSourceMetadata>, Invalid> {
        Ok(self
            .prepared_revision
            .as_ref()
            .ok_or(Invalid("prepared source inventory unavailable"))?
            .streamed_sources())
    }
    /// Complete typed dependency inventory for ordinary native publication.
    /// Actual catalog leases are checked by the output's existing domain owner.
    pub fn stream_resources_on_control(
        &self,
    ) -> Result<impl Iterator<Item = crate::StreamResourceMetadata> + '_, Invalid> {
        let revision = self
            .prepared_revision
            .as_ref()
            .ok_or(Invalid("prepared source inventory unavailable"))?;
        Ok(revision
            .streamed_sources()
            .cloned()
            .map(crate::StreamResourceMetadata::Audio)
            .chain(
                revision
                    .scalar_automation_bindings()
                    .map(|binding| crate::StreamResourceMetadata::Scalar(binding.source.clone())),
            ))
    }

    /// Whether any Arrangement or Session source needs the streamed page
    /// provider. Cached by the prepared revision, not rescanned per block.
    pub fn contains_streamed_sources(&self) -> bool {
        self.prepared_revision.as_ref().map_or_else(
            || self.project.contains_streamed_sources(),
            |revision| revision.contains_streamed_sources(),
        )
    }

    /// Arrangement cursor used by the existing native timeline and seek API.
    pub fn position(&self) -> u64 {
        self.session_runtime
            .as_ref()
            .map_or(self.position, |runtime| runtime.arrangement_frame())
    }

    /// Session-owned Arrangement loop state, unavailable for legacy playback.
    pub fn arrangement_loop_state(&self) -> Option<crate::session_playback::ArrangementLoopState> {
        self.session_runtime
            .as_ref()
            .map(|runtime| runtime.arrangement_loop_state())
    }

    /// Frames by which Main output trails the scheduled content frame: the
    /// graph's pre-Main compensation plus declared Main processor latency.
    pub fn main_output_latency_frames(&self) -> u32 {
        self.project.main_latency_frames()
    }

    /// Monotonic render clock, independent of Arrangement loop wrapping.
    pub fn performance_frame(&self) -> u64 {
        self.session_runtime
            .as_ref()
            .map_or(self.performance_frame, |runtime| {
                runtime.performance_frame()
            })
    }

    /// Callback-owned boundary for a prepared quantized seek. The caller keeps
    /// the current graph rendering until this frame, then installs the fresh
    /// preroll candidate using ordinary seek discontinuity semantics.
    pub fn quantized_seek_frame(
        &self,
        quantization: crate::LaunchQuantization,
    ) -> Result<u64, Invalid> {
        self.session_runtime
            .as_ref()
            .ok_or(Invalid("quantized seek requires prepared session timing"))?
            .scheduled_at(quantization)
            .map(|(frame, _)| frame)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    /// Actual graph-local audition loop; independent of the saved loop marker.
    pub fn section_loop(&self) -> Option<hex_project::LoopRegion> {
        self.session_runtime
            .as_ref()
            .and_then(|runtime| runtime.transient_loop)
    }

    /// Configured content-clock law for a control-side automation capture.
    /// Includes a transient section loop and excludes loops during linear export.
    /// A first-pass origin at/after its end still advances linearly, following
    /// the renderer's absolute-seek rule. This read-only fact grants no capture
    /// admission: the caller must fence the exact output, source and transport
    /// epoch through the existing delivered-input endpoint.
    pub fn automation_capture_loop(&self) -> Option<hex_project::LoopRegion> {
        self.session_runtime.as_ref().map_or_else(
            || self.project.loop_region().filter(|region| region.enabled()),
            |runtime| runtime.loop_region(),
        )
    }

    /// True after the legacy monotonic clock reached its representable limit.
    /// Finite Arrangement playback continues, while Session promotion refuses.
    pub fn performance_clock_exhausted(&self) -> bool {
        self.performance_clock_exhausted
    }

    pub fn playing(&self) -> bool {
        self.session_runtime
            .as_ref()
            .map_or(self.playing, |runtime| runtime.playing())
    }

    /// Opaque Session route-state stamp. It changes only when displayed route
    /// state or its musical reanchor changes, and wraps; compare for equality.
    pub fn session_route_state_generation(&self) -> u64 {
        self.session_runtime
            .as_ref()
            .map_or(0, |runtime| runtime.route_state_generation())
    }

    /// Number of Session route entries in project order, or zero when Session
    /// playback is unavailable or this wrapper uses legacy Arrangement playback.
    pub fn session_route_count(&self) -> usize {
        self.session_runtime
            .as_ref()
            .map_or(0, |runtime| runtime.route_count())
    }

    /// Effective Session clock translation for revision-bound route observers.
    pub fn session_clock_anchor(&self) -> Option<crate::SessionClockAnchor> {
        self.session_runtime
            .as_ref()
            .and_then(|runtime| runtime.clock_anchor())
    }

    /// Read one Session route without allocation.
    pub fn session_route_status(&self, index: usize) -> Option<crate::SessionRouteStatus> {
        self.session_runtime
            .as_ref()
            .and_then(|runtime| runtime.route_status(index))
    }

    /// Compatibility transport entry point. Callback code uses `try_apply_action`
    /// so Session query errors are reported through typed rejection telemetry.
    pub fn apply(&mut self, command: PlaybackCommand) {
        let _ = self.try_apply_action(PlaybackAction::Transport(command));
    }

    /// Apply immediately. A pending declicked Pause/Stop completes first.
    pub fn try_apply_action(&mut self, action: PlaybackAction) -> Result<(), PlaybackActionError> {
        self.release_device_insert_cut();
        self.complete_transport_fade();
        self.apply_now(action)
    }

    /// Apply with the transport declick described in `transport_fade`: a
    /// Pause/Stop, Seek or PlayFrom while playing is admitted now and applied
    /// after its fade, a Seek or PlayFrom then ramping in at its target; Play from a
    /// paused, nonzero position ramps in. Static refusals are
    /// reported here. A deferred action that fails at its fade end falls back
    /// to Stop and is reported once by `take_transport_fade_failure`.
    pub fn try_apply_declicked_action(
        &mut self,
        action: PlaybackAction,
    ) -> Result<(), PlaybackActionError> {
        self.release_device_insert_cut();
        self.complete_transport_fade();
        if (transport_fade::fades_out(action) || transport_fade::seeks(action)) && self.playing() {
            self.admit_declicked_stop(action)?;
            self.transport_fade.begin_out(action);
            return Ok(());
        }
        let stopped = !self.playing();
        let resume = transport_fade::resumes(action) && stopped && self.position() != 0;
        self.apply_now(action)?;
        if stopped && self.playing() {
            // Play from frame zero keeps its first transient.
            let frames = if resume {
                transport_fade::fade_frames(self.sample_rate())
            } else {
                0
            };
            self.transport_fade.begin_in(frames);
        }
        Ok(())
    }

    /// Apply a pending declicked Pause/Stop now, at the current frame, so
    /// a host can settle its own transport effects before a later action.
    pub fn finish_transport_fade(&mut self) {
        self.complete_transport_fade();
    }

    /// The Pause/Stop whose fade is in progress, if any.
    pub fn pending_transport_fade(&self) -> Option<PlaybackAction> {
        self.transport_fade.pending()
    }

    /// Begin an output fade for a confirmed graph insert on a playing Session
    /// runtime. The callback owns the decision and must install or release the
    /// cut. Ordinary transport actions and priority Stop supersede it.
    pub fn begin_device_insert_cut(&mut self) -> bool {
        self.session_runtime
            .as_deref()
            .is_some_and(crate::SessionPlayback::playing)
            && self.transport_fade.begin_device_insert_cut()
    }

    /// Frames to the cut if a fresh insert fade starts on this callback.
    pub fn device_insert_fade_frames(&self) -> u32 {
        transport_fade::fade_frames(self.sample_rate())
    }

    pub(crate) fn preview_stream_device_insert(
        &mut self,
        previous: &mut Self,
        fade_remaining: u32,
        maximum_frames: usize,
        ordinary: bool,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<crate::StreamChunkWindow, PlaybackHandoffError> {
        let unavailable = PlaybackHandoffError::RuntimeFenceChanged;
        if previous.device_insert_held() != (fade_remaining == 0) {
            return Err(unavailable);
        }
        let next = self.session_runtime.as_mut().ok_or(unavailable)?;
        let previous = previous.session_runtime.as_mut().ok_or(unavailable)?;
        next.preview_stream_device_insert(previous, fade_remaining, maximum_frames, ordinary, visit)
            .map_err(PlaybackHandoffError::Session)
    }

    /// The output is silent and the Session clock is frozen at the fade end.
    pub fn device_insert_held(&self) -> bool {
        self.transport_fade.device_insert_held()
    }

    /// The callback still owns a fade out or a silent cut for this insert.
    pub fn device_insert_cut_pending(&self) -> bool {
        self.transport_fade.device_insert_cut_pending()
    }

    /// Restore the predecessor after a refusal, or resume the successor after
    /// it has inherited the silent cut. The full output ramps up from the
    /// current gain, without issuing a transport command or resetting history.
    pub fn release_device_insert_cut(&mut self) {
        let frames = transport_fade::fade_frames(self.sample_rate());
        if self.transport_fade.release_device_insert_cut(frames)
            && let Some(runtime) = self.session_runtime.as_deref_mut()
        {
            runtime.set_playing_at_device_insert_cut(true);
        }
    }

    /// One deferred Pause/Stop refusal recorded at its fade end.
    pub fn take_transport_fade_failure(&mut self) -> Option<PlaybackActionRejection> {
        self.transport_fade.take_failure()
    }

    /// The latest deferred Pause/Stop completion and its audible end frame,
    /// reported once. Stop resets the transport position to zero, so this is
    /// the only place its audible end is observable.
    pub fn take_transport_fade_completion(&mut self) -> Option<TransportFadeCompletion> {
        self.transport_fade.take_completion()
    }

    fn admit_declicked_stop(&self, action: PlaybackAction) -> Result<(), PlaybackActionError> {
        if let PlaybackAction::Transport(PlaybackCommand::PlayFrom(frame)) = action {
            return match self.session_runtime.as_deref() {
                Some(runtime) => runtime
                    .admits_play_from(frame)
                    .map_err(PlaybackActionError::Session),
                None => Ok(()),
            };
        }
        let command = match action {
            PlaybackAction::Transport(PlaybackCommand::Pause) => {
                crate::SessionPlaybackCommand::Pause
            }
            PlaybackAction::Transport(PlaybackCommand::Stop) => crate::SessionPlaybackCommand::Stop,
            PlaybackAction::Transport(PlaybackCommand::Seek(frame)) => {
                crate::SessionPlaybackCommand::Seek(frame)
            }
            PlaybackAction::Session(command) => command,
            _ => {
                return Err(PlaybackActionError::Session(
                    crate::SessionPlaybackError::InvalidCommand,
                ));
            }
        };
        match self.session_runtime.as_deref() {
            Some(runtime) => runtime
                .admits_transport(&command)
                .map_err(PlaybackActionError::Session),
            None if matches!(action, PlaybackAction::Session(_)) => {
                Err(match self.session_unavailable {
                    Some(error) => PlaybackActionError::SessionUnavailable(error),
                    None => PlaybackActionError::SessionNotPrepared,
                })
            }
            None => Ok(()),
        }
    }

    /// Apply a pending declicked Pause/Stop now, at the current frame.
    fn complete_transport_fade(&mut self) {
        let Some(action) = self.transport_fade.take_pending() else {
            return;
        };
        let audible_end_frame = self.position();
        let result = self.apply_now(action);
        self.transport_fade
            .record_completion(TransportFadeCompletion {
                action,
                audible_end_frame,
                applied: result.is_ok(),
            });
        match result {
            // Paused tails and monitoring, and the seek target, return
            // without a step.
            Ok(()) if transport_fade::pauses(action) || transport_fade::seeks(action) => self
                .transport_fade
                .begin_in(transport_fade::fade_frames(self.sample_rate())),
            Ok(()) => {}
            Err(error) => {
                self.transport_fade
                    .record_failure(error.telemetry_rejection());
                // The output is already silent; never leave it playing unheard.
                let _ = self.apply_now(PlaybackAction::Transport(PlaybackCommand::Stop));
            }
        }
    }

    fn apply_now(&mut self, action: PlaybackAction) -> Result<(), PlaybackActionError> {
        let result = self.apply_action_inner(action, None);
        if result.is_ok()
            && matches!(
                action,
                PlaybackAction::Transport(
                    PlaybackCommand::Stop
                        | PlaybackCommand::Pause
                        | PlaybackCommand::Seek(_)
                        | PlaybackCommand::PlayFrom(_)
                ) | PlaybackAction::Session(
                    crate::SessionPlaybackCommand::Stop
                        | crate::SessionPlaybackCommand::Pause
                        | crate::SessionPlaybackCommand::Seek(_)
                )
            )
        {
            self.reset_metronome();
        }
        result
    }

    /// Consume the output's independent priority Stop after its control endpoint revoked the
    /// activation prefix. Transport and voice history reset; only an ordinary output-bound
    /// keyboard root survives, so a newer physical epoch needs no graph publication. Direct
    /// `apply(Stop)`, queued Stop, seeks and lifecycle cancellation remain terminal.
    pub fn apply_priority_stop(
        &mut self,
        clock: &crate::computer_keyboard::KeyboardActivationClock,
    ) {
        self.transport_fade.clear();
        if self
            .apply_action_inner(
                PlaybackAction::Transport(PlaybackCommand::Stop),
                Some(clock),
            )
            .is_ok()
        {
            self.reset_metronome();
        }
    }

    fn apply_action_inner(
        &mut self,
        action: PlaybackAction,
        priority_stop: Option<&crate::computer_keyboard::KeyboardActivationClock>,
    ) -> Result<(), PlaybackActionError> {
        if matches!(
            action,
            PlaybackAction::Transport(
                PlaybackCommand::Stop | PlaybackCommand::Seek(_) | PlaybackCommand::PlayFrom(_)
            ) | PlaybackAction::Session(
                crate::SessionPlaybackCommand::Stop | crate::SessionPlaybackCommand::Seek(_)
            )
        ) {
            if let Some(clock) = priority_stop {
                if let Some(runtime) = self.session_runtime.as_mut() {
                    runtime.cancel_current_sampler_keyboard();
                    runtime.cancel_foreign_keyboard(Some(clock));
                    if let Some(keyboard) = runtime.computer_keyboard.as_mut() {
                        keyboard.priority_stop(clock);
                    }
                }
            } else {
                self.cancel_computer_keyboard();
            }
            self.cancel_tuning_audition();
        }
        match action {
            PlaybackAction::SetMetronome(enabled) => {
                if enabled && !self.metronome_supported() {
                    return Err(PlaybackActionError::MetronomeUnavailable);
                }
                if let Some(metronome) = self.metronome.as_mut() {
                    metronome.set_enabled(enabled);
                }
                Ok(())
            }
            PlaybackAction::Transport(command) => {
                if let Some(runtime) = self.session_runtime.as_mut() {
                    let command = match command {
                        PlaybackCommand::Play => crate::SessionPlaybackCommand::Play,
                        PlaybackCommand::PlayFrom(frame) => {
                            return runtime
                                .play_from(frame)
                                .map_err(PlaybackActionError::Session);
                        }
                        PlaybackCommand::Pause => crate::SessionPlaybackCommand::Pause,
                        PlaybackCommand::Stop => crate::SessionPlaybackCommand::Stop,
                        PlaybackCommand::Seek(frame) => crate::SessionPlaybackCommand::Seek(frame),
                    };
                    runtime.apply(command).map_err(PlaybackActionError::Session)
                } else {
                    match command {
                        PlaybackCommand::Play => self.playing = true,
                        PlaybackCommand::PlayFrom(frame) => {
                            if let Some(runtime) = self.linear_runtime.as_mut() {
                                runtime.reset();
                            }
                            self.position = frame;
                            self.performance_frame = frame;
                            self.performance_clock_exhausted = false;
                            self.playing = true;
                        }
                        PlaybackCommand::Pause => self.playing = false,
                        PlaybackCommand::Stop => {
                            self.playing = false;
                            if let Some(runtime) = self.linear_runtime.as_mut() {
                                runtime.reset();
                            }
                            self.position = 0;
                            self.performance_frame = 0;
                            self.performance_clock_exhausted = false;
                        }
                        PlaybackCommand::Seek(frame) => {
                            if let Some(runtime) = self.linear_runtime.as_mut() {
                                runtime.reset();
                            }
                            self.position = frame;
                            self.performance_frame = frame;
                            self.performance_clock_exhausted = false;
                        }
                    }
                    Ok(())
                }
            }
            PlaybackAction::Session(command) => {
                let Some(runtime) = self.session_runtime.as_mut() else {
                    return Err(match self.session_unavailable {
                        Some(error) => PlaybackActionError::SessionUnavailable(error),
                        None => PlaybackActionError::SessionNotPrepared,
                    });
                };
                runtime.apply(command).map_err(PlaybackActionError::Session)
            }
            PlaybackAction::LaunchAndPlay(command) => {
                let Some(runtime) = self.session_runtime.as_mut() else {
                    return Err(match self.session_unavailable {
                        Some(error) => PlaybackActionError::SessionUnavailable(error),
                        None => PlaybackActionError::SessionNotPrepared,
                    });
                };
                if !matches!(
                    command,
                    crate::SessionPlaybackCommand::LaunchClip { .. }
                        | crate::SessionPlaybackCommand::LaunchScene { .. }
                ) {
                    return Err(PlaybackActionError::Session(
                        crate::SessionPlaybackError::InvalidCommand,
                    ));
                }
                runtime
                    .apply(command)
                    .map_err(PlaybackActionError::Session)?;
                runtime
                    .apply(crate::SessionPlaybackCommand::Play)
                    .map_err(PlaybackActionError::Session)
            }
        }
    }

    /// Preserve legacy finite transport behavior. Native publication uses the
    /// fallible companion to carry Session routes and reject unsafe demotions.
    /// A changed tempo map keeps the cursor's Arrangement beat. Refusal
    /// leaves this transport unchanged when the conversion cannot be proven.
    pub fn inherit_transport(&mut self, previous: &Self) -> Result<(), PlaybackHandoffError> {
        let position =
            remapped_linear_position(&self.project, &previous.project, previous.position())?;
        self.position = position;
        self.performance_frame = previous.performance_frame();
        self.performance_clock_exhausted = previous.performance_clock_exhausted();
        self.playing = previous.playing();
        self.inherit_metronome(previous);
        Ok(())
    }

    fn inherit_metronome(&mut self, previous: &Self) {
        if !self.metronome_supported() {
            if let Some(metronome) = self.metronome.as_mut() {
                metronome.set_enabled(false);
            }
            return;
        }
        let same_timing = match (&self.prepared_revision, &previous.prepared_revision) {
            (Some(next), Some(old)) => {
                next.session().timing().same_map(old.session().timing())
                    && next.session().meter_map() == old.session().meter_map()
                    && self.project.loop_region() == previous.project.loop_region()
            }
            _ => false,
        };
        if let (Some(next), Some(old)) = (&mut self.metronome, &previous.metronome) {
            next.inherit(old, same_timing);
        }
    }

    #[cfg(test)]
    pub(crate) fn test_swap_compensation_origins(
        &mut self,
        previous: &mut Self,
        groups: &mut crate::audio_candidate::groups::Transfer<'_>,
    ) -> Result<(usize, Option<usize>), crate::SessionPlaybackError> {
        let next = self
            .session_runtime
            .as_mut()
            .ok_or(crate::SessionPlaybackError::DeviceRuntime)?;
        let old = previous
            .session_runtime
            .as_mut()
            .ok_or(crate::SessionPlaybackError::DeviceRuntime)?;
        next.test_swap_compensation_origins(old, groups)
    }
    pub(crate) fn inherit_candidate_groups(
        &mut self,
        previous: &mut Self,
        groups: crate::audio_candidate::groups::Transfer<'_>,
        confirmed_insert: bool,
    ) -> Result<(), PlaybackHandoffError> {
        if confirmed_insert && !previous.device_insert_held()
            || !confirmed_insert && previous.device_insert_cut_pending()
        {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        let next = self
            .session_runtime
            .as_mut()
            .ok_or(PlaybackHandoffError::ActiveSessionStateCannotBeDropped)?;
        let old = previous
            .session_runtime
            .as_mut()
            .ok_or(PlaybackHandoffError::ActiveSessionStateCannotBeDropped)?;
        next.inherit_candidate_groups(old, groups, confirmed_insert)
            .map_err(PlaybackHandoffError::Session)?;
        self.inherit_metronome(previous);
        if confirmed_insert {
            *self.transport_fade = std::mem::take(&mut *previous.transport_fade);
            self.release_device_insert_cut();
        }
        Ok(())
    }

    /// Inherit graph-local state at a publication boundary without allocating
    /// or retaining ownership of the previous graph.
    pub fn try_inherit_runtime_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<(), PlaybackHandoffError> {
        self.try_inherit_runtime_impl(previous, false, false, false, None)
    }

    /// Mark this prepared insert for the same no-restart law in stream
    /// preflight and callback commit. Only Session graphs support that proof.
    pub fn require_continuous_device_insert(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.require_continuous_insert = true;
        }
    }

    /// Ordinary publication may rebind a fresh monitor to the same physical input client in
    /// the same explicitly known project generation. This carries no voice/DSP history or
    /// source compatibility. Unknown generations (including legacy defaults) authorize nothing.
    /// Selection precedes graph inheritance; old input custody changes only after it succeeds.
    pub fn try_inherit_runtime_in_control_context(
        &mut self,
        previous: &mut Self,
        generation: Option<u64>,
        previous_generation: Option<u64>,
    ) -> Result<(), PlaybackHandoffError> {
        self.try_inherit_runtime_impl(
            previous,
            generation.is_some() && generation == previous_generation,
            false,
            false,
            None,
        )
    }

    /// Use the graph-owned comparison data; the publication owner must validate
    /// the original endpoint and transaction before calling. The data is put
    /// back on every result and remains owned until control reclaims the graph.
    pub fn try_inherit_runtime_with_stored_foreign_continuation(
        &mut self,
        previous: &mut Self,
        generation: Option<u64>,
        previous_generation: Option<u64>,
    ) -> Result<(), PlaybackHandoffError> {
        if generation.is_none() || generation != previous_generation {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        self.try_inherit_runtime_impl(previous, true, false, true, None)
    }

    /// Borrow a control-prepared tempo anchor through the ordinary Playback
    /// lifecycle. Both graphs must have the same known control generation.
    /// The publication owner must validate the output, command,
    /// route, fault and cancellation fence and reclaim the plan on control.
    pub fn try_inherit_runtime_with_tempo_anchor(
        &mut self,
        previous: &mut Self,
        generation: Option<u64>,
        previous_generation: Option<u64>,
        anchor: &crate::session_playback::PreparedTempoAnchor,
    ) -> Result<(), PlaybackHandoffError> {
        if generation.is_none()
            || generation != previous_generation
            || self.session_runtime.is_none()
            || previous.session_runtime.is_none()
        {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        self.try_inherit_runtime_impl(previous, true, false, false, Some(anchor))
    }

    /// Reset an explicitly prepared bounce successor at a verified live
    /// Arrangement fence. The publication owner must validate the fence and
    /// unchanged source context in the callback before invoking this method.
    pub fn try_restart_runtime_under_arrangement_fence(
        &mut self,
        previous: &mut Self,
        generation: Option<u64>,
        previous_generation: Option<u64>,
    ) -> Result<(), PlaybackHandoffError> {
        if generation.is_none()
            || generation != previous_generation
            || !self.live_restart_requested()
            || previous.session_runtime.is_none()
        {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        self.try_inherit_runtime_impl(previous, true, true, false, None)
    }

    fn try_inherit_runtime_impl(
        &mut self,
        previous: &mut Self,
        same_control_context: bool,
        fenced_restart: bool,
        use_stored_continuation: bool,
        tempo_anchor: Option<&crate::session_playback::PreparedTempoAnchor>,
    ) -> Result<(), PlaybackHandoffError> {
        if use_stored_continuation && !self.has_foreign_continuation_states() {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        if use_stored_continuation
            && (fenced_restart
                || tempo_anchor.is_some()
                || self.session_runtime.is_none()
                || previous.session_runtime.is_none())
        {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        if previous
            .session_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.midi_capture_active())
        {
            return Err(PlaybackHandoffError::Session(
                crate::SessionPlaybackError::DeviceRuntime,
            ));
        }
        let rebind_keyboard = same_control_context && self.can_rebind_keyboard_input(previous);
        let continued_audition = match (&self.tuning_audition, &previous.tuning_audition) {
            (Some(next), Some(old)) if next.shares_live_lease(old) => {
                Some(next.continued_synth(old).map_err(|_| {
                    PlaybackHandoffError::Session(crate::SessionPlaybackError::DeviceRuntime)
                })?)
            }
            _ => None,
        };
        let preserve_keyboard = match (
            self.session_runtime
                .as_ref()
                .and_then(|runtime| runtime.computer_keyboard.as_ref()),
            previous
                .session_runtime
                .as_ref()
                .and_then(|runtime| runtime.computer_keyboard.as_ref()),
        ) {
            (Some(next), Some(old)) if next.requests_preservation_from(old) => {
                let exact_revision = self
                    .prepared_revision
                    .as_ref()
                    .zip(previous.prepared_revision.as_ref())
                    .is_some_and(|(next, old)| Arc::ptr_eq(next, old));
                let tuning_revision = next.tuning_preservation_matches(old)
                    && self
                        .prepared_revision
                        .as_ref()
                        .zip(previous.prepared_revision.as_ref())
                        .is_some_and(|(next, old)| next.tuning_family() == old.tuning_family());
                // Input-only source schedulers carry no synthesis history.
                // Their exact live tokens can follow a source publication only
                // after Session's complete source runtime/import proof succeeds.
                let source_revision = same_control_context
                    && next.is_source_input()
                    && old.is_source_input()
                    && self
                        .prepared_revision
                        .as_ref()
                        .zip(previous.prepared_revision.as_ref())
                        .is_some_and(|(next_revision, old_revision)| {
                            let source =
                                |revision: &PreparedRevision, track: hex_project::TrackId| {
                                    revision
                                        .arrangement()
                                        .tracks()
                                        .iter()
                                        .position(|row| row.canonical_track_id() == Some(track))
                                        .and_then(|index| revision.source_routing().source(index))
                                };
                            source(next_revision, next.track())
                                .zip(source(old_revision, old.track()))
                                .is_some_and(|(next, old)| next == old)
                        });
                if (!exact_revision && !tuning_revision && !source_revision)
                    || !next.can_preserve_from(old)
                {
                    return Err(PlaybackHandoffError::Session(
                        crate::SessionPlaybackError::DeviceRuntime,
                    ));
                }
                true
            }
            _ => false,
        };
        let confirmed_insert_cut = previous.transport_fade.device_insert_held();
        match (&mut self.session_runtime, &mut previous.session_runtime) {
            (Some(next), Some(old)) => (if fenced_restart {
                next.try_inherit_state_fenced_restart_from(old)
            } else if use_stored_continuation {
                let Some(states) = next.foreign_continuation.take() else {
                    return Err(PlaybackHandoffError::RuntimeFenceChanged);
                };
                let result = match states.matcher() {
                    Ok(matcher) => {
                        next.try_inherit_state_live_with_foreign_continuation_from(old, &matcher)
                    }
                    Err(_) => Err(crate::SessionPlaybackError::DeviceRuntime),
                };
                next.foreign_continuation = Some(states);
                result
            } else if let Some(anchor) = tempo_anchor {
                next.try_inherit_state_with_tempo_anchor(old, anchor)
            } else if confirmed_insert_cut {
                next.try_inherit_state_at_device_insert_cut(old)
            } else {
                next.try_inherit_state_live_from(old)
            })
            .map(|_| ())
            .map_err(PlaybackHandoffError::Session)?,
            (Some(next), None) => {
                if previous.performance_clock_exhausted {
                    return Err(PlaybackHandoffError::PerformanceClockExhausted);
                }
                let position =
                    remapped_linear_position(&self.project, &previous.project, previous.position)?;
                next.validate_legacy_transport(position, previous.performance_frame)
                    .map_err(PlaybackHandoffError::Session)?;
                if legacy_arrangement_state_cannot_be_seeded(previous) {
                    return Err(PlaybackHandoffError::ArrangementStateCannotBeCaptured);
                }
                next.seed_legacy_transport(position, previous.performance_frame, previous.playing)
                    .map_err(PlaybackHandoffError::Session)?;
            }
            (None, Some(old)) => {
                if self.linear_runtime.is_none()
                    && let Some(error) = self.session_unavailable
                {
                    return Err(PlaybackHandoffError::Session(error));
                }
                if old.has_active_arrangement_voices() {
                    return Err(PlaybackHandoffError::ActiveArrangementStateCannotBeDropped);
                }
                if !old.can_demote_to_arrangement() {
                    return Err(PlaybackHandoffError::ActiveSessionStateCannotBeDropped);
                }
                self.position = remapped_linear_position(
                    &self.project,
                    &previous.project,
                    old.arrangement_frame(),
                )?;
                self.performance_frame = old.performance_frame();
                self.performance_clock_exhausted = false;
                self.playing = old.playing();
            }
            (None, None) => {
                if let (Some(candidate), Some(old)) = (
                    self.linear_runtime.as_ref(),
                    previous.linear_runtime.as_mut(),
                ) && candidate.transition_busy_from(&self.project, old, &previous.project)
                {
                    // Pulse #4558: a paused linear graph renders nothing, so
                    // settle its inaudible transitions instead of deferring
                    // forever; resume ramps in from silence.
                    if !previous.playing {
                        old.settle_idle_transitions_for(
                            &previous.project,
                            candidate,
                            &self.project,
                        );
                    }
                    if previous.playing
                        || candidate.transition_busy_from(&self.project, old, &previous.project)
                    {
                        // Pulse #4178: defer; never refuse into a fresh history.
                        return Err(PlaybackHandoffError::Session(
                            crate::SessionPlaybackError::DeviceTransitionBusy,
                        ));
                    }
                }
                if let (Some(candidate), Some(old)) = (
                    self.linear_runtime.as_ref(),
                    previous.linear_runtime.as_ref(),
                ) && !candidate.can_inherit_from(&self.project, old, &previous.project)
                {
                    return Err(PlaybackHandoffError::Session(
                        crate::SessionPlaybackError::DeviceRuntime,
                    ));
                }
                let position =
                    remapped_linear_position(&self.project, &previous.project, previous.position)?;
                if let (Some(candidate), Some(old)) = (
                    self.linear_runtime.as_mut(),
                    previous.linear_runtime.as_ref(),
                ) {
                    candidate.inherit_from(&self.project, old, &previous.project);
                }
                self.position = position;
                self.performance_frame = previous.performance_frame;
                self.performance_clock_exhausted = previous.performance_clock_exhausted;
                self.playing = previous.playing;
            }
        }
        if same_control_context
            && !fenced_restart
            && let (Some(next), Some(old)) = (&mut self.session_runtime, &previous.session_runtime)
        {
            next.inherit_browser_audition(old);
        }
        self.inherit_metronome(previous);
        if let (Some(continued), Some(next), Some(old)) = (
            continued_audition,
            self.tuning_audition.as_mut(),
            previous.tuning_audition.as_mut(),
        ) {
            next.preserve_from(old, continued);
        }
        previous.cancel_tuning_audition();
        if preserve_keyboard {
            if let (Some(next), Some(old)) = (
                self.session_runtime
                    .as_mut()
                    .and_then(|runtime| runtime.computer_keyboard.as_mut()),
                previous
                    .session_runtime
                    .as_mut()
                    .and_then(|runtime| runtime.computer_keyboard.as_mut()),
            ) {
                next.preserve_from(old).map_err(|_| {
                    PlaybackHandoffError::Session(crate::SessionPlaybackError::DeviceRuntime)
                })?;
            }
        } else {
            if rebind_keyboard {
                previous.detach_rebound_keyboard_input();
            }
            previous.cancel_computer_keyboard();
        }
        // A fade in progress continues on the inheriting graph's transport.
        *self.transport_fade = std::mem::take(&mut *previous.transport_fade);
        Ok(())
    }

    fn can_rebind_keyboard_input(&self, previous: &Self) -> bool {
        if self
            .session_runtime
            .as_ref()
            .zip(previous.session_runtime.as_ref())
            .is_some_and(|(next, old)| next.shares_foreign_keyboard(old))
        {
            return true;
        }
        self.session_runtime
            .as_ref()
            .and_then(|runtime| runtime.computer_keyboard.as_ref())
            .zip(
                previous
                    .session_runtime
                    .as_ref()
                    .and_then(|runtime| runtime.computer_keyboard.as_ref()),
            )
            .is_some_and(|(next, old)| next.can_rebind_input_from(old))
    }
    fn detach_rebound_keyboard_input(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.detach_foreign_keyboard();
            if let Some(keyboard) = runtime.computer_keyboard.as_mut() {
                keyboard.detach_rebound_input();
            }
        }
    }

    /// Infallible input-only retirement at the callback's final accepted-install cut. Fresh
    /// seeks retain their reconstructed history and read held keys with fresh monitor voices.
    /// Call only after all fence/cancellation/refusal decisions, before canceling the old graph.
    pub fn detach_rebound_inputs_from(
        &self,
        previous: &mut Self,
        generation: Option<u64>,
        previous_generation: Option<u64>,
    ) {
        if generation.is_none() || generation != previous_generation {
            return;
        }
        if self.can_rebind_keyboard_input(previous) {
            previous.detach_rebound_keyboard_input();
        }
        if let (Some(next), Some(old)) = (&self.tuning_audition, &mut previous.tuning_audition)
            && next.can_rebind_input_from(old)
        {
            old.detach_rebound_input();
        }
    }

    /// Enable live scalar observations on an already prepared runtime. The
    /// audio publication owner enables this after preparation. Preroll/seek
    /// preparation must leave it disabled: endpoints may also be retained by
    /// applied playback. Enablement grants no publication or delivery authority.
    pub fn set_device_metering(&mut self, enabled: bool) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.set_device_metering(enabled);
        }
    }

    /// Admit capture against the installed engine graph. The caller still owns
    /// output/source/transport authority and must confirm delivered host output.
    pub fn arm_midi_capture(
        &mut self,
        binding: crate::midi_capture::MidiCaptureBinding,
        clock: &crate::computer_keyboard::KeyboardActivationClock,
    ) -> Result<crate::midi_capture::MidiCaptureClock, Invalid> {
        self.session_runtime
            .as_mut()
            .ok_or(Invalid("midi capture session runtime is unavailable"))?
            .arm_midi_capture(binding, clock)
    }
    /// Negative-only current input/fault check for ending a take before Stop
    /// resets playback. Unlike the sealed render getter, this checks live input.
    pub fn midi_capture_finish_fault(
        &self,
        take_id: u64,
    ) -> Option<crate::midi_capture::MidiCaptureFault> {
        self.session_runtime
            .as_ref()?
            .computer_keyboard
            .as_ref()?
            .capture_finish_fault(take_id)
    }
    pub fn midi_capture_fault(
        &self,
        take_id: u64,
    ) -> Option<crate::midi_capture::MidiCaptureFault> {
        self.session_runtime.as_ref()?.midi_capture_fault(take_id)
    }
    pub fn finish_midi_capture(
        &mut self,
        take_id: u64,
    ) -> Result<crate::midi_capture::MidiCaptureClock, Invalid> {
        self.session_runtime
            .as_mut()
            .ok_or(Invalid("midi capture session runtime is unavailable"))?
            .finish_midi_capture(take_id)
    }
    pub fn midi_capture_render(
        &self,
        take_id: u64,
    ) -> Result<Option<crate::midi_capture::MidiCaptureRender<'_>>, Invalid> {
        self.session_runtime
            .as_ref()
            .map_or(Ok(None), |runtime| runtime.midi_capture_render(take_id))
    }

    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        self.render_with_post_node_observer(output, None)
    }

    /// Render a separately prepared paused monitor without mutating the
    /// installed Arrangement or terminating an in-flight transport fade.
    pub fn render_with_paused_track_monitor(
        &self,
        monitor: &mut crate::session_playback::PausedTrackMonitor,
        input: &[Frame],
        output: &mut [Frame],
    ) -> Result<(), Invalid> {
        monitor
            .render(self.paused_monitor_runtime(output)?, input, output)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    /// Render an independent keyboard audition over a settled paused graph.
    pub fn render_paused_keyboard_monitor(
        &self,
        monitor: &mut crate::PausedKeyboardMonitor,
        output: &mut [Frame],
    ) -> Result<(), Invalid> {
        monitor
            .render(self.paused_monitor_runtime(output)?, output)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    /// Advance only the paused monitor's retained audio after input removal.
    /// `false` means this was the final block, which can still contain output.
    pub fn drain_paused_track_monitor(
        &self,
        monitor: &mut crate::session_playback::PausedTrackMonitor,
        output: &mut [Frame],
    ) -> Result<bool, Invalid> {
        monitor
            .drain(self.paused_monitor_runtime(output)?, output)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    /// Whether installing this candidate must wait for the audible private
    /// PCM predecessor's bounded device transition to finish.
    pub fn paused_track_monitor_blocks_replacement(
        &self,
        monitor: &crate::PausedTrackMonitor,
    ) -> bool {
        self.session_runtime
            .as_ref()
            .is_some_and(|runtime| monitor.blocks_graph_replacement(runtime))
    }

    /// Render the paused graph through its separately admitted foreign effect
    /// endpoints. The supplied callback view must belong to the monitor and
    /// preserve the host's ordinary final-format acceptance frontier.
    pub fn render_paused_track_monitor_with_foreign(
        &self,
        monitor: &mut crate::PausedTrackMonitor,
        input: &[Frame],
        output: &mut [Frame],
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        monitor
            .render_with_foreign(self.paused_monitor_runtime(output)?, input, output, foreign)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    /// Render loaned resident notes through the private downstream graph.
    /// The host must route instrument calls to the original worker, effect
    /// calls to the monitor's exact workers, and accept staging only after
    /// final output conversion. On error it retires the monitor on control.
    /// Resume, Stop, seek (even to the same frame), and source replacement
    /// end this activation and require fresh ownership before rendering again.
    pub fn render_paused_source_monitor_with_foreign(
        &mut self,
        monitor: &mut crate::PausedSourceMonitor,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        let settled = self.pending_transport_fade().is_none();
        let result = match self.session_runtime.as_mut() {
            Some(runtime) if settled => monitor
                .render(runtime, output, observer, foreign)
                .map_err(|error| Invalid(error.diagnostic_message())),
            _ => Err(Invalid(
                "paused source monitoring requires a settled session runtime",
            )),
        };
        if result.is_err() {
            foreign.reject_callback();
            output.fill([0.0; 2]);
        } else {
            self.reset_metronome();
            self.mix_tuning_audition(output);
        }
        result
    }

    /// Opt this resident graph into the output-owned paused source route.
    /// Ordinary pause blocks (including the suffix of a transport fade) hold
    /// the Arrangement until a private monitor can render the loaned notes.
    /// Call before publishing the graph on control.
    pub fn defer_hosted_paused_source_monitor_on_control(&mut self) -> bool {
        if self
            .prepared_revision
            .as_deref()
            .is_some_and(crate::PausedSourceMonitor::supports_revision_on_control)
            && let Some(runtime) = self.session_runtime.as_mut()
        {
            runtime.defer_paused_source_monitor = true;
            return true;
        }
        false
    }

    /// Continue the same monitor callback with zero input. The final block
    /// can contain audio even when the engine reports no remaining tail.
    pub fn drain_paused_track_monitor_with_foreign(
        &self,
        monitor: &mut crate::PausedTrackMonitor,
        output: &mut [Frame],
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<bool, Invalid> {
        monitor
            .drain_with_foreign(self.paused_monitor_runtime(output)?, output, foreign)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    fn paused_monitor_runtime(&self, output: &mut [Frame]) -> Result<&SessionPlayback, Invalid> {
        if let Some(runtime) = self.session_runtime.as_ref()
            && self.pending_transport_fade().is_none()
        {
            return Ok(runtime);
        }
        output.fill([0.0; 2]);
        Err(Invalid(
            "paused track monitoring requires a settled session runtime",
        ))
    }

    /// Render one bounded physical callback with a borrowed live track block.
    /// Track input follows transport fades; only timing-only project taps are admitted.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_live_track_audio(
        &mut self,
        output: &mut [Frame],
        track: hex_project::TrackId,
        input: &[Frame],
        pages: Option<&mut dyn MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        self.render_live_audio(
            output, track, input, pages, observer, project, None, foreign,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_live_audio<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        track: hex_project::TrackId,
        input: &[Frame],
        mut pages: Option<&mut P>,
        mut observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        sources: Option<&crate::native_device_resources::current_sampler::CurrentSamplerSources>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        let source_ref = sources.map(crate::native_device_resources::SamplerSourcesRef::Current);
        let valid = self.session_runtime.as_ref().is_some_and(|runtime| {
            runtime
                .validate_live_audio(
                    track,
                    input,
                    output.len(),
                    project.as_ref().is_some_and(|tap| !tap.is_timing_only()),
                    source_ref,
                )
                .is_ok()
        });
        if !valid {
            output.fill([0.0; 2]);
            foreign.reject_callback();
            return Err(Invalid("invalid live track audio request"));
        }
        self.render_declicked(output, |playback, output, offset| {
            let mut shifted = ShiftedForeign {
                inner: &mut *foreign,
                offset,
            };
            let result = playback
                .session_runtime
                .as_mut()
                .ok_or(Invalid(
                    "live track audio requires a prepared session runtime",
                ))
                .and_then(|runtime| {
                    with_part_offset(project.as_deref_mut(), offset, |project| {
                        with_shifted_observer(observer.as_deref_mut(), offset, |observer| {
                            if runtime.playing() {
                                runtime.render_live_audio(
                                    output,
                                    track,
                                    &input[offset..offset + output.len()],
                                    pages.as_deref_mut(),
                                    observer,
                                    project,
                                    source_ref,
                                    &mut shifted,
                                )
                            } else if let Some(sources) = sources {
                                runtime.render_current_sampler_with_foreign(
                                    output,
                                    pages.as_deref_mut(),
                                    observer,
                                    project,
                                    sources,
                                    &mut shifted,
                                )
                            } else {
                                runtime.render_internal_without_live_audio(
                                    output,
                                    pages.as_deref_mut(),
                                    observer,
                                    project,
                                    &mut shifted,
                                )
                            }
                            .map_err(|error| Invalid(error.diagnostic_message()))
                        })
                    })
                });
            if result.is_err() {
                foreign.reject_callback();
                output.fill([0.0; 2]);
            }
            result
        })
    }

    /// The normal canonical renderer with the audio owner's already retained
    /// foreign endpoints. No finite fallback may omit a required native effect.
    /// Staged input is still uncommitted: only the audio owner can accept it
    /// after final host conversion. Monitor audio remains after the project tap.
    pub fn render_with_foreign_processors(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut dyn MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        let mut pages = pages;
        let mut observer = observer;
        let mut project = project;
        self.render_declicked(output, |playback, output, offset| {
            let mut shifted = ShiftedForeign {
                inner: &mut *foreign,
                offset,
            };
            let result = playback
                .session_runtime
                .as_mut()
                .ok_or(Invalid(
                    "foreign effects require a prepared session runtime",
                ))
                .and_then(|runtime| {
                    with_part_offset(project.as_deref_mut(), offset, |project| {
                        with_shifted_observer(observer.as_deref_mut(), offset, |observer| {
                            runtime
                                .render_with_foreign_processors(
                                    output,
                                    pages
                                        .as_mut()
                                        .map(|pages| &mut **pages as &mut dyn MediaPages),
                                    observer,
                                    project,
                                    &mut shifted,
                                )
                                .map_err(|error| Invalid(error.diagnostic_message()))
                        })
                    })
                });
            if result.is_err() {
                foreign.reject_callback();
                output.fill([0.0; 2]);
            }
            result
        })
    }

    /// Render while exposing bounded borrowed output from each processed
    /// native device node. The observer is audio-thread code when used by
    /// `hex-audio`: it must not allocate, block or retain the slice.
    pub fn render_with_post_node_observer(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), Invalid> {
        if let Some(observer) = observer {
            self.render_with_node_signal_observer(
                output,
                Some(&mut |identity, phase, offset, frames| {
                    if phase == crate::NodeSignalPhase::Output {
                        observer(identity, offset, frames);
                    }
                }),
            )
        } else {
            self.render_with_node_signal_observer(output, None)
        }
    }

    /// Render with synchronous input/output taps around each whole native entry.
    /// Input precedes host input trim; output follows effect and output trim.
    /// Observations commit only after render and final caller validation succeed.
    pub fn render_with_node_signal_observer(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<(), Invalid> {
        self.render_with_signal_observers(output, observer, None)
    }

    /// Observe the actual project mix before monitor-only metronome audio.
    /// A fallback without the canonical Session renderer reports Unavailable
    /// through the project tap and preserves its ordinary audio behavior.
    pub fn render_with_signal_observers(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), Invalid> {
        let mut observer = observer;
        let mut project = project;
        self.render_declicked(output, |playback, output, offset| {
            playback.render_signal_part(
                output,
                offset,
                observer.as_deref_mut(),
                project.as_deref_mut(),
            )
        })
    }

    fn render_signal_part(
        &mut self,
        output: &mut [Frame],
        offset: usize,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), Invalid> {
        if let Some(runtime) = self.session_runtime.as_mut() {
            return with_part_offset(project, offset, |project| {
                with_shifted_observer(observer, offset, |observer| {
                    runtime
                        .render_with_signal_observers(output, observer, project)
                        .map_err(|error| Invalid(error.diagnostic_message()))
                })
            });
        }
        if let Some(project) = project.as_mut() {
            project.unavailable();
        }
        if self.session_unavailable.is_some()
            && (self
                .prepared_revision
                .as_ref()
                .is_some_and(|revision| revision.session().has_mutable_instruments())
                || !self.project.main_device_chain.is_empty()
                || self
                    .project
                    .tracks()
                    .iter()
                    .any(|track| track.has_device_chain() || track.has_instrument_source()))
        {
            return Err(Invalid("prepared native playback runtime is unavailable"));
        }
        if self.playing && !output.is_empty() && self.contains_streamed_sources() {
            return Err(Invalid("streamed playback requires a media page provider"));
        }
        let (next_performance, clock_exhausted) = if self.playing {
            match self.performance_frame.checked_add(
                u64::try_from(output.len()).map_err(|_| Invalid("frame count overflow"))?,
            ) {
                Some(frame) if !self.performance_clock_exhausted => (frame, false),
                Some(_) | None => (u64::MAX, true),
            }
        } else {
            (self.performance_frame, self.performance_clock_exhausted)
        };
        self.render_finite_arrangement_with_pages::<dyn MediaPages>(output, None)?;
        self.performance_frame = next_performance;
        self.performance_clock_exhausted = clock_exhausted;
        Ok(())
    }

    pub(crate) fn session_runtime_mut_for_stream_action(
        &mut self,
    ) -> Result<&mut SessionPlayback, PlaybackActionError> {
        self.session_runtime
            .as_deref_mut()
            .ok_or(PlaybackActionError::SessionNotPrepared)
    }
    pub(crate) fn visit_next_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<crate::StreamChunkWindow, Invalid> {
        self.session_runtime
            .as_ref()
            .ok_or(Invalid("candidate session runtime unavailable"))?
            .visit_next_stream_chunk(maximum_frames, playing, visit)
    }
    pub(crate) fn visit_ordinary_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<crate::StreamChunkWindow, Invalid> {
        self.session_runtime
            .as_ref()
            .ok_or(Invalid("ordinary session runtime unavailable"))?
            .visit_ordinary_stream_chunk(maximum_frames, playing, visit)
    }
    pub(crate) fn visit_current_stream_quantum(
        &mut self,
        maximum_frames: usize,
        playing: Option<bool>,
        ordinary: bool,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<usize, Invalid> {
        self.session_runtime
            .as_mut()
            .ok_or(Invalid("session runtime unavailable"))?
            .visit_current_stream_quantum(maximum_frames, playing, ordinary, visit)
    }
    pub(crate) fn visit_current_stream_quantum_at(
        &mut self,
        start_offset: u64,
        maximum_frames: usize,
        playing: Option<bool>,
        ordinary: bool,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<usize, crate::playback::StreamQuantumProjectionError> {
        self.session_runtime
            .as_mut()
            .ok_or(crate::playback::StreamQuantumProjectionError::Projection(
                Invalid("session runtime unavailable"),
            ))?
            .visit_current_stream_quantum_at(start_offset, maximum_frames, playing, ordinary, visit)
    }
    pub(crate) fn preview_stream_handoff_with_report(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
        ordinary: bool,
        fenced_restart: bool,
    ) -> Result<(crate::StreamChunkWindow, crate::SessionHandoffReport), PlaybackHandoffError> {
        self.preview_stream_handoff_with_continuation_and_report(
            previous,
            maximum_frames,
            playing,
            visit,
            ordinary,
            fenced_restart,
            None,
        )
    }
    #[allow(clippy::too_many_arguments)] // Exact borrowed comparison plus the existing preview inputs.
    pub(crate) fn preview_stream_handoff_with_continuation_and_report(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
        ordinary: bool,
        fenced_restart: bool,
        continuation: Option<&hex_lattice::ForeignConfigurationContinuation<'_>>,
    ) -> Result<(crate::StreamChunkWindow, crate::SessionHandoffReport), PlaybackHandoffError> {
        if fenced_restart && !self.live_restart_requested() {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        let invalid = || PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        let next = self.session_runtime.as_mut().ok_or_else(invalid)?;
        let old = previous.session_runtime.as_mut().ok_or_else(invalid)?;
        next.preview_stream_handoff_with_continuation_and_report(
            old,
            maximum_frames,
            playing,
            visit,
            ordinary,
            fenced_restart,
            continuation,
        )
        .map_err(PlaybackHandoffError::Session)
    }
    pub(crate) fn preview_stream_handoff_with_stored_foreign_continuation(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<crate::StreamChunkWindow, PlaybackHandoffError> {
        let invalid = || PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        let next = self.session_runtime.as_mut().ok_or_else(invalid)?;
        let old = previous.session_runtime.as_mut().ok_or_else(invalid)?;
        let Some(states) = next.foreign_continuation.take() else {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        };
        let result = match states.matcher() {
            Ok(matcher) => next
                .preview_stream_handoff_with_continuation_and_report(
                    old,
                    maximum_frames,
                    playing,
                    visit,
                    true,
                    false,
                    Some(&matcher),
                )
                .map(|(window, _)| window)
                .map_err(PlaybackHandoffError::Session),
            Err(_) => Err(PlaybackHandoffError::RuntimeFenceChanged),
        };
        next.foreign_continuation = Some(states);
        result
    }

    /// Render through the same Arrangement loop segmentation or mixed Session
    /// route kernel while borrowing streamed pages from the caller.
    pub fn render_with_pages<P: MediaPages>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), Invalid> {
        self.render_declicked(output, |playback, output, _| {
            if let Some(runtime) = playback.session_runtime.as_mut() {
                return runtime
                    .render_with_pages(output, pages)
                    .map_err(|error| Invalid(error.diagnostic_message()));
            }
            playback.render_pages_fallback(output, pages)
        })
    }

    /// Borrow pages and optional post-node observation for one block. Neither
    /// borrow is retained; the observer must not allocate, block, clone owners
    /// or retain samples. Observations commit only after successful block and
    /// final output validation. Observed rendering requires a Session runtime;
    /// refusal leaves output, clocks, page provider and playback state unchanged.
    /// With no observer, the existing finite page-rendering fallback is retained.
    pub fn render_with_pages_and_post_node_observer<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), Invalid> {
        if let Some(observer) = observer {
            self.render_with_pages_and_node_signal_observer(
                output,
                pages,
                Some(&mut |identity, phase, offset, frames| {
                    if phase == crate::NodeSignalPhase::Output {
                        observer(identity, offset, frames);
                    }
                }),
            )
        } else {
            self.render_with_pages_and_node_signal_observer(output, pages, None)
        }
    }

    /// Render with synchronous input/output taps around each whole native entry.
    /// Input precedes host input trim; output follows effect and output trim.
    /// Observations commit only after render and final caller validation succeed.
    pub fn render_with_pages_and_node_signal_observer<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<(), Invalid> {
        self.render_with_pages_and_signal_observers(output, pages, observer, None)
    }

    /// Page-backed project/device observations from one actual render pass.
    pub fn render_with_pages_and_signal_observers<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), Invalid> {
        let mut observer = observer;
        let mut project = project;
        self.render_declicked(output, |playback, output, offset| {
            let Some(runtime) = playback.session_runtime.as_mut() else {
                if let Some(project) = project.as_deref_mut() {
                    project.unavailable();
                }
                if observer.is_some() {
                    return Err(Invalid(
                        "post-node observation requires a prepared session runtime",
                    ));
                }
                return playback.render_pages_fallback(output, pages);
            };
            with_part_offset(project.as_deref_mut(), offset, |project| {
                with_shifted_observer(observer.as_deref_mut(), offset, |observer| {
                    runtime
                        .render_with_pages_and_signal_observers(output, pages, observer, project)
                        .map_err(|error| Invalid(error.diagnostic_message()))
                })
            })
        })
    }

    /// Bounded current canonical identity check, intended once per selection or
    /// active revision change rather than once per captured sample.
    pub fn project_output_supported(&self, tracks: &[u64]) -> bool {
        self.session_runtime.is_some() && self.project.project_track_ids.contains(tracks, || {})
    }

    /// Check selected sources against the current canonical render graph.
    pub fn project_sample_sources_supported(
        &self,
        tracks: &[u64],
        returns: &[crate::ReturnId],
    ) -> bool {
        self.project_output_supported(tracks)
            && returns.len() <= crate::MAX_RETURN_TRACKS
            && returns
                .iter()
                .all(|id| self.project.return_buses().iter().any(|bus| bus.id == *id))
    }

    fn render_pages_fallback<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), Invalid> {
        if self.session_unavailable.is_some()
            && (self
                .prepared_revision
                .as_ref()
                .is_some_and(|revision| revision.session().has_mutable_instruments())
                || !self.project.main_device_chain.is_empty()
                || self
                    .project
                    .tracks()
                    .iter()
                    .any(|track| track.has_device_chain() || track.has_instrument_source()))
        {
            return Err(Invalid("prepared native playback runtime is unavailable"));
        }
        let (next_performance, clock_exhausted) = if self.playing {
            match self.performance_frame.checked_add(
                u64::try_from(output.len()).map_err(|_| Invalid("frame count overflow"))?,
            ) {
                Some(frame) if !self.performance_clock_exhausted => (frame, false),
                Some(_) | None => (u64::MAX, true),
            }
        } else {
            (self.performance_frame, self.performance_clock_exhausted)
        };
        self.render_finite_arrangement_with_pages(output, Some(pages))?;
        self.performance_frame = next_performance;
        self.performance_clock_exhausted = clock_exhausted;
        Ok(())
    }

    /// Render one callback as parts split at a pending fade end.
    /// Each part is followed by its declick gain and then monitor-only metronome
    /// and tuning audio. A refused first part keeps the renderer's own atomic
    /// refusal (the output is left as the renderer left it). A part refused
    /// after an earlier part rendered silences the whole callback; rendered
    /// parts keep their committed clock, as consecutive callbacks would.
    /// Without a pending fade end this is exactly one part.
    fn render_declicked(
        &mut self,
        output: &mut [Frame],
        mut part: impl FnMut(&mut Self, &mut [Frame], usize) -> Result<(), Invalid>,
    ) -> Result<(), Invalid> {
        let frames = transport_fade::fade_frames(self.sample_rate());
        let mut offset = 0;
        loop {
            if self.transport_fade.device_insert_held() {
                output[offset..].fill([0.0; 2]);
                return Ok(());
            }
            let start = self.position();
            let playing = self.playing();
            let part_len = self.transport_fade.part_len(output.len() - offset, frames);
            let end = offset + part_len;
            let chunk = &mut output[offset..end];
            if let Err(error) = part(self, chunk, offset) {
                if offset > 0 {
                    output.fill([0.0; 2]);
                }
                return Err(error);
            }
            let completed = self.transport_fade.apply(chunk, frames);
            self.mix_metronome(start, playing, chunk);
            self.mix_tuning_audition(chunk);
            if completed {
                self.complete_transport_fade();
            }
            if self.transport_fade.apply_device_insert_cut(chunk, frames)
                && let Some(runtime) = self.session_runtime.as_deref_mut()
            {
                runtime.set_playing_at_device_insert_cut(false);
            }
            offset = end;
            if offset >= output.len() {
                return Ok(());
            }
        }
    }

    fn render_finite_arrangement_with_pages<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        let Some(region) = self
            .project
            .loop_region()
            .filter(|region| region.enabled() && self.position < region.end())
        else {
            return self.render_linear_with_pages(output, pages);
        };
        output.fill([0.0; 2]);
        if !self.playing || output.is_empty() {
            return Ok(());
        }

        let start = region.start();
        let end = region.end();
        let length = end - start;
        let mut output_offset = 0_usize;
        while output_offset < output.len() {
            if self.position == end {
                self.position = start;
            }
            let remaining = output.len() - output_offset;
            if self.position == start
                && let Ok(period_length) = usize::try_from(length)
            {
                let stateful = self
                    .linear_runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.has_stateful_processors());
                if period_length > 0 && period_length <= remaining && !stateful {
                    let period_end = output_offset + period_length;
                    self.render_linear_with_pages(
                        &mut output[output_offset..period_end],
                        pages.as_deref_mut(),
                    )?;
                    self.position = start;

                    let mut filled = period_length;
                    while filled < remaining {
                        let copy_length = filled.min(remaining - filled);
                        let source_start = output_offset;
                        let source_end = source_start + copy_length;
                        let destination = output_offset + filled;
                        output.copy_within(source_start..source_end, destination);
                        filled += copy_length;
                    }
                    let offset = u64::try_from(remaining % period_length)
                        .map_err(|_| Invalid("loop offset overflow"))?;
                    self.position = start + offset;
                    output_offset = output.len();
                    continue;
                }
            }
            let span_length = if self.position < start {
                remaining.min(usize::try_from(start - self.position).unwrap_or(usize::MAX))
            } else {
                remaining.min(usize::try_from(end - self.position).unwrap_or(usize::MAX))
            };
            if span_length == 0 {
                self.position = start;
                continue;
            }
            let span_end = output_offset + span_length;
            self.render_linear_with_pages(
                &mut output[output_offset..span_end],
                pages.as_deref_mut(),
            )?;
            output_offset = span_end;
            if self.position == end {
                self.position = start;
            }
        }
        Ok(())
    }

    fn render_linear_with_pages<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        let runtime = self
            .linear_runtime
            .as_mut()
            .ok_or(Invalid("linear renderer runtime is unavailable"))?;
        if let Some(pages) = pages {
            runtime.render_with_pages(
                &self.project,
                &mut self.position,
                self.playing,
                output,
                Some(pages),
            )
        } else {
            runtime.render_without_pages(&self.project, &mut self.position, self.playing, output)
        }
    }
}

/// Report a render part's node signals at their callback-relative offset.
fn with_shifted_observer<R>(
    observer: Option<&mut crate::NodeSignalObserver<'_>>,
    offset: usize,
    render: impl FnOnce(Option<&mut crate::NodeSignalObserver<'_>>) -> R,
) -> R {
    match observer {
        Some(inner) if offset != 0 => {
            let mut shifted = |identity: crate::PostNodeIdentity,
                               phase: crate::NodeSignalPhase,
                               at: usize,
                               frames: &[Frame]| {
                inner(identity, phase, offset.saturating_add(at), frames);
            };
            render(Some(&mut shifted))
        }
        other => render(other),
    }
}

/// Report a render part's project signals at their callback-relative offset.
fn with_part_offset<'a, 'observer, R>(
    project: Option<&mut crate::ProjectOutputTap<'a, 'observer>>,
    offset: usize,
    render: impl FnOnce(Option<&mut crate::ProjectOutputTap<'a, 'observer>>) -> R,
) -> R {
    match project {
        Some(tap) => {
            let previous = tap.part_offset();
            tap.replace_part_offset(previous.saturating_add(offset));
            let result = render(Some(&mut *tap));
            tap.replace_part_offset(previous);
            result
        }
        None => render(None),
    }
}

/// Foreign endpoints stage one physical callback; a declicked render part
/// reports its segments at their callback-relative offset.
struct ShiftedForeign<'a> {
    inner: &'a mut dyn hex_lattice::ForeignProcessors,
    offset: usize,
}

impl hex_lattice::ForeignProcessors for ShiftedForeign<'_> {
    fn returns_post_trim(&self, node: hex_lattice::ForeignProcessorNode<'_>) -> bool {
        self.inner.returns_post_trim(node)
    }
    fn process_segment(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.inner.process_segment(
            node,
            self.offset.saturating_add(frame_offset),
            frames,
            clock,
        )
    }
    fn process_segment_with_parameters(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [Frame],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.process_segment_with_notes(node, frame_offset, frames, parameters, &[], clock)
    }
    fn process_segment_with_notes(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [Frame],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        notes: &[hex_lattice::ForeignNote],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.inner.process_segment_with_notes(
            node,
            self.offset.saturating_add(frame_offset),
            frames,
            parameters,
            notes,
            clock,
        )
    }
    fn process_segment_with_modulation(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [Frame],
        lane: hex_lattice::ForeignModulationSamples<'_>,
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        let Some(offset) = self.offset.checked_add(frame_offset) else {
            self.inner.reject_callback();
            frames.fill([0.0; 2]);
            return Err(hex_lattice_contract::DeviceContractError::ForeignProcessingFailed);
        };
        self.inner
            .process_segment_with_modulation(node, offset, frames, lane, clock)
    }
    fn committed_tail_is_drained(&self, node: hex_lattice::ForeignProcessorNode<'_>) -> bool {
        self.inner.committed_tail_is_drained(node)
    }
    fn reject_callback(&mut self) {
        self.inner.reject_callback();
    }
}

/// The legacy finite cursor at the donor cursor's Arrangement beat when the
/// tempo map changed (Pulse #4118, #4135); an unchanged map keeps the frame.
/// Every legacy cursor handoff (linear, promotion, demotion and transport
/// copy) uses it. Frame zero remains beat zero even without timing metadata. A cursor
/// inside the donor's active loop that remaps at or after the candidate loop
/// end wraps by whole loop lengths, as the finite loop renderer does. Linear
/// MIDI renders statelessly from the cursor, so a held note continues as the
/// candidate's occurrence at that beat and ends at its remapped gate.
fn remapped_linear_position(
    next: &Project,
    previous: &Project,
    position: u64,
) -> Result<u64, PlaybackHandoffError> {
    use crate::MidiTiming as _;
    let conversion = || PlaybackHandoffError::Session(crate::SessionPlaybackError::TempoConversion);
    if position == 0 || next.tempo_map == previous.tempo_map {
        return Ok(position);
    }
    let (Some(new_timing), Some(old_timing)) =
        (next.midi_volume_timing(), previous.midi_volume_timing())
    else {
        return Err(conversion());
    };
    let beat = old_timing
        .frame_to_beat(position)
        .map_err(|_| conversion())?;
    let frame = new_timing.beat_to_frame(beat).map_err(|_| conversion())?;
    let looped = previous
        .loop_region()
        .is_some_and(|region| region.enabled() && position < region.end());
    let Some(region) = next
        .loop_region()
        .filter(|region| looped && region.enabled() && frame >= region.end())
    else {
        return Ok(frame);
    };
    let length = region
        .end()
        .checked_sub(region.start())
        .filter(|length| *length > 0)
        .ok_or_else(conversion)?;
    let offset = frame.checked_sub(region.start()).ok_or_else(conversion)?;
    Ok(region.start() + offset % length)
}

fn legacy_arrangement_state_cannot_be_seeded(previous: &Playback) -> bool {
    if previous.performance_frame != 0
        && (!previous.project.main_device_chain.is_empty()
            || previous
                .project
                .tracks()
                .iter()
                .any(crate::Track::has_device_chain))
    {
        return true;
    }
    for track in previous.project.tracks() {
        if !track.instrument_enabled() {
            continue;
        }
        let mut active = false;
        if track
            .for_each_active_midi_span(previous.position, |_, _| active = true)
            .is_err()
            || active
        {
            return true;
        }
    }
    false
}
