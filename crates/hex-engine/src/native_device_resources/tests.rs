use super::*;
use crate::{AudioStorageReservation, Frame, OwnedAudioInputs, OwnedAudioSource};
use hex_lattice_contract::{DeviceAssetReference, DeviceTypeId};
const PCM: [[f32; 2]; 4] = [[0.25, -0.75], [-0.5, 1.0], [0.75, -0.25], [-1.0, 0.5]];
fn ids() -> Vec<AssetId> {
    ids_with_frames(4)
}
fn ids_with_frames(frames: u64) -> Vec<AssetId> {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(44_100).unwrap(), 8).unwrap();
    (0..3)
        .map(|n| {
            let hex_project::Created::Asset(id) = editor
                .execute(hex_project::Command::AddAsset {
                    path: format!("source-{n}.wav").into(),
                    sample_rate: 44_100,
                    frames,
                })
                .unwrap()
            else {
                panic!("asset")
            };
            id
        })
        .collect()
}
fn bank(ids: &[AssetId]) -> Option<AudioInputStorageAndObligations> {
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
fn specs(ids: &[AssetId]) -> Vec<OriginalResourceSource> {
    ids[..2]
        .iter()
        .map(|&asset| OriginalResourceSource {
            asset,
            sample_rate: 44_100,
            frames: 4,
        })
        .collect()
}
fn limits() -> ResourceCatalogReservation {
    ResourceCatalogReservation {
        metadata_bytes: 65_536,
        hash_frames: 8,
    }
}
fn state(asset: AssetId) -> NativeDeviceState {
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
    for wrong_fact in [
        OriginalResourceSource {
            sample_rate: 48_000,
            ..expected[0]
        },
        OriginalResourceSource {
            frames: 5,
            ..expected[0]
        },
    ] {
        let mut wrong = specs(&ids);
        wrong[0] = wrong_fact;
        assert!(matches!(
            DeviceResourceCatalog::prepare_on_control(&bank, &wrong, limits(), &cancel),
            Err(ResourcePreparationError::InvalidSource)
        ));
    }
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
        DeviceResourceCatalog::prepare_on_control(
            &bank,
            &expected,
            ResourceCatalogReservation {
                metadata_bytes: bytes - 1,
                ..limits()
            },
            &cancel,
        ),
        Err(ResourcePreparationError::MetadataCapacity)
    ));
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
fn streamed_storage_is_not_substituted_for_original_resident_pcm() {
    let ids = ids();
    if bank(&ids).is_none() {
        return;
    }
    let metadata = crate::StreamedSourceMetadata::new(
        crate::StreamSourceKey::new(11, ids[0]),
        44_100,
        44_100,
        4,
        4,
        hex_project::IDENTITY_CONVERSION,
        [9; 32],
    )
    .unwrap();
    let source = OwnedAudioSource::try_streamed(metadata).ok().unwrap();
    let bank = OwnedAudioInputs::try_new(
        vec![(ids[0], source)],
        AudioStorageReservation {
            allocation_items: 8,
            handle_obligations: 8,
            max_prepared_data_bytes: 65_536,
            max_construction_data_bytes: 65_536,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap()
    .prepare_input_storage_on_control()
    .ok()
    .unwrap();
    let before = bank.summary();
    assert!(matches!(
        DeviceResourceCatalog::prepare_on_control(
            &bank,
            &specs(&ids)[..1],
            limits(),
            &AtomicBool::new(false),
        ),
        Err(ResourcePreparationError::InvalidSource)
    ));
    assert_eq!(bank.summary(), before);
    bank.validate_unresolved_on_control().unwrap();
}

#[test]
fn original_resource_hashes_match_scalar_protocol_at_chunk_tails() {
    if !crate::pcm_arc::native_layout_known() {
        assert!(OwnedAudioSource::try_resident(44_100, PCM.to_vec()).is_err());
        eprintln!("resource hash unsupported constructor: no native ownership proof");
        return;
    }
    for count in [1, 255, 256, 257, 511, 512, 513] {
        let ids = ids_with_frames(count as u64);
        let pcm: Vec<Frame> = (0..count)
            .map(|i| {
                [
                    if i % 17 == 0 {
                        -0.0
                    } else {
                        (i % 127) as f32 / 128.0 - 0.5
                    },
                    ((i * 19) % 127) as f32 / 128.0 - 0.5,
                ]
            })
            .collect();
        // Independent legacy byte protocol, anchored by the separate fixed
        // Python-hashlib golden above. In particular do not normalize -0.0.
        let mut reference = Sha256::new();
        reference.update(b"hex.device-resource.original-stereo.v1\0");
        reference.update(44_100_u32.to_le_bytes());
        reference.update((count as u64).to_le_bytes());
        for frame in &pcm {
            for sample in frame {
                reference.update(sample.to_bits().to_le_bytes());
            }
        }
        let expected: [u8; 32] = reference.finalize().into();
        let source = OwnedAudioSource::try_resident(44_100, pcm).ok().unwrap();
        let inputs = OwnedAudioInputs::try_new(
            vec![(ids[0], source)],
            AudioStorageReservation {
                allocation_items: 8,
                handle_obligations: 8,
                max_prepared_data_bytes: 4 * 1024 * 1024,
                max_construction_data_bytes: 8 * 1024 * 1024,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        )
        .ok()
        .unwrap();
        let before = inputs.test_bank_identity();
        let catalog = DeviceResourceCatalog::prepare_inputs_on_control(
            &inputs,
            &[OriginalResourceSource {
                asset: ids[0],
                sample_rate: 44_100,
                frames: count as u64,
            }],
            ResourceCatalogReservation {
                metadata_bytes: 65_536,
                hash_frames: count as u64,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let scope = catalog
            .prepare_scope_on_control(&state(ids[0]), 4096)
            .unwrap();
        let view = catalog.resources_from_inputs(&inputs, &scope).unwrap();
        let actual = view.get(0).unwrap();
        assert_eq!(
            actual.identity.content, expected,
            "original frame count {count}"
        );
        assert_eq!(actual.sample_rate, 44_100);
        assert_eq!(actual.pcm.frames(), count);
        assert_eq!(inputs.test_bank_identity(), before);
    }
    eprintln!("resource hash boundary controls=completed");
}
