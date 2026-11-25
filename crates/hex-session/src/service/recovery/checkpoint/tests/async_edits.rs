use super::*;

#[test]
fn consecutive_track_mutes_commit_while_first_checkpoint_is_held() {
    check_consecutive_track_mutes(false);
}

#[test]
fn consecutive_track_mutes_preserve_conflicting_recovery_bytes() {
    check_consecutive_track_mutes(true);
}

fn check_consecutive_track_mutes(conflict: bool) {
    use std::time::{Duration, Instant};
    let directory =
        std::env::temp_dir().join(format!("hex-mute-queue-{}-{conflict}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    type SwitchStates = Vec<(hex_engine::AudioSourceContext, Vec<bool>)>;
    struct SwitchSink(Arc<std::sync::Mutex<SwitchStates>>);
    impl WorkerAudioSink for SwitchSink {
        fn initialize(&mut self, _: &std::sync::atomic::AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
            self.0.lock().unwrap().push((
                source.context(),
                source
                    .project_metadata()
                    .tracks()
                    .iter()
                    .map(|track| track.muted())
                    .collect(),
            ));
        }
        fn poll(&mut self) {}
        fn close(&mut self) {}
    }
    let sources = Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = Service::with_audio_sink(
        16,
        8_000_000,
        Some(path.clone()),
        Box::new(SwitchSink(Arc::clone(&sources))),
    )
    .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "track".into(),
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
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let first_terminal = next(&service);
    let first = service.try_track_switch_view().unwrap();
    assert!(
        first
            .tracks
            .iter()
            .find(|item| item.track == track)
            .unwrap()
            .muted
    );
    let foreign = b"other writer's journal";
    if conflict {
        std::fs::write(&path, foreign).unwrap();
    }
    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    let second_terminal = next(&service);
    let second = service
        .try_track_switch_view()
        .expect("second mute must commit while the first checkpoint is held");
    let until = Instant::now() + Duration::from_millis(180);
    let source_applied = loop {
        if sources.lock().unwrap().iter().any(|(context, states)| {
            context.editor_revision == second.revision && states == &[false]
        }) {
            break true;
        }
        if Instant::now() >= until {
            break false;
        }
        std::thread::yield_now();
    };
    release.send(()).unwrap();
    assert_eq!(second.previous_revision, first.revision);
    assert!(
        !second
            .tracks
            .iter()
            .find(|item| item.track == track)
            .unwrap()
            .muted
    );
    assert!(
        source_applied,
        "second mute's canonical audio source must be acknowledged before release"
    );
    assert_eq!(first_terminal.state.revision, first.revision);
    assert_eq!(second_terminal.state.revision, second.revision);
    if conflict {
        let failure = wait_for_recovery_failure(&service);
        assert!(!failure.request_complete && failure.recovery_failure.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), foreign);
    } else {
        assert!(first_terminal.error.is_none() && second_terminal.error.is_none());
        wait_for_journal(&path, &second_terminal.state.project);
    }
    service.submit(Request::Undo).unwrap();
    assert!(
        next_complete(&service)
            .state
            .project
            .track(track)
            .unwrap()
            .muted()
    );
    service.submit(Request::Undo).unwrap();
    assert!(
        !next_complete(&service)
            .state
            .project
            .track(track)
            .unwrap()
            .muted()
    );
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn committed_second_mute_survives_event_consumer_closure() {
    check_mute_custody_on_exit(true);
}

#[test]
fn committed_second_mute_survives_shutdown() {
    check_mute_custody_on_exit(false);
}

fn check_mute_custody_on_exit(close_consumer: bool) {
    use std::time::{Duration, Instant};
    let directory = std::env::temp_dir().join(format!(
        "hex-mute-exit-{}-{close_consumer}",
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
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "track".into(),
        }))
        .unwrap();
    let initial = Arc::clone(&next(&service).state.project);
    wait_for_journal(&path, &initial);
    let track = initial.tracks()[0].id();
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let _first = service.try_track_switch_view().unwrap();
    service
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    let until = Instant::now() + Duration::from_millis(180);
    let second = loop {
        if let Ok(view) = service.try_track_switch_view() {
            break view;
        }
        assert!(Instant::now() < until, "second mute did not commit");
        std::thread::yield_now();
    };
    let until = Instant::now() + Duration::from_millis(180);
    while sources
        .lock()
        .unwrap()
        .last()
        .is_none_or(|context| context.editor_revision != second.revision)
    {
        assert!(Instant::now() < until, "second source was not acknowledged");
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
    assert_eq!(hex_storage::load_journal(&path).unwrap(), *initial);
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn paste_does_not_wait_for_pending_track_switch_checkpoint() {
    use std::time::Duration;
    let directory =
        std::env::temp_dir().join(format!("hex-mute-paste-fence-{}", std::process::id()));
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
        .submit(Request::Edit(Command::ToggleMute(track)))
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let first = next(&service);
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 48_000,
        })
        .unwrap();
    let second = next(&service);
    assert!(first.error.is_none() && second.error.is_none());
    assert!(first.state.project.track(track).unwrap().muted());
    assert_eq!(
        second
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        2
    );
    release.send(()).unwrap();
    wait_for_journal(&path, &second.state.project);
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn track_switch_view_and_ack_precede_checkpoint_without_blocking() {
    let directory =
        std::env::temp_dir().join(format!("hex-track-switch-recovery-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let diagnostics = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&diagnostics);
    service
        .set_diagnostic_observer(Arc::new(
            crate::service::diagnostics::DiagnosticObserver::new(move |event| {
                if matches!(
                    event.stage,
                    crate::service::diagnostics::DiagnosticStage::OutcomeReady {
                        request_complete: true,
                        ..
                    }
                ) {
                    recorded.lock().unwrap().push((
                        event.request.and_then(|request| request.operation),
                        event.details.clone(),
                    ));
                }
            }),
        ))
        .unwrap();
    next(&service);
    let mut last = None;
    for name in ["first", "second"] {
        service
            .submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .unwrap();
        last = Some(next(&service));
    }
    let last = last.unwrap();
    wait_for_journal(&path, &last.state.project);
    let original = hex_storage::load_journal(&path).unwrap();
    let first = original.tracks()[0].id();
    let second = original.tracks()[1].id();
    let both = hex_project::TrackSelectionIds::new(&[first, second]).unwrap();
    let mut preceding = original;
    for (command, expected) in [
        (
            Command::ToggleSolo {
                track: first,
                exclusive: true,
            },
            [(false, true), (false, false)],
        ),
        (Command::ToggleMute(first), [(true, true), (false, false)]),
        (
            Command::ToggleTracksMute(both),
            [(true, true), (true, false)],
        ),
    ] {
        let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
        let (release, release_rx) = std::sync::mpsc::sync_channel(1);
        CHECKPOINT_GATE
            .lock()
            .unwrap()
            .push((path.clone(), entered_tx, release_rx));
        service.submit(Request::Edit(command)).unwrap();
        entered
            .recv_timeout(std::time::Duration::from_millis(500))
            .unwrap();
        let complete = next(&service);
        let visible = service
            .try_track_switch_view()
            .expect("committed track switches");
        assert_eq!(visible.revision, visible.previous_revision + 1);
        assert_eq!(
            visible
                .tracks
                .iter()
                .map(|track| (track.muted, track.soloed))
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(hex_storage::load_journal(&path).unwrap(), preceding);
        assert!(complete.request_complete && complete.error.is_none());
        assert_eq!(visible.revision, complete.state.revision);
        release.send(()).unwrap();
        wait_for_journal(&path, &complete.state.project);
        preceding = hex_storage::load_journal(&path).unwrap();
    }
    service.submit(Request::Undo).unwrap();
    let undone = next(&service);
    assert!(undone.error.is_none());
    assert!(undone.state.project.track(first).unwrap().muted());
    assert!(!undone.state.project.track(second).unwrap().muted());
    service.submit(Request::Redo).unwrap();
    let redone = next(&service);
    assert!(redone.error.is_none());
    assert_eq!(*redone.state.project, preceding);
    let rows = diagnostics.lock().unwrap();
    let timed: Vec<_> = rows
        .iter()
        .filter_map(|(operation, details)| {
            let value = details
                .split_whitespace()
                .find_map(|field| field.strip_prefix("track_switch_recovery_us="))?;
            assert!(value.parse::<u128>().is_ok());
            *operation
        })
        .collect();
    assert_eq!(timed, ["toggle_solo", "toggle_mute", "toggle_tracks_mute"]);
    drop(rows);
    service.shutdown(std::time::Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn consecutive_section_copies_stage_while_first_checkpoint_is_held() {
    for variant in 0..4 {
        check_consecutive_section_copies(false, variant);
    }
}

#[test]
fn consecutive_section_copies_preserve_conflicting_recovery_bytes() {
    check_consecutive_section_copies(true, 0);
}

fn check_consecutive_section_copies(conflict: bool, variant: u8) {
    use std::time::Duration;
    let directory = std::env::temp_dir().join(format!(
        "hex-section-copy-stage-{}-{conflict}-{variant}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let initial = next(&service);
    service
        .submit(Request::EditArrangementSection {
            project_generation: 0,
            revision: initial.state.revision,
            command: Command::AddArrangementSection {
                name: "intro".into(),
                start: hex_project::BeatTime::ZERO,
                end: hex_project::BeatTime::ONE,
                color: hex_project::ElementColor::Cyan,
            },
        })
        .unwrap();
    let first_section = next(&service);
    let Some(Created::ArrangementSection(section)) = first_section.created else {
        panic!("section")
    };
    service
        .submit(Request::EditArrangementSection {
            project_generation: 0,
            revision: first_section.state.revision,
            command: Command::AddArrangementSection {
                name: "outro".into(),
                start: hex_project::BeatTime::ONE,
                end: hex_project::BeatTime::from_quarters(2).unwrap(),
                color: hex_project::ElementColor::Cyan,
            },
        })
        .unwrap();
    let before = next(&service);
    wait_for_journal(&path, &before.state.project);
    let Some(Created::ArrangementSection(target)) = before.created else {
        panic!("target")
    };
    let initial_revision = before.state.revision;
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    let duplicate = |revision| Request::EditArrangementSection {
        project_generation: 0,
        revision,
        command: Command::DuplicateArrangementSection(section),
    };
    let first_command = match variant {
        0 => Command::DuplicateArrangementSection(section),
        1 => Command::DuplicateArrangementSectionAt {
            section,
            target,
            after: true,
        },
        2 => Command::AppendArrangementSectionCopy {
            section,
            start: hex_project::BeatTime::from_quarters(2).unwrap(),
            gap_name: "gap".into(),
            gap_color: hex_project::ElementColor::Cyan,
        },
        3 => Command::ReplaceArrangementSection {
            source: section,
            target,
            copy: true,
        },
        _ => panic!("unknown section-copy variant"),
    };
    service
        .submit(Request::EditArrangementSection {
            project_generation: 0,
            revision: initial_revision,
            command: first_command,
        })
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let first_terminal = next(&service);
    let first = service.try_arrangement_edit_view().unwrap();
    let first_state = first.state.upgrade().unwrap();
    assert_eq!(first_state.revision, initial_revision + 1);
    if conflict {
        std::fs::write(&path, b"another writer's journal").unwrap();
    }
    service.submit(duplicate(initial_revision + 1)).unwrap();
    let second_terminal = next(&service);
    let second_view = service
        .try_arrangement_edit_view()
        .expect("second section copy must commit before checkpoint release");
    let second_state = second_view.state.upgrade().unwrap();
    release.send(()).unwrap();
    let final_count = if variant == 3 { 3 } else { 4 };
    assert_eq!(
        second_state.project.arrangement_sections().len(),
        final_count
    );
    assert_eq!(first_terminal.state.revision, initial_revision + 1);
    assert_eq!(second_terminal.state.revision, initial_revision + 2);
    if conflict {
        let failure = wait_for_recovery_failure(&service);
        assert!(!failure.request_complete && failure.recovery_failure.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), b"another writer's journal");
    } else {
        assert!(first_terminal.error.is_none() && second_terminal.error.is_none());
        wait_for_journal(&path, &second_terminal.state.project);
    }
    service.submit(duplicate(initial_revision)).unwrap();
    let stale = next_complete(&service);
    assert!(stale.error.is_some());
    assert_eq!(stale.state.revision, initial_revision + 2);
    service.submit(Request::Undo).unwrap();
    assert_eq!(
        next_complete(&service)
            .state
            .project
            .arrangement_sections()
            .len(),
        final_count - 1
    );
    service.submit(Request::Redo).unwrap();
    assert_eq!(
        next_complete(&service)
            .state
            .project
            .arrangement_sections()
            .len(),
        final_count
    );
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn section_copy_and_undo_ack_immediately_and_coalesce_journal() {
    let directory = std::env::temp_dir().join(format!(
        "hex-section-copy-checkpoint-{}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let initial = next(&service);
    service
        .submit(Request::EditArrangementSection {
            project_generation: 0,
            revision: initial.state.revision,
            command: Command::AddArrangementSection {
                name: "intro".into(),
                start: hex_project::BeatTime::ZERO,
                end: hex_project::BeatTime::ONE,
                color: hex_project::ElementColor::Cyan,
            },
        })
        .unwrap();
    let before = next(&service);
    wait_for_journal(&path, &before.state.project);
    let Some(Created::ArrangementSection(section)) = before.created else {
        panic!("section");
    };
    let duplicate = |revision| Request::EditArrangementSection {
        project_generation: 0,
        revision,
        command: Command::DuplicateArrangementSection(section),
    };
    service.submit(duplicate(initial.state.revision)).unwrap();
    assert!(next(&service).error.is_some());
    assert!(service.try_arrangement_edit_view().is_err());

    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service.submit(duplicate(before.state.revision)).unwrap();
    entered
        .recv_timeout(std::time::Duration::from_millis(500))
        .unwrap();
    let committed = next(&service);
    let display = service
        .try_arrangement_edit_view()
        .expect("committed section before checkpoint");
    assert_eq!(display.project_generation, 0);
    assert_eq!(display.previous_revision, before.state.revision);
    let visible = display.state.upgrade().unwrap();
    assert_eq!(visible.revision, before.state.revision + 1);
    assert_eq!(visible.project.arrangement_sections().len(), 2);
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *before.state.project
    );
    assert!(committed.request_complete && committed.error.is_none());
    assert!(Arc::ptr_eq(&visible, &committed.state));
    // Undo does not block on the held checkpoint and coalesces the latest state.
    service.submit(Request::Undo).unwrap();
    let undone_event = next(&service);
    release.send(()).unwrap();
    wait_for_journal(&path, &undone_event.state.project);
    let mut undone = undone_event.state.project.snapshot();
    let before = before.state.project.snapshot();
    undone.next_id = before.next_id;
    assert_eq!(undone, before);
    service.submit(Request::Redo).unwrap();
    assert_eq!(next(&service).state.project, committed.state.project);
    assert!(service.try_arrangement_edit_view().is_err());
    service.shutdown(std::time::Duration::from_secs(2)).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn consecutive_clip_pastes_can_commit_while_the_first_checkpoint_is_held() {
    consecutive_pastes_with_checkpoint_conflict(false);
}

#[test]
fn unread_paste_views_coalesce_while_first_checkpoint_is_held() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let directory = std::env::temp_dir().join(format!(
        "hex-unread-paste-checkpoint-{}",
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
    let (entered_tx, entered) = mpsc::sync_channel(1);
    let (release, release_rx) = mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    let initial_source_count = sources.lock().unwrap().len();
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 48_000,
        })
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let until = Instant::now() + Duration::from_millis(500);
    while sources.lock().unwrap().len() == initial_source_count {
        assert!(
            Instant::now() < until,
            "first paste did not update the audio source"
        );
        std::thread::yield_now();
    }
    let first_source_count = sources.lock().unwrap().len();
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 96_000,
        })
        .unwrap();
    let until = Instant::now() + Duration::from_millis(500);
    while sources.lock().unwrap().len() == first_source_count {
        assert!(
            Instant::now() < until,
            "second paste did not update the audio source"
        );
        std::thread::yield_now();
    }
    let observation = service.try_arrangement_edit_view().unwrap();
    let state = observation.state.upgrade().unwrap();
    assert_eq!(
        observation.first_previous_revision + 1,
        observation.previous_revision
    );
    assert_eq!(state.revision, observation.previous_revision + 1);
    assert_eq!(state.project.track(track).unwrap().midi_clips().len(), 3);
    assert!(service.try_arrangement_edit_view().is_err());
    release.send(()).unwrap();
    let first = next(&service);
    let second = next(&service);
    assert!(first.error.is_none() && second.error.is_none());
    assert_eq!(first.state.revision, observation.previous_revision);
    assert_eq!(second.state.revision, state.revision);
    wait_for_journal(&path, &second.state.project);
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn consecutive_clip_pastes_preserve_a_conflicting_recovery_file() {
    consecutive_pastes_with_checkpoint_conflict(true);
}

fn consecutive_pastes_with_checkpoint_conflict(conflict: bool) {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "hex-paste-queue-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
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
    let (entered_tx, entered) = mpsc::sync_channel(1);
    let (release, release_rx) = mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 48_000,
        })
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let first_terminal = next(&service);
    let first = service.try_arrangement_edit_view().unwrap();
    let first_state = first.state.upgrade().unwrap();
    assert_eq!(
        first_state.project.track(track).unwrap().midi_clips().len(),
        2
    );
    let foreign = b"another writer's recovery file";
    if conflict {
        std::fs::write(&path, foreign).unwrap();
    }
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 96_000,
        })
        .unwrap();
    let second_terminal = next(&service);
    let second = service
        .try_arrangement_edit_view()
        .expect("second paste must commit while the first checkpoint is held");
    assert_eq!(second.previous_revision, first_state.revision);
    let second_state = second.state.upgrade().unwrap();
    assert_eq!(
        second_state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        3
    );
    let second_context = second_state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    let until = Instant::now() + Duration::from_millis(180);
    let saw_second_source = loop {
        if sources.lock().unwrap().last().copied() == Some(second_context) {
            break true;
        }
        if Instant::now() >= until {
            break false;
        }
        std::thread::yield_now();
    };
    release.send(()).unwrap();
    assert!(
        saw_second_source,
        "second paste must update the audio source while the first checkpoint is held"
    );
    if conflict {
        let failure = wait_for_recovery_failure(&service);
        assert!(!failure.request_complete && failure.error.is_some());
        assert!(failure.recovery_failure.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), foreign);
    } else {
        assert!(first_terminal.error.is_none() && second_terminal.error.is_none());
        wait_for_journal(&path, &second_terminal.state.project);
    }
    assert_eq!(second_terminal.state.revision, second_state.revision);
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
    let first_redo = next_complete(&service);
    assert_eq!(
        first_redo
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        2
    );
    service.submit(Request::Redo).unwrap();
    let second_redo = next_complete(&service);
    assert_eq!(
        second_redo
            .state
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .len(),
        3
    );
    if conflict {
        assert!(second_redo.recovery_failure.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), foreign);
    } else {
        wait_for_journal(&path, &second_redo.state.project);
    }
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn new_project_waits_for_an_in_flight_paste_checkpoint() {
    let directory = std::env::temp_dir().join(format!("hex-paste-new-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let track = prepare_paste(&service, &path);
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 48_000,
        })
        .unwrap();
    entered
        .recv_timeout(std::time::Duration::from_millis(500))
        .unwrap();
    let paste = next(&service);
    assert!(paste.error.is_none());
    let view = service.try_arrangement_edit_view().unwrap();
    let pasted = view.state.upgrade().unwrap();
    assert_eq!(paste.state.revision, pasted.revision);
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    assert!(matches!(
        service.try_event(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    release.send(()).unwrap();
    let replacement = next(&service);
    assert!(replacement.error.is_none());
    assert!(replacement.project_replaced);
    assert_ne!(
        replacement
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        view.project_generation
    );
    assert_eq!(hex_storage::load_journal(&path).unwrap(), *pasted.project);
    service.shutdown(std::time::Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn project_replacement_follows_both_committed_pastes_without_stale_source() {
    use std::time::{Duration, Instant};
    let directory = std::env::temp_dir().join(format!("hex-paste-replace-{}", std::process::id()));
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
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 48_000,
        })
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let first = service.try_arrangement_edit_view().unwrap();
    let first_revision = first.state.upgrade().unwrap().revision;
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 96_000,
        })
        .unwrap();
    let until = Instant::now() + Duration::from_millis(180);
    let second = loop {
        if let Ok(view) = service.try_arrangement_edit_view() {
            break view;
        }
        assert!(Instant::now() < until, "second paste did not commit");
        std::thread::yield_now();
    };
    let second_state = second.state.upgrade().unwrap();
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::Resident,
            reservation: None,
        })
        .unwrap();
    release.send(()).unwrap();
    let first = next(&service);
    let second = next(&service);
    let replacement = next(&service);
    assert!(first.error.is_none() && second.error.is_none() && replacement.error.is_none());
    assert_eq!(
        (first.state.revision, second.state.revision),
        (first_revision, second_state.revision)
    );
    assert!(replacement.project_replaced);
    let replacement_context = replacement
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    assert_ne!(
        replacement_context.project_generation,
        second_state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation
    );
    assert_eq!(
        sources.lock().unwrap().last().copied(),
        Some(replacement_context)
    );
    assert_eq!(
        hex_storage::load_journal(&path).unwrap(),
        *second_state.project
    );
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn third_paste_coalesces_without_waiting_for_in_flight_checkpoint() {
    use std::time::Duration;
    let directory = std::env::temp_dir().join(format!("hex-paste-full-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let track = prepare_paste(&service, &path);
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 48_000,
        })
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let first = next(&service);
    let first_view = service.try_arrangement_edit_view().unwrap();
    let first_revision = first_view.state.upgrade().unwrap().revision;
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 96_000,
        })
        .unwrap();
    let second = next(&service);
    let second_view = service.try_arrangement_edit_view().unwrap();
    let second_revision = second_view.state.upgrade().unwrap().revision;
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 144_000,
        })
        .unwrap();
    let third = next(&service);
    assert!(first.error.is_none() && second.error.is_none() && third.error.is_none());
    assert_eq!(first.state.revision, first_revision);
    assert_eq!(second.state.revision, second_revision);
    assert_eq!(third.state.revision, second_revision + 1);
    assert_eq!(
        third.state.project.track(track).unwrap().midi_clips().len(),
        4
    );
    release.send(()).unwrap();
    wait_for_journal(&path, &third.state.project);
    for remaining in [3, 2, 1] {
        service.submit(Request::Undo).unwrap();
        assert_eq!(
            next(&service)
                .state
                .project
                .track(track)
                .unwrap()
                .midi_clips()
                .len(),
            remaining
        );
    }
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn retained_paste_terminals_survive_a_full_event_queue() {
    use crate::service::diagnostics::{DiagnosticObserver, DiagnosticStage};
    use std::time::{Duration, Instant};
    let directory = std::env::temp_dir().join(format!("hex-paste-events-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let track = prepare_paste(&service, &path);
    let queued = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&queued);
    service
        .set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
            if matches!(event.stage, DiagnosticStage::OutcomeQueued)
                && event
                    .request
                    .is_some_and(|request| request.kind == "copy_clips")
            {
                observed.fetch_add(1, Ordering::Relaxed);
            }
        })))
        .unwrap();
    // Copying an empty selection fails without changing the clipboard or project.
    service.submit(Request::CopyClips(Vec::new())).unwrap();
    service.submit(Request::CopyClips(Vec::new())).unwrap();
    let until = Instant::now() + Duration::from_secs(1);
    while queued.load(Ordering::Relaxed) < 2 {
        assert!(Instant::now() < until, "event queue did not fill");
        std::thread::yield_now();
    }
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 48_000,
        })
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let first = service.try_arrangement_edit_view().unwrap();
    let first_revision = first.state.upgrade().unwrap().revision;
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 96_000,
        })
        .unwrap();
    // Drain the two prefilled CopyClips events so immediate paste ACKs flow without losing order.
    let _first_copy = next(&service);
    let _second_copy = next(&service);
    let first = next(&service);
    let second = next(&service);
    release.send(()).unwrap();
    assert!(first.error.is_none() && second.error.is_none());
    assert_eq!(
        (first.state.revision, second.state.revision),
        (first_revision, first_revision + 1)
    );
    wait_for_journal(&path, &second.state.project);
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn committed_second_paste_is_checkpointed_when_event_consumer_closes() {
    check_second_paste_custody_on_exit(true);
}

