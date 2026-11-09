//! Bounded control-side ownership between delivered automation receipts and durable drafts.
//!
//! The canonical worker normalizes the source once before spawning this separate disk lane.
//! Three preallocated 64-event loans cover reservations, queued/writing batches, unpolled ACKs
//! and rejected batches together. Reserve a loan BEFORE draining audio receipts. A refusal
//! returns its owned data; it cannot be silently restamped onto another lane. Accepted input
//! and durable sync are distinct. Terminal intent has an independent slot and follows every
//! admitted batch. A failed writer retains its unsynced batches for explicit recovery.
//!
//! No callback, UI, canonical edit, playback evaluator or event-file reclamation lives here.
//! The joining owner belongs on Session control, never GPUI or an audio thread. Disk operations
//! are not interruptible; audio Stop remains with the independent audio endpoint. Drop requests
//! interruption, joins on control and leaves the outcome in the shared observation owner. Files
//! survive every Drop. Later canonical adoption must validate source/target and acquire file
//! custody; producing a durable draft alone does not implement a playable recording workflow.
use super::{Session, recovery};
use hex_storage::automation_draft::DraftWriter;
mod native_device;
mod recovery_copy;
pub(in crate::service) use recovery_copy::copy_outcome_with_tail_on_control;
mod recovery_worker;
pub use hex_storage::automation_draft::{
    AppliedEvent, DraftAbortReason, DraftClock, DraftPrefixIdentity, DraftProgress,
    DraftProvenance, DraftTarget, DraftTerminal,
};
pub(in crate::service) use native_device::native_application_clock;
pub use native_device::{
    DeviceDraftInput, Vst3ApplicationValue, Vst3CaptureTarget, Vst3ParameterApplication,
};
pub use recovery_copy::{RecoveryCopyReceipt, copy_failed_on_control};
pub use recovery_worker::{RecoveryCopyCompletion, RecoveryCopyWorker};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::{self, JoinHandle},
};

pub const BATCH_SLOTS: usize = 3;
pub const EVENTS_PER_BATCH: usize = hex_storage::automation_draft::MAX_APPEND_EVENTS;
const ACK_SLOTS: usize = BATCH_SLOTS + 1;

/// Independent recovery files live outside rotating journal/Save As slots.
/// This query does not create a directory or fall back to another user's home.
pub fn default_directory() -> Result<PathBuf, String> {
    let journal = recovery::untitled_recovery_path()?;
    journal
        .parent()
        .map(|parent| parent.join("automation-drafts"))
        .ok_or_else(|| "automation recovery directory is unavailable".into())
}
pub struct DraftDiscovery {
    /// Candidate paths only; explicit opening validates source, limits and integrity.
    pub candidates: Vec<PathBuf>,
    /// Work was bounded by examined directory entries, including unknown files.
    pub truncated: bool,
}
/// A control-owned directory cursor keeps discovery bounded without imposing a lifetime
/// file-count ceiling. Candidate identity is its complete path, never a painted row index.
/// Directory changes can affect OS enumeration; callers reconcile refreshed rows by path.
pub struct DraftDiscoveryCursor {
    entries: Option<std::fs::ReadDir>,
    pending: Option<std::io::Result<std::fs::DirEntry>>,
}
impl DraftDiscoveryCursor {
    pub fn open_on_control(directory: &Path) -> Result<Self, String> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        Ok(Self {
            entries,
            pending: None,
        })
    }
    /// Unknown files count toward the work budget; symlinks are never followed or modified.
    /// At most one prefetched directory entry survives between pages.
    pub fn next_page_on_control(&mut self, max_entries: usize) -> Result<DraftDiscovery, String> {
        if !(1..=4096).contains(&max_entries) {
            return Err("automation draft discovery budget must be within 1..4096".into());
        }
        let mut candidates = Vec::new();
        candidates
            .try_reserve_exact(max_entries)
            .map_err(|error| error.to_string())?;
        for _ in 0..max_entries {
            let entry = self
                .pending
                .take()
                .or_else(|| self.entries.as_mut().and_then(Iterator::next));
            let Some(entry) = entry else {
                candidates.sort();
                return Ok(DraftDiscovery {
                    candidates,
                    truncated: false,
                });
            };
            let entry = entry.map_err(|error| error.to_string())?;
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "hex-automation-draft")
            {
                candidates.push(entry.path());
            }
        }
        self.pending = self.entries.as_mut().and_then(Iterator::next);
        candidates.sort();
        Ok(DraftDiscovery {
            candidates,
            truncated: self.pending.is_some(),
        })
    }
}
/// One bounded first-page query. Use a cursor to enumerate the remaining files. No automatic
/// reclamation exists: abandoning input preserves the durable draft for explicit recovery.
pub fn discover_on_control(directory: &Path, max_entries: usize) -> Result<DraftDiscovery, String> {
    DraftDiscoveryCursor::open_on_control(directory)?.next_page_on_control(max_entries)
}

