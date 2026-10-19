use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation, SharedMetadata,
};
fn limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 32,
        handle_rows: 16,
        root_rows: 4,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
#[test]
fn physical_projection_keeps_nonstatic_borrows_and_actual_vec_identity() {
    let text = String::from("actual borrowed input");
    let mut values = Vec::with_capacity(11);
    values.push(text.as_str());
    let other = vec![text.as_str()];
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    VecRef::of(&values).observe(&mut ledger).unwrap();
    ledger
        .borrow_metadata_vec_backing_on_control(&values)
        .unwrap();
    assert_eq!(ledger.summary().unwrap().data_allocations, 1);
    assert_eq!(
        ledger.summary().unwrap().data_bytes,
        values.capacity() * std::mem::size_of::<&str>()
    );
    assert!(
        VecRef::of(&values)
            .element(0)
            .unwrap()
            .same(BodyRef::of(&values[0]))
    );
    assert!(
        !VecRef::of(&values)
            .element(0)
            .unwrap()
            .same(BodyRef::of(&other[0]))
    );
    let pair = (&values, &text);
    assert!(BodyRef::of(&pair).contains(BodyRef::of(&pair.0)));
    assert!(!BodyRef::of(&pair).contains(BodyRef::of(&other)));
    assert!(
        ledger.summary().unwrap().require_complete().is_err(),
        "no referenced child/source authority"
    );
}
#[test]
fn paired_projection_dedups_data_preserves_distinct_fields_and_legacy_observations() {
    let a = SharedMetadata::new_on_control([1u64, 2]);
    let b = a.clone();
    let equal_but_distinct = SharedMetadata::new_on_control([1u64, 2]);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    ledger.borrow_shared_metadata_body_on_control(&a).unwrap();
    let initial = ledger.summary().unwrap();
    SharedMetadataRef::of(&a).observe(&mut ledger).unwrap();
    assert_eq!(
        ledger.summary().unwrap(),
        initial,
        "same REAL field dedup across old and new visitors"
    );
    assert_eq!(
        ledger.validate_selected_shared_handles_on_control(),
        Err(AudioStorageError::UnknownStorage),
        "omitted real clone is not closed"
    );
    SharedMetadataRef::of(&b).observe(&mut ledger).unwrap();
    ledger.borrow_shared_metadata_body_on_control(&b).unwrap();
    SharedMetadataRef::of(&equal_but_distinct)
        .observe(&mut ledger)
        .unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(ledger.selected_shared_counts_on_control(), (2, 3));
    assert!(ledger.summary().unwrap().require_complete().is_err());
}
#[test]
fn paired_projection_accepts_nonstatic_metadata_without_making_referenced_child_complete() {
    let text = String::from("outside borrowed child");
    let owner = SharedMetadata::new_on_control(text.as_str());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    SharedMetadataRef::of(&owner).observe(&mut ledger).unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(ledger.selected_shared_counts_on_control(), (1, 1));
    assert!(ledger.summary().unwrap().require_complete().is_err());
}
