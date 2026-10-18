use super::*;
use hex_engine::AudioStorageReservation;
use hex_engine::native_device_resources::ResourceCatalogReservation;
use hex_project::{Created, Editor};

fn fixture() -> (Session, DeviceId) {
    let mut session = Session::new(48_000, 8, 1024).unwrap();
    let Created::Track(track) = session
        .editor
        .execute(Command::AddMidiTrack {
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
        .execute(Command::ReplaceDeviceState {
            device,
            state: hex_engine::empty_sampler_state_on_control().unwrap(),
        })
        .unwrap();
    (session, device)
}
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

#[test]
fn actual_worker_source_binds_candidate_without_publishing_and_survives_staging_retirement() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    for stereo in [false, true] {
        let (session, device) = fixture();
        let original = session.project().clone();
        let source = if stereo {
            OwnedAudioSource::try_resident(24_000, vec![[0.25, -0.5]; 4])
                .ok()
                .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(24_000, vec![0.25; 4])
                .ok()
                .unwrap()
        };
        let prepared = PreparedDeviceSample::from_native_on_control(
            device,
            session.project().device(device).unwrap().state(),
            "sample.wav".into(),
            source,
        )
        .unwrap();
        assert_eq!(prepared.source_sample_rate(), 24_000);
        assert_eq!(prepared.source_frames(), 4);
        let mut candidate = Editor::new(original.clone(), 8).unwrap();
        let Created::Asset(asset) = candidate
            .execute(prepared.assignment_command_on_control())
            .unwrap()
        else {
            panic!("asset")
        };
        assert_eq!(
            candidate.project().asset(asset).unwrap().playback_frames(),
            8
        );
        let cancel = AtomicBool::new(false);
        assert!(
            prepared
                .prepare_resource_bank_for_candidate_on_control(
                    &session,
                    candidate.project(),
                    &[],
                    limits(),
                    &cancel,
                )
                .is_err()
        );
        let bank = prepared
            .prepare_resource_bank_for_candidate_on_control(
                &session,
                candidate.project(),
                &[asset],
                limits(),
                &cancel,
            )
            .unwrap();
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(hex_engine::AudioStorageLedgerReservation {
                data_rows: 64,
                handle_rows: 64,
                root_rows: 16,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            })
            .unwrap();
        ledger
            .borrow_device_resource_bank_on_control(&bank, AudioStorageRole::Caller)
            .unwrap();
        // Staging is a real field, not merely an asset identity.
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        prepared
            .visit_original_pcm_on_control(&mut ledger, AudioStorageRole::Caller)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert!(ledger.summary().unwrap().require_complete().is_err());
        // A newly retained lease is another actual owner, even when its PCM
        // is already present. Selected closure must notice the omitted field.
        let retained = bank.borrow_original_source_on_control(asset, 4096).unwrap();
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        drop(ledger);
        drop(retained);
        let retained = bank.borrow_original_source_on_control(asset, 4096).unwrap();
        assert_eq!(*session.project(), original);
        assert_eq!(session.resident_frames(), 0);
        assert!(!session.media.contains_key(&asset));
        cancel.store(true, Ordering::Relaxed);
        assert!(
            prepared
                .prepare_resource_bank_for_candidate_on_control(
                    &session,
                    candidate.project(),
                    &[asset],
                    limits(),
                    &cancel,
                )
                .is_err()
        );
        drop(prepared);
        drop(bank);
        drop(candidate);
        drop(session);
        let view = retained.view().unwrap();
        if stereo {
            assert_eq!(view.resident_frames().unwrap(), &[[0.25, -0.5]; 4]);
        } else {
            assert_eq!(view.resident_mono_samples().unwrap(), &[0.25; 4]);
        }
    }
}

#[test]
fn bounded_file_worker_preserves_original_rate_and_reports_cancellation_and_missing_source() {
    let (session, device) = fixture();
    let state = hex_engine::empty_sampler_state_on_control().unwrap();
    let path = std::env::temp_dir().join(format!("hex-sampler-worker-{}.wav", std::process::id()));
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&44_u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&24_000_u32.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&8_u32.to_le_bytes());
    for sample in [8192_i16, -16384, 0, 32767] {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(&path, wav).unwrap();
    let cancel = AtomicBool::new(false);
    let limits = hex_media::Limits::new(1024, 8);
    let prepared = session
        .prepare_device_sample_on_worker(device, &state, &path, limits, &cancel)
        .unwrap();
    assert_eq!(prepared.path(), path);
    assert_eq!(
        (prepared.source_sample_rate(), prepared.source_frames()),
        (24_000, 4)
    );
    match &prepared.source {
        OriginalAudio::Mono(source) => assert_eq!(
            source.view().resident_mono_samples().unwrap(),
            &[0.25, -0.5, 0.0, 32767.0 / 32768.0]
        ),
        OriginalAudio::Stereo(source) => assert_eq!(source.frames()[0], [0.25, 0.25]),
    }
    assert!(
        PreparedDeviceSample::decode_on_worker(
            device,
            &state,
            &path,
            hex_media::Limits::new(1024, 3),
            &cancel
        )
        .is_err()
    );
    cancel.store(true, Ordering::Relaxed);
    assert!(
        PreparedDeviceSample::decode_on_worker(device, &state, &path, limits, &cancel).is_err()
    );
    std::fs::remove_file(&path).unwrap();
    cancel.store(false, Ordering::Relaxed);
    assert!(
        PreparedDeviceSample::decode_on_worker(device, &state, &path, limits, &cancel).is_err()
    );
}

#[test]
fn bounded_file_worker_target_is_checked_before_missing_file_io_and_never_edits_history() {
    let (mut session, device) = fixture();
    let captured = session.project().device(device).unwrap().state().clone();
    let original = session.project().clone();
    let missing =
        std::env::temp_dir().join(format!("hex-sampler-absent-{}.wav", std::process::id()));
    let cancel = AtomicBool::new(false);
    let limits = hex_media::Limits::new(1024, 8);
    // A genuine target reaches I/O and reports missing media, not stale target.
    assert!(matches!(
        session.prepare_device_sample_on_worker(device, &captured, &missing, limits, &cancel),
        Err(SessionError::Media(_))
    ));
    assert_eq!(*session.project(), original);
    let replaced = NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("test.other.instrument"),
        1,
        vec![],
    )
    .unwrap();
    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: replaced.clone(),
        })
        .unwrap();
    let before = session.project().clone();
    let undo = session.editor.can_undo();
    let redo = session.editor.can_redo();
    assert!(matches!(
        session.prepare_device_sample_on_worker(device, &captured, &missing, limits, &cancel),
        Err(SessionError::Invalid(
            "sample loading canceled · target instrument changed"
        ))
    ));
    // Matching identity with the wrong actual instrument type also refuses
    // before file access; an arbitrary state is never treated as a Sampler.
    assert!(matches!(
        session.prepare_device_sample_on_worker(device, &replaced, &missing, limits, &cancel),
        Err(SessionError::Engine(_))
    ));
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(
        session.prepare_device_sample_on_worker(device, &captured, &missing, limits, &cancel),
        Err(SessionError::Media(hex_media::ImportError::Cancelled))
    ));
    assert_eq!(*session.project(), before);
    assert_eq!(
        (session.editor.can_undo(), session.editor.can_redo()),
        (undo, redo)
    );
    assert_eq!(session.resident_frames(), 0);
}

