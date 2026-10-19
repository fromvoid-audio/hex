//! Exact captured target, complete constructor and same-parent refusal/cleanup.
use super::*;

#[test]
fn captured_sampler_target_survives_current_editor_history_pruning() {
    let Fixture {
        bank,
        mut editor,
        previous,
        mut preparation,
        signal,
        worker,
        job: _,
        transforms,
    } = Fixture::new();
    drop(previous);
    let target = crate::SharedMetadata::new_on_control(editor.project().clone());
    let track = editor.project().tracks()[0].id();
    editor.execute(Command::RemoveTrack(track)).unwrap();
    for n in 0..10 {
        let Created::Track(new_track) = editor
            .execute(Command::AddTrack {
                name: format!("new authoring {n}"),
            })
            .unwrap()
        else {
            panic!("new track");
        };
        editor.execute(Command::RemoveTrack(new_track)).unwrap();
    }
    assert!(editor.retained_projects_on_control().all(|project| {
        project
            .tracks()
            .iter()
            .all(|candidate| candidate.id() != track)
    }));
    assert!(target.tracks()[0].instrument().is_some());
    // The bank is installed in the actual existing parent BEFORE construction,
    // never handed back through an external borrowed-bank argument.
    preparation.original_resources = Some(bank);
    let mut original = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    signal.visit_storage_on_control(&mut original).unwrap();
    worker.visit_storage_on_control(&mut original).unwrap();
    original
        .borrow_shared_metadata_body_on_control(&target)
        .unwrap();
    original
        .borrow_project_exclusive_buffers_on_control(&target)
        .unwrap();
    original
        .borrow_project_midi_sources_on_control(&target)
        .unwrap();
    original
        .borrow_project_native_states_on_control(&target)
        .unwrap();
    preparation
        .compile_original_sampler_target_on_control(
            &target,
            &editor,
            None,
            &original,
            &transforms,
            limits(),
        )
        .expect("compile actual captured sampler target, not current authoring project");
    let actual = preparation.arrangement_clips.root.project().unwrap();
    assert_eq!(actual.tracks.len(), target.tracks().len());
    assert_eq!(actual.tracks[0].canonical_track_id(), Some(track));
    assert!(actual.tracks[0].mutable_instrument);
    assert_eq!(
        preparation
            .arrangement_clips
            .preview
            .metadata_at(None)
            .unwrap()
            .clip_count,
        target.tracks()[0].midi_clips().len(),
        "the actual captured target must produce its persistent compiler preview"
    );
    assert!(
        preparation
            .arrangement_clips
            .preview
            .can_transfer_revision()
    );
    let mut preview = None;
    preparation
        .arrangement_clips
        .transfer_preview_revision(&mut preview);
    let clips = preview.as_ref().unwrap().clips();
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].clip_id, target.tracks()[0].midi_clips()[0].id());
    assert_eq!(clips[0].authored_note_count, 1);
    assert_eq!(clips[0].occurrence_count, 1);
    assert_eq!(clips[0].samples.len(), 1);
    assert_eq!(clips[0].samples[0].pitch, 60);
    assert!(clips[0].samples[0].frame_gate_end > clips[0].samples[0].frame_start);
    // Restore the exact transferred owner to the original persistent custodian
    // before invoking its existing cleanup, rather than dropping a loose copy.
    preparation
        .arrangement_clips
        .preview
        .restore_revision(&mut preview);
    assert!(preview.is_none());
    drop(original);
    preparation.close_on_control();
    preparation.cleanup_on_control();
    assert_ne!(preparation.phase(), AudioPreparationPhase::FailedIncomplete);
}

