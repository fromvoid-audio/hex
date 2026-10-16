use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub(super) fn supported(test: &str) -> bool {
    match OwnedAudioSource::try_resident(48_000, vec![[0.0; 2]; 8]) {
        Ok(_) => {
            eprintln!("hex Session preparation evidence: {test}; branch=supported-constructor");
            true
        }
        Err(refusal) => {
            assert_eq!(refusal.reason, AudioStorageError::UnsupportedConstructor);
            eprintln!("hex Session preparation evidence: {test}; branch=unsupported-refusal");
            false
        }
    }
}
pub(super) fn fixture(
    streamed: bool,
) -> (hex_project::Project, OwnedAudioInputs, AudioSourceToken) {
    use hex_project::{Command, Created, Editor};
    let (project, bank) = super::tests::fixture(1);
    let track = project.tracks()[0].id();
    let asset = project.assets().next().unwrap().id();
    let mut editor = Editor::new(project, 32).unwrap();
    for (index, name) in ["session α", "session β"].iter().enumerate() {
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: format!("scene {index}"),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: (*name).into(),
                asset,
                source_start: index as u64,
                source_end: 8,
            })
            .unwrap();
    }
    let mut bank = if streamed {
        let reservation = bank.reservation;
        drop(bank);
        let metadata = crate::StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(11, asset),
            48_000,
            48_000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            [7; 32],
        )
        .unwrap();
        let source = OwnedAudioSource::try_streamed(metadata).ok().unwrap();
        OwnedAudioInputs::try_new(vec![(asset, source)], reservation)
            .ok()
            .unwrap()
    } else {
        bank
    };
    let token = bank.source_token(asset).unwrap();
    let _alias = bank.alias_on_control(&token).unwrap(); // actual caller alias remains bank-owned
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
fn actual_resident_streamed_cached_aliases_names_and_reserved_handle_destinations_survive_recovery()
{
    if !supported("resident_streamed_cache") {
        return;
    }
    for streamed in [false, true] {
        let (project, bank, original_token) = fixture(streamed);
        let pointer = bank.sources.as_ptr();
        let capacity = bank.sources.capacity();
        let bytes = crate::audio_input_storage::parts(&bank.sources[0].1).unwrap();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = begin(&mut custody, &project, bank);
        assert_eq!(
            custody.phase(),
            AudioPreparationPhase::ArrangementAndSessionSources
        );
        assert_eq!(custody.installed_source_bindings(), 1);
        assert_eq!(custody.installed_session_source_bindings(), (2, 1));
        assert_eq!(
            custody
                .capacity_report()
                .session
                .reserved_handle_destinations,
            3
        );
        assert_eq!(
            custody.capacity_report().declared_source_bytes,
            bytes.0 + bytes.1
        );
        assert!(
            custody
                .session
                .validate_actual_aliases(custody.bank.as_ref().unwrap())
        );
        assert_eq!(
            custody.session.names().collect::<Vec<_>>(),
            ["session α", "session β"]
        );
        assert!(custody.capacity_report().session.name_backing_bytes >= 18);
        let retained = custody.capacity_report();
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert_eq!(custody.installed_session_source_bindings(), (0, 0));
        assert_eq!(custody.capacity_report(), retained); // no eager data/claim subtraction
        let bank = custody.recover_on_control(&token).unwrap();
        assert_eq!(bank.sources.as_ptr(), pointer);
        assert_eq!(bank.sources.capacity(), capacity);
        assert_eq!(bank.owning_handle_count(), 2);
        assert!(bank.source_view(&original_token).is_ok());
    }
    eprintln!(
        "hex Session preparation evidence: resident_streamed_cache; supported-controls=completed; cache=1; cells=2; independent-handles=3"
    );
}

#[test]
fn every_session_alias_failure_and_late_interval_or_name_failure_retains_original_bank() {
    if !supported("late_session_failures") {
        return;
    }
    for failure in 0..4 {
        let (project, bank, original_token) = fixture(false);
        let pointer = bank.sources.as_ptr();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        match failure {
            0 | 1 => custody.session.fail_after_alias = Some(failure),
            2 => custody.session.bad_interval = Some(0),
            _ => custody.session.fail_name = Some(0),
        }
        let token = begin(&mut custody, &project, bank);
        assert_eq!(custody.phase(), AudioPreparationPhase::Cleaning);
        assert_eq!(
            custody.installed_session_source_bindings(),
            (if failure == 1 { 2 } else { 1 }, 1)
        );
        assert!(custody.recover_on_control(&token).is_none());
        custody.cleanup_on_control();
        let bank = custody.recover_on_control(&token).unwrap();
        assert_eq!(bank.sources.as_ptr(), pointer);
        assert_eq!(bank.owning_handle_count(), 2);
        assert!(bank.source_view(&original_token).is_ok());
    }
    eprintln!(
        "hex Session preparation evidence: late_session_failures; supported-controls=completed; injected-phases=4"
    );
}

