//! Command classification, publication queues, and replacement policies.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct QueuedCommand {
    pub(super) ticket: u64,
    pub(super) action: PlaybackAction,
    pub(super) project_generation: Option<u64>,
    pub(super) expected_source: Option<AppliedAudioSource>,
    pub(super) sampler_edit_gate: u64,
    pub(super) transport_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CommandCut {
    pub(super) stop_through: Option<u64>,
    pub(super) process_through: u64,
}

pub(super) fn command_cut(entry_ticket: u64, stop_marker: u64) -> CommandCut {
    let stop_through = (stop_marker != 0).then_some(stop_marker.saturating_sub(1));
    CommandCut {
        stop_through,
        process_through: stop_through.map_or(entry_ticket, |stop| stop.max(entry_ticket)),
    }
}

pub(super) fn action_invalidates_processor_audition(action: PlaybackAction) -> bool {
    match action {
        PlaybackAction::Transport(
            PlaybackCommand::Stop
            | PlaybackCommand::Seek(_)
            | PlaybackCommand::PlayFrom(_)
            | PlaybackCommand::PlayFromWithNoteChase { .. },
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

pub(super) fn is_pause_action(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(PlaybackCommand::Pause)
            | PlaybackAction::Session(hex_engine::SessionPlaybackCommand::Pause)
    )
}

pub(super) fn starts_playback(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(
            PlaybackCommand::Play
                | PlaybackCommand::PlayFrom(_)
                | PlaybackCommand::PlayFromWithNoteChase { .. }
        ) | PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::Play
                | hex_engine::SessionPlaybackCommand::LaunchClip { .. }
                | hex_engine::SessionPlaybackCommand::LaunchClipWithNoteChase { .. }
                | hex_engine::SessionPlaybackCommand::LaunchScene { .. }
                | hex_engine::SessionPlaybackCommand::LaunchSceneWithNoteChase { .. }
                | hex_engine::SessionPlaybackCommand::CueMidiClip { .. }
        ) | PlaybackAction::LaunchAndPlay(_)
    )
}

// A start queued before an output fault must not revive that output, even
// if graph recovery clears the fault before the command reaches the callback.
pub(super) fn stale_output_start(status: &Status, queued: QueuedCommand) -> bool {
    starts_playback(queued.action)
        && queued.transport_epoch <= status.last_output_fault_epoch.load(Acquire)
}

// A refused PlayFrom keeps its frame as recovery cursor intent. Seek is not
// a start: it still applies to the stopped graph while a render fault is latched.
pub(super) fn fault_cursor_intent(action: PlaybackAction) -> Option<u64> {
    match action {
        PlaybackAction::Transport(
            PlaybackCommand::PlayFrom(frame) | PlaybackCommand::PlayFromWithNoteChase { frame, .. },
        ) => Some(frame),
        _ => None,
    }
}

pub(super) fn recovery_cursor_action(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(
            PlaybackCommand::Stop
                | PlaybackCommand::Seek(_)
                | PlaybackCommand::PlayFrom(_)
                | PlaybackCommand::PlayFromWithNoteChase { .. }
        ) | PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::Stop | hex_engine::SessionPlaybackCommand::Seek(_)
        )
    )
}

pub(crate) struct Publication {
    pub(super) pcm_frames: u64,
    pub(super) forwarded_pcm: bool,
    pub(super) revision: u64,
    /// UI/control project context this compiled playback belongs to. This is
    /// audio publication metadata, not project schema or engine state.
    pub(super) control_context_generation: u64,
    // A legacy default zero is not proof of a real project generation.
    pub(super) explicit_control_context: bool,
    pub(super) source_context: Option<hex_engine::AudioSourceContext>,
    pub(super) policy: PublicationPolicy,
    pub(super) fresh_runtime_cancellation: Option<PublicationReceiptOwner>,
    // Callback-owned performance boundary for a pending quantized seek or
    // an insert waiting for the predecessor's admitted route action.
    pub(super) scheduled_cut_frame: Option<u64>,
    pub(super) streams: Option<Box<crate::candidate_custody::OrdinaryStreams>>,
    pub(super) playback: Box<Playback>,
    // Drop the source observation on control before releasing the native
    // source reference, which keeps its actual paired allocation alive.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) native_continuation: Option<hex_plugin_host::transport::ParameterInputSource>,
    // Field order matters: destroy the actual ordinary graph before releasing
    // its native source references. This publication is destroyed on control.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) native_references: Option<crate::candidate_custody::OrdinaryNativeReferences>,
}

