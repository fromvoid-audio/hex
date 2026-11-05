//! Authored Session MIDI history, persistence, and playback through the GPUI worker contract.
mod common;

use common::wait;
use hex_engine::playback::Playback;
use hex_engine::{
    LaunchQuantization, PlaybackAction, PreparedRevision, SessionPlaybackCommand,
    SessionPlaybackOwner, SessionRouteTarget,
};
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
        let path =
            std::env::temp_dir().join(format!("hex-session-workflow-{}-{id}", std::process::id()));
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

fn created(event: Event, expected: &'static str) -> (Event, Created) {
    let result = event.created.expect("edit should return a created ID");
    let matches = matches!(
        (expected, result),
        ("track", Created::Track(_))
            | ("scene", Created::Scene(_))
            | ("session clip", Created::SessionClip(_))
            | ("arrangement clip", Created::Clip(_))
    );
    assert!(matches, "unexpected {expected} creation result: {result:?}");
    // The value is Copy, so retain the full event for its canonical state and artifact.
    let result = event.created.expect("created result retained");
    (event, result)
}

fn add_note(service: &Service, clip: hex_project::ClipId, pitch: u8) -> hex_project::NoteId {
    add_note_at(service, clip, pitch, BeatTime::ZERO)
}

fn add_note_at(
    service: &Service,
    clip: hex_project::ClipId,
    pitch: u8,
    start: BeatTime,
) -> hex_project::NoteId {
    let event = edit(
        service,
        Command::AddMidiNote {
            clip,
            start,
            duration: BeatTime::from_quarters(1).expect("one-quarter gate"),
            pitch,
            channel: 0,
            velocity: 100,
        },
    );
    assert!(matches!(event.created, Some(Created::Note(_))));
    match event.created.expect("note ID") {
        Created::Note(id) => id,
        result => panic!("unexpected note creation result: {result:?}"),
    }
}

fn prepared(event: &Event) -> std::sync::Arc<PreparedRevision> {
    event
        .playback
        .as_ref()
        .and_then(Playback::prepared_revision)
        .expect("event playback retains its compiled revision")
        .clone()
}

