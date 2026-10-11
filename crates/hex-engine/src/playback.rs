//! Audio-owned playback of an immutable prepared project revision.
//! Construction and destruction belong outside the callback.
use crate::{
    Frame, Invalid, MediaPages, PreparedRevision, Project, RendererRuntime, SessionPlayback,
};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackCommand {
    Play,
    /// Seek and start in one callback action, resetting the same state as Seek.
    PlayFrom(u64),
    Pause,
    Stop,
    Seek(u64),
}

/// Fixed-size action passed through the native callback command ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackAction {
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
            _ => None,
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
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
        }
    }
}

impl PlaybackActionError {
    /// Compact stable category for callback-to-control telemetry.
    pub const fn telemetry_rejection(self) -> PlaybackActionRejection {
        match self {
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
}

impl PlaybackHandoffRejection {
    pub const fn from_error(error: PlaybackHandoffError) -> Self {
        match error {
            PlaybackHandoffError::Session(crate::SessionPlaybackError::CandidateAlreadyUsed) => {
                Self::SessionRuntime
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
            Self::FreshRuntimeCancelled => "fresh playback was cancelled before it became active",
        }
    }
}

impl PlaybackHandoffError {
    pub const fn is_transient(self) -> bool {
        matches!(
            self,
            Self::Session(crate::SessionPlaybackError::ReleaseBankFull)
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

/// Playback graph, with the legacy finite renderer kept as the compatibility path.
pub struct Playback {
    project: Arc<Project>,
    prepared_revision: Option<Arc<PreparedRevision>>,
    session_runtime: Option<Box<SessionPlayback>>,
    linear_runtime: Option<Box<RendererRuntime>>,
    session_unavailable: Option<crate::SessionPlaybackError>,
    position: u64,
    performance_frame: u64,
    performance_clock_exhausted: bool,
    playing: bool,
}
impl Playback {
    pub fn new(project: Project) -> Self {
        Self::from_shared(Arc::new(project))
    }

    /// Share a control-side compiled project without recompiling or copying its graph.
    /// This constructor deliberately keeps legacy Arrangement-only behavior.
    pub fn from_shared(project: Arc<Project>) -> Self {
        let linear_runtime = Some(Box::new(RendererRuntime::new(&project)));
        Self {
            project,
            prepared_revision: None,
            session_runtime: None,
            linear_runtime,
            session_unavailable: None,
            position: 0,
            performance_frame: 0,
            performance_clock_exhausted: false,
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
        let position = runtime.arrangement_frame();
        let performance_frame = runtime.performance_frame();
        let playing = runtime.playing();
        Self {
            project,
            prepared_revision: Some(prepared_revision),
            session_runtime: Some(Box::new(runtime)),
            linear_runtime: None,
            session_unavailable: None,
            position,
            performance_frame,
            performance_clock_exhausted: false,
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
        let refuses_unowned_prepared_fallback = session_runtime.is_none()
            && session_unavailable.is_some()
            && (prepared_revision.session().has_mutable_instruments()
                || project
                    .tracks()
                    .iter()
                    .any(|track| track.has_stateful_devices() || track.has_instrument_source()));
        let linear_runtime = (session_runtime.is_none() && !refuses_unowned_prepared_fallback)
            .then(|| Box::new(RendererRuntime::new(&project)));
        Self {
            project,
            prepared_revision: Some(prepared_revision),
            session_runtime,
            linear_runtime,
            session_unavailable,
            position: 0,
            performance_frame: 0,
            performance_clock_exhausted: false,
            playing: false,
        }
    }

    pub fn prepared_revision(&self) -> Option<&Arc<PreparedRevision>> {
        self.prepared_revision.as_ref()
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

    pub fn sample_rate(&self) -> u32 {
        self.project.sample_rate()
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

    /// Monotonic render clock, independent of Arrangement loop wrapping.
    pub fn performance_frame(&self) -> u64 {
        self.session_runtime
            .as_ref()
            .map_or(self.performance_frame, |runtime| {
                runtime.performance_frame()
            })
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

    pub fn try_apply_action(&mut self, action: PlaybackAction) -> Result<(), PlaybackActionError> {
        match action {
            PlaybackAction::Transport(command) => {
                if let Some(runtime) = self.session_runtime.as_mut() {
                    let command = match command {
                        PlaybackCommand::Play => crate::SessionPlaybackCommand::Play,
                        PlaybackCommand::PlayFrom(frame) => {
                            runtime
                                .apply(crate::SessionPlaybackCommand::Seek(frame))
                                .map_err(PlaybackActionError::Session)?;
                            runtime
                                .apply(crate::SessionPlaybackCommand::Play)
                                .map_err(PlaybackActionError::Session)?;
                            return Ok(());
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
    pub fn inherit_transport(&mut self, previous: &Self) {
        self.position = previous.position();
        self.performance_frame = previous.performance_frame();
        self.performance_clock_exhausted = previous.performance_clock_exhausted();
        self.playing = previous.playing();
    }

    /// Inherit graph-local state at a publication boundary without allocating
    /// or retaining ownership of the previous graph.
    pub fn try_inherit_runtime_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<(), PlaybackHandoffError> {
        match (&mut self.session_runtime, &mut previous.session_runtime) {
            (Some(next), Some(old)) => next
                .try_inherit_state_from(old)
                .map(|_| ())
                .map_err(PlaybackHandoffError::Session)?,
            (Some(next), None) => {
                if previous.performance_clock_exhausted {
                    return Err(PlaybackHandoffError::PerformanceClockExhausted);
                }
                next.validate_legacy_transport(previous.performance_frame)
                    .map_err(PlaybackHandoffError::Session)?;
                if legacy_arrangement_state_cannot_be_seeded(previous) {
                    return Err(PlaybackHandoffError::ArrangementStateCannotBeCaptured);
                }
                next.seed_legacy_transport(
                    previous.position,
                    previous.performance_frame,
                    previous.playing,
                )
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
                self.position = old.arrangement_frame();
                self.performance_frame = old.performance_frame();
                self.performance_clock_exhausted = false;
                self.playing = old.playing();
            }
            (None, None) => {
                if let (Some(candidate), Some(old)) = (
                    self.linear_runtime.as_ref(),
                    previous.linear_runtime.as_ref(),
                ) && !candidate.can_inherit_from(&self.project, old, &previous.project)
                {
                    return Err(PlaybackHandoffError::Session(
                        crate::SessionPlaybackError::DeviceRuntime,
                    ));
                }
                if let (Some(candidate), Some(old)) = (
                    self.linear_runtime.as_mut(),
                    previous.linear_runtime.as_ref(),
                ) {
                    candidate.inherit_from(&self.project, old, &previous.project);
                }
                self.position = previous.position;
                self.performance_frame = previous.performance_frame;
                self.performance_clock_exhausted = previous.performance_clock_exhausted;
                self.playing = previous.playing;
            }
        }
        Ok(())
    }

    /// Render either the selected mixed source or the legacy finite Arrangement.
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        self.render_with_post_node_observer(output, None)
    }

    /// Render while exposing bounded borrowed output from each processed
    /// native device node. The observer is audio-thread code when used by
    /// `hex-audio`: it must not allocate, block or retain the slice.
    pub fn render_with_post_node_observer(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), Invalid> {
        if let Some(runtime) = self.session_runtime.as_mut() {
            return runtime
                .render_with_post_node_observer(output, observer)
                .map_err(|_| Invalid("session playback render failed"));
        }
        if self.session_unavailable.is_some()
            && (self
                .prepared_revision
                .as_ref()
                .is_some_and(|revision| revision.session().has_mutable_instruments())
                || self
                    .project
                    .tracks()
                    .iter()
                    .any(|track| track.has_stateful_devices() || track.has_instrument_source()))
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

    /// Render through the same Arrangement loop segmentation or mixed Session
    /// route kernel while borrowing streamed pages from the caller.
    pub fn render_with_pages<P: MediaPages>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), Invalid> {
        if let Some(runtime) = self.session_runtime.as_mut() {
            return runtime
                .render_with_pages(output, pages)
                .map_err(|_| Invalid("session playback render failed"));
        }
        if self.session_unavailable.is_some()
            && (self
                .prepared_revision
                .as_ref()
                .is_some_and(|revision| revision.session().has_mutable_instruments())
                || self
                    .project
                    .tracks()
                    .iter()
                    .any(|track| track.has_stateful_devices() || track.has_instrument_source()))
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

    fn render_finite_arrangement_with_pages<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        let Some(region) = self.project.loop_region().filter(|region| region.enabled()) else {
            return self.render_linear_with_pages(output, pages);
        };
        output.fill([0.0; 2]);
        if !self.playing || output.is_empty() {
            return Ok(());
        }

        let start = region.start();
        let end = region.end();
        let length = end - start;
        if self.position >= end {
            self.position = start + (self.position - start) % length;
        }

        let mut output_offset = 0_usize;
        while output_offset < output.len() {
            if self.position == end {
                self.position = start;
            }
            let remaining = output.len() - output_offset;
            if self.position == start {
                if let Ok(period_length) = usize::try_from(length) {
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

fn legacy_arrangement_state_cannot_be_seeded(previous: &Playback) -> bool {
    if previous.performance_frame != 0
        && previous
            .project
            .tracks()
            .iter()
            .any(crate::Track::has_stateful_devices)
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
