//! Bounded streamed transport intent, separate from dequeued/applied prefixes.
use super::*;
use hex_engine::SessionPlaybackCommand;
pub(super) struct RecordedStreamOutcome {
    pub(super) applied_play: Option<u64>,
    pub(super) applied_action: Option<u64>,
    pub(super) rejected_action: Option<(u64, PlaybackActionRejection)>,
    pub(super) fault: Option<Invalid>,
}
pub(super) fn record_outcome(
    status: &Status,
    pending_play: &mut Option<u64>,
    prefix: u64,
    action_pending: bool,
    outcome: crate::candidate_custody::StreamOutcome,
) -> RecordedStreamOutcome {
    let play = if outcome.play_applied {
        pending_play.take()
    } else {
        None
    };
    let (buffering, committed) = match outcome.progress {
        crate::candidate_custody::StreamProgress::Complete => (false, 0),
        crate::candidate_custody::StreamProgress::Buffering { committed_frames } => {
            (true, committed_frames as u64)
        }
    };
    status.stream_committed_frames.store(committed, Relaxed);
    status.stream_buffering.store(buffering, Release);
    if outcome.source_changed {
        status.stream_source_changed.store(true, Release);
    }
    status
        .stream_readiness_refused
        .store(outcome.readiness_refused, Release);
    if let Some(ticket) = outcome.activated {
        status.stream_activated_command.store(ticket, Release);
    }
    if let Some((ticket, reason)) = outcome.action_rejection {
        status.publish_rejected_action(ticket, reason);
    }
    if pending_play.is_none() && !action_pending {
        status.stream_applied_command_prefix.store(prefix, Release);
    }
    RecordedStreamOutcome {
        applied_play: play,
        applied_action: outcome.action_applied,
        rejected_action: outcome.action_rejection,
        fault: outcome.fault,
    }
}
impl Callback {
    pub(super) fn resolve_recovery_seek(
        &mut self,
        action_pending: bool,
        rejection: Option<(u64, PlaybackActionRejection)>,
    ) {
        let Some(ticket) = self.recovery_seek_pending.filter(|_| !action_pending) else {
            return;
        };
        self.recovery_seek_pending = None;
        if rejection.is_none_or(|(rejected, _)| rejected != ticket) {
            self.recovery_position_frozen = false;
            self.publish_transport();
        }
    }
    /// Yield only a successor's exact outstanding reservation after the
    /// current window reports capacity pressure. The next nonempty render
    /// gives the current window one opportunity before another admission.
    pub(super) fn yield_pending_stream_successor(&mut self) -> bool {
        let mut yielded = false;
        if let Some(streams) = self
            .deferred
            .as_mut()
            .and_then(|next| next.streams.as_mut())
            && streams.has_install_pending()
        {
            streams.cancel_install(&mut self.domain_pages);
            yielded = true;
        }
        if let Some(next) = self.deferred_eligible.as_mut()
            && next.has_stream_install_pending()
        {
            next.cancel_stream_readiness(&mut self.domain_pages);
            yielded = true;
        }
        if yielded {
            self.yield_stream_successor_once = true;
        }
        yielded
    }
    pub(super) fn stream_action_pending(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(ActivePublication::has_stream_action)
    }
    pub(super) fn cancel_active_stream_action(&mut self) {
        self.cancel_active_stream_action_with_reason(
            PlaybackActionRejection::StreamActionCancelled,
        );
    }
    fn cancel_active_stream_action_with_reason(&mut self, reason: PlaybackActionRejection) {
        self.cancel_active_stream_action_with_outcome(
            reason,
            action_outcomes::Outcome::Rejected(reason),
        );
    }
    fn cancel_active_stream_action_with_outcome(
        &mut self,
        reason: PlaybackActionRejection,
        outcome: action_outcomes::Outcome,
    ) {
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
            && let Some(ticket) = active.cancel_stream_action(&mut self.domain_pages)
        {
            if self.recovery_seek_pending == Some(ticket) {
                self.recovery_seek_pending = None;
            }
            self.status.publish_rejected_action(ticket, reason);
            self.stage_stream_outcome(ticket, outcome);
        }
    }
    fn stage_stream_action(&mut self, action: PlaybackAction, ticket: u64) {
        if self.status.publication_pending.load(Acquire) || self.stream_pending_play.is_some() {
            self.status
                .publish_rejected_action(ticket, PlaybackActionRejection::StreamActionBusy);
            self.stage_stream_outcome(
                ticket,
                action_outcomes::Outcome::Rejected(PlaybackActionRejection::StreamActionBusy),
            );
            return;
        }
        let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
        else {
            self.status
                .publish_rejected_action(ticket, PlaybackActionRejection::SessionNotPrepared);
            self.stage_stream_outcome(
                ticket,
                action_outcomes::Outcome::Rejected(PlaybackActionRejection::SessionNotPrepared),
            );
            return;
        };
        match active.stage_stream_action(action, ticket) {
            crate::candidate_custody::StageAction::Refused(reason) => {
                self.status.publish_rejected_action(ticket, reason);
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Rejected(reason));
            }
            crate::candidate_custody::StageAction::Scheduled => {
                self.status.stream_scheduled_command.store(ticket, Release);
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            crate::candidate_custody::StageAction::Applied => {
                if recovery_cursor_action(action) {
                    self.recovery_position_frozen = false;
                }
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            crate::candidate_custody::StageAction::Pending => {
                if self.recovery_position_frozen && recovery_cursor_action(action) {
                    self.recovery_seek_pending = Some(ticket);
                }
            }
        }
    }
    pub(super) fn streamed_transport_mode(&self) -> bool {
        self.stream_pending_play.is_some() || self.stream_action_pending()
            || self.status.verified_streaming_mode.load(Acquire)
            || self.active.as_ref().is_some_and(ActivePublication::streaming_enabled)
            || self.deferred.as_ref().is_some_and(|next| next.streams.is_some())
            || self.publications.peek().is_ok_and(|next| matches!(next, QueuedPublication::Legacy(next) if next.streams.is_some()))
            || self.deferred_eligible.as_ref().is_some_and(|next| next.streaming_enabled())
            || self.publications.peek().is_ok_and(|next| matches!(next, QueuedPublication::Eligible(next) if next.streaming_enabled()))
    }
    fn invalidate_stream_readiness(&mut self) {
        if let Some(streams) = self
            .deferred
            .as_mut()
            .and_then(|next| next.streams.as_mut())
        {
            streams.cancel_install(&mut self.domain_pages);
        }
        if let Some(next) = self.deferred_eligible.as_mut() {
            next.cancel_stream_readiness(&mut self.domain_pages);
        }
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
        {
            active.revoke_launch_prewarm(&mut self.domain_pages);
            active.clear_stream_roles(&mut self.domain_pages);
        }
    }
    /// Cancel only already staged streamed intent. Commands not yet dequeued
    /// retain their ordering, including a later explicit seek or user Stop.
    pub(super) fn cancel_stream_for_fault(&mut self) {
        self.cancel_active_stream_action_with_reason(PlaybackActionRejection::OutputFailed);
        if let Some(ticket) = self.stream_pending_play.take() {
            self.status
                .publish_rejected_action(ticket, PlaybackActionRejection::OutputFailed);
            self.stage_stream_outcome(
                ticket,
                action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
            );
        }
        self.invalidate_stream_readiness();
        self.refresh_stream_schedule();
    }
    pub(super) fn finish_pending_stream_play(&mut self) {
        let Some(ticket) = self.stream_pending_play else {
            return;
        };
        let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        else {
            return;
        };
        match active
            .playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        {
            Ok(()) => {
                self.stream_pending_play = None;
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            Err(error) => {
                self.status
                    .publish_rejected_action(ticket, error.telemetry_rejection());
                self.stream_pending_play = None;
                self.stage_stream_outcome(
                    ticket,
                    action_outcomes::Outcome::Rejected(error.telemetry_rejection()),
                );
            }
        }
        self.status
            .stream_applied_command_prefix
            .store(self.stream_prefix, Release);
    }
    fn apply_stream_zero_page_action(
        &mut self,
        action: PlaybackAction,
        ticket: u64,
    ) -> action_outcomes::Outcome {
        if matches!(action, PlaybackAction::Transport(PlaybackCommand::Stop)) {
            self.revoke_current_sampler_keyboard();
        }
        let closing = self.status.closing.load(Acquire);
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            // Pause/Stop fade out before applying unless the stream is closing.
            // Readiness invalidation and schedule refresh stay at command time.
            let applied = if closing {
                active.playback.try_apply_action(action)
            } else {
                active.playback.try_apply_declicked_action(action)
            };
            if applied.is_ok() {
                self.transport_fade_ticket =
                    (active.playback.pending_transport_fade() == Some(action)).then_some(ticket);
            }
            match applied {
                Ok(()) if recovery_cursor_action(action) && !closing => {
                    self.recovery_position_frozen = false;
                    action_outcomes::Outcome::Applied
                }
                Ok(()) => action_outcomes::Outcome::Applied,
                Err(error) => {
                    let reason = error.telemetry_rejection();
                    self.status.publish_rejected_action(ticket, reason);
                    action_outcomes::Outcome::Rejected(reason)
                }
            }
        } else if !matches!(
            action,
            PlaybackAction::Transport(PlaybackCommand::Pause | PlaybackCommand::Stop)
        ) {
            // Pause/Stop are valid zero-page intents before the first runtime.
            // Other actions cannot claim application to a nonexistent owner.
            self.status
                .publish_rejected_action(ticket, PlaybackActionRejection::SessionNotPrepared);
            action_outcomes::Outcome::Rejected(PlaybackActionRejection::SessionNotPrepared)
        } else {
            action_outcomes::Outcome::Applied
        }
    }
    fn refresh_stream_schedule(&mut self) {
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
            && let Some(ticket) = active.refresh_scheduled_after_command(&mut self.domain_pages)
        {
            self.status
                .stream_invalidated_schedule
                .store(ticket, Release);
        }
    }
    pub(super) fn consume_stream_commands(&mut self) {
        let cut = command_cut(
            self.status.admitted_ticket.load(Acquire),
            self.take_stop_marker(),
        );
        self.consume_stream_commands_through(cut, false);
    }
    pub(super) fn consume_stream_commands_through(&mut self, cut: CommandCut, fault: bool) {
        if let Some(stop) = cut.stop_through {
            if !fault && !self.status.closing.load(Acquire) {
                self.recovery_position_frozen = false;
                self.status.recovery_position.store(0, Relaxed);
            }
            if !fault {
                self.trace_audio(
                    AudioDiagnosticKind::PriorityStop {
                        through_ticket: stop,
                    },
                    self.status.applied_revision.load(Relaxed),
                    None,
                    0,
                );
                self.finish_midi_before_stop();
                self.revoke_output_transport();
                self.revoke_current_sampler_keyboard();
            }
            if fault {
                self.cancel_active_stream_action_with_reason(PlaybackActionRejection::OutputFailed);
            } else {
                self.cancel_active_stream_action_with_outcome(
                    PlaybackActionRejection::StreamActionCancelled,
                    action_outcomes::Outcome::Superseded,
                );
            }
            if let Some(ticket) = self.stream_pending_play.take() {
                if fault {
                    self.status
                        .publish_rejected_action(ticket, PlaybackActionRejection::OutputFailed);
                    self.stage_stream_outcome(
                        ticket,
                        action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                    );
                } else {
                    self.stage_stream_outcome(ticket, action_outcomes::Outcome::Superseded);
                }
            }
            self.invalidate_stream_readiness();
            if !fault {
                if let Some(ActivePublication::Legacy(active)) = self.active.as_mut() {
                    if self.status.closing.load(Acquire) {
                        active
                            .playback
                            .apply_priority_stop(&self.status.keyboard_activation);
                    } else {
                        active
                            .playback
                            .apply_declicked_priority_stop(&self.status.keyboard_activation);
                    }
                    self.transport_fade_ticket = None;
                } else {
                    let _ = self.apply_stream_zero_page_action(
                        PlaybackAction::Transport(PlaybackCommand::Stop),
                        stop,
                    );
                }
                self.invalidate_gain_recording(
                    crate::gain_recording::AbortReason::TransportChanged,
                );
                self.invalidate_processor_audition_request();
            }
            self.stream_prefix = self.stream_prefix.max(stop);
            self.refresh_stream_schedule();
        }
        let mut consumed = None;
        for _ in 0..COMMANDS {
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
            if fault && sampler_edit::is_stop(queued.action) {
                self.status.recovery_position.store(0, Release);
                self.stage_stream_outcome(queued.ticket, action_outcomes::Outcome::Applied);
            } else if fault {
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                self.stage_stream_outcome(
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                );
            }
            self.stream_prefix = self.stream_prefix.max(queued.ticket);
            if fault {
                continue;
            }
            if cut.stop_through.is_some_and(|stop| queued.ticket <= stop) {
                self.stage_stream_outcome(queued.ticket, action_outcomes::Outcome::Superseded);
                continue;
            }
            if stale_output_start(&self.status, queued) {
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                self.stage_stream_outcome(
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                );
                continue;
            }
            if self.status.render_fault.load(Acquire) && starts_playback(queued.action) {
                if let Some(frame) = fault_cursor_intent(queued.action) {
                    self.status.recovery_position.store(frame, Release);
                    // An earlier applied Seek unfroze the cursor; keep this intent
                    // from being overwritten by the stopped graph's position.
                    self.recovery_position_frozen = true;
                }
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                self.stage_stream_outcome(
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                );
                continue;
            }
            if !self.status.sampler_edit.permits(queued) {
                self.status.publish_rejected_action(
                    queued.ticket,
                    PlaybackActionRejection::SamplerEditBusy,
                );
                self.stage_stream_outcome(
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::SamplerEditBusy),
                );
                continue;
            }
            match queued.action {
                PlaybackAction::Transport(PlaybackCommand::Play)
                | PlaybackAction::Session(SessionPlaybackCommand::Play) => {
                    if self.stream_action_pending() || self.stream_pending_play.is_some() {
                        self.status.publish_rejected_action(
                            queued.ticket,
                            PlaybackActionRejection::StreamActionBusy,
                        );
                        self.stage_stream_outcome(
                            queued.ticket,
                            action_outcomes::Outcome::Rejected(
                                PlaybackActionRejection::StreamActionBusy,
                            ),
                        );
                    } else {
                        self.stream_pending_play = Some(queued.ticket);
                    }
                }
                PlaybackAction::Transport(PlaybackCommand::Pause | PlaybackCommand::Stop)
                | PlaybackAction::Session(
                    SessionPlaybackCommand::Pause | SessionPlaybackCommand::Stop,
                ) => {
                    self.cancel_active_stream_action();
                    if let Some(ticket) = self.stream_pending_play.take() {
                        self.status.publish_rejected_action(
                            ticket,
                            PlaybackActionRejection::StreamActionCancelled,
                        );
                        self.stage_stream_outcome(
                            ticket,
                            action_outcomes::Outcome::Rejected(
                                PlaybackActionRejection::StreamActionCancelled,
                            ),
                        );
                    }
                    self.invalidate_stream_readiness();
                    self.invalidate_midi_recording(
                        crate::midi_recording::AbortReason::TransportChanged,
                    );
                    let outcome = self.apply_stream_zero_page_action(queued.action, queued.ticket);
                    self.stage_stream_outcome(queued.ticket, outcome);
                    self.invalidate_gain_recording(
                        crate::gain_recording::AbortReason::TransportChanged,
                    );
                }
                PlaybackAction::Session(SessionPlaybackCommand::CancelPending { track }) => {
                    if let Some(mut active) = self
                        .active
                        .as_mut()
                        .and_then(ActivePublication::stream_view)
                    {
                        match active.cancel_pending_routes(
                            SessionPlaybackCommand::CancelPending { track },
                            &mut self.domain_pages,
                        ) {
                            Ok(Some(ticket)) => {
                                if self.recovery_seek_pending == Some(ticket) {
                                    self.recovery_seek_pending = None;
                                }
                                self.status.publish_rejected_action(
                                    ticket,
                                    PlaybackActionRejection::StreamActionCancelled,
                                );
                                self.stage_stream_outcome(
                                    ticket,
                                    action_outcomes::Outcome::Rejected(
                                        PlaybackActionRejection::StreamActionCancelled,
                                    ),
                                );
                                self.stage_stream_outcome(
                                    queued.ticket,
                                    action_outcomes::Outcome::Applied,
                                );
                            }
                            Err(error) => {
                                let reason = error.telemetry_rejection();
                                self.status.publish_rejected_action(queued.ticket, reason);
                                self.stage_stream_outcome(
                                    queued.ticket,
                                    action_outcomes::Outcome::Rejected(reason),
                                );
                            }
                            _ => self.stage_stream_outcome(
                                queued.ticket,
                                action_outcomes::Outcome::Applied,
                            ),
                        }
                    } else {
                        self.status.publish_rejected_action(
                            queued.ticket,
                            PlaybackActionRejection::SessionNotPrepared,
                        );
                        self.stage_stream_outcome(
                            queued.ticket,
                            action_outcomes::Outcome::Rejected(
                                PlaybackActionRejection::SessionNotPrepared,
                            ),
                        );
                    }
                }
                PlaybackAction::SetMetronome(_) => {
                    let outcome = self.apply_stream_zero_page_action(queued.action, queued.ticket);
                    self.stage_stream_outcome(queued.ticket, outcome);
                }
                _ => self.stage_stream_action(queued.action, queued.ticket),
            }
            self.refresh_stream_schedule();
        }
        self.publish_transport();
        self.publish_active_routes(false);
        if let Some(ticket) = consumed {
            self.status.consumed_command.store(ticket, Release);
        }
        if self.stream_pending_play.is_none() && !self.stream_action_pending() {
            self.status
                .stream_applied_command_prefix
                .store(self.stream_prefix, Release);
        }
    }
}