struct Batch {
    slot: usize,
    ticket: u64,
    events: [AppliedEvent; EVENTS_PER_BATCH],
    len: usize,
}
impl Batch {
    fn events(&self) -> &[AppliedEvent] {
        &self.events[..self.len]
    }
}
enum Ack {
    Durable(Box<Batch>, DraftProgress),
    Failed(String, DraftProgress),
    Terminal(DraftProgress, Option<String>),
}
struct State {
    queue: VecDeque<Box<Batch>>,
    acknowledgements: VecDeque<Ack>,
    terminal: Option<DraftTerminal>,
    accepted_events: u64,
    progress: DraftProgress,
    failed: bool,
    finished: bool,
    outcome: Option<DraftWorkerOutcome>,
    #[cfg(feature = "recording-fault-test-support")]
    fail_next_append: bool,
}
struct Shared {
    path: PathBuf,
    state: Mutex<State>,
    wake: Condvar,
}
impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poison) => {
                let mut state = poison.into_inner();
                state.failed = true;
                state.terminal.get_or_insert(DraftTerminal::Interrupted);
                self.wake.notify_one();
                state
            }
        }
    }
    fn finish(&self, terminal: DraftTerminal) -> Result<(), &'static str> {
        let mut state = self.state();
        if state.terminal.is_some() {
            return Err("automation draft terminal is already owned");
        }
        state.terminal = Some(terminal);
        self.wake.notify_one();
        Ok(())
    }
}

/// One non-Clone input and its three reusable buffer loans. No file or joining owner escapes.
pub struct DraftInput {
    shared: Arc<Shared>,
    free: [Option<Box<Batch>>; BATCH_SLOTS],
    next_ticket: u64,
}
/// Joining/control owner, kept separately from any UI-facing input observation.
pub struct DraftWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DraftLaneProgress {
    pub accepted_events: u64,
    pub durable_events: u64,
    pub terminal_requested: Option<DraftTerminal>,
    pub terminal_durable: Option<DraftTerminal>,
    pub failed: bool,
    pub finished: bool,
}
#[derive(Debug)]
pub enum DraftWriteAck {
    Durable {
        ticket: u64,
        progress: DraftProgress,
    },
    Failed {
        message: String,
        progress: DraftProgress,
    },
    Terminal {
        progress: DraftProgress,
        failure: Option<String>,
    },
}

/// Original-source data retained after writer refusal/failure, never an adoptable lane.
pub struct RetainedBatch(Box<Batch>);
impl RetainedBatch {
    pub fn ticket(&self) -> u64 {
        self.0.ticket
    }
    pub fn events(&self) -> &[AppliedEvent] {
        self.0.events()
    }
}
pub struct DraftWorkerOutcome {
    pub path: PathBuf,
    pub admitted_events: u64,
    pub progress: DraftProgress,
    /// Exact header and last sync-acknowledged event chain, retained after writer failure.
    pub durable_prefix: DraftPrefixIdentity,
    pub requested_terminal: DraftTerminal,
    pub failure: Option<String>,
    /// At most three original batches, including any partially written/synced batch.
    /// Recovery must compare the file's validated prefix; blindly appending duplicates is wrong.
    pub retained: Vec<RetainedBatch>,
}

/// Normalize source paths through the existing saved/recovery owner before creating a file.
/// Provenance is audit information; the canonical caller owns its live revision admission.
pub fn start_on_control(
    session: &Session,
    document_base: Option<&Path>,
    path: PathBuf,
    target: DraftTarget,
    provenance: DraftProvenance,
) -> Result<(DraftInput, DraftWorker), String> {
    let source = recovery::project_for_save(session, document_base)?;
    let path = recovery::absolute_document_path(&path)?;
    let writer = DraftWriter::create_new(path, &source, target, provenance)
        .map_err(|error| error.to_string())?;
    spawn_writer_on_control(writer)
}

