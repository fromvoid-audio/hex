//! One scalar-only prepared action on the actual persistent active State.
use super::*;
use hex_engine::{PlaybackAction, PlaybackActionRejection};
pub(crate) enum StageAction {
    Applied,
    Pending,
    Refused(PlaybackActionRejection),
}
pub(super) enum ActionResolution {
    Ready(Option<(u64, PlaybackActionRejection)>),
    Pending,
}
impl Active {
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
        if self.has_stream_action() {
            return StageAction::Refused(PlaybackActionRejection::StreamActionBusy);
        }
        let Some(runtime) = self.state.runtime.as_mut() else {
            return StageAction::Refused(PlaybackActionRejection::SessionNotPrepared);
        };
        let prepared = match runtime.prepare_stream_action(action) {
            Ok(prepared) => prepared,
            Err(error) => return StageAction::Refused(error.telemetry_rejection()),
        };
        if prepared.scheduled_due_frame().is_some() {
            // Future scheduling/prewarm is connected in the next native layer.
            return StageAction::Refused(PlaybackActionRejection::StreamActionBusy);
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
                return ActionResolution::Ready(
                    self.cancel_stream_action(request.domains)
                        .map(|ticket| (ticket, PlaybackActionRejection::StreamPagesNotReady)),
                );
            }
        }
        let promotions = match self.state.prepare_stream_commit(request.domains) {
            Ok(promotions) => promotions,
            Err(_) => {
                return ActionResolution::Ready(
                    self.cancel_stream_action(request.domains)
                        .map(|ticket| (ticket, PlaybackActionRejection::StreamPagesNotReady)),
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
            return ActionResolution::Ready(Some((pending.ticket, error.telemetry_rejection())));
        }
        for promotion in promotions.into_iter().flatten() {
            promotion.commit();
        }
        self.state.record_stream_commit();
        ActionResolution::Ready(None)
    }
}
