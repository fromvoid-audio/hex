use super::*;
use crate::{AudioSourceBinding, PreparedImport, ResidentPcm, original_audio::OriginalAudio};
use hex_engine::{
    Audio, AudioStorageLedgerReservation, AudioStorageReservation, OwnedAudioInputs,
    OwnedAudioSource,
    native_device_resources::{
        DeviceResourceCatalog, OriginalResourceLayout, OriginalResourceSource,
        ResourceCatalogReservation,
    },
};
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

#[test]
fn cached_sine_actual_session_prepared_reuse_and_export_fields_share_one_body() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported cached sine owner; no positive alias proof");
        return;
    }
    eprintln!("SUPPORTED actual Session cached/prepared/control-export sine fields");
    let session = Session::new(48_000, 8, 1024).unwrap();
    let prepared = session.compile_prepared_revision(None).unwrap();
    let reused = session.compile_prepared_revision(Some(&prepared)).unwrap();
    let exported = session.sine_table();
    assert!(exported.shares_owner_with(prepared.session().sine_table()));
    assert!(exported.shares_owner_with(reused.session().sine_table()));
    let collect = |budget| -> Result<_, AudioStorageError> {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
        session.visit_cached_sine_storage_on_control(&mut ledger)?;
        let one = ledger.summary()?;
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        prepared
            .session()
            .sine_table()
            .visit_storage_on_control(&mut ledger)?;
        reused
            .session()
            .sine_table()
            .visit_storage_on_control(&mut ledger)?;
        exported.visit_storage_on_control(&mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        let joined = ledger.summary()?;
        assert_eq!(joined.data_bytes, one.data_bytes);
        assert_eq!(joined.data_allocations, 1);
        session.visit_cached_sine_storage_on_control(&mut ledger)?;
        assert_eq!(ledger.summary()?, joined);
        Ok(joined)
    };
    let summary = collect(ledger_limits()).unwrap();
    let mut exact = ledger_limits();
    exact.max_retained_bytes = summary.retained_bytes;
    exact.max_construction_peak_bytes = summary.construction_peak_bytes;
    assert_eq!(collect(exact).unwrap(), summary);
    exact.max_retained_bytes -= 1;
    assert!(collect(exact).is_err());
}

