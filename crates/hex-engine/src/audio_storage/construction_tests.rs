//! Refusal must precede even the first inspection-backing reservation.
use super::*;
fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 11,
        handle_rows: 13,
        root_rows: 7,
        max_retained_bytes: usize::MAX,
        max_construction_peak_bytes: usize::MAX,
    }
}
#[test]
fn inspection_ledger_limits_refuse_before_first_backing_reservation() {
    let mut limits = reservation();
    let requested = 11 * size_of::<Data<'_>>()
        + 13 * size_of::<Handle<'_>>()
        + 7 * size_of::<Root<'_>>()
        + size_of::<AudioRootStorageLedger<'_>>();
    let ledger = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
    assert_eq!(ledger.metadata_capacity_bytes(), Some(requested));
    for peak in [false, true] {
        limits.max_retained_bytes = requested;
        limits.max_construction_peak_bytes = requested;
        if peak {
            limits.max_construction_peak_bytes -= 1;
        } else {
            limits.max_retained_bytes -= 1;
        }
        BACKING_RESERVATIONS.with(|count| count.set(0));
        assert!(matches!(
            AudioRootStorageLedger::try_new_on_control(limits),
            Err(AudioStorageError::ReservationExceeded)
        ));
        assert_eq!(
            BACKING_RESERVATIONS.with(|count| count.get()),
            0,
            "limit refusal allocated/reserved inspection metadata first"
        );
    }
}
