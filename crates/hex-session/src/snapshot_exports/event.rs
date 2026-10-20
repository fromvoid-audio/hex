//! One bounded worker-event transition, before transport binding/channel escape.
//! Both physical slots are selected before the caller's pending view is consumed.
use super::*;

// Never exported as a reservation/approval. The actual family writer owns this
// selection until insertion or normal guard release; no record has moved yet.
pub(super) struct EventDestination {
    snapshot_index: usize,
    snapshot_generation: u64,
    read_index: usize,
    read_generation: u64,
    revision: u64,
}

impl SnapshotExportRecords {
    pub(super) fn event_destination_on_control(
        &self,
    ) -> Result<EventDestination, ExportRecordError> {
        if self.closed {
            return Err(ExportRecordError::Closed);
        }
        let (snapshot_index, record) = self
            .snapshots
            .iter()
            .enumerate()
            .find(|(_, record)| record.snapshot.is_none() && record.event_metadata.is_none())
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
            .iter()
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

        Ok(EventDestination {
            snapshot_index,
            snapshot_generation,
            read_index,
            read_generation,
            revision,
        })
    }

    pub(super) fn insert_event_at_destination_on_control(
        &mut self,
        destination: EventDestination,
        source: &mut Option<ViewState>,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        let EventDestination {
            snapshot_index,
            snapshot_generation,
            read_index,
            read_generation,
            revision,
        } = destination;
        // The selection has never left the same exclusive record guard. Only
        // these private methods can create/use it; no structural mutation was
        // exposed between selection, complete inspection and this insertion.
        let record = self
            .snapshots
            .get_mut(snapshot_index)
            .ok_or(ExportRecordError::Stale)?;
        let read = self.reads.get(read_index).ok_or(ExportRecordError::Stale)?;

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

    pub(super) fn insert_source_event_at_destination_on_control(
        &mut self,
        destination: EventDestination,
        source: &mut Option<ViewState>,
        metadata: &mut Option<super::source_event::SourceEventMetadata>,
    ) -> Result<SnapshotReadStamp, ExportRecordError> {
        use hex_engine::sampler_host_fields::SharedMetadataRef;
        if source.is_none() || metadata.is_none() {
            return Err(ExportRecordError::Stale);
        }
        // Layout qualification only. Whole caller/children/old+new/scratch and
        // BOTH caps were checked by the closed constructor, not this helper.
        SharedMetadataRef::constructor_bytes::<ViewState>()
            .map_err(|_| ExportRecordError::Reservation)?;
        SharedMetadataRef::constructor_bytes::<super::source_event::SourceEventMetadata>()
            .map_err(|_| ExportRecordError::Reservation)?;
        let EventDestination {
            snapshot_index,
            snapshot_generation,
            read_index,
            read_generation,
            revision,
        } = destination;
        let record = self
            .snapshots
            .get_mut(snapshot_index)
            .ok_or(ExportRecordError::Stale)?;
        let read = self.reads.get(read_index).ok_or(ExportRecordError::Stale)?;
        if record.cleanup_started
            || record.snapshot.is_some()
            || record.event_metadata.is_some()
            || record.readers.load(Ordering::Acquire) != 0
            || read.occupied.load(Ordering::Acquire)
        {
            return Err(ExportRecordError::Stale);
        }
        // Private SAME-writer destination, all fallible work before the first
        // take. Interrupted birth remains nonreusable/charged, never vacant.
        record.cleanup_started = true;
        let (view, event) = match (source.take(), metadata.take()) {
            (Some(view), Some(event)) => (view, event),
            (view, event) => {
                *source = view;
                *metadata = event;
                record.cleanup_started = false;
                return Err(ExportRecordError::Stale);
            }
        };
        record.snapshot = Some(SharedMetadata::new_on_control(view));
        record.event_metadata = Some(SharedMetadata::new_on_control(event));
        record.generation = snapshot_generation;
        record.retired = true;
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

    #[cfg(test)]
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
        let destination = self.event_destination_on_control()?;
        self.insert_event_at_destination_on_control(destination, source)
    }
}

#[cfg(test)]
mod tests;
