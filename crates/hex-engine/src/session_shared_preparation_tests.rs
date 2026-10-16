use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn begin(
    custody: &mut AudioPreparationCustody,
    project: &hex_project::Project,
    bank: OwnedAudioInputs,
) -> Result<AudioPreparationToken, Box<AudioArrangementRefusal>> {
    custody.begin_input_on_control(
        AudioArrangementInput::new(project, 11, bank).with_session_sources(),
    )
}
#[test]
fn actual_fixed_inventory_allocations_and_independent_handle_locations_are_conserved() {
    if !super::session_source_tests::supported("shared_inventory_owners") {
        return;
    }
    for streamed in [false, true] {
        let (project, bank, source_token) = super::session_source_tests::fixture(streamed);
        let pointer = bank.sources.as_ptr();
        let capacity = bank.sources.capacity();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = begin(&mut custody, &project, bank).ok().unwrap();
        let report = custody.capacity_report().shared;
        assert_eq!(report.unresolved_allocations, 2);
        assert_eq!(report.installed_handle_obligations, 3);
        assert_eq!(report.reserved_handle_destinations, 3);
        assert_eq!(
            report.sine_payload_bytes,
            std::mem::size_of::<crate::SineTable>()
        );
        assert_eq!(
            report.context_payload_bytes,
            std::mem::size_of::<crate::shared_analytic::SharedAnalyticContextTable>()
        );
        assert_eq!(
            report.bytes().unwrap(),
            report.sine_payload_bytes
                + report.sine_declared_overhead_bytes
                + report.context_payload_bytes
                + report.context_declared_overhead_bytes
        );
        let ids = custody.shared.handle_ids();
        assert!(ids.iter().all(|id| *id != 0));
        assert!(ids[0] != ids[1] && ids[0] != ids[2] && ids[1] != ids[2]);
        assert_eq!(custody.shared.context_strong_count(), 2);
        crate::owned_shared::reset_clone_count();
        assert_eq!(custody.shared.observe().unwrap(), report);
        assert_eq!(crate::owned_shared::clone_count(), 0);
        assert_eq!(custody.shared.context_strong_count(), 2);
        // Equal table contents are NOT allocation/handle identity.
        let (other_project, other_bank, _) = super::session_source_tests::fixture(streamed);
        let mut other = AudioPreparationCustody::try_new_on_control().unwrap();
        let other_token = begin(&mut other, &other_project, other_bank).ok().unwrap();
        assert!(!custody.shared.same_context_allocation(&other.shared));
        assert_ne!(custody.shared.handle_ids(), other.shared.handle_ids());
        assert!(other.cancel(&other_token));
        other.cleanup_on_control();
        assert!(other.recover_on_control(&other_token).is_some());
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert_eq!(custody.shared.context_strong_count(), 0);
        assert_eq!(custody.shared.observe().unwrap().bytes(), Ok(0));
        let bank = custody.recover_on_control(&token).unwrap();
        assert_eq!(bank.sources.as_ptr(), pointer);
        assert_eq!(bank.sources.capacity(), capacity);
        assert!(bank.source_view(&source_token).is_ok());
    }
    eprintln!(
        "hex Session preparation evidence: shared_inventory_owners; supported-controls=completed; allocations=2; independent-handles=3"
    );
}
#[test]
fn every_fixed_owner_construction_failure_recovers_exact_original_bank_and_alias_tokens() {
    if !super::session_source_tests::supported("shared_inventory_failures") {
        return;
    }
    for phase in 0..4 {
        let (project, bank, source_token) = super::session_source_tests::fixture(false);
        let pointer = bank.sources.as_ptr();
        let capacity = bank.sources.capacity();
        let reservation = bank.reservation;
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        custody.shared.fail_after = Some(phase);
        let bank = match begin(&mut custody, &project, bank) {
            Err(refusal) => {
                assert!(phase < 3);
                assert_eq!(refusal.reason, AudioStorageError::Allocation);
                assert_eq!(refusal.capacity.shared.installed_handle_obligations, phase);
                assert_eq!(custody.phase(), AudioPreparationPhase::Vacant);
                refusal.inputs
            }
            Ok(token) => {
                assert_eq!(phase, 3);
                assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
                assert_eq!(custody.shared.context_strong_count(), 2);
                custody.cleanup_on_control();
                custody.recover_on_control(&token).unwrap()
            }
        };
        assert_eq!(bank.sources.as_ptr(), pointer);
        assert_eq!(bank.sources.capacity(), capacity);
        assert_eq!(bank.reservation, reservation);
        assert!(bank.source_view(&source_token).is_ok());
        assert_eq!(custody.shared.context_strong_count(), 0);
        custody.shared.fail_after = None;
        let accepted = begin(&mut custody, &project, bank).ok().unwrap();
        assert!(custody.cancel(&accepted));
        custody.cleanup_on_control();
        assert!(custody.recover_on_control(&accepted).is_some());
    }
    eprintln!(
        "hex Session preparation evidence: shared_inventory_failures; supported-controls=completed; injected-phases=4"
    );
}
#[test]
fn exact_caller_handle_and_byte_reservations_refuse_before_bank_movement() {
    if !super::session_source_tests::supported("shared_inventory_pressure") {
        return;
    }
    let (project, mut bank, source_token) = super::session_source_tests::fixture(false);
    // The existing source-only 8-handle declaration cannot cover 6 source
    // handles plus the new 3 fixed inventory handles. This remains a refusal.
    bank.reservation.handle_obligations = 8;
    let pointer = bank.sources.as_ptr();
    let capacity = bank.sources.capacity();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let refusal = begin(&mut custody, &project, bank).err().unwrap();
    assert_eq!(refusal.reason, AudioStorageError::Capacity);
    assert_eq!(refusal.inputs.reservation.handle_obligations, 8);
    assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
    assert_eq!(refusal.inputs.sources.capacity(), capacity);
    assert!(refusal.inputs.source_view(&source_token).is_ok());
    assert_eq!(custody.shared.observe().unwrap().bytes(), Ok(0));
    let mut bank = refusal.inputs;
    bank.reservation.handle_obligations = 16;
    let token = begin(&mut custody, &project, bank).ok().unwrap();
    let needed = custody.capacity_report().retained_stage_bytes;
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    let mut bank = custody.recover_on_control(&token).unwrap();
    bank.reservation.max_prepared_data_bytes = needed - 1;
    let mut fresh = AudioPreparationCustody::try_new_on_control().unwrap();
    let refusal = begin(&mut fresh, &project, bank).err().unwrap();
    assert_eq!(refusal.reason, AudioStorageError::ReservationExceeded);
    assert!(refusal.capacity.shared.bytes().unwrap() > 0);
    assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
    assert!(refusal.inputs.source_view(&source_token).is_ok());
    let overflow = crate::SessionSharedCapacity {
        sine_payload_bytes: usize::MAX,
        context_payload_bytes: 1,
        ..Default::default()
    };
    assert_eq!(overflow.bytes(), Err(AudioStorageError::Overflow));
    let mut empty = crate::session_shared_preparation::SessionSharedConstruction::default();
    assert_eq!(
        empty.reserve(usize::MAX, usize::MAX),
        Err(AudioStorageError::Overflow)
    );
    assert_eq!(empty.handle_ids(), [0; 3]);
    eprintln!(
        "hex Session preparation evidence: shared_inventory_pressure; supported-controls=completed"
    );
}
#[test]
fn first_construction_panic_retains_persistent_shared_owners_without_bank_movement_or_retry() {
    if !super::session_source_tests::supported("shared_inventory_panic") {
        return;
    }
    let (project, bank, source_token) = super::session_source_tests::fixture(false);
    let pointer = bank.sources.as_ptr();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    custody.shared.panic_after = Some(2);
    let refusal = begin(&mut custody, &project, bank).err().unwrap();
    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(custody.first_panic.is_some());
    assert_eq!(custody.shared.context_strong_count(), 1);
    assert_eq!(
        custody
            .shared
            .observe()
            .unwrap()
            .installed_handle_obligations,
        2
    );
    assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
    assert!(refusal.inputs.source_view(&source_token).is_ok());
    custody.cleanup_on_control();
    let refusal = begin(&mut custody, &project, refusal.inputs).err().unwrap();
    assert_eq!(refusal.reason, AudioStorageError::Capacity);
    assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
    assert_eq!(custody.shared.context_strong_count(), 1);
    assert!(custody.first_panic.is_some());
    eprintln!(
        "hex Session preparation evidence: shared_inventory_panic; supported-controls=completed; persistent-payload=1"
    );
}
#[test]
fn actual_owner_slot_destructor_panic_pins_remaining_shared_handles_and_bank_on_control() {
    if !super::session_source_tests::supported("shared_inventory_destructor_panic") {
        return;
    }
    let (project, bank, _) = super::session_source_tests::fixture(false);
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut custody, &project, bank).ok().unwrap();
    let thread = std::thread::current().id();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&count);
    custody.shared.probes[0] = Some(DropProbe(Some(Box::new(move || {
        assert_eq!(std::thread::current().id(), thread);
        observed.fetch_add(1, Ordering::Relaxed);
        panic!("actual fixed inventory slot destructor");
    }))));
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(custody.bank.is_some());
    assert!(custody.first_panic.is_some());
    assert_eq!(custody.shared.context_strong_count(), 2);
    assert_eq!(count.load(Ordering::Relaxed), 1);
    custody.cleanup_on_control();
    assert!(custody.recover_on_control(&token).is_none());
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert_eq!(custody.shared.context_strong_count(), 2);
    eprintln!(
        "hex Session preparation evidence: shared_inventory_destructor_panic; supported-controls=completed; destructor-count=1"
    );
}
