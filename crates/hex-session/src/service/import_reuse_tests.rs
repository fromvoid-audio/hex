//! The single-file import requests skip decoding a file whose identity still
//! matches a resident asset, and decode a changed file as a new asset.
use super::service_tests::{TestDir, next_event, wav_mono};
use super::*;
use std::{fs, path::Path};

/// Overwrite `path` with same-length bytes that are not a WAV and restore its
/// modified time, so its identity is unchanged but any decode would fail.
pub(super) fn make_undecodable_keeping_identity(path: &Path) {
    let metadata = fs::metadata(path).unwrap();
    let modified = metadata.modified().unwrap();
    let length = usize::try_from(metadata.len()).unwrap();
    fs::write(path, vec![0xa5_u8; length]).unwrap();
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert!(hex_media::decode_wav_file(path, Limits::new(1 << 20, 1 << 20)).is_err());
}

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next_event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}

#[test]
fn unchanged_file_reimports_through_every_request_without_decoding() {
    let directory = TestDir::new();
    let path = directory.0.join("loop.wav");
    fs::write(&path, wav_mono(&[1_000, -2_000, 3_000, -4_000])).unwrap();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    next_event(&service);
    let imported = run(&service, Request::Import(path.clone()));
    let track = imported.state.project.tracks()[0].id();
    let scene = run(
        &service,
        Request::Edit(Command::AddScene {
            name: "scene".into(),
        }),
    )
    .state
    .project
    .scenes()[0]
        .id();
    make_undecodable_keeping_identity(&path);

    for request in [
        Request::Import(path.clone()),
        Request::ImportToTrack {
            path: path.clone(),
            track,
            start: 100,
        },
        Request::ImportToTrackWithOverwrite {
            path: path.clone(),
            track,
            start: 200,
        },
        Request::ImportToNewTrack {
            path: path.clone(),
            before_track: Some(track),
            start: 0,
        },
        Request::ImportToSession {
            path: path.clone(),
            track,
            scene,
        },
    ] {
        let event = run(&service, request);
        assert_eq!(event.state.project.assets().count(), 1);
    }
    let project = run(&service, Request::Import(path.clone()))
        .state
        .project
        .clone();
    assert_eq!(project.tracks().len(), 4);
    assert_eq!(project.track(track).unwrap().clips().len(), 3);
    assert!(project.session_cell(track, scene).is_some());
}

#[test]
fn changed_file_decodes_again_as_a_new_asset() {
    let directory = TestDir::new();
    let path = directory.0.join("take.wav");
    fs::write(&path, wav_mono(&[1_000, -2_000, 3_000, -4_000])).unwrap();
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    next_event(&service);
    run(&service, Request::Import(path.clone()));
    fs::write(&path, wav_mono(&[500; 6])).unwrap();

    let project = run(&service, Request::Import(path.clone()))
        .state
        .project
        .clone();
    let frames = project
        .assets()
        .map(|asset| asset.source_frames())
        .collect::<Vec<_>>();
    assert_eq!(frames, [4, 6]);

    // A changed file that no longer decodes reports the decode failure.
    fs::write(&path, b"not a wav").unwrap();
    service.submit(Request::Import(path)).unwrap();
    let refused = next_event(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, project);
}
