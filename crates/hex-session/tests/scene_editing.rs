//! Scene duplication and ordering through the GPUI-facing worker contract.
mod common;

use common::wait;
use hex_engine::playback::Playback;
use hex_engine::{LaunchQuantization, PlaybackAction, PreparedRevision, SessionPlaybackCommand};
use hex_project::{BeatTime, Command, Created, SessionCellContent};
use hex_session::service::{Event, Request, Service};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hex-session-scene-editing-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create isolated test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn edit(service: &Service, command: Command) -> Event {
    service.submit(Request::Edit(command)).expect("submit edit");
    wait(service)
}

fn prepared(event: &Event) -> std::sync::Arc<PreparedRevision> {
    event
        .playback
        .as_ref()
        .and_then(Playback::prepared_revision)
        .expect("event carries the prepared revision")
        .clone()
}

fn render_clip(
    revision: std::sync::Arc<PreparedRevision>,
    clip: hex_project::ClipId,
    frames: usize,
    partitions: &[usize],
) -> Vec<[f32; 2]> {
    let mut playback =
        Playback::try_from_prepared_with_session(revision).expect("prepared scene runtime");
    playback
        .try_apply_action(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .expect("launch Session clip");
    let mut output = Vec::with_capacity(frames);
    let mut remaining = frames;
    let mut partition_index = 0;
    while remaining > 0 {
        let count = partitions[partition_index % partitions.len()].min(remaining);
        let start = output.len();
        output.resize(start + count, [0.0; 2]);
        playback
            .render(&mut output[start..start + count])
            .expect("render launched clip");
        remaining -= count;
        partition_index += 1;
    }
    output
}

#[test]
fn worker_duplicates_reorders_saves_and_reopens_session_scenes() {
    let directory = TempDir::new();
    let path = directory.path().join("scene-editing.hex");
    let service = Service::new(48_000, 32, 0).expect("session worker");
    assert!(wait(&service).playback.is_some());

    let track = match edit(
        &service,
        Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        },
    )
    .created
    .expect("instrument track")
    {
        Created::Track(id) => id,
        result => panic!("unexpected track result: {result:?}"),
    };
    let source_scene = match edit(
        &service,
        Command::AddScene {
            name: "original".into(),
        },
    )
    .created
    .expect("source scene")
    {
        Created::Scene(id) => id,
        result => panic!("unexpected scene result: {result:?}"),
    };
    let source_clip = match edit(
        &service,
        Command::CreateSessionMidiClip {
            track,
            scene: source_scene,
            name: "authored".into(),
            source_length: BeatTime::from_quarters(4).expect("source length"),
        },
    )
    .created
    .expect("source Session clip")
    {
        Created::SessionClip(id) => id,
        result => panic!("unexpected clip result: {result:?}"),
    };
    let source_note = match edit(
        &service,
        Command::AddMidiNote {
            clip: source_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).expect("gate"),
            pitch: 64,
            channel: 0,
            velocity: 100,
        },
    )
    .created
    .expect("source note")
    {
        Created::Note(id) => id,
        result => panic!("unexpected note result: {result:?}"),
    };

    let duplicate_event = edit(&service, Command::DuplicateScene(source_scene));
    let duplicate_scene = match duplicate_event.created.expect("duplicate scene ID") {
        Created::Scene(id) => id,
        result => panic!("unexpected duplicate result: {result:?}"),
    };
    let duplicate_clip = match duplicate_event
        .state
        .project
        .session_cell(track, duplicate_scene)
        .expect("duplicated Session cell")
    {
        SessionCellContent::Midi(clip) => clip.id(),
        SessionCellContent::Audio(_) => panic!("duplicated MIDI cell expected"),
        SessionCellContent::Stop => panic!("duplicated MIDI cell expected"),
    };
    let duplicate_note = duplicate_event
        .state
        .project
        .session_midi_clip(duplicate_clip)
        .unwrap()
        .source()
        .notes()[0]
        .id();
    assert_ne!(duplicate_clip, source_clip);
    assert_ne!(duplicate_note, source_note);
    assert_eq!(
        duplicate_event
            .state
            .project
            .scenes()
            .iter()
            .map(|scene| scene.id())
            .collect::<Vec<_>>(),
        vec![source_scene, duplicate_scene]
    );
    let duplicate_revision = prepared(&duplicate_event);
    assert_eq!(
        duplicate_revision
            .session()
            .scenes()
            .iter()
            .map(|scene| scene.id)
            .collect::<Vec<_>>(),
        vec![source_scene, duplicate_scene]
    );
    assert_eq!(
        duplicate_revision.session().cell(track, duplicate_scene),
        Some(hex_engine::PreparedSessionCellContent::Midi(duplicate_clip))
    );

    service.submit(Request::Undo).expect("undo duplication");
    let undone = wait(&service);
    assert_eq!(undone.state.project.scenes().len(), 1);
    assert_eq!(undone.state.project.session_midi_clip(duplicate_clip), None);
    assert_eq!(prepared(&undone).session().scenes().len(), 1);
    assert!(undone.state.unsaved);
    service.submit(Request::Redo).expect("redo duplication");
    let redone = wait(&service);
    assert_eq!(
        redone
            .state
            .project
            .session_midi_clip(duplicate_clip)
            .unwrap()
            .id(),
        duplicate_clip
    );
    assert_eq!(
        prepared(&redone)
            .session()
            .midi_clip(duplicate_clip)
            .unwrap()
            .id,
        duplicate_clip
    );

    let moved = edit(
        &service,
        Command::MoveScene {
            scene: duplicate_scene,
            before: Some(source_scene),
        },
    );
    assert_eq!(
        moved
            .state
            .project
            .scenes()
            .iter()
            .map(|scene| scene.id())
            .collect::<Vec<_>>(),
        vec![duplicate_scene, source_scene]
    );
    assert_eq!(
        prepared(&moved)
            .session()
            .scenes()
            .iter()
            .map(|scene| scene.id)
            .collect::<Vec<_>>(),
        vec![duplicate_scene, source_scene]
    );
    assert_eq!(prepared(&moved).session().midi_clips().len(), 2);
    service.submit(Request::Undo).expect("undo reorder");
    let order_undo = wait(&service);
    assert_eq!(order_undo.state.project.scenes()[0].id(), source_scene);
    assert_eq!(prepared(&order_undo).session().scenes()[0].id, source_scene);
    service.submit(Request::Redo).expect("redo reorder");
    let order_redo = wait(&service);
    assert_eq!(order_redo.state.project.scenes()[0].id(), duplicate_scene);

    let expected_project = order_redo.state.project.as_ref().clone();
    let expected_revision = prepared(&order_redo);
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save duplicated and ordered scenes");
    let saved = wait(&service);
    assert!(saved.save_complete);
    assert!(!saved.state.unsaved);

    let reopened_service = Service::new(48_000, 32, 0).expect("fresh session worker");
    wait(&reopened_service);
    reopened_service
        .submit(Request::Open(path.clone()))
        .expect("open saved scene project");
    let reopened = wait(&reopened_service);
    assert!(reopened.open_complete && reopened.project_replaced);
    assert!(!reopened.state.unsaved);
    assert_eq!(
        reopened.state.document_path.as_deref(),
        Some(path.as_path())
    );
    assert_eq!(reopened.state.project.as_ref(), &expected_project);
    let reopened_revision = prepared(&reopened);
    assert_eq!(
        reopened_revision
            .session()
            .scenes()
            .iter()
            .map(|scene| scene.id)
            .collect::<Vec<_>>(),
        vec![duplicate_scene, source_scene]
    );
    assert_eq!(reopened_revision.session().midi_clips().len(), 2);
    assert_eq!(
        reopened_revision.session().cell(track, source_scene),
        Some(hex_engine::PreparedSessionCellContent::Midi(source_clip))
    );
    assert_eq!(
        reopened_revision.session().cell(track, duplicate_scene),
        Some(hex_engine::PreparedSessionCellContent::Midi(duplicate_clip))
    );

    let frames = 24_001;
    let source_pcm = render_clip(expected_revision, source_clip, frames, &[frames]);
    let duplicate_pcm = render_clip(
        reopened_revision,
        duplicate_clip,
        frames,
        &[257, 1_024, 513, 2_047],
    );
    assert!(source_pcm.iter().any(|frame| frame[0].abs() > 0.0001));
    assert_eq!(duplicate_pcm, source_pcm);
}
