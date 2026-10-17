use super::*;
use crate::{PreparedImport, ResidentPcm, original_audio::OriginalAudio};
use hex_engine::{Audio, AudioStorageLedgerReservation};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};

fn limits() -> ResidentResourcePlanReservation {
    ResidentResourcePlanReservation {
        inputs: AudioStorageReservation {
            allocation_items: 8,
            handle_obligations: 8,
            max_prepared_data_bytes: 1_000_000,
            max_construction_data_bytes: 1_000_000,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
        catalog: ResourceCatalogReservation {
            metadata_bytes: 65_536,
            hash_frames: 8,
        },
        max_preparation_metadata_bytes: 65_536,
        max_retained_data_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
fn ledger_limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 128,
        root_rows: 64,
        max_retained_bytes: 2_000_000,
        max_construction_peak_bytes: 2_000_000,
    }
}
fn fixture() -> Option<(Session, Vec<AssetId>)> {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported native constructor; no Session source-plan admission proof");
        return None;
    }
    let sources = [
        OwnedAudioSource::try_resident_mono(44_100, vec![0.25, -0.5, 0.75, -1.0])
            .ok()
            .unwrap(),
        OwnedAudioSource::try_resident(
            44_100,
            vec![[0.25, -0.75], [-0.5, 1.0], [0.75, -0.25], [-1.0, 0.5]],
        )
        .ok()
        .unwrap(),
    ];
    let mut session = Session::new(48_000, 16, 1024).unwrap();
    let mut ids = Vec::new();
    for source in sources {
        let prepared = PreparedImport::from_native_source(
            "source.wav".into(),
            source,
            48_000,
            hex_media::ResampleLimits {
                max_source_frames: 1024,
                max_output_frames: 1024,
            },
        )
        .unwrap();
        let (hex_project::Created::Imported { asset, .. }, revision) =
            session.import_prepared_revision(prepared, None).unwrap()
        else {
            panic!("import")
        };
        ids.push(asset);
        drop(revision); // Actual prepared graphs must join the union if retained.
    }
    ids.sort_unstable_by_key(|asset| asset.value());
    Some((session, ids))
}
fn state(ids: &[AssetId]) -> NativeDeviceState {
    NativeDeviceState::new(
        DeviceTypeId::from_static("test.session.resource.scope"),
        1,
        vec![],
    )
    .unwrap()
    .with_asset_references(
        ids.iter()
            .enumerate()
            .map(|(i, asset)| DeviceAssetReference {
                slot: DeviceAssetSlot::new(i as u32 + 1).unwrap(),
                asset: DeviceAssetKey::new(asset.value()).unwrap(),
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn actual_mixed_session_plan_reuses_originals_and_closes_only_with_every_real_pcm_owner() {
    let Some((session, ids)) = fixture() else {
        return;
    };
    let cancel = AtomicBool::new(false);
    let bank = session
        .prepare_resident_resource_bank_on_control(&ids, limits(), &cancel)
        .unwrap();
    let lease = bank.prepare_lease_on_control(&state(&ids), 4096).unwrap();
    {
        let mono = bank
            .borrow_original_source_on_control(ids[0], 4096)
            .unwrap();
        let stereo = bank
            .borrow_original_source_on_control(ids[1], 4096)
            .unwrap();
        assert_eq!(mono.source_facts().unwrap().sample_rate, 44_100);
        assert_eq!(stereo.source_facts().unwrap().sample_rate, 44_100);
        let pcm = mono.view().unwrap().resident_mono_samples().unwrap();
        assert_eq!(pcm, [0.25, -0.5, 0.75, -1.0]);
        assert_eq!(
            pcm.as_ptr(),
            session
                .source_mono(ids[0])
                .unwrap()
                .resident_mono_samples()
                .unwrap()
                .as_ptr()
        );
        let pcm = stereo.view().unwrap().resident_frames().unwrap();
        assert_eq!(
            pcm,
            [[0.25, -0.75], [-0.5, 1.0], [0.75, -0.25], [-1.0, 0.5]]
        );
        assert_eq!(
            pcm.as_ptr(),
            session.source_audio(ids[1]).unwrap().frames().as_ptr()
        );
    }
    session
        .with_resident_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits()).unwrap();
            ledger
                .borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Current)
                .unwrap();
            ledger
                .borrow_device_resource_lease_on_control(&lease, AudioStorageRole::Current)
                .unwrap();
            assert_eq!(
                ledger.validate_selected_shared_handles_on_control(),
                Err(AudioStorageError::UnknownStorage)
            );
            inventory
                .visit_on_control(&mut ledger, AudioStorageRole::Caller)
                .unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
            assert_eq!(ledger.selected_shared_counts_on_control().0, 5); // two originals, two derivatives, one retained root
            let bytes = ledger.summary().unwrap().retained_bytes;
            drop(ledger);
            let mut small = ledger_limits();
            small.max_retained_bytes = bytes - 1;
            let refuses = (|| {
                let mut ledger = AudioRootStorageLedger::try_new_on_control(small)?;
                ledger.borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Current)?;
                ledger
                    .borrow_device_resource_lease_on_control(&lease, AudioStorageRole::Current)?;
                inventory.visit_on_control(&mut ledger, AudioStorageRole::Caller)?;
                ledger.summary()
            })();
            assert!(matches!(
                refuses,
                Err(AudioStorageError::ReservationExceeded)
            ));
            // This deliberately remains incomplete retirement/publication evidence.
            assert!(bank.input_storage_summary().retained_data_bytes > 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn current_and_candidate_plans_share_pcm_but_every_bank_alias_must_be_selected() {
    let Some((session, ids)) = fixture() else {
        return;
    };
    let cancel = AtomicBool::new(false);
    let current = session
        .prepare_resident_resource_bank_on_control(&ids, limits(), &cancel)
        .unwrap();
    let candidate = session
        .prepare_resident_resource_bank_on_control(&ids, limits(), &cancel)
        .unwrap();
    session
        .with_resident_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits()).unwrap();
            inventory
                .visit_on_control(&mut ledger, AudioStorageRole::Caller)
                .unwrap();
            ledger
                .borrow_device_resource_bank_on_control(&current, AudioStorageRole::Current)
                .unwrap();
            assert_eq!(
                ledger.validate_selected_shared_handles_on_control(),
                Err(AudioStorageError::UnknownStorage)
            );
            ledger
                .borrow_device_resource_bank_on_control(&candidate, AudioStorageRole::Archive)
                .unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
            assert_eq!(ledger.selected_shared_counts_on_control().0, 6); // same four PCM allocations plus two actual roots
            drop(ledger);

            Ok(())
        })
        .unwrap();
    drop(current);
    let snapshot = candidate
        .borrow_original_source_on_control(ids[0], 4096)
        .unwrap();
    drop(candidate);
    drop(session);
    assert_eq!(
        snapshot.view().unwrap().resident_mono_samples().unwrap(),
        [0.25, -0.5, 0.75, -1.0]
    );
}

