use super::*;
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};

fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 8,
        handle_rows: 8,
        root_rows: 2,
        max_retained_bytes: usize::MAX,
        max_construction_peak_bytes: usize::MAX,
    }
}
fn state() -> NativeDeviceState {
    NativeDeviceState::new(
        DeviceTypeId::from_static("test.bootstrap"),
        1,
        vec![7; 4096],
    )
    .unwrap()
}

#[test]
fn arithmetic_replay_many_actual_state_aliases_fits_configured_rows_distinct_equal_is_separate() {
    assert!(canonical_native_state_storage_layout_supported());
    let original = state();
    let aliases: Vec<_> = (0..100).map(|_| original.clone()).collect();
    let independent_equal = state();
    let mut actual = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    actual
        .borrow_canonical_native_state_on_control(&original)
        .unwrap();
    let one = actual.summary().unwrap().data_bytes;
    fn visit_aliases<'a>(
        aliases: &'a [NativeDeviceState],
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        for alias in aliases {
            ledger.borrow_canonical_native_state_on_control(alias)?;
        }
        Ok(())
    }
    // One configured normal DATA row is sufficient for all 100 aliases; the
    // handle reservation covers actual physical fields, not replay visits.
    // Replaying must not enlarge it just to fit traversal repetitions.
    let mut configured = reservation();
    configured.data_rows = 1;
    configured.handle_rows = 128;
    let mut normal = AudioRootStorageLedger::try_new_on_control(configured).unwrap();
    visit_aliases(&aliases, &mut normal).unwrap();
    assert_eq!(normal.summary().unwrap().data_bytes, one);
    assert_eq!(normal.summary().unwrap().unresolved_handles, 100);
    let start = std::time::Instant::now();
    let cost = AudioRootStorageLedger::arithmetic_cost(configured, one, |ledger| {
        visit_aliases(&aliases, ledger)
    })
    .unwrap();
    assert_eq!(cost.data_bytes, one);
    assert!(cost.replayed);
    assert_eq!(cost.observation_visits, 20100);
    eprintln!(
        "100 aliases: {} observation visits in {:?}",
        cost.observation_visits,
        start.elapsed()
    );
    assert_eq!(
        AudioRootStorageLedger::arithmetic_cost(configured, one - 1, |ledger| visit_aliases(
            &aliases, ledger
        )),
        Err(AudioStorageError::ReservationExceeded)
    );
    assert_eq!(
        AudioRootStorageLedger::arithmetic_cost(configured, one, |ledger| {
            visit_aliases(&aliases, ledger)?;
            ledger.borrow_canonical_native_state_on_control(&independent_equal)
        }),
        Err(AudioStorageError::ReservationExceeded)
    );
    let both = AudioRootStorageLedger::arithmetic_cost(configured, 2 * one, |ledger| {
        visit_aliases(&aliases, ledger)?;
        ledger.borrow_canonical_native_state_on_control(&independent_equal)
    })
    .unwrap();
    assert_eq!(both.data_bytes, 2 * one);
    // A new physical owner with identical bytes is never alias-deduplicated.
    assert!(!original.shares_payload_storage_with(&independent_equal));
}

#[test]
fn arithmetic_existing_shared_requires_earlier_declaration_and_conflicts_refuse() {
    let owner = Arc::new(7usize);
    let field = 1usize;
    fn declare_owner<'a>(
        owner: &'a Arc<usize>,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.bytes(Owner::shared(owner), Location("test-owner", 0), 24)
    }
    fn existing_owner<'a>(
        owner: &'a Arc<usize>,
        field: &'a usize,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.existing_shared(owner, Owner::exclusive(field), Location("test-field", 0))
    }
    let known = AudioRootStorageLedger::arithmetic_cost(reservation(), 24, |ledger| {
        declare_owner(&owner, ledger)?;
        existing_owner(&owner, &field, ledger)
    })
    .unwrap();
    assert_eq!(known.data_bytes, 24);
    assert!(known.replayed);
    assert_eq!(
        AudioRootStorageLedger::arithmetic_cost(reservation(), 24, |ledger| existing_owner(
            &owner, &field, ledger
        )),
        Err(AudioStorageError::SourceMismatch)
    );
    assert_eq!(
        AudioRootStorageLedger::arithmetic_cost(reservation(), 24, |ledger| {
            existing_owner(&owner, &field, ledger)?;
            declare_owner(&owner, ledger)
        }),
        Err(AudioStorageError::SourceMismatch)
    );
    assert_eq!(
        AudioRootStorageLedger::arithmetic_cost(reservation(), 24, |ledger| {
            declare_owner(&owner, ledger)?;
            ledger.bytes(Owner::shared(&owner), Location("test-owner", 0), 25)
        }),
        Err(AudioStorageError::UnknownStorage)
    );
}

