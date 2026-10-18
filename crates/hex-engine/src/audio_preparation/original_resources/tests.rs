use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole,
    native_device_resources::{
        DeviceResourceBank, DeviceResourceCatalog, OriginalResourceLayout, OriginalResourceSource,
        ResourceCatalogReservation,
    },
};
use hex_project::{Command, Created, Editor, Project};
use std::sync::atomic::AtomicBool;

fn fixture(stereo: bool) -> (DeviceResourceBank, hex_project::AssetId) {
    let mut editor = Editor::new(Project::new(24000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "original.wav".into(),
            sample_rate: 24000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let source = if stereo {
        crate::OwnedAudioSource::try_resident(24000, vec![[0.25, -0.5]; 4])
            .ok()
            .unwrap()
    } else {
        crate::OwnedAudioSource::try_resident_mono(24000, vec![0.25; 4])
            .ok()
            .unwrap()
    };
    let inputs = crate::OwnedAudioInputs::try_new(
        vec![(asset, source)],
        crate::AudioStorageReservation {
            allocation_items: 16,
            handle_obligations: 16,
            max_prepared_data_bytes: 65536,
            max_construction_data_bytes: 65536,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap()
    .prepare_input_storage_on_control()
    .ok()
    .unwrap();
    let cancel = AtomicBool::new(false);
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &inputs,
        &[OriginalResourceSource {
            asset,
            sample_rate: 24000,
            frames: 4,
            layout: if stereo {
                OriginalResourceLayout::Stereo
            } else {
                OriginalResourceLayout::Mono
            },
        }],
        ResourceCatalogReservation {
            metadata_bytes: 65536,
            hash_frames: 4,
        },
        &cancel,
    )
    .unwrap();
    (
        DeviceResourceBank::try_new_on_control(inputs, catalog, 65536, 65536, &cancel)
            .ok()
            .unwrap(),
        asset,
    )
}
fn reservation(limit: usize) -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 64,
        root_rows: 16,
        max_retained_bytes: limit,
        max_construction_peak_bytes: 1_000_000,
    }
}
#[test]
fn original_resource_destination_visits_actual_bank_and_retires_on_control() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported original resource custody");
        return;
    }
    for stereo in [false, true] {
        eprintln!("SUPPORTED original resource custody stereo={stereo}");
        let retired_before = crate::native_device_resources::retained::RETIRED_RESOURCE_ROOTS
            .with(|count| count.get());
        let (bank, asset) = fixture(stereo);
        let lease = bank.borrow_original_source_on_control(asset, 4096).unwrap();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        // Private test injection only. No public bank adoption/install operation
        // exists yet; the legacy compiler continues to refuse nonempty custody.
        custody.original_resources = Some(bank);
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(reservation(1_000_000)).unwrap();
        ledger
            .borrow_custody_on_control(&custody, AudioStorageRole::Current)
            .unwrap();
        assert!(ledger.validate_closed_shared_handles().is_err());
        ledger
            .borrow_original_source_lease_on_control(&lease, AudioStorageRole::Caller)
            .unwrap();
        ledger.validate_closed_shared_handles().unwrap();
        let observed = ledger.summary().unwrap();
        let empty = AudioPreparationCustody::try_new_on_control().unwrap();
        let mut baseline =
            AudioRootStorageLedger::try_new_on_control(reservation(1_000_000)).unwrap();
        baseline
            .borrow_custody_on_control(&empty, AudioStorageRole::Caller)
            .unwrap();
        let empty_bytes = baseline.summary().unwrap().data_bytes;
        let mut independent =
            AudioRootStorageLedger::try_new_on_control(reservation(1_000_000)).unwrap();
        independent
            .borrow_device_resource_bank_on_control(
                custody.original_resources.as_ref().unwrap(),
                AudioStorageRole::Caller,
            )
            .unwrap();
        independent
            .borrow_original_source_lease_on_control(&lease, AudioStorageRole::Caller)
            .unwrap();
        independent.validate_closed_shared_handles().unwrap();
        assert_eq!(
            observed.data_bytes - empty_bytes,
            independent.summary().unwrap().data_bytes - std::mem::size_of::<DeviceResourceBank>()
        );
        drop(independent);
        drop(baseline);
        let mut omitted =
            AudioRootStorageLedger::try_new_on_control(reservation(1_000_000)).unwrap();
        omitted
            .borrow_original_source_lease_on_control(&lease, AudioStorageRole::Caller)
            .unwrap();
        assert!(omitted.validate_closed_shared_handles().is_err());
        drop(omitted);
        // Repeat embedded field traversal, not registration of a second root.
        custody
            .visit_audio_storage(&mut ledger, AudioStorageRole::Current)
            .unwrap();
        assert_eq!(ledger.summary().unwrap(), observed);
        drop(ledger);
        let mut exact =
            AudioRootStorageLedger::try_new_on_control(reservation(observed.retained_bytes))
                .unwrap();
        exact
            .borrow_custody_on_control(&custody, AudioStorageRole::Current)
            .unwrap();
        exact
            .borrow_original_source_lease_on_control(&lease, AudioStorageRole::Caller)
            .unwrap();
        exact.validate_closed_shared_handles().unwrap();
        drop(exact);
        let mut short =
            AudioRootStorageLedger::try_new_on_control(reservation(observed.retained_bytes - 1))
                .unwrap();
        let result = short
            .borrow_custody_on_control(&custody, AudioStorageRole::Current)
            .and_then(|()| {
                short.borrow_original_source_lease_on_control(&lease, AudioStorageRole::Caller)
            });
        assert_eq!(result, Err(AudioStorageError::ReservationExceeded));
        drop(short);
        assert!(custody.original_resources.is_some());
        custody.close_on_control();
        custody.cleanup_on_control();
        assert_eq!(custody.phase(), AudioPreparationPhase::Vacant);
        assert!(custody.original_resources.is_none());
        assert!(!custody.original_resources_drop_pending);
        let view = lease.view().unwrap();
        assert_eq!(view.sample_rate(), 24000);
        if stereo {
            assert_eq!(view.resident_frames().unwrap(), &[[0.25, -0.5]; 4]);
        } else {
            assert_eq!(view.resident_mono_samples().unwrap(), &[0.25; 4]);
        }
        assert_eq!(
            crate::native_device_resources::retained::RETIRED_RESOURCE_ROOTS
                .with(|count| count.get()),
            retired_before
        );
        drop(lease);
        assert_eq!(
            crate::native_device_resources::retained::RETIRED_RESOURCE_ROOTS
                .with(|count| count.get()),
            retired_before + 1
        );
        custody.cleanup_on_control(); // normal cleanup is idempotent
    }
}