#[test]
fn plan_work_metadata_alias_and_cancel_failures_leave_actual_session_and_history_intact() {
    let Some((session, ids)) = fixture() else {
        return;
    };
    let cancel = AtomicBool::new(false);
    let before = session.project().clone();
    let charge = session.resident_frames();
    let mut cases = [limits(); 3];
    cases[0].catalog.hash_frames = 7;
    cases[1].max_preparation_metadata_bytes = 0;
    cases[2].inputs.handle_obligations = 1;
    for limits in cases {
        assert!(
            session
                .prepare_resident_resource_bank_on_control(&ids, limits, &cancel)
                .is_err()
        );
        assert_eq!(session.project(), &before);
        assert_eq!(session.resident_frames(), charge);
        assert!(session.can_undo());
        assert!(!session.can_redo());
    }
    assert!(matches!(
        session.prepare_resident_resource_bank_on_control(&[ids[1], ids[0]], limits(), &cancel),
        Err(ResourcePreparationError::InvalidSource)
    ));
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(
        session.prepare_resident_resource_bank_on_control(&ids, limits(), &cancel),
        Err(ResourcePreparationError::Cancelled)
    ));
    cancel.store(false, Ordering::Relaxed);
    let bank = session
        .prepare_resident_resource_bank_on_control(&ids, limits(), &cancel)
        .unwrap();
    session
        .with_resident_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits()).unwrap();
            inventory
                .visit_on_control(&mut ledger, AudioStorageRole::Caller)
                .unwrap();
            ledger
                .borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Current)
                .unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap(); // failed aliases were actually retired
            Ok(())
        })
        .unwrap();
}

