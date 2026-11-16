//! Bounded operations on the same closed constructor runtime. No Playback,
//! owning root handle or mutable DSP provider escapes through this facade.
use super::*;
mod stream_view;
pub use stream_view::StreamPlayback;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCandidateRuntimeStatus {
    pub position: u64,
    pub performance_frame: u64,
    pub playing: bool,
    /// Session Arrangement loop pass and active region, if supported.
    pub arrangement_loop_state: Option<crate::session_playback::ArrangementLoopState>,
    pub metronome_enabled: bool,
    pub session_capability: crate::SessionCapabilityStatus,
    /// True only for a successfully admitted, installed private CURRENT runtime.
    pub sampler_console: bool,
    pub route_state_generation: u64,
    pub route_count: usize,
    /// Frames by which Main output trails `position`.
    pub main_output_latency_frames: u32,
    /// Content-clock loop law that wraps `position`.
    pub content_loop: Option<hex_project::LoopRegion>,
}
impl ClaimedAudioCandidateRuntime {
    fn current_playback(&self) -> Option<&Playback> {
        (self.inner.phase == ClaimedPhase::Current)
            .then_some(())
            .and_then(|()| self.inner.runtime.as_ref().map(|r| &r.playback))
    }
    fn current_playback_mut(&mut self) -> Option<&mut Playback> {
        (self.inner.phase == ClaimedPhase::Current)
            .then_some(())
            .and_then(|()| self.inner.runtime.as_mut().map(|r| &mut r.playback))
    }
    pub(super) fn current_runtime_mut(&mut self) -> Option<&mut RuntimeOwner> {
        if self.inner.phase != ClaimedPhase::Current {
            return None;
        }
        self.inner.runtime.as_deref_mut()
    }
    pub fn status(&self) -> Option<AudioCandidateRuntimeStatus> {
        let playback = self.current_playback()?;
        Some(AudioCandidateRuntimeStatus {
            position: playback.position(),
            performance_frame: playback.performance_frame(),
            playing: playback.playing(),
            arrangement_loop_state: playback.arrangement_loop_state(),
            metronome_enabled: playback.metronome_enabled(),
            session_capability: playback.session_capability_status(),
            sampler_console: self
                .inner
                .runtime
                .as_ref()
                .is_some_and(|r| r.sampler_console_available()),
            route_state_generation: playback.session_route_state_generation(),
            route_count: playback.session_route_count(),
            main_output_latency_frames: playback.main_output_latency_frames(),
            content_loop: playback.automation_capture_loop(),
        })
    }
    /// Scalar effective clock metadata from the current admitted runtime only.
    pub fn session_clock_anchor(&self) -> Option<crate::SessionClockAnchor> {
        self.current_playback()?.session_clock_anchor()
    }
    pub fn session_route_status(&self, index: usize) -> Option<crate::SessionRouteStatus> {
        self.current_playback()?.session_route_status(index)
    }
    pub fn render_with_post_node_observer(
        &mut self,
        output: &mut [crate::Frame],
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), crate::Invalid> {
        if let Some(observer) = observer {
            self.render_with_node_signal_observer(
                output,
                Some(&mut |id, phase, offset, frames| {
                    if phase == crate::NodeSignalPhase::Output {
                        observer(id, offset, frames);
                    }
                }),
            )
        } else {
            self.render_with_node_signal_observer(output, None)
        }
    }
    pub fn render_with_node_signal_observer(
        &mut self,
        output: &mut [crate::Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<(), crate::Invalid> {
        self.render_with_signal_observers(output, observer, None)
    }
    pub fn render_with_signal_observers(
        &mut self,
        output: &mut [crate::Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), crate::Invalid> {
        self.current_runtime_mut()
            .ok_or(crate::Invalid("candidate runtime is not current"))?
            .render_with_signal_observers(output, observer, project)
    }
    /// Render live input through the current closed runtime and retained resources.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_live_track_audio(
        &mut self,
        output: &mut [crate::Frame],
        track: hex_project::TrackId,
        input: &[crate::Frame],
        pages: Option<&mut dyn crate::MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), crate::Invalid> {
        let Some(runtime) = self.current_runtime_mut() else {
            foreign.reject_callback();
            output.fill([0.0; 2]);
            return Err(crate::Invalid("candidate runtime is not current"));
        };
        runtime
            .render_with_live_track_audio(output, track, input, pages, observer, project, foreign)
    }
    /// Stage native effects through the original closed runtime and sampler
    /// sources. Success is not output acceptance; the audio owner alone accepts
    /// the complete converted physical callback. No finite renderer fallback.
    pub fn render_with_foreign_processors(
        &mut self,
        output: &mut [crate::Frame],
        pages: Option<&mut dyn crate::MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), crate::Invalid> {
        let Some(runtime) = self.current_runtime_mut() else {
            foreign.reject_callback();
            output.fill([0.0; 2]);
            return Err(crate::Invalid("candidate runtime is not current"));
        };
        runtime.render_with_foreign_processors(output, pages, observer, project, foreign)
    }
    pub fn project_output_supported(&self, tracks: &[u64]) -> bool {
        self.current_playback()
            .is_some_and(|p| p.project_output_supported(tracks))
    }
    pub fn project_sample_sources_supported(
        &self,
        tracks: &[u64],
        returns: &[crate::ReturnId],
    ) -> bool {
        self.current_playback()
            .is_some_and(|p| p.project_sample_sources_supported(tracks, returns))
    }
    pub fn render_with_pages_and_post_node_observer<P: crate::MediaPages + ?Sized>(
        &mut self,
        output: &mut [crate::Frame],
        pages: &mut P,
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), crate::Invalid> {
        if let Some(observer) = observer {
            self.render_with_pages_and_node_signal_observer(
                output,
                pages,
                Some(&mut |id, phase, offset, frames| {
                    if phase == crate::NodeSignalPhase::Output {
                        observer(id, offset, frames);
                    }
                }),
            )
        } else {
            self.render_with_pages_and_node_signal_observer(output, pages, None)
        }
    }
    pub fn render_with_pages_and_node_signal_observer<P: crate::MediaPages + ?Sized>(
        &mut self,
        output: &mut [crate::Frame],
        pages: &mut P,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<(), crate::Invalid> {
        self.render_with_pages_and_signal_observers(output, pages, observer, None)
    }
    pub fn render_with_pages_and_signal_observers<P: crate::MediaPages + ?Sized>(
        &mut self,
        output: &mut [crate::Frame],
        pages: &mut P,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), crate::Invalid> {
        self.current_runtime_mut()
            .ok_or(crate::Invalid("candidate runtime is not current"))?
            .render_with_pages_and_signal_observers(output, pages, observer, project)
    }
    /// Match cancellation against the same prepared action/runtime's route targets.
    pub fn stream_action_targets_track(
        &mut self,
        prepared: &crate::PreparedStreamAction,
        track: Option<hex_project::TrackId>,
    ) -> Result<bool, crate::PlaybackActionError> {
        if !prepared.matches_owner(self.identity(), self.source_context()) {
            return Err(crate::PlaybackActionError::Session(
                crate::SessionPlaybackError::InvalidCommand,
            ));
        }
        self.current_playback_mut()
            .ok_or(crate::PlaybackActionError::SessionNotPrepared)?
            .stream_action_targets_track(prepared, track)
    }
    /// Prepare a correlated action without committing any runtime state.
    /// Page/custody and native command/device checks remain with the host.
    pub fn prepare_stream_action(
        &mut self,
        action: crate::PlaybackAction,
    ) -> Result<crate::PreparedStreamAction, crate::PlaybackActionError> {
        if self
            .inner
            .runtime
            .as_ref()
            .is_some_and(|r| r.sampler_sources.is_some())
        {
            return Err(crate::PlaybackActionError::Session(
                crate::SessionPlaybackError::InvalidCommand,
            ));
        }
        let identity = self.identity();
        let context = self.source_context();
        let mut prepared = self
            .current_playback_mut()
            .ok_or(crate::PlaybackActionError::SessionNotPrepared)?
            .prepare_stream_action(action)?;
        prepared.bind_owner(identity, context);
        Ok(prepared)
    }
    pub fn visit_prepared_stream_action(
        &mut self,
        prepared: &crate::PreparedStreamAction,
        maximum_frames: usize,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), crate::Invalid>,
    ) -> Result<crate::StreamChunkWindow, crate::Invalid> {
        if !prepared.matches_owner(self.identity(), self.source_context()) {
            return Err(crate::Invalid("stream action owner differs"));
        }
        self.current_playback_mut()
            .ok_or(crate::Invalid("stream current owner missing"))?
            .visit_prepared_stream_action(prepared, maximum_frames, visit)
    }
    pub fn visit_pending_stream_chunk(
        &mut self,
        due: u64,
        maximum_frames: usize,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), crate::Invalid>,
    ) -> Result<crate::StreamChunkWindow, crate::Invalid> {
        self.current_playback_mut()
            .ok_or(crate::Invalid("stream current owner missing"))?
            .visit_pending_stream_chunk(due, maximum_frames, visit)
    }
    /// Recheck the exact original owner/clocks/routes, then run existing typed
    /// semantics once. The move-only plan is consumed even on stale refusal.
    pub fn commit_stream_action(
        &mut self,
        prepared: crate::PreparedStreamAction,
    ) -> Result<(), crate::PlaybackActionError> {
        if !prepared.matches_owner(self.identity(), self.source_context()) {
            return Err(crate::PlaybackActionError::Session(
                crate::SessionPlaybackError::InvalidCommand,
            ));
        }
        self.current_playback_mut()
            .ok_or(crate::PlaybackActionError::SessionNotPrepared)?
            .commit_stream_action(prepared)
    }
    pub fn set_device_metering(&mut self, enabled: bool) -> bool {
        let Some(playback) = self.current_playback_mut() else {
            return false;
        };
        playback.set_device_metering(enabled);
        true
    }
    pub fn arm_midi_capture(
        &mut self,
        binding: crate::midi_capture::MidiCaptureBinding,
        clock: &crate::computer_keyboard::KeyboardActivationClock,
    ) -> Result<crate::midi_capture::MidiCaptureClock, crate::Invalid> {
        self.current_playback_mut()
            .ok_or(crate::Invalid("candidate runtime is not current"))?
            .arm_midi_capture(binding, clock)
    }
    pub fn midi_capture_finish_fault(
        &self,
        take_id: u64,
    ) -> Option<crate::midi_capture::MidiCaptureFault> {
        self.current_playback()?.midi_capture_finish_fault(take_id)
    }
    pub fn midi_capture_fault(
        &self,
        take_id: u64,
    ) -> Option<crate::midi_capture::MidiCaptureFault> {
        self.current_playback()?.midi_capture_fault(take_id)
    }
    pub fn finish_midi_capture(
        &mut self,
        take_id: u64,
    ) -> Result<crate::midi_capture::MidiCaptureClock, crate::Invalid> {
        self.current_playback_mut()
            .ok_or(crate::Invalid("candidate runtime is not current"))?
            .finish_midi_capture(take_id)
    }
    pub fn midi_capture_render(
        &self,
        take_id: u64,
    ) -> Result<Option<crate::midi_capture::MidiCaptureRender<'_>>, crate::Invalid> {
        self.current_playback()
            .ok_or(crate::Invalid("candidate runtime is not current"))?
            .midi_capture_render(take_id)
    }
    pub fn native_parameter_input_target(&self) -> Option<crate::NativeParameterInputTarget> {
        self.current_playback()?.native_parameter_input_target()
    }
    pub fn set_native_parameter_input(
        &mut self,
        operation: hex_lattice::NativeTransientOperation,
    ) -> Result<(), crate::Invalid> {
        self.current_playback_mut()
            .ok_or(crate::Invalid("candidate runtime is not current"))?
            .set_native_parameter_input(operation)
    }
    pub fn invalidate_native_parameter_input(&mut self) {
        if let Some(playback) = self.current_playback_mut() {
            playback.invalidate_native_parameter_input();
        }
    }
    pub fn finish_native_parameter_input(&mut self) {
        if let Some(playback) = self.current_playback_mut() {
            playback.finish_native_parameter_input();
        }
    }
    pub fn take_native_parameter_application(
        &mut self,
    ) -> Option<crate::NativeParameterApplication> {
        self.current_playback_mut()?
            .take_native_parameter_application()
    }
    pub fn gain_recording_ready(&self, track: u64) -> bool {
        self.current_playback()
            .is_some_and(|p| p.gain_recording_ready(track))
    }
    pub fn gain_recording_track(&self) -> Option<u64> {
        self.current_playback()?
            .prepared_revision()?
            .gain_recording_track()
            .map(|t| t.value())
    }
    pub fn set_gain_recording_override(
        &mut self,
        track: u64,
        gain: f32,
    ) -> Result<(), crate::Invalid> {
        self.current_playback_mut()
            .ok_or(crate::Invalid("candidate runtime is not current"))?
            .set_gain_recording_override(track, gain)
    }
    pub fn clear_gain_recording_override(&mut self) -> bool {
        let Some(playback) = self.current_playback_mut() else {
            return false;
        };
        playback.clear_gain_recording_override();
        true
    }
    pub fn release_gain_recording_override(&mut self) -> bool {
        let Some(playback) = self.current_playback_mut() else {
            return false;
        };
        playback.release_gain_recording_override();
        true
    }
    pub fn gain_recording_clock(&self, track: u64) -> Option<(u64, u64, u64)> {
        self.current_playback()?.gain_recording_clock(track)
    }
    pub fn select_processor_audition(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: Option<crate::ProcessorAuditionSelection>,
    ) -> bool {
        self.current_playback_mut()
            .is_some_and(|p| p.select_processor_audition(identity, selection))
    }
    pub fn transfer_processor_audition_from_release_tail(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: crate::ProcessorAuditionSelection,
    ) -> bool {
        self.current_playback_mut()
            .is_some_and(|p| p.transfer_processor_audition_from_release_tail(identity, selection))
    }
    pub fn processor_audition_is_active(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: crate::ProcessorAuditionSelection,
    ) -> bool {
        self.current_playback_mut()
            .is_some_and(|p| p.processor_audition_is_active(identity, selection))
    }
    pub fn cancel_processor_audition(&mut self, identity: crate::PostNodeIdentity) -> bool {
        self.current_playback_mut()
            .is_some_and(|p| p.cancel_processor_audition(identity))
    }
    pub fn cancel_all_processor_auditions(&mut self) -> bool {
        let Some(playback) = self.current_playback_mut() else {
            return false;
        };
        playback.cancel_all_processor_auditions();
        true
    }
    pub fn cancel_retired_processor_auditions(&mut self) -> bool {
        let Some(playback) = self.current_playback_mut() else {
            return false;
        };
        playback.cancel_retired_processor_auditions();
        true
    }
    /// Caller already validated the same actual source/context/fence. Only the
    /// monitoring scalar transfers; this cannot mint graph-origin credit.
    pub fn inherit_metronome_for_fresh_seek(&mut self, previous: &Self) -> bool {
        let (Some(playback), Some(previous)) =
            (self.current_playback_mut(), previous.current_playback())
        else {
            return false;
        };
        playback.inherit_metronome_for_fresh_seek(previous);
        true
    }
}
