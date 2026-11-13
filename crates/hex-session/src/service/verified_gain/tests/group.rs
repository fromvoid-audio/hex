//! Correlate every group member with the actual worker-owned preview/source.
use super::*;
use crate::device_gesture::GestureRequest;
use hex_project::{Command, Created, Editor, RelativeTrackGains, TrackId};

fn group_fixture(verified: bool) -> (Service, Vec<TrackId>, GestureKey, Event) {
    let service = Service::new(48_000, 16, 0).unwrap();
    next(&service);
    if verified {
        send(
            &service,
            Request::NewWithAudioMode {
                mode: DocumentAudioMode::VerifiedStreaming,
                reservation: Some(crate::SourcePlanReservation {
                    reader: hex_media::VerifiedWavReaderLimits::new(
                        hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                        1024 * 1024,
                    ),
                    max_retained_sources: 1,
                    max_path_bytes: 1024 * 1024,
                    max_manifest_bytes: 1024 * 1024,
                }),
            },
        );
    }
    let mut ids = Vec::new();
    let mut state = None;
    for gain in [0.5, 1.5] {
        let added = send(
            &service,
            Request::Edit(Command::AddTrack {
                name: "gain".into(),
            }),
        );
        let Some(Created::Track(track)) = added.created else {
            panic!("track")
        };
        state = Some(send(&service, Request::Edit(Command::Gain { track, gain })).state);
        ids.push(track);
    }
    let state = state.unwrap();
    let group = RelativeTrackGains::capture(&state.project, ids[0], &ids).unwrap();
    let key = GestureKey {
        generation: state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        token: 1,
    };
    let began = send(
        &service,
        Request::DeviceGesture(GestureRequest::BeginTrackGains { key, group }),
    );
    assert!(began.gesture_ack.unwrap().accepted);
    let changed = send(
        &service,
        Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.75),
        }),
    );
    assert!(changed.gesture_ack.as_ref().unwrap().accepted);
    (service, ids, key, changed)
}

#[test]
fn grouped_verified_projection_checks_non_anchor_values_and_frozen_origins() {
    let (service, ids, key, mut changed) = group_fixture(true);
    let source = VerifiedTrackGainSource::capture(&changed, None)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(
        source.canonical(),
        changed.state.canonical_audio_source.as_ref().unwrap()
    ));
    let original = Arc::clone(&changed.device_preview.as_ref().unwrap().state.project);
    let mut altered = Editor::new((*original).clone(), 1).unwrap();
    altered
        .execute(Command::Gain {
            track: ids[1],
            gain: 0.125,
        })
        .unwrap();
    Arc::get_mut(&mut changed.device_preview.as_mut().unwrap().state)
        .unwrap()
        .project = Arc::new(altered.project().clone());
    assert!(
        VerifiedTrackGainSource::capture(&changed, None).is_err(),
        "an unchanged anchor cannot authorize another member's wrong preview"
    );
    Arc::get_mut(&mut changed.device_preview.as_mut().unwrap().state)
        .unwrap()
        .project = original;
    let mut false_base = Editor::new((*changed.state.project).clone(), 1).unwrap();
    false_base
        .execute(Command::Gain {
            track: ids[1],
            gain: 1.75,
        })
        .unwrap();
    // Both versions clamp to the same 2.0 preview. Correlation must still refuse
    // a different frozen origin rather than trusting the final displayed value.
    changed.active_device_gesture.as_mut().unwrap().track_gains =
        Some(RelativeTrackGains::capture(false_base.project(), ids[0], &ids).unwrap());
    assert!(VerifiedTrackGainSource::capture(&changed, None).is_err());
    let cancelled = send(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    let terminal = VerifiedTrackGainSource::capture(&cancelled, Some((key, 1)))
        .unwrap()
        .unwrap();
    assert_eq!(terminal.terminal_key(), Some(key));
    assert!(terminal.preview_identity().is_none());
}

#[test]
fn group_commit_save_and_reopen_preserve_ordinary_gain_fields() {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let directory = Directory(std::env::temp_dir().join(format!(
        "hex-group-gain-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir(&directory.0).unwrap();
    let (service, ids, key, _) = group_fixture(false);
    let committed = send(
        &service,
        Request::DeviceGesture(GestureRequest::Commit {
            key,
            sequence: 2,
            manual_edit: true,
            value: ParameterValue::Float(0.75),
        }),
    );
    assert!(committed.gesture_ack.as_ref().unwrap().accepted);
    assert!(committed.active_device_gesture.is_none());
    assert_eq!(committed.state.project.track(ids[0]).unwrap().gain(), 0.75);
    assert_eq!(committed.state.project.track(ids[1]).unwrap().gain(), 2.0);
    let path = directory.0.join("saved.hex");
    let saved = send(&service, Request::SaveNew(path.clone()));
    assert!(saved.save_complete);
    let reopened = send(&service, Request::Open(path));
    assert_eq!(reopened.state.project, committed.state.project);
    assert!(reopened.active_device_gesture.is_none());
}
