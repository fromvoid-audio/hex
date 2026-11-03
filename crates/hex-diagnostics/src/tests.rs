//! Observable chronology, bounded loss/storage and shutdown/failure regressions.
use super::*;
use std::{fs, io, sync::atomic::AtomicU64};

static NEXT: AtomicU64 = AtomicU64::new(0);
pub(super) fn fixture() -> PathBuf {
    let path = env::temp_dir().join(format!(
        "hex-diagnostics-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}
fn pair() -> (DiagnosticsHandle, Receiver<Record>) {
    let (sender, receiver) = mpsc::sync_channel(EVENT_CAPACITY);
    (
        DiagnosticsHandle {
            sender,
            shared: Arc::new(Shared::new()),
            path: Arc::new(PathBuf::new()),
        },
        receiver,
    )
}
#[derive(Clone)]
struct MemoryWriter(Arc<Mutex<String>>);
impl LogWriter for MemoryWriter {
    fn line(&mut self, value: &str) -> io::Result<()> {
        self.0.lock().unwrap().push_str(value);
        Ok(())
    }
}
fn memory_worker(
    handle: &DiagnosticsHandle,
    receiver: Receiver<Record>,
    mut writer: impl LogWriter + Send + 'static,
) -> SessionLog {
    let shared = Arc::clone(&handle.shared);
    let (done_sender, completed) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        run_records(&mut writer, &receiver, &shared);
        let _ = done_sender.send(());
    });
    SessionLog {
        handle: handle.clone(),
        worker: Some(worker),
        completed,
    }
}
fn field(line: &str, name: &str) -> u128 {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(name))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn delayed_writer_preserves_producer_time_sequence_and_reports_lost_attempts() {
    let (handle, receiver) = pair();
    handle.record("earlier").unwrap();
    let latest_producer_wall = unix_nanos();
    let latest_producer_elapsed = handle.shared.started.elapsed().as_nanos();
    // Start the actual record drain only after the producer observation above.
    let output = Arc::new(Mutex::new(String::new()));
    let mut log = memory_worker(&handle, receiver, MemoryWriter(Arc::clone(&output)));
    log.shutdown().unwrap();
    let text = output.lock().unwrap();
    let event = text
        .lines()
        .find(|line| line.starts_with("event "))
        .unwrap();
    assert!(field(event, "unix_nanos=") <= latest_producer_wall);
    assert!(field(event, "elapsed_nanos=") <= latest_producer_elapsed);
    assert_eq!(field(event, "sequence="), 1);
    assert!(event.ends_with("message=\"earlier\""));
    assert!(text.contains("session_finished"));
    assert_eq!(handle.statistics().written, 1);
}

#[test]
fn queue_pressure_has_exact_sequence_gap_and_recovers_without_capacity_wait() {
    let (handle, receiver) = pair();
    for _ in 0..EVENT_CAPACITY {
        handle.record("queued").unwrap();
    }
    assert_eq!(handle.record("lost"), Err(PublishError::Full));
    assert_eq!(handle.overflow_count(), 1);
    assert_eq!(receiver.recv().unwrap().sequence, 1);
    handle.record("recovered").unwrap();
    let records: Vec<_> = receiver.try_iter().collect();
    assert_eq!(records.last().unwrap().sequence, EVENT_CAPACITY as u64 + 2);
    assert_eq!(records.last().unwrap().message, "recovered");
    assert_eq!(handle.statistics().queue_full, 1);
    let output = Arc::new(Mutex::new(String::new()));
    let mut writer = MemoryWriter(Arc::clone(&output));
    assert!(write_statistics(
        &mut writer,
        &handle.shared,
        "diagnostics_loss",
        &mut LogStatistics::default(),
        false
    ));
    assert!(output.lock().unwrap().contains("queue_full: 1"));
}

#[test]
fn contended_producer_waits_for_the_gate_and_loses_nothing() {
    let (handle, receiver) = pair();
    let gate = handle.shared.admission.lock().unwrap();
    let contended = {
        let handle = handle.clone();
        thread::spawn(move || handle.record("contended"))
    };
    // A held gate admits nothing; the contended producer is still pending.
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    drop(gate);
    assert_eq!(contended.join().unwrap(), Ok(()));
    handle.record("after contention").unwrap();
    let first = receiver.recv().unwrap();
    assert_eq!((first.sequence, first.message.as_str()), (1, "contended"));
    assert_eq!(receiver.recv().unwrap().sequence, 2);
    assert_eq!(handle.overflow_count(), 0);
}

#[test]
fn fresh_sessions_preserve_bounded_utf8_truncation_and_unambiguous_escaping() {
    let directory = fixture();
    let mut log =
        SessionLog::start_with_context(directory.clone(), "build=exact flags=actions").unwrap();
    let path = log.path();
    let handle = log.handle();
    let oversized = "€".repeat(MAX_EVENT_BYTES);
    handle.record(&oversized).unwrap();
    handle
        .record("first\nsecond\rthird\\n\t\u{1b}[31m\"quoted\"")
        .unwrap();
    log.shutdown().unwrap();
    let contents = fs::read_to_string(&path).unwrap();
    let bounded = contents
        .lines()
        .find(|line| line.starts_with("event "))
        .unwrap();
    assert!(bounded.contains("truncated=true"));
    assert_eq!(field(bounded, "original_bytes="), oversized.len() as u128);
    assert_eq!(bounded.matches('€').count(), (MAX_EVENT_BYTES - 1) / 3);
    assert!(
        contents.contains("message=\"first\\nsecond\\rthird\\\\n\\t\\u001b[31m\\\"quoted\\\"\"")
    );
    assert_eq!(contents.matches('\u{1b}').count(), 0);
    assert_eq!(handle.statistics().truncated, 1);
    assert_eq!(handle.record("closed"), Err(PublishError::Closed));
    let mut second = SessionLog::start(directory.clone()).unwrap();
    assert_ne!(path, second.path());
    second.shutdown().unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn startup_failure_reports_the_filesystem_error() {
    let directory = fixture();
    let file = directory.join("not-a-directory");
    fs::write(&file, "occupied").unwrap();
    assert!(SessionLog::start(file.join("logs")).is_err());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn closing_waits_for_previously_admitted_producer_before_final_drain() {
    let (handle, receiver) = pair();
    let output = Arc::new(Mutex::new(String::new()));
    let mut log = memory_worker(&handle, receiver, MemoryWriter(Arc::clone(&output)));
    let (held, held_rx) = mpsc::sync_channel(1);
    let (resume, resume_rx) = mpsc::sync_channel(1);
    let producer = handle.clone();
    let sending = thread::spawn(move || {
        // Deliberately freeze the real record admission window between its
        // closing check and try_send. No production test hook or timing guess.
        let _gate = producer.shared.admission.lock().unwrap();
        assert!(!producer.shared.closing.load(Ordering::Acquire));
        held.send(()).unwrap();
        resume_rx.recv().unwrap();
        producer
            .sender
            .try_send(Record {
                sequence: 1,
                unix_nanos: 123,
                elapsed_nanos: 456,
                original_bytes: 8,
                message: "admitted".into(),
            })
            .unwrap();
    });
    held_rx.recv().unwrap();
    let stopping = thread::spawn(move || log.shutdown());
    while !handle.shared.closing.load(Ordering::Acquire) {
        thread::yield_now();
    }
    resume.send(()).unwrap();
    sending.join().unwrap();
    stopping.join().unwrap().unwrap();
    let contents = output.lock().unwrap();
    assert!(contents.contains("message=\"admitted\""));
    assert!(contents.find("admitted").unwrap() < contents.find("session_finished").unwrap());
    assert_eq!(handle.record("later"), Err(PublishError::Closed));
}

struct BlockedWriter {
    entered: SyncSender<()>,
    resume: Receiver<()>,
    output: MemoryWriter,
    blocked: bool,
}
impl LogWriter for BlockedWriter {
    fn line(&mut self, value: &str) -> io::Result<()> {
        if !self.blocked {
            self.blocked = true;
            self.entered.send(()).unwrap();
            self.resume.recv().unwrap();
        }
        self.output.line(value)
    }
}
#[test]
fn stalled_disk_writer_does_not_hold_shutdown_forever_and_reports_incompleteness() {
    let (handle, receiver) = pair();
    let output = Arc::new(Mutex::new(String::new()));
    let (entered, entered_rx) = mpsc::sync_channel(1);
    let (resume, resume_rx) = mpsc::sync_channel(1);
    let mut log = memory_worker(
        &handle,
        receiver,
        BlockedWriter {
            entered,
            resume: resume_rx,
            output: MemoryWriter(Arc::clone(&output)),
            blocked: false,
        },
    );
    handle.record("queued before stall").unwrap();
    entered_rx.recv().unwrap();
    let result = log.shutdown_with_timeout(Duration::from_millis(20));
    assert!(result.unwrap_err().contains("timed out"));
    assert!(
        handle
            .failure()
            .unwrap()
            .contains("final records may be missing")
    );
    assert_eq!(handle.record("after shutdown"), Err(PublishError::Closed));
    resume.send(()).unwrap();
    log.completed.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(output.lock().unwrap().contains("failed=true"));
}

#[test]
fn failed_writer_stops_admission_and_exposes_unwritten_queue_records() {
    struct FailingWriter;
    impl LogWriter for FailingWriter {
        fn line(&mut self, _: &str) -> io::Result<()> {
            Err(io::Error::other("fixture write failure"))
        }
    }
    let (handle, receiver) = pair();
    handle.record("first").unwrap();
    handle.record("second").unwrap();
    let mut log = memory_worker(&handle, receiver, FailingWriter);
    log.completed.recv_timeout(Duration::from_secs(2)).unwrap();
    // Restore a completion observation after inspecting it above.
    let (sender, completed) = mpsc::sync_channel(1);
    sender.send(()).unwrap();
    log.completed = completed;
    assert!(
        log.shutdown()
            .unwrap_err()
            .contains("fixture write failure")
    );
    assert_eq!(handle.statistics().write_failed, 1);
    assert_eq!(handle.statistics().abandoned, 1);
    assert_eq!(handle.statistics().written, 0);
    assert_eq!(handle.record("after failure"), Err(PublishError::Closed));
}

#[test]
fn sequence_exhaustion_refuses_without_reusing_an_event_identity() {
    let (handle, receiver) = pair();
    handle
        .shared
        .sequence
        .store(u64::MAX - 1, Ordering::Relaxed);
    handle.record("last").unwrap();
    assert_eq!(receiver.recv().unwrap().sequence, u64::MAX);
    assert_eq!(handle.record("cannot wrap"), Err(PublishError::Closed));
    assert!(handle.failure().unwrap().contains("sequence exhausted"));
    assert_eq!(handle.statistics().last_sequence, u64::MAX);
}

#[test]
fn concurrent_producers_within_capacity_lose_no_records() {
    let (handle, receiver) = pair();
    let output = Arc::new(Mutex::new(String::new()));
    let mut log = memory_worker(&handle, receiver, MemoryWriter(Arc::clone(&output)));
    let start = Arc::new(std::sync::Barrier::new(5));
    let mut producers = Vec::new();
    for producer in 0..4 {
        let handle = handle.clone();
        let start = Arc::clone(&start);
        producers.push(thread::spawn(move || {
            start.wait();
            // 4 x 16 fills at most the 64-record queue, so every record must land.
            for step in 0..16 {
                handle
                    .record(&format!("producer={producer} step={step}"))
                    .unwrap();
            }
        }));
    }
    start.wait();
    for producer in producers {
        producer.join().unwrap();
    }
    log.shutdown().unwrap();
    let contents = output.lock().unwrap();
    let sequences: Vec<_> = contents
        .lines()
        .filter(|line| line.starts_with("event "))
        .map(|line| field(line, "sequence="))
        .collect();
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    let stats = handle.statistics();
    assert_eq!(stats.last_sequence, 64);
    assert_eq!(sequences.len(), 64);
    assert_eq!((stats.written, stats.queue_full, stats.closed), (64, 0, 0));
    assert!(contents.contains("session_finished"));
}

#[test]
fn stalled_startup_returns_failure_and_closes_admission_without_waiting_for_disk() {
    let shared = Arc::new(Shared::new());
    let (startup, receiver) = mpsc::sync_channel(1);
    let (resume, resume_rx) = mpsc::sync_channel(1);
    let (ended, ended_rx) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        resume_rx.recv().unwrap();
        let _ = startup.send(Ok(PathBuf::from("late-unused.log")));
        ended.send(()).unwrap();
    });
    let result = await_startup(receiver, worker, &shared, Duration::from_millis(20));
    assert!(result.unwrap_err().contains("startup timed out"));
    assert!(shared.closing.load(Ordering::Acquire));
    assert!(shared.failed.load(Ordering::Acquire));
    resume.send(()).unwrap();
    ended_rx.recv_timeout(Duration::from_secs(2)).unwrap();
}
