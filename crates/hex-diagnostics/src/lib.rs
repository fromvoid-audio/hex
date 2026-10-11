//! Bounded, asynchronous per-session diagnostic logging.

use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const EVENT_CAPACITY: usize = 64;
pub const MAX_EVENT_BYTES: usize = 4096;
pub const RETAINED_SESSIONS: usize = 10;

const RECEIVE_INTERVAL: Duration = Duration::from_millis(50);
const SESSION_NAME_ATTEMPTS: usize = 64;

/// Resolve Hex's per-user diagnostic directory without touching the filesystem.
pub fn default_directory() -> Result<PathBuf, String> {
    let home = env::var_os("HOME").filter(|value| !value.is_empty());
    let Some(home) = home else {
        return Err("home directory is unavailable".to_owned());
    };
    let home = PathBuf::from(home);
    if cfg!(target_os = "macos") {
        Ok(home.join("Library").join("Logs").join("hex"))
    } else {
        Ok(home.join(".local").join("state").join("hex").join("logs"))
    }
}

/// The result of submitting one event to the bounded worker queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishError {
    /// The worker queue already contains its maximum number of events.
    Full,
    /// The session owner is shutting down or the worker has exited.
    Closed,
}

struct Shared {
    closing: AtomicBool,
    overflow_count: AtomicU64,
    last_error: Mutex<Option<String>>,
}

/// Cloneable producer handle. Publishing truncates at a UTF-8 boundary and
/// never waits for filesystem work or queue capacity.
#[derive(Clone)]
pub struct DiagnosticsHandle {
    sender: SyncSender<String>,
    shared: Arc<Shared>,
    path: Arc<PathBuf>,
}

impl DiagnosticsHandle {
    /// Queue one diagnostic event. The worker escapes line breaks before writing.
    pub fn record(&self, message: &str) -> Result<(), PublishError> {
        if self.shared.closing.load(Ordering::Acquire) {
            return Err(PublishError::Closed);
        }
        let mut end = message.len().min(MAX_EVENT_BYTES);
        while !message.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        let bounded = message[..end].to_owned();
        match self.sender.try_send(bounded) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.shared.overflow_count.fetch_add(1, Ordering::Relaxed);
                Err(PublishError::Full)
            }
            Err(TrySendError::Disconnected(_)) => Err(PublishError::Closed),
        }
    }

    /// Path for the current session file, known after `SessionLog::start` returns.
    pub fn path(&self) -> PathBuf {
        self.path.as_ref().clone()
    }

    /// Number of producer submissions rejected because the queue was full.
    pub fn overflow_count(&self) -> u64 {
        self.shared.overflow_count.load(Ordering::Relaxed)
    }

    /// Return the latest worker error when the snapshot lock is immediately
    /// available. `None` means there is no error or the worker is updating it.
    pub fn failure(&self) -> Option<String> {
        self.shared.last_error.try_lock().ok()?.clone()
    }
}

/// Session owner. Keep this on the application composition side and shut it
/// down after UI/control producers have stopped; joining can wait for disk I/O.
pub struct SessionLog {
    handle: DiagnosticsHandle,
    worker: Option<JoinHandle<()>>,
}

impl SessionLog {
    /// Start a worker, create a fresh log, and prune older Hex session logs.
    /// All directory and file operations run on the worker thread.
    pub fn start(directory: PathBuf) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(EVENT_CAPACITY);
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            closing: AtomicBool::new(false),
            overflow_count: AtomicU64::new(0),
            last_error: Mutex::new(None),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("hex-diagnostics".to_owned())
            .spawn(move || run_worker(directory, receiver, startup_sender, worker_shared))
            .map_err(|error| format!("could not start session log worker: {error}"))?;

