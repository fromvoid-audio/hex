//! Borrowed access to the shared readiness/transport kernel. These views contain
//! no runtime, source, page endpoint or native-owner moves. Constructor custody,
//! cancellation and final destruction remain in the actual State/Root.
use super::*;
use hex_stream::ProtectionRole;

#[derive(Clone, Copy)]
pub(super) struct Correlation {
    pub(super) root: u64,
    pub(super) owner: hex_stream::ReadinessArtifact,
}
pub(super) struct StateView<'a> {
    pub(super) runtime: Option<hex_engine::StreamPlayback<'a>>,
    pub(super) streaming: &'a mut Option<Workspace>,
    pub(super) domain_refs: [Option<AudioCandidateDomainToken>; 2],
    pub(super) context_generation: u64,
    pub(super) correlation: Correlation,
}
pub(crate) struct ActiveView<'a> {
    pub(super) state: StateView<'a>,
}
impl ActiveView<'_> {
    pub(crate) fn capacity_conflict(&self) -> bool {
        self.state
            .streaming
            .as_ref()
            .is_some_and(|workspace| workspace.capacity_conflict)
    }
    /// A current render window has reserved readiness but has not committed.
    pub(crate) fn has_pending_render_window(&self) -> bool {
        self.state.streaming.as_ref().is_some_and(|workspace| {
            workspace.pending_action.is_none()
                && workspace.role == Some(ProtectionRole::Transition)
                && workspace.stamps.iter().any(Option::is_some)
        })
    }
    pub(crate) fn cancel_pending_render_window(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        self.state.cancel_stream_pending(domains);
    }
}
impl State {
    fn stream_view(&mut self) -> StateView<'_> {
        StateView {
            runtime: self.runtime.as_mut().map(|r| r.stream_playback()),
            streaming: &mut self.streaming,
            domain_refs: self.domain_refs,
            context_generation: self.context_generation,
            correlation: Correlation {
                root: self.stamp.root,
                owner: self.stamp.owner.into(),
            },
        }
    }
    pub(in crate::candidate_custody::eligible::publication) fn cancel_stream_pending(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        self.stream_view().cancel_stream_pending(domains);
    }
    pub(in crate::candidate_custody::eligible::publication) fn prepare_stream_window(
        &mut self,
        previous: Option<&mut State>,
        request: &mut StreamInstall<'_>,
        role: ProtectionRole,
    ) -> Result<Option<hex_engine::StreamChunkWindow>, hex_engine::Invalid> {
        let mut previous = previous.map(State::stream_view);
        self.stream_view()
            .prepare_stream_window(previous.as_mut(), request, role)
    }
    pub(in crate::candidate_custody::eligible::publication) fn prepare_stream_commit<'a>(
        &mut self,
        domains: &'a mut [Option<DomainEnvelope>; 2],
    ) -> Result<[Option<hex_stream::PreparedRolePromotion<'a>>; 2], hex_engine::Invalid> {
        self.stream_view().prepare_stream_commit(domains)
    }
    pub(in crate::candidate_custody::eligible::publication) fn record_stream_commit(&mut self) {
        self.stream_view().record_stream_commit();
    }
}
impl Active {
    pub(crate) fn stream_view(&mut self) -> ActiveView<'_> {
        ActiveView {
            state: self.state.stream_view(),
        }
    }
    pub(crate) fn streaming_enabled(&self) -> bool {
        self.state.streaming.is_some()
    }
    pub(crate) fn has_stream_action(&self) -> bool {
        self.state
            .streaming
            .as_ref()
            .is_some_and(|w| w.pending_action.is_some())
    }
    pub(crate) fn clear_stream_roles(&mut self, domains: &mut [Option<DomainEnvelope>; 2]) {
        self.stream_view().clear_stream_roles(domains);
    }
    pub(crate) fn publication_replaces_prewarm(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        self.stream_view().publication_replaces_prewarm(domains);
    }
    #[cfg(test)]
    pub(crate) fn publication_abandoned(&mut self) {
        self.stream_view().publication_abandoned();
    }
    pub(crate) fn inherit_schedule_observation(&mut self, previous: &mut Active) -> Option<u64> {
        self.stream_view()
            .inherit_schedule_observation(&mut previous.stream_view())
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn service_launch_prewarm(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        prefix: u64,
        frames: usize,
    ) -> bool {
        self.stream_view()
            .service_launch_prewarm(domains, prefix, frames)
    }
}
