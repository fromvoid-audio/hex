//! Physical runtime/anchor split. It grants no native publication or cost
//! completeness by itself. Hosts must retain exact covering anchors and their
//! own actual catalog/resource custody until every report is consumed.
use super::*;
mod playback;
pub use playback::{AudioCandidateRuntimeStatus, StreamPlayback};

pub struct AudioCandidateSplitRefusal {
    pub reason: AudioStorageError,
    pub owner: AudioCandidateOwner,
}
/// The original preparation wrapper/bank/token stays intact in control custody.
pub struct AudioCandidateAnchors {
    pub(super) owner: AudioCandidateOwner,
}
struct ClaimedState {
    identity: AudioCandidateIdentity,
    source_context: Option<crate::AudioSourceContext>,
    runtime: Option<Box<RuntimeOwner>>,
    groups: Vec<Group>,
    journal: Vec<groups::TransferRow>,
    phase: ClaimedPhase,
    first_panic: Option<Box<dyn Any + Send>>,
    owned_runtime_bytes: Option<usize>,
}
impl ClaimedState {
    fn wrapper_storage_bytes(&self) -> usize {
        let Self {
            identity: _,
            source_context: _,
            runtime: _,
            groups: _,
            journal: _,
            phase: _,
            first_panic: _,
            owned_runtime_bytes: _,
        } = self;
        // Existing Runtime/Vec allocations move unchanged from the constructor;
        // only this wrapper is a new allocation at the split frontier.
        std::mem::size_of::<Self>()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClaimedPhase {
    Current,
    Retired,
    Pending,
    Disposed,
    Failed,
}
/// No raw Playback/Arc, mutable-root, Clone or legacy reconstruction escape.
/// Its exact nested Runtime Box and group/journal backings come from the same
/// original constructor; a split never reruns the compiler or adopts a runtime.
pub struct ClaimedAudioCandidateRuntime {
    inner: Box<ClaimedState>,
}
/// A borrow of both actual halves, never a detached scalar capability. Native
/// hosts must retain these exact anchors before transferring their runtime.
pub struct AudioCandidateConstructorClaims<'a> {
    anchors: &'a AudioCandidateAnchors,
    runtime: &'a ClaimedAudioCandidateRuntime,
}
impl AudioCandidateConstructorClaims<'_> {
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.runtime.identity()
    }
    pub fn summary(&self) -> Option<AudioCandidateConstructorSummary> {
        self.anchors.owner.inner.constructor_data
    }
}
#[derive(Clone, Copy)]
pub(crate) enum CandidateHandoff<'a> {
    Ordinary,
    ApprovedInsert,
    Tempo(
        &'a crate::session_playback::PreparedTempoAnchor,
        crate::session_playback::ClaimedTempoPair,
    ),
}

