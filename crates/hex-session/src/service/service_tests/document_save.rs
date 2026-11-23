use super::*;

#[test]
fn time_clipboard_is_worker_owned_cut_is_one_revision_and_open_clears_it() {
    let directory = TestDir::new();
    let path = directory.0.join("time.hex");
    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "hybrid".into(),
        }))
        .expect("track");
    let track = next_event(&service).state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(4).expect("length"),
        }))
        .expect("clip");
    let placed = next_event(&service);
    let range = hex_project::TimeRange::new(vec![track], 24_000, 72_000).expect("range");
    service
        .submit(Request::CopyTimeRange(range.clone()))
        .expect("copy");
    let copied = next_event(&service);
    assert!(copied.error.is_none());
    assert_eq!(copied.state.revision, placed.state.revision);
    assert!(copied.playback.is_none());
    service
        .submit(Request::InsertTimeRange {
            track,
            frame: 120_000,
        })
        .expect("insert from worker clipboard");
    let inserted = next_event(&service);
    assert!(inserted.error.is_none());
    assert_eq!(inserted.state.revision, copied.state.revision + 1);
    assert_eq!(inserted.state.project.tracks()[0].midi_clips().len(), 2);
    service.submit(Request::Undo).expect("undo insert");
    let insert_undone = next_event(&service);
    assert_eq!(insert_undone.state.revision, inserted.state.revision + 1);
    assert_eq!(
        insert_undone.state.project.tracks()[0].midi_clips().len(),
        1
    );
    let mut other =
        hex_project::Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
    other
        .execute(Command::AddTrack {
            name: "first".into(),
        })
        .expect("first");
    let missing = match other
        .execute(Command::AddTrack {
            name: "second".into(),
        })
        .expect("second")
    {
        Created::Track(id) => id,
        result => panic!("unexpected result {result:?}"),
    };
    service
        .submit(Request::CutTimeRange(
            hex_project::TimeRange::new(vec![missing], 0, 1).expect("shape"),
        ))
        .expect("refused cut");
    let refused_cut = next_event(&service);
    assert!(refused_cut.error.is_some());
    let mut refused_snapshot = refused_cut.state.project.snapshot();
    let copied_snapshot = copied.state.project.snapshot();
    refused_snapshot.next_id = copied_snapshot.next_id;
    assert_eq!(refused_snapshot, copied_snapshot);
    service
        .submit(Request::Open(directory.0.join("missing.hex")))
        .expect("refused open");
    let refused_open = next_event(&service);
    assert!(refused_open.error.is_some());
    assert!(!refused_open.project_replaced);
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 120_000,
        })
        .expect("retained clipboard");
    let retained = next_event(&service);
    assert!(retained.error.is_none());
    service.submit(Request::Undo).expect("undo retained paste");
    let restored = next_event(&service);
    service.submit(Request::CutTimeRange(range)).expect("cut");
    let cut = next_event(&service);
    assert!(cut.error.is_none());
    assert_eq!(cut.state.revision, restored.state.revision + 1);
    assert_eq!(cut.state.project.tracks()[0].midi_clips().len(), 2);
    service.submit(Request::Undo).expect("undo");
    assert_eq!(
        next_event(&service).state.project.tracks()[0]
            .midi_clips()
            .len(),
        1
    );
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 96_000,
        })
        .expect("paste");
    let pasted = next_event(&service);
    assert!(pasted.error.is_none());
    assert_eq!(pasted.state.project.tracks()[0].midi_clips().len(), 2);
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save");
    assert!(next_event(&service).error.is_none());
    service.submit(Request::Open(path)).expect("open");
    let opened = next_event(&service);
    assert!(opened.project_replaced);
    let source = opened
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    assert_eq!(source.project_generation, 1);
    assert_eq!(source.editor_revision, opened.state.revision);
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 192_000,
        })
        .expect("empty paste");
    let refused = next_event(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.revision, opened.state.revision);
    assert_eq!(refused.state.project, opened.state.project);
}

