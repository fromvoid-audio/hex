use super::*;
use hex_project::{Command, Created, Editor};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn fixture(
    streamed: bool,
    distinct: bool,
) -> (hex_project::Project, OwnedAudioInputs, AudioSourceToken) {
    let (project, bank) = super::tests::fixture(1);
    let asset = project.assets().next().unwrap().id();
    let track = project.tracks()[0].id();
    let mut editor = Editor::new(project, 32).unwrap();
    let Created::Scene(first) = editor
        .execute(Command::AddScene {
            name: "first".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::Scene(second) = editor
        .execute(Command::AddScene {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let second_asset = if distinct {
        let Created::Asset(id) = editor
            .execute(Command::AddAsset {
                path: "other.wav".into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        id
    } else {
        asset
    };
    // Intentionally opposite clip-ID / canonical cell order.
    for (scene, asset, name) in [
        (second, second_asset, "second audio"),
        (first, asset, "first audio"),
    ] {
        editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: name.into(),
                asset,
                source_start: 0,
                source_end: 8,
            })
            .unwrap();
    }
    let Created::Scene(stop) = editor
        .execute(Command::AddScene {
            name: "stop".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    editor
        .execute(Command::SetSessionStopCell { track, scene: stop })
        .unwrap();
    let mut sources = Vec::new();
    for id in if distinct {
        vec![asset, second_asset]
    } else {
        vec![asset]
    } {
        let source = if streamed {
            OwnedAudioSource::try_streamed(
                crate::StreamedSourceMetadata::new(
                    crate::StreamSourceKey::new(11, id),
                    48_000,
                    48_000,
                    8,
                    8,
                    hex_project::IDENTITY_CONVERSION,
                    [9; 32],
                )
                .unwrap(),
            )
            .ok()
            .unwrap()
        } else {
            OwnedAudioSource::try_resident(48_000, vec![[0.25; 2]; 8])
                .ok()
                .unwrap()
        };
        sources.push((id, source));
    }
    let reservation = bank.reservation;
    drop(bank);
    let mut bank = OwnedAudioInputs::try_new(sources, reservation)
        .ok()
        .unwrap();
    let token = bank.source_token(asset).unwrap();
    let _caller_alias = bank.alias_on_control(&token).unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.session.cells.reverse();
    (
        hex_project::Project::from_snapshot(snapshot).unwrap(),
        bank,
        token,
    )
}
fn begin(
    c: &mut AudioPreparationCustody,
    p: &hex_project::Project,
    b: OwnedAudioInputs,
) -> Result<AudioPreparationToken, Box<AudioArrangementRefusal>> {
    c.begin_input_on_control(AudioArrangementInput::new(p, 11, b).with_session_inventory())
}
#[test]
fn real_final_inventory_preserves_sorted_pairs_canonical_cells_and_exact_independent_handles() {
    if !super::session_source_tests::supported("final_session_inventory") {
        return;
    }
    for streamed in [false, true] {
        for distinct in [false, true] {
            let (project, bank, source_token) = fixture(streamed, distinct);
            let pointer = bank.sources.as_ptr();
            let capacity = bank.sources.capacity();
            let reservation = bank.reservation;
            let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
            let token = begin(&mut c, &project, bank).ok().unwrap();
            assert_eq!(
                c.phase(),
                AudioPreparationPhase::ArrangementAndSessionSources
            );
            assert!(c.assembly.sort_changed());
            c.assembly.validate_pairs(c.bank.as_ref().unwrap()).unwrap();
            assert_eq!(
                c.assembly.original_clip_handles(),
                c.assembly.actual_clip_handles()
            );
            let handles = c.assembly.actual_clip_handles();
            assert_eq!(handles.len(), 2);
            assert_ne!(handles[0], handles[1]);
            assert_eq!(c.assembly.clip_data_alias(), !distinct);
            let view = c.session_inventory_metadata().unwrap();
            assert_eq!(
                view.scene_names().map(|(_, n)| n).collect::<Vec<_>>(),
                ["first", "second", "stop"]
            );
            assert_eq!(
                view.track_ids().collect::<Vec<_>>(),
                project.tracks().iter().map(|t| t.id()).collect::<Vec<_>>()
            );
            let ids = view.audio_clip_ids().collect::<Vec<_>>();
            assert!(ids[0] < ids[1]);
            assert_eq!(view.contains_streamed_sources(), streamed);
            for cell in project.session_cells() {
                let expected = match cell.content() {
                    hex_project::SessionCellContent::Audio(a) => {
                        crate::PreparedSessionCellContent::Audio(a.id())
                    }
                    hex_project::SessionCellContent::Stop => {
                        crate::PreparedSessionCellContent::Stop
                    }
                    _ => panic!("unsupported"),
                };
                assert_eq!(view.cell(cell.track(), cell.scene()), Some(expected));
            }
            assert_eq!(view.streamed_source(ids[0]).is_some(), streamed);
            assert_eq!(
                c.installed_session_source_bindings(),
                (2, if distinct { 2 } else { 1 })
            );
            let report = c.capacity_report();
            assert_eq!(report.assembly, c.assembly.capacity(true).unwrap());
            assert!(report.assembly.bytes().unwrap() > 0);
            // Existing name charge groups follow the moved allocations, not empty
            // staging strings plus a second charge for final strings.
            assert_eq!(
                c.assembly.final_names_bytes(),
                report.session.name_backing_bytes + report.structure.name_backing_bytes
            );
            if distinct {
                let expected = c
                    .bank
                    .as_ref()
                    .unwrap()
                    .sources
                    .iter()
                    .map(|(_, s)| {
                        let (p, o) = crate::audio_input_storage::parts(s).unwrap();
                        p + o
                    })
                    .sum::<usize>();
                assert_eq!(report.declared_source_bytes, expected);
            }
            assert!(c.cancel(&token));
            c.cleanup_on_control();
            assert!(c.session_inventory_metadata().is_none());
            let bank = c.recover_on_control(&token).unwrap();
            assert_eq!(bank.sources.as_ptr(), pointer);
            assert_eq!(bank.sources.capacity(), capacity);
            assert_eq!(bank.reservation, reservation);
            assert!(bank.source_view(&source_token).is_ok());
        }
    }
    eprintln!(
        "hex Session preparation evidence: final_session_inventory; supported-controls=completed; resident-streamed-distinct-shared=4"
    );
}
#[test]
fn occupied_and_mismatched_final_destinations_refuse_before_staging_owners_move() {
    if !super::session_source_tests::supported("final_session_refusal") {
        return;
    }
    for occupied in [false, true] {
        let (project, bank, source_token) = fixture(false, false);
        let pointer = bank.sources.as_ptr();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        c.assembly.occupy_name = occupied;
        c.assembly.wrong_handle = !occupied;
        let token = begin(&mut c, &project, bank).ok().unwrap();
        assert_eq!(c.phase(), AudioPreparationPhase::Cleaning);
        assert_eq!(c.reason(), Some(AudioStorageError::SourceMismatch));
        assert_eq!(c.session.counts(), (2, 1));
        assert_eq!(c.assembly.installed_clip_handles(), 0);
        assert_eq!(
            c.session.names().collect::<Vec<_>>(),
            ["first audio", "second audio"]
        );
        assert!(c.session_inventory_metadata().is_none());
        c.cleanup_on_control();
        let bank = c.recover_on_control(&token).unwrap();
        assert_eq!(bank.sources.as_ptr(), pointer);
        assert!(bank.source_view(&source_token).is_ok());
    }
    eprintln!(
        "hex Session preparation evidence: final_session_refusal; supported-controls=completed; originals=retained"
    );
}
#[test]
fn every_move_failure_and_late_pack_failure_cleanup_partial_or_installed_owners_before_bank_recovery()
 {
    if !super::session_source_tests::supported("final_session_move_failures") {
        return;
    }
    // 3 scene names, track name, 2 source rows, shared/tempo, final pack.
    for phase in 0..8 {
        let (project, bank, source_token) = fixture(false, false);
        let pointer = bank.sources.as_ptr();
        let capacity = bank.sources.capacity();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        c.assembly.fail_after_move = Some(phase);
        let token = begin(&mut c, &project, bank).ok().unwrap();
        assert_eq!(c.phase(), AudioPreparationPhase::Cleaning, "phase {phase}");
        assert!(c.session_inventory_metadata().is_none());
        c.cleanup_on_control();
        let bank = c.recover_on_control(&token).unwrap();
        assert_eq!(bank.sources.as_ptr(), pointer);
        assert_eq!(bank.sources.capacity(), capacity);
        assert!(bank.source_view(&source_token).is_ok());
        c.assembly.fail_after_move = None;
        let next = begin(&mut c, &project, bank).ok().unwrap();
        assert!(c.session_inventory_metadata().is_some());
        assert_eq!(
            c.capacity_report().assembly.previous_growth_backing_bytes,
            0
        );
        assert!(c.cancel(&next));
        c.cleanup_on_control();
        assert!(c.recover_on_control(&next).is_some());
    }
    eprintln!(
        "hex Session preparation evidence: final_session_move_failures; supported-controls=completed; phases=8"
    );
}
#[test]
fn partial_and_installed_panic_keep_actual_owners_payload_and_bank_no_retry() {
    if !super::session_source_tests::supported("final_session_panic") {
        return;
    }
    for phase in [0, 4, 7] {
        let (project, bank, _) = fixture(false, false);
        let pointer = bank.sources.as_ptr();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        c.assembly.panic_after_move = Some(phase);
        let token = begin(&mut c, &project, bank).ok().unwrap();
        assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
        assert!(c.first_panic.is_some());
        assert_eq!(c.bank.as_ref().unwrap().sources.as_ptr(), pointer);
        assert!(c.session_inventory_metadata().is_none());
        let before = c.assembly.capacity(false).unwrap();
        c.cleanup_on_control();
        assert!(!c.cancel(&token));
        assert!(c.recover_on_control(&token).is_none());
        assert_eq!(c.assembly.capacity(false).unwrap(), before);
    }
    eprintln!(
        "hex Session preparation evidence: final_session_panic; supported-controls=completed; partial-installed=3"
    );
}
#[test]
fn actual_final_source_disposal_panic_retains_separate_name_shared_timing_bank_and_tombstone() {
    if !super::session_source_tests::supported("final_session_disposal_panic") {
        return;
    }
    let (project, bank, _) = fixture(true, false);
    let pointer = bank.sources.as_ptr();
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut c, &project, bank).ok().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&count);
    let thread = std::thread::current().id();
    c.assembly.disposal_probe = Some(DropProbe(Some(Box::new(move || {
        assert_eq!(std::thread::current().id(), thread);
        observed.fetch_add(1, Ordering::Relaxed);
        panic!("actual persistent final source disposal probe");
    }))));
    assert!(c.cancel(&token));
    c.cleanup_on_control();
    assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(c.assembly.retained_after_disposal_panic());
    assert_eq!(c.bank.as_ref().unwrap().sources.as_ptr(), pointer);
    assert!(c.first_panic.is_some());
    assert_eq!(count.load(Ordering::Relaxed), 1);
    c.cleanup_on_control();
    assert!(c.recover_on_control(&token).is_none());
    assert_eq!(count.load(Ordering::Relaxed), 1);
    eprintln!(
        "hex Session preparation evidence: final_session_disposal_panic; supported-controls=completed; destructor-count=1"
    );
}
#[test]
fn final_target_spare_capacity_and_old_new_overlap_are_charged_on_reuse_and_refusal() {
    if !super::session_source_tests::supported("final_session_capacity") {
        return;
    }
    let (project, bank, source_token) = fixture(false, false);
    let pointer = bank.sources.as_ptr();
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut c, &project, bank).ok().unwrap();
    let report = c.capacity_report();
    assert!(report.assembly.bytes().unwrap() > 0);
    assert!(c.cancel(&token));
    c.cleanup_on_control();
    let bank = c.recover_on_control(&token).unwrap();
    let next = begin(&mut c, &project, bank).ok().unwrap();
    assert_eq!(
        c.capacity_report().assembly.previous_growth_backing_bytes,
        0
    );
    assert_eq!(
        c.capacity_report().assembly.bytes(),
        report.assembly.bytes()
    );
    assert!(c.cancel(&next));
    c.cleanup_on_control();
    let bank = c.recover_on_control(&next).unwrap();
    let old = c
        .begin_arrangement_on_control(&project, 11, bank)
        .ok()
        .unwrap();
    assert_eq!(
        c.capacity_report().assembly.bytes(),
        report.assembly.bytes()
    );
    assert!(c.cancel(&old));
    c.cleanup_on_control();
    let bank = c.recover_on_control(&old).unwrap();
    let mut editor = Editor::new(project.clone(), 16).unwrap();
    let asset = project.assets().next().unwrap().id();
    let track = project.tracks()[0].id();
    for index in 0..2 {
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: format!("growth {index}"),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: format!("growth audio {index}"),
                asset,
                source_start: 0,
                source_end: 8,
            })
            .unwrap();
    }
    let grown = begin(&mut c, editor.project(), bank).ok().unwrap();
    let grown_report = c.capacity_report();
    assert!(grown_report.assembly.previous_growth_backing_bytes > 0);
    assert_eq!(
        grown_report.construction_peak_bytes,
        grown_report.retained_stage_bytes
            + grown_report.previous_slot_backing_bytes
            + grown_report.session.previous_backing_bytes
            + grown_report.structure.previous_growth_backing_bytes
            + grown_report.tempo.previous_growth_backing_bytes
            + grown_report.assembly.previous_growth_backing_bytes
    );
    assert!(c.cancel(&grown));
    c.cleanup_on_control();
    let mut bank = c.recover_on_control(&grown).unwrap();
    bank.reservation.max_prepared_data_bytes = report.retained_stage_bytes - 1;
    let mut fresh = AudioPreparationCustody::try_new_on_control().unwrap();
    let refusal = begin(&mut fresh, &project, bank).err().unwrap();
    assert_eq!(refusal.reason, AudioStorageError::ReservationExceeded);
    assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
    assert!(refusal.inputs.source_view(&source_token).is_ok());
    assert!(refusal.capacity.assembly.bytes().unwrap() > 0);
    let overflow = crate::SessionAssemblyCapacity {
        scene_bytes: usize::MAX,
        pair_bytes: 1,
        ..Default::default()
    };
    assert_eq!(overflow.bytes(), Err(AudioStorageError::Overflow));
    eprintln!(
        "hex Session preparation evidence: final_session_capacity; supported-controls=completed"
    );
}

#[test]
fn final_pair_transfer_adds_no_control_arc_clone_beyond_actual_staging_positive_control() {
    if !super::session_source_tests::supported("final_session_clone_conservation") {
        return;
    }
    let mut counts = Vec::new();
    for assemble in [false, true] {
        let (project, bank, _) = fixture(true, false);
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        crate::owned_shared::reset_clone_count();
        let input = AudioArrangementInput::new(&project, 11, bank);
        let input = if assemble {
            input.with_session_inventory()
        } else {
            input.with_session_sources()
        };
        let token = c.begin_input_on_control(input).ok().unwrap();
        counts.push(crate::owned_shared::clone_count());
        assert!(counts.last().copied().unwrap() > 0); // real clone counter positive control
        if assemble {
            let expected = project
                .session_cells()
                .iter()
                .map(|c| {
                    (
                        c.track(),
                        c.scene(),
                        match c.content() {
                            hex_project::SessionCellContent::Audio(a) => {
                                crate::PreparedSessionCellContent::Audio(a.id())
                            }
                            hex_project::SessionCellContent::Stop => {
                                crate::PreparedSessionCellContent::Stop
                            }
                            _ => panic!("midi"),
                        },
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(c.assembly.cell_order(), expected);
            assert!(c.session_inventory_metadata().is_some());
        }
        assert!(c.cancel(&token));
        c.cleanup_on_control();
        assert!(c.recover_on_control(&token).is_some());
    }
    assert_eq!(counts[0], counts[1]);
    eprintln!(
        "hex Session preparation evidence: final_session_clone_conservation; supported-controls=completed; staging-clones={}",
        counts[0]
    );
}
