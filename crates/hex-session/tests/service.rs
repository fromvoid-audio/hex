use hex_engine::{
    Frame, LaunchQuantization, PlaybackAction, PreparedRevision, SessionPlaybackCommand,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{Command, Created, Editor, Project};
use hex_session::service::{Event, Request, Service, SubmitError};
use std::{
    fs, thread,
    time::{Duration, Instant},
};

fn wav_mono_pcm16(samples: &[i16]) -> Vec<u8> {
    let mut data = Vec::new();
    for sample in samples {
        data.extend_from_slice(&sample.to_le_bytes());
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36_u32 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes
}

fn next_event(service: &Service) -> Event {
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("worker event did not arrive: {error}"),
        }
    }
}

fn prepared(event: &Event) -> std::sync::Arc<PreparedRevision> {
    event
        .playback
        .as_ref()
        .and_then(Playback::prepared_revision)
        .expect("event carries prepared playback")
        .clone()
}

fn render_session_audio(
    revision: std::sync::Arc<PreparedRevision>,
    clip: hex_project::ClipId,
    partitions: &[usize],
) -> Vec<Frame> {
    let frame_count = partitions.iter().sum();
    let mut playback = Playback::try_from_prepared_with_session(revision)
        .expect("prepared revision admits Session playback");
    playback
        .try_apply_action(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .expect("launch Session audio clip");
    let mut rendered = Vec::with_capacity(frame_count);
    for &partition in partitions {
        let start = rendered.len();
        rendered.resize(start + partition, [0.0; 2]);
        playback
            .render(&mut rendered[start..start + partition])
            .expect("render Session audio clip");
    }
    rendered
}

fn expect_edited_audio_pcm(event: &Event, clip: hex_project::ClipId) {
    assert_eq!(
        render_session_audio(prepared(event), clip, &[1, 2, 1, 1]),
        [
            [-0.25, -0.25],
            [0.5, 0.5],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0]
        ]
    );
}