        let path = match startup_receiver.recv() {
            Ok(Ok(path)) => path,
            Ok(Err(reason)) => {
                let _ = worker.join();
                return Err(format!("could not start session log: {reason}"));
            }
            Err(_) => {
                let _ = worker.join();
                return Err("session log worker exited before creating the session file".to_owned());
            }
        };
        let handle = DiagnosticsHandle {
            sender,
            shared,
            path: Arc::new(path),
        };
        Ok(Self {
            handle,
            worker: Some(worker),
        })
    }

    pub fn handle(&self) -> DiagnosticsHandle {
        self.handle.clone()
    }

    pub fn path(&self) -> PathBuf {
        self.handle.path()
    }

    /// Close the worker and wait for its bounded drain and final flush.
    pub fn shutdown(&mut self) -> Result<(), String> {
        self.handle.shared.closing.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| "session log worker stopped unexpectedly".to_owned())?;
        }
        match self.handle.failure() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for SessionLog {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn run_worker(
    directory: PathBuf,
    receiver: Receiver<String>,
    startup: SyncSender<Result<PathBuf, String>>,
    shared: Arc<Shared>,
) {
    let (path, mut writer) = match create_session(&directory) {
        Ok(session) => session,
        Err(error) => {
            let _ = startup.send(Err(error.to_string()));
            return;
        }
    };
    let retention_error = prune_old_sessions(&directory, &path).err();
    if let Err(error) = write_header(&mut writer) {
        let _ = startup.send(Err(error.to_string()));
        return;
    }
    if let Some(error) = retention_error {
        set_error(&shared, format!("session log retention failed: {error}"));
    }
    if startup.send(Ok(path)).is_err() {
        return;
    }

    let mut reported_overflow = 0_u64;
    loop {
        if shared.closing.load(Ordering::Acquire) {
            drain_bounded(&receiver, &mut writer, &shared, &mut reported_overflow);
            write_overflow_delta(&mut writer, &shared, &mut reported_overflow);
            break;
        }
        match receiver.recv_timeout(RECEIVE_INTERVAL) {
            Ok(message) => write_event(&mut writer, &message, &shared),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        write_overflow_delta(&mut writer, &shared, &mut reported_overflow);
    }
}

fn create_session(directory: &Path) -> io::Result<(PathBuf, BufWriter<File>)> {
    fs::create_dir_all(directory)?;
    let mut timestamp = unix_nanos();
    for _ in 0..SESSION_NAME_ATTEMPTS {
        let path = directory.join(format!("session-{timestamp}-{}.log", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, BufWriter::new(file))),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                timestamp = timestamp.saturating_add(1);
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a unique session log name",
    ))
}

fn write_header(writer: &mut BufWriter<File>) -> io::Result<()> {
    writeln!(
        writer,
        "session_started unix_nanos={} pid={} hex_version={}",
        unix_nanos(),
        std::process::id(),
        env!("CARGO_PKG_VERSION")
    )?;
    writer.flush()
}

fn write_event(writer: &mut impl Write, message: &str, shared: &Shared) {
    let result = (|| -> io::Result<()> {
        write!(writer, "event unix_nanos={} ", unix_nanos())?;
        for byte in message.bytes() {
            match byte {
                b'\n' => writer.write_all(b"\\n")?,
                b'\r' => writer.write_all(b"\\r")?,
                _ => writer.write_all(&[byte])?,
            }
        }
        writer.write_all(b"\n")?;
        writer.flush()
    })();
    if let Err(error) = result {
        set_error(shared, format!("session log write failed: {error}"));
    }
}

fn write_overflow_delta(writer: &mut impl Write, shared: &Shared, reported: &mut u64) {
    let total = shared.overflow_count.load(Ordering::Relaxed);
    if total <= *reported {
        return;
    }
    let result =
        writeln!(writer, "diagnostics_overflow total={total}").and_then(|()| writer.flush());
    if let Err(error) = result {
        set_error(
            shared,
            format!("session log overflow write failed: {error}"),
        );
    }
    *reported = total;
}

fn drain_bounded(
    receiver: &Receiver<String>,
    writer: &mut impl Write,
    shared: &Shared,
    reported_overflow: &mut u64,
) {
    for _ in 0..EVENT_CAPACITY {
        match receiver.try_recv() {
            Ok(message) => write_event(writer, &message, shared),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
    write_overflow_delta(writer, shared, reported_overflow);
}

fn prune_old_sessions(directory: &Path, current: &Path) -> io::Result<()> {
    let mut sessions = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(timestamp) = session_timestamp(&entry.file_name()) else {
            continue;
        };
        sessions.push((timestamp, entry.path()));
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.0));

    let mut first_error = None;
    let current_index = sessions.iter().position(|(_, path)| path == current);
    let retained_noncurrent = if current_index.is_some_and(|index| index >= RETAINED_SESSIONS) {
        RETAINED_SESSIONS.saturating_sub(1)
    } else {
        RETAINED_SESSIONS
    };
    for (index, (_, path)) in sessions.into_iter().enumerate() {
        if path == current || index < retained_noncurrent {
            continue;
        }
        if let Err(error) = fs::remove_file(path) {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn session_timestamp(name: &std::ffi::OsStr) -> Option<u128> {
    let name = name.to_str()?;
    let body = name.strip_prefix("session-")?.strip_suffix(".log")?;
    let (timestamp, pid) = body.split_once('-')?;
    if timestamp.is_empty()
        || !timestamp.bytes().all(|byte| byte.is_ascii_digit())
        || pid.is_empty()
        || !pid.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    timestamp.parse().ok()
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos()
}

fn set_error(shared: &Shared, message: String) {
    if let Ok(mut error) = shared.last_error.lock() {
        *error = Some(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> PathBuf {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path =
            env::temp_dir().join(format!("hex-diagnostics-test-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("fixture directory");
        path
    }

    fn session_files(directory: &Path) -> Vec<PathBuf> {
        fs::read_dir(directory)
            .expect("list fixture")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                fs::symlink_metadata(path)
                    .map(|metadata| metadata.file_type().is_file())
                    .unwrap_or(false)
                    && path
                        .file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .and_then(|name| session_timestamp(std::ffi::OsStr::new(name)))
                        .is_some()
            })
            .collect()
    }

    #[test]
    fn creates_a_fresh_file_for_each_session_and_escapes_bounded_utf8_events() {
        let directory = fixture();
        let first_path;
        {
            let mut log = SessionLog::start(directory.clone()).expect("start first session");
            first_path = log.path();
            let handle = log.handle();
            let oversized = "€".repeat(MAX_EVENT_BYTES);
            handle.record(&oversized).expect("queue bounded utf8");
            handle
                .record("first\nsecond\rthird")
                .expect("queue multiline");
            log.shutdown().expect("shutdown first session");
            assert_eq!(handle.record("after close"), Err(PublishError::Closed));
        }
        let contents = fs::read_to_string(&first_path).expect("read first session");
        let bounded_line = contents
            .lines()
            .find(|line| line.starts_with("event unix_nanos="))
            .expect("bounded event line");
        let (timestamp, bounded_message) = bounded_line
            .strip_prefix("event unix_nanos=")
            .and_then(|line| line.split_once(' '))
            .expect("event timestamp and message");
        assert!(timestamp.parse::<u128>().is_ok());
        assert_eq!(bounded_message.len(), MAX_EVENT_BYTES - 1);
        assert!(contents.lines().any(|line| {
            line.strip_prefix("event unix_nanos=")
                .and_then(|line| line.split_once(' '))
                .is_some_and(|(_, message)| message == "first\\nsecond\\rthird")
        }));

        let second_path;
        {
            let mut log = SessionLog::start(directory.clone()).expect("start second session");
            second_path = log.path();
            log.shutdown().expect("shutdown second session");
        }
        assert_ne!(first_path, second_path);
        assert_eq!(session_files(&directory).len(), 2);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn retains_ten_owned_regular_logs_and_preserves_unrelated_files_and_symlinks() {
        let directory = fixture();
        for timestamp in 1..=11 {
            fs::write(
                directory.join(format!("session-{timestamp}-7.log")),
                b"old session",
            )
            .expect("seed old session");
        }
        let unrelated = directory.join("session-not-ours.log");
        fs::write(&unrelated, b"preserve").expect("seed unrelated file");
        let link = directory.join("session-500-7.log");
        #[cfg(unix)]
        std::os::unix::fs::symlink(directory.join("session-1-7.log"), &link).expect("seed symlink");

        let mut log = SessionLog::start(directory.clone()).expect("start logger");
        log.shutdown().expect("shutdown logger");

        assert_eq!(session_files(&directory).len(), RETAINED_SESSIONS);
        assert!(unrelated.exists());
        #[cfg(unix)]
        assert!(
            fs::symlink_metadata(&link)
                .expect("inspect preserved symlink")
                .file_type()
                .is_symlink()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn reports_startup_failures() {
        let parent = fixture();
        let file = parent.join("not-a-directory");
        fs::write(&file, b"occupied").expect("create blocking file");
        assert!(SessionLog::start(file.join("logs")).is_err());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn full_queue_is_counted_without_a_consumer_and_overflow_line_is_written() {
        let (sender, receiver) = mpsc::sync_channel(EVENT_CAPACITY);
        let shared = Arc::new(Shared {
            closing: AtomicBool::new(false),
            overflow_count: AtomicU64::new(0),
            last_error: Mutex::new(None),
        });
        let handle = DiagnosticsHandle {
            sender,
            shared: Arc::clone(&shared),
            path: Arc::new(PathBuf::new()),
        };
        for _ in 0..EVENT_CAPACITY {
            assert_eq!(handle.record("queued"), Ok(()));
        }
        assert_eq!(handle.record("overflow"), Err(PublishError::Full));
        assert_eq!(handle.overflow_count(), 1);

        let mut output = Vec::new();
        let mut reported = 0;
        write_overflow_delta(&mut output, &shared, &mut reported);
        assert_eq!(output, b"diagnostics_overflow total=1\n");
        assert_eq!(reported, 1);
        drop(receiver);
    }

    #[test]
    fn event_write_failures_are_exposed_to_the_session_owner() {
        struct FailingWriter;

        impl Write for FailingWriter {
            fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("fixture write failure"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let shared = Arc::new(Shared {
            closing: AtomicBool::new(false),
            overflow_count: AtomicU64::new(0),
            last_error: Mutex::new(None),
        });
        write_event(&mut FailingWriter, "reproduction event", &shared);
        assert!(
            shared
                .last_error
                .lock()
                .expect("read worker failure")
                .as_deref()
                .is_some_and(|message| message.contains("fixture write failure"))
        );

        let (sender, receiver) = mpsc::sync_channel(EVENT_CAPACITY);
        drop(receiver);
        let handle = DiagnosticsHandle {
            sender,
            shared,
            path: Arc::new(PathBuf::new()),
        };
        let mut owner = SessionLog {
            handle,
            worker: Some(thread::spawn(|| {})),
        };
        assert!(owner.shutdown().is_err());
    }
}
