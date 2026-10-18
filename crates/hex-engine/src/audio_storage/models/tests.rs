use super::*;
use hex_project::{BeatTime, ClipId, Command, Created, Editor, Project};
#[path = "../../../build_support/midi_gate.rs"]
mod midi_gate;
use crate::pcm_arc::build_gate_tests as pcm_gate;
fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 128,
        root_rows: 8,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}

#[test]
fn actual_asset_history_candidate_and_duplicate_visits_charge_capacity_and_paths_once() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    for name in ["first.wav", "second.wav"] {
        editor
            .execute(Command::AddAsset {
                path: name.into(),
                sample_rate: 48_000,
                frames: 4,
            })
            .unwrap();
    }
    assert!(editor.undo());
    assert!(editor.can_redo());
    let candidate = editor.project().clone();
    let expected_project = |project: &Project| {
        project.asset_entries_on_control().capacity()
            * std::mem::size_of::<(hex_project::AssetId, hex_project::Asset)>()
            + project
                .assets()
                .map(|asset| asset.path_buffer_on_control().capacity())
                .sum::<usize>()
    };
    let history: usize = editor
        .retained_projects_on_control()
        .map(expected_project)
        .sum();
    let expected = history + expected_project(&candidate);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .borrow_editor_asset_storage_on_control(&editor)
        .unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, history);
    assert_ne!(history, expected); // omitting the actual pending candidate undercounts
    ledger
        .borrow_project_asset_storage_on_control(&candidate)
        .unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, expected);
    ledger
        .borrow_project_asset_storage_on_control(&candidate)
        .unwrap();
    ledger
        .borrow_editor_asset_storage_on_control(&editor)
        .unwrap();
    let full = ledger.summary().unwrap();
    assert_eq!(full.data_bytes, expected);
    assert_eq!(full.unknown_storage, 0); // selected fields only, never complete Project
    let mut exact = reservation();
    exact.max_retained_bytes = full.retained_bytes;
    exact.max_construction_peak_bytes = full.construction_peak_bytes;
    let mut accepted = AudioRootStorageLedger::try_new_on_control(exact).unwrap();
    accepted
        .borrow_editor_asset_storage_on_control(&editor)
        .unwrap();
    accepted
        .borrow_project_asset_storage_on_control(&candidate)
        .unwrap();
    assert_eq!(
        accepted.summary().unwrap().retained_bytes,
        full.retained_bytes
    );
    exact.max_retained_bytes -= 1;
    let mut refused = AudioRootStorageLedger::try_new_on_control(exact).unwrap();
    refused
        .borrow_editor_asset_storage_on_control(&editor)
        .unwrap();
    assert_eq!(
        refused.borrow_project_asset_storage_on_control(&candidate),
        Err(AudioStorageError::ReservationExceeded)
    );
    assert!(refused.summary().is_err());
    assert_eq!(editor.project().assets().count(), 1);
    assert!(editor.can_redo());
}
fn project() -> (Project, ClipId) {
    let mut project = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let Created::Track(track) = project
        .execute(Command::AddMidiTrack {
            name: "model source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = project
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    (project.project().clone(), clip)
}
fn clip(project: &Project) -> &MidiClip {
    &project.tracks()[0].midi_clips()[0]
}
fn edit(project: &mut Project, command: Command) -> Created {
    let mut editor = Editor::new(project.clone(), 1).unwrap();
    let result = editor.execute(command).unwrap();
    *project = editor.project().clone();
    result
}
fn add(project: &mut Project, clip: ClipId, pitch: u8) -> hex_project::NoteId {
    let Created::Note(id) = edit(
        project,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch,
            channel: 0,
            velocity: 100,
        },
    ) else {
        panic!("note")
    };
    id
}

#[test]
fn canonical_fields_are_distinct_from_sources_and_duplicate_field_visits_refuse() {
    let (mut project, id) = project();
    add(&mut project, id, 60);
    let original = clip(&project);
    let identity = original.source_identity();
    let duplicate = original.clone();
    let a = original.source_storage_on_control();
    let b = identity.source_storage_on_control();
    assert!(a.shares_source_with(&b));
    assert!(!a.is_same_owning_field_as(&b));
    assert!(a.is_same_owning_field_as(&original.source_storage_on_control()));
    if !native_layout_known() {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        assert_eq!(
            ledger.borrow_midi_clip_source_on_control(original),
            Err(AudioStorageError::UnknownStorage)
        );
        return;
    }
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger.borrow_midi_clip_source_on_control(original).unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    ledger
        .borrow_midi_source_identity_on_control(&identity)
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    ledger
        .borrow_midi_clip_source_on_control(&duplicate)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(ledger.summary().unwrap().data_allocations, 2);
    assert_eq!(ledger.shared_resource_counts(), (1, 3));
    assert_eq!(
        ledger.borrow_midi_clip_source_on_control(original),
        Err(AudioStorageError::SourceMismatch)
    );
    assert!(ledger.summary().is_err());
}
#[test]
fn cow_and_unique_mutation_keep_old_identity_and_charge_actual_spare_empty_backing() {
    let (mut project, id) = project();
    let notes: Vec<_> = (0..5)
        .map(|pitch| add(&mut project, id, 60 + pitch))
        .collect();
    let old = clip(&project).source_identity();
    assert!(old.source_storage_on_control().notes_capacity() >= 5);
    let old_capacity = old.source_storage_on_control().notes_capacity();
    for note in notes {
        edit(&mut project, Command::RemoveMidiNote(note));
    }
    let current = clip(&project);
    assert!(current.notes().is_empty());
    assert!(current.source_storage_on_control().notes_capacity() > 0);
    assert!(
        !current
            .source_storage_on_control()
            .shares_source_with(&old.source_storage_on_control())
    );
    if !native_layout_known() {
        return;
    }
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger.borrow_midi_clip_source_on_control(current).unwrap();
    ledger.borrow_midi_source_identity_on_control(&old).unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let view = current.source_storage_on_control();
    let body = Layout::array::<AtomicUsize>(2)
        .unwrap()
        .extend(Layout::new::<hex_project::MidiSource>())
        .unwrap()
        .0
        .pad_to_align()
        .size();
    let expected = 2 * body
        + (old_capacity + view.notes_capacity()) * std::mem::size_of::<hex_project::MidiNote>();
    let total = ledger.summary().unwrap();
    assert_eq!(total.data_bytes, expected);
    assert_eq!(total.data_allocations, 4);
    assert_eq!(
        total.require_complete(),
        Err(AudioStorageIncomplete::RetirementReceiptUnknown)
    );
    drop(ledger);
    let exact = AudioStorageLedgerReservation {
        max_retained_bytes: total.retained_bytes,
        max_construction_peak_bytes: total.construction_peak_bytes,
        ..reservation()
    };
    for difference in [0, 1] {
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                max_retained_bytes: exact.max_retained_bytes - difference,
                ..exact
            })
            .unwrap();
        ledger.borrow_midi_clip_source_on_control(current).unwrap();
        let result = ledger.borrow_midi_source_identity_on_control(&old);
        assert_eq!(result.is_ok(), difference == 0);
    }
    let mut peak = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        max_construction_peak_bytes: total.construction_peak_bytes - 1,
        ..reservation()
    })
    .unwrap();
    peak.borrow_midi_clip_source_on_control(current).unwrap();
    assert_eq!(
        peak.borrow_midi_source_identity_on_control(&old),
        Err(AudioStorageError::ReservationExceeded)
    );
    let mut handles = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        handle_rows: 1,
        ..reservation()
    })
    .unwrap();
    handles.borrow_midi_clip_source_on_control(current).unwrap();
    assert_eq!(
        handles.borrow_midi_source_identity_on_control(&old),
        Err(AudioStorageError::Capacity)
    );
    drop(peak);
    drop(handles);
    // Identity alone still owns both old allocations after the actual clip dies.
    drop(project);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger.borrow_midi_source_identity_on_control(&old).unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(
        ledger.summary().unwrap().data_bytes,
        body + old_capacity * std::mem::size_of::<hex_project::MidiNote>()
    );
}
#[test]
fn raw_model_arc_and_untracked_wrapper_remain_unknown_in_the_model_domain() {
    let (project, _) = project();
    let raw = Arc::new((**clip(&project)).clone());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert_eq!(
        ledger.mutate(|ledger| ledger.model_source(&raw)),
        Err(AudioStorageError::UnknownStorage)
    );
    let untracked = crate::owned_shared::OwnedShared::untracked(raw);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert_eq!(
        ledger.mutate(|ledger| ledger.model_source(&untracked)),
        Err(AudioStorageError::UnknownStorage)
    );
}
#[test]
fn actual_history_current_undo_redo_fields_close_without_synthetic_owners() {
    if !native_layout_known() {
        return;
    }
    let (mut project, id) = project();
    add(&mut project, id, 60);
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::RenameTrack {
            track: editor.project().tracks()[0].id(),
            name: "history".into(),
        })
        .unwrap();
    let identity = clip(editor.project()).source_identity();
    for _ in 0..2 {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        ledger
            .borrow_editor_midi_sources_on_control(&editor)
            .unwrap();
        assert!(ledger.validate_closed_shared_handles().is_err());
        ledger
            .borrow_midi_source_identity_on_control(&identity)
            .unwrap();
        ledger.validate_closed_shared_handles().unwrap();
        assert_eq!(ledger.shared_resource_counts(), (1, 3));
        assert_eq!(ledger.summary().unwrap().data_allocations, 2);
        drop(ledger);
        if editor.can_undo() {
            assert!(editor.undo());
        } else {
            assert!(editor.redo());
        }
    }
}
#[test]
fn equal_content_snapshot_sources_are_separate_allocations_and_omission_is_not_closure() {
    assert_ne!(midi_gate::FAMILY, "arc-audiodata-retained-vec-v1");
    if !native_layout_known() {
        return;
    }
    let (mut first, id) = project();
    add(&mut first, id, 60);
    let second = Project::from_snapshot(first.snapshot()).unwrap();
    assert_eq!(clip(&first).notes(), clip(&second).notes());
    assert!(
        !clip(&first)
            .source_storage_on_control()
            .shares_source_with(&clip(&second).source_storage_on_control())
    );
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .borrow_project_midi_sources_on_control(&first)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(ledger.summary().unwrap().data_allocations, 2); // second entire source is unselected
    ledger
        .borrow_project_midi_sources_on_control(&second)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(ledger.summary().unwrap().data_allocations, 4);
    assert_eq!(ledger.shared_resource_counts(), (2, 2));
}
#[test]
fn actual_session_routed_and_cached_prepared_fields_are_all_required_then_retire_on_control() {
    if !native_layout_known() {
        return;
    }
    let (mut project, id) = project();
    add(&mut project, id, 60);
    let track = project.tracks()[0].id();
    let Created::Track(destination) = edit(
        &mut project,
        Command::AddMidiTrack {
            name: "routed destination".into(),
        },
    ) else {
        panic!("destination")
    };
    let Created::Scene(scene) = edit(
        &mut project,
        Command::AddScene {
            name: "scene".into(),
        },
    ) else {
        panic!("scene")
    };
    let Created::SessionClip(session) = edit(
        &mut project,
        Command::CreateSessionMidiClip {
            track,
            scene,
            name: "canonical session".into(),
            source_length: BeatTime::from_quarters(4).unwrap(),
        },
    ) else {
        panic!("session")
    };
    add(&mut project, session, 64);
    edit(
        &mut project,
        Command::SetMidiSource {
            destination,
            source: Some(track),
        },
    );
    let table = Arc::new(crate::SineTable::new());
    let media = std::collections::BTreeMap::new();
    let first =
        crate::PreparedRevision::prepare(&project, &media, Arc::clone(&table), None).unwrap();
    let second =
        crate::PreparedRevision::prepare(&project, &media, table, Some(first.session())).unwrap();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .borrow_project_midi_sources_on_control(&project)
        .unwrap();
    ledger
        .borrow_revision_midi_sources_on_control(&second)
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    ledger
        .borrow_revision_midi_sources_on_control(&first)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(ledger.shared_resource_counts(), (2, 6)); // arrangement1; Session model1 + two actual fields per revision
    assert_eq!(ledger.summary().unwrap().data_allocations, 4);
    drop(ledger);
    drop(first);
    drop(project);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .borrow_revision_midi_sources_on_control(&second)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(ledger.shared_resource_counts(), (1, 2));
    assert_eq!(ledger.summary().unwrap().data_allocations, 2);
}
#[test]
fn actual_mutable_arrangement_descriptor_requires_its_source_alias_and_retains_it_after_model_retirement()
 {
    if !native_layout_known() {
        return;
    }
    let (mut project, id) = project();
    add(&mut project, id, 60);
    let device = project.tracks()[0].instrument().unwrap().id();
    let registry = hex_devices::native_instrument_registry().unwrap();
    let state = registry
        .default_state(hex_devices::operator_state::OPERATOR_TYPE_ID)
        .unwrap();
    let state = registry
        .patch_parameter(
            &state,
            hex_devices::operator_state::harmonic_parameter(1, 2),
            hex_lattice_contract::ParameterValue::Float(0.25),
        )
        .unwrap();
    edit(&mut project, Command::ReplaceDeviceState { device, state });
    let revision = crate::PreparedRevision::prepare(
        &project,
        &std::collections::BTreeMap::new(),
        Arc::new(crate::SineTable::new()),
        None,
    )
    .unwrap();
    assert!(revision.arrangement().tracks()[0].has_mutable_instrument());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .borrow_project_midi_sources_on_control(&project)
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err()); // actual descriptor alias is omitted
    ledger
        .borrow_revision_midi_sources_on_control(&revision)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(ledger.shared_resource_counts(), (1, 2));
    assert_eq!(ledger.summary().unwrap().data_allocations, 2);
    drop(ledger);
    drop(project);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .borrow_revision_midi_sources_on_control(&revision)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(ledger.shared_resource_counts(), (1, 1));
    assert_eq!(ledger.summary().unwrap().data_allocations, 2);
}
