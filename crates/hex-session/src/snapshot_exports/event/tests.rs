use super::*;

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
fn limits() -> SnapshotExportReservation {
    SnapshotExportReservation {
        snapshot_records: 2,
        read_records: 2,
        max_bookkeeping_bytes: 16_384,
    }
}

#[test]
fn worker_event_snapshot_and_read_publish_together_or_keep_exact_pending_owner() {
    let (mut family, producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
    let mut first = pending(7);
    let read = family
        .insert_event_read_on_control(&producer, &mut first)
        .unwrap();
    assert!(first.is_none());
    let old_index = read.snapshot.index;
    let old_generation = read.snapshot.generation;
    let derived = family.derive_read(&read).unwrap();
    assert_eq!(family.read_with(&derived, |view| view.revision()), Ok(7));
    let mut replacement = pending(11);
    let field = std::ptr::from_ref(&replacement.as_ref().unwrap().project);
    let before = family
        .try_inventory_guard_on_control()
        .unwrap()
        .inventory_revision();
    // A snapshot slot is free, but both read slots belong to the old frame.
    assert!(matches!(
        family.insert_event_read_on_control(&producer, &mut replacement),
        Err(ExportRecordError::Reservation)
    ));
    assert_eq!(
        std::ptr::from_ref(&replacement.as_ref().unwrap().project),
        field
    );
    {
        let guarded = family.try_inventory_guard_on_control().unwrap();
        assert_eq!(guarded.inventory_revision(), before);
        assert_eq!(guarded.snapshot_count(), 1);
    }
    family.release_read(read).unwrap();
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Pending)
    );
    assert_eq!(family.read_with(&derived, |view| view.revision()), Ok(7));
    assert_eq!(
        family
            .try_inventory_guard_on_control()
            .unwrap()
            .snapshot_count(),
        1
    );
    family.release_read(derived).unwrap();
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Pending)
    );
    assert_eq!(
        family
            .try_inventory_guard_on_control()
            .unwrap()
            .snapshot_count(),
        0
    );
    let next = family
        .insert_event_read_on_control(&producer, &mut replacement)
        .unwrap();
    assert!(replacement.is_none());
    assert_eq!(next.snapshot.index, old_index);
    assert_ne!(next.snapshot.generation, old_generation);
    assert_eq!(family.read_with(&next, |view| view.revision()), Ok(11));
    family.abandon_on_control(&producer).unwrap();
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Pending)
    );
    assert_eq!(family.read_with(&next, |view| view.revision()), Ok(11));
    family.release_read(next).unwrap();
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Disposed)
    );
}

#[test]
fn worker_event_snapshot_refuses_busy_foreign_closed_and_exhausted_without_consumption() {
    let (family, producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
    let (_, foreign) = SessionExportFamily::try_new_on_control(limits()).unwrap();
    let mut source = pending(19);
    let field = std::ptr::from_ref(&source.as_ref().unwrap().project);
    let guard = family.try_inventory_guard_on_control().unwrap();
    assert!(matches!(
        family.insert_event_read_on_control(&producer, &mut source),
        Err(ExportRecordError::Busy)
    ));
    drop(guard);
    assert!(matches!(
        family.insert_event_read_on_control(&foreign, &mut source),
        Err(ExportRecordError::Stale)
    ));
    family.abandon_on_control(&producer).unwrap();
    assert!(matches!(
        family.insert_event_read_on_control(&producer, &mut source),
        Err(ExportRecordError::Closed)
    ));
    let mut records = SnapshotExportRecords::try_new_on_control(limits()).unwrap();
    records.revision = u64::MAX;
    assert!(matches!(
        records.insert_event_read_on_control(&mut source),
        Err(ExportRecordError::Exhausted)
    ));
    records.revision = 0;
    for read in &records.reads {
        read.generation.store(u64::MAX, Ordering::Release);
    }
    assert!(matches!(
        records.insert_event_read_on_control(&mut source),
        Err(ExportRecordError::Reservation)
    ));
    assert_eq!(std::ptr::from_ref(&source.as_ref().unwrap().project), field);
    assert_eq!(records.inventory_revision(), 0);
    assert!(
        records
            .snapshots
            .iter()
            .all(|record| record.snapshot.is_none())
    );
    assert!(
        records
            .reads
            .iter()
            .all(|read| !read.occupied.load(Ordering::Acquire))
    );
}
