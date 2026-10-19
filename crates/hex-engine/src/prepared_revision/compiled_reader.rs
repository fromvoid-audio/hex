//! Constructor-paired immutable readers. Root custody, never the last reader,
//! remains responsible for the revision and its descendants' final retirement.
use super::*;
use crate::{AudioPreparedRevisionMetadata, AudioStorageError as Error, owned_shared::OwnedShared};
use std::sync::atomic::{AtomicUsize, Ordering};

const CLOSED: usize = 1;
const FAILED: usize = 2;
const FLAGS: usize = CLOSED | FAILED;
const READER: usize = 4;

/// One inline atomic word: no pthread lock, parking state or hidden child.
#[derive(Default)]
pub(super) struct ReaderState {
    word: AtomicUsize,
}
impl ReaderState {
    pub(super) const fn supported() -> bool {
        cfg!(target_has_atomic = "ptr")
    }
    pub(super) fn declared_bytes() -> Result<usize, Error> {
        if !Self::supported() {
            return Err(Error::UnsupportedConstructor);
        }
        let (payload, header) =
            ReaderOwner::declared_parts().ok_or(Error::UnsupportedConstructor)?;
        payload.checked_add(header).ok_or(Error::Overflow)
    }
    pub(super) fn construct_on_control(
        destination: &mut Option<ReaderOwner>,
        max_retained: usize,
        max_peak: usize,
    ) -> Result<(), Error> {
        if destination.is_some() {
            return Err(Error::SourceMismatch);
        }
        let bytes = Self::declared_bytes()?;
        if bytes > max_retained || bytes > max_peak {
            return Err(Error::ReservationExceeded);
        }
        *destination = Some(OwnedShared::new_owned(Self::default()));
        Ok(())
    }
    fn fail(&self) {
        self.word.fetch_or(CLOSED | FAILED, Ordering::AcqRel);
    }
    #[cfg(test)]
    pub(super) fn observed_word_for_test(&self) -> usize {
        self.word.load(Ordering::Acquire)
    }
    fn release(&self) {
        // A sealed handle decrements exactly once. The preliminary check avoids
        // subtraction on an already-invalid zero count; the postcheck latches
        // any internal fault rather than interpreting a wrapped count as valid.
        if self.word.load(Ordering::Acquire) & !FLAGS == 0 {
            self.word.store(CLOSED | FAILED, Ordering::Release);
            return;
        }
        let old = self.word.fetch_sub(READER, Ordering::Release);
        if old & !FLAGS == 0 {
            self.word.store(CLOSED | FAILED, Ordering::Release);
        }
    }
}
pub(super) type ReaderOwner = OwnedShared<ReaderState>;

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

// An abnormal registration unwind drops actual partial paired clones BEFORE
// failing and releasing its reserved count. The original owner stays rooted.
struct RegistrationReservation<'a> {
    state: &'a ReaderState,
    active: bool,
}
impl Drop for RegistrationReservation<'_> {
    fn drop(&mut self) {
        if self.active {
            // Partial paired clones are later-declared locals at the callsite;
            // Rust drops them BEFORE this reservation on abnormal unwind.
            self.state.fail();
            self.state.release();
        }
    }
}

impl AudioCompiledRevision {
    pub(super) fn register(
        revision: &OwnedShared<PreparedRevision>,
        registration: &ReaderOwner,
        identity: AudioCompiledRevisionIdentity,
    ) -> Result<Self, Error> {
        if !ReaderState::supported() {
            return Err(Error::UnsupportedConstructor);
        }
        if revision.parts().is_none() || registration.parts().is_none() {
            return Err(Error::UnknownStorage);
        }
        let state = &**registration;
        let word = state.word.load(Ordering::Acquire);
        if word & FAILED != 0 {
            return Err(Error::UnknownStorage);
        }
        if word & CLOSED != 0 {
            return Err(Error::SourceMismatch);
        }
        let next = word.checked_add(READER).ok_or(Error::Overflow)?;
        // Reserve BOTH identities and check overflow BEFORE any paired clone.
        let revision_handle = crate::audio_only_inputs::handle()?;
        let registration_handle = crate::audio_only_inputs::handle()?;
        #[cfg(test)]
        test_hook(TestPoint::BeforeCas);
        // One attempt only. Contention is a retryable control refusal, not an
        // Unknown owner or permission to forget a pending producer reservation.
        state
            .word
            .compare_exchange(word, next, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::ExternalReadersPending)?;
        let mut reservation = RegistrationReservation {
            state,
            active: true,
        };
        #[cfg(test)]
        test_hook(TestPoint::RegisteredBeforeClone);
        let paired_revision = revision.clone_on_control();
        #[cfg(test)]
        test_hook(TestPoint::RevisionCloned);
        let paired_registration = registration.clone_on_control();
        #[cfg(test)]
        test_hook(TestPoint::BothCloned);
        // Later-declared paired clones drop before the RAII reservation on
        // unwind. This production interval has only infallible fixed moves.
        let result = Self {
            identity,
            revision: Some(paired_revision),
            registration: paired_registration,
            revision_handle,
            registration_handle,
        };
        reservation.active = false;
        Ok(result)
    }

