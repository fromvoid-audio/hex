//! Borrowed access to the shared readiness/transport kernel. These views contain
//! no runtime, source, page endpoint or native-owner moves. Constructor custody,
//! cancellation and final destruction remain in the actual State/Root.
use super::*;
use hex_engine::PlaybackAction;
use hex_stream::ProtectionRole;

pub(super) struct StateView<'a> {
    pub(super) runtime: Option<&'a mut hex_engine::ClaimedAudioCandidateRuntime>,
    pub(super) streaming: &'a mut Option<Workspace>,
    pub(super) domain_refs: [Option<AudioCandidateDomainToken>; 2],
    pub(super) context_generation: u64,
    pub(super) stamp: Stamp,
}
pub(super) struct ActiveView<'a> {
    pub(super) state: StateView<'a>,
}
impl State {
    fn stream_view(&mut self) -> StateView<'_> {
        StateView {
            runtime: self.runtime.as_mut(),
            streaming: &mut self.streaming,
            domain_refs: self.domain_refs,
            context_generation: self.context_generation,
            stamp: self.stamp,
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
    fn stream_view(&mut self) -> ActiveView<'_> {
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
    pub(crate) fn stage_stream_action(
        &mut self,
        action: PlaybackAction,
        ticket: u64,
    ) -> StageAction {
        self.stream_view().stage_stream_action(action, ticket)
    }
    pub(crate) fn cancel_stream_action(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) -> Option<u64> {
        self.stream_view().cancel_stream_action(domains)
    }
    pub(crate) fn cancel_pending_routes(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) -> Result<Option<u64>, hex_engine::PlaybackActionError> {
        self.stream_view().cancel_pending_routes(command, domains)
    }
    pub(crate) fn clear_stream_roles(&mut self, domains: &mut [Option<DomainEnvelope>; 2]) {
        self.stream_view().clear_stream_roles(domains);
    }
    pub(crate) fn revoke_launch_prewarm(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) -> bool {
        self.stream_view().revoke_launch_prewarm(domains)
    }
    pub(crate) fn publication_replaces_prewarm(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        self.stream_view().publication_replaces_prewarm(domains);
    }
    pub(crate) fn inherit_schedule_observation(&mut self, previous: &mut Active) -> Option<u64> {
        self.stream_view()
            .inherit_schedule_observation(&mut previous.stream_view())
    }
    pub(crate) fn refresh_scheduled_after_command(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) -> Option<u64> {
        self.stream_view().refresh_scheduled_after_command(domains)
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
    pub(crate) fn render_streamed(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        pending_play: bool,
        prefix: u64,
        output: &mut [hex_engine::Frame],
        observer: Option<&mut hex_engine::NodeSignalObserver<'_>>,
    ) -> StreamOutcome {
        self.stream_view()
            .render_streamed(domains, pending_play, prefix, output, observer)
    }
    pub(crate) fn render_streamed_with_project(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        pending_play: bool,
        prefix: u64,
        output: &mut [hex_engine::Frame],
        observer: Option<&mut hex_engine::NodeSignalObserver<'_>>,
        project: Option<&mut hex_engine::ProjectOutputTap<'_, '_>>,
    ) -> StreamOutcome {
        self.stream_view().render_streamed_with_project(
            domains,
            pending_play,
            prefix,
            output,
            observer,
            project,
        )
    }
}
