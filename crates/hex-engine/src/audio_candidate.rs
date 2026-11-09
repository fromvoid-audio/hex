//! Constructor-owned runtime custody, separate from cost and publication authority.
//!
//! The actual root, preparation incarnation and runtime identity travel together; caller scalars,
//! equal graph values and raw Playback owners cannot manufacture that association. The constructor
//! admits only its explicitly implemented closed classes, not every device accepted by the
//! ordinary registry. Origin books follow real owner moves; copying compatible DSP history does
//! not transfer source custody. Every destructive frontier records Pending before taking an owner.
//! Only a normal return from the actual destructor closes that frontier; failure retains the first
//! payload and remaining owners without retry or invented completion.
#[cfg(test)]
mod auto_pan_modulation_tests;
mod claimed;
#[cfg(test)]
mod compressor_modulation_tests;
#[cfg(test)]
mod delay_storage_tests;
mod draft;
#[cfg(test)]
mod eq8_storage_tests;
#[cfg(test)]
mod gate_modulation_tests;
#[cfg(test)]
mod grouped_audio_tests;
#[cfg(test)]
mod host_output_modulation_tests;
#[cfg(test)]
mod overdrive_modulation_tests;
#[cfg(test)]
mod overdrive_storage_tests;
mod preparation;
#[cfg(test)]
mod rattle_modulation_tests;
mod reservation;
#[cfg(test)]
mod reverb_tests;
pub use draft::AudioCandidateDraft;
pub use preparation::{AudioCandidateEffects, AudioCandidatePreparation};
pub use reservation::AudioCandidateReservationQuote;
mod storage;
pub use storage::AudioCandidateConstructorSummary;
pub(crate) mod groups;
use crate::{
    AudioPreparationCustody, AudioPreparationPhase, AudioPreparationToken, AudioStorageError,
    OwnedAudioInputs, playback::Playback,
};
pub use claimed::{
    AudioCandidateAnchors, AudioCandidateConstructorClaims, AudioCandidateRuntimeStatus,
    AudioCandidateSplitRefusal, ClaimedAudioCandidateRuntime, StreamPlayback,
};
pub use groups::{AudioCandidateRuntimeDisposed, AudioCandidateTransfer};
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
    fn dormant_constructor_handles_and_vacant_locations_survive_supported_play_pause_stop() {
        let Some((custody, token, pointer, capacity)) =
            crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let mut owner = AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        assert_eq!(owner.foreign_nodes_on_control().unwrap().count(), 0);
        let locations: std::collections::BTreeSet<_> = owner
            .inner
            .groups
            .iter()
            .map(|group| group.location)
            .collect();
        assert_eq!(locations.len(), owner.inner.groups.len());
        assert_eq!(locations.iter().filter(|(kind, _)| *kind == 9).count(), 1);
        let count = owner.inner.groups.iter().filter(|g| g.present).count();
        assert!(owner.inner.groups.iter().any(|g| !g.present));
        owner
            .test_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        owner.test_render(&mut [[0.0; 2]; 3]).unwrap();
        for action in [crate::PlaybackCommand::Pause, crate::PlaybackCommand::Stop] {
            owner
                .test_apply_action(crate::PlaybackAction::Transport(action))
                .unwrap();
            owner.test_render(&mut [[1.0; 2]; 2]).unwrap();
            owner.inner.validate_groups().unwrap();
        }
        let receipt = owner.dispose_on_control().unwrap();
        assert_eq!(receipt.closed_owner_groups(), count);
        assert!(
            owner
                .inner
                .groups
                .iter()
                .filter(|g| !g.present)
                .all(|g| !g.closed)
        );
        let bank = owner.recover_inputs_on_control().unwrap();
        assert_eq!(
            (bank.sources.as_ptr() as usize, bank.sources.capacity()),
            (pointer, capacity)
        );
    }
    #[test]
    fn actual_unsupported_tail_population_pins_original_runtime_without_false_group_closure() {
        let Some((custody, token, _, _)) = crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let mut owner = AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        owner
            .inner
            .runtime
            .as_mut()
            .unwrap()
            .playback
            .test_populate_unsupported_tail();
        assert_eq!(
            owner.dispose_on_control().err(),
            Some(AudioStorageError::UnsupportedGraph)
        );
        assert_eq!(owner.phase(), AudioCandidateDisposalPhase::FailedIncomplete);
        assert!(owner.inner.runtime.is_some());
        assert!(owner.inner.groups.iter().all(|g| !g.closed));
        assert!(owner.recover_inputs_on_control().is_none());
        assert!(owner.dispose_on_control().is_err());
        // Explicit control-side fixture teardown; no disposal receipt was issued.
    }
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
            let expected_groups = owner.inner.groups.iter().filter(|g| g.present).count();
            assert!(expected_groups > 0);
            assert!(owner.inner.runtime_storage_is_unknown());
            assert!(owner.recover_inputs_on_control().is_none());
            let original_runtime =
                owner.inner.runtime.as_deref().unwrap() as *const RuntimeOwner as usize;
            let observed = Arc::new(std::sync::Mutex::new(None));
            let record = observed.clone();
            owner.inner.runtime.as_mut().unwrap().probe =
                Some(RuntimeDropProbe(Some(Box::new(move || {
                    *record.lock().unwrap() = Some((original_runtime, std::thread::current().id()));
                }))));
            let caller = std::thread::current().id();
            let (mut owner, disposed, destructor_thread) = std::thread::spawn(move || {
                let disposed = owner.dispose_on_control().unwrap();
                (owner, disposed, std::thread::current().id())
            })
            .join()
            .unwrap();
            assert_ne!(caller, destructor_thread);
            assert_eq!(
                *observed.lock().unwrap(),
                Some((original_runtime, destructor_thread))
            );
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
    /// The actual runtime is exclusively leased; preparation remains in anchors.
    Leased,
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
    present: bool,
    closed: bool,
    origin: Option<groups::Origin>,
}
mod current_sampler;
struct RuntimeOwner {
    playback: Playback,
    sampler_sources: Option<(
        crate::owned_shared::OwnedShared<
            crate::native_device_resources::current_sampler::CurrentSamplerSources,
        >,
        u64,
    )>,
    #[cfg(any(test, feature = "instrument-test-support"))]
    probe: Option<RuntimeDropProbe>,
}
#[cfg(any(test, feature = "instrument-test-support"))]
struct RuntimeDropProbe(Option<Box<dyn FnOnce() + Send>>);
#[cfg(any(test, feature = "instrument-test-support"))]
impl Drop for RuntimeDropProbe {
    fn drop(&mut self) {
        if let Some(probe) = self.0.take() {
            probe();
        }
    }
}
/// Default builds have no raw Playback, Arc, render, inheritance or mutable-root
/// escape. Non-default headless support exposes only bounded actions/render and
/// scalar transport. Accepted owners must remain in persistent host custody.
pub struct AudioCandidateOwner {
    // One control-constructed opaque handle keeps callback queue envelopes
    // fixed and small. No public Box/Arc, Deref, Clone or mutable state escape.
    inner: Box<CandidateState>,
}
impl AudioCandidateOwner {
    /// Read-only scalar identity for actual opaque runtime custody probes.
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_runtime_address(&self) -> Option<usize> {
        self.inner
            .runtime
            .as_deref()
            .map(|r| r as *const RuntimeOwner as usize)
    }
    /// Control-only headless observer on the actual nested Runtime Box Drop.
    /// Returns a scalar address only; no pointer, Playback or owning handle escapes.
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_observe_runtime_drop(
        &mut self,
        observer: impl FnOnce(usize) + Send + 'static,
    ) -> Option<usize> {
        if self.inner.phase != AudioCandidateDisposalPhase::Owned {
            return None;
        }
        let runtime = self.inner.runtime.as_mut()?;
        if runtime.probe.is_some() {
            return None;
        }
        let address = runtime.as_ref() as *const RuntimeOwner as usize;
        runtime.probe = Some(RuntimeDropProbe(Some(Box::new(move || observer(address)))));
        Some(address)
    }
    /// Narrow non-default headless test support; never yields a runtime/root.
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_apply_action(
        &mut self,
        action: crate::PlaybackAction,
    ) -> Result<(), crate::Invalid> {
        let runtime = self
            .inner
            .runtime
            .as_mut()
            .ok_or(crate::Invalid("candidate runtime unavailable"))?;
        runtime
            .apply_action(action)
            .map_err(|_| crate::Invalid("candidate action refused"))
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_render(&mut self, output: &mut [crate::Frame]) -> Result<(), crate::Invalid> {
        let runtime = self
            .inner
            .runtime
            .as_mut()
            .ok_or(crate::Invalid("candidate runtime unavailable"))?;
        runtime.render_with_signal_observers(output, None, None)
    }
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_transport(&self) -> Option<(u64, u64, bool)> {
        self.inner.runtime.as_ref().map(|runtime| {
            (
                runtime.playback.position(),
                runtime.playback.performance_frame(),
                runtime.playback.playing(),
            )
        })
    }
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
    pub fn source_context(&self) -> Option<crate::AudioSourceContext> {
        self.inner.custody.source_context()
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
    /// Control-only normal runtime destruction. The returned book does not
    /// release prepared roots/bank; every origin archive must ingest it first.
    pub fn dispose_runtime_on_control(
        &mut self,
    ) -> Result<AudioCandidateRuntimeDisposed, AudioStorageError> {
        self.inner.dispose_runtime_on_control()
    }
    pub fn finish_disposal_on_control(
        &mut self,
    ) -> Result<AudioCandidateDisposed, AudioStorageError> {
        self.inner.finish_disposal_on_control()
    }
    pub fn recover_inputs_on_control(&mut self) -> Option<OwnedAudioInputs> {
        self.inner.recover_inputs_on_control()
    }
    #[cfg(test)]
    pub(crate) fn test_render_with_foreign(
        &mut self,
        output: &mut [crate::Frame],
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), crate::Invalid> {
        self.inner
            .runtime
            .as_mut()
            .ok_or(crate::Invalid("candidate runtime unavailable"))?
            .render_with_foreign_processors(output, None, None, None, foreign)
    }
    /// Borrow foreign nodes from the original prepared root on the control lane.
    /// Configuration, processor identity and foreign binding remain separate
    /// fields of the actual prepared instance; no caller summary is accepted.
    /// This read-only view neither transfers native resources nor proves source
    /// admission. The audio owner must match every node to its retained source
    /// and keep that source alive through the original graph's terminal custody.
    /// Bypassed nodes are included because their endpoints still advance.
    pub fn foreign_nodes_on_control(
        &self,
    ) -> Result<impl Iterator<Item = &hex_lattice::PreparedRuntimeInstance>, AudioStorageError>
    {
        let revision = self.inner.custody.candidate_root_view(&self.inner.token)?;
        let arrangement = revision.arrangement();
        Ok(arrangement
            .tracks
            .iter()
            .flat_map(|track| track.device_chain.iter())
            .chain(
                arrangement
                    .return_buses
                    .iter()
                    .flat_map(|bus| bus.device_chain.iter()),
            )
            .chain(arrangement.main_device_chain.iter())
            .filter(|node| {
                matches!(
                    node.definition,
                    Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
                )
            }))
    }
    /// Query the original closed constructor, never a caller-provided summary.
    /// Source/endpoint admission remains the native audio owner's responsibility.
    pub fn requires_effect_graph_custody_on_control(&self) -> Result<bool, AudioStorageError> {
        let revision = self.inner.custody.candidate_root_view(&self.inner.token)?;
        Ok(!revision.arrangement().main_device_chain.is_empty()
            || revision
                .arrangement()
                .return_buses
                .iter()
                .any(|r| !r.device_chain.is_empty())
            || revision
                .arrangement()
                .signal_graph
                .as_ref()
                .is_some_and(|g| g.needs_runtime())
            || self.foreign_nodes_on_control()?.next().is_some())
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
    journal: Vec<groups::TransferRow>,
    archive: groups::Archive,
    runtime_receipt: Option<AudioCandidateRuntimeDisposed>,
    phase: AudioCandidateDisposalPhase,
    first_panic: Option<Box<dyn Any + Send>>,
    receipt_taken: bool,
    sample_rate: u32,
    recovered_inputs: Option<OwnedAudioInputs>,
    inputs_drop_pending: bool,
    inputs_disposed: bool,
    constructor_data: Option<AudioCandidateConstructorSummary>,
}
impl CandidateState {
    /// Refusals return the same preparation custody and original bank. The
    /// temporary root handle and failed runtime are dropped on this control call.
    pub fn try_new_on_control(
        custody: AudioPreparationCustody,
        token: AudioPreparationToken,
    ) -> Result<Self, Box<AudioCandidateRefusal>> {
        let mut draft = AudioCandidateDraft::new_on_control();
        // Preserve the existing consuming compatibility contract. The new Root
        // preparation route instead calls the borrowed, persistent draft API.
        match draft.build_on_control(&custody, &token, 0, || Ok(())) {
            Ok(()) => {
                let Some(archive) = draft.archive.take() else {
                    return Err(Box::new(AudioCandidateRefusal {
                        reason: AudioStorageError::SourceMismatch,
                        custody,
                        token,
                    }));
                };
                let Some(identity) = draft.identity else {
                    return Err(Box::new(AudioCandidateRefusal {
                        reason: AudioStorageError::SourceMismatch,
                        custody,
                        token,
                    }));
                };
                Ok(Self {
                    identity,
                    custody,
                    token,
                    runtime: draft.runtime.take(),
                    groups: std::mem::take(&mut draft.groups),
                    journal: std::mem::take(&mut draft.journal),
                    archive,
                    runtime_receipt: None,
                    phase: AudioCandidateDisposalPhase::Owned,
                    first_panic: None,
                    receipt_taken: false,
                    sample_rate: draft.sample_rate,
                    recovered_inputs: None,
                    inputs_drop_pending: false,
                    inputs_disposed: false,
                    constructor_data: None,
                })
            }
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
        validate_runtime_groups(runtime, &self.groups)
    }

    /// Borrow persistent storage outside ordinary locks. A destructor failure is
    /// terminal: retain the first payload and all remaining roots, never retry.
    fn dispose_runtime_on_control(
        &mut self,
    ) -> Result<AudioCandidateRuntimeDisposed, AudioStorageError> {
        if self.phase != AudioCandidateDisposalPhase::Owned || self.receipt_taken {
            return Err(AudioStorageError::UnknownStorage);
        }
        if let Err(error) = self.validate_groups() {
            if error == AudioStorageError::UnsupportedGraph {
                self.phase = AudioCandidateDisposalPhase::FailedIncomplete;
            }
            return Err(error);
        }
        if self.runtime.is_none() {
            self.phase = AudioCandidateDisposalPhase::FailedIncomplete;
            return Err(AudioStorageError::UnknownStorage);
        }
        self.phase = AudioCandidateDisposalPhase::RuntimePending;
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(self.runtime.take()))) {
            self.first_panic = Some(payload);
            self.phase = AudioCandidateDisposalPhase::FailedIncomplete;
            return Err(AudioStorageError::UnknownStorage);
        }
        self.phase = AudioCandidateDisposalPhase::RootsCleaning;
        for group in &mut self.groups {
            group.closed = group.present;
        }
        Ok(AudioCandidateRuntimeDisposed {
            runtime: self.identity,
            groups: std::mem::take(&mut self.groups),
        })
    }
    pub fn dispose_on_control(&mut self) -> Result<AudioCandidateDisposed, AudioStorageError> {
        if self.receipt_taken {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.phase == AudioCandidateDisposalPhase::Owned {
            self.runtime_receipt = Some(self.dispose_runtime_on_control()?);
        }
        if let Some(proof) = &self.runtime_receipt {
            // Compatibility disposal is valid only for an entirely local book.
            // Foreign origins need the actual host's atomic all-archive fanout.
            if proof
                .groups
                .iter()
                .any(|g| g.origin.is_some_and(|o| o.runtime != self.identity))
            {
                return Err(AudioStorageError::UnknownStorage);
            }
            if !self.archive.original_disposed {
                self.archive.preflight(proof)?;
                self.archive.commit(proof);
            }
        }
        self.finish_disposal_on_control()
    }
    fn finish_disposal_on_control(&mut self) -> Result<AudioCandidateDisposed, AudioStorageError> {
        if self.receipt_taken {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.phase != AudioCandidateDisposalPhase::RootsCleaning || !self.archive.all_closed() {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.custody.close_on_control();
        self.custody.cleanup_on_control();
        if self.custody.phase() != AudioPreparationPhase::Recoverable {
            self.phase = AudioCandidateDisposalPhase::FailedIncomplete;
            return Err(AudioStorageError::UnknownStorage);
        }
        self.phase = AudioCandidateDisposalPhase::Complete;
        self.receipt_taken = true;
        Ok(AudioCandidateDisposed {
            identity: self.identity,
            closed_groups: self.archive.groups.len(),
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

fn validate_runtime_groups(
    runtime: &RuntimeOwner,
    groups: &[Group],
) -> Result<(), AudioStorageError> {
    let mut index = 0;
    runtime
        .playback
        .visit_candidate_groups(&mut |kind, member, present| {
            validate_supported_location(kind, present && !matches!(kind, 1 | 5))?;
            if matches!(kind, 1 | 5)
                && present
                && groups.get(index).is_none_or(|g| g.origin.is_none())
            {
                return Err(AudioStorageError::UnsupportedGraph);
            }
            if groups.get(index).is_none_or(|g| {
                g.location != (kind, member)
                    || g.present != present
                    || g.closed
                    || (matches!(kind, 1 | 5) && present && g.origin.is_none())
            }) {
                return Err(AudioStorageError::SourceMismatch);
            }
            index += 1;
            Ok(())
        })?;
    if index != groups.len() {
        return Err(AudioStorageError::SourceMismatch);
    }
    Ok(())
}

// Explicit closed constructor classes permit known fresh chains, sampler and
// compensation storage. Fresh tails and analytic-context fanout remain
// unsupported; retained chain tails require original transfer origins.
// Enumeration includes
// every vacant slot; absence is validated, never converted into an owner group.
fn validate_supported_location(kind: u8, present: bool) -> Result<(), AudioStorageError> {
    match kind {
        0 | 2 | 4 | 7 | 8 | 9..=12 | 14 => Ok(()),
        1 | 3 | 5 | 6 | 13 if !present => Ok(()),
        1 | 3 | 5 | 6 | 13 => Err(AudioStorageError::UnsupportedGraph),
        _ => Err(AudioStorageError::SourceMismatch),
    }
}
