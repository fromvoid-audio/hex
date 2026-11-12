//! Worker-owned recovery availability and explicitly confirmed slot replacement.
use super::*;

#[cfg(test)]
type CheckpointGate = (
    PathBuf,
    std::sync::mpsc::SyncSender<()>,
    std::sync::mpsc::Receiver<()>,
);
#[cfg(test)]
static CHECKPOINT_GATE: std::sync::Mutex<Vec<CheckpointGate>> = std::sync::Mutex::new(Vec::new());

/// Persistent query, independent of the most recent command's status/error.
/// Working-project edits can commit even while automatic recovery is unavailable.
#[derive(Debug)]
pub struct RecoveryFailure {
    pub id: u64,
    pub project_generation: u64,
    pub path: PathBuf,
    pub explanation: String,
    pub preserved_path: Option<PathBuf>,
    can_restart: bool,
}
impl RecoveryFailure {
    pub fn can_restart(&self) -> bool {
        self.can_restart
    }
}

/// Confirmation is bound to the exact observed failure and working revision.
/// This request changes recovery storage only, never project/history/media.
#[derive(Clone, Copy, Debug)]
pub struct RecoveryRestart {
    pub project_generation: u64,
    pub revision: u64,
    pub failure_id: u64,
}

pub(in crate::service) struct RecoveryContext<'a> {
    pub session: &'a Session,
    pub base: Option<&'a Path>,
    pub document: Option<&'a DocumentHandle>,
    pub untitled: &'a Path,
    pub journal: &'a mut Option<hex_storage::Journal>,
    pub generation: u64,
}
#[derive(Default)]
pub(in crate::service) struct RecoveryTracker {
    failure: Option<Arc<RecoveryFailure>>,
    // Exact old bytes remain exclusively on the service worker. Events carry
    // metadata and a failure ID, never this storage token or its allocation.
    restart: Option<hex_storage::JournalRestartToken>,
    sequence: u64,
}
impl RecoveryTracker {
    fn clear(&mut self) {
        self.failure = None;
        self.restart = None;
    }

    pub fn failure(&self) -> Option<Arc<RecoveryFailure>> {
        self.failure.clone()
    }

    fn failed(
        &mut self,
        path: PathBuf,
        generation: u64,
        reason: String,
        preserved_path: Option<PathBuf>,
    ) {
        self.sequence = self.sequence.saturating_add(1);
        // Exhaustion is terminal for reset permits, never an identity reuse.
        let restart = (self.sequence != u64::MAX)
            .then(|| hex_storage::JournalRestartToken::capture(path.clone()).ok())
            .flatten();
        self.failure = Some(Arc::new(RecoveryFailure {
            id: self.sequence,
            project_generation: generation,
            path,
            explanation: reason,
            preserved_path,
            can_restart: restart.is_some(),
        }));
        self.restart = restart;
    }

    pub fn maintain(
        &mut self,
        context: RecoveryContext<'_>,
        unsaved: bool,
        changed: bool,
    ) -> Result<Option<PathBuf>, String> {
        let path = checkpoint_path(context.document, context.untitled)?;
        #[cfg(test)]
        {
            let gate = {
                let mut gates = CHECKPOINT_GATE.lock().unwrap();
                gates
                    .iter()
                    .position(|(target, _, _)| *target == path)
                    .map(|index| gates.remove(index))
            };
            if let Some((_, entered, release)) = gate {
                let _ = entered.send(());
                let _ = release.recv_timeout(std::time::Duration::from_millis(500));
            }
        }
        if self
            .failure
            .as_ref()
            .is_some_and(|f| f.path != path || f.project_generation != context.generation)
        {
            self.clear();
        }
        let result = if !unsaved {
            let result = context
                .journal
                .as_mut()
                .map_or(Ok(()), hex_storage::Journal::clear)
                .map_err(|e| e.to_string());
            if result.is_ok() {
                *context.journal = None;
            }
            result.map(|()| None)
        } else if changed {
            if self.failure.as_ref().is_some_and(|f| f.can_restart()) {
                // The UI retains the actionable failure. Re-decoding this exact
                // unsupported slot for every successful edit cannot repair it.
                return Ok(None);
            }
            checkpoint_project(
                context.session,
                context.base,
                context.document,
                context.untitled,
                context.journal,
            )
        } else {
            return Ok(None);
        };
        match &result {
            Ok(_) => self.clear(),
            Err(reason) => self.failed(path, context.generation, reason.clone(), None),
        }
        result
    }

