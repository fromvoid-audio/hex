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
    assert_eq!(project.tracks()[0].clips()[0].duration(), 96);
    assert_eq!(fs::read(&path).expect("original"), original);
}

#[test]
fn only_single_file_imports_can_request_recovery() {
    assert!(TruncatedImportRecovery::new(Request::SaveCurrent).is_err());
    let path = PathBuf::from("/a.wav");
    let recovery = TruncatedImportRecovery::new(Request::Import(path.clone())).expect("import");
    assert!(matches!(recovery.request(), Request::Import(p) if *p == path));
}