#[test]
fn import_event_contains_cached_waveform_and_compiled_audio() {
    let service = Service::new(48_000, 16, 32).expect("service");
    let initial = next_event(&service);
    assert_eq!(initial.state.revision, 0);
    assert!(!initial.state.unsaved);
    assert!(!initial.save_complete);
    assert!(initial.playback.is_some());

    let path = std::env::temp_dir().join(format!(
        "hex-session-service-{}-{}.wav",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::write(&path, wav_mono_pcm16(&[i16::MIN, 0, 16_384])).expect("write fixture");
    service
        .submit(Request::Import(path.clone()))
        .expect("submit import");
    let imported = next_event(&service);
    let _ = fs::remove_file(&path);

    assert_eq!(imported.state.revision, 1);
    assert!(imported.state.unsaved);
    assert!(imported.error.is_none());
    assert_eq!(imported.state.project.tracks().len(), 1);
    assert_eq!(imported.state.project.tracks()[0].clips().len(), 1);
    let clip = &imported.state.project.tracks()[0].clips()[0];
    assert_eq!(clip.start(), 0);
    assert_eq!(clip.duration(), 3);
    assert_eq!(imported.state.waveforms.len(), 1);
    let mut playback = imported.playback.expect("compiled playback");
    playback.apply(PlaybackCommand::Play);
    let mut frames: [Frame; 3] = [[0.0; 2]; 3];
    playback.render(&mut frames).expect("render");
    assert_eq!(frames, [[-1.0, -1.0], [0.0, 0.0], [0.5, 0.5]]);
}

#[test]
fn worker_import_to_existing_hybrid_track_keeps_audio_and_midi_through_save_open() {
    let directory = TempDir::new();
    let service = Service::new(48_000, 16, 32).expect("service");
    let _ = next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "hybrid lane".into(),
        }))
        .expect("add hybrid track");
    let track = match next_event(&service).created.expect("track") {
        Created::Track(track) => track,
        other => panic!("unexpected track result: {other:?}"),
    };
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        }))
        .expect("add arrangement midi clip");
    let midi_clip = match next_event(&service).created {
        Some(Created::Clip(clip)) => clip,
        other => panic!("missing midi clip result: {other:?}"),
    };
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip: midi_clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .expect("add arrangement midi note");
    let noted = next_event(&service);
    assert!(noted.error.is_none(), "{:?}", noted.error);
    assert_eq!(
        noted
            .state
            .project
            .track(track)
            .expect("hybrid track")
            .midi_clips()[0]
            .notes()
            .len(),
        1
    );
    let source = directory.0.join("Sample.WAV");
    fs::write(&source, wav_mono_pcm16(&[10_000, -10_000, 5_000])).expect("write wav fixture");
    service
        .submit(Request::ImportToTrackWithOverwrite {
            path: source,
            track,
            start: 17,
        })
        .expect("import on selected track");
    let imported = next_event(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let expected_project = imported.state.project.clone();
    let lane = imported.state.project.track(track).expect("hybrid track");
    assert_eq!(
        lane.clips().len(),
        1,
        "status={:?}, revision={}, tracks={:?}",
        imported.status,
        imported.state.revision,
        imported.state.project.tracks()
    );
    assert_eq!(lane.clips()[0].start(), 17);
    assert_eq!(lane.midi_clips().len(), 2, "import cuts the MIDI victim");
    assert!(lane.midi_clips().iter().all(|clip| clip.notes().len() == 1));
    assert_ne!(lane.midi_clips()[0].id(), lane.midi_clips()[1].id());

    let project_path = directory.0.join("hybrid.hex");
    service
        .submit(Request::SaveNew(project_path.clone()))
        .expect("save hybrid project");
    let saved = next_event(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    let reopened_service = Service::new(48_000, 16, 32).expect("reopen service");
    let _ = next_event(&reopened_service);
    reopened_service
        .submit(Request::Open(project_path))
        .expect("open hybrid project");
    let reopened = next_event(&reopened_service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    let lane = reopened.state.project.track(track).expect("reopened track");
    assert_eq!(lane.clips().len(), 1);
    assert_eq!(lane.clips()[0].start(), 17);
    assert_eq!(lane.midi_clips().len(), 2);
    assert!(lane.midi_clips().iter().all(|clip| clip.notes().len() == 1));
    assert_eq!(reopened.state.project, expected_project);
    let mut playback = reopened.playback.expect("reopened playback");
    playback.apply(PlaybackCommand::Play);
    let mut frames: [Frame; 256] = [[0.0; 2]; 256];
    playback
        .render(&mut frames)
        .expect("render reopened project");
    assert!(frames.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
}

#[test]
fn worker_imports_audio_to_a_session_cell_as_a_one_shot_and_preserves_source_name() {
    let service = Service::new(48_000, 16, 32).expect("service");
    let _ = next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "audio lane".into(),
        }))
        .expect("add audio track");
    let track_event = next_event(&service);
    let track = match track_event.created.expect("track id") {
        Created::Track(id) => id,
        result => panic!("unexpected track result: {result:?}"),
    };
    service
        .submit(Request::Edit(Command::AddScene {
            name: "verse".into(),
        }))
        .expect("add scene");
    let scene_event = next_event(&service);
    let scene = match scene_event.created.expect("scene id") {
        Created::Scene(id) => id,
        result => panic!("unexpected scene result: {result:?}"),
    };
    service
        .submit(Request::Edit(Command::AddScene {
            name: "empty target".into(),
        }))
        .expect("add empty destination scene");
    let empty_scene_event = next_event(&service);
    let empty_scene = match empty_scene_event.created.expect("empty scene id") {
        Created::Scene(id) => id,
        result => panic!("unexpected scene result: {result:?}"),
    };
    let directory =
        std::env::temp_dir().join(format!("hex-session-audio-import-{}", std::process::id()));
    fs::create_dir_all(&directory).expect("create fixture directory");
    let source = directory.join("CasePreservedAudio.wav");
    fs::write(&source, wav_mono_pcm16(&[8_192, -8_192, 16_384, 0])).expect("write source wav");

    service
        .submit(Request::ImportToSession {
            path: source.clone(),
            track,
            scene,
        })
        .expect("import Session source");
    let imported = next_event(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    assert!(imported.state.unsaved);
    assert_eq!(imported.state.project.assets().count(), 1);
    let Some(hex_project::SessionCellContent::Audio(clip)) =
        imported.state.project.session_cell(track, scene)
    else {
        panic!("import must create an audio Session cell");
    };
    assert_eq!(clip.name(), "CasePreservedAudio");
    assert_eq!((clip.source_start(), clip.source_end()), (0, 4));
    assert_eq!(imported.state.waveforms[&clip.asset()].source_frames, 4);
    let clip_id = clip.id();
    let asset_id = clip.asset();

    let before_occupied = imported.state.project.clone();
    service
        .submit(Request::ImportToSession {
            path: source.clone(),
            track,
            scene,
        })
        .expect("submit occupied-cell import");
    let occupied = next_event(&service);
    assert!(occupied.error.is_some());
    assert!(occupied.playback.is_none());
    assert_eq!(occupied.state.revision, imported.state.revision);
    assert_eq!(occupied.state.project.as_ref(), before_occupied.as_ref());
    assert_eq!(occupied.state.can_undo, imported.state.can_undo);
    assert_eq!(occupied.state.can_redo, imported.state.can_redo);
    assert_eq!(occupied.state.document_path, imported.state.document_path);
    assert_eq!(occupied.state.unsaved, imported.state.unsaved);

    let missing_path = directory.join("missing-audio.wav");
    service
        .submit(Request::ImportToSession {
            path: missing_path,
            track,
            scene: empty_scene,
        })
        .expect("submit missing-media import");
    let missing = next_event(&service);
    assert!(missing.error.is_some());
    assert!(missing.playback.is_none());
    assert_eq!(missing.state.revision, imported.state.revision);
    assert_eq!(missing.state.project.as_ref(), before_occupied.as_ref());
    assert_eq!(missing.state.can_undo, imported.state.can_undo);
    assert_eq!(missing.state.can_redo, imported.state.can_redo);
    assert_eq!(missing.state.document_path, imported.state.document_path);
    assert_eq!(missing.state.unsaved, imported.state.unsaved);
    service
        .submit(Request::RestorePlayback { generation: 11 })
        .expect("restore graph after refused imports");
    let after_refusals = next_event(&service);
    assert!(after_refusals.error.is_none());
    assert_eq!(after_refusals.restore_complete, Some(11));
    assert_eq!(
        render_session_audio(prepared(&after_refusals), clip_id, &[2, 1, 2]),
        [
            [0.25, 0.25],
            [-0.25, -0.25],
            [0.5, 0.5],
            [0.0, 0.0],
            [0.0, 0.0]
        ]
    );

    service.submit(Request::Undo).expect("undo Session import");
    let undo_import = next_event(&service);
    assert!(undo_import.error.is_none());
    assert!(undo_import.state.can_redo);
    assert!(
        undo_import
            .state
            .project
            .session_cell(track, scene)
            .is_none()
    );
    assert_eq!(undo_import.state.project.assets().count(), 0);
    service.submit(Request::Redo).expect("redo Session import");
    let redo_import = next_event(&service);
    assert!(redo_import.error.is_none());
    assert_eq!(redo_import.state.project.as_ref(), before_occupied.as_ref());
    assert_eq!(
        redo_import
            .state
            .project
            .session_audio_clip(clip_id)
            .map(|clip| (clip.asset(), clip.source_start(), clip.source_end())),
        Some((asset_id, 0, 4))
    );

    let project_path = directory.join("session-audio.hex");
    service
        .submit(Request::SaveNew(project_path.clone()))
        .expect("save original Session audio project");
    let saved_new = next_event(&service);
    assert!(saved_new.error.is_none(), "{:?}", saved_new.error);
    assert!(saved_new.save_complete);
    assert!(!saved_new.state.unsaved);
    assert_eq!(
        saved_new.state.document_path.as_deref(),
        Some(project_path.as_path())
    );

    service
        .submit(Request::Edit(Command::SetSessionAudioClipSource {
            clip: clip_id,
            asset: asset_id,
            source_start: 1,
            source_end: 3,
        }))
        .expect("edit Session source window");
    let edited = next_event(&service);
    assert!(edited.error.is_none(), "{:?}", edited.error);
    assert!(edited.state.unsaved);
    assert_eq!(
        edited
            .state
            .project
            .session_audio_clip(clip_id)
            .map(|clip| (clip.source_start(), clip.source_end())),
        Some((1, 3))
    );
    let edited_project = edited.state.project.clone();
    service.submit(Request::Undo).expect("undo source edit");
    let undo_source = next_event(&service);
    assert!(undo_source.error.is_none());
    assert_eq!(
        undo_source
            .state
            .project
            .session_audio_clip(clip_id)
            .map(|clip| (clip.source_start(), clip.source_end())),
        Some((0, 4))
    );
    assert!(undo_source.state.unsaved);
    service.submit(Request::Redo).expect("redo source edit");
    let edited = next_event(&service);
    assert!(edited.error.is_none());
    assert_eq!(edited.state.project.as_ref(), edited_project.as_ref());
    assert!(edited.state.unsaved);

    service
        .submit(Request::SaveCurrent)
        .expect("save current Session audio project");
    let saved_current = next_event(&service);
    assert!(saved_current.error.is_none(), "{:?}", saved_current.error);
    assert!(saved_current.save_complete);
    assert!(!saved_current.state.unsaved);
    assert_eq!(
        saved_current.state.project.as_ref(),
        edited_project.as_ref()
    );
    assert_eq!(
        saved_current.state.document_path.as_deref(),
        Some(project_path.as_path())
    );

    let fresh_service = Service::new(48_000, 16, 32).expect("fresh service");
    let _ = next_event(&fresh_service);
    fresh_service
        .submit(Request::Open(project_path.clone()))
        .expect("reopen Session audio project");
    let reopened = next_event(&fresh_service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert_eq!(reopened.state.project.as_ref(), edited_project.as_ref());
    assert_eq!(reopened.state.project.sample_rate(), 48_000);
    let Some(hex_project::SessionCellContent::Audio(reopened_clip)) =
        reopened.state.project.session_cell(track, scene)
    else {
        panic!("reopened project must retain its audio Session cell");
    };
    assert_eq!(reopened_clip.name(), "CasePreservedAudio");
    assert_eq!(
        (reopened_clip.source_start(), reopened_clip.source_end()),
        (1, 3)
    );
    let reopened_asset = reopened
        .state
        .project
        .asset(reopened_clip.asset())
        .expect("asset");
    assert_eq!(reopened_asset.path(), source.as_path());
    assert_eq!(
        reopened.state.waveforms[&reopened_clip.asset()].source_frames,
        4
    );
    assert_eq!(reopened_clip.id(), clip_id);
    assert_eq!(reopened_clip.asset(), asset_id);
    expect_edited_audio_pcm(&reopened, clip_id);

    let constrained = Service::new(48_000, 16, 5).expect("constrained service");
    let _ = next_event(&constrained);
    constrained
        .submit(Request::Edit(Command::AddTrack {
            name: "audio lane".into(),
        }))
        .expect("add constrained audio track");
    let constrained_track = match next_event(&constrained)
        .created
        .expect("constrained track id")
    {
        Created::Track(id) => id,
        result => panic!("unexpected constrained track result: {result:?}"),
    };
    constrained
        .submit(Request::Edit(Command::AddScene {
            name: "constrained target".into(),
        }))
        .expect("add constrained scene");
    let constrained_scene = match next_event(&constrained)
        .created
        .expect("constrained scene id")
    {
        Created::Scene(id) => id,
        result => panic!("unexpected constrained scene result: {result:?}"),
    };
    constrained
        .submit(Request::Edit(Command::AddScene {
            name: "over-budget target".into(),
        }))
        .expect("add over-budget destination");
    let over_budget_scene = match next_event(&constrained).created.expect("scene id") {
        Created::Scene(id) => id,
        result => panic!("unexpected destination scene result: {result:?}"),
    };
    let resident_source = directory.join("resident.wav");
    fs::write(&resident_source, wav_mono_pcm16(&[8_192, -8_192])).expect("write resident wav");
    constrained
        .submit(Request::ImportToSession {
            path: resident_source,
            track: constrained_track,
            scene: constrained_scene,
        })
        .expect("import resident source under budget");
    let resident = next_event(&constrained);
    assert!(resident.error.is_none(), "{:?}", resident.error);
    let Some(hex_project::SessionCellContent::Audio(resident_clip)) = resident
        .state
        .project
        .session_cell(constrained_track, constrained_scene)
    else {
        panic!("resident Session audio clip expected");
    };
    let resident_clip_id = resident_clip.id();
    let resident_project = resident.state.project.clone();
    assert_eq!(resident.state.project.assets().count(), 1);

    let budget_baseline = {
        constrained
            .submit(Request::RestorePlayback { generation: 19 })
            .expect("restore constrained graph");
        next_event(&constrained)
    };
    let budget_revision = budget_baseline.state.revision;
    let budget_project = budget_baseline.state.project.clone();
    assert_eq!(budget_project.as_ref(), resident_project.as_ref());
    constrained
        .submit(Request::ImportToSession {
            path: source.clone(),
            track: constrained_track,
            scene: over_budget_scene,
        })
        .expect("submit over-budget import");
    let budget_refusal = next_event(&constrained);
    assert!(budget_refusal.error.is_some());
    assert!(budget_refusal.playback.is_none());
    assert_eq!(budget_refusal.state.revision, budget_revision);
    assert_eq!(
        budget_refusal.state.project.as_ref(),
        budget_project.as_ref()
    );
    assert_eq!(
        budget_refusal.state.can_undo,
        budget_baseline.state.can_undo
    );
    assert_eq!(
        budget_refusal.state.can_redo,
        budget_baseline.state.can_redo
    );
    assert_eq!(
        budget_refusal.state.document_path,
        budget_baseline.state.document_path
    );
    assert_eq!(budget_refusal.state.unsaved, budget_baseline.state.unsaved);
    assert_eq!(budget_refusal.state.project.assets().count(), 1);
    constrained
        .submit(Request::RestorePlayback { generation: 20 })
        .expect("restore graph after over-budget import");
    let retained_after_budget = next_event(&constrained);
    assert!(retained_after_budget.error.is_none());
    assert_eq!(
        render_session_audio(
            prepared(&retained_after_budget),
            resident_clip_id,
            &[1, 2, 2]
        ),
        [
            [0.25, 0.25],
            [-0.25, -0.25],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 0.0]
        ]
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn restore_returns_current_paused_graph_without_mutating_project_state() {
    let service = Service::new(48_000, 16, 32).expect("service");
    let initial = next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack { name: "one".into() }))
        .expect("submit edit");
    let edited = next_event(&service);
    assert_eq!(edited.state.revision, 1);
    assert!(edited.state.unsaved);
    let path = edited.state.document_path.clone();

    service
        .submit(Request::RestorePlayback { generation: 73 })
        .expect("submit restore");
    let restored = next_event(&service);

    assert_eq!(restored.restore_complete, Some(73));
    assert!(restored.request_complete);
    assert!(restored.error.is_none());
    assert_eq!(restored.state.revision, edited.state.revision);
    assert!(restored.state.unsaved);
    assert_eq!(restored.state.document_path, path);
    assert_eq!(restored.state.project, edited.state.project);
    assert!(!restored.playback.expect("restored graph").playing());

    service
        .submit(Request::Edit(Command::AddTrack { name: "two".into() }))
        .expect("submit later edit");
    let later = next_event(&service);
    assert_eq!(later.state.revision, 2);
    assert_eq!(later.state.project.tracks().len(), 2);
    assert_eq!(initial.state.revision, 0);
}

#[test]
fn split_edit_event_returns_the_exact_right_clip_id() {
    let service = Service::new(48_000, 16, 32).expect("service");
    let _ = next_event(&service);
    let path = std::env::temp_dir().join(format!(
        "hex-session-split-{}-{}.wav",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::write(&path, wav_mono_pcm16(&[1000, 2000, 3000, 4000])).expect("write fixture");
    service
        .submit(Request::Import(path.clone()))
        .expect("submit import");
    let imported = next_event(&service);
    let _ = fs::remove_file(&path);
    let original = imported.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(Command::SplitClip {
            clip: original,
            frame: 2,
        }))
        .expect("submit split");
    let split = next_event(&service);
    assert!(split.error.is_none());
    assert!(split.request_complete);
    let right = match split.created {
        Some(Created::Clip(id)) => id,
        other => panic!("split event did not return the right clip ID: {other:?}"),
    };
    let clips = split.state.project.tracks()[0].clips();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].id(), original);
    assert_eq!(clips[1].id(), right);
    assert_eq!(clips[1].start(), 2);
}

