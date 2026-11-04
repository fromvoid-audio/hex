//! Public worker recovery, relink atomicity, restart and mixed project round trips.
use hex_project::{BeatTime, Command, Created, Meter, Project, Tempo};
use hex_session::service::{
    Event, ProjectSource, Request, Service, missing_media::missing_media_report,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-project-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("isolated fixture directory");
        Self(path)
    }
    fn slot(&self) -> PathBuf {
        self.0.join("recovery/untitled.hex-journal")
    }
    fn service(&self) -> Service {
        Service::with_recovery(48_000, 32, 128, self.slot()).expect("worker")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn wait(service: &Service) -> Event {
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(1))
            }
            Err(error) => panic!("worker event missing: {error}"),
        }
    }
}
fn request(service: &Service, request: Request) -> Event {
    service.submit(request).expect("submit");
    let event = wait(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn edit(service: &Service, command: Command) -> Event {
    request(service, Request::Edit(command))
}
fn open(
    service: &Service,
    source: ProjectSource,
    relinks: BTreeMap<hex_project::AssetId, PathBuf>,
) -> Event {
    request(service, Request::OpenWith { source, relinks })
}
fn wav(path: &Path, frames: u32) {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(frames * 2).to_le_bytes());
    for _ in 0..frames {
        bytes.extend_from_slice(&8192_i16.to_le_bytes());
    }
    fs::write(path, bytes).expect("original test WAV");
}
fn mixed_fixture(service: &Service, media: &Path) -> Project {
    let imported = request(service, Request::Import(media.to_path_buf()));
    let track = imported.state.project.tracks()[0].id();
    let scene = match edit(
        service,
        Command::AddScene {
            name: "MixedCase Scene".into(),
        },
    )
    .created
    {
        Some(Created::Scene(id)) => id,
        other => panic!("scene: {other:?}"),
    };
    request(
        service,
        Request::ImportToSession {
            path: media.to_path_buf(),
            track,
            scene,
        },
    );
    let stop_scene = match edit(
        service,
        Command::AddScene {
            name: "Stop".into(),
        },
    )
    .created
    {
        Some(Created::Scene(id)) => id,
        other => panic!("scene: {other:?}"),
    };
    edit(
        service,
        Command::SetSessionStopCell {
            track,
            scene: stop_scene,
        },
    );
    let midi_track = match edit(
        service,
        Command::AddMidiTrack {
            name: "Midi".into(),
        },
    )
    .created
    {
        Some(Created::Track(id)) => id,
        other => panic!("track: {other:?}"),
    };
    let clip = match edit(
        service,
        Command::PlaceMidiClip {
            track: midi_track,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(4).expect("length"),
        },
    )
    .created
    {
        Some(Created::Clip(id)) => id,
        other => panic!("clip: {other:?}"),
    };
    edit(
        service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 61,
            channel: 2,
            velocity: 99,
        },
    );
    let session_clip = match edit(
        service,
        Command::CreateSessionMidiClip {
            track: midi_track,
            scene,
            name: "Source".into(),
            source_length: BeatTime::from_quarters(4).expect("length"),
        },
    )
    .created
    {
        Some(Created::SessionClip(id)) => id,
        other => panic!("session clip: {other:?}"),
    };
    edit(
        service,
        Command::AddMidiNote {
            clip: session_clip,
            start: BeatTime::ONE,
            duration: BeatTime::ONE,
            pitch: 65,
            channel: 0,
            velocity: 88,
        },
    );
    let return_track = match edit(
        service,
        Command::AddReturnTrack {
            name: "Return".into(),
        },
    )
    .created
    {
        Some(Created::ReturnTrack(id)) => id,
        other => panic!("return: {other:?}"),
    };
    edit(
        service,
        Command::SetReturnSend {
            track,
            return_track,
            gain: 0.25,
        },
    );
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .expect("registered state");
    edit(
        service,
        Command::InsertReturnDevice {
            return_track,
            before: None,
            state,
        },
    );
    edit(
        service,
        Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(137_000_000).expect("tempo"),
        },
    );
    edit(
        service,
        Command::SetMeterPoint {
            beat: BeatTime::ZERO,
            meter: Meter::new(7, 8).expect("meter"),
        },
    )
    .state
    .project
    .as_ref()
    .clone()
}