#[test]
fn actual_worker_history_media_and_exported_aliases_join_one_scoped_ledger() {
    if !hex_engine::audio_storage_constructor_layout_supported()
        || !hex_engine::canonical_native_state_storage_layout_supported()
    {
        eprintln!("unsupported worker model/media constructors; no positive union proof");
        return;
    }
    eprintln!("SUPPORTED actual worker current Undo Redo and Mono/Stereo media/export union");
    for stereo in [false, true] {
        let mut session = Session::new(48_000, 8, 1024).unwrap();
        let hex_project::Created::Track(track) = session
            .editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "sampler".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let device = session
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        session
            .editor
            .execute(hex_project::Command::ReplaceDeviceState {
                device,
                state: hex_engine::empty_sampler_state_on_control().unwrap(),
            })
            .unwrap();
        let source = if stereo {
            OwnedAudioSource::try_resident(48_000, vec![[0.25, -0.5]; 4])
                .ok()
                .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(48_000, vec![0.25; 4])
                .ok()
                .unwrap()
        };
        let prepared = PreparedImport::from_native_source(
            "worker-original.wav".into(),
            source,
            48_000,
            hex_media::ResampleLimits {
                max_source_frames: 4,
                max_output_frames: 4,
            },
        )
        .unwrap();
        let hex_project::Created::Asset(asset) = session
            .editor
            .execute(hex_project::Command::AddAsset {
                path: "worker-original.wav".into(),
                sample_rate: 48_000,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        session
            .media
            .try_insert_on_control(
                asset,
                crate::ResidentAsset {
                    source_audio: prepared.source_audio,
                    audio: Some(prepared.audio),
                    waveform: crate::SharedWaveform::new_on_control(prepared.waveform),
                    frames: 4,
                },
                hex_project::MAX_ASSETS,
            )
            .unwrap();
        // Undo removes the asset only from current. Actual Redo and retained
        // media still own real storage; neither can disappear from the union.
        assert!(session.editor.undo());
        assert!(session.project().asset(asset).is_none());
        assert!(session.editor.can_undo() && session.editor.can_redo());
        let waveforms = session.waveforms();
        let captured = crate::SharedProject::new_on_control(session.project().clone());
        let run = |budget| {
            session.with_resident_pcm_inventory_on_control(|inventory| {
                let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
                inventory.visit_worker_model_media_storage_on_control(&mut ledger)?;
                let selected = ledger.summary()?;
                assert!(
                    ledger
                        .validate_selected_shared_handles_on_control()
                        .is_err(),
                    "real exported model/waveform fields cannot be omitted"
                );
                crate::visit_shared_project_selected_storage_on_control(&captured, &mut ledger)?;
                ledger.borrow_metadata_entries_backing_on_control(&waveforms)?;
                for waveform in waveforms.values() {
                    hex_media::waveform::borrow_shared_storage_on_control(waveform, &mut ledger)?;
                }
                ledger.validate_selected_shared_handles_on_control()?;
                let joined = ledger.summary()?;
                assert!(
                    joined.data_bytes > selected.data_bytes,
                    "independent exported model/container backing is real, not content-deduped"
                );
                inventory.visit_worker_model_media_storage_on_control(&mut ledger)?;
                assert_eq!(ledger.summary()?, joined, "same actual fields deduplicate");
                assert!(
                    joined.require_complete().is_err(),
                    "enclosing/native/other worker roots remain open"
                );
                Ok(joined)
            })
        };
        let mut budget = ledger_limits();
        budget.data_rows = 512;
        budget.handle_rows = 512;
        let summary = run(budget).unwrap();
        budget.max_retained_bytes = summary.retained_bytes;
        budget.max_construction_peak_bytes = summary.construction_peak_bytes;
        assert_eq!(run(budget).unwrap(), summary);
        budget.max_retained_bytes -= 1;
        assert!(run(budget).is_err());
        assert_eq!(session.project(), captured.as_ref());
        assert!(session.editor.can_undo() && session.editor.can_redo());
    }
}

#[test]
fn actual_session_bank_and_exported_maps_keep_independent_backings_and_shared_children() {
    let Some((session, ids)) = fixture() else {
        return;
    };
    let bank = session.media_bank().unwrap();
    let second_bank = session.media_bank().unwrap();
    let bindings = session.source_bindings();
    let waveforms = session.waveforms();
    let second_waveforms = session.waveforms();
    assert_eq!(bank.resident.len(), ids.len());
    assert_eq!(bindings.len(), ids.len());
    assert_eq!(waveforms.len(), ids.len());
    assert!(!std::ptr::eq(
        bank.resident.as_slice().as_ptr(),
        second_bank.resident.as_slice().as_ptr()
    ));
    assert!(!std::ptr::eq(
        waveforms.as_slice().as_ptr(),
        second_waveforms.as_slice().as_ptr()
    ));
    assert_eq!(
        bank.resident.backing_bytes().unwrap(),
        bank.resident.capacity() * std::mem::size_of::<(AssetId, Audio)>()
    );
    for id in ids {
        let original = session.audio(id).unwrap();
        let copied = bank.resident.get(&id).unwrap();
        let AudioSourceBinding::Resident(binding) = bindings.get(&id).unwrap() else {
            panic!("resident binding")
        };
        // Pointer equality here tests sharing only; it grants no provenance or admission.
        assert!(std::ptr::eq(
            original.frames().as_ptr(),
            copied.frames().as_ptr()
        ));
        assert!(std::ptr::eq(
            original.frames().as_ptr(),
            binding.frames().as_ptr()
        ));
        assert_eq!(copied.frames(), original.frames());
        assert!(
            (waveforms.get(&id).unwrap())
                .shares_owner_with(&session.media.get(&id).unwrap().waveform)
        );
        assert!(
            (waveforms.get(&id).unwrap()).shares_owner_with(second_waveforms.get(&id).unwrap())
        );
    }
    // These actual copies survive the producing Session; they are distinct live
    // container obligations, even though the PCM/summary children are shared.
    drop(session);
    assert!(
        bank.resident
            .values()
            .all(|audio| audio.frames().len() == 4)
    );
    assert_eq!(waveforms.len(), 2);
    assert_eq!(second_bank.resident.len(), 2);
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
fn device_only_mono_residency_keeps_original_and_waveform_without_stereo_derivative() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let cancel = AtomicBool::new(false);
    let mut session = Session::new(48_000, 8, 1024).unwrap();
    let hex_project::Created::Asset(asset) = session
        .editor
        .execute(hex_project::Command::AddAsset {
            path: "device-original.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let source = OwnedAudioSource::try_resident_mono(48_000, vec![0.25, -0.5, 0.75, -1.0])
        .ok()
        .unwrap();
    let alias = source
        .alias_for_resource_plan_on_control(std::mem::size_of::<OwnedAudioSource>())
        .unwrap();
    let inputs = OwnedAudioInputs::try_new(vec![(asset, alias)], limits().inputs)
        .ok()
        .unwrap()
        .prepare_input_storage_on_control()
        .ok()
        .unwrap();
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &inputs,
        &[OriginalResourceSource {
            asset,
            sample_rate: 48_000,
            frames: 4,
            layout: OriginalResourceLayout::Mono,
        }],
        limits().catalog,
        &cancel,
    )
    .unwrap();
    let backing =
        DeviceResourceBank::try_new_on_control(inputs, catalog, 65_536, 1_000_000, &cancel)
            .ok()
            .unwrap();
    let waveform = hex_media::waveform::summarize_original(
        &backing
            .borrow_original_source_on_control(asset, 4096)
            .unwrap(),
        4,
        4,
        4096,
        &cancel,
    )
    .unwrap();
    assert_eq!(waveform.source_frames, 4);
    let bins = waveform.cached_bin_count();
    let original_capacity = source.resident_pcm_capacity_frames().unwrap();
    session
        .media
        .try_insert_on_control(
            asset,
            crate::ResidentAsset {
                source_audio: OriginalAudio::Mono(source),
                audio: None,
                waveform: crate::SharedWaveform::new_on_control(waveform),
                frames: 4,
            },
            hex_project::MAX_ASSETS,
        )
        .unwrap();
    session.resident_frames = original_capacity as u64;
    session.resident_waveform_bins = bins;
    session.resident_waveform_bytes = bins * std::mem::size_of::<[hex_media::waveform::Peak; 2]>();
    drop(backing); // The accepted immutable waveform retains this same source root.
    assert!(session.audio(asset).is_none());
    assert!(session.media_bank().unwrap().resident.is_empty());
    assert_eq!(session.media.get(&asset).unwrap().pcm().count(), 1);
    assert_eq!(session.resident_frames(), original_capacity as u64);
    assert_eq!(
        session
            .source_mono(asset)
            .unwrap()
            .resident_mono_samples()
            .unwrap(),
        &[0.25, -0.5, 0.75, -1.0]
    );
    let bank = session
        .prepare_resident_resource_bank_on_control(&[asset], limits(), &cancel)
        .unwrap();
    session
        .with_resident_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits())?;
            ledger.borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Caller)?;
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            inventory.visit_on_control(&mut ledger, AudioStorageRole::Current)?;
            ledger.validate_selected_shared_handles_on_control()?;
            assert!(ledger.summary()?.require_complete().is_err());
            Ok(())
        })
        .unwrap();
    let retained = bank.borrow_original_source_on_control(asset, 4096).unwrap();
    drop(session);
    drop(bank);
    assert_eq!(
        retained.view().unwrap().resident_mono_samples().unwrap()[3],
        -1.0
    );
}