#[test]
fn midi_split_edit_compiles_and_undo_redo_keep_the_exact_right_id() {
    let service = Service::new(48_000, 16, 32).expect("service");
    let _ = next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "hybrid".into(),
        }))
        .expect("add track");
    let track = match next_event(&service).created {
        Some(Created::Track(track)) => track,
        other => panic!("missing track result: {other:?}"),
    };
    let length = hex_project::BeatTime::from_quarters(2).expect("two beats");
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length,
        }))
        .expect("place midi clip");
    let clip = match next_event(&service).created {
        Some(Created::Clip(clip)) => clip,
        other => panic!("missing midi clip result: {other:?}"),
    };
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: length,
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .expect("add midi note");
    let _ = next_event(&service);
    let split_beat = hex_project::BeatTime::ONE;
    service
        .submit(Request::Edit(Command::SplitMidiClip {
            clip,
            beat: split_beat,
        }))
        .expect("submit midi split");
    let split = next_event(&service);
    assert!(split.error.is_none(), "{:?}", split.error);
    let right = match split.created {
        Some(Created::Clip(id)) => id,
        other => panic!("split event did not return the right clip ID: {other:?}"),
    };
    let clips = split
        .state
        .project
        .track(track)
        .expect("hybrid track")
        .midi_clips();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].id(), clip);
    assert_eq!(clips[1].id(), right);
    assert_eq!(clips[1].source_start(), hex_project::BeatTime::ZERO);
    assert!(!clips[1].looping());
    assert_eq!(clips[0].notes().len(), 1);
    assert_eq!(clips[1].notes().len(), 1);
    assert_eq!(clips[1].notes()[0].start(), hex_project::BeatTime::ZERO);
    assert_eq!(clips[1].notes()[0].duration(), hex_project::BeatTime::ONE);
    assert_ne!(clips[0].notes()[0].id(), clips[1].notes()[0].id());

    service.submit(Request::Undo).expect("undo split");
    let undone = next_event(&service);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(
        undone
            .state
            .project
            .track(track)
            .expect("hybrid track")
            .midi_clips()
            .len(),
        1
    );
    service.submit(Request::Redo).expect("redo split");
    let redone = next_event(&service);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(
        redone
            .state
            .project
            .track(track)
            .expect("hybrid track")
            .midi_clips()[1]
            .id(),
        right
    );
}

#[test]
fn dirty_state_tracks_edits_saves_and_undo_conservatively() {
    let directory = TempDir::new();
    let service = Service::new(48_000, 16, 32).expect("service");
    let initial = next_event(&service);
    assert!(!initial.state.unsaved);

    service
        .submit(Request::Edit(Command::AddTrack { name: "one".into() }))
        .expect("submit edit");
    let edited = next_event(&service);
    assert!(edited.state.unsaved);

    service.submit(Request::Undo).expect("submit undo");
    let undone = next_event(&service);
    assert!(undone.state.unsaved, "undo remains conservatively dirty");
    assert!(undone.state.project.tracks().is_empty());

    let blocked_path = directory.path().join("existing.hex");
    fs::write(&blocked_path, b"keep").expect("write existing file");
    service
        .submit(Request::SaveNew(blocked_path))
        .expect("submit failing save");
    let failed_save = next_event(&service);
    assert!(failed_save.error.is_some());
    assert!(failed_save.state.unsaved);

    let saved_path = directory.path().join("saved.hex");
    service
        .submit(Request::SaveNew(saved_path))
        .expect("submit save");
    let saved = next_event(&service);
    assert!(saved.error.is_none());
    assert!(saved.save_complete);
    assert!(!saved.state.unsaved);
}

#[test]
fn open_restores_ids_and_relative_media_with_fresh_history() {
    let directory = TempDir::new();
    let copied_directory = TempDir::new();
    fs::write(
        directory.path().join("Audio-MiXeD.wav"),
        wav_mono_pcm16(&[8192, -8192]),
    )
    .expect("write source");
    fs::write(
        directory.path().join("Unused-MiXeD.wav"),
        wav_mono_pcm16(&[4096]),
    )
    .expect("write unreferenced source");
    let project_path = directory.path().join("Set.hex");
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let ids = match editor
        .execute(Command::ImportAudioTrack {
            name: "Bass MiXeD".into(),
            path: "Audio-MiXeD.wav".into(),
            sample_rate: 48_000,
            frames: 2,
        })
        .expect("create project content")
    {
        Created::Imported { track, asset, clip } => (track, asset, clip),
        other => panic!("unexpected result: {other:?}"),
    };
    let unreferenced_asset = match editor
        .execute(Command::AddAsset {
            path: "Unused-MiXeD.wav".into(),
            sample_rate: 48_000,
            frames: 1,
        })
        .expect("add unreferenced asset")
    {
        Created::Asset(asset) => asset,
        other => panic!("unexpected result: {other:?}"),
    };
    hex_storage::save_new(&project_path, editor.project()).expect("save fixture project");

    let service = Service::new(48_000, 16, 32).expect("service");
    let _ = next_event(&service);
    service
        .submit(Request::Open(project_path.clone()))
        .expect("submit open");
    let opened = next_event(&service);

    assert!(opened.error.is_none());
    assert!(opened.project_replaced);
    assert!(opened.open_complete);
    assert!(opened.playback.is_some());
    assert!(!opened.state.unsaved);
    assert_eq!(
        opened.state.document_path.as_deref(),
        Some(project_path.as_path())
    );
    assert!(!opened.state.can_undo);
    assert!(!opened.state.can_redo);
    let (track_id, asset_id, clip_id) = ids;
    assert_eq!(opened.state.project.tracks()[0].id(), track_id);
    assert_eq!(opened.state.project.tracks()[0].name(), "Bass MiXeD");
    assert_eq!(opened.state.project.tracks()[0].clips()[0].id(), clip_id);
    assert_eq!(
        opened.state.project.asset(asset_id).expect("asset").path(),
        Path::new("Audio-MiXeD.wav")
    );
    assert_eq!(opened.state.waveforms.len(), 2);
    let mut playback = opened.playback.expect("compiled graph");
    playback.apply(PlaybackCommand::Play);
    let mut frames = [[0.0; 2]; 2];
    playback.render(&mut frames).expect("render loaded media");
    assert_eq!(frames, [[0.25, 0.25], [-0.25, -0.25]]);

    let blocked_path = copied_directory.path().join("Existing.hex");
    fs::write(&blocked_path, b"preserve existing project").expect("create target");
    service
        .submit(Request::SaveNew(blocked_path.clone()))
        .expect("submit refused save");
    let refused = next_event(&service);
    assert!(refused.error.is_some());
    assert!(refused.save_complete);
    assert!(refused.playback.is_none());
    assert_eq!(refused.state.revision, opened.state.revision);
    assert_eq!(
        refused.state.document_path.as_deref(),
        Some(project_path.as_path())
    );
    assert_eq!(
        fs::read(&blocked_path).expect("read existing target"),
        b"preserve existing project"
    );
    assert_eq!(
        refused.state.project.asset(asset_id).expect("asset").path(),
        Path::new("Audio-MiXeD.wav")
    );

    let copied_path = copied_directory.path().join("Copied.hex");
    service
        .submit(Request::SaveNew(copied_path.clone()))
        .expect("submit save as");
    let saved = next_event(&service);
    assert!(saved.error.is_none());
    assert!(saved.save_complete);
    assert!(!saved.state.unsaved);
    assert!(saved.playback.is_none());
    assert_eq!(saved.state.revision, opened.state.revision);
    assert_eq!(
        saved.state.document_path.as_deref(),
        Some(copied_path.as_path())
    );
    assert_eq!(
        saved
            .state
            .project
            .asset(unreferenced_asset)
            .expect("unused asset")
            .path(),
        Path::new("Unused-MiXeD.wav")
    );

    service
        .submit(Request::SaveCurrent)
        .expect("save copied project in place");
    let resaved = next_event(&service);
    assert!(resaved.error.is_none(), "{:?}", resaved.error);
    assert!(resaved.save_complete);
    assert!(!resaved.state.unsaved);
    assert_eq!(resaved.state.revision, opened.state.revision);
    let recovery = hex_storage::load_recovery(&copied_path).expect("previous generation");
    assert_eq!(recovery.project.tracks(), saved.state.project.tracks());
    assert_eq!(
        recovery
            .project
            .asset(asset_id)
            .expect("recovered asset")
            .path(),
        directory.path().join("Audio-MiXeD.wav")
    );

    let reopened = Service::new(48_000, 16, 32).expect("reopen service");
    let _ = next_event(&reopened);
    reopened
        .submit(Request::Open(copied_path))
        .expect("submit copied project open");
    let copied = next_event(&reopened);
    assert!(copied.error.is_none());
    assert!(copied.project_replaced);
    assert_eq!(copied.state.project.tracks()[0].id(), track_id);
    assert_eq!(copied.state.project.tracks()[0].clips()[0].id(), clip_id);
    assert_eq!(copied.state.project.assets().count(), 2);
    assert_eq!(copied.state.waveforms.len(), 2);
    assert_eq!(
        copied.state.project.asset(asset_id).expect("asset").path(),
        directory.path().join("Audio-MiXeD.wav")
    );
    assert_eq!(
        copied
            .state
            .project
            .asset(unreferenced_asset)
            .expect("unreferenced asset")
            .path(),
        directory.path().join("Unused-MiXeD.wav")
    );
}

