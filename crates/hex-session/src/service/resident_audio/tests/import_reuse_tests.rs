//! The resident-publication import path reuses an unchanged resident file
//! without decoding it and decodes a changed file as a new asset.
use super::*;
use crate::service::import_reuse_tests::make_undecodable_keeping_identity;
use crate::service::service_tests::{TestDir, wav_mono};
use std::fs;

#[test]
fn resident_publication_reuses_an_unchanged_file_and_decodes_a_changed_one() {
    let directory = TestDir::new();
    let path = directory.0.join("hit.wav");
    fs::write(&path, wav_mono(&[2_000, -2_000, 4_000])).unwrap();
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        None,
        fixture_sink(),
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
    let run = |request| {
        service.submit(request).unwrap();
        let event = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let _ = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    let imported = run(Request::Import(path.clone()));
    let track = imported.state.project.tracks()[0].id();
    make_undecodable_keeping_identity(&path);

    let reused = run(Request::ImportToTrack {
        path: path.clone(),
        track,
        start: 10,
    });
    assert_eq!(reused.state.project.assets().count(), 1);
    assert_eq!(reused.state.project.track(track).unwrap().clips().len(), 2);
    let undone = run(Request::Undo);
    // Undo keeps the id allocator's high-water mark, so compare content.
    assert_eq!(
        undone.state.project.tracks(),
        imported.state.project.tracks()
    );
    assert!(
        undone
            .state
            .project
            .assets()
            .eq(imported.state.project.assets())
    );

    fs::write(&path, wav_mono(&[1_000; 5])).unwrap();
    let decoded = run(Request::Import(path));
    let frames = decoded
        .state
        .project
        .assets()
        .map(|asset| asset.source_frames())
        .collect::<Vec<_>>();
    assert_eq!(frames, [3, 5]);
}

#[test]
fn resident_publication_captures_new_track_gain_and_reuses_media() {
    let directory = TestDir::new();
    let path = directory.0.join("gain.wav");
    fs::write(&path, wav_mono(&[2000, -2000, 4000])).unwrap();
    let retained = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
    let service = Service::with_audio_sink_and_native_reservation(
        16,
        8_000_000,
        None,
        fixture_sink(),
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
    let _ = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    let submit = |gain| {
        service
            .submit(Request::ImportToNewTrackWithGain {
                path: path.clone(),
                before_track: None,
                start: 0,
                gain,
            })
            .unwrap();
        let event = service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap();
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    };
    let first = submit(0.25);
    assert_eq!(first.state.project.tracks()[0].gain(), 0.25);
    make_undecodable_keeping_identity(&path);
    let second = submit(0.5);
    assert_eq!(second.state.project.tracks()[1].gain(), 0.5);
    assert_eq!(second.state.project.assets().count(), 1);
    service.submit(Request::Undo).unwrap();
    let undone = service
        .events
        .recv_timeout(crate::test_wait::HANG_BOUND)
        .unwrap();
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project.tracks(), first.state.project.tracks());
}
