//! Concrete Session adapter payload for the existing lifecycle family hook.
//! No audio/GPUI edge, owning child export, registry admission or byte release.
use super::*;
use std::sync::{RwLock, TryLockError};

/// Exactly one active producer receives this correlation at construction.
/// It carries no family/snapshot owner and cannot fabricate a read capability.
#[derive(Debug)]
pub struct SnapshotProducerKey {
    family: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotFamilyCleanup {
    Pending,
    Disposed,
}
/// Stored once in persistent lifecycle custody before any frame escape.
/// Its private record lock also guards actual inventory through publication.
/// Neither this payload nor its producer key is a native endpoint/device lease.
pub struct SessionExportFamily {
    identity: u64,
    records: RwLock<SnapshotExportRecords>,
    abandoned: AtomicBool,
}
impl SessionExportFamily {
    pub fn try_new_on_control(
        limits: SnapshotExportReservation,
    ) -> Result<(Self, SnapshotProducerKey), ExportRecordError> {
        let overhead = std::mem::size_of::<Self>()
            .checked_sub(std::mem::size_of::<SnapshotExportRecords>())
            .ok_or(ExportRecordError::Reservation)?;
        let mut record_limits = limits;
        record_limits.max_bookkeeping_bytes = limits
            .max_bookkeeping_bytes
            .checked_sub(overhead)
            .ok_or(ExportRecordError::Reservation)?;
        let records = SnapshotExportRecords::try_new_on_control(record_limits)?;
        let key = SnapshotProducerKey {
            family: records.family,
        };
        Ok((
            Self {
                identity: records.family,
                records: RwLock::new(records),
                abandoned: AtomicBool::new(false),
            },
            key,
        ))
    }
    pub fn insert_on_control(
        &self,
        key: &SnapshotProducerKey,
        source: &mut Option<ViewState>,
    ) -> Result<SnapshotStamp, ExportRecordError> {
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if key.family != records.family {
            return Err(ExportRecordError::Stale);
        }
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        records.insert_on_control(source)
    }
    pub fn reserve_read_on_control(
        &self,
        key: &SnapshotProducerKey,
        snapshot: SnapshotStamp,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        if key.family != records.family {
            return Err(ExportRecordError::Stale);
        }
        if self.abandoned.load(Ordering::Acquire) {
            return Err(ExportRecordError::Closed);
        }
        records.reserve_read(snapshot)
    }
    pub fn retire_on_control(
        &self,
        key: &SnapshotProducerKey,
        snapshot: SnapshotStamp,
    ) -> Result<(), ExportRecordError> {
        let mut records = self
            .records
            .try_write()
            .map_err(|_| ExportRecordError::Busy)?;
        if key.family != records.family {
            return Err(ExportRecordError::Stale);
        }
        records.retire_on_control(snapshot)
    }
    pub fn read_with<R>(
        &self,
        stamp: &SnapshotReadStamp,
        read: impl for<'a> FnOnce(SnapshotRead<'a>) -> R,
    ) -> Result<R, ExportRecordError> {
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        records.read_with(stamp, read)
    }
    pub fn derive_read(
        &self,
        stamp: &SnapshotReadStamp,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        self.records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?
            .derive_read(stamp)
    }
    /// A release cannot lose its final stamp just because control is processing
    /// another bounded bookkeeping step. Shared reads do not exclude rendering;
    /// a valid outstanding reader prevents any eligible owning destructor.
    /// No GUI response/native barrier or owning shutdown enqueue is involved.
    pub fn release_read(&self, stamp: SnapshotReadStamp) -> Result<(), ExportRecordError> {
        self.records
            .read()
            .map_err(|_| ExportRecordError::Stale)?
            .release_read(stamp)
    }
    /// Durable producer revocation; cleanup observes it even with lost wakeups.
    pub fn abandon_on_control(&self, key: &SnapshotProducerKey) -> Result<(), ExportRecordError> {
        if key.family != self.identity {
            return Err(ExportRecordError::Stale);
        }
        self.abandoned.store(true, Ordering::Release);
        Ok(())
    }
    /// Scoped actual guard, acquired BEFORE any short native publication barrier.
    /// The read surface exposes queries only. Actual joint inventory/seal work
    /// still needs its typed adapter inside this same protected scope.
    /// ```compile_fail
    /// use hex_session::snapshot_exports::SessionExportFamily;
    /// fn escape(family: &SessionExportFamily) {
    ///     let _escaped = family.inspect_queries_on_control(|view| view);
    /// }
    /// ```
    pub fn inspect_queries_on_control<R>(
        &self,
        read: impl for<'a> FnOnce(SnapshotFamilyQueries<'a>) -> R,
    ) -> Result<R, ExportRecordError> {
        let records = self
            .records
            .try_read()
            .map_err(|_| ExportRecordError::Busy)?;
        Ok(read(SnapshotFamilyQueries { records: &records }))
    }
    /// Called only by the already-registered persistent control adapter. One
    /// eligible snapshot owner Drop maximum; no guard waits for UI. Its caller
    /// must catch a destructor panic and freeze the exact family slot/charge.
    /// A pre-Drop latch prevents retries even outside that registered caller.
    pub fn cleanup_one_on_control(&mut self) -> Result<SnapshotFamilyCleanup, ExportRecordError> {
        let mut records = match self.records.try_write() {
            Ok(records) => records,
            Err(TryLockError::WouldBlock) => return Ok(SnapshotFamilyCleanup::Pending),
            Err(TryLockError::Poisoned(_)) => return Err(ExportRecordError::Stale),
        };
        if self.abandoned.load(Ordering::Acquire) {
            records.close_on_control()?;
        }
        records.collect_releases_on_control()?;
        if records
            .snapshots
            .iter()
            .any(|record| record.cleanup_started && record.snapshot.is_none())
        {
            return Err(ExportRecordError::Stale);
        }
        let eligible = records.snapshots.iter().position(|record| {
            record.retired
                && record.snapshot.is_some()
                && record.readers.load(Ordering::Acquire) == 0
        });
        if let Some(index) = eligible {
            let revision = records
                .revision
                .checked_add(1)
                .ok_or(ExportRecordError::Exhausted)?;
            records.revision = revision;
            records.snapshots[index].cleanup_started = true;
            drop(records.snapshots[index].snapshot.take());
            records.snapshots[index].cleanup_started = false;
        }
        if records.closed
            && records
                .snapshots
                .iter()
                .all(|record| record.snapshot.is_none())
            && records
                .reads
                .iter()
                .all(|read| !read.occupied.load(Ordering::Acquire))
        {
            Ok(SnapshotFamilyCleanup::Disposed)
        } else {
            Ok(SnapshotFamilyCleanup::Pending)
        }
    }
}
/// No records/owners are extractable, including through a held seal scope.
pub struct SnapshotFamilyQueries<'a> {
    records: &'a SnapshotExportRecords,
}
impl<'a> SnapshotFamilyQueries<'a> {
    /// Actual nested records while the enclosing family read guard stays held.
    /// The enclosing holder/table and every other root join the SAME ledger;
    /// this neither supplies a construction quote nor authorizes publication.
    pub fn visit_selected_storage_on_control(
        &self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.records.visit_selected_storage_on_control(ledger)
    }