#[test]
fn failed_open_keeps_active_project_and_graph_revision() {
    let directory = TempDir::new();
    let active = Service::new(48_000, 16, 32).expect("service");
    let _ = next_event(&active);
    active
        .submit(Request::Edit(Command::AddTrack {
            name: "keep me".into(),
        }))
        .expect("submit active edit");
    let edited = next_event(&active);
    let revision = edited.state.revision;
    assert!(edited.playback.is_some());

    // Missing ordinary media opens offline instead (see missing_media tests).
    for (name, source_name, frames, sample_rate, create_source) in [
        ("mismatch.hex", "short.wav", 3_u64, 48_000_u32, true),
        ("rate.hex", "Unused.wav", 2_u64, 44_100_u32, false),
    ] {
        if create_source {
            fs::write(
                directory.path().join(source_name),
                wav_mono_pcm16(&[100, 200]),
            )
            .expect("write mismatched source");
        }
        let project_path = directory.path().join(name);
        let mut editor =
            Editor::new(Project::new(sample_rate).expect("project"), 8).expect("editor");
        editor
            .execute(Command::ImportAudioTrack {
                name: "incoming".into(),
                path: source_name.into(),
                sample_rate,
                frames,
            })
            .expect("create import metadata");
        hex_storage::save_new(&project_path, editor.project()).expect("save fixture project");
        active
            .submit(Request::Open(project_path))
            .expect("submit open");
        let rejected = next_event(&active);
        assert!(rejected.error.is_some());
        assert!(rejected.open_complete);
        assert!(!rejected.project_replaced);
        assert!(rejected.playback.is_none());
        assert_eq!(rejected.state.revision, revision);
        assert_eq!(rejected.state.project.tracks().len(), 1);
        assert_eq!(rejected.state.project.tracks()[0].name(), "keep me");
    }

    let mut project = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    project
        .execute(Command::AddAsset {
            path: "Missing-Unused.wav".into(),
            sample_rate: 48_000,
            frames: 1,
        })
        .expect("add unreferenced metadata");
    let path = directory.path().join("missing-unreferenced.hex");
    hex_storage::save_new(&path, project.project()).expect("save project fixture");
    active
        .submit(Request::Open(path))
        .expect("submit open with missing unreferenced asset");
    // A missing unreferenced asset is kept offline rather than dropped.
    let opened = next_event(&active);
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert!(opened.project_replaced);
    let asset = opened.state.project.assets().next().expect("asset kept");
    assert_eq!(asset.path(), std::path::Path::new("Missing-Unused.wav"));
    assert!(opened.state.project.asset_offline(asset.id()));
}

#[test]
fn save_current_refuses_untitled_then_saves_repeatedly_and_keeps_later_edits_dirty() {
    let directory = TempDir::new();
    let service = Service::new(48_000, 16, 32).expect("service");
    let initial = next_event(&service);
    assert_eq!(initial.state.document_path, None);
    service
        .submit(Request::SaveCurrent)
        .expect("untitled save request");
    let refused = next_event(&service);
    assert!(refused.save_complete);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.contains("before it has a file path"))
    );
    assert_eq!(refused.state.document_path, None);
    assert!(!refused.state.unsaved);

    service
        .submit(Request::Edit(Command::AddTrack { name: "one".into() }))
        .expect("add first track");
    let edited = next_event(&service);
    let path = directory.path().join("Current.hex");
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save as");
    let saved = next_event(&service);
    assert!(saved.error.is_none());
    assert!(saved.save_complete);
    assert!(!saved.state.unsaved);
    assert_eq!(saved.state.document_path.as_deref(), Some(path.as_path()));
    assert_eq!(saved.state.revision, edited.state.revision);

    service
        .submit(Request::SaveCurrent)
        .expect("repeat current save");
    let resaved = next_event(&service);
    assert!(resaved.error.is_none(), "{:?}", resaved.error);
    assert!(resaved.save_complete);
    assert!(!resaved.state.unsaved);
    assert_eq!(resaved.state.revision, saved.state.revision);
    assert_eq!(
        hex_storage::load_recovery(&path).expect("recovery").project,
        *saved.state.project
    );

    service
        .submit(Request::SaveCurrent)
        .expect("queue current save");
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "later".into(),
        }))
        .expect("queue later edit");
    let saved_before_edit = next_event(&service);
    let edit_after_save = next_event(&service);
    assert!(saved_before_edit.save_complete);
    assert!(!edit_after_save.save_complete);
    assert!(!saved_before_edit.state.unsaved);
    assert!(edit_after_save.state.unsaved);
    assert_eq!(
        edit_after_save.state.revision,
        saved_before_edit.state.revision + 1
    );
    assert_eq!(
        edit_after_save.state.document_path.as_deref(),
        Some(path.as_path())
    );
}

#[test]
fn current_save_conflict_and_failed_open_preserve_association_and_revision() {
    let directory = TempDir::new();
    let path = directory.path().join("Original.hex");
    hex_storage::save_new(&path, &Project::new(48_000).expect("project")).expect("write project");
    let service = Service::new(48_000, 16, 32).expect("service");
    let _ = next_event(&service);
    service
        .submit(Request::Open(path.clone()))
        .expect("open project");
    let opened = next_event(&service);
    assert!(opened.error.is_none());
    assert_eq!(opened.state.document_path.as_deref(), Some(path.as_path()));

    let mut externally_changed = fs::read(&path).expect("read project");
    externally_changed.push(b' ');
    fs::write(&path, externally_changed).expect("external formatting change");
    service
        .submit(Request::SaveCurrent)
        .expect("attempt conflicting save");
    let conflict = next_event(&service);
    assert!(conflict.save_complete);
    assert!(
        conflict
            .error
            .as_deref()
            .is_some_and(|error| error.contains("changed outside hex"))
    );
    assert_eq!(conflict.state.revision, opened.state.revision);
    assert_eq!(
        conflict.state.document_path.as_deref(),
        Some(path.as_path())
    );
    assert!(conflict.playback.is_none());
    assert!(!conflict.state.unsaved);

    let missing = directory.path().join("missing.hex");
    service.submit(Request::Open(missing)).expect("failed open");
    let failed_open = next_event(&service);
    assert!(failed_open.error.is_some());
    assert!(!failed_open.project_replaced);
    assert_eq!(failed_open.state.revision, opened.state.revision);
    assert_eq!(
        failed_open.state.document_path.as_deref(),
        Some(path.as_path())
    );
}

