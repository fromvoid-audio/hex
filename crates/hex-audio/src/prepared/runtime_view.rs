//! Private borrowed operations on the one active musical owner. This cannot
//! adopt, clone, extract or expose a Playback from a constructor runtime.
use super::*;
use hex_engine::{AudioCandidateRuntimeStatus, ClaimedAudioCandidateRuntime};
pub(super) enum RuntimeRef<'a> {
    Legacy(&'a Playback),
    Eligible(&'a ClaimedAudioCandidateRuntime),
}
pub(super) enum RuntimeMut<'a> {
    Legacy(&'a mut Playback),
    Eligible(&'a mut ClaimedAudioCandidateRuntime),
}
pub(super) struct ActiveRef<'a> {
    pub(super) revision: u64,
    pub(super) control_context_generation: u64,
    pub(super) playback: RuntimeRef<'a>,
}
pub(super) struct ActiveMut<'a> {
    pub(super) revision: u64,
    pub(super) control_context_generation: u64,
    pub(super) playback: RuntimeMut<'a>,
}
impl ActivePublication {
    pub(super) fn current(&self) -> Option<ActiveRef<'_>> {
        match self {
            Self::Legacy(active) => Some(ActiveRef {
                revision: active.revision,
                control_context_generation: active.control_context_generation,
                playback: RuntimeRef::Legacy(&active.playback),
            }),
            Self::Eligible(active) => {
                let runtime = active.runtime()?;
                runtime.status()?;
                Some(ActiveRef {
                    revision: active.revision(),
                    control_context_generation: active.context_generation(),
                    playback: RuntimeRef::Eligible(runtime),
                })
            }
            Self::Candidate(_) => None,
        }
    }
    pub(super) fn current_mut(&mut self) -> Option<ActiveMut<'_>> {
        match self {
            Self::Legacy(active) => Some(ActiveMut {
                revision: active.revision,
                control_context_generation: active.control_context_generation,
                playback: RuntimeMut::Legacy(&mut active.playback),
            }),
            Self::Eligible(active) => {
                let revision = active.revision();
                let context = active.context_generation();
                let runtime = active.runtime_mut()?;
                runtime.status()?;
                Some(ActiveMut {
                    revision,
                    control_context_generation: context,
                    playback: RuntimeMut::Eligible(runtime),
                })
            }
            Self::Candidate(_) => None,
        }
    }
}
impl RuntimeRef<'_> {
    pub(super) fn project_output_supported(&self, tracks: &[u64]) -> bool {
        match self {
            Self::Legacy(p) => p.project_output_supported(tracks),
            Self::Eligible(p) => p.project_output_supported(tracks),
        }
    }
    fn status(&self) -> Option<AudioCandidateRuntimeStatus> {
        match self {
            Self::Eligible(runtime) => runtime.status(),
            Self::Legacy(p) => Some(AudioCandidateRuntimeStatus {
                position: p.position(),
                performance_frame: p.performance_frame(),
                playing: p.playing(),
                metronome_enabled: p.metronome_enabled(),
                session_capability: p.session_capability_status(),
                sampler_console: false,
                route_state_generation: p.session_route_state_generation(),
                route_count: p.session_route_count(),
            }),
        }
    }
    pub(super) fn sample_rate(&self) -> u32 {
        match self {
            Self::Legacy(p) => p.sample_rate(),
            Self::Eligible(p) => p.sample_rate().unwrap_or(0),
        }
    }
    pub(super) fn position(&self) -> u64 {
        self.status().map_or(0, |s| s.position)
    }
    pub(super) fn performance_frame(&self) -> u64 {
        self.status().map_or(0, |s| s.performance_frame)
    }
    pub(super) fn playing(&self) -> bool {
        self.status().is_some_and(|s| s.playing)
    }
    pub(super) fn metronome_enabled(&self) -> bool {
        self.status().is_some_and(|s| s.metronome_enabled)
    }
    pub(super) fn session_capability_status(&self) -> SessionCapabilityStatus {
        self.status()
            .map_or(SessionCapabilityStatus::RuntimeUnavailable, |s| {
                s.session_capability
            })
    }
    pub(super) fn sampler_console_available(&self) -> bool {
        self.status().is_some_and(|s| s.sampler_console)
    }
    pub(super) fn session_route_state_generation(&self) -> u64 {
        self.status().map_or(0, |s| s.route_state_generation)
    }
    pub(super) fn session_route_count(&self) -> usize {
        self.status().map_or(0, |s| s.route_count)
    }
    pub(super) fn session_clock_anchor(&self) -> Option<hex_engine::SessionClockAnchor> {
        match self {
            Self::Legacy(p) => p.session_clock_anchor(),
            Self::Eligible(p) => p.session_clock_anchor(),
        }
    }
    pub(super) fn session_route_status(
        &self,
        index: usize,
    ) -> Option<hex_engine::SessionRouteStatus> {
        match self {
            Self::Legacy(p) => p.session_route_status(index),
            Self::Eligible(p) => p.session_route_status(index),
        }
    }
    pub(super) fn gain_recording_ready(&self, track: u64) -> bool {
        match self {
            Self::Legacy(p) => p.gain_recording_ready(track),
            Self::Eligible(p) => p.gain_recording_ready(track),
        }
    }
    pub(super) fn gain_recording_track(&self) -> Option<u64> {
        match self {
            Self::Legacy(p) => p
                .prepared_revision()?
                .gain_recording_track()
                .map(|t| t.value()),
            Self::Eligible(p) => p.gain_recording_track(),
        }
    }
    pub(super) fn gain_recording_clock(&self, track: u64) -> Option<(u64, u64, u64)> {
        match self {
            Self::Legacy(p) => p.gain_recording_clock(track),
            Self::Eligible(p) => p.gain_recording_clock(track),
        }
    }
}
impl RuntimeMut<'_> {
    fn read(&self) -> RuntimeRef<'_> {
        match self {
            Self::Legacy(p) => RuntimeRef::Legacy(p),
            Self::Eligible(p) => RuntimeRef::Eligible(p),
        }
    }
    pub(super) fn sample_rate(&self) -> u32 {
        self.read().sample_rate()
    }
    pub(super) fn playing(&self) -> bool {
        self.read().playing()
    }
    pub(super) fn performance_frame(&self) -> u64 {
        self.read().performance_frame()
    }
    pub(super) fn gain_recording_ready(&self, track: u64) -> bool {
        self.read().gain_recording_ready(track)
    }
    pub(super) fn gain_recording_clock(&self, track: u64) -> Option<(u64, u64, u64)> {
        self.read().gain_recording_clock(track)
    }
    pub(super) fn try_apply_action(
        &mut self,
        action: PlaybackAction,
    ) -> Result<(), hex_engine::PlaybackActionError> {
        match self {
            Self::Legacy(p) => p.try_apply_action(action),
            Self::Eligible(p) => p.apply_action(action),
        }
    }
    pub(super) fn apply_priority_stop(
        &mut self,
        clock: &hex_engine::computer_keyboard::KeyboardActivationClock,
    ) {
        match self {
            Self::Legacy(p) => p.apply_priority_stop(clock),
            Self::Eligible(p) => {
                let _ = p.apply_priority_stop(clock);
            }
        }
    }
    pub(super) fn apply(&mut self, command: PlaybackCommand) {
        match self {
            Self::Legacy(p) => p.apply(command),
            Self::Eligible(p) => {
                let _ = p.apply_action(PlaybackAction::Transport(command));
            }
        }
    }
    pub(super) fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        match self {
            Self::Legacy(p) => p.render(output),
            Self::Eligible(p) => p.render(output),
        }
    }
    pub(super) fn render_with_signal_observers(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut hex_engine::NodeSignalObserver<'_>>,
        project: Option<&mut hex_engine::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), Invalid> {
        match self {
            Self::Legacy(p) => p.render_with_signal_observers(output, observer, project),
            Self::Eligible(p) => p.render_with_signal_observers(output, observer, project),
        }
    }
    pub(super) fn clear_gain_recording_override(&mut self) {
        match self {
            Self::Legacy(p) => p.clear_gain_recording_override(),
            Self::Eligible(p) => {
                let _ = p.clear_gain_recording_override();
            }
        }
    }
    pub(super) fn set_gain_recording_override(
        &mut self,
        track: u64,
        gain: f32,
    ) -> Result<(), Invalid> {
        match self {
            Self::Legacy(p) => p.set_gain_recording_override(track, gain),
            Self::Eligible(p) => p.set_gain_recording_override(track, gain),
        }
    }
    pub(super) fn select_processor_audition(
        &mut self,
        identity: PostNodeIdentity,
        selection: Option<hex_engine::ProcessorAuditionSelection>,
    ) -> bool {
        match self {
            Self::Legacy(p) => p.select_processor_audition(identity, selection),
            Self::Eligible(p) => p.select_processor_audition(identity, selection),
        }
    }
    pub(super) fn cancel_processor_audition(&mut self, identity: PostNodeIdentity) -> bool {
        match self {
            Self::Legacy(p) => p.cancel_processor_audition(identity),
            Self::Eligible(p) => p.cancel_processor_audition(identity),
        }
    }
    pub(super) fn processor_audition_is_active(
        &mut self,
        identity: PostNodeIdentity,
        selection: hex_engine::ProcessorAuditionSelection,
    ) -> bool {
        match self {
            Self::Legacy(p) => p.processor_audition_is_active(identity, selection),
            Self::Eligible(p) => p.processor_audition_is_active(identity, selection),
        }
    }
    pub(super) fn cancel_all_processor_auditions(&mut self) {
        match self {
            Self::Legacy(p) => p.cancel_all_processor_auditions(),
            Self::Eligible(p) => {
                let _ = p.cancel_all_processor_auditions();
            }
        }
    }
}
