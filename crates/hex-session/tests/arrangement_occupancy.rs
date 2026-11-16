//! Real control-worker publication, literal audio and current-format refusal.
use hex_engine::{PlaybackCommand, playback::Playback};
use hex_project::{BeatTime, Command};
use hex_session::service::{Event, Request, Service};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

fn receive(service: &Service) -> Event {
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(Instant::now() < deadline, "worker did not complete");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = terminal(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn terminal(service: &Service) -> Event {
    loop {
        let event = receive(service);
        if event.request_complete {
            return event;
        }
        assert!(event.error.is_none(), "{:?}", event.error);
    }
}
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-arrangement-occupancy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn wav(&self, name: &str, frames: u32, sample: i16) -> PathBuf {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8_000u32.to_le_bytes());
        bytes.extend_from_slice(&16_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        for _ in 0..frames {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn render(event: &Event, block: usize) -> Vec<[f32; 2]> {
    let revision = event
        .playback
        .as_ref()
        .and_then(Playback::prepared_revision)
        .unwrap()
        .clone();
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    playback.apply(PlaybackCommand::Play);
    let mut pcm = vec![[0.0; 2]; 1_000];
    for chunk in pcm.chunks_mut(block) {
        playback.render(chunk).unwrap();
    }
    pcm
}

#[test]
fn worker_raw_import_and_midi_placement_carve_literal_pcm_and_saved_history() {
    for (midi, source_length) in [(false, 1_000), (true, 1_000), (false, 500), (true, 500)] {
        let files = Directory::new();
        let service = Service::new(8_000, 32, 8_000_000).unwrap();
        let initial = receive(&service);
        assert!(initial.error.is_none());
        let imported = run(
            &service,
            Request::Import(files.wav("original.wav", 1_000, 8_192)),
        );
        // Import requests publish their canonical replacement without a Created echo.
        let track = imported.state.project.tracks()[0].id();
        let original = imported.state.project.tracks()[0].clips()[0].id();
        let asset = imported.state.project.clip(original).unwrap().asset();
        let imported = if source_length < 1_000 {
            run(
                &service,
                Request::Edit(Command::TrimClip {
                    clip: original,
                    start: 0,
                    duration: 1_000,
                    source_offset: 0,
                    source_length,
                }),
            )
        } else {
            imported
        };
        let replacement = if midi {
            Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_ratio(400, 4_000).unwrap(),
                length: BeatTime::from_ratio(200, 4_000).unwrap(),
            })
        } else {
            Request::ImportToTrack {
                path: files.wav("replacement.wav", 200, -4_096),
                track,
                start: 400,
            }
        };
        let accepted = run(&service, replacement);
        let project = &accepted.state.project;
        let retained = project.clip(original).unwrap();
        assert_eq!(
            (retained.start(), retained.duration(), retained.asset()),
            (0, 400, asset)
        );
        let right = project
            .track(track)
            .unwrap()
            .clips()
            .iter()
            .find(|clip| clip.start() == 600)
            .unwrap();
        assert_eq!((right.duration(), right.asset()), (400, asset));
        if source_length == 1_000 {
            assert_eq!(right.source_offset(), 600);
        } else {
            assert!(
                right.audio_edit().is_some(),
                "silent suffix retains its original audio domain"
            );
        }
        let literal = (0..1_000)
            .map(|frame| {
                [if (400..600).contains(&frame) {
                    if midi { 0.0 } else { -0.125 }
                } else if frame < source_length {
                    0.25
                } else {
                    0.0
                }; 2]
            })
            .collect::<Vec<_>>();
        assert_eq!(render(&accepted, 61), literal);
        let undo = run(&service, Request::Undo);
        assert_eq!(undo.state.project.tracks(), imported.state.project.tracks());
        assert_eq!(
            render(&undo, 127),
            (0..1_000)
                .map(|frame| [if frame < source_length { 0.25 } else { 0.0 }; 2])
                .collect::<Vec<_>>()
        );
        let redo = run(&service, Request::Redo);
        assert_eq!(redo.state.project, accepted.state.project);
        assert_eq!(render(&redo, 137), literal);
        let path = files.0.join("working.hex");
        let saved = run(&service, Request::SaveNew(path.clone()));
        assert!(saved.save_complete);
        let original_bytes = fs::read(&path).unwrap();
        run(&service, Request::Edit(Command::MasterGain { gain: 0.5 }));
        let opened = run(&service, Request::Open(path.clone()));
        assert!(opened.open_complete && opened.project_replaced);
        assert_eq!(opened.state.project, accepted.state.project);
        assert_eq!(render(&opened, 89), literal);
        // Corrupt only a clip's placement in a separate current-format document.
        let mut document: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
        assert_eq!(document["version"], hex_storage::CURRENT_SCHEMA_VERSION);
        let clips = document
            .pointer_mut("/tracks/0/audio_clips")
            .unwrap()
            .as_array_mut()
            .unwrap();
        let right = clips.iter_mut().find(|clip| clip["start"] == 600).unwrap();
        right["start"] = serde_json::json!(200);
        let invalid = serde_json::to_vec_pretty(&document).unwrap();
        let bad = files.0.join("overlap.hex");
        fs::write(&bad, &invalid).unwrap();
        run(&service, Request::Edit(Command::MasterGain { gain: 0.5 }));
        let working = run(&service, Request::Undo);
        assert!(working.state.can_redo);
        service.submit(Request::Open(bad.clone())).unwrap();
        let refused = terminal(&service);
        assert!(
            refused
                .error
                .as_ref()
                .is_some_and(|error| error.contains("overlap needs opposing audio fade ramps")),
            "{:?}",
            refused.error
        );
        assert!(!refused.project_replaced);
        assert_eq!(refused.state.project, working.state.project);
        assert_eq!(refused.state.revision, working.state.revision);
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (working.state.can_undo, working.state.can_redo)
        );
        assert_eq!(fs::read(bad).unwrap(), invalid);
        assert_eq!(fs::read(path).unwrap(), original_bytes);
    }
}

#[test]
fn faster_global_tempo_crops_adjacent_audio_in_one_publication_and_roundtrip() {
    let files = Directory::new();
    let source = files.wav("adjacent-tempo.wav", 1_000, 8_192);
    let original_bytes = fs::read(&source).unwrap();
    let service = Service::new(8_000, 32, 8_000_000).unwrap();
    assert!(receive(&service).error.is_none());
    let imported = run(&service, Request::Import(source.clone()));
    let first = &imported.state.project.tracks()[0].clips()[0];
    let (clip, asset) = (first.id(), first.asset());
    let track = imported.state.project.tracks()[0].id();
    let before = run(
        &service,
        Request::Edit(Command::PlaceClip {
            track,
            asset,
            start: 1_000,
            duration: 1_000,
            source_offset: 0,
            source_length: 1_000,
            looping: false,
        }),
    );
    let changed = run(
        &service,
        Request::Edit(Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(240_000_000).unwrap(),
        }),
    );
    assert!(changed.error.is_none(), "{:?}", changed.error);
    assert_eq!(changed.state.revision, before.state.revision + 1);
    let clips = changed.state.project.track(track).unwrap().clips();
    assert_eq!(clips.len(), 2);
    assert_eq!(
        (clips[0].id(), clips[0].start(), clips[0].duration()),
        (clip, 0, 500)
    );
    assert_eq!(
        (clips[1].id(), clips[1].start(), clips[1].duration()),
        (
            before.state.project.track(track).unwrap().clips()[1].id(),
            500,
            1_000
        )
    );
    let render = |event: &Event| {
        let revision = event
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .unwrap()
            .clone();
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        playback.apply(PlaybackCommand::Play);
        let mut pcm = vec![[0.0; 2]; 1_501];
        for block in pcm.chunks_mut(61) {
            playback.render(block).unwrap();
        }
        assert_eq!(&pcm[..1_500], &vec![[0.25; 2]; 1_500]);
        assert_eq!(pcm[1_500], [0.0; 2]);
    };
    render(&changed);
    assert_eq!(
        run(&service, Request::Undo).state.project,
        before.state.project
    );
    let redo = run(&service, Request::Redo);
    assert_eq!(redo.state.project, changed.state.project);
    render(&redo);
    let path = files.0.join("adjacent-tempo.hex");
    assert!(run(&service, Request::SaveNew(path.clone())).save_complete);
    run(&service, Request::Edit(Command::MasterGain { gain: 0.5 }));
    let reopened = run(&service, Request::Open(path));
    assert!(reopened.open_complete && reopened.project_replaced);
    assert_eq!(reopened.state.project, changed.state.project);
    render(&reopened);
    assert_eq!(fs::read(source).unwrap(), original_bytes);
}

