//! Actual Vec fields only, independent of shared-source provenance.
use super::*;
fn budget() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 16,
        root_rows: 8,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
#[test]
fn borrowed_vec_backing_static_and_nonstatic_fields_dedup_actual_capacity_without_children() {
    let text = String::from("borrowed locally");
    let mut borrowed = Vec::with_capacity(19);
    borrowed.push(text.as_str());
    let mut independent = Vec::with_capacity(19);
    independent.push(text.as_str());
    let mut owned = Vec::<u32>::with_capacity(7);
    owned.push(1);
    let collect = |reservation| -> Result<_, AudioStorageError> {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
        ledger.borrow_metadata_vec_backing_on_control(&borrowed)?;
        ledger.borrow_metadata_vec_backing_on_control(&borrowed)?;
        assert_eq!(
            ledger.summary()?.data_bytes,
            borrowed.capacity() * std::mem::size_of::<&str>()
        );
        ledger.borrow_metadata_vec_backing_on_control(&independent)?;
        // Private old static path and public borrowed path share ONE domain.
        ledger.mutate(|inner| inner.vec(&owned))?;
        ledger.borrow_metadata_vec_backing_on_control(&owned)?;
        let summary = ledger.summary()?;
        assert_eq!(summary.data_allocations, 3);
        assert_eq!(
            summary.data_bytes,
            (borrowed.capacity() + independent.capacity()) * std::mem::size_of::<&str>()
                + owned.capacity() * std::mem::size_of::<u32>()
        );
        assert!(
            summary.require_complete().is_err(),
            "referenced String/other roots not certified"
        );
        Ok(summary)
    };
    let measured = collect(budget()).unwrap();
    let mut exact = budget();
    exact.max_retained_bytes = measured.retained_bytes;
    exact.max_construction_peak_bytes = measured.construction_peak_bytes;
    assert_eq!(collect(exact).unwrap(), measured);
    exact.max_retained_bytes -= 1;
    assert!(collect(exact).is_err());
}
#[test]
fn borrowed_vec_backing_empty_reserved_zst_and_conflicting_rows_keep_existing_semantics() {
    let reserved = Vec::<u64>::with_capacity(13);
    let empty = Vec::<u64>::new();
    let mut zst = Vec::<()>::with_capacity(17);
    zst.resize(17, ());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
    for values in [&reserved, &empty] {
        ledger
            .borrow_metadata_vec_backing_on_control(values)
            .unwrap();
    }
    ledger.borrow_metadata_vec_backing_on_control(&zst).unwrap();
    let summary = ledger.summary().unwrap();
    assert_eq!(summary.data_allocations, 1);
    assert_eq!(summary.data_bytes, reserved.capacity() * 8);
    assert_eq!(
        ledger.data.len(),
        3,
        "zero-byte fields still occupy real bounded observation rows"
    );
    let error = ledger.mutate(|inner| {
        inner.data(
            Owner::exclusive_vec(&reserved),
            Location("vec-buffer", 0),
            Declaration {
                bytes: 1,
                capacity: reserved.capacity(),
                element_bytes: 8,
                overhead: 0,
            },
            None,
        )
    });
    assert_eq!(error, Err(AudioStorageError::UnknownStorage));
    assert!(
        ledger.summary().is_err(),
        "failed-ledger invalidation remains intact"
    );
}