#[test]
fn stale_target_and_wrong_candidate_are_refused_before_publication() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (mut session, device) = fixture();
    let prepared = PreparedDeviceSample::from_native_on_control(
        device,
        session.project().device(device).unwrap().state(),
        "sample.wav".into(),
        OwnedAudioSource::try_resident_mono(48_000, vec![0.5; 4])
            .ok()
            .unwrap(),
    )
    .unwrap();
    assert!(
        prepared
            .prepare_resource_bank_for_candidate_on_control(
                &session,
                session.project(),
                &[],
                limits(),
                &AtomicBool::new(false),
            )
            .is_err()
    );
    let changed = session
        .project()
        .device(device)
        .unwrap()
        .state()
        .clone()
        .with_output_gain_millidb(-600)
        .unwrap();
    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: changed,
        })
        .unwrap();
    assert!(
        prepared
            .validate_target_on_control(session.project())
            .is_err()
    );
    assert!(
        session
            .editor
            .execute(prepared.assignment_command_on_control())
            .is_err()
    );
    assert_eq!(session.project().assets().count(), 0);
}

#[test]
fn real_worker_waveform_reuses_original_root_and_keeps_deep_zoom_after_control_retirement() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    for stereo in [false, true] {
        let (session, device) = fixture();
        let source = if stereo {
            OwnedAudioSource::try_resident(
                48_000,
                vec![[0.25, -0.75], [-0.5, 1.0], [0.75, -0.25], [-1.0, 0.5]],
            )
            .ok()
            .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(48_000, vec![0.25, -0.5, 0.75, -1.0])
                .ok()
                .unwrap()
        };
        let prepared = PreparedDeviceSample::from_native_on_control(
            device,
            session.project().device(device).unwrap().state(),
            "sample.wav".into(),
            source,
        )
        .unwrap();
        let mut candidate = Editor::new(session.project().clone(), 8).unwrap();
        let Created::Asset(asset) = candidate
            .execute(prepared.assignment_command_on_control())
            .unwrap()
        else {
            panic!("asset")
        };
        let cancel = AtomicBool::new(false);
        let waveform_limits = DeviceSampleWaveformLimits {
            bins: 1,
            scan_frames: 4,
            source_metadata_bytes: 4096,
        };
        assert!(
            prepared
                .prepare_resources_and_waveform_on_worker(
                    &session,
                    candidate.project(),
                    &[asset],
                    limits(),
                    DeviceSampleWaveformLimits {
                        scan_frames: 3,
                        ..waveform_limits
                    },
                    &cancel
                )
                .is_err()
        );
        let (bank, waveform) = prepared
            .prepare_resources_and_waveform_on_worker(
                &session,
                candidate.project(),
                &[asset],
                limits(),
                waveform_limits,
                &cancel,
            )
            .unwrap();
        let lease = bank.borrow_original_source_on_control(asset, 4096).unwrap();
        assert!(waveform.shares_original_with(&lease));
        assert!(waveform.audio_source().is_none());
        assert_eq!(waveform.source_frames, 4);
        // One cached bin cannot explain this one-frame extrema; it must use
        // the actual retained original source, not fabricated/stretched peaks.
        drop(lease);
        drop(bank);
        drop(prepared);
        drop(candidate);
        drop(session);
        let peaks = waveform.peaks_for_frames(1, 2).unwrap();
        assert_eq!((peaks[0].min, peaks[0].max), (-0.5, -0.5));
        let right = if stereo { 1.0 } else { -0.5 };
        assert_eq!((peaks[1].min, peaks[1].max), (right, right));
    }
}

