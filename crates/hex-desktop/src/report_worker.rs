//! Desktop-owned background worker for closed-loop issue reporting.
//!
//! Runs strictly off the GPUI render thread and never touches the audio
//! callback thread. Slices diagnostic logs at frozen `T_report`, redacts
//! payloads, persists atomically to the offline outbox, flushes pending items
//! on startup, and publishes non-blocking status back to `hex-ui`.

use hex_diagnostics::report::{ReportSliceConfig, build_report_bundle};
use hex_report::{
    CurlTransport, DEFAULT_PROJECT_ID, IntakeConfig, RedactionContext, ReportEnvelope,
    ReportOutbox, ReportTransport, default_outbox_dir, format_redacted_preview, redact_envelope,
};
use hex_ui::{ReportCommand, ReportPreview, ReportStatus, ReportWorker};
use std::{
    io,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const POLL_SLICE: Duration = Duration::from_millis(50);
const DEFAULT_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

struct ExitSignal(Arc<(Mutex<bool>, Condvar)>);

impl Drop for ExitSignal {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.0;
        let mut finished = lock.lock().unwrap_or_else(|poison| poison.into_inner());
        *finished = true;
        condvar.notify_all();
    }
}

/// Owned handle for the desktop issue-report background worker.
pub(super) struct ReportWorkerHandle {
    stop: Arc<AtomicBool>,
    done: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
    join_timeout: Duration,
}

impl ReportWorkerHandle {
    pub(super) fn shutdown(&mut self) -> bool {
        let Some(thread) = self.thread.take() else {
            return true;
        };
        self.stop.store(true, Ordering::Release);
        let (lock, condvar) = &*self.done;
        let deadline = Instant::now() + self.join_timeout;
        let mut finished = lock.lock().unwrap_or_else(|poison| poison.into_inner());
        while !*finished {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            if remaining.is_zero() {
                break;
            }
            let (guard, result) = condvar
                .wait_timeout(finished, remaining)
                .unwrap_or_else(|poison| poison.into_inner());
            finished = guard;
            if result.timed_out() {
                break;
            }
        }
        let exited = *finished;
        drop(finished);
        if exited {
            let _ = thread.join();
            true
        } else {
            false
        }
    }
}

impl Drop for ReportWorkerHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Start the default desktop-owned report worker.
pub(super) fn start(worker: ReportWorker) -> io::Result<ReportWorkerHandle> {
    let outbox_dir = default_outbox_dir().map_err(io::Error::other)?;
    let redaction = RedactionContext::from_env();
    start_with(
        worker,
        ReportOutbox::new(outbox_dir),
        redaction,
        Box::new(|| {
            let cfg = IntakeConfig::load_default().map_err(|err| err.to_string())?;
            let project_id = cfg.project_id;
            Ok((
                project_id,
                Box::new(CurlTransport::new(cfg)) as Box<dyn ReportTransport>,
            ))
        }),
    )
}

type TransportFactory =
    Box<dyn Fn() -> Result<(u64, Box<dyn ReportTransport>), String> + Send + 'static>;