#[test]
fn request_queue_is_bounded_and_dropping_service_is_nonblocking() {
    let service = Service::new(48_000, 16, 32).expect("service");
    let mut full_request = None;
    for index in 0..128 {
        match service.submit(Request::Edit(Command::AddTrack {
            name: format!("queued {index}"),
        })) {
            Ok(()) => {}
            Err(SubmitError::Full(request)) => {
                full_request = Some(request);
                break;
            }
            Err(SubmitError::Closed(_)) => panic!("worker exited unexpectedly"),
            Err(SubmitError::Busy(..)) => panic!("non-export request was marked busy"),
        }
    }
    assert!(matches!(
        full_request,
        Some(Request::Edit(Command::AddTrack { .. }))
    ));
    let started = Instant::now();
    drop(service);
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn duplicate_events_report_worker_ids_and_render_a_sequential_chain() {
    let service = Service::new(48_000, 16, 32).expect("service");
    let _ = next_event(&service);
    let path = std::env::temp_dir().join(format!(
        "hex-session-duplicate-{}-{}.wav",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::write(&path, wav_mono_pcm16(&[8192, -8192, 4096])).expect("write fixture");
    service
        .submit(Request::Import(path.clone()))
        .expect("submit import");
    let imported = next_event(&service);
    let _ = fs::remove_file(&path);
    let original = imported.state.project.tracks()[0].clips()[0].id();

    service
        .submit(Request::Edit(Command::DuplicateClip(original)))
        .expect("submit first duplicate");
    let first = next_event(&service);
    assert!(first.duplicate_complete);
    let first_id = match first.created {
        Some(Created::Clip(id)) => id,
        other => panic!("duplicate event missing created clip: {other:?}"),
    };
    service
        .submit(Request::Edit(Command::DuplicateClip(first_id)))
        .expect("submit second duplicate");
    let second = next_event(&service);
    assert!(second.duplicate_complete);
    let second_id = match second.created {
        Some(Created::Clip(id)) => id,
        other => panic!("duplicate event missing created clip: {other:?}"),
    };
    service
        .submit(Request::Edit(Command::DuplicateClip(second_id)))
        .expect("submit third duplicate");
    let third = next_event(&service);
    assert!(third.duplicate_complete);
    let third_id = match third.created {
        Some(Created::Clip(id)) => id,
        other => panic!("duplicate event missing created clip: {other:?}"),
    };
    assert_ne!(first_id, second_id);
    assert!(second_id.value() > first_id.value());
    assert!(third_id.value() > second_id.value());
    assert_eq!(
        first.state.project.clip(first_id).expect("first").start(),
        3
    );
    assert_eq!(
        second
            .state
            .project
            .clip(second_id)
            .expect("second")
            .start(),
        6
    );
    assert_eq!(
        third.state.project.clip(third_id).expect("third").start(),
        9
    );

    let mut playback = third.playback.expect("compiled duplicate graph");
    playback.apply(PlaybackCommand::Play);
    let mut rendered = [[0.0; 2]; 12];
    playback.render(&mut rendered).expect("render duplicates");
    assert_eq!(
        rendered,
        [
            [0.25, 0.25],
            [-0.25, -0.25],
            [0.125, 0.125],
            [0.25, 0.25],
            [-0.25, -0.25],
            [0.125, 0.125],
            [0.25, 0.25],
            [-0.25, -0.25],
            [0.125, 0.125],
            [0.25, 0.25],
            [-0.25, -0.25],
            [0.125, 0.125],
        ]
    );
}

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_DIRECTORY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-session-open-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create test directory");
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

fn wav_at_rate(rate: u32, samples: &[i16]) -> Vec<u8> {
    let mut bytes = wav_mono_pcm16(samples);
    bytes[24..28].copy_from_slice(&rate.to_le_bytes());
    bytes[28..32].copy_from_slice(&(rate * 2).to_le_bytes());
    bytes
}

#[test]
fn mixed_rate_import_save_reopen_is_exact_and_changed_source_is_atomic() {
    let directory = TempDir::new();
    let source_path = directory.path().join("Source-MiXeD.wav");
    let project_path = directory.path().join("Converted.hex");
    let samples: Vec<i16> = (0..480).map(|i| ((i % 80) as i16 - 40) * 400).collect();
    fs::write(&source_path, wav_at_rate(48_000, &samples)).expect("source");
    // The old derivative-only budget must refuse rather than hide the480-frame
    // original. The successful fixture reserves exactly both retained buffers.
    let derivative_only = Service::new(44_100, 16, 441).expect("small service");
    let initial = next_event(&derivative_only);
    derivative_only
        .submit(Request::Import(source_path.clone()))
        .unwrap();
    let refused = next_event(&derivative_only);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, initial.state.project);
    assert_eq!(refused.state.unsaved, initial.state.unsaved);
    drop(derivative_only);
    let service = Service::new(44_100, 16, 480 + 441).expect("source plus derivative budget");
    let _ = next_event(&service);
    service
        .submit(Request::Import(source_path.clone()))
        .expect("import");
    let imported = next_event(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let asset_id = imported.state.project.tracks()[0].clips()[0].asset();
    let metadata = imported.state.project.asset(asset_id).expect("asset");
    assert_eq!(metadata.source_sample_rate(), 48_000);
    assert_eq!(metadata.source_frames(), 480);
    assert_eq!(metadata.playback_frames(), 441);
    assert_eq!(imported.state.waveforms[&asset_id].source_frames, 441);
    let mut playback = imported.playback.expect("graph");
    playback.apply(PlaybackCommand::Play);
    let mut baseline = vec![[0.0; 2]; 441];
    playback.render(&mut baseline).expect("render imported");
    service
        .submit(Request::SaveNew(project_path.clone()))
        .expect("save");
    let saved = next_event(&service);
    assert!(saved.error.is_none());
    assert!(!saved.state.unsaved);
    let json = fs::read_to_string(&project_path).expect("saved JSON");
    let document: serde_json::Value = serde_json::from_str(&json).expect("project json");
    // A rate-converted import records sinc v2, which only schema64 carries.
    assert_eq!(document["version"], hex_storage::SINC_V2_SCHEMA_VERSION);
    assert_eq!(document["routing"], serde_json::json!([]));
    assert!(document["import_provenance"].is_null());
    service
        .submit(Request::Open(project_path.clone()))
        .expect("open");
    let reopened = next_event(&service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert!(reopened.project_replaced);
    assert_eq!(reopened.state.project, saved.state.project);
    let mut playback = reopened.playback.expect("reopened graph");
    playback.apply(PlaybackCommand::Play);
    let mut restored = vec![[0.0; 2]; 441];
    for chunk in restored.chunks_mut(29) {
        playback.render(chunk).expect("render");
    }
    assert_eq!(restored, baseline);

    fs::write(&source_path, wav_at_rate(44_100, &samples)).expect("changed source rate");
    service
        .submit(Request::Open(project_path))
        .expect("open changed source");
    let rejected = next_event(&service);
    assert!(rejected.error.is_some());
    assert!(!rejected.project_replaced);
    assert!(rejected.playback.is_none());
    assert_eq!(rejected.state.revision, reopened.state.revision);
    assert_eq!(rejected.state.project, reopened.state.project);
    assert!(!rejected.state.unsaved);
}

#[test]
fn browser_sample_drop_inserts_track_and_clip_atomically_and_round_trips() {
    let directory = TempDir::new();
    let source_path = directory.path().join("Drop.WaV");
    let project_path = directory.path().join("Inserted.hex");
    fs::write(
        &source_path,
        wav_mono_pcm16(&[1_000, -2_000, 3_000, -4_000]),
    )
    .expect("sample fixture");
    let service = Service::new(48_000, 16, 128).expect("service");
    let _ = next_event(&service);

    let mut original_tracks = Vec::new();
    for name in ["first", "second"] {
        service
            .submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .expect("add track");
        let event = next_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        let track = event.state.project.tracks().last().expect("track").id();
        original_tracks.push(track);
    }

    service
        .submit(Request::ImportToNewTrack {
            path: source_path.clone(),
            before_track: Some(original_tracks[1]),
            start: 96,
        })
        .expect("submit insertion import");
    let inserted = next_event(&service);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let project = &inserted.state.project;
    let new_track = project.tracks()[1].id();
    assert_eq!(
        project
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>(),
        [original_tracks[0], new_track, original_tracks[1]]
    );
    assert_eq!(
        project.track(new_track).expect("new track").clips().len(),
        1
    );
    let clip = &project.track(new_track).expect("new track").clips()[0];
    assert_eq!(clip.start(), 96);
    assert_eq!(clip.duration(), 4);

    service.submit(Request::Undo).expect("undo insertion");
    let undone = next_event(&service);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project.tracks().len(), 2);
    assert_eq!(undone.state.project.assets().count(), 0);
    assert!(undone.state.can_redo);

    service.submit(Request::Redo).expect("redo insertion");
    let redone = next_event(&service);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, inserted.state.project);
    service
        .submit(Request::SaveNew(project_path.clone()))
        .expect("save inserted project");
    let saved = next_event(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    service
        .submit(Request::Open(project_path))
        .expect("reopen inserted project");
    let reopened = next_event(&service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert_eq!(reopened.state.project, saved.state.project);

    let stale_anchor = original_tracks[0];
    service
        .submit(Request::Edit(Command::RemoveTrack(stale_anchor)))
        .expect("remove future insertion anchor");
    let removed = next_event(&service);
    assert!(removed.error.is_none(), "{:?}", removed.error);
    service
        .submit(Request::ImportToNewTrack {
            path: directory.path().join("missing.wav"),
            before_track: Some(stale_anchor),
            start: 0,
        })
        .expect("submit stale insertion anchor");
    let refused = next_event(&service);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| { error.contains("insertion anchor no longer exists") })
    );
    assert_eq!(refused.state.project, removed.state.project);
    assert_eq!(refused.state.revision, removed.state.revision);
    assert_eq!(refused.state.can_undo, removed.state.can_undo);
}

#[test]
fn arrangement_clip_track_insertion_edits_compile_undo_and_round_trip() {
    let directory = TempDir::new();
    let source_path = directory.path().join("source.wav");
    let project_path = directory.path().join("moved-clips.hex");
    fs::write(
        &source_path,
        wav_mono_pcm16(&[8_000, -8_000, 4_000, -4_000]),
    )
    .expect("sample fixture");
    let service = Service::new(48_000, 16, 128).expect("service");
    let _ = next_event(&service);

    let mut original_tracks = Vec::new();
    for name in ["audio source", "midi source", "anchor"] {
        service
            .submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .expect("add hybrid track");
        let event = next_event(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        original_tracks.push(match event.created.expect("track identity") {
            Created::Track(track) => track,
            other => panic!("unexpected create result: {other:?}"),
        });
    }

    service
        .submit(Request::ImportToTrack {
            path: source_path,
            track: original_tracks[0],
            start: 0,
        })
        .expect("import source audio");
    let imported = next_event(&service);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let audio_clip = imported
        .state
        .project
        .track(original_tracks[0])
        .unwrap()
        .clips()[0]
        .id();
    let audio_asset = imported
        .state
        .project
        .track(original_tracks[0])
        .unwrap()
        .clips()[0]
        .asset();

    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track: original_tracks[1],
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        }))
        .expect("place midi clip");
    let placed = next_event(&service);
    assert!(placed.error.is_none(), "{:?}", placed.error);
    let midi_clip = match placed.created.expect("midi clip identity") {
        Created::Clip(clip) => clip,
        other => panic!("unexpected create result: {other:?}"),
    };
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip: midi_clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .expect("add note");
    let noted = next_event(&service);
    assert!(noted.error.is_none(), "{:?}", noted.error);
    let note_id = noted
        .state
        .project
        .track(original_tracks[1])
        .unwrap()
        .midi_clips()[0]
        .notes()[0]
        .id();

    service
        .submit(Request::Edit(Command::MoveClipToNewTrack {
            initial_gain: Some(0.25),
            clip: audio_clip,
            name: "moved audio".into(),
            before_track: Some(original_tracks[2]),
            start: 96,
        }))
        .expect("move audio into a new track");
    let audio_moved = next_event(&service);
    assert!(audio_moved.error.is_none(), "{:?}", audio_moved.error);
    let audio_track = match audio_moved.created.expect("inserted audio track") {
        Created::Track(track) => track,
        other => panic!("unexpected move result: {other:?}"),
    };
    assert_eq!(audio_moved.state.project.tracks()[2].id(), audio_track);
    assert_eq!(
        audio_moved.state.project.track(audio_track).unwrap().gain(),
        0.25
    );
    let moved_audio = &audio_moved
        .state
        .project
        .track(audio_track)
        .unwrap()
        .clips()[0];
    assert_eq!(moved_audio.id(), audio_clip);
    assert_eq!(moved_audio.asset(), audio_asset);
    assert_eq!(moved_audio.start(), 96);
    assert!(
        audio_moved
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .is_some()
    );

    service
        .submit(Request::Edit(Command::MoveMidiClipToNewTrack {
            initial_gain: None,
            clip: midi_clip,
            name: "moved midi".into(),
            before_track: None,
            start: hex_project::BeatTime::from_quarters(2).expect("two beats"),
        }))
        .expect("move midi into a new bottom track");
    let midi_moved = next_event(&service);
    assert!(midi_moved.error.is_none(), "{:?}", midi_moved.error);
    let midi_track = match midi_moved.created.expect("inserted midi track") {
        Created::Track(track) => track,
        other => panic!("unexpected move result: {other:?}"),
    };
    assert_eq!(
        midi_moved.state.project.tracks().last().unwrap().id(),
        midi_track
    );
    let moved_midi = &midi_moved
        .state
        .project
        .track(midi_track)
        .unwrap()
        .midi_clips()[0];
    assert_eq!(moved_midi.id(), midi_clip);
    assert_eq!(
        moved_midi.start(),
        hex_project::BeatTime::from_quarters(2).expect("two beats")
    );
    assert_eq!(moved_midi.notes()[0].id(), note_id);
    assert!(
        midi_moved
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .is_some()
    );

    service.submit(Request::Undo).expect("undo midi move");
    let midi_undone = next_event(&service);
    assert!(midi_undone.error.is_none(), "{:?}", midi_undone.error);
    assert!(
        midi_undone
            .state
            .project
            .track(original_tracks[1])
            .is_some()
    );
    assert!(midi_undone.state.project.track(midi_track).is_none());
    service.submit(Request::Redo).expect("redo midi move");
    let midi_redone = next_event(&service);
    assert!(midi_redone.error.is_none(), "{:?}", midi_redone.error);
    assert_eq!(midi_redone.state.project, midi_moved.state.project);

    service
        .submit(Request::Undo)
        .expect("undo midi move before save");
    let _ = next_event(&service);
    service.submit(Request::Undo).expect("undo audio move");
    let audio_undone = next_event(&service);
    assert!(audio_undone.error.is_none(), "{:?}", audio_undone.error);
    assert!(
        audio_undone
            .state
            .project
            .track(original_tracks[0])
            .unwrap()
            .clips()
            .iter()
            .any(|clip| clip.id() == audio_clip)
    );
    assert!(audio_undone.state.project.track(audio_track).is_none());
    service.submit(Request::Redo).expect("redo audio move");
    let audio_redone = next_event(&service);
    assert!(audio_redone.error.is_none(), "{:?}", audio_redone.error);
    assert_eq!(
        audio_redone.state.project.tracks(),
        audio_moved.state.project.tracks()
    );
    assert_eq!(
        audio_redone.state.project.assets().count(),
        audio_moved.state.project.assets().count()
    );

    service
        .submit(Request::SaveNew(project_path.clone()))
        .expect("save moved clips");
    let saved = next_event(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);
    service
        .submit(Request::Open(project_path))
        .expect("reopen moved clips");
    let reopened = next_event(&service);
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert_eq!(reopened.state.project, saved.state.project);
    assert!(
        reopened
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .is_some()
    );
}

