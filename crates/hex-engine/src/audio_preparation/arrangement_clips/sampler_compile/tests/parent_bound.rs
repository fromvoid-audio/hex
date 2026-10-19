use super::*;

fn original<'a>(
    bank: &'a DeviceResourceBank,
    signal: &'a crate::RetainedControlCancellationSignal,
    worker: &'a crate::RetainedControlCancellation,
) -> AudioRootStorageLedger<'a> {
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    ledger
        .borrow_device_resource_bank_on_control(bank, AudioStorageRole::Caller)
        .unwrap();
    signal.visit_storage_on_control(&mut ledger).unwrap();
    worker.visit_storage_on_control(&mut ledger).unwrap();
    ledger.growth(4096).unwrap();
    ledger
}

#[test]
fn parent_bound_nonempty_success_owns_context_and_children_from_birth_through_cleanup() {
    let Fixture {
        bank,
        mut editor,
        previous,
        mut preparation,
        signal,
        worker,
        job,
        transforms,
    } = Fixture::new();
    editor
        .execute(Command::AddReturnTrack {
            name: "parent bus".into(),
        })
        .unwrap();
    // Exercise nonempty Return DSP/control ownership and actual send metadata,
    // rather than only an empty Return chain's outer allocation.
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
    // Force NEW loaded context/factory construction; no previous owner reuse.
    drop(previous);
    let original = original(&bank, &signal, &worker);
    let before = original.summary().unwrap();
    let parent_address = &preparation as *const _;
    let destination = &preparation.arrangement_clips as *const _;
    preparation
        .compile_original_sampler_on_control(&editor, &bank, None, &original, &transforms, limits())
        .unwrap();
    assert_eq!(&preparation as *const _, parent_address);
    assert_eq!(&preparation.arrangement_clips as *const _, destination);
    let project = preparation.arrangement_clips.root.project().unwrap();
    assert_eq!(project.tracks.len(), 1);
    assert_eq!(project.return_buses.len(), 1);
    assert_eq!(project.return_buses[0].device_chain.len(), 1);
    assert_eq!(
        project.return_buses[0].device_chain[0].device_type,
        hex_devices::utility::UTILITY_TYPE_ID
    );
    assert_eq!(project.tracks[0].sends.len(), 1);
    assert_eq!(project.shared_contexts.owners().count(), 1);
    assert_eq!(
        project.tracks[0].instrument.as_ref().unwrap().spans.len(),
        1
    );
    let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
    crate::owned_shared::reset_clone_count();
    let check = allocation_counter::measure(|| preparation.inspect_sampler_destination().unwrap());
    assert_eq!(check.count_total, 0);
    assert_eq!(check.count_current, 0);
    assert_eq!(crate::owned_shared::clone_count(), 0);
    assert_eq!(
        crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
        ids
    );
    // The nonempty actual parent is also visited in place, not rebuilt into a
    // synthetic cleanup parent. Closure includes all borrowed input aliases.
    let inputs = Inputs {
        producer: None,
        rows: &preparation.arrangement_clips,
        preparation: &preparation,
        editor: &editor,
        target: None,
        resources: &bank,
        previous: None,
        original: &original,
    };
    transforms
        .with_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits())?;
            inputs.visit(inventory, &mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            assert!(ledger.summary()?.require_complete().is_err());
            Ok(())
        })
        .unwrap();
    let order = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let child = order.clone();
    preparation.arrangement_clips.tracks.probe = Some(crate::audio_preparation::DropProbe(Some(
        Box::new(move || {
            assert_eq!(child.fetch_add(1, std::sync::atomic::Ordering::SeqCst), 0);
        }),
    )));
    let context = order.clone();
    preparation.arrangement_clips.root.context_probe = Some(crate::audio_preparation::DropProbe(
        Some(Box::new(move || {
            assert_eq!(context.fetch_add(1, std::sync::atomic::Ordering::SeqCst), 1);
        })),
    ));
    preparation.cleanup_on_control();
    assert_eq!(preparation.phase(), AudioPreparationPhase::Recoverable);
    assert_eq!(order.load(std::sync::atomic::Ordering::SeqCst), 2);
    preparation.cleanup_on_control();
    assert_eq!(order.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(preparation.arrangement_clips.root_context().is_none());
    assert_eq!(original.summary().unwrap(), before);
    assert!(job.inspect_payload::<u8, _>(|value| *value == 0).unwrap());
}