#[test]
fn builtin_fm4_state_edits_save_reopen_undo_redo_and_reject_cycles_atomically() {
    use hex_engine::NativeDeviceClass;
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "fm4 persistence".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .expect("add track");
    let track_event = next_event(&service);
    let track = track_event.state.project.tracks()[0].id();
    let instrument = track_event.state.project.tracks()[0]
        .instrument()
        .expect("default sine slot");
    let device = instrument.id();
    let fm4 = DeviceTypeId::from_static("hex.instrument.fm4");

    service
        .submit(Request::ReplaceNativeDevice {
            device,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(fm4),
        })
        .expect("replace default instrument with registered FM4");
    let admitted = next_event(&service);
    assert!(admitted.error.is_none(), "{:?}", admitted.error);
    let admitted_state = admitted.state.project.device(device).unwrap().state();
    assert_eq!(admitted_state.type_id(), fm4);
    let carrier = ParameterId::from_static(29);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            admitted_state,
            carrier,
        ),
        Ok(ParameterValue::Float(1.0))
    );

    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: carrier,
            value: ParameterValue::Float(0.625),
        })
        .expect("edit FM4 carrier");
    let changed = next_event(&service);
    assert!(changed.error.is_none(), "{:?}", changed.error);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            changed.state.project.device(device).unwrap().state(),
            carrier,
        ),
        Ok(ParameterValue::Float(0.625))
    );

    service
        .submit(Request::Undo)
        .expect("undo FM4 parameter edit");
    let undone = next_event(&service);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            undone.state.project.device(device).unwrap().state(),
            carrier,
        ),
        Ok(ParameterValue::Float(1.0))
    );
    service
        .submit(Request::Redo)
        .expect("redo FM4 parameter edit");
    let redone = next_event(&service);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            redone.state.project.device(device).unwrap().state(),
            carrier,
        ),
        Ok(ParameterValue::Float(0.625))
    );

    let directory = TestDir::new();
    let path = directory.0.join("fm4.hex");
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save FM4 project");
    let saved = next_event(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    let reopened_service = Service::new(48_000, 16, 0).expect("reopen service");
    next_event(&reopened_service);
    reopened_service
        .submit(Request::Open(path))
        .expect("reopen FM4 project");
    let reopened = next_event(&reopened_service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    let persisted = reopened
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap();
    assert_eq!(persisted.id(), device);
    assert_eq!(persisted.state().type_id(), fm4);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            persisted.state(),
            carrier,
        ),
        Ok(ParameterValue::Float(0.625))
    );

    reopened_service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: ParameterId::from_static(14),
            value: ParameterValue::Float(0.2),
        })
        .expect("author first acyclic edge");
    let edged = next_event(&reopened_service);
    assert!(edged.error.is_none(), "{:?}", edged.error);
    let before_cycle = edged.state.clone();
    reopened_service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: ParameterId::from_static(17),
            value: ParameterValue::Float(0.2),
        })
        .expect("submit cyclic edge");
    let rejected = next_event(&reopened_service);
    assert_eq!(
        rejected.error.as_deref(),
        Some("device parameter value is invalid")
    );
    assert_eq!(rejected.state.revision, before_cycle.revision);
    assert_eq!(rejected.state.project, before_cycle.project);
    assert_eq!(rejected.state.can_undo, before_cycle.can_undo);
    assert!(rejected.playback.is_none());
}

