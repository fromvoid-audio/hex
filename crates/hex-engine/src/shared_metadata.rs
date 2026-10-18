//! Restricted facade over the existing constructor-paired shared owner.
//! Only the fresh sized outer allocation is qualified. Payload children must
//! be visited by their owning module; this is never whole-graph admission.
use crate::owned_shared::OwnedShared;

/// Immutable control-side metadata. Clone/move preserve the receipt beside the
/// actual owner. Allocation and potentially final drops belong on control.
/// There is no raw Arc adoption, mutable borrow or detachable receipt.
///
/// ```compile_fail
/// let raw = std::sync::Arc::new(4_u32);
/// let _: hex_engine::SharedMetadata<u32> = raw.into();
/// ```
/// ```compile_fail
/// let mut owner = hex_engine::SharedMetadata::new_on_control(vec![1_u8]);
/// owner.push(2);
/// ```
/// ```compile_fail
/// let owner = hex_engine::SharedMetadata::new_on_control(4_u32);
/// let _: &std::sync::Arc<u32> = owner.as_ref();
/// ```
#[derive(Debug)]
pub struct SharedMetadata<T> {
    owner: OwnedShared<T>,
}
impl<T> SharedMetadata<T> {
    /// Consume the fresh owned value once through the existing sized shared
    /// constructor. Unsupported layout remains Unknown when inventoried.
    /// Quote old/new/value-temporary overlap before constructing this owner.
    pub fn new_on_control(value: T) -> Self {
        Self {
            owner: OwnedShared::new_owned(value),
        }
    }
    /// Allocation equality for cache reuse only; never source provenance.
    pub fn shares_owner_with(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(self.owner.arc(), other.owner.arc())
    }
}
impl<T> Clone for SharedMetadata<T> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone_on_control(),
        }
    }
}
impl<T> AsRef<T> for SharedMetadata<T> {
    fn as_ref(&self) -> &T {
        self.owner.arc().as_ref()
    }
}
impl<T> std::ops::Deref for SharedMetadata<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.as_ref()
    }
}

impl<'a> crate::AudioRootStorageLedger<'a> {
    /// Register the actual paired outer field and header/body only. This does
    /// not declare any unseen payload child, peak, custody or final retirement.
    pub fn borrow_shared_metadata_body_on_control<T: 'static>(
        &mut self,
        value: &'a SharedMetadata<T>,
    ) -> Result<(), crate::AudioStorageError> {
        use crate::audio_storage::Owner;
        self.mutate(|ledger| {
            let field = Owner::exclusive(value);
            if ledger.contains_shared_field(value) {
                return Ok(());
            }
            ledger.shared(
                value.owner.arc(),
                value.owner.parts(),
                field,
                crate::audio_only_inputs::handle()?,
                None,
            )
        })
    }
    /// Actual requested backing for a borrowed Vec field. Its inline header
    /// belongs to the enclosing body; nested values require their own traversal.
    /// No caller byte count, len estimate or allocator/RSS declaration is used.
    pub fn borrow_metadata_vec_backing_on_control<T: 'static>(
        &mut self,
        value: &'a Vec<T>,
    ) -> Result<(), crate::AudioStorageError> {
        self.mutate(|ledger| ledger.vec(value))
    }
    /// Actual capacity-visible entry backing. Inline entry fields are included
    /// here; owned children still require their module's traversal. The inline
    /// container header belongs to its enclosing owner.
    pub fn borrow_metadata_entries_backing_on_control<K: 'static, V: 'static>(
        &mut self,
        value: &'a hex_project::SortedEntries<K, V>,
    ) -> Result<(), crate::AudioStorageError> {
        self.mutate(|ledger| ledger.entries(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation};
    fn reservation() -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: 16,
            root_rows: 4,
            max_retained_bytes: 65_536,
            max_construction_peak_bytes: 65_536,
        }
    }
    #[test]
    fn shared_metadata_inspection_never_clones_and_distinguishes_alias_from_equal_body() {
        let owner = SharedMetadata::new_on_control([1_u64; 4]);
        let alias = owner.clone();
        let equal = SharedMetadata::new_on_control([1_u64; 4]);
        assert!(owner.shares_owner_with(&alias));
        assert!(!owner.shares_owner_with(&equal));
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        if !crate::pcm_arc::native_layout_known() {
            assert_eq!(
                ledger.borrow_shared_metadata_body_on_control(&owner),
                Err(AudioStorageError::UnknownStorage)
            );
            return;
        }
        crate::owned_shared::reset_clone_count();
        ledger
            .borrow_shared_metadata_body_on_control(&owner)
            .unwrap();
        let bytes = ledger.summary().unwrap().data_bytes;
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        ledger
            .borrow_shared_metadata_body_on_control(&alias)
            .unwrap();
        ledger
            .borrow_shared_metadata_body_on_control(&owner)
            .unwrap();
        assert_eq!(ledger.summary().unwrap().data_bytes, bytes);
        assert_eq!(ledger.selected_shared_counts_on_control(), (1, 2));
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        ledger
            .borrow_shared_metadata_body_on_control(&equal)
            .unwrap();
        assert_eq!(ledger.summary().unwrap().data_bytes, 2 * bytes);
        assert_eq!(crate::owned_shared::clone_count(), 0);
        assert!(ledger.summary().unwrap().require_complete().is_err());
    }
    #[test]
    fn shared_metadata_private_untracked_owner_stays_unknown_even_when_unique() {
        let owner = SharedMetadata {
            owner: OwnedShared::untracked(std::sync::Arc::new([1_u64; 4])),
        };
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        assert_eq!(
            ledger.borrow_shared_metadata_body_on_control(&owner),
            Err(AudioStorageError::UnknownStorage)
        );
        assert!(ledger.summary().is_err());
    }
    thread_local! { static DROPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
    struct Leaf;
    impl Drop for Leaf {
        fn drop(&mut self) {
            DROPS.with(|value| value.set(value.get() + 1));
        }
    }
    #[test]
    fn shared_metadata_old_exports_survive_control_replacement_and_final_drop_occurs_once() {
        DROPS.with(|value| value.set(0));
        let mut current = SharedMetadata::new_on_control(Leaf);
        let old_export = current.clone();
        current = SharedMetadata::new_on_control(Leaf);
        DROPS.with(|value| assert_eq!(value.get(), 0));
        drop(old_export);
        DROPS.with(|value| assert_eq!(value.get(), 1));
        drop(current);
        DROPS.with(|value| assert_eq!(value.get(), 2));
    }
}
