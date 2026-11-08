//! One control-owned recovery copy and its original custody.
use super::{DraftWorkerOutcome, copy_failed_on_control};
use hex_storage::automation_draft::DraftReadLimits;
use std::{
    path::PathBuf,
    sync::Arc,
    thread::{self, JoinHandle},
};

/// Completion retains the original writer outcome even if copying failed.
pub struct RecoveryCopyCompletion {
    pub outcome: Arc<DraftWorkerOutcome>,
    pub result: Result<(), String>,
}

struct Running {
    outcome: Arc<DraftWorkerOutcome>,
    thread: JoinHandle<Result<(), String>>,
}

/// A single worker; retain the caller's own outcome until recovery is resolved.
/// Poll from control, never the audio callback. Dropping a running worker joins it
/// and can wait for disk I/O; only the shutdown/control owner may drop it.
#[derive(Default)]
pub struct RecoveryCopyWorker {
    running: Option<Running>,
}

impl RecoveryCopyWorker {
    pub fn start(
        &mut self,
        outcome: &Arc<DraftWorkerOutcome>,
        destination: PathBuf,
        limits: DraftReadLimits,
        max_events: u64,
    ) -> Result<(), String> {
        self.start_with(outcome, move |outcome| {
            copy_failed_on_control(outcome, &destination, limits, max_events)
        })
    }

    pub(in crate::service) fn start_with(
        &mut self,
        outcome: &Arc<DraftWorkerOutcome>,
        copy: impl FnOnce(&DraftWorkerOutcome) -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        if self.running.is_some() {
            return Err("automation recovery copy is already running".into());
        }
        let thread_outcome = Arc::clone(outcome);
        let thread = thread::Builder::new()
            .name("hex-automation-recovery".into())
            .spawn(move || copy(&thread_outcome))
            .map_err(|error| format!("could not start automation recovery: {error}"))?;
        self.running = Some(Running {
            outcome: Arc::clone(outcome),
            thread,
        });
        Ok(())
    }

    /// Returns a completion only after the thread has finished; it never waits for disk.
    pub fn poll(&mut self) -> Option<RecoveryCopyCompletion> {
        if !self
            .running
            .as_ref()
            .is_some_and(|running| running.thread.is_finished())
        {
            return None;
        }
        let running = self.running.take()?;
        let result = running.thread.join().unwrap_or_else(|_| {
            Err("automation recovery worker failed; retain original custody".into())
        });
        Some(RecoveryCopyCompletion {
            outcome: running.outcome,
            result,
        })
    }
}

impl Drop for RecoveryCopyWorker {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            let _ = running.thread.join();
        }
    }
}
