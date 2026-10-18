//! Restricted facade over the existing constructor-paired shared owner.
//! Only the fresh sized outer allocation is qualified. Payload children must
//! be visited by their owning module; this is never whole-graph admission.
use crate::owned_shared::OwnedShared;
use std::{
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_CACHE_IDENTITY: AtomicU64 = AtomicU64::new(1);

/// Nonowning immutable cache version. Equality permits geometry reuse only;
/// this retains no allocation and proves no storage provenance or lifetime.
/// Values cannot be fabricated, adopted from an address, or reused after wrap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
///
/// ```compile_fail
/// let _ = hex_engine::SharedMetadataCacheIdentity(1);
/// ```
pub struct SharedMetadataCacheIdentity(NonZeroU64);

fn next_cache_identity(counter: &AtomicU64) -> Option<SharedMetadataCacheIdentity> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .ok()
        .and_then(NonZeroU64::new)
        .map(SharedMetadataCacheIdentity)
}

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
    cache_identity: Option<SharedMetadataCacheIdentity>,
}
impl<T> SharedMetadata<T> {
    /// Consume the fresh owned value once through the existing sized shared
    /// constructor. Unsupported layout remains Unknown when inventoried.
    /// Quote old/new/value-temporary overlap before constructing this owner.
    pub fn new_on_control(value: T) -> Self {
        Self {
            owner: OwnedShared::new_owned(value),
            cache_identity: next_cache_identity(&NEXT_CACHE_IDENTITY),
        }
    }
    /// A stable version shared only by clones of this exact immutable owner.
    /// Exhaustion disables cache reuse; it never wraps or invents provenance.
    /// The enclosing wrapper's actual size includes this inline scalar.
    pub fn cache_identity(&self) -> Option<SharedMetadataCacheIdentity> {
        self.cache_identity
    }
    /// Allocation equality for cache reuse only; never source provenance.
    pub fn shares_owner_with(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(self.owner.arc(), other.owner.arc())
    }
}
// Preserve value equality semantics of the previous Arc<T> consumers.
// Content equality is never allocation identity or storage provenance.
impl<T: PartialEq> PartialEq for SharedMetadata<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}
impl<T: Eq> Eq for SharedMetadata<T> {}
impl<T> Clone for SharedMetadata<T> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone_on_control(),
            cache_identity: self.cache_identity,
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

// Narrow private bridge from the ACTUAL paired prepared-revision field. This
// clones the existing receipt WITH its owner; raw/untracked parts stay Unknown.
// It neither adopts an Arc nor adds a generic public owner/receipt constructor.
impl SharedMetadata<crate::MidiArrangementPreview> {
    pub(crate) fn clone_prepared_midi_on_control(
        owner: &OwnedShared<crate::MidiArrangementPreview>,
    ) -> Self {
        Self {
            owner: owner.clone_on_control(),
            // The older prepared owner has no stored cache-version cell. Do
            // not derive one from its address or create a second version map.
            cache_identity: None,
        }
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
    /// Actual elements may borrow control guards; no Any/static widening occurs.
    /// ```compile_fail
    /// let mut values = vec![1_u8];
    /// let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
    ///     hex_engine::AudioStorageLedgerReservation { data_rows: 8, handle_rows: 8,
    ///         root_rows: 4, max_retained_bytes: 100000, max_construction_peak_bytes: 100000 }).unwrap();
    /// ledger.borrow_metadata_vec_backing_on_control(&values).unwrap();
    /// values.push(2);
    /// let _ = ledger.summary();
    /// ```
    /// ```compile_fail
    /// let values = vec![1_u8];
    /// let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
    ///     hex_engine::AudioStorageLedgerReservation { data_rows: 8, handle_rows: 8,
    ///         root_rows: 4, max_retained_bytes: 100000, max_construction_peak_bytes: 100000 }).unwrap();
    /// ledger.borrow_metadata_vec_backing_on_control(&values).unwrap();
    /// drop(values);
    /// let _ = ledger.summary();
    /// ```
    pub fn borrow_metadata_vec_backing_on_control<T>(
        &mut self,
        value: &'a Vec<T>,
    ) -> Result<(), crate::AudioStorageError> {
        self.mutate(|ledger| ledger.vec(value))
    }
    /// Actual Unix PathBuf backing capacity, including empty reserved storage.
    /// On the supported pinned Linux/macOS targets PathBuf -> OsString ->
    /// sys::os_str::bytes::Buf owns Vec<u8>; inline headers belong to the parent.
    /// Other targets refuse rather than estimating encoded length or layout.
    pub fn borrow_metadata_path_backing_on_control(
        &mut self,
        value: &'a std::path::PathBuf,
    ) -> Result<(), crate::AudioStorageError> {
        if !cfg!(any(
            all(
                target_arch = "x86_64",
                target_os = "linux",
                target_env = "gnu"
            ),
            all(target_arch = "aarch64", target_os = "macos"),
        )) || !crate::audio_storage_constructor_layout_supported()
        {
            return Err(crate::AudioStorageError::UnknownStorage);
        }
        self.mutate(|ledger| {
            ledger.bytes(
                crate::audio_storage::Owner::exclusive(value),
                crate::audio_storage::Location("unix-path-buffer", 0),
                value.capacity(),
            )
        })
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
    #[test]
    fn cache_versions_are_nonowning_unique_and_never_wrap() {
        let owner = SharedMetadata::new_on_control([3_u8; 8]);
        let version = owner.cache_identity().unwrap();
        let alias = owner.clone();
        assert_eq!(alias.cache_identity(), Some(version));
        let equal = SharedMetadata::new_on_control([3_u8; 8]);
        assert_ne!(equal.cache_identity(), Some(version));
        let count = std::sync::Arc::strong_count(owner.owner.arc());
        for _ in 0..128 {
            assert_eq!(owner.cache_identity(), Some(version));
        }
        assert_eq!(std::sync::Arc::strong_count(owner.owner.arc()), count);
        drop(alias);
        drop(owner);
        // An old nonowning cache key cannot match any later allocation, even
        // if an allocator subsequently reuses the same address.
        assert_ne!(
            SharedMetadata::new_on_control([3_u8; 8]).cache_identity(),
            Some(version)
        );
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(
            next_cache_identity(&counter),
            Some(SharedMetadataCacheIdentity(
                NonZeroU64::new(u64::MAX - 1).unwrap()
            ))
        );
        assert_eq!(next_cache_identity(&counter), None);
        assert_eq!(next_cache_identity(&counter), None);
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn a_retained_cache_version_does_not_retain_the_payload() {
        DROPS.with(|drops| drops.set(0));
        let version = {
            let owner = SharedMetadata::new_on_control(Leaf);
            owner.cache_identity().unwrap()
        };
        DROPS.with(|drops| assert_eq!(drops.get(), 1));
        // Copying/retaining the cache key has no owner and cannot delay Drop.
        let retained_key = version;
        assert_eq!(retained_key, version);
        DROPS.with(|drops| assert_eq!(drops.get(), 1));
    }

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
            cache_identity: None,
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
