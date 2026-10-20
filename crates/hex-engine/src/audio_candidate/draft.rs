//! Borrowed construction for a host's already-rooted preparation cell. No Drop
//! implementation performs cleanup; sticky failures must remain in host custody.
use super::*;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Empty,
    Building,
    Ready,
    Refused,
    DropPending,
    Disposed,
    Transferred,
    Failed,
}
pub struct AudioCandidateDraft {
    pub(super) identity: Option<AudioCandidateIdentity>,
    pub(super) sample_rate: u32,
    pub(super) runtime: Option<Box<RuntimeOwner>>,
    pub(super) runtime_lease: Option<groups::RuntimeStorageLease>,
    pub(super) groups: groups::GroupBook,
    pub(super) journal: groups::TransferJournal,
    pub(super) archive: Option<groups::Archive>,
    purpose: ConstructorPurpose,
    phase: Phase,
    first_panic: Option<Box<dyn Any + Send>>,
}
impl AudioCandidateDraft {
    /// Empty inline destination; reserve its host slot before collecting a bank.
    pub fn new_on_control() -> Self {
        Self {
            identity: None,
            sample_rate: 0,
            runtime: None,
            runtime_lease: None,
            groups: groups::GroupBook::default(),
            journal: groups::TransferJournal::default(),
            archive: None,
            purpose: ConstructorPurpose::NativeFresh,
            phase: Phase::Empty,
            first_panic: None,
        }
    }
    /// Custody and its original bank remain borrowed throughout every fallible
    /// step. Each successfully constructed draft owner is stored before the next
    /// allocation/validation. No native/publication authority is produced here.
    pub fn prepare_on_control(
        &mut self,
        custody: &AudioPreparationCustody,
        token: &AudioPreparationToken,
    ) -> Result<(), AudioStorageError> {
        self.prepare_with(custody, token, || Ok(()))
    }
    pub(super) fn prepare_main_on_control(
        &mut self,
        custody: &AudioPreparationCustody,
        token: &AudioPreparationToken,
    ) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Empty {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.purpose = ConstructorPurpose::MainExport;
        self.prepare_with(custody, token, || Ok(()))
    }
    fn prepare_with(
        &mut self,
        custody: &AudioPreparationCustody,
        token: &AudioPreparationToken,
        after_runtime: impl FnOnce() -> Result<(), AudioStorageError>,
    ) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Empty {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.phase = Phase::Building;
        match catch_unwind(AssertUnwindSafe(|| {
            self.build_on_control(custody, token, std::mem::size_of::<Self>(), after_runtime)
        })) {
            Ok(Ok(())) => {
                self.phase = Phase::Ready;
                Ok(())
            }
            Ok(Err(error)) => {
                self.phase = Phase::Refused;
                Err(error)
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = Phase::Failed;
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
    pub(super) fn build_on_control(
        &mut self,
        custody: &AudioPreparationCustody,
        token: &AudioPreparationToken,
        draft_inline_bytes: usize,
        after_runtime: impl FnOnce() -> Result<(), AudioStorageError>,
    ) -> Result<(), AudioStorageError> {
        let runtime_id = crate::audio_only_inputs::handle()?;
        let (root, incarnation, revision) = custody.candidate_root(token)?;
        let identity = AudioCandidateIdentity {
            root,
            incarnation,
            runtime: runtime_id,
        };
        self.identity = Some(identity);
        self.sample_rate = revision.arrangement().sample_rate();
        let group_bound = crate::session_playback::candidate_group_reservation(
            revision.arrangement().tracks().len(),
            revision.arrangement().return_buses().len(),
        )?;
        self.runtime = Some(Box::new(RuntimeOwner {
            playback: match self.purpose {
                ConstructorPurpose::NativeFresh => Playback::new_audio_candidate(revision)?,
                ConstructorPurpose::MainExport => {
                    Playback::new_audio_candidate_for_purpose(revision, self.purpose)?
                }
            },
            #[cfg(any(test, feature = "instrument-test-support"))]
            probe: None,
        }));
        after_runtime()?;
        let runtime = &self
            .runtime
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .playback;
        let mut count = 0_usize;
        runtime.visit_candidate_groups(&mut |kind, _, present| {
            validate_supported_location(kind, present)?;
            count = count.checked_add(1).ok_or(AudioStorageError::Overflow)?;
            Ok(())
        })?;
        if count > group_bound {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        self.groups
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        runtime.validate_candidate_runtime_storage(
            self.groups
                .capacity()
                .checked_mul(std::mem::size_of::<Group>())
                .and_then(|n| n.checked_add(std::mem::size_of::<RuntimeOwner>()))
                .and_then(|n| n.checked_add(std::mem::size_of::<CandidateState>()))
                .and_then(|n| n.checked_add(draft_inline_bytes))
                .ok_or(AudioStorageError::Overflow)?,
        )?;
        runtime.visit_candidate_groups(&mut |kind, index, present| {
            let member = self.groups.len();
            self.groups.push(Group {
                location: (kind, index),
                present,
                closed: false,
                lease: None,
                origin: present.then_some(groups::Origin {
                    runtime: identity,
                    member,
                }),
            });
            Ok(())
        })?;
        // Bound the actual new paired registration bodies before allocating
        // the first one; the existing concrete runtime cap is unchanged.
        let shell_bytes = groups::runtime_registration_bytes()?
            .checked_add(groups::group_book_registration_bytes()?)
            .and_then(|n| n.checked_add(groups::group_book_construction_scratch_bytes().ok()?))
            .ok_or(AudioStorageError::Overflow)?
            .checked_add(groups::journal_registration_bytes()?)
            .and_then(|n| n.checked_add(groups::journal_construction_scratch_bytes().ok()?))
            .ok_or(AudioStorageError::Overflow)?;
        let registration_bytes = self
            .groups
            .iter()
            .filter(|g| g.origin.is_some())
            .count()
            .checked_mul(groups::registration_bytes()?)
            .and_then(|n| n.checked_add(shell_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        runtime.validate_candidate_runtime_storage(
            self.groups
                .capacity()
                .checked_mul(std::mem::size_of::<Group>())
                .and_then(|n| {
                    n.checked_add(count.checked_mul(std::mem::size_of::<groups::ArchivedOrigin>())?)
                })
                .and_then(|n| n.checked_add(registration_bytes))
                .and_then(|n| n.checked_add(std::mem::size_of::<RuntimeOwner>()))
                .and_then(|n| n.checked_add(std::mem::size_of::<CandidateState>()))
                .and_then(|n| n.checked_add(draft_inline_bytes))
                .ok_or(AudioStorageError::Overflow)?,
        )?;
        self.journal
            .try_reserve_exact(count)
            .map_err(|_| AudioStorageError::Allocation)?;
        let (archive, runtime_lease) =
            groups::Archive::prepare(identity, &mut self.groups, &mut self.journal)?;
        self.archive = Some(archive);
        self.runtime_lease = Some(runtime_lease);
        let extra = self
            .archive
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .metadata_bytes()?
            .checked_add(
                self.journal
                    .capacity()
                    .checked_mul(std::mem::size_of::<groups::TransferRow>())
                    .ok_or(AudioStorageError::Overflow)?,
            )
            .ok_or(AudioStorageError::Overflow)?;
        runtime.validate_candidate_runtime_storage(
            extra
                .checked_add(
                    self.groups
                        .capacity()
                        .checked_mul(std::mem::size_of::<Group>())
                        .ok_or(AudioStorageError::Overflow)?,
                )
                .and_then(|n| n.checked_add(std::mem::size_of::<RuntimeOwner>()))
                .and_then(|n| n.checked_add(std::mem::size_of::<CandidateState>()))
                .and_then(|n| n.checked_add(draft_inline_bytes))
                .ok_or(AudioStorageError::Overflow)?,
        )?;
        Ok(())
    }
    /// The final ownership transfer has no fallible work after taking custody.
    /// Every destination and actual root association is checked first. Control
    /// Box allocation follows the existing fatal-OOM convention.
    pub fn finish_on_control(
        &mut self,
        custody: &mut Option<AudioPreparationCustody>,
        token: &mut Option<AudioPreparationToken>,
        destination: &mut Option<AudioCandidateOwner>,
    ) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Ready || destination.is_some() || self.runtime.is_none() {
            return Err(AudioStorageError::SourceMismatch);
        }
        let identity = self.identity.ok_or(AudioStorageError::SourceMismatch)?;
        let source = custody.as_ref().ok_or(AudioStorageError::SourceMismatch)?;
        let source_token = token.as_ref().ok_or(AudioStorageError::SourceMismatch)?;
        let root = source.candidate_root_view(source_token)?;
        let runtime_root = self
            .runtime
            .as_ref()
            .and_then(|r| r.playback.prepared_revision())
            .ok_or(AudioStorageError::SourceMismatch)?;
        if !std::sync::Arc::ptr_eq(root, runtime_root) {
            return Err(AudioStorageError::SourceMismatch);
        }
        let Some(archive) = self.archive.take() else {
            return Err(AudioStorageError::SourceMismatch);
        };
        let Some(source) = custody.take() else {
            self.archive = Some(archive);
            return Err(AudioStorageError::SourceMismatch);
        };
        let Some(source_token) = token.take() else {
            *custody = Some(source);
            self.archive = Some(archive);
            return Err(AudioStorageError::SourceMismatch);
        };
        *destination = Some(AudioCandidateOwner {
            inner: Box::new(CandidateState {
                identity,
                custody: source,
                token: source_token,
                runtime: self.runtime.take(),
                runtime_lease: self.runtime_lease.take(),
                groups: std::mem::take(&mut self.groups),
                journal: std::mem::take(&mut self.journal),
                archive,
                runtime_receipt: None,
                phase: AudioCandidateDisposalPhase::Owned,
                first_panic: None,
                receipt_taken: false,
                sample_rate: self.sample_rate,
                recovered_inputs: None,
                inputs_drop_pending: false,
                inputs_disposed: false,
                constructor_data: None,
            }),
        });
        self.phase = Phase::Transferred;
        Ok(())
    }
    /// A failed construction/drop is sticky. Hosts retain the entire cell and
    /// all remaining preparation owners instead of retrying or releasing costs.
    pub fn failed_incomplete(&self) -> bool {
        matches!(self.phase, Phase::Failed | Phase::DropPending)
    }
    /// Drop only actual draft runtime aliases before the host cleans the original
    /// preparation. Metadata stays in this cell until its own normal destruction.
    /// No publication-return or owner-group receipt is minted for a draft.
    pub fn dispose_on_control(&mut self) -> Result<(), AudioStorageError> {
        match self.phase {
            Phase::Failed | Phase::DropPending => return Err(AudioStorageError::UnknownStorage),
            Phase::Disposed | Phase::Transferred => return Ok(()),
            _ => {}
        }
        self.phase = Phase::DropPending;
        if let Some(archive) = &self.archive {
            let lease = self
                .runtime_lease
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?;
            archive.validate_runtime_pair(lease)?;
            archive.validate_fresh_pairs(&self.groups)?;
            groups::begin_disposal(&self.groups)?;
            lease.begin_disposal();
        } else if self.runtime_lease.is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        match catch_unwind(AssertUnwindSafe(|| drop(self.runtime.take()))) {
            Ok(()) => {
                if let Some(archive) = &mut self.archive {
                    let lease = self
                        .runtime_lease
                        .as_ref()
                        .ok_or(AudioStorageError::SourceMismatch)?;
                    groups::complete_disposal(&self.groups);
                    lease.complete_disposal();
                    archive.close_disposed_draft(&self.groups, lease)?;
                }
                self.phase = Phase::Disposed;
                Ok(())
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = Phase::Failed;
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
}

impl AudioCandidateDraft {
    pub(super) fn visit_transferred_host_shell<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        owner: &AudioCandidateOwner,
    ) -> Result<(), AudioStorageError> {
        self.visit_transferred_shell_for_purpose(ledger, owner, ConstructorPurpose::NativeFresh)
    }
    pub(super) fn visit_transferred_main_shell<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        owner: &AudioCandidateOwner,
    ) -> Result<(), AudioStorageError> {
        self.visit_transferred_shell_for_purpose(ledger, owner, ConstructorPurpose::MainExport)
    }
    fn visit_transferred_shell_for_purpose<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        owner: &AudioCandidateOwner,
        expected: ConstructorPurpose,
    ) -> Result<(), AudioStorageError> {
        let Self {
            identity,
            sample_rate,
            runtime,
            runtime_lease,
            groups,
            journal,
            archive,
            purpose,
            phase,
            first_panic,
        } = self;
        if *phase != Phase::Transferred
            || *identity != Some(owner.identity())
            || *sample_rate != owner.sample_rate()
            || *purpose != expected
            || runtime.is_some()
            || runtime_lease.is_some()
            || archive.is_some()
            || first_panic.is_some()
            || !groups.is_empty()
            || !journal.is_empty()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        // Includes any real remaining empty backing/metadata. Transferred does
        // not itself declare those bytes absent or retire the destination.
        groups.visit(ledger)?;
        journal.visit(ledger)
    }

    pub(super) fn visit_unconstructed_host_children<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Self {
            identity,
            sample_rate: _,
            runtime,
            runtime_lease,
            groups,
            journal,
            archive,
            purpose: _,
            phase,
            first_panic,
        } = self;
        if *phase != Phase::Empty
            || identity.is_some()
            || runtime.is_some()
            || runtime_lease.is_some()
            || archive.is_some()
            || first_panic.is_some()
            || !groups.is_empty()
            || !journal.is_empty()
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        groups.visit(ledger)?;
        journal.visit(ledger)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn borrowed_draft_finishes_same_runtime_and_recovers_exact_original_bank() {
        let Some((custody, token, pointer, capacity)) =
            crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let mut custody = Some(custody);
        let mut token = Some(token);
        let mut draft = AudioCandidateDraft::new_on_control();
        draft
            .prepare_on_control(custody.as_ref().unwrap(), token.as_ref().unwrap())
            .unwrap();
        let runtime = draft.runtime.as_deref().unwrap() as *const RuntimeOwner;
        let groups = draft.groups.as_ptr();
        let journal = draft.journal.as_ptr();
        let identity = draft.identity.unwrap();
        let mut destination = None;
        // A foreign token refuses before moving any draft/custody destination.
        let Some((other, foreign, _, _)) = crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let original = token.take().unwrap();
        token = Some(foreign);
        assert!(
            draft
                .finish_on_control(&mut custody, &mut token, &mut destination)
                .is_err()
        );
        assert!(destination.is_none());
        assert!(custody.is_some());
        token = Some(original);
        draft
            .finish_on_control(&mut custody, &mut token, &mut destination)
            .unwrap();
        assert!(custody.is_none() && token.is_none());
        let mut owner = destination.take().unwrap();
        assert_eq!(owner.identity(), identity);
        assert_eq!(
            owner.inner.runtime.as_deref().unwrap() as *const RuntimeOwner,
            runtime
        );
        assert_eq!(owner.inner.groups.as_ptr(), groups);
        assert_eq!(owner.inner.journal.as_ptr(), journal);
        assert!(
            draft
                .finish_on_control(&mut custody, &mut token, &mut destination)
                .is_err()
        );
        owner.dispose_on_control().unwrap();
        let bank = owner.recover_inputs_on_control().unwrap();
        assert_eq!(
            (bank.sources.as_ptr() as usize, bank.sources.capacity()),
            (pointer, capacity)
        );
        drop(other);
    }

    #[test]
    fn ordinary_draft_refusal_retains_runtime_until_explicit_control_cleanup() {
        let Some((mut custody, token, pointer, capacity)) =
            crate::audio_preparation::candidate_test_fixture(false)
        else {
            return;
        };
        let mut draft = AudioCandidateDraft::new_on_control();
        assert_eq!(
            draft.prepare_with(&custody, &token, || Err(AudioStorageError::Capacity)),
            Err(AudioStorageError::Capacity)
        );
        assert!(draft.runtime.is_some());
        assert!(!draft.failed_incomplete());
        let observed = Arc::new(std::sync::Mutex::new(None));
        let record = observed.clone();
        draft.runtime.as_mut().unwrap().probe = Some(RuntimeDropProbe(Some(Box::new(move || {
            *record.lock().unwrap() = Some(std::thread::current().id());
        }))));
        let caller = std::thread::current().id();
        let (mut custody, thread_id) = std::thread::spawn(move || {
            draft.dispose_on_control().unwrap();
            draft.dispose_on_control().unwrap();
            drop(draft);
            custody.close_on_control();
            custody.cleanup_on_control();
            (custody, std::thread::current().id())
        })
        .join()
        .unwrap();
        assert_ne!(caller, thread_id);
        assert_eq!(*observed.lock().unwrap(), Some(thread_id));
        let bank = custody.recover_on_control(&token).unwrap();
        assert_eq!(
            (bank.sources.as_ptr() as usize, bank.sources.capacity()),
            (pointer, capacity)
        );
    }

    #[test]
    fn draft_construction_and_actual_runtime_destructor_panics_are_sticky() {
        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        for during_construction in [true, false] {
            let Some((custody, token, _, _)) =
                crate::audio_preparation::candidate_test_fixture(false)
            else {
                return;
            };
            let mut draft = AudioCandidateDraft::new_on_control();
            let drops = Arc::new(AtomicUsize::new(0));
            let calls = Arc::new(AtomicUsize::new(0));
            if during_construction {
                let counter = calls.clone();
                let payload = drops.clone();
                assert!(
                    draft
                        .prepare_with(&custody, &token, || {
                            counter.fetch_add(1, Ordering::SeqCst);
                            std::panic::panic_any(Payload(payload));
                        })
                        .is_err()
                );
                assert!(draft.runtime.is_some());
            } else {
                draft.prepare_on_control(&custody, &token).unwrap();
                let counter = calls.clone();
                let payload = drops.clone();
                draft.runtime.as_mut().unwrap().probe =
                    Some(RuntimeDropProbe(Some(Box::new(move || {
                        counter.fetch_add(1, Ordering::SeqCst);
                        std::panic::panic_any(Payload(payload));
                    }))));
                assert!(draft.dispose_on_control().is_err());
                assert!(draft.runtime.is_none());
            }
            assert!(draft.failed_incomplete());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(drops.load(Ordering::SeqCst), 0);
            let first = draft.first_panic.as_ref().unwrap().as_ref() as *const dyn Any as *const ();
            assert!(draft.prepare_on_control(&custody, &token).is_err());
            assert!(draft.dispose_on_control().is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                draft.first_panic.as_ref().unwrap().as_ref() as *const dyn Any as *const (),
                first
            );
            assert_eq!(
                custody.phase(),
                AudioPreparationPhase::ArrangementAndSessionSources
            );
            drop(draft); // Explicit fixture teardown, not production failed-cell disposal.
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        }
    }
}
