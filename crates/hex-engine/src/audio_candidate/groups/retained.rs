//! Constructor-paired group lifecycle, not a declaration of transported DATA.
//! The birth archive retains the record while the non-Clone lease travels with
//! the actual chain. Only normal physical destruction followed by archive
//! fanout closes it. Unexpected lease loss remains sticky and unresolved.
use super::{AudioStorageError, Group, Origin};
use crate::SharedMetadata;
use std::sync::atomic::{AtomicU8, Ordering};

const LIVE: u8 = 0;
const DROP_PENDING: u8 = 1;
const PHYSICALLY_DISPOSED: u8 = 2;
const CLOSED: u8 = 3;
const FAILED: u8 = 4;

struct RegistrationState {
    origin: Origin,
    phase: AtomicU8,
}
pub(super) struct GroupRegistration {
    state: SharedMetadata<RegistrationState>,
}
// Deliberately neither Clone nor Copy. A transfer moves this exact handle.
pub(in crate::audio_candidate) struct GroupLease {
    state: SharedMetadata<RegistrationState>,
}
pub(in crate::audio_candidate) fn registration_bytes() -> Result<usize, AudioStorageError> {
    let (body, header) = crate::owned_shared::OwnedShared::<RegistrationState>::declared_parts()
        .ok_or(AudioStorageError::UnknownStorage)?;
    body.checked_add(header).ok_or(AudioStorageError::Overflow)
}
impl GroupRegistration {
    pub(super) fn new_on_control(origin: Origin) -> (Self, GroupLease) {
        let state = SharedMetadata::new_on_control(RegistrationState {
            origin,
            phase: AtomicU8::new(LIVE),
        });
        let lease = GroupLease {
            state: state.clone(),
        };
        (Self { state }, lease)
    }
    pub(super) fn native_owner(&self) -> crate::audio_storage::Owner<'_> {
        crate::audio_storage::Owner::shared(self.state.paired_owner_on_control().arc())
    }
    pub(super) fn retained_live(&self) -> Result<bool, AudioStorageError> {
        match self.state.phase.load(Ordering::Acquire) {
            LIVE => Ok(true),
            CLOSED => Ok(false),
            _ => Err(AudioStorageError::UnknownStorage),
        }
    }
    pub(super) fn matches_live(&self, lease: &GroupLease) -> bool {
        self.state.shares_owner_with(&lease.state)
            && self.state.phase.load(Ordering::Acquire) == LIVE
    }
    pub(super) fn matches_disposed(&self, lease: &GroupLease) -> bool {
        self.state.shares_owner_with(&lease.state)
            && self.state.phase.load(Ordering::Acquire) == PHYSICALLY_DISPOSED
    }
    pub(super) fn close_after_fanout(&self) {
        self.state.phase.store(CLOSED, Ordering::Release);
    }
    pub(in crate::audio_candidate) fn visit<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        if ledger.skip_observed_shared_field(&self.state) {
            return Ok(());
        }
        let owner = self.state.paired_owner_on_control();
        ledger.observe_shared(
            owner.arc(),
            owner.parts(),
            crate::audio_storage::Owner::exclusive(&self.state),
        )
    }
}
impl GroupLease {
    pub(in crate::audio_candidate) fn matches_origin(&self, origin: Origin) -> bool {
        self.state.origin == origin && self.state.phase.load(Ordering::Acquire) == LIVE
    }
    pub(in crate::audio_candidate) fn visit<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        if ledger.skip_observed_shared_field(&self.state) {
            return Ok(());
        }
        let owner = self.state.paired_owner_on_control();
        ledger.observe_shared(
            owner.arc(),
            owner.parts(),
            crate::audio_storage::Owner::exclusive(&self.state),
        )
    }
}
impl Drop for GroupLease {
    fn drop(&mut self) {
        // Dropping a handle is not physical-disposal proof. The persistent
        // archive keeps this failure and its allocation alive.
        if self.state.phase.load(Ordering::Acquire) != CLOSED {
            self.state.phase.store(FAILED, Ordering::Release);
        }
    }
}
pub(in crate::audio_candidate) fn begin_disposal(
    groups: &[Group],
) -> Result<(), AudioStorageError> {
    // Validate ALL pairs before changing any phase. Callers have already
    // validated the corresponding actual runtime topology under exclusion.
    if groups.iter().any(|group| !group.lease_matches_origin()) {
        return Err(AudioStorageError::SourceMismatch);
    }
    for lease in groups.iter().filter_map(|group| group.lease.as_ref()) {
        lease.state.phase.store(DROP_PENDING, Ordering::Release);
    }
    Ok(())
}
pub(in crate::audio_candidate) fn complete_disposal(groups: &[Group]) {
    // Called only after the real Runtime Box destructor returned normally.
    // A caught destructor unwind leaves DROP_PENDING permanently unresolved.
    for lease in groups.iter().filter_map(|group| group.lease.as_ref()) {
        lease
            .state
            .phase
            .store(PHYSICALLY_DISPOSED, Ordering::Release);
    }
}