pub(super) struct RenderOutcome {
    pub(super) audible_recorded: bool,
    pub(super) start: Option<(u64, u64, bool)>,
}
// Exactly one active musical runtime, including the constructor-owned path.
pub(super) enum ActivePublication {
    Eligible(crate::candidate_custody::ActiveEligibleCandidate),
    Legacy(Publication),
    Candidate(crate::candidate_custody::ActiveCandidate),
}
impl ActivePublication {
    pub(super) fn legacy(&self) -> Option<&Publication> {
        match self {
            Self::Legacy(value) => Some(value),
            Self::Candidate(_) | Self::Eligible(_) => None,
        }
    }
    pub(super) fn legacy_mut(&mut self) -> Option<&mut Publication> {
        match self {
            Self::Legacy(value) => Some(value),
            Self::Candidate(_) | Self::Eligible(_) => None,
        }
    }
}
pub(super) enum QueuedPublication {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    NativeEpoch {
        target: native_sources::NativeEpochRuntimeTarget,
        fence: StoppedEditFence,
    },
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    Native(crate::candidate_custody::NativeEndpointEnvelope),
    Domain(crate::candidate_custody::DomainEnvelope),
    Eligible(crate::candidate_custody::QueuedEligibleCandidate),
    Legacy(Publication),
    Candidate(crate::candidate_custody::QueuedCandidate),
}
pub(crate) enum ReturnedPublication {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    Native(crate::candidate_custody::NativeEndpointEnvelope),
    Domain(crate::candidate_custody::DomainEnvelope),
    Eligible(crate::candidate_custody::ReturnedEligibleCandidate),
    Legacy(Publication),
    Candidate(crate::candidate_custody::ReturnedCandidate),
}
// A refused queue returns the same fixed 128-byte carrier unchanged. Boxing
// the error would add a separate failure allocation and ownership container.
#[allow(clippy::result_large_err)]
pub(super) fn enqueue_legacy(
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

pub(super) fn native_epoch_accepts_successor(
    returned: Option<hex_engine::AudioSourceContext>,
    successor: Option<hex_engine::AudioSourceContext>,
) -> bool {
    returned
        .zip(successor)
        .is_some_and(|(returned, successor)| {
            returned.project_generation == successor.project_generation
                && successor.editor_revision >= returned.editor_revision
        })
}

/// Controls whether a newly compiled runtime inherits transport and Session
/// route state from the currently active project revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationPolicy {
    /// Preserve transport, launch ownership, and compatible voice state.
    InheritRuntime,
    /// Install a staged device edit only at the captured source and clock,
    /// with cancellation ordered before any destructive inheritance.
    DeviceInsert {
        predecessor: AppliedAudioSource,
        fence: FreshRuntimeFence,
    },
    /// Install a caller-prepared fresh runtime without inheriting from the active one.
    /// The policy does not sanitize a pre-mutated candidate itself.
    InstallFreshRuntime,
    /// Install only if the active runtime, route generation, and command prefix
    /// still match the caller's control-side capture.
    InstallFreshRuntimeIfUnchanged(FreshRuntimeFence),
    /// A prepared quantized seek waits on the predecessor's musical clock.
    /// The first ready callback selects the next grid boundary. Installation
    /// uses ordinary seek/reset semantics, retaining the candidate's DSP clock.
    InstallQuantizedSeekWhenReady {
        fence: FreshRuntimeFence,
        quantization: hex_engine::LaunchQuantization,
    },
    /// Inherit only through the source-bound cut admitted by the tempo publisher.
    InheritAtTempoCut(FreshRuntimeFence),
    /// Authored structural edit: the shared stopped gate remains held until exact
    /// installation. Stop keeps priority; Play cannot run on the old graph.
    InstallUnderStoppedArrangement(StoppedEditFence),
    /// Same-generation native renewal after actual stopped graph/endpoint return.
    /// Only its ready native gate admits this ordinary successor; it restores the
    /// returned revision or a newer canonical revision of the same project
    /// generation whose live handoff was refused, never an older revision or
    /// different project generation.
    InstallUnderNativeEpoch(StoppedEditFence),
    /// Inherit the exact still-active ordinary native graph while its stopped
    /// fence remains held. This does not renew or return its original sources.
    InheritUnderStoppedNative {
        source: AppliedAudioSource,
        fence: StoppedEditFence,
    },
    /// Inherit only while the native parameter gesture's exact ordinary source
    /// and transport are still current. This grants no native worker custody.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    InheritUnderNativeParameter {
        source: AppliedAudioSource,
        transport_epoch: u64,
        playing: bool,
    },
    /// Structural live edit under the same source/command gate; inherit the actual
    /// installation-boundary transport, routes and voice/release state.
    InheritUnderArrangement(StoppedEditFence),
    /// One-way continuous handoff on a combined-configured output from an active
    /// verified constructor graph (or vacant combined output when `predecessor`
    /// is `None`) to an admitted ordinary resource graph. Preserves the active
    /// verified graph and output on refusal.
    HandoffFromVerified {
        predecessor: Option<AppliedAudioSource>,
    },
}

