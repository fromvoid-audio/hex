//! Audio-owned playback of an immutable prepared project revision.
//! Construction and destruction belong outside the callback.
use crate::{
    Frame, Invalid, MediaPages, PreparedRevision, Project, RendererRuntime, SessionPlayback,
};
use std::sync::Arc;

mod candidate_storage;
mod metronome;
mod tuning_audition;
use metronome::Metronome;

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
            _ => None,
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
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
            8 => Some(Self::ConstructorClaimsUnavailable),
            9 => Some(Self::StreamedPagesNotReady),
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
    metronome: Option<Box<Metronome>>,
    tuning_audition: Option<Box<crate::tuning_audition::Runtime>>,
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
    /// Conservative control-side PCM capacity of this actual immutable graph.
    /// Shared clip leaves are counted once within the graph; no owner escapes.
    pub fn resident_pcm_capacity_frames(&self) -> Option<u64> {
        let sources = || {
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
        };
        sources().enumerate().try_fold(0u64, |n, (i, source)| {
            if sources().take(i).any(|old| old.shares_pcm_with(source)) {
                return Some(n);
            }
            n.checked_add(u64::try_from(source.pcm_capacity_frames()).ok()?)
        })
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
            || compiled.instrument_state.as_ref() != Some(canonical.state())
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
        let canonical = project
            .tracks()
            .iter()
            .find(|candidate| candidate.id() == track)
            .and_then(hex_project::Track::instrument)
            .ok_or(Invalid("computer keyboard track is unavailable"))?;
        let compiled = self
            .project
            .tracks()
            .iter()
            .find(|candidate| candidate.canonical_track_id() == Some(track))
            .ok_or(Invalid("computer keyboard track is unavailable"))?;
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
        if !runtime.computer_keyboard_latency_supported() {
            return Err(Invalid(
                "computer keyboard monitoring is unavailable with positive latency devices",
            ));
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
        let mut candidate = Box::new(if let Some(hint) = hint {
            crate::computer_keyboard::ComputerKeyboardRuntime::prepare_for_tuning(
                project, track, input, family, hint,
            )?
        } else {
            crate::computer_keyboard::ComputerKeyboardRuntime::prepare(project, track, input)?
        });
        candidate.set_tuning_family(family);
        let previous = runtime.computer_keyboard.replace(candidate);
        if runtime.validate_candidate_runtime_storage(0).is_err() {
            runtime.computer_keyboard = previous;
            return Err(Invalid("computer keyboard exceeds graph storage budget"));
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
        let start = self.position();
        let playing = self.playing();
        self.session_runtime
            .as_mut()
            .ok_or(Invalid("current sampler runtime missing"))?
            .render_current_sampler(output, pages, observer, project, sources)
            .map_err(|e| Invalid(e.diagnostic_message()))?;
        self.mix_metronome(start, playing, output);
        self.mix_tuning_audition(output);
        Ok(())
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
            session_runtime,
            linear_runtime,
            session_unavailable: _,
            position: _,
            performance_frame: _,
            performance_clock_exhausted: _,
            playing: _,
        } = self;
        if tuning_audition.is_some() {
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
        let metronome = (!runtime.is_linear_arrangement_export())
            .then(|| Metronome::prepare(project.sample_rate()).ok().map(Box::new))
            .flatten();
        let position = runtime.arrangement_frame();
        let performance_frame = runtime.performance_frame();
        let playing = runtime.playing();
        Self {
            metronome,
            tuning_audition: None,
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
        let metronome = Metronome::prepare(project.sample_rate()).ok().map(Box::new);
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
                self.project.loop_region(),
                output,
            );
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
        let result = self.apply_action_inner(action);
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

    fn apply_action_inner(&mut self, action: PlaybackAction) -> Result<(), PlaybackActionError> {
        if matches!(
            action,
            PlaybackAction::Transport(
                PlaybackCommand::Stop | PlaybackCommand::Seek(_) | PlaybackCommand::PlayFrom(_)
            ) | PlaybackAction::Session(
                crate::SessionPlaybackCommand::Stop | crate::SessionPlaybackCommand::Seek(_)
            )
        ) {
            self.cancel_computer_keyboard();
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
        self.inherit_metronome(previous);
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

    pub(crate) fn inherit_candidate_groups(
        &mut self,
        previous: &mut Self,
        groups: crate::audio_candidate::groups::Transfer<'_>,
    ) -> Result<(), PlaybackHandoffError> {
        let next = self
            .session_runtime
            .as_mut()
            .ok_or(PlaybackHandoffError::ActiveSessionStateCannotBeDropped)?;
        let old = previous
            .session_runtime
            .as_mut()
            .ok_or(PlaybackHandoffError::ActiveSessionStateCannotBeDropped)?;
        next.inherit_candidate_groups(old, groups)
            .map_err(PlaybackHandoffError::Session)?;
        self.inherit_metronome(previous);
        Ok(())
    }

    /// Inherit graph-local state at a publication boundary without allocating
    /// or retaining ownership of the previous graph.
    pub fn try_inherit_runtime_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<(), PlaybackHandoffError> {
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
                if (!exact_revision && !tuning_revision) || !next.can_preserve_from(old) {
                    return Err(PlaybackHandoffError::Session(
                        crate::SessionPlaybackError::DeviceRuntime,
                    ));
                }
                true
            }
            _ => false,
        };
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
            previous.cancel_computer_keyboard();
        }
        Ok(())
    }

    /// Render either the selected mixed source or the legacy finite Arrangement.
    pub fn set_device_metering(&mut self, enabled: bool) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.set_device_metering(enabled);
        }
    }

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
        mut project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), Invalid> {
        let start = self.position();
        let playing = self.playing();
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime
                .render_with_signal_observers(output, observer, project)
                .map_err(|error| Invalid(error.diagnostic_message()))?;
            self.mix_metronome(start, playing, output);
            self.mix_tuning_audition(output);
            return Ok(());
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
        self.mix_metronome(start, playing, output);
        self.mix_tuning_audition(output);
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
        visit: &mut impl FnMut(crate::StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<crate::StreamChunkWindow, Invalid> {
        self.session_runtime
            .as_ref()
            .ok_or(Invalid("candidate session runtime unavailable"))?
            .visit_next_stream_chunk(maximum_frames, playing, visit)
    }
    pub(crate) fn preview_stream_handoff(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<crate::StreamChunkWindow, PlaybackHandoffError> {
        let invalid = || PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        let next = self.session_runtime.as_mut().ok_or_else(invalid)?;
        let old = previous.session_runtime.as_mut().ok_or_else(invalid)?;
        next.preview_stream_handoff(old, maximum_frames, playing, visit)
            .map_err(PlaybackHandoffError::Session)
    }
    /// Render through the same Arrangement loop segmentation or mixed Session
    /// route kernel while borrowing streamed pages from the caller.
    pub fn render_with_pages<P: MediaPages>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), Invalid> {
        let start = self.position();
        let playing = self.playing();
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime
                .render_with_pages(output, pages)
                .map_err(|error| Invalid(error.diagnostic_message()))?;
            self.mix_metronome(start, playing, output);
            self.mix_tuning_audition(output);
            return Ok(());
        }
        self.render_pages_fallback(output, pages)
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
        mut project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), Invalid> {
        let start = self.position();
        let playing = self.playing();
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime
                .render_with_pages_and_signal_observers(output, pages, observer, project)
                .map_err(|error| Invalid(error.diagnostic_message()))?;
            self.mix_metronome(start, playing, output);
            self.mix_tuning_audition(output);
            return Ok(());
        }
        if let Some(project) = project.as_mut() {
            project.unavailable();
        }
        if observer.is_some() {
            return Err(Invalid(
                "post-node observation requires a prepared session runtime",
            ));
        }
        self.render_pages_fallback(output, pages)
    }

    /// Bounded current canonical identity check, intended once per selection or
    /// active revision change rather than once per captured sample.
    pub fn project_output_supported(&self, tracks: &[u64]) -> bool {
        self.session_runtime.is_some() && self.project.project_track_ids.contains(tracks, || {})
    }

    fn render_pages_fallback<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), Invalid> {
        let start = self.position();
        let playing = self.playing();
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
        self.mix_metronome(start, playing, output);
        self.mix_tuning_audition(output);
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