#[test]
fn name_reservation_and_alias_pressure_refuse_before_transfer_with_original_token_capacity() {
    if !supported("session_preflight_refusal") {
        return;
    }
    for name_failure in [true, false] {
        let (project, mut bank, original_token) = fixture(false);
        let pointer = bank.sources.as_ptr();
        let original_capacity = bank.sources.capacity();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        if name_failure {
            custody.session.fail_name_reserve = Some(1);
        } else {
            bank.reservation.handle_obligations = 4;
        }
        let reservation = bank.reservation;
        let refusal = custody
            .begin_input_on_control(
                AudioArrangementInput::new(&project, 11, bank).with_session_sources(),
            )
            .err()
            .unwrap();
        assert_eq!(
            refusal.reason,
            if name_failure {
                AudioStorageError::Allocation
            } else {
                AudioStorageError::Capacity
            }
        );
        assert_eq!(custody.phase(), AudioPreparationPhase::Vacant);
        assert_eq!(custody.installed_source_bindings(), 0);
        assert_eq!(custody.installed_session_source_bindings(), (0, 0));
        assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
        assert_eq!(refusal.inputs.sources.capacity(), original_capacity);
        assert_eq!(refusal.inputs.reservation, reservation);
        assert!(refusal.inputs.source_view(&original_token).is_ok());
        assert_eq!(refusal.inputs.owning_handle_count(), 2);
        assert!(refusal.capacity.session.cell_capacity >= 2);
    }
    eprintln!(
        "hex Session preparation evidence: session_preflight_refusal; supported-controls=completed"
    );
}

#[test]
fn session_construction_panic_pins_bank_cache_partial_cells_and_first_payload_without_retry() {
    if !supported("session_construction_panic") {
        return;
    }
    let (project, bank, _) = fixture(true);
    let pointer = bank.sources.as_ptr();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    custody.session.panic_after_alias = Some(0);
    let token = begin(&mut custody, &project, bank);
    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    assert_eq!(custody.installed_session_source_bindings(), (1, 1));
    assert_eq!(custody.bank.as_ref().unwrap().sources.as_ptr(), pointer);
    let payload = custody.first_panic.as_ref().unwrap().as_ref() as *const dyn Any;
    custody.cleanup_on_control();
    assert!(!custody.cancel(&token));
    assert!(custody.recover_on_control(&token).is_none());
    assert!(std::ptr::eq(
        payload,
        custody.first_panic.as_ref().unwrap().as_ref()
    ));
    assert_eq!(custody.installed_session_source_bindings(), (1, 1));
    eprintln!(
        "hex Session preparation evidence: session_construction_panic; supported-controls=completed"
    );
}

#[test]
fn actual_session_disposal_panic_retains_remaining_cache_cells_bank_and_tombstone() {
    if !supported("session_destructor_panic") {
        return;
    }
    let (project, bank, _) = fixture(false);
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut custody, &project, bank);
    let count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&count);
    let thread = std::thread::current().id();
    custody.session.set_disposal_probe(
        0,
        DropProbe(Some(Box::new(move || {
            assert_eq!(std::thread::current().id(), thread);
            observed.fetch_add(1, Ordering::Relaxed);
            panic!("actual persistent Session slot field destructor");
        }))),
    );
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    assert_eq!(custody.installed_session_source_bindings(), (1, 1));
    assert!(custody.bank.is_some());
    assert!(custody.first_panic.is_some());
    custody.cleanup_on_control();
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert!(custody.recover_on_control(&token).is_none());
    eprintln!(
        "hex Session preparation evidence: session_destructor_panic; supported-controls=completed; destructor-count=1"
    );
}
#[test]
fn retained_session_spare_capacity_is_charged_once_for_reuse_and_arrangement_only_next_stage() {
    if !supported("session_retained_capacity_reuse") {
        return;
    }
    let (project, bank, original_token) = fixture(false);
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let first = begin(&mut custody, &project, bank);
    let old = custody.capacity_report();
    assert!(custody.cancel(&first));
    custody.cleanup_on_control();
    let bank = custody.recover_on_control(&first).unwrap();
    let next = begin(&mut custody, &project, bank);
    let reused = custody.capacity_report();
    assert_eq!(reused.session.cell_bytes, old.session.cell_bytes);
    assert_eq!(reused.session.cache_bytes, old.session.cache_bytes);
    assert_eq!(reused.session.previous_backing_bytes, 0);
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
    assert_eq!(report.session.cell_bytes, old.session.cell_bytes);
    assert_eq!(report.session.cache_bytes, old.session.cache_bytes);
    assert_eq!(report.session.name_backing_bytes, 0);
    assert_eq!(report.session.reserved_handle_destinations, 0);
    assert_eq!(custody.installed_session_source_bindings(), (0, 0));
    assert!(custody.cancel(&arrange));
    custody.cleanup_on_control();
    let bank = custody.recover_on_control(&arrange).unwrap();
    assert!(bank.source_view(&original_token).is_ok());
    eprintln!(
        "hex Session preparation evidence: session_retained_capacity_reuse; supported-controls=completed"
    );
}
