//! One bounded worker-event transition, before transport binding/channel escape.
//! Both physical slots are selected before the caller's pending view is consumed.
use super::*;

impl SnapshotExportRecords {
    pub(super) fn insert_event_read_on_control(
        &mut self,
        source: &mut Option<ViewState>,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        if self.closed {
            return Err(ExportRecordError::Closed);
        }
        if source.is_none() {
            return Err(ExportRecordError::Stale);
        }
        let (snapshot_index, record) = self
            .snapshots
            .iter_mut()
            .enumerate()
            .find(|(_, record)| record.snapshot.is_none())
            .ok_or(ExportRecordError::Reservation)?;
        // A consumed/failed cleanup is not a reusable vacant snapshot.
        if record.cleanup_started || record.readers.load(Ordering::Acquire) != 0 {
            return Err(ExportRecordError::Stale);
        }
        let snapshot_generation = record
            .generation
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        let (read_index, read) = self
            .reads
            .iter_mut()
            .enumerate()
            .find(|(_, read)| {
                !read.occupied.load(Ordering::Acquire)
                    && read.generation.load(Ordering::Acquire) != u64::MAX
            })
            .ok_or(ExportRecordError::Reservation)?;
        let read_generation = read
            .generation
            .load(Ordering::Acquire)
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;

        // Exclusive records access (the actual inner writer in the family API)
        // excludes derive/insertion/cleanup. No fallible slot/generation step
        // remains after this take. Fresh paired allocation/children and its
        // old/new/input construction overlap require the caller's prior quote.
        let view = source.take().ok_or(ExportRecordError::Stale)?;
        record.snapshot = Some(SharedMetadata::new_on_control(view));
        record.generation = snapshot_generation;
        record.retired = true; // cleanup eligible only AFTER all durable ACKs
        record.readers.store(1, Ordering::Release);
        record.cleanup_started = false;
        self.revision = revision;
        read.generation.store(read_generation, Ordering::Relaxed);
        read.snapshot_index.store(snapshot_index, Ordering::Relaxed);
        read.snapshot_generation
            .store(snapshot_generation, Ordering::Relaxed);
        read.released.store(false, Ordering::Relaxed);
        read.occupied.store(true, Ordering::Release);
        Ok(SnapshotReadStamp {
            family: self.family,
            index: read_index,
            generation: read_generation,
            snapshot: SnapshotStamp {
                family: self.family,
                index: snapshot_index,
                generation: snapshot_generation,
            },
        })
    }
}

#[cfg(test)]
mod tests;
