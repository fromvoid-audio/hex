use super::*;
use crate::audio_preparation::{
    AudioArrangementInput, AudioPreparationCustody, AudioPreparationPhase,
};
use hex_project::{Command, Created, Editor};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn supported(name: &str) -> bool {
    crate::audio_preparation::session_source_tests::supported(name)
}
fn fixture(streamed: bool) -> (hex_project::Project, OwnedAudioInputs) {
    let (project, bank) = super::tests::fixture(streamed);
    let track = project.tracks()[0].id();
    let arrangement_clip = project.tracks()[0].clips()[0].id();
    let asset = project.assets().next().unwrap().id();
    let mut editor = Editor::new(project, 8).unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "audio scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "source".into(),
            asset,
            source_start: 1,
            source_end: 7,
        })
        .unwrap();
    editor
        .execute(Command::SetMeterPoint {
            beat: hex_project::BeatTime::from_quarters(3).unwrap(),
            meter: hex_project::Meter::new(7, 8).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipGain {
            clip: arrangement_clip,
            millidb: -6000,
        })
        .unwrap();
    let session_clip = match editor
        .project()
        .session_cells()
        .iter()
        .next()
        .unwrap()
        .content()
    {
        hex_project::SessionCellContent::Audio(clip) => clip.id(),
        _ => panic!("audio"),
    };
    editor
        .execute(Command::SetAudioClipGain {
            clip: session_clip,
            millidb: -3000,
        })
        .unwrap();
    let Created::Scene(stop) = editor
        .execute(Command::AddScene {
            name: "stop scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    editor
        .execute(Command::SetSessionStopCell { track, scene: stop })
        .unwrap();
    (
        hex_project::Project::from_snapshot(editor.project().snapshot()).unwrap(),
        bank,
    )
}
fn begin(
    c: &mut AudioPreparationCustody,
    project: &hex_project::Project,
    bank: OwnedAudioInputs,
) -> Result<crate::AudioPreparationToken, Box<crate::AudioArrangementRefusal>> {
    c.begin_input_on_control(
        AudioArrangementInput::new(project, 11, bank).with_audio_prepared_revision(),
    )
}
#[test]
fn four_actual_revision_roots_preserve_pcm_session_sources_aliases_and_capacity() {
    if !supported("private_prepared_revision_pcm") {
        return;
    }
    for streamed in [false, true] {
        let (project, bank) = fixture(streamed);
        let ptr = bank.sources.as_ptr();
        let cap = bank.sources.capacity();
        let reservation = bank.reservation;
        let source_token = bank
            .source_token(project.assets().next().unwrap().id())
            .unwrap();
        let sources: BTreeMap<_, _> = bank
            .sources
            .iter()
            .map(|(id, source)| {
                (
                    *id,
                    match &source.kind {
                        SourceKind::Resident(audio) => {
                            crate::compile::PreparedAudioSource::Resident(audio.clone())
                        }
                        SourceKind::Streamed(meta) => {
                            crate::compile::PreparedAudioSource::Streamed(Arc::clone(meta.arc()))
                        }
                    },
                )
            })
            .collect();
        let expected = crate::PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &sources,
            11,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = begin(&mut custody, &project, bank).ok().unwrap();
        assert_eq!(
            custody.phase(),
            AudioPreparationPhase::ArrangementAndSessionSources
        );
        let owner = custody.revision.owner_for_test().unwrap();
        let revision = owner.as_ref();
        assert_eq!(Arc::strong_count(owner.arc()), 1);
        assert_eq!(Arc::strong_count(revision.arrangement()), 1);
        assert_eq!(Arc::strong_count(revision.session()), 1);
        assert_eq!(Arc::strong_count(revision.midi_preview()), 1);
        let root = revision.arrangement().as_ref();
        let view = custody.revision.storage_view().unwrap();
        assert!(std::ptr::eq(root, view.arrangement.unwrap()));
        let session = view.session.unwrap();
        assert!(Arc::ptr_eq(
            root.shared_contexts.arc(),
            session.shared_contexts()
        ));
        assert_eq!(Arc::strong_count(root.shared_contexts.arc()), 2);
        assert_eq!(session.meter_map(), project.meter_map());
        assert_eq!(
            session.audio_clips()[0].gain,
            expected.session().audio_clips()[0].gain
        );
        assert_eq!(
            custody.capacity_report().assembly.meter_owned_allocations,
            2
        );
        assert!(custody.capacity_report().assembly.meter_bytes > 0);
        assert_eq!(session.audio_clips().len(), 1);
        assert_eq!(session.cells().len(), 2);
        assert_eq!(session.cells(), expected.session().cells());
        assert_eq!(
            session.audio_clips()[0].source_start,
            expected.session().audio_clips()[0].source_start
        );
        assert_eq!(
            session.audio_clips()[0].source_end,
            expected.session().audio_clips()[0].source_end
        );
        let mut renderer = crate::Renderer::new(root);
        renderer.play();
        let mut oracle = crate::Renderer::new(expected.arrangement());
        oracle.play();
        for length in [3, 17, 1, 31] {
            let mut actual = vec![[0.0; 2]; length];
            let mut expected = actual.clone();
            renderer
                .render_with_pages(&mut actual, &mut super::tests::Pages)
                .unwrap();
            oracle
                .render_with_pages(&mut expected, &mut super::tests::Pages)
                .unwrap();
            assert_eq!(actual, expected);
        }
        let report = custody.capacity_report();
        assert_eq!(report.revision_roots.actual_allocations, 4);
        assert_eq!(report.revision_roots.reserved_handle_destinations, 4);
        assert_eq!(
            report.revision_roots.actual_payload_bytes
                + report.revision_roots.actual_declared_overhead_bytes,
            report.revision_roots.reserved_data_bytes
        );
        assert!(report.revision_roots.constructed);
        assert_eq!(
            report.arrangement_clips,
            custody
                .arrangement_clips
                .capacity_at(true, view.arrangement, view.preview)
                .unwrap()
        );
        assert_eq!(
            report.assembly,
            custody.assembly.capacity_at(true, view.session).unwrap()
        );
        assert_eq!(
            report.shared,
            custody
                .shared
                .observe_at(
                    custody.arrangement_clips.root_context_at(view.arrangement),
                    custody.assembly.shared_storage_at(view.session).unwrap()
                )
                .unwrap()
        );
        assert_eq!(report.shared.installed_handle_obligations, 3);
        assert!(report.assembly.audio_bytes > 0);
        assert_eq!(custody.installed_source_bindings(), 2);
        assert_eq!(custody.installed_session_source_bindings().0, 1);
        assert_eq!(
            custody
                .audio_prepared_revision_metadata()
                .unwrap()
                .contains_streamed_sources,
            streamed
        );
        assert_eq!(custody.audio_preview_metadata().unwrap().clip_count, 0);
        assert_eq!(
            custody
                .session_inventory_metadata()
                .unwrap()
                .audio_clip_ids()
                .count(),
            1
        );
        assert!(custody.arrangement_track_metadata(0).is_some());
        assert!(custody.arrangement_return_metadata().is_some());
        custody
            .assembly
            .validate_pairs_at(custody.bank.as_ref().unwrap(), view.session)
            .unwrap();
        custody
            .arrangement_clips
            .validate_root_sources_at(
                &custody.slots,
                custody.bank.as_ref().unwrap(),
                view.arrangement,
            )
            .unwrap();
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::Recoverable);
        assert_eq!(
            custody.revision.capacity().unwrap(),
            crate::AudioRevisionRootCapacity::default()
        );
        let recovered = custody.recover_on_control(&token).unwrap();
        assert_eq!(recovered.sources.as_ptr(), ptr);
        assert_eq!(recovered.sources.capacity(), cap);
        assert_eq!(recovered.reservation, reservation);
        assert!(recovered.source_view(&source_token).is_ok());
    }
    eprintln!(
        "hex preparation evidence: private_prepared_revision_pcm; supported-controls=completed; four-actual-roots; resident-streamed-parity; exact-bank-restored"
    );
}
#[test]
fn every_private_revision_reserve_transfer_and_install_error_or_panic_retains_custody() {
    if !supported("private_prepared_revision_frontiers") {
        return;
    }
    for panic in [false, true] {
        for frontier in 0..12 {
            let (project, bank) = fixture(true);
            let ptr = bank.sources.as_ptr();
            let cap = bank.sources.capacity();
            let reservation = bank.reservation;
            let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
            if panic {
                c.revision.panic_after = Some(frontier);
            } else {
                c.revision.fail_after = Some(frontier);
            }
            match begin(&mut c, &project, bank) {
                Err(refusal) => {
                    assert!(frontier < 4);
                    assert_eq!(refusal.inputs.sources.as_ptr(), ptr);
                    assert_eq!(refusal.inputs.sources.capacity(), cap);
                    assert_eq!(refusal.inputs.reservation, reservation);
                    assert_eq!(
                        c.phase(),
                        if panic {
                            AudioPreparationPhase::FailedIncomplete
                        } else {
                            AudioPreparationPhase::Vacant
                        }
                    );
                }
                Ok(token) => {
                    assert!(frontier >= 4);
                    let before = c.revision.capacity().unwrap();
                    let retained = c.bank.as_ref().unwrap();
                    assert_eq!(retained.sources.as_ptr(), ptr);
                    assert_eq!(retained.sources.capacity(), cap);
                    assert_eq!(retained.reservation, reservation);
                    if panic {
                        assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
                        let first =
                            c.first_panic.as_ref().unwrap().as_ref() as *const dyn std::any::Any;
                        assert!(!c.cancel(&token));
                        c.cleanup_on_control();
                        c.close_on_control();
                        c.cleanup_on_control();
                        assert_eq!(c.revision.capacity().unwrap(), before);
                        assert!(std::ptr::eq(
                            first,
                            c.first_panic.as_ref().unwrap().as_ref()
                        ));
                        assert!(c.recover_on_control(&token).is_none());
                    } else {
                        assert_eq!(c.phase(), AudioPreparationPhase::Cleaning);
                        c.cleanup_on_control();
                        assert_eq!(
                            c.phase(),
                            AudioPreparationPhase::Recoverable,
                            "frontier {frontier}"
                        );
                        let recovered = c.recover_on_control(&token).unwrap();
                        assert_eq!(recovered.sources.as_ptr(), ptr);
                        assert_eq!(recovered.sources.capacity(), cap);
                        assert_eq!(recovered.reservation, reservation);
                    }
                }
            }
        }
    }
    eprintln!(
        "hex preparation evidence: private_prepared_revision_frontiers; supported-controls=completed; twelve-error-panic-frontiers; original-bank-or-retained-owner"
    );
}
#[test]
fn root_wrapping_adds_no_aliases_and_equal_content_revisions_have_distinct_roots() {
    if !supported("private_prepared_revision_aliases") {
        return;
    }
    let mut counts = Vec::new();
    let mut custodians = Vec::new();
    for rooted in [false, true, true] {
        let (project, bank) = fixture(true);
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        crate::owned_shared::reset_clone_count();
        let input = AudioArrangementInput::new(&project, 11, bank);
        let token = c
            .begin_input_on_control(if rooted {
                input.with_audio_prepared_revision()
            } else {
                input.with_audio_project_inventory()
            })
            .ok()
            .unwrap();
        counts.push(crate::owned_shared::clone_count());
        custodians.push((c, token));
    }
    assert_eq!(counts[0], counts[1]);
    assert_eq!(counts[1], counts[2]);
    let left = custodians[1].0.revision.owner_for_test().unwrap();
    let right = custodians[2].0.revision.owner_for_test().unwrap();
    assert!(!Arc::ptr_eq(left.arc(), right.arc()));
    assert!(!Arc::ptr_eq(left.arrangement(), right.arrangement()));
    assert!(!Arc::ptr_eq(left.session(), right.session()));
    assert!(!Arc::ptr_eq(left.midi_preview(), right.midi_preview()));
    for (c, token) in &mut custodians {
        assert!(c.cancel(token));
        c.cleanup_on_control();
        assert!(c.recover_on_control(token).is_some());
    }
}
#[test]
fn actual_alias_or_weak_refusal_pins_identical_revision_and_never_retries() {
    if !supported("private_prepared_revision_extraction") {
        return;
    }
    for weak in [false, true] {
        let (project, bank) = fixture(false);
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = begin(&mut c, &project, bank).ok().unwrap();
        let owner = c.revision.owner_for_test().unwrap();
        let ptr = Arc::as_ptr(owner.arc());
        let alias = (!weak).then(|| owner.clone_on_control());
        let observer = weak.then(|| Arc::downgrade(owner.arc()));
        assert!(c.cancel(&token));
        c.cleanup_on_control();
        assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
        assert_eq!(Arc::as_ptr(c.revision.owner_for_test().unwrap().arc()), ptr);
        assert_eq!(c.revision.capacity().unwrap().actual_allocations, 4);
        assert!(c.bank.is_some());
        drop(alias);
        drop(observer);
        c.cleanup_on_control();
        assert!(
            c.revision
                .restore(&mut c.arrangement_clips, &mut c.assembly)
                .is_err()
        );
        assert_eq!(Arc::as_ptr(c.revision.owner_for_test().unwrap().arc()), ptr);
        assert!(c.recover_on_control(&token).is_none());
    }
}
#[test]
fn child_alias_refusal_retains_partial_extraction_in_persistent_slots() {
    if !supported("private_prepared_revision_child_alias") {
        return;
    }
    for child in 0..3 {
        let (project, bank) = fixture(true);
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = begin(&mut c, &project, bank).ok().unwrap();
        let owner = c.revision.owner_for_test().unwrap();
        let arrangement = (child == 0).then(|| Arc::clone(owner.arrangement()));
        let session = (child == 1).then(|| Arc::clone(owner.session()));
        let preview = (child == 2).then(|| Arc::clone(owner.midi_preview()));
        let before = c.revision.storage_view().unwrap();
        let pointers = (
            before.arrangement.unwrap() as *const crate::Project,
            before.session.unwrap() as *const crate::PreparedSessionInventory,
            before.preview.unwrap() as *const crate::MidiArrangementPreview,
        );
        assert!(c.cancel(&token));
        c.cleanup_on_control();
        assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
        let report = c.revision.capacity().unwrap();
        assert_eq!(report.control_extracted_allocations, child + 1);
        assert_eq!(report.actual_allocations, 3 - child);
        let after = c.revision.storage_view().unwrap();
        // Extracted inline values move, while refused and subsequent Arc owners
        // retain their exact allocation. Every child remains present somewhere.
        assert!(after.arrangement.is_some() && after.session.is_some() && after.preview.is_some());
        if child == 0 {
            assert_eq!(
                after.arrangement.unwrap() as *const crate::Project,
                pointers.0
            );
        }
        if child <= 1 {
            assert_eq!(
                after.session.unwrap() as *const crate::PreparedSessionInventory,
                pointers.1
            );
        }
        assert_eq!(
            after.preview.unwrap() as *const crate::MidiArrangementPreview,
            pointers.2
        );
        c.arrangement_clips
            .validate_root_sources_at(&c.slots, c.bank.as_ref().unwrap(), after.arrangement)
            .unwrap();
        c.assembly
            .validate_pairs_at(c.bank.as_ref().unwrap(), after.session)
            .unwrap();
        drop(arrangement);
        drop(session);
        drop(preview);
        c.cleanup_on_control();
        assert!(
            c.revision
                .restore(&mut c.arrangement_clips, &mut c.assembly)
                .is_err()
        );
        assert_eq!(c.revision.capacity().unwrap(), report);
        assert!(c.recover_on_control(&token).is_none());
    }
}
#[test]
fn occupied_restore_destination_and_invalid_handle_preserve_all_four_owners() {
    if !supported("private_prepared_revision_restore_refusal") {
        return;
    }
    for invalid_handle in [false, true] {
        let (project, bank) = fixture(false);
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = begin(&mut c, &project, bank).ok().unwrap();
        let ptr = Arc::as_ptr(c.revision.owner_for_test().unwrap().arc());
        if invalid_handle {
            c.revision.corrupt_handle_for_test();
        } else {
            c.arrangement_clips
                .root
                .tracks
                .push(crate::Track::new(Vec::new(), 1.0, false).unwrap());
        }
        assert!(c.cancel(&token));
        c.cleanup_on_control();
        assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
        assert_eq!(Arc::as_ptr(c.revision.owner_for_test().unwrap().arc()), ptr);
        assert!(c.bank.is_some());
        c.cleanup_on_control();
        assert!(c.recover_on_control(&token).is_none());
    }
}
#[test]
fn child_destructor_panic_after_four_root_extraction_retains_bank_and_first_payload() {
    if !supported("private_prepared_revision_child_panic") {
        return;
    }
    let (project, bank) = fixture(false);
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&count);
    c.arrangement_clips.tracks.probe = Some(crate::audio_preparation::DropProbe(Some(Box::new(
        move || {
            observed.fetch_add(1, Ordering::SeqCst);
            panic!("revision child destructor");
        },
    ))));
    let token = begin(&mut c, &project, bank).ok().unwrap();
    assert!(c.cancel(&token));
    c.cleanup_on_control();
    assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(c.revision.capacity().unwrap().actual_allocations, 0);
    assert!(c.bank.is_some());
    let first = c.first_panic.as_ref().unwrap().as_ref() as *const dyn std::any::Any;
    c.cleanup_on_control();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(std::ptr::eq(
        first,
        c.first_panic.as_ref().unwrap().as_ref()
    ));
    assert!(c.recover_on_control(&token).is_none());
}