fn start_with(
    worker: ReportWorker,
    outbox: ReportOutbox,
    redaction: RedactionContext,
    transport_factory: TransportFactory,
) -> io::Result<ReportWorkerHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_stop = Arc::clone(&stop);
    let worker_done = Arc::clone(&done);

    let thread = std::thread::Builder::new()
        .name("hex-report-control".into())
        .spawn(move || {
            let _exit = ExitSignal(worker_done);

            // Best-effort startup outbox flush when intake credentials are configured.
            if !worker_stop.load(Ordering::Acquire)
                && let Ok((_, transport)) = transport_factory()
            {
                let _ = outbox.flush_pending(transport.as_ref());
            }

            loop {
                if worker_stop.load(Ordering::Acquire) {
                    break;
                }
                let command = match worker.commands.recv_timeout(POLL_SLICE) {
                    Ok(cmd) => cmd,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if worker_stop.load(Ordering::Acquire) {
                    break;
                }
                match command {
                    ReportCommand::DismissStatus => {
                        worker.publish(ReportStatus::Idle);
                    }
                    ReportCommand::FlushOutbox => {
                        if let Ok((_, transport)) = transport_factory() {
                            let _ = outbox.flush_pending(transport.as_ref());
                        }
                    }
                    ReportCommand::Preview(req) => {
                        let bundle = slice_optional_bundle(
                            req.include_diagnostics,
                            req.active_log.as_ref(),
                            req.snapshot.reported_at_unix_nanos,
                        );
                        let envelope = ReportEnvelope::with_request_id(
                            "00000000-0000-4000-8000-000000000000".into(),
                            DEFAULT_PROJECT_ID,
                            req.kind,
                            req.user_summary,
                            req.snapshot,
                            bundle.as_ref(),
                        );
                        let redacted = redact_envelope(&envelope, &redaction);
                        let text = format_redacted_preview(&redacted);
                        worker.deliver_preview(ReportPreview { id: req.id, text });
                    }
                    ReportCommand::Submit(req) => {
                        worker.publish(ReportStatus::Submitting(req.id));
                        let bundle = slice_optional_bundle(
                            req.include_diagnostics,
                            req.active_log.as_ref(),
                            req.snapshot.reported_at_unix_nanos,
                        );
                        let loaded_transport = transport_factory();
                        let project_id = loaded_transport
                            .as_ref()
                            .map(|(pid, _)| *pid)
                            .unwrap_or(DEFAULT_PROJECT_ID);

                        let envelope = ReportEnvelope::new(
                            project_id,
                            req.kind,
                            req.user_summary,
                            req.snapshot,
                            bundle.as_ref(),
                        );
                        let redacted = redact_envelope(&envelope, &redaction);

                        if let Err(error) = outbox.enqueue(&redacted) {
                            worker.publish(ReportStatus::Failed {
                                id: req.id,
                                error: format!("could not save report to outbox: {error}"),
                            });
                            continue;
                        }

                        match loaded_transport {
                            Ok((_, transport)) => match transport.submit(&redacted) {
                                Ok(receipt) => {
                                    let _ = outbox.remove(&redacted.request_id);
                                    // Best-effort flush of any previously queued offline reports.
                                    let _ = outbox.flush_pending(transport.as_ref());
                                    worker.publish(ReportStatus::Submitted {
                                        id: req.id,
                                        report_id: receipt.report_id,
                                        deduplicated: receipt.deduplicated,
                                    });
                                }
                                Err(error) => {
                                    let reason = error.to_string();
                                    if let Ok(items) = outbox.list_pending()
                                        && let Some(item) = items.iter().find(|item| {
                                            item.envelope.request_id == redacted.request_id
                                        })
                                    {
                                        let _ = outbox.record_failure(item, &reason);
                                    }
                                    worker.publish(ReportStatus::QueuedOffline {
                                        id: req.id,
                                        reason,
                                    });
                                }
                            },
                            Err(config_err) => {
                                worker.publish(ReportStatus::QueuedOffline {
                                    id: req.id,
                                    reason: config_err,
                                });
                            }
                        }
                    }
                }
            }
        })?;

    Ok(ReportWorkerHandle {
        stop,
        done,
        thread: Some(thread),
        join_timeout: DEFAULT_JOIN_TIMEOUT,
    })
}

