use super::*;

#[test]
fn staged_edit_keeps_audio_control_polling_during_checkpoint() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    let directory = std::env::temp_dir().join(format!("hex-recovery-poll-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let requested = Arc::new(AtomicBool::new(false));
    let revision = Arc::new(AtomicU64::new(0));
    let (polled_tx, polled) = std::sync::mpsc::sync_channel(1);
    let mut service = Service::with_audio_sink(
        16,
        8_000_000,
        Some(path.clone()),
        Box::new(PollSink {
            requested: Arc::clone(&requested),
            revision: Arc::clone(&revision),
            polled: polled_tx,
        }),
    )
    .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "source".into(),
        }))
        .unwrap();
    let before = next(&service);
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    let locator = |frame| Request::Edit(Command::AddArrangementLocator { frame });
    service.submit(locator(24_000)).unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    service.submit(locator(48_000)).unwrap();
    let deadline = Instant::now() + Duration::from_millis(180);
    loop {
        if revision.load(Ordering::Acquire) == before.state.revision + 2 {
            break;
        }
        assert!(Instant::now() < deadline, "second edit did not commit");
        std::thread::yield_now();
    }
    requested.store(true, Ordering::Release);
    let result = polled.recv_timeout(Duration::from_millis(150));
    assert!(
        service.try_event().is_err(),
        "terminal preceded checkpoint release"
    );
    release.send(()).unwrap();
    assert!(
        result.is_ok(),
        "audio control was not polled while checkpoint was held"
    );
    assert_eq!(next(&service).state.revision, before.state.revision + 1);
    assert_eq!(next(&service).state.revision, before.state.revision + 2);
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn locator_recovery_timing_covers_the_held_checkpoint() {
    use crate::service::diagnostics::{DiagnosticObserver, DiagnosticStage};
    use std::time::Duration;

    let directory = std::env::temp_dir().join(format!("hex-locator-timing-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    next(&service);
    let measured = Arc::new(AtomicU64::new(u64::MAX));
    let output = Arc::clone(&measured);
    service
        .set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
            if matches!(event.stage, DiagnosticStage::OutcomeReady { .. })
                && event
                    .request
                    .is_some_and(|request| request.operation == Some("add_arrangement_locator"))
                && let Some(value) = event
                    .details
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("locator_recovery_us="))
                && let Ok(value) = value.parse::<u64>()
            {
                output.store(value, Ordering::Relaxed);
            }
        })))
        .unwrap();
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::Edit(Command::AddArrangementLocator {
            frame: 48_000,
        }))
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    assert!(
        service
            .events
            .recv_timeout(Duration::from_millis(20))
            .is_err()
    );
    release.send(()).unwrap();
    let outcome = next(&service);
    assert!(matches!(
        outcome.created,
        Some(Created::ArrangementLocator(_))
    ));
    assert!(outcome.error.is_none());
    let duration = measured.load(Ordering::Relaxed);
    assert!(
        duration != u64::MAX && duration >= 20_000,
        "missing or incomplete locator recovery timing: {duration}"
    );
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *outcome.state.project
    );
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn unread_track_switch_view_retains_latest_consecutive_commit() {
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "track".into(),
        }))
        .unwrap();
    let track = next(&service).state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    let first = next(&service);
    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    let second = next(&service);
    let view = service.try_track_switch_view().unwrap();
    assert_eq!(view.first_previous_revision + 1, first.state.revision);
    assert_eq!(view.previous_revision, first.state.revision);
    assert_eq!(view.revision, second.state.revision);
    assert!(
        !view
            .tracks
            .iter()
            .find(|item| item.track == track)
            .unwrap()
            .muted
    );
}