#[test]
fn four_root_reservations_refuse_handle_and_byte_pressure_before_bank_consumption() {
    if !supported("private_prepared_revision_reservations") {
        return;
    }
    let (project, mut bank) = fixture(false);
    // One bank, two Arrangement clips, cache+Session clip, two Track handles,
    // two Return handles, preview slice, three shared anchors and four roots.
    bank.reservation.handle_obligations = 17;
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut c, &project, bank).ok().unwrap();
    let report = c.capacity_report();
    assert!(report.revision_roots.reserved_data_bytes > 0);
    assert!(c.cancel(&token));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&token).is_some());
    for pressure in 0..3 {
        let (project, mut bank) = fixture(false);
        match pressure {
            0 => bank.reservation.handle_obligations = 16,
            1 => bank.reservation.max_prepared_data_bytes = report.retained_stage_bytes - 1,
            _ => bank.reservation.max_construction_data_bytes = report.construction_peak_bytes - 1,
        }
        let pointer = bank.sources.as_ptr();
        let capacity = bank.sources.capacity();
        let reservation = bank.reservation;
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        let refusal = begin(&mut c, &project, bank).err().unwrap();
        assert_eq!(
            refusal.reason,
            if pressure == 0 {
                AudioStorageError::Capacity
            } else {
                AudioStorageError::ReservationExceeded
            }
        );
        assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
        assert_eq!(refusal.inputs.sources.capacity(), capacity);
        assert_eq!(refusal.inputs.reservation, reservation);
        assert_eq!(c.phase(), AudioPreparationPhase::Vacant);
        assert_eq!(
            c.revision.capacity().unwrap(),
            crate::AudioRevisionRootCapacity::default()
        );
    }
}

