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
    let (_, device) = fixture();
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
    let prepared =
        PreparedDeviceSample::decode_on_worker(device, &state, &path, limits, &cancel).unwrap();
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
