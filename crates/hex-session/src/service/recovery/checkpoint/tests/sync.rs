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
    wait_for_journal(&path, &before.state.project);
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    let locator = |frame| Request::Edit(Command::AddArrangementLocator { frame });
    service.submit(locator(24_000)).unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    assert_eq!(next(&service).state.revision, before.state.revision + 1);
    service.submit(locator(48_000)).unwrap();
    let second = next(&service);
    assert_eq!(second.state.revision, before.state.revision + 2);
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
    release.send(()).unwrap();
    assert!(
        result.is_ok(),
        "audio control was not polled while checkpoint was held"
    );
    wait_for_journal(&path, &second.state.project);
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn locator_recovery_timing_records_scheduling_without_blocking_on_held_checkpoint() {
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
    // Edit ACK must arrive immediately while the checkpoint gate is still held.
    let outcome = next(&service);
    assert!(matches!(
        outcome.created,
        Some(Created::ArrangementLocator(_))
    ));
    assert!(outcome.error.is_none());
    let duration = measured.load(Ordering::Relaxed);
    assert!(
        duration != u64::MAX,
        "missing locator recovery scheduling timing"
    );
    release.send(()).unwrap();
    wait_for_journal(&path, &outcome.state.project);
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
fn committed_time_range_view_and_ack_precede_checkpoint_completion() {
    use std::time::Duration;
    let directory =
        std::env::temp_dir().join(format!("hex-time-copy-display-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let track = prepare_paste(&service, &path);
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
    let terminal = next(&service);
    let view = service
        .try_arrangement_edit_view()
        .expect("committed range-copy view");
    let journal_before_release = hex_storage::load_journal(&path).unwrap();
    release.send(()).unwrap();
    assert!(matches!(view.kind, ArrangementEditViewKind::TimeRangeCopy));
    let state = view.state.upgrade().unwrap();
    assert_eq!(journal_before_release, before);
    assert_eq!(state.project.track(track).unwrap().midi_clips().len(), 2);
    assert!(terminal.error.is_none());
    assert_eq!(terminal.state.revision, view.previous_revision + 1);
    assert!(Arc::ptr_eq(&state, &terminal.state));
    wait_for_journal(&path, &state.project);
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
fn committed_locator_views_and_acks_precede_checkpoint_completion() {
    use std::time::Duration;
    let (service, directory, path, _, initial) = locator_worker_fixture("views");
    let (entered, release) = hold_locator_checkpoint(&path);
    submit_locator(&service, 24_000);
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let first = next(&service);
    let first_view = service
        .try_arrangement_edit_view()
        .expect("first committed locator view");
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
    let second = next(&service);
    let second_view = service
        .try_arrangement_edit_view()
        .expect("second committed locator view");
    release.send(()).unwrap();
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
    assert_eq!(first.created, Some(Created::ArrangementLocator(first_id)));
    assert_eq!(second.created, Some(Created::ArrangementLocator(second_id)));
    wait_for_journal(&path, &second.state.project);
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
    let initial_event = next(&service);
    let initial = initial_event.state.revision;
    wait_for_journal(&path, &initial_event.state.project);
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
    let first = next(&service);
    if conflict {
        std::fs::write(&path, b"another writer's journal").unwrap();
    }
    service
        .submit(Request::Edit(Command::AddArrangementLocator {
            frame: 48_000,
        }))
        .unwrap();
    let second = next(&service);
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
        let failure = wait_for_recovery_failure(&service);
        assert!(!failure.request_complete && failure.recovery_failure.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), b"another writer's journal");
    } else {
        assert!(first.error.is_none() && second.error.is_none());
        wait_for_journal(&path, &second.state.project);
    }
    for (request, count) in [
        (Request::Undo, 1),
        (Request::Undo, 0),
        (Request::Redo, 1),
        (Request::Redo, 2),
    ] {
        service.submit(request).unwrap();
        assert_eq!(
            next_complete(&service)
                .state
                .project
                .arrangement_locators()
                .len(),
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
    let added = next(&service);
    wait_for_journal(&path, &added.state.project);
    (service, directory, path, sources, added.state.revision)
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
fn locator_checkpoint_coalesces_third_add_and_preserves_duplicate_noop() {
    use std::time::Duration;
    let (service, directory, path, sources, initial) = locator_worker_fixture("bound");
    let (entered, release) = hold_locator_checkpoint(&path);
    submit_locator(&service, 24_000);
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let first = next(&service);
    assert_eq!(first.state.revision, initial + 1);
    submit_locator(&service, 48_000);
    let second = next(&service);
    assert_eq!(second.state.revision, initial + 2);
    wait_locator_revision(&sources, initial + 2);
    // Third edit commits and ACKs immediately while the first checkpoint is still held.
    submit_locator(&service, 72_000);
    let third = next(&service);
    assert_eq!(third.state.revision, initial + 3);
    assert!(matches!(
        third.created,
        Some(Created::ArrangementLocator(_))
    ));
    assert!(third.error.is_none());
    wait_locator_revision(&sources, initial + 3);
    release.send(()).unwrap();
    wait_for_journal(&path, &third.state.project);
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
    let first = next(&service);
    let Some(Created::ArrangementLocator(id)) = first.created else {
        panic!("missing created locator")
    };
    submit_locator(&service, 24_000);
    let duplicate = next(&service);
    assert!(!matches!(
        duplicate.created,
        Some(Created::ArrangementLocator(_))
    ));
    assert_eq!(duplicate.state.project.arrangement_locators().len(), 1);
    assert!(duplicate.state.project.arrangement_locator(id).is_some());
    release.send(()).unwrap();
    wait_for_journal(&path, &duplicate.state.project);
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
        let added = next(&service);
        assert!(matches!(
            added.created,
            Some(Created::ArrangementLocator(_))
        ));
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
fn locator_checkpoint_does_not_block_mixed_edits_and_coalesces_journal() {
    use std::time::Duration;
    let (service, directory, path, sources, initial) = locator_worker_fixture("mixed");
    let (entered, release) = hold_locator_checkpoint(&path);
    submit_locator(&service, 24_000);
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let locator = next(&service);
    assert!(matches!(
        locator.created,
        Some(Created::ArrangementLocator(_))
    ));
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "later".into(),
        }))
        .unwrap();
    let later = next(&service);
    assert!(matches!(later.created, Some(Created::Track(_))));
    assert_eq!(later.state.revision, initial + 2);
    wait_locator_revision(&sources, initial + 2);
    release.send(()).unwrap();
    wait_for_journal(&path, &later.state.project);
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
    use std::time::Duration;
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
    let track = prepare_paste(&service, &path);
    let initial_revision = sources.lock().unwrap().last().unwrap().editor_revision;
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    let mut terminals = Vec::new();
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
        terminals.push(next(&service));
    }
    let second = terminals.pop().unwrap();
    let first = terminals.pop().unwrap();
    release.send(()).unwrap();
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
        let failure = wait_for_recovery_failure(&service);
        assert!(!failure.request_complete && failure.recovery_failure.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), b"another writer's journal");
    } else {
        assert!(first.error.is_none() && second.error.is_none());
        wait_for_journal(&path, &second.state.project);
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
        next_complete(&service)
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
        next_complete(&service)
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
        next_complete(&service)
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
        next_complete(&service)
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
    let track = prepare_paste(&service, &path);
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
    let track = prepare_paste(&service, &journal);
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
    let copied = next(&service);
    assert!(copied.error.is_none());
    assert_eq!(*copied.state.project, *expected.project());
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
fn third_time_copy_coalesces_without_waiting_for_in_flight_checkpoint() {
    use std::time::Duration;
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
    let track = prepare_paste(&service, &path);
    let initial = sources.lock().unwrap().last().unwrap().editor_revision;
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    let mut terminals = Vec::new();
    for start in [0, 24_000, 48_000] {
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
        terminals.push(next(&service));
    }
    let third = terminals.pop().unwrap();
    let second = terminals.pop().unwrap();
    let first = terminals.pop().unwrap();
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
    release.send(()).unwrap();
    wait_for_journal(&path, &third.state.project);
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn track_switch_does_not_wait_for_pending_time_copy_checkpoint() {
    use std::time::Duration;
    let directory =
        std::env::temp_dir().join(format!("hex-time-copy-mute-fence-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let track = prepare_paste(&service, &path);
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
    let first = next(&service);
    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    let second = next(&service);
    assert!(first.error.is_none() && second.error.is_none());
    assert_eq!(
        first.state.project.track(track).unwrap().midi_clips().len(),
        2
    );
    assert!(second.state.project.track(track).unwrap().muted());
    release.send(()).unwrap();
    wait_for_journal(&path, &second.state.project);
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rapid_mixed_edits_coalesce_checkpoints_and_ack_without_fsync_delay() {
    use hex_project::BeatTime;
    use std::time::{Duration, Instant};

    let directory =
        std::env::temp_dir().join(format!("hex-coalesce-latency-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 64, 8_000_000, path.clone()).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "piano".into(),
        }))
        .unwrap();
    let track = next(&service).state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let placed = next(&service);
    let clip = placed.state.project.track(track).unwrap().midi_clips()[0].id();
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .unwrap();
    let with_note = next(&service);
    let Some(Created::Note(note)) = with_note.created else {
        panic!("expected note");
    };
    wait_for_journal(&path, &with_note.state.project);

    // 1. Hold checkpoint #1 and fire 16 mixed edits (clip moves, note moves, mutes, undo).
    // All 16 must ACK immediately without blocking on CHECKPOINT_GATE, and coalesce into
    // at most ONE follow-up disk checkpoint after gate release.
    let (entered_1_tx, entered_1) = std::sync::mpsc::sync_channel(1);
    let (release_1, release_1_rx) = std::sync::mpsc::sync_channel(1);
    let (entered_2_tx, entered_2) = std::sync::mpsc::sync_channel(1);
    let (release_2, release_2_rx) = std::sync::mpsc::sync_channel(1);
    let (entered_3_tx, entered_3) = std::sync::mpsc::sync_channel(1);
    let (release_3, release_3_rx) = std::sync::mpsc::sync_channel(1);
    {
        let mut gates = CHECKPOINT_GATE.lock().unwrap();
        gates.push((path.clone(), entered_1_tx, release_1_rx));
        gates.push((path.clone(), entered_2_tx, release_2_rx));
        gates.push((path.clone(), entered_3_tx, release_3_rx));
    }

    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    entered_1.recv_timeout(Duration::from_millis(500)).unwrap();
    let mut latest = next(&service);
    assert!(latest.error.is_none());

    for step in 1_u64..=15 {
        let request = match step % 4 {
            0 => Request::Undo,
            1 => Request::Edit(Command::MoveMidiClip {
                clip,
                track,
                start: BeatTime::from_quarters(step).unwrap(),
            }),
            2 => Request::Edit(Command::UpdateMidiNote {
                note,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60 + (step as u8 % 12),
                channel: 0,
                velocity: 100,
            }),
            _ => Request::Edit(Command::ToggleSolo {
                track,
                exclusive: true,
            }),
        };
        service.submit(request).unwrap();
        latest = next(&service);
        assert!(latest.error.is_none(), "step {step}: {:?}", latest.error);
    }

    // Release gate #1 -> coalesced snapshot runs as checkpoint #2.
    release_1.send(()).unwrap();
    entered_2.recv_timeout(Duration::from_millis(500)).unwrap();
    release_2.send(()).unwrap();
    wait_for_journal(&path, &latest.state.project);
    // Gate #3 must NOT be entered: all 15 rapid edits coalesced into checkpoint #2.
    assert!(
        entered_3.recv_timeout(Duration::from_millis(30)).is_err(),
        "rapid edits should coalesce into a single follow-up checkpoint"
    );
    // Clean up unused gate #3 before real-fsync latency checks.
    drop(release_3);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .retain(|(target, _, _)| target != &path);

    // 2. Measure real recovery-enabled worker edit-to-ACK latency for clip move,
    // note move, mute toggle, and undo (each must stay well clear of 3x fsync latency).
    for request in [
        Request::Edit(Command::MoveMidiClip {
            clip,
            track,
            start: BeatTime::from_quarters(2).unwrap(),
        }),
        Request::Edit(Command::UpdateMidiNote {
            note,
            start: BeatTime::ONE,
            duration: BeatTime::ONE,
            pitch: 67,
            channel: 0,
            velocity: 100,
        }),
        Request::Edit(Command::ToggleMute(track)),
        Request::Undo,
    ] {
        let started = Instant::now();
        service.submit(request).unwrap();
        latest = next(&service);
        let elapsed = started.elapsed();
        assert!(latest.error.is_none());
        assert!(
            elapsed < Duration::from_millis(50),
            "edit ACK should not wait on recovery fsync, took {elapsed:?}"
        );
    }

    service.shutdown(Duration::from_secs(2)).unwrap();
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *latest.state.project
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}