#[test]
fn new_touch_graphs_refuse_before_source_moves() {
    if !supported("unsupported_main_graphs") {
        return;
    }
    let (project, bank) = fixture(false);
    let track = project.tracks()[0].id();
    let mut editor = Editor::new(project, 8).unwrap();
    let epoch = editor.project().tracks()[0].gain_lane().recording_epoch();
    editor
        .execute(Command::SpliceGainTouch {
            track,
            expected_epoch: epoch,
            span: hex_project::GainTouchSpan {
                source_start: hex_project::BeatTime::ZERO,
                start: hex_project::BeatTime::ZERO,
                end: hex_project::BeatTime::from_quarters(1).unwrap(),
                context: hex_project::GainTouchContext {
                    recording_epoch: epoch + 1,
                    project_generation: 1,
                    canonical_revision: 1,
                    audio_revision: 1,
                    transport_epoch: 1,
                    gesture_epoch: 1,
                    gesture_serial: 1,
                    sample_rate: editor.project().sample_rate(),
                    loop_pass: 0,
                },
                steps: vec![hex_project::GainTouchStep {
                    beat: hex_project::BeatTime::ZERO,
                    gain: 0.25,
                    origin_beat: hex_project::BeatTime::ZERO,
                    source_sequence: 1,
                }],
            },
        })
        .unwrap();
    let pointer = bank.sources.as_ptr();
    let count = bank.owning_handle_count();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    crate::owned_shared::reset_clone_count();
    let refused = begin(&mut custody, editor.project(), bank).err().unwrap();
    assert_eq!(refused.reason, AudioStorageError::UnsupportedGraph);
    assert_eq!(refused.inputs.sources.as_ptr(), pointer);
    assert_eq!(refused.inputs.owning_handle_count(), count);
    assert_eq!(crate::owned_shared::clone_count(), 0);
    assert_eq!(custody.phase(), AudioPreparationPhase::Vacant);
}