/// Retain the actual configured playback clock for later completed-take queries.
/// The canonical caller must bind this exact prepared playback/output/source and
/// transport epoch before beginning input. These metadata are not an admission
/// receipt and must never be inferred from the saved loop marker after recording.
/// Native publication requires the playback and output rates to match.
pub fn start_with_playback_on_control(
    session: &Session,
    document_base: Option<&Path>,
    path: PathBuf,
    target: DraftTarget,
    provenance: DraftProvenance,
    playback: &hex_engine::playback::Playback,
) -> Result<(DraftInput, DraftWorker), String> {
    let source = recovery::project_for_save(session, document_base)?;
    let path = recovery::absolute_document_path(&path)?;
    let clock = hex_storage::automation_draft::CaptureClock {
        sample_rate: playback.sample_rate(),
        loop_region: playback.automation_capture_loop(),
    };
    let writer = DraftWriter::create_with_clock(path, &source, target, provenance, clock)
        .map_err(|error| error.to_string())?;
    spawn_writer_on_control(writer)
}

/// The writer is already source-validated and exclusively owned. All buffer allocation and
/// thread creation happen before a caller can reserve a receipt batch.
fn spawn_writer_on_control(writer: DraftWriter) -> Result<(DraftInput, DraftWorker), String> {
    spawn_with_start(writer, || {})
}
fn spawn_with_start(
    mut writer: DraftWriter,
    start: impl FnOnce() + Send + 'static,
) -> Result<(DraftInput, DraftWorker), String> {
    let zero = hex_project::AutomationValue::new(0.0).map_err(|error| error.to_string())?;
    let empty = AppliedEvent {
        sequence: 0,
        performance_frame: 0,
        content_frame: 0,
        loop_pass: 0,
        value: zero,
    };
    let free = std::array::from_fn(|slot| {
        Some(Box::new(Batch {
            slot,
            ticket: 0,
            events: [empty; EVENTS_PER_BATCH],
            len: 0,
        }))
    });
    let mut queue = VecDeque::new();
    queue
        .try_reserve_exact(BATCH_SLOTS)
        .map_err(|error| error.to_string())?;
    let mut acknowledgements = VecDeque::new();
    acknowledgements
        .try_reserve_exact(ACK_SLOTS)
        .map_err(|error| error.to_string())?;
    let mut retained = Vec::new();
    retained
        .try_reserve_exact(BATCH_SLOTS)
        .map_err(|error| error.to_string())?;
    let shared = Arc::new(Shared {
        path: writer.path().to_path_buf(),
        state: Mutex::new(State {
            queue,
            acknowledgements,
            terminal: None,
            accepted_events: 0,
            progress: writer.progress(),
            failed: false,
            finished: false,
            outcome: None,
            #[cfg(feature = "recording-fault-test-support")]
            fail_next_append: false,
        }),
        wake: Condvar::new(),
    });
    let worker_shared = Arc::clone(&shared);
    let thread = thread::Builder::new()
        .name("hex-automation-writer".into())
        .spawn(move || {
            start();
            let mut failure = None;
            let terminal = loop {
                let mut state = worker_shared.state();
                while state.queue.is_empty() && state.terminal.is_none() {
                    state = match worker_shared.wake.wait(state) {
                        Ok(state) => state,
                        Err(poison) => {
                            let mut state = poison.into_inner();
                            state.failed = true;
                            state.terminal = Some(DraftTerminal::Interrupted);
                            state
                        }
                    };
                }
                if state.failed && failure.is_none() {
                    failure =
                        Some("automation draft control state failed; retain the take".to_owned());
                }
                let Some(batch) = state.queue.pop_front() else {
                    break state.terminal.unwrap_or(DraftTerminal::Interrupted);
                };
                #[cfg(feature = "recording-fault-test-support")]
                let inject_failure = std::mem::take(&mut state.fail_next_append);
                drop(state);
                if failure.is_some() {
                    retained.push(RetainedBatch(batch));
                    continue;
                }
                #[cfg(feature = "recording-fault-test-support")]
                if inject_failure {
                    let message = "injected automation writer failure; retain the take".to_owned();
                    let progress = writer.progress();
                    let mut state = worker_shared.state();
                    state.failed = true;
                    state
                        .acknowledgements
                        .push_back(Ack::Failed(message.clone(), progress));
                    retained.push(RetainedBatch(batch));
                    failure = Some(message);
                    continue;
                }
                match writer.append(batch.events()) {
                    Ok(progress) => {
                        let mut state = worker_shared.state();
                        state.progress = progress;
                        state
                            .acknowledgements
                            .push_back(Ack::Durable(batch, progress));
                    }
                    Err(error) => {
                        let message = error.to_string();
                        let mut state = worker_shared.state();
                        state.progress = error.progress;
                        state.failed = true;
                        state
                            .acknowledgements
                            .push_back(Ack::Failed(message.clone(), error.progress));
                        retained.push(RetainedBatch(batch));
                        failure = Some(message);
                    }
                }
            };
            if failure.is_none()
                && let Err(error) = writer.finish(terminal)
            {
                failure = Some(error.to_string());
            }
            let progress = writer.progress();
            let durable_prefix = writer.durable_prefix_identity();
            let path = writer.path().to_path_buf();
            // Release the exclusive file lock before publishing a completed observation.
            drop(writer);
            let mut state = worker_shared.state();
            state.progress = progress;
            state.failed |= failure.is_some();
            state
                .acknowledgements
                .push_back(Ack::Terminal(progress, failure.clone()));
            state.finished = true;
            state.outcome = Some(DraftWorkerOutcome {
                path,
                admitted_events: state.accepted_events,
                progress,
                durable_prefix,
                requested_terminal: terminal,
                failure,
                retained,
            });
        })
        .map_err(|error| format!("could not start automation writer: {error}"))?;
    Ok((
        DraftInput {
            shared: Arc::clone(&shared),
            free,
            next_ticket: 0,
        },
        DraftWorker {
            shared,
            thread: Some(thread),
        },
    ))
}