#[test]
fn captured_sampler_complete_revision_is_registered_at_birth_and_retires_in_same_parent() {
    let Fixture {
        bank,
        mut editor,
        previous,
        mut preparation,
        signal,
        worker,
        transforms,
        ..
    } = Fixture::new();
    drop(previous);
    editor
        .execute(Command::AddReturnTrack {
            name: "compiled sampler return".into(),
        })
        .unwrap();
    use hex_lattice_contract::NativeDeviceMetadata;
    let return_track = editor.project().return_tracks()[0].id();
    editor
        .execute(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: hex_devices::utility::UTILITY_DEFINITION
                .default_state()
                .unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetReturnSend {
            track: editor.project().tracks()[0].id(),
            return_track,
            gain: 0.375,
        })
        .unwrap();
    let target = crate::SharedMetadata::new_on_control(editor.project().clone());
    preparation.original_resources = Some(bank);
    let mut original = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    signal.visit_storage_on_control(&mut original).unwrap();
    worker.visit_storage_on_control(&mut original).unwrap();
    original
        .borrow_shared_metadata_body_on_control(&target)
        .unwrap();
    original
        .borrow_project_exclusive_buffers_on_control(&target)
        .unwrap();
    original
        .borrow_project_midi_sources_on_control(&target)
        .unwrap();
    original
        .borrow_project_native_states_on_control(&target)
        .unwrap();
    let parent_address = &preparation as *const _;
    let token = preparation
        .compile_original_sampler_revision_on_control(
            &target,
            &editor,
            None,
            &original,
            &transforms,
            limits(),
        )
        .expect("complete actual captured target through qualified constructor, not adoption");
    assert_eq!(&preparation as *const _, parent_address);
    assert_eq!(
        preparation.phase(),
        AudioPreparationPhase::ArrangementAndSessionSources
    );
    let view = preparation.revision.storage_view().unwrap();
    let arrangement = view.arrangement.unwrap();
    let session = view.session.unwrap();
    assert_eq!(arrangement.tracks.len(), 1);
    assert!(arrangement.tracks[0].mutable_instrument);
    assert_eq!(arrangement.return_buses.len(), 1);
    assert_eq!(arrangement.return_buses[0].device_chain.len(), 1);
    assert_eq!(session.tracks().len(), target.tracks().len());
    assert!(
        session
            .mutable_instrument_factory(target.tracks()[0].id())
            .is_some(),
        "Arrangement runtime also obtains its actual Sampler factory from the Session inventory"
    );
    assert_eq!(view.preview.unwrap().clips()[0].samples[0].pitch, 60);
    assert!(std::sync::Arc::ptr_eq(
        arrangement.shared_contexts.arc(),
        session.shared_contexts().arc()
    ));
    let reader = preparation
        .register_compiled_reader_on_control(&token)
        .unwrap();
    assert_eq!(reader.metadata().track_count, 1);
    assert_eq!(reader.metadata().sample_rate, target.sample_rate());
    transforms
        .with_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits())?;
            let inputs = Inputs {
                rows: &preparation.arrangement_clips,
                preparation: &preparation,
                editor: &editor,
                target: Some(&target),
                resources: preparation.original_resources.as_ref().unwrap(),
                previous: None,
                original: &original,
            };
            inputs.visit(inventory, &mut ledger)?;
            reader.visit_shared_storage_on_control(&mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            assert!(
                ledger.summary()?.require_complete().is_err(),
                "public whole-host receipt remains unqualified"
            );
            Ok(())
        })
        .expect("inventory actual complete constructor, loaded context aliases and reader fields");
    drop(original);
    preparation.close_on_control();
    preparation.cleanup_on_control();
    assert_ne!(
        preparation.phase(),
        AudioPreparationPhase::Recoverable,
        "registered reader must retain actual constructor root"
    );
    drop(reader);
    preparation.cleanup_on_control();
    assert_eq!(preparation.phase(), AudioPreparationPhase::Recoverable);
}