fn storage_reservation() -> crate::AudioStorageLedgerReservation {
    crate::AudioStorageLedgerReservation {
        data_rows: 1024,
        handle_rows: 128,
        root_rows: 8,
        max_retained_bytes: usize::MAX,
        max_construction_peak_bytes: usize::MAX,
    }
}
#[test]
fn borrowed_storage_inventory_covers_real_roots_without_retirement_admission() {
    if !supported("borrowed_storage_real_roots") {
        return;
    }
    for streamed in [false, true] {
        let (project, bank) = fixture(streamed);
        let bank_handles = bank.owning_handle_count();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        let token = begin(&mut c, &project, bank).ok().unwrap();
        let capacity = c.capacity_report();
        let clones = crate::owned_shared::clone_count();
        let mut ledger =
            crate::AudioRootStorageLedger::try_new_on_control(storage_reservation()).unwrap();
        ledger
            .borrow_custody_on_control(&c, crate::AudioStorageRole::Current)
            .unwrap();
        let summary = ledger.summary().unwrap();
        assert_eq!(summary.unknown_storage, 0);
        assert!(summary.require_known_storage().is_ok());
        assert_eq!(
            summary.require_complete(),
            Err(crate::AudioStorageIncomplete::RetirementReceiptUnknown)
        );
        assert_eq!(summary.unresolved_retirements, 1);
        assert_eq!(
            summary.unresolved_handles,
            bank_handles
                + 1 // actual empty Main prepared-slice allocation
                + 2
                + 1
                + 1
                + capacity.shared.installed_handle_obligations
                + capacity
                    .arrangement_clips
                    .tracks
                    .independent_handle_obligations
                + capacity
                    .arrangement_clips
                    .returns
                    .independent_handle_obligations
                + capacity
                    .arrangement_clips
                    .preview
                    .independent_handle_obligations
                + 4
        );
        assert_eq!(ledger.streamed_sources().count(), usize::from(streamed));
        assert!(summary.data_allocations > 20);
        assert!(summary.construction_peak_bytes >= summary.retained_bytes);
        assert_eq!(crate::owned_shared::clone_count(), clones);
        drop(ledger);
        c.cancel(&token);
        c.cleanup_on_control();
        assert!(c.recover_on_control(&token).is_some());
    }
}
#[test]
fn borrowed_storage_pressure_refuses_without_consuming_original_owners() {
    if !supported("borrowed_storage_pressure") {
        return;
    }
    let (project, bank) = fixture(false);
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = begin(&mut c, &project, bank).ok().unwrap();
    let mut reservation = storage_reservation();
    reservation.data_rows = 1;
    let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    assert_eq!(
        ledger.borrow_custody_on_control(&c, crate::AudioStorageRole::Current),
        Err(AudioStorageError::Capacity)
    );
    assert_eq!(ledger.summary(), Err(AudioStorageError::UnknownStorage));
    drop(ledger);
    c.cancel(&token);
    c.cleanup_on_control();
    assert!(c.recover_on_control(&token).is_some());
}