/// A loan is reserved before reading receipts. Dropping an unsubmitted reservation explicitly
/// abandons its unadmitted contents and returns the buffer. A submit refusal owns its data.
pub struct ReservedBatch<'a> {
    input: &'a mut DraftInput,
    batch: Option<Box<Batch>>,
}
#[derive(Debug)]
pub enum DraftSubmitError {
    Refused(RefusedBatch),
    InvalidReservation,
}
impl std::fmt::Display for DraftSubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(batch) => f.write_str(batch.reason()),
            Self::InvalidReservation => {
                f.write_str("automation draft reservation is no longer active")
            }
        }
    }
}
impl std::error::Error for DraftSubmitError {}
pub struct RefusedBatch {
    origin: Arc<Shared>,
    batch: Box<Batch>,
    reason: &'static str,
}
impl std::fmt::Debug for RefusedBatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefusedBatch")
            .field("reason", &self.reason)
            .field("events", &self.batch.len)
            .finish_non_exhaustive()
    }
}
impl RefusedBatch {
    pub fn path(&self) -> &Path {
        &self.origin.path
    }
    pub fn reason(&self) -> &'static str {
        self.reason
    }
    pub fn events(&self) -> &[AppliedEvent] {
        self.batch.events()
    }
    /// Only the original lane may own a retry; source changes never restamp this batch.
    pub fn retry(self, input: &mut DraftInput) -> Result<u64, Self> {
        if !Arc::ptr_eq(&self.origin, &input.shared) {
            return Err(self);
        }
        input.submit(self.batch)
    }
}
impl DraftInput {
    #[cfg(feature = "recording-fault-test-support")]
    pub(in crate::service) fn inject_next_append_failure(&mut self) -> Result<(), &'static str> {
        let mut state = self.shared.state();
        if state.failed
            || state.terminal.is_some()
            || state.fail_next_append
            || !state.queue.is_empty()
        {
            return Err("automation writer cannot arm failure injection");
        }
        state.fail_next_append = true;
        Ok(())
    }
    pub fn path(&self) -> &Path {
        &self.shared.path
    }
    /// Once-only recovery custody, including after the joining owner was dropped.
    /// None means pending, already taken, or unavailable after an unexpected worker failure;
    /// inspect progress.finished/failed and preserve the file in that last case.
    /// This never waits or joins. The worker releases its file before publishing the outcome.
    pub fn try_take_outcome(&mut self) -> Option<DraftWorkerOutcome> {
        self.shared.state().outcome.take()
    }
    /// None is backpressure: leave native receipts unconsumed until a durable ACK returns a loan.
    pub fn reserve_batch(&mut self) -> Result<Option<ReservedBatch<'_>>, &'static str> {
        {
            let state = self.shared.state();
            if state.terminal.is_some() {
                return Err("automation draft is finishing");
            }
            if state.failed {
                return Err("automation draft writer failed; retain the take");
            }
        }
        let Some(slot) = self.free.iter().position(Option::is_some) else {
            return Ok(None);
        };
        let next = self
            .next_ticket
            .checked_add(1)
            .ok_or("automation draft batch identity exhausted")?;
        let Some(mut batch) = self.free[slot].take() else {
            return Ok(None);
        };
        self.next_ticket = next;
        batch.ticket = next;
        Ok(Some(ReservedBatch {
            input: self,
            batch: Some(batch),
        }))
    }
    fn submit(&mut self, batch: Box<Batch>) -> Result<u64, RefusedBatch> {
        let mut state = self.shared.state();
        let reason = if state.terminal.is_some() {
            Some("automation draft is finishing")
        } else if state.failed {
            Some("automation draft writer failed; retain the take")
        } else if state.queue.len() >= BATCH_SLOTS {
            Some("automation draft queue is full")
        } else if state
            .accepted_events
            .checked_add(batch.len as u64)
            .is_none()
        {
            Some("automation draft event count exhausted")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(RefusedBatch {
                origin: Arc::clone(&self.shared),
                batch,
                reason,
            });
        }
        let ticket = batch.ticket;
        state.accepted_events += batch.len as u64;
        state.queue.push_back(batch);
        self.shared.wake.notify_one();
        Ok(ticket)
    }
    pub fn finish(&mut self, terminal: DraftTerminal) -> Result<(), &'static str> {
        self.shared.finish(terminal)
    }
    pub fn progress(&self) -> DraftLaneProgress {
        let state = self.shared.state();
        DraftLaneProgress {
            accepted_events: state.accepted_events,
            durable_events: state.progress.durable_events,
            terminal_requested: state.terminal,
            terminal_durable: state.progress.durable_terminal,
            failed: state.failed,
            finished: state.finished,
        }
    }
    pub fn poll(&mut self) -> Option<DraftWriteAck> {
        let ack = self.shared.state().acknowledgements.pop_front()?;
        Some(match ack {
            Ack::Durable(mut batch, progress) => {
                let ticket = batch.ticket;
                batch.len = 0;
                let slot = batch.slot;
                self.free[slot] = Some(batch);
                DraftWriteAck::Durable { ticket, progress }
            }
            Ack::Failed(message, progress) => DraftWriteAck::Failed { message, progress },
            Ack::Terminal(progress, failure) => DraftWriteAck::Terminal { progress, failure },
        })
    }
}
impl ReservedBatch<'_> {
    pub fn remaining(&self) -> usize {
        self.batch
            .as_ref()
            .map_or(0, |batch| EVENTS_PER_BATCH - batch.len)
    }
    pub fn push(&mut self, event: AppliedEvent) -> Result<(), AppliedEvent> {
        let Some(batch) = self.batch.as_mut() else {
            return Err(event);
        };
        let Some(slot) = batch.events.get_mut(batch.len) else {
            return Err(event);
        };
        *slot = event;
        batch.len += 1;
        Ok(())
    }
    pub fn submit(mut self) -> Result<u64, DraftSubmitError> {
        // This private Option is taken exactly once by this consuming method.
        let batch = self.batch.take();
        match batch {
            Some(batch) => self.input.submit(batch).map_err(DraftSubmitError::Refused),
            None => Err(DraftSubmitError::InvalidReservation),
        }
    }
}
impl Drop for ReservedBatch<'_> {
    fn drop(&mut self) {
        if let Some(mut batch) = self.batch.take() {
            batch.len = 0;
            let slot = batch.slot;
            self.input.free[slot] = Some(batch);
        }
    }
}
impl Drop for DraftInput {
    fn drop(&mut self) {
        let _ = self.shared.finish(DraftTerminal::Interrupted);
    }
}
impl DraftWorker {
    pub fn request_interruption_on_control(&self) -> Result<(), &'static str> {
        self.shared.finish(DraftTerminal::Interrupted)
    }
    /// Called on joining Session control only. Existing terminal intent is preserved.
    pub fn join_on_control(mut self) -> Result<DraftWorkerOutcome, String> {
        self.join()?;
        self.shared
            .state()
            .outcome
            .take()
            .ok_or_else(|| "automation writer has no terminal result".into())
    }
    fn join(&mut self) -> Result<(), String> {
        let _ = self.shared.finish(DraftTerminal::Interrupted);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            let mut state = self.shared.state();
            state.failed = true;
            state.finished = true;
            return Err("automation writer failed; recover its retained file".to_owned());
        }
        Ok(())
    }
}
impl Drop for DraftWorker {
    fn drop(&mut self) {
        let _ = self.join();
    }
}

#[cfg(test)]
pub(super) mod tests;