#[test]
fn pending_decoded_original_joins_existing_bank_without_publishing_or_omitting_temporary_owner() {
    let Some((session, mut ids)) = fixture() else {
        return;
    };
    let before = session.project().clone();
    let resident_frames = session.resident_frames();
    let mut candidate = hex_project::Editor::new(before.clone(), 8).unwrap();
    let hex_project::Created::Asset(asset) = candidate
        .execute(hex_project::Command::AddAsset {
            path: "pending-device.wav".into(),
            sample_rate: 48_000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let metadata = candidate.project().asset(asset).unwrap();
    let pending = OriginalAudio::Mono(
        OwnedAudioSource::try_resident_mono(48_000, vec![0.125, -0.25, 0.5, -1.0])
            .ok()
            .unwrap(),
    );
    let cancel = AtomicBool::new(false);
    // No ignored pending buffer, shadowed current asset, or mismatched metadata.
    assert!(
        session
            .prepare_resident_resource_bank_with_pending_on_control(
                &ids,
                Some((metadata, &pending)),
                limits(),
                &cancel,
            )
            .is_err()
    );
    let wrong_rate = OriginalAudio::Mono(
        OwnedAudioSource::try_resident_mono(44_100, vec![0.5; 4])
            .ok()
            .unwrap(),
    );
    ids.push(asset);
    assert!(
        session
            .prepare_resident_resource_bank_with_pending_on_control(
                &ids,
                Some((metadata, &wrong_rate)),
                limits(),
                &cancel,
            )
            .is_err()
    );
    let existing = session.project().asset(ids[0]).unwrap();
    assert!(
        session
            .prepare_resident_resource_bank_with_pending_on_control(
                &ids,
                Some((existing, &wrong_rate)),
                limits(),
                &cancel,
            )
            .is_err()
    );
    drop(wrong_rate);
    let mut reservation = limits();
    reservation.catalog.hash_frames = 12;
    let bank = session
        .prepare_resident_resource_bank_with_pending_on_control(
            &ids,
            Some((metadata, &pending)),
            reservation,
            &cancel,
        )
        .unwrap();
    assert_eq!(*session.project(), before);
    assert_eq!(session.resident_frames(), resident_frames);
    assert!(!session.media.contains_key(&asset));
    session
        .with_resident_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits())?;
            ledger.borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Caller)?;
            inventory.visit_on_control(&mut ledger, AudioStorageRole::Current)?;
            // A successful bank/catalog is insufficient: the still-live decoded
            // temporary is an actual physical owner until commit or cancellation.
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            let OriginalAudio::Mono(source) = &pending else {
                panic!("mono")
            };
            ledger.borrow_native_source_on_control(source, AudioStorageRole::Caller)?;
            ledger.validate_selected_shared_handles_on_control()?;
            assert!(ledger.summary()?.require_complete().is_err());
            Ok(())
        })
        .unwrap();
    let retained = bank.borrow_original_source_on_control(asset, 4096).unwrap();
    drop(candidate);
    drop(session);
    drop(pending);
    drop(bank);
    assert_eq!(
        retained.view().unwrap().resident_mono_samples().unwrap(),
        &[0.125, -0.25, 0.5, -1.0]
    );
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

