use super::*;
use crate::{AudioStorageReservation, OwnedAudioInputs, OwnedAudioSource};
use hex_lattice_contract::{DeviceAssetReference, DeviceTypeId};
const PCM: [[f32; 2]; 4] = [[0.25, -0.75], [-0.5, 1.0], [0.75, -0.25], [-1.0, 0.5]];
pub(crate) fn ids() -> Vec<AssetId> {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(44_100).unwrap(), 8).unwrap();
    (0..3)
        .map(|n| {
            let hex_project::Created::Asset(id) = editor
                .execute(hex_project::Command::AddAsset {
                    path: format!("source-{n}.wav").into(),
                    sample_rate: 44_100,
                    frames: 4,
                })
                .unwrap()
            else {
                panic!("asset")
            };
            id
        })
        .collect()
}
pub(crate) fn bank(ids: &[AssetId]) -> Option<AudioInputStorageAndObligations> {
    if !crate::pcm_arc::native_layout_known() {
        assert!(OwnedAudioSource::try_resident(44_100, PCM.to_vec()).is_err());
        eprintln!("resource bank unsupported constructor: no native ownership proof");
        return None;
    }
    let sources = vec![
        (
            ids[0],
            OwnedAudioSource::try_resident(44_100, PCM.to_vec())
                .ok()
                .unwrap(),
        ),
        (
            ids[1],
            OwnedAudioSource::try_resident(44_100, vec![[0.5; 2]; 4])
                .ok()
                .unwrap(),
        ),
    ];
    let reservation = AudioStorageReservation {
        allocation_items: 8,
        handle_obligations: 8,
        max_prepared_data_bytes: 65_536,
        max_construction_data_bytes: 65_536,
        caller_retained_bytes: 0,
        caller_construction_bytes: 0,
    };
    Some(
        OwnedAudioInputs::try_new(sources, reservation)
            .ok()
            .unwrap()
            .prepare_input_storage_on_control()
            .ok()
            .unwrap(),
    )
}
pub(crate) fn specs(ids: &[AssetId]) -> Vec<OriginalResourceSource> {
    ids[..2]
        .iter()
        .map(|&asset| OriginalResourceSource {
            layout: OriginalResourceLayout::Stereo,
            asset,
            sample_rate: 44_100,
            frames: 4,
        })
        .collect()
}
pub(crate) fn limits() -> ResourceCatalogReservation {
    ResourceCatalogReservation {
        metadata_bytes: 65_536,
        hash_frames: 8,
    }
}
pub(crate) fn state(asset: AssetId) -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::from_static("test.resource-scope"), 1, vec![])
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap()
}
#[test]
fn actual_original_bank_scopes_preserve_stereo_rate_and_reused_slots() {
    let ids = ids();
    let Some(bank) = bank(&ids) else { return };
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &bank,
        &specs(&ids),
        limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let old = catalog
        .prepare_scope_on_control(&state(ids[0]), 4096)
        .unwrap();
    let candidate = catalog
        .prepare_scope_on_control(&state(ids[1]), 4096)
        .unwrap();
    let old_view = catalog.resources(&bank, &old).unwrap();
    let next_view = catalog.resources(&bank, &candidate).unwrap();
    let accepted = old_view.get(0).unwrap();
    assert_eq!(accepted.sample_rate, 44_100);
    // Independent Python hashlib/struct little-endian protocol oracle, not the
    // implementation's hashing helper. Asset IDs are intentionally excluded.
    assert_eq!(
        accepted.identity.content,
        [
            0x74, 0x5f, 0x58, 0x42, 0xb9, 0xf7, 0x5e, 0xf5, 0x6d, 0x65, 0xc6, 0x2d, 0x2c, 0xd6,
            0x95, 0xf1, 0x7b, 0x60, 0xa2, 0x05, 0xe6, 0x8a, 0xb9, 0xd1, 0x56, 0x50, 0x3f, 0x37,
            0x2e, 0xc8, 0xa1, 0x92
        ]
    );
    let tuning = hex_lattice::TuningTable::default();
    let context = hex_lattice::DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources: &old_view,
    };
    context.validate_for(&state(ids[0])).unwrap();
    assert!(context.validate_for(&state(ids[1])).is_err());
    bank.validate_unresolved_on_control().unwrap();

    assert_eq!(accepted.slot, next_view.get(0).unwrap().slot);
    assert_ne!(accepted.identity, next_view.get(0).unwrap().identity);
    for (age, expected) in PCM.into_iter().enumerate() {
        assert_eq!(accepted.pcm.frame(age), Some(expected));
        assert_eq!(next_view.get(0).unwrap().pcm.frame(age), Some([0.5; 2]));
    }
    // Preparing/reusing the same authored slot does not replace the old actual scope.
    assert_eq!(old_view.get(0).unwrap().pcm.frame(3), Some(PCM[3]));
    assert!(old_view.get(1).is_none());
    assert!(catalog.declared_metadata_bytes().unwrap() <= limits().metadata_bytes);
    assert!(old.declared_metadata_bytes().unwrap() <= 4096);
    eprintln!("resource bank actual original scopes: supported-controls=completed");
}
#[test]
fn content_identity_does_not_rebind_tokens_or_grant_another_catalog_scope() {
    let ids = ids();
    let Some(bank_a) = bank(&ids) else { return };
    let bank_b = bank(&ids).unwrap();
    let cancel = AtomicBool::new(false);
    let a = DeviceResourceCatalog::prepare_on_control(&bank_a, &specs(&ids), limits(), &cancel)
        .unwrap();
    let b = DeviceResourceCatalog::prepare_on_control(&bank_b, &specs(&ids), limits(), &cancel)
        .unwrap();
    let old = a.prepare_scope_on_control(&state(ids[0]), 4096).unwrap();
    assert_eq!(
        a.identity(ids[0]),
        b.identity(ids[0]),
        "same verified PCM content, different actual bank"
    );
    assert!(matches!(
        a.resources(&bank_b, &old),
        Err(ResourcePreparationError::Storage(
            AudioStorageError::SourceMismatch
        ))
    ));
    assert!(matches!(
        b.resources(&bank_b, &old),
        Err(ResourcePreparationError::InvalidScope)
    ));
    assert_eq!(
        a.resources(&bank_a, &old)
            .unwrap()
            .get(0)
            .unwrap()
            .pcm
            .frame(0),
        Some(PCM[0])
    );
}
#[test]
fn refusal_cancel_and_exact_budgets_leave_actual_bank_unchanged() {
    let ids = ids();
    let Some(bank) = bank(&ids) else { return };
    let before = bank.summary();
    let expected = specs(&ids);
    let cancel = AtomicBool::new(false);
    for (reservation, error) in [
        (
            ResourceCatalogReservation {
                metadata_bytes: 0,
                ..limits()
            },
            ResourcePreparationError::MetadataCapacity,
        ),
        (
            ResourceCatalogReservation {
                hash_frames: 7,
                ..limits()
            },
            ResourcePreparationError::WorkCapacity,
        ),
    ] {
        assert!(
            matches!(DeviceResourceCatalog::prepare_on_control(&bank,&expected,reservation,&cancel),Err(actual) if actual==error)
        );
        assert_eq!(bank.summary(), before);
    }
    let mut wrong = specs(&ids);
    wrong[0].sample_rate = 48_000;
    assert!(matches!(
        DeviceResourceCatalog::prepare_on_control(&bank, &wrong, limits(), &cancel),
        Err(ResourcePreparationError::InvalidSource)
    ));
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(
        DeviceResourceCatalog::prepare_on_control(&bank, &expected, limits(), &cancel),
        Err(ResourcePreparationError::Cancelled)
    ));
    assert_eq!(bank.summary(), before);
    cancel.store(false, Ordering::Relaxed);
    let catalog =
        DeviceResourceCatalog::prepare_on_control(&bank, &expected, limits(), &cancel).unwrap();
    let bytes = catalog.declared_metadata_bytes().unwrap();
    assert!(
        DeviceResourceCatalog::prepare_on_control(
            &bank,
            &expected,
            ResourceCatalogReservation {
                metadata_bytes: bytes,
                ..limits()
            },
            &cancel
        )
        .is_ok()
    );
    assert!(matches!(
        catalog.prepare_scope_on_control(&state(ids[2]), 4096),
        Err(ResourcePreparationError::InvalidScope)
    ));
    let scope = catalog
        .prepare_scope_on_control(&state(ids[0]), 4096)
        .unwrap();
    let bytes = scope.declared_metadata_bytes().unwrap();
    assert!(
        catalog
            .prepare_scope_on_control(&state(ids[0]), bytes)
            .is_ok()
    );
    assert!(matches!(
        catalog.prepare_scope_on_control(&state(ids[0]), bytes - 1),
        Err(ResourcePreparationError::MetadataCapacity)
    ));
    assert_eq!(bank.summary(), before);
}