#[test]
fn global_tempo_worker_publishes_natural_audio_onset_and_reopens_frozen_timing() {
    use hex_project::{Tempo, WarpMode, default_audio_warp};
    let files = Directory::new();
    let service = Service::new(8_000, 32, 8_000_000).unwrap();
    assert!(receive(&service).error.is_none());
    let imported = run(
        &service,
        Request::Import(files.wav("tempo.wav", 1_000, 8_192)),
    );
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let moved = run(
        &service,
        Request::Edit(Command::MoveClip {
            clip,
            track,
            start: 4_000,
        }),
    );
    let settings = default_audio_warp(
        moved.state.project.clip(clip).unwrap(),
        moved.state.project.tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    run(
        &service,
        Request::Edit(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        }),
    );
    let before = run(
        &service,
        Request::Edit(Command::SetAudioClipWarp {
            clip,
            settings: None,
        }),
    );
    let changed = run(
        &service,
        Request::Edit(Command::SetProjectTempo {
            tempo: Tempo::new(60_000_000).unwrap(),
        }),
    );
    let audio = changed.state.project.clip(clip).unwrap();
    assert_eq!((audio.start(), audio.duration()), (8_000, 1_000));
    let render_onset = |event: &Event, onset: u64| {
        let revision = event
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .unwrap()
            .clone();
        let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
        playback.apply(PlaybackCommand::PlayFrom(onset - 1));
        let mut pcm = vec![[0.0; 2]; 1_002];
        for chunk in pcm.chunks_mut(61) {
            playback.render(chunk).unwrap();
        }
        let mut literal = vec![[0.25; 2]; 1_002];
        literal[0] = [0.0; 2];
        literal[1_001] = [0.0; 2];
        assert_eq!(pcm, literal);
    };
    render_onset(&changed, 8_000);
    let undo = run(&service, Request::Undo);
    assert_eq!(undo.state.project, before.state.project);
    render_onset(&undo, 4_000);
    let redo = run(&service, Request::Redo);
    assert_eq!(redo.state.project, changed.state.project);
    render_onset(&redo, 8_000);
    let path = files.0.join("tempo.hex");
    assert!(run(&service, Request::SaveNew(path.clone())).save_complete);
    run(
        &service,
        Request::Edit(Command::SetProjectTempo {
            tempo: Tempo::new(90_000_000).unwrap(),
        }),
    );
    let reopened = run(&service, Request::Open(path));
    assert!(reopened.open_complete && reopened.project_replaced);
    assert_eq!(reopened.state.project, changed.state.project);
    render_onset(&reopened, 8_000);
}