#[test]
fn retained_cancellation_direct_session_constructor_visits_actual_session_and_transform_fields() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    eprintln!(
        "SUPPORTED direct retained Session cancellation construction and actual transform field"
    );
    let signal = hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
    let job = hex_engine::RetainedControlJob::try_from_owned_on_control(
        &mut Some(0_u8),
        usize::MAX,
        |_: &mut u8| Ok::<_, ()>(hex_engine::RetainedControlJobProgress::Pending),
    )
    .unwrap();
    let route = job.route();
    let session = Session::new_with_retained_cancellation_on_control(
        48_000,
        8,
        1024,
        signal.bind_worker_on_control(route),
    )
    .unwrap();
    assert_eq!(session.project().sample_rate(), 48_000);
    let collect = |limits| -> Result<_, AudioStorageError> {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits)?;
        session.visit_cancellation_storage_on_control(&mut ledger)?;
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        signal.visit_storage_on_control(&mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        assert_eq!(ledger.selected_shared_counts_on_control(), (1, 3));
        let measured = ledger.summary()?;
        assert_eq!(measured.data_allocations, 1);
        session.visit_cancellation_storage_on_control(&mut ledger)?;
        assert_eq!(ledger.summary()?, measured);
        Ok(measured)
    };
    let summary = collect(ledger_limits()).unwrap();
    let mut exact = ledger_limits();
    exact.max_retained_bytes = summary.retained_bytes;
    exact.max_construction_peak_bytes = summary.construction_peak_bytes;
    assert_eq!(collect(exact).unwrap(), summary);
    exact.max_retained_bytes -= 1;
    assert!(collect(exact).is_err());
    use hex_engine::CancellationQuery;
    assert!(!session.shutdown.is_cancelled());
    signal.cancel(route);
    assert!(session.shutdown.is_cancelled());
    let raw = Session::new(48_000, 8, 1024).unwrap();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits()).unwrap();
    assert_eq!(
        raw.visit_cancellation_storage_on_control(&mut ledger),
        Err(AudioStorageError::UnknownStorage)
    );
}

