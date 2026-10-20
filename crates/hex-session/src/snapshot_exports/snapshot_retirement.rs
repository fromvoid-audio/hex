//! The SAME family's persistent snapshot/metadata cleanup destination.
//! Pending precedes both takes; payload destruction runs without the inner lock.
use super::*;

#[derive(Default)]
pub(super) struct SnapshotRetirement {
    family: u64,
    index: usize,
    generation: u64,
    view: Option<SharedMetadata<ViewState>>,
    metadata: Option<SharedMetadata<source_event::SourceEventMetadata>>,
    pending: bool,
    released: bool,
    #[cfg(test)]
    pub(super) panic_after_view: bool,
}
impl SnapshotRetirement {
    #[cfg(test)]
    pub(super) fn retains_metadata(&self) -> bool {
        self.metadata.is_some()
    }
    pub(super) fn vacant(&self) -> bool {
        !self.pending && !self.released && self.view.is_none() && self.metadata.is_none()
    }
    pub(super) fn require_vacant_storage(&self) -> Result<(), hex_engine::AudioStorageError> {
        if self.vacant() {
            Ok(())
        } else {
            Err(hex_engine::AudioStorageError::UnknownStorage)
        }
    }
    pub(super) fn begin(
        &mut self,
        records: &mut SnapshotExportRecords,
    ) -> Result<bool, ExportRecordError> {
        if !self.vacant()
            || records
                .snapshots
                .iter()
                .any(|record| record.cleanup_started)
        {
            return Err(ExportRecordError::Stale);
        }
        let Some(index) = records.snapshots.iter().position(|record| {
            record.retired
                && record.snapshot.is_some()
                && record.readers.load(Ordering::Acquire) == 0
        }) else {
            return Ok(false);
        };
        let revision = records
            .revision
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        let record = records
            .snapshots
            .get_mut(index)
            .ok_or(ExportRecordError::Stale)?;
        self.family = records.family;
        self.index = index;
        self.generation = record.generation;
        self.pending = true; // BEFORE destructive work; never a free-slot receipt.
        record.cleanup_started = true;
        records.revision = revision;
        self.view = record.snapshot.take();
        self.metadata = record.event_metadata.take();
        Ok(true)
    }
    pub(super) fn release(&mut self) {
        drop(self.view.take());
        #[cfg(test)]
        if self.panic_after_view {
            panic!("injected snapshot boundary after view disposal");
        }
        drop(self.metadata.take());
        self.released = true;
    }
    pub(super) fn finish(
        &mut self,
        records: &mut SnapshotExportRecords,
    ) -> Result<(), ExportRecordError> {
        if !self.pending
            || !self.released
            || self.view.is_some()
            || self.metadata.is_some()
            || records.family != self.family
        {
            return Err(ExportRecordError::Stale);
        }
        let record = records
            .snapshots
            .get_mut(self.index)
            .ok_or(ExportRecordError::Stale)?;
        if record.generation != self.generation
            || !record.cleanup_started
            || record.snapshot.is_some()
            || record.event_metadata.is_some()
            || record.readers.load(Ordering::Acquire) != 0
        {
            return Err(ExportRecordError::Stale);
        }
        record.cleanup_started = false;
        self.pending = false;
        self.released = false;
        Ok(())
    }
}
