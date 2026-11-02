use super::*;
use crate::service::Service;
use crate::service::service_tests::{TestDir, next_event, wav_mono};
use std::fs;

/// A mono PCM16 recording whose `data` chunk declares more bytes than exist,
/// ending in one byte of an incomplete frame.
fn truncated_recording(readable: usize, missing_frames: usize) -> Vec<u8> {
    let samples: Vec<i16> = (0..readable + missing_frames)
        .map(|index| (index as i16).wrapping_mul(97))
        .collect();
    let mut bytes = wav_mono(&samples);
    bytes.truncate(bytes.len() - missing_frames * 2 + 1);
    bytes
}

#[test]
fn truncated_import_is_refused_with_a_typed_report_then_recovered_only_on_request() {
    let directory = TestDir::new();
    let path = directory.0.join("cut-short.wav");
    let original = truncated_recording(96, 32);
    fs::write(&path, &original).expect("truncated fixture");
    let Err(ImportError::Truncated(expected)) =
        hex_media::decode_wav_file(&path, Limits::new(1 << 20, 1 << 20))
    else {
        panic!("fixture must be refused as truncated")
    };
    assert_eq!(expected.readable_frames, 96);

    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    next_event(&service);
    service
        .submit(Request::Import(path.clone()))
        .expect("plain import");
    let refused = next_event(&service);
    assert!(refused.error.is_some());
    assert_eq!(
        refused.import_truncation,
        Some(ImportTruncation::new(&path, expected))
    );
    assert!(refused.state.project.tracks().is_empty());

    let recovery =
        TruncatedImportRecovery::new(Request::Import(path.clone())).expect("single-file import");
    service
        .submit(Request::RecoverTruncatedImport(recovery))
        .expect("recovery import");
    let recovered = next_event(&service);
    assert!(recovered.error.is_none(), "{:?}", recovered.error);
    assert!(recovered.import_truncation.is_none());
    let status = recovered.status.expect("recovery status");
    assert!(
        status.contains("recovered 96 readable frames"),
        "fidelity loss must be visible: {status}"
    );
    let project = &recovered.state.project;
    let asset = project.assets().next().expect("recovered asset");
    assert_eq!(asset.source_frames(), 96);
    let source = asset.path().to_path_buf();
    assert_ne!(source, path);
    assert!(status.contains(source.to_str().unwrap()), "{status}");
    let reopened_pcm = hex_media::decode_wav_file(&source, Limits::new(1 << 20, 1024)).unwrap();
    assert_eq!(reopened_pcm.frames(), 96);
    assert_eq!(project.tracks()[0].clips()[0].duration(), 96);
    let project_path = directory.0.join("recovered.hex");
    service
        .submit(Request::SaveNew(project_path.clone()))
        .unwrap();
    let saved = next_event(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    service.submit(Request::Open(project_path)).unwrap();
    let opened = next_event(&service);
    assert!(opened.error.is_none(), "{:?}", opened.error);
    let asset = opened.state.project.assets().next().unwrap();
    assert_eq!(asset.path(), source);
    let mut playback = opened.playback.expect("reopened playback");
    playback.apply(hex_engine::PlaybackCommand::Play);
    let mut rendered = [[0.0; 2]; 96];
    playback.render(&mut rendered).unwrap();
    assert!(rendered.iter().flatten().any(|sample| sample.abs() > 0.001));
    assert_eq!(fs::read(&path).expect("original"), original);
}

#[test]
fn only_single_file_imports_can_request_recovery() {
    assert!(TruncatedImportRecovery::new(Request::SaveCurrent).is_err());
    let path = PathBuf::from("/a.wav");
    let recovery = TruncatedImportRecovery::new(Request::Import(path.clone())).expect("import");
    assert!(matches!(recovery.request(), Request::Import(p) if *p == path));
}

#[test]
fn failed_admission_reports_and_retains_the_complete_recovered_file() {
    let directory = TestDir::new();
    let original_path = directory.0.join("truncated.wav");
    let original = truncated_recording(96, 32);
    fs::write(&original_path, &original).unwrap();
    let service = Service::new(48_000, 8, 1).unwrap();
    next_event(&service);
    let recovery = TruncatedImportRecovery::new(Request::Import(original_path.clone())).unwrap();
    service
        .submit(Request::RecoverTruncatedImport(recovery))
        .unwrap();
    let failed = next_event(&service);
    let message = failed.error.expect("resident limit must refuse admission");
    let recovered: Vec<_> = fs::read_dir(&directory.0)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path != &original_path)
        .collect();
    assert_eq!(recovered.len(), 1);
    assert!(
        message.contains(recovered[0].to_str().unwrap()),
        "{message}"
    );
    assert_eq!(
        hex_media::decode_wav_file(&recovered[0], Limits::new(1024 * 1024, 1024))
            .unwrap()
            .frames(),
        96
    );
    assert!(failed.state.project.assets().next().is_none());
    assert_eq!(fs::read(original_path).unwrap(), original);
}

