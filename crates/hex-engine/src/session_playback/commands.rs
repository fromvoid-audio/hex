use super::*;
use crate::MidiTiming;

impl SessionPlayback {
    pub fn apply(&mut self, command: SessionPlaybackCommand) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        if self.linear_arrangement_export
            && matches!(
                command,
                SessionPlaybackCommand::LaunchClip { .. }
                    | SessionPlaybackCommand::LaunchScene { .. }
                    | SessionPlaybackCommand::StopTrack { .. }
                    | SessionPlaybackCommand::StopAllClips { .. }
                    | SessionPlaybackCommand::BackToArrangement { .. }
                    | SessionPlaybackCommand::CancelPending { .. }
            )
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        match command {
            SessionPlaybackCommand::Play => self.playing = true,
            SessionPlaybackCommand::Pause => self.playing = false,
            SessionPlaybackCommand::Stop => {
                if self.stop_transport() {
                    self.advance_route_state_generation();
                }
            }
            SessionPlaybackCommand::Seek(frame) => self.seek(frame)?,
            SessionPlaybackCommand::CancelPending { track } => {
                let mut changed = false;
                if let Some(track) = track {
                    let index = self.route_index(track)?;
                    changed = self.routes[index].pending.take().is_some();
                } else {
                    for route in &mut self.routes {
                        changed |= route.pending.take().is_some();
                    }
                }
                if changed {
                    self.advance_route_state_generation();
                }
            }
            command => self.apply_route_command(command)?,
        }
        Ok(())
    }

    fn stop_transport(&mut self) -> bool {
        let mut changed = self.performance_frame != 0;
        self.playing = false;
        self.performance_frame = 0;
        self.arrangement_frame = 0;
        self.arrangement_loop_pass = 0;
        self.anchor = None;
        self.historical_floor = None;
        self.route_historical_floors.fill(None);
        self.analytic_continuations.fill(None);
        self.analytic_continuation_scratch.fill(None);
        self.releases.clear();
        self.suppressions.clear();
        self.release_order = 0;
        for chain in &mut self.device_chains {
            chain.reset();
        }
        for chain in &mut self.return_chains {
            chain.reset();
        }
        self.return_effect_tails.reset();
        for chain in self.tail_groups.iter_mut().flatten() {
            chain.reset();
        }
        self.tail_output_active.fill(false);
        self.tail_output_scratch.fill(false);
        for runtime in self.mutable_instruments.iter_mut().flatten() {
            runtime.reset();
        }
        for tail in self.tail_mutable_instruments.iter_mut().flatten() {
            tail.runtime.reset();
        }
        for route in &mut self.routes {
            changed |= route.pending.take().is_some();
            if let RouteOwner::Session { clip, launch } = route.owner {
                changed |= launch != BeatTime::ZERO;
                route.owner = RouteOwner::Session {
                    clip,
                    launch: BeatTime::ZERO,
                };
            }
            if let RouteOwner::SessionAudio { clip, launch_frame } = route.owner {
                changed |= launch_frame != 0;
                route.owner = RouteOwner::SessionAudio {
                    clip,
                    launch_frame: 0,
                };
            }
        }
        changed
    }

    fn seek(&mut self, frame: u64) -> Result<(), SessionPlaybackError> {
        if self
            .tail_mutable_instruments
            .iter()
            .flatten()
            .any(|tail| !tail.runtime.committed_is_quiescent())
        {
            return Err(SessionPlaybackError::MutableInstrumentActive);
        }
        for index in 0..self.mutable_instruments.len() {
            let Some(runtime) = self.mutable_instruments[index].as_ref() else {
                continue;
            };
            if !runtime.committed_is_quiescent() {
                return Err(SessionPlaybackError::MutableInstrumentActive);
            }
            let owner = self
                .routes
                .get(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?
                .owner;
            if self.mutable_destination_has_active_occurrence(index, owner, frame)? {
                return Err(SessionPlaybackError::MutableInstrumentChase);
            }
        }
        let base_timing = base_timing_for(self.revision.session(), self.periodic_tempo);
        let beat = base_timing
            .frame_to_beat(frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let mut changed = self.performance_frame != frame;
        for route in &self.routes {
            changed |= route.pending.is_some();
            if let RouteOwner::Session { launch, .. } = route.owner {
                changed |= launch != beat;
            }
            if let RouteOwner::SessionAudio { launch_frame, .. } = route.owner {
                changed |= launch_frame != frame;
            }
        }
        self.performance_frame = frame;
        self.arrangement_frame = frame;
        self.arrangement_loop_pass = 0;
        self.anchor = None;
        self.analytic_continuations.fill(None);
        self.analytic_continuation_scratch.fill(None);
        self.releases.clear();
        self.suppressions.clear();
        self.historical_floor = None;
        self.route_historical_floors.fill(None);
        self.release_order = 0;
        for chain in &mut self.device_chains {
            chain.reset();
        }
        for chain in &mut self.return_chains {
            chain.reset();
        }
        self.return_effect_tails.reset();
        for chain in self.tail_groups.iter_mut().flatten() {
            chain.reset();
        }
        self.tail_output_active.fill(false);
        self.tail_output_scratch.fill(false);
        for runtime in self.mutable_instruments.iter_mut().flatten() {
            runtime.reset();
        }
        for tail in self.tail_mutable_instruments.iter_mut().flatten() {
            tail.runtime.reset();
        }
        for route in &mut self.routes {
            route.pending = None;
            if let RouteOwner::Session { clip, .. } = route.owner {
                route.owner = RouteOwner::Session { clip, launch: beat };
            }
            if let RouteOwner::SessionAudio { clip, .. } = route.owner {
                route.owner = RouteOwner::SessionAudio {
                    clip,
                    launch_frame: frame,
                };
            }
        }
        if changed {
            self.advance_route_state_generation();
        }
        Ok(())
    }

    fn mutable_destination_has_active_occurrence(
        &mut self,
        index: usize,
        owner: RouteOwner,
        frame: u64,
    ) -> Result<bool, SessionPlaybackError> {
        let end = frame
            .checked_add(1)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        match owner {
            RouteOwner::Arrangement => {
                let track = self
                    .revision
                    .arrangement()
                    .tracks()
                    .get(index)
                    .ok_or(SessionPlaybackError::UnknownTrack)?;
                let mut active = false;
                track
                    .for_each_mutable_midi_span(frame, end, |span| {
                        active |= span.start < frame && span.end > frame;
                    })
                    .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
                Ok(active)
            }
            RouteOwner::Session { clip, launch } => {
                let prepared = self
                    .revision
                    .session()
                    .midi_clip(clip)
                    .ok_or(SessionPlaybackError::UnknownClip)?;
                if !prepared.instrument_enabled()
                    || !prepared.instrument_template().is_event_source()
                {
                    return Ok(false);
                }
                let timing =
                    timing_view_for(self.revision.session(), self.periodic_tempo, self.anchor);
                prepared
                    .instrument_template()
                    .query(launch, &timing, frame, end, &mut self.spans)
                    .map_err(SessionPlaybackError::Midi)?;
                Ok(self
                    .spans
                    .spans()
                    .any(|span| span.start_frame < frame && span.end_frame > frame))
            }
            RouteOwner::SessionAudio { .. } | RouteOwner::Silence => Ok(false),
        }
    }

    fn route_index(&self, track: TrackId) -> Result<usize, SessionPlaybackError> {
        self.routes
            .iter()
            .position(|route| route.id == track)
            .ok_or(SessionPlaybackError::UnknownTrack)
    }

    fn apply_route_command(
        &mut self,
        command: SessionPlaybackCommand,
    ) -> Result<(), SessionPlaybackError> {
        self.route_scratch.copy_from_slice(&self.routes);
        self.release_scratch.clear();
        self.release_scratch.extend_from_slice(&self.releases);
        self.release_order_scratch = self.release_order;
        self.suppression_scratch.clear();
        self.suppression_scratch.extend(
            self.suppressions
                .iter()
                .copied()
                .filter(|entry| entry.until_frame > self.performance_frame),
        );
        self.release_scratch
            .retain(|entry| entry.voice.end_frame() > self.performance_frame);
        let (target_frame, target_beat) = match command {
            SessionPlaybackCommand::LaunchClip { quantization, .. }
            | SessionPlaybackCommand::LaunchScene { quantization, .. }
            | SessionPlaybackCommand::StopTrack { quantization, .. }
            | SessionPlaybackCommand::StopAllClips { quantization }
            | SessionPlaybackCommand::BackToArrangement { quantization } => {
                self.scheduled_at(quantization)?
            }
            _ => return Err(SessionPlaybackError::InvalidCommand),
        };
        let immediate = !self.playing || target_frame == self.performance_frame;
        let moment = TransitionMoment {
            target_frame,
            target_beat,
            immediate,
            source_frame: self.arrangement_frame,
            performance_frame: self.performance_frame,
        };
        match command {
            SessionPlaybackCommand::LaunchClip { clip, .. } => {
                let track = self
                    .revision
                    .session()
                    .midi_clip(clip)
                    .map(|prepared| prepared.track)
                    .or_else(|| {
                        self.revision
                            .session()
                            .audio_clip(clip)
                            .map(|prepared| prepared.track)
                    })
                    .ok_or(SessionPlaybackError::UnknownClip)?;
                let index = self.route_index(track)?;
                self.set_route(index, RouteTarget::Session(clip), moment)?;
            }
            SessionPlaybackCommand::LaunchScene { scene, .. } => {
                let (start, end) = self
                    .revision
                    .session()
                    .scene_cell_range(scene)
                    .ok_or(SessionPlaybackError::UnknownScene)?;
                for scene_position in start..end {
                    let cell_index = self
                        .revision
                        .session()
                        .scene_cell_index(scene_position)
                        .ok_or(SessionPlaybackError::UnknownScene)?;
                    let Some(cell) = self.revision.session().cells().get(cell_index).copied()
                    else {
                        return Err(SessionPlaybackError::UnknownScene);
                    };
                    let target = match cell.content {
                        crate::PreparedSessionCellContent::Stop => RouteTarget::Silence,
                        crate::PreparedSessionCellContent::Midi(clip) => {
                            let prepared = self
                                .revision
                                .session()
                                .midi_clip(clip)
                                .ok_or(SessionPlaybackError::UnknownClip)?;
                            if prepared.track != cell.track {
                                return Err(SessionPlaybackError::ClipTrackMismatch);
                            }
                            RouteTarget::Session(clip)
                        }
                        crate::PreparedSessionCellContent::Audio(clip) => {
                            let prepared = self
                                .revision
                                .session()
                                .audio_clip(clip)
                                .ok_or(SessionPlaybackError::UnknownClip)?;
                            if prepared.track != cell.track {
                                return Err(SessionPlaybackError::ClipTrackMismatch);
                            }
                            RouteTarget::Session(clip)
                        }
                    };
                    let index = self.route_index(cell.track)?;
                    self.set_route(index, target, moment)?;
                }
            }
            SessionPlaybackCommand::StopTrack { track, .. } => {
                let index = self.route_index(track)?;
                self.set_route(index, RouteTarget::Silence, moment)?;
            }
            SessionPlaybackCommand::StopAllClips { .. } => {
                for index in 0..self.route_scratch.len() {
                    self.set_route(index, RouteTarget::Silence, moment)?;
                }
            }
            SessionPlaybackCommand::BackToArrangement { .. } => {
                for index in 0..self.route_scratch.len() {
                    self.set_route(index, RouteTarget::Arrangement, moment)?;
                }
            }
            _ => return Err(SessionPlaybackError::InvalidCommand),
        }
        self.check_release_reservations(&self.route_scratch, self.release_scratch.len())?;
        self.release_scratch
            .sort_unstable_by_key(|entry| (entry.track_index, entry.tail_group, entry.order));
        let route_changed = self.routes != self.route_scratch;
        std::mem::swap(&mut self.routes, &mut self.route_scratch);
        std::mem::swap(&mut self.releases, &mut self.release_scratch);
        std::mem::swap(&mut self.suppressions, &mut self.suppression_scratch);
        self.release_order = self.release_order_scratch;
        if route_changed {
            self.advance_route_state_generation();
        }
        Ok(())
    }

    fn scheduled_at(
        &self,
        quantization: LaunchQuantization,
    ) -> Result<(u64, BeatTime), SessionPlaybackError> {
        let timing = self.timing_view();
        if !self.playing || quantization == LaunchQuantization::Immediate {
            let beat = timing
                .frame_to_beat(self.performance_frame)
                .map_err(|_| SessionPlaybackError::TempoConversion)?;
            return Ok((self.performance_frame, beat));
        }

        let current = timing
            .frame_to_beat(self.performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let bar_count = current.raw() / FOUR_QUARTERS_RAW;
        let last_boundary_raw = bar_count
            .checked_mul(FOUR_QUARTERS_RAW)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let last_boundary = BeatTime::from_raw(last_boundary_raw);
        if self
            .anchor
            .is_none_or(|anchor| last_boundary >= anchor.old_beat)
            && timing
                .beat_to_frame(last_boundary)
                .map_err(|_| SessionPlaybackError::TempoConversion)?
                == self.performance_frame
        {
            return Ok((self.performance_frame, last_boundary));
        }
        let next_raw = bar_count
            .checked_add(1)
            .and_then(|count| count.checked_mul(FOUR_QUARTERS_RAW))
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let next = BeatTime::from_raw(next_raw);
        let frame = timing
            .beat_to_frame(next)
            .map_err(|_| SessionPlaybackError::TempoConversion)?
            .max(self.performance_frame);
        Ok((frame, next))
    }

    fn set_route(
        &mut self,
        index: usize,
        target: RouteTarget,
        moment: TransitionMoment,
    ) -> Result<(), SessionPlaybackError> {
        let owner = self
            .route_scratch
            .get(index)
            .ok_or(SessionPlaybackError::UnknownTrack)?
            .owner;
        if self
            .mutable_instruments
            .get(index)
            .and_then(Option::as_ref)
            .is_some()
        {
            let next_owner =
                self.owner_for_target(target, moment.target_beat, moment.performance_frame)?;
            if next_owner != owner {
                if moment.target_frame > self.performance_frame && self.playing {
                    return Err(SessionPlaybackError::MutableInstrumentRouteBoundary);
                }
                if self.mutable_instruments[index]
                    .as_ref()
                    .is_some_and(|runtime| !runtime.committed_is_quiescent())
                {
                    return Err(SessionPlaybackError::MutableInstrumentActive);
                }
                if matches!(next_owner, RouteOwner::Arrangement)
                    && self.mutable_destination_has_active_occurrence(
                        index,
                        next_owner,
                        moment.source_frame,
                    )?
                {
                    return Err(SessionPlaybackError::MutableInstrumentChase);
                }
            }
        }
        if moment.immediate {
            if transition_captures(owner, target) {
                self.capture_outgoing(index, owner, moment.source_frame, moment.performance_frame)?;
            }
            let next_owner =
                self.owner_for_target(target, moment.target_beat, moment.performance_frame)?;
            let route = self
                .route_scratch
                .get_mut(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            route.pending = None;
            route.owner = next_owner;
            self.suppression_scratch
                .retain(|entry| entry.track != route.id);
        } else {
            let route = self
                .route_scratch
                .get_mut(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            route.pending = Some(PendingRoute {
                frame: moment.target_frame,
                beat: moment.target_beat,
                target,
            });
        }
        Ok(())
    }

    pub(super) fn capture_outgoing(
        &mut self,
        index: usize,
        owner: RouteOwner,
        source_frame: u64,
        performance_frame: u64,
    ) -> Result<(), SessionPlaybackError> {
        let arrangement = self.revision.arrangement();
        match owner {
            RouteOwner::Arrangement => {
                let track = arrangement
                    .tracks()
                    .get(index)
                    .ok_or(SessionPlaybackError::UnknownTrack)?;
                let mut failure = None;
                let mut next_order = self.release_order_scratch;
                track
                    .for_each_active_midi_span(source_frame, |span, sampler| {
                        // The transition boundary belongs to the incoming
                        // owner. A note whose onset equals `source_frame` has
                        // not rendered yet and must not be carried as an
                        // outgoing tail.
                        if span.start >= source_frame {
                            return;
                        }
                        let voice = match MidiReleaseVoice::from_finite_span(
                            span.start,
                            span.gate_end,
                            span.end,
                            sampler.detach(&self.analytic_continuations),
                            source_frame,
                            performance_frame,
                        ) {
                            Ok(voice) => voice,
                            Err(_) => {
                                failure = Some(SessionPlaybackError::ArithmeticOverflow);
                                return;
                            }
                        };
                        if let Some(voice) = voice {
                            let Some(route) = self.routes.get(index) else {
                                failure = Some(SessionPlaybackError::UnknownTrack);
                                return;
                            };
                            if let Err(error) = append_release(
                                &mut self.release_scratch,
                                self.release_capacity,
                                &mut next_order,
                                ReleaseEntry {
                                    track_index: index,
                                    track_id: route.id,
                                    order: 0,
                                    mix: track.mix(),
                                    tail_group: None,
                                    continuation: None,
                                    voice,
                                },
                            ) {
                                failure = Some(error);
                            }
                        }
                    })
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                self.release_order_scratch = next_order;
                if let Some(error) = failure {
                    return Err(error);
                }
            }
            RouteOwner::Session { clip, launch } => {
                // A prior handoff may have copied active held voices into the
                // shared release bank. Cut those copies in-place in scratch
                // before querying the current template so they are neither
                // left ringing nor captured twice.
                for entry in &mut self.release_scratch {
                    if entry.track_id != self.routes[index].id || entry.continuation.is_none() {
                        continue;
                    }
                    entry.voice = entry
                        .voice
                        .reconcile_at(performance_frame, None)
                        .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    entry.voice.detach(&self.analytic_continuations);
                    entry.continuation = None;
                }
                let prepared = self
                    .revision
                    .session()
                    .midi_clip(clip)
                    .ok_or(SessionPlaybackError::UnknownClip)?;
                if prepared.track != self.routes[index].id {
                    return Err(SessionPlaybackError::ClipTrackMismatch);
                }
                let end = performance_frame
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let timing =
                    timing_view_for(self.revision.session(), self.periodic_tempo, self.anchor);
                let template = prepared.instrument_template();
                if let Some(floor) = self.historical_floor_for(index) {
                    template
                        .query_after_onset(
                            launch,
                            &timing,
                            performance_frame,
                            end,
                            &mut self.spans,
                            floor,
                        )
                        .map_err(SessionPlaybackError::Midi)?;
                } else {
                    template
                        .query(launch, &timing, performance_frame, end, &mut self.spans)
                        .map_err(SessionPlaybackError::Midi)?;
                }
                for span in self.spans.spans() {
                    if is_suppressed_in(
                        &self.suppression_scratch,
                        self.routes[index].id,
                        ContinuationKey {
                            clip: span.clip,
                            note: span.note,
                            iteration: span.iteration,
                        },
                        performance_frame,
                    ) {
                        continue;
                    }
                    if span.start_frame >= performance_frame {
                        continue;
                    }
                    if let Some(mut voice) = span
                        .release_at(performance_frame)
                        .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?
                    {
                        voice.detach(&self.analytic_continuations);
                        let route = self
                            .routes
                            .get(index)
                            .ok_or(SessionPlaybackError::UnknownTrack)?;
                        let mix = arrangement
                            .tracks()
                            .get(index)
                            .ok_or(SessionPlaybackError::UnknownTrack)?
                            .mix();
                        append_release(
                            &mut self.release_scratch,
                            self.release_capacity,
                            &mut self.release_order_scratch,
                            ReleaseEntry {
                                track_index: index,
                                track_id: route.id,
                                order: 0,
                                mix,
                                tail_group: None,
                                continuation: None,
                                voice,
                            },
                        )?;
                    }
                }
            }
            RouteOwner::SessionAudio { .. } => {}
            RouteOwner::Silence => {}
        }
        Ok(())
    }

    fn owner_voice_budget(&self, index: usize, owner: RouteOwner) -> usize {
        match owner {
            RouteOwner::Arrangement => self
                .revision
                .arrangement()
                .tracks()
                .get(index)
                .filter(|track| track.has_instrument_source())
                .map_or(0, |_| MAX_SIMULTANEOUS_VOICES_PER_TRACK),
            RouteOwner::Session { clip, .. } => self
                .revision
                .session()
                .midi_clip(clip)
                .map_or(0, |prepared| prepared.work.active_voices),
            RouteOwner::SessionAudio { .. } => 0,
            RouteOwner::Silence => 0,
        }
    }

    fn check_release_reservations(
        &self,
        routes: &[TrackRoute],
        release_count: usize,
    ) -> Result<(), SessionPlaybackError> {
        let reserved = routes
            .iter()
            .enumerate()
            .try_fold(0_usize, |sum, (index, route)| {
                let pending = if route
                    .pending
                    .is_some_and(|pending| transition_captures(route.owner, pending.target))
                {
                    self.owner_voice_budget(index, route.owner)
                } else {
                    0
                };
                sum.checked_add(pending)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)
            })?;
        if release_count
            .checked_add(reserved)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?
            > self.release_capacity
        {
            return Err(SessionPlaybackError::ReleaseBankFull);
        }
        Ok(())
    }
}

fn is_suppressed_in(
    suppressions: &[MidiOccurrenceSuppression],
    track: TrackId,
    key: ContinuationKey,
    frame: u64,
) -> bool {
    let target = (track, key.clip, key.note, key.iteration, 0);
    let start = suppressions.partition_point(|entry| {
        (
            entry.track,
            entry.clip,
            entry.note,
            entry.iteration,
            entry.loop_pass,
        ) < target
    });
    suppressions[start..]
        .iter()
        .take_while(|entry| {
            (
                entry.track,
                entry.clip,
                entry.note,
                entry.iteration,
                entry.loop_pass,
            ) == target
        })
        .any(|entry| entry.mutable_signature().is_none() && entry.until_frame > frame)
}

pub(super) fn transition_captures(owner: RouteOwner, target: RouteTarget) -> bool {
    !matches!(
        (owner, target),
        (RouteOwner::Arrangement, RouteTarget::Arrangement)
            | (RouteOwner::Silence, RouteTarget::Silence)
    )
}
