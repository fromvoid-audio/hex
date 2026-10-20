//! Three existing Root fields, moved with their individual constructor leases.
//! No owning getter, Clone implementation, pointer adoption or cleanup permit.
use super::*;
use hex_engine::sampler_host_fields::{NativeRootFieldLease, NativeRootFieldRegistration};
const PUBLICATION: usize = 0;
const CALLBACK: usize = 1;
const RETIREMENT: usize = 2;
pub(crate) struct RootActor {
    root: Option<CandidateRoot>,
    lease: Option<NativeRootFieldLease>,
}
impl std::ops::Deref for RootActor {
    type Target = CandidateRoot;
    fn deref(&self) -> &CandidateRoot {
        self.root
            .as_ref()
            .expect("Root actor field already released")
    }
}
impl RootActor {
    fn is_bound_for_role(&self, role: usize) -> bool {
        match (&self.root.as_ref().unwrap().0.root_fields, &self.lease) {
            (None, None) => true, // original unqualified resident path
            (Some(fields), Some(lease)) => fields.matches_actual_field_role_on_control(lease, role),
            _ => false,
        }
    }
    pub(crate) fn is_publication_field(&self) -> bool {
        self.is_bound_for_role(PUBLICATION)
    }
    pub(crate) fn is_callback_field(&self) -> bool {
        self.is_bound_for_role(CALLBACK)
    }
    pub(crate) fn is_retirement_field(&self) -> bool {
        self.is_bound_for_role(RETIREMENT)
    }
    fn cloned(root: &CandidateRoot, role: usize) -> Result<Self, hex_engine::Invalid> {
        let lease = root
            .0
            .root_fields
            .as_ref()
            .map(|fields| fields.reserve_field_on_control(role))
            .transpose()
            .map_err(|_| hex_engine::Invalid("root actor field unavailable"))?;
        // No fallible work/allocation/callback after reserving the actual field.
        Ok(Self {
            root: Some(root.clone()),
            lease,
        })
    }
    fn owned(root: CandidateRoot, role: usize) -> Result<Self, CandidateRoot> {
        let lease = match root
            .0
            .root_fields
            .as_ref()
            .map(|fields| fields.reserve_field_on_control(role))
            .transpose()
        {
            Ok(lease) => lease,
            Err(_) => return Err(root),
        };
        Ok(Self {
            root: Some(root),
            lease,
        })
    }
}
impl Drop for RootActor {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.as_mut() {
            if lease.begin_actual_field_drop_on_control().is_err() {
                return;
            }
            drop(self.root.take());
            let _ = lease.complete_actual_field_drop_on_control();
        }
    }
}
impl CandidateRoot {
    pub(crate) fn publication_root_field(&self) -> Result<RootActor, hex_engine::Invalid> {
        RootActor::cloned(self, PUBLICATION)
    }
    pub(crate) fn callback_root_field(self) -> Result<RootActor, hex_engine::Invalid> {
        RootActor::owned(self, CALLBACK)
            .map_err(|_| hex_engine::Invalid("callback root field unavailable"))
    }
    pub(crate) fn retirement_root_field(self) -> RootActor {
        match RootActor::owned(self, RETIREMENT) {
            Ok(actor) => actor,
            // Existing infallible retirement construction MUST retain the
            // incoming real Root and terminals. The consumer refuses this
            // unbound field before any destructive step, never a fake receipt.
            Err(root) => RootActor {
                root: Some(root),
                lease: None,
            },
        }
    }
    pub(crate) fn root_fields_requested_bytes() -> Option<usize> {
        NativeRootFieldRegistration::requested_bytes_on_control().ok()
    }
    pub(crate) fn root_fields_scratch_bytes() -> Option<usize> {
        NativeRootFieldRegistration::construction_scratch_bytes_on_control()
            .ok()?
            .checked_add(size_of::<[RootActor; 3]>())
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests {
    use super::*;
    use hex_engine::{AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation};
    #[test]
    fn actual_captured_main_root_pair_counts_each_alias_and_recycles_only_after_normal_drop() {
        let root = CandidateRoot::new_registered(1).unwrap();
        for _ in 0..2 {
            let mut lease = root
                .0
                .root_fields
                .as_ref()
                .unwrap()
                .reserve_captured_main_field_on_control()
                .unwrap();
            let actual = root.clone();
            assert!(
                root.0
                    .root_fields
                    .as_ref()
                    .unwrap()
                    .reserve_captured_main_field_on_control()
                    .is_err()
            );
            root.test_native_root_header_fields().unwrap();
            let free = allocation_counter::measure(|| {
                lease.begin_actual_field_drop_on_control().unwrap();
                drop(actual);
                lease.complete_actual_field_drop_on_control().unwrap();
                drop(lease);
            });
            assert_eq!((free.count_total, free.count_current), (0, 0));
            root.test_native_root_header_fields().unwrap();
        }
    }
    #[test]
    fn actual_root_field_move_release_and_duplicate_refusal_do_not_allocate() {
        let root = CandidateRoot::new_registered(1).unwrap();
        root.test_native_root_header_fields().unwrap();
        let mut actor = None;
        let birth =
            allocation_counter::measure(|| actor = Some(root.publication_root_field().unwrap()));
        assert_eq!((birth.count_total, birth.count_current), (0, 0));
        root.test_native_root_header_fields().unwrap();
        let collision =
            allocation_counter::measure(|| assert!(root.publication_root_field().is_err()));
        assert_eq!((collision.count_total, collision.count_current), (0, 0));
        let moved = actor.take().unwrap();
        assert!(moved.is_publication_field());
        let released = allocation_counter::measure(|| drop(moved));
        assert_eq!((released.count_total, released.count_current), (0, 0));
        root.test_native_root_header_fields().unwrap();
        assert!(
            root.publication_root_field().is_err(),
            "disposed roles never mint replacement field lineage"
        );
    }
    #[test]
    fn actual_three_root_fields_match_only_their_constructor_roles_without_allocation() {
        let root = CandidateRoot::new_registered(1).unwrap();
        let publication = root.publication_root_field().unwrap();
        let callback = root.clone().callback_root_field().unwrap();
        let retirement = root.clone().retirement_root_field();
        let foreign = NativeRootFieldRegistration::new_on_control().unwrap();
        let fields = root.0.root_fields.as_ref().unwrap();
        let measured = allocation_counter::measure(|| {
            assert!(publication.is_publication_field());
            assert!(!publication.is_callback_field());
            assert!(!publication.is_retirement_field());
            assert!(!callback.is_publication_field());
            assert!(callback.is_callback_field());
            assert!(!callback.is_retirement_field());
            assert!(!retirement.is_publication_field());
            assert!(!retirement.is_callback_field());
            assert!(retirement.is_retirement_field());
            for (role, actor) in [&publication, &callback, &retirement]
                .into_iter()
                .enumerate()
            {
                let lease = actor.lease.as_ref().unwrap();
                assert!(fields.matches_actual_lease_on_control(lease));
                assert!(fields.matches_actual_field_role_on_control(lease, role));
                assert!(!fields.matches_actual_field_role_on_control(lease, 3));
                assert!(!fields.matches_actual_field_role_on_control(lease, usize::MAX));
                assert!(!foreign.matches_actual_field_role_on_control(lease, role));
            }
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
        root.test_native_root_header_fields().unwrap();
        drop((publication, callback, retirement));
        root.test_native_root_header_fields().unwrap();
    }
    #[test]
    fn actual_pending_and_failed_root_field_cannot_be_inspected_as_reusable() {
        let root = CandidateRoot::new_registered(1).unwrap();
        let mut actor = root.publication_root_field().unwrap();
        actor
            .lease
            .as_mut()
            .unwrap()
            .begin_actual_field_drop_on_control()
            .unwrap();
        assert_eq!(
            root.test_native_root_header_fields(),
            Err(AudioStorageError::UnknownStorage)
        );
        assert!(root.clone().callback_root_field().is_err());
        // Interrupt the actual field's protocol: Drop cannot complete/retry it.
        drop(actor);
        assert_eq!(
            root.test_native_root_header_fields(),
            Err(AudioStorageError::UnknownStorage)
        );
        assert!(
            root.0
                .root_fields
                .as_ref()
                .unwrap()
                .retained_metadata_bytes_on_control()
                .is_err()
        );
    }
    #[test]
    fn foreign_root_registration_refuses_actual_containment_before_closure() {
        let root = CandidateRoot::new_registered(1).unwrap();
        let foreign = CandidateRoot::new_registered(1).unwrap();
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                data_rows: 16,
                handle_rows: 16,
                root_rows: 4,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            })
            .unwrap();
        assert_eq!(
            ledger.borrow_sampler_native_root_fields_on_control(
                hex_engine::sampler_host_fields::SharedMetadataRef::of(&root.0),
                foreign.0.root_fields.as_ref(),
            ),
            Err(AudioStorageError::SourceMismatch)
        );
        // Failed partial ledgers are not reused as closure evidence.
        root.test_native_root_header_fields().unwrap();
    }
}