#[test]
fn parent_bound_original_unrelated_parent_or_data_cannot_supply_cleanup_binding() {
    for unrelated in [false, true] {
        let Fixture {
            bank,
            editor,
            previous,
            mut preparation,
            signal,
            worker,
            job: _job,
            transforms,
        } = Fixture::new();
        let other = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        let data = vec![9_u8; 32];
        let mut original = original(&bank, &signal, &worker);
        if unrelated {
            original
                .borrow_custody_on_control(&other, AudioStorageRole::Current)
                .unwrap();
        } else {
            original
                .borrow_metadata_vec_backing_on_control(&data)
                .unwrap();
        }
        let before = original.summary().unwrap();
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        assert_eq!(
            preparation.compile_original_sampler_on_control(
                &editor,
                &bank,
                Some(&previous),
                &original,
                &transforms,
                limits()
            ),
            Err(AudioStorageError::SourceMismatch)
        );
        assert_eq!(crate::owned_shared::birth_count(), 0);
        assert_eq!(crate::owned_shared::clone_count(), 0);
        assert!(!preparation.arrangement_clips.has_next());
        assert_eq!(original.summary().unwrap(), before);
    }
}

#[test]
fn parent_bound_inspection_is_allocation_id_clone_and_cleanup_free() {
    let Fixture {
        bank,
        editor,
        previous,
        preparation,
        signal,
        worker,
        job: _job,
        transforms,
    } = Fixture::new();
    let original = original(&bank, &signal, &worker);
    let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
    crate::owned_shared::reset_birth_count();
    crate::owned_shared::reset_clone_count();
    let inputs = Inputs {
        producer: None,
        rows: &preparation.arrangement_clips,
        preparation: &preparation,
        editor: &editor,
        target: None,
        resources: &bank,
        previous: Some(&previous),
        original: &original,
    };
    let measured = allocation_counter::measure(|| {
        preparation.inspect_sampler_destination().unwrap();
        transforms
            .with_pcm_inventory_on_control(|inventory| {
                // Allocation-free arithmetic traversal, mandatory normal pass is
                // separately exercised by actual compiler construction tests.
                AudioRootStorageLedger::arithmetic_cost_for_limits(
                    limits(),
                    usize::MAX,
                    usize::MAX,
                    |ledger| inputs.visit(inventory, ledger),
                )
                .map(|_| ())
            })
            .unwrap();
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(measured.count_current, 0);
    assert_eq!(measured.bytes_total, 0);
    assert_eq!(crate::owned_shared::birth_count(), 0);
    assert_eq!(crate::owned_shared::clone_count(), 0);
    assert_eq!(
        crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
        ids
    );
    assert!(original.summary().unwrap().require_complete().is_err());
    assert!(
        crate::AudioRootStorageSummary::default()
            .require_complete()
            .is_err()
    );
}

fn bound_limits(
    preparation: &crate::AudioPreparationCustody,
    editor: &hex_project::Editor,
    bank: &DeviceResourceBank,
    previous: &OwnedShared<SharedAnalyticContextTable>,
    original: &AudioRootStorageLedger<'_>,
    transforms: &AudioTransformPreparation,
) -> (usize, usize) {
    let construction = preparation
        .arrangement_clips
        .sampler_construction(
            editor.project(),
            DeviceResourcePreparation {
                bank,
                max_scope_metadata_bytes: 65_536,
            },
        )
        .unwrap();
    let inputs = Inputs {
        producer: None,
        rows: &preparation.arrangement_clips,
        preparation,
        editor,
        target: None,
        resources: bank,
        previous: Some(previous),
        original,
    };
    transforms
        .with_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits())?;
            inputs.visit(inventory, &mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            let actual = ledger.summary()?;
            let metadata =
                ledger.metadata_capacity_bytes().unwrap() + size_of::<AudioRootStorageLedger<'_>>();
            let cache = inventory.original_only_storage_cost()?;
            let growth = original.original_live_growth_allowance()?;
            Ok((
                actual.data_bytes
                    + metadata
                    + construction.retained
                    + cache.transaction_new_backing_bytes,
                actual.data_bytes
                    + metadata
                    + construction.peak
                    + cache.transaction_new_backing_bytes
                    + growth
                    + actual.construction_peak_bytes
                    - actual.retained_bytes,
            ))
        })
        .unwrap()
}

