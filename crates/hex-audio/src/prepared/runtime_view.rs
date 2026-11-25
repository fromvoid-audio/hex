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
    pub(super) fn project_sample_sources_supported(
        &self,
        layout: &crate::sample_recording::Layout,
    ) -> bool {
        match self {
            Self::Legacy(p) => p.project_sample_sources_supported(&layout.tracks, &layout.returns),
            Self::Eligible(p) => {
                p.project_sample_sources_supported(&layout.tracks, &layout.returns)
            }
        }
    }

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
                arrangement_loop_state: p.arrangement_loop_state(),
                metronome_enabled: p.metronome_enabled(),
                session_capability: p.session_capability_status(),
                sampler_console: false,
                route_state_generation: p.session_route_state_generation(),
                route_count: p.session_route_count(),
                main_output_latency_frames: p.main_output_latency_frames(),
                content_loop: p.automation_capture_loop(),
            }),
        }
    }
    pub(super) fn transport(
        &self,
    ) -> Option<(
        u64,
        u64,
        bool,
        Option<hex_engine::session_playback::ArrangementLoopState>,
    )> {
        match self {
            Self::Legacy(playback) => Some((
                playback.position(),
                playback.performance_frame(),
                playback.playing(),
                playback.arrangement_loop_state(),
            )),
            Self::Eligible(runtime) => runtime.status().map(|status| {
                (
                    status.position,
                    status.performance_frame,
                    status.playing,
                    status.arrangement_loop_state,
                )
            }),
        }
    }
    pub(super) fn sample_rate(&self) -> u32 {
        match self {
            Self::Legacy(p) => p.sample_rate(),
            Self::Eligible(p) => p.sample_rate().unwrap_or(0),
        }
    }
    pub(super) fn main_output_latency_frames(&self) -> Option<u32> {
        self.status()
            .map(|status| status.main_output_latency_frames)
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
    /// The current graph has finished its de-click and holds the musical cut.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn device_insert_held(&self) -> bool {
        match self {
            Self::Legacy(playback) => playback.device_insert_held(),
            Self::Eligible(runtime) => runtime.device_insert_held(),
        }
    }
    pub(super) fn metronome_enabled(&self) -> bool {
        self.status().is_some_and(|s| s.metronome_enabled)
    }
    /// Audible-playhead facts: Main output latency and the enabled content
    /// loop as half-open `(start, end)` frames.
    pub(super) fn audible_timing(&self) -> (u32, Option<(u64, u64)>) {
        self.status().map_or((0, None), |s| {
            (
                s.main_output_latency_frames,
                s.content_loop
                    .filter(|region| region.enabled())
                    .map(|region| (region.start(), region.end())),
            )
        })
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
    pub(super) fn midi_capture_render(
        &self,
        take_id: u64,
    ) -> Result<Option<hex_engine::midi_capture::MidiCaptureRender<'_>>, Invalid> {
        match self {
            Self::Legacy(p) => p.midi_capture_render(take_id),
            Self::Eligible(p) => p.midi_capture_render(take_id),
        }
    }
    pub(super) fn midi_capture_finish_fault(
        &self,
        take_id: u64,
    ) -> Option<hex_engine::midi_capture::MidiCaptureFault> {
        match self {
            Self::Legacy(p) => p.midi_capture_finish_fault(take_id),
            Self::Eligible(p) => p.midi_capture_finish_fault(take_id),
        }
    }
    pub(super) fn midi_capture_fault(
        &self,
        take_id: u64,
    ) -> Option<hex_engine::midi_capture::MidiCaptureFault> {
        match self {
            Self::Legacy(p) => p.midi_capture_fault(take_id),
            Self::Eligible(p) => p.midi_capture_fault(take_id),
        }
    }
    pub(super) fn native_parameter_input_target(
        &self,
    ) -> Option<hex_engine::NativeParameterInputTarget> {
        match self {
            Self::Legacy(p) => p.native_parameter_input_target(),
            Self::Eligible(p) => p.native_parameter_input_target(),
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
    pub(super) fn native_parameter_input_target(
        &self,
    ) -> Option<hex_engine::NativeParameterInputTarget> {
        self.read().native_parameter_input_target()
    }
    pub(super) fn set_native_parameter_input(
        &mut self,
        operation: hex_lattice::NativeTransientOperation,
    ) -> Result<(), Invalid> {
        match self {
            Self::Legacy(p) => p.set_native_parameter_input(operation),
            Self::Eligible(p) => p.set_native_parameter_input(operation),
        }
    }
    pub(super) fn invalidate_native_parameter_input(&mut self) {
        match self {
            Self::Legacy(p) => p.invalidate_native_parameter_input(),
            Self::Eligible(p) => p.invalidate_native_parameter_input(),
        }
    }
    pub(super) fn finish_native_parameter_input(&mut self) {
        match self {
            Self::Legacy(p) => p.finish_native_parameter_input(),
            Self::Eligible(p) => p.finish_native_parameter_input(),
        }
    }
    pub(super) fn take_native_parameter_application(
        &mut self,
    ) -> Option<hex_engine::NativeParameterApplication> {
        match self {
            Self::Legacy(p) => p.take_native_parameter_application(),
            Self::Eligible(p) => p.take_native_parameter_application(),
        }
    }
    pub(super) fn position(&self) -> u64 {
        self.read().position()
    }
    pub(super) fn arrangement_loop_pass(&self) -> u64 {
        match self {
            Self::Legacy(p) => p.arrangement_loop_pass(),
            Self::Eligible(p) => p.arrangement_loop_pass(),
        }
    }
    pub(super) fn main_output_latency_frames(&self) -> Option<u32> {
        self.read()
            .status()
            .map(|status| status.main_output_latency_frames)
    }
    pub(super) fn arm_midi_capture(
        &mut self,
        binding: hex_engine::midi_capture::MidiCaptureBinding,
        clock: &hex_engine::computer_keyboard::KeyboardActivationClock,
    ) -> Result<hex_engine::midi_capture::MidiCaptureClock, Invalid> {
        match self {
            Self::Legacy(p) => p.arm_midi_capture(binding, clock),
            Self::Eligible(p) => p.arm_midi_capture(binding, clock),
        }
    }
    pub(super) fn finish_midi_capture(
        &mut self,
        take_id: u64,
    ) -> Result<hex_engine::midi_capture::MidiCaptureClock, Invalid> {
        match self {
            Self::Legacy(p) => p.finish_midi_capture(take_id),
            Self::Eligible(p) => p.finish_midi_capture(take_id),
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
    /// Pause/Stop while playing fade out before applying (Pulse #4138).
    pub(super) fn try_apply_declicked_action(
        &mut self,
        action: PlaybackAction,
    ) -> Result<(), hex_engine::PlaybackActionError> {
        match self {
            Self::Legacy(p) => p.try_apply_declicked_action(action),
            Self::Eligible(p) => p.apply_declicked_action(action),
        }
    }
    pub(super) fn finish_transport_fade(&mut self) {
        match self {
            Self::Legacy(p) => p.finish_transport_fade(),
            Self::Eligible(p) => p.finish_transport_fade(),
        }
    }
    pub(super) fn pending_transport_fade(&self) -> Option<PlaybackAction> {
        match self {
            Self::Legacy(p) => p.pending_transport_fade(),
            Self::Eligible(p) => p.pending_transport_fade(),
        }
    }
    pub(super) fn take_transport_fade_failure(
        &mut self,
    ) -> Option<hex_engine::PlaybackActionRejection> {
        match self {
            Self::Legacy(p) => p.take_transport_fade_failure(),
            Self::Eligible(p) => p.take_transport_fade_failure(),
        }
    }
    pub(super) fn apply_priority_stop(
        &mut self,
        clock: &hex_engine::computer_keyboard::KeyboardActivationClock,
    ) -> Result<(), hex_engine::PlaybackActionError> {
        match self {
            Self::Legacy(p) => {
                p.apply_priority_stop(clock);
                Ok(())
            }
            Self::Eligible(p) => p.apply_priority_stop(clock),
        }
    }
    pub(super) fn apply(
        &mut self,
        command: PlaybackCommand,
    ) -> Result<(), hex_engine::PlaybackActionError> {
        match self {
            Self::Legacy(p) => {
                p.apply(command);
                Ok(())
            }
            Self::Eligible(p) => p.apply_action(PlaybackAction::Transport(command)),
        }
    }
    pub(super) fn render_with_signal_observers(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut hex_engine::NodeSignalObserver<'_>>,
        project: Option<&mut hex_engine::ProjectOutputTap<'_, '_>>,
        foreign: Option<&mut dyn hex_lattice::ForeignProcessors>,
        wave: Option<&mut dyn hex_engine::WaveExecutor>,
    ) -> Result<(), Invalid> {
        match (self, foreign) {
            (Self::Legacy(p), Some(foreign)) => p.render_with_foreign_processors_and_wave(
                output, None, observer, project, foreign, wave,
            ),
            (Self::Eligible(p), Some(foreign)) => p.render_with_foreign_processors_and_wave(
                output, None, observer, project, foreign, wave,
            ),
            (Self::Legacy(p), None) => {
                p.render_with_signal_observers_and_wave(output, observer, project, wave)
            }
            (Self::Eligible(p), None) => {
                p.render_with_signal_observers_and_wave(output, observer, project, wave)
            }
        }
    }
    pub(super) fn render_with_live_track_audio(
        &mut self,
        output: &mut [Frame],
        track: hex_engine::OutputCaptureId,
        input: &[Frame],
        observer: Option<&mut hex_engine::NodeSignalObserver<'_>>,
        project: Option<&mut hex_engine::ProjectOutputTap<'_, '_>>,
        foreign: Option<&mut dyn hex_lattice::ForeignProcessors>,
    ) -> Result<(), Invalid> {
        let hex_engine::OutputCaptureId::Track(track) = track else {
            return Err(Invalid("input monitor target must be a track"));
        };
        let mut refuse = hex_lattice::RefuseForeign;
        let foreign = foreign.unwrap_or(&mut refuse);
        match self {
            Self::Legacy(p) => p.render_with_live_track_audio(
                output, track, input, None, observer, project, foreign,
            ),
            Self::Eligible(p) => p.render_with_live_track_audio(
                output, track, input, None, observer, project, foreign,
            ),
        }
    }
    /// Clears any active gain-recording override on the current runtime.
    /// Returns `false` only if an `Eligible` runtime is no longer `Current`
    /// (`ActivePublication::current_mut` validates `Current` on entry).
    pub(super) fn clear_gain_recording_override(&mut self) -> bool {
        match self {
            Self::Legacy(p) => {
                p.clear_gain_recording_override();
                true
            }
            Self::Eligible(p) => p.clear_gain_recording_override(),
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
    /// Apply one desired input without clearing a still-active override.
    pub(super) fn set_gain_recording_input(
        &mut self,
        input: Option<(u64, f32)>,
    ) -> Result<(), Invalid> {
        match input {
            Some((track, gain)) => self.set_gain_recording_override(track, gain),
            None => match self {
                Self::Legacy(p) => {
                    p.release_gain_recording_override();
                    Ok(())
                }
                Self::Eligible(p) => p
                    .release_gain_recording_override()
                    .then_some(())
                    .ok_or(Invalid("candidate runtime is not current")),
            },
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
    /// Cancels all active processor auditions on the current runtime.
    /// Returns `false` only if an `Eligible` runtime is no longer `Current`
    /// (`ActivePublication::current_mut` validates `Current` on entry).
    pub(super) fn cancel_all_processor_auditions(&mut self) -> bool {
        match self {
            Self::Legacy(p) => {
                p.cancel_all_processor_auditions();
                true
            }
            Self::Eligible(p) => p.cancel_all_processor_auditions(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligible_runtime_view_propagates_priority_stop_and_transport_errors_without_allocating() {
        let Some(mut owner) = super::super::candidate_tests::candidate() else {
            return;
        };
        owner
            .seal_constructor_on_control(hex_engine::AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 2048,
                root_rows: 4,
                max_retained_bytes: 64 * 1024 * 1024,
                max_construction_peak_bytes: 64 * 1024 * 1024,
            })
            .unwrap();
        let (mut runtime, mut anchor) = owner.split_constructor_on_control().ok().unwrap();
        let clock = hex_engine::computer_keyboard::KeyboardActivationClock::default();

        // On a Current Eligible runtime, priority Stop and Stop transport commands succeed
        // with zero callback allocations.
        let mut ok_probe = || {
            let mut view = RuntimeMut::Eligible(&mut runtime);
            assert!(view.apply_priority_stop(&clock).is_ok());
            assert!(view.apply(PlaybackCommand::Stop).is_ok());
            assert!(view.set_gain_recording_input(None).is_ok());
            assert!(view.clear_gain_recording_override());
            assert!(view.cancel_all_processor_auditions());
        };
        #[cfg(feature = "allocation-probe")]
        {
            let info = allocation_counter::measure(&mut ok_probe);
            assert_eq!(
                (
                    info.count_total,
                    info.bytes_total,
                    info.count_current,
                    info.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        ok_probe();

        // Dispose the runtime so it is no longer Current; control methods must surface
        // failures explicitly instead of silently discarding them, with zero allocations.
        let proof = runtime.dispose_on_control().unwrap();
        let mut err_probe = || {
            let mut view = RuntimeMut::Eligible(&mut runtime);
            assert_eq!(
                view.apply_priority_stop(&clock),
                Err(hex_engine::PlaybackActionError::SessionNotPrepared)
            );
            assert_eq!(
                view.apply(PlaybackCommand::Stop),
                Err(hex_engine::PlaybackActionError::SessionNotPrepared)
            );
            assert!(view.set_gain_recording_input(None).is_err());
            assert!(view.finish_midi_capture(1).is_err());
            assert!(!view.clear_gain_recording_override());
            assert!(!view.cancel_all_processor_auditions());
        };
        #[cfg(feature = "allocation-probe")]
        {
            let info = allocation_counter::measure(&mut err_probe);
            assert_eq!(
                (
                    info.count_total,
                    info.bytes_total,
                    info.count_current,
                    info.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        err_probe();

        let mut anchors = [Some(anchor)];
        proof.record_anchor_cells_on_control(&mut anchors).unwrap();
        anchor = anchors[0].take().unwrap();
        anchor.finish_disposal_on_control().unwrap();
    }
}