fn slice_optional_bundle(
    include_diagnostics: bool,
    active_log: Option<&PathBuf>,
    reported_at_unix_nanos: u128,
) -> Option<hex_diagnostics::report::DiagnosticReportBundle> {
    if !include_diagnostics {
        return None;
    }
    let path = active_log?;
    build_report_bundle(path, reported_at_unix_nanos, &ReportSliceConfig::default()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_report::{IntakeReceipt, ReportKind, RuntimeSnapshot, TransportError};
    use hex_ui::{ReportPreviewRequest, ReportRequest};
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct MockTransport {
        fail: bool,
        captured: Arc<Mutex<Vec<ReportEnvelope>>>,
    }

    impl ReportTransport for MockTransport {
        fn submit(&self, envelope: &ReportEnvelope) -> Result<IntakeReceipt, TransportError> {
            self.captured.lock().unwrap().push(envelope.clone());
            if self.fail {
                return Err(TransportError::Network("simulated offline".into()));
            }
            Ok(IntakeReceipt {
                report_id: 101,
                project_id: envelope.project_id,
                request_id: envelope.request_id.clone(),
                status: "new".into(),
                deduplicated: false,
            })
        }
    }

    fn wait_for_status(
        client: &hex_ui::ReportClient,
        predicate: impl Fn(&ReportStatus) -> bool,
    ) -> ReportStatus {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let status = client.status();
            if predicate(&status) {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for report status; last={status:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn worker_slices_redacts_submits_and_flushes_offline_outbox_on_restart() {
        let root = temp_dir("hex-report-worker");
        let log_path = root.join("session.log");
        fs::write(
            &log_path,
            "session_started log_schema=3 segment=0\nevent level=error category=audio unix_nanos=5000000000 elapsed_nanos=1 sequence=1 truncated=false message=\"opened /Users/alice/test.hex plsr_secret123\"\n",
        )
        .unwrap();
        let outbox_dir = root.join("outbox");
        let redaction = RedactionContext::new(
            Some("/Users/alice".into()),
            Some("alice".into()),
            Some("mac-studio".into()),
        );

        // 1. Offline submit persists to outbox and reports QueuedOffline.
        let captured_offline = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured_offline);
        let (client, worker) = hex_ui::report_channel();
        let mut handle = start_with(
            worker,
            ReportOutbox::new(outbox_dir.clone()),
            redaction.clone(),
            Box::new(move || {
                Ok((
                    2,
                    Box::new(MockTransport {
                        fail: true,
                        captured: Arc::clone(&captured_clone),
                    }) as Box<dyn ReportTransport>,
                ))
            }),
        )
        .unwrap();

        let snapshot = RuntimeSnapshot {
            reported_at_unix_nanos: 6_000_000_000,
            view_mode: "arrangement".into(),
            ..Default::default()
        };

        // Preview request resolves off-thread with redacted log slice.
        client
            .send(ReportCommand::Preview(ReportPreviewRequest {
                id: 1,
                kind: ReportKind::Bug,
                user_summary: "at /Users/alice".into(),
                include_diagnostics: true,
                snapshot: snapshot.clone(),
                active_log: Some(log_path.clone()),
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let preview = loop {
            if let Some(preview) = client.take_preview() {
                break preview;
            }
            assert!(Instant::now() < deadline, "timed out waiting for preview");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(preview.id, 1);
        assert!(preview.text.contains("~/test.hex"));
        assert!(!preview.text.contains("plsr_secret123"));

        // Submit while offline -> QueuedOffline + persisted in outbox.
        client
            .send(ReportCommand::Submit(ReportRequest {
                id: 2,
                kind: ReportKind::Bug,
                user_summary: "crash at /Users/alice/song.hex".into(),
                include_diagnostics: true,
                snapshot,
                active_log: Some(log_path),
            }))
            .unwrap();
        let status = wait_for_status(&client, |s| matches!(s, ReportStatus::QueuedOffline { .. }));
        assert!(matches!(status, ReportStatus::QueuedOffline { id: 2, .. }));
        assert!(handle.shutdown());
        drop(client);

        let pending = ReportOutbox::new(outbox_dir.clone())
            .list_pending()
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert!(!pending[0].envelope.user_summary.contains("/Users/alice"));
        assert!(!pending[0].envelope.digest_text.contains("plsr_secret123"));

        // 2. Restart worker with online transport -> startup flushes outbox!
        let captured_online = Arc::new(Mutex::new(Vec::new()));
        let captured_online_clone = Arc::clone(&captured_online);
        let (_client2, worker2) = hex_ui::report_channel();
        let mut handle2 = start_with(
            worker2,
            ReportOutbox::new(outbox_dir.clone()),
            redaction,
            Box::new(move || {
                Ok((
                    2,
                    Box::new(MockTransport {
                        fail: false,
                        captured: Arc::clone(&captured_online_clone),
                    }) as Box<dyn ReportTransport>,
                ))
            }),
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while !ReportOutbox::new(outbox_dir.clone())
            .list_pending()
            .unwrap()
            .is_empty()
        {
            assert!(
                Instant::now() < deadline,
                "startup flush did not drain outbox"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(captured_online.lock().unwrap().len(), 1);
        assert!(handle2.shutdown());
        fs::remove_dir_all(root).unwrap();
    }
}