#[test]
fn selection_warp_is_one_worker_publication_with_per_source_settings_and_history() {
    use hex_project::{ClipEdit, WarpMode};
    let files = Directory::new();
    let service = Service::new(8_000, 32, 8_000_000).unwrap();
    assert!(receive(&service).error.is_none());
    let imported = run(
        &service,
        Request::Import(files.wav("selection-a.wav", 1_000, 8_192)),
    );
    let first = imported.state.project.tracks()[0].clips()[0].id();
    let before = run(
        &service,
        Request::Import(files.wav("selection-b.wav", 500, -4_096)),
    );
    let second = before.state.project.tracks()[1].clips()[0].id();
    let warped = run(
        &service,
        Request::Edit(Command::EditClips {
            clips: vec![first, second],
            edit: ClipEdit::WarpEnabled { enabled: true },
        }),
    );
    assert_eq!(warped.state.revision, before.state.revision + 1);
    for (clip, frames) in [(first, 1_000), (second, 500)] {
        let domain = &warped
            .state
            .project
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain;
        assert!(domain.warp_enabled);
        let settings = domain.warp.as_ref().unwrap();
        assert!(matches!(settings.mode, WarpMode::Beats { .. }));
        assert_eq!(settings.markers.last().unwrap().source_frame, frames);
    }
    let undo = run(&service, Request::Undo);
    assert_eq!(undo.state.project, before.state.project);
    let redo = run(&service, Request::Redo);
    assert_eq!(redo.state.project, warped.state.project);
    let natural = run(
        &service,
        Request::Edit(Command::EditClips {
            clips: vec![first, second],
            edit: ClipEdit::WarpEnabled { enabled: false },
        }),
    );
    let expected = (0..1_000)
        .map(|frame| [if frame < 500 { 0.125 } else { 0.25 }; 2])
        .collect::<Vec<_>>();
    assert_eq!(render(&natural, 61), expected);
    let path = files.0.join("group.hex");
    run(&service, Request::SaveNew(path.clone()));
    run(
        &service,
        Request::Edit(Command::EditClips {
            clips: vec![first, second],
            edit: ClipEdit::Gain { millidb: -6_000 },
        }),
    );
    let opened = run(&service, Request::Open(path));
    assert_eq!(opened.state.project, natural.state.project);
    assert_eq!(render(&opened, 127), expected);
}
