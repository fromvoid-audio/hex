//! Constructor-paired immutable readers. Root custody, never the last reader,
//! remains responsible for the revision and its descendants' final retirement.
use super::*;
use crate::{AudioPreparedRevisionMetadata, AudioStorageError as Error, owned_shared::OwnedShared};
use std::sync::Mutex;

#[derive(Default)]
pub(super) struct ReaderState {
    readers: usize,
    closing: bool,
    failed: bool,
}

pub(super) type ReaderOwner = OwnedShared<Mutex<ReaderState>>;

/// Correlation of a reader with its actual sealed constructor incarnation.
/// Scalar equality alone grants no storage, cleanup or publication authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCompiledRevisionIdentity {
    pub(crate) root: u64,
    pub(crate) incarnation: u64,
}

/// One registered immutable compiled input. There is no raw-Arc adoption,
/// owning revision getter or automatic Clone. Native mutable Playback remains
/// in its existing candidate/control/reaper custody.
/// ```compile_fail
/// fn adopt(raw: std::sync::Arc<hex_engine::PreparedRevision>) {
///     let _: hex_engine::AudioCompiledRevision = raw.into();
/// }
/// ```
pub struct AudioCompiledRevision {
    identity: AudioCompiledRevisionIdentity,
    revision: Option<OwnedShared<PreparedRevision>>,
    registration: ReaderOwner,
    revision_handle: u64,
    registration_handle: u64,
}

impl AudioCompiledRevision {
    pub(super) fn register(
        revision: &OwnedShared<PreparedRevision>,
        registration: &ReaderOwner,
        identity: AudioCompiledRevisionIdentity,
    ) -> Result<Self, Error> {
        if revision.parts().is_none() || registration.parts().is_none() {
            return Err(Error::UnknownStorage);
        }
        let mut state = registration.lock().map_err(|_| Error::UnknownStorage)?;
        if state.closing || state.failed {
            return Err(Error::SourceMismatch);
        }
        let next = state.readers.checked_add(1).ok_or(Error::Overflow)?;
        // Reserve both observation identities BEFORE either paired clone.
        let revision_handle = crate::audio_only_inputs::handle()?;
        let registration_handle = crate::audio_only_inputs::handle()?;
        let result = Self {
            identity,
            revision: Some(revision.clone_on_control()),
            registration: registration.clone_on_control(),
            revision_handle,
            registration_handle,
        };
        state.readers = next;
        Ok(result)
    }

    /// Reserve another actual registered field before moving it into an export
    /// or compiled record. This cannot reopen a root that started retirement.
    pub fn try_duplicate_on_control(&self) -> Result<Self, Error> {
        Self::register(
            self.revision.as_ref().ok_or(Error::SourceMismatch)?,
            &self.registration,
            self.identity,
        )
    }

    pub fn identity(&self) -> AudioCompiledRevisionIdentity {
        self.identity
    }

    pub fn metadata(&self) -> AudioPreparedRevisionMetadata {
        let revision = self.revision.as_ref().expect("live registered revision");
        AudioPreparedRevisionMetadata {
            sample_rate: revision.arrangement.sample_rate(),
            track_count: revision.arrangement.tracks.len(),
            return_count: revision.arrangement.return_buses.len(),
            contains_streamed_sources: revision.contains_streamed_sources(),
        }
    }

    /// Observe BOTH physical paired handles at their actual retained location.
    /// Root/child declarations and lifetime closure still require the actual
    /// existing preparation custodian; this is not an admission operation.
    pub fn visit_shared_storage_on_control<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), Error> {
        use crate::audio_storage::Owner;
        let revision = self.revision.as_ref().ok_or(Error::UnknownStorage)?;
        ledger.shared(
            revision.arc(),
            revision.parts(),
            Owner::exclusive(revision),
            self.revision_handle,
            None,
        )?;
        ledger.shared(
            self.registration.arc(),
            self.registration.parts(),
            Owner::exclusive(&self.registration),
            self.registration_handle,
            None,
        )
    }
}

impl Drop for AudioCompiledRevision {
    fn drop(&mut self) {
        // This lock is control-only and independent of the revision allocation.
        // Drop its Arc BEFORE releasing the registration, while root retirement
        // is excluded. The root's retained owner prevents a final child drop.
        let mut state = match self.registration.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.failed = true;
                state
            }
        };
        drop(self.revision.take());
        match state.readers.checked_sub(1) {
            Some(next) => state.readers = next,
            None => state.failed = true,
        }
    }
}

/// Must run only AFTER native mutable runtime aliases have retired. Registration
/// is sealed before checking aliases, so no reader can appear after this check.
pub(super) fn retirement_pending(
    revision: &OwnedShared<PreparedRevision>,
    registration: &ReaderOwner,
) -> Result<bool, Error> {
    let mut state = registration.lock().map_err(|_| Error::UnknownStorage)?;
    state.closing = true;
    if state.failed
        || revision.parts().is_none()
        || registration.parts().is_none()
        || Arc::weak_count(revision.arc()) != 0
        || Arc::strong_count(revision.arc())
            != state.readers.checked_add(1).ok_or(Error::Overflow)?
    {
        state.failed = true; // unexplained alias is sticky; never retry adoption
        return Err(Error::UnknownStorage);
    }
    // A dropping reader can still own its registration field after releasing
    // the lock. Retain that small allocation until this final field drop too.
    Ok(state.readers != 0 || Arc::strong_count(registration.arc()) != 1)
}