#[test]
fn original_only_worker_children_join_real_sampler_history_empty_cache_and_sine() {
    if !hex_engine::audio_storage_constructor_layout_supported()
        || !hex_engine::canonical_native_state_storage_layout_supported()
        || !hex_engine::canonical_midi_storage_layout_supported()
    {
        return;
    }
    eprintln!(
        "SUPPORTED actual empty Sampler Session/history, reserved transform backing, cached sine and paired cancellation union"
    );
    let signal = hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
    let job = hex_engine::RetainedControlJob::try_from_owned_on_control(
        &mut Some(0_u8),
        usize::MAX,
        |_: &mut u8| Ok::<_, ()>(hex_engine::RetainedControlJobProgress::Pending),
    )
    .unwrap();
    let mut session = Session::new_with_retained_cancellation_on_control(
        48_000,
        8,
        1024,
        signal.bind_worker_on_control(job.route()),
    )
    .unwrap();
    let hex_project::Created::Track(track) = session
        .editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    session
        .editor
        .execute(hex_project::Command::ReplaceDeviceState {
            device,
            state: hex_engine::empty_sampler_state_on_control().unwrap(),
        })
        .unwrap();
    // Actual compiler reserves empty transform backing too. Its temporary
    // prepared owners retire before this selected worker-only union.
    drop(session.compile_prepared_revision(None).unwrap());
    let sine_alias = session.sine_table();
    let collect = |reservation| {
        session.with_resident_pcm_inventory_on_control(|inventory| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
            inventory.visit_original_only_worker_children_on_control(&mut ledger)?;
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            signal.visit_storage_on_control(&mut ledger)?;
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            sine_alias.visit_storage_on_control(&mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            let measured = ledger.summary()?;
            inventory.visit_original_only_worker_children_on_control(&mut ledger)?;
            assert_eq!(ledger.summary()?, measured);
            assert!(session.audio_clip_waveforms.try_borrow_mut().is_err());
            Ok(measured)
        })
    };
    let measured = collect(ledger_limits()).unwrap();
    let mut exact = ledger_limits();
    exact.max_retained_bytes = measured.retained_bytes;
    exact.max_construction_peak_bytes = measured.construction_peak_bytes;
    assert_eq!(collect(exact).unwrap(), measured);
    exact.max_retained_bytes -= 1;
    assert!(collect(exact).is_err());
    let raw = Session::new(48_000, 8, 1024).unwrap();
    raw.with_resident_pcm_inventory_on_control(|inventory| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(ledger_limits())?;
        assert_eq!(
            inventory.visit_original_only_worker_children_on_control(&mut ledger),
            Err(AudioStorageError::UnknownStorage)
        );
        Ok(())
    })
    .unwrap();
}