#[test]
fn raw_audio_original_remains_unknown_at_the_actual_session_plan_boundary() {
    let audio = Audio::new(48_000, vec![[0.25, -0.5]; 4]).unwrap();
    let pointer = audio.frames().as_ptr();
    let member = PreparedImport {
        path: "raw.wav".into(),
        source_sample_rate: 48_000,
        source_frames: 4,
        conversion_version: hex_project::IDENTITY_CONVERSION,
        source_audio: OriginalAudio::Stereo(audio.clone()),
        waveform: hex_media::waveform::summarize(&audio, 4).unwrap(),
        audio,
    };
    let mut session = Session::new(48_000, 16, 4).unwrap();
    let (hex_project::Created::Imported { asset, .. }, revision) =
        session.import_prepared_revision(member, None).unwrap()
    else {
        panic!("import")
    };
    let before = session.project().clone();
    assert!(matches!(
        session.prepare_resident_resource_bank_on_control(
            &[asset],
            limits(),
            &AtomicBool::new(false)
        ),
        Err(ResourcePreparationError::Storage(
            AudioStorageError::UnknownStorage
        ))
    ));
    assert_eq!(session.project(), &before);
    assert_eq!(
        session.source_audio(asset).unwrap().frames().as_ptr(),
        pointer
    );
    assert!(
        ResidentPcm::Stereo(session.audio(asset).unwrap())
            .shares(session.media.get(&asset).unwrap().source_audio.pcm())
    );
    drop(revision);
}

#[test]
fn actual_processed_clip_cache_joins_original_and_candidate_pcm_without_source_relabelling() {
    let Some((mut session, ids)) = fixture() else {
        return;
    };
    let clip = session.project().tracks()[0].clips()[0].id();
    let source = session.audio(ids[0]).unwrap().frames().to_vec();
    let (_, revision) = session
        .command_prepared_revision(
            hex_project::Command::SetAudioClipSourceGain {
                clip,
                points: vec![
                    hex_project::SourceGainPoint {
                        frame: 0,
                        gain_q16: 32768,
                    },
                    hex_project::SourceGainPoint {
                        frame: source.len() as u64,
                        gain_q16: 32768,
                    },
                ],
            },
            None,
        )
        .unwrap();
    let view = revision
        .arrangement()
        .audio_clip_views()
        .iter()
        .find(|view| view.clip == clip)
        .unwrap();
    for (actual, expected) in view.audio.frames().iter().zip(&source) {
        assert_eq!(*actual, [expected[0] * 0.5, expected[1] * 0.5]);
    }
    assert!(matches!(
        OwnedAudioSource::alias_native_audio_on_control(&view.audio, 4096),
        Err(AudioStorageError::UnknownStorage)
    ));
    drop(revision); // Otherwise its actual clip/preview handles must also be selected.
    let bank = session
        .prepare_resident_resource_bank_on_control(&ids, limits(), &AtomicBool::new(false))
        .unwrap();
    session
        .with_resident_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits()).unwrap();
            ledger
                .borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Current)
                .unwrap();
            inventory
                .visit_on_control(&mut ledger, AudioStorageRole::Caller)
                .unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
            assert_eq!(ledger.selected_shared_counts_on_control().0, 6); // originals2 + project2 + processed1 + root1
            drop(ledger);

            Ok(())
        })
        .unwrap();
    let before = session.project().clone();
    assert!(session.undo());
    assert!(session.redo());
    assert_eq!(session.project(), &before);
    session
        .with_resident_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits()).unwrap();
            inventory
                .visit_on_control(&mut ledger, AudioStorageRole::Caller)
                .unwrap();
            ledger
                .borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Current)
                .unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
            Ok(())
        })
        .unwrap();
}