    /// Reserve another actual registered field before moving it into an export
    /// or compiled record. This cannot reopen a root that started retirement.
    /// ExternalReadersPending means this attempt lost its single CAS: retain
    /// the producer/slot and retry later; it is not a failed owner declaration.
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
        // The root waits for this count, so this Arc cannot be its final owner.
        drop(self.revision.take());
        #[cfg(test)]
        test_hook(TestPoint::RevisionDropped);
        self.registration.release();
        #[cfg(test)]
        test_hook(TestPoint::CountReleased);
        // The registration owner field drops only AFTER the release decrement.
    }
}

/// Only after mutable native runtime aliases retire. Counts never declare
/// provenance: at zero the actual remaining revision/Weak aliases still match.
pub(super) fn retirement_pending(
    revision: &OwnedShared<PreparedRevision>,
    registration: &ReaderOwner,
) -> Result<bool, Error> {
    registration.word.fetch_or(CLOSED, Ordering::AcqRel);
    let word = registration.word.load(Ordering::Acquire);
    if word & FAILED != 0
        || !ReaderState::supported()
        || revision.parts().is_none()
        || registration.parts().is_none()
    {
        registration.fail();
        return Err(Error::UnknownStorage);
    }
    if word & !FLAGS != 0 {
        // Count can precede clone or follow Arc drop: NEVER reconcile counts
        // during either in-flight interval. No root movement occurs here.
        return Ok(true);
    }
    // Acquire zero orders all prior reader revision drops. CLOSED prevents any
    // new registered clone; unexplained aliases are a sticky Unknown refusal.
    if Arc::strong_count(revision.arc()) != 1
        || Arc::weak_count(revision.arc()) != 0
        || Arc::weak_count(registration.arc()) != 0
    {
        registration.fail();
        return Err(Error::UnknownStorage);
    }
    // A dying handle can still own its registration field after decrement.
    Ok(Arc::strong_count(registration.arc()) != 1)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestPoint {
    BeforeCas,
    RegisteredBeforeClone,
    RevisionCloned,
    BothCloned,
    RevisionDropped,
    CountReleased,
}
#[cfg(test)]
type TestHook = Box<dyn FnMut(TestPoint)>;
#[cfg(test)]
thread_local! {
    static TEST_HOOK: std::cell::RefCell<Option<TestHook>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
fn test_hook(point: TestPoint) {
    let hook = TEST_HOOK.with(|hook| hook.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook(point);
        TEST_HOOK.with(|slot| *slot.borrow_mut() = Some(hook));
    }
}

#[cfg(test)]
mod tests {
    use super::{CLOSED, FAILED, FLAGS, READER, ReaderOwner, ReaderState, TEST_HOOK, TestPoint};
    use crate::{AudioPreparationPhase, AudioStorageLedgerReservation, AudioStorageRole};
    use std::sync::{Arc, Barrier, atomic::Ordering};

    struct HookReset;
    impl Drop for HookReset {
        fn drop(&mut self) {
            TEST_HOOK.with(|hook| *hook.borrow_mut() = None);
        }
    }
    fn hook(f: impl FnMut(TestPoint) + 'static) -> HookReset {
        TEST_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(f)));
        HookReset
    }

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
        assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
        assert!(custody.recover_on_control(&token).is_none());
        assert!(custody.capacity_report().revision_roots.constructed);
        drop(reader);
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
        drop(unexplained);
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

    #[test]
    fn compiled_reader_reserved_count_before_clone_prevents_early_retirement() {
        let Some((mut custody, token)) = fixture() else {
            return;
        };
        let reader = custody.register_compiled_reader_on_control(&token).unwrap();
        let reached = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let (worker_reached, worker_resume) = (reached.clone(), resume.clone());
        let worker = std::thread::spawn(move || {
            let _reset = hook(move |point| {
                if point == TestPoint::RegisteredBeforeClone {
                    worker_reached.wait();
                    worker_resume.wait();
                }
            });
            let duplicate = reader.try_duplicate_on_control().unwrap();
            (reader, duplicate)
        });
        reached.wait();
        assert_eq!(custody.reader_word_for_test(), Some(2 * READER));
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
        assert!(custody.capacity_report().revision_roots.constructed);
        resume.wait();
        let (reader, duplicate) = worker.join().unwrap();
        assert!(duplicate.try_duplicate_on_control().is_err());
        drop(reader);
        drop(duplicate);
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::Recoverable);
        assert!(custody.recover_on_control(&token).is_some());
    }

    #[test]
    fn compiled_reader_release_intervals_preserve_revision_and_registration_charges() {
        let Some((mut custody, token)) = fixture() else {
            return;
        };
        let reader = custody.register_compiled_reader_on_control(&token).unwrap();
        let reached = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let (worker_reached, worker_resume) = (reached.clone(), resume.clone());
        let worker = std::thread::spawn(move || {
            let _reset = hook(move |point| {
                if matches!(point, TestPoint::RevisionDropped | TestPoint::CountReleased) {
                    worker_reached.wait();
                    worker_resume.wait();
                }
            });
            drop(reader);
        });
        assert!(custody.cancel(&token));
        for remaining in [READER, 0] {
            reached.wait();
            custody.cleanup_on_control();
            assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
            assert_eq!(custody.reader_word_for_test(), Some(CLOSED | remaining));
            let report = custody.capacity_report().revision_roots;
            assert!(report.constructed && report.reader_registration_constructed);
            assert!(report.reader_registration_bytes > 0);
            resume.wait();
        }
        worker.join().unwrap();
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::Recoverable);
        assert!(custody.recover_on_control(&token).is_some());
    }

    #[test]
    fn compiled_reader_single_cas_collision_preserves_owner_and_allows_later_retry() {
        let Some((mut custody, token)) = fixture() else {
            return;
        };
        let source = Arc::new(custody.register_compiled_reader_on_control(&token).unwrap());
        let other = std::rc::Rc::new(std::cell::RefCell::new(None));
        {
            let (source_in_hook, other_in_hook) = (source.clone(), other.clone());
            let _reset = hook(move |point| {
                if point == TestPoint::BeforeCas {
                    *other_in_hook.borrow_mut() =
                        Some(source_in_hook.try_duplicate_on_control().unwrap());
                }
            });
            assert!(matches!(
                source.try_duplicate_on_control(),
                Err(crate::AudioStorageError::ExternalReadersPending)
            ));
        }
        assert_eq!(custody.reader_word_for_test(), Some(2 * READER));
        let retried = source.try_duplicate_on_control().unwrap();
        assert_eq!(retried.identity(), source.identity());
        assert_eq!(custody.reader_word_for_test(), Some(3 * READER));
        drop(retried);
        drop(other.borrow_mut().take());
        drop(source);
        assert_eq!(custody.reader_word_for_test(), Some(0));
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert!(custody.recover_on_control(&token).is_some());
    }

    #[test]
    fn compiled_reader_interrupted_registration_drops_partial_clones_then_latches_failure() {
        for point in [
            TestPoint::RegisteredBeforeClone,
            TestPoint::RevisionCloned,
            TestPoint::BothCloned,
        ] {
            let Some((mut custody, token)) = fixture() else {
                return;
            };
            let outcome = {
                let _reset = hook(move |actual| {
                    assert_ne!(actual, point, "injected interrupted registration");
                });
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    custody.register_compiled_reader_on_control(&token)
                }))
            };
            assert!(outcome.is_err());
            assert_eq!(custody.reader_word_for_test(), Some(CLOSED | FAILED));
            assert_eq!(custody.revision_strong_count_for_test().unwrap(), 1);
            assert!(custody.cancel(&token));
            custody.cleanup_on_control();
            assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
            assert!(custody.recover_on_control(&token).is_none());
            custody.cleanup_on_control();
            assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
        }
        eprintln!(
            "SUPPORTED interrupted reader registration: completed3 before-clone/one-clone/both-clones; no partial aliases or count leak; sticky failure"
        );
    }

    #[test]
    fn compiled_reader_atomic_body_and_first_touch_have_no_hidden_allocation() {
        if !ReaderState::supported() || !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        let (payload, header) = ReaderOwner::declared_parts().unwrap();
        assert_eq!(
            payload,
            std::mem::size_of::<std::sync::atomic::AtomicUsize>()
        );
        let mut state = None;
        let born = allocation_counter::measure(|| {
            state = Some(ReaderOwner::new_owned(ReaderState::default()));
        });
        assert_eq!(born.count_total, 1);
        assert_eq!(born.bytes_total, (payload + header) as u64);
        drop(state);
        let Some((mut custody, token)) = fixture() else {
            return;
        };
        let expected = custody
            .capacity_report()
            .revision_roots
            .reader_registration_bytes;
        assert_eq!(expected, payload + header);
        let alloc = allocation_counter::measure(|| {
            let reader = custody.register_compiled_reader_on_control(&token).unwrap();
            let second = reader.try_duplicate_on_control().unwrap();
            drop(second);
            drop(reader);
        });
        assert_eq!(alloc.count_total, 0);
        assert_eq!(alloc.bytes_total, 0);
        assert!(custody.cancel(&token));
        let retire = allocation_counter::measure(|| custody.cleanup_on_control());
        assert_eq!(retire.count_total, 0);
        assert_eq!(retire.bytes_total, 0);
        assert!(custody.recover_on_control(&token).is_some());
        let Some((mut zero, token)) = fixture() else {
            return;
        };
        assert!(zero.cancel(&token));
        let retire = allocation_counter::measure(|| zero.cleanup_on_control());
        assert_eq!(retire.count_total, 0);
        assert_eq!(retire.bytes_total, 0);
        assert_eq!(zero.phase(), AudioPreparationPhase::Recoverable);
        assert!(zero.recover_on_control(&token).is_some());
        eprintln!(
            "SUPPORTED atomic reader state: payload={payload} header={header} constructor_allocs={} constructor_bytes={} first_register_duplicate_drop_allocs={} zero_reader_retire_allocs={}",
            born.count_total, born.bytes_total, alloc.count_total, retire.count_total
        );
    }

    #[test]
    fn compiled_reader_overflow_and_invalid_zero_release_do_not_wrap_into_reusable_state() {
        let Some((mut custody, token)) = fixture() else {
            return;
        };
        let reader = custody.register_compiled_reader_on_control(&token).unwrap();
        let clones = crate::owned_shared::clone_count();
        let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|n| n.get());
        reader
            .registration
            .word
            .store(usize::MAX & !FLAGS, Ordering::Release);
        assert!(matches!(
            reader.try_duplicate_on_control(),
            Err(crate::AudioStorageError::Overflow)
        ));
        assert_eq!(crate::owned_shared::clone_count(), clones);
        assert_eq!(
            crate::audio_only_inputs::ISSUED_HANDLES.with(|n| n.get()),
            ids
        );
        // Explicit internal-fault injection; this is not a genuine huge owner
        // population and never establishes storage or admission authority.
        reader.registration.word.store(0, Ordering::Release);
        drop(reader);
        assert_eq!(custody.reader_word_for_test(), Some(CLOSED | FAILED));
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
        assert!(custody.recover_on_control(&token).is_none());
    }

    #[test]
    fn compiled_reader_registration_weak_alias_refuses_before_root_extraction() {
        let Some((mut custody, token)) = fixture() else {
            return;
        };
        let reader = custody.register_compiled_reader_on_control(&token).unwrap();
        let weak = Arc::downgrade(reader.registration.arc());
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
        drop(reader);
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
        assert!(custody.capacity_report().revision_roots.constructed);
        drop(weak);
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
        assert!(custody.recover_on_control(&token).is_none());
    }

    #[test]
    fn compiled_reader_new_body_exact_and_each_minus_one_refuse_before_allocation_or_birth() {
        if !ReaderState::supported() || !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        let bytes = ReaderState::declared_bytes().unwrap();
        for (retained, peak) in [(bytes - 1, bytes), (bytes, bytes - 1)] {
            let mut actual_destination = None;
            let before = crate::owned_shared::birth_count();
            let mut result = None;
            let alloc = allocation_counter::measure(|| {
                result = Some(ReaderState::construct_on_control(
                    &mut actual_destination,
                    retained,
                    peak,
                ));
            });
            assert_eq!(
                result.unwrap(),
                Err(crate::AudioStorageError::ReservationExceeded)
            );
            assert!(actual_destination.is_none());
            assert_eq!(crate::owned_shared::birth_count(), before);
            assert_eq!(alloc.count_total, 0);
            assert_eq!(alloc.bytes_total, 0);
        }
        let mut actual_destination = None;
        let alloc = allocation_counter::measure(|| {
            ReaderState::construct_on_control(&mut actual_destination, bytes, bytes).unwrap()
        });
        assert_eq!(alloc.count_total, 1);
        assert_eq!(alloc.bytes_total, bytes as u64);
        let duplicate = allocation_counter::measure(|| {
            assert_eq!(
                ReaderState::construct_on_control(&mut actual_destination, bytes, bytes),
                Err(crate::AudioStorageError::SourceMismatch)
            )
        });
        assert_eq!(duplicate.count_total, 0);
        assert_eq!(duplicate.bytes_total, 0);
        eprintln!(
            "SUPPORTED actual reader destination exact retained={bytes} peak={bytes}; both-minusone allocations=0/births=0; exact allocations={} bytes={}",
            alloc.count_total, alloc.bytes_total
        );
    }
}
