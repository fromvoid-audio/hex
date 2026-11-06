//! One scalar-only prepared action on the actual persistent active State.
use super::*;
use hex_engine::{PlaybackAction, PlaybackActionRejection};
pub(crate) enum StageAction {
    Applied,
    Pending,
    Scheduled,
    Refused(PlaybackActionRejection),
}
pub(super) enum ActionResolution {
    Ready(Option<(u64, Result<(), PlaybackActionRejection>)>),
    Pending,
}
impl ActiveView<'_> {
    pub(crate) fn has_stream_action(&self) -> bool {
        self.state
            .streaming
            .as_ref()
            .is_some_and(|w| w.pending_action.is_some())
    }
    pub(crate) fn stage_stream_action(
        &mut self,
        action: PlaybackAction,
        ticket: u64,
    ) -> StageAction {
        if self.has_stream_action()
            || self
                .state
                .streaming
                .as_ref()
                .is_some_and(|w| w.scheduled.is_some())
        {
            return StageAction::Refused(PlaybackActionRejection::StreamActionBusy);
        }
        let Some(runtime) = self.state.runtime.as_mut() else {
            return StageAction::Refused(PlaybackActionRejection::SessionNotPrepared);
        };
        let prepared = match runtime.prepare_stream_action(action) {
            Ok(prepared) => prepared,
            Err(error) => return StageAction::Refused(error.telemetry_rejection()),
        };
        if let Some(due) = prepared.scheduled_due_frame() {
            let Some(workspace) = self.state.streaming.as_mut() else {
                return StageAction::Refused(PlaybackActionRejection::SessionNotPrepared);
            };
            if !matches!(workspace.candidate_role, CandidateRole::None) {
                return StageAction::Refused(PlaybackActionRejection::StreamActionBusy);
            }
            if let Err(error) = runtime.commit_stream_action(prepared) {
                return StageAction::Refused(error.telemetry_rejection());
            }
            let pending = runtime.status().is_some_and(|status| {
                (0..status.route_count).any(|index| {
                    runtime.session_route_status(index).is_some_and(|route| {
                        route
                            .pending
                            .is_some_and(|pending| pending.performance_frame == due)
                    })
                })
            });
            if !pending {
                return StageAction::Applied;
            }
            workspace.scheduled = Some(ScheduledAction {
                ticket,
                due,
                allow_prewarm: true,
                pressure_blocked: false,
            });
            return StageAction::Scheduled;
        }
        if !prepared.playing_after_commit() {
            return match runtime.commit_stream_action(prepared) {
                Ok(()) => StageAction::Applied,
                Err(error) => StageAction::Refused(error.telemetry_rejection()),
            };
        }
        let Some(workspace) = self.state.streaming.as_mut() else {
            return StageAction::Refused(PlaybackActionRejection::SessionNotPrepared);
        };
        workspace.pending_action = Some(PendingAction { prepared, ticket });
        StageAction::Pending
    }
    pub(crate) fn cancel_stream_action(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) -> Option<u64> {
        let pending = self.state.streaming.as_mut()?.pending_action.take()?;
        self.state.cancel_stream_pending(domains);
        Some(pending.ticket)
    }
    pub(crate) fn cancel_pending_routes(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) -> Result<Option<u64>, hex_engine::PlaybackActionError> {
        let hex_engine::SessionPlaybackCommand::CancelPending { track } = command else {
            return Err(hex_engine::PlaybackActionError::Session(
                hex_engine::SessionPlaybackError::InvalidCommand,
            ));
        };
        let runtime = self
            .state
            .runtime
            .as_mut()
            .ok_or(hex_engine::PlaybackActionError::SessionNotPrepared)?;
        let matching = if let Some(pending) = self
            .state
            .streaming
            .as_ref()
            .and_then(|w| w.pending_action.as_ref())
        {
            runtime.stream_action_targets_track(&pending.prepared, track)?
        } else {
            false
        };
        runtime.apply_action(PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::CancelPending { track },
        ))?;
        let cancelled = if matching {
            self.cancel_stream_action(domains)
        } else {
            None
        };
        self.revoke_launch_prewarm(domains);
        Ok(cancelled)
    }
    pub(super) fn resolve_stream_action(
        &mut self,
        request: &mut StreamInstall<'_>,
    ) -> ActionResolution {
        if !self.has_stream_action() {
            return ActionResolution::Ready(None);
        }
        match self.state.prepare_stream_window(
            None,
            request,
            hex_stream::ProtectionRole::Transition,
        ) {
            Ok(Some(_)) => {}
            Ok(None) => return ActionResolution::Pending,
            Err(_) => {
                let source_changed = self
                    .state
                    .streaming
                    .as_mut()
                    .is_some_and(|workspace| std::mem::take(&mut workspace.source_changed));
                let reason = if source_changed {
                    PlaybackActionRejection::StreamSourceChanged
                } else {
                    PlaybackActionRejection::StreamPagesNotReady
                };
                return ActionResolution::Ready(
                    self.cancel_stream_action(request.domains)
                        .map(|ticket| (ticket, Err(reason))),
                );
            }
        }
        let promotions = match self.state.prepare_stream_commit(request.domains) {
            Ok(promotions) => promotions,
            Err(_) => {
                return ActionResolution::Ready(
                    self.cancel_stream_action(request.domains)
                        .map(|ticket| (ticket, Err(PlaybackActionRejection::StreamPagesNotReady))),
                );
            }
        };
        let pending = self
            .state
            .streaming
            .as_mut()
            .and_then(|w| w.pending_action.take());
        let Some(pending) = pending else {
            return ActionResolution::Ready(None);
        };
        let result = self
            .state
            .runtime
            .as_mut()
            .ok_or(hex_engine::PlaybackActionError::SessionNotPrepared)
            .and_then(|runtime| runtime.commit_stream_action(pending.prepared));
        if let Err(error) = result {
            self.state.cancel_stream_pending(request.domains);
            return ActionResolution::Ready(Some((
                pending.ticket,
                Err(error.telemetry_rejection()),
            )));
        }
        for promotion in promotions.into_iter().flatten() {
            promotion.commit();
        }
        self.state.record_stream_commit();
        ActionResolution::Ready(Some((pending.ticket, Ok(()))))
    }
}