#[test]
fn conversion_output_cap_failure_preserves_revision_and_history() {
    let directory = TempDir::new();
    let source_path = directory.path().join("Upsample.wav");
    fs::write(&source_path, wav_at_rate(8_000, &[100; 10])).expect("source");
    let service = Service::new(48_000, 16, 59).expect("service");
    let _ = next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "keep".into(),
        }))
        .expect("edit");
    let before = next_event(&service);
    service
        .submit(Request::Import(source_path))
        .expect("import");
    let refused = next_event(&service);
    assert!(refused.error.is_some());
    assert!(refused.playback.is_none());
    assert_eq!(refused.state.project, before.state.project);
    assert_eq!(refused.state.revision, before.state.revision);
    assert_eq!(refused.state.can_undo, before.state.can_undo);
    assert!(refused.state.unsaved);
}

#[test]
fn solo_worker_edits_publish_renderable_state_and_persist_additive_selection() {
    let directory = TempDir::new();
    let service = Service::new(48_000, 16, 8).expect("service");
    let _ = next_event(&service);
    let mut imported_tracks = Vec::new();
    for (name, sample) in [("first.wav", 8_192), ("second.wav", 16_384)] {
        let path = directory.path().join(name);
        fs::write(&path, wav_mono_pcm16(&[sample; 2])).expect("source");
        service.submit(Request::Import(path)).expect("import");
        let event = next_event(&service);
        assert!(event.error.is_none());
        imported_tracks.push(event.state.project.tracks().last().expect("track").id());
    }
    service
        .submit(Request::Edit(Command::ToggleSolo {
            track: imported_tracks[0],
            exclusive: true,
        }))
        .expect("exclusive solo");
    let exclusive = next_event(&service);
    assert!(exclusive.state.unsaved);
    let mut graph = exclusive.playback.expect("solo graph");
    graph.apply(PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 2];
    graph.render(&mut output).expect("solo render");
    assert_eq!(output, [[0.25; 2]; 2]);

    service
        .submit(Request::Edit(Command::ToggleSolo {
            track: imported_tracks[1],
            exclusive: false,
        }))
        .expect("additive solo");
    let additive = next_event(&service);
    assert!(
        additive
            .state
            .project
            .tracks()
            .iter()
            .all(|track| track.soloed())
    );
    let mut graph = additive.playback.expect("additive graph");
    graph.apply(PlaybackCommand::Play);
    graph.render(&mut output).expect("additive render");
    assert_eq!(output, [[0.75; 2]; 2]);
    let path = directory.path().join("Solo.hex");
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save");
    assert!(next_event(&service).error.is_none());
    service.submit(Request::Open(path)).expect("open");
    let restored = next_event(&service);
    assert!(restored.error.is_none());
    assert_eq!(restored.state.project, additive.state.project);
    assert!(!restored.state.unsaved);
    assert!(!restored.state.can_undo);
    let mut graph = restored.playback.expect("restored solo graph");
    graph.apply(PlaybackCommand::Play);
    graph.render(&mut output).expect("restored render");
    assert_eq!(output, [[0.75; 2]; 2]);
}

