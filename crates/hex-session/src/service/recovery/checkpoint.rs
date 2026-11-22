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

    /// Whether an admitted publication still needs its control owner to wait.
    pub fn async_waiting(&self) -> bool {
        self.pending.is_some() && self.worker.as_ref().is_some_and(|worker| !worker.ready())
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
mod tests;
