//! Constructor-owned runtime custody, independent of cost/admission authority.
//! Teardown follows original177 claims_teardown's Pending-before-take protocol.
use crate::{
    AudioPreparationCustody, AudioPreparationPhase, AudioPreparationToken, AudioStorageError,
    OwnedAudioInputs, playback::Playback,
};
use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
};

/// An actual constructor/root/runtime incarnation; no public scalar constructor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCandidateIdentity {
    root: u64,
    incarnation: u64,
    runtime: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[test]
    fn actual_candidate_normal_disposal_recovers_original_bank_without_freeing_its_pcm() {
        for streamed in [false, true] {
            let Some((custody, token, pointer, capacity)) =
                crate::audio_preparation::candidate_test_fixture(streamed)
            else {
                return;
            };
            let mut owner = AudioCandidateOwner::try_new_on_control(custody, token)
                .ok()
                .unwrap();
            let identity = owner.identity();
            let expected_groups = owner.inner.groups.len();
            assert!(expected_groups > 0);
            assert!(owner.inner.runtime_storage_is_unknown());
            assert!(owner.recover_inputs_on_control().is_none());
            let disposed = owner.dispose_on_control().unwrap();
            assert_eq!(disposed.identity(), identity);
            assert_eq!(disposed.closed_owner_groups(), expected_groups);
            assert!(owner.dispose_on_control().is_err());
            let inputs = owner.recover_inputs_on_control().unwrap();
            assert_eq!(inputs.sources.as_ptr() as usize, pointer);
            assert_eq!(inputs.sources.capacity(), capacity);
            assert_eq!(inputs.sources.len(), 1); // retained original owner, not PCM-free proof
        }
    }
    #[test]
    fn actual_nested_destructor_panic_retains_remaining_roots_first_payload_and_never_retries() {
        let Some((custody, token, _, _)) = crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let mut owner = AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let payload_drops = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let payload = payload_drops.clone();
        owner.inner.runtime.as_mut().unwrap().probe =
            Some(RuntimeDropProbe(Some(Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                std::panic::panic_any(Payload(payload));
            }))));
        assert!(owner.dispose_on_control().is_err());
        assert!(owner.inner.runtime.is_none()); // actual Box field destruction was attempted
        assert_eq!(owner.phase(), AudioCandidateDisposalPhase::FailedIncomplete);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(payload_drops.load(Ordering::SeqCst), 0);
        let first =
            owner.inner.first_panic.as_ref().unwrap().as_ref() as *const dyn Any as *const ();
        assert!(owner.dispose_on_control().is_err());
        assert!(owner.recover_inputs_on_control().is_none());
        assert_eq!(
            owner.inner.first_panic.as_ref().unwrap().as_ref() as *const dyn Any as *const (),
            first
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(owner.inner.groups.iter().all(|group| !group.closed));
        assert_eq!(
            owner.inner.custody.phase(),
            AudioPreparationPhase::ArrangementAndSessionSources
        );
        // Explicit test teardown on this control thread; production retains the
        // failed capsule. The payload was never dropped during owner unwinding.
        drop(owner);
        assert_eq!(payload_drops.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn foreign_token_and_changed_owner_book_refuse_without_taking_original_owner() {
        let Some((custody, token, pointer, capacity)) =
            crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let Some((other_custody, other_token, _, _)) =
            crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let refused = AudioCandidateOwner::try_new_on_control(custody, other_token)
            .err()
            .unwrap();
        assert_eq!(refused.reason, AudioStorageError::SourceMismatch);
        let mut owner = AudioCandidateOwner::try_new_on_control(refused.custody, token)
            .ok()
            .unwrap();
        let old = owner.inner.groups[0].location;
        owner.inner.groups[0].location = (u8::MAX, 0);
        assert!(owner.dispose_on_control().is_err());
        assert!(owner.inner.runtime.is_some());
        owner.inner.groups[0].location = old;
        owner.dispose_on_control().unwrap();
        let bank = owner.recover_inputs_on_control().unwrap();
        assert_eq!(bank.sources.as_ptr() as usize, pointer);
        assert_eq!(bank.sources.capacity(), capacity);
        drop(other_custody); // unaccepted test fixture, explicit control teardown
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCandidateDisposalPhase {
    Owned,
    RuntimePending,
    RootsCleaning,
    Complete,
    FailedIncomplete,
}
pub struct AudioCandidateRefusal {
    pub reason: AudioStorageError,
    pub custody: AudioPreparationCustody,
    pub token: AudioPreparationToken,
}
/// Move-only physical destructor-return fact. This proves neither complete
/// storage, source/catalog release, installation nor native quiescence.
pub struct AudioCandidateDisposed {
    identity: AudioCandidateIdentity,
    closed_groups: usize,
}
impl AudioCandidateDisposed {
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.identity
    }
    /// Independently constructor-bound owner locations, not deduplicated DATA
    /// allocations or a claim that retained original PCM has been freed.
    pub fn closed_owner_groups(&self) -> usize {
        self.closed_groups
    }
}
struct Group {
    location: (u8, usize),
    closed: bool,
}
struct RuntimeOwner {
    playback: Playback,
    #[cfg(test)]
    probe: Option<RuntimeDropProbe>,
}
#[cfg(test)]
struct RuntimeDropProbe(Option<Box<dyn FnOnce() + Send>>);
#[cfg(test)]
impl Drop for RuntimeDropProbe {
    fn drop(&mut self) {
        if let Some(probe) = self.0.take() {
            probe();
        }
    }
}
/// No raw Playback, Arc, render, inheritance or mutable-root escape. A host must
/// anchor accepted owners in its persistent lifecycle custody before queueing.
pub struct AudioCandidateOwner {
    // One control-constructed opaque handle keeps callback queue envelopes
    // fixed and small. No public Box/Arc, Deref, Clone or mutable state escape.
    inner: Box<CandidateState>,
}
impl AudioCandidateOwner {
    pub fn try_new_on_control(
        custody: AudioPreparationCustody,
        token: AudioPreparationToken,
    ) -> Result<Self, Box<AudioCandidateRefusal>> {
        CandidateState::try_new_on_control(custody, token).map(|state| Self {
            inner: Box::new(state),
        })
    }
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.inner.identity()
    }
    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    pub fn phase(&self) -> AudioCandidateDisposalPhase {
        self.inner.phase()
    }
    pub fn runtime_storage_is_unknown(&self) -> bool {
        self.inner.runtime_storage_is_unknown()
    }
    pub fn preparation(&self) -> &AudioPreparationCustody {
        self.inner.preparation()
    }
    pub fn streamed_sources(&self) -> impl Iterator<Item = &crate::StreamedSourceMetadata> {
        self.inner.streamed_sources()
    }
    pub fn dispose_on_control(&mut self) -> Result<AudioCandidateDisposed, AudioStorageError> {
        self.inner.dispose_on_control()
    }
    pub fn recover_inputs_on_control(&mut self) -> Option<OwnedAudioInputs> {
        self.inner.recover_inputs_on_control()
    }
    pub fn dispose_original_inputs_on_control(&mut self) -> Result<(), AudioStorageError> {
        self.inner.dispose_original_inputs_on_control()
    }
}
struct CandidateState {
    identity: AudioCandidateIdentity,
    custody: AudioPreparationCustody,
    token: AudioPreparationToken,
    runtime: Option<Box<RuntimeOwner>>,
    groups: Vec<Group>,
    phase: AudioCandidateDisposalPhase,
    first_panic: Option<Box<dyn Any + Send>>,
    receipt_taken: bool,
    sample_rate: u32,
    recovered_inputs: Option<OwnedAudioInputs>,
    inputs_drop_pending: bool,
    inputs_disposed: bool,
}
impl CandidateState {
    /// Refusals return the same preparation custody and original bank. The
    /// temporary root handle and failed runtime are dropped on this control call.
    pub fn try_new_on_control(
        custody: AudioPreparationCustody,
        token: AudioPreparationToken,
    ) -> Result<Self, Box<AudioCandidateRefusal>> {
        let result = (|| {
            let runtime_id = crate::audio_only_inputs::handle()?;
            let (root, incarnation, revision) = custody.candidate_root(&token)?;
            let sample_rate = revision.arrangement().sample_rate();
            let runtime = Playback::new_audio_candidate(revision)?;
            let mut count = 0_usize;
            runtime.visit_candidate_groups(&mut |_, _, present| {
                if present {
                    count = count.checked_add(1).ok_or(AudioStorageError::Overflow)?;
                }
                Ok(())
            })?;
            let mut groups = Vec::new();
            groups
                .try_reserve_exact(count)
                .map_err(|_| AudioStorageError::Allocation)?;
            runtime.validate_candidate_runtime_storage(
                groups
                    .capacity()
                    .checked_mul(std::mem::size_of::<Group>())
                    .and_then(|n| n.checked_add(std::mem::size_of::<RuntimeOwner>()))
                    .and_then(|n| n.checked_add(std::mem::size_of::<CandidateState>()))
                    .ok_or(AudioStorageError::Overflow)?,
            )?;
            runtime.visit_candidate_groups(&mut |kind, index, present| {
                if present {
                    groups.push(Group {
                        location: (kind, index),
                        closed: false,
                    });
                }
                Ok(())
            })?;
            Ok((
                AudioCandidateIdentity {
                    root,
                    incarnation,
                    runtime: runtime_id,
                },
                sample_rate,
                Box::new(RuntimeOwner {
                    playback: runtime,
                    #[cfg(test)]
                    probe: None,
                }),
                groups,
            ))
        })();
        match result {
            Ok((identity, sample_rate, runtime, groups)) => Ok(Self {
                identity,
                custody,
                token,
                runtime: Some(runtime),
                groups,
                phase: AudioCandidateDisposalPhase::Owned,
                first_panic: None,
                receipt_taken: false,
                sample_rate,
                recovered_inputs: None,
                inputs_drop_pending: false,
                inputs_disposed: false,
            }),
            Err(reason) => Err(Box::new(AudioCandidateRefusal {
                reason,
                custody,
                token,
            })),
        }
    }
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.identity
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn phase(&self) -> AudioCandidateDisposalPhase {
        self.phase
    }
    /// The new runtime and its nested owners have no exhaustive DATA declaration
    /// yet. Physical disposal never upgrades PR233's unresolved cost obligations.
    pub fn runtime_storage_is_unknown(&self) -> bool {
        true
    }
    pub fn preparation(&self) -> &AudioPreparationCustody {
        &self.custody
    }
    pub fn streamed_sources(&self) -> impl Iterator<Item = &crate::StreamedSourceMetadata> {
        self.custody.candidate_sources()
    }
    fn validate_groups(&self) -> Result<(), AudioStorageError> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?;
        let mut index = 0;
        runtime
            .playback
            .visit_candidate_groups(&mut |kind, member, present| {
                if present {
                    if self
                        .groups
                        .get(index)
                        .is_none_or(|g| g.location != (kind, member) || g.closed)
                    {
                        return Err(AudioStorageError::SourceMismatch);
                    }
                    index += 1;
                }
                Ok(())
            })?;
        if index != self.groups.len() {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }

    /// Borrow persistent storage outside ordinary locks. A destructor failure is
    /// terminal: retain the first payload and all remaining roots, never retry.
    pub fn dispose_on_control(&mut self) -> Result<AudioCandidateDisposed, AudioStorageError> {
        if self.receipt_taken {
            return Err(AudioStorageError::SourceMismatch);
        }
        if matches!(
            self.phase,
            AudioCandidateDisposalPhase::RuntimePending
                | AudioCandidateDisposalPhase::FailedIncomplete
        ) {
            return Err(AudioStorageError::UnknownStorage);
        }
        if self.phase == AudioCandidateDisposalPhase::Owned {
            self.validate_groups()?;
            if self.runtime.is_none() {
                self.phase = AudioCandidateDisposalPhase::FailedIncomplete;
                return Err(AudioStorageError::UnknownStorage);
            }
            self.phase = AudioCandidateDisposalPhase::RuntimePending;
            let result = catch_unwind(AssertUnwindSafe(|| {
                drop(self.runtime.take());
            }));
            if let Err(payload) = result {
                self.first_panic = Some(payload);
                self.phase = AudioCandidateDisposalPhase::FailedIncomplete;
                return Err(AudioStorageError::UnknownStorage);
            }
            self.phase = AudioCandidateDisposalPhase::RootsCleaning;
            for group in &mut self.groups {
                group.closed = true;
            }
            self.custody.close_on_control();
        }
        if self.phase == AudioCandidateDisposalPhase::RootsCleaning {
            self.custody.cleanup_on_control();
            if self.custody.phase() != AudioPreparationPhase::Recoverable {
                self.phase = AudioCandidateDisposalPhase::FailedIncomplete;
                return Err(AudioStorageError::UnknownStorage);
            }
            self.phase = AudioCandidateDisposalPhase::Complete;
        }
        self.receipt_taken = true;
        Ok(AudioCandidateDisposed {
            identity: self.identity,
            closed_groups: self.groups.iter().filter(|g| g.closed).count(),
        })
    }
    /// Recovery conserves the original bank and its charges. It is separate from
    /// disposal of derived runtime/root aliases, never a claim that PCM was freed.
    pub fn recover_inputs_on_control(&mut self) -> Option<OwnedAudioInputs> {
        if self.phase != AudioCandidateDisposalPhase::Complete
            || self.inputs_drop_pending
            || self.inputs_disposed
        {
            return None;
        }
        self.custody.recover_on_control(&self.token)
    }
    /// Abandoned accepted custody has no caller awaiting bank recovery. Dispose
    /// its original bank on control, with a persistent once-only panic barrier.
    pub fn dispose_original_inputs_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.inputs_disposed {
            return Ok(());
        }
        if self.phase != AudioCandidateDisposalPhase::Complete || self.inputs_drop_pending {
            return Err(AudioStorageError::UnknownStorage);
        }
        if self.recovered_inputs.is_none() {
            self.recovered_inputs = self.custody.recover_on_control(&self.token);
        }
        if self.recovered_inputs.is_none() {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.inputs_drop_pending = true;
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(self.recovered_inputs.take())))
        {
            self.first_panic = Some(payload);
            self.phase = AudioCandidateDisposalPhase::FailedIncomplete;
            return Err(AudioStorageError::UnknownStorage);
        }
        self.inputs_disposed = true;
        Ok(())
    }
}
