//! Private publication operations; tracked origins never expose raw Playback.
use super::*;
impl PublicationPlayback {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn from_tracked(owner: hex_engine::storage_report::OwnerTrackedPlayback) -> Self {
        Self {
            live: None,
            retired: None,
            tracked: Some(owner),
            tracked_retired: None,
            manifest: None,
        }
    }
    pub(crate) fn tracked_matches_anchors(
        &self,
        anchors: &hex_engine::storage_report::AnchoredPlaybackClaims,
    ) -> bool {
        if let Some(owner) = self.tracked.as_ref() {
            owner.matches_anchors(anchors)
        } else {
            self.tracked_retired
                .as_ref()
                .is_some_and(|owner| owner.matches_anchors(anchors))
        }
    }
    pub(crate) fn tracked_identity(&self) -> Option<hex_engine::storage_report::ClaimIdentity> {
        self.tracked
            .as_ref()
            .map(|owner| owner.identity())
            .or_else(|| self.tracked_retired.as_ref().map(|owner| owner.identity()))
    }
    pub(crate) fn sample_rate(&self) -> u32 {
        if let Some(owner) = self.tracked.as_ref() {
            owner.sample_rate()
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .sample_rate()
        }
    }
    pub(crate) fn position(&self) -> u64 {
        if let Some(owner) = self.tracked.as_ref() {
            owner.position()
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .position()
        }
    }
    pub(crate) fn performance_frame(&self) -> u64 {
        if let Some(owner) = self.tracked.as_ref() {
            owner.performance_frame()
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .performance_frame()
        }
    }
    pub(crate) fn playing(&self) -> bool {
        if let Some(owner) = self.tracked.as_ref() {
            owner.playing()
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .playing()
        }
    }
    pub(crate) fn session_capability_status(&self) -> hex_engine::SessionCapabilityStatus {
        if let Some(owner) = self.tracked.as_ref() {
            owner.session_capability_status()
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .session_capability_status()
        }
    }
    pub(crate) fn session_route_state_generation(&self) -> u64 {
        if let Some(owner) = self.tracked.as_ref() {
            owner.session_route_state_generation()
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .session_route_state_generation()
        }
    }
    pub(crate) fn session_route_count(&self) -> usize {
        if let Some(owner) = self.tracked.as_ref() {
            owner.session_route_count()
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .session_route_count()
        }
    }
    pub(crate) fn session_route_status(
        &self,
        index: usize,
    ) -> Option<hex_engine::SessionRouteStatus> {
        if let Some(owner) = self.tracked.as_ref() {
            owner.session_route_status(index)
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .session_route_status(index)
        }
    }
    pub(crate) fn cancel_all_processor_auditions(&mut self) {
        if let Some(owner) = self.tracked.as_mut() {
            owner.cancel_all_processor_auditions()
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .cancel_all_processor_auditions()
        }
    }
    pub(crate) fn cancel_retired_processor_auditions(&mut self) {
        if let Some(owner) = self.tracked.as_mut() {
            owner.cancel_retired_processor_auditions()
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .cancel_retired_processor_auditions()
        }
    }
    pub(crate) fn cancel_processor_audition(
        &mut self,
        identity: hex_engine::PostNodeIdentity,
    ) -> bool {
        if let Some(owner) = self.tracked.as_mut() {
            owner.cancel_processor_audition(identity)
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .cancel_processor_audition(identity)
        }
    }
    pub(crate) fn select_processor_audition(
        &mut self,
        identity: hex_engine::PostNodeIdentity,
        selection: Option<hex_engine::ProcessorAuditionSelection>,
    ) -> bool {
        if let Some(owner) = self.tracked.as_mut() {
            owner.select_processor_audition(identity, selection)
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .select_processor_audition(identity, selection)
        }
    }
    pub(crate) fn processor_audition_is_active(
        &mut self,
        identity: hex_engine::PostNodeIdentity,
        selection: hex_engine::ProcessorAuditionSelection,
    ) -> bool {
        if let Some(owner) = self.tracked.as_mut() {
            owner.processor_audition_is_active(identity, selection)
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .processor_audition_is_active(identity, selection)
        }
    }
    pub(crate) fn transfer_processor_audition_from_release_tail(
        &mut self,
        identity: hex_engine::PostNodeIdentity,
        selection: hex_engine::ProcessorAuditionSelection,
    ) -> bool {
        if let Some(owner) = self.tracked.as_mut() {
            owner.transfer_processor_audition_from_release_tail(identity, selection)
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .transfer_processor_audition_from_release_tail(identity, selection)
        }
    }
    pub(crate) fn queue_typing_input(
        &mut self,
        ticket: u64,
        action: hex_engine::typing_protocol::Action,
    ) -> Result<(), hex_engine::typing_protocol::Refusal> {
        if let Some(owner) = self.tracked.as_mut() {
            owner.queue_typing_input(ticket, action)
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .queue_typing_input(ticket, action)
        }
    }
    pub(crate) fn cut_typing_input(&mut self, through_epoch: u64) {
        if let Some(owner) = self.tracked.as_mut() {
            owner.cut_typing_input(through_epoch)
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .cut_typing_input(through_epoch)
        }
    }
    pub(crate) fn typing_admission(&self) -> Option<hex_engine::live_input::TypingAdmission> {
        if let Some(owner) = self.tracked.as_ref() {
            owner.typing_admission()
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .typing_admission()
        }
    }
    pub(crate) fn typing_cut_through_epoch(&self) -> u64 {
        if let Some(owner) = self.tracked.as_ref() {
            owner.typing_cut_through_epoch()
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .typing_cut_through_epoch()
        }
    }
    pub(crate) fn take_typing_application(
        &mut self,
    ) -> Option<hex_engine::live_input::TypingApplication> {
        if let Some(owner) = self.tracked.as_mut() {
            owner.take_typing_application()
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .take_typing_application()
        }
    }
    pub(crate) fn try_apply_action(
        &mut self,
        action: hex_engine::PlaybackAction,
    ) -> Result<(), hex_engine::PlaybackActionError> {
        if let Some(owner) = self.tracked.as_mut() {
            owner.try_apply_action(action)
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .try_apply_action(action)
        }
    }
    pub(crate) fn render_with_post_node_observer(
        &mut self,
        output: &mut [hex_engine::Frame],
        observer: Option<&mut hex_engine::PostNodeObserver<'_>>,
    ) -> Result<(), hex_engine::Invalid> {
        if let Some(owner) = self.tracked.as_mut() {
            owner.render_with_post_node_observer(output, observer)
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .render_with_post_node_observer(output, observer)
        }
    }

