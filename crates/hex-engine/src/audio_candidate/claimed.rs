//! Physical runtime/anchor split. It grants no native publication or cost
//! completeness by itself. Hosts must retain exact covering anchors and their
//! own actual catalog/resource custody until every report is consumed.
use super::*;

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
pub(super) fn reserved_state_bytes() -> usize {
    std::mem::size_of::<ClaimedState>()
}
impl AudioCandidateOwner {
    /// All reported refusals precede owner movement. Box allocation follows the
    /// existing control-side fatal-OOM convention. The host must preflight and
    /// charge this wrapper plus all simultaneous anchor/runtime storage first.
    pub fn split_constructor_on_control(
        mut self,
    ) -> Result<
        (ClaimedAudioCandidateRuntime, AudioCandidateAnchors),
        Box<AudioCandidateSplitRefusal>,
    > {
        if self.inner.phase != AudioCandidateDisposalPhase::Owned
            || self.inner.runtime_receipt.is_some()
        {
            return Err(Box::new(AudioCandidateSplitRefusal {
                reason: AudioStorageError::SourceMismatch,
                owner: self,
            }));
        }
        if let Err(reason) = self.inner.validate_groups() {
            return Err(Box::new(AudioCandidateSplitRefusal {
                reason,
                owner: self,
            }));
        }
        if let Err(reason) = self.inner.validate_fresh_origins() {
            return Err(Box::new(AudioCandidateSplitRefusal {
                reason,
                owner: self,
            }));
        }
        let mut state = Box::new(ClaimedState {
            identity: self.identity(),
            runtime: None,
            groups: Vec::new(),
            journal: Vec::new(),
            phase: ClaimedPhase::Current,
            first_panic: None,
            owned_runtime_bytes: self.inner.constructor_data.map(|s| s.owned_runtime_bytes),
        });
        let _actual_wrapper_bytes = state.wrapper_storage_bytes();
        // Only fixed moves after every check/allocation. The actual Runtime Box
        // and the original outer preparation wrapper both keep their addresses.
        state.runtime = self.inner.runtime.take();
        state.groups = std::mem::take(&mut self.inner.groups);
        state.journal = std::mem::take(&mut self.inner.journal);
        self.inner.phase = AudioCandidateDisposalPhase::Leased;
        Ok((
            ClaimedAudioCandidateRuntime { inner: state },
            AudioCandidateAnchors { owner: self },
        ))
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
    #[cfg(test)]
    pub(super) fn test_set_owned_runtime_reservation(&mut self, bytes: usize) -> Option<usize> {
        self.inner.owned_runtime_bytes.replace(bytes)
    }
    #[cfg(test)]
    pub(super) fn test_track_boxes(
        &self,
        id: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        self.inner.runtime.as_ref()?.playback.test_track_boxes(id)
    }
    #[cfg(test)]
    pub(super) fn test_tail_boxes(
        &self,
        id: hex_project::TrackId,
    ) -> Option<(Option<usize>, Option<usize>)> {
        self.inner.runtime.as_ref()?.playback.test_tail_boxes(id)
    }
    #[cfg(test)]
    pub(super) fn test_tail_activity(&self, id: hex_project::TrackId) -> Option<(bool, bool)> {
        self.inner.runtime.as_ref()?.playback.test_tail_activity(id)
    }
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.inner.identity
    }
    pub fn sample_rate(&self) -> Option<u32> {
        self.inner
            .runtime
            .as_ref()
            .map(|r| r.playback.sample_rate())
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
            .playback
            .try_apply_action(action)
    }
    pub fn render(&mut self, output: &mut [crate::Frame]) -> Result<(), crate::Invalid> {
        if self.inner.phase != ClaimedPhase::Current {
            return Err(crate::Invalid("candidate runtime is not current"));
        }
        self.inner
            .runtime
            .as_mut()
            .ok_or(crate::Invalid("candidate runtime unavailable"))?
            .playback
            .render(output)
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
        let transfer = groups::Transfer::new(
            &mut self.inner.groups,
            &mut old.inner.groups,
            &mut self.inner.journal,
            self.inner
                .owned_runtime_bytes
                .as_mut()
                .ok_or_else(invalid)?,
        );
        next_runtime
            .playback
            .inherit_candidate_groups(&mut old_runtime.playback, transfer)?;
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
        runtime.render(&mut [[0.0; 2]; 8]).unwrap();
        assert!(anchor.constructor_claims(&runtime).is_none());
        let (runtime, proof) = std::thread::spawn(move || {
            let proof = runtime.dispose_on_control().unwrap();
            (runtime, proof)
        })
        .join()
        .unwrap();
        assert_eq!(runtime.inner.phase, ClaimedPhase::Disposed);
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