fn render_session(event: &Event, partitions: &[usize]) -> Vec<[f32; 2]> {
    let prepared = event
        .playback
        .as_ref()
        .and_then(|p| p.prepared_revision())
        .expect("prepared graph")
        .clone();
    let scene = event.state.project.scenes()[0].id();
    let mut playback = hex_engine::playback::Playback::try_from_prepared_with_session(prepared)
        .expect("Session playback");
    playback
        .try_apply_action(hex_engine::PlaybackAction::LaunchAndPlay(
            hex_engine::SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
        ))
        .expect("explicit launch recreates runtime route");
    let mut output = vec![[0.0; 2]; 25_000];
    let mut cursor = 0;
    let mut index = 0;
    while cursor < output.len() {
        let end = (cursor + partitions[index % partitions.len()]).min(output.len());
        playback.render(&mut output[cursor..end]).expect("render");
        cursor = end;
        index += 1;
    }
    output
}

#[test]
fn abrupt_worker_process_exit_recovers_the_acknowledged_mixed_project() {
    let directory = Temp::new();
    wav(&directory.0.join("OriginalCase.wav"), 8);
    let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "crash_worker_child", "--nocapture"])
        .env("HEX_RECOVERY_WORKER_FIXTURE", &directory.0)
        .status()
        .expect("child worker process");
    assert_eq!(status.code(), Some(73));
    let expected =
        hex_storage::load(directory.0.join("expected.hex")).expect("acknowledged reference");
    let fresh = directory.service();
    wait(&fresh);
    let recovered = open(
        &fresh,
        ProjectSource::Journal(directory.slot()),
        BTreeMap::new(),
    );
    assert_eq!(recovered.state.project.as_ref(), &expected);
    assert!(recovered.state.unsaved && recovered.state.document_path.is_none());
    assert!(
        render_session(&recovered, &[1, 17, 257])
            .iter()
            .any(|frame| frame[0] != 0.0)
    );
}

#[test]
fn crash_worker_child() {
    let Some(root) = std::env::var_os("HEX_RECOVERY_WORKER_FIXTURE") else {
        return;
    };
    let directory = Temp(root.into());
    let service = directory.service();
    wait(&service);
    let authored = mixed_fixture(&service, &directory.0.join("OriginalCase.wav"));
    hex_storage::save_new(directory.0.join("expected.hex"), &authored)
        .expect("acknowledged reference");
    std::process::exit(73); // No endpoint/worker destructors or graceful shutdown.
}

#[test]
fn leftover_untitled_survives_worker_restart_and_failed_checkpoint_preserves_edits() {
    let directory = Temp::new();
    let first = directory.service();
    wait(&first);
    let old = edit(
        &first,
        Command::AddTrack {
            name: "first untitled".into(),
        },
    );
    assert_eq!(
        hex_storage::load_journal(&directory.slot()).expect("first checkpoint"),
        *old.state.project
    );
    let old_bytes = fs::read(directory.slot()).unwrap();
    drop(first);
    let latest = directory.service();
    wait(&latest);
    let new = edit(
        &latest,
        Command::AddTrack {
            name: "most recent untitled".into(),
        },
    );
    assert_eq!(
        hex_storage::load_journal(&directory.slot()).expect("latest only"),
        *new.state.project
    );
    let preserved = fs::read_dir(directory.slot().parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("preserved-recovery-")
        })
        .unwrap()
        .join(directory.slot().file_name().unwrap());
    assert_eq!(fs::read(&preserved).unwrap(), old_bytes);
    assert_eq!(
        hex_storage::load_journal(&preserved).unwrap(),
        *old.state.project
    );
    assert_eq!(
        new.status.as_deref(),
        Some("previous recovery journal preserved beside its original slot")
    );
    drop(latest);
    let restarted = directory.service();
    wait(&restarted);
    let recovered = open(
        &restarted,
        ProjectSource::Journal(directory.slot()),
        BTreeMap::new(),
    );
    assert_eq!(recovered.state.project, new.state.project);
    assert!(recovered.state.unsaved && recovered.state.document_path.is_none());
    assert!(recovered.playback.is_some());
    fs::write(directory.slot(), b"unrecognized external contents").expect("fault fixture");
    restarted
        .submit(Request::Edit(Command::AddTrack {
            name: "edit remains usable".into(),
        }))
        .expect("edit");
    let failed = wait(&restarted);
    assert!(
        failed
            .error
            .as_deref()
            .unwrap()
            .contains("project changed outside hex")
    );
    assert_eq!(failed.state.project.tracks().len(), 2);
    assert!(failed.state.unsaved && failed.state.can_undo && failed.playback.is_some());
    assert_eq!(
        fs::read(directory.slot()).expect("preserved external contents"),
        b"unrecognized external contents"
    );
}

