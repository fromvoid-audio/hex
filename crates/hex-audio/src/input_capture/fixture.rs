//! Test-only synthetic input; no CPAL device is opened.
use super::*;

/// Observes whether the synthetic input worker accepted a start request.
pub struct InputObserver {
    started: Arc<AtomicBool>,
    capture_frame: Arc<std::sync::atomic::AtomicU32>,
}
impl InputObserver {
    pub fn started(&self) -> bool {
        self.started.load(Acquire)
    }
    /// Set the synthetic shared-clock capture instant before granting start.
    pub fn set_capture_frame(&self, frame: u32) -> bool {
        if self.started() {
            return false;
        }
        self.capture_frame.store(frame, Release);
        true
    }
}

/// Create one finite mono input and its independent monitor. The fixed input
/// timestamp is selected against `HeadlessBackend::render_recording_fixture`.
pub fn monitored_input() -> Result<(InputCapture, InputMonitor, InputObserver), InputCaptureError> {
    let id: InputDeviceId = format!("{}:fixture-input", cpal::default_host().id())
        .parse()
        .map_err(|_| InputCaptureError::InvalidConfiguration)?;
    let config = InputConfig {
        sample_rate: 48_000,
        channels: 1,
    };
    let (output, input) = RingBuffer::new(PACKETS);
    let shared = Shared::opening();
    let (monitor_writer, monitor) = InputMonitor::channel(id.clone(), config, &shared, false);
    let (worker, observer) = fixture_worker(
        shared,
        WriterOutput::Recording {
            output,
            monitor: Some(monitor_writer),
        },
    )?;
    Ok((
        InputCapture {
            worker,
            id,
            config,
            input,
            prefetched: None,
            preroll_frames: 0,
            recording_ready: None,
        },
        monitor,
        observer,
    ))
}

/// A synthetic monitor-only device with the same feed and control lifecycle.
pub fn monitor_only_input()
-> Result<(MonitorCapture, InputMonitor, InputObserver), InputCaptureError> {
    let id: InputDeviceId = format!("{}:fixture-monitor", cpal::default_host().id())
        .parse()
        .map_err(|_| InputCaptureError::InvalidConfiguration)?;
    let config = InputConfig {
        sample_rate: 48_000,
        channels: 1,
    };
    let shared = Shared::opening();
    let (writer, monitor) = InputMonitor::channel(id, config, &shared, true);
    let (worker, observer) = fixture_worker(shared, WriterOutput::Monitoring(writer))?;
    Ok((MonitorCapture { worker }, monitor, observer))
}

fn fixture_worker(
    shared: Arc<Shared>,
    output: WriterOutput<cpal::StreamInstant>,
) -> Result<(CaptureWorker, InputObserver), InputCaptureError> {
    shared.phase.store(READY, Release);
    let started = Arc::new(AtomicBool::new(false));
    let worker_started = Arc::clone(&started);
    let capture_frame = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let worker_capture_frame = Arc::clone(&capture_frame);
    let worker_state = Arc::clone(&shared);
    let handle = thread::Builder::new()
        .name("hex-fixture-input".into())
        .spawn(move || {
            let mut writer = Writer {
                output,
                shared: Arc::clone(&worker_state),
                channels: 1,
                sequence: 0,
                previous: None,
            };
            while worker_state.phase.load(Acquire) != STARTING && !worker_state.stop.load(Acquire) {
                thread::park();
            }
            if !worker_state.stop.load(Acquire) {
                worker_state.phase.store(CAPTURING, Release);
                worker_started.store(true, Release);
                let nanos = u64::from(worker_capture_frame.load(Acquire)) * 1_000_000_000 / 48_000;
                let stamp = cpal::StreamInstant::new(
                    50 + (nanos / 1_000_000_000) as i64,
                    (nanos % 1_000_000_000) as u32,
                );
                writer.submit(&[0.25; 1024], stamp);
                while !worker_state.stop.load(Acquire) {
                    thread::park_timeout(Duration::from_millis(5));
                }
            }
            drop(writer);
            worker_state
                .terminal
                .store(worker_state.fault.load(Acquire) + 1, Release);
            worker_state.phase.store(FINISHED, Release);
        })
        .map_err(InputCaptureError::Spawn)?;
    let (_, error) = mpsc::sync_channel(1);
    Ok((
        CaptureWorker {
            shared,
            handle,
            error,
        },
        InputObserver {
            started,
            capture_frame,
        },
    ))
}
