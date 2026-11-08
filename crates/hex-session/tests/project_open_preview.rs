//! Replacement is transactional with respect to acknowledged tuning previews.
use hex_project::Command;
use hex_session::{
    DocumentAudioMode, SourcePlanReservation,
    service::{Event, ProjectSource, Request, Service},
    tuning_gesture::{TuningGestureKey, TuningGestureRequest},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

struct Fixture {
    service: Service,
    directory: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "hex-open-preview-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let service = Service::new(48_000, 16, 0).unwrap();
        wait(&service);
        Self { service, directory }
    }
    fn send(&self, request: Request) -> Event {
        self.service.submit(request).unwrap();
        wait(&self.service)
    }
    fn preview(&self) -> Event {
        let key = TuningGestureKey {
            generation: 0,
            token: 1,
        };
        let begun = self.send(Request::TuningGesture(TuningGestureRequest::Begin { key }));
        assert!(begun.tuning_gesture_ack.unwrap().accepted);
        let mut scale = hex_tuning::Scale::default();
        scale.set_reference(60, 333.0, 0).unwrap();
        let tuning =
            hex_tuning::ProjectTuning::new(true, scale, hex_tuning::Audition::default()).unwrap();
        let preview = self.send(Request::TuningGesture(TuningGestureRequest::Change {
            key,
            sequence: 1,
            tuning,
        }));
        assert!(preview.error.is_none(), "{:?}", preview.error);
        assert!(preview.tuning_preview.is_some());
        assert!(preview.active_tuning_gesture.is_some());
        preview
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn wait(service: &Service) -> Event {
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(error) => panic!("missing worker event: {error}"),
        }
    }
}

fn save_unpreparable(path: &std::path::Path) {
    use hex_project::{BeatTime, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "dense".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene");
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "dense".into(),
            source_length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip");
    };
    for pitch in 0..65 {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).unwrap(),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    hex_storage::save_new(path, editor.project()).unwrap();
    assert_eq!(hex_storage::load(path).unwrap(), *editor.project());
}

fn reservation(mode: DocumentAudioMode) -> Option<SourcePlanReservation> {
    (mode == DocumentAudioMode::VerifiedStreaming).then_some(SourcePlanReservation {
        reader: hex_media::VerifiedWavReaderLimits::new(
            hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
            1024 * 1024,
        ),
        max_retained_sources: 1,
        max_path_bytes: 1024 * 1024,
        max_manifest_bytes: 1024 * 1024,
    })
}

#[test]
fn failed_open_preserves_acknowledged_preview_canonical_source_and_history() {
    for mode in [
        DocumentAudioMode::Resident,
        DocumentAudioMode::VerifiedStreaming,
    ] {
        let fixture = Fixture::new();
        let edited = fixture.send(Request::Edit(Command::AddTrack {
            name: "kept".into(),
        }));
        assert!(edited.error.is_none());
        let before = fixture.preview();
        let source = before.state.canonical_audio_source.as_ref().unwrap();
        let accepted = Arc::clone(
            edited
                .playback
                .as_ref()
                .unwrap()
                .prepared_revision()
                .unwrap(),
        );
        for failure in 0..3 {
            let path = fixture
                .directory
                .join(["missing.hex", "malformed.hex", "unpreparable.hex"][failure]);
            if failure == 1 {
                std::fs::write(&path, b"not a current project").unwrap();
            }
            if failure == 2 {
                save_unpreparable(&path);
            }
            let failed = fixture.send(Request::OpenWithAudioMode {
                source: ProjectSource::Saved(path),
                relinks: BTreeMap::new(),
                mode,
                reservation: reservation(mode),
            });
            assert!(failed.error.is_some());
            assert!(!failed.project_replaced);
            assert_eq!(failed.active_tuning_gesture, before.active_tuning_gesture);
            assert!(
                failed.playback.is_none(),
                "a refused replacement must not publish over the audible preview"
            );
            assert_eq!(failed.state.project, before.state.project);
            assert_eq!(failed.state.revision, before.state.revision);
            assert_eq!(
                (failed.state.can_undo, failed.state.can_redo),
                (before.state.can_undo, before.state.can_redo)
            );
            assert!(Arc::ptr_eq(
                failed.state.canonical_audio_source.as_ref().unwrap(),
                source
            ));
            let restored = fixture.send(Request::RestorePlayback { generation: 7 });
            assert!(restored.error.is_none());
            assert!(Arc::ptr_eq(
                restored
                    .playback
                    .as_ref()
                    .unwrap()
                    .prepared_revision()
                    .unwrap(),
                &accepted
            ));
            assert_eq!(restored.active_tuning_gesture, before.active_tuning_gesture);
        }
        let undone = fixture.send(Request::TuningGesture(TuningGestureRequest::Cancel {
            key: before.active_tuning_gesture.unwrap().key,
        }));
        assert!(undone.error.is_none());
        let undone = fixture.send(Request::Undo);
        assert!(undone.error.is_none());
        assert!(
            undone.state.project.tracks().is_empty(),
            "failed Open must not insert a tuning history entry"
        );
    }
}