#[test]
fn captured_complete_revision_exact_and_minus_one_refuse_before_constructor_birth() {
    for mode in ["exact", "retained-minus1", "peak-minus1", "rows"] {
        let Fixture {
            bank,
            editor,
            previous,
            mut preparation,
            signal,
            worker,
            transforms,
            ..
        } = Fixture::new();
        drop(previous);
        let target = crate::SharedMetadata::new_on_control(editor.project().clone());
        preparation.original_resources = Some(bank);
        let mut original = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        signal.visit_storage_on_control(&mut original).unwrap();
        worker.visit_storage_on_control(&mut original).unwrap();
        original
            .borrow_shared_metadata_body_on_control(&target)
            .unwrap();
        original
            .borrow_project_exclusive_buffers_on_control(&target)
            .unwrap();
        original
            .borrow_project_midi_sources_on_control(&target)
            .unwrap();
        original
            .borrow_project_native_states_on_control(&target)
            .unwrap();
        let (retained, peak) = transforms
            .with_pcm_inventory_on_control(|inventory| {
                let inputs = Inputs {
                    rows: &preparation.arrangement_clips,
                    preparation: &preparation,
                    editor: &editor,
                    target: Some(&target),
                    resources: preparation.original_resources.as_ref().unwrap(),
                    previous: None,
                    original: &original,
                };
                let mut ledger = AudioRootStorageLedger::try_new_on_control(limits())?;
                inputs.visit(inventory, &mut ledger)?;
                ledger.validate_selected_shared_handles_on_control()?;
                let actual = ledger.summary()?;
                let construction = preparation
                    .arrangement_clips
                    .sampler_construction_with_preview(
                        &target,
                        DeviceResourcePreparation {
                            bank: preparation.original_resources.as_ref().unwrap(),
                            max_scope_metadata_bytes: 65_536,
                        },
                        true,
                    )?;
                let complete = preparation.sampler_completion_bytes(&target)?;
                let backing = AudioRootStorageLedger::requested_metadata_bytes(limits())?;
                let metadata = backing + size_of::<AudioRootStorageLedger<'_>>();
                let cache = inventory.original_only_storage_cost()?;
                Ok((
                    actual.data_bytes
                        + metadata
                        + cache.transaction_new_backing_bytes
                        + construction.retained
                        + complete,
                    actual.data_bytes + actual.construction_peak_bytes - actual.retained_bytes
                        + metadata
                        + cache.transaction_new_backing_bytes
                        + construction.peak
                        + complete
                        + original.original_live_growth_allowance()?,
                ))
            })
            .unwrap();
        let mut reservation = limits();
        reservation.max_retained_bytes = retained;
        reservation.max_construction_peak_bytes = peak;
        match mode {
            "retained-minus1" => reservation.max_retained_bytes -= 1,
            "peak-minus1" => reservation.max_construction_peak_bytes -= 1,
            "rows" => reservation.handle_rows = 1,
            _ => {}
        }
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        let mut outcome = None;
        let measured = allocation_counter::measure(|| {
            outcome = Some(preparation.compile_original_sampler_revision_on_control(
                &target,
                &editor,
                None,
                &original,
                &transforms,
                reservation,
            ));
        });
        let outcome = outcome.unwrap();
        eprintln!(
            "complete target {mode}: retained={retained}, peak={peak}, allocations={measured:?}, births={}, clones={}",
            crate::owned_shared::birth_count(),
            crate::owned_shared::clone_count()
        );
        if mode == "exact" {
            let token = outcome.expect("exact full constructor allowance");
            let reader = preparation
                .register_compiled_reader_on_control(&token)
                .unwrap();
            assert_eq!(reader.metadata().track_count, 1);
            drop(reader);
        } else {
            assert!(
                matches!(
                    outcome,
                    Err(AudioStorageError::ReservationExceeded | AudioStorageError::Capacity)
                ),
                "{mode}"
            );
            assert_eq!(crate::owned_shared::birth_count(), 0, "{mode}");
            assert_eq!(crate::owned_shared::clone_count(), 0, "{mode}");
            assert!(!preparation.revision.capacity().unwrap().constructed);
            assert!(!preparation.arrangement_clips.has_next());
            if mode != "rows" {
                assert_eq!(measured.count_total, 0, "{mode}");
            }
        }
        drop(original);
        preparation.close_on_control();
        preparation.cleanup_on_control();
        assert_ne!(
            preparation.phase(),
            AudioPreparationPhase::FailedIncomplete,
            "{mode}"
        );
    }
}