impl PublicationPolicy {
    pub(super) fn inherits_runtime(self) -> bool {
        match self {
            Self::InheritRuntime
            | Self::DeviceInsert { .. }
            | Self::InheritAtTempoCut(_)
            | Self::InheritUnderArrangement(_)
            | Self::InheritUnderStoppedNative { .. } => true,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::InheritUnderNativeParameter { .. } => true,
            _ => false,
        }
    }

    pub(super) fn fresh_fence(self) -> Option<FreshRuntimeFence> {
        match self {
            Self::InstallFreshRuntimeIfUnchanged(fence)
            | Self::InstallQuantizedSeekWhenReady { fence, .. }
            | Self::InheritAtTempoCut(fence)
            | Self::DeviceInsert { fence, .. } => Some(fence),
            _ => None,
        }
    }
}

/// Control-side freshness proof for a candidate prepared from an observed
/// runtime state. Revision zero represents an empty audio owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshRuntimeFence {
    pub(super) expected_active_revision: u64,
    pub(super) route_state_generation: u64,
    pub(super) admitted_command_ticket: u64,
    pub(super) output_fault_epoch: u64,
    pub(super) consumed_stop_generation: u64,
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

    pub const fn output_fault_epoch(self) -> u64 {
        self.output_fault_epoch
    }

    /// Callback-owned Stop sequence. Odd values are in flight; exhaustion refuses new fences.
    pub const fn consumed_stop_generation(self) -> u64 {
        self.consumed_stop_generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_cut_and_action_classification_respect_stop_markers() {
        assert_eq!(
            command_cut(5, 0),
            CommandCut {
                stop_through: None,
                process_through: 5,
            }
        );
        assert_eq!(
            command_cut(3, 6),
            CommandCut {
                stop_through: Some(5),
                process_through: 5,
            }
        );
        assert_eq!(
            command_cut(8, 6),
            CommandCut {
                stop_through: Some(5),
                process_through: 8,
            }
        );
        assert!(is_pause_action(PlaybackAction::Transport(
            PlaybackCommand::Pause
        )));
        assert!(!is_pause_action(PlaybackAction::Transport(
            PlaybackCommand::Play
        )));
        assert!(starts_playback(PlaybackAction::Transport(
            PlaybackCommand::Play
        )));
        assert!(PublicationPolicy::InheritRuntime.inherits_runtime());
        assert!(!PublicationPolicy::InstallFreshRuntime.inherits_runtime());
    }
}
