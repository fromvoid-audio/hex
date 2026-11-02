use crate::*;
use hex_media::{Limits, decode_wav_file, resample};
use std::{
    fs,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

fn decoded(rate: u32, frames: u32) -> DecodedAudio {
    let path = std::env::temp_dir().join(format!(
        "hex-original-pcm-{}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = frames * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for index in 0..frames {
        wav.extend_from_slice(&(1000 + index as i16 * 113).to_le_bytes());
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
    let law = hex_project::import_conversion_version(32_000, 16_000);
    let expected = resample_with_version(
        &original,
        16_000,
        ResampleLimits {
            max_source_frames: 1024,
            max_output_frames: 1024,
        },
        hex_media::ResampleVersion::for_conversion(law).unwrap(),
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
        file_identity: None,
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
fn sinc_v2_asset_saves_as_schema64_and_reopens_with_its_own_conversion_law() {
    let limits = ResampleLimits {
        max_source_frames: 1024,
        max_output_frames: 1024,
    };
    let decoded = decoded(32_000, 250);
    let v1 = resample(decoded.audio(), 16_000, limits).unwrap();
    let v2 = hex_media::resample_with_version(
        decoded.audio(),
        16_000,
        limits,
        hex_media::ResampleVersion::V2,
    )
    .unwrap();
    assert_ne!(v1.frames(), v2.frames(), "the laws differ for this source");
    let mut session = Session::new(16_000, 16, 1024).unwrap();
    let prepared = PreparedImport::for_conversion(
        decoded.clone(),
        16_000,
        limits,
        hex_project::SINC_CONVERSION_V2,
    )
    .unwrap();
    let id = asset(session.import_prepared_revision(prepared, None).unwrap().0);
    assert_eq!(session.audio(id).unwrap().frames(), v2.frames());

    let bytes = hex_storage::encode(session.project()).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["version"], hex_storage::SINC_V2_SCHEMA_VERSION);
    let loaded = hex_storage::decode(&bytes).unwrap();
    assert_eq!(
        loaded.asset(id).unwrap().conversion_version(),
        hex_project::SINC_CONVERSION_V2
    );
    let reopened = Session::from_loaded(loaded, 16, 1024, vec![(id, decoded)]).unwrap();
    assert_eq!(reopened.audio(id).unwrap().frames(), v2.frames());
}