#[test]
fn successful_open_and_new_retire_preview_only_after_replacement() {
    for new in [false, true] {
        for mode in [
            DocumentAudioMode::Resident,
            DocumentAudioMode::VerifiedStreaming,
        ] {
            let fixture = Fixture::new();
            let path = fixture.directory.join("saved.hex");
            let saved = fixture.send(Request::SaveNew(path.clone()));
            assert!(saved.error.is_none());
            let before = fixture.preview();
            let request = if new {
                Request::NewWithAudioMode {
                    mode,
                    reservation: reservation(mode),
                }
            } else {
                Request::OpenWithAudioMode {
                    source: ProjectSource::Saved(path),
                    relinks: BTreeMap::new(),
                    mode,
                    reservation: reservation(mode),
                }
            };
            let replaced = fixture.send(request);
            assert!(replaced.error.is_none(), "{:?}", replaced.error);
            assert!(replaced.project_replaced);
            assert!(replaced.active_tuning_gesture.is_none());
            assert!(replaced.tuning_preview.is_none());
            assert_eq!(
                replaced.state.project.tuning(),
                before.state.project.tuning()
            );
            assert!(!replaced.state.can_undo);
            assert!(!replaced.state.can_redo);
            assert_eq!(
                replaced
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context()
                    .project_generation,
                1
            );
        }
    }
}

#[test]
fn save_new_and_current_commit_preview_and_keep_one_undo_entry() {
    for current in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.directory.join("saved.hex");
        if current {
            assert!(fixture.send(Request::SaveNew(path.clone())).error.is_none());
        }
        let before = fixture.preview();
        let draft = before.active_tuning_gesture.unwrap().tuning;
        let saved = fixture.send(if current {
            Request::SaveCurrent
        } else {
            Request::SaveNew(path.clone())
        });
        assert!(saved.error.is_none(), "{:?}", saved.error);
        assert!(saved.save_complete);
        assert!(!saved.project_replaced);
        assert_eq!(saved.state.project.tuning(), draft);
        assert!(saved.active_tuning_gesture.is_none());
        assert!(saved.tuning_preview.is_none());
        assert_eq!(saved.state.revision, before.state.revision + 1);
        let undone = fixture.send(Request::Undo);
        assert_eq!(undone.state.project.tuning(), before.state.project.tuning());
        assert!(!undone.state.can_undo);
        let redone = fixture.send(Request::Redo);
        assert_eq!(redone.state.project.tuning(), draft);
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
        let reopened = fixture.send(Request::Open(path));
        assert!(reopened.error.is_none());
        assert_eq!(reopened.state.project.tuning(), draft);
    }
}