#[test]
fn borrowed_storage_retained_roles_and_failure_custody_never_drop_or_close_owners() {
    if !supported("borrowed_storage_retained_failure") {
        return;
    }
    let (project, a) = fixture(false);
    let (_, b) = fixture(false);
    let mut current = AudioPreparationCustody::try_new_on_control().unwrap();
    let mut previous = AudioPreparationCustody::try_new_on_control().unwrap();
    let a = begin(&mut current, &project, a).ok().unwrap();
    let b = begin(&mut previous, &project, b).ok().unwrap();
    let mut one = crate::AudioRootStorageLedger::try_new_on_control(storage_reservation()).unwrap();
    one.borrow_custody_on_control(&current, crate::AudioStorageRole::Current)
        .unwrap();
    let mut two = crate::AudioRootStorageLedger::try_new_on_control(storage_reservation()).unwrap();
    two.borrow_custody_on_control(&previous, crate::AudioStorageRole::Predecessor)
        .unwrap();
    let expected = one.summary().unwrap().data_bytes + two.summary().unwrap().data_bytes;
    for role in [
        crate::AudioStorageRole::Predecessor,
        crate::AudioStorageRole::Archive,
        crate::AudioStorageRole::PendingReclaim,
    ] {
        let mut ledger =
            crate::AudioRootStorageLedger::try_new_on_control(storage_reservation()).unwrap();
        ledger
            .borrow_custody_on_control(&current, crate::AudioStorageRole::Current)
            .unwrap();
        ledger.borrow_custody_on_control(&previous, role).unwrap();
        let sum = ledger.summary().unwrap();
        assert_eq!(sum.data_bytes, expected);
        assert_eq!(sum.unresolved_retirements, 2);
        assert!(sum.require_complete().is_err());
    }
    drop(one);
    drop(two);
    current.cancel(&a);
    previous.cancel(&b);
    current.cleanup_on_control();
    previous.cleanup_on_control();
    assert!(current.recover_on_control(&a).is_some());
    assert!(previous.recover_on_control(&b).is_some());
    for panic in [false, true] {
        let (project, bank) = fixture(false);
        let pointer = bank.sources.as_ptr();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        if panic {
            c.revision.panic_after = Some(9);
        } else {
            c.revision.fail_after = Some(9);
        }
        let token = begin(&mut c, &project, bank).ok().unwrap();
        let mut ledger =
            crate::AudioRootStorageLedger::try_new_on_control(storage_reservation()).unwrap();
        ledger
            .borrow_custody_on_control(&c, crate::AudioStorageRole::FailedConstruction)
            .unwrap();
        let report = ledger.summary().unwrap();
        assert!(report.data_bytes > 0);
        assert!(report.require_complete().is_err());
        assert_eq!(report.unknown_storage != 0, panic);
        drop(ledger);
        assert_eq!(c.bank.as_ref().unwrap().sources.as_ptr(), pointer);
        c.cleanup_on_control();
        if panic {
            assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
            assert!(c.recover_on_control(&token).is_none());
        } else {
            assert_eq!(
                c.recover_on_control(&token).unwrap().sources.as_ptr(),
                pointer
            );
        }
    }
}
#[test]
fn borrowed_storage_checked_reservation_overflow_precedes_any_owner_borrow() {
    let mut r = storage_reservation();
    r.data_rows = usize::MAX;
    assert!(matches!(
        crate::AudioRootStorageLedger::try_new_on_control(r),
        Err(AudioStorageError::Overflow)
    ));
}