#[test]
fn utility_controls_survive_history_and_project_reopen() {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "utility persistence".into(),
        }))
        .expect("add track");
    let track = next_event(&service).state.project.tracks()[0].id();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(DeviceTypeId::from_static("hex.utility")),
            before: None,
        })
        .expect("add utility");
    let mut current = next_event(&service);
    assert!(current.error.is_none(), "{:?}", current.error);
    let device = current.state.project.track(track).unwrap().devices()[0].id();
    // Exercise both matrix and stateful fields through the service rather
    // than just round-tripping the codec's own output.
    for (id, value) in [
        (1, ParameterValue::Float(2.0)),
        (2, ParameterValue::Float(-0.25)),
        (3, ParameterValue::Float(2.5)),
        (4, ParameterValue::Choice(3)),
        (5, ParameterValue::Bool(true)),
        (6, ParameterValue::Bool(true)),
        (7, ParameterValue::Bool(true)),
        (8, ParameterValue::Bool(true)),
        (9, ParameterValue::Bool(true)),
        (10, ParameterValue::Float(0.5)),
        (11, ParameterValue::Bool(true)),
        (12, ParameterValue::Bool(true)),
        (13, ParameterValue::Float(500.0)),
        (14, ParameterValue::Bool(true)),
    ] {
        let parameter = ParameterId::from_static(id);
        let prior = current
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .clone();
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter,
                value,
            })
            .expect("edit utility parameter");
        let changed = next_event(&service);
        assert!(changed.error.is_none(), "{:?}", changed.error);
        let expected = changed
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .clone();
        assert_eq!(
            hex_engine::native_device_parameter(&expected, parameter),
            Ok(value)
        );
        service.submit(Request::Undo).expect("undo utility edit");
        let undone = next_event(&service);
        assert!(undone.error.is_none(), "{:?}", undone.error);
        assert_eq!(undone.state.project.device(device).unwrap().state(), &prior);
        service.submit(Request::Redo).expect("redo utility edit");
        current = next_event(&service);
        assert!(current.error.is_none(), "{:?}", current.error);
        assert_eq!(
            current.state.project.device(device).unwrap().state(),
            &expected
        );
    }
    let expected = current
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone();
    let directory = TestDir::new();
    let path = directory.0.join("utility.hex");
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save utility");
    let saved = next_event(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    let reopened_service = Service::new(48_000, 16, 0).expect("reopen service");
    next_event(&reopened_service);
    reopened_service
        .submit(Request::Open(path))
        .expect("open utility");
    let reopened = next_event(&reopened_service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    let persisted = &reopened.state.project.track(track).unwrap().devices()[0];
    assert_eq!(persisted.id(), device);
    assert_eq!(persisted.state(), &expected);
}

#[test]
fn save_and_export_barriers_finalize_latest_parameter_value() {
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    use hex_lattice_contract::{ParameterId, ParameterValue};

    let directory = TestDir::new();
    let path = directory.0.join("gesture.hex");
    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "gesture save".into(),
        }))
        .expect("add track");
    let track = next_event(&service).state.project.tracks()[0].id();
    let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .expect("add utility");
    let added = next_event(&service);
    let device = added.state.project.tracks()[0].devices()[0].id();
    let gain = ParameterId::new(1).unwrap();
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save baseline");
    let saved = next_event(&service);
    assert!(saved.error.is_none());
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::device_gesture(GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter {
                device,
                parameter: gain,
            },
        }))
        .expect("begin parameter edit");
    next_event(&service);
    service
        .submit(Request::device_gesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(2.75),
        }))
        .expect("preview parameter edit");
    next_event(&service);
    let prior_file = std::fs::read(&path).unwrap();
    let stale = SaveCorrelation {
        request_id: 1,
        project_generation: 0,
        revision: saved.state.revision + 1,
    };
    service
        .submit_correlated_save(Request::SaveCurrent, stale)
        .expect("admit stale correlated save");
    let refused = next_event(&service);
    assert!(refused.save_complete && refused.error.is_some());
    assert_eq!(refused.save_correlation, Some(stale));
    assert!(!refused.gesture_protocol_ack);
    assert!(refused.active_device_gesture.is_some());
    assert_eq!(refused.state.revision, saved.state.revision);
    assert_eq!(std::fs::read(&path).unwrap(), prior_file);
    service.submit(Request::SaveCurrent).expect("save barrier");
    let saved_gesture = next_event(&service);
    assert!(saved_gesture.error.is_none());
    assert!(saved_gesture.save_complete);
    assert!(saved_gesture.gesture_protocol_ack);
    assert_eq!(saved_gesture.state.revision, saved.state.revision + 1);
    assert!(!saved_gesture.state.unsaved);
    service
        .submit(Request::Open(path.clone()))
        .expect("reopen saved gesture");
    let reopened = next_event(&service);
    assert!(reopened.project_replaced);
    assert_eq!(
        hex_engine::native_device_parameter(
            reopened.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(2.75)
    );

    let next_key = GestureKey {
        generation: 1,
        token: 1,
    };
    service
        .submit(Request::device_gesture(GestureRequest::Begin {
            key: next_key,
            target: GestureTarget::DeviceParameter {
                device,
                parameter: gain,
            },
        }))
        .expect("begin export parameter edit");
    next_event(&service);
    service
        .submit(Request::device_gesture(GestureRequest::Change {
            key: next_key,
            sequence: 1,
            value: ParameterValue::Float(4.0),
        }))
        .expect("preview export parameter edit");
    next_event(&service);
    service
        .submit(Request::Export(directory.0.join("empty.wav")))
        .expect("export barrier");
    let export_barrier = next_event(&service);
    assert!(export_barrier.gesture_protocol_ack);
    assert_eq!(export_barrier.state.revision, reopened.state.revision + 1);
    assert_eq!(
        hex_engine::native_device_parameter(
            export_barrier.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(4.0)
    );
    assert_eq!(
        next_export_event(&service).result.unwrap_err().message(),
        "invalid wav export: invalid export range"
    );
    let replace_key = GestureKey {
        generation: 1,
        token: 2,
    };
    service
        .submit(Request::device_gesture(GestureRequest::Begin {
            key: replace_key,
            target: GestureTarget::DeviceParameter {
                device,
                parameter: gain,
            },
        }))
        .expect("begin before replacement");
    next_event(&service);
    service
        .submit(Request::device_gesture(GestureRequest::Change {
            key: replace_key,
            sequence: 1,
            value: ParameterValue::Float(5.0),
        }))
        .expect("preview before replacement");
    next_event(&service);
    service.submit(Request::Open(path)).expect("open barrier");
    let replaced = next_event(&service);
    assert!(replaced.project_replaced);
    assert!(replaced.active_device_gesture.is_none());
    assert_eq!(
        hex_engine::native_device_parameter(
            replaced.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(2.75)
    );
    service
        .submit(Request::device_gesture(GestureRequest::Cancel {
            key: replace_key,
        }))
        .expect("stale project gesture");
    let stale = next_event(&service);
    assert!(
        stale
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.reason.is_some())
    );
    assert!(stale.active_device_gesture.is_none());
}

#[test]
fn save_conflict_error_keeps_the_mixed_case_path_exact() {
    let directory = TestDir::new();
    let folder = directory.0.join("My Songs");
    fs::create_dir(&folder).expect("mixed-case folder");
    let path = folder.join("Live Set.hex");
    hex_storage::save_new(&path, &Project::new(48_000).expect("project")).expect("write project");
    let service = Service::new(48_000, 16, 32).expect("service");
    let _ = next_event(&service);
    service
        .submit(Request::Open(path.clone()))
        .expect("open project");
    assert!(next_event(&service).error.is_none());
    let mut changed = fs::read(&path).expect("read project");
    changed.push(b' ');
    fs::write(&path, changed).expect("external change");
    service
        .submit(Request::SaveCurrent)
        .expect("conflicting save");
    let error = next_event(&service).error.expect("conflict error");
    assert_eq!(
        text::plain(&error),
        format!(
            "project changed outside hex at {}; your version was not saved. use save as to \
             keep it under a new name, then reopen the original to load or compare the \
             outside changes",
            path.display()
        )
    );
}

#[test]
fn committed_save_warning_adopts_published_token_and_keeps_next_save_valid() {
    let directory = TestDir::new();
    let path = directory.0.join("CurrentCase.hex");
    let previous = Project::new(48_000).expect("initial project");
    let token = hex_storage::save_new_with_token(&path, &previous)
        .expect("create project and capture token");
    let mut editor = hex_project::Editor::new(previous, 8).expect("editor");
    editor
        .execute(Command::AddTrack {
            name: "published".into(),
        })
        .expect("edit project");

    let mut outcome =
        hex_storage::replace_existing(&path, &token, editor.project()).expect("commit replacement");
    let recovery_path = outcome.recovery_path.clone();
    outcome.warning = Some(hex_storage::ReplaceWarning {
        operation: "synchronize the project directory",
        source: io::Error::other("injected durability failure"),
    });
    let mut document = DocumentHandle {
        path: path.clone(),
        token,
    };
    let commit = apply_current_save_outcome(&mut document, outcome);

    assert!(commit.dirty);
    assert_eq!(
        commit.status,
        "project was published, but durability could not be confirmed"
    );
    let warning = text::plain(&commit.error.expect("visible durability warning"));
    assert!(warning.contains(&format!(
        "complete project was published at {}",
        path.display()
    )));
    assert!(warning.contains("injected durability failure"));
    assert!(warning.contains(&format!("recovery copy is at {}", recovery_path.display())));
    assert!(document.token.check_unchanged(&path).is_ok());

    let next = hex_storage::replace_existing(&document.path, &document.token, editor.project())
        .expect("next Save Current must match the committed token");
    assert!(next.warning.is_none());
    assert_eq!(next.path, path);
}

#[test]
fn lost_file_attributes_keep_a_clean_save_and_name_the_loss_in_its_status() {
    let directory = TestDir::new();
    let path = directory.0.join("Tagged.hex");
    let project = Project::new(48_000).expect("project");
    let token = hex_storage::save_new_with_token(&path, &project).expect("create project");
    let metadata_warning = |attribute: &str| hex_storage::MetadataWarning {
        attribute: Some(attribute.to_owned()),
        operation: "write a file attribute",
        source: io::Error::other("injected attribute failure"),
    };

    let mut outcome =
        hex_storage::replace_existing(&path, &token, &project).expect("replace project");
    outcome.metadata_warnings = vec![metadata_warning("com.apple.metadata:_kMDItemUserTags")];
    let mut document = DocumentHandle {
        path: path.clone(),
        token,
    };
    let commit = apply_current_save_outcome(&mut document, outcome);
    assert!(!commit.dirty);
    assert!(commit.error.is_none(), "{:?}", commit.error);
    assert_eq!(
        text::plain(&commit.status),
        "project saved · finder tags could not be kept · could not write a file attribute \
         com.apple.metadata:_kMDItemUserTags: injected attribute failure"
    );

    let mut outcome = hex_storage::replace_existing(&path, &document.token, &project)
        .expect("replace project again");
    outcome.metadata_warnings = vec![metadata_warning("user.other")];
    outcome.warning = Some(hex_storage::ReplaceWarning {
        operation: "synchronize the project directory",
        source: io::Error::other("injected durability failure"),
    });
    let commit = apply_current_save_outcome(&mut document, outcome);
    assert!(commit.dirty, "durability warnings keep their precedence");
    assert_eq!(
        commit.status,
        "project was published, but durability could not be confirmed"
    );
    assert!(commit.error.is_some());
}

#[test]
fn save_as_onto_an_existing_project_replaces_it_and_binds_the_document() {
    let directory = TestDir::new();
    let existing = directory.0.join("existing.hex");
    let previous = Project::new(44_100).expect("previous project");
    hex_storage::save_new(&existing, &previous).expect("existing project");
    let foreign = directory.0.join("notes.hex");
    fs::write(&foreign, b"not a hex project").expect("foreign file");

    let service = Service::new(48_000, 32, 8_000_000).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "replacing".into(),
        }))
        .unwrap();
    let edited = next_event(&service);

    service.submit(Request::SaveNew(foreign.clone())).unwrap();
    let refused = next_event(&service);
    let reason = refused.error.expect("non-project destination refused");
    assert!(reason.contains("choose another name"), "{reason}");
    assert_eq!(fs::read(&foreign).unwrap(), b"not a hex project");
    assert!(refused.state.document_path.is_none());

    service.submit(Request::SaveNew(existing.clone())).unwrap();
    let saved = next_event(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    assert_eq!(
        saved.state.document_path.as_deref(),
        Some(existing.as_path())
    );
    assert_eq!(hex_storage::load(&existing).unwrap(), *edited.state.project);
    assert_eq!(
        hex_storage::load_recovery(&existing).unwrap().project,
        previous
    );

    service.submit(Request::SaveCurrent).unwrap();
    let current = next_event(&service);
    assert!(current.error.is_none(), "{:?}", current.error);
}