#[test]
fn pending_device_sample_inventory_joins_actual_history_command_states_path_and_original_pcm() {
    if !hex_engine::audio_storage_constructor_layout_supported()
        || !hex_engine::canonical_native_state_storage_layout_supported()
    {
        eprintln!("unsupported pending sample storage constructors; no positive proof");
        return;
    }
    eprintln!(
        "SUPPORTED actual pending device decode, assignment command, model/history and original PCM inventory"
    );
    for stereo in [false, true] {
        let (session, device) = fixture();
        let source = if stereo {
            OwnedAudioSource::try_resident(24_000, vec![[0.25, -0.5]; 4])
                .ok()
                .unwrap()
        } else {
            OwnedAudioSource::try_resident_mono(24_000, vec![0.25; 4])
                .ok()
                .unwrap()
        };
        let mut path = PathBuf::with_capacity(311);
        path.push("pending-original.wav");
        let pending = PreparedDeviceSample::from_native_on_control(
            device,
            session.project().device(device).unwrap().state(),
            path,
            source,
        )
        .unwrap();
        let command = pending.assignment_command_on_control();
        let Command::ImportDeviceAsset {
            expected_state,
            state,
            path,
            ..
        } = &command
        else {
            panic!("assignment")
        };
        assert_eq!(expected_state, &pending.expected_state);
        assert_eq!(state, &pending.assignment.state);
        let budget = hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 512,
            root_rows: 16,
            max_retained_bytes: 4_000_000,
            max_construction_peak_bytes: 4_000_000,
        };
        let collect = |budget| -> Result<_, AudioStorageError> {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
            pending.visit_original_pcm_on_control(&mut ledger, AudioStorageRole::Caller)?;
            let pcm = ledger.summary()?.data_bytes;
            pending.visit_selected_storage_on_control(&mut ledger, AudioStorageRole::Caller)?;
            let staging = ledger.summary()?.data_bytes;
            let requested = |length| {
                std::alloc::Layout::array::<std::sync::atomic::AtomicUsize>(2)
                    .unwrap()
                    .extend(std::alloc::Layout::array::<u8>(length).unwrap())
                    .unwrap()
                    .0
                    .pad_to_align()
                    .size()
            };
            assert_eq!(
                staging - pcm,
                pending.path.capacity()
                    + requested(pending.expected_state.payload().len())
                    + requested(pending.assignment.state.payload().len())
            );
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err(),
                "actual model/history and pending command fields have not joined yet"
            );
            ledger.borrow_editor_native_states_on_control(&session.editor)?;
            ledger.borrow_canonical_native_state_on_control(expected_state)?;
            ledger.borrow_canonical_native_state_on_control(state)?;
            ledger.borrow_metadata_path_backing_on_control(path)?;
            ledger.validate_selected_shared_handles_on_control()?;
            let measured = ledger.summary()?;
            pending.visit_selected_storage_on_control(&mut ledger, AudioStorageRole::Caller)?;
            ledger.borrow_editor_native_states_on_control(&session.editor)?;
            assert_eq!(ledger.summary()?, measured);
            assert!(measured.require_complete().is_err());
            Ok(measured)
        };
        let measured = collect(budget).unwrap();
        let mut exact = budget;
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        assert_eq!(pending.source_frames(), 4);
        assert_eq!(pending.source_sample_rate(), 24_000);
        assert_eq!(session.project().assets().count(), 0);
        assert!(
            session
                .project()
                .device(device)
                .unwrap()
                .state()
                .asset_references()
                .is_empty()
        );
    }
}