pub(super) fn reserved_state_bytes() -> usize {
    std::mem::size_of::<ClaimedState>()
}
impl AudioCandidateOwner {
    /// All reported refusals precede owner movement. Box allocation follows the
    /// existing control-side fatal-OOM convention. The host must preflight and
    /// charge this wrapper plus all simultaneous anchor/runtime storage first.
    pub fn split_constructor_on_control(
        self,
    ) -> Result<
        (ClaimedAudioCandidateRuntime, AudioCandidateAnchors),
        Box<AudioCandidateSplitRefusal>,
    > {
        match self.prepare_constructor_split() {
            Ok(state) => Ok(self.commit_constructor_split(state)),
            Err(reason) => Err(Box::new(AudioCandidateSplitRefusal {
                reason,
                owner: self,
            })),
        }
    }
    fn prepare_constructor_split(&self) -> Result<Box<ClaimedState>, AudioStorageError> {
        if self.inner.phase != AudioCandidateDisposalPhase::Owned
            || self.inner.runtime_receipt.is_some()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.inner.validate_groups()?;
        self.inner.validate_fresh_origins()?;
        let state = Box::new(ClaimedState {
            identity: self.identity(),
            source_context: self.source_context(),
            runtime: None,
            groups: Vec::new(),
            journal: Vec::new(),
            phase: ClaimedPhase::Current,
            first_panic: None,
            owned_runtime_bytes: self.inner.constructor_data.map(|s| s.owned_runtime_bytes),
        });
        let _actual_wrapper_bytes = state.wrapper_storage_bytes();
        Ok(state)
    }
    fn commit_constructor_split(
        mut self,
        mut state: Box<ClaimedState>,
    ) -> (ClaimedAudioCandidateRuntime, AudioCandidateAnchors) {
        // Only fixed moves after every check/allocation. The actual Runtime Box
        // and the original outer preparation wrapper both keep their addresses.
        state.runtime = self.inner.runtime.take();
        state.groups = std::mem::take(&mut self.inner.groups);
        state.journal = std::mem::take(&mut self.inner.journal);
        self.inner.phase = AudioCandidateDisposalPhase::Leased;
        (
            ClaimedAudioCandidateRuntime { inner: state },
            AudioCandidateAnchors { owner: self },
        )
    }
}
impl AudioCandidatePreparation {
    /// Final transfer to the native host's already-reserved publication receiver.
    /// Every fallible validation/allocation borrows the rooted owner; an error
    /// leaves it in this cell. The caller must immediately retain BOTH resulting
    /// halves before granting any installation authority.
    pub fn split_for_publication_on_control(
        &mut self,
    ) -> Result<(ClaimedAudioCandidateRuntime, AudioCandidateAnchors), AudioStorageError> {
        let state = self
            .owner_on_control()
            .ok_or(AudioStorageError::SourceMismatch)?
            .prepare_constructor_split()?;
        let owner = self
            .take_preflighted_owner()
            .ok_or(AudioStorageError::SourceMismatch)?;
        Ok(owner.commit_constructor_split(state))
    }
}
impl AudioCandidateAnchors {
    pub fn constructor_claims<'a>(
        &'a self,
        runtime: &'a ClaimedAudioCandidateRuntime,
    ) -> Option<AudioCandidateConstructorClaims<'a>> {
        if !self.matches_current(runtime)
            || self.owner.inner.constructor_data.is_none()
            || runtime.inner.phase != ClaimedPhase::Current
            || !runtime
                .inner
                .runtime
                .as_ref()?
                .playback
                .candidate_constructor_is_fresh()
            || runtime
                .inner
                .groups
                .iter()
                .enumerate()
                .any(|(member, group)| {
                    group.closed
                        || group.origin
                            != group.present.then_some(groups::Origin {
                                runtime: self.identity(),
                                member,
                            })
                })
        {
            return None;
        }
        Some(AudioCandidateConstructorClaims {
            anchors: self,
            runtime,
        })
    }
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.owner.identity()
    }
    pub fn preparation(&self) -> &AudioPreparationCustody {
        self.owner.preparation()
    }
    pub fn matches_current(&self, runtime: &ClaimedAudioCandidateRuntime) -> bool {
        if self.identity() != runtime.identity()
            || self.owner.source_context() != runtime.source_context()
            || self.owner.inner.phase != AudioCandidateDisposalPhase::Leased
        {
            return false;
        }
        let Ok(root) = self
            .owner
            .inner
            .custody
            .candidate_root_view(&self.owner.inner.token)
        else {
            return false;
        };
        runtime
            .inner
            .runtime
            .as_ref()
            .and_then(|r| r.playback.prepared_revision())
            .is_some_and(|r| std::sync::Arc::ptr_eq(r, root))
    }
    /// Prepare a cut from these two actual constructor roots on control.
    /// The plan borrows their allocation identities weakly; the host still
    /// owns graph admission, the live transport fence and all page custody.
    pub fn prepare_tempo_successor_on_control(
        &self,
        successor: &Self,
        arrangement_frame: u64,
        performance_frame: u64,
        loop_state: crate::session_playback::ArrangementLoopState,
        cut_performance_frame: u64,
    ) -> Result<crate::session_playback::PreparedTempoAnchor, crate::SessionPlaybackError> {
        let invalid = crate::SessionPlaybackError::InvalidCommand;
        if self.identity() == successor.identity()
            || [self, successor]
                .into_iter()
                .any(|anchors| anchors.owner.phase() != AudioCandidateDisposalPhase::Leased)
        {
            return Err(invalid);
        }
        let revision = |anchors: &Self| {
            anchors
                .owner
                .inner
                .custody
                .candidate_root_view(&anchors.owner.inner.token)
                .map(std::sync::Arc::clone)
                .map_err(|_| invalid)
        };
        crate::session_playback::PreparedTempoAnchor::prepare_from_running_observation(
            revision(self)?,
            revision(successor)?,
            arrangement_frame,
            performance_frame,
            loop_state,
            cut_performance_frame,
        )
        .map(|anchor| {
            anchor.bind_claimed_pair(crate::session_playback::ClaimedTempoPair {
                previous: self.identity(),
                candidate: successor.identity(),
            })
        })
    }

    pub fn original_runtime_disposed(&self) -> bool {
        self.owner.inner.archive.original_disposed
    }
    pub fn all_origins_closed(&self) -> bool {
        self.owner.inner.archive.all_closed()
    }
    /// Normal runtime proof must already have been atomically ingested into
    /// every actual origin anchor. No scalar identity can authorize this step.
    pub fn finish_disposal_on_control(
        &mut self,
    ) -> Result<AudioCandidateDisposed, AudioStorageError> {
        if self.owner.inner.phase == AudioCandidateDisposalPhase::Leased
            && self.owner.inner.archive.all_closed()
        {
            self.owner.inner.phase = AudioCandidateDisposalPhase::RootsCleaning;
        }
        self.owner.finish_disposal_on_control()
    }
    pub fn recover_inputs_on_control(&mut self) -> Option<OwnedAudioInputs> {
        self.owner.recover_inputs_on_control()
    }
    pub fn dispose_original_inputs_on_control(&mut self) -> Result<(), AudioStorageError> {
        self.owner.dispose_original_inputs_on_control()
    }
}
impl ClaimedAudioCandidateRuntime {
    pub fn source_context(&self) -> Option<crate::AudioSourceContext> {
        self.inner.source_context
    }

