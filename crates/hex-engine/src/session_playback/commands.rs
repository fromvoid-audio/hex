use super::*;
use crate::MidiTiming;
mod pause_analytic;

impl SessionPlayback {
    pub fn apply(&mut self, command: SessionPlaybackCommand) -> Result<(), SessionPlaybackError> {
        self.admits_transport(&command)?;
        self.candidate_admission_open = false;
        if self.linear_arrangement_export
            && matches!(
                command,
                SessionPlaybackCommand::LaunchClip { .. }
                    | SessionPlaybackCommand::CueMidiClip { .. }
                    | SessionPlaybackCommand::LaunchScene { .. }
                    | SessionPlaybackCommand::StopTrack { .. }
                    | SessionPlaybackCommand::StopAllClips { .. }
                    | SessionPlaybackCommand::BackToArrangement { .. }
                    | SessionPlaybackCommand::CancelPending { .. }
            )
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.apply_admitted(command)
    }

    /// Refusals that depend only on this runtime's prepared shape, not on its
    /// clock or voice state. A declicked Pause/Stop is checked here when it is
    /// requested, before its fade begins.
    pub(crate) fn admits_transport(
        &self,
        command: &SessionPlaybackCommand,
    ) -> Result<(), SessionPlaybackError> {
        if self.stopped_input.is_some()
            && !matches!(
                command,
                SessionPlaybackCommand::Stop
                    | SessionPlaybackCommand::Pause
                    | SessionPlaybackCommand::Seek(_)
            )
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if self.current_sampler_console
            && matches!(
                command,
                SessionPlaybackCommand::Pause | SessionPlaybackCommand::Seek(1..)
            )
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if self.revision.requires_control_sampler_sources()
            && !self.current_sampler_console
            && !matches!(
                &command,
                SessionPlaybackCommand::Play
                    | SessionPlaybackCommand::Stop
                    | SessionPlaybackCommand::Seek(0)
            )
        {
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler supports frame-zero offline export only",
            ));
        }
        Ok(())
    }

    /// Validate PlayFrom without changing the running playback before a fade.
    pub(crate) fn admits_play_from(&self, frame: u64) -> Result<(), SessionPlaybackError> {
        if self.stopped_input.is_some() {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if self.linear_arrangement_export || self.revision.requires_control_sampler_sources() {
            self.admits_transport(&SessionPlaybackCommand::Seek(frame))?;
            self.admits_transport(&SessionPlaybackCommand::Play)
        } else {
            self.play_from_coordinates(frame).map(|_| ())
        }
    }

    fn apply_admitted(
        &mut self,
        command: SessionPlaybackCommand,
    ) -> Result<(), SessionPlaybackError> {
        match command {
            SessionPlaybackCommand::Play => {
                if !self.playing {
                    self.end_paused_source_interval();
                }
                if !self.playing
                    && let Some(keyboard) = &mut self.computer_keyboard
                    && self.revision.arrangement().tracks().iter().any(|track| {
                        track.canonical_track_id() == Some(keyboard.track())
                            && track.source_input_enabled()
                    })
                {
                    keyboard.clear_committed_muted_source_onsets();
                }
                self.playing = true;
            }
            SessionPlaybackCommand::Pause => {
                self.invalidate_native_parameter_input();
                if self.playing {
                    // Pause releases every held instrument note and never
                    // re-chases it on resume; only new note-ons play (#4139).
                    // Analytic routes capture releases and suppress the held
                    // occurrences. Every instrument track binds a native
                    // source, so its mutable runtime is a source owner and is
                    // cut below and rendered while paused. Only sampler-only
                    // offline export lacks one, and it refuses Pause.
                    self.pause_analytic_sources()?;
                    self.pending_shared_chase = None;
                    self.pending_shared_live = None;
                    self.foreign_arrangement.cut();
                    // A removed receiver may already own a detached release.
                    // Only explicit discontinuities reset admitted monitoring.
                    if !self.monitor_source_releases
                        && self.revision.arrangement().tracks().iter().any(|track| {
                            track.source_id().is_some() && track.receives_source_audio()
                        })
                        && !self.can_freeze_idle_latency_history()
                    {
                        self.monitor_source_releases = true;
                    }
                    for runtime in self
                        .mutable_instruments
                        .iter_mut()
                        .flatten()
                        .filter(|runtime| runtime.is_source_owner())
                    {
                        runtime.cut_gate_deadlines_at(self.performance_frame);
                    }
                    for tail in self.tail_mutable_instruments.iter_mut().flatten() {
                        if tail.runtime.is_source_owner() {
                            tail.runtime.cut_gate_deadlines_at(self.performance_frame);
                        }
                    }
                }
                self.playing = false;
                self.browser_audition.voices = Default::default();
            }
            SessionPlaybackCommand::Stop => {
                self.end_paused_source_interval();
                self.clear_gain_recording_override();
                self.invalidate_native_parameter_input();
                if self.stop_transport() {
                    self.advance_route_state_generation();
                }
            }
            SessionPlaybackCommand::Seek(frame) => {
                self.seek(frame)?;
                self.clear_gain_recording_override();
                self.invalidate_native_parameter_input();
            }
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

    /// A stopped preroll preserves authored state until an input actually needs
    /// the source kernel. Its first live activation releases those clip gates
    /// before monitoring; subsequent input replacement preserves source age.
    pub(crate) fn activate_source_monitor_on_control(
        &mut self,
    ) -> Result<(), SessionPlaybackError> {
        if !self.playing && !self.monitor_source_releases {
            self.pause_analytic_sources()?;
            for runtime in self
                .mutable_instruments
                .iter_mut()
                .flatten()
                .filter(|runtime| runtime.is_source_owner())
            {
                runtime.cut_gate_deadlines_at(self.performance_frame);
            }
            for tail in self.tail_mutable_instruments.iter_mut().flatten() {
                if tail.runtime.is_source_owner() {
                    tail.runtime.cut_gate_deadlines_at(self.performance_frame);
                }
            }
        }
        self.monitor_source_releases = true;
        Ok(())
    }

    fn end_paused_source_interval(&mut self) {
        if let Some(monitor) = &mut self.paused_source_monitor {
            monitor.end_resident_interval();
        }
    }

    pub(super) fn stop_transport(&mut self) -> bool {
        for fader in &mut self.track_faders[..self.routes.len()] {
            fader.revoke_resident_input();
        }
        self.browser_audition.voices = Default::default();
        self.foreign_arrangement.cut();
        let mut changed = self.performance_frame != 0;
        self.playing = false;
        self.monitor_source_releases = false;
        if let Some(input) = &mut self.stopped_input {
            input.rendered = false;
        }
        self.performance_frame = 0;
        self.arrangement_frame = 0;
        self.arrangement_loop_pass = 0;
        self.anchor = None;
        self.interactive_start_frame = None;
        self.pending_shared_chase = None;
        self.pending_shared_live = None;
        self.historical_floor = None;
        self.route_historical_floors.fill(None);
        self.analytic_continuations.fill(None);
        self.analytic_continuation_scratch.fill(None);
        self.releases.clear();
        self.suppressions.clear();
        self.release_order = 0;
        self.main_chain.reset();
        for chain in self.source_effects.chains_mut() {
            chain.reset();
        }
        for chain in &mut self.device_chains {
            chain.reset();
        }
        for chain in &mut self.return_chains {
            chain.reset();
        }
        self.return_effect_tails.reset();
        if let Some(compensation) = self.compensation.as_mut() {
            compensation.delays.reset_epoch();
        }
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
            route.current_first_frame = false;
            route.audio_release = None;
            changed |= route.pending.take().is_some();
            if let RouteOwner::Session { clip, origin } = route.owner {
                changed |= origin != SessionMidiOrigin::launch(BeatTime::ZERO);
                route.owner = RouteOwner::Session {
                    clip,
                    origin: SessionMidiOrigin::launch(BeatTime::ZERO),
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

    pub(super) fn seek_coordinates(
        &mut self,
        frame: u64,
    ) -> Result<BeatTime, SessionPlaybackError> {
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
            let owner = effective_midi_route(&self.revision, &self.routes, index)?.owner;
            if self.mutable_destination_has_active_occurrence(index, owner, frame)? {
                return Err(SessionPlaybackError::MutableInstrumentChase);
            }
        }
        let base_timing = base_timing_for(self.revision.session(), self.periodic_tempo_at(frame));
        let beat = base_timing
            .frame_to_beat(frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        Ok(beat)
    }

    fn seek(&mut self, frame: u64) -> Result<(), SessionPlaybackError> {
        let beat = self.seek_coordinates(frame)?;
        self.install_seek(frame, beat);
        Ok(())
    }

    /// Interactive discontinuity: reset old voices/effect tails, retrigger gate-active
    /// Arrangement notes at the destination, and retain their authored note-offs.
    /// No song-prefix replay, runtime construction or allocation occurs here.
    /// Strict Seek and control-side exact-history preroll retain their own laws.
    pub(crate) fn play_from(&mut self, frame: u64) -> Result<(), SessionPlaybackError> {
        if self.stopped_input.is_some() {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        if self.linear_arrangement_export || self.revision.requires_control_sampler_sources() {
            // Keep the independently admitted offline/private source policy.
            self.apply(SessionPlaybackCommand::Seek(frame))?;
            return self.apply(SessionPlaybackCommand::Play);
        }
        let beat = self.play_from_coordinates(frame)?;
        let generation = self.route_state_generation;
        self.install_seek(frame, beat);
        if self.route_state_generation == generation {
            self.advance_route_state_generation();
        }
        self.interactive_start_frame = Some(frame);
        self.clear_gain_recording_override();
        self.invalidate_native_parameter_input();
        self.candidate_admission_open = false;
        self.playing = true;
        Ok(())
    }

    pub(super) fn play_from_coordinates(
        &self,
        frame: u64,
    ) -> Result<BeatTime, SessionPlaybackError> {
        // Private offline/CURRENT-only source owners keep their capability
        // fences. Ordinary resource-backed samplers use the mutable event path.
        if self.current_sampler_console || self.revision.requires_control_sampler_sources() {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        frame
            .checked_add(1)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        base_timing_for(self.revision.session(), self.periodic_tempo_at(frame))
            .frame_to_beat(frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)
    }

    pub(super) fn install_seek(&mut self, frame: u64, beat: BeatTime) {
        self.end_paused_source_interval();
        self.browser_audition.voices = Default::default();
        self.monitor_source_releases = false;
        if let Some(input) = &mut self.stopped_input {
            input.rendered = false;
        }
        let mut changed = self.performance_frame != frame;
        for route in &self.routes {
            changed |= route.pending.is_some();
            if let RouteOwner::Session { origin, .. } = route.owner {
                changed |= origin != SessionMidiOrigin::launch(beat);
            }
            if let RouteOwner::SessionAudio { launch_frame, .. } = route.owner {
                changed |= launch_frame != frame;
            }
        }
        self.performance_frame = frame;
        self.arrangement_frame = frame;
        self.arrangement_loop_pass = 0;
        self.anchor = None;
        self.interactive_start_frame = None;
        self.pending_shared_chase = None;
        self.pending_shared_live = None;
        self.historical_floor = None;
        self.route_historical_floors.fill(None);
        self.restart_audio_history();
        for route in &mut self.routes {
            route.pending = None;
            if let RouteOwner::Session { clip, .. } = route.owner {
                route.owner = RouteOwner::Session {
                    clip,
                    origin: SessionMidiOrigin::launch(beat),
                };
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
    }

    /// The seek law's audio cut without moving the transport: analytic
    /// releases/continuations, device chains, return/effect tails, the
    /// compensation epoch and mutable voices start empty. Allocation-free:
    /// existing owners are cleared or reset in place.
    pub(super) fn restart_audio_history(&mut self) {
        self.foreign_arrangement.cut();
        for route in &mut self.routes {
            route.audio_release = None;
        }
        self.analytic_continuations.fill(None);
        self.analytic_continuation_scratch.fill(None);
        self.releases.clear();
        self.suppressions.clear();
        self.release_order = 0;
        self.main_chain.reset();
        for chain in self.source_effects.chains_mut() {
            chain.reset();
        }
        for chain in &mut self.device_chains {
            chain.reset();
        }
        for chain in &mut self.return_chains {
            chain.reset();
        }
        self.return_effect_tails.reset();
        if let Some(compensation) = self.compensation.as_mut() {
            compensation.delays.reset_epoch();
        }
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
    }

    pub(crate) fn cut_retained_foreign_gates_from(&mut self, previous: &mut Self) {
        // Exact source identity also proves the prepared generator index order.
        if Arc::ptr_eq(&self.revision, &previous.revision) {
            self.authored_clock_epoch = previous.authored_clock_epoch;
            self.foreign_arrangement
                .cut_retained_gates_from(&mut previous.foreign_arrangement);
        }
    }

    pub(super) fn mutable_destination_has_active_occurrence(
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
            RouteOwner::Session { clip, origin } => {
                let prepared = crate::session_inventory::EffectiveSessionMidi::new(
                    self.revision.session(),
                    self.midi_admission.as_ref(),
                )
                .midi_clip_for_track(clip, self.routes[index].id)
                .ok_or(SessionPlaybackError::UnknownClip)?;
                if !prepared.instrument_enabled()
                    || !prepared.instrument_template().is_event_source()
                {
                    return Ok(false);
                }
                let timing = timing_view_for(
                    self.revision.session(),
                    self.periodic_tempo_at(frame),
                    self.anchor,
                );
                prepared
                    .instrument_template()
                    .query_with_origin(origin, &timing, frame, end, &mut self.spans)
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
        self.plan_route_command(command)?;
        self.commit_route_scratch();
        Ok(())
    }

    /// Prepare only existing preallocated scalar/release scratch. No route,
    /// DSP, clock, owner or generation commit occurs here.
    pub(super) fn plan_route_command(
        &mut self,
        command: SessionPlaybackCommand,
    ) -> Result<TransitionMoment, SessionPlaybackError> {
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
            .retain(|entry| entry.end_frame() > self.performance_frame);
        let (target_frame, target_beat) = match command {
            SessionPlaybackCommand::LaunchClip { quantization, .. }
            | SessionPlaybackCommand::LaunchScene { quantization, .. }
            | SessionPlaybackCommand::StopTrack { quantization, .. }
            | SessionPlaybackCommand::StopAllClips { quantization }
            | SessionPlaybackCommand::BackToArrangement { quantization } => {
                self.scheduled_at(quantization)?
            }
            SessionPlaybackCommand::CueMidiClip { .. } => {
                self.scheduled_at(LaunchQuantization::Immediate)?
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
                let track = crate::session_inventory::EffectiveSessionMidi::new(
                    self.revision.session(),
                    self.midi_admission.as_ref(),
                )
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
                self.set_route(index, RouteTarget::Session(clip), moment, None)?;
            }
            SessionPlaybackCommand::CueMidiClip {
                track,
                clip,
                source_beat,
                expected_route_generation,
            } => {
                if expected_route_generation != self.route_state_generation {
                    return Err(SessionPlaybackError::InvalidCommand);
                }
                let index = self.route_index(track)?;
                if !matches!(self.routes[index].owner, RouteOwner::Session { clip: active, .. } if active == clip)
                {
                    return Err(SessionPlaybackError::InvalidCommand);
                }
                let prepared = crate::session_inventory::EffectiveSessionMidi::new(
                    self.revision.session(),
                    self.midi_admission.as_ref(),
                )
                .midi_clip_for_track(clip, track)
                .ok_or(SessionPlaybackError::UnknownClip)?;
                let elapsed = prepared
                    .source_template()
                    .earliest_elapsed_for_source(source_beat)
                    .map_err(SessionPlaybackError::Midi)?
                    .ok_or(SessionPlaybackError::InvalidCommand)?;
                let timing = self.timing_view();
                if timing
                    .beat_to_frame(target_beat)
                    .map_err(|_| SessionPlaybackError::TempoConversion)?
                    != self.performance_frame
                {
                    return Err(SessionPlaybackError::TempoConversion);
                }
                // CURRENT's same-runtime cut is proven only for its direct destination.
                // Other mutable fan-out has no equivalent atomic release reservation.
                for destination in 0..self.route_scratch.len() {
                    if destination != index
                        && self.revision.arrangement().tracks()[destination]
                            .midi_source_index
                            .unwrap_or(destination)
                            == index
                        && self
                            .mutable_instruments
                            .get(destination)
                            .and_then(Option::as_ref)
                            .is_some()
                    {
                        return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                            "midi cue does not support mutable fan-out",
                        ));
                    }
                }
                self.set_route(
                    index,
                    RouteTarget::Session(clip),
                    moment,
                    Some(SessionMidiOrigin::new(target_beat, elapsed)),
                )?;
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
                            let prepared = crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            )
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
                    self.set_route(index, target, moment, None)?;
                }
            }
            SessionPlaybackCommand::StopTrack { track, .. } => {
                let index = self.route_index(track)?;
                self.set_route(index, RouteTarget::Silence, moment, None)?;
            }
            SessionPlaybackCommand::StopAllClips { .. } => {
                for index in 0..self.route_scratch.len() {
                    self.set_route(index, RouteTarget::Silence, moment, None)?;
                }
            }
            SessionPlaybackCommand::BackToArrangement { .. } => {
                for index in 0..self.route_scratch.len() {
                    self.set_route(index, RouteTarget::Arrangement, moment, None)?;
                }
            }
            _ => return Err(SessionPlaybackError::InvalidCommand),
        }
        self.check_release_reservations(&self.route_scratch, self.release_scratch.len())?;
        self.release_scratch.sort_unstable_by_key(|entry| {
            (
                entry.location.track_index(),
                entry.location.tail_group(),
                entry.order,
            )
        });
        Ok(moment)
    }

    /// A MIDI contributor borrows its source's sole receiver runtime. Returning
    /// an index does not transfer its DSP or resource lease to the contributor.
    pub(super) fn shared_source_runtime_index(&self, contributor: usize) -> Option<usize> {
        let routing = self.revision.source_routing();
        let receiver = routing.receiver_index(contributor)?;
        self.mutable_instruments
            .get(receiver)
            .and_then(Option::as_ref)
            .filter(|runtime| routing.is_shared(contributor) || runtime.is_source_owner())
            .map(|_| receiver)
    }

    /// Reserve authored and live source ledgers plus every prospective contributor's
    /// full admitted voice envelope. Released voices stay charged through their
    /// quiet deadline. Silence is always allowed to release notes, even when a
    /// new launch would exceed the source's capacity.
    pub(super) fn check_shared_transition_capacity(
        &self,
        root: usize,
        next_owner: RouteOwner,
        frame: u64,
        speculative: bool,
    ) -> Result<(), SessionPlaybackError> {
        if matches!(
            next_owner,
            RouteOwner::Silence | RouteOwner::SessionAudio { .. }
        ) {
            return Ok(());
        }
        let routing = self.revision.source_routing();
        for receiver in 0..self.route_scratch.len() {
            let Some(runtime) = self.mutable_instruments[receiver].as_ref() else {
                continue;
            };
            if !routing.is_shared(receiver) || routing.receiver_index(receiver) != Some(receiver) {
                continue;
            }
            let contributors = routing.contributors(receiver);
            let affected = contributors.iter().any(|&index| {
                let index = usize::from(index);
                self.revision.arrangement().tracks()[index]
                    .midi_source_index
                    .unwrap_or(index)
                    == root
            });
            if !affected {
                continue;
            }
            let mut charge = runtime
                .transition_proofs(speculative)
                .iter()
                .filter(|occurrence| occurrence.quiet_deadline_frame > frame)
                .count()
                .checked_add(runtime.transition_live_count(speculative))
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            for &contributor in contributors {
                let index = usize::from(contributor);
                let track = &self.revision.arrangement().tracks()[index];
                if !track.source_input_enabled() {
                    continue;
                }
                let owner = if track.midi_source_index.unwrap_or(index) == root {
                    next_owner
                } else {
                    effective_midi_route(&self.revision, &self.route_scratch, index)?.owner
                };
                if matches!(owner, RouteOwner::Arrangement | RouteOwner::Session { .. }) {
                    charge = charge
                        .checked_add(self.source_voice_envelope(index))
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                }
            }
            if charge
                > runtime
                    .max_active_occurrences()
                    .min(runtime.transition_proof_capacity())
            {
                return Err(SessionPlaybackError::SharedSourceVoiceCapacity);
            }
        }
        Ok(())
    }

    fn commit_route_scratch(&mut self) {
        // Entire command/scene has passed all fallible planning. Accepted cuts
        // change only committed deadlines; DSP receives NoteOff next segment.
        if self.current_sampler_console {
            for index in 0..self.routes.len() {
                if self.routes[index].current_incarnation
                    != self.route_scratch[index].current_incarnation
                    && let Some(runtime) = self
                        .mutable_instruments
                        .get_mut(index)
                        .and_then(Option::as_mut)
                {
                    runtime.cut_gate_deadlines_at(self.performance_frame);
                }
            }
        }
        for contributor in 0..self.routes.len() {
            if self.routes[contributor].current_incarnation
                != self.route_scratch[contributor].current_incarnation
                && let Some(receiver) = self.shared_source_runtime_index(contributor)
                && let Some(runtime) = self.mutable_instruments[receiver].as_mut()
            {
                runtime.cut_contributor_at(self.routes[contributor].id, self.performance_frame);
            }
        }
        let route_changed = self.routes != self.route_scratch;
        std::mem::swap(&mut self.routes, &mut self.route_scratch);
        std::mem::swap(&mut self.releases, &mut self.release_scratch);
        std::mem::swap(&mut self.suppressions, &mut self.suppression_scratch);
        self.release_order = self.release_order_scratch;
        if route_changed {
            self.advance_route_state_generation();
        }
    }

    pub(crate) fn scheduled_at(
        &self,
        quantization: LaunchQuantization,
    ) -> Result<(u64, BeatTime), SessionPlaybackError> {
        quantization
            .scheduled_at(
                &self.timing_view(),
                self.revision.session().meter_map(),
                self.performance_frame,
                self.playing,
                self.anchor.map(|anchor| anchor.old_beat),
            )
            .map_err(|_| SessionPlaybackError::TempoConversion)
    }

    fn set_route(
        &mut self,
        index: usize,
        target: RouteTarget,
        moment: TransitionMoment,
        origin_override: Option<SessionMidiOrigin>,
    ) -> Result<(), SessionPlaybackError> {
        let owner = self
            .route_scratch
            .get(index)
            .ok_or(SessionPlaybackError::UnknownTrack)?
            .owner;
        let next_owner = match (
            self.owner_for_target(target, moment.target_beat, moment.performance_frame)?,
            origin_override,
        ) {
            (RouteOwner::Session { clip, .. }, Some(origin)) => {
                RouteOwner::Session { clip, origin }
            }
            (owner, None) => owner,
            _ => return Err(SessionPlaybackError::InvalidCommand),
        };
        let has_clip_volume = match next_owner {
            RouteOwner::Session { clip, .. } => self
                .revision
                .session()
                .midi_clip(clip)
                .is_some_and(|clip| clip.volume_program().is_some()),
            _ => false,
        };
        if next_owner != owner || matches!(target, RouteTarget::Session(_)) {
            self.check_shared_transition_capacity(index, next_owner, moment.target_frame, false)?;
        }
        let proven_current = self.current_sampler_route_track(index);
        let current_reservation = if proven_current
            && (next_owner != owner || matches!(target, RouteTarget::Session(_)))
        {
            let delta = moment
                .target_frame
                .checked_sub(moment.performance_frame)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            let arrangement_cut = moment
                .source_frame
                .checked_add(delta)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            Some(self.current_route_reservation(
                index,
                next_owner,
                moment.performance_frame,
                moment.target_frame,
                arrangement_cut,
                false,
            )?)
        } else {
            None
        };
        for destination in 0..self.route_scratch.len() {
            let track = &self.revision.arrangement().tracks()[destination];
            if track.midi_source_index.unwrap_or(destination) != index {
                continue;
            }
            if has_clip_volume
                && track
                    .foreign_instrument
                    .as_ref()
                    .is_some_and(|node| !node.bypassed)
            {
                return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                    "console clip volume is unavailable for vst3 instruments",
                ));
            }
            if !proven_current
                && self.shared_source_runtime_index(destination).is_none()
                && self
                    .mutable_instruments
                    .get(destination)
                    .and_then(Option::as_ref)
                    .is_some()
                && (next_owner != owner || origin_override.is_some())
            {
                if moment.target_frame > self.performance_frame && self.playing {
                    return Err(SessionPlaybackError::MutableInstrumentRouteBoundary);
                }
                if self.mutable_instruments[destination]
                    .as_ref()
                    .is_some_and(|runtime| !runtime.committed_is_quiescent())
                {
                    return Err(SessionPlaybackError::MutableInstrumentActive);
                }
                if matches!(next_owner, RouteOwner::Arrangement)
                    && self.mutable_destination_has_active_occurrence(
                        destination,
                        next_owner,
                        moment.source_frame,
                    )?
                {
                    return Err(SessionPlaybackError::MutableInstrumentChase);
                }
            }
        }
        let fade = self.audio_release_fade();
        if moment.immediate {
            if !proven_current && transition_captures(owner, target) {
                for destination in 0..self.route_scratch.len() {
                    if self.revision.arrangement().tracks()[destination]
                        .midi_source_index
                        .unwrap_or(destination)
                        == index
                    {
                        if self.shared_source_runtime_index(destination).is_some() {
                            self.route_scratch[destination].current_incarnation = self
                                .route_scratch[destination]
                                .current_incarnation
                                .checked_add(1)
                                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                        } else {
                            self.capture_outgoing(
                                destination,
                                owner,
                                moment.source_frame,
                                moment.performance_frame,
                            )?;
                        }
                    }
                }
            }
            let route = self
                .route_scratch
                .get_mut(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            route.pending = None;
            route.owner = next_owner;
            if self.playing {
                audio_release::begin(route, owner, moment.performance_frame, fade)?;
            }
            if let Some(reservation) = current_reservation {
                route.current_incarnation = reservation.incarnation;
                route.current_first_frame = true;
            }
            for destination in 0..self.route_scratch.len() {
                if self.revision.arrangement().tracks()[destination]
                    .midi_source_index
                    .unwrap_or(destination)
                    == index
                {
                    let id = self.route_scratch[destination].id;
                    self.suppression_scratch.retain(|entry| entry.track != id);
                }
            }
        } else {
            let route = self
                .route_scratch
                .get_mut(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            if next_owner != owner && audio_release::conflicts(route, moment.target_frame) {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            route.pending = Some(PendingRoute {
                frame: moment.target_frame,
                beat: moment.target_beat,
                target,
                current_reservation,
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
        self.capture_outgoing_with_source_binding(
            index,
            owner,
            source_frame,
            performance_frame,
            false,
        )
    }

    pub(super) fn capture_outgoing_with_source_binding(
        &mut self,
        index: usize,
        owner: RouteOwner,
        source_frame: u64,
        performance_frame: u64,
        keep_current_source: bool,
    ) -> Result<(), SessionPlaybackError> {
        let arrangement = self.revision.arrangement();
        let source_origin = ReleaseSourceOrigin::capture(&self.revision, index);
        let audio_index = self
            .revision
            .source_routing()
            .receiver_index(index)
            .unwrap_or(index);
        let audio_track = arrangement
            .tracks()
            .get(audio_index)
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        if source_origin.is_some() && !arrangement.tracks()[index].source_input_enabled() {
            return Ok(());
        }
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
                        if span.start >= source_frame
                            || is_suppressed_in(
                                &self.suppression_scratch,
                                self.routes[index].id,
                                ContinuationKey {
                                    clip: span.clip,
                                    note: span.note,
                                    iteration: span.iteration,
                                },
                                performance_frame,
                            )
                        {
                            return;
                        }
                        let voice = match MidiReleaseVoice::from_finite_span(
                            span.start,
                            span.gate_end,
                            span.end,
                            if keep_current_source {
                                sampler
                            } else {
                                sampler.detach(&self.analytic_continuations)
                            },
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
                            let clip_gain = match super::clip_gain::arrangement(
                                track,
                                self.revision.session().timing(),
                                span.clip,
                                source_frame,
                            ) {
                                Ok((gain, _)) => gain,
                                Err(error) => {
                                    failure = Some(error);
                                    return;
                                }
                            };
                            if let Err(error) = append_release(
                                &mut self.release_scratch,
                                self.release_capacity,
                                &mut next_order,
                                ReleaseEntry {
                                    source_origin,
                                    timeline: ReleaseTimeline::Arrangement,
                                    monitor_offset: 0,
                                    location: match ReleaseLocation::new(audio_index, None) {
                                        Ok(location) => location,
                                        Err(error) => {
                                            failure = Some(error);
                                            return;
                                        }
                                    },
                                    track_id: route.id,
                                    order: 0,
                                    mix: gain_recording::effective_mix(
                                        audio_track,
                                        source_frame,
                                        self.revision.gain_recording_track(),
                                        self.live_gain.audible(),
                                    ),
                                    continuation: keep_current_source.then_some(
                                        SessionContinuation {
                                            key: ContinuationKey {
                                                clip: span.clip,
                                                note: span.note,
                                                iteration: span.iteration,
                                            },
                                            onset_beat: span.onset_beat,
                                            gate_end_beat: span.gate_end_beat,
                                            pitch: span.pitch,
                                            channel: span.channel,
                                            velocity: span.velocity,
                                        },
                                    ),
                                    inline_tuning_owned: false,
                                    clip_gain,
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
            RouteOwner::Session { clip, origin } => {
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
                    if !keep_current_source {
                        entry.voice.detach(&self.analytic_continuations);
                    }
                    if !keep_current_source {
                        entry.continuation = None;
                    }
                }
                let prepared = crate::session_inventory::EffectiveSessionMidi::new(
                    self.revision.session(),
                    self.midi_admission.as_ref(),
                )
                .midi_clip_for_track(clip, self.routes[index].id)
                .ok_or(SessionPlaybackError::UnknownClip)?;
                if prepared.track != self.routes[index].id {
                    return Err(SessionPlaybackError::ClipTrackMismatch);
                }
                let end = performance_frame
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let timing =
                    timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor);
                let template = prepared.instrument_template();
                if let Some(floor) = self.historical_floor_for(index) {
                    template
                        .query_after_onset_with_origin(
                            origin,
                            &timing,
                            performance_frame,
                            end,
                            &mut self.spans,
                            floor,
                        )
                        .map_err(SessionPlaybackError::Midi)?;
                } else {
                    template
                        .query_with_origin(origin, &timing, performance_frame, end, &mut self.spans)
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
                        if !keep_current_source {
                            voice.detach(&self.analytic_continuations);
                        }
                        let route = self
                            .routes
                            .get(index)
                            .ok_or(SessionPlaybackError::UnknownTrack)?;
                        let mix = gain_recording::effective_mix(
                            audio_track,
                            source_frame,
                            self.revision.gain_recording_track(),
                            self.live_gain.audible(),
                        );
                        let clip_gain = super::clip_gain::at_cut(
                            self,
                            index,
                            owner,
                            span.clip,
                            source_frame,
                            performance_frame,
                        )?
                        .0;
                        append_release(
                            &mut self.release_scratch,
                            self.release_capacity,
                            &mut self.release_order_scratch,
                            ReleaseEntry {
                                source_origin,
                                timeline: ReleaseTimeline::Session,
                                monitor_offset: 0,
                                location: ReleaseLocation::new(audio_index, None)?,
                                track_id: route.id,
                                order: 0,
                                mix,
                                continuation: keep_current_source.then_some(SessionContinuation {
                                    key: ContinuationKey {
                                        clip: span.clip,
                                        note: span.note,
                                        iteration: span.iteration,
                                    },
                                    onset_beat: span.onset_beat,
                                    gate_end_beat: span.gate_end_beat,
                                    pitch: span.pitch,
                                    channel: span.channel,
                                    velocity: span.velocity,
                                }),
                                inline_tuning_owned: false,
                                clip_gain,
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
            RouteOwner::Session { clip, .. } => {
                crate::session_inventory::EffectiveSessionMidi::new(
                    self.revision.session(),
                    self.midi_admission.as_ref(),
                )
                .midi_clip_for_track(clip, self.routes[index].id)
                .map_or(0, |prepared| prepared.work().active_voices)
            }
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
            .try_fold(0_usize, |sum, (index, _)| {
                let route = effective_midi_route(&self.revision, routes, index)?;
                let pending = if !self.current_sampler_route_track(index)
                    && route
                        .pending
                        .is_some_and(|pending| transition_captures(route.owner, pending.target))
                {
                    self.owner_voice_budget(index, route.owner)
                } else {
                    0
                };
                let audio = routes.get(index).map_or(0, |route| {
                    audio_release::charged(route, self.performance_frame)
                });
                sum.checked_add(pending)
                    .and_then(|sum| sum.checked_add(audio))
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
