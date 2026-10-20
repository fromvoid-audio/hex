//! Constructor-paired cached sine ownership. No raw Arc adoption or escape.
use crate::{AudioRootStorageLedger, AudioStorageError, SharedMetadata, SineTable};

/// One immutable DSP table. Fresh construction and owning control clones keep
/// the existing sized-owner receipt inseparable; borrowing exposes only the
/// fixed scalar table. Actual fields/other roots and construction overlap must
/// still join admission. This is not a native publication permit.
/// ```compile_fail
/// let raw = std::sync::Arc::new(hex_engine::SineTable::new());
/// let _: hex_engine::SharedSineTable = raw.into();
/// ```
/// ```compile_fail
/// let table = hex_engine::SharedSineTable::new_on_control();
/// let _: &std::sync::Arc<hex_engine::SineTable> = table.as_ref();
/// ```
#[derive(Clone, Debug)]
pub struct SharedSineTable {
    metadata: SharedMetadata<SineTable>,
}
impl SharedSineTable {
    /// One fresh requested shared allocation for this fixed leaf only. Inline
    /// wrapper/body-parent bytes and old/current/native coexistence are the
    /// caller's separate SAME-ceiling obligations, not a new allowance or RSS.
    pub fn fresh_allocation_quote_on_control() -> Result<usize, AudioStorageError> {
        let (payload, header) = crate::owned_shared::OwnedShared::<SineTable>::declared_parts()
            .ok_or(AudioStorageError::UnsupportedConstructor)?;
        payload
            .checked_add(header)
            .ok_or(AudioStorageError::Overflow)
    }
    /// Refuse BEFORE preparing/allocating the table if its requested allocation
    /// exceeds the caller's previously reserved component portion. This does
    /// not replace the whole old/new/input/temporary construction quote.
    pub fn try_new_on_control(max_allocation_bytes: usize) -> Result<Self, AudioStorageError> {
        if Self::fresh_allocation_quote_on_control()? > max_allocation_bytes {
            return Err(AudioStorageError::ReservationExceeded);
        }
        Ok(Self::new_on_control())
    }
    pub fn new_on_control() -> Self {
        Self {
            metadata: SharedMetadata::new_on_control(SineTable::new()),
        }
    }
    /// Reuse optimization only; never an allocation-origin or custody proof.
    pub fn shares_owner_with(&self, other: &Self) -> bool {
        self.metadata.shares_owner_with(&other.metadata)
    }
    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.require_leaf_on_control()?;
        ledger.borrow_shared_metadata_body_on_control(&self.metadata)
    }
    pub(crate) fn require_leaf_on_control(&self) -> Result<(), AudioStorageError> {
        if self.control_storage_declaration()
            != (hex_lattice::PreparedStorageDeclaration::ExclusiveOnly {
                bytes: std::mem::size_of::<SineTable>(),
            })
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(())
    }
    pub(crate) fn metadata_on_control(&self) -> &SharedMetadata<SineTable> {
        &self.metadata
    }
    pub(crate) fn into_native_paired_on_control(
        self,
    ) -> (
        std::sync::Arc<SineTable>,
        crate::owned_shared::SharedDeclaration,
    ) {
        self.metadata.into_native_sine_pair_on_control()
    }
    /// Fixed native-assembly move ONLY: both parts come from the existing
    /// private OwnedShared::into_paired_parts_on_control route, with no owning
    /// clone, allocation, table copy or receipt reattachment by public callers.
    pub(crate) fn from_native_paired_on_control(
        owner: std::sync::Arc<SineTable>,
        receipt: crate::owned_shared::SharedDeclaration,
    ) -> Self {
        Self {
            metadata: SharedMetadata::from_native_sine_pair_on_control(owner, receipt),
        }
    }
}
impl Default for SharedSineTable {
    fn default() -> Self {
        Self::new_on_control()
    }
}
impl AsRef<SineTable> for SharedSineTable {
    fn as_ref(&self) -> &SineTable {
        self.metadata.as_ref()
    }
}
impl std::ops::Deref for SharedSineTable {
    type Target = SineTable;
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn limits() -> crate::AudioStorageLedgerReservation {
        crate::AudioStorageLedgerReservation {
            data_rows: 32,
            handle_rows: 32,
            root_rows: 8,
            max_retained_bytes: 2_000_000,
            max_construction_peak_bytes: 2_000_000,
        }
    }
    #[test]
    fn cached_sine_counts_actual_aliases_independent_equal_tables_and_exact_budget() {
        if !crate::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported sized sine owner; no positive storage proof");
            return;
        }
        eprintln!("SUPPORTED actual paired cached sine and independent equal allocation");
        let first = SharedSineTable::new_on_control();
        let alias = first.clone();
        let independent = SharedSineTable::new_on_control();
        assert!(first.shares_owner_with(&alias));
        assert!(!first.shares_owner_with(&independent));
        let collect = |budget| -> Result<_, AudioStorageError> {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
            first.visit_storage_on_control(&mut ledger)?;
            let one = ledger.summary()?;
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            alias.visit_storage_on_control(&mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            assert_eq!(ledger.summary()?.data_bytes, one.data_bytes);
            independent.visit_storage_on_control(&mut ledger)?;
            let joined = ledger.summary()?;
            assert_eq!(joined.data_allocations, 2);
            assert_eq!(joined.data_bytes, one.data_bytes * 2);
            assert_eq!(ledger.selected_shared_counts_on_control(), (2, 3));
            first.visit_storage_on_control(&mut ledger)?;
            assert_eq!(ledger.summary()?, joined);
            Ok(joined)
        };
        let measured = collect(limits()).unwrap();
        let quote = SharedSineTable::fresh_allocation_quote_on_control().unwrap();
        assert_eq!(measured.data_bytes, quote * 2);
        assert!(matches!(
            SharedSineTable::try_new_on_control(quote - 1),
            Err(AudioStorageError::ReservationExceeded)
        ));
        let exact_owner = SharedSineTable::try_new_on_control(quote).unwrap();
        assert!(!exact_owner.shares_owner_with(&first));
        let mut exact = limits();
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        // Borrowed DSP use keeps the original fixed-table PCM law and owner.
        let left = first.prepare_voice(48_000, 69, 127).unwrap();
        let right = independent.prepare_voice(48_000, 69, 127).unwrap();
        let mut previous = 0.0;
        let mut crossings = 0;
        let mut energy = 0.0;
        for age in 0..48_000 {
            let value = first.sample(left, age, 48_000);
            assert_eq!(value, independent.sample(right, age, 48_000));
            assert!(value.is_finite());
            crossings += usize::from(previous <= 0.0 && value > 0.0);
            energy += f64::from(value).powi(2);
            previous = value;
        }
        assert_eq!(crossings, 440);
        assert!((0.06..0.075).contains(&(energy / 48_000.0).sqrt()));
        assert_eq!(first.sample(left, 100_000, 48_000), 0.0);
    }
    #[test]
    fn cached_sine_native_pair_bridge_preserves_actual_allocation_and_unknown_refusal() {
        if !crate::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported native paired sine bridge; no positive storage proof");
            return;
        }
        eprintln!("SUPPORTED actual private sine paired install and teardown moves");
        let owner = crate::owned_shared::OwnedShared::new_owned(SineTable::new());
        let (arc, receipt) = owner.into_paired_parts_on_control();
        let address = std::sync::Arc::as_ptr(&arc);
        let paired = SharedSineTable::from_native_paired_on_control(arc, receipt);
        {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
            paired.visit_storage_on_control(&mut ledger).unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
        }
        let (arc, receipt) = paired.into_native_paired_on_control();
        assert_eq!(std::sync::Arc::as_ptr(&arc), address);
        assert_eq!(std::sync::Arc::strong_count(&arc), 1);
        let paired = SharedSineTable::from_native_paired_on_control(arc, receipt);
        assert_eq!(
            std::sync::Arc::as_ptr(paired.metadata_on_control().paired_owner_on_control().arc()),
            address
        );
        let raw =
            crate::owned_shared::OwnedShared::untracked(std::sync::Arc::new(SineTable::new()));
        let (arc, receipt) = raw.into_paired_parts_on_control();
        let refused = SharedSineTable::from_native_paired_on_control(arc, receipt);
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        assert_eq!(
            refused.visit_storage_on_control(&mut ledger),
            Err(AudioStorageError::UnknownStorage)
        );
    }
}
