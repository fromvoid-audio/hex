use super::*;
use crate::{AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation};

fn limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 32,
        handle_rows: 32,
        root_rows: 4,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}

#[test]
fn original_only_transform_inventory_charges_reserved_empty_cache_and_pending_backings() {
    if !crate::audio_storage_constructor_layout_supported() {
        return;
    }
    eprintln!(
        "SUPPORTED actual original-only transform empty reserved Vec backings and paired cancellation"
    );
    let signal = crate::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
    let job = crate::RetainedControlJob::try_from_owned_on_control(
        &mut Some(0_u8),
        usize::MAX,
        |_: &mut u8| Ok::<_, ()>(crate::RetainedControlJobProgress::Pending),
    )
    .unwrap();
    let worker = signal.bind_worker_on_control(job.route());
    let context = AudioTransformPreparation::with_retained_cancellation_on_control(worker.clone());
    context.transaction(|| Ok::<_, Invalid>(())).unwrap();
    let committed_bytes = context.cache.borrow().entries.capacity() * std::mem::size_of::<Entry>();
    assert!(committed_bytes >= MAX_CACHED_TRANSFORMS * std::mem::size_of::<Entry>());
    *context.pending.borrow_mut() = Some(Vec::with_capacity(17));
    let pending_bytes =
        context.pending.borrow().as_ref().unwrap().capacity() * std::mem::size_of::<Entry>();
    let collect = |reservation| {
        context.with_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
            signal.visit_storage_on_control(&mut ledger)?;
            worker.visit_storage_on_control(&mut ledger)?;
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            let omitted = ledger.summary()?.data_bytes;
            inventory.visit_original_only_control_storage_on_control(&mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            let observed = ledger.summary()?;
            assert_eq!(
                observed.data_bytes,
                omitted + committed_bytes + pending_bytes
            );
            assert_eq!(observed.data_allocations, 3);
            inventory.visit_original_only_control_storage_on_control(&mut ledger)?;
            assert_eq!(ledger.summary()?, observed);
            assert!(context.cache.try_borrow_mut().is_err());
            assert!(context.pending.try_borrow_mut().is_err());
            assert!(context.tempo.try_borrow_mut().is_err());
            Ok(observed)
        })
    };
    let observed = collect(limits()).unwrap();
    let mut exact = limits();
    exact.max_retained_bytes = observed.retained_bytes;
    exact.max_construction_peak_bytes = observed.construction_peak_bytes;
    assert_eq!(collect(exact).unwrap(), observed);
    exact.max_retained_bytes -= 1;
    assert!(collect(exact).is_err());
    let raw = AudioTransformPreparation::default();
    let local = AudioTransformPreparation {
        budget: budget::BudgetOwner::local(1_000_000),
        ..AudioTransformPreparation::with_retained_cancellation_on_control(worker.clone())
    };
    for unsupported in [&raw, &local] {
        unsupported
            .with_pcm_inventory_on_control(|inventory| {
                let mut ledger = AudioRootStorageLedger::try_new_on_control(limits())?;
                assert_eq!(
                    inventory.visit_original_only_control_storage_on_control(&mut ledger),
                    Err(AudioStorageError::UnknownStorage)
                );
                Ok(())
            })
            .unwrap();
    }
    // Even an EMPTY abandoned transaction is failed custody, not an admitted
    // empty-cache graph. Use the actual supported signal/production budget so
    // another unsupported child cannot accidentally explain this refusal.
    let failed = AudioTransformPreparation::with_retained_cancellation_on_control(worker.clone());
    let fault = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), Invalid> = failed.transaction(|| {
            panic!("injected empty transform transaction failure");
        });
    }));
    assert!(fault.is_err());
    assert!(failed.failed_incomplete_on_control());
    failed
        .with_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits())?;
            assert_eq!(
                inventory.visit_original_only_control_storage_on_control(&mut ledger),
                Err(AudioStorageError::UnknownStorage)
            );
            Ok(())
        })
        .unwrap();
    // A real retained tempo owner is not qualified by the empty-cache proof.
    context
        .shared_tempo(hex_project::Project::new(48_000).unwrap().tempo_map())
        .unwrap();
    context
        .with_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits())?;
            assert_eq!(
                inventory.visit_original_only_control_storage_on_control(&mut ledger),
                Err(AudioStorageError::UnknownStorage)
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn original_only_bootstrap_cost_tracks_actual_cache_growth_and_releases_guards() {
    assert!(
        crate::audio_storage_constructor_layout_supported(),
        "this oracle must execute on the supported pinned native std"
    );
    let signal = crate::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
    let job = crate::RetainedControlJob::try_from_owned_on_control(
        &mut Some(0_u8),
        usize::MAX,
        |_: &mut u8| Ok::<_, ()>(crate::RetainedControlJobProgress::Pending),
    )
    .unwrap();
    let worker = signal.bind_worker_on_control(job.route());
    for capacity in [0, 1, 129, 256, 400] {
        let context =
            AudioTransformPreparation::with_retained_cancellation_on_control(worker.clone());
        context.cache.borrow_mut().entries.reserve_exact(capacity);
        let old_capacity = context.cache.borrow().entries.capacity();
        let old_bytes = old_capacity * size_of::<Entry>();
        let cost = context
            .with_pcm_inventory_on_control(|inventory| {
                let cost = inventory.original_only_storage_cost()?;
                assert_eq!(cost.existing_backing_bytes, old_bytes);
                assert!(context.cache.try_borrow_mut().is_err());
                assert!(context.pending.try_borrow_mut().is_err());
                assert!(context.tempo.try_borrow_mut().is_err());
                Ok(cost)
            })
            .unwrap();
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        context.transaction(|| Ok::<_, Invalid>(())).unwrap();
        let actual = context.cache.borrow().entries.capacity() * size_of::<Entry>();
        assert!(actual >= MAX_CACHED_TRANSFORMS * size_of::<Entry>());
        if old_capacity < MAX_CACHED_TRANSFORMS {
            assert_eq!(cost.transaction_new_backing_bytes, actual);
            // Old allocation is STILL part of the preconstruction union.
            assert_eq!(
                cost.existing_backing_bytes + cost.transaction_new_backing_bytes,
                old_bytes + actual
            );
        } else {
            assert_eq!(cost.transaction_new_backing_bytes, 0);
            assert_eq!(actual, old_bytes);
        }
        assert_eq!(crate::owned_shared::birth_count(), 0);
        assert_eq!(crate::owned_shared::clone_count(), 0);
        *context.pending.borrow_mut() = Some(Vec::with_capacity(17));
        let pending_bytes =
            context.pending.borrow().as_ref().unwrap().capacity() * size_of::<Entry>();
        context
            .with_pcm_inventory_on_control(|inventory| {
                assert_eq!(
                    inventory
                        .original_only_storage_cost()?
                        .existing_backing_bytes,
                    actual + pending_bytes
                );
                Ok(())
            })
            .unwrap();
        let failed_visit: Result<(), AudioStorageError> =
            context.with_pcm_inventory_on_control(|_| Err(AudioStorageError::Capacity));
        assert_eq!(failed_visit, Err(AudioStorageError::Capacity));
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _: Result<(), AudioStorageError> = context.with_pcm_inventory_on_control(|_| {
                panic!("injected scoped inventory panic");
            });
        }));
        assert!(panic.is_err());
        // No guard/reference escaped either error path. This SAME persistent
        // preparation subsequently obtains the mutable transaction borrows.
        *context.pending.borrow_mut() = None;
        context.transaction(|| Ok::<_, Invalid>(())).unwrap();
        assert!(context.cache.try_borrow_mut().is_ok());
        assert!(context.pending.try_borrow_mut().is_ok());
        assert!(context.tempo.try_borrow_mut().is_ok());
        eprintln!(
            "SUPPORTED guarded metadata capacity={old_capacity} old={old_bytes} actual={actual} pending={pending_bytes}"
        );
    }
}