// Separate shell identity from independently travelling group identities. The
// shell record is retained by its original archive; its lease moves into the
// actual ClaimedState, and finally into the real normal-destructor proof.
struct RuntimeRegistrationState {
    identity: super::AudioCandidateIdentity,
    phase: AtomicU8,
}
pub(in crate::audio_candidate) struct RuntimeStorageRegistration {
    state: SharedMetadata<RuntimeRegistrationState>,
    physical: Option<crate::audio_storage::NativePhysicalCapture>,
}
pub(in crate::audio_candidate) struct RuntimeStorageLease {
    state: SharedMetadata<RuntimeRegistrationState>,
}
pub(in crate::audio_candidate) fn runtime_registration_bytes() -> Result<usize, AudioStorageError> {
    let (body, header) =
        crate::owned_shared::OwnedShared::<RuntimeRegistrationState>::declared_parts()
            .ok_or(AudioStorageError::UnknownStorage)?;
    body.checked_add(header).ok_or(AudioStorageError::Overflow)
}
impl RuntimeStorageRegistration {
    pub(super) fn new_on_control(
        identity: super::AudioCandidateIdentity,
    ) -> (Self, RuntimeStorageLease) {
        let state = SharedMetadata::new_on_control(RuntimeRegistrationState {
            identity,
            phase: AtomicU8::new(LIVE),
        });
        let lease = RuntimeStorageLease {
            state: state.clone(),
        };
        (
            Self {
                state,
                physical: None,
            },
            lease,
        )
    }
    pub(super) fn native_owner(&self) -> crate::audio_storage::Owner<'_> {
        crate::audio_storage::Owner::shared(self.state.paired_owner_on_control().arc())
    }
    pub(super) fn retained_live(&self) -> Result<bool, AudioStorageError> {
        match self.state.phase.load(Ordering::Acquire) {
            LIVE => Ok(true),
            CLOSED => Ok(false),
            _ => Err(AudioStorageError::UnknownStorage),
        }
    }
    pub(super) fn visit_partition<'a>(
        &'a self,
        partition: crate::audio_storage::NativeRuntimePartition,
        registrations: &'a dyn crate::audio_storage::NativeArchiveAnchorFields,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.physical
            .as_ref()
            .ok_or(AudioStorageError::UnknownStorage)?
            .visit_partition(partition, registrations, ledger)
    }
    pub(super) fn retire_anchor_on_control(
        &mut self,
        first_panic: &mut Option<Box<dyn std::any::Any + Send>>,
    ) -> Result<(), AudioStorageError> {
        if let Some(physical) = &mut self.physical {
            physical.retire_anchor_on_control(first_panic)?;
        }
        Ok(())
    }
    pub(super) fn matches_live(&self, lease: &RuntimeStorageLease) -> bool {
        self.state.shares_owner_with(&lease.state)
            && self.state.phase.load(Ordering::Acquire) == LIVE
    }
    pub(super) fn matches_disposed(&self, lease: &RuntimeStorageLease) -> bool {
        self.state.shares_owner_with(&lease.state)
            && self.state.phase.load(Ordering::Acquire) == PHYSICALLY_DISPOSED
    }
    pub(super) fn close_after_fanout(&self) {
        self.state.phase.store(CLOSED, Ordering::Release);
    }
    pub(in crate::audio_candidate) fn visit<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        if ledger.skip_observed_shared_field(&self.state) {
            return Ok(());
        }
        let owner = self.state.paired_owner_on_control();
        ledger.observe_shared(
            owner.arc(),
            owner.parts(),
            crate::audio_storage::Owner::exclusive(&self.state),
        )?;
        if let Some(physical) = &self.physical {
            physical.visit_metadata(ledger)?;
        }
        Ok(())
    }
    pub(super) fn has_physical(&self) -> bool {
        self.physical.is_some()
    }
    pub(super) fn install_physical(
        &mut self,
        physical: crate::audio_storage::NativePhysicalCapture,
    ) -> Result<(), AudioStorageError> {
        if self.physical.is_some() || self.state.phase.load(Ordering::Acquire) != LIVE {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.physical = Some(physical);
        Ok(())
    }
}
impl RuntimeStorageLease {
    pub(in crate::audio_candidate) fn matches_identity(
        &self,
        identity: super::AudioCandidateIdentity,
    ) -> bool {
        self.state.identity == identity && self.state.phase.load(Ordering::Acquire) == LIVE
    }
    pub(in crate::audio_candidate) fn begin_disposal(&self) {
        self.state.phase.store(DROP_PENDING, Ordering::Release);
    }
    pub(in crate::audio_candidate) fn complete_disposal(&self) {
        self.state
            .phase
            .store(PHYSICALLY_DISPOSED, Ordering::Release);
    }
    pub(in crate::audio_candidate) fn visit<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        if ledger.skip_observed_shared_field(&self.state) {
            return Ok(());
        }
        let owner = self.state.paired_owner_on_control();
        ledger.observe_shared(
            owner.arc(),
            owner.parts(),
            crate::audio_storage::Owner::exclusive(&self.state),
        )
    }
}
impl Drop for RuntimeStorageLease {
    fn drop(&mut self) {
        if self.state.phase.load(Ordering::Acquire) != CLOSED {
            self.state.phase.store(FAILED, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_candidate::{
        AudioCandidateDisposalPhase, AudioCandidateOwner, RuntimeDropProbe,
    };

    fn owner() -> AudioCandidateOwner {
        let (custody, token, _, _) =
            crate::audio_preparation::candidate_test_fixture(false).unwrap();
        AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap()
    }
    #[test]
    fn actual_constructor_pairs_refuse_same_origin_foreign_registration_and_missing_lease() {
        let mut owner = owner();
        owner.inner.validate_fresh_origins().unwrap();
        let group = owner
            .inner
            .groups
            .iter()
            .position(|g| g.origin.is_some())
            .unwrap();
        let origin = owner.inner.groups[group].origin.unwrap();
        let original = owner.inner.groups[group].lease.take().unwrap();
        assert_eq!(
            owner.inner.validate_fresh_origins(),
            Err(AudioStorageError::SourceMismatch)
        );
        let (foreign_registration, foreign_lease) = GroupRegistration::new_on_control(origin);
        owner.inner.groups[group].lease = Some(foreign_lease);
        // Scalar origin equality cannot stand in for the actual birth pair.
        assert!(owner.inner.groups[group].lease_matches_origin());
        assert_eq!(
            owner.inner.validate_fresh_origins(),
            Err(AudioStorageError::SourceMismatch)
        );
        drop(owner.inner.groups[group].lease.replace(original));
        assert_eq!(
            foreign_registration.state.phase.load(Ordering::Acquire),
            FAILED
        );
        owner.inner.validate_fresh_origins().unwrap();
        owner.dispose_on_control().unwrap();
        assert!(owner.inner.archive.all_closed());
    }
    #[test]
    fn real_runtime_destructor_unwind_keeps_actual_birth_records_pending_and_cannot_retry() {
        let mut owner = owner();
        owner.inner.runtime.as_mut().unwrap().probe =
            Some(RuntimeDropProbe(Some(Box::new(|| {
                panic!("retained-group actual runtime destructor fault");
            }))));
        assert_eq!(
            owner.dispose_runtime_on_control().err(),
            Some(AudioStorageError::UnknownStorage)
        );
        assert_eq!(owner.phase(), AudioCandidateDisposalPhase::FailedIncomplete);
        assert!(owner.inner.runtime.is_none());
        assert!(!owner.inner.archive.original_disposed);
        assert_eq!(
            owner
                .inner
                .runtime_lease
                .as_ref()
                .unwrap()
                .state
                .phase
                .load(Ordering::Acquire),
            DROP_PENDING
        );
        assert!(
            owner
                .inner
                .groups
                .iter()
                .filter_map(|g| g.lease.as_ref())
                .all(|lease| lease.state.phase.load(Ordering::Acquire) == DROP_PENDING)
        );
        assert_eq!(
            owner.dispose_runtime_on_control().err(),
            Some(AudioStorageError::UnknownStorage)
        );
        assert!(
            owner
                .inner
                .first_panic
                .as_ref()
                .unwrap()
                .downcast_ref::<&str>()
                .unwrap()
                .contains("actual runtime destructor fault")
        );
    }
    #[test]
    fn actual_pair_metadata_dedups_body_but_requires_each_handle_and_both_exact_caps() {
        let owner = owner();
        let row = &owner.inner.archive.groups[0];
        let lease = owner
            .inner
            .groups
            .iter()
            .find_map(|g| {
                g.lease
                    .as_ref()
                    .filter(|lease| row.registration.matches_live(lease))
            })
            .unwrap();
        let reservation = crate::AudioStorageLedgerReservation {
            data_rows: 8,
            handle_rows: 8,
            root_rows: 2,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        };
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
        row.registration.visit(&mut ledger).unwrap();
        let body = ledger.summary().unwrap().data_bytes;
        assert_eq!(body, registration_bytes().unwrap());
        assert_eq!(
            ledger.validate_selected_shared_handles_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
        lease.visit(&mut ledger).unwrap();
        assert_eq!(ledger.summary().unwrap().data_bytes, body);
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        let summary = ledger.summary().unwrap();
        drop(ledger);
        let mut exact = reservation;
        exact.max_retained_bytes = summary.retained_bytes;
        exact.max_construction_peak_bytes = summary.construction_peak_bytes;
        for (retained_minus, peak_minus, passes) in [(0, 0, true), (1, 0, false), (0, 1, false)] {
            let mut limits = exact;
            limits.max_retained_bytes -= retained_minus;
            limits.max_construction_peak_bytes -= peak_minus;
            let result =
                crate::AudioRootStorageLedger::try_new_on_control(limits).and_then(|mut ledger| {
                    row.registration.visit(&mut ledger)?;
                    lease.visit(&mut ledger)?;
                    ledger.validate_selected_shared_handles_on_control()?;
                    ledger.summary()
                });
            assert_eq!(
                result.is_ok(),
                passes,
                "retained-minus={retained_minus} peak-minus={peak_minus}"
            );
        }
        let extra = lease.state.clone();
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
        row.registration.visit(&mut ledger).unwrap();
        lease.visit(&mut ledger).unwrap();
        assert_eq!(
            ledger.validate_selected_shared_handles_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
        drop(ledger);
        drop(extra);
        // This probe covers this concrete metadata body/ledger only. Neither
        // ceiling here is a whole-runtime preconstruction admission proof.
    }
    #[test]
    fn shell_pair_moves_into_claimed_state_and_proof_without_recreating_registration() {
        let mut owner = owner();
        let limits = crate::AudioStorageLedgerReservation {
            data_rows: 4096,
            handle_rows: 2048,
            root_rows: 4,
            max_retained_bytes: 64 * 1024 * 1024,
            max_construction_peak_bytes: 64 * 1024 * 1024,
        };
        owner.seal_constructor_on_control(limits).unwrap();
        let (mut runtime, mut anchor) = owner.split_constructor_on_control().ok().unwrap();
        assert!(anchor.owner.inner.runtime_lease.is_none());
        assert!(anchor.constructor_claims(&runtime).is_some());
        let proof = runtime.dispose_on_control().unwrap();
        assert_eq!(
            runtime.dispose_on_control().err(),
            Some(AudioStorageError::UnknownStorage)
        );
        assert!(proof.runtime_lease.is_some());
        anchor.owner.inner.archive.preflight(&proof).unwrap();
        anchor.owner.inner.archive.commit(&proof);
        assert!(anchor.owner.inner.archive.all_closed());
        drop(proof);
        assert!(matches!(
            anchor.finish_disposal_on_control(),
            Err(AudioStorageError::ExternalReadersPending)
        ));
        drop(runtime); // Same actual claimed Box is independently retained.
        anchor.finish_disposal_on_control().unwrap();
    }
    #[test]
    fn unpublished_actual_draft_cancel_closes_local_pairs_only_after_normal_destructor() {
        let (custody, token, _, _) =
            crate::audio_preparation::candidate_test_fixture(false).unwrap();
        let mut draft = crate::AudioCandidateDraft::new_on_control();
        draft.prepare_on_control(&custody, &token).unwrap();
        assert!(!draft.archive.as_ref().unwrap().all_closed());
        draft.dispose_on_control().unwrap();
        assert!(draft.runtime.is_none());
        assert!(draft.archive.as_ref().unwrap().all_closed());
        assert_eq!(
            draft
                .runtime_lease
                .as_ref()
                .unwrap()
                .state
                .phase
                .load(Ordering::Acquire),
            CLOSED
        );
        draft.dispose_on_control().unwrap();
    }
    #[test]
    fn shell_same_identity_foreign_record_and_missing_lease_refuse() {
        let mut owner = owner();
        let original = owner.inner.runtime_lease.take().unwrap();
        assert_eq!(
            owner.inner.validate_fresh_origins(),
            Err(AudioStorageError::SourceMismatch)
        );
        let (foreign, lease) = RuntimeStorageRegistration::new_on_control(owner.identity());
        owner.inner.runtime_lease = Some(lease);
        assert!(
            owner
                .inner
                .runtime_lease
                .as_ref()
                .unwrap()
                .matches_identity(owner.identity())
        );
        assert_eq!(
            owner.inner.validate_fresh_origins(),
            Err(AudioStorageError::SourceMismatch)
        );
        drop(owner.inner.runtime_lease.replace(original));
        assert_eq!(foreign.state.phase.load(Ordering::Acquire), FAILED);
        owner.inner.validate_fresh_origins().unwrap();
        owner.dispose_on_control().unwrap();
    }
    #[test]
    fn actual_normal_drop_requires_fanout_and_double_fanout_refuses() {
        let mut owner = owner();
        let proof = owner.dispose_runtime_on_control().unwrap();
        assert!(!owner.inner.archive.all_closed());
        assert!(
            proof
                .groups
                .iter()
                .filter_map(|g| g.lease.as_ref())
                .all(|lease| lease.state.phase.load(Ordering::Acquire) == PHYSICALLY_DISPOSED)
        );
        owner.inner.archive.preflight(&proof).unwrap();
        owner.inner.archive.commit(&proof);
        assert!(owner.inner.archive.all_closed());
        assert_eq!(
            owner.inner.archive.preflight(&proof),
            Err(AudioStorageError::SourceMismatch)
        );
        drop(proof);
        assert!(
            owner.inner.archive.groups.iter().all(|row| row
                .registration
                .state
                .phase
                .load(Ordering::Acquire)
                == CLOSED)
        );
        owner.finish_disposal_on_control().unwrap();
    }
}