#[test]
fn committed_second_paste_is_checkpointed_when_shutdown_begins() {
    check_second_paste_custody_on_exit(false);
}

fn check_second_paste_custody_on_exit(close_consumer: bool) {
    use std::time::{Duration, Instant};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "hex-paste-exit-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
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
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 48_000,
        })
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let _first = service.try_arrangement_edit_view().unwrap();
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 96_000,
        })
        .unwrap();
    let until = Instant::now() + Duration::from_millis(180);
    let second = loop {
        if let Ok(view) = service.try_arrangement_edit_view() {
            break view;
        }
        assert!(Instant::now() < until, "second paste did not commit");
        std::thread::yield_now();
    };
    let second = second.state.upgrade().unwrap();
    let context = second.canonical_audio_source.as_ref().unwrap().context();
    let until = Instant::now() + Duration::from_millis(180);
    while sources.lock().unwrap().last().copied() != Some(context) {
        assert!(Instant::now() < until, "second source was not acknowledged");
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
        assert!(
            Instant::now() < until,
            "service did not stop after consumer closure"
        );
        std::thread::yield_now();
    }
    service.shutdown(Duration::from_secs(2)).unwrap();
    assert_eq!(hex_storage::load_journal(&path).unwrap(), *second.project);
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[derive(Clone, Copy)]
enum PasteFence {
    Save,
    Open,
    Restart,
}

