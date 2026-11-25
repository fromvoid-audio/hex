//! Spectrum analysis, sample/gain/parameter recording, and active route/transport publication.
use crate::prepared::*;

impl Callback {
    pub(in crate::prepared) fn begin_project_spectrum(&mut self, frames: usize) {
        let selection = self.status.project_spectrum.read().filter(|s| s.active);
        let active = self.active.as_ref().and_then(ActivePublication::current);
        let Some(project) = self
            .spectrum_capture
            .as_mut()
            .and_then(|c| c.project.as_mut())
        else {
            return;
        };
        let valid = match (selection, active) {
            (Some(selection), Some(active))
                if selection.sample_rate_hz == self.rate
                    && active.playback.sample_rate() == self.rate
                    && active.control_context_generation == selection.generation
                    && active.revision >= selection.activation_floor =>
            {
                if project.needs_source_validation(selection, active.revision) {
                    if active.playback.project_output_supported(selection.tracks()) {
                        project.source_validated(selection, active.revision);
                        true
                    } else {
                        false
                    }
                } else {
                    true
                }
            }
            _ => false,
        };
        project.begin(selection, frames, valid);
    }
    pub(in crate::prepared) fn finish_project_spectrum(
        &mut self,
        captured_at: Instant,
        output_valid: bool,
    ) -> Option<crate::spectrum_worker::CaptureOutcome> {
        let selection = self
            .spectrum_capture
            .as_ref()
            .and_then(|c| c.project.as_ref())
            .and_then(|p| p.selection())?;
        let active = self.active.as_ref().and_then(ActivePublication::current);
        let revision = active.as_ref().map_or(0, |active| active.revision);
        let valid = output_valid
            && !self.status.closing.load(Acquire)
            && self.status.project_spectrum.token() == selection.token
            && selection.sample_rate_hz == self.rate
            && active.is_some_and(|active| {
                active.control_context_generation == selection.generation
                    && active.playback.sample_rate() == self.rate
                    && active.revision >= selection.activation_floor
            });
        self.spectrum_capture
            .as_mut()?
            .project
            .as_mut()?
            .finish(captured_at, revision, valid)
    }
    pub(in crate::prepared) fn finish_analysis_block(
        &mut self,
        captured_start: Instant,
        output_valid: bool,
    ) -> (
        Option<crate::spectrum_worker::CaptureOutcome>,
        Option<crate::spectrum_worker::CaptureOutcome>,
    ) {
        let project = self.finish_project_spectrum(captured_start, output_valid);
        let node = self.finish_eq_spectrum(captured_start, output_valid);
        (node, project)
    }

    /// Accept or reject the staged post-node signal after the caller has
    /// validated the complete final output block. A skipped attempt consumes a
    /// per-target sequence so the next accepted window cannot bridge the gap.
    /// Finalizes every target; the return value describes the first active slot
    /// for single-target callers. Native delivery does not depend on this value.
    pub fn finish_spectrum_block(
        &mut self,
        captured_start: Instant,
        output_valid: bool,
    ) -> Option<crate::spectrum_worker::CaptureOutcome> {
        self.finish_analysis_block(captured_start, output_valid).0
    }

    pub(crate) fn set_sample_recording_output_clock(
        &mut self,
        mut clock: Option<crate::sample_recording::OutputClock>,
    ) {
        if let Some(clock) = &mut clock {
            clock.graph_latency_frames = self
                .active
                .as_ref()
                .and_then(ActivePublication::current)
                .and_then(|active| active.playback.main_output_latency_frames());
        }
        self.sample_recording.set_output_clock(clock);
    }

