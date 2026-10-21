//! Bounded operations on the same closed constructor runtime. No Playback,
//! owning root handle or mutable DSP provider escapes through this facade.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCandidateRuntimeStatus {
    pub position: u64,
    pub performance_frame: u64,
    pub playing: bool,
    pub metronome_enabled: bool,
    pub session_capability: crate::SessionCapabilityStatus,
    pub route_state_generation: u64,
    pub route_count: usize,
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
    fn current_runtime_mut(&mut self) -> Option<&mut RuntimeOwner> {
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
            metronome_enabled: playback.metronome_enabled(),
            session_capability: playback.session_capability_status(),
            route_state_generation: playback.session_route_state_generation(),
            route_count: playback.session_route_count(),
        })
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
    pub fn project_output_supported(&self, tracks: &[u64]) -> bool {
        self.current_playback()
            .is_some_and(|p| p.project_output_supported(tracks))
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
        visit: &mut impl FnMut(crate::StreamPageExtent) -> Result<(), crate::Invalid>,
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
        visit: &mut impl FnMut(crate::StreamPageExtent) -> Result<(), crate::Invalid>,
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
