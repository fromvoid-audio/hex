//! Transport and runtime handoff, hosted move requests, and native removal transitions.
use super::*;

impl Playback {
    /// Preserve legacy finite transport behavior. Native publication uses the
    /// fallible companion to carry Session routes and reject unsafe demotions.
    /// A changed tempo map keeps the cursor's Arrangement beat. Refusal
    /// leaves this transport unchanged when the conversion cannot be proven.
    pub fn inherit_transport(&mut self, previous: &Self) -> Result<(), PlaybackHandoffError> {
        let position =
            remapped_linear_position(&self.project, &previous.project, previous.position())?;
        self.position = position;
        self.performance_frame = previous.performance_frame();
        self.performance_clock_exhausted = previous.performance_clock_exhausted();
        self.playing = previous.playing();
        self.inherit_metronome(previous);
        Ok(())
    }

    pub(super) fn inherit_metronome(&mut self, previous: &Self) {
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

    #[cfg(test)]
    pub(crate) fn test_swap_compensation_origins(
        &mut self,
        previous: &mut Self,
        groups: &mut crate::audio_candidate::groups::Transfer<'_>,
    ) -> Result<(usize, Option<usize>), crate::SessionPlaybackError> {
        let next = self
            .session_runtime
            .as_mut()
            .ok_or(crate::SessionPlaybackError::DeviceRuntime)?;
        let old = previous
            .session_runtime
            .as_mut()
            .ok_or(crate::SessionPlaybackError::DeviceRuntime)?;
        next.test_swap_compensation_origins(old, groups)
    }
    pub(crate) fn inherit_candidate_groups(
        &mut self,
        previous: &mut Self,
        groups: crate::audio_candidate::groups::Transfer<'_>,
        handoff: crate::audio_candidate::CandidateHandoff<'_>,
    ) -> Result<(), PlaybackHandoffError> {
        let confirmed_insert = matches!(
            handoff,
            crate::audio_candidate::CandidateHandoff::ApprovedInsert
        );
        if confirmed_insert && !previous.device_insert_held()
            || !confirmed_insert && previous.device_insert_cut_pending()
        {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        let next = self
            .session_runtime
            .as_mut()
            .ok_or(PlaybackHandoffError::ActiveSessionStateCannotBeDropped)?;
        let old = previous
            .session_runtime
            .as_mut()
            .ok_or(PlaybackHandoffError::ActiveSessionStateCannotBeDropped)?;
        next.inherit_candidate_groups(old, groups, handoff)
            .map_err(PlaybackHandoffError::Session)?;
        self.inherit_metronome(previous);
        if confirmed_insert {
            *self.transport_fade = std::mem::take(&mut *previous.transport_fade);
            self.release_device_insert_cut();
        }
        Ok(())
    }

    /// Inherit graph-local state at a publication boundary without allocating
    /// or retaining ownership of the previous graph.
    pub fn try_inherit_runtime_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<(), PlaybackHandoffError> {
        self.try_inherit_runtime_impl(previous, false, false, false, None)
    }

    /// Mark this prepared insert for the same no-restart law in stream
    /// preflight and callback commit. Only Session graphs support that proof.
    pub fn require_continuous_device_insert(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.require_continuous_insert = true;
        }
    }

    /// Mark an unpublished ordinary candidate for its exact hosted move proof.
    pub fn require_hosted_instrument_move(
        &mut self,
        movement: HostedInstrumentMove,
    ) -> Result<(), Invalid> {
        let (owner, binding) = self
            .foreign_instrument_binding_on_control(movement.receiver, movement.device)
            .ok_or(Invalid("hosted move receiver has no prepared worker"))?;
        if movement.donor == movement.receiver
            || owner != crate::ProcessorChainOwner::Track(movement.receiver.value())
            || movement.receiver_binding != binding
            || movement.donor_binding == binding
            || movement.donor_binding.root != binding.root
        {
            return Err(Invalid(
                "hosted instrument receiver differs from its fresh worker",
            ));
        }
        let runtime = self
            .session_runtime
            .as_mut()
            .ok_or(Invalid("hosted move has no session runtime"))?;
        runtime.hosted_move = Some(HostedMoveRequest::Instrument(movement));
        Ok(())
    }

    /// Mark one unpublished ordinary graph for the exact hosted effect cut.
    pub fn require_hosted_track_effect_move(
        &mut self,
        movement: HostedTrackEffectMove,
    ) -> Result<(), Invalid> {
        let installed = self
            .project
            .tracks
            .iter()
            .find(|track| track.canonical_track_id() == Some(movement.receiver));
        let matching = installed.is_some_and(|track| {
            matches!(track.device_chain.as_ref(), [node]
            if node.owner == crate::ProcessorChainOwner::Track(movement.receiver.value())
                && node.device_id == movement.device.value()
                && node.foreign_processor().is_some_and(|facts| {
                    !facts.is_instrument_profile() && facts.binding() == Some(movement.binding)
                }))
        });
        if movement.donor == movement.receiver || !matching {
            return Err(Invalid(
                "hosted effect receiver differs from the fresh worker",
            ));
        }
        let runtime = self
            .session_runtime
            .as_mut()
            .ok_or(Invalid("hosted effect move has no session runtime"))?;
        runtime.hosted_move = Some(HostedMoveRequest::Effect(movement));
        Ok(())
    }

    /// Fence a same-Track reorder to the fresh endpoint in this prepared graph.
    pub fn require_hosted_track_effect_reorder(
        &mut self,
        reorder: HostedTrackEffectReorder,
    ) -> Result<(), Invalid> {
        let owner = crate::ProcessorChainOwner::Track(reorder.track.value());
        let device = reorder.device.value();
        let matching = self.project.tracks.iter().any(|track| {
            track.canonical_track_id() == Some(reorder.track)
                && track.device_chain.iter().any(|node| {
                    node.owner == owner
                        && node.device_id == device
                        && node.foreign_processor().is_some_and(|facts| {
                            !facts.is_instrument_profile()
                                && facts.binding() == Some(reorder.binding)
                        })
                })
        });
        if !matching
            || reorder.binding == reorder.previous_binding
            || reorder.binding.root != reorder.previous_binding.root
            || reorder.binding.instance != device
            || reorder.previous_binding.instance != device
        {
            return Err(Invalid("hosted reorder differs from its fresh worker"));
        }
        let runtime = self
            .session_runtime
            .as_mut()
            .ok_or(Invalid("hosted reorder has no session runtime"))?;
        runtime.hosted_move = Some(HostedMoveRequest::Reorder(reorder));
        Ok(())
    }

    /// Advance the Engine's stage-local fade for an exact staged resident
    /// removal. The publication owner authenticates the source and command fence.
    pub fn advance_native_removal_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<crate::session_playback::NativeRemovalProgress, PlaybackHandoffError> {
        use crate::session_playback::NativeRemovalProgress;
        let (Some(next), Some(old)) = (&mut self.session_runtime, &mut previous.session_runtime)
        else {
            return Ok(NativeRemovalProgress::NotApplicable);
        };
        next.advance_native_removal_from(old)
            .map_err(PlaybackHandoffError::Session)
    }

    /// Plan a stage-local removal before the stream owner proves its pages.
    pub fn plan_native_removal_from(
        &mut self,
        previous: &Self,
    ) -> Result<bool, PlaybackHandoffError> {
        match (&mut self.session_runtime, &previous.session_runtime) {
            (Some(next), Some(old)) => next
                .plan_native_removal_from(old)
                .map_err(PlaybackHandoffError::Session),
            _ => Ok(false),
        }
    }

    /// Advance a planned stage fade with the publication window protected.
    pub fn advance_planned_native_removal_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<crate::session_playback::NativeRemovalProgress, PlaybackHandoffError> {
        match (&mut self.session_runtime, &mut previous.session_runtime) {
            (Some(next), Some(old)) => next
                .advance_planned_native_removal_from(old)
                .map_err(PlaybackHandoffError::Session),
            _ => Ok(crate::session_playback::NativeRemovalProgress::NotApplicable),
        }
    }

    /// True once this candidate has mutated the predecessor’s stage mix.
    pub fn native_removal_started(&self) -> bool {
        self.session_runtime
            .as_deref()
            .is_some_and(crate::SessionPlayback::native_removal_started)
    }

    /// Restore the original stage of an abandoned removal on its donor.
    pub fn cancel_native_removal_from(&mut self, previous: &mut Self) {
        if let (Some(next), Some(old)) = (&mut self.session_runtime, &mut previous.session_runtime)
        {
            next.cancel_native_removal_from(old);
        }
    }

    /// A previously armed stage must be restored if its route becomes ineligible.
    pub fn native_removal_pending(&self) -> bool {
        self.session_runtime
            .as_deref()
            .is_some_and(crate::SessionPlayback::native_removal_pending)
    }

    /// Exact active endpoint and settled dry PCM length needed at this cut.
    pub fn pending_foreign_main_pass(
        &self,
    ) -> Option<(hex_lattice_contract::ForeignProcessorBinding, u32)> {
        self.session_runtime.as_deref()?.pending_foreign_main_pass()
    }

    /// Supply a value-only snapshot from the active endpoint after host acceptance.
    /// It becomes audible only if this candidate inherits the same fenced predecessor.
    pub fn stage_foreign_main_pass_delay(
        &mut self,
        binding: hex_lattice_contract::ForeignProcessorBinding,
        delay: hex_lattice::MainPassDelay,
    ) -> bool {
        self.session_runtime
            .as_deref_mut()
            .is_some_and(|next| next.stage_foreign_main_pass_delay(binding, delay))
    }

    /// Identity and binding of the active Return whose input-aligned bypass
    /// must settle before the original worker and graph can be returned.
    pub fn pending_foreign_return_pass(
        &self,
    ) -> Option<(
        hex_lattice::ProcessorInstanceIdentity,
        hex_lattice_contract::ForeignProcessorBinding,
    )> {
        self.session_runtime
            .as_deref()?
            .pending_foreign_return_pass()
    }

    /// Latest performance frame for a running direct hosted effect cut.
    pub fn pending_hosted_track_effect_deadline(&self) -> Option<u64> {
        self.session_runtime
            .as_deref()?
            .pending_hosted_track_effect_deadline()
    }

    /// Stage Main PCM for the matching live Return cut after its exact endpoint
    /// settles. Uses the donor's current committed route stamp and inline copy.
    pub fn stage_foreign_return_pass_delay(
        &mut self,
        previous: &Self,
        identity: hex_lattice::ProcessorInstanceIdentity,
        binding: hex_lattice_contract::ForeignProcessorBinding,
    ) -> bool {
        self.session_runtime
            .as_deref_mut()
            .zip(previous.session_runtime.as_deref())
            .is_some_and(|(next, old)| next.stage_foreign_return_pass_delay(old, identity, binding))
    }

    /// Ordinary publication may rebind a fresh monitor to the same physical input client in
    /// the same explicitly known project generation. This carries no voice/DSP history or
    /// source compatibility. Unknown generations (including legacy defaults) authorize nothing.
    /// Selection precedes graph inheritance; old input custody changes only after it succeeds.
    pub fn try_inherit_runtime_in_control_context(
        &mut self,
        previous: &mut Self,
        generation: Option<u64>,
        previous_generation: Option<u64>,
    ) -> Result<(), PlaybackHandoffError> {
        self.try_inherit_runtime_impl(
            previous,
            generation.is_some() && generation == previous_generation,
            false,
            false,
            None,
        )
    }

    /// Use the graph-owned comparison data; the publication owner must validate
    /// the original endpoint and transaction before calling. The data is put
    /// back on every result and remains owned until control reclaims the graph.
    pub fn try_inherit_runtime_with_stored_foreign_continuation(
        &mut self,
        previous: &mut Self,
        generation: Option<u64>,
        previous_generation: Option<u64>,
    ) -> Result<(), PlaybackHandoffError> {
        if generation.is_none() || generation != previous_generation {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        self.try_inherit_runtime_impl(previous, true, false, true, None)
    }

    /// Borrow a control-prepared tempo anchor through the ordinary Playback
    /// lifecycle. Both graphs must have the same known control generation.
    /// The publication owner must validate the output, command,
    /// route, fault and cancellation fence and reclaim the plan on control.
    pub fn try_inherit_runtime_with_tempo_anchor(
        &mut self,
        previous: &mut Self,
        generation: Option<u64>,
        previous_generation: Option<u64>,
        anchor: &crate::session_playback::PreparedTempoAnchor,
    ) -> Result<(), PlaybackHandoffError> {
        if generation.is_none()
            || generation != previous_generation
            || self.session_runtime.is_none()
            || previous.session_runtime.is_none()
        {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        self.try_inherit_runtime_impl(previous, true, false, false, Some(anchor))
    }

    /// Reset an explicitly prepared bounce successor at a verified live
    /// Arrangement fence. The publication owner must validate the fence and
    /// unchanged source context in the callback before invoking this method.
    pub fn try_restart_runtime_under_arrangement_fence(
        &mut self,
        previous: &mut Self,
        generation: Option<u64>,
        previous_generation: Option<u64>,
    ) -> Result<(), PlaybackHandoffError> {
        if generation.is_none()
            || generation != previous_generation
            || !self.live_restart_requested()
            || previous.session_runtime.is_none()
        {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        self.try_inherit_runtime_impl(previous, true, true, false, None)
    }

    pub(super) fn try_inherit_runtime_impl(
        &mut self,
        previous: &mut Self,
        same_control_context: bool,
        fenced_restart: bool,
        use_stored_continuation: bool,
        tempo_anchor: Option<&crate::session_playback::PreparedTempoAnchor>,
    ) -> Result<(), PlaybackHandoffError> {
        if use_stored_continuation && !self.has_foreign_continuation_states() {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        if use_stored_continuation
            && (fenced_restart
                || tempo_anchor.is_some()
                || self.session_runtime.is_none()
                || previous.session_runtime.is_none())
        {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        if previous
            .session_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.midi_capture_active())
        {
            return Err(PlaybackHandoffError::Session(
                crate::SessionPlaybackError::DeviceRuntime,
            ));
        }
        let rebind_keyboard = same_control_context && self.can_rebind_keyboard_input(previous);
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
                // Input-only source schedulers carry no synthesis history.
                // Their exact live tokens can follow a source publication only
                // after Session's complete source runtime/import proof succeeds.
                let source_revision = same_control_context
                    && next.is_source_input()
                    && old.is_source_input()
                    && self
                        .prepared_revision
                        .as_ref()
                        .zip(previous.prepared_revision.as_ref())
                        .is_some_and(|(next_revision, old_revision)| {
                            let source =
                                |revision: &PreparedRevision, track: hex_project::TrackId| {
                                    revision
                                        .arrangement()
                                        .tracks()
                                        .iter()
                                        .position(|row| row.canonical_track_id() == Some(track))
                                        .and_then(|index| revision.source_routing().source(index))
                                };
                            source(next_revision, next.track())
                                .zip(source(old_revision, old.track()))
                                .is_some_and(|(next, old)| next == old)
                        });
                if (!exact_revision && !tuning_revision && !source_revision)
                    || !next.can_preserve_from(old)
                {
                    return Err(PlaybackHandoffError::Session(
                        crate::SessionPlaybackError::DeviceRuntime,
                    ));
                }
                true
            }
            _ => false,
        };
        let confirmed_insert_cut = previous.transport_fade.device_insert_held();
        match (&mut self.session_runtime, &mut previous.session_runtime) {
            (Some(next), Some(old)) => (if fenced_restart {
                next.try_inherit_state_fenced_restart_from(old)
            } else if use_stored_continuation {
                let Some(states) = next.foreign_continuation.take() else {
                    return Err(PlaybackHandoffError::RuntimeFenceChanged);
                };
                let result = match states.matcher() {
                    Ok(matcher) => {
                        next.try_inherit_state_live_with_foreign_continuation_from(old, &matcher)
                    }
                    Err(_) => Err(crate::SessionPlaybackError::DeviceRuntime),
                };
                next.foreign_continuation = Some(states);
                result
            } else if let Some(anchor) = tempo_anchor {
                next.try_inherit_state_with_tempo_anchor(old, anchor)
            } else if confirmed_insert_cut {
                next.try_inherit_state_at_device_insert_cut(old)
            } else {
                next.try_inherit_state_live_from(old)
            })
            .map(|_| ())
            .map_err(PlaybackHandoffError::Session)?,
            (Some(next), None) => {
                if previous.performance_clock_exhausted {
                    return Err(PlaybackHandoffError::PerformanceClockExhausted);
                }
                let position =
                    remapped_linear_position(&self.project, &previous.project, previous.position)?;
                next.validate_legacy_transport(position, previous.performance_frame)
                    .map_err(PlaybackHandoffError::Session)?;
                if legacy_arrangement_state_cannot_be_seeded(previous) {
                    return Err(PlaybackHandoffError::ArrangementStateCannotBeCaptured);
                }
                next.seed_legacy_transport(position, previous.performance_frame, previous.playing)
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
                self.position = remapped_linear_position(
                    &self.project,
                    &previous.project,
                    old.arrangement_frame(),
                )?;
                self.performance_frame = old.performance_frame();
                self.performance_clock_exhausted = false;
                self.playing = old.playing();
            }
            (None, None) => {
                if let (Some(candidate), Some(old)) = (
                    self.linear_runtime.as_ref(),
                    previous.linear_runtime.as_mut(),
                ) && candidate.transition_busy_from(&self.project, old, &previous.project)
                {
                    // Pulse #4558: a paused linear graph renders nothing, so
                    // settle its inaudible transitions instead of deferring
                    // forever; resume ramps in from silence.
                    if !previous.playing {
                        old.settle_idle_transitions_for(
                            &previous.project,
                            candidate,
                            &self.project,
                        );
                    }
                    if previous.playing
                        || candidate.transition_busy_from(&self.project, old, &previous.project)
                    {
                        // Pulse #4178: defer; never refuse into a fresh history.
                        return Err(PlaybackHandoffError::Session(
                            crate::SessionPlaybackError::DeviceTransitionBusy,
                        ));
                    }
                }
                if let (Some(candidate), Some(old)) = (
                    self.linear_runtime.as_ref(),
                    previous.linear_runtime.as_ref(),
                ) && !candidate.can_inherit_from(&self.project, old, &previous.project)
                {
                    return Err(PlaybackHandoffError::Session(
                        crate::SessionPlaybackError::DeviceRuntime,
                    ));
                }
                let position =
                    remapped_linear_position(&self.project, &previous.project, previous.position)?;
                if let (Some(candidate), Some(old)) = (
                    self.linear_runtime.as_mut(),
                    previous.linear_runtime.as_ref(),
                ) {
                    candidate.inherit_from(&self.project, old, &previous.project);
                }
                self.position = position;
                self.performance_frame = previous.performance_frame;
                self.performance_clock_exhausted = previous.performance_clock_exhausted;
                self.playing = previous.playing;
            }
        }
        if same_control_context
            && !fenced_restart
            && let (Some(next), Some(old)) = (&mut self.session_runtime, &previous.session_runtime)
        {
            next.inherit_browser_audition(old);
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
            if rebind_keyboard {
                previous.detach_rebound_keyboard_input();
            }
            previous.cancel_computer_keyboard();
        }
        // A fade in progress continues on the inheriting graph's transport.
        *self.transport_fade = std::mem::take(&mut *previous.transport_fade);
        Ok(())
    }

    pub(super) fn can_rebind_keyboard_input(&self, previous: &Self) -> bool {
        if self
            .session_runtime
            .as_ref()
            .zip(previous.session_runtime.as_ref())
            .is_some_and(|(next, old)| next.shares_foreign_keyboard(old))
        {
            return true;
        }
        self.session_runtime
            .as_ref()
            .and_then(|runtime| runtime.computer_keyboard.as_ref())
            .zip(
                previous
                    .session_runtime
                    .as_ref()
                    .and_then(|runtime| runtime.computer_keyboard.as_ref()),
            )
            .is_some_and(|(next, old)| next.can_rebind_input_from(old))
    }
    pub(super) fn detach_rebound_keyboard_input(&mut self) {
        if let Some(runtime) = self.session_runtime.as_mut() {
            runtime.detach_foreign_keyboard();
            if let Some(keyboard) = runtime.computer_keyboard.as_mut() {
                keyboard.detach_rebound_input();
            }
        }
    }

    /// Infallible input-only retirement at the callback's final accepted-install cut. Fresh
    /// seeks retain their reconstructed history and read held keys with fresh monitor voices.
    /// Call only after all fence/cancellation/refusal decisions, before canceling the old graph.
    pub fn detach_rebound_inputs_from(
        &self,
        previous: &mut Self,
        generation: Option<u64>,
        previous_generation: Option<u64>,
    ) {
        if generation.is_none() || generation != previous_generation {
            return;
        }
        if self.can_rebind_keyboard_input(previous) {
            previous.detach_rebound_keyboard_input();
        }
        if let (Some(next), Some(old)) = (&self.tuning_audition, &mut previous.tuning_audition)
            && next.can_rebind_input_from(old)
        {
            old.detach_rebound_input();
        }
    }
}

pub(super) fn legacy_arrangement_state_cannot_be_seeded(previous: &Playback) -> bool {
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
            .for_each_active_midi_span(previous.position, None, |_, _| active = true)
            .is_err()
            || active
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherit_transport_preserves_position_and_playing_state() {
        let project = Project::new(48_000, Vec::new()).unwrap();
        let mut previous = Playback::new(project.clone());
        previous
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(320)))
            .unwrap();
        let mut candidate = Playback::new(project);
        candidate.inherit_transport(&previous).unwrap();
        assert!(candidate.playing());
        assert_eq!(candidate.position(), 320);
    }
}