    pub(crate) fn finish_sample_recording(&mut self, output_valid: bool) {
        let recording_source = self.recording_source();
        if output_valid
            && self.sample_recording.selection().is_some_and(|layout| {
                self.status.closing.load(Acquire)
                    || layout
                        .expected_source
                        .is_some_and(|source| Some(source) != recording_source)
                    || self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::current)
                        .is_none_or(|active| {
                            active.control_context_generation != layout.generation
                                || !active.playback.project_sample_sources_supported(layout)
                        })
            })
        {
            self.sample_recording
                .end(crate::sample_recording::End::SourceChanged);
            return;
        }
        self.sample_recording.finish(output_valid);
    }
    pub(in crate::prepared) fn finish_eq_spectrum(
        &mut self,
        captured_start: Instant,
        output_valid: bool,
    ) -> Option<crate::spectrum_worker::CaptureOutcome> {
        let active = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .map(|active| {
                (
                    active.control_context_generation,
                    active.revision,
                    active.playback.sample_rate(),
                )
            });
        let mut first = None;
        for (slot, stage) in self.spectrum_stages.iter() {
            let Some(target) = stage.target else {
                continue;
            };
            let valid = output_valid
                && stage.complete()
                && self
                    .spectrum_target
                    .current_targets()
                    .and_then(|targets| targets.get(slot))
                    == Some(target)
                && self.spectrum_target.current_epoch() == stage.epoch
                && active.is_some_and(|(generation, revision, rate)| {
                    generation == target.project_generation
                        && revision >= target.activation_floor
                        && rate == target.sample_rate_hz
                })
                && target.sample_rate_hz == self.rate;
            let outcome = self.spectrum_capture.as_mut().map(|capture| {
                if valid {
                    capture.submit_pair_for_slot_at(
                        slot,
                        SpectrumStamp {
                            target: target.token,
                            generation: target.project_generation,
                            sample_rate_hz: target.sample_rate_hz,
                            applied_revision: active.map_or(0, |(_, revision, _)| revision),
                        },
                        &stage.input_frames[..stage.expected],
                        &stage.frames[..stage.expected],
                        captured_start,
                    )
                } else {
                    capture.skip_slot(slot)
                }
            });
            if first.is_none() {
                first = outcome;
            }
        }
        self.spectrum_stages.clear();
        first
    }

    pub(in crate::prepared) fn invalidate_gain_recording(
        &mut self,
        reason: crate::gain_recording::AbortReason,
    ) {
        self.revoke_native_effects();
        self.invalidate_transport_recording(reason);
    }

    pub(in crate::prepared) fn invalidate_for_transport_change(&mut self, action: PlaybackAction) {
        let reason = crate::gain_recording::AbortReason::TransportChanged;
        if is_pause_action(action) {
            // Pause retains the active graph for monitoring, release tails
            // and resume. The native endpoints belong to that same graph.
            self.invalidate_transport_recording(reason);
        } else {
            self.invalidate_gain_recording(reason);
        }
    }

    pub(in crate::prepared) fn invalidate_transport_recording(
        &mut self,
        reason: crate::gain_recording::AbortReason,
    ) {
        // Reuse the existing transport-discontinuity owner/epoch; asynchronous
        // output must not expose pre-seek/pause/stop buffers in a new context.
        self.revoke_forwarded_output_transport();
        self.abort_transport_inputs(reason);
        let prior_epoch = self
            .status
            .transport_epoch
            .fetch_update(AcqRel, Acquire, |epoch| epoch.checked_add(1));
        if matches!(reason, crate::gain_recording::AbortReason::OutputFailed) {
            self.status
                .last_output_fault_epoch
                .store(prior_epoch.unwrap_or_else(|epoch| epoch), Release);
        }
        if prior_epoch.is_err() {
            self.reject_paused_monitors(input_monitor::MonitorStatus::Stopped);
            self.status.gain_recording_fault.store(true, Release);
        }
    }
    /// Abort the gain take and native parameter gesture without revoking the
    /// output transport or advancing its epoch. A declicked Pause/Stop does
    /// this when consumed; the rest of `invalidate_gain_recording` follows at
    /// its fade end.
    pub(in crate::prepared) fn abort_transport_inputs(
        &mut self,
        reason: crate::gain_recording::AbortReason,
    ) {
        if matches!(reason, crate::gain_recording::AbortReason::TransportChanged) {
            self.sample_recording.end_bound_for_transport_change();
        }
        self.gain_recording.abort(reason);
        self.invalidate_native_parameter_input(match reason {
            crate::gain_recording::AbortReason::SourceChanged => {
                crate::native_parameter_input::AbortReason::SourceChanged
            }
            crate::gain_recording::AbortReason::TransportChanged => {
                crate::native_parameter_input::AbortReason::TransportChanged
            }
            crate::gain_recording::AbortReason::OutputFailed => {
                crate::native_parameter_input::AbortReason::OutputFailed
            }
            crate::gain_recording::AbortReason::Cancelled => {
                crate::native_parameter_input::AbortReason::Cancelled
            }
            crate::gain_recording::AbortReason::ClockExhausted => {
                crate::native_parameter_input::AbortReason::ClockExhausted
            }
        });
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.clear_gain_recording_override();
        }
    }
    pub(in crate::prepared) fn invalidate_native_parameter_input(
        &mut self,
        reason: crate::native_parameter_input::AbortReason,
    ) {
        self.native_parameter_input.abort(reason);
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.invalidate_native_parameter_input();
        }
    }
    pub(in crate::prepared) fn prepare_native_parameter_quantum(&mut self) {
        use crate::native_parameter_input::AbortReason;
        let source = self.status.recording_source();
        let transport_epoch = self.status.transport_epoch.load(Acquire);
        let unsupported_output = self.output_transport.is_some();
        let status = &self.status;
        let active = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut);
        let had_input = self.native_parameter_input.has_input();
        let live = self.native_parameter_input.prepare(|target| {
            if status.render_fault.load(Acquire)
                || target.transport_epoch <= status.last_output_fault_epoch.load(Acquire)
            {
                return Err(AbortReason::OutputFailed);
            }
            if status.native_parameter_input_revoked(target.gesture_serial) {
                return Err(AbortReason::Cancelled);
            }
            if unsupported_output {
                return Err(AbortReason::UnsupportedOutput);
            }
            let active = active.as_ref().ok_or(AbortReason::SourceChanged)?;
            if source != Some(target.source)
                || active.revision != target.source.audio_revision
                || active.control_context_generation != target.source.context.project_generation
                || active.playback.native_parameter_input_target() != Some(target.input)
            {
                return Err(AbortReason::SourceChanged);
            }
            if transport_epoch != target.transport_epoch || !active.playback.playing() {
                return Err(AbortReason::TransportChanged);
            }
            Ok(())
        });
        if let Some(mut active) = active {
            if let Some(operation) = live {
                if active
                    .playback
                    .set_native_parameter_input(operation)
                    .is_err()
                {
                    self.native_parameter_input
                        .abort(AbortReason::TargetUnavailable);
                    active.playback.invalidate_native_parameter_input();
                }
            } else if had_input {
                active.playback.invalidate_native_parameter_input();
            }
        }
    }
    /// Final checked host write, not physical loudspeaker delivery. Both Set
    /// and Clear wait for the actual processed sample plus remaining Main delay.
    pub fn finish_native_parameter_quantum(&mut self, valid: bool) {
        use crate::native_parameter_input::AbortReason;
        let invalidated = self.native_parameter_input.owner().and_then(|target| {
            if self.status.closing.load(Acquire)
                || self
                    .status
                    .native_parameter_input_revoked(target.gesture_serial)
            {
                Some(AbortReason::Cancelled)
            } else if self.status.pending_stop_marker.load(Acquire) != 0
                || self.status.transport_epoch.load(Acquire) != target.transport_epoch
            {
                Some(AbortReason::TransportChanged)
            } else if self.status.recording_source() != Some(target.source) {
                Some(AbortReason::SourceChanged)
            } else {
                None
            }
        });
        if let Some(reason) = invalidated {
            self.invalidate_native_parameter_input(reason);
            return;
        }
        let cancelled = self.native_parameter_input.owner().is_some_and(|target| {
            self.status
                .native_parameter_input_revoked(target.gesture_serial)
        });
        let mut active = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut);
        let application = active
            .as_mut()
            .and_then(|active| active.playback.take_native_parameter_application());
        let end = active
            .as_ref()
            .map_or(0, |active| active.playback.performance_frame());
        let had_input = self.native_parameter_input.has_input();
        let cleared =
            self.native_parameter_input
                .finish_quantum(application, end, valid, cancelled);
        if had_input
            && !self.native_parameter_input.has_input()
            && let Some(active) = &mut active
        {
            if cleared {
                active.playback.finish_native_parameter_input();
            } else {
                active.playback.invalidate_native_parameter_input();
            }
        }
        if self.native_parameter_input.receipt_fault {
            self.status
                .native_parameter_input_fault
                .store(true, Release);
        }
    }
    pub(in crate::prepared) fn prepare_gain_recording_quantum(&mut self) {
        use crate::gain_recording::{AbortReason, Clock};
        let transport_epoch = self.status.transport_epoch.load(Acquire);
        let status = &self.status;
        let active = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut);
        let live = self.gain_recording.prepare(|target| {
            if status.render_fault.load(Acquire)
                || target.transport_epoch <= status.last_output_fault_epoch.load(Acquire)
            {
                return Err(AbortReason::OutputFailed);
            }
            let active = active.as_ref().ok_or(AbortReason::SourceChanged)?;
            if active.revision != target.audio_revision
                || active.control_context_generation != target.project_generation
                || active.playback.sample_rate() != target.sample_rate
                || !active.playback.gain_recording_ready(target.track_id)
            {
                return Err(AbortReason::SourceChanged);
            }
            if transport_epoch != target.transport_epoch || !active.playback.playing() {
                return Err(AbortReason::TransportChanged);
            }
            let (content_frame, delivery_delay, loop_pass) = active
                .playback
                .gain_recording_clock(target.track_id)
                .ok_or(AbortReason::SourceChanged)?;
            Ok(Clock {
                performance_frame: active.playback.performance_frame(),
                content_frame,
                delivery_delay,
                loop_pass,
            })
        });
        if let Some(mut active) = active
            && active.playback.set_gain_recording_input(live).is_err()
        {
            active.playback.clear_gain_recording_override();
            self.gain_recording.abort(AbortReason::SourceChanged);
        }
    }
    /// Confirm the exact converted output quantum; this is sink delivery, not
    /// physical loudspeaker playback. Failure discards the entire pending take.
    pub fn finish_gain_recording_quantum(&mut self, output_valid: bool) {
        if !output_valid
            && let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
        {
            active.playback.clear_gain_recording_override();
        }
        let end = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .map_or(0, |active| active.playback.performance_frame());
        self.gain_recording.finish_quantum(end, output_valid);
        if self.gain_recording.receipt_fault {
            self.status.gain_recording_fault.store(true, Release);
        }
    }

    /// Drain committed applied Session route transitions from the active
    /// runtime and stamp them with this output's identity, installed revision,
    /// control generation, and transport epoch. Zero callback allocation; any
    /// overflow is latched on `Status` without perturbing audio.
    pub(in crate::prepared) fn flush_session_route_transitions(&mut self) {
        if self.status.closing.load(Acquire)
            || self.status.device_fault.load(Acquire)
            || self.status.render_fault.load(Acquire)
        {
            return;
        }
        let output_identity = self.candidate_root.root_id();
        let transport_epoch = self.status.transport_epoch.load(Acquire);
        let status = &self.status;
        let producer = &mut self.route_transitions;
        let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        else {
            return;
        };
        let audio_revision = active.revision;
        let control_generation = active.control_context_generation;
        let engine_overflowed =
            active
                .playback
                .drain_applied_session_route_transitions(|transition| {
                    producer.push(
                        status,
                        SessionRouteTransitionObservation {
                            output_identity,
                            audio_revision,
                            control_generation,
                            transport_epoch,
                            transition,
                        },
                    );
                });
        if engine_overflowed {
            producer.record_engine_overflow(status);
        }
    }

    pub(in crate::prepared) fn publish_active_routes(&mut self, force: bool) {
        let Some(active) = self.active.as_ref().and_then(ActivePublication::current) else {
            return;
        };
        let revision = active.revision;
        let playback = &active.playback;
        let generation = playback.session_route_state_generation();
        let key = (revision, generation, playback.session_clock_anchor());
        if !force && self.published_route_key == Some(key) {
            return;
        }

        self.status
            .sampler_console
            .store(playback.sampler_console_available(), Release);
        let capability = playback.session_capability_status();
        self.status
            .session_capability
            .store(capability as u32, Relaxed);
        let count = if capability == SessionCapabilityStatus::Available {
            playback.session_route_count()
        } else {
            0
        };
        self.status.begin_route_snapshot();
        for index in 0..MAX_TRACKS {
            #[cfg(test)]
            {
                self.route_slot_visits = self.route_slot_visits.saturating_add(1);
            }
            if index < count {
                if let Some(route) = playback.session_route_status(index) {
                    self.status.publish_route_slot(index, route);
                } else if let Some(slot) = self.status.route_slots.get(index) {
                    slot.clear();
                }
            } else if let Some(slot) = self.status.route_slots.get(index) {
                slot.clear();
            }
        }
        self.status.publish_route_header(
            revision,
            count,
            capability,
            generation,
            playback.performance_frame(),
            playback.session_clock_anchor(),
        );
        self.status.end_route_snapshot();
        self.published_route_key = Some(key);
    }
    pub(in crate::prepared) fn publish_transport(&self) {
        let observation =
            self.active
                .as_ref()
                .and_then(ActivePublication::current)
                .and_then(|active| {
                    active.playback.transport().map(
                        |(position, performance, playing, loop_state)| TransportObservation {
                            audio_revision: active.revision,
                            control_generation: active.control_context_generation,
                            consumed_command_ticket: self.status.consumed_command.load(Acquire),
                            arrangement_frame: position,
                            performance_frame: performance,
                            playing,
                            loop_state,
                        },
                    )
                });
        self.status.transport_observation.publish(observation);
        self.status
            .recording_source
            .publish(self.recording_source());
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let native_revision = self
            .native_sources
            .ordinary_runtime_requires_sources()
            .then(|| {
                self.active
                    .as_ref()
                    .and_then(ActivePublication::legacy)
                    .map(|active| active.revision)
            })
            .flatten()
            .unwrap_or(0);
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let native_revision = 0;
        self.status
            .ordinary_native_revision
            .store(native_revision, Release);
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(ActivePublication::Candidate(active)) = self.active.as_ref() {
            if let Some((position, frame, playing)) = active.owner.test_transport() {
                self.status.position.store(position, Relaxed);
                if !self.recovery_position_frozen {
                    self.status.recovery_position.store(position, Relaxed);
                }
                self.status.performance_frame.store(frame, Relaxed);
                self.status.playing.store(playing, Relaxed);
            }
            return;
        }
        let ready =
            self.active
                .as_ref()
                .and_then(ActivePublication::current)
                .is_some_and(|active| {
                    active.playback.playing()
                        && active.playback.gain_recording_track().is_some_and(|track| {
                            active.playback.gain_recording_clock(track).is_some()
                        })
                });
        self.status.gain_recording_ready.store(ready, Release);
        if let Some(active) = self.active.as_ref().and_then(ActivePublication::current) {
            let position = active.playback.position();
            self.status.position.store(position, Relaxed);
            self.publish_audible_position();
            if !self.recovery_position_frozen {
                self.status.recovery_position.store(position, Relaxed);
            }
            self.status
                .performance_frame
                .store(active.playback.performance_frame(), Relaxed);
            self.status
                .playing
                .store(active.playback.playing(), Relaxed);
            self.status
                .metronome_enabled
                .store(active.playback.metronome_enabled(), Relaxed);
        }
    }
}
