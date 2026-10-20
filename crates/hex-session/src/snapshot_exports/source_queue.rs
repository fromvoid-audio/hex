//! Capacity-two Event transport inside the EXISTING family's exclusion/body.
//! Entries retain actual reserved reads, not owners or copied admission quotes.
use super::*;

pub(super) const CAPACITY: usize = 2;

#[derive(Default)]
pub(super) struct SourceEventQueue {
    entries: [Option<Entry>; CAPACITY],
    sequence: u64,
}

struct Entry {
    sequence: u64,
    canonical: SnapshotReadStamp,
    device: Option<SnapshotReadStamp>,
    tuning: Option<SnapshotReadStamp>,
}
/// Correlation of the still-retained head. Copying this is no read/admission
/// authority: claim validates the SAME actual head and every reserved read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceEventHead {
    family: u64,
    sequence: u64,
}
/// Exact transferred reads, including an optional pre-reserved canonical alias
/// for current+pending installation. No owning model or runtime is transported.
pub struct SourceEventDelivery {
    pub(crate) canonical: SnapshotReadStamp,
    pub(crate) canonical_pending: Option<SnapshotReadStamp>,
    pub(crate) device: Option<SnapshotReadStamp>,
    pub(crate) tuning: Option<SnapshotReadStamp>,
}
impl SnapshotExportRecords {
    fn validate_source_entry(&self, entry: &Entry) -> Result<(), ExportRecordError> {
        self.snapshot_for_read(&entry.canonical)?;
        let metadata = self
            .record(entry.canonical.snapshot)?
            .event_metadata
            .as_ref()
            .ok_or(ExportRecordError::Stale)?;
        if entry.device.is_some() != metadata.facts.device_preview.is_some()
            || entry.tuning.is_some() != metadata.facts.tuning_preview.is_some()
        {
            return Err(ExportRecordError::Stale);
        }
        let reads = [
            Some(&entry.canonical),
            entry.device.as_ref(),
            entry.tuning.as_ref(),
        ];
        for (index, read) in reads.iter().enumerate() {
            let Some(read) = read else { continue };
            self.snapshot_for_read(read)?;
            for other in reads.iter().take(index).flatten() {
                if read.index == other.index {
                    return Err(ExportRecordError::Stale);
                }
            }
        }
        Ok(())
    }
    pub(super) fn validate_source_events_on_control(&self) -> Result<(), ExportRecordError> {
        if self.source_events.entries[0].is_none() && self.source_events.entries[1].is_some() {
            return Err(ExportRecordError::Stale);
        }
        let mut previous = 0;
        for entry in self.source_events.entries.iter().flatten() {
            if entry.sequence <= previous || entry.sequence > self.source_events.sequence {
                return Err(ExportRecordError::Stale);
            }
            self.validate_source_entry(entry)?;
            previous = entry.sequence;
        }
        Ok(())
    }
    pub(super) fn enqueue_source_event(
        &mut self,
        canonical: &mut Option<SnapshotReadStamp>,
        device: &mut Option<SnapshotReadStamp>,
        tuning: &mut Option<SnapshotReadStamp>,
    ) -> Result<(), ExportRecordError> {
        if self.closed {
            return Err(ExportRecordError::Closed);
        }
        self.validate_source_events_on_control()?;
        let index = self
            .source_events
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(ExportRecordError::Reservation)?;
        let sequence = self
            .source_events
            .sequence
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        // Validate actual role reads before consuming any producer field.
        let current = canonical.as_ref().ok_or(ExportRecordError::Stale)?;
        self.snapshot_for_read(current)?;
        let metadata = self
            .record(current.snapshot)?
            .event_metadata
            .as_ref()
            .ok_or(ExportRecordError::Stale)?;
        if device.is_some() != metadata.facts.device_preview.is_some()
            || tuning.is_some() != metadata.facts.tuning_preview.is_some()
        {
            return Err(ExportRecordError::Stale);
        }
        let reads = [Some(current), device.as_ref(), tuning.as_ref()];
        for (position, read) in reads.iter().enumerate() {
            let Some(read) = read else { continue };
            self.snapshot_for_read(read)?;
            if reads
                .iter()
                .take(position)
                .flatten()
                .any(|other| other.index == read.index)
            {
                return Err(ExportRecordError::Stale);
            }
        }
        let canonical = canonical.take().ok_or(ExportRecordError::Stale)?;
        self.source_events.entries[index] = Some(Entry {
            sequence,
            canonical,
            device: device.take(),
            tuning: tuning.take(),
        });
        self.source_events.sequence = sequence;
        self.revision = revision;
        Ok(())
    }
    pub(super) fn source_event_head(&self) -> Result<Option<SourceEventHead>, ExportRecordError> {
        if self.closed {
            return Err(ExportRecordError::Closed);
        }
        self.validate_source_events_on_control()?;
        Ok(self.source_events.entries[0]
            .as_ref()
            .map(|entry| SourceEventHead {
                family: self.family,
                sequence: entry.sequence,
            }))
    }
    pub(super) fn read_source_event_head<R>(
        &self,
        read: impl for<'a> FnOnce(SourceEventHead, SnapshotRead<'a>) -> R,
    ) -> Result<Option<R>, ExportRecordError> {
        let Some(head) = self.source_event_head()? else {
            return Ok(None);
        };
        let entry = self.source_events.entries[0]
            .as_ref()
            .ok_or(ExportRecordError::Stale)?;
        self.read_with(&entry.canonical, |snapshot| read(head, snapshot))
            .map(Some)
    }
    pub(super) fn read_source_event_following<R>(
        &self,
        pending: Option<(SourceEventHead, &SnapshotReadStamp)>,
        previous: &SnapshotReadStamp,
        generation: u64,
        read: impl for<'a> FnOnce(SourceEventHead, SourceEventSourceRelation, SnapshotRead<'a>) -> R,
    ) -> Result<Option<R>, ExportRecordError> {
        let (head, current) = if let Some((head, current)) = pending {
            // Only the actual receiver supplies its still-owned claim pair.
            // It may precede the current queue head after a Busy bind.
            if head.family != self.family
                || head.sequence == 0
                || head.sequence > self.source_events.sequence
                || self.record(current.snapshot)?.event_metadata.is_none()
            {
                return Err(ExportRecordError::Stale);
            }
            (head, current)
        } else {
            let Some(head) = self.source_event_head()? else {
                return Ok(None);
            };
            let entry = self.source_events.entries[0]
                .as_ref()
                .ok_or(ExportRecordError::Stale)?;
            (head, &entry.canonical)
        };
        let current_view = self.snapshot_for_read(current)?;
        let previous_view = self.snapshot_for_read(previous)?;
        let relation = SourceEventSourceRelation::between(previous_view, current_view, generation);
        // Both actual views and the queue/claim remain borrowed under the SAME
        // existing record guard. No claim/derive/ACK or allocation occurs.
        self.read_with(current, |snapshot| read(head, relation, snapshot))
            .map(Some)
    }
    pub(super) fn receive_source_event(
        &mut self,
        head: SourceEventHead,
        canonical_pending: bool,
        destination: &mut Option<SourceEventDelivery>,
    ) -> Result<(), ExportRecordError> {
        if destination.is_some() || self.source_event_head()? != Some(head) {
            return Err(ExportRecordError::Stale);
        }
        // ACK bits must become actual vacancies BEFORE fanout reservation.
        self.collect_releases_on_control()?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(ExportRecordError::Exhausted)?;
        let entry = self.source_events.entries[0]
            .as_ref()
            .ok_or(ExportRecordError::Stale)?;
        let extra = if canonical_pending {
            Some(self.derive_read(&entry.canonical)?)
        } else {
            None
        };
        // No callback, allocation, fallible reserve or owning Drop follows the
        // successful extra reservation. All source owners remain in records.
        let Some(entry) = self.source_events.entries[0].take() else {
            if let Some(extra) = extra {
                self.release_read(extra)?;
            }
            return Err(ExportRecordError::Stale);
        };
        self.source_events.entries[0] = self.source_events.entries[1].take();
        *destination = Some(SourceEventDelivery {
            canonical: entry.canonical,
            canonical_pending: extra,
            device: entry.device,
            tuning: entry.tuning,
        });
        self.revision = revision;
        Ok(())
    }
    pub(super) fn abandon_source_events(&mut self) -> Result<(), ExportRecordError> {
        // Validate ALL before consuming any. Releases merely mark durable ACKs;
        // actual payload destructors run later outside the inner writer.
        self.validate_source_events_on_control()?;
        for entry in self.source_events.entries.iter().flatten() {
            self.read_record(&entry.canonical)?
                .released
                .store(true, Ordering::Release);
            for read in [entry.device.as_ref(), entry.tuning.as_ref()]
                .into_iter()
                .flatten()
            {
                self.read_record(read)?
                    .released
                    .store(true, Ordering::Release);
            }
        }
        self.source_events.entries = [None, None];
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(
        family: &SessionExportFamily,
        key: &SnapshotProducerKey,
        session: &crate::Session,
        revision: u64,
    ) -> SnapshotReadStamp {
        let mut view = Some(crate::service::view_state_owned(
            session,
            hex_engine::empty_midi_preview_on_control(),
            revision,
            1,
            false,
            None,
        ));
        let mut metadata = Some(super::source_event::SourceEventMetadata {
            facts: SourceEventFacts {
                request_complete: true,
                ..Default::default()
            },
            status: Some(format!("event {revision}")),
            error: None,
            audio_batch_failure: None,
            gesture_ack: None,
            tuning_gesture_ack: None,
            tuning_library: None,
        });
        family
            .try_event_construction_on_control(key)
            .unwrap()
            .insert_source_on_control(&mut view, &mut metadata)
            .unwrap()
    }
    fn family(reads: usize) -> (SessionExportFamily, SnapshotProducerKey) {
        SessionExportFamily::try_new_on_control(SnapshotExportReservation {
            snapshot_records: 4,
            read_records: reads,
            max_bookkeeping_bytes: 16_384,
        })
        .unwrap()
    }
    #[test]
    fn retained_head_full_and_fanout_refuse_before_move_then_collect_actual_ack() {
        let (mut family, key) = family(3);
        let session = crate::Session::new(48_000, 1, 1024).unwrap();
        let mut first = Some(source(&family, &key, &session, 1));
        let mut second = Some(source(&family, &key, &session, 2));
        let mut third = Some(source(&family, &key, &session, 3));
        family
            .enqueue_source_event_on_control(&key, &mut first, &mut None, &mut None)
            .unwrap();
        family
            .enqueue_source_event_on_control(&key, &mut second, &mut None, &mut None)
            .unwrap();
        let identity = std::ptr::from_ref(third.as_ref().unwrap());
        assert_eq!(
            family.enqueue_source_event_on_control(&key, &mut third, &mut None, &mut None),
            Err(ExportRecordError::Reservation)
        );
        assert_eq!(identity, std::ptr::from_ref(third.as_ref().unwrap()));
        let head = family
            .read_source_event_head_on_control(|head, read| {
                assert_eq!(read.revision(), 1);
                assert_eq!(read.event_metadata().unwrap().status(), Some("event 1"));
                head
            })
            .unwrap()
            .unwrap();
        let mut destination = None;
        assert_eq!(
            family.receive_source_event_on_control(head, true, &mut destination),
            Err(ExportRecordError::Reservation)
        );
        assert!(destination.is_none());
        assert_eq!(family.source_event_head_on_control().unwrap(), Some(head));
        // Mark-only ACK is occupied until receive's actual collector runs.
        family.release_read(third.take().unwrap()).unwrap();
        family
            .receive_source_event_on_control(head, true, &mut destination)
            .unwrap();
        let delivery = destination.take().unwrap();
        assert_eq!(
            family.read_with(&delivery.canonical, |read| read.revision()),
            Ok(1)
        );
        assert_eq!(
            family.read_with(delivery.canonical_pending.as_ref().unwrap(), |read| read
                .revision()),
            Ok(1)
        );
        assert!(delivery.device.is_none() && delivery.tuning.is_none());
        assert_eq!(
            family.receive_source_event_on_control(head, false, &mut destination),
            Err(ExportRecordError::Stale)
        );
        let next = family.source_event_head_on_control().unwrap().unwrap();
        assert_ne!(head, next);
        family.release_read(delivery.canonical).unwrap();
        family
            .release_read(delivery.canonical_pending.unwrap())
            .unwrap();
        family.abandon_on_control(&key).unwrap();
        // The queued second Event is acknowledged by actual abandonment; no
        // UI receive or fake destructor receipt is needed to reclaim it.
        for _ in 0..4 {
            if family.cleanup_one_on_control().unwrap() == SnapshotFamilyCleanup::Disposed {
                return;
            }
        }
        panic!("actual queued reads prevented normal cleanup");
    }
    #[test]
    fn source_queue_contention_and_foreign_read_preserve_actual_producer() {
        let (family, key) = family(4);
        let (foreign, foreign_key) = self::family(4);
        let session = crate::Session::new(48_000, 1, 1024).unwrap();
        let mut read = Some(source(&family, &key, &session, 9));
        let guard = family.try_inventory_guard_on_control().unwrap();
        assert_eq!(
            family.enqueue_source_event_on_control(&key, &mut read, &mut None, &mut None),
            Err(ExportRecordError::Busy)
        );
        assert!(read.is_some());
        drop(guard);
        assert_eq!(
            foreign.enqueue_source_event_on_control(&foreign_key, &mut read, &mut None, &mut None),
            Err(ExportRecordError::Stale)
        );
        assert!(read.is_some());
        family
            .enqueue_source_event_on_control(&key, &mut read, &mut None, &mut None)
            .unwrap();
        assert!(read.is_none());
    }
}