#[test]
fn retained_scopes_inventory_one_actual_bank_and_every_physical_handle() {
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
    let ids = ids();
    let Some(bank) = bank(&ids) else { return };
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &bank,
        &specs(&ids),
        limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let owner = DeviceResourceBank::try_new_on_control(
        bank,
        catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    let old = owner
        .prepare_lease_on_control(&state(ids[0]), 4096)
        .unwrap();
    let candidate = owner
        .prepare_lease_on_control(&state(ids[1]), 4096)
        .unwrap();
    let reservation = AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 16,
        root_rows: 8,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    };
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    ledger
        .borrow_device_resource_bank_on_control(&owner, AudioStorageRole::Caller)
        .unwrap();
    let before = ledger.summary().unwrap();
    assert_eq!(before.unresolved_handles, 3);
    assert!(ledger.validate_closed_shared_handles().is_err());
    ledger
        .borrow_device_resource_lease_on_control(&old, AudioStorageRole::Predecessor)
        .unwrap();
    ledger
        .borrow_device_resource_lease_on_control(&candidate, AudioStorageRole::Current)
        .unwrap();
    let after = ledger.summary().unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(after.unresolved_handles, 5);
    assert_eq!(after.unresolved_retirements, 3);
    assert_eq!(after.unknown_storage, 0);
    assert_eq!(
        after.data_bytes - before.data_bytes,
        old.declared_metadata_bytes().unwrap() + candidate.declared_metadata_bytes().unwrap()
    );
    assert_eq!(after.data_allocations - before.data_allocations, 2);
    assert_eq!(
        after.require_complete(),
        Err(crate::AudioStorageIncomplete::RetirementReceiptUnknown)
    );
    let exact = after.retained_bytes;
    drop(ledger);
    let mut refused = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        max_retained_bytes: exact - 1,
        ..reservation
    })
    .unwrap();
    refused
        .borrow_device_resource_bank_on_control(&owner, AudioStorageRole::Caller)
        .unwrap();
    refused
        .borrow_device_resource_lease_on_control(&old, AudioStorageRole::Predecessor)
        .unwrap();
    assert_eq!(
        refused.borrow_device_resource_lease_on_control(&candidate, AudioStorageRole::Current),
        Err(AudioStorageError::ReservationExceeded)
    );
    assert!(refused.summary().is_err());
    drop(refused);
    assert_eq!(old.get(0).unwrap().pcm.frame(0), Some(PCM[0]));
    assert_eq!(candidate.get(0).unwrap().pcm.frame(0), Some([0.5; 2]));
}