#[test]
fn each_single_file_destination_binds_the_new_complete_source() {
    for destination in 0..4 {
        let directory = TestDir::new();
        let path = directory.0.join("truncated.wav");
        let original = truncated_recording(96, 32);
        fs::write(&path, &original).unwrap();
        let service = Service::new(48_000, 8, 8_000_000).unwrap();
        next_event(&service);
        service
            .submit(Request::Edit(hex_project::Command::AddTrack {
                name: "target".into(),
            }))
            .unwrap();
        let track = next_event(&service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(hex_project::Command::AddScene {
                name: "scene".into(),
            }))
            .unwrap();
        let scene = next_event(&service).state.project.scenes()[0].id();
        let request = match destination {
            0 => Request::ImportToTrack {
                path: path.clone(),
                track,
                start: 0,
            },
            1 => Request::ImportToTrackWithOverwrite {
                path: path.clone(),
                track,
                start: 0,
            },
            2 => Request::ImportToNewTrack {
                path: path.clone(),
                before_track: None,
                start: 0,
            },
            _ => Request::ImportToSession {
                path: path.clone(),
                track,
                scene,
            },
        };
        let recovery = TruncatedImportRecovery::new(request).unwrap();
        service
            .submit(Request::RecoverTruncatedImport(recovery))
            .unwrap();
        let event = next_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        let asset = event.state.project.assets().next().unwrap();
        assert_ne!(asset.path(), path);
        assert_eq!(
            hex_media::decode_wav_file(asset.path(), Limits::new(1 << 20, 1024))
                .unwrap()
                .frames(),
            96
        );
        assert_eq!(fs::read(path).unwrap(), original);
    }
}

#[test]
fn resident_publication_path_uses_the_complete_recovered_source() {
    let directory = TestDir::new();
    let path = directory.0.join("truncated.wav");
    let original = truncated_recording(96, 32);
    fs::write(&path, &original).unwrap();
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        None,
        super::super::resident_audio::tests::fixture_sink(),
        crate::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 64,
                root_rows: 64,
                max_retained_bytes: retained,
                max_construction_peak_bytes: retained
                    + hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES,
            },
            max_scan_frames: 1024,
        },
    )
    .unwrap();
    next_event(&service);
    let recovery = TruncatedImportRecovery::new(Request::Import(path.clone())).unwrap();
    service
        .submit(Request::RecoverTruncatedImport(recovery))
        .unwrap();
    let event = next_event(&service);
    assert!(event.error.is_none(), "{:?}", event.error);
    let asset = event.state.project.assets().next().unwrap();
    assert_ne!(asset.path(), path);
    assert_eq!(
        hex_media::decode_wav_file(asset.path(), Limits::new(1 << 20, 1024))
            .unwrap()
            .frames(),
        96
    );
    assert_eq!(fs::read(path).unwrap(), original);
}