#[test]
fn captured_complete_revision_partial_session_and_revision_failure_retain_same_parent_cleanup() {
    for mode in [
        "structure",
        "session-move",
        "revision-transfer",
        "session-panic",
        "revision-panic",
    ] {
        let Fixture {
            bank,
            editor,
            previous,
            mut preparation,
            signal,
            worker,
            transforms,
            ..
        } = Fixture::new();
        drop(previous);
        let target = crate::SharedMetadata::new_on_control(editor.project().clone());
        preparation.original_resources = Some(bank);
        let mut original = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        signal.visit_storage_on_control(&mut original).unwrap();
        worker.visit_storage_on_control(&mut original).unwrap();
        original
            .borrow_shared_metadata_body_on_control(&target)
            .unwrap();
        original
            .borrow_project_exclusive_buffers_on_control(&target)
            .unwrap();
        original
            .borrow_project_midi_sources_on_control(&target)
            .unwrap();
        original
            .borrow_project_native_states_on_control(&target)
            .unwrap();
        match mode {
            "structure" => preparation.structure.fail_reserve = Some(0),
            "session-move" => preparation.assembly.fail_after_move = Some(0),
            "revision-transfer" => preparation.revision.fail_after = Some(5),
            "session-panic" => preparation.assembly.panic_after_move = Some(0),
            "revision-panic" => preparation.revision.panic_after = Some(5),
            _ => unreachable!(),
        }
        let parent_address = &preparation as *const _;
        assert!(
            preparation
                .compile_original_sampler_revision_on_control(
                    &target,
                    &editor,
                    None,
                    &original,
                    &transforms,
                    limits()
                )
                .is_err(),
            "{mode}"
        );
        assert_eq!(&preparation as *const _, parent_address);
        drop(original);
        if mode.ends_with("panic") {
            assert_eq!(preparation.phase(), AudioPreparationPhase::FailedIncomplete);
            assert!(preparation.first_panic.is_some());
            let payload = preparation.first_panic.as_ref().unwrap().as_ref() as *const _;
            preparation.cleanup_on_control();
            assert!(std::ptr::addr_eq(
                payload,
                preparation.first_panic.as_ref().unwrap().as_ref() as *const _
            ));
        } else {
            preparation.cleanup_on_control();
            assert_eq!(
                preparation.phase(),
                AudioPreparationPhase::Recoverable,
                "{mode}"
            );
            preparation.cleanup_on_control();
            assert_eq!(
                preparation.phase(),
                AudioPreparationPhase::Recoverable,
                "{mode}"
            );
        }
    }
}