#[test]
fn loop_worker_edit_is_dirty_undoable_compiled_and_saved_with_extended_view_horizon() {
    let directory = TempDir::new();
    let source = directory.path().join("loop-source.wav");
    fs::write(&source, wav_mono_pcm16(&[4_096, 8_192, 12_288, 16_384])).expect("source");
    let service = Service::new(48_000, 16, 8).expect("service");
    let _ = next_event(&service);
    service.submit(Request::Import(source)).expect("import");
    let imported = next_event(&service);
    assert!(imported.error.is_none());

    service
        .submit(Request::Edit(Command::SetLoopRegion {
            start: 1,
            end: 3,
            enabled: true,
        }))
        .expect("set loop region");
    let looped = next_event(&service);
    assert!(looped.error.is_none());
    assert!(looped.state.unsaved);
    assert!(looped.state.can_undo);
    assert_eq!(
        looped.state.project.loop_region().map(|region| (
            region.start(),
            region.end(),
            region.enabled()
        )),
        Some((1, 3, true))
    );
    // The worker must publish the returned canonical project. The engine owns
    // loop DSP (including its end envelope); do not duplicate that law here.
    let asset = looped
        .state
        .project
        .assets()
        .next()
        .expect("imported asset")
        .id();
    let media = std::collections::BTreeMap::from([(
        asset,
        hex_engine::Audio::new(48_000, vec![[0.125; 2], [0.25; 2], [0.375; 2], [0.5; 2]])
            .expect("known source"),
    )]);
    let revision = PreparedRevision::prepare(
        &looped.state.project,
        &media,
        std::sync::Arc::new(hex_engine::SineTable::new()),
        None,
    )
    .expect("canonical loop graph");
    let mut canonical = Playback::try_from_prepared_with_session(std::sync::Arc::new(revision))
        .expect("canonical playback");
    canonical.apply(PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; 7];
    canonical
        .render(&mut expected)
        .expect("canonical loop render");
    let mut playback = looped.playback.expect("compiled loop graph");
    playback.apply(PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 7];
    playback.render(&mut output).expect("loop render");
    assert_eq!(output, expected);
    assert_eq!(output[0], [0.125; 2]);
    assert_eq!(output[1], [0.25; 2]);
    assert_eq!(output[3], output[1], "worker retains the loop origin");

    service.submit(Request::Undo).expect("undo loop");
    let undone = next_event(&service);
    assert!(undone.error.is_none());
    assert!(undone.state.unsaved);
    assert_eq!(undone.state.project.loop_region(), None);
    let mut linear = undone.playback.expect("compiled linear graph");
    linear.apply(PlaybackCommand::Play);
    linear.render(&mut output).expect("linear render");
    assert_eq!(output[0], [0.125, 0.125]);
    assert_eq!(output[3], [0.5, 0.5]);
    assert_eq!(output[4], [0.0, 0.0]);

    service.submit(Request::Redo).expect("redo loop");
    let redone = next_event(&service);
    assert!(redone.error.is_none());
    assert!(redone.state.project.loop_region().unwrap().enabled());

    let extended_end = 500_000;
    service
        .submit(Request::Edit(Command::SetLoopRegion {
            start: extended_end - 1,
            end: extended_end,
            enabled: false,
        }))
        .expect("set disabled distant region");
    let extended = next_event(&service);
    assert!(extended.error.is_none());
    assert!(extended.state.timeline_frames >= extended_end);
    let path = directory.path().join("Loop.hex");
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save loop project");
    let saved = next_event(&service);
    assert!(saved.error.is_none());
    assert!(!saved.state.unsaved);
    service
        .submit(Request::Open(path))
        .expect("reopen loop project");
    let reopened = next_event(&service);
    assert!(reopened.error.is_none());
    assert_eq!(
        reopened.state.project.loop_region().map(|region| (
            region.start(),
            region.end(),
            region.enabled()
        )),
        Some((extended_end - 1, extended_end, false))
    );
}

#[test]
fn arrangement_catalog_drop_routes_all_destinations_and_is_one_undo() {
    use hex_engine::NativeDeviceClass;
    use hex_lattice_contract::DeviceTypeId;
    use hex_session::service::ArrangementDeviceDestination;
    for class in [NativeDeviceClass::Instrument, NativeDeviceClass::Effect] {
        for destination_kind in 0..4 {
            let service = Service::new(48_000, 16, 128).expect("service");
            let mut baseline = next_event(&service);
            if destination_kind != 2 {
                for name in ["source", "anchor"] {
                    service
                        .submit(Request::Edit(Command::AddTrack { name: name.into() }))
                        .unwrap();
                    baseline = next_event(&service);
                }
                let first = baseline.state.project.tracks()[0].id();
                // Retain actual clip content through insertion/replacement and Undo.
                service
                    .submit(Request::Edit(Command::PlaceMidiClip {
                        track: first,
                        start: hex_project::BeatTime::ZERO,
                        length: hex_project::BeatTime::ONE,
                    }))
                    .unwrap();
                baseline = next_event(&service);
                assert!(baseline.error.is_none());
                let directory = TempDir::new();
                let path = directory.path().join("Original.WAV");
                fs::write(&path, wav_mono_pcm16(&[8000, -8000, 4000, -4000])).unwrap();
                service
                    .submit(Request::ImportToTrack {
                        path,
                        track: first,
                        start: 96,
                    })
                    .unwrap();
                baseline = next_event(&service);
                assert!(baseline.error.is_none());
                service
                    .submit(Request::AddNativeDevice {
                        track: first,
                        class: NativeDeviceClass::Effect,
                        type_id: Box::new(DeviceTypeId::from_static("hex.utility")),
                        before: None,
                    })
                    .unwrap();
                baseline = next_event(&service);
                if destination_kind == 3 {
                    service
                        .submit(Request::Edit(Command::RemoveInstrument { track: first }))
                        .unwrap();
                    baseline = next_event(&service);
                }
            }
            let first = baseline.state.project.tracks().first();
            let destination = match destination_kind {
                0 | 3 => ArrangementDeviceDestination::ExistingTrack {
                    track: first.unwrap().id(),
                    instrument: first.unwrap().instrument().map(hex_project::Device::id),
                },
                1 => ArrangementDeviceDestination::NewTrack {
                    before_track: Some(baseline.state.project.tracks()[1].id()),
                },
                _ => ArrangementDeviceDestination::NewTrack { before_track: None },
            };
            let type_id = DeviceTypeId::from_static(match class {
                NativeDeviceClass::Instrument => "hex.instrument.fm4",
                NativeDeviceClass::Effect => "hex.utility",
            });
            service
                .submit(Request::DropBrowserDeviceOnArrangement {
                    new_track_gain: None,
                    project_generation: 0,
                    revision: baseline.state.revision,
                    class,
                    type_id,
                    destination,
                })
                .unwrap();
            let applied = next_event(&service);
            assert!(applied.error.is_none(), "{:?}", applied.error);
            assert_eq!(applied.state.revision, baseline.state.revision + 1);
            assert!(
                applied.created.is_none(),
                "drop must not request a selection change"
            );
            let target_index = usize::from(destination_kind == 1);
            let target = &applied.state.project.tracks()[target_index];
            match class {
                NativeDeviceClass::Instrument => {
                    assert_eq!(target.instrument().unwrap().state().type_id(), type_id)
                }
                NativeDeviceClass::Effect => {
                    let existing = first.map_or(0, |track| {
                        if destination_kind == 0 || destination_kind == 3 {
                            track.devices().len()
                        } else {
                            0
                        }
                    });
                    assert_eq!(target.devices().len(), existing + 1);
                    assert_eq!(target.devices().last().unwrap().state().type_id(), type_id);
                }
            }
            if destination_kind == 0 || destination_kind == 3 {
                assert_eq!(target.id(), first.unwrap().id());
                assert_eq!(target.midi_clips(), first.unwrap().midi_clips());
                assert_eq!(target.clips(), first.unwrap().clips());
                assert_eq!(
                    &target.devices()[..first.unwrap().devices().len()],
                    first.unwrap().devices()
                );
                assert_eq!(
                    applied.state.project.tracks()[1],
                    baseline.state.project.tracks()[1]
                );
            } else if destination_kind == 1 {
                assert_eq!(
                    applied.state.project.tracks()[0],
                    baseline.state.project.tracks()[0]
                );
                assert_eq!(
                    applied.state.project.tracks()[2],
                    baseline.state.project.tracks()[1]
                );
            }
            service.submit(Request::Undo).unwrap();
            let undone = next_event(&service);
            // Canonical Undo deliberately retains the ID high-water mark.
            assert_eq!(
                undone.state.project.tracks(),
                baseline.state.project.tracks()
            );
            assert_eq!(
                undone.state.project.assets().collect::<Vec<_>>(),
                baseline.state.project.assets().collect::<Vec<_>>()
            );
            assert_eq!(
                undone.state.project.session(),
                baseline.state.project.session()
            );
            service.submit(Request::Redo).unwrap();
            assert_eq!(next_event(&service).state.project, applied.state.project);
        }
    }
}