#[test]
fn reopening_saved_project_preserves_leftover_journal_on_first_edit() {
    let directory = Temp::new();
    let path = directory.0.join("Saved.hex");
    let first = directory.service();
    wait(&first);
    request(&first, Request::SaveNew(path.clone()));
    let original = edit(
        &first,
        Command::AddTrack {
            name: "old edits".into(),
        },
    );
    let slot = hex_storage::journal_path(&path).unwrap();
    let bytes = fs::read(&slot).unwrap();
    drop(first);
    let second = directory.service();
    wait(&second);
    open(&second, ProjectSource::Saved(path), BTreeMap::new());
    let changed = edit(
        &second,
        Command::AddTrack {
            name: "new edits".into(),
        },
    );
    let preserved = fs::read_dir(directory.0.as_path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("preserved-recovery-")
        })
        .unwrap()
        .join(slot.file_name().unwrap());
    assert_eq!(fs::read(&preserved).unwrap(), bytes);
    assert_eq!(
        hex_storage::load_journal(&preserved).unwrap(),
        *original.state.project
    );
    assert_eq!(
        hex_storage::load_journal(&slot).unwrap(),
        *changed.state.project
    );
}

#[test]
fn mixed_project_relink_recovery_save_as_current_and_session_round_trip() {
    let directory = Temp::new();
    let media = directory.0.join("OriginalCase.wav");
    wav(&media, 8);
    let service = directory.service();
    wait(&service);
    let authored = mixed_fixture(&service, &media);
    assert_eq!(
        hex_storage::load_journal(&directory.slot()).expect("mixed checkpoint"),
        authored
    );
    let target = directory.0.join("Original.hex");
    request(&service, Request::SaveNew(target.clone()));
    assert!(
        !directory.slot().exists(),
        "durable Save As clears owned untitled slot"
    );
    let track = authored.tracks()[0].id();
    let dirty = edit(&service, Command::Gain { track, gain: 0.5 });
    let dirty_project = dirty.state.project.as_ref().clone();
    let original_pcm = render_session(&dirty, &[1, 17, 257]);
    let saved_bytes = fs::read(&target).expect("saved source bytes");
    let named_journal = hex_storage::journal_path(&target).expect("named slot");
    assert_eq!(
        hex_storage::load_journal(&named_journal).expect("saved edits"),
        dirty_project
    );
    assert_eq!(
        fs::read(&target).expect("saved source unchanged"),
        saved_bytes
    );
    drop(service);
    let moved = directory.0.join("MovedCase.wav");
    fs::rename(&media, &moved).expect("missing original media");
    let source = ProjectSource::SavedJournal(target.clone());
    let missing = missing_media_report(&source).expect("discover missing stable assets");
    // The Session cell re-imports the unchanged file onto the Arrangement
    // clip's resident asset, so one asset is missing.
    assert_eq!(missing.len(), 1);
    let restarted = directory.service();
    wait(&restarted);
    edit(
        &restarted,
        Command::AddTrack {
            name: "working project remains".into(),
        },
    );
    let working_path = directory.0.join("Working.hex");
    let initial = request(&restarted, Request::SaveNew(working_path.clone()));
    let wrong = directory.0.join("WrongLength.wav");
    wav(&wrong, 7);
    let bad_relinks = missing.iter().map(|m| (m.asset, wrong.clone())).collect();
    restarted
        .submit(Request::OpenWith {
            source: source.clone(),
            relinks: bad_relinks,
        })
        .expect("bad relink");
    let rejected = wait(&restarted);
    assert!(
        rejected
            .error
            .as_deref()
            .is_some_and(|s| s.contains("rate or frame count"))
    );
    assert!(rejected.open_complete && !rejected.project_replaced);
    assert_eq!(rejected.state.project, initial.state.project);
    assert_eq!(rejected.state.revision, initial.state.revision);
    assert_eq!(rejected.state.document_path, initial.state.document_path);
    assert_eq!(rejected.state.can_undo, initial.state.can_undo);
    assert!(rejected.playback.is_none());
    let restored = request(&restarted, Request::RestorePlayback { generation: 2 });
    assert_eq!(restored.state.project, initial.state.project);
    assert!(restored.playback.is_some());
    assert_eq!(
        fs::read(&target).expect("source preserved after refusal"),
        saved_bytes
    );
    let relinks = missing.iter().map(|m| (m.asset, moved.clone())).collect();
    let recovered = open(&restarted, source, relinks);
    let mut expected = dirty_project.snapshot();
    for asset in &mut expected.assets {
        asset.path = moved.clone();
    }
    let expected = Project::from_snapshot(expected).expect("expected path-only changes");
    assert_eq!(
        recovered.state.project.as_ref(),
        &expected,
        "all IDs, timeline, Session, devices, routing, maps and allocator survive"
    );
    assert!(recovered.state.unsaved && recovered.state.document_path.is_none());
    assert_eq!(
        fs::read(&target).expect("recovery never overwrites saved source"),
        saved_bytes
    );
    assert_eq!(
        render_session(&recovered, &[512, 31]),
        original_pcm,
        "relinked source and explicit Session relaunch preserve PCM across partitions"
    );
    let inventory = recovered
        .playback
        .as_ref()
        .and_then(|p| p.prepared_revision())
        .expect("prepared recovery")
        .session();
    assert_eq!(inventory.audio_clips().len(), 1);
    assert_eq!(inventory.midi_clips().len(), 1);
    let second_dir = directory.0.join("other");
    fs::create_dir(&second_dir).expect("different destination directory");
    let saved_as = second_dir.join("Relinked.hex");
    request(&restarted, Request::SaveNew(saved_as.clone()));
    assert!(!directory.slot().exists());
    edit(&restarted, Command::Gain { track, gain: 0.75 });
    let saved = request(&restarted, Request::SaveCurrent);
    assert!(!saved.state.unsaved);
    assert!(
        !hex_storage::journal_path(&saved_as)
            .expect("journal path")
            .exists()
    );
    let fresh = directory.service();
    wait(&fresh);
    let reopened = request(&fresh, Request::Open(saved_as.clone()));
    assert_eq!(reopened.state.project, saved.state.project);
    let saved_playback = request(&restarted, Request::RestorePlayback { generation: 99 });
    assert_eq!(
        render_session(&reopened, &[13, 257]),
        render_session(&saved_playback, &[256, 7])
    );
    assert_eq!(
        hex_storage::load_recovery(&saved_as)
            .expect("previous save is separate")
            .project,
        expected
    );
    assert_eq!(
        fs::read(&target).expect("original preserved throughout"),
        saved_bytes
    );
    let previous = open(
        &fresh,
        ProjectSource::PreviousSave(saved_as),
        BTreeMap::new(),
    );
    assert_eq!(previous.state.project.as_ref(), &expected);
    assert!(previous.state.unsaved && previous.state.document_path.is_none());
}
