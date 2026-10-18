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
