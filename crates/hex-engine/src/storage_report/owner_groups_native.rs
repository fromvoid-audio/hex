//! Audited native scalar/command/render forwarding; no ownership/preparation escape.
use super::OwnerTrackedPlayback;
impl OwnerTrackedPlayback {
    /// Borrow actual pairing; no underlying root/runtime handle escapes.
    pub fn matches_anchors(&self, anchors: &super::AnchoredPlaybackClaims) -> bool {
        self.demand_pair_valid() && anchors.matches_owner(&self.owner)
    }
    pub fn same_prepared_owner(&self, root: &crate::OwnedPreparedRevision) -> bool {
        self.demand_pair_valid() && self.owner.same_prepared_owner(root)
    }

    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn sample_rate(&self) -> u32 {
        self.owner.playback.sample_rate()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn position(&self) -> u64 {
        self.owner.playback.position()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn performance_frame(&self) -> u64 {
        self.owner.playback.performance_frame()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn playing(&self) -> bool {
        self.owner.playback.playing()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn session_capability_status(&self) -> crate::SessionCapabilityStatus {
        self.owner.playback.session_capability_status()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn session_route_state_generation(&self) -> u64 {
        self.owner.playback.session_route_state_generation()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn session_route_count(&self) -> usize {
        self.owner.playback.session_route_count()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn session_route_status(&self, index: usize) -> Option<crate::SessionRouteStatus> {
        self.owner.playback.session_route_status(index)
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn cancel_all_processor_auditions(&mut self) {
        self.owner.playback.cancel_all_processor_auditions()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn cancel_retired_processor_auditions(&mut self) {
        self.owner.playback.cancel_retired_processor_auditions()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn cancel_processor_audition(&mut self, identity: crate::PostNodeIdentity) -> bool {
        self.owner.playback.cancel_processor_audition(identity)
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn select_processor_audition(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: Option<hex_lattice::ProcessorAuditionSelection>,
    ) -> bool {
        self.owner
            .playback
            .select_processor_audition(identity, selection)
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn processor_audition_is_active(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: hex_lattice::ProcessorAuditionSelection,
    ) -> bool {
        self.owner
            .playback
            .processor_audition_is_active(identity, selection)
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn transfer_processor_audition_from_release_tail(
        &mut self,
        identity: crate::PostNodeIdentity,
        selection: hex_lattice::ProcessorAuditionSelection,
    ) -> bool {
        self.owner
            .playback
            .transfer_processor_audition_from_release_tail(identity, selection)
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn queue_typing_input(
        &mut self,
        ticket: u64,
        action: crate::typing_protocol::Action,
    ) -> Result<(), crate::typing_protocol::Refusal> {
        self.owner.playback.queue_typing_input(ticket, action)
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn cut_typing_input(&mut self, through_epoch: u64) {
        self.owner.playback.cut_typing_input(through_epoch)
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn typing_admission(&self) -> Option<crate::live_input::TypingAdmission> {
        self.owner.playback.typing_admission()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn typing_cut_through_epoch(&self) -> u64 {
        self.owner.playback.typing_cut_through_epoch()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn take_typing_application(&mut self) -> Option<crate::live_input::TypingApplication> {
        self.owner.playback.take_typing_application()
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn try_apply_action(
        &mut self,
        action: crate::PlaybackAction,
    ) -> Result<(), crate::PlaybackActionError> {
        self.owner.playback.try_apply_action(action)
    }
    /// Borrow the existing operation; no owner or mutable runtime escapes.
    pub fn render_with_post_node_observer(
        &mut self,
        output: &mut [crate::Frame],
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), crate::Invalid> {
        self.owner
            .playback
            .render_with_post_node_observer(output, observer)
    }
}

impl super::RetiredOwnerTrackedPlayback {
    /// Retired owners expose pairing and control teardown only, never rendering.
    pub fn matches_anchors(&self, anchors: &super::AnchoredPlaybackClaims) -> bool {
        self.owner.matches_anchors(anchors)
    }
}
impl super::OwnerGroupArchive {
    pub fn matches_tracked_owner(&self, owner: &OwnerTrackedPlayback) -> bool {
        self.runtime == owner.identity()
            && owner.demand_pair_valid()
            && self.report.incomplete_source == owner.is_incomplete_fixture()
            && owner
                .owner
                .playback
                .claim_prepared_owner()
                .is_some_and(|root| std::sync::Arc::ptr_eq(root, &self.root))
    }
}
