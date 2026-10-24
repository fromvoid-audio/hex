use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn fixture() -> (hex_project::Project, OwnedAudioInputs, AudioSourceToken) {
    use hex_project::{Command, Created, Editor};
    let (project, bank, token) = super::session_source_tests::fixture(false);
    let first_scene = project.scenes()[0].id();
    let mut editor = Editor::new(project, 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "metadata track".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    editor
        .execute(Command::SetSessionStopCell {
            track,
            scene: first_scene,
        })
        .unwrap();
    let Created::Scene(empty) = editor
        .execute(Command::AddScene {
            name: "empty scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    editor
        .execute(Command::MoveScene {
            scene: empty,
            before: Some(first_scene),
        })
        .unwrap();
    (editor.project().clone(), bank, token)
}
fn begin(
    custody: &mut AudioPreparationCustody,
    project: &hex_project::Project,
    bank: OwnedAudioInputs,
) -> AudioPreparationToken {
    custody
        .begin_input_on_control(
            AudioArrangementInput::new(project, 11, bank).with_session_sources(),
        )
        .ok()
        .unwrap()
}
#[test]
fn actual_scene_track_stop_index_owners_are_bound_accounted_and_recover_original_bank() {
    if !super::session_source_tests::supported("structure_owners_and_indices") {
        return;
    }
    let (project, bank, source_token) = fixture();
    let pointer = bank.sources.as_ptr();
    let original_capacity = bank.sources.capacity();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    custody.structure.extra_name_reserve = 37;
    let token = begin(&mut custody, &project, bank);
    assert_eq!(custody.session_structure_counts(), (3, 2, 3));
    assert_eq!(custody.installed_session_source_bindings(), (2, 1));
    custody.structure.validate(&custody.session, true).unwrap();
    let snapshot = custody.structure.snapshot();
    assert_eq!(snapshot.scene_names, ["empty scene", "scene 0", "scene 1"]);
    assert_eq!(snapshot.track_names, ["audio", "metadata track"]);
    assert_eq!(snapshot.stop_cells, 1);
    assert!(snapshot.ranges.iter().any(|r| r.1 == r.2));
    assert_eq!(snapshot.ranges.iter().map(|r| r.2 - r.1).sum::<usize>(), 3);
    let report = custody.capacity_report().structure;
    assert!(report.scene_capacity >= 3 && report.track_capacity >= 2 && report.cell_capacity >= 3);
    assert!(
        report.name_backing_bytes
            >= 5 * 37
                + snapshot
                    .scene_names
                    .iter()
                    .chain(&snapshot.track_names)
                    .map(|name| name.len())
                    .sum::<usize>()
    );
    assert_eq!(report.unresolved_owned_allocations, 11); // six Vecs + five independent Strings
    {
        let mut owners = Vec::new();
        custody
            .structure
            .visit_owners(|owner| {
                owners.push(owner);
                Ok(())
            })
            .unwrap();
        let mut again = Vec::new();
        custody
            .structure
            .visit_owners(|owner| {
                again.push(owner);
                Ok(())
            })
            .unwrap();
        assert_eq!(owners.len(), 11);
        assert_eq!(
            owners.iter().map(|o| o.bytes().unwrap()).sum::<usize>(),
            report.bytes().unwrap()
        );
        for (index, owner) in owners.iter().enumerate() {
            assert!(owner.same_owner(&again[index]));
            assert!(owners[..index].iter().all(|old| !old.same_owner(owner)));
        }
        let mut partial = Vec::new();
        let refused = custody.structure.visit_owners(|owner| {
            partial.push(owner);
            if partial.len() == 2 {
                return Err(AudioStorageError::Capacity);
            }
            Ok(())
        });
        assert_eq!(refused, Err(AudioStorageError::Capacity));
        assert_eq!(partial.len(), 2);
        partial.clear(); // partial owner sinks cannot authorize completeness
    }
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    assert_eq!(custody.capacity_report().structure, report); // charges/facts not subtracted by cleanup
    let bank = custody.recover_on_control(&token).unwrap();
    assert_eq!(bank.sources.as_ptr(), pointer);
    assert_eq!(bank.sources.capacity(), original_capacity);
    assert_eq!(bank.owning_handle_count(), 2);
    assert!(bank.source_view(&source_token).is_ok());
    eprintln!(
        "hex Session preparation evidence: structure_owners_and_indices; supported-controls=completed; exclusive-allocations=11"
    );
}
#[test]
fn every_structural_reserve_and_name_failure_returns_original_bank_without_source_clones() {
    if !super::session_source_tests::supported("structure_preflight_failures") {
        return;
    }
    for fault in 0..11 {
        let (project, bank, source_token) = fixture();
        let pointer = bank.sources.as_ptr();
        let capacity = bank.sources.capacity();
        let reservation = bank.reservation;
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        if fault < 6 {
            custody.structure.fail_reserve = Some(fault);
        } else {
            custody.structure.fail_name = Some(fault - 6);
        }
        let refusal = custody
            .begin_input_on_control(
                AudioArrangementInput::new(&project, 11, bank).with_session_sources(),
            )
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStorageError::Allocation);
        assert_eq!(custody.phase(), AudioPreparationPhase::Vacant);
        assert_eq!(custody.installed_source_bindings(), 0);
        assert_eq!(custody.installed_session_source_bindings(), (0, 0));
        assert_eq!(custody.session_structure_counts(), (0, 0, 0));
        assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
        assert_eq!(refusal.inputs.sources.capacity(), capacity);
        assert_eq!(refusal.inputs.reservation, reservation);
        assert!(refusal.inputs.source_view(&source_token).is_ok());
        assert_eq!(refusal.inputs.owning_handle_count(), 2);
    }
    eprintln!(
        "hex Session preparation evidence: structure_preflight_failures; supported-controls=completed; injected-phases=11"
    );
}
#[test]
fn structural_source_binding_requires_exact_handle_owner_not_identical_project_or_content() {
    if !super::session_source_tests::supported("structure_binding_identity") {
        return;
    }
    let (project, bank, _) = fixture();
    let (other_project, other_bank, _) = fixture();
    let mut first = AudioPreparationCustody::try_new_on_control().unwrap();
    let first_token = begin(&mut first, &project, bank);
    let mut other = AudioPreparationCustody::try_new_on_control().unwrap();
    let other_token = begin(&mut other, &other_project, other_bank);
    assert!(first.structure.validate(&other.session, true).is_err());
    assert_eq!(first.installed_session_source_bindings(), (2, 1));
    assert_eq!(other.installed_session_source_bindings(), (2, 1));
    {
        let mut a = Vec::new();
        let mut b = Vec::new();
        first
            .structure
            .visit_owners(|owner| {
                a.push(owner);
                Ok(())
            })
            .unwrap();
        other
            .structure
            .visit_owners(|owner| {
                b.push(owner);
                Ok(())
            })
            .unwrap();
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(x, y)| !x.same_owner(y)));
    }
    first.structure.duplicate_first_index();
    assert!(first.structure.validate(&first.session, true).is_err());
    assert!(first.cancel(&first_token));
    first.cleanup_on_control();
    assert!(first.recover_on_control(&first_token).is_some());
    assert!(other.cancel(&other_token));
    other.cleanup_on_control();
    assert!(other.recover_on_control(&other_token).is_some());
    eprintln!(
        "hex Session preparation evidence: structure_binding_identity; supported-controls=completed"
    );
}
#[test]
fn actual_structural_destructor_panic_pins_remaining_names_bank_and_first_payload_no_retry() {
    if !super::session_source_tests::supported("structure_destructor_panic") {
        return;
    }
    let (project, bank, _) = fixture();
    let pointer = bank.sources.as_ptr();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut custody, &project, bank);
    let count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&count);
    let thread = std::thread::current().id();
    custody.structure.set_disposal_probe(
        0,
        DropProbe(Some(Box::new(move || {
            assert_eq!(std::thread::current().id(), thread);
            observed.fetch_add(1, Ordering::Relaxed);
            panic!("actual structural name-owner field destructor");
        }))),
    );
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert_eq!(custody.bank.as_ref().unwrap().sources.as_ptr(), pointer);
    assert!(custody.first_panic.is_some());
    assert!(!custody.structure.snapshot().scene_names[1].is_empty());
    custody.cleanup_on_control();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert!(custody.recover_on_control(&token).is_none());
    eprintln!(
        "hex Session preparation evidence: structure_destructor_panic; supported-controls=completed; destructor-count=1"
    );
}
#[test]
fn structural_spare_backings_remain_charged_for_reuse_and_arrangement_only_then_capacity_refusal() {
    if !super::session_source_tests::supported("structure_retained_capacity") {
        return;
    }
    let (project, bank, source_token) = fixture();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut custody, &project, bank);
    let first = custody.capacity_report().structure;
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    let bank = custody.recover_on_control(&token).unwrap();
    let next = begin(&mut custody, &project, bank);
    let reused = custody.capacity_report();
    assert_eq!(reused.structure.previous_growth_backing_bytes, 0);
    assert_eq!(reused.structure.scene_bytes, first.scene_bytes);
    assert_eq!(
        reused.construction_peak_bytes,
        reused.retained_stage_bytes + reused.previous_slot_backing_bytes
    );
    assert!(custody.cancel(&next));
    custody.cleanup_on_control();
    let bank = custody.recover_on_control(&next).unwrap();
    let arrange = custody
        .begin_arrangement_on_control(&project, 11, bank)
        .ok()
        .unwrap();
    let report = custody.capacity_report();
    assert_eq!(report.structure.scene_bytes, first.scene_bytes);
    assert_eq!(report.structure.name_backing_bytes, 0);
    assert_eq!(report.structure.unresolved_owned_allocations, 6);
    assert!(custody.cancel(&arrange));
    custody.cleanup_on_control();
    let mut bank = custody.recover_on_control(&arrange).unwrap();
    let pointer = bank.sources.as_ptr();
    bank.reservation.max_prepared_data_bytes = 0;
    let refusal = custody
        .begin_input_on_control(
            AudioArrangementInput::new(&project, 11, bank).with_session_sources(),
        )
        .err()
        .unwrap();
    assert_eq!(refusal.reason, AudioStorageError::ReservationExceeded);
    assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
    assert!(refusal.inputs.source_view(&source_token).is_ok());
    let overflow = crate::SessionStructureCapacity {
        scene_bytes: usize::MAX,
        track_bytes: 1,
        ..Default::default()
    };
    assert_eq!(overflow.bytes(), Err(AudioStorageError::Overflow));
    eprintln!(
        "hex Session preparation evidence: structure_retained_capacity; supported-controls=completed"
    );
}
