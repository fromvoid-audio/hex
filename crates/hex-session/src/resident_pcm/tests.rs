use crate::*;
use hex_media::{Limits, decode_wav_file};
use std::{
    fs,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

fn decoded(rate: u32, frames: u32) -> DecodedAudio {
    decoded_channels(rate, frames, 1)
}
fn decoded_channels(rate: u32, frames: u32, channels: u16) -> DecodedAudio {
    let path = std::env::temp_dir().join(format!(
        "hex-original-pcm-{}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let align = channels * 2;
    let bytes = frames * u32::from(align);
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
    wav.extend_from_slice(&align.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for index in 0..frames {
        wav.extend_from_slice(&(1000 + index as i16 * 113).to_le_bytes());
        if channels == 2 {
            wav.extend_from_slice(&(-2000 - index as i16 * 71).to_le_bytes());
        }
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(&wav).unwrap();
    drop(file);
    let decoded = decode_wav_file(&path, Limits::new(8192, 1024)).unwrap();
    fs::remove_file(path).unwrap();
    decoded
}
fn prepared(decoded: DecodedAudio) -> PreparedImport {
    PreparedImport::at_rate(
        decoded,
        16_000,
        ResampleLimits {
            max_source_frames: 1024,
            max_output_frames: 1024,
        },
    )
    .unwrap()
}
fn asset(created: Created) -> AssetId {
    let Created::Imported { asset, .. } = created else {
        panic!("import")
    };
    asset
}

#[test]
fn mixed_rate_source_survives_derivation_history_and_open_with_exact_budget() {
    let decoded = decoded(32_000, 12);
    let original = decoded.audio().clone();
    let expected = resample(
        &original,
        16_000,
        ResampleLimits {
            max_source_frames: 1024,
            max_output_frames: 1024,
        },
    )
    .unwrap();
    let mut session = Session::new(16_000, 16, 18).unwrap();
    let id = asset(
        session
            .import_prepared_revision(prepared(decoded.clone()), None)
            .unwrap()
            .0,
    );
    let source = session.source_audio(id).unwrap();
    assert_eq!(source.sample_rate(), 32_000);
    assert_eq!(source.frames(), original.frames());
    assert!(source.shares_pcm_with(&original));
    assert!(!source.shares_pcm_with(session.audio(id).unwrap()));
    assert_eq!(session.audio(id).unwrap().frames(), expected.frames());
    assert_eq!(session.resident_frames(), 18);
    let saved = session.project().clone();
    assert!(session.undo());
    assert!(session.source_audio(id).unwrap().shares_pcm_with(&original));
    assert_eq!(session.resident_frames(), 18);
    assert!(session.redo());
    assert_eq!(session.project(), &saved);
    assert!(Session::from_loaded(saved.clone(), 16, 17, vec![(id, decoded.clone())]).is_err());
    let reopened = Session::from_loaded(saved, 16, 18, vec![(id, decoded)]).unwrap();
    assert_eq!(reopened.resident_frames(), 18);
    assert_eq!(
        reopened.source_audio(id).unwrap().frames(),
        original.frames()
    );
    assert_eq!(reopened.audio(id).unwrap().frames(), expected.frames());
}

#[test]
fn identity_aliases_share_one_buffer_across_batch_and_loaded_assets() {
    let decoded = decoded(16_000, 12);
    let original = decoded.audio().clone();
    let mut session = Session::new(16_000, 16, 12).unwrap();
    let before = session.project().clone();
    let created = session
        .import_prepared_batch_revision(
            vec![prepared(decoded.clone()), prepared(decoded.clone())],
            import_batch::AudioBatchDestination::NewTracks {
                before_track: None,
                start: 0,
            },
            &before,
            None,
            || Ok(()),
        )
        .unwrap()
        .0;
    let ids: Vec<_> = created.into_iter().map(asset).collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert_eq!(session.resident_frames(), 12);
    for id in &ids {
        assert!(
            session
                .source_audio(*id)
                .unwrap()
                .shares_pcm_with(&original)
        );
        assert!(session.audio(*id).unwrap().shares_pcm_with(&original));
    }
    let reopened = Session::from_loaded(
        session.project().clone(),
        16,
        12,
        ids.iter().map(|id| (*id, decoded.clone())).collect(),
    )
    .unwrap();
    assert_eq!(reopened.resident_frames(), 12);
}

#[test]
fn capacity_is_charged_and_failed_mixed_import_preserves_redo_and_resident_sources() {
    let mut values = Vec::with_capacity(128);
    values.resize(4, [0.25; 2]);
    let audio = Audio::new(16_000, values).unwrap();
    assert_eq!(audio.pcm_capacity_frames(), 128);
    let oversized = PreparedImport {
        path: "capacity.wav".into(),
        source_sample_rate: 16_000,
        source_frames: 4,
        conversion_version: hex_project::IDENTITY_CONVERSION,
        source_audio: audio.clone(),
        waveform: waveform::summarize(&audio, 1024).unwrap(),
        audio,
    };
    let mut small = Session::new(16_000, 16, 127).unwrap();
    let before = small.project().clone();
    assert!(small.import_prepared_revision(oversized, None).is_err());
    assert_eq!(small.project(), &before);
    assert_eq!(small.resident_frames(), 0);
    let mut session = Session::new(16_000, 16, 25).unwrap();
    let id = asset(
        session
            .import_prepared_revision(prepared(decoded(16_000, 8)), None)
            .unwrap()
            .0,
    );
    let after = session.project().clone();
    let original = session.source_audio(id).unwrap().clone();
    assert!(session.undo());
    let before = session.project().clone();
    let history = (session.can_undo(), session.can_redo());
    assert!(
        session
            .import_prepared_revision(prepared(decoded(32_000, 12)), None)
            .is_err()
    );
    assert_eq!(session.project(), &before);
    assert_eq!((session.can_undo(), session.can_redo()), history);
    assert_eq!(session.resident_frames(), 8);
    assert!(session.source_audio(id).unwrap().shares_pcm_with(&original));
    assert!(session.redo());
    assert_eq!(session.project(), &after);
}

#[test]
fn batch_union_refuses_atomically_and_session_cells_retain_original_rate() {
    let mut session = Session::new(16_000, 16, 35).unwrap();
    let before = session.project().clone();
    let result = session.import_prepared_batch_revision(
        vec![prepared(decoded(32_000, 12)), prepared(decoded(32_000, 12))],
        import_batch::AudioBatchDestination::NewTracks {
            before_track: None,
            start: 0,
        },
        &before,
        None,
        || Ok(()),
    );
    assert!(result.is_err()); // Two distinct18-frame allocations exceed35.
    assert_eq!(session.project(), &before);
    assert_eq!(session.resident_frames(), 0);
    let Created::Track(track) = session
        .command(Command::AddTrack {
            name: "sample".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Scene(scene) = session
        .command(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    session
        .import_session_audio_clip_revision(prepared(decoded(32_000, 12)), track, scene, None)
        .unwrap();
    let id = session.project().assets().next().unwrap().id();
    assert_eq!(session.source_audio(id).unwrap().sample_rate(), 32_000);
    assert_eq!(session.source_audio(id).unwrap().frames().len(), 12);
    assert_eq!(session.audio(id).unwrap().sample_rate(), 16_000);
    assert_eq!(session.audio(id).unwrap().frames().len(), 6);
    assert_eq!(session.resident_frames(), 18);
    assert!(session.undo());
    assert_eq!(session.source_audio(id).unwrap().frames().len(), 12);
    assert!(session.redo());
    assert_eq!(session.resident_frames(), 18);
}

#[test]
fn actual_stereo_import_preserves_native_original_identity_derivative_and_snapshot_owners() {
    use hex_engine::native_device_resources::{
        DeviceResourceBank, DeviceResourceCatalog, OriginalResourceLayout, OriginalResourceSource,
        ResourceCatalogReservation,
    };
    use hex_engine::{
        AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageReservation,
        AudioStorageRole, OwnedAudioInputs, OwnedAudioSource,
    };
    use std::sync::atomic::AtomicBool;
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported constructor; no native Session import proof");
        return;
    }
    for project_rate in [32_000, 16_000] {
        let decoded = decoded_channels(32_000, 12, 2);
        let original_pointer = decoded.audio().frames().as_ptr();
        let expected: Vec<_> = (0..12)
            .map(|n| {
                [
                    (1000 + n * 113) as f32 / 32768.0,
                    (-2000 - n * 71) as f32 / 32768.0,
                ]
            })
            .collect();
        let prepared = PreparedImport::at_rate(
            decoded,
            project_rate,
            ResampleLimits {
                max_source_frames: 1024,
                max_output_frames: 1024,
            },
        )
        .unwrap();
        assert_eq!(prepared.source_audio.frames().as_ptr(), original_pointer);
        assert_eq!(prepared.source_audio.frames(), expected);
        assert_eq!(
            prepared.source_audio.shares_pcm_with(&prepared.audio),
            project_rate == 32_000
        );
        assert!(prepared.waveform.shares_audio_with(&prepared.audio));
        // Both playback and waveform backing carry actual native owner lineage.
        let playback = OwnedAudioSource::try_from_native_audio_on_control(prepared.audio.clone())
            .ok()
            .unwrap();
        let backing = OwnedAudioSource::try_from_native_audio_on_control(
            prepared.waveform.audio_source().unwrap().clone(),
        )
        .ok()
        .unwrap();
        drop(playback);
        drop(backing);
        let mut session = Session::new(project_rate, 16, 18).unwrap();
        let id = asset(session.import_prepared_revision(prepared, None).unwrap().0);
        assert_eq!(
            session.resident_frames(),
            if project_rate == 32_000 { 12 } else { 18 }
        );
        let saved = session.project().clone();
        assert!(session.undo());
        assert!(session.redo());
        assert_eq!(session.project(), &saved);
        let source = OwnedAudioSource::try_from_native_audio_on_control(
            session.source_audio(id).unwrap().clone(),
        )
        .ok()
        .unwrap();
        let bank = OwnedAudioInputs::try_new(
            vec![(id, source)],
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
        let catalog = DeviceResourceCatalog::prepare_on_control(
            &bank,
            &[OriginalResourceSource {
                asset: id,
                sample_rate: 32_000,
                frames: 12,
                layout: OriginalResourceLayout::Stereo,
            }],
            ResourceCatalogReservation {
                metadata_bytes: 65_536,
                hash_frames: 12,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let root = DeviceResourceBank::try_new_on_control(
            bank,
            catalog,
            65_536,
            65_536,
            &AtomicBool::new(false),
        )
        .ok()
        .unwrap();
        let snapshot = root.borrow_original_source_on_control(id, 4096).unwrap();
        assert_eq!(
            snapshot.view().unwrap().resident_frames().unwrap().as_ptr(),
            original_pointer
        );
        // Real Session/project/history/cache owners remain live; they cannot be
        // excluded merely because a standalone resource bank fits its budget.
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                data_rows: 64,
                handle_rows: 16,
                root_rows: 8,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            })
            .unwrap();
        ledger
            .borrow_device_resource_bank_on_control(&root, AudioStorageRole::Caller)
            .unwrap();
        ledger
            .borrow_original_source_lease_on_control(&snapshot, AudioStorageRole::Archive)
            .unwrap();
        assert_eq!(
            ledger.validate_selected_shared_handles_on_control(),
            Err(hex_engine::AudioStorageError::UnknownStorage)
        );
        drop(ledger);
        drop(root);
        drop(session); // Control retirement of all actual media/history/cache owners.
        assert_eq!(
            snapshot.view().unwrap().resident_frames().unwrap(),
            expected
        );
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                data_rows: 64,
                handle_rows: 16,
                root_rows: 8,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            })
            .unwrap();
        ledger
            .borrow_original_source_lease_on_control(&snapshot, AudioStorageRole::Archive)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
    }
}