#[test]
fn arithmetic_never_exports_summary_or_closure_and_unknown_is_not_completion() {
    let data = vec![1u8; 8];
    let cost = AudioRootStorageLedger::arithmetic_cost(reservation(), 8, |ledger| {
        ledger.mutate(|ledger| ledger.vec(&data))?;
        assert_eq!(ledger.summary(), Err(AudioStorageError::UnknownStorage));
        assert_eq!(
            ledger.validate_selected_shared_handles_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
        assert!(ledger.data.is_empty() && ledger.handles.is_empty() && ledger.roots.is_empty());
        Ok(())
    })
    .unwrap();
    assert_eq!(cost.data_bytes, 8);
    assert!(!cost.replayed);
    assert_eq!(
        AudioRootStorageLedger::arithmetic_cost(reservation(), 8, |ledger| ledger
            .mutate(|ledger| ledger.unknown())),
        Err(AudioStorageError::UnknownStorage)
    );
}

#[test]
fn arithmetic_actual_pcm_union_replay_issues_no_handles_and_allocates_nothing() {
    assert!(crate::audio_storage_constructor_layout_supported());
    let make = || {
        crate::OwnedAudioSource::try_resident_mono(48_000, vec![0.25; 1024 * 1024])
            .ok()
            .unwrap()
    };
    let original = make();
    let aliases: Vec<_> = (0..100)
        .map(|_| {
            original
                .alias_for_resource_plan_on_control(usize::MAX)
                .unwrap()
        })
        .collect();
    let equal = make();
    let mut limits = reservation();
    limits.data_rows = 2;
    limits.handle_rows = 128;
    let mut normal = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
    normal
        .borrow_embedded_native_source_on_control(&original)
        .unwrap();
    for alias in &aliases {
        normal
            .borrow_embedded_native_source_on_control(alias)
            .unwrap();
    }
    let expected = normal.summary().unwrap().data_bytes;
    assert!(expected > 4 * 1024 * 1024);
    normal.validate_closed_shared_handles().unwrap();
    let before = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
    let global_before = crate::audio_only_inputs::next_handle_observation();
    let mut result = None;
    let start = std::time::Instant::now();
    let allocations = allocation_counter::measure(|| {
        result = Some(AudioRootStorageLedger::arithmetic_cost(
            limits,
            expected,
            |ledger| {
                ledger.borrow_embedded_native_source_on_control(&original)?;
                for alias in &aliases {
                    ledger.borrow_embedded_native_source_on_control(alias)?;
                }
                Ok(())
            },
        ));
    });
    let elapsed = start.elapsed();
    let global_after = crate::audio_only_inputs::next_handle_observation();
    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.bytes_total, 0);
    assert_eq!(allocations.count_current, 0);
    assert_eq!(allocations.bytes_current, 0);
    assert_eq!(
        crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
        before
    );
    let cost = result.unwrap().unwrap();
    assert_eq!(cost.data_bytes, expected);
    assert!(cost.replayed);
    eprintln!(
        "PCM 101 actual aliases: {expected}B DATA, {} replay observation visits, {elapsed:?}, allocator={allocations:?}, actual global handle counter {global_before}->{global_after} (parallel issuers may advance globally)",
        cost.observation_visits
    );
    let distinct = AudioRootStorageLedger::arithmetic_cost(limits, expected, |ledger| {
        ledger.borrow_embedded_native_source_on_control(&original)?;
        ledger.borrow_embedded_native_source_on_control(&equal)
    });
    assert_eq!(distinct, Err(AudioStorageError::ReservationExceeded));
}