    /// Non-default fixture pressure on the real transfer preflight allowance.
    /// This changes no owner/location/activity flag and exposes no runtime.
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_set_owned_runtime_reservation(&mut self, bytes: usize) -> Option<usize> {
        self.inner.owned_runtime_bytes.replace(bytes)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_track_boxes(
        &self,
        id: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        self.inner.runtime.as_ref()?.playback.test_track_boxes(id)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_tail_boxes(
        &self,
        id: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        self.inner.runtime.as_ref()?.playback.test_tail_boxes(id)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_tail_activity(&self, id: hex_project::TrackId) -> Option<(bool, bool)> {
        self.inner.runtime.as_ref()?.playback.test_tail_activity(id)
    }
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.inner.identity
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_prepared_strong_count(&self) -> Option<usize> {
        self.inner
            .runtime
            .as_ref()?
            .playback
            .prepared_revision()
            .map(std::sync::Arc::strong_count)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_runtime_address(&self) -> Option<usize> {
        self.inner
            .runtime
            .as_ref()
            .map(|r| r.as_ref() as *const RuntimeOwner as usize)
    }
    pub fn sample_rate(&self) -> Option<u32> {
        self.inner
            .runtime
            .as_ref()
            .map(|r| r.playback.sample_rate())
    }
    /// Actual CURRENT sampler transport and activation, including epoch zero
    /// before its first keyboard input. No source or runtime owner is exposed.
    pub fn current_sampler_edit_status(&self) -> Option<(AudioCandidateRuntimeStatus, u64)> {
        let status = self.status()?;
        let runtime = self.inner.runtime.as_ref()?;
        runtime.sampler_sources.as_ref()?;
        Some((
            status,
            runtime
                .playback
                .current_sampler_keyboard_epoch()
                .unwrap_or(0),
        ))
    }
    /// Actual runtime activation scalar, independent of a newer mailbox binding.
    pub fn current_sampler_keyboard_epoch(&self) -> Option<u64> {
        let runtime = self.inner.runtime.as_ref()?;
        runtime.sampler_sources.as_ref()?;
        runtime.playback.current_sampler_keyboard_epoch()
    }
    pub fn current_sampler_keyboard_rejected_epoch(&self) -> Option<u64> {
        let runtime = self.inner.runtime.as_ref()?;
        runtime.sampler_sources.as_ref()?;
        runtime.playback.current_sampler_keyboard_rejected_epoch()
    }
    /// Scalar input only; the current runtime retains its exact constructor bank.
    pub fn apply_current_sampler_keyboard(
        &mut self,
        frame: Option<crate::computer_keyboard::CurrentSamplerKeyboardFrame>,
    ) -> Result<bool, crate::Invalid> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::Invalid("sampler runtime is not current"));
        }
        let runtime = self
            .inner
            .runtime
            .as_mut()
            .ok_or(crate::Invalid("sampler runtime unavailable"))?;
        if runtime.sampler_sources.is_none() {
            return Ok(false);
        }
        runtime.playback.apply_current_sampler_keyboard(frame)?;
        Ok(true)
    }
    /// Priority output Stop with the host's exact scalar clock. Ordinary playback may retain
    /// only its matching output-bound input; legacy native CURRENT keeps terminal Stop.
    pub fn apply_priority_stop(
        &mut self,
        clock: &crate::computer_keyboard::KeyboardActivationClock,
    ) -> Result<(), crate::PlaybackActionError> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::PlaybackActionError::SessionNotPrepared);
        }
        let runtime = self
            .inner
            .runtime
            .as_mut()
            .ok_or(crate::PlaybackActionError::SessionNotPrepared)?;
        if runtime.sampler_sources.is_some() {
            runtime.apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Stop,
            ))
        } else {
            runtime.playback.apply_priority_stop(clock);
            Ok(())
        }
    }
    pub fn apply_action(
        &mut self,
        action: crate::PlaybackAction,
    ) -> Result<(), crate::PlaybackActionError> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::PlaybackActionError::SessionNotPrepared);
        }
        self.inner
            .runtime
            .as_mut()
            .ok_or(crate::PlaybackActionError::SessionNotPrepared)?
            .apply_action(action)
    }
    /// `apply_action` with the transport declick of
    /// `Playback::try_apply_declicked_action`.
    pub fn apply_declicked_action(
        &mut self,
        action: crate::PlaybackAction,
    ) -> Result<(), crate::PlaybackActionError> {
        self.current_owner()?.apply_declicked_action(action)
    }
    /// The Pause/Stop whose fade is in progress, if any.
    pub fn pending_transport_fade(&self) -> Option<crate::PlaybackAction> {
        self.inner
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.playback.pending_transport_fade())
    }
    /// `Playback::finish_transport_fade` on the current runtime.
    pub fn finish_transport_fade(&mut self) {
        if let Some(runtime) = self.inner.runtime.as_mut() {
            runtime.playback.finish_transport_fade();
        }
    }
    /// One deferred Pause/Stop refusal recorded at its fade end.
    pub fn take_transport_fade_failure(&mut self) -> Option<crate::PlaybackActionRejection> {
        self.inner
            .runtime
            .as_mut()
            .and_then(|runtime| runtime.playback.take_transport_fade_failure())
    }
    fn current_owner(&mut self) -> Result<&mut RuntimeOwner, crate::PlaybackActionError> {
        self.current_runtime_mut()
            .ok_or(crate::PlaybackActionError::SessionNotPrepared)
    }
    pub fn render(&mut self, output: &mut [crate::Frame]) -> Result<(), crate::Invalid> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::Invalid("candidate runtime is not current"));
        }
        self.inner
            .runtime
            .as_mut()
            .ok_or(crate::Invalid("candidate runtime unavailable"))?
            .render_with_signal_observers(output, None, None)
    }
    /// Pure exact next-chunk demand. A proposed Play/Pause flag is speculative;
    /// neither transport nor page readiness is committed by this observation.
    pub fn visit_next_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), crate::Invalid>,
    ) -> Result<crate::StreamChunkWindow, crate::Invalid> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::Invalid("candidate runtime is not current"));
        }
        self.inner
            .runtime
            .as_ref()
            .ok_or(crate::Invalid("candidate runtime unavailable"))?
            .playback
            .visit_next_stream_chunk(maximum_frames, playing, visit)
    }
    /// Visit bounded callback demand without changing committed playback state.
    pub fn visit_current_stream_quantum(
        &mut self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), crate::Invalid>,
    ) -> Result<usize, crate::Invalid> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::Invalid("candidate runtime is not current"));
        }
        self.inner
            .runtime
            .as_mut()
            .ok_or(crate::Invalid("candidate runtime unavailable"))?
            .playback
            .visit_current_stream_quantum(maximum_frames, playing, false, visit)
    }
    /// Visit at most 256 frames at a checked offset from the current clock.
    /// This speculative observation does not commit playback state.
    pub fn visit_current_stream_quantum_at(
        &mut self,
        start_offset: u64,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), crate::Invalid>,
    ) -> Result<usize, crate::playback::StreamQuantumProjectionError> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::playback::StreamQuantumProjectionError::Projection(
                crate::Invalid("candidate runtime is not current"),
            ));
        }
        self.inner
            .runtime
            .as_mut()
            .ok_or(crate::playback::StreamQuantumProjectionError::Projection(
                crate::Invalid("candidate runtime unavailable"),
            ))?
            .playback
            .visit_current_stream_quantum_at(start_offset, maximum_frames, playing, false, visit)
    }
    /// Runs the real handoff's pure preflight against the actual predecessor.
    /// Scratch planning may change; committed owners/history/routes do not.
    pub fn preview_stream_handoff(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), crate::Invalid>,
    ) -> Result<crate::StreamChunkWindow, crate::PlaybackHandoffError> {
        self.preview_stream_handoff_with_report(previous, maximum_frames, playing, visit)
            .map(|(window, _)| window)
    }
    /// Preview the same claimed handoff and its running-audio classification.
    /// The report is observational and does not grant publication authority.
    pub fn preview_stream_handoff_with_report(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), crate::Invalid>,
    ) -> Result<(crate::StreamChunkWindow, crate::SessionHandoffReport), crate::PlaybackHandoffError>
    {
        let invalid =
            || crate::PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        if self.inner.phase != ClaimedPhase::Current
            || previous.inner.phase != ClaimedPhase::Current
            || self.identity() == previous.identity()
        {
            return Err(invalid());
        }
        let next = self.inner.runtime.as_mut().ok_or_else(invalid)?;
        let old = previous.inner.runtime.as_mut().ok_or_else(invalid)?;
        if next.sampler_sources.is_some() || old.sampler_sources.is_some() {
            return Err(invalid());
        }
        next.playback.preview_stream_handoff_with_report(
            &mut old.playback,
            maximum_frames,
            playing,
            visit,
            false,
            false,
        )
    }
    /// The current covering graph is actually borrowed through this runtime's
    /// own original prepared-root handle. There is no detached report accessor.
    pub fn contains_streamed_sources(&self) -> bool {
        self.inner
            .runtime
            .as_ref()
            .is_none_or(|r| r.playback.contains_streamed_sources())
    }
    pub fn try_inherit_and_retire_in_place(
        &mut self,
        previous: &mut Option<Self>,
        retired: &mut Option<Self>,
        manifest: &mut Option<AudioCandidateTransfer>,
    ) -> Result<(), crate::PlaybackHandoffError> {
        self.inherit_and_retire(previous, retired, manifest, CandidateHandoff::Ordinary)
    }

    /// Install an explicitly approved successor at the predecessor's held
    /// fade. The ordinary claims, transfer journal and retirement still own
    /// every resource that moves between these two graphs.
    pub fn try_inherit_and_retire_at_device_insert_cut(
        &mut self,
        previous: &mut Option<Self>,
        retired: &mut Option<Self>,
        manifest: &mut Option<AudioCandidateTransfer>,
    ) -> Result<(), crate::PlaybackHandoffError> {
        self.inherit_and_retire(
            previous,
            retired,
            manifest,
            CandidateHandoff::ApprovedInsert,
        )
    }

    /// Install the exact compensated tempo switch and retire the real donor
    /// through the same constructor transfer journal. A wrong clock, loop or
    /// revision refuses before changing the donor, manifest or origins. The
    /// host must still validate the live fence and actual protected pages.
    pub fn try_inherit_and_retire_at_tempo_cut(
        &mut self,
        previous: &mut Option<Self>,
        retired: &mut Option<Self>,
        manifest: &mut Option<AudioCandidateTransfer>,
        anchor: &crate::session_playback::PreparedTempoAnchor,
    ) -> Result<(), crate::PlaybackHandoffError> {
        let previous_identity = previous
            .as_ref()
            .ok_or(crate::PlaybackHandoffError::RuntimeFenceChanged)?
            .identity();
        let pair = crate::session_playback::ClaimedTempoPair {
            previous: previous_identity,
            candidate: self.identity(),
        };
        self.inherit_and_retire(
            previous,
            retired,
            manifest,
            CandidateHandoff::Tempo(anchor, pair),
        )
    }

    fn inherit_and_retire(
        &mut self,
        previous: &mut Option<Self>,
        retired: &mut Option<Self>,
        manifest: &mut Option<AudioCandidateTransfer>,
        handoff: CandidateHandoff<'_>,
    ) -> Result<(), crate::PlaybackHandoffError> {
        let invalid =
            || crate::PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        if self.inner.phase != ClaimedPhase::Current || retired.is_some() || manifest.is_some() {
            return Err(invalid());
        }
        let old = previous.as_mut().ok_or_else(invalid)?;
        if old.inner.phase != ClaimedPhase::Current || old.identity() == self.identity() {
            return Err(invalid());
        }
        let next_runtime = self.inner.runtime.as_mut().ok_or_else(invalid)?;
        let old_runtime = old.inner.runtime.as_mut().ok_or_else(invalid)?;
        if next_runtime.sampler_sources.is_some() || old_runtime.sampler_sources.is_some() {
            return Err(invalid());
        }
        let transfer = groups::Transfer::new(
            &mut self.inner.groups,
            &mut old.inner.groups,
            &mut self.inner.journal,
            self.inner
                .owned_runtime_bytes
                .as_mut()
                .ok_or_else(invalid)?,
        );
        next_runtime.playback.inherit_candidate_groups(
            &mut old_runtime.playback,
            transfer,
            handoff,
        )?;
        *manifest = Some(AudioCandidateTransfer {
            predecessor: old.identity(),
            successor: self.identity(),
            rows: std::mem::take(&mut self.inner.journal),
        });
        old.inner.phase = ClaimedPhase::Retired;
        *retired = previous.take();
        Ok(())
    }
    /// A successful Drop closes only the actual remaining origin book. Neither
    /// anchor cleanup, catalog release nor original-bank recovery is implied.
    pub fn dispose_on_control(
        &mut self,
    ) -> Result<AudioCandidateRuntimeDisposed, AudioStorageError> {
        if !matches!(
            self.inner.phase,
            ClaimedPhase::Current | ClaimedPhase::Retired
        ) {
            return Err(AudioStorageError::UnknownStorage);
        }
        let runtime = self
            .inner
            .runtime
            .as_ref()
            .ok_or(AudioStorageError::UnknownStorage)?;
        validate_runtime_groups(runtime, &self.inner.groups)?;
        self.inner.phase = ClaimedPhase::Pending;
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(self.inner.runtime.take()))) {
            self.inner.first_panic = Some(payload);
            self.inner.phase = ClaimedPhase::Failed;
            return Err(AudioStorageError::UnknownStorage);
        }
        self.inner.phase = ClaimedPhase::Disposed;
        for group in &mut self.inner.groups {
            group.closed = group.present;
        }
        Ok(AudioCandidateRuntimeDisposed {
            runtime: self.inner.identity,
            groups: std::mem::take(&mut self.inner.groups),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_compensation_swap_closes_displaced_and_adopted_original_origins() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        for incoming in [None, Some(0)] {
            let (mut old, old_anchor) =
                AudioCandidateOwner::test_compensation_origin_owner(Some(3))
                    .split_constructor_on_control()
                    .ok()
                    .unwrap();
            let (mut next, next_anchor) =
                AudioCandidateOwner::test_compensation_origin_owner(incoming)
                    .split_constructor_on_control()
                    .ok()
                    .unwrap();
            let old_id = old.identity();
            let new_id = next.identity();
            let next_runtime = next.inner.runtime.as_mut().unwrap();
            let old_runtime = old.inner.runtime.as_mut().unwrap();
            let mut transfer = groups::Transfer::new(
                &mut next.inner.groups,
                &mut old.inner.groups,
                &mut next.inner.journal,
                next.inner.owned_runtime_bytes.as_mut().unwrap(),
            );
            let (original, displaced) = next_runtime
                .playback
                .test_swap_compensation_origins(&mut old_runtime.playback, &mut transfer)
                .unwrap();
            assert_eq!(displaced.is_some(), incoming.is_some());
            if let Some(displaced) = displaced {
                assert_ne!(original, displaced);
            }
            transfer.commit();
            let manifest = AudioCandidateTransfer {
                predecessor: old_id,
                successor: new_id,
                rows: std::mem::take(&mut next.inner.journal),
            };
            assert_eq!(manifest.moved_groups_from(old_id), 1);
            assert_eq!(
                manifest.moved_groups_from(new_id),
                usize::from(incoming.is_some())
            );
            assert_eq!(manifest.moved_groups(), 1 + usize::from(incoming.is_some()));
            let mut anchors = [Some(old_anchor), Some(next_anchor)];
            old.dispose_on_control()
                .unwrap()
                .record_anchor_cells_on_control(&mut anchors)
                .unwrap();
            assert!(
                anchors[0]
                    .as_mut()
                    .unwrap()
                    .finish_disposal_on_control()
                    .is_err(),
                "adopted original storage still lives in the successor"
            );
            next.dispose_on_control()
                .unwrap()
                .record_anchor_cells_on_control(&mut anchors)
                .unwrap();
            for anchor in anchors.iter_mut().flatten() {
                anchor.finish_disposal_on_control().unwrap();
                assert!(anchor.recover_inputs_on_control().is_some());
            }
        }
    }
    #[test]
    fn split_keeps_actual_runtime_box_and_original_custodian_until_normal_bound_disposal() {
        let Some((custody, token, pointer, capacity)) =
            crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let mut owner = AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        let summary = owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 2048,
                root_rows: 4,
                max_retained_bytes: 64 * 1024 * 1024,
                max_construction_peak_bytes: 64 * 1024 * 1024,
            })
            .unwrap();
        let box_address = owner.inner.runtime.as_deref().unwrap() as *const RuntimeOwner as usize;
        let wrapper_address = owner.inner.as_ref() as *const CandidateState as usize;
        let groups_address = owner.inner.groups.as_ptr() as usize;
        let journal_address = owner.inner.journal.as_ptr() as usize;
        let identity = owner.identity();
        let (mut runtime, mut anchor) = owner.split_constructor_on_control().ok().unwrap();
        assert_eq!(
            summary.member_rows,
            runtime.inner.groups.capacity() + anchor.owner.inner.archive.groups.capacity()
        );
        assert_eq!(summary.transfer_rows, runtime.inner.journal.capacity());
        assert_eq!(runtime.identity(), identity);
        assert!(anchor.matches_current(&runtime));
        assert_eq!(
            anchor.constructor_claims(&runtime).unwrap().summary(),
            Some(summary)
        );
        assert_eq!(
            runtime.inner.runtime.as_deref().unwrap() as *const RuntimeOwner as usize,
            box_address
        );
        assert_eq!(
            anchor.owner.inner.as_ref() as *const CandidateState as usize,
            wrapper_address
        );
        assert_eq!(runtime.inner.groups.as_ptr() as usize, groups_address);
        assert_eq!(runtime.inner.journal.as_ptr() as usize, journal_address);
        assert!(anchor.recover_inputs_on_control().is_none());
        assert!(anchor.finish_disposal_on_control().is_err());
        runtime
            .apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        assert!(runtime.status().unwrap().playing);
        assert!(runtime.set_device_metering(true));
        let mut output = [[0.0; 2]; 8];
        runtime
            .render_with_post_node_observer(&mut output, None)
            .unwrap();
        assert!(anchor.constructor_claims(&runtime).is_none());
        let (runtime, proof) = std::thread::spawn(move || {
            let proof = runtime.dispose_on_control().unwrap();
            (runtime, proof)
        })
        .join()
        .unwrap();
        assert_eq!(runtime.inner.phase, ClaimedPhase::Disposed);
        let mut runtime = runtime;
        assert!(runtime.status().is_none());
        assert!(runtime.session_route_status(0).is_none());
        assert!(!runtime.set_device_metering(true));
        assert!(
            runtime
                .render_with_post_node_observer(&mut [[0.0; 2]; 1], None)
                .is_err()
        );
        assert!(runtime.set_gain_recording_override(1, 1.0).is_err());
        assert_eq!(proof.identity(), identity);
        let mut anchors = [Some(anchor)];
        proof.record_anchor_cells_on_control(&mut anchors).unwrap();
        assert!(proof.record_anchor_cells_on_control(&mut anchors).is_err());
        let anchor = anchors[0].as_mut().unwrap();
        anchor.finish_disposal_on_control().unwrap();
        let bank = anchor.recover_inputs_on_control().unwrap();
        assert_eq!(
            (bank.sources.as_ptr() as usize, bank.sources.capacity()),
            (pointer, capacity)
        );
    }
}