#[test]
fn save_waits_for_an_in_flight_paste_checkpoint() {
    check_paste_fence(PasteFence::Save);
}

#[test]
fn open_waits_for_an_in_flight_paste_checkpoint() {
    check_paste_fence(PasteFence::Open);
}

#[test]
fn stale_recovery_restart_waits_for_an_in_flight_paste_checkpoint() {
    check_paste_fence(PasteFence::Restart);
}

fn check_paste_fence(fence: PasteFence) {
    use std::time::{Duration, Instant};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "hex-paste-fence-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let journal = directory.join("untitled.hex-journal");
    let target = directory.join("other.hex");
    let other = Project::new(48_000).unwrap();
    if matches!(fence, PasteFence::Open) {
        hex_storage::save_new(&target, &other).unwrap();
    }
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, journal.clone()).unwrap();
    let track = prepare_paste(&service, &journal);
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((journal.clone(), entered_tx, release_rx));
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 48_000,
        })
        .unwrap();
    entered.recv_timeout(Duration::from_millis(500)).unwrap();
    let paste = next(&service);
    assert!(paste.error.is_none());
    let view = service.try_arrangement_edit_view().unwrap();
    let pasted = view.state.upgrade().unwrap();
    assert_eq!(paste.state.revision, pasted.revision);
    let request = match fence {
        PasteFence::Save => Request::SaveNew(target.clone()),
        PasteFence::Open => Request::Open(target.clone()),
        PasteFence::Restart => Request::RestartRecovery(RecoveryRestart {
            project_generation: view.project_generation,
            revision: pasted.revision,
            failure_id: 1,
        }),
    };
    service.submit(request).unwrap();
    let until = Instant::now() + Duration::from_millis(60);
    while Instant::now() < until {
        assert!(matches!(
            service.try_event(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        if matches!(fence, PasteFence::Save) {
            assert!(!target.exists());
        }
        std::thread::yield_now();
    }
    release.send(()).unwrap();
    let terminal = next(&service);
    match fence {
        PasteFence::Save => {
            assert!(terminal.error.is_none(), "{:?}", terminal.error);
            assert!(terminal.save_complete);
            assert_eq!(hex_storage::load(&target).unwrap(), *pasted.project);
        }
        PasteFence::Open => {
            assert!(terminal.error.is_none(), "{:?}", terminal.error);
            assert!(terminal.project_replaced);
            assert_eq!(*terminal.state.project, other);
            assert_eq!(
                hex_storage::load_journal(&journal).unwrap(),
                *pasted.project
            );
        }
        PasteFence::Restart => {
            assert!(terminal.error.is_some());
            assert_eq!(
                hex_storage::load_journal(&journal).unwrap(),
                *pasted.project
            );
        }
    }
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn shutdown_retains_an_in_flight_paste_checkpoint_until_publication() {
    let directory = std::env::temp_dir().join(format!("hex-paste-shutdown-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let track = prepare_paste(&service, &path);
    let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
    let (release, release_rx) = std::sync::mpsc::sync_channel(1);
    CHECKPOINT_GATE
        .lock()
        .unwrap()
        .push((path.clone(), entered_tx, release_rx));
    service
        .submit(Request::PasteTimeRange {
            track,
            frame: 48_000,
        })
        .unwrap();
    entered
        .recv_timeout(std::time::Duration::from_millis(500))
        .unwrap();
    let view = service.try_arrangement_edit_view().unwrap();
    let pasted = view.state.upgrade().unwrap();
    assert!(
        service
            .shutdown(std::time::Duration::from_millis(20))
            .is_err()
    );
    release.send(()).unwrap();
    service.shutdown(std::time::Duration::from_secs(2)).unwrap();
    assert_eq!(hex_storage::load_journal(&path).unwrap(), *pasted.project);
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn paste_view_and_ack_precede_checkpoint_for_single_clip_and_multi_clip_insert() {
    use hex_project::BeatTime;
    use std::sync::mpsc::{self, TryRecvError};
    use std::time::Duration;

    let directory =
        std::env::temp_dir().join(format!("hex-paste-checkpoint-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let diagnostics = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&diagnostics);
    service
        .set_diagnostic_observer(Arc::new(diagnostics::DiagnosticObserver::new(
            move |event| {
                if let Some(timing) = event
                    .details
                    .split_whitespace()
                    .find_map(|field| field.strip_prefix("paste_recovery_us="))
                {
                    sink.lock()
                        .unwrap()
                        .push((event.stage, timing.parse::<u128>().unwrap()));
                }
            },
        )))
        .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "source".into(),
        }))
        .unwrap();
    let track = next(&service).state.project.tracks()[0].id();
    service
        .submit(Request::PasteTimeRange { track, frame: 0 })
        .unwrap();
    let refused = next(&service);
    assert!(refused.request_complete && refused.error.is_some());
    assert!(matches!(
        service.try_arrangement_edit_view(),
        Err(TryRecvError::Empty)
    ));

    let mut originals = Vec::new();
    let mut before = refused.state;
    for start in [BeatTime::ZERO, BeatTime::ONE] {
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start,
                length: BeatTime::ONE,
            }))
            .unwrap();
        let placed = next(&service);
        let Some(Created::Clip(id)) = placed.created else {
            panic!("placed MIDI clip");
        };
        originals.push(id);
        before = placed.state;
    }
    wait_for_journal(&path, &before.project);
    let beat_frames = before
        .project
        .tempo_map()
        .beat_to_frame(BeatTime::ONE)
        .unwrap();
    for (sources, request, count) in [
        (
            originals[..1].to_vec(),
            Request::PasteTimeRange {
                track,
                frame: 2 * beat_frames,
            },
            3,
        ),
        (
            originals.clone(),
            Request::InsertTimeRange { track, frame: 0 },
            5,
        ),
    ] {
        service.submit(Request::CopyClips(sources)).unwrap();
        assert!(next(&service).error.is_none());
        let (entered_tx, entered) = mpsc::sync_channel(1);
        let (release, release_rx) = mpsc::sync_channel(1);
        CHECKPOINT_GATE
            .lock()
            .unwrap()
            .push((path.clone(), entered_tx, release_rx));
        service.submit(request).unwrap();
        entered.recv_timeout(Duration::from_millis(500)).unwrap();

        let terminal = next(&service);
        let display = service.try_arrangement_edit_view().unwrap();
        assert!(matches!(display.kind, ArrangementEditViewKind::Paste));
        assert_eq!(display.project_generation, 0);
        assert_eq!(display.previous_revision, before.revision);
        let committed = display.state.upgrade().unwrap();
        assert_eq!(committed.revision, before.revision + 1);
        let clips = committed.project.track(track).unwrap().midi_clips();
        assert_eq!(clips.len(), count);
        assert_eq!(
            clips
                .iter()
                .find(|clip| clip.id() == originals[0])
                .unwrap()
                .start(),
            if count == 5 {
                BeatTime::from_quarters(2).unwrap()
            } else {
                BeatTime::ZERO
            }
        );
        assert_eq!(hex_storage::load_journal(&path).unwrap(), *before.project);
        assert!(terminal.request_complete && terminal.error.is_none());
        assert!(Arc::ptr_eq(&committed, &terminal.state));
        release.send(()).unwrap();
        wait_for_journal(&path, &committed.project);

        service.submit(Request::Undo).unwrap();
        let undone = next(&service);
        assert!(undone.error.is_none());
        assert_eq!(
            undone.state.project.track(track).unwrap().midi_clips(),
            before.project.track(track).unwrap().midi_clips()
        );
        service.submit(Request::Redo).unwrap();
        let redone = next(&service);
        assert!(redone.error.is_none());
        assert_eq!(
            redone.state.project.track(track).unwrap().midi_clips(),
            clips
        );
        wait_for_journal(&path, &redone.state.project);
        before = redone.state;
    }
    let rows = diagnostics.lock().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(stage, _)| matches!(
        stage,
        diagnostics::DiagnosticStage::OutcomeReady {
            request_complete: true,
            error: false,
            ..
        }
    )));
    drop(rows);
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn midi_note_update_reports_only_its_recovery_maintenance() {
    use hex_project::{BeatTime, MidiNoteUpdate};
    use std::time::Duration;

    let directory = std::env::temp_dir().join(format!("hex-midi-timing-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let rows = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&rows);
    service
        .set_diagnostic_observer(Arc::new(diagnostics::DiagnosticObserver::new(
            move |event| {
                sink.lock()
                    .unwrap()
                    .push((event.request, event.stage, event.details.clone()));
            },
        )))
        .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "source".into(),
        }))
        .unwrap();
    let Some(Created::Track(track)) = next(&service).created else {
        panic!("track")
    };
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let Some(Created::Clip(clip)) = next(&service).created else {
        panic!("clip")
    };
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
    let Some(Created::Note(note)) = next(&service).created else {
        panic!("note")
    };
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 0,
            velocity: 100,
        }))
        .unwrap();
    let second_note = next(&service);
    let Some(Created::Note(other)) = second_note.created else {
        panic!("second note")
    };
    wait_for_journal(&path, &second_note.state.project);
    let update = |start| MidiNoteUpdate {
        note,
        start,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 100,
    };
    let first = update(BeatTime::ONE);
    let second = update(BeatTime::from_quarters(2).unwrap());
    for command in [
        Command::UpdateMidiNote {
            note,
            start: first.start,
            duration: first.duration,
            pitch: first.pitch,
            channel: first.channel,
            velocity: first.velocity,
        },
        Command::UpdateMidiNotes {
            clip,
            notes: vec![
                second,
                MidiNoteUpdate {
                    note: other,
                    pitch: 64,
                    ..second
                },
            ],
        },
    ] {
        let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
        let (release, release_rx) = std::sync::mpsc::sync_channel(1);
        CHECKPOINT_GATE
            .lock()
            .unwrap()
            .push((path.clone(), entered_tx, release_rx));
        service.submit(Request::Edit(command)).unwrap();
        entered.recv_timeout(Duration::from_millis(500)).unwrap();
        let terminal = next(&service);
        assert!(terminal.request_complete && terminal.error.is_none());
        release.send(()).unwrap();
        wait_for_journal(&path, &terminal.state.project);
    }
    service
        .submit(Request::Edit(Command::UpdateMidiNote {
            note,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 255,
            channel: 0,
            velocity: 100,
        }))
        .unwrap();
    assert!(next(&service).error.is_some());
    service.submit(Request::Undo).unwrap();
    next(&service);
    let rows = rows.lock().unwrap();
    let timed: Vec<_> = rows
        .iter()
        .filter_map(|(request, stage, details)| {
            details
                .split_whitespace()
                .find_map(|part| part.strip_prefix("midi_note_recovery_us="))
                .map(|value| (request, stage, value))
        })
        .collect();
    assert_eq!(timed.len(), 2);
    for (request, stage, value) in timed {
        assert!(
            matches!(request, Some(request) if matches!(request.operation, Some("update_midi_note" | "update_midi_notes")))
        );
        assert!(matches!(
            stage,
            diagnostics::DiagnosticStage::OutcomeReady {
                request_complete: true,
                ..
            }
        ));
        assert!(value.parse::<u128>().is_ok());
    }
    drop(rows);
    service.shutdown(Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn duplicate_view_and_ack_precede_checkpoint_while_preserving_journal_custody() {
    let directory =
        std::env::temp_dir().join(format!("hex-duplicate-checkpoint-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("untitled.hex-journal");
    let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
    let diagnostic_rows = Arc::new(std::sync::Mutex::new(Vec::new()));
    let diagnostic_sink = Arc::clone(&diagnostic_rows);
    service
        .set_diagnostic_observer(Arc::new(diagnostics::DiagnosticObserver::new(
            move |event| {
                diagnostic_sink.lock().unwrap().push((
                    event.request,
                    event.stage,
                    event.details.clone(),
                ));
            },
        )))
        .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "source".into(),
        }))
        .unwrap();
    let track = next(&service).state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        }))
        .unwrap();
    let placed = next(&service);
    wait_for_journal(&path, &placed.state.project);
    let mut clip = placed.state.project.tracks()[0].midi_clips()[0].id();
    let mut revision = placed.state.revision;
    for alias in [false, true] {
        let (entered_tx, entered) = std::sync::mpsc::sync_channel(1);
        let (release, release_rx) = std::sync::mpsc::sync_channel(1);
        CHECKPOINT_GATE
            .lock()
            .unwrap()
            .push((path.clone(), entered_tx, release_rx));
        let request = if alias {
            Request::DuplicateAliasClips {
                project_generation: 0,
                revision,
                clips: vec![clip],
            }
        } else {
            Request::DuplicateClips {
                project_generation: 0,
                revision,
                clips: vec![clip],
            }
        };
        service.submit(request).unwrap();
        entered
            .recv_timeout(std::time::Duration::from_millis(500))
            .unwrap();
        let terminal = next(&service);
        let display = service.try_arrangement_edit_view().unwrap();
        assert_eq!(display.project_generation, 0);
        assert_eq!(display.previous_revision, revision);
        let observed = display.state.upgrade().unwrap();
        assert_eq!(observed.revision, revision + 1);
        assert_eq!(
            hex_storage::load_journal(&path).unwrap().tracks()[0]
                .midi_clips()
                .len(),
            if alias { 2 } else { 1 }
        );
        assert!(terminal.request_complete && terminal.error.is_none());
        assert!(Arc::ptr_eq(&observed, &terminal.state));
        release.send(()).unwrap();
        wait_for_journal(&path, &terminal.state.project);
        let Some(Created::Clips { first, last }) = terminal.created else {
            panic!("created duplicate");
        };
        assert_eq!(first, last);
        clip = first;
        revision = terminal.state.revision;
    }
    let before_range = hex_storage::load_journal(&path).unwrap();
    let clip = before_range.midi_clip(clip).unwrap();
    let tempo = before_range.tempo_map();
    let start = tempo.beat_to_frame(clip.start()).unwrap();
    let end_beat = hex_project::BeatTime::from_raw(clip.start().raw() + clip.length().raw());
    let end = tempo.beat_to_frame(end_beat).unwrap();
    let range = hex_project::TimeRange::new(vec![track], start, end).unwrap();
    service
        .submit(Request::Edit(Command::CopyTimeRangeTo {
            range,
            track,
            frame: end,
        }))
        .unwrap();
    let range_terminal = next(&service);
    assert!(range_terminal.request_complete && range_terminal.error.is_none());
    assert_eq!(range_terminal.state.revision, revision + 1);
    wait_for_journal(&path, &range_terminal.state.project);

    service.submit(Request::Undo).unwrap();
    let unrelated = next(&service);
    assert!(unrelated.request_complete && unrelated.error.is_none());
    let rows = diagnostic_rows.lock().unwrap();
    let copy_requests = [
        ("duplicate_clips", None),
        ("duplicate_alias_clips", None),
        ("edit", Some("copy_time_range_to")),
    ];
    for (kind, operation) in copy_requests {
        let submitted: Vec<_> = rows
            .iter()
            .filter_map(|(request, stage, _)| {
                request.filter(|request| {
                    request.kind == kind
                        && request.operation == operation
                        && *stage == diagnostics::DiagnosticStage::SubmitAttempt
                })
            })
            .collect();
        let [submitted] = submitted.as_slice() else {
            panic!("expected one submission for {kind}");
        };
        let timed: Vec<_> = rows
            .iter()
            .filter(|(request, _, _)| request.as_ref() == Some(submitted))
            .flat_map(|(_, stage, details)| {
                details
                    .split_whitespace()
                    .filter_map(|part| part.strip_prefix("duplicate_recovery_us="))
                    .map(move |value| (stage, value))
            })
            .collect();
        let [
            (
                diagnostics::DiagnosticStage::OutcomeReady {
                    request_complete: true,
                    ..
                },
                value,
            ),
        ] = timed.as_slice()
        else {
            panic!("expected recovery timing only on the terminal outcome for {kind}");
        };
        assert!(value.parse::<u128>().is_ok());
    }
    assert!(rows.iter().all(|(request, _, details)| {
        request.is_some_and(|request| copy_requests.contains(&(request.kind, request.operation)))
            || !details
                .split_whitespace()
                .any(|part| part.starts_with("duplicate_recovery_us="))
    }));
    drop(rows);
    service.shutdown(std::time::Duration::from_secs(2)).unwrap();
    drop(service);
    std::fs::remove_dir_all(directory).unwrap();
}