#[test]
fn parent_bound_exact_minus_one_and_row_shortage_precede_new_owners() {
    for mode in ["exact", "retained-minus1", "peak-minus1", "rows"] {
        let Fixture {
            bank,
            editor,
            previous,
            mut preparation,
            signal,
            worker,
            job: _job,
            transforms,
        } = Fixture::new();
        let original = original(&bank, &signal, &worker);
        let before = original.summary().unwrap();
        let (retained, peak) = bound_limits(
            &preparation,
            &editor,
            &bank,
            &previous,
            &original,
            &transforms,
        );
        let mut reservation = AudioStorageLedgerReservation {
            max_retained_bytes: retained,
            max_construction_peak_bytes: peak,
            ..limits()
        };
        match mode {
            "retained-minus1" => reservation.max_retained_bytes -= 1,
            "peak-minus1" => reservation.max_construction_peak_bytes -= 1,
            "rows" => reservation.root_rows = 1,
            _ => {}
        }
        let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        let mut result = Ok(());
        let measured = allocation_counter::measure(|| {
            result = preparation.compile_original_sampler_on_control(
                &editor,
                &bank,
                Some(&previous),
                &original,
                &transforms,
                reservation,
            );
        });
        eprintln!(
            "same parent {mode}: retained={retained} peak={peak} result={result:?} alloc={measured:?}"
        );
        if mode == "exact" {
            result.unwrap();
            assert!(preparation.arrangement_clips.root.project().is_some());
            preparation.cleanup_on_control();
            assert_eq!(preparation.phase(), AudioPreparationPhase::Recoverable);
        } else {
            if mode == "rows" {
                assert_eq!(result, Err(AudioStorageError::Capacity));
            } else {
                assert_eq!(result, Err(AudioStorageError::ReservationExceeded));
                assert_eq!(measured.count_total, 0);
                assert_eq!(measured.bytes_total, 0);
            }
            assert_eq!(crate::owned_shared::birth_count(), 0);
            assert_eq!(crate::owned_shared::clone_count(), 0);
            assert_eq!(
                crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
                ids
            );
            assert!(!preparation.arrangement_clips.has_next());
        }
        assert_eq!(original.summary().unwrap(), before);
    }
}

#[test]
fn parent_bound_cancel_and_track_error_retain_real_partial_owners_for_cleanup() {
    for mode in ["cancel", "track-error"] {
        let Fixture {
            bank,
            editor,
            previous,
            mut preparation,
            signal,
            worker,
            job: _job,
            transforms,
        } = Fixture::new();
        if mode == "cancel" {
            transforms.cancel_after_polls_for_test(2);
        } else {
            preparation.arrangement_clips.tracks.fail_after = Some(0);
        }
        // Force NEW loaded context/factory construction; no previous owner reuse.
        drop(previous);
        let original = original(&bank, &signal, &worker);
        let before = original.summary().unwrap();
        assert!(
            preparation
                .compile_original_sampler_on_control(
                    &editor,
                    &bank,
                    None,
                    &original,
                    &transforms,
                    limits()
                )
                .is_err()
        );
        assert_eq!(preparation.phase(), AudioPreparationPhase::Vacant);
        assert!(preparation.arrangement_clips.has_next());
        assert!(preparation.arrangement_clips.root_context().is_some());
        preparation.inspect_sampler_destination().unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        preparation.arrangement_clips.root.context_probe = Some(
            crate::audio_preparation::DropProbe(Some(Box::new(move || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }))),
        );
        preparation.cleanup_on_control();
        assert_eq!(preparation.phase(), AudioPreparationPhase::Recoverable);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        preparation.cleanup_on_control();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(original.summary().unwrap(), before);
    }
}

#[test]
fn parent_bound_constructor_panic_preserves_first_payload_and_cleanup_never_retries_pending() {
    for retirement_panic in [false, true] {
        let Fixture {
            bank,
            editor,
            previous,
            mut preparation,
            signal,
            worker,
            job: _job,
            transforms,
        } = Fixture::new();
        preparation.arrangement_clips.tracks.panic_after = Some(0);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        preparation.arrangement_clips.root.context_probe = Some(
            crate::audio_preparation::DropProbe(Some(Box::new(move || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert!(!retirement_panic, "secondary context retirement panic");
            }))),
        );
        // Force NEW loaded context/factory construction; no previous owner reuse.
        drop(previous);
        let original = original(&bank, &signal, &worker);
        let before = original.summary().unwrap();
        assert_eq!(
            preparation.compile_original_sampler_on_control(
                &editor,
                &bank,
                None,
                &original,
                &transforms,
                limits()
            ),
            Err(AudioStorageError::UnknownStorage)
        );
        assert_eq!(preparation.phase(), AudioPreparationPhase::FailedIncomplete);
        assert!(transforms.failed_incomplete_on_control());
        let payload = preparation.first_panic.as_ref().unwrap();
        assert!(
            payload
                .downcast_ref::<String>()
                .unwrap()
                .contains("paired compiler Track move panic")
        );
        let identity = payload.as_ref() as *const dyn std::any::Any as *const ();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(preparation.inspect_sampler_destination().is_err());
        preparation.cleanup_on_control();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            preparation.first_panic.as_ref().unwrap().as_ref() as *const dyn std::any::Any
                as *const (),
            identity
        );
        assert!(preparation.arrangement_clips.root_context().is_none());
        if retirement_panic {
            assert!(preparation.arrangement_clips.has_next());
            assert!(
                preparation
                    .arrangement_clips
                    .capacity_at(false, None, None)
                    .is_err()
            );
            assert!(preparation.arrangement_clips.clear_disposed().is_err());
        }
        assert_eq!(original.summary().unwrap(), before);
    }
}

