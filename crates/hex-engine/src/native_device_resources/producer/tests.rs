use super::*;
use std::sync::atomic::AtomicBool;

#[test]
fn original_plan_inspection_covers_actual_input_backing_aliases_without_births() {
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
    let ids = crate::native_device_resources::tests::ids();
    let source = OwnedAudioSource::try_resident_mono(44100, vec![0.25; 4])
        .ok()
        .unwrap();
    let mut entries = Vec::with_capacity(7);
    entries.push(OriginalResourceInput::native(ids[0], &source));
    entries.push(OriginalResourceInput::native(ids[1], &source));
    let plan = OriginalResourceBankPlan::borrow(&entries);
    let reservation = AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 16,
        root_rows: 8,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    };
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    ledger
        .borrow_native_source_on_control(&source, AudioStorageRole::Caller)
        .unwrap();
    let before = ledger.summary().unwrap().data_bytes;
    crate::owned_shared::reset_birth_count();
    crate::owned_shared::reset_clone_count();
    let ids_before = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
    let measured = allocation_counter::measure(|| {
        plan.visit_storage_on_control(&mut ledger).unwrap();
        plan.visit_storage_on_control(&mut ledger).unwrap();
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(crate::owned_shared::birth_count(), 0);
    assert_eq!(crate::owned_shared::clone_count(), 0);
    assert_eq!(
        crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
        ids_before
    );
    assert_eq!(
        ledger.summary().unwrap().data_bytes - before,
        size_of_val(&entries) + entries.capacity() * size_of::<OriginalResourceInput<'_>>()
    );
    ledger.validate_closed_shared_handles().unwrap();
    let measured = allocation_counter::measure(|| {
        AudioRootStorageLedger::arithmetic_cost_for_limits(
            reservation,
            usize::MAX,
            usize::MAX,
            |ledger| plan.visit_storage_on_control(ledger),
        )
        .unwrap();
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(
        crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
        ids_before
    );
    let raw = crate::Audio::new(44100, vec![[0.0; 2]; 4]).unwrap();
    let raw_entries = vec![OriginalResourceInput::stereo(ids[0], &raw)];
    let raw_plan = OriginalResourceBankPlan::borrow(&raw_entries);
    let mut refused = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    assert_eq!(
        raw_plan.visit_storage_on_control(&mut refused),
        Err(AudioStorageError::UnknownStorage)
    );
    assert_eq!(refused.summary(), Err(AudioStorageError::UnknownStorage));
}

fn limits() -> OriginalResourcePlanReservation {
    OriginalResourcePlanReservation {
        inputs: crate::AudioStorageReservation {
            allocation_items: 16,
            handle_obligations: 32,
            max_prepared_data_bytes: 1_000_000,
            max_construction_data_bytes: 1_000_000,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
        catalog: ResourceCatalogReservation {
            metadata_bytes: 65536,
            hash_frames: 8,
        },
        max_preparation_metadata_bytes: 65536,
        max_retained_data_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
#[test]
fn typed_original_producer_keeps_actual_original_layout_rate_and_allocation() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported typed original producer");
        return;
    }
    eprintln!("SUPPORTED typed original producer Mono/Stereo");
    let ids = crate::native_device_resources::tests::ids();
    let stereo = OwnedAudioSource::try_resident(44100, vec![[0.25, -0.5]; 4])
        .ok()
        .unwrap()
        .try_into_native_audio_on_control()
        .ok()
        .unwrap();
    let mono = OwnedAudioSource::try_resident_mono(24000, vec![0.75; 4])
        .ok()
        .unwrap();
    let entries = vec![
        OriginalResourceInput::stereo(ids[0], &stereo),
        OriginalResourceInput::native(ids[1], &mono),
    ];
    let plan = OriginalResourceBankPlan::borrow(&entries);
    let cancel = AtomicBool::new(false);
    let bank = plan.prepare_on_control(limits(), &cancel).unwrap();
    let stereo_lease = bank
        .borrow_original_source_on_control(ids[0], 4096)
        .unwrap();
    let mono_lease = bank
        .borrow_original_source_on_control(ids[1], 4096)
        .unwrap();
    assert_eq!(stereo_lease.view().unwrap().sample_rate(), 44100);
    assert_eq!(mono_lease.view().unwrap().sample_rate(), 24000);
    assert_eq!(
        stereo_lease
            .view()
            .unwrap()
            .resident_frames()
            .unwrap()
            .as_ptr(),
        stereo.frames().as_ptr()
    );
    assert_eq!(
        mono_lease
            .view()
            .unwrap()
            .resident_mono_samples()
            .unwrap()
            .as_ptr(),
        mono.view().resident_mono_samples().unwrap().as_ptr()
    );
    drop(bank);
    drop(entries);
    drop(stereo);
    drop(mono);
    assert_eq!(
        stereo_lease.view().unwrap().resident_frames().unwrap(),
        &[[0.25, -0.5]; 4]
    );
    assert_eq!(
        mono_lease.view().unwrap().resident_mono_samples().unwrap(),
        &[0.75; 4]
    );
}
#[test]
fn typed_original_producer_preserves_raw_refusal_and_charges_reserved_input_capacity() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported typed original producer limits");
        return;
    }
    eprintln!("SUPPORTED typed original producer raw/metadata/cancel refusal");
    let ids = crate::native_device_resources::tests::ids();
    let raw = crate::Audio::new(44100, vec![[0.25, -0.5]; 4]).unwrap();
    let entries = vec![OriginalResourceInput::stereo(ids[0], &raw)];
    let cancel = AtomicBool::new(false);
    assert!(matches!(
        OriginalResourceBankPlan::borrow(&entries).prepare_on_control(limits(), &cancel),
        Err(ResourcePreparationError::Storage(
            AudioStorageError::UnknownStorage
        ))
    ));
    assert_eq!(raw.frames(), &[[0.25, -0.5]; 4]);
    let empty = Vec::<OriginalResourceInput<'_>>::with_capacity(17);
    let plan = OriginalResourceBankPlan::borrow(&empty);
    let input_bytes = OriginalResourceBankPlan::input_bytes(&empty).unwrap();
    assert_eq!(
        input_bytes,
        empty.capacity() * size_of::<OriginalResourceInput<'_>>()
            + size_of::<Vec<OriginalResourceInput<'_>>>()
    );
    let minimum = input_bytes
        + size_of::<Vec<(AssetId, OwnedAudioSource)>>()
        + size_of::<Vec<OriginalResourceSource>>();
    let mut exact = limits();
    exact.max_preparation_metadata_bytes = minimum;
    let bank = plan.prepare_on_control(exact, &cancel).unwrap();
    assert_eq!(bank.input_storage_summary().unique_allocations, 0);
    exact.max_preparation_metadata_bytes -= 1;
    assert!(matches!(
        plan.prepare_on_control(exact, &cancel),
        Err(ResourcePreparationError::MetadataCapacity)
    ));
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        plan.prepare_on_control(limits(), &cancelled),
        Err(ResourcePreparationError::Cancelled)
    ));
}
