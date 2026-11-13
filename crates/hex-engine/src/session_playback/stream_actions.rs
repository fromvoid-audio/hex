//! Pure action projection using the same preallocated route/seek planners.
use super::*;
use crate::MidiTiming;
use crate::{
    Invalid, PlaybackAction, PlaybackActionError, PlaybackCommand, StreamChunkWindow,
    StreamResourceExtent,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Expected {
    runtime: u64,
    revision: usize,
    performance: u64,
    arrangement: u64,
    loop_pass: u64,
    routes: u64,
    playing: bool,
    anchor: Option<SessionClockAnchor>,
    periodic: Option<PreparedPeriodicSessionTempo>,
}
enum Resolved {
    Seek { frame: u64, beat: BeatTime },
    Route(TransitionMoment),
}
/// Move-only correlated action plan for one exact runtime. Private fields and
/// constructor; this is not page/custody or native command/device authority.
pub struct PreparedStreamAction {
    // Selected only by the engine's borrowed ordinary facade. Constructor
    // entry points keep their closed-family gates; this is not native custody.
    ordinary: bool,
    expected: Expected,
    action: PlaybackAction,
    resolved: Resolved,
    owner: Option<crate::AudioCandidateIdentity>,
    context: Option<crate::AudioSourceContext>,
}
impl PreparedStreamAction {
    /// Scheduling is distinct from activation. A seek target is not a due time.
    pub fn scheduled_due_frame(&self) -> Option<u64> {
        match self.resolved {
            Resolved::Route(moment) if !moment.immediate => Some(moment.target_frame),
            _ => None,
        }
    }
    pub fn playing_after_commit(&self) -> bool {
        self.expected.playing
            || matches!(
                self.action,
                PlaybackAction::Transport(PlaybackCommand::PlayFrom(_))
                    | PlaybackAction::LaunchAndPlay(_)
            )
    }
    pub(crate) fn bind_owner(
        &mut self,
        owner: crate::AudioCandidateIdentity,
        context: Option<crate::AudioSourceContext>,
    ) {
        self.owner = Some(owner);
        self.context = context;
    }
    pub(crate) fn matches_owner(
        &self,
        owner: crate::AudioCandidateIdentity,
        context: Option<crate::AudioSourceContext>,
    ) -> bool {
        self.owner == Some(owner) && self.context == context
    }
    pub(crate) fn bind_ordinary_context(&mut self, context: crate::AudioSourceContext) {
        self.context = Some(context);
    }
    pub(crate) fn check_ordinary_context(
        &self,
        context: crate::AudioSourceContext,
    ) -> Result<(), PlaybackActionError> {
        if !self.ordinary || self.owner.is_some() || self.context != Some(context) {
            return Err(PlaybackActionError::Session(
                SessionPlaybackError::InvalidCommand,
            ));
        }
        Ok(())
    }
    pub(crate) fn action(&self) -> PlaybackAction {
        self.action
    }
}
impl SessionPlayback {
    fn stream_action_expected(&self) -> Expected {
        Expected {
            // Reuse the existing nontransferable runtime incarnation; addresses
            // may be recycled while callers still retain a move-only plan.
            runtime: self.capture_runtime,
            revision: Arc::as_ptr(&self.revision).addr(),
            performance: self.performance_frame,
            arrangement: self.arrangement_frame,
            loop_pass: self.arrangement_loop_pass,
            routes: self.route_state_generation,
            playing: self.playing,
            anchor: self.anchor,
            periodic: self.periodic_tempo(),
        }
    }
    pub(crate) fn validate_stream_action(
        &self,
        prepared: &PreparedStreamAction,
    ) -> Result<(), SessionPlaybackError> {
        if (!prepared.ordinary && !self.supports_closed_stream_demand())
            || prepared.expected != self.stream_action_expected()
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        Ok(())
    }
    pub(crate) fn stream_action_targets_track(
        &self,
        prepared: &PreparedStreamAction,
        track: Option<TrackId>,
    ) -> Result<bool, SessionPlaybackError> {
        self.validate_stream_action(prepared)?;
        let Resolved::Route(_) = prepared.resolved else {
            return Ok(false);
        };
        let Some(track) = track else {
            return Ok(true);
        };
        if !self.routes.iter().any(|route| route.id == track) {
            return Err(SessionPlaybackError::UnknownTrack);
        }
        let command = match prepared.action {
            PlaybackAction::Session(command) | PlaybackAction::LaunchAndPlay(command) => command,
            _ => return Ok(false),
        };
        Ok(match command {
            SessionPlaybackCommand::LaunchClip { clip, .. } => {
                self.revision
                    .session()
                    .audio_clip(clip)
                    .is_some_and(|clip| clip.track == track)
                    || crate::session_inventory::EffectiveSessionMidi::new(
                        self.revision.session(),
                        self.midi_admission.as_ref(),
                    )
                    .midi_clip(clip)
                    .is_some_and(|clip| clip.track == track)
            }
            SessionPlaybackCommand::LaunchScene { scene, .. } => self
                .revision
                .session()
                .cells()
                .iter()
                .any(|cell| cell.scene == scene && cell.track == track),
            SessionPlaybackCommand::StopTrack { track: target, .. }
            | SessionPlaybackCommand::CueMidiClip { track: target, .. } => track == target,
            SessionPlaybackCommand::StopAllClips { .. }
            | SessionPlaybackCommand::BackToArrangement { .. } => true,
            _ => false,
        })
    }
    pub(crate) fn prepare_stream_action(
        &mut self,
        action: PlaybackAction,
    ) -> Result<PreparedStreamAction, SessionPlaybackError> {
        self.prepare_stream_action_in_scope(action, false)
    }
    pub(crate) fn prepare_stream_action_in_scope(
        &mut self,
        action: PlaybackAction,
        ordinary: bool,
    ) -> Result<PreparedStreamAction, SessionPlaybackError> {
        if (!ordinary && !self.supports_closed_stream_demand()) || self.linear_arrangement_export {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        let resolved = match action {
            PlaybackAction::Transport(PlaybackCommand::PlayFrom(frame)) => Resolved::Seek {
                frame,
                beat: self.play_from_coordinates(frame)?,
            },
            PlaybackAction::Transport(PlaybackCommand::Seek(frame))
            | PlaybackAction::Session(SessionPlaybackCommand::Seek(frame)) => Resolved::Seek {
                frame,
                beat: self.seek_coordinates(frame)?,
            },
            PlaybackAction::Session(
                command @ (SessionPlaybackCommand::LaunchClip { .. }
                | SessionPlaybackCommand::CueMidiClip { .. }
                | SessionPlaybackCommand::LaunchScene { .. }
                | SessionPlaybackCommand::StopTrack { .. }
                | SessionPlaybackCommand::StopAllClips { .. }
                | SessionPlaybackCommand::BackToArrangement { .. }),
            )
            | PlaybackAction::LaunchAndPlay(
                command @ (SessionPlaybackCommand::LaunchClip { .. }
                | SessionPlaybackCommand::LaunchScene { .. }),
            ) => Resolved::Route(self.plan_route_command(command)?),
            _ => return Err(SessionPlaybackError::InvalidCommand),
        };
        Ok(PreparedStreamAction {
            ordinary,
            expected: self.stream_action_expected(),
            action,
            resolved,
            owner: None,
            context: None,
        })
    }
    pub(crate) fn visit_prepared_stream_action(
        &mut self,
        prepared: &PreparedStreamAction,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.validate_stream_action(prepared)
            .map_err(|e| Invalid(e.diagnostic_message()))?;
        let (performance, arrangement, pass, anchor) = match prepared.resolved {
            Resolved::Seek { frame, beat } => {
                self.route_scratch.copy_from_slice(&self.routes);
                for route in &mut self.route_scratch {
                    route.pending = None;
                    route.owner = match route.owner {
                        RouteOwner::Session { clip, .. } => RouteOwner::Session {
                            clip,
                            origin: SessionMidiOrigin::launch(beat),
                        },
                        RouteOwner::SessionAudio { clip, .. } => RouteOwner::SessionAudio {
                            clip,
                            launch_frame: frame,
                        },
                        owner => owner,
                    };
                }
                (frame, frame, 0, None)
            }
            Resolved::Route(moment) => {
                let command = match prepared.action {
                    PlaybackAction::Session(command) | PlaybackAction::LaunchAndPlay(command) => {
                        command
                    }
                    _ => return Err(Invalid("stream route plan differs")),
                };
                let actual = self
                    .plan_route_command(command)
                    .map_err(|e| Invalid(e.diagnostic_message()))?;
                if actual.target_frame != moment.target_frame
                    || actual.target_beat != moment.target_beat
                    || actual.immediate != moment.immediate
                {
                    return Err(Invalid("stream route due time changed"));
                }
                let (arrangement, pass) = if moment.immediate && !prepared.playing_after_commit() {
                    // A paused command does not normalize the committed cursor.
                    // Play performs that normalization at the render frontier.
                    (self.arrangement_frame, self.arrangement_loop_pass)
                } else {
                    self.project_stream_arrangement(moment.target_frame)?
                };
                (moment.target_frame, arrangement, pass, self.anchor)
            }
        };
        if prepared.playing_after_commit() {
            self.project_due_stream_routes(performance, anchor)?;
        }
        self.visit_stream_chunk_with_history(
            prepared.ordinary,
            &self.route_scratch,
            performance,
            arrangement,
            self.route_state_generation,
            pass,
            prepared.playing_after_commit(),
            maximum_frames,
            visit,
            if matches!(&prepared.resolved, Resolved::Seek { .. }) {
                super::stream_demand::ScalarOriginHistory::Empty
            } else {
                super::stream_demand::ScalarOriginHistory::Current(self)
            },
        )
    }
    pub(super) fn project_stream_arrangement(
        &self,
        performance: u64,
    ) -> Result<(u64, u64), Invalid> {
        let delta = performance
            .checked_sub(self.performance_frame)
            .ok_or(Invalid("stream projection precedes current clock"))?;
        let frame = self
            .arrangement_frame
            .checked_add(delta)
            .ok_or(Invalid("stream projection overflow"))?;
        match self.active_loop_region() {
            Some(region) => {
                render::normalize_loop_cursor(frame, region, self.arrangement_loop_pass)
                    .map_err(|e| Invalid(e.diagnostic_message()))
            }
            None => Ok((frame, self.arrangement_loop_pass)),
        }
    }
    fn project_due_stream_routes(
        &mut self,
        performance: u64,
        anchor: Option<SessionClockAnchor>,
    ) -> Result<(), Invalid> {
        for index in 0..self.route_scratch.len() {
            let Some(pending) = self.route_scratch[index]
                .pending
                .filter(|pending| pending.frame <= performance)
            else {
                continue;
            };
            let is_audio = matches!(pending.target,RouteTarget::Session(clip) if self.revision.session().audio_clip(clip).is_some());
            let beat = if matches!(pending.target, RouteTarget::Session(_)) && !is_audio {
                timing_view_for(self.revision.session(), self.periodic_tempo(), anchor)
                    .frame_to_beat(pending.frame)
                    .map_err(|_| Invalid("stream projected beat unavailable"))?
            } else {
                pending.beat
            };
            let previous = self.route_scratch[index].owner;
            self.route_scratch[index].owner = self
                .owner_for_target(pending.target, beat, pending.frame)
                .map_err(|e| Invalid(e.diagnostic_message()))?;
            self.route_scratch[index].pending = None;
            let fade = self.audio_release_fade();
            // Other tracks may have transitioned before the requested frontier;
            // their releases start at their own due frames, not at this visit.
            super::audio_release::begin(
                &mut self.route_scratch[index],
                previous,
                pending.frame,
                fade,
            )
            .map_err(|e| Invalid(e.diagnostic_message()))?;
        }
        Ok(())
    }
    /// Observe an already scheduled original due boundary, including loop wrap.
    /// Native prewarm is revocable; current due demand must still be validated.
    pub(crate) fn visit_pending_stream_chunk(
        &mut self,
        due: u64,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.visit_pending_stream_chunk_in_scope(due, maximum_frames, visit, false)
    }
    pub(crate) fn visit_pending_stream_chunk_in_scope(
        &mut self,
        due: u64,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
        ordinary: bool,
    ) -> Result<StreamChunkWindow, Invalid> {
        if (!ordinary && !self.supports_closed_stream_demand())
            || !self
                .routes
                .iter()
                .any(|route| route.pending.is_some_and(|pending| pending.frame == due))
        {
            return Err(Invalid("stream scheduled boundary differs"));
        }
        let (arrangement, pass) = self.project_stream_arrangement(due)?;
        self.route_scratch.copy_from_slice(&self.routes);
        self.project_due_stream_routes(due, self.anchor)?;
        self.visit_stream_chunk_in_scope(
            ordinary,
            &self.route_scratch,
            due,
            arrangement,
            self.route_state_generation,
            pass,
            self.playing,
            maximum_frames,
            visit,
        )
    }
}
impl crate::playback::Playback {
    pub(crate) fn stream_action_targets_track(
        &mut self,
        prepared: &PreparedStreamAction,
        track: Option<TrackId>,
    ) -> Result<bool, PlaybackActionError> {
        self.session_runtime_mut_for_stream_action()?
            .stream_action_targets_track(prepared, track)
            .map_err(PlaybackActionError::Session)
    }
    pub(crate) fn prepare_stream_action(
        &mut self,
        action: PlaybackAction,
    ) -> Result<PreparedStreamAction, PlaybackActionError> {
        self.session_runtime_mut_for_stream_action()?
            .prepare_stream_action(action)
            .map_err(PlaybackActionError::Session)
    }
    pub(crate) fn prepare_ordinary_stream_action(
        &mut self,
        action: PlaybackAction,
    ) -> Result<PreparedStreamAction, PlaybackActionError> {
        self.session_runtime_mut_for_stream_action()?
            .prepare_stream_action_in_scope(action, true)
            .map_err(PlaybackActionError::Session)
    }
    pub(crate) fn visit_ordinary_pending_stream_chunk(
        &mut self,
        due: u64,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.session_runtime_mut_for_stream_action()
            .map_err(|_| Invalid("stream session unavailable"))?
            .visit_pending_stream_chunk_in_scope(due, maximum_frames, visit, true)
    }
    pub(crate) fn visit_prepared_stream_action(
        &mut self,
        prepared: &PreparedStreamAction,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.session_runtime_mut_for_stream_action()
            .map_err(|_| Invalid("stream session unavailable"))?
            .visit_prepared_stream_action(prepared, maximum_frames, visit)
    }
    pub(crate) fn visit_pending_stream_chunk(
        &mut self,
        due: u64,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.session_runtime_mut_for_stream_action()
            .map_err(|_| Invalid("stream session unavailable"))?
            .visit_pending_stream_chunk(due, maximum_frames, visit)
    }
    pub(crate) fn commit_stream_action(
        &mut self,
        prepared: PreparedStreamAction,
    ) -> Result<(), PlaybackActionError> {
        self.session_runtime_mut_for_stream_action()?
            .validate_stream_action(&prepared)
            .map_err(PlaybackActionError::Session)?;
        self.try_apply_action(prepared.action())
    }
}
