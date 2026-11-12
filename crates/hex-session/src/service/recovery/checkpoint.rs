//! Worker-owned recovery availability and explicitly confirmed slot replacement.
use super::*;

#[cfg(test)]
type CheckpointGate = (
    PathBuf,
    std::sync::mpsc::SyncSender<()>,
    std::sync::mpsc::Receiver<()>,
);
#[cfg(test)]
pub(super) static CHECKPOINT_GATE: std::sync::Mutex<Vec<CheckpointGate>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub(super) fn wait_at_checkpoint(path: &Path) {
    let gate = {
        let mut gates = CHECKPOINT_GATE.lock().unwrap();
        gates
            .iter()
            .position(|(target, _, _)| target == path)
            .map(|index| gates.remove(index))
    };
    if let Some((_, entered, release)) = gate {
        let _ = entered.send(());
        let _ = release.recv_timeout(std::time::Duration::from_millis(500));
    }
}

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
    worker: Option<super::checkpoint_worker::CheckpointWorker>,
    pending: Option<(PathBuf, u64, std::time::Instant)>,
}
impl RecoveryTracker {
    /// Start a changed, unsaved checkpoint. The caller must retain its exact
    /// outcome and fence journal lifecycle operations until completion.
    pub fn start_async(&mut self, context: RecoveryContext<'_>) -> Result<bool, String> {
        if self.pending.is_some() {
            return Err("recovery checkpoint is already pending".into());
        }
        let path = checkpoint_path(context.document, context.untitled)?;
        if self.failure.as_ref().is_some_and(|failure| {
            failure.path != path || failure.project_generation != context.generation
        }) {
            self.clear();
        }
        if self
            .failure
            .as_ref()
            .is_some_and(|failure| failure.can_restart())
        {
            return Ok(false);
        }
        let started = std::time::Instant::now();
        let result = PreparedCheckpoint::capture(&context).and_then(|checkpoint| {
            if self.worker.is_none() {
                self.worker = Some(super::checkpoint_worker::CheckpointWorker::new()?);
            }
            let worker = self
                .worker
                .as_mut()
                .ok_or("recovery checkpoint worker is unavailable")?;
            worker.start(checkpoint, context.journal)
        });
        match result {
            Ok(()) => {
                self.pending = Some((path, context.generation, started));
                Ok(true)
            }
            Err(reason) => {
                self.worker.take();
                self.failed(path, context.generation, reason.clone(), None);
                Err(reason)
            }
        }
    }

    pub fn async_ready(&self) -> bool {
        self.pending.is_some() && self.worker.as_ref().is_some_and(|worker| worker.ready())
    }