#[test]
fn captured_sampler_complete_revision_enters_actual_candidate_runtime() {
    for with_return in [false, true] {
        let Fixture {
            bank,
            mut editor,
            previous,
            mut preparation,
            signal,
            worker,
            transforms,
            ..
        } = Fixture::new();
        drop(previous);
        if with_return {
            use hex_lattice_contract::NativeDeviceMetadata;
            editor
                .execute(Command::AddReturnTrack {
                    name: "actual sampler return".into(),
                })
                .unwrap();
            let return_track = editor.project().return_tracks()[0].id();
            editor
                .execute(Command::InsertReturnDevice {
                    return_track,
                    before: None,
                    state: hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
                        .unwrap(),
                })
                .unwrap();
            editor
                .execute(Command::SetReturnSend {
                    track: editor.project().tracks()[0].id(),
                    return_track,
                    gain: 0.25,
                })
                .unwrap();
        }
        let target = crate::SharedMetadata::new_on_control(editor.project().clone());
        preparation.original_resources = Some(bank);
        let mut original = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        signal.visit_storage_on_control(&mut original).unwrap();
        worker.visit_storage_on_control(&mut original).unwrap();
        let token = preparation
            .compile_original_sampler_revision_on_control(
                &target,
                &editor,
                None,
                &original,
                &transforms,
                limits(),
            )
            .unwrap();
        let mut draft = crate::AudioCandidateDraft::new_on_control();
        draft.prepare_on_control(&preparation, &token).unwrap();
        let mut custody = Some(preparation);
        let mut token = Some(token);
        let mut owner = None;
        draft
            .finish_on_control(&mut custody, &mut token, &mut owner)
            .unwrap();
        assert!(custody.is_none() && token.is_none());
        let prepared_owner = owner.as_mut().unwrap();
        // The earlier compiler fixture's 16 MiB is not the native runtime budget.
        // Keep that real refusal, then use the EXISTING production composition
        // (compiled/input allowance plus unchanged 32 MiB mutable runtime ceiling).
        assert_eq!(
            prepared_owner.seal_constructor_on_control(limits()),
            Err(AudioStorageError::ReservationExceeded)
        );
        let mut native_limits = limits();
        native_limits.max_retained_bytes += crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
        native_limits.max_construction_peak_bytes += crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES;
        eprintln!("actual existing native composition: {native_limits:?}");
        assert_eq!(
            prepared_owner.seal_constructor_on_control(native_limits),
            Err(AudioStorageError::UnknownStorage),
            "owner-only walker must not hide real authoring aliases"
        );
        let unrelated_target = crate::SharedMetadata::new_on_control(
            hex_project::Project::new(target.sample_rate()).unwrap(),
        );
        assert_eq!(
            prepared_owner.seal_sampler_constructor_on_control(
                &unrelated_target,
                &editor,
                None,
                &original,
                &transforms,
                native_limits
            ),
            Err(AudioStorageError::UnknownStorage),
            "missing real captured fields cannot be replaced by an unrelated equal-rate target"
        );
        let unrelated_editor =
            hex_project::Editor::new(hex_project::Project::new(target.sample_rate()).unwrap(), 8)
                .unwrap();
        assert_eq!(
            prepared_owner.seal_sampler_constructor_on_control(
                &target,
                &unrelated_editor,
                None,
                &original,
                &transforms,
                native_limits
            ),
            Err(AudioStorageError::UnknownStorage),
            "actual current/history aliases remain required"
        );
        let omitted_state = target.tracks()[0].instrument().unwrap().state().clone();
        assert_eq!(
            prepared_owner.seal_sampler_constructor_on_control(
                &target,
                &editor,
                None,
                &original,
                &transforms,
                native_limits
            ),
            Err(AudioStorageError::UnknownStorage),
            "an additional omitted physical state field remains unknown"
        );
        drop(omitted_state);
        let result = prepared_owner.seal_sampler_constructor_on_control(
            &target,
            &editor,
            None,
            &original,
            &transforms,
            native_limits,
        );
        drop(original);
        assert!(
            result.is_ok(),
            "actual qualified Sampler candidate storage must close: {result:?}"
        );
        let actual_owner = owner.take().unwrap();
        let (mut runtime, anchors) = actual_owner.split_constructor_on_control().ok().unwrap();
        assert!(anchors.matches_current(&runtime));
        runtime
            .apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        let mut output = [[0.0; 2]; 8];
        let mut render = Ok(());
        let measured = allocation_counter::measure(|| {
            render = runtime.render(&mut output);
        });
        render.unwrap();
        assert_eq!(measured.count_total, 0);
        assert_eq!(measured.count_current, 0);
        let expected = crate::native_device_resources::tests::PCM
            .map(|frame| frame.map(|value| if with_return { value * 1.25 } else { value }));
        assert_eq!(
            &output[..4],
            &expected,
            "actual track/Return PCM path {with_return}"
        );
        assert_eq!(&output[4..], &[[0.0; 2]; 4]);
        let proof = runtime.dispose_on_control().unwrap();
        let mut anchors = [Some(anchors)];
        proof.record_anchor_cells_on_control(&mut anchors).unwrap();
        let anchors = anchors[0].as_mut().unwrap();
        assert!(anchors.original_runtime_disposed());
        assert!(anchors.all_origins_closed());
        anchors.finish_disposal_on_control().unwrap();
    }
}

