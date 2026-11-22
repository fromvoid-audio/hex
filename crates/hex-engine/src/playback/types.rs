//! Public playback commands, actions, handoff errors, and continuation descriptors.
use super::*;

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

/// Whether one ordinary interactive destination retriggers notes already held there.
/// This option belongs to the action, not a graph, export or structural edit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NoteChase {
    #[default]
    On,
    Off,
}

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
    /// The same interactive discontinuity with an explicit, captured note policy.
    /// Existing `PlayFrom` callers retain their default-on behavior.
    PlayFromWithNoteChase {
        frame: u64,
        note_chase: NoteChase,
    },
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

/// One direct Track instrument cut with a fresh worker at the receiver.
/// The output owner fences the original publication and Engine checks both
/// bindings against the two prepared graphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostedInstrumentMove {
    pub donor: hex_project::TrackId,
    pub receiver: hex_project::TrackId,
    pub device: hex_project::DeviceId,
    pub donor_binding: hex_lattice_contract::ForeignProcessorBinding,
    pub receiver_binding: hex_lattice_contract::ForeignProcessorBinding,
}

/// An exact direct Track effect cut with a fresh SDK endpoint at the receiver.
/// The control caller still owns the original applied-source proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostedTrackEffectMove {
    pub donor: hex_project::TrackId,
    pub receiver: hex_project::TrackId,
    pub device: hex_project::DeviceId,
    pub binding: hex_lattice_contract::ForeignProcessorBinding,
}

/// A same-Track permutation with independently prepared incoming DSP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostedTrackEffectReorder {
    pub track: hex_project::TrackId,
    pub device: hex_project::DeviceId,
    pub previous_binding: hex_lattice_contract::ForeignProcessorBinding,
    pub binding: hex_lattice_contract::ForeignProcessorBinding,
}

/// An exact ordinary hosted Track cut. The playback marker and
/// Desktop receipt both keep the same request through admission or refusal.
#[derive(Clone, Copy, Debug)]
pub enum HostedMoveRequest {
    Instrument(HostedInstrumentMove),
    Effect(HostedTrackEffectMove),
    Reorder(HostedTrackEffectReorder),
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
    pub(super) identity: hex_lattice::ProcessorInstanceIdentity,
    pub(super) binding: hex_lattice_contract::ForeignProcessorBinding,
    pub(super) previous: hex_lattice_contract::NativeDeviceState,
    pub(super) candidate: hex_lattice_contract::NativeDeviceState,
}
impl StoredForeignContinuation {
    pub(super) fn matcher(
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
    pub(super) fn retained_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(self.previous.retained_control_metadata_bytes()?)?
            .checked_add(self.candidate.retained_control_metadata_bytes()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_rejection_and_capability_codes_round_trip() {
        for status in [
            SessionCapabilityStatus::Available,
            SessionCapabilityStatus::RuntimeUnavailable,
            SessionCapabilityStatus::Legacy,
        ] {
            assert_eq!(
                SessionCapabilityStatus::from_code(status as u32),
                Some(status)
            );
        }
        assert_eq!(
            PlaybackHandoffRejection::from_code(
                PlaybackHandoffRejection::RuntimeFenceChanged as u32
            ),
            Some(PlaybackHandoffRejection::RuntimeFenceChanged)
        );
    }
}
