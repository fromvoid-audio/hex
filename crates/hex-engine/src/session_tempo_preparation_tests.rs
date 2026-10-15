use super::*;
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
fn completed_canonical_owner_and_timing_are_exact_and_spare_backings_survive_bank_recovery() {
    if !super::session_source_tests::supported("tempo_transfer_and_reuse") {
        return;
    }
    let (project, bank, source_token) = super::session_source_tests::fixture(false);
    let pointer = bank.sources.as_ptr();
    let capacity = bank.sources.capacity();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut custody, &project, bank).ok().unwrap();
    let report = custody.capacity_report();
    let tempo = report.tempo;
    assert_eq!(tempo.copy_backing_bytes, 0);
    assert_eq!(tempo.unresolved_owned_allocations, 2);
    assert!(tempo.completed_backing_bytes > 0);
    let actual = custody.tempo.canonical.as_ref().unwrap();
    assert_eq!(actual, project.tempo_map());
    assert_eq!(
        actual.owned_allocation_layout().checked_backing_bytes(),
        Some(tempo.completed_backing_bytes)
    );
    for quarter in 0..40 {
        let beat = hex_project::BeatTime::from_quarters(quarter).unwrap();
        let frame = project.tempo_map().beat_to_frame(beat).unwrap();
        assert_eq!(actual.beat_to_frame(beat), Ok(frame));
        assert_eq!(
            actual.frame_to_beat(frame),
            project.tempo_map().frame_to_beat(frame)
        );
    }
    {
        let mut owners = Vec::new();
        custody
            .tempo
            .visit_owners(|owner| {
                owners.push(owner);
                Ok(())
            })
            .unwrap();
        assert_eq!(owners.len(), 2);
        assert!(!owners[0].same_owner(&owners[1]));
        assert_eq!(
            owners
                .iter()
                .map(|owner| owner.bytes().unwrap())
                .sum::<usize>(),
            tempo.completed_backing_bytes
        );
        let mut other = Vec::new();
        custody
            .tempo
            .visit_owners(|owner| {
                other.push(owner);
                Ok(())
            })
            .unwrap();
        assert!(
            owners
                .iter()
                .zip(&other)
                .all(|(left, right)| left.same_owner(right))
        );
        let mut different = crate::session_tempo_preparation::SessionTempoConstruction::default();
        different.reserve(&project).unwrap();
        different.construct().unwrap();
        let mut different_owners = Vec::new();
        different
            .visit_owners(|owner| {
                different_owners.push(owner);
                Ok(())
            })
            .unwrap();
        assert!(
            owners
                .iter()
                .zip(&different_owners)
                .all(|(left, right)| !left.same_owner(right))
        );
        let mut visited = 0;
        assert_eq!(
            custody.tempo.visit_owners(|_| {
                visited += 1;
                Err(AudioStorageError::Capacity)
            }),
            Err(AudioStorageError::Capacity)
        );
        assert_eq!(visited, 1); // discard this speculative partial sink
    }
    assert_eq!(
        custody.tempo.reserve(&project),
        Err(AudioStorageError::Capacity)
    );
    assert_eq!(
        custody.tempo.canonical.as_ref().unwrap(),
        project.tempo_map()
    );
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    assert!(custody.tempo.canonical.is_none());
    let retained = custody.tempo.observe(false).unwrap();
    assert_eq!(retained.copy_backing_bytes, tempo.completed_backing_bytes);
    assert_eq!(retained.completed_backing_bytes, 0);
    assert_eq!(retained.unresolved_owned_allocations, 2); // not lifetime closure
    let bank = custody.recover_on_control(&token).unwrap();
    assert_eq!(bank.sources.as_ptr(), pointer);
    assert_eq!(bank.sources.capacity(), capacity);
    assert!(bank.source_view(&source_token).is_ok());
    let next = begin(&mut custody, &project, bank).ok().unwrap();
    let reused = custody.capacity_report();
    assert_eq!(reused.tempo.previous_growth_backing_bytes, 0);
    assert_eq!(
        reused.tempo.completed_backing_bytes,
        tempo.completed_backing_bytes
    );
    assert!(custody.cancel(&next));
    custody.cleanup_on_control();
    let bank = custody.recover_on_control(&next).unwrap();
    let arrangement = custody
        .begin_arrangement_on_control(&project, 11, bank)
        .ok()
        .unwrap();
    assert_eq!(
        custody.capacity_report().tempo.copy_backing_bytes,
        tempo.completed_backing_bytes
    );
    assert!(custody.cancel(&arrangement));
    custody.cleanup_on_control();
    assert!(custody.recover_on_control(&arrangement).is_some());
    eprintln!(
        "hex Session preparation evidence: tempo_transfer_and_reuse; supported-controls=completed; unresolved-allocations=2"
    );
}
#[test]
fn late_copy_failure_and_byte_refusal_return_exact_bank_with_tokens_and_charge_partial_storage() {
    if !super::session_source_tests::supported("tempo_failure_and_reservation") {
        return;
    }
    let (project, bank, token) = super::session_source_tests::fixture(false);
    let pointer = bank.sources.as_ptr();
    let capacity = bank.sources.capacity();
    let reservation = bank.reservation;
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    custody.tempo.fail_after_copy = true;
    let refused = begin(&mut custody, &project, bank).err().unwrap();
    assert_eq!(refused.reason, AudioStorageError::Allocation);
    assert_eq!(refused.inputs.sources.as_ptr(), pointer);
    assert_eq!(refused.inputs.sources.capacity(), capacity);
    assert_eq!(refused.inputs.reservation, reservation);
    assert!(refused.inputs.source_view(&token).is_ok());
    assert!(refused.capacity.tempo.copy_backing_bytes > 0);
    assert_eq!(custody.phase(), AudioPreparationPhase::Vacant);
    assert!(custody.tempo.observe(false).unwrap().copy_backing_bytes > 0);
    custody.tempo.fail_after_copy = false;
    let accepted = begin(&mut custody, &project, refused.inputs).ok().unwrap();
    let needed = custody.capacity_report().retained_stage_bytes;
    assert!(custody.cancel(&accepted));
    custody.cleanup_on_control();
    let mut bank = custody.recover_on_control(&accepted).unwrap();
    bank.reservation.max_prepared_data_bytes = needed - 1;
    let mut fresh = AudioPreparationCustody::try_new_on_control().unwrap();
    let refused = begin(&mut fresh, &project, bank).err().unwrap();
    assert_eq!(refused.reason, AudioStorageError::ReservationExceeded);
    assert_eq!(refused.inputs.sources.as_ptr(), pointer);
    assert!(refused.inputs.source_view(&token).is_ok());
    assert!(refused.capacity.tempo.copy_backing_bytes > 0);
    let overflow = crate::SessionTempoCapacity {
        copy_backing_bytes: usize::MAX,
        completed_backing_bytes: 1,
        ..Default::default()
    };
    assert_eq!(overflow.bytes(), Err(AudioStorageError::Overflow));
    eprintln!(
        "hex Session preparation evidence: tempo_failure_and_reservation; supported-controls=completed"
    );
}
#[test]
fn tempo_panic_leaves_actual_persistent_buffers_and_original_bank_without_retry() {
    if !super::session_source_tests::supported("tempo_construction_panic") {
        return;
    }
    let (project, bank, source_token) = super::session_source_tests::fixture(false);
    let pointer = bank.sources.as_ptr();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    custody.tempo.panic_after_copy = true;
    let refusal = begin(&mut custody, &project, bank).err().unwrap();
    assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
    assert!(refusal.inputs.source_view(&source_token).is_ok());
    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(custody.first_panic.is_some());
    assert!(custody.tempo.observe(false).unwrap().copy_backing_bytes > 0);
    assert!(custody.tempo.canonical.is_none());
    custody.cleanup_on_control();
    let second = begin(&mut custody, &project, refusal.inputs).err().unwrap();
    assert_eq!(second.reason, AudioStorageError::Capacity);
    assert_eq!(second.inputs.sources.as_ptr(), pointer);
    assert!(custody.first_panic.is_some());
    eprintln!(
        "hex Session preparation evidence: tempo_construction_panic; supported-controls=completed; persistent-payload=1"
    );
}
