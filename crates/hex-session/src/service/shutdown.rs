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
    pub fn shutdown(&mut self, timeout: Duration) -> Result<(), ShutdownError> {
        self.shutdown.store(true, Ordering::Release);
        // Disconnect wakes a worker blocked in its idle request receive.
        self.requests = None;
        let Some(worker) = self.worker.as_ref() else {
            return Ok(());
        };
        let deadline = Instant::now().checked_add(timeout);
        while !worker.is_finished() {
            if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
                return Err(ShutdownError::Timeout(timeout));
            }
            thread::sleep(Duration::from_millis(2));
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
    fn shutdown_during_a_running_export_cancels_joins_and_removes_its_temporary_file() {
        let directory = TestDir::new();
        let source = directory.0.join("source.wav");
        fs::write(&source, wav_mono(&[16_384, 8_192])).unwrap();
        let mut service = Service::new(48_000, 16, 8).unwrap();
        next_event(&service);
        service.submit(Request::Import(source)).unwrap();
        assert!(next_event(&service).error.is_none());
        let output = directory.0.join("long.wav");
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
                overwrite: false,
            })
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while control.progress().processed_frames == 0 || temporary_files(&directory.0).is_empty() {
            assert!(Instant::now() < until, "export never started rendering");
            thread::sleep(Duration::from_millis(1));
        }
        let started = Instant::now();
        service.shutdown(Duration::from_secs(3)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(control.is_cancelled());
        assert!(control.progress().processed_frames < 48_000 * 60 * 60);
        assert!(temporary_files(&directory.0).is_empty());
        assert!(!output.exists());
        assert!(matches!(
            service.submit(Request::Undo),
            Err(SubmitError::Closed(Request::Undo))
        ));
        assert_eq!(service.shutdown(Duration::ZERO), Ok(()), "idempotent");
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
}
