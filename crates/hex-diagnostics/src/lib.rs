//! # Purpose
//! Own bounded asynchronous per-session diagnostic logging and recent retention.
//!
//! # Dependencies
//! Standard library only. Desktop owns startup and shutdown; control producers
//! enqueue bounded records. Producers may wait for another producer's bounded,
//! allocation-free enqueue critical section, so they must never be called from
//! a realtime/audio thread.
//!
//! # Does not own
//! No audio/project authority, action replay, source admission or musical ACK.
//! Enqueue success is not durable delivery. Producer wall/monotonic times and
//! sequence precede writing; loss and truncation are explicit. Sequence orders
//! enqueue attempts, not independent actions on different threads. Recent files
//! roll at 8 MiB with four segments/session and ten retained sessions (320 MiB
//! for new-format files, plus a bounded rollover header per session); older
//! unbounded logs expire through normal retention.
//! Startup and shutdown use two-second writer waits. Shutdown closes admission,
//! observes the last enqueue, then drains at most 64 accepted records. Stalled
//! OS I/O may outlive a reported timeout
//! on its detached worker; it never blocks audio or owns project/media files.

mod event;
mod rolling;

use event::{Context, Record};
use std::{
    env,
    path::PathBuf,
    sync::{
        Arc, Mutex, TryLockError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const EVENT_CAPACITY: usize = 64;
pub const MAX_EVENT_BYTES: usize = 4096;
pub const RETAINED_SESSIONS: usize = 10;
pub const MAX_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
pub const RETAINED_SEGMENTS: usize = 4;
const RECEIVE_INTERVAL: Duration = Duration::from_millis(50);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

pub fn default_directory() -> Result<PathBuf, String> {
    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "home directory is unavailable".to_owned())?;
    let home = PathBuf::from(home);
    Ok(if cfg!(target_os = "macos") {
        home.join("Library").join("Logs").join("hex")
    } else {
        home.join(".local").join("state").join("hex").join("logs")
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishError {
    Full,
    Closed,
}

/// Cumulative observations, not a coherent transactional snapshot. Counts
/// include rejected attempts; a footer covers only activity before shutdown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogStatistics {
    pub last_sequence: u64,
    pub written: u64,
    pub queue_full: u64,
    pub closed: u64,
    pub truncated: u64,
    pub write_failed: u64,
    pub abandoned: u64,
}
struct Shared {
    started: Instant,
    started_unix: u128,
    closing: AtomicBool,
    failed: AtomicBool,
    written: AtomicU64,
    admission: Mutex<()>,
    sequence: AtomicU64,
    queue_full: AtomicU64,
    closed: AtomicU64,
    truncated: AtomicU64,
    write_failed: AtomicU64,
    abandoned: AtomicU64,
    last_error: Mutex<Option<String>>,
}
impl Shared {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            started_unix: unix_nanos(),
            closing: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            written: AtomicU64::new(0),
            admission: Mutex::new(()),
            sequence: AtomicU64::new(0),
            queue_full: AtomicU64::new(0),
            closed: AtomicU64::new(0),
            truncated: AtomicU64::new(0),
            write_failed: AtomicU64::new(0),
            abandoned: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }
    fn next_sequence(&self) -> Option<u64> {
        self.sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .ok()
            .and_then(|previous| previous.checked_add(1))
    }
    fn statistics(&self) -> LogStatistics {
        LogStatistics {
            last_sequence: self.sequence.load(Ordering::Relaxed),
            written: self.written.load(Ordering::Relaxed),
            queue_full: self.queue_full.load(Ordering::Relaxed),
            closed: self.closed.load(Ordering::Relaxed),
            truncated: self.truncated.load(Ordering::Relaxed),
            write_failed: self.write_failed.load(Ordering::Relaxed),
            abandoned: self.abandoned.load(Ordering::Relaxed),
        }
    }
    fn fail(&self, message: String) {
        self.closing.store(true, Ordering::Release);
        if let Ok(mut error) = self.last_error.try_lock() {
            *error = Some(event::bounded(&message).to_owned());
        }
        self.failed.store(true, Ordering::Release);
    }
}

/// Control-only producer: at most 4096 UTF-8 message bytes, a 64-record queue
/// and explicit rejection when it is full. Concurrent producers wait for the
/// enqueue gate instead of dropping records; its critical section performs no
/// allocation, formatting or I/O and never blocks on the queue. Never format or
/// call on a realtime/audio thread.
#[derive(Clone)]
pub struct DiagnosticsHandle {
    sender: SyncSender<Record>,
    shared: Arc<Shared>,
    path: Arc<PathBuf>,
}
impl DiagnosticsHandle {
    pub fn record(&self, message: &str) -> Result<(), PublishError> {
        if self.shared.closing.load(Ordering::Acquire) {
            self.shared.closed.fetch_add(1, Ordering::Relaxed);
            return Err(PublishError::Closed);
        }
        // Allocate before admission: neither another producer nor shutdown
        // waits on formatting/I/O while this producer holds the gate.
        let bounded = event::bounded(message).to_owned();
        // Waiting is bounded by another producer's short, allocation-free
        // section. Dropping instead lost lifecycle records whenever a holder
        // was preempted (#4204). The shutdown drain still only try-locks.
        let Ok(_admission) = self.shared.admission.lock() else {
            self.shared.fail("session log admission failed".into());
            return Err(PublishError::Closed);
        };
        if self.shared.closing.load(Ordering::Acquire) {
            self.shared.closed.fetch_add(1, Ordering::Relaxed);
            return Err(PublishError::Closed);
        }
        let Some(sequence) = self.shared.next_sequence() else {
            self.shared.fail("session log sequence exhausted".into());
            return Err(PublishError::Closed);
        };
        if bounded.len() < message.len() {
            self.shared.truncated.fetch_add(1, Ordering::Relaxed);
        }
        let record = Record {
            sequence,
            unix_nanos: unix_nanos(),
            elapsed_nanos: self.shared.started.elapsed().as_nanos(),
            original_bytes: message.len(),
            message: bounded,
        };
        match self.sender.try_send(record) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.shared.queue_full.fetch_add(1, Ordering::Relaxed);
                Err(PublishError::Full)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.closed.fetch_add(1, Ordering::Relaxed);
                self.shared.fail("session log worker is unavailable".into());
                Err(PublishError::Closed)
            }
        }
    }
    /// Stable active filename. Older recent segments use .1.log through .3.log.
    pub fn path(&self) -> PathBuf {
        self.path.as_ref().clone()
    }
    /// Backpressure losses from a full queue.
    pub fn overflow_count(&self) -> u64 {
        self.shared.queue_full.load(Ordering::Relaxed)
    }
    pub fn statistics(&self) -> LogStatistics {
        self.shared.statistics()
    }
    pub fn failure(&self) -> Option<String> {
        self.shared
            .last_error
            .try_lock()
            .ok()
            .and_then(|error| error.clone())
            .or_else(|| {
                self.shared
                    .failed
                    .load(Ordering::Acquire)
                    .then(|| "session log failure · details unavailable".into())
            })
    }
}