#[test]
fn arithmetic_canonical_observations_do_not_mint_global_handle_ids() {
    let state = state();
    let mut result = None;
    let before = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
    let global_before = crate::audio_only_inputs::next_handle_observation();
    let allocations = allocation_counter::measure(|| {
        result = Some(AudioRootStorageLedger::arithmetic_cost(
            reservation(),
            usize::MAX,
            |ledger| ledger.borrow_canonical_native_state_on_control(&state),
        ));
    });
    let global_after = crate::audio_only_inputs::next_handle_observation();
    assert_eq!(result.unwrap().unwrap().data_bytes, 4096 + 16);
    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.bytes_total, 0);
    assert_eq!(allocations.count_current, 0);
    assert_eq!(allocations.bytes_current, 0);
    assert_eq!(
        crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
        before
    );
    eprintln!(
        "canonical observation allocator={allocations:?}; actual global handle counter {global_before}->{global_after}"
    );
}

#[test]
fn arithmetic_replay_has_finite_work_bound_and_checked_overflow() {
    let value = vec![0u8; 1];
    assert_eq!(
        AudioRootStorageLedger::arithmetic_cost(reservation(), 0, |ledger| {
            for _ in 0..3000 {
                ledger.vec(&value)?;
            }
            Ok(())
        }),
        Err(AudioStorageError::Capacity)
    );
    assert_eq!(
        AudioRootStorageLedger::arithmetic_cost(reservation(), usize::MAX, |ledger| {
            ledger.bytes(
                Owner::exclusive(&value),
                Location("overflow", 0),
                usize::MAX,
            )?;
            ledger.bytes(Owner::exclusive(&value), Location("overflow", 1), 1)
        }),
        Err(AudioStorageError::Overflow)
    );
}