    pub fn inventory_revision(&self) -> u64 {
        self.records.inventory_revision()
    }
    pub fn snapshot_count(&self) -> usize {
        self.records
            .snapshots
            .iter()
            .filter(|record| record.snapshot.is_some())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limits() -> SnapshotExportReservation {
        SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 2,
            max_bookkeeping_bytes: 16_384,
        }
    }
    fn pending(revision: u64) -> Option<ViewState> {
        let session = crate::Session::new(48_000, 8, 1024).unwrap();
        Some(crate::service::view_state_owned(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            revision,
            1,
            false,
            None,
        ))
    }
    #[test]
    fn actual_family_old_read_survives_abandon_until_durable_release_and_control_drop() {
        let (mut family, producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
        let mut source = pending(3);
        let snapshot = family.insert_on_control(&producer, &mut source).unwrap();
        let read = family.reserve_read_on_control(&producer, snapshot).unwrap();
        family.abandon_on_control(&producer).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Pending)
        );
        assert_eq!(family.read_with(&read, |view| view.revision()), Ok(3));
        let derived = family.derive_read(&read).unwrap();
        family.release_read(read).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Pending)
        );
        assert_eq!(family.read_with(&derived, |view| view.revision()), Ok(3));
        family.release_read(derived).unwrap();
        assert_eq!(
            family.inspect_queries_on_control(|view| view.snapshot_count()),
            Ok(1)
        );
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Disposed)
        );
        assert_eq!(
            family.inspect_queries_on_control(|view| view.snapshot_count()),
            Ok(0)
        );
        assert!(matches!(
            family.reserve_read_on_control(&producer, snapshot),
            Err(ExportRecordError::Closed)
        ));
    }
    #[test]
    fn actual_family_reclaimed_record_reuse_preserves_generation_and_new_source() {
        let (mut family, producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
        let mut old = pending(10);
        let first = family.insert_on_control(&producer, &mut old).unwrap();
        family.retire_on_control(&producer, first).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Pending)
        );
        let mut next = pending(11);
        let second = family.insert_on_control(&producer, &mut next).unwrap();
        assert_eq!(first.index, second.index);
        assert_ne!(first.generation, second.generation);
        assert!(matches!(
            family.reserve_read_on_control(&producer, first),
            Err(ExportRecordError::Stale)
        ));
        let read = family.reserve_read_on_control(&producer, second).unwrap();
        assert_eq!(family.read_with(&read, |view| view.revision()), Ok(11));
        family.release_read(read).unwrap();
        family.abandon_on_control(&producer).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Disposed)
        );
    }
    #[test]
    fn actual_family_guard_refuses_insert_and_abandon_is_durable_without_waiting_for_it() {
        let (family, producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
        let (other, other_producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
        let mut source = pending(5);
        assert_eq!(
            family.insert_on_control(&other_producer, &mut source),
            Err(ExportRecordError::Stale)
        );
        family
            .inspect_queries_on_control(|view| {
                let revision = view.inventory_revision();
                assert_eq!(
                    family.insert_on_control(&producer, &mut source),
                    Err(ExportRecordError::Busy)
                );
                assert!(source.is_some());
                family.abandon_on_control(&producer).unwrap();
                assert_eq!(view.inventory_revision(), revision);
                assert_eq!(view.snapshot_count(), 0);
            })
            .unwrap();
        assert_eq!(
            family.insert_on_control(&producer, &mut source),
            Err(ExportRecordError::Closed)
        );
        assert!(source.is_some());
        assert_eq!(
            other.abandon_on_control(&producer),
            Err(ExportRecordError::Stale)
        );
    }
}
