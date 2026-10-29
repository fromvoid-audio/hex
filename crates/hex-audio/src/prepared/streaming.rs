//! Bounded streamed transport intent, separate from dequeued/applied prefixes.
use super::*;
use hex_engine::SessionPlaybackCommand;
pub(super) fn record_outcome(
    status: &Status,
    pending_play: &mut Option<u64>,
    prefix: u64,
    action_pending: bool,
    outcome: crate::candidate_custody::StreamOutcome,
) -> Result<(), Invalid> {
    if outcome.play_applied {
        *pending_play = None;
    }
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
    outcome.fault.map_or(Ok(()), Err)
}
impl Callback {
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
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
            && let Some(ticket) = active.cancel_stream_action(&mut self.domain_pages)
        {
            self.status
                .publish_rejected_action(ticket, PlaybackActionRejection::StreamActionCancelled);
        }
    }
    fn stage_stream_action(&mut self, action: PlaybackAction, ticket: u64) {
        if self.status.publication_pending.load(Acquire) || self.stream_pending_play.is_some() {
            self.status
                .publish_rejected_action(ticket, PlaybackActionRejection::StreamActionBusy);
            return;
        }
        let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
        else {
            self.status
                .publish_rejected_action(ticket, PlaybackActionRejection::SessionNotPrepared);
            return;
        };
        match active.stage_stream_action(action, ticket) {
            crate::candidate_custody::StageAction::Refused(reason) => {
                self.status.publish_rejected_action(ticket, reason)
            }
            crate::candidate_custody::StageAction::Scheduled => {
                self.status.stream_scheduled_command.store(ticket, Release)
            }
            _ => {}
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
            Ok(()) => self.stream_pending_play = None,
            Err(error) => {
                self.status
                    .publish_rejected_action(ticket, error.telemetry_rejection());
                self.stream_pending_play = None;
            }
        }
        self.status
            .stream_applied_command_prefix
            .store(self.stream_prefix, Release);
    }
    fn apply_stream_zero_page_action(&mut self, action: PlaybackAction, ticket: u64) {
        if matches!(action, PlaybackAction::Transport(PlaybackCommand::Stop)) {
            self.revoke_current_sampler_keyboard();
        }
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            if let Err(error) = active.playback.try_apply_action(action) {
                self.status
                    .publish_rejected_action(ticket, error.telemetry_rejection());
            }
        } else if !matches!(
            action,
            PlaybackAction::Transport(PlaybackCommand::Pause | PlaybackCommand::Stop)
        ) {
            // Pause/Stop are valid zero-page intents before the first runtime.
            // Other actions cannot claim application to a nonexistent owner.
            self.status
                .publish_rejected_action(ticket, PlaybackActionRejection::SessionNotPrepared);
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
            self.status.pending_stop_marker.swap(0, AcqRel),
        );
        if let Some(stop) = cut.stop_through {
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
            self.cancel_active_stream_action();
            self.stream_pending_play = None;
            self.invalidate_stream_readiness();
            if let Some(ActivePublication::Legacy(active)) = self.active.as_mut() {
                active
                    .playback
                    .apply_priority_stop(&self.status.keyboard_activation);
            } else {
                self.apply_stream_zero_page_action(
                    PlaybackAction::Transport(PlaybackCommand::Stop),
                    stop,
                );
            }
            self.invalidate_gain_recording(crate::gain_recording::AbortReason::TransportChanged);
            self.invalidate_processor_audition_request();
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
            self.trace_command_dequeued(queued, cut);
            self.stream_prefix = self.stream_prefix.max(queued.ticket);
            if cut.stop_through.is_some_and(|stop| queued.ticket <= stop) {
                continue;
            }
            if !self.status.sampler_edit.permits(queued) {
                self.status.publish_rejected_action(
                    queued.ticket,
                    PlaybackActionRejection::SamplerEditBusy,
                );
                continue;
            }
            match queued.action {
                PlaybackAction::Transport(PlaybackCommand::Play)
                | PlaybackAction::Session(SessionPlaybackCommand::Play) => {
                    if self.stream_action_pending() {
                        self.status.publish_rejected_action(
                            queued.ticket,
                            PlaybackActionRejection::StreamActionBusy,
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
                    self.stream_pending_play = None;
                    self.invalidate_stream_readiness();
                    self.invalidate_midi_recording(
                        crate::midi_recording::AbortReason::TransportChanged,
                    );
                    self.apply_stream_zero_page_action(queued.action, queued.ticket);
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
                            Ok(Some(ticket)) => self.status.publish_rejected_action(
                                ticket,
                                PlaybackActionRejection::StreamActionCancelled,
                            ),
                            Err(error) => self.status.publish_rejected_action(
                                queued.ticket,
                                error.telemetry_rejection(),
                            ),
                            _ => {}
                        }
                    } else {
                        self.status.publish_rejected_action(
                            queued.ticket,
                            PlaybackActionRejection::SessionNotPrepared,
                        );
                    }
                }
                PlaybackAction::SetMetronome(_) => {
                    self.apply_stream_zero_page_action(queued.action, queued.ticket)
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
