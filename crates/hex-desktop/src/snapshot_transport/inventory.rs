//! Concrete closed Session-family inventory adapter, never UI owner access or
//! a reusable admission permit. Publication must join other roots/real peak and
//! use the EXISTING source/cancellation linearization under its barrier LAST.
use hex_audio::{RetainedFamilyError, RetainedFamilyInventory, RetainedFamilyInventoryIncarnation};
use hex_engine::{AudioRootStorageLedger, AudioStorageError};
use hex_session::snapshot_exports::{
    ExportRecordError, SessionExportFamily, SnapshotFamilyInventoryGuard,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotFamilyInventoryError {
    Reservation,
    Allocation,
    Busy,
    Stale,
    Failed,
    Unsupported,
}
impl From<RetainedFamilyError> for SnapshotFamilyInventoryError {
    fn from(error: RetainedFamilyError) -> Self {
        match error {
            RetainedFamilyError::Reservation | RetainedFamilyError::Exhausted => Self::Reservation,
            RetainedFamilyError::Allocation => Self::Allocation,
            RetainedFamilyError::Busy => Self::Busy,
            RetainedFamilyError::Stale => Self::Stale,
            RetainedFamilyError::Failed => Self::Failed,
        }
    }
}
impl From<ExportRecordError> for SnapshotFamilyInventoryError {
    fn from(error: ExportRecordError) -> Self {
        match error {
            ExportRecordError::Reservation | ExportRecordError::Exhausted => Self::Reservation,
            ExportRecordError::Allocation => Self::Allocation,
            ExportRecordError::Busy => Self::Busy,
            ExportRecordError::Stale | ExportRecordError::Closed => Self::Stale,
        }
    }
}
/// Nonowning view over SEPARATE local lists. No guard/payload/owner extraction.
/// All actual outer/inner guards remain on the acquiring thread until the
/// continuation ends; the caller declares its ledger inside that scope.
pub struct SnapshotFamilyInventory<'a> {
    outer: &'a RetainedFamilyInventory,
    inner: &'a Vec<SnapshotFamilyInventoryGuard<'a>>,
    scratch_bytes: usize,
}
impl SnapshotFamilyInventory<'_> {
    pub fn configured_slots(&self) -> usize {
        self.outer.slot_count()
    }
    pub fn active_families(&self) -> usize {
        self.inner.len()
    }
    /// Actual guard scratch including local list headers. The parent control
    /// quote must cover it; ledger traversal below charges backing allocations.
    pub fn requested_scratch_bytes(&self) -> usize {
        self.scratch_bytes
    }
    /// Observe actual preparation correlations under these guards. No scalar
    /// comparison replaces native source/candidate/cancel publication checks.
    pub fn incarnations_and_revisions(
        &self,
    ) -> impl Iterator<Item = (RetainedFamilyInventoryIncarnation, u64)> + '_ {
        self.outer
            .slots()
            .filter(|slot| slot.job().is_some())
            .map(|slot| slot.incarnation())
            .zip(self.inner.iter().map(|guard| guard.inventory_revision()))
    }
    /// Preparation correlation check while ACTUAL guards remain held. This is
    /// not an admission permit or replacement for source/candidate/cancellation
    /// validation at the existing native publication linearization point.
    pub fn check_prepared_revisions(
        &self,
        expected: &[(RetainedFamilyInventoryIncarnation, u64)],
    ) -> Result<(), SnapshotFamilyInventoryError> {
        if self
            .incarnations_and_revisions()
            .eq(expected.iter().copied())
        {
            Ok(())
        } else {
            Err(SnapshotFamilyInventoryError::Stale)
        }
    }
    pub fn visit_selected_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.outer.visit_backings_on_control(ledger)?;
        // This is the REAL non-static Vec of real borrowed inner guards, using
        // the same exclusive Vec field domain as ordinary static Vec backing.
        ledger.borrow_metadata_vec_backing_on_control(self.inner)?;
        for slot in self.outer.slots() {
            slot.validate_intact_custody_on_control()
                .map_err(|_| AudioStorageError::UnknownStorage)?;
            if let Some(job) = slot.job() {
                job.visit_outer_storage_on_control(ledger)?;
            }
        }
        for guard in self.inner {
            guard.visit_selected_storage_on_control(ledger)?;
        }
        Ok(())
    }
}
fn inner_scratch(capacity: usize) -> Result<usize, SnapshotFamilyInventoryError> {
    capacity
        .checked_mul(std::mem::size_of::<SnapshotFamilyInventoryGuard<'_>>())
        .and_then(|bytes| {
            bytes.checked_add(std::mem::size_of::<Vec<SnapshotFamilyInventoryGuard<'_>>>())
        })
        .ok_or(SnapshotFamilyInventoryError::Reservation)
}
/// max_scratch_bytes comes from the SAME retained/old-new-temporary ceiling,
/// not another DATA allowance. All allocation/list growth finishes before the
/// ledger or any native barrier. Reserved/failed/unknown families REFUSE before
/// invoking the continuation; existing custody/cancellation remains unchanged.
pub fn with_snapshot_family_inventory_on_control<R>(
    max_scratch_bytes: usize,
    inspect: impl for<'a> FnOnce(SnapshotFamilyInventory<'a>) -> R,
) -> Result<R, SnapshotFamilyInventoryError> {
    let (slots, outer_requested) =
        hex_audio::retained_family_inventory_reservation_quote_on_control()?;
    let inner_requested = inner_scratch(slots)?;
    let requested = outer_requested
        .checked_add(inner_requested)
        .filter(|bytes| *bytes <= max_scratch_bytes)
        .ok_or(SnapshotFamilyInventoryError::Reservation)?;
    let _ = requested; // conservative checked quote, not observed historical peak
    // Separate locals: outer guards first, then full inner list; caller's ledger
    // is created last in inspect and ends before these lists can be dropped.
    let outer = hex_audio::try_retained_family_inventory_on_control(
        max_scratch_bytes
            .checked_sub(inner_requested)
            .ok_or(SnapshotFamilyInventoryError::Reservation)?,
    )?;
    let mut inner = Vec::new();
    inner
        .try_reserve_exact(slots)
        .map_err(|_| SnapshotFamilyInventoryError::Allocation)?;
    let scratch_bytes = outer
        .requested_scratch_bytes()?
        .checked_add(inner_scratch(inner.capacity())?)
        .filter(|bytes| *bytes <= max_scratch_bytes)
        .ok_or(SnapshotFamilyInventoryError::Reservation)?;
    for slot in outer.slots() {
        slot.validate_intact_custody_on_control()?;
        if let Some(job) = slot.job() {
            let family = job
                .borrow_payload_on_control::<SessionExportFamily>()
                .ok_or(SnapshotFamilyInventoryError::Unsupported)?;
            // At most one inner per configured outer slot; capacity was fully
            // reserved and observed BEFORE any inner guard was acquired.
            inner.push(family.try_inventory_guard_on_control()?);
        }
    }
    Ok(inspect(SnapshotFamilyInventory {
        outer: &outer,
        inner: &inner,
        scratch_bytes,
    }))
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests;