fn render_session_scene(
    revision: std::sync::Arc<PreparedRevision>,
    scene: hex_project::SceneId,
    frames: usize,
    partitions: &[usize],
) -> Vec<[f32; 2]> {
    let mut playback = Playback::try_from_prepared_with_session(revision)
        .expect("prepared revision admits Session playback");
    playback
        .try_apply_action(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .expect("launch Session scene");
    let mut output = Vec::with_capacity(frames);
    let mut remaining = frames;
    let mut partition_index = 0;
    while remaining > 0 {
        let partition = partitions[partition_index % partitions.len()].min(remaining);
        let start = output.len();
        output.resize(start + partition, [0.0; 2]);
        playback
            .render(&mut output[start..start + partition])
            .expect("render mixed playback");
        remaining -= partition;
        partition_index += 1;
    }
    output
}

#[test]
fn session_midi_history_save_reopen_and_mixed_playback_workflow() {
    let directory = TempDir::new();
    let saved_path = directory.path().join("session-workflow.hex");
    let service = Service::new(48_000, 64, 0).expect("session worker");
    assert!(wait(&service).playback.is_some());

    let (track_event, result) = created(
        edit(
            &service,
            Command::InsertTrackWithDevice {
                name: "session instrument".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            },
        ),
        "track",
    );
    let track = match result {
        Created::Track(id) => id,
        _ => unreachable!("creation type checked above"),
    };
    assert_eq!(track_event.state.project.tracks()[0].id(), track);

    let arrangement_event = edit(
        &service,
        Command::InsertTrackWithDevice {
            name: "arrangement instrument".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        },
    );
    let arrangement_track = match arrangement_event.created.expect("Arrangement track") {
        Created::Track(id) => id,
        result => panic!("unexpected track result: {result:?}"),
    };
    let (arrangement_clip_event, result) = created(
        edit(
            &service,
            Command::PlaceMidiClip {
                track: arrangement_track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).expect("four-quarter clip"),
            },
        ),
        "arrangement clip",
    );
    let arrangement_clip = match result {
        Created::Clip(id) => id,
        _ => unreachable!("creation type checked above"),
    };
    let arrangement_note = add_note(&service, arrangement_clip, 48);

    let (scene_a_event, result) = created(
        edit(
            &service,
            Command::AddScene {
                name: "session a".into(),
            },
        ),
        "scene",
    );
    let scene_a = match result {
        Created::Scene(id) => id,
        _ => unreachable!("creation type checked above"),
    };
    let scene_b_event = edit(
        &service,
        Command::AddScene {
            name: "session b".into(),
        },
    );
    let scene_b = match scene_b_event.created.expect("second scene") {
        Created::Scene(id) => id,
        result => panic!("unexpected scene result: {result:?}"),
    };
    let scene_stop_event = edit(
        &service,
        Command::AddScene {
            name: "session stop".into(),
        },
    );
    let scene_stop = match scene_stop_event.created.expect("stop scene") {
        Created::Scene(id) => id,
        result => panic!("unexpected scene result: {result:?}"),
    };

    let (_clip_a_event, result) = created(
        edit(
            &service,
            Command::CreateSessionMidiClip {
                track,
                scene: scene_a,
                name: "short loop".into(),
                source_length: BeatTime::from_quarters(4).expect("source length"),
            },
        ),
        "session clip",
    );
    let clip_a = match result {
        Created::SessionClip(id) => id,
        _ => unreachable!("creation type checked above"),
    };
    let note_a = add_note(&service, clip_a, 60);
    let _loop_a = edit(
        &service,
        Command::SetSessionMidiClipSource {
            clip: clip_a,
            source_length: BeatTime::from_quarters(4).expect("source length"),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::from_quarters(2).expect("loop end"),
            looping: true,
        },
    );

    let (_clip_b_event, result) = created(
        edit(
            &service,
            Command::CreateSessionMidiClip {
                track,
                scene: scene_b,
                name: "long source".into(),
                source_length: BeatTime::from_quarters(8).expect("source length"),
            },
        ),
        "session clip",
    );
    let clip_b = match result {
        Created::SessionClip(id) => id,
        _ => unreachable!("creation type checked above"),
    };
    let note_b = add_note_at(
        &service,
        clip_b,
        67,
        BeatTime::from_quarters(1).expect("note inside selected source window"),
    );
    let _loop_b = edit(
        &service,
        Command::SetSessionMidiClipSource {
            clip: clip_b,
            source_length: BeatTime::from_quarters(8).expect("source length"),
            source_start: BeatTime::from_quarters(1).expect("source start"),
            loop_start: BeatTime::from_quarters(1).expect("loop start"),
            loop_end: BeatTime::from_quarters(5).expect("loop end"),
            looping: false,
        },
    );
    let stop = edit(
        &service,
        Command::SetSessionStopCell {
            track,
            scene: scene_stop,
        },
    );

    let authored = stop.state.project.as_ref().clone();
    let authored_revision = prepared(&stop);
    assert_eq!(authored_revision.session().scenes().len(), 3);
    assert_eq!(authored_revision.session().midi_clips().len(), 2);
    assert_eq!(
        authored.session_cell(track, scene_stop),
        Some(&SessionCellContent::Stop)
    );
    assert_eq!(authored.midi_note(note_a).unwrap().pitch(), 60);
    assert_eq!(authored.midi_note(note_b).unwrap().pitch(), 67);
    assert_eq!(authored.midi_note(arrangement_note).unwrap().pitch(), 48);
    assert_eq!(authored.session_midi_clip(clip_a).unwrap().id(), clip_a);
    assert_eq!(authored.session_midi_clip(clip_b).unwrap().id(), clip_b);
    assert_eq!(authored.scenes()[0].id(), scene_a);
    assert_eq!(authored.scenes()[1].id(), scene_b);
    assert_eq!(authored.scenes()[2].id(), scene_stop);
    assert_eq!(
        authored
            .session_midi_clip(clip_a)
            .unwrap()
            .source()
            .source_length(),
        BeatTime::from_quarters(4).unwrap()
    );
    assert!(
        authored
            .session_midi_clip(clip_a)
            .unwrap()
            .source()
            .looping()
    );
    assert_eq!(
        authored
            .session_midi_clip(clip_a)
            .unwrap()
            .source()
            .loop_end(),
        BeatTime::from_quarters(2).unwrap()
    );
    assert_eq!(
        authored
            .session_midi_clip(clip_b)
            .unwrap()
            .source()
            .source_length(),
        BeatTime::from_quarters(8).unwrap()
    );
    assert_eq!(
        authored
            .session_midi_clip(clip_b)
            .unwrap()
            .source()
            .source_start(),
        BeatTime::from_quarters(1).unwrap()
    );
    assert_eq!(
        authored
            .session_midi_clip(clip_b)
            .unwrap()
            .source()
            .loop_end(),
        BeatTime::from_quarters(5).unwrap()
    );
    assert!(
        !authored
            .session_midi_clip(clip_b)
            .unwrap()
            .source()
            .looping()
    );

    let cleared = edit(
        &service,
        Command::ClearSessionCell {
            track,
            scene: scene_b,
        },
    );
    assert!(cleared.playback.is_some());
    assert!(cleared.state.unsaved);
    assert_eq!(cleared.state.project.session_midi_clip(clip_b), None);
    assert!(
        cleared
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .unwrap()
            .session()
            .midi_clip(clip_b)
            .is_none()
    );
    service.submit(Request::Undo).expect("undo clear cell");
    let clear_undo = wait(&service);
    assert_eq!(clear_undo.state.project.as_ref(), &authored);
    assert!(clear_undo.state.unsaved);
    assert!(clear_undo.playback.is_some());
    assert_eq!(
        prepared(&clear_undo)
            .session()
            .midi_clip(clip_b)
            .unwrap()
            .id,
        clip_b
    );
    service.submit(Request::Redo).expect("redo clear cell");
    let clear_redo = wait(&service);
    assert_eq!(clear_redo.state.project.session_midi_clip(clip_b), None);
    assert!(clear_redo.playback.is_some());
    service
        .submit(Request::Undo)
        .expect("restore authored cell");
    let restored = wait(&service);
    assert_eq!(restored.state.project.as_ref(), &authored);

    let removed_scene = edit(&service, Command::RemoveScene(scene_stop));
    assert!(removed_scene.playback.is_some());
    assert!(
        removed_scene
            .state
            .project
            .scenes()
            .iter()
            .all(|scene| scene.id() != scene_stop)
    );
    service.submit(Request::Undo).expect("undo scene removal");
    let scene_undo = wait(&service);
    assert_eq!(scene_undo.state.project.as_ref(), &authored);
    assert!(scene_undo.state.unsaved);
    assert!(scene_undo.playback.is_some());
    service.submit(Request::Redo).expect("redo scene removal");
    let scene_redo = wait(&service);
    assert!(
        scene_redo
            .state
            .project
            .scenes()
            .iter()
            .all(|scene| scene.id() != scene_stop)
    );
    assert!(scene_redo.playback.is_some());
    service
        .submit(Request::Undo)
        .expect("restore scene before save");
    let pre_save = wait(&service);
    assert_eq!(pre_save.state.project.as_ref(), &authored);

    service
        .submit(Request::SaveNew(saved_path.clone()))
        .expect("save authored Session project");
    let saved = wait(&service);
    assert!(saved.save_complete);
    assert!(!saved.state.unsaved);
    assert_eq!(
        saved.state.document_path.as_deref(),
        Some(saved_path.as_path())
    );

    let rename = edit(
        &service,
        Command::RenameScene {
            scene: scene_b,
            name: "session b saved".into(),
        },
    );
    assert!(rename.state.unsaved);
    let expected_project = rename.state.project.as_ref().clone();
    let expected_revision = prepared(&rename);
    service
        .submit(Request::SaveCurrent)
        .expect("update saved project");
    let saved_current = wait(&service);
    assert!(saved_current.save_complete);
    assert!(!saved_current.state.unsaved);

    // Route and transport state is runtime-only. Build the same explicit launch
    // sequence on the pre-save artifact and the freshly reopened graph.
    let scene_a_frames = 120_001;
    let expected_scene_a_pcm = render_session_scene(
        expected_revision.clone(),
        scene_a,
        scene_a_frames,
        &[scene_a_frames],
    );
    assert!(
        expected_scene_a_pcm
            .iter()
            .any(|frame| frame[0].abs() > 0.0001)
    );
    assert!(
        expected_scene_a_pcm[100_000..]
            .iter()
            .any(|frame| frame[0].abs() > 0.0001),
        "the looping Session source continues after the Arrangement companion ends"
    );
    let scene_b_frames = 180_001;
    let expected_scene_b_pcm = render_session_scene(
        expected_revision.clone(),
        scene_b,
        scene_b_frames,
        &[scene_b_frames],
    );
    assert!(
        expected_scene_b_pcm
            .iter()
            .any(|frame| frame[0].abs() > 0.0001)
    );
    assert_ne!(
        &expected_scene_a_pcm[..2_048],
        &expected_scene_b_pcm[..2_048]
    );
    assert!(
        expected_scene_b_pcm[168_001..]
            .iter()
            .all(|frame| *frame == [0.0; 2]),
        "the one-shot and its Arrangement companion are finished"
    );

    let reopened_service = Service::new(48_000, 64, 0).expect("fresh session worker");
    wait(&reopened_service);
    reopened_service
        .submit(Request::Open(saved_path.clone()))
        .expect("open saved project in fresh service");
    let reopened = wait(&reopened_service);
    assert!(reopened.open_complete && reopened.project_replaced);
    assert!(!reopened.state.unsaved);
    assert_eq!(
        reopened.state.document_path.as_deref(),
        Some(saved_path.as_path())
    );
    assert_eq!(reopened.state.project.as_ref(), &expected_project);
    assert_eq!(reopened.state.project.scenes(), expected_project.scenes());
    assert_eq!(
        reopened
            .state
            .project
            .session_midi_clip(clip_a)
            .unwrap()
            .source(),
        expected_project.session_midi_clip(clip_a).unwrap().source()
    );
    assert_eq!(
        reopened
            .state
            .project
            .session_midi_clip(clip_b)
            .unwrap()
            .source(),
        expected_project.session_midi_clip(clip_b).unwrap().source()
    );
    assert_eq!(
        reopened.state.project.midi_note(note_a).unwrap().id(),
        note_a
    );
    assert_eq!(
        reopened.state.project.midi_note(note_b).unwrap().id(),
        note_b
    );
    assert_eq!(
        reopened
            .state
            .project
            .midi_note(arrangement_note)
            .unwrap()
            .id(),
        arrangement_note
    );
    let reopened_revision = prepared(&reopened);
    assert_eq!(reopened_revision.session().scenes().len(), 3);
    assert_eq!(reopened_revision.session().midi_clips().len(), 2);
    assert_eq!(
        reopened_revision.session().cell(track, scene_a),
        Some(hex_engine::PreparedSessionCellContent::Midi(clip_a))
    );
    assert_eq!(
        reopened_revision.session().cell(track, scene_b),
        Some(hex_engine::PreparedSessionCellContent::Midi(clip_b))
    );
    assert_eq!(
        reopened_revision.session().cell(track, scene_stop),
        Some(hex_engine::PreparedSessionCellContent::Stop)
    );
    assert_eq!(
        render_session_scene(
            reopened_revision.clone(),
            scene_a,
            scene_a_frames,
            &[257, 1_024, 513, 4_096, 2_047]
        ),
        expected_scene_a_pcm
    );
    assert_eq!(
        render_session_scene(
            reopened_revision.clone(),
            scene_b,
            scene_b_frames,
            &[1_001, 127, 4_093, 512, 2_048]
        ),
        expected_scene_b_pcm
    );

    // A canceled quantized launch leaves the active route and its rendered output
    // unchanged. This exercises the public runtime action contract, not internals.
    let mut canceled = Playback::try_from_prepared_with_session(reopened_revision.clone())
        .expect("reopened Session runtime");
    let mut baseline = Playback::try_from_prepared_with_session(reopened_revision)
        .expect("comparison Session runtime");
    let launch = PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchScene {
        scene: scene_a,
        quantization: LaunchQuantization::Immediate,
    });
    canceled
        .try_apply_action(launch)
        .expect("start Session route");
    baseline
        .try_apply_action(launch)
        .expect("start comparison route");
    let mut warmup_a = [[0.0; 2]; 257];
    let mut warmup_b = [[0.0; 2]; 257];
    canceled
        .render(&mut warmup_a)
        .expect("advance canceled runtime");
    baseline
        .render(&mut warmup_b)
        .expect("advance baseline runtime");
    assert_eq!(warmup_a, warmup_b);
    let route_index = (0..canceled.session_route_count())
        .find(|&index| canceled.session_route_status(index).unwrap().track == track)
        .expect("Session instrument route");
    assert_eq!(
        canceled.session_route_status(route_index).unwrap().owner,
        SessionPlaybackOwner::Session { clip: clip_a }
    );
    let arrangement_route = (0..canceled.session_route_count())
        .find(|&index| canceled.session_route_status(index).unwrap().track == arrangement_track)
        .expect("Arrangement companion route");
    assert_eq!(
        canceled
            .session_route_status(arrangement_route)
            .unwrap()
            .owner,
        SessionPlaybackOwner::Arrangement
    );
    canceled
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchScene {
                scene: scene_b,
                quantization: LaunchQuantization::FourQuarter,
            },
        ))
        .expect("schedule quantized scene");
    let pending = canceled.session_route_status(route_index).unwrap();
    let pending_status = pending.pending.unwrap();
    assert_eq!(
        pending.owner,
        SessionPlaybackOwner::Session { clip: clip_a }
    );
    assert_eq!(
        pending_status.target,
        SessionRouteTarget::Session { clip: clip_b }
    );
    let pending_frame = pending_status.performance_frame;
    canceled
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::CancelPending { track: Some(track) },
        ))
        .expect("cancel pending scene");
    let canceled_route = canceled.session_route_status(route_index).unwrap();
    assert_eq!(
        canceled_route.owner,
        SessionPlaybackOwner::Session { clip: clip_a }
    );
    assert!(canceled_route.pending.is_none());
    for _ in 0..32 {
        if canceled.performance_frame() > pending_frame {
            break;
        }
        let before = canceled.performance_frame();
        let mut canceled_pcm = [[0.0; 2]; 4_096];
        let mut baseline_pcm = [[0.0; 2]; 4_096];
        canceled
            .render(&mut canceled_pcm)
            .expect("render canceled launch");
        baseline
            .render(&mut baseline_pcm)
            .expect("render no launch");
        assert_eq!(canceled_pcm, baseline_pcm);
        assert!(canceled.performance_frame() > before);
    }
    assert!(canceled.performance_frame() > pending_frame);
    let after_deadline = canceled.session_route_status(route_index).unwrap();
    assert_eq!(
        after_deadline.owner,
        SessionPlaybackOwner::Session { clip: clip_a }
    );
    assert!(after_deadline.pending.is_none());

    assert_eq!(
        arrangement_clip_event
            .state
            .project
            .midi_clip(arrangement_clip)
            .unwrap()
            .id(),
        arrangement_clip
    );
    assert_eq!(scene_a_event.state.project.scenes()[0].id(), scene_a);
}