/// Desktop/control owner. Stop producers before shutdown. A failed/timed-out
/// shutdown reports incomplete logging; detached disk work owns only its files.
pub struct SessionLog {
    handle: DiagnosticsHandle,
    worker: Option<JoinHandle<()>>,
    completed: Receiver<()>,
}
impl SessionLog {
    pub fn start(directory: PathBuf) -> Result<Self, String> {
        Self::start_with_context(directory, "")
    }
    /// Immutable bounded build/configuration context is repeated on every
    /// segment, so a rolling capture retains provenance without the first file.
    pub fn start_with_context(directory: PathBuf, context: &str) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(EVENT_CAPACITY);
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let (done_sender, completed) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared::new());
        let worker_shared = Arc::clone(&shared);
        let context = Context::new(context);
        let worker = thread::Builder::new()
            .name("hex-diagnostics".into())
            .spawn(move || {
                run_worker(directory, context, receiver, startup_sender, worker_shared);
                let _ = done_sender.try_send(());
            })
            .map_err(|error| format!("could not start session log worker: {error}"))?;
        let (path, worker) = await_startup(startup_receiver, worker, &shared, SHUTDOWN_TIMEOUT)?;
        Ok(Self {
            handle: DiagnosticsHandle {
                sender,
                shared,
                path: Arc::new(path),
            },
            worker: Some(worker),
            completed,
        })
    }
    pub fn handle(&self) -> DiagnosticsHandle {
        self.handle.clone()
    }
    pub fn path(&self) -> PathBuf {
        self.handle.path()
    }
    pub fn shutdown(&mut self) -> Result<(), String> {
        self.shutdown_with_timeout(SHUTDOWN_TIMEOUT)
    }
    fn shutdown_with_timeout(&mut self, timeout: Duration) -> Result<(), String> {
        if let Some(worker) = self.worker.take() {
            let deadline = Instant::now() + timeout;
            self.handle.shared.closing.store(true, Ordering::Release);
            match self
                .completed
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                    if worker.join().is_err() {
                        self.handle
                            .shared
                            .fail("session log worker stopped unexpectedly".into());
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.handle.shared.fail(
                        "session log shutdown timed out · final records may be missing".into(),
                    );
                    drop(worker);
                    return Err(
                        "session log shutdown timed out · final records may be missing".into(),
                    );
                }
            }
        }
        self.handle.failure().map_or(Ok(()), Err)
    }
}
impl Drop for SessionLog {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// A slow filesystem must not hold the application composition thread forever.
/// On timeout, closing prevents the detached startup worker from pruning logs
/// or accepting records when it eventually makes progress.
fn await_startup(
    receiver: Receiver<Result<PathBuf, String>>,
    worker: JoinHandle<()>,
    shared: &Shared,
    timeout: Duration,
) -> Result<(PathBuf, JoinHandle<()>), String> {
    match receiver.recv_timeout(timeout) {
        Ok(Ok(path)) => Ok((path, worker)),
        Ok(Err(reason)) => {
            let _ = worker.join();
            Err(format!("could not start session log: {reason}"))
        }
        Err(RecvTimeoutError::Disconnected) => {
            let _ = worker.join();
            Err("session log worker exited before creating the session file".into())
        }
        Err(RecvTimeoutError::Timeout) => {
            shared.fail("session log startup timed out · logging is unavailable".into());
            drop(worker);
            Err("session log startup timed out · logging is unavailable".into())
        }
    }
}

trait LogWriter {
    fn line(&mut self, value: &str) -> std::io::Result<()>;
}
fn run_worker(
    directory: PathBuf,
    context: Context,
    receiver: Receiver<Record>,
    startup: SyncSender<Result<PathBuf, String>>,
    shared: Arc<Shared>,
) {
    if shared.closing.load(Ordering::Acquire) {
        return;
    }
    let mut writer = match rolling::SessionWriter::create(&directory, context, &shared) {
        Ok(writer) => writer,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return;
        }
    };
    if shared.closing.load(Ordering::Acquire) {
        return;
    }
    if let Err(error) = rolling::prune(&directory, writer.path()) {
        shared.fail(format!("session log retention failed: {error}"));
    }
    if startup.send(Ok(writer.path().to_owned())).is_err() {
        return;
    }
    run_records(&mut writer, &receiver, &shared);
}
fn run_records(writer: &mut impl LogWriter, receiver: &Receiver<Record>, shared: &Shared) {
    let mut reported = LogStatistics::default();
    loop {
        // A producer may have passed the closing check while holding the
        // enqueue gate. Only the worker's observation of that gate released
        // authorizes the final bounded drain; new producers recheck closing.
        let closed = if shared.closing.load(Ordering::Acquire) {
            match shared.admission.try_lock() {
                Ok(_gate) => true,
                Err(TryLockError::WouldBlock) => false,
                Err(TryLockError::Poisoned(_)) => {
                    shared.fail("session log admission failed during drain".into());
                    true
                }
            }
        } else {
            false
        };
        if closed {
            for _ in 0..EVENT_CAPACITY {
                match receiver.try_recv() {
                    Ok(record) => {
                        if !write_record(writer, &record, shared) {
                            abandon(receiver, shared);
                            return;
                        }
                    }
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            let _ = write_statistics(writer, shared, "session_finished", &mut reported, true);
            return;
        }
        match receiver.recv_timeout(RECEIVE_INTERVAL) {
            Ok(record) => {
                if !write_record(writer, &record, shared) {
                    abandon(receiver, shared);
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = write_statistics(writer, shared, "session_finished", &mut reported, true);
                return;
            }
        }
        if !write_statistics(writer, shared, "diagnostics_loss", &mut reported, false) {
            abandon(receiver, shared);
            return;
        }
    }
}
fn write_record(writer: &mut impl LogWriter, record: &Record, shared: &Shared) -> bool {
    if let Err(error) = writer.line(&record.line()) {
        shared.write_failed.fetch_add(1, Ordering::Relaxed);
        shared.fail(format!("session log write failed: {error}"));
        false
    } else {
        shared.written.fetch_add(1, Ordering::Relaxed);
        true
    }
}
fn write_statistics(
    writer: &mut impl LogWriter,
    shared: &Shared,
    stage: &str,
    reported: &mut LogStatistics,
    force: bool,
) -> bool {
    let stats = shared.statistics();
    let changed = stats.queue_full != reported.queue_full
        || stats.truncated != reported.truncated
        || stats.closed != reported.closed;
    if !force && !changed {
        return true;
    }
    let mut line = format!(
        "{stage} writer_unix_nanos={} elapsed_nanos={} failed={} statistics={stats:?} failure=",
        unix_nanos(),
        shared.started.elapsed().as_nanos(),
        shared.failed.load(Ordering::Acquire)
    );
    let failure = shared
        .last_error
        .try_lock()
        .ok()
        .and_then(|error| error.clone());
    let detail = failure
        .as_deref()
        .unwrap_or(if shared.failed.load(Ordering::Acquire) {
            "details unavailable"
        } else {
            "none"
        });
    event::quoted(&mut line, detail);
    line.push('\n');
    match writer.line(&line) {
        Ok(()) => {
            *reported = stats;
            true
        }
        Err(error) => {
            shared.write_failed.fetch_add(1, Ordering::Relaxed);
            shared.fail(format!("session log status write failed: {error}"));
            false
        }
    }
}
fn abandon(receiver: &Receiver<Record>, shared: &Shared) {
    for _ in 0..EVENT_CAPACITY {
        if receiver.try_recv().is_err() {
            break;
        }
        shared.abandoned.fetch_add(1, Ordering::Relaxed);
    }
}
fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos()
}

#[cfg(test)]
mod tests;