#[test]
fn committed_time_range_view_precedes_recovery_terminal() {
    use std::time::Duration;
    let directory =
        std::env::temp_dir().join(format!("hex-time-copy-display-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let track = prepare_paste(&service);
    let before = hex_storage::load_journal(&path).unwrap();
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::Edit(Command::CopyTimeRangeTo {
            range: hex_project::TimeRange::new(vec![track], 0, 24_000).unwrap(),
            track,
            frame: 24_000,
        }))
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let observed = service.try_arrangement_edit_view().ok();
    let terminal_early = service.try_event().is_ok();
    let journal_before_release = hex_storage::load_journal(&path).unwrap();
    release.send(()).unwrap();
    let view = observed.expect("committed range-copy view must precede checkpoint terminal");
    assert!(matches!(view.kind, ArrangementEditViewKind::TimeRangeCopy));
    let state = view.state.upgrade().unwrap();
    assert!(!terminal_early);
    assert_eq!(journal_before_release, before);
    assert_eq!(state.project.track(track).unwrap().midi_clips().len(), 2);
    let terminal = next(&service);
    assert!(terminal.error.is_none());
    assert_eq!(terminal.state.revision, view.previous_revision + 1);
    assert!(Arc::ptr_eq(&state, &terminal.state));
    assert_eq!(hex_storage::load_journal(&path).unwrap(), *state.project);
    service
        .submit(Request::Edit(Command::CopyTimeRangeTo {
            range: hex_project::TimeRange::new(vec![track], 0, 24_000).unwrap(),
            track,
            frame: u64::MAX,
        }))
        .unwrap();
    let refused = next(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.revision, state.revision);
    assert_eq!(*refused.state.project, *state.project);
    assert!(matches!(
        service.try_arrangement_edit_view(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn committed_locator_views_precede_ordered_recovery_terminals() {
    use std::time::{Duration, Instant};
    let (service, directory, path, _, initial) = locator_worker_fixture("views");
    let (entered, release) = hold_locator_checkpoint(&path);
    submit_locator(&service, 24_000);
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let until = Instant::now() + Duration::from_millis(180);
    let first_view = loop {
        if let Ok(view) = service.try_arrangement_edit_view() {
            break Some(view);
        }
        if Instant::now() >= until {
            break None;
        }
        std::thread::yield_now();
    };
    if first_view.is_none() {
        release.send(()).unwrap();
    }
    let first_view = first_view.expect("first committed locator view before recovery terminal");
    let first_state = first_view.state.upgrade().unwrap();
    assert_eq!(first_state.revision, initial + 1);
    let first_id = first_state.project.arrangement_locators()[0].id();
    assert!(matches!(first_view.kind, ArrangementEditViewKind::LocatorAdd(id) if id == first_id));
    assert_eq!(
        first_state
            .project
            .arrangement_locator(first_id)
            .unwrap()
            .frame(),
        24_000
    );
    submit_locator(&service, 48_000);
    let until = Instant::now() + Duration::from_millis(180);
    let second_view = loop {
        if let Ok(view) = service.try_arrangement_edit_view() {
            break Some(view);
        }
        if Instant::now() >= until {
            break None;
        }
        std::thread::yield_now();
    };
    assert!(matches!(
        service.try_event(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    release.send(()).unwrap();
    let second_view = second_view.expect("second committed locator view before recovery terminal");
    let second_state = second_view.state.upgrade().unwrap();
    assert_eq!(second_state.revision, initial + 2);
    let second_id = second_state.project.arrangement_locators()[1].id();
    assert!(matches!(second_view.kind, ArrangementEditViewKind::LocatorAdd(id) if id == second_id));
    assert_ne!(first_id, second_id);
    assert_eq!(
        second_state
            .project
            .arrangement_locator(second_id)
            .unwrap()
            .frame(),
        48_000
    );
    assert_eq!(
        first_view.project_generation,
        second_view.project_generation
    );
    assert_eq!(
        (first_view.previous_revision, second_view.previous_revision),
        (initial, initial + 1)
    );
    let first = next(&service);
    let second = next(&service);
    assert_eq!(first.created, Some(Created::ArrangementLocator(first_id)));
    assert_eq!(second.created, Some(Created::ArrangementLocator(second_id)));
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *second.state.project
    );
    submit_locator(&service, 48_000);
    next(&service);
    assert!(matches!(
        service.try_arrangement_edit_view(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    submit_locator(&service, u64::MAX);
    assert!(next(&service).error.is_some());
    assert!(matches!(
        service.try_arrangement_edit_view(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn consecutive_locator_adds_commit_while_first_checkpoint_is_held() {
    check_consecutive_locator_adds(false);
}

#[test]
fn consecutive_locator_adds_preserve_conflicting_recovery_bytes() {
    check_consecutive_locator_adds(true);
}

fn check_consecutive_locator_adds(conflict: bool) {
    use std::time::{Duration, Instant};
    let directory = std::env::temp_dir().join(format!(
        "hex-locator-queue-{}-{conflict}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let sources = Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = Service::with_audio_sink(
        16,
        8_000_000,
        Some(path.clone()),
        Box::new(SourceSink(Arc::clone(&sources))),
    )
    .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "source".into(),
        }))
        .unwrap();
    let initial = next(&service).state.revision;
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::Edit(Command::AddArrangementLocator {
            frame: 24_000,
        }))
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    if conflict {
        std::fs::write(&path, b"another writer's journal").unwrap();
    }
    service
        .submit(Request::Edit(Command::AddArrangementLocator {
            frame: 48_000,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_millis(180);
    let second_applied = loop {
        if sources
            .lock()
            .unwrap()
            .iter()
            .any(|source| source.editor_revision == initial + 2)
        {
            break true;
        }
        if Instant::now() >= until {
            break false;
        }
        std::thread::yield_now();
    };
    release.send(()).unwrap();
    assert!(
        second_applied,
        "second locator must commit while first checkpoint is held"
    );
    let first = next(&service);
    let second = next(&service);
    assert_eq!(
        (first.state.revision, second.state.revision),
        (initial + 1, initial + 2)
    );
    let (Some(Created::ArrangementLocator(first_id)), Some(Created::ArrangementLocator(second_id))) =
        (first.created, second.created)
    else {
        panic!("missing created locator identity")
    };
    assert_ne!(first_id, second_id);
    assert_eq!(
        second
            .state
            .project
            .arrangement_locator(first_id)
            .unwrap()
            .frame(),
        24_000
    );
    assert_eq!(
        second
            .state
            .project
            .arrangement_locator(second_id)
            .unwrap()
            .frame(),
        48_000
    );
    if conflict {
        assert!(first.recovery_failure.is_some() && second.recovery_failure.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), b"another writer's journal");
    } else {
        assert!(first.error.is_none() && second.error.is_none());
        assert_eq!(
            hex_storage::load_journal(&path).unwrap(),
            *second.state.project
        );
    }
    for (request, count) in [
        (Request::Undo, 1),
        (Request::Undo, 0),
        (Request::Redo, 1),
        (Request::Redo, 2),
    ] {
        service.submit(request).unwrap();
        assert_eq!(
            next(&service).state.project.arrangement_locators().len(),
            count
        );
    }
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

fn locator_worker_fixture(
    label: &str,
) -> (
    Service,
    PathBuf,
    PathBuf,
    Arc<std::sync::Mutex<Vec<hex_engine::AudioSourceContext>>>,
    u64,
) {
    let directory =
        std::env::temp_dir().join(format!("hex-locator-{label}-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let sources = Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = Service::with_audio_sink(
        16,
        8_000_000,
        Some(path.clone()),
        Box::new(SourceSink(Arc::clone(&sources))),
    )
    .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "source".into(),
        }))
        .unwrap();
    let revision = next(&service).state.revision;
    (service, directory, path, sources, revision)
}

fn hold_locator_checkpoint(
    path: &Path,
) -> (
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::SyncSender<()>,
) {
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.to_owned(), entered_tx, release_rx));
    (entered, release)
}

fn submit_locator(service: &Service, frame: u64) {
    service
        .submit(Request::Edit(Command::AddArrangementLocator { frame }))
        .unwrap();
}

fn wait_locator_revision(
    sources: &std::sync::Mutex<Vec<hex_engine::AudioSourceContext>>,
    revision: u64,
) {
    let until = std::time::Instant::now() + std::time::Duration::from_millis(180);
    while !sources
        .lock()
        .unwrap()
        .iter()
        .any(|source| source.editor_revision == revision)
    {
        assert!(
            std::time::Instant::now() < until,
            "locator source was not acknowledged"
        );
        std::thread::yield_now();
    }
}

#[test]
fn locator_checkpoint_bounds_third_add_and_preserves_duplicate_noop() {
    use std::time::{Duration, Instant};
    let (service, directory, path, sources, initial) = locator_worker_fixture("bound");
    let (entered, release) = hold_locator_checkpoint(&path);
    submit_locator(&service, 24_000);
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    submit_locator(&service, 48_000);
    wait_locator_revision(&sources, initial + 2);
    submit_locator(&service, 72_000);
    let until = Instant::now() + Duration::from_millis(30);
    while Instant::now() < until {
        assert!(
            !sources
                .lock()
                .unwrap()
                .iter()
                .any(|source| source.editor_revision == initial + 3)
        );
        assert!(matches!(
            service.try_event(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        std::thread::yield_now();
    }
    release.send(()).unwrap();
    for revision in initial + 1..=initial + 3 {
        let event = next(&service);
        assert_eq!(event.state.revision, revision);
        assert!(matches!(
            event.created,
            Some(Created::ArrangementLocator(_))
        ));
        assert!(event.error.is_none());
    }
    submit_locator(&service, 72_000);
    let duplicate = next(&service);
    assert!(!matches!(
        duplicate.created,
        Some(Created::ArrangementLocator(_))
    ));
    assert_eq!(duplicate.state.project.arrangement_locators().len(), 3);
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *duplicate.state.project
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn duplicate_locator_during_checkpoint_keeps_one_identity_and_undo() {
    use std::time::Duration;
    let (service, directory, path, _, _) = locator_worker_fixture("duplicate");
    let (entered, release) = hold_locator_checkpoint(&path);
    submit_locator(&service, 24_000);
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    submit_locator(&service, 24_000);
    assert!(
        service
            .events
            .recv_timeout(Duration::from_millis(30))
            .is_err()
    );
    release.send(()).unwrap();
    let first = next(&service);
    let Some(Created::ArrangementLocator(id)) = first.created else {
        panic!("missing created locator")
    };
    let duplicate = next(&service);
    assert!(!matches!(
        duplicate.created,
        Some(Created::ArrangementLocator(_))
    ));
    assert_eq!(duplicate.state.project.arrangement_locators().len(), 1);
    assert!(duplicate.state.project.arrangement_locator(id).is_some());
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *duplicate.state.project
    );
    service.submit(Request::Undo).unwrap();
    assert!(
        next(&service)
            .state
            .project
            .arrangement_locators()
            .is_empty()
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn committed_locators_survive_consumer_closure() {
    check_locator_custody(true);
}

#[test]
fn committed_locators_survive_shutdown() {
    check_locator_custody(false);
}

fn check_locator_custody(close_consumer: bool) {
    use std::time::{Duration, Instant};
    let (mut service, directory, path, sources, initial) =
        locator_worker_fixture(if close_consumer { "closed" } else { "shutdown" });
    let mut expected =
        hex_project::Editor::new(hex_storage::load_journal(&path).unwrap(), 16).unwrap();
    let (entered, release) = hold_locator_checkpoint(&path);
    for frame in [24_000, 48_000] {
        expected
            .execute(Command::AddArrangementLocator { frame })
            .unwrap();
        submit_locator(&service, frame);
        if frame == 24_000 {
            entered.recv_timeout(Duration::from_millis(500)).unwrap();
        }
    }
    wait_locator_revision(&sources, initial + 2);
    if close_consumer {
        let (_sender, receiver) = std::sync::mpsc::sync_channel(1);
        let replacement = crate::service::diagnostics::EventReceiver::new(
            receiver,
            Arc::clone(&service.diagnostics),
        );
        drop(std::mem::replace(&mut service.events, replacement));
    } else {
        assert!(service.shutdown(Duration::from_millis(20)).is_err());
    }
    release.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while !service
        .worker
        .as_ref()
        .is_some_and(std::thread::JoinHandle::is_finished)
    {
        assert!(Instant::now() < until, "service did not stop");
        std::thread::yield_now();
    }
    service.shutdown(Duration::from_secs(2)).unwrap();
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *expected.project()
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn locator_checkpoint_fences_save_and_open() {
    use std::time::{Duration, Instant};
    for open in [false, true] {
        let (service, directory, path, _, _) =
            locator_worker_fixture(if open { "open" } else { "save" });
        let target = directory.join("other.hex");
        let other = Project::new(48_000).unwrap();
        if open {
            hex_storage::save_new(&target, &other).unwrap();
        }
        let (entered, release) = hold_locator_checkpoint(&path);
        submit_locator(&service, 24_000);
        entered.recv_timeout(Duration::from_millis(500)).unwrap();
        service
            .submit(if open {
                Request::Open(target.clone())
            } else {
                Request::SaveNew(target.clone())
            })
            .unwrap();
        let until = Instant::now() + Duration::from_millis(30);
        while Instant::now() < until {
            assert!(matches!(
                service.try_event(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            if !open {
                assert!(!target.exists());
            }
            std::thread::yield_now();
        }
        release.send(()).unwrap();
        let added = next(&service);
        assert!(matches!(
            added.created,
            Some(Created::ArrangementLocator(_))
        ));
        let terminal = next(&service);
        assert!(terminal.error.is_none());
        if open {
            assert!(terminal.project_replaced);
            assert_eq!(*terminal.state.project, other);
            assert_eq!(
                hex_storage::load_journal(&path).unwrap(),
                *added.state.project
            );
        } else {
            assert!(terminal.save_complete);
            assert_eq!(hex_storage::load(&target).unwrap(), *added.state.project);
        }
        drop(service);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn locator_checkpoint_fences_mixed_edits() {
    use std::time::{Duration, Instant};
    let (service, directory, path, sources, initial) = locator_worker_fixture("mixed");
    let (entered, release) = hold_locator_checkpoint(&path);
    submit_locator(&service, 24_000);
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "later".into(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_millis(30);
    while Instant::now() < until {
        assert!(
            !sources
                .lock()
                .unwrap()
                .iter()
                .any(|source| source.editor_revision == initial + 2)
        );
        assert!(matches!(
            service.try_event(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        std::thread::yield_now();
    }
    release.send(()).unwrap();
    assert!(matches!(
        next(&service).created,
        Some(Created::ArrangementLocator(_))
    ));
    let later = next(&service);
    assert!(matches!(later.created, Some(Created::Track(_))));
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *later.state.project
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn consecutive_time_range_copies_commit_while_first_checkpoint_is_held() {
    check_consecutive_time_range_copies(false);
}

#[test]
fn consecutive_time_range_copies_preserve_conflicting_recovery_bytes() {
    check_consecutive_time_range_copies(true);
}

fn check_consecutive_time_range_copies(conflict: bool) {
    use std::time::{Duration, Instant};
    let directory = std::env::temp_dir().join(format!(
        "hex-range-copy-queue-{}-{conflict}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let sources = Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = Service::with_audio_sink(
        16,
        8_000_000,
        Some(path.clone()),
        Box::new(SourceSink(Arc::clone(&sources))),
    )
    .unwrap();
    let track = prepare_paste(&service);
    let initial_revision = sources.lock().unwrap().last().unwrap().editor_revision;
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    for start in [0, 24_000] {
        let range = hex_project::TimeRange::new(vec![track], start, start + 24_000).unwrap();
        service
            .submit(Request::Edit(Command::CopyTimeRangeTo {
                range,
                track,
                frame: start + 24_000,
            }))
            .unwrap();
        if start == 0 {
            entered.recv_timeout(Duration::from_millis(500)).unwrap();
            if conflict {
                std::fs::write(&path, b"another writer's journal").unwrap();
            }
        }
    }
    let until = Instant::now() + Duration::from_millis(180);
    let second_applied = loop {
        if sources
            .lock()
            .unwrap()
            .iter()
            .any(|source| source.editor_revision == initial_revision + 2)
        {
            break true;
        }
        if Instant::now() >= until {
            break false;
        }
        std::thread::yield_now();
    };
    release.send(()).unwrap();
    assert!(
        second_applied,
        "second copy must commit while first checkpoint is held"
    );
    let first = next(&service);
    let second = next(&service);
    assert_eq!(first.state.revision, initial_revision + 1);
    assert_eq!(second.state.revision, initial_revision + 2);
    let revisions: Vec<_> = sources
        .lock()
        .unwrap()
        .iter()
        .filter(|context| context.editor_revision > initial_revision)
        .map(|context| context.editor_revision)
        .collect();
    assert_eq!(revisions, [initial_revision + 1, initial_revision + 2]);
    if conflict {
        assert!(first.recovery_failure.is_some() && second.recovery_failure.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), b"another writer's journal");
    } else {
        assert!(first.error.is_none() && second.error.is_none());
        assert_eq!(
            hex_storage::load_journal(&path).unwrap(),
            *second.state.project
        );
    }
    assert_eq!(
        second
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        3
    );
    service.submit(Request::Undo).unwrap();
    assert_eq!(
        next(&service)
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        2
    );
    service.submit(Request::Undo).unwrap();
    assert_eq!(
        next(&service)
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        1
    );
    service.submit(Request::Redo).unwrap();
    assert_eq!(
        next(&service)
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        2
    );
    service.submit(Request::Redo).unwrap();
    assert_eq!(
        next(&service)
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        3
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn committed_second_time_copy_survives_consumer_closure() {
    check_time_copy_custody_on_exit(true);
}

#[test]
fn committed_second_time_copy_survives_shutdown() {
    check_time_copy_custody_on_exit(false);
}

fn check_time_copy_custody_on_exit(close_consumer: bool) {
    use std::time::{Duration, Instant};
    let directory = std::env::temp_dir().join(format!(
        "hex-time-copy-exit-{}-{close_consumer}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let sources = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut service = Service::with_audio_sink(
        16,
        8_000_000,
        Some(path.clone()),
        Box::new(SourceSink(Arc::clone(&sources))),
    )
    .unwrap();
    let track = prepare_paste(&service);
    let initial_revision = sources.lock().unwrap().last().unwrap().editor_revision;
    let mut expected =
        hex_project::Editor::new(hex_storage::load_journal(&path).unwrap(), 16).unwrap();
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    for start in [0, 24_000] {
        let make = || Command::CopyTimeRangeTo {
            range: hex_project::TimeRange::new(vec![track], start, start + 24_000).unwrap(),
            track,
            frame: start + 24_000,
        };
        expected.execute(make()).unwrap();
        service.submit(Request::Edit(make())).unwrap();
        if start == 0 {
            entered.recv_timeout(Duration::from_millis(500)).unwrap();
        }
    }
    let until = Instant::now() + Duration::from_millis(180);
    while sources
        .lock()
        .unwrap()
        .last()
        .is_none_or(|context| context.editor_revision != initial_revision + 2)
    {
        assert!(
            Instant::now() < until,
            "second copy source was not acknowledged"
        );
        std::thread::yield_now();
    }
    if close_consumer {
        let (_sender, receiver) = std::sync::mpsc::sync_channel(1);
        let replacement = crate::service::diagnostics::EventReceiver::new(
            receiver,
            Arc::clone(&service.diagnostics),
        );
        drop(std::mem::replace(&mut service.events, replacement));
    } else {
        assert!(service.shutdown(Duration::from_millis(20)).is_err());
    }
    release.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while !service
        .worker
        .as_ref()
        .is_some_and(std::thread::JoinHandle::is_finished)
    {
        assert!(Instant::now() < until, "service did not stop");
        std::thread::yield_now();
    }
    service.shutdown(Duration::from_secs(2)).unwrap();
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *expected.project()
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn time_range_copy_checkpoint_fences_save() {
    check_time_copy_document_fence(false);
}

#[test]
fn time_range_copy_checkpoint_fences_open() {
    check_time_copy_document_fence(true);
}

fn check_time_copy_document_fence(open: bool) {
    use std::time::{Duration, Instant};
    let directory = std::env::temp_dir().join(format!(
        "hex-time-copy-document-{}-{open}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let journal = directory.join("untitled.hex-journal");
    let target = directory.join("other.hex");
    let other = Project::new(48_000).unwrap();
    if open {
        hex_storage::save_new(&target, &other).unwrap();
    }
    let service = Service::with_recovery(48_000, 16, 8_000_000, journal.clone()).unwrap();
    let track = prepare_paste(&service);
    let mut expected =
        hex_project::Editor::new(hex_storage::load_journal(&journal).unwrap(), 16).unwrap();
    let command = || Command::CopyTimeRangeTo {
        range: hex_project::TimeRange::new(vec![track], 0, 24_000).unwrap(),
        track,
        frame: 24_000,
    };
    expected.execute(command()).unwrap();
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((journal.clone(), entered_tx, release_rx));
    service.submit(Request::Edit(command())).unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    service
        .submit(if open {
            Request::Open(target.clone())
        } else {
            Request::SaveNew(target.clone())
        })
        .unwrap();
    let until = Instant::now() + Duration::from_millis(60);
    while Instant::now() < until {
        assert!(matches!(
            service.try_event(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        if !open {
            assert!(!target.exists());
        }
        std::thread::yield_now();
    }
    release.send(()).unwrap();
    let copied = next(&service);
    assert!(copied.error.is_none());
    assert_eq!(*copied.state.project, *expected.project());
    let terminal = next(&service);
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    if open {
        assert!(terminal.project_replaced);
        assert_eq!(*terminal.state.project, other);
        assert_eq!(
            hex_storage::load_journal(&journal).unwrap(),
            *expected.project()
        );
    } else {
        assert!(terminal.save_complete);
        assert_eq!(hex_storage::load(&target).unwrap(), *expected.project());
    }
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn third_time_copy_waits_for_the_single_checkpoint_slot() {
    use std::time::{Duration, Instant};
    let directory = std::env::temp_dir().join(format!("hex-time-copy-full-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let sources = Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = Service::with_audio_sink(
        16,
        8_000_000,
        Some(path.clone()),
        Box::new(SourceSink(Arc::clone(&sources))),
    )
    .unwrap();
    let track = prepare_paste(&service);
    let initial = sources.lock().unwrap().last().unwrap().editor_revision;
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    for start in [0, 24_000] {
        service
            .submit(Request::Edit(Command::CopyTimeRangeTo {
                range: hex_project::TimeRange::new(vec![track], start, start + 24_000).unwrap(),
                track,
                frame: start + 24_000,
            }))
            .unwrap();
        if start == 0 {
            entered.recv_timeout(Duration::from_millis(500)).unwrap();
        }
    }
    let until = Instant::now() + Duration::from_millis(180);
    while sources
        .lock()
        .unwrap()
        .last()
        .is_none_or(|context| context.editor_revision != initial + 2)
    {
        assert!(Instant::now() < until, "second copy did not commit");
        std::thread::yield_now();
    }
    service
        .submit(Request::Edit(Command::CopyTimeRangeTo {
            range: hex_project::TimeRange::new(vec![track], 48_000, 72_000).unwrap(),
            track,
            frame: 72_000,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_millis(60);
    while Instant::now() < until {
        assert_eq!(
            sources.lock().unwrap().last().unwrap().editor_revision,
            initial + 2
        );
        std::thread::yield_now();
    }
    release.send(()).unwrap();
    let first = next(&service);
    let second = next(&service);
    let third = next(&service);
    assert!(first.error.is_none() && second.error.is_none() && third.error.is_none());
    assert_eq!(
        (
            first.state.revision,
            second.state.revision,
            third.state.revision
        ),
        (initial + 1, initial + 2, initial + 3)
    );
    assert_eq!(
        third.state.project.track(track).unwrap().midi_clips().len(),
        4
    );
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *third.state.project
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn track_switch_waits_for_pending_time_copy_checkpoint() {
    use std::time::Duration;
    let directory =
        std::env::temp_dir().join(format!("hex-time-copy-mute-fence-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let track = prepare_paste(&service);
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::Edit(Command::CopyTimeRangeTo {
            range: hex_project::TimeRange::new(vec![track], 0, 24_000).unwrap(),
            track,
            frame: 24_000,
        }))
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    assert!(matches!(
        service.events.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(matches!(
        service.try_track_switch_view(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    release.send(()).unwrap();
    let first = next(&service);
    let second = next(&service);
    assert!(first.error.is_none() && second.error.is_none());
    assert_eq!(
        first.state.project.track(track).unwrap().midi_clips().len(),
        2
    );
    assert!(second.state.project.track(track).unwrap().muted());
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *second.state.project
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}
