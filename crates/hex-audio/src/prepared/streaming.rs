//! Bounded streamed transport intent, separate from dequeued/applied prefixes.
use super::*;
use hex_engine::SessionPlaybackCommand;
pub(super) struct RecordedStreamOutcome {
    pub(super) applied_play: Option<u64>,
    pub(super) applied_action: Option<u64>,
    pub(super) rejected_action: Option<(u64, PlaybackActionRejection)>,
    pub(super) fault: Option<Invalid>,
}
/// Count an audible stream starvation: this chunk held silence for pages
/// while `playing` (published after the previous callback) was already true
/// and no Play or stream action pre-roll was waiting. One relaxed RMW; no
/// allocation or waiting. Install/replacement holds outside an active stream
/// view are not counted.
pub(super) fn count_starvation(
    status: &Status,
    play_pending: bool,
    action_pending: bool,
    outcome: &crate::candidate_custody::StreamOutcome,
) {
    let buffering = matches!(
        outcome.progress,
        crate::candidate_custody::StreamProgress::Buffering { .. }
    );
    if buffering && !play_pending && !action_pending && status.playing.load(Relaxed) {
        status.stream_starved_callbacks.fetch_add(1, Relaxed);
    }
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
    if outcome.read_failed {
        status.stream_read_failed.store(true, Release);
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
impl Status {
    /// Take the current-demand audio read-error notification on control/UI.
    /// Exactly one notification observer may consume it. Repeated failures
    /// coalesce until taken; a later failure remains observable. No readiness,
    /// identity change, persistent fault or successful recovery is implied.
    pub fn take_stream_read_failure(&self) -> bool {
        self.stream_read_failed
            .swap(false, std::sync::atomic::Ordering::AcqRel)
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
    pub(super) fn retry_pending_stream_capacity(&mut self) -> bool {
        if let Some(streams) = self
            .deferred
            .as_mut()
            .and_then(|next| next.streams.as_mut())
        {
            return streams.retry_capacity_conflict();
        }
        self.deferred_eligible
            .as_mut()
            .is_some_and(|next| next.retry_stream_capacity_conflict())
    }
    pub(super) fn reject_pending_stream_successor(&mut self, reason: PlaybackHandoffRejection) {
        if self
            .deferred
            .as_ref()
            .is_some_and(|next| next.streams.is_some())
            && let Some(mut next) = self.deferred.take()
        {
            if let Some(streams) = next.streams.as_mut() {
                streams.cancel_install(&mut self.domain_pages);
            }
            if let Some(cancellation) = next.fresh_runtime_cancellation.as_ref() {
                cancellation.finish();
            }
            self.reject_publication(next.revision, reason);
            self.return_graph(next);
            self.status.publication_pending.store(false, Release);
        }
        if self
            .deferred_eligible
            .as_ref()
            .is_some_and(|next| next.streaming_enabled())
            && let Some(mut next) = self.deferred_eligible.take()
        {
            next.cancel_stream_readiness(&mut self.domain_pages);
            self.reject_publication(next.revision(), reason);
            self.return_publication(ReturnedPublication::Eligible(next.reject_before_install()));
            self.status.publication_pending.store(false, Release);
            if let Some(ticket) = self.stream_pending_play.take() {
                self.status
                    .publish_rejected_action(ticket, PlaybackActionRejection::SessionNotPrepared);
                self.stage_stream_outcome(
                    ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::SessionNotPrepared),
                );
                self.status
                    .stream_applied_command_prefix
                    .store(self.stream_prefix, Release);
            }
        }
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
        self.invalidate_stream_readiness_keeping_active_pages();
        self.clear_active_stream_roles();
    }
    /// Cancel staged and future stream readiness while the active pages stay
    /// requested, as a declicked Pause/Stop still renders its fade from them.
    fn invalidate_stream_readiness_keeping_active_pages(&mut self) {
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
        }
    }
    pub(super) fn clear_active_stream_roles(&mut self) {
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
        {
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
        // A resume from a paused position ramps in, as on the resident path.
        let play = PlaybackAction::Transport(PlaybackCommand::Play);
        let applied = if self.status.closing.load(Acquire) {
            active.playback.try_apply_action(play)
        } else {
            active.playback.try_apply_declicked_action(play)
        };
        match applied {
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
    /// Apply a zero-page action. A queued `transport_change` Pause/Stop
    /// while playing is Applied now and declicked unless closing or
    /// buffering; its keyboard, output and active page effects follow its
    /// fade. Otherwise they apply at once, as before.
    fn apply_stream_zero_page_action(
        &mut self,
        action: PlaybackAction,
        ticket: u64,
        transport_change: bool,
    ) -> action_outcomes::Outcome {
        let stop = matches!(action, PlaybackAction::Transport(PlaybackCommand::Stop));
        let closing = self.status.closing.load(Acquire);
        // A buffering stream already outputs silence and cannot advance a fade.
        let declick = transport_change && !closing && !self.status.stream_buffering.load(Acquire);
        let mut fading = false;
        let result = if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            let applied = if declick {
                active.playback.try_apply_declicked_action(action)
            } else {
                active.playback.try_apply_action(action)
            };
            fading = applied.is_ok() && active.playback.pending_transport_fade() == Some(action);
            let outcome = match applied {
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
            };
            if fading {
                self.begin_transport_fade(ticket, action, true);
            }
            outcome
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
        };
        if !fading {
            if stop {
                self.revoke_current_sampler_keyboard();
            }
            if transport_change {
                self.clear_active_stream_roles();
            }
        }
        result
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
        if cut.stop_through.is_some() {
            // The immediate priority Stop follows an interrupted fade's effects.
            self.settle_transport_fade();
        }
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
                    active
                        .playback
                        .apply_priority_stop(&self.status.keyboard_activation);
                } else {
                    let _ = self.apply_stream_zero_page_action(
                        PlaybackAction::Transport(PlaybackCommand::Stop),
                        stop,
                        false,
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
            // Each command follows the audible end of an earlier fade.
            self.settle_transport_fade();
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
                    self.invalidate_stream_readiness_keeping_active_pages();
                    self.invalidate_midi_recording(
                        crate::midi_recording::AbortReason::TransportChanged,
                    );
                    let outcome =
                        self.apply_stream_zero_page_action(queued.action, queued.ticket, true);
                    self.stage_stream_outcome(queued.ticket, outcome);
                    if self.transport_fade.is_some() {
                        self.abort_transport_inputs(
                            crate::gain_recording::AbortReason::TransportChanged,
                        );
                    } else {
                        self.invalidate_for_transport_change(queued.action);
                    }
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
                    let outcome =
                        self.apply_stream_zero_page_action(queued.action, queued.ticket, false);
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

#[cfg(test)]
mod tests {
    use super::{Relaxed, Status, count_starvation, record_outcome};
    use crate::candidate_custody::{StreamOutcome, StreamProgress};

    fn outcome(progress: StreamProgress) -> StreamOutcome {
        StreamOutcome {
            progress,
            play_applied: false,
            readiness_refused: false,
            source_changed: false,
            read_failed: false,
            fault: None,
            action_rejection: None,
            action_applied: None,
            activated: None,
        }
    }

    #[test]
    fn only_holds_during_established_playback_count_as_starvation() {
        let status = Status::default();
        let held = outcome(StreamProgress::Buffering {
            committed_frames: 64,
        });
        let complete = outcome(StreamProgress::Complete);
        // Stopped, Play pre-roll and action pre-roll holds are expected waits.
        count_starvation(&status, false, false, &held);
        status.playing.store(true, Relaxed);
        count_starvation(&status, true, false, &held);
        count_starvation(&status, false, true, &held);
        count_starvation(&status, false, false, &complete);
        assert_eq!(status.stream_starved_callbacks.load(Relaxed), 0);
        count_starvation(&status, false, false, &held);
        count_starvation(
            &status,
            false,
            false,
            &outcome(StreamProgress::Buffering {
                committed_frames: 0,
            }),
        );
        assert_eq!(status.stream_starved_callbacks.load(Relaxed), 2);
    }

    #[test]
    fn read_failure_notifications_coalesce_until_taken_and_allow_later_faults() {
        let status = Status::default();
        let mut pending = None;
        assert!(!status.take_stream_read_failure());
        for read_failed in [true, true, false] {
            let mut result = outcome(StreamProgress::Complete);
            result.read_failed = read_failed;
            record_outcome(&status, &mut pending, 0, false, result);
        }
        assert!(status.take_stream_read_failure());
        assert!(!status.take_stream_read_failure());
        let mut result = outcome(StreamProgress::Buffering {
            committed_frames: 0,
        });
        result.read_failed = true;
        record_outcome(&status, &mut pending, 0, false, result);
        assert!(status.take_stream_read_failure());
        assert!(!status.take_stream_read_failure());
        assert!(!status.stream_source_changed.load(Relaxed));
        assert!(!status.render_fault.load(Relaxed));
    }
}