    pub(crate) fn apply(&mut self, command: PlaybackCommand) {
        if let Some(owner) = self.tracked.as_mut() {
            owner.apply(command);
        } else {
            self.live
                .as_deref_mut()
                .expect("retired publication is not renderable")
                .apply(command);
        }
    }
    pub(crate) fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        if let Some(owner) = self.tracked.as_mut() {
            owner.render(output)
        } else {
            self.live
                .as_deref_mut()
                .ok_or(Invalid("retired publication cannot render"))?
                .render(output)
        }
    }
    pub(crate) fn render_with_pages_and_post_node_observer<P: hex_engine::MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        observer: Option<&mut hex_engine::PostNodeObserver<'_>>,
    ) -> Result<(), Invalid> {
        if let Some(owner) = self.tracked.as_mut() {
            owner.render_with_pages_and_post_node_observer(output, pages, observer)
        } else {
            self.live
                .as_deref_mut()
                .ok_or(Invalid("retired publication cannot render"))?
                .render_with_pages_and_post_node_observer(output, pages, observer)
        }
    }
    pub(crate) fn prepared_origin(&self) -> hex_engine::playback::PreparedOrigin {
        if self.tracked.is_some() || self.tracked_retired.is_some() {
            hex_engine::playback::PreparedOrigin::OwnedMetadata
        } else {
            self.live
                .as_deref()
                .expect("retired publication is not renderable")
                .prepared_origin()
        }
    }
    pub(crate) fn prepared_revision(&self) -> Option<&Arc<hex_engine::PreparedRevision>> {
        // Legacy availability only; tracked origin must never be treated as unprepared.
        self.live.as_deref().and_then(Playback::prepared_revision)
    }
    pub(crate) fn visit_next_stream_chunk(
        &self,
        maximum_frames: usize,
        visit: &mut impl FnMut(hex_engine::StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<hex_engine::StreamChunkWindow, Invalid> {
        if let Some(owner) = self.tracked.as_ref() {
            owner.visit_next_stream_chunk(maximum_frames, visit)
        } else {
            self.live
                .as_deref()
                .ok_or(Invalid("retired publication cannot plan"))?
                .visit_next_stream_chunk(maximum_frames, visit)
        }
    }
    pub(crate) fn preview_stream_handoff(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        visit: &mut impl FnMut(hex_engine::StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<hex_engine::StreamChunkWindow, hex_engine::PlaybackHandoffError> {
        match (self.tracked.as_mut(), previous.tracked.as_mut()) {
            (Some(next), Some(old)) => next.preview_stream_handoff(old, maximum_frames, visit),
            (None, None) => match (self.live.as_deref_mut(), previous.live.as_deref_mut()) {
                (Some(next), Some(old)) => next.preview_stream_handoff(old, maximum_frames, visit),
                _ => Err(hex_engine::PlaybackHandoffError::ArrangementStateCannotBeCaptured),
            },
            _ => Err(hex_engine::PlaybackHandoffError::ArrangementStateCannotBeCaptured),
        }
    }
    pub(crate) fn try_inherit_runtime_from(
        &mut self,
        previous: &mut Self,
    ) -> Result<(), hex_engine::PlaybackHandoffError> {
        // Tracked owners always require the conserved retiring commit below.
        match (self.live.as_deref_mut(), previous.live.as_deref_mut()) {
            (Some(next), Some(old)) => next.try_inherit_runtime_from(old),
            _ => Err(hex_engine::PlaybackHandoffError::ArrangementStateCannotBeCaptured),
        }
    }
    pub(crate) fn try_inherit_and_retire_previous_in_place(
        &mut self,
        previous: &mut Self,
    ) -> Result<(), hex_engine::playback::BorrowedRetiringHandoffFailure> {
        if let Some(next) = self.tracked.as_mut() {
            return next
                .try_inherit_and_retire_in_place(
                    &mut previous.tracked,
                    &mut previous.tracked_retired,
                    &mut previous.manifest,
                )
                .map_err(
                    |failure| hex_engine::playback::BorrowedRetiringHandoffFailure {
                        error: failure.error,
                        candidate_consumed: failure.candidate_consumed,
                    },
                );
        }
        match self.live.as_deref_mut() {
            Some(next) if previous.tracked.is_none() && previous.tracked_retired.is_none() => next
                .try_inherit_and_retire_previous_in_place(
                    &mut previous.live,
                    &mut previous.retired,
                ),
            _ => Err(hex_engine::playback::BorrowedRetiringHandoffFailure {
                error: hex_engine::PlaybackHandoffError::ArrangementStateCannotBeCaptured,
                candidate_consumed: false,
            }),
        }
    }
}
