//! A host's persistent, unpublished construction destination. No Drop cleanup
//! and no owner getter: cancellation must retain this cell until explicit normal
//! cleanup, while a failed cell remains quarantined by its host.
use super::*;
use hex_project::{AssetId, Project};

pub struct AudioCandidatePreparation {
    sources: Vec<(AssetId, crate::OwnedAudioSource)>,
    resident_draft: Vec<crate::Frame>,
    reservation: crate::AudioStorageReservation,
    bank: Option<OwnedAudioInputs>,
    custody: Option<AudioPreparationCustody>,
    token: Option<AudioPreparationToken>,
    draft: Option<AudioCandidateDraft>,
    owner: Option<AudioCandidateOwner>,
    owner_runtime_done: bool,
    cleaning: bool,
    complete: bool,
    failed: bool,
    first_panic: Option<Box<dyn Any + Send>>,
}
impl AudioCandidatePreparation {
    /// The host reserves the actual runtime slot and this full construction
    /// allowance before calling. No source or graph owner is consumed here.
    pub fn try_new_on_control(
        source_capacity: usize,
        reservation: crate::AudioStorageReservation,
    ) -> Result<Self, AudioStorageError> {
        let requested = source_capacity
            .checked_mul(std::mem::size_of::<(AssetId, crate::OwnedAudioSource)>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
            .and_then(|n| {
                n.checked_add(
                    reservation
                        .handle_obligations
                        .checked_mul(std::mem::size_of::<crate::audio_only_inputs::AliasRow>())?,
                )
            })
            .and_then(|n| n.checked_add(reservation.caller_retained_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        if source_capacity > reservation.handle_obligations
            || requested > reservation.max_prepared_data_bytes
            || requested
                .checked_add(reservation.caller_construction_bytes)
                .is_none_or(|n| n > reservation.max_construction_data_bytes)
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        let mut result = Self {
            sources: Vec::new(),
            resident_draft: Vec::new(),
            reservation,
            bank: None,
            custody: Some(AudioPreparationCustody::try_new_on_control()?),
            token: None,
            draft: Some(AudioCandidateDraft::new_on_control()),
            owner: None,
            owner_runtime_done: false,
            cleaning: false,
            complete: false,
            failed: false,
            first_panic: None,
        };
        result
            .sources
            .try_reserve_exact(source_capacity)
            .map_err(|_| AudioStorageError::Allocation)?;
        result.validate_collection_bytes(0)?;
        Ok(result)
    }
    fn collecting(&self, asset: AssetId) -> Result<(), AudioStorageError> {
        if self.failed
            || self.cleaning
            || self.complete
            || self.bank.is_some()
            || self.owner.is_some()
            || self.sources.len() == self.sources.capacity()
            || self.resident_draft.capacity() != 0
        {
            return Err(AudioStorageError::Capacity);
        }
        if self.sources.iter().any(|(id, _)| *id == asset) {
            return Err(AudioStorageError::DuplicateAsset);
        }
        Ok(())
    }
    fn validate_collection_bytes(&self, additional: usize) -> Result<(), AudioStorageError> {
        let data = self.sources.iter().try_fold(0usize, |n, (_, source)| {
            let (payload, overhead) = crate::audio_input_storage::parts(source)?;
            n.checked_add(payload)
                .and_then(|n| n.checked_add(overhead))
                .ok_or(AudioStorageError::Overflow)
        })?;
        let total = self
            .sources
            .capacity()
            .checked_mul(std::mem::size_of::<(AssetId, crate::OwnedAudioSource)>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
            .and_then(|n| {
                n.checked_add(
                    self.reservation
                        .handle_obligations
                        .checked_mul(std::mem::size_of::<crate::audio_only_inputs::AliasRow>())?,
                )
            })
            .and_then(|n| {
                n.checked_add(
                    self.resident_draft
                        .capacity()
                        .checked_mul(std::mem::size_of::<crate::Frame>())?,
                )
            })
            .and_then(|n| n.checked_add(data))
            .and_then(|n| n.checked_add(additional))
            .and_then(|n| n.checked_add(self.reservation.caller_retained_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        let peak = total
            .checked_add(self.reservation.caller_construction_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        if total > self.reservation.max_prepared_data_bytes
            || peak > self.reservation.max_construction_data_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        Ok(())
    }
    /// Copies a control-side resident value into a fresh constructor allocation;
    /// the caller's legacy Audio Arc is never adopted as native custody.
    pub fn copy_resident_source_on_control(
        &mut self,
        asset: AssetId,
        audio: &crate::Audio,
    ) -> Result<(), AudioStorageError> {
        self.collecting(asset)?;
        if !self.resident_draft.is_empty() || self.resident_draft.capacity() != 0 {
            return Err(AudioStorageError::SourceMismatch);
        }
        let requested = audio
            .frames()
            .len()
            .checked_mul(std::mem::size_of::<crate::Frame>())
            .and_then(|n| n.checked_add(std::mem::size_of::<crate::AudioData>()))
            .ok_or(AudioStorageError::Overflow)?;
        self.validate_collection_bytes(requested)?;
        self.resident_draft
            .try_reserve_exact(audio.frames().len())
            .map_err(|_| AudioStorageError::Allocation)?;
        self.validate_collection_bytes(std::mem::size_of::<crate::AudioData>())?;
        self.resident_draft.extend_from_slice(audio.frames());
        // This fixed constructor has no user code or recoverable unwind after
        // taking the Vec; all Result refusals return that exact Vec. Fatal OOM
        // follows the existing constructor convention.
        match crate::OwnedAudioSource::try_resident(
            audio.sample_rate(),
            std::mem::take(&mut self.resident_draft),
        ) {
            Ok(source) => {
                self.sources.push((asset, source));
                Ok(())
            }
            Err(refused) => {
                self.resident_draft = refused.frames;
                Err(refused.reason)
            }
        }
    }
    pub fn add_streamed_source_on_control(
        &mut self,
        metadata: crate::StreamedSourceMetadata,
    ) -> Result<(), AudioStorageError> {
        let asset = metadata.key().asset();
        self.collecting(asset)?;
        self.validate_collection_bytes(std::mem::size_of::<crate::StreamedSourceMetadata>())?;
        let source =
            crate::OwnedAudioSource::try_streamed(metadata).map_err(|refused| refused.reason)?;
        self.sources.push((asset, source));
        Ok(())
    }
    /// Low-level exact Project/context bridge. Session snapshots call this with
    /// their paired canonical Project and counters; numbers grant no custody.
    pub fn prepare_on_control(
        &mut self,
        project: &Project,
        source_generation: u64,
        context: crate::AudioSourceContext,
    ) -> Result<(), AudioStorageError> {
        if self.failed
            || self.cleaning
            || self.complete
            || self.owner.is_some()
            || self.resident_draft.capacity() != 0
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.bank.is_none() && self.token.is_none() {
            OwnedAudioInputs::try_new_in_place(
                &mut self.sources,
                self.reservation,
                &mut self.bank,
            )?;
        }
        let custody = self
            .custody
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        if self.token.is_none() {
            self.token = Some(
                custody
                    .begin_prepared_revision_in_place_on_control(
                        project,
                        source_generation,
                        context,
                        &mut self.bank,
                    )
                    .map_err(|refused| refused.reason)?,
            );
        }
        let draft = self
            .draft
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        draft.prepare_on_control(
            custody,
            self.token
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?,
        )?;
        draft.finish_on_control(&mut self.custody, &mut self.token, &mut self.owner)
    }
    /// Scalar test observation of the actual rooted collection backing; never
    /// an owner, source accessor or publication capability.
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_source_collection_identity(&self) -> (usize, usize) {
        (self.sources.as_ptr() as usize, self.sources.capacity())
    }
    pub(super) fn owner_ref_on_control(&self) -> Option<&AudioCandidateOwner> {
        self.owner.as_ref()
    }
    pub(super) fn take_preflighted_owner(&mut self) -> Option<AudioCandidateOwner> {
        self.owner.take()
    }
    pub fn owner_on_control(&mut self) -> Option<&mut AudioCandidateOwner> {
        self.owner.as_mut()
    }
    pub fn failed_incomplete(&self) -> bool {
        self.failed
            || self
                .draft
                .as_ref()
                .is_some_and(AudioCandidateDraft::failed_incomplete)
            || self
                .custody
                .as_ref()
                .is_some_and(|c| c.phase() == AudioPreparationPhase::FailedIncomplete)
            || self
                .owner
                .as_ref()
                .is_some_and(|o| o.phase() == AudioCandidateDisposalPhase::FailedIncomplete)
    }
    /// Explicit cleanup of unpublished work; no NeverInstalled/Returned receipt
    /// exists because this owner has not crossed a publication queue boundary.
    /// Includes the emptied draft/custody wrappers and original source backing.
    pub fn cleanup_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.complete {
            return Ok(());
        }
        if self.failed_incomplete() || self.cleaning {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.cleaning = true;
        match catch_unwind(AssertUnwindSafe(|| {
            if let Some(owner) = self.owner.as_mut() {
                if !self.owner_runtime_done {
                    owner.dispose_on_control()?;
                    self.owner_runtime_done = true;
                }
                owner.dispose_original_inputs_on_control()?;
            }
            drop(self.owner.take());
            if let Some(draft) = self.draft.as_mut() {
                draft.dispose_on_control()?;
            }
            drop(self.draft.take());
            if let Some(custody) = self.custody.as_mut() {
                custody.close_on_control();
                custody.cleanup_on_control();
                if custody.phase() == AudioPreparationPhase::FailedIncomplete {
                    return Err(AudioStorageError::UnknownStorage);
                }
                if let Some(token) = self.token.as_ref() {
                    if self.bank.is_some() {
                        return Err(AudioStorageError::SourceMismatch);
                    }
                    self.bank = Some(
                        custody
                            .recover_on_control(token)
                            .ok_or(AudioStorageError::SourceMismatch)?,
                    );
                } else if custody.phase() != AudioPreparationPhase::Vacant {
                    return Err(AudioStorageError::SourceMismatch);
                }
            }
            drop(self.custody.take());
            self.token = None;
            drop(self.bank.take());
            drop(std::mem::take(&mut self.sources));
            drop(std::mem::take(&mut self.resident_draft));
            Ok(())
        })) {
            Ok(Ok(())) => {
                self.cleaning = false;
                self.complete = true;
                Ok(())
            }
            Ok(Err(reason)) => {
                self.failed = true;
                Err(reason)
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.failed = true;
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    fn reservation() -> crate::AudioStorageReservation {
        crate::AudioStorageReservation {
            allocation_items: 256,
            handle_obligations: 4096,
            max_prepared_data_bytes: 64 * 1024 * 1024,
            max_construction_data_bytes: 64 * 1024 * 1024,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        }
    }
    fn collected() -> Option<(Project, AudioCandidatePreparation)> {
        if !crate::audio_storage_constructor_layout_supported() {
            return None;
        }
        let project = crate::audio_preparation::candidate_test_project();
        let asset = project.assets().next().unwrap().id();
        let audio = crate::Audio::new(48000, vec![[0.25; 2]; 8]).unwrap();
        let original = audio.frames().as_ptr();
        let mut cell = AudioCandidatePreparation::try_new_on_control(1, reservation()).unwrap();
        cell.copy_resident_source_on_control(asset, &audio).unwrap();
        let crate::audio_only_inputs::SourceKind::Resident(copy) = &cell.sources[0].1.kind else {
            panic!("resident");
        };
        assert_ne!(copy.frames().as_ptr(), original);
        assert_eq!(copy.frames(), audio.frames());
        Some((project, cell))
    }
    #[test]
    fn persistent_collection_refusal_retry_and_original_owner_cleanup_are_control_side() {
        let Some((project, mut cell)) = collected() else {
            return;
        };
        let pointer = cell.sources.as_ptr();
        let capacity = cell.sources.capacity();
        let context = crate::AudioSourceContext {
            project_generation: 2,
            editor_revision: 8,
        };
        assert_eq!(
            cell.prepare_on_control(&project, 0, context),
            Err(AudioStorageError::SourceMismatch)
        );
        assert_eq!(
            (
                cell.bank.as_ref().unwrap().sources.as_ptr(),
                cell.bank.as_ref().unwrap().sources.capacity()
            ),
            (pointer, capacity)
        );
        cell.prepare_on_control(&project, 11, context).unwrap();
        assert!(cell.bank.is_none() && cell.custody.is_none() && cell.token.is_none());
        let observed = Arc::new(std::sync::Mutex::new(None));
        let result = observed.clone();
        let owner = cell.owner.as_mut().unwrap();
        let runtime = owner.inner.runtime.as_deref().unwrap() as *const RuntimeOwner as usize;
        owner.inner.runtime.as_mut().unwrap().probe =
            Some(RuntimeDropProbe(Some(Box::new(move || {
                *result.lock().unwrap() = Some((runtime, std::thread::current().id()));
            }))));
        let caller = std::thread::current().id();
        let cell = std::thread::spawn(move || {
            cell.cleanup_on_control().unwrap();
            cell.cleanup_on_control().unwrap();
            cell
        })
        .join()
        .unwrap();
        let (_, cleanup_thread) = observed.lock().unwrap().unwrap();
        assert_ne!(cleanup_thread, caller);
        assert_eq!(*observed.lock().unwrap(), Some((runtime, cleanup_thread)));
        assert!(
            cell.complete && cell.owner.is_none() && cell.custody.is_none() && cell.draft.is_none()
        );
        assert_eq!(cell.sources.capacity(), 0);
        assert!(!cell.failed_incomplete());
    }
    #[test]
    fn collection_and_completed_owner_cancel_clean_but_failed_runtime_is_retained_once() {
        let Some((_, mut partial)) = collected() else {
            return;
        };
        partial.cleanup_on_control().unwrap();
        assert!(partial.complete && partial.custody.is_none() && partial.draft.is_none());
        let Some((project, mut cell)) = collected() else {
            return;
        };
        cell.prepare_on_control(
            &project,
            11,
            crate::AudioSourceContext {
                project_generation: 0,
                editor_revision: 0,
            },
        )
        .unwrap();
        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let payload = drops.clone();
        cell.owner
            .as_mut()
            .unwrap()
            .inner
            .runtime
            .as_mut()
            .unwrap()
            .probe = Some(RuntimeDropProbe(Some(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            std::panic::panic_any(Payload(payload));
        }))));
        assert!(cell.cleanup_on_control().is_err());
        assert!(cell.owner.is_some() && cell.draft.is_some() && cell.failed_incomplete());
        assert_eq!(
            (calls.load(Ordering::SeqCst), drops.load(Ordering::SeqCst)),
            (1, 0)
        );
        assert!(cell.cleanup_on_control().is_err());
        assert_eq!(
            (calls.load(Ordering::SeqCst), drops.load(Ordering::SeqCst)),
            (1, 0)
        );
        drop(cell); // Explicit fixture teardown; failed production cells stay rooted.
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