#[test]
fn prepared_midi_preview_tracks_edits_history_open_and_metadata_only_events() {
    let directory = TestDir::new();
    let path = directory.0.join("preview.hex");
    let service = Service::new(48_000, 8, 0).expect("service");
    let initial = next_event(&service);
    assert!(initial.state.midi_preview.clips().is_empty());

    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "keys".into(),
        }))
        .expect("add instrument track");
    let track = next_event(&service).state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(4).expect("clip length"),
        }))
        .expect("place clip");
    let placed = next_event(&service);
    assert!(placed.playback.is_some());
    let clip = placed.state.project.tracks()[0].midi_clips()[0].id();
    assert_eq!(
        placed
            .state
            .midi_preview
            .get(clip)
            .unwrap()
            .occurrence_count,
        0
    );

    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_ratio(1, 4).expect("note gate"),
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .expect("add note");
    let noted = next_event(&service);
    assert!(noted.playback.is_some());
    assert_eq!(
        noted.state.midi_preview.get(clip).unwrap().occurrence_count,
        1
    );
    assert!(noted.state.revision > placed.state.revision);

    service.submit(Request::Undo).expect("undo note");
    let undone = next_event(&service);
    assert!(undone.playback.is_some());
    assert_eq!(
        undone
            .state
            .midi_preview
            .get(clip)
            .unwrap()
            .occurrence_count,
        0
    );
    service.submit(Request::Redo).expect("redo note");
    let redone = next_event(&service);
    assert!(redone.playback.is_some());
    let redone_artifact = Arc::clone(
        redone
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("redo playback retains prepared revision"),
    );
    assert_eq!(
        redone
            .state
            .midi_preview
            .get(clip)
            .unwrap()
            .occurrence_count,
        1
    );

    service
        .submit(Request::Edit(Command::SetMidiClipSource {
            clip,
            source_length: hex_project::BeatTime::from_raw(2),
            source_start: hex_project::BeatTime::ZERO,
            loop_start: hex_project::BeatTime::ZERO,
            loop_end: hex_project::BeatTime::from_raw(1),
            looping: true,
            length: hex_project::BeatTime::from_raw(65_538),
        }))
        .expect("submit refused loop edit");
    let refused = next_event(&service);
    assert!(refused.error.is_some());
    assert!(refused.playback.is_none());
    assert_eq!(refused.state.revision, redone.state.revision);
    assert!(Arc::ptr_eq(
        &refused.state.midi_preview,
        &redone.state.midi_preview
    ));

    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save preview project");
    let saved = next_event(&service);
    assert!(saved.error.is_none());
    assert!(saved.playback.is_none());
    assert_eq!(saved.state.revision, redone.state.revision);
    assert!(Arc::ptr_eq(
        &saved.state.midi_preview,
        &redone.state.midi_preview
    ));

    service
        .submit(Request::Open(path))
        .expect("reopen saved project");
    let reopened = next_event(&service);
    assert!(reopened.error.is_none());
    assert!(reopened.playback.is_some());
    assert!(reopened.project_replaced);
    assert_eq!(reopened.state.revision, saved.state.revision + 1);
    let reopened_artifact = reopened
        .playback
        .as_ref()
        .and_then(Playback::prepared_revision)
        .expect("opened playback retains prepared revision");
    assert!(Arc::ptr_eq(
        redone_artifact.session().sine_table(),
        reopened_artifact.session().sine_table()
    ));
    assert_eq!(
        reopened
            .state
            .midi_preview
            .get(clip)
            .unwrap()
            .occurrence_count,
        1
    );
}