#[test]
fn retained_root_refusals_preserve_native_bank_and_leave_no_extra_owning_handle() {
    let ids = ids();
    let Some(bank) = bank(&ids) else { return };
    let before = bank.summary();
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &bank,
        &specs(&ids),
        limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let refusal =
        DeviceResourceBank::try_new_on_control(bank, catalog, 0, 65_536, &AtomicBool::new(false))
            .err()
            .unwrap();
    assert_eq!(
        refusal.reason,
        AudioStorageError::ReservationExceeded.into()
    );
    assert_eq!(refusal.bank.summary(), before);
    let refusal = DeviceResourceBank::try_new_on_control(
        refusal.bank,
        refusal.catalog,
        65_536,
        65_536,
        &AtomicBool::new(true),
    )
    .err()
    .unwrap();
    assert_eq!(refusal.reason, ResourcePreparationError::Cancelled);
    let scope = refusal
        .catalog
        .prepare_scope_on_control(&state(ids[0]), 4096)
        .unwrap();
    assert_eq!(
        refusal
            .catalog
            .resources(&refusal.bank, &scope)
            .unwrap()
            .get(0)
            .unwrap()
            .pcm
            .frame(0),
        Some(PCM[0])
    );
    let owner = DeviceResourceBank::try_new_on_control(
        refusal.bank,
        refusal.catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    assert_eq!(
        owner
            .prepare_lease_on_control(
                &state(ids[0]),
                std::mem::size_of::<PreparedDeviceResourceLease>() - 1
            )
            .err(),
        Some(ResourcePreparationError::MetadataCapacity)
    );
    let mut ledger =
        crate::AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
            data_rows: 64,
            handle_rows: 16,
            root_rows: 8,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        })
        .unwrap();
    ledger
        .borrow_device_resource_bank_on_control(&owner, crate::AudioStorageRole::Caller)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(ledger.summary().unwrap().unresolved_handles, 3);
}

