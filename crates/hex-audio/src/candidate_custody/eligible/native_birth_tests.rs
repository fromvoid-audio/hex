use super::*;
use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation};

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
fn actual_root_and_two_signals_born_paired_match_all_allocated_headers_and_payloads() {
    let mut root = None;
    let measured = allocation_counter::measure(|| root = Some(CandidateRoot::new(3).unwrap()));
    let root = root.unwrap();
    let bank = lock(&root.0.bank);
    let expected = root_allocation_bytes().unwrap()
        + eligible::pages::signal_data_bytes().unwrap()
        + bank.slots.capacity() * std::mem::size_of::<Slot>();
    assert_eq!(measured.count_total, 4);
    assert_eq!(measured.bytes_total as usize, expected);
    let native = lock(&root.0.eligible.bank);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    let visited = allocation_counter::measure(|| {
        ledger
            .borrow_shared_metadata_body_on_control(&root.0)
            .unwrap();
        ledger
            .borrow_metadata_vec_backing_on_control(&bank.slots)
            .unwrap();
        for domain in &native.domains {
            ledger
                .borrow_shared_metadata_body_on_control(&domain.signal)
                .unwrap();
        }
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
    });
    assert_eq!((visited.count_total, visited.count_current), (0, 0));
    assert_eq!(ledger.summary().unwrap().data_allocations, 4);
    assert_eq!(ledger.summary().unwrap().data_bytes, expected);
    assert_eq!(ledger.selected_shared_counts_on_control(), (3, 3));
    assert!(
        ledger.summary().unwrap().require_complete().is_err(),
        "actual payload descendants and retirement still required"
    );
    eprintln!(
        "SUPPORTED actual Root+2DomainSignals constructor4allocations/{expected}bytes: paired body+header and actual Vec capacity once; inspection0alloc/free/clone; full root admission remains refused"
    );
}

#[test]
fn actual_root_and_signal_alias_fields_are_mandatory_distinct_roots_stay_distinct() {
    let root = CandidateRoot::new(2).unwrap();
    let alias = root.clone();
    let other = CandidateRoot::new(2).unwrap();
    assert!(root.same_root(&alias));
    assert!(!root.same_root(&other));
    let bank = lock(&root.0.eligible.bank);
    let signal = bank.domains[0].signal.clone();
    assert!(signal.shares_owner_with(&bank.domains[0].signal));
    assert!(!signal.shares_owner_with(&bank.domains[1].signal));
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    ledger
        .borrow_shared_metadata_body_on_control(&root.0)
        .unwrap();
    ledger
        .borrow_shared_metadata_body_on_control(&bank.domains[0].signal)
        .unwrap();
    assert!(
        ledger
            .validate_selected_shared_handles_on_control()
            .is_err(),
        "actual omitted Root/signal aliases refuse"
    );
    ledger
        .borrow_shared_metadata_body_on_control(&alias.0)
        .unwrap();
    ledger
        .borrow_shared_metadata_body_on_control(&signal)
        .unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(ledger.selected_shared_counts_on_control(), (2, 4));
    let before = ledger.summary().unwrap().data_bytes;
    ledger
        .borrow_shared_metadata_body_on_control(&root.0)
        .unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, before);
    ledger
        .borrow_shared_metadata_body_on_control(&other.0)
        .unwrap();
    ledger
        .borrow_shared_metadata_body_on_control(&bank.domains[1].signal)
        .unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(ledger.selected_shared_counts_on_control(), (4, 6));
    assert_eq!(
        ledger.summary().unwrap().data_bytes,
        2 * root_allocation_bytes().unwrap() + eligible::pages::signal_data_bytes().unwrap()
    );
    assert!(ledger.summary().unwrap().require_complete().is_err());
    eprintln!(
        "SUPPORTED actual Root/signal aliases mandatory, repeated samefield dedup, equal-size distinct Root/signal DATA distinct; no raw Arc adoption or complete graph declaration"
    );
}
