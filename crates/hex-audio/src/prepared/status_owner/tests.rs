use super::*;
fn facts(archive: &StatusArchive, rows: usize) -> Result<(usize, usize), AudioStorageError> {
    let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
        hex_engine::AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: rows,
            root_rows: 4,
            max_retained_bytes: 1 << 20,
            max_construction_peak_bytes: 1 << 20,
        },
    )?;
    archive.fields()?.visit_storage_on_control(&mut ledger)?;
    ledger.validate_selected_shared_handles_on_control()?;
    Ok((
        ledger.summary()?.data_bytes,
        ledger.selected_shared_counts_on_control().1,
    ))
}
#[test]
fn actual_seven_status_actors_keep_individual_handles_and_root_anchor_until_final_drop() {
    let archive = StatusArchive::new().unwrap();
    let initial = archive.initial().unwrap();
    let bytes = StatusArchive::requested_bytes().unwrap();
    assert_eq!(facts(&archive, 32).unwrap(), (bytes, 10));
    let mut actors = None;
    let measured = allocation_counter::measure(|| {
        actors = Some([
            archive.duplicate(&initial.publication, CLIENT).unwrap(),
            archive.duplicate(&initial.callback, NATIVE_DATA).unwrap(),
            archive.duplicate(&initial.callback, NATIVE_ERROR).unwrap(),
        ]);
    });
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!(facts(&archive, 32).unwrap(), (bytes, 16));
    let measured = allocation_counter::measure(|| drop(initial));
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!(facts(&archive, 32).unwrap(), (bytes, 8));
    let measured = allocation_counter::measure(|| drop(actors.take()));
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!(facts(&archive, 32).unwrap(), (bytes, 2));
    assert!(archive.field(CLIENT).is_err()); // disposed is never reusable
    let measured = allocation_counter::measure(|| drop(archive));
    assert_eq!(
        (measured.count_current, measured.bytes_current),
        (-3, -(bytes as i64))
    );
    eprintln!(
        "SUPPORTED actual Status seven actor roles 10→16→8→2 individual data/lease handles; actor release0alloc/free, final Root anchor3frees/{bytes}B; no full native admission"
    );
}
#[test]
fn foreign_status_occupied_role_unexplained_alias_and_row_shortage_refuse() {
    let a = StatusArchive::new().unwrap();
    let b = StatusArchive::new().unwrap();
    let actors = a.initial().unwrap();
    let foreign = b.initial().unwrap();
    let legacy = StatusHandle::legacy_fixture(Status::default());
    let measured = allocation_counter::measure(|| {
        assert!(a.duplicate(&foreign.publication, CLIENT).is_err());
        assert!(a.duplicate(&actors.publication, CALLBACK).is_err());
        assert!(a.duplicate(&legacy, CLIENT).is_err());
    });
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert!(facts(&a, 9).is_err());
    let unexplained = a.value.clone();
    assert!(facts(&a, 32).is_err());
    drop(unexplained);
    assert_eq!(
        facts(&a, 32).unwrap().0,
        StatusArchive::requested_bytes().unwrap()
    );
    let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
        hex_engine::AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: 32,
            root_rows: 4,
            max_retained_bytes: 1 << 20,
            max_construction_peak_bytes: 1 << 20,
        },
    )
    .unwrap();
    a.fields()
        .unwrap()
        .visit_storage_on_control(&mut ledger)
        .unwrap();
    b.fields()
        .unwrap()
        .visit_storage_on_control(&mut ledger)
        .unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(
        ledger.summary().unwrap().data_bytes,
        2 * StatusArchive::requested_bytes().unwrap()
    );
}

#[test]
fn actual_status_field_role_foreign_owner_and_legacy_refuse_without_allocating() {
    let archive = StatusArchive::new().unwrap();
    let fields = archive.initial().unwrap();
    let foreign = StatusArchive::new().unwrap();
    let other = foreign.initial().unwrap();
    let legacy = StatusHandle::legacy_fixture(Status::try_new().unwrap());
    let matching = allocation_counter::measure(|| {
        assert!(archive.matches_actual_field(&fields.publication, PUBLICATION));
        assert!(archive.matches_actual_field(&fields.callback, CALLBACK));
        assert!(!archive.matches_actual_field(&fields.publication, CALLBACK));
        assert!(!archive.matches_actual_field(&fields.callback, PUBLICATION));
        assert!(!archive.matches_actual_field(&other.callback, CALLBACK));
        assert!(!archive.matches_actual_field(&legacy, CALLBACK));
        assert!(!archive.matches_actual_field(&fields.callback, usize::MAX));
    });
    assert_eq!((matching.count_total, matching.count_current), (0, 0));
    facts(&archive, 32).unwrap();
    facts(&foreign, 32).unwrap();
}