    pub fn restart(
        &mut self,
        context: RecoveryContext<'_>,
        revision: u64,
        intent: RecoveryRestart,
    ) -> Result<(), String> {
        let failure = self
            .failure
            .as_ref()
            .ok_or("automatic recovery no longer needs a fresh journal")?;
        let path = checkpoint_path(context.document, context.untitled)?;
        if intent.project_generation != context.generation
            || intent.revision != revision
            || failure.id != intent.failure_id
            || failure.project_generation != context.generation
            || failure.path != path
        {
            return Err(
                "recovery restart refused because the working project or recovery slot changed"
                    .into(),
            );
        }
        let token = self.restart.as_ref().ok_or(
            "this recovery failure cannot start a fresh journal; save the project instead",
        )?;
        let project = project_for_save(context.session, context.base)?;
        let outcome = hex_storage::restart_journal(token, &project);
        if let Some(journal) = outcome.journal {
            *context.journal = Some(journal);
        }
        match outcome.result {
            Ok(()) => {
                self.clear();
                Ok(())
            }
            Err(error) => {
                let reason = if outcome.preserved_path.is_some() {
                    format!("{error}; the previous recovery journal was preserved")
                } else {
                    error.to_string()
                };
                self.failed(path, context.generation, reason, outcome.preserved_path);
                Err("automatic recovery restart needs attention · see recovery details".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct Fixture {
        directory: PathBuf,
        path: PathBuf,
        original: Vec<u8>,
        service: Service,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "hex-recovery-service-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("untitled.hex-journal");
            let payload = br#"{"format":"hex-project","version":0}"#;
            let name = path.file_name().unwrap().to_str().unwrap().as_bytes();
            let mut original = b"HEXJRNL\0".to_vec();
            original.extend_from_slice(&1_u32.to_le_bytes());
            original.extend_from_slice(&(name.len() as u32).to_le_bytes());
            original.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            original.extend_from_slice(name);
            original.extend_from_slice(payload);
            original.extend_from_slice(&1_u64.to_le_bytes());
            std::fs::write(&path, &original).unwrap();
            let service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
            assert!(next(&service).error.is_none());
            Self {
                directory,
                path,
                original,
                service,
            }
        }
        fn edit(&self, name: &str) -> Event {
            self.service
                .submit(Request::Edit(Command::AddTrack { name: name.into() }))
                .unwrap();
            next(&self.service)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    fn next(service: &Service) -> Event {
        // Blocks on the event channel; bounded only against hangs.
        match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
            Ok(event) => event,
            Err(error) => panic!("next: worker event absent: {error}"),
        }
    }
    fn intent(event: &Event) -> RecoveryRestart {
        let failure = event.recovery_failure.as_ref().unwrap();
        RecoveryRestart {
            project_generation: failure.project_generation,
            revision: event.state.revision,
            failure_id: failure.id,
        }
    }

    #[test]
    fn solo_view_precedes_recovery_terminal_without_claiming_durability() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "hex-solo-recovery-{}-{}",
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let visible = loop {
            if let Ok(view) = service.try_solo_view() {
                break view;
            }
            assert!(std::time::Instant::now() < deadline, "no solo observation");
            std::thread::yield_now();
        };
        assert!(visible.tracks.contains(&(track, true)));
        assert!(matches!(
            service.try_event(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        let complete = next(&service);
        assert!(complete.request_complete && complete.error.is_none());
        assert_eq!(visible.revision, complete.state.revision);
        assert_eq!(
            hex_storage::load_journal(&path).unwrap(),
            *complete.state.project
        );
        service.shutdown(std::time::Duration::from_secs(2)).unwrap();
        drop(service);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn duplicate_view_precedes_checkpoint_but_the_terminal_keeps_recovery_custody() {
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
            let display = service.try_duplicate_view().unwrap();
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
            assert!(matches!(
                service.try_event(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            release.send(()).unwrap();
            let terminal = next(&service);
            assert!(terminal.request_complete && terminal.error.is_none());
            assert!(Arc::ptr_eq(&observed, &terminal.state));
            assert_eq!(
                hex_storage::load_journal(&path).unwrap(),
                *terminal.state.project
            );
            let Some(Created::Clips { first, last }) = terminal.created else {
                panic!("created duplicate");
            };
            assert_eq!(first, last);
            clip = first;
            revision = terminal.state.revision;
        }
        service.submit(Request::Undo).unwrap();
        let unrelated = next(&service);
        assert!(unrelated.request_complete && unrelated.error.is_none());
        let rows = diagnostic_rows.lock().unwrap();
        for kind in ["duplicate_clips", "duplicate_alias_clips"] {
            let submitted: Vec<_> = rows
                .iter()
                .filter_map(|(request, stage, _)| {
                    request.filter(|request| {
                        request.kind == kind
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
            request.is_some_and(|request| {
                matches!(request.kind, "duplicate_clips" | "duplicate_alias_clips")
            }) || !details
                .split_whitespace()
                .any(|part| part.starts_with("duplicate_recovery_us="))
        }));
        drop(rows);
        service.shutdown(std::time::Duration::from_secs(2)).unwrap();
        drop(service);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn solo_observation_does_not_hide_a_recovery_conflict() {
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
        let view = service.try_solo_view().unwrap();
        assert!(view.tracks.contains(&(track, true)));
        std::fs::write(&path, b"other writer").unwrap();
        release.send(()).unwrap();
        let terminal = next(&service);
        assert!(terminal.request_complete && terminal.error.is_some());
        assert!(terminal.recovery_failure.is_some());
        assert!(terminal.state.project.track(track).unwrap().soloed());
        assert_eq!(std::fs::read(&path).unwrap(), b"other writer");
        service.shutdown(std::time::Duration::from_secs(2)).unwrap();
        drop(service);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unread_solo_observation_never_blocks_later_terminals() {
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
        let retained = service.try_solo_view().unwrap();
        assert!(retained.tracks.contains(&(track, true)));
        service.shutdown(std::time::Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn unavailable_slot_is_persistent_and_confirmed_restart_preserves_history_save_and_recovery() {
        let fixture = Fixture::new();
        let first = fixture.edit("first");
        assert!(
            first
                .error
                .as_ref()
                .unwrap()
                .contains("unsupported older project schema")
        );
        assert!(first.state.unsaved && first.state.can_undo);
        assert_eq!(first.state.project.tracks().len(), 1);
        let failure = first.recovery_failure.as_ref().unwrap();
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
        assert_eq!(
            hex_storage::load_journal(&fixture.path).unwrap().snapshot(),
            subsequent.state.project.snapshot()
        );
        for (request, count) in [(Request::Undo, 2), (Request::Redo, 3)] {
            fixture.service.submit(request).unwrap();
            let event = next(&fixture.service);
            assert!(event.error.is_none() && event.recovery_failure.is_none());
            assert_eq!(event.state.project.tracks().len(), count);
            assert_eq!(
                hex_storage::load_journal(&fixture.path).unwrap().snapshot(),
                event.state.project.snapshot()
            );
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
        let first = fixture.edit("first");
        let stale = intent(&first);
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
}
