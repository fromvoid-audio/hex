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
        Default::default(),
        revision,
        1,
        false,
        None,
    ))
}
#[test]
fn bounded_snapshot_records_preserve_actual_pending_owner_and_exact_old_reads() {
    let mut records = SnapshotExportRecords::try_new_on_control(limits()).unwrap();
    let mut old = pending(7);
    let old_stamp = records.insert_on_control(&mut old).unwrap();
    assert!(old.is_none());
    let read = records.reserve_read(old_stamp).unwrap();
    let mut next = pending(8);
    let new_stamp = records.insert_on_control(&mut next).unwrap();
    let mut refused = pending(9);
    let project_field = &refused.as_ref().unwrap().project as *const _;
    assert_eq!(
        records.insert_on_control(&mut refused),
        Err(ExportRecordError::Reservation)
    );
    assert_eq!(
        project_field,
        &refused.as_ref().unwrap().project as *const _
    );
    records.retire_on_control(old_stamp).unwrap();
    assert!(matches!(
        records.reserve_read(old_stamp),
        Err(ExportRecordError::Closed)
    ));
    records.close_on_control().unwrap();
    assert_eq!(
        records.read_with(&read, |view| (view.revision(), view.sample_rate())),
        Ok((7, 48_000))
    );
    assert!(matches!(
        records.reserve_read(new_stamp),
        Err(ExportRecordError::Closed)
    ));
    let derived = records.derive_read(&read).unwrap();
    assert_eq!(records.read_with(&derived, |view| view.revision()), Ok(7));
    records.release_read(read).unwrap();
    assert_eq!(
        records.snapshots[old_stamp.index]
            .readers
            .load(Ordering::Acquire),
        2
    );
    assert_eq!(records.collect_releases_on_control(), Ok(1));
    assert_eq!(
        records.snapshots[old_stamp.index]
            .readers
            .load(Ordering::Acquire),
        1
    );
    records.release_read(derived).unwrap();
    assert_eq!(records.collect_releases_on_control(), Ok(1));
    assert!(
        records.snapshots[old_stamp.index].snapshot.is_some(),
        "release is not owning cleanup"
    );
    assert_eq!(records.collect_releases_on_control(), Ok(0));
}
#[test]
fn read_incarnations_refuse_wrong_family_late_release_and_full_slots_without_inventory_change() {
    let mut records = SnapshotExportRecords::try_new_on_control(limits()).unwrap();
    let mut source = pending(11);
    let snapshot = records.insert_on_control(&mut source).unwrap();
    let read = records.reserve_read(snapshot).unwrap();
    let stale = SnapshotReadStamp {
        family: read.family,
        index: read.index,
        generation: read.generation,
        snapshot: read.snapshot,
    };
    let second = records.derive_read(&read).unwrap();
    let revision = records.inventory_revision();
    assert!(matches!(
        records.derive_read(&read),
        Err(ExportRecordError::Reservation)
    ));
    let foreign = SnapshotReadStamp {
        family: read.family + 1,
        index: read.index,
        generation: read.generation,
        snapshot: read.snapshot,
    };
    assert_eq!(
        records.read_with(&foreign, |_| panic!("must not call invalid read")),
        Err(ExportRecordError::Stale)
    );
    assert_eq!(records.inventory_revision(), revision);
    records.release_read(read).unwrap();
    assert_eq!(records.collect_releases_on_control(), Ok(1));
    let replacement = records.derive_read(&second).unwrap();
    assert_eq!(replacement.index, stale.index);
    assert_ne!(replacement.generation, stale.generation);
    assert_eq!(records.release_read(stale), Err(ExportRecordError::Stale));
    assert_eq!(
        records.read_with(&replacement, |view| view.revision()),
        Ok(11)
    );
    assert_eq!(records.inventory_revision(), revision);
    records.release_read(replacement).unwrap();
    records.release_read(second).unwrap();
    assert_eq!(records.collect_releases_on_control(), Ok(2));
}
#[test]
fn record_reservation_checks_empty_overflow_and_actual_backing_budget() {
    let quote = SnapshotExportRecords::reservation_quote(limits()).unwrap();
    let mut exact = limits();
    exact.max_bookkeeping_bytes = quote;
    let records = SnapshotExportRecords::try_new_on_control(limits()).unwrap();
    assert!(records.snapshots.capacity() >= 2);
    assert!(records.reads.capacity() >= 2);
    let observed = std::mem::size_of::<SnapshotExportRecords>()
        + records.snapshots.capacity() * std::mem::size_of::<SnapshotRecord>()
        + records.reads.capacity() * std::mem::size_of::<ReadRecord>();
    assert!(observed <= limits().max_bookkeeping_bytes);
    exact.max_bookkeeping_bytes = quote - 1;
    assert!(matches!(
        SnapshotExportRecords::try_new_on_control(exact),
        Err(ExportRecordError::Reservation)
    ));
    exact = limits();
    exact.read_records = usize::MAX;
    assert!(SnapshotExportRecords::reservation_quote(exact).is_err());
    exact.read_records = 0;
    assert!(SnapshotExportRecords::reservation_quote(exact).is_err());
}

#[test]
fn sampler_panel_read_keeps_exact_old_empty_and_loaded_authored_snapshot_without_owner_escape() {
    use hex_project::{Command, Created};
    let mut session = crate::Session::new(48_000, 8, 1024).unwrap();
    let Created::Track(track) = session
        .editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_engine::empty_sampler_state_on_control().unwrap(),
        })
        .unwrap();
    let mut old = Some(crate::service::view_state_owned(
        &session,
        Default::default(),
        7,
        1,
        false,
        None,
    ));
    let mut records = SnapshotExportRecords::try_new_on_control(limits()).unwrap();
    let old_stamp = records.insert_on_control(&mut old).unwrap();
    let old_read = records.reserve_read(old_stamp).unwrap();
    let loaded = hex_engine::sampler_replacement_state_on_control(
        session.project().device(device).unwrap().state(),
        37,
    )
    .unwrap();
    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: loaded.state,
        })
        .unwrap();
    let mut next = Some(crate::service::view_state_owned(
        &session,
        Default::default(),
        8,
        1,
        false,
        None,
    ));
    let next_stamp = records.insert_on_control(&mut next).unwrap();
    let next_read = records.reserve_read(next_stamp).unwrap();
    records.retire_on_control(old_stamp).unwrap();
    drop(session);
    assert_eq!(
        records
            .read_with(&old_read, |snapshot| {
                assert!(matches!(
                    snapshot.sampler_state(device).unwrap(),
                    hex_engine::SamplerDeviceStateRead::Empty
                ));
                snapshot.revision()
            })
            .unwrap(),
        7
    );
    assert_eq!(
        records
            .read_with(&next_read, |snapshot| {
                let hex_engine::SamplerDeviceStateRead::Sample(read) =
                    snapshot.sampler_state(device).unwrap()
                else {
                    panic!("loaded authored sample")
                };
                assert_eq!(read.region().start, 0);
                assert_eq!(read.region().end, 37);
                assert_eq!(
                    read.mode(),
                    hex_engine::SamplerReadMode::Classic { loop_region: None }
                );
                assert_eq!(read.slice_starts().len(), 0);
                assert_eq!(read.root_note(), 60);
                assert_eq!(read.gain(), 1.0);
                snapshot.revision()
            })
            .unwrap(),
        8
    );
    records.release_read(old_read).unwrap();
    records.release_read(next_read).unwrap();
    assert_eq!(records.collect_releases_on_control().unwrap(), 2);
}
