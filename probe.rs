use hex_engine::{AudioPreparationCustody, AudioPreparationPhase, AudioRootStorageLedger,
    AudioRootStorageSummary, AudioStorageIncomplete, AudioStorageLedgerReservation, AudioStorageRole};
fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation { data_rows: 128, handle_rows: 128, root_rows: 16,
        max_retained_bytes: 1 << 20, max_construction_peak_bytes: 1 << 20 }
}
fn main() {
    let scalar = AudioRootStorageSummary::default();
    println!("PUBLIC_ZERO_SCALARS: {scalar:?}; complete={:?}", scalar.require_complete());
    assert_eq!(scalar.require_complete(), Err(AudioStorageIncomplete::RetirementReceiptUnknown));
    {
        let ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        ledger.validate_selected_shared_handles_on_control().unwrap();
        let summary = ledger.summary().unwrap();
        assert_eq!(summary.unknown_storage, 0);
        assert_eq!(summary.unresolved_handles, 0);
        assert_eq!(summary.unresolved_retirements, 0);
        println!("ACTUAL_EMPTY_LEDGER: {summary:?}; complete={:?}", summary.require_complete());
        assert_eq!(summary.require_complete(), Err(AudioStorageIncomplete::RetirementReceiptUnknown));
    }
    let parent = AudioPreparationCustody::try_new_on_control().unwrap();
    assert_eq!(parent.phase(), AudioPreparationPhase::Vacant);
    {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        ledger.borrow_custody_on_control(&parent, AudioStorageRole::Current).unwrap();
        ledger.validate_selected_shared_handles_on_control().unwrap();
        let summary = ledger.summary().unwrap();
        summary.require_known_storage().unwrap();
        println!("ACTUAL_FIRST_BOUND_ROOT: owner=AudioPreparationCustody address={:p} role=Current phase={:?}; {summary:?}; complete={:?}", &parent, parent.phase(), summary.require_complete());
        assert_eq!(summary.unknown_storage, 0);
        assert_eq!(summary.unresolved_handles, 0);
        assert_eq!(summary.unresolved_retirements, 1);
        assert_eq!(summary.require_complete(), Err(AudioStorageIncomplete::RetirementReceiptUnknown));
    }
    // Fail fast: after a visitor error, drop its ledger. Do not reuse partial
    // observations or pass their totals to construction/complete checks.
    {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        ledger.borrow_custody_on_control(&parent, AudioStorageRole::Current).unwrap();
        let failed = ledger.borrow_custody_on_control(&parent, AudioStorageRole::Current);
        assert!(failed.is_err());
        println!("DUPLICATE_ROOT_VISITOR_REFUSED: {failed:?}; ledger discarded");
        drop(ledger);
    }
    println!("4 diagnostic discriminators PASS; no retirement/admission upgrade");
}
