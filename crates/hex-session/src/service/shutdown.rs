//! Bounded Service teardown. Signals the shared shutdown flag and disconnects
//! the request FIFO, then waits for the session worker to finish. That worker
//! owns the export lane, whose drop cancels a dispatched cancellable export and
//! joins the export worker, so one finished session worker proves both stopped.
//! A job that cannot be cancelled may outlive the bound; the handle is then kept
//! so a later call can still join it, and nothing blocks past the caller's bound.
use super::*;
use std::time::{Duration, Instant};

/// Bound used by `Drop`, short enough that dropping never stalls a UI thread.
pub const DROP_SHUTDOWN_BOUND: Duration = Duration::from_millis(100);

/// Why a bounded shutdown did not confirm that the workers stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownError {
    /// The workers were still running at the bound; they remain signalled.
    Timeout(Duration),
    /// The session worker ended by panicking; it is joined and gone.
    WorkerPanicked,
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout(bound) => write!(
                f,
                "project workers did not stop within {} ms",
                bound.as_millis()
            ),
            Self::WorkerPanicked => f.write_str("project worker stopped unexpectedly"),
        }
    }
}

impl std::error::Error for ShutdownError {}

impl Service {
    /// Stop the session and export workers and join them within `timeout`.
    /// Further submissions return `SubmitError::Closed`. Idempotent: after a
    /// timeout the workers stay signalled and a later call may join them.
    /// A completed export can still be read with `try_export_event` if its
    /// result channel has capacity; if the channel is full or disconnected
    /// during shutdown, or if `Service` is dropped without consuming a
    /// recovery-bearing export terminal, [`crate::ExportRecoveryJournal`]
    /// preserves the exact staging/recovery path and original failure durably
    /// without delaying shutdown or modifying staged files.
    pub fn shutdown(&mut self, timeout: Duration) -> Result<(), ShutdownError> {
        self.shutdown.store(true, Ordering::Release);
        // Disconnect wakes a worker blocked in its idle request receive; notify
        // all condvar signals so any bounded backpressure/fence wait exits immediately.
        self.requests = None;
        self.export_drain_signal.notify();
        self.outcome_drain_signal.notify();
        self.worker_wake_signal.notify();
        let Some(worker) = self.worker.as_ref() else {
            return Ok(());
        };
        let deadline = Instant::now().checked_add(timeout);
        while !worker.is_finished() {
            let seq = self.worker_exited.snapshot();
            if worker.is_finished() {
                break;
            }
            let Some(remaining) = deadline.and_then(|d| d.checked_duration_since(Instant::now()))
            else {
                return Err(ShutdownError::Timeout(timeout));
            };
            self.worker_exited
                .wait_since(seq, remaining.min(Duration::from_millis(2)));
        }
        match self.worker.take().map(JoinHandle::join) {
            Some(Err(_)) => Err(ShutdownError::WorkerPanicked),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::service_tests::{TestDir, next_event, wav_mono};
    use super::*;
    use std::{fs, sync::Barrier};

    fn temporary_files(directory: &Path) -> Vec<PathBuf> {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".hex-export-"))
            })
            .collect()
    }

    #[test]
    fn shutdown_during_a_running_export_cancels_joins_and_retains_its_temporary_file() {
        let directory = TestDir::new();
        let source = directory.0.join("source.wav");
        fs::write(&source, wav_mono(&[16_384, 8_192])).unwrap();
        let mut service = Service::new(48_000, 16, 8).unwrap();
        next_event(&service);
        service.submit(Request::Import(source)).unwrap();
        assert!(next_event(&service).error.is_none());
        let mixed_case = directory.0.join("MixedCase");
        fs::create_dir(&mixed_case).unwrap();
        let output = mixed_case.join("long.wav");
        let control = ExportControl::default();
        service
            .submit(Request::ExportRange {
                path: output.clone(),
                start: 0,
                // Far longer than the test runs; only cancellation ends it.
                frames: Some(48_000 * 60 * 60),
                signal: ExportSignal::Main,
                control: control.clone(),
                format: hex_export::ExportFormat::Pcm16,
                publication: ExportPublication::CreateNew,
            })
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while control.progress().processed_frames == 0 || temporary_files(&mixed_case).is_empty() {
            assert!(Instant::now() < until, "export never started rendering");
            thread::sleep(Duration::from_millis(1));
        }
        let started = Instant::now();
        service.shutdown(Duration::from_secs(3)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(control.is_cancelled());
        assert!(control.progress().processed_frames < 48_000 * 60 * 60);
        let stages = temporary_files(&mixed_case);
        assert_eq!(stages.len(), 1);
        let event = service.try_export_event().expect("shutdown export result");
        let error = event.result.expect_err("shutdown cancels the export");
        let rendered = text::plain(&text::lowercase_outside_verbatim(error.message()));
        assert!(rendered.contains("export cancelled"), "{rendered}");
        assert!(
            rendered.contains(&stages[0].display().to_string()),
            "{rendered}"
        );
        assert!(!output.exists());
        assert!(matches!(
            service.submit(Request::Undo),
            Err(SubmitError::Closed(request)) if matches!(*request, Request::Undo)
        ));
        assert_eq!(service.shutdown(Duration::ZERO), Ok(()), "idempotent");
    }

    #[test]
    fn active_export_cancellation_reports_retained_staging_path() {
        let directory = TestDir::new();
        let mixed_case = directory.0.join("MixedCase");
        fs::create_dir(&mixed_case).unwrap();
        let source = directory.0.join("source.wav");
        fs::write(&source, wav_mono(&[16_384, 8_192])).unwrap();
        let service = Service::new(48_000, 16, 8).unwrap();
        next_event(&service);
        service.submit(Request::Import(source)).unwrap();
        assert!(next_event(&service).error.is_none());
        let output = mixed_case.join("long.wav");
        let control = ExportControl::default();
        service
            .submit(Request::ExportRange {
                path: output.clone(),
                start: 0,
                frames: Some(48_000 * 60 * 60),
                signal: ExportSignal::Main,
                control: control.clone(),
                format: hex_export::ExportFormat::Pcm16,
                publication: ExportPublication::CreateNew,
            })
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while control.progress().processed_frames == 0 || temporary_files(&mixed_case).is_empty() {
            assert!(Instant::now() < until, "export never started rendering");
            thread::sleep(Duration::from_millis(1));
        }
        control.cancel();
        let event = loop {
            if let Ok(event) = service.try_export_event() {
                break event;
            }
            assert!(
                Instant::now() < until,
                "export cancellation was not delivered"
            );
            thread::sleep(Duration::from_millis(1));
        };
        let error = event.result.unwrap_err();
        let stages = temporary_files(&mixed_case);
        assert_eq!(stages.len(), 1);
        assert!(error.message().contains("export cancelled"));
        let rendered = text::plain(&text::lowercase_outside_verbatim(error.message()));
        assert!(rendered.contains(&stages[0].display().to_string()));
        assert!(!output.exists());
    }

    #[test]
    fn uncancellable_export_times_out_then_a_later_shutdown_joins_it() {
        let directory = TestDir::new();
        let source = directory.0.join("source.wav");
        fs::write(&source, wav_mono(&[16_384, 8_192])).unwrap();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let exporter: Exporter = {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            // Only its duration matters; the terminal is never observed.
            Arc::new(move |_, _, _, _, _, _, _, _| {
                entered.wait();
                release.wait();
                Err("held export released".into())
            })
        };
        let mut service = Service::with_exporter(48_000, 8, 8, exporter).unwrap();
        next_event(&service);
        service.submit(Request::Import(source)).unwrap();
        next_event(&service);
        service
            .submit(Request::Export(directory.0.join("held.wav")))
            .unwrap();
        entered.wait();
        let bound = Duration::from_millis(50);
        assert_eq!(service.shutdown(bound), Err(ShutdownError::Timeout(bound)));
        assert_eq!(
            ShutdownError::Timeout(bound).to_string(),
            "project workers did not stop within 50 ms"
        );
        release.wait();
        assert_eq!(service.shutdown(Duration::from_secs(3)), Ok(()));
    }

    #[test]
    fn shutdown_and_exit_persist_retained_export_stage_durably_without_consuming_channel() {
        let directory = TestDir::new();
        let source = directory.0.join("source.wav");
        fs::write(&source, wav_mono(&[16_384, 8_192])).unwrap();
        let recovery_log = directory.0.join("export-recovery-notices.log");
        let mixed_case = directory.0.join("MixedCase");
        fs::create_dir(&mixed_case).unwrap();
        let output = mixed_case.join("ExistingTarget.wav");
        fs::write(&output, b"original-destination-bytes").unwrap();
        let control = ExportControl::default();
        let stage_path = {
            let mut service = Service::new(48_000, 16, 8).unwrap();
            service.set_export_recovery_path(Some(recovery_log.clone()));
            next_event(&service);
            service.submit(Request::Import(source)).unwrap();
            assert!(next_event(&service).error.is_none());
            service
                .submit(Request::ExportRange {
                    path: output.clone(),
                    start: 0,
                    frames: Some(48_000 * 60 * 60),
                    signal: ExportSignal::Main,
                    control: control.clone(),
                    format: hex_export::ExportFormat::Pcm16,
                    publication: ExportPublication::confirmed_save(&output).unwrap(),
                })
                .unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            while control.progress().processed_frames == 0
                || temporary_files(&mixed_case).is_empty()
            {
                assert!(Instant::now() < until, "export never started rendering");
                thread::sleep(Duration::from_millis(1));
            }
            let stages = temporary_files(&mixed_case);
            assert_eq!(stages.len(), 1);
            service.shutdown(Duration::from_secs(3)).unwrap();
            // Simulate application exit without the UI ever calling `try_export_event()`.
            stages[0].clone()
        };

        // Existing destination bytes and retained staging file are both preserved.
        assert_eq!(fs::read(&output).unwrap(), b"original-destination-bytes");
        assert!(stage_path.exists());

        // A later session reads the durable notice log and recovers the exact
        // staging location and original cancellation cause.
        let notices = crate::ExportRecoveryJournal::load_notices(&recovery_log)
            .expect("load durable notices");
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, crate::ExportRecoveryKind::StagingRetained);
        assert_eq!(notices[0].destination_path, output);
        assert_eq!(
            notices[0].staging_path.as_deref(),
            Some(stage_path.as_path())
        );
        assert!(notices[0].message.contains("export cancelled"));
    }

    #[test]
    fn shutdown_with_saturated_or_disconnected_result_channel_persists_staged_export_recovery() {
        for disconnected in [false, true] {
            let directory = TestDir::new();
            let mixed_case = directory.0.join("MixedCase");
            fs::create_dir(&mixed_case).unwrap();
            let destination = mixed_case.join("InFlight.wav");
            let recovery_log = directory.0.join("export-recovery-notices.log");
            let recovery = crate::ExportRecoveryJournal::new(Some(recovery_log.clone()));

            let project = Project::new(48_000).unwrap();
            let prepared = Arc::new(
                PreparedRevision::prepare(
                    &project,
                    &std::collections::BTreeMap::new(),
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let control = ExportControl::default();
            let worker_control = control.clone();
            let job = ExportJob {
                job_id: 1,
                revision: 5,
                path: destination.clone(),
                prepared,
                sources: None,
                resources: None,
                frames: 48_000 * 60 * 60,
                controlled: Some(ControlledExport {
                    start: 0,
                    signal: ExportSignal::Main,
                    control: control.clone(),
                    format: hex_export::ExportFormat::Pcm16,
                    publication: ExportPublication::CreateNew,
                }),
                streamed: None,
                foreign: None,
            };

            let (work_tx, work_rx) = mpsc::sync_channel(1);
            let (finished_tx, finished_rx) = mpsc::sync_channel(1);
            let (events_tx, events_rx) = mpsc::sync_channel(1);
            let events_rx = if disconnected {
                drop(events_rx);
                None
            } else {
                // Saturate the single-slot result channel before shutdown.
                events_tx
                    .try_send(ExportEvent {
                        revision: 99,
                        path: PathBuf::from("prior.wav"),
                        result: Err("prior export result".into()),
                    })
                    .unwrap();
                Some(events_rx)
            };

            let shutdown = Arc::new(AtomicBool::new(false));
            let worker_shutdown = Arc::clone(&shutdown);
            let worker_recovery = Arc::clone(&recovery);
            let drain_signal = Arc::new(WakeSignal::default());
            let worker_drain = Arc::clone(&drain_signal);
            let worker = thread::spawn(move || {
                run_export_worker_with_recovery(
                    work_rx,
                    events_tx,
                    Arc::new(
                        |path, prepared, start, frames, signal, format, ctrl, publication| {
                            hex_export::render_prepared_with_policy(hex_export::ExportJobSpec {
                                path,
                                revision: Arc::clone(prepared),
                                resources: None,
                                start,
                                frames,
                                signal,
                                format,
                                control: ctrl,
                                publication,
                            })
                            .map_err(ExportFailure::from)
                        },
                    ),
                    finished_tx,
                    worker_shutdown,
                    client::WakeHook::default(),
                    worker_drain,
                    Some(worker_recovery),
                    Arc::new(std::sync::Mutex::new(None)),
                );
            });

            work_tx.send(ExportWork::Render(job)).unwrap();
            drop(work_tx);

            let until = Instant::now() + Duration::from_secs(5);
            while worker_control.progress().processed_frames == 0
                || temporary_files(&mixed_case).is_empty()
            {
                assert!(Instant::now() < until, "staged export never started");
                thread::sleep(Duration::from_millis(1));
            }
            let stages = temporary_files(&mixed_case);
            assert_eq!(stages.len(), 1);

            // Trigger bounded shutdown: signal shutdown, cancel in-flight export, wake drain.
            shutdown.store(true, Ordering::Release);
            worker_control.cancel();
            drain_signal.notify();

            finished_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("worker must finish within bounded shutdown");
            worker.join().unwrap();

            // Prior queued event in saturated channel remains intact.
            if let Some(rx) = events_rx {
                let prior = rx.try_recv().unwrap();
                assert_eq!(prior.revision, 99);
            }

            // Retained staging file still exists; destination was never created.
            assert!(stages[0].exists());
            assert!(!destination.exists());

            // Durable recovery log holds exact staging path and original failure.
            let loaded = crate::ExportRecoveryJournal::load_notices(&recovery_log)
                .expect("load durable notices");
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].revision, 5);
            assert_eq!(loaded[0].kind, crate::ExportRecoveryKind::StagingRetained);
            assert_eq!(loaded[0].destination_path, destination);
            assert_eq!(loaded[0].staging_path.as_deref(), Some(stages[0].as_path()));
            assert!(loaded[0].message.contains("export cancelled"));
        }
    }
}