#[test]
fn arithmetic_actual_guarded_bank_history_context_inputs_release_before_mutation() {
    let (bank, owner, editor) = crate::shared_analytic::sampler_tests::retained_fixture().unwrap();
    let table = crate::shared_analytic::sampler_tests::retained_table_fixture(owner);
    let signal = crate::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
    let job = crate::RetainedControlJob::try_from_owned_on_control(
        &mut Some(0_u8),
        usize::MAX,
        |_: &mut u8| Ok::<_, ()>(crate::RetainedControlJobProgress::Pending),
    )
    .unwrap();
    let worker = signal.bind_worker_on_control(job.route());
    let transforms =
        crate::audio_transform::AudioTransformPreparation::with_retained_cancellation_on_control(
            worker.clone(),
        );
    let configured = AudioStorageLedgerReservation {
        data_rows: 1024,
        handle_rows: 1024,
        root_rows: 128,
        max_retained_bytes: 4 * 1024 * 1024,
        max_construction_peak_bytes: 4 * 1024 * 1024,
    };
    fn external<'a>(
        bank: &'a crate::native_device_resources::DeviceResourceBank,
        editor: &'a hex_project::Editor,
        table: &'a crate::owned_shared::OwnedShared<
            crate::shared_analytic::SharedAnalyticContextTable,
        >,
        signal: &'a crate::RetainedControlCancellationSignal,
        worker: &'a crate::RetainedControlCancellation,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.borrow_device_resource_bank_on_control(bank, AudioStorageRole::Current)?;
        ledger.borrow_editor_history_buffers_on_control(editor)?;
        ledger.borrow_editor_exclusive_buffers_on_control(editor)?;
        ledger.borrow_editor_native_states_on_control(editor)?;
        crate::shared_analytic::resource_storage::visit_table(
            table,
            ledger,
            AudioStorageRole::Predecessor,
        )?;
        signal.visit_storage_on_control(ledger)?;
        worker.visit_storage_on_control(ledger)
    }
    let mut original = AudioRootStorageLedger::try_new_on_control(configured).unwrap();
    external(&bank, &editor, &table, &signal, &worker, &mut original).unwrap();
    let original_summary = original.summary().unwrap();
    let expected = transforms
        .with_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(configured)?;
            external(&bank, &editor, &table, &signal, &worker, &mut ledger)?;
            inventory.visit_original_only_control_storage_on_control(&mut ledger)?;
            Ok(ledger.summary()?.data_bytes)
        })
        .unwrap();
    let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
    let mut outcome = None;
    let allocations = allocation_counter::measure(|| {
        outcome = Some(transforms.with_pcm_inventory_on_control(|inventory| {
            let cost = AudioRootStorageLedger::arithmetic_cost(configured, expected, |ledger| {
                external(&bank, &editor, &table, &signal, &worker, ledger)?;
                inventory.visit_original_only_control_storage_on_control(ledger)
            })?;
            assert_eq!(cost.data_bytes, expected);
            Ok(())
        }));
    });
    outcome.unwrap().unwrap();
    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.bytes_total, 0);
    assert_eq!(allocations.count_current, 0);
    assert_eq!(allocations.bytes_current, 0);
    assert_eq!(
        crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
        ids
    );
    // Failure releases actual three RefCell guards; no borrowed key/record can
    // escape this HRTB callback. Original inventory remains intact.
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), AudioStorageError> =
            transforms.with_pcm_inventory_on_control(|inventory| {
                AudioRootStorageLedger::arithmetic_cost(configured, expected, |ledger| {
                    inventory.visit_original_only_control_storage_on_control(ledger)?;
                    panic!("arithmetic guarded visitor sentinel");
                })?;
                Ok(())
            });
    }));
    let payload = panic.unwrap_err();
    assert_eq!(
        payload.downcast_ref::<&str>(),
        Some(&"arithmetic guarded visitor sentinel")
    );
    assert_eq!(original.summary().unwrap(), original_summary);
    // This transaction takes mutable guards and grows the existing cache.
    transforms
        .transaction(|| Ok::<_, crate::Invalid>(()))
        .unwrap();
    assert_eq!(original.summary().unwrap(), original_summary);
    assert!(original.summary().unwrap().require_complete().is_err());
    eprintln!(
        "actual bank + editor/history + previous loaded context + guarded cancellation/cache DATA={expected}B; allocator={allocations:?}; panic preserves original ledger and subsequent transaction succeeds"
    );
}

#[test]
fn arithmetic_original_ledger_actual_backings_and_union_are_charged_without_duplicate_data() {
    let first = state();
    let second = state();
    let mut original = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    original
        .borrow_canonical_native_state_on_control(&first)
        .unwrap();
    let original_summary = original.summary().unwrap();
    let configured = AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 64,
        root_rows: 8,
        ..reservation()
    };
    let mut normal = AudioRootStorageLedger::try_new_on_control(configured).unwrap();
    normal
        .borrow_canonical_native_state_on_control(&first)
        .unwrap();
    normal
        .borrow_canonical_native_state_on_control(&second)
        .unwrap();
    let data = normal.summary().unwrap().data_bytes;
    normal.original_ledger_storage(&original).unwrap();
    assert_eq!(
        normal.summary().unwrap().data_bytes,
        data + original_summary.ledger_bytes
    );
    assert_eq!(normal.summary().unwrap().unresolved_handles, 2);
    let mut result = None;
    let allocations = allocation_counter::measure(|| {
        result = Some(AudioRootStorageLedger::arithmetic_cost(
            configured,
            data + original_summary.ledger_bytes,
            |ledger| {
                ledger.borrow_canonical_native_state_on_control(&first)?;
                ledger.borrow_canonical_native_state_on_control(&second)?;
                ledger.original_ledger_storage(&original)
            },
        ));
    });
    assert_eq!(
        result.unwrap().unwrap().data_bytes,
        data + original_summary.ledger_bytes
    );
    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.bytes_total, 0);
    assert_eq!(allocations.count_current, 0);
    assert_eq!(allocations.bytes_current, 0);
    assert_eq!(original.summary().unwrap(), original_summary);
}
