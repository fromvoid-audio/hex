//! Worker-owned recovery availability and explicitly confirmed slot replacement.
use super::*;

#[cfg(any(test, feature = "recovery-checkpoint-test-support"))]
type CheckpointGate = (
    PathBuf,
    std::sync::mpsc::SyncSender<()>,
    std::sync::mpsc::Receiver<()>,
);
#[cfg(any(test, feature = "recovery-checkpoint-test-support"))]
pub(super) static CHECKPOINT_GATE: std::sync::Mutex<Vec<CheckpointGate>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(any(test, feature = "recovery-checkpoint-test-support"))]
pub(super) fn wait_at_checkpoint(path: &Path) {
    let gate = {
        let mut gates = CHECKPOINT_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        gates
            .iter()
            .position(|(target, _, _)| target == path)
            .map(|index| gates.remove(index))
    };
    if let Some((_, entered, release)) = gate {
        let _ = entered.send(());
        let timeout = if cfg!(feature = "recovery-checkpoint-test-support") {
            std::time::Duration::from_secs(2)
        } else {
            std::time::Duration::from_millis(500)
        };
        let _ = release.recv_timeout(timeout);
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
pub(in crate::service) struct CompletedCheckpoint {
    pub(in crate::service) result: Result<Option<PathBuf>, String>,
    pub(in crate::service) failure_changed: bool,
}

struct InFlightCheckpoint {
    path: PathBuf,
    generation: u64,
}

struct CoalescedCheckpoint {
    path: PathBuf,
    generation: u64,
    checkpoint: PreparedCheckpoint,
}

#[derive(Default)]
pub(in crate::service) struct RecoveryTracker {
    failure: Option<Arc<RecoveryFailure>>,
    // Exact old bytes remain exclusively on the service worker. Events carry
    // metadata and a failure ID, never this storage token or its allocation.
    restart: Option<hex_storage::JournalRestartToken>,
    sequence: u64,
    worker: Option<super::checkpoint_worker::CheckpointWorker>,
    in_flight: Option<InFlightCheckpoint>,
    coalesced: Option<CoalescedCheckpoint>,
}
impl RecoveryTracker {
    /// Whether any background checkpoint is currently writing or coalesced to run next.
    pub fn has_pending(&self) -> bool {
        self.in_flight.is_some() || self.coalesced.is_some()
    }

    /// Whether an in-flight checkpoint has finished on the worker and can be collected now.
    pub fn async_ready(&self) -> bool {
        self.in_flight.is_some() && self.worker.as_ref().is_some_and(|worker| worker.ready())
    }

    fn start_in_flight(
        &mut self,
        path: PathBuf,
        generation: u64,
        checkpoint: PreparedCheckpoint,
        journal: &mut Option<hex_storage::Journal>,
    ) -> Result<(), String> {
        if self.worker.is_none() {
            self.worker = Some(super::checkpoint_worker::CheckpointWorker::new()?);
        }
        let worker = self
            .worker
            .as_mut()
            .ok_or("recovery checkpoint worker is unavailable")?;
        match worker.start(checkpoint, journal) {
            Ok(()) => {
                self.in_flight = Some(InFlightCheckpoint { path, generation });
                Ok(())
            }
            Err(reason) => {
                self.worker.take();
                self.failed(path, generation, reason.clone(), None);
                Err(reason)
            }
        }
    }

    fn collect_in_flight(
        &mut self,
        journal: &mut Option<hex_storage::Journal>,
    ) -> Option<CompletedCheckpoint> {
        let InFlightCheckpoint { path, generation } = self.in_flight.take()?;
        let had_failure = self.failure.is_some();
        let result = match self.worker.as_mut() {
            Some(worker) => worker.finish(journal),
            None => Err("recovery checkpoint worker is unavailable".into()),
        };
        match &result {
            Ok(_) => self.clear(),
            Err(reason) => {
                self.worker.take();
                self.failed(path, generation, reason.clone(), None);
            }
        }
        let failure_changed = had_failure != self.failure.is_some() || result.is_err();
        Some(CompletedCheckpoint {
            result,
            failure_changed,
        })
    }

    fn start_coalesced_if_idle(
        &mut self,
        journal: &mut Option<hex_storage::Journal>,
    ) -> Option<CompletedCheckpoint> {
        if self.in_flight.is_some() {
            return None;
        }
        let next = self.coalesced.take()?;
        if self.failure.as_ref().is_some_and(|failure| {
            failure.path != next.path || failure.project_generation != next.generation
        }) {
            self.clear();
        }
        if self
            .failure
            .as_ref()
            .is_some_and(|failure| failure.can_restart())
        {
            return None;
        }
        match self.start_in_flight(next.path, next.generation, next.checkpoint, journal) {
            Ok(()) => None,
            Err(reason) => Some(CompletedCheckpoint {
                result: Err(reason),
                failure_changed: true,
            }),
        }
    }

    /// Capture an in-memory snapshot and either start it immediately on the
    /// background checkpoint worker or coalesce it over any unstarted snapshot.
    /// Never blocks on disk I/O or `fsync()`.
    pub fn schedule_async(
        &mut self,
        context: RecoveryContext<'_>,
    ) -> Result<Vec<CompletedCheckpoint>, String> {
        let path = checkpoint_path(context.document, context.untitled)?;
        if self.failure.as_ref().is_some_and(|failure| {
            failure.path != path || failure.project_generation != context.generation
        }) {
            self.clear();
        }
        let mut completed = Vec::new();
        if self.async_ready()
            && let Some(done) = self.collect_in_flight(context.journal)
        {
            completed.push(done);
        }
        if self
            .failure
            .as_ref()
            .is_some_and(|failure| failure.can_restart())
        {
            self.coalesced = None;
            return Ok(completed);
        }
        let checkpoint = match PreparedCheckpoint::capture(&context) {
            Ok(checkpoint) => checkpoint,
            Err(reason) => {
                self.failed(path, context.generation, reason.clone(), None);
                return Err(reason);
            }
        };
        if self.in_flight.is_none() {
            self.coalesced = None;
            self.start_in_flight(path, context.generation, checkpoint, context.journal)?;
        } else {
            self.coalesced = Some(CoalescedCheckpoint {
                path,
                generation: context.generation,
                checkpoint,
            });
        }
        Ok(completed)
    }

    /// Non-blockingly collect a finished in-flight checkpoint and start any
    /// coalesced latest snapshot on the background worker.
    pub fn poll_complete(
        &mut self,
        journal: &mut Option<hex_storage::Journal>,
    ) -> Vec<CompletedCheckpoint> {
        let mut completed = Vec::new();
        if self.async_ready()
            && let Some(done) = self.collect_in_flight(journal)
        {
            completed.push(done);
        }
        if let Some(done) = self.start_coalesced_if_idle(journal) {
            completed.push(done);
        }
        completed
    }

    /// Block only at explicit durability/lifecycle fences (Save, Open, New,
    /// RestartRecovery, sample-rate replacement, and shutdown/exit) until both
    /// the in-flight checkpoint and any coalesced latest snapshot have finished
    /// and returned journal custody.
    pub fn flush_blocking(
        &mut self,
        journal: &mut Option<hex_storage::Journal>,
        mut poll_audio: impl FnMut(),
    ) -> Vec<CompletedCheckpoint> {
        let mut completed = Vec::new();
        loop {
            while self.in_flight.is_some() {
                while !self.async_ready() {
                    poll_audio();
                    std::thread::sleep(super::super::stopped_arrangement::FENCE_POLL);
                }
                if let Some(done) = self.collect_in_flight(journal) {
                    completed.push(done);
                }
            }
            if let Some(done) = self.start_coalesced_if_idle(journal) {
                completed.push(done);
            }
            if self.in_flight.is_none() && self.coalesced.is_none() {
                break;
            }
        }
        completed
    }

    /// Clear recovery for a clean/saved state. Discards any unstarted coalesced
    /// snapshot, reclaims journal custody from an in-flight checkpoint if needed,
    /// and clears the on-disk journal synchronously.
    pub fn clear_for_saved(
        &mut self,
        context: RecoveryContext<'_>,
        mut poll_audio: impl FnMut(),
    ) -> Result<Option<PathBuf>, String> {
        self.coalesced = None;
        while self.in_flight.is_some() {
            while !self.async_ready() {
                poll_audio();
                std::thread::sleep(super::super::stopped_arrangement::FENCE_POLL);
            }
            let _ = self.collect_in_flight(context.journal);
        }
        self.maintain(context, false, false)
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
mod tests;