#[test]
fn original_resource_destination_moves_with_real_candidate_and_retires_after_runtime() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported original resource candidate move");
        return;
    }
    for stereo in [false, true] {
        eprintln!("SUPPORTED original resource candidate move stereo={stereo}");
        let (resources, asset) = fixture(stereo);
        let lease = resources
            .borrow_original_source_on_control(asset, 4096)
            .unwrap();
        let Some((mut custody, token, _, _)) =
            crate::audio_preparation::candidate_test_fixture(false)
        else {
            panic!("supported candidate fixture");
        };
        // The compiled audio-only fixture is already prepared. Inject only this
        // selected child; no compiler/resource admission seal is granted here.
        custody.original_resources = Some(resources);
        let summary = custody
            .original_resources
            .as_ref()
            .unwrap()
            .input_storage_summary();
        let mut draft = crate::AudioCandidateDraft::new_on_control();
        draft.prepare_on_control(&custody, &token).unwrap();
        let mut custody = Some(custody);
        let mut token = Some(token);
        let mut owner = None;
        draft
            .finish_on_control(&mut custody, &mut token, &mut owner)
            .unwrap();
        assert!(custody.is_none() && token.is_none());
        let mut owner = owner.unwrap();
        assert_eq!(
            owner
                .preparation()
                .original_resources
                .as_ref()
                .unwrap()
                .input_storage_summary(),
            summary
        );
        owner.dispose_on_control().unwrap();
        assert!(owner.preparation().original_resources.is_none());
        assert!(!owner.preparation().original_resources_drop_pending);
        let inputs = owner.recover_inputs_on_control().unwrap();
        assert!(!inputs.sources.is_empty());
        let view = lease.view().unwrap();
        if stereo {
            assert_eq!(view.resident_frames().unwrap(), &[[0.25, -0.5]; 4]);
        } else {
            assert_eq!(view.resident_mono_samples().unwrap(), &[0.25; 4]);
        }
    }
}

#[test]
fn original_resource_destination_unresolved_drop_refuses_observation_and_retry() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported original resource unresolved custody");
        return;
    }
    eprintln!("SUPPORTED original resource unresolved custody");
    let (resources, _) = fixture(false);
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    custody.original_resources = Some(resources);
    // Inject an unresolved marker, not a successful final-drop or panic proof.
    custody.original_resources_drop_pending = true;
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation(1_000_000)).unwrap();
    assert_eq!(
        ledger.borrow_custody_on_control(&custody, AudioStorageRole::FailedConstruction),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(ledger.summary().is_err());
    drop(ledger);
    custody.cleanup_on_control();
    assert_eq!(custody.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(custody.original_resources.is_some());
    custody.cleanup_on_control();
    assert!(custody.original_resources.is_some());
}