#[test]
fn prepared_revision_tracks_session_inventory_history_open_restore_and_refusals() {
    let directory = TestDir::new();
    let saved_path = directory.0.join("session-grid.hex");
    let invalid_path = directory.0.join("overdense-session.hex");
    let service = Service::new(48_000, 64, 0).expect("service");
    let _ = next_event(&service);

    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "keys".into(),
        }))
        .expect("add instrument track");
    let track = next_event(&service).state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::AddScene {
            name: "first".into(),
        }))
        .expect("add scene");
    let scene = next_event(&service).state.project.scenes()[0].id();
    service
        .submit(Request::Edit(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "cell".into(),
            source_length: hex_project::BeatTime::from_quarters(4).expect("source"),
        }))
        .expect("create Session MIDI cell");
    let clip_event = next_event(&service);
    let clip = match clip_event.created.expect("created Session clip") {
        hex_project::Created::SessionClip(id) => id,
        result => panic!("unexpected creation result: {result:?}"),
    };
    let empty_artifact = clip_event
        .playback
        .as_ref()
        .and_then(Playback::prepared_revision)
        .expect("compiled Session inventory");
    assert_eq!(
        empty_artifact.session().cell(track, scene),
        Some(hex_engine::PreparedSessionCellContent::Midi(clip))
    );
    assert_eq!(
        empty_artifact
            .session()
            .midi_clip(clip)
            .unwrap()
            .work
            .active_voices,
        0
    );

    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_quarters(1).expect("gate"),
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .expect("add Session note");
    let noted = next_event(&service);
    let noted_artifact = Arc::clone(
        noted
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("note edit artifact"),
    );
    assert_eq!(
        noted
            .state
            .project
            .session_midi_clip(clip)
            .unwrap()
            .source()
            .notes()
            .len(),
        1
    );
    assert_eq!(
        noted_artifact
            .session()
            .midi_clip(clip)
            .unwrap()
            .work
            .active_voices,
        1
    );

    service.submit(Request::Undo).expect("undo Session note");
    let undone = next_event(&service);
    assert_eq!(
        undone
            .state
            .project
            .session_midi_clip(clip)
            .unwrap()
            .source()
            .notes()
            .len(),
        0
    );
    assert_eq!(
        undone
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .unwrap()
            .session()
            .midi_clip(clip)
            .unwrap()
            .work
            .active_voices,
        0
    );
    service.submit(Request::Redo).expect("redo Session note");
    let redone = next_event(&service);
    assert_eq!(
        redone
            .state
            .project
            .session_midi_clip(clip)
            .unwrap()
            .source()
            .notes()
            .len(),
        1
    );
    let redone_artifact = Arc::clone(
        redone
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("redo artifact"),
    );
    assert_eq!(
        redone_artifact
            .session()
            .midi_clip(clip)
            .unwrap()
            .work
            .active_voices,
        1
    );

    service
        .submit(Request::SaveNew(saved_path.clone()))
        .expect("save populated Session project");
    let saved = next_event(&service);
    assert!(saved.error.is_none());
    service
        .submit(Request::Open(saved_path))
        .expect("reopen populated Session project");
    let reopened = next_event(&service);
    assert!(reopened.error.is_none());
    assert_eq!(
        reopened
            .state
            .project
            .session_midi_clip(clip)
            .unwrap()
            .source()
            .notes()
            .len(),
        1
    );
    let reopened_artifact = Arc::clone(
        reopened
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("opened artifact"),
    );
    assert_eq!(
        reopened_artifact.session().cell(track, scene),
        Some(hex_engine::PreparedSessionCellContent::Midi(clip))
    );
    assert_eq!(
        reopened_artifact
            .session()
            .midi_clip(clip)
            .unwrap()
            .work
            .active_voices,
        1
    );

    service
        .submit(Request::Edit(Command::AddScene {
            name: "dense".into(),
        }))
        .expect("add dense scene");
    let dense_scene = match next_event(&service).created.unwrap() {
        hex_project::Created::Scene(id) => id,
        result => panic!("unexpected scene result: {result:?}"),
    };
    service
        .submit(Request::Edit(Command::CreateSessionMidiClip {
            track,
            scene: dense_scene,
            name: "unlaunched dense cell".into(),
            source_length: hex_project::BeatTime::from_quarters(4).expect("source"),
        }))
        .expect("create unlaunched cell");
    let dense_clip = match next_event(&service).created.unwrap() {
        hex_project::Created::SessionClip(id) => id,
        result => panic!("unexpected clip result: {result:?}"),
    };
    let notes = (0..64_u8)
        .map(|pitch| hex_project::MidiNoteInsert {
            active: true,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_quarters(1).expect("gate"),
            pitch,
            channel: 0,
            velocity: 100,
        })
        .collect();
    service
        .submit(Request::Edit(Command::AddMidiNotes {
            clip: dense_clip,
            notes,
        }))
        .expect("add admitted dense source");
    let dense = next_event(&service);
    assert!(dense.error.is_none());
    assert_eq!(
        dense
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .unwrap()
            .session()
            .midi_clip(dense_clip)
            .unwrap()
            .work
            .active_voices,
        64
    );
    service
        .submit(Request::Edit(Command::AddScene {
            name: "redo branch".into(),
        }))
        .expect("create redo branch");
    let _ = next_event(&service);
    service.submit(Request::Undo).expect("undo scene branch");
    let dense_before_refusal = next_event(&service);
    assert!(dense_before_refusal.state.can_redo);
    let accepted_artifact = Arc::clone(
        dense_before_refusal
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("accepted dense artifact"),
    );
    let prior_revision = dense_before_refusal.state.revision;
    let prior_project = dense_before_refusal.state.project.as_ref().clone();
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip: dense_clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_quarters(1).expect("gate"),
            pitch: 64,
            channel: 0,
            velocity: 100,
        }))
        .expect("submit over-dense edit");
    let refused = next_event(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.revision, prior_revision);
    assert_eq!(refused.state.project.as_ref(), &prior_project);
    assert!(refused.state.can_redo);
    assert!(refused.playback.is_none());
    service
        .submit(Request::RestorePlayback { generation: 9 })
        .expect("restore accepted Session artifact");
    let restored = next_event(&service);
    let restored_artifact = restored
        .playback
        .as_ref()
        .and_then(Playback::prepared_revision)
        .expect("restored Session artifact");
    assert!(Arc::ptr_eq(&accepted_artifact, restored_artifact));
    assert_eq!(
        restored_artifact
            .session()
            .midi_clip(dense_clip)
            .unwrap()
            .work
            .active_voices,
        64
    );

    let mut invalid_editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let invalid_track = match invalid_editor
        .execute(Command::AddMidiTrack {
            name: "invalid".into(),
        })
        .unwrap()
    {
        hex_project::Created::Track(id) => id,
        _ => panic!("track result"),
    };
    let invalid_scene = match invalid_editor
        .execute(Command::AddScene {
            name: "invalid".into(),
        })
        .unwrap()
    {
        hex_project::Created::Scene(id) => id,
        _ => panic!("scene result"),
    };
    let invalid_clip = match invalid_editor
        .execute(Command::CreateSessionMidiClip {
            track: invalid_track,
            scene: invalid_scene,
            name: "too many voices".into(),
            source_length: hex_project::BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    {
        hex_project::Created::SessionClip(id) => id,
        _ => panic!("clip result"),
    };
    for pitch in 0..65_u8 {
        invalid_editor
            .execute(Command::AddMidiNote {
                clip: invalid_clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(1).unwrap(),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    hex_storage::save_new(&invalid_path, invalid_editor.project()).expect("save invalid fixture");
    service
        .submit(Request::Open(invalid_path))
        .expect("submit over-dense Session open");
    let rejected_open = next_event(&service);
    assert!(rejected_open.open_complete);
    assert!(rejected_open.error.is_some());
    assert!(!rejected_open.project_replaced);
    assert_eq!(rejected_open.state.revision, prior_revision);
    assert_eq!(rejected_open.state.project.as_ref(), &prior_project);
    service
        .submit(Request::RestorePlayback { generation: 10 })
        .expect("restore after failed open");
    let restored_after_open = next_event(&service);
    let restored_open_artifact = restored_after_open
        .playback
        .as_ref()
        .and_then(Playback::prepared_revision)
        .expect("retained artifact after failed open");
    assert!(Arc::ptr_eq(&accepted_artifact, restored_open_artifact));
}