#[test]
fn arrangement_catalog_drop_refuses_stale_catalog_and_captured_targets_atomically() {
    use hex_engine::NativeDeviceClass;
    use hex_lattice_contract::DeviceTypeId;
    use hex_session::service::ArrangementDeviceDestination;
    let service = Service::new(48_000, 16, 128).unwrap();
    next_event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "target".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let baseline = next_event(&service);
    let track = baseline.state.project.tracks()[0].id();
    for refusal in 0..5 {
        service
            .submit(Request::DropBrowserDeviceOnArrangement {
                new_track_gain: None,
                project_generation: u64::from(refusal == 0),
                revision: baseline.state.revision + u64::from(refusal == 1),
                class: NativeDeviceClass::Instrument,
                type_id: DeviceTypeId::from_static(if refusal == 2 {
                    "unknown.device"
                } else if refusal == 3 {
                    "hex.utility"
                } else {
                    "hex.instrument.fm4"
                }),
                destination: ArrangementDeviceDestination::ExistingTrack {
                    track,
                    instrument: if refusal == 4 {
                        None
                    } else {
                        baseline
                            .state
                            .project
                            .track(track)
                            .unwrap()
                            .instrument()
                            .map(hex_project::Device::id)
                    },
                },
            })
            .unwrap();
        let refused = next_event(&service);
        assert!(refused.error.is_some(), "refusal {refusal}");
        assert_eq!(refused.state.project, baseline.state.project);
        assert_eq!(refused.state.revision, baseline.state.revision);
        assert_eq!(refused.state.can_undo, baseline.state.can_undo);
    }
    service
        .submit(Request::Edit(Command::RemoveTrack(track)))
        .unwrap();
    let removed = next_event(&service);
    for destination in [
        ArrangementDeviceDestination::ExistingTrack {
            track,
            instrument: None,
        },
        ArrangementDeviceDestination::NewTrack {
            before_track: Some(track),
        },
    ] {
        service
            .submit(Request::DropBrowserDeviceOnArrangement {
                new_track_gain: None,
                project_generation: 0,
                revision: removed.state.revision,
                class: NativeDeviceClass::Effect,
                type_id: DeviceTypeId::from_static("hex.utility"),
                destination,
            })
            .unwrap();
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, removed.state.project);
        assert_eq!(refused.state.revision, removed.state.revision);
        assert_eq!(refused.state.can_undo, removed.state.can_undo);
    }
}

#[test]
fn arrangement_catalog_atomic_command_preserves_project_on_compile_and_capacity_failure() {
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    use hex_project::TrackDeviceSlot;
    for slot in [TrackDeviceSlot::Instrument, TrackDeviceSlot::Effect] {
        let mut session = hex_session::Session::new(48_000, 16, 0).unwrap();
        let baseline = session.project().clone();
        let unsupported =
            NativeDeviceState::new(DeviceTypeId::from_static("unknown.device"), 1, Vec::new())
                .unwrap();
        let result = session.command_prepared_revision(
            Command::InsertTrackWithDevice {
                name: "new track".into(),
                before_track: None,
                slot,
                state: unsupported,
            },
            None,
        );
        assert!(result.is_err());
        assert_eq!(session.project(), &baseline);
        assert!(!session.can_undo());
    }
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    for _ in 0..hex_project::MAX_TRACKS {
        editor
            .execute(Command::AddTrack {
                name: "existing".into(),
            })
            .unwrap();
    }
    let mut session =
        hex_session::Session::from_loaded(editor.project().clone(), 16, 0, Vec::new()).unwrap();
    let baseline = session.project().clone();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    assert!(
        session
            .command_prepared_revision(
                Command::InsertTrackWithDevice {
                    name: "over capacity".into(),
                    before_track: None,
                    slot: TrackDeviceSlot::Effect,
                    state,
                },
                None
            )
            .is_err()
    );
    assert_eq!(session.project(), &baseline);
    assert!(!session.can_undo());
}

#[test]
fn clip_conversion_worker_refuses_audio_and_stale_selection_without_publication() {
    let service = Service::new(48_000, 16, 1024).unwrap();
    next_event(&service);
    let path = std::env::temp_dir().join(format!(
        "hex-convert-service-{}-{}.wav",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&path, wav_mono_pcm16(&[4000; 512])).unwrap();
    service.submit(Request::Import(path.clone())).unwrap();
    let imported = next_event(&service);
    let track = &imported.state.project.tracks()[0];
    let clip = track.clips()[0].id();
    let revision = imported.state.revision;
    let request = |project_generation, revision| Request::ConvertClip {
        project_generation,
        revision,
        clip,
        to_audio: false,
    };
    for stale in [request(1, revision), request(0, revision - 1)] {
        service.submit(stale).unwrap();
        let refused = next_event(&service);
        assert!(refused.error.as_ref().unwrap().contains("project changed"));
        assert!(refused.playback.is_none());
        assert_eq!(refused.state.project, imported.state.project);
        assert_eq!(refused.state.revision, revision);
    }
    let bytes = fs::read(&path).unwrap();
    service.submit(request(0, revision)).unwrap();
    let refused = next_event(&service);
    assert_eq!(
        refused.error.as_deref(),
        Some(hex_project::AUDIO_TO_MIDI_UNSUPPORTED)
    );
    assert!(refused.playback.is_none() && refused.created.is_none());
    assert_eq!(refused.state.project, imported.state.project);
    assert_eq!(refused.state.revision, revision);
    assert_eq!(refused.state.unsaved, imported.state.unsaved);
    assert_eq!(refused.state.can_undo, imported.state.can_undo);
    assert_eq!(refused.state.can_redo, imported.state.can_redo);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    service
        .submit(Request::Edit(Command::AddScene {
            name: "scene".into(),
        }))
        .unwrap();
    let scene_event = next_event(&service);
    let Some(Created::Scene(scene)) = scene_event.created else {
        panic!("scene")
    };
    service
        .submit(Request::Edit(Command::CopyArrangementAudioClipToSession {
            source: clip,
            scene,
        }))
        .unwrap();
    let copied = next_event(&service);
    let Some(Created::SessionClip(cell)) = copied.created else {
        panic!("session clip")
    };
    for id in [clip, cell] {
        for command_request in [false, true] {
            let request = if command_request {
                Request::Edit(Command::ConvertAudioClipToMidi(id))
            } else {
                Request::ConvertClip {
                    project_generation: 0,
                    revision: copied.state.revision,
                    clip: id,
                    to_audio: false,
                }
            };
            service.submit(request).unwrap();
            let refused = next_event(&service);
            assert!(
                refused.error.as_deref().is_some_and(
                    |message| message.ends_with(hex_project::AUDIO_TO_MIDI_UNSUPPORTED)
                )
            );
            assert!(refused.playback.is_none() && refused.created.is_none());
            assert_eq!(refused.state.project, copied.state.project);
            assert_eq!(refused.state.revision, copied.state.revision);
            assert_eq!(refused.state.can_undo, copied.state.can_undo);
            assert_eq!(refused.state.can_redo, copied.state.can_redo);
            assert_eq!(refused.state.unsaved, copied.state.unsaved);
        }
    }
    // Refusals did not add history: one Undo still removes the session copy.
    service.submit(Request::Undo).unwrap();
    let undone = next_event(&service);
    let mut actual = undone.state.project.snapshot();
    actual.next_id = scene_event.state.project.snapshot().next_id;
    assert_eq!(actual, scene_event.state.project.snapshot());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::remove_file(path).unwrap();
}

#[test]
fn selected_solo_worker_publishes_the_complete_group_and_undoes_once() {
    let directory = TempDir::new();
    let service = Service::new(48_000, 16, 64).unwrap();
    let _ = next_event(&service);
    let mut tracks = Vec::new();
    for (name, sample) in [("group-first.wav", 8192), ("group-last.wav", 16384)] {
        let path = directory.path().join(name);
        fs::write(&path, wav_mono_pcm16(&[sample; 2])).unwrap();
        service.submit(Request::Import(path)).unwrap();
        let event = next_event(&service);
        tracks.push(event.state.project.tracks().last().unwrap().id());
    }
    service
        .submit(Request::Edit(Command::ToggleSoloTracks {
            tracks: hex_project::TrackSelectionIds::new(&tracks).unwrap(),
            exclusive: true,
        }))
        .unwrap();
    let event = next_event(&service);
    assert!(event.error.is_none(), "{:?}", event.error);
    assert!(
        event
            .state
            .project
            .tracks()
            .iter()
            .all(|track| track.soloed())
    );
    let mut graph = event.playback.unwrap();
    graph.apply(PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 2];
    graph.render(&mut output).unwrap();
    assert_eq!(output, [[0.75; 2]; 2]);
    service.submit(Request::Undo).unwrap();
    assert!(
        next_event(&service)
            .state
            .project
            .tracks()
            .iter()
            .all(|track| !track.soloed())
    );
    service.submit(Request::Redo).unwrap();
    assert!(
        next_event(&service)
            .state
            .project
            .tracks()
            .iter()
            .all(|track| track.soloed())
    );
}