#[test]
fn captured_target_alias_and_unrelated_bank_refuse_before_compiler_birth() {
    for (mode, complete) in [
        ("omitted-target-alias", false),
        ("foreign-bank", false),
        ("missing-bank", false),
        ("omitted-target-alias", true),
        ("foreign-bank", true),
        ("missing-bank", true),
    ] {
        let Fixture {
            bank,
            editor,
            previous,
            mut preparation,
            signal,
            worker,
            job: _,
            transforms,
        } = Fixture::new();
        drop(previous);
        let target = crate::SharedMetadata::new_on_control(editor.project().clone());
        let omitted = (mode == "omitted-target-alias").then(|| target.clone());
        let foreign = (mode == "foreign-bank").then(Fixture::new);
        if mode != "missing-bank" {
            preparation.original_resources = Some(bank);
        }
        let mut original = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        signal.visit_storage_on_control(&mut original).unwrap();
        worker.visit_storage_on_control(&mut original).unwrap();
        if let Some(foreign) = &foreign {
            original
                .borrow_device_resource_bank_on_control(&foreign.bank, AudioStorageRole::Caller)
                .unwrap();
        }
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        // Count actual physical model fields independently from ledger rows.
        // These are normal observation IDs, not constructor/owner identities.
        fn model_fields(project: &hex_project::Project) -> usize {
            let devices = project
                .tracks()
                .iter()
                .flat_map(|track| track.devices().iter().chain(track.instrument()))
                .chain(
                    project
                        .return_tracks()
                        .iter()
                        .flat_map(|track| track.devices()),
                );
            let states: usize = devices
                .map(|device| {
                    1 + usize::from(device.state().asset_reference_field_on_control().is_some())
                })
                .sum();
            let midi = project
                .tracks()
                .iter()
                .map(|track| track.midi_clips().len())
                .sum::<usize>()
                + project
                    .session_cells()
                    .iter()
                    .filter(|cell| {
                        matches!(cell.content(), hex_project::SessionCellContent::Midi(_))
                    })
                    .count();
            states + midi
        }
        let expected_normal_observations = editor.retained_projects_on_control().map(model_fields).sum::<usize>()
            + model_fields(&target) + 1 /* target paired Project field */ + 1 /* actual transform cancellation */;
        assert_eq!(expected_normal_observations, 14);
        let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
        let mut outcome = Ok(());
        let measured = allocation_counter::measure(|| {
            outcome = if complete {
                preparation
                    .compile_original_sampler_revision_on_control(
                        &target,
                        &editor,
                        None,
                        &original,
                        &transforms,
                        limits(),
                    )
                    .map(|_| ())
            } else {
                preparation.compile_original_sampler_target_on_control(
                    &target,
                    &editor,
                    None,
                    &original,
                    &transforms,
                    limits(),
                )
            };
        });
        assert!(outcome.is_err(), "{mode}");
        assert_eq!(crate::owned_shared::birth_count(), 0, "{mode}");
        assert_eq!(crate::owned_shared::clone_count(), 0, "{mode}");
        assert_eq!(
            crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()) - ids,
            if mode == "missing-bank" {
                0
            } else {
                expected_normal_observations
            },
            "{mode}: normal physical observations are not compiler owner births"
        );
        assert_eq!(
            outcome,
            Err(if mode == "omitted-target-alias" {
                AudioStorageError::UnknownStorage
            } else {
                AudioStorageError::SourceMismatch
            }),
            "{mode}"
        );
        assert!(
            preparation.arrangement_clips.root.project().is_none(),
            "{mode}"
        );
        if mode == "missing-bank" {
            assert_eq!(measured.count_total, 0);
        }
        drop(original);
        drop((omitted, foreign));
        preparation.close_on_control();
        preparation.cleanup_on_control();
        assert_ne!(preparation.phase(), AudioPreparationPhase::FailedIncomplete);
    }
}