#[test]
fn parent_bound_failed_pending_invalid_and_omitted_alias_refuse_without_cleanup() {
    for mode in [
        "failed",
        "first-panic",
        "pending",
        "invalid-original",
        "omitted-alias",
        "cancelled",
    ] {
        let Fixture {
            bank,
            editor,
            previous,
            mut preparation,
            signal,
            worker,
            job,
            transforms,
        } = Fixture::new();
        let omitted = (mode == "omitted-alias").then(|| previous.clone_on_control());
        match mode {
            "failed" => preparation.phase = AudioPreparationPhase::FailedIncomplete,
            "first-panic" => preparation.first_panic = Some(Box::new("original first panic")),
            "pending" => preparation.arrangement_clips.metadata_disposal = Disposal::Pending,
            "cancelled" => signal.cancel(job.route()),
            _ => {}
        }
        let mut original = original(&bank, &signal, &worker);
        if mode == "invalid-original" {
            // Intentionally malformed ORIGINAL input: no closure or summary
            // is taken from this failed ledger; the operation must refuse it.
            assert!(
                original
                    .borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Caller)
                    .is_err()
            );
        }
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        let mut result = Ok(());
        let measured = allocation_counter::measure(|| {
            result = preparation.compile_original_sampler_on_control(
                &editor,
                &bank,
                Some(&previous),
                &original,
                &transforms,
                limits(),
            );
        });
        assert!(result.is_err(), "{mode}");
        assert_eq!(crate::owned_shared::birth_count(), 0, "{mode}");
        assert_eq!(crate::owned_shared::clone_count(), 0, "{mode}");
        assert!(preparation.arrangement_clips.root_context().is_none());
        if mode != "omitted-alias" {
            assert_eq!(measured.count_total, 0, "{mode}");
        }
        if mode == "first-panic" {
            assert_eq!(
                preparation
                    .first_panic
                    .as_ref()
                    .unwrap()
                    .downcast_ref::<&str>(),
                Some(&"original first panic")
            );
        }
        drop(omitted);
    }
}

#[test]
fn parent_bound_opaque_context_refuses_before_any_move_or_cleanup() {
    let Fixture {
        mut preparation, ..
    } = Fixture::new();
    let crate::shared_analytic::cleanup_tests::OpaqueFixture {
        table,
        owner,
        drops,
    } = crate::shared_analytic::cleanup_tests::opaque_fixture();
    preparation
        .arrangement_clips
        .root
        .begin_compiler_context()
        .unwrap();
    *preparation.arrangement_clips.root.compiler_destinations().1 = Some(table);
    let actual = preparation
        .arrangement_clips
        .root_context()
        .unwrap()
        .as_ref() as *const _;
    let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
    crate::owned_shared::reset_clone_count();
    let measured = allocation_counter::measure(|| {
        assert_eq!(
            preparation.inspect_sampler_destination(),
            Err(AudioStorageError::UnknownStorage)
        );
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(crate::owned_shared::clone_count(), 0);
    assert_eq!(
        crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
        ids
    );
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(
        preparation
            .arrangement_clips
            .root_context()
            .unwrap()
            .as_ref() as *const _,
        actual
    );
    preparation.cleanup_on_control();
    assert_eq!(preparation.phase(), AudioPreparationPhase::FailedIncomplete);
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(
        preparation
            .arrangement_clips
            .root_context()
            .unwrap()
            .as_ref() as *const _,
        actual
    );
    // Fixture-only terminal release outside every observed control scope.
    drop(preparation);
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 0);
    drop(owner);
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn parent_bound_arithmetic_original_has_no_normal_authority_or_side_effects() {
    let f = Fixture::new();
    crate::owned_shared::reset_birth_count();
    crate::owned_shared::reset_clone_count();
    let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
    let measured = allocation_counter::measure(|| {
        AudioRootStorageLedger::arithmetic_cost_for_limits(
            limits(),
            usize::MAX,
            usize::MAX,
            |original| {
                let inputs = Inputs {
                    producer: None,
                    rows: &f.preparation.arrangement_clips,
                    preparation: &f.preparation,
                    editor: &f.editor,
                    target: None,
                    resources: &f.bank,
                    previous: Some(&f.previous),
                    original,
                };
                assert!(matches!(
                    inputs.inspect(&f.transforms, limits()),
                    Err(AudioStorageError::UnknownStorage)
                ));
                Ok(())
            },
        )
        .unwrap();
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(measured.count_current, 0);
    assert_eq!(crate::owned_shared::birth_count(), 0);
    assert_eq!(crate::owned_shared::clone_count(), 0);
    assert_eq!(
        crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
        ids
    );
    assert!(!f.preparation.arrangement_clips.has_next());
}