#[cfg(test)]
mod tests {
    use crate::{AudioPreparationPhase, AudioStorageLedgerReservation, AudioStorageRole};

    fn fixture() -> Option<(crate::AudioPreparationCustody, crate::AudioPreparationToken)> {
        crate::audio_preparation::candidate_test_fixture(false)
            .map(|(custody, token, _, _)| (custody, token))
    }

    #[test]
    fn compiled_reader_holds_actual_root_through_cancel_until_last_registered_release() {
        let Some((mut custody, token)) = fixture() else {
            return;
        };
        let reader = custody.register_compiled_reader_on_control(&token).unwrap();
        let second = reader.try_duplicate_on_control().unwrap();
        let metadata = reader.metadata();
        assert_eq!(metadata.sample_rate, 48_000);
        let before = custody.capacity_report().revision_roots;
        assert!(before.constructed && before.reader_registration_constructed);
        assert!(before.reader_registration_bytes > 0);
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
        assert_eq!(custody.capacity_report().revision_roots, before);
        assert!(reader.try_duplicate_on_control().is_err());
        assert!(custody.recover_on_control(&token).is_none());
        drop(reader);
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
        assert_eq!(second.metadata(), metadata);
        drop(second);
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::Recoverable);
        assert!(custody.recover_on_control(&token).is_some());
        assert!(custody.recover_on_control(&token).is_none());
        custody.cleanup_on_control();
    }

    #[test]
    fn compiled_reader_does_not_qualify_unregistered_revision_alias_or_foreign_token() {
        let Some((mut custody, token)) = fixture() else {
            return;
        };
        let Some((_, other_token)) = fixture() else {
            return;
        };
        assert!(
            custody
                .register_compiled_reader_on_control(&other_token)
                .is_err()
        );
        let (_, _, unexplained) = custody.candidate_root(&token).unwrap();
        let reader = custody.register_compiled_reader_on_control(&token).unwrap();
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
        assert!(custody.recover_on_control(&token).is_none());
        assert!(custody.capacity_report().revision_roots.constructed);
        drop(unexplained);
        drop(reader);
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    }

    #[test]
    fn compiled_reader_inventory_requires_each_actual_registered_handle() {
        let Some((mut custody, token)) = fixture() else {
            return;
        };
        let reader = custody.register_compiled_reader_on_control(&token).unwrap();
        for include in [false, true] {
            let mut ledger =
                crate::AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                    data_rows: 4096,
                    handle_rows: 4096,
                    root_rows: 8,
                    max_retained_bytes: 64 * 1024 * 1024,
                    max_construction_peak_bytes: 64 * 1024 * 1024,
                })
                .unwrap();
            ledger.root(&custody, AudioStorageRole::Current).unwrap();
            custody
                .visit_audio_storage(&mut ledger, AudioStorageRole::Current)
                .unwrap();
            if include {
                ledger.root(&reader, AudioStorageRole::Current).unwrap();
                reader.visit_shared_storage_on_control(&mut ledger).unwrap();
            }
            ledger.summary().unwrap().require_known_storage().unwrap();
            assert_eq!(ledger.validate_closed_shared_handles().is_ok(), include);
        }
        drop(reader);
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert!(custody.recover_on_control(&token).is_some());
    }

    #[test]
    fn compiled_reader_release_races_retirement_without_false_unknown_or_final_child_drop() {
        for _ in 0..32 {
            let Some((mut custody, token)) = fixture() else {
                return;
            };
            let reader = custody.register_compiled_reader_on_control(&token).unwrap();
            let release = std::thread::spawn(move || drop(reader));
            assert!(custody.cancel(&token));
            custody.cleanup_on_control();
            assert!(matches!(
                custody.phase(),
                AudioPreparationPhase::Cleaning | AudioPreparationPhase::Recoverable
            ));
            release.join().unwrap();
            custody.cleanup_on_control();
            assert_eq!(custody.phase(), AudioPreparationPhase::Recoverable);
            assert!(custody.recover_on_control(&token).is_some());
        }
    }

    #[test]
    fn compiled_reader_pending_candidate_disposes_mutable_runtime_once_and_retries_root_only() {
        let Some((custody, token)) = fixture() else {
            return;
        };
        let mut candidate = crate::AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        let reader = candidate.register_compiled_reader_on_control().unwrap();
        let drops = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = drops.clone();
        candidate
            .test_observe_runtime_drop(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .unwrap();
        assert!(matches!(
            candidate.dispose_on_control(),
            Err(crate::AudioStorageError::ExternalReadersPending)
        ));
        assert_eq!(
            candidate.phase(),
            crate::AudioCandidateDisposalPhase::RootsCleaning
        );
        assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(candidate.recover_inputs_on_control().is_none());
        assert!(matches!(
            candidate.dispose_on_control(),
            Err(crate::AudioStorageError::ExternalReadersPending)
        ));
        assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(reader.try_duplicate_on_control().is_err());
        drop(reader);
        candidate.dispose_on_control().unwrap();
        assert_eq!(
            candidate.phase(),
            crate::AudioCandidateDisposalPhase::Complete
        );
        assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(candidate.recover_inputs_on_control().is_some());
        assert!(candidate.recover_inputs_on_control().is_none());
    }
}