#[test]
fn native_mono_custody_preserves_actual_layout_and_deduplicates_bounded_aliases() {
    let ids = ids();
    let mut samples = Vec::with_capacity(9);
    samples.extend([0.25, -0.5, 0.75, -1.0]);
    let pointer = samples.as_ptr();
    let result = OwnedAudioSource::try_resident_mono(44_100, samples);
    if !crate::pcm_arc::native_layout_known() {
        assert_eq!(
            result.err().unwrap().reason,
            AudioStorageError::UnsupportedConstructor
        );
        return;
    }
    let source = result.ok().unwrap();
    let reservation = AudioStorageReservation {
        allocation_items: 8,
        handle_obligations: 4,
        max_prepared_data_bytes: 65_536,
        max_construction_data_bytes: 65_536,
        caller_retained_bytes: 0,
        caller_construction_bytes: 0,
    };
    let mut bank = OwnedAudioInputs::try_new(vec![(ids[0], source)], reservation)
        .ok()
        .unwrap();
    let token = bank.source_token(ids[0]).unwrap();
    assert!(
        bank.source_view(&token)
            .unwrap()
            .resident_frames()
            .is_none()
    );
    assert_eq!(
        bank.source_view(&token)
            .unwrap()
            .resident_mono_samples()
            .unwrap()
            .as_ptr(),
        pointer
    );
    for _ in 0..3 {
        bank.alias_on_control(&token).unwrap();
    }
    assert_eq!(
        bank.alias_on_control(&token).err(),
        Some(AudioStorageError::Capacity)
    );
    let bank = bank.prepare_input_storage_on_control().ok().unwrap();
    assert_eq!(bank.summary().unique_allocations, 2);
    assert_eq!(bank.summary().unresolved_handles, 4);
    let expected = [OriginalResourceSource {
        asset: ids[0],
        sample_rate: 44_100,
        frames: 4,
        layout: OriginalResourceLayout::Mono,
    }];
    let mut wrong_layout = expected;
    wrong_layout[0].layout = OriginalResourceLayout::Stereo;
    assert!(matches!(
        DeviceResourceCatalog::prepare_on_control(
            &bank,
            &wrong_layout,
            limits(),
            &AtomicBool::new(false)
        ),
        Err(ResourcePreparationError::InvalidSource)
    ));
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &bank,
        &expected,
        limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let expected_hash: [u8; 32] = [
        0x1d, 0xdf, 0x45, 0xd7, 0x99, 0xd2, 0x24, 0x11, 0x06, 0x68, 0xae, 0x17, 0x69, 0xcf, 0xac,
        0xba, 0x98, 0x47, 0x82, 0xe7, 0xb5, 0x20, 0x2d, 0x3b, 0x7a, 0x04, 0x1e, 0x2d, 0xc4, 0x3a,
        0x6e, 0xf1,
    ];
    assert_eq!(catalog.identity(ids[0]).unwrap().content, expected_hash);
    let owner = DeviceResourceBank::try_new_on_control(
        bank,
        catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    let lease = owner
        .prepare_lease_on_control(&state(ids[0]), 4096)
        .unwrap();
    let view = lease.get(0).unwrap();
    assert_eq!(view.sample_rate, 44_100);
    let DeviceResourcePcm::Mono(actual) = view.pcm else {
        panic!("actual mono layout was substituted")
    };
    assert_eq!(actual.as_ptr(), pointer);
    assert_eq!(actual, [0.25, -0.5, 0.75, -1.0]);
    let note = hex_lattice::conformance::resource_pcm::prepare_note(
        &state(ids[0]),
        &hex_lattice::DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &lease,
        },
        0,
    )
    .unwrap();
    for (age, sample) in actual.iter().copied().enumerate() {
        assert_eq!(note.sample(age as u64, 4, &lease), [sample; 2]);
    }
    let mut ledger =
        crate::AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
            data_rows: 64,
            handle_rows: 16,
            root_rows: 8,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        })
        .unwrap();
    ledger
        .borrow_device_resource_bank_on_control(&owner, crate::AudioStorageRole::Caller)
        .unwrap();
    ledger
        .borrow_device_resource_lease_on_control(&lease, crate::AudioStorageRole::Current)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert_eq!(ledger.summary().unwrap().unresolved_handles, 6);
}

