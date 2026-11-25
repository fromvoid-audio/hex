use super::*;

#[test]
fn track_switch_observation_does_not_hide_a_recovery_conflict() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "hex-solo-conflict-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "first".into(),
        }))
        .unwrap();
    let added = next(&service);
    wait_for_journal(&path, &added.state.project);
    let track = added.state.project.tracks()[0].id();
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::Edit(Command::ToggleSolo {
            track,
            exclusive: true,
        }))
        .unwrap();
    entered
        .recv_timeout(std::time::Duration::from_millis(500))
        .unwrap();
    let view = service.try_track_switch_view().unwrap();
    assert!(
        view.tracks
            .iter()
            .any(|switch| switch.track == track && switch.soloed)
    );
    let terminal = next(&service);
    assert!(terminal.request_complete && terminal.error.is_none());
    assert!(terminal.state.project.track(track).unwrap().soloed());
    std::fs::write(&path, b"other writer").unwrap();
    release.send(()).unwrap();
    let failure = wait_for_recovery_failure(&service);
    assert!(!failure.request_complete && failure.error.is_some());
    assert!(failure.recovery_failure.is_some());
    assert!(failure.state.project.track(track).unwrap().soloed());
    assert_eq!(std::fs::read(&path).unwrap(), b"other writer");
    service.shutdown(std::time::Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn unread_track_switch_observation_never_blocks_later_terminals() {
    let mut service = Service::new(48_000, 16, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "first".into(),
        }))
        .unwrap();
    let added = next(&service);
    let track = added.state.project.tracks()[0].id();
    for expected in [true, false] {
        service
            .submit(Request::Edit(Command::ToggleSolo {
                track,
                exclusive: true,
            }))
            .unwrap();
        let terminal = next(&service);
        assert!(terminal.request_complete && terminal.error.is_none());
        assert_eq!(
            terminal.state.project.track(track).unwrap().soloed(),
            expected
        );
    }
    let retained = service.try_track_switch_view().unwrap();
    assert_eq!(retained.first_previous_revision, added.state.revision);
    assert_eq!(retained.revision, added.state.revision + 2);
    assert!(
        retained
            .tracks
            .iter()
            .any(|switch| switch.track == track && !switch.soloed)
    );
    service.shutdown(std::time::Duration::from_secs(2)).unwrap();
}

#[test]
fn unavailable_slot_is_persistent_and_confirmed_restart_preserves_history_save_and_recovery() {
    let fixture = Fixture::new();
    let first = fixture.edit("first");
    assert!(first.error.is_none());
    assert!(first.state.unsaved && first.state.can_undo);
    assert_eq!(first.state.project.tracks().len(), 1);
    let failure_event = wait_for_recovery_failure(&fixture.service);
    assert!(
        failure_event
            .error
            .as_ref()
            .unwrap()
            .contains("unsupported older project schema")
    );
    let failure = failure_event.recovery_failure.as_ref().unwrap();
    assert!(failure.can_restart());
    let second = fixture.edit("second");
    assert!(
        second.error.is_none(),
        "do not repeat the same unsupported checkpoint attempt"
    );
    assert!(Arc::ptr_eq(
        second.recovery_failure.as_ref().unwrap(),
        failure
    ));
    assert_eq!(std::fs::read(&fixture.path).unwrap(), fixture.original);
    let before = second.state.project.snapshot();
    fixture
        .service
        .submit(Request::RestartRecovery(intent(&second)))
        .unwrap();
    let restarted = next(&fixture.service);
    assert!(restarted.error.is_none(), "{:?}", restarted.error);
    assert!(restarted.recovery_failure.is_none());
    assert_eq!(restarted.state.project.snapshot(), before);
    assert_eq!(restarted.state.revision, second.state.revision);
    assert_eq!(restarted.state.can_undo, second.state.can_undo);
    assert_eq!(restarted.state.can_redo, second.state.can_redo);
    assert!(restarted.state.unsaved);
    assert_eq!(
        hex_storage::load_journal(&fixture.path).unwrap().snapshot(),
        before
    );
    let preserved = std::fs::read_dir(&fixture.directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap()
        .join(fixture.path.file_name().unwrap());
    assert_eq!(std::fs::read(&preserved).unwrap(), fixture.original);
    let subsequent = fixture.edit("after restart");
    assert!(subsequent.error.is_none() && subsequent.recovery_failure.is_none());
    assert_eq!(subsequent.state.project.tracks().len(), 3);
    wait_for_journal(&fixture.path, &subsequent.state.project);
    for (request, count) in [(Request::Undo, 2), (Request::Redo, 3)] {
        fixture.service.submit(request).unwrap();
        let event = next(&fixture.service);
        assert!(event.error.is_none() && event.recovery_failure.is_none());
        assert_eq!(event.state.project.tracks().len(), count);
        wait_for_journal(&fixture.path, &event.state.project);
    }
    let saved = fixture.directory.join("working.hex");
    fixture
        .service
        .submit(Request::SaveNew(saved.clone()))
        .unwrap();
    let event = next(&fixture.service);
    assert!(event.error.is_none() && !event.state.unsaved);
    assert_eq!(
        hex_storage::load(&saved).unwrap().snapshot(),
        event.state.project.snapshot()
    );
    assert!(!fixture.path.exists());
    assert_eq!(std::fs::read(&preserved).unwrap(), fixture.original);
}

#[test]
fn stale_revision_generation_and_failure_cannot_replace_the_captured_slot() {
    let fixture = Fixture::new();
    let _first = fixture.edit("first");
    let first_failure = wait_for_recovery_failure(&fixture.service);
    let stale = intent(&first_failure);
    let current = fixture.edit("second");
    let before = current.state.project.snapshot();
    for request in [
        stale,
        RecoveryRestart {
            project_generation: stale.project_generation + 1,
            ..intent(&current)
        },
        RecoveryRestart {
            failure_id: stale.failure_id + 1,
            ..intent(&current)
        },
    ] {
        fixture
            .service
            .submit(Request::RestartRecovery(request))
            .unwrap();
        let event = next(&fixture.service);
        assert!(event.error.as_ref().unwrap().contains("changed"));
        assert_eq!(event.state.project.snapshot(), before);
        assert_eq!(std::fs::read(&fixture.path).unwrap(), fixture.original);
    }
    let third = fixture.edit("third");
    assert_eq!(third.state.project.tracks().len(), 3);
    let foreign = b"externally replaced journal";
    std::fs::write(&fixture.path, foreign).unwrap();
    fixture
        .service
        .submit(Request::RestartRecovery(intent(&third)))
        .unwrap();
    let refused = next(&fixture.service);
    assert!(refused.error.is_some());
    assert_eq!(
        refused.state.project.snapshot(),
        third.state.project.snapshot()
    );
    assert_eq!(std::fs::read(&fixture.path).unwrap(), foreign);
    assert!(
        refused
            .recovery_failure
            .as_ref()
            .unwrap()
            .explanation
            .contains("changed outside hex")
    );
    assert!(
        std::fs::read_dir(&fixture.directory)
            .unwrap()
            .all(|entry| !entry.unwrap().path().is_dir())
    );
}