    /// Collect the sole pending publication on control and update the same
    /// persistent failure state used by synchronous maintenance.
    pub fn finish_async(
        &mut self,
        journal: &mut Option<hex_storage::Journal>,
    ) -> (std::time::Duration, Result<Option<PathBuf>, String>) {
        let Some((path, generation, started)) = self.pending.take() else {
            return (
                std::time::Duration::ZERO,
                Err("no recovery checkpoint is pending".into()),
            );
        };
        let (completed, result) = match self.worker.as_mut() {
            Some(worker) => worker.finish_timed(journal),
            None => (
                std::time::Instant::now(),
                Err("recovery checkpoint worker is unavailable".into()),
            ),
        };
        match &result {
            Ok(_) => self.clear(),
            Err(reason) => {
                self.worker.take();
                self.failed(path, generation, reason.clone(), None);
            }
        }
        (completed.saturating_duration_since(started), result)
    }
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
            PreparedCheckpoint::capture(&context).and_then(|checkpoint| {
                if self.worker.is_none() {
                    self.worker = Some(super::checkpoint_worker::CheckpointWorker::new()?);
                }
                let worker = self
                    .worker
                    .as_mut()
                    .ok_or("recovery checkpoint worker is unavailable")?;
                let result = worker
                    .start(checkpoint, context.journal)
                    .and_then(|()| worker.finish(context.journal));
                if result.is_err() {
                    // A later explicit restart must be able to create a fresh
                    // worker even when the previous one disconnected.
                    self.worker.take();
                }
                result
            })
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
    struct SourceSink(Arc<std::sync::Mutex<Vec<hex_engine::AudioSourceContext>>>);
    impl WorkerAudioSink for SourceSink {
        fn initialize(&mut self, _: &std::sync::atomic::AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
            self.0.lock().unwrap().push(source.context());
        }
        fn poll(&mut self) {}
        fn close(&mut self) {}
    }
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
    #[test]
    fn locator_recovery_timing_covers_the_held_checkpoint() {
        use super::super::diagnostics::{DiagnosticObserver, DiagnosticStage};
        use std::time::Duration;

        let directory =
            std::env::temp_dir().join(format!("hex-locator-timing-{}", std::process::id()));
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

    fn prepare_paste(service: &Service) -> hex_project::TrackId {
        next(service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "source".into(),
            }))
            .unwrap();
        let track = next(service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::ONE,
            }))
            .unwrap();
        let original = next(service);
        let clip = original.state.project.track(track).unwrap().midi_clips()[0].id();
        service.submit(Request::CopyClips(vec![clip])).unwrap();
        assert!(next(service).error.is_none());
        track
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
        assert!(
            matches!(first_view.kind, ArrangementEditViewKind::LocatorAdd(id) if id == first_id)
        );
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
        let second_view =
            second_view.expect("second committed locator view before recovery terminal");
        let second_state = second_view.state.upgrade().unwrap();
        assert_eq!(second_state.revision, initial + 2);
        let second_id = second_state.project.arrangement_locators()[1].id();
        assert!(
            matches!(second_view.kind, ArrangementEditViewKind::LocatorAdd(id) if id == second_id)
        );
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
        let (
            Some(Created::ArrangementLocator(first_id)),
            Some(Created::ArrangementLocator(second_id)),
        ) = (first.created, second.created)
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
            let replacement = super::super::diagnostics::EventReceiver::new(
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
            let replacement = super::super::diagnostics::EventReceiver::new(
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
        let directory =
            std::env::temp_dir().join(format!("hex-time-copy-full-{}", std::process::id()));
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
        let track = next(&service).state.project.tracks()[0].id();
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
        let until = Instant::now() + Duration::from_millis(180);
        let second = loop {
            if let Ok(view) = service.try_track_switch_view() {
                break Some(view);
            }
            if Instant::now() >= until {
                break None;
            }
            std::thread::yield_now();
        };
        let until = Instant::now() + Duration::from_millis(180);
        let source_applied = loop {
            if let Some(view) = second.as_ref()
                && sources.lock().unwrap().iter().any(|(context, states)| {
                    context.editor_revision == view.revision && states == &[false]
                })
            {
                break true;
            }
            if Instant::now() >= until {
                break false;
            }
            std::thread::yield_now();
        };
        release.send(()).unwrap();
        let second = second.expect("second mute must commit while the first checkpoint is held");
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
        let first_terminal = next(&service);
        let second_terminal = next(&service);
        assert_eq!(first_terminal.state.revision, first.revision);
        assert_eq!(second_terminal.state.revision, second.revision);
        if conflict {
            assert!(
                first_terminal.recovery_failure.is_some()
                    && second_terminal.recovery_failure.is_some()
            );
            assert_eq!(std::fs::read(&path).unwrap(), foreign);
        } else {
            assert!(first_terminal.error.is_none() && second_terminal.error.is_none());
            assert_eq!(
                hex_storage::load_journal(&path).unwrap(),
                *second_terminal.state.project
            );
        }
        service.submit(Request::Undo).unwrap();
        assert!(next(&service).state.project.track(track).unwrap().muted());
        service.submit(Request::Undo).unwrap();
        assert!(!next(&service).state.project.track(track).unwrap().muted());
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
            let replacement = super::super::diagnostics::EventReceiver::new(
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
    fn paste_waits_for_pending_track_switch_checkpoint() {
        use std::time::Duration;
        let directory =
            std::env::temp_dir().join(format!("hex-mute-paste-fence-{}", std::process::id()));
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
            .submit(Request::Edit(Command::ToggleMute(track)))
            .unwrap();
        entered.recv_timeout(Duration::from_millis(500)).unwrap();
        service
            .submit(Request::PasteTimeRange {
                track,
                frame: 48_000,
            })
            .unwrap();
        assert!(matches!(
            service.events.recv_timeout(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(matches!(
            service.try_arrangement_edit_view(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        let first = next(&service);
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
        assert_eq!(
            hex_storage::load_journal(&path).unwrap(),
            *second.state.project
        );
        drop(service);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn track_switch_view_precedes_recovery_terminal_without_claiming_durability() {
        let directory =
            std::env::temp_dir().join(format!("hex-track-switch-recovery-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("untitled.hex-journal");
        let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
        let diagnostics = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&diagnostics);
        service
            .set_diagnostic_observer(Arc::new(
                super::super::diagnostics::DiagnosticObserver::new(move |event| {
                    if matches!(
                        event.stage,
                        super::super::diagnostics::DiagnosticStage::OutcomeReady {
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
        for name in ["first", "second"] {
            service
                .submit(Request::Edit(Command::AddTrack { name: name.into() }))
                .unwrap();
            next(&service);
        }
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
            assert!(matches!(
                service.try_event(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            release.send(()).unwrap();
            let complete = next(&service);
            assert!(complete.request_complete && complete.error.is_none());
            assert_eq!(visible.revision, complete.state.revision);
            preceding = hex_storage::load_journal(&path).unwrap();
            assert_eq!(preceding, *complete.state.project);
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
    fn section_copy_view_precedes_the_durable_terminal() {
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
        assert!(matches!(
            service.try_event(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        let committed = next(&service);
        assert!(committed.request_complete && committed.error.is_none());
        assert!(Arc::ptr_eq(&visible, &committed.state));
        assert_eq!(
            hex_storage::load_journal(&path).unwrap(),
            *committed.state.project
        );
        service.submit(Request::Undo).unwrap();
        let mut undone = next(&service).state.project.snapshot();
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
        let track = prepare_paste(&service);
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
        assert_eq!(
            hex_storage::load_journal(&path).unwrap(),
            *second.state.project
        );
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
        let track = prepare_paste(&service);
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
        let until = Instant::now() + Duration::from_millis(180);
        let second = loop {
            if let Ok(view) = service.try_arrangement_edit_view() {
                break Some(view);
            }
            if Instant::now() >= until {
                break None;
            }
            std::thread::yield_now();
        };
        let second = second.expect("second paste must commit while the first checkpoint is held");
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
        let first_terminal = next(&service);
        let second_terminal = next(&service);
        if conflict {
            assert!(first_terminal.error.is_some());
            assert!(first_terminal.recovery_failure.is_some());
            assert!(second_terminal.recovery_failure.is_some());
            assert_eq!(std::fs::read(&path).unwrap(), foreign);
        } else {
            assert!(first_terminal.error.is_none() && second_terminal.error.is_none());
            assert_eq!(
                hex_storage::load_journal(&path).unwrap(),
                *second_terminal.state.project
            );
        }
        assert_eq!(second_terminal.state.revision, second_state.revision);
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
        let first_redo = next(&service);
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
        let second_redo = next(&service);
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
            assert_eq!(
                hex_storage::load_journal(&path).unwrap(),
                *second_redo.state.project
            );
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
        let track = prepare_paste(&service);
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
        let paste = next(&service);
        assert!(paste.error.is_none());
        assert_eq!(paste.state.revision, pasted.revision);
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
        let directory =
            std::env::temp_dir().join(format!("hex-paste-replace-{}", std::process::id()));
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
    fn third_paste_waits_for_the_single_pending_checkpoint_slot() {
        use std::time::{Duration, Instant};
        let directory = std::env::temp_dir().join(format!("hex-paste-full-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("untitled.hex-journal");
        let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
        let track = prepare_paste(&service);
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
        let second_revision = second.state.upgrade().unwrap().revision;
        service
            .submit(Request::PasteTimeRange {
                track,
                frame: 144_000,
            })
            .unwrap();
        let until = Instant::now() + Duration::from_millis(60);
        while Instant::now() < until {
            assert!(matches!(
                service.try_arrangement_edit_view(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            std::thread::yield_now();
        }
        release.send(()).unwrap();
        let first = next(&service);
        let second = next(&service);
        let third = next(&service);
        assert!(first.error.is_none() && second.error.is_none() && third.error.is_none());
        assert_eq!(first.state.revision, first_revision);
        assert_eq!(second.state.revision, second_revision);
        assert_eq!(third.state.revision, second_revision + 1);
        assert_eq!(
            third.state.project.track(track).unwrap().midi_clips().len(),
            4
        );
        assert_eq!(
            hex_storage::load_journal(&path).unwrap(),
            *third.state.project
        );
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
        use super::super::diagnostics::{DiagnosticObserver, DiagnosticStage};
        use std::time::{Duration, Instant};
        let directory =
            std::env::temp_dir().join(format!("hex-paste-events-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("untitled.hex-journal");
        let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
        let track = prepare_paste(&service);
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
        let until = Instant::now() + Duration::from_millis(180);
        let second = loop {
            if let Ok(view) = service.try_arrangement_edit_view() {
                break view;
            }
            assert!(Instant::now() < until, "second paste did not commit");
            std::thread::yield_now();
        };
        let second_revision = second.state.upgrade().unwrap().revision;
        release.send(()).unwrap();
        let _first_copy = next(&service);
        let _second_copy = next(&service);
        let first = next(&service);
        let second = next(&service);
        assert!(first.error.is_none() && second.error.is_none());
        assert_eq!(
            (first.state.revision, second.state.revision),
            (first_revision, second_revision)
        );
        assert_eq!(
            hex_storage::load_journal(&path).unwrap(),
            *second.state.project
        );
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
        let track = prepare_paste(&service);
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
            let replacement = super::super::diagnostics::EventReceiver::new(
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
        let track = prepare_paste(&service);
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
        let view = service.try_arrangement_edit_view().unwrap();
        let pasted = view.state.upgrade().unwrap();
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
        let paste = next(&service);
        assert!(paste.error.is_none());
        assert_eq!(paste.state.revision, pasted.revision);
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
        let directory =
            std::env::temp_dir().join(format!("hex-paste-shutdown-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("untitled.hex-journal");
        let mut service = Service::with_recovery(48_000, 16, 8_000_000, path.clone()).unwrap();
        let track = prepare_paste(&service);
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
    fn paste_view_precedes_checkpoint_for_single_clip_and_multi_clip_insert() {
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
            assert!(matches!(service.try_event(), Err(TryRecvError::Empty)));
            release.send(()).unwrap();

            let terminal = next(&service);
            assert!(terminal.request_complete && terminal.error.is_none());
            assert!(Arc::ptr_eq(&committed, &terminal.state));
            assert_eq!(
                hex_storage::load_journal(&path).unwrap(),
                *committed.project
            );
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

        let directory =
            std::env::temp_dir().join(format!("hex-midi-timing-{}", std::process::id()));
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
        let Some(Created::Note(other)) = next(&service).created else {
            panic!("second note")
        };
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
            assert!(matches!(
                service.try_event(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            release.send(()).unwrap();
            let terminal = next(&service);
            assert!(terminal.request_complete && terminal.error.is_none());
            assert_eq!(
                hex_storage::load_journal(&path).unwrap(),
                *terminal.state.project
            );
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
        assert_eq!(
            hex_storage::load_journal(&path).unwrap(),
            *range_terminal.state.project
        );

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
            request
                .is_some_and(|request| copy_requests.contains(&(request.kind, request.operation)))
                || !details
                    .split_whitespace()
                    .any(|part| part.starts_with("duplicate_recovery_us="))
        }));
        drop(rows);
        service.shutdown(std::time::Duration::from_secs(2)).unwrap();
        drop(service);
        std::fs::remove_dir_all(directory).unwrap();
    }

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
