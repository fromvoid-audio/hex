//! Block start, command consumption, actions, processor auditions, and fault fencing.
use crate::prepared::*;

impl Callback {
    pub fn begin_block(&mut self) {
        self.begin_block_inner();
        self.flush_stream_outcomes();
    }
    pub(in crate::prepared) fn begin_block_inner(&mut self) {
        self.cold_device_fade = None;
        self.transport_setup_open = false;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.paused_monitor_host_frames = None;
        }
        self.device_fault_fenced_in_block = false;
        let closing = self.status.closing.load(Acquire);
        let device_fault = self.status.device_fault.load(Acquire);
        self.input_monitor.begin_block(closing, device_fault);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.paused_notes.begin_block(closing, device_fault);
        if self.status.device_fault.load(Acquire) && !self.status.closing.load(Acquire) {
            self.fence_device_fault();
            return;
        }
        if self.status.closing.load(Acquire) || self.status.pending_stop_marker.load(Acquire) != 0 {
            self.revoke_output_transport();
        }
        self.sample_preview
            .begin_block(self.status.closing.load(Acquire));
        if self.status.closing.load(Acquire) {
            self.invalidate_midi_recording(crate::midi_recording::AbortReason::Cancelled);
            self.invalidate_gain_recording(crate::gain_recording::AbortReason::Cancelled);
        }
        self.processor_audition.begin_block();
        self.apply_processor_audition_boundary();
        self.spectrum_target.begin_block();
        #[cfg(test)]
        if let Some(barrier) = self.test_after_spectrum_target_boundary.take() {
            barrier.wait();
            barrier.wait();
        }
        if let Some(publication) = self.held_retirement.take() {
            self.return_publication(publication);
            if self.held_retirement.is_some() {
                self.abort_blocked_verified_insert();
                self.abort_blocked_device_insert();
                // Ownership backpressure must not delay the independent Stop
                // path. Keep the graph retained while consuming bounded control.
                if self.status.closing.load(Acquire) {
                    self.status.playing.store(false, Relaxed);
                } else {
                    self.consume_commands();
                }
                return;
            }
        }
        self.return_requested_domain_pages();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.native_sources.refresh_runtime_requirements(
            self.active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|active| active.playback.as_ref()),
        );
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.return_requested_native_sources();
        if self.status.closing.load(Acquire) {
            // A candidate return may end this block early; closing is already
            // silent and must expose stopped status at every such frontier.
            self.status.playing.store(false, Relaxed);
            self.release_device_insert_fade_pins();
            if !self.retired.is_full()
                && let Some(mut value) = self.deferred_eligible.take()
            {
                if value.receipt().is_some() {
                    value.cancel_stream_readiness(&mut self.domain_pages);
                }
                self.return_publication(ReturnedPublication::Eligible(
                    value.reject_before_install(),
                ));
                self.status.publication_pending.store(false, Release);
            }
            if !self.retired.is_full()
                && let Some(publication) = self.deferred.take()
            {
                self.return_rejected_graph(publication);
            }
            if !self.retired.is_full() {
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::current_mut)
                {
                    active.playback.cancel_all_processor_auditions();
                }
                if matches!(self.active, Some(ActivePublication::Legacy(_)))
                    && let Some(ActivePublication::Legacy(publication)) = self.active.take()
                {
                    self.return_graph(publication);
                }
            }
            if !self.retired.is_full()
                && let Ok(publication) = self.publications.pop()
            {
                let publication = match publication {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    QueuedPublication::NativeEpoch { .. } => return,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    QueuedPublication::Native(endpoint) => {
                        self.return_publication(ReturnedPublication::Native(endpoint));
                        return;
                    }
                    QueuedPublication::Domain(pages) => {
                        self.return_publication(ReturnedPublication::Domain(pages));
                        return;
                    }
                    QueuedPublication::Eligible(value) => {
                        self.return_publication(ReturnedPublication::Eligible(
                            value.reject_before_install(),
                        ));
                        self.status.publication_pending.store(false, Release);
                        return;
                    }
                    QueuedPublication::Legacy(publication) => publication,
                    QueuedPublication::Candidate(candidate) => {
                        self.return_publication(ReturnedPublication::Candidate(
                            candidate.reject_before_install(),
                        ));
                        return;
                    }
                };
                self.return_rejected_graph(publication);
            }
            self.status.playing.store(false, Relaxed);
            if self.active.is_none()
                && self.published_route_key.is_some()
                && self.deferred.is_none()
                && self.deferred_eligible.is_none()
                && self.publications.is_empty()
            {
                self.status.publish_empty_routes();
                self.published_route_key = None;
            }
            if self.active.is_none()
                && self.deferred.is_none()
                && self.deferred_eligible.is_none()
                && self.publications.is_empty()
                && self.held_retirement.is_none()
                && self.sample_preview.is_empty()
            {
                self.status.shutdown_complete.store(true, Release);
            }
            return;
        }
        if self.deferred.is_none()
            && self.deferred_eligible.is_none()
            && (self.active.is_none() || !self.retired.is_full())
            && let Ok(publication) = self.publications.pop()
        {
            match publication {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                QueuedPublication::NativeEpoch { target, fence } => {
                    self.consume_commands();
                    self.return_native_epoch_runtime(target, fence);
                    return;
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                QueuedPublication::Native(endpoint) => {
                    if let Err(endpoint) = self.install_native_source_endpoint(endpoint) {
                        self.return_publication(ReturnedPublication::Native(endpoint));
                    }
                    self.consume_commands();
                    return;
                }
                QueuedPublication::Domain(pages) => {
                    if let Some(index) = pages.index_for(self.candidate_root.root_id())
                        && self.domain_pages[index].is_none()
                    {
                        self.domain_pages[index] = Some(pages);
                        if let Some(pages) = self.domain_pages[index].as_ref() {
                            pages.installed_on_callback();
                        }
                    } else {
                        self.return_publication(ReturnedPublication::Domain(pages));
                    }
                }
                QueuedPublication::Eligible(value) => {
                    self.deferred_eligible = Some(value);
                }
                QueuedPublication::Legacy(publication) => {
                    if matches!(
                        self.active,
                        Some(ActivePublication::Candidate(_) | ActivePublication::Eligible(_))
                    ) {
                        self.return_graph(publication);
                        self.consume_commands();
                        return;
                    }
                    self.deferred = Some(publication);
                }
                QueuedPublication::Candidate(candidate) => {
                    if let Err(candidate) = self.install_candidate(candidate) {
                        // Default production admission remains an unconditional
                        // refusal before active installation or render.
                        self.return_publication(ReturnedPublication::Candidate(
                            candidate.reject_before_install(),
                        ));
                    }
                    // Rejection must not starve priority Stop or the bounded
                    // command prefix belonging to the still-active graph.
                    self.consume_commands();
                    return;
                }
            }
        }

        if self.deferred_eligible.is_some() {
            self.advance_eligible_publication(None);
            self.consume_commands();
            return;
        }

        self.advance_legacy_publication(None);
        self.consume_commands();
    }

    pub(in crate::prepared) fn reject_publication(
        &mut self,
        revision: u64,
        reason: PlaybackHandoffRejection,
    ) {
        self.status.publish_rejected_revision(revision, reason);
        self.trace_audio(
            AudioDiagnosticKind::PublicationRejected { reason },
            revision,
            None,
            0,
        );
    }

    pub(in crate::prepared) fn trace_audio(
        &mut self,
        kind: AudioDiagnosticKind,
        revision: u64,
        context: Option<hex_engine::AudioSourceContext>,
        frames: usize,
    ) {
        if !self
            .status
            .audio_diagnostics_flags()
            .contains(kind.category())
        {
            return;
        }
        let event = AudioDiagnosticEvent {
            sequence: 0,
            output: self.candidate_root.root_id(),
            revision,
            context,
            applied_revision: self.status.applied_revision.load(Relaxed),
            consumed_command: self.status.consumed_command.load(Relaxed),
            performance_frame: self.status.performance_frame.load(Relaxed),
            arrangement_frame: self.status.position.load(Relaxed),
            rate: self.rate,
            frames,
            commands_pending: self.commands.slots(),
            retirement_slots: self.retired.slots(),
            kind,
        };
        self.diagnostics
            .push(event, &self.status.diagnostics_dropped);
    }
    /// Native adapter supplies measured host callback duration and entry cadence.
    /// Reading the clock and formatting remain outside this record operation.
    pub(crate) fn trace_callback_timing(
        &mut self,
        elapsed_ns: u64,
        deadline_ns: u64,
        frames: usize,
        start_interval_ns: Option<u64>,
    ) {
        self.trace_audio(
            AudioDiagnosticKind::CallbackTiming {
                elapsed_ns,
                deadline_ns,
                start_interval_ns,
            },
            self.status.applied_revision.load(Relaxed),
            None,
            frames,
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(in crate::prepared) fn native_parameter_fence_matches(
        &self,
        source: AppliedAudioSource,
        transport_epoch: u64,
        playing: bool,
        successor: Option<hex_engine::AudioSourceContext>,
    ) -> bool {
        let predecessor = self.active.as_ref().and_then(ActivePublication::legacy);
        self.status.native_parameter_source(playing) == Some((source, transport_epoch))
            && source.output_identity == self.candidate_root.root_id()
            && predecessor.is_some_and(|active| {
                active.revision == source.audio_revision
                    && active.source_context == Some(source.context)
            })
            && successor.is_some_and(|context| {
                context.project_generation == source.context.project_generation
                    && source.context.editor_revision.checked_add(1)
                        == Some(context.editor_revision)
            })
    }

    pub(in crate::prepared) fn paused_input_graph_handoff(
        &self,
        next: &Publication,
    ) -> Option<(AppliedAudioSource, AppliedAudioSource, u64)> {
        let previous = self.active.as_ref()?.legacy()?;
        if !next.policy.inherits_runtime()
            || previous.playback.playing()
            || next.playback.sample_rate() != self.rate
            || self.status.pending_stop_marker.load(Acquire) != 0
        {
            return None;
        }
        let source = self.recording_source()?;
        let context = next.source_context?;
        if context.project_generation != source.context.project_generation {
            return None;
        }
        let successor = AppliedAudioSource {
            context,
            audio_revision: next.revision,
            output_identity: source.output_identity,
        };
        Some((source, successor, self.status.transport_epoch.load(Acquire)))
    }

    pub(in crate::prepared) fn runtime_fence_changed(
        status: &Status,
        active: Option<&Publication>,
        fence: FreshRuntimeFence,
    ) -> bool {
        let (revision, routes) = active.map_or((0, 0), |active| {
            (
                active.revision,
                active.playback.session_route_state_generation(),
            )
        });
        Self::runtime_frontier_changed(status, revision, routes, fence)
    }

    pub(in crate::prepared) fn runtime_frontier_changed(
        status: &Status,
        revision: u64,
        routes: u64,
        fence: FreshRuntimeFence,
    ) -> bool {
        revision != fence.expected_active_revision
            || routes != fence.route_state_generation
            || status.admitted_ticket.load(Acquire) != fence.admitted_command_ticket
            || status.last_output_fault_epoch.load(Acquire) != fence.output_fault_epoch
            || status.consumed_stop_generation.load(Acquire) != fence.consumed_stop_generation
            || status.render_fault.load(Acquire)
            || status.device_fault.load(Acquire)
            || status.pending_stop_marker.load(Acquire) != 0
    }

    pub(in crate::prepared) fn apply_processor_audition_boundary(&mut self) {
        let Some(boundary) = self.processor_audition.take_boundary() else {
            return;
        };
        if let Some(previous) = boundary.previous
            && let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
        {
            if active.control_context_generation == previous.project_generation {
                if boundary.clear_mode == processor_audition::ClearMode::Immediate {
                    let _ = active.playback.cancel_processor_audition(previous.identity);
                } else {
                    let _ = active
                        .playback
                        .select_processor_audition(previous.identity, None);
                }
            } else if boundary.clear_mode == processor_audition::ClearMode::Immediate {
                active.playback.cancel_all_processor_auditions();
            }
        } else if boundary.clear_mode == processor_audition::ClearMode::Immediate
            && let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
        {
            // A prior smooth release already cleared its lease target, but its
            // fade may still be audible. A later hard lifecycle cancel must
            // clear that residual transient state too.
            active.playback.cancel_all_processor_auditions();
        }
        self.processor_audition_lease = None;
        self.processor_audition_rejected_epoch = 0;
    }

    pub(in crate::prepared) fn reconcile_processor_audition(&mut self) {
        let epoch = self.processor_audition.current_epoch();
        let Some(target) = self.processor_audition.current() else {
            return;
        };
        if self.processor_audition_rejected_epoch == epoch {
            return;
        }
        let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        else {
            return;
        };
        if active.playback.sample_rate() != target.sample_rate_hz
            || target.sample_rate_hz != self.rate
        {
            active.playback.cancel_all_processor_auditions();
            self.processor_audition.reject_current(epoch);
            self.processor_audition_rejected_epoch = epoch;
            self.processor_audition_lease = None;
            return;
        }
        if active.control_context_generation < target.project_generation
            || active.revision < target.activation_floor
        {
            return;
        }
        if active.control_context_generation != target.project_generation {
            active.playback.cancel_all_processor_auditions();
            self.processor_audition.reject_current(epoch);
            self.processor_audition_rejected_epoch = epoch;
            self.processor_audition_lease = None;
            return;
        }
        if self.processor_audition_lease != Some(target) {
            if !active
                .playback
                .select_processor_audition(target.identity, Some(target.selection))
            {
                self.processor_audition.reject_current(epoch);
                self.processor_audition_rejected_epoch = epoch;
                self.processor_audition_lease = None;
                return;
            }
            self.processor_audition_lease = Some(target);
        }
        if !active
            .playback
            .processor_audition_is_active(target.identity, target.selection)
        {
            let _ = active.playback.cancel_processor_audition(target.identity);
            self.processor_audition.reject_current(epoch);
            self.processor_audition_rejected_epoch = epoch;
            self.processor_audition_lease = None;
        }
    }

    pub(in crate::prepared) fn consume_commands(&mut self) {
        if self.streamed_transport_mode() {
            self.consume_stream_commands();
            self.acknowledge_sampler_edit();
            return;
        }
        // Capture the published queue prefix before clearing the marker. Later
        // producer admissions stay queued for the next block; a Stop arriving
        // after this swap also remains pending for the next block.
        let entry_ticket = self.status.admitted_ticket.load(Acquire);
        let stop_marker = self.take_stop_marker();
        self.consume_commands_through(command_cut(entry_ticket, stop_marker));
        self.finish_stop_marker(stop_marker);
        self.acknowledge_sampler_edit();
    }

    // Publish the user Stop's recovery cursor before clearing its marker.
    // A racing newer marker stays pending for the next block; no callback retry.
    pub(in crate::prepared) fn take_stop_marker(&self) -> u64 {
        let marker = self.status.pending_stop_marker.load(Acquire);
        if marker != 0 {
            // This callback is the sole writer. The odd in-flight generation
            // is visible before clearing the marker, until Stop's transport
            // and route observations have been published. Saturation refuses
            // new fences permanently while priority Stop remains effective.
            let generation = self.status.consumed_stop_generation.load(Relaxed);
            self.status
                .consumed_stop_generation
                .store(generation.saturating_add(1), Release);
            // A reader may observe the cleared marker before Stop finishes.
            // Refuse any existing stopped witness before opening that window.
            self.status.sampler_edit.interrupt_for_stop();
            if !self.status.closing.load(Acquire) {
                self.status.recovery_position.store(0, Release);
            }
            let _ = self
                .status
                .pending_stop_marker
                .compare_exchange(marker, 0, AcqRel, Acquire);
        }
        marker
    }

    pub(in crate::prepared) fn finish_stop_marker(&self, marker: u64) {
        if marker != 0 {
            let generation = self.status.consumed_stop_generation.load(Relaxed);
            self.status
                .consumed_stop_generation
                .store(generation.saturating_add(1), Release);
        }
    }

    pub(in crate::prepared) fn trace_command_dequeued(
        &mut self,
        queued: QueuedCommand,
        cut: CommandCut,
    ) {
        self.trace_audio(
            AudioDiagnosticKind::CommandDequeued {
                ticket: queued.ticket,
                action: queued.action,
                discarded_by_stop: cut.stop_through.is_some_and(|stop| queued.ticket <= stop),
            },
            self.status.applied_revision.load(Relaxed),
            None,
            0,
        );
    }

    pub(in crate::prepared) fn consume_commands_through(&mut self, cut: CommandCut) {
        self.consume_commands_through_inner(cut, false);
    }

    pub(in crate::prepared) fn finish_action_outcome(
        &mut self,
        ticket: u64,
        outcome: action_outcomes::Outcome,
    ) {
        if let Some(writer) = self.action_writer.as_mut() {
            // A mismatch latches an observable failure and refuses further admission.
            let _ = writer.finish(ticket, outcome);
        }
    }

    pub(in crate::prepared) fn stage_stream_outcome(
        &mut self,
        ticket: u64,
        outcome: action_outcomes::Outcome,
    ) {
        let Some(writer) = self.action_writer.as_ref() else {
            return;
        };
        let Some(slot) = self.stream_outcomes.get_mut(self.stream_outcome_count) else {
            writer.fail_invariant();
            return;
        };
        *slot = Some((ticket, outcome));
        self.stream_outcome_count += 1;
    }

    pub(in crate::prepared) fn flush_stream_outcomes(&mut self) {
        if self.stream_outcome_count == 0 {
            return;
        }
        #[cfg(test)]
        if let Some(barrier) = self.test_before_action_outcome_status.take() {
            barrier.wait();
            barrier.wait();
        }
        self.publish_transport();
        self.publish_active_routes(false);
        let count = std::mem::take(&mut self.stream_outcome_count);
        for index in 0..count {
            if let Some((ticket, outcome)) = self.stream_outcomes[index].take() {
                self.finish_action_outcome(ticket, outcome);
            }
        }
    }

    pub(in crate::prepared) fn consume_commands_through_inner(
        &mut self,
        cut: CommandCut,
        mut fault: bool,
    ) {
        // A publication can complete a streamed terminal before handing the
        // same block to resident consumption. Preserve completion order.
        self.flush_stream_outcomes();
        if cut.stop_through.is_some() {
            // The immediate priority Stop follows an interrupted fade's effects.
            self.settle_transport_fade();
        }
        if let Some(through_ticket) = cut.stop_through {
            if !fault && !self.status.closing.load(Acquire) {
                self.recovery_position_frozen = false;
                self.status.recovery_position.store(0, Relaxed);
            }
            if !fault {
                self.trace_audio(
                    AudioDiagnosticKind::PriorityStop { through_ticket },
                    self.status.applied_revision.load(Relaxed),
                    None,
                    0,
                );
                self.finish_midi_before_stop();
                self.revoke_output_transport();
                self.revoke_current_sampler_keyboard();
                #[cfg(all(test, feature = "allocation-probe"))]
                self.apply_candidate_action(PlaybackAction::Transport(PlaybackCommand::Stop));
                self.invalidate_gain_recording(
                    crate::gain_recording::AbortReason::TransportChanged,
                );
                let mut priority_stop_failed = None;
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::current_mut)
                {
                    let block_start = active.playback.position();
                    if active
                        .playback
                        .apply_priority_stop(&self.status.keyboard_activation)
                        .is_err()
                    {
                        priority_stop_failed = Some(block_start);
                    }
                }
                self.invalidate_processor_audition_request();
                if let Some(block_start) = priority_stop_failed {
                    self.report_render_fault(
                        Some(&Invalid("priority stop failed on active playback")),
                        block_start,
                    );
                    fault = true;
                }
            }
        }
        let mut consumed = None;
        let mut outcomes = [None; COMMANDS];
        for staged in &mut outcomes {
            if !self
                .commands
                .peek()
                .is_ok_and(|queued| queued.ticket <= cut.process_through)
            {
                break;
            }
            let Ok(queued) = self.commands.pop() else {
                break;
            };
            consumed = Some(queued.ticket);
            if !fault {
                self.trace_command_dequeued(queued, cut);
            }
            if !fault
                && cut.stop_through.is_none_or(|stop| queued.ticket > stop)
                && queued.project_generation.is_some_and(|generation| {
                    self.recording_source()
                        .is_none_or(|source| source.context.project_generation != generation)
                })
            {
                let reason = PlaybackActionRejection::ProjectChanged;
                self.status.publish_rejected_action(queued.ticket, reason);
                *staged = Some((queued.ticket, action_outcomes::Outcome::Rejected(reason)));
                continue;
            }
            if !fault
                && cut.stop_through.is_none_or(|stop| queued.ticket > stop)
                && queued
                    .expected_source
                    .is_some_and(|expected| self.recording_source() != Some(expected))
            {
                let reason = PlaybackActionRejection::AudioSourceChanged;
                self.status.publish_rejected_action(queued.ticket, reason);
                *staged = Some((queued.ticket, action_outcomes::Outcome::Rejected(reason)));
                continue;
            }
            // Each command follows the audible end of an earlier fade.
            self.settle_transport_fade();
            if fault && sampler_edit::is_stop(queued.action) {
                self.status.recovery_position.store(0, Release);
                *staged = Some((queued.ticket, action_outcomes::Outcome::Applied));
            } else if fault {
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                ));
            }
            if fault {
                continue;
            }
            if stale_output_start(&self.status, queued) {
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                ));
                continue;
            }
            if self.status.render_fault.load(Acquire) && starts_playback(queued.action) {
                if cut.stop_through.is_none_or(|stop| queued.ticket > stop)
                    && let Some(frame) = fault_cursor_intent(queued.action)
                {
                    self.status.recovery_position.store(frame, Release);
                    // An earlier applied Seek unfroze the cursor; keep this intent
                    // from being overwritten by the stopped graph's position.
                    self.recovery_position_frozen = true;
                }
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                ));
                continue;
            }
            if !self.status.sampler_edit.permits(queued) {
                self.status.publish_rejected_action(
                    queued.ticket,
                    PlaybackActionRejection::SamplerEditBusy,
                );
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::SamplerEditBusy),
                ));
                continue;
            }
            if cut
                .stop_through
                .is_none_or(|stop_through| queued.ticket > stop_through)
            {
                #[cfg(all(test, feature = "allocation-probe"))]
                self.apply_candidate_action(queued.action);
                let mut invalidate_audition = false;
                let mut fading = false;
                let outcome;
                let closing = self.status.closing.load(Acquire);
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::current_mut)
                {
                    let applied = if closing {
                        active.playback.try_apply_action(queued.action)
                    } else {
                        active.playback.try_apply_declicked_action(queued.action)
                    };
                    match applied {
                        Ok(()) => {
                            fading =
                                active.playback.pending_transport_fade() == Some(queued.action);
                            outcome = action_outcomes::Outcome::Applied;
                            if recovery_cursor_action(queued.action) {
                                self.recovery_position_frozen = false;
                            }
                            invalidate_audition =
                                action_invalidates_processor_audition(queued.action);
                        }
                        Err(error) => {
                            let reason = error.telemetry_rejection();
                            self.status.publish_rejected_action(queued.ticket, reason);
                            outcome = action_outcomes::Outcome::Rejected(reason);
                        }
                    }
                } else {
                    self.status.publish_rejected_action(
                        queued.ticket,
                        PlaybackActionRejection::NoActivePlayback,
                    );
                    outcome = action_outcomes::Outcome::Rejected(
                        PlaybackActionRejection::NoActivePlayback,
                    );
                }
                *staged = Some((queued.ticket, outcome));
                let stop = matches!(
                    queued.action,
                    PlaybackAction::Transport(PlaybackCommand::Stop)
                );
                if fading {
                    // The output transport, epoch and sampler keyboard stay
                    // live until the fade's audible end.
                    self.begin_transport_fade(queued.ticket, queued.action, false);
                } else if stop {
                    self.revoke_current_sampler_keyboard();
                }
                if invalidate_audition
                    || matches!(
                        queued.action,
                        PlaybackAction::Transport(PlaybackCommand::Pause)
                            | PlaybackAction::Session(hex_engine::SessionPlaybackCommand::Pause)
                    )
                {
                    self.invalidate_midi_recording(
                        crate::midi_recording::AbortReason::TransportChanged,
                    );
                    if fading {
                        self.abort_transport_inputs(
                            crate::gain_recording::AbortReason::TransportChanged,
                        );
                    } else {
                        self.invalidate_for_transport_change(queued.action);
                    }
                }
                if invalidate_audition {
                    self.invalidate_processor_audition_request();
                }
            } else {
                *staged = Some((queued.ticket, action_outcomes::Outcome::Superseded));
            }
        }
        #[cfg(test)]
        if let Some(barrier) = self.test_before_action_outcome_status.take() {
            barrier.wait();
            barrier.wait();
        }
        self.publish_transport();
        self.publish_active_routes(false);
        if let Some(ticket) = consumed {
            self.status.consumed_command.store(ticket, Release);
        }
        for (ticket, outcome) in outcomes.into_iter().flatten() {
            self.finish_action_outcome(ticket, outcome);
        }
    }
    /// Latch the first actual render cause before Stop changes the cursor.
    /// None denotes nonfinite output after successful rendering.
    /// Native adapters call this on the callback; no formatting, I/O or ownership drops.
    pub fn report_render_fault(&mut self, error: Option<&Invalid>, block_start_frame: u64) {
        if !self.status.render_fault.load(Acquire) {
            self.status.render_fault_record.publish(
                error.map_or("audio output contains nonfinite samples", |error| error.0),
                self.status.applied_revision.load(Relaxed),
                block_start_frame,
            );
            self.status.render_fault.store(true, Release);
        }
        self.stop_after_fault();
    }
    /// Stop immediately after a render fault; does not dispose graph ownership.
    pub fn stop_after_fault(&mut self) {
        self.release_device_insert_fade_pins();
        if let Some(receipt) = self
            .deferred_eligible
            .as_ref()
            .and_then(|next| next.device_insert())
        {
            receipt.returned(None);
        }
        if let Some(next) = self.deferred.as_ref()
            && next.approved_device_insert().is_some()
            && let Some(receipt) = next.fresh_runtime_cancellation.as_ref()
        {
            receipt.returned(None);
        }
        // The fault Stop below revokes every transport effect immediately.
        self.transport_fade = None;
        self.recovery_position_frozen = true;
        if self.streamed_transport_mode() {
            self.cancel_stream_for_fault();
        }
        self.recovery_seek_pending = None;
        self.sample_preview.invalidate_on_fault();
        self.sample_recording
            .end(crate::sample_recording::End::OutputFailed);
        self.invalidate_midi_recording(crate::midi_recording::AbortReason::OutputFailed);
        self.revoke_output_transport();
        self.status.sampler_keyboard.revoke_on_callback();
        #[cfg(all(test, feature = "allocation-probe"))]
        self.apply_candidate_action(PlaybackAction::Transport(PlaybackCommand::Stop));
        self.invalidate_gain_recording(crate::gain_recording::AbortReason::OutputFailed);
        let mut fault_stop_failed = None;
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            let block_start = active.playback.position();
            if active.playback.apply(PlaybackCommand::Stop).is_err() {
                fault_stop_failed = Some(block_start);
            }
        }
        if let Some(block_start) = fault_stop_failed
            && !self.status.render_fault.load(Acquire)
        {
            self.status.render_fault_record.publish(
                "fault stop failed on active playback",
                self.status.applied_revision.load(Relaxed),
                block_start,
            );
            self.status.render_fault.store(true, Release);
        }
        self.processor_audition.invalidate_current_immediately();
        self.invalidate_processor_audition_request();
        self.publish_transport();
        self.publish_active_routes(false);
        self.flush_stream_outcomes();
    }

    // The internal cut is never published as a priority Stop: that marker is
    // reserved for the user's explicit cursor-reset intent. Drain at most one
    // bounded command prefix per callback and leave graph custody unchanged.
    pub(crate) fn fence_device_fault(&mut self) {
        if self.device_fault_fenced_in_block || self.status.closing.load(Acquire) {
            return;
        }
        self.device_fault_fenced_in_block = true;
        let entry = self.status.admitted_ticket.load(Acquire);
        let user_stop = self.take_stop_marker();
        if !self.device_fault_stopped {
            self.device_fault_stopped = true;
            self.stop_after_fault();
        } else {
            // A begin that raced the error still receives its reserved terminal.
            self.sample_recording
                .end(crate::sample_recording::End::OutputFailed);
            self.invalidate_midi_recording(crate::midi_recording::AbortReason::OutputFailed);
            self.gain_recording
                .abort(crate::gain_recording::AbortReason::OutputFailed);
            self.invalidate_native_parameter_input(
                crate::native_parameter_input::AbortReason::OutputFailed,
            );
        }
        let through = entry.max(user_stop.saturating_sub(1));
        let cut = CommandCut {
            stop_through: Some(through),
            process_through: through,
        };
        if self.streamed_transport_mode() {
            self.consume_stream_commands_through(cut, true);
        } else {
            self.consume_commands_through_inner(cut, true);
        }
        self.finish_stop_marker(user_stop);
        self.acknowledge_sampler_edit();
        self.flush_stream_outcomes();
    }

    pub(in crate::prepared) fn invalidate_processor_audition_request(&mut self) {
        if self.processor_audition.has_request_after_hard_cancel() {
            return;
        }
        let epoch = self.processor_audition.current_epoch();
        self.processor_audition.reject_current(epoch);
        self.processor_audition_rejected_epoch = epoch;
        self.processor_audition_lease = None;
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.cancel_all_processor_auditions();
        }
    }
}