#[test]
fn mono_constructor_numeric_refusals_return_the_actual_caller_buffer() {
    for (rate, values, expected) in [
        (0, vec![0.25], AudioStorageError::InvalidRate),
        (44_100, vec![], AudioStorageError::InvalidSamples),
        (44_100, vec![f32::NAN], AudioStorageError::InvalidSamples),
        (
            44_100,
            vec![f32::INFINITY],
            AudioStorageError::InvalidSamples,
        ),
    ] {
        let pointer = values.as_ptr();
        let capacity = values.capacity();
        let bits: Vec<_> = values.iter().map(|sample| sample.to_bits()).collect();
        let refusal = OwnedAudioSource::try_resident_mono(rate, values)
            .err()
            .unwrap();
        assert_eq!(refusal.reason, expected);
        assert_eq!(refusal.samples.as_ptr(), pointer);
        assert_eq!(refusal.samples.capacity(), capacity);
        assert_eq!(
            refusal
                .samples
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
            bits
        );
    }
}

#[test]
fn original_media_views_share_actual_root_and_close_every_handle_before_control_retirement() {
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
    let ids = ids();
    let Some(bank) = bank(&ids) else { return };
    let retired_before = super::retained::RETIRED_RESOURCE_ROOTS.with(|n| n.get());
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &bank,
        &specs(&ids),
        limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let owner = DeviceResourceBank::try_new_on_control(
        bank,
        catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    let quote = std::mem::size_of::<OriginalSourceLease>();
    assert!(matches!(
        owner.borrow_original_source_on_control(ids[0], quote - 1),
        Err(ResourcePreparationError::MetadataCapacity)
    ));
    assert!(matches!(
        owner.borrow_original_source_on_control(ids[2], quote),
        Err(ResourcePreparationError::InvalidSource)
    ));
    let media = owner
        .borrow_original_source_on_control(ids[0], quote)
        .unwrap();
    assert!(matches!(
        media.try_clone_on_control(quote - 1),
        Err(ResourcePreparationError::MetadataCapacity)
    ));
    let snapshot = media.try_clone_on_control(quote).unwrap();
    assert!(snapshot.shares_source_with(&media));
    let other = owner
        .borrow_original_source_on_control(ids[1], quote)
        .unwrap();
    assert!(!other.shares_source_with(&media));
    let provider = owner
        .prepare_lease_on_control(&state(ids[0]), 4096)
        .unwrap();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 16,
        root_rows: 8,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    })
    .unwrap();
    ledger
        .borrow_device_resource_bank_on_control(&owner, AudioStorageRole::Caller)
        .unwrap();
    ledger
        .borrow_device_resource_lease_on_control(&provider, AudioStorageRole::Current)
        .unwrap();
    let before = ledger.summary().unwrap();
    ledger
        .borrow_original_source_lease_on_control(&media, AudioStorageRole::Caller)
        .unwrap();
    ledger
        .borrow_original_source_lease_on_control(&other, AudioStorageRole::Archive)
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    ledger
        .borrow_original_source_lease_on_control(&snapshot, AudioStorageRole::PendingReclaim)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let after = ledger.summary().unwrap();
    assert_eq!(after.data_bytes - before.data_bytes, quote * 3);
    assert_eq!(after.unresolved_handles - before.unresolved_handles, 3);
    assert_eq!(after.unknown_storage, 0);
    assert_eq!(
        after.require_complete(),
        Err(crate::AudioStorageIncomplete::RetirementReceiptUnknown)
    );
    drop(ledger);
    drop(owner);
    drop(provider);
    drop(other);
    assert_eq!(media.source_facts().unwrap().asset, ids[0]);
    assert_eq!(media.view().unwrap().resident_frames().unwrap(), PCM);
    let returned = std::thread::spawn(move || {
        assert_eq!(snapshot.view().unwrap().resident_frames().unwrap(), PCM);
        // Borrow only on the worker; final ownership returns to control.
        snapshot
    })
    .join()
    .unwrap();
    drop(media);
    assert_eq!(
        super::retained::RETIRED_RESOURCE_ROOTS.with(|n| n.get()),
        retired_before
    );
    drop(returned);
    assert_eq!(
        super::retained::RETIRED_RESOURCE_ROOTS.with(|n| n.get()),
        retired_before + 1
    );
}
