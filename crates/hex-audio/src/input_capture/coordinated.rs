//! Coordinate input WAV writing and confirmed output timing off the callbacks.
//!
//! Both existing bounded recorder owners retain their own callback custody.
//! These workers do not infer a shared clock, compensate latency or edit a project.
use super::{InputCapture, InputStop, InputTake, InputTakeEnd, write_input_take};
use crate::sample_recording::{End, OutputClockIndex, OutputClockIndexError, Reader};
use std::{
    fmt, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering::{Acquire, Release},
        },
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(1);
const PREPARE_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

struct Timeouts {
    prepare: Duration,
    stop: Duration,
}

/// An explicit non-success reason. The result always retains the requested
/// take path; a writer still being retired may finish that file later.
#[derive(Debug)]
pub enum CoordinatedFailure {
    CancelledBeforeInput,
    OutputNotReady,
    OutputPrepareTimeout,
    OutputStopped(End),
    OutputIndex(OutputClockIndexError),
    InputBeforeStart {
        reason: super::InputEnd,
        detail: Option<String>,
    },
    InputWorkerStart(io::Error),
    InputWorkerExited,
    InputWrite(io::Error),
    InputInterrupted,
    InputStopTimeout,
    OutputStopTimeout,
}
impl fmt::Display for CoordinatedFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CancelledBeforeInput => f.write_str("recording was stopped before input began"),
            Self::OutputNotReady => f.write_str("output timing recorder is unavailable"),
            Self::OutputPrepareTimeout => f.write_str("playback timing did not become available"),
            Self::OutputStopped(end) => f.write_str(match end {
                End::Stopped => "output timing stopped before input finished",
                End::OutputFailed => "audio output failed during recording",
                End::SourceChanged => "audio output source changed during recording",
                End::Backpressure => "output timing queue overflowed",
                End::InvalidBlock => "audio output produced an invalid block",
                End::UnsupportedOutput => "audio output cannot provide recording timing",
                End::StorageFailed => "output recording storage failed",
            }),
            Self::OutputIndex(error) => write!(f, "output timing failed: {error}"),
            Self::InputBeforeStart { reason, detail } => {
                if let Some(detail) = detail {
                    write!(f, "audio input failed before recording: {detail}")
                } else {
                    write!(
                        f,
                        "audio input failed before recording: {}",
                        match reason {
                            super::InputEnd::Stopped => "stopped",
                            super::InputEnd::Backpressure => "queue overflow",
                            super::InputEnd::InvalidBuffer => "invalid audio buffer",
                            super::InputEnd::ClockChanged => "clock changed",
                            super::InputEnd::DeviceFailed => "device unavailable",
                            super::InputEnd::OpenFailed => "device could not open",
                            super::InputEnd::WorkerFailed => "input worker failed",
                        }
                    )
                }
            }
            Self::InputWorkerStart(error) => write!(f, "input writer could not start: {error}"),
            Self::InputWorkerExited => f.write_str("input writer exited without a result"),
            Self::InputWrite(error) => write!(f, "input take could not be written: {error}"),
            Self::InputInterrupted => f.write_str("input recording was interrupted"),
            Self::InputStopTimeout => f.write_str("input recording did not stop in time"),
            Self::OutputStopTimeout => f.write_str("output timing did not stop in time"),
        }
    }
}
impl std::error::Error for CoordinatedFailure {}

/// The recorded objects and their outcome. A failed outcome may retain an
/// input prefix, but can never be mistaken for a completed recording.
pub enum CoordinatedOutcome {
    Completed {
        path: PathBuf,
        input: InputTake,
        output: OutputClockIndex,
    },
    Failed {
        path: PathBuf,
        input: Option<InputTake>,
        reason: CoordinatedFailure,
    },
}

/// Control-side job handle. Stop and Drop only request asynchronous stop; they
/// never join a worker or wait for either native callback.
pub struct CoordinatedJob {
    input_stop: InputStop,
    cancel: Arc<AtomicBool>,
    result: Receiver<CoordinatedOutcome>,
    path: PathBuf,
}
impl CoordinatedJob {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn stop_on_control(&self) {
        self.cancel.store(true, Release);
        self.input_stop.stop_on_control();
    }
    /// `Disconnected` means there is no unread result, either because the
    /// coordinator exited without one or because it was already received.
    pub fn try_result_on_control(&mut self) -> Result<Option<CoordinatedOutcome>, TryRecvError> {
        match self.result.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
impl Drop for CoordinatedJob {
    fn drop(&mut self) {
        self.stop_on_control();
    }
}

/// Start one coordinator with an unstarted input and a newly reserved output
/// reader. Output must actually deliver a timed span before input writing starts.
/// The requested file must not exist; no project or clock authority is inferred.
pub fn spawn_coordinated_take(
    input: InputCapture,
    output: Reader,
    path: PathBuf,
) -> io::Result<CoordinatedJob> {
    spawn_with_timeouts(input, output, path, PREPARE_TIMEOUT, STOP_TIMEOUT)
}

fn spawn_with_timeouts(
    input: InputCapture,
    output: Reader,
    path: PathBuf,
    prepare_timeout: Duration,
    stop_timeout: Duration,
) -> io::Result<CoordinatedJob> {
    spawn_with_writer(
        input,
        output,
        path,
        prepare_timeout,
        stop_timeout,
        write_input_take,
    )
}

fn spawn_with_writer(
    input: InputCapture,
    output: Reader,
    path: PathBuf,
    prepare_timeout: Duration,
    stop_timeout: Duration,
    writer: impl FnOnce(InputCapture, &Path) -> io::Result<InputTake> + Send + 'static,
) -> io::Result<CoordinatedJob> {
    if !matches!(
        super::InputCapture::phase(&input),
        super::InputPhase::Opening | super::InputPhase::Ready
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "input capture has already started or stopped",
        ));
    }
    if input.source().1.sample_rate != output.sample_rate() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "input and output sample rates differ",
        ));
    }
    let input_stop = input.stop_handle_on_control();
    let worker_stop = input_stop.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let worker_path = path.clone();
    let (sender, result) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("hex-input-take-coordinator".into())
        .spawn(move || {
            let outcome = coordinate(
                input,
                output,
                worker_path,
                worker_stop,
                &worker_cancel,
                Timeouts {
                    prepare: prepare_timeout,
                    stop: stop_timeout,
                },
                writer,
            );
            let _ = sender.try_send(outcome);
        })?;
    Ok(CoordinatedJob {
        input_stop,
        cancel,
        result,
        path,
    })
}

fn failure(
    path: PathBuf,
    input: Option<InputTake>,
    reason: CoordinatedFailure,
) -> CoordinatedOutcome {
    CoordinatedOutcome::Failed {
        path,
        input,
        reason,
    }
}

fn coordinate(
    mut input: InputCapture,
    mut output: Reader,
    path: PathBuf,
    stop: InputStop,
    cancel: &AtomicBool,
    timeouts: Timeouts,
    writer: impl FnOnce(InputCapture, &Path) -> io::Result<InputTake> + Send + 'static,
) -> CoordinatedOutcome {
    let mut index = OutputClockIndex::new(&output);
    if !output.ready() {
        return failure(path, None, CoordinatedFailure::OutputNotReady);
    }
    let prepare_start = Instant::now();
    let mut checked = 0;
    loop {
        if cancel.load(Acquire) {
            output.stop();
            return failure(path, None, CoordinatedFailure::CancelledBeforeInput);
        }
        if let super::InputPhase::Finished(reason) = input.phase() {
            output.stop();
            return failure(
                path,
                None,
                CoordinatedFailure::InputBeforeStart {
                    reason,
                    detail: input.take_open_error_on_control(),
                },
            );
        }
        if input.failure() == Some(super::InputEnd::WorkerFailed) {
            output.stop();
            return failure(
                path,
                None,
                CoordinatedFailure::InputBeforeStart {
                    reason: super::InputEnd::WorkerFailed,
                    detail: input.take_open_error_on_control(),
                },
            );
        }
        match index.poll(&mut output) {
            Err(error) => return failure(path, None, CoordinatedFailure::OutputIndex(error)),
            Ok(Some(end)) => return failure(path, None, CoordinatedFailure::OutputStopped(end)),
            Ok(None) => {}
        }
        let mut timed = false;
        while checked < index.len() {
            if index
                .packet(checked)
                .is_some_and(|packet| !packet.timing.is_empty())
            {
                timed = true;
            }
            checked += 1;
        }
        if timed {
            break;
        }
        if prepare_start.elapsed() >= timeouts.prepare {
            output.stop();
            return failure(path, None, CoordinatedFailure::OutputPrepareTimeout);
        }
        thread::sleep(POLL);
    }
    if cancel.load(Acquire) {
        output.stop();
        return failure(path, None, CoordinatedFailure::CancelledBeforeInput);
    }
    let (send_input, input_result) = mpsc::sync_channel(1);
    let writer_path = path.clone();
    if let Err(error) = thread::Builder::new()
        .name("hex-input-take-writer".into())
        .spawn(move || {
            let result = writer(input, &writer_path);
            let _ = send_input.try_send(result);
        })
    {
        output.stop();
        return failure(path, None, CoordinatedFailure::InputWorkerStart(error));
    }
    let mut stop_requested = None;
    let take = loop {
        match index.poll(&mut output) {
            Err(error) => {
                stop.stop_on_control();
                return failure(path, None, CoordinatedFailure::OutputIndex(error));
            }
            Ok(Some(end)) => {
                stop.stop_on_control();
                return failure(path, None, CoordinatedFailure::OutputStopped(end));
            }
            Ok(None) => {}
        }
        match input_result.try_recv() {
            Ok(Ok(take)) => break take,
            Ok(Err(error)) => {
                output.stop();
                return failure(path, None, CoordinatedFailure::InputWrite(error));
            }
            Err(TryRecvError::Disconnected) => {
                output.stop();
                return failure(path, None, CoordinatedFailure::InputWorkerExited);
            }
            Err(TryRecvError::Empty) => {}
        }
        if cancel.load(Acquire) {
            let at = stop_requested.get_or_insert_with(Instant::now);
            if at.elapsed() >= timeouts.stop {
                output.stop();
                return failure(path, None, CoordinatedFailure::InputStopTimeout);
            }
        }
        thread::sleep(POLL);
    };
    output.stop();
    let stop_start = Instant::now();
    loop {
        match index.poll(&mut output) {
            Err(error) => return failure(path, Some(take), CoordinatedFailure::OutputIndex(error)),
            Ok(Some(End::Stopped)) => {
                if matches!(take.end, InputTakeEnd::Stopped) {
                    return CoordinatedOutcome::Completed {
                        path,
                        input: take,
                        output: index,
                    };
                }
                return failure(path, Some(take), CoordinatedFailure::InputInterrupted);
            }
            Ok(Some(end)) => {
                return failure(path, Some(take), CoordinatedFailure::OutputStopped(end));
            }
            Ok(None) => {}
        }
        if stop_start.elapsed() >= timeouts.stop {
            return failure(path, Some(take), CoordinatedFailure::OutputStopTimeout);
        }
        thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        input_capture::{self, Shared, Writer},
        sample_recording::{self, OutputClock},
    };
    use cpal::StreamInstant;
    use hex_engine::ProjectSignal;
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "hex-coordinated-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn mock_input(fail: bool) -> (InputCapture, Receiver<()>) {
        let (producer, input) = rtrb::RingBuffer::new(input_capture::PACKETS);
        let shared = Arc::new(Shared {
            phase: AtomicU8::new(input_capture::READY),
            stop: AtomicBool::new(false),
            fault: AtomicU8::new(input_capture::NO_FAULT),
            terminal: AtomicU8::new(0),
        });
        let state = Arc::clone(&shared);
        let (started, signal) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let mut writer = Writer {
                output: producer,
                shared: Arc::clone(&state),
                channels: 1,
                sequence: 0,
                previous: None,
            };
            while state.phase.load(Acquire) != input_capture::STARTING && !state.stop.load(Acquire)
            {
                thread::park();
            }
            if !state.stop.load(Acquire) {
                state.phase.store(input_capture::CAPTURING, Release);
                writer.submit(&[0.25; 64], StreamInstant::new(50, 0));
                let _ = started.try_send(());
                if fail {
                    state.fail(input_capture::DEVICE_FAILED);
                }
                while !state.stop.load(Acquire) {
                    thread::park();
                }
            }
            drop(writer);
            state.terminal.store(state.fault.load(Acquire) + 1, Release);
            state.phase.store(input_capture::FINISHED, Release);
        });
        let (_, error) = mpsc::sync_channel(1);
        (
            InputCapture {
                id: format!("{}:test-input", cpal::default_host().id())
                    .parse()
                    .unwrap(),
                config: input_capture::InputConfig {
                    sample_rate: 48_000,
                    channels: 1,
                },
                input,
                shared,
                worker,
                error,
            },
            signal,
        )
    }
    fn output() -> (
        sample_recording::Control,
        sample_recording::Callback,
        Reader,
        hex_engine::PreparedTempoMap,
    ) {
        let (mut control, callback) = sample_recording::channel(48_000);
        let reader = control.start(1, &[], true).unwrap();
        let project = hex_project::Project::new(48_000).unwrap();
        let map =
            hex_engine::PreparedTempoMap::prepare(project.tempo_map().clone(), 48_000).unwrap();
        (control, callback, reader, map)
    }
    fn emit(
        callback: &mut sample_recording::Callback,
        map: &hex_engine::PreparedTempoMap,
        offset: u32,
        valid: bool,
    ) -> bool {
        callback.begin(64);
        if let Some((layout, stage)) = callback.observation() {
            stage.observe(layout, ProjectSignal::Master, 0, &[[0.1, 0.1]; 64]);
            stage.observe_timing(0, u64::from(offset), 64, map);
            callback.set_output_clock(Some(OutputClock {
                playback: StreamInstant::new(50, 0),
                offset_frames: offset,
            }));
            callback.finish(valid);
            true
        } else {
            false
        }
    }
    fn wait_started(
        callback: &mut sample_recording::Callback,
        map: &hex_engine::PreparedTempoMap,
        started: &Receiver<()>,
        offset: &mut u32,
    ) {
        let deadline = Instant::now() + Duration::from_millis(700);
        loop {
            if started.try_recv().is_ok() {
                return;
            }
            if emit(callback, map, *offset, true) {
                *offset += 64;
            }
            assert!(Instant::now() < deadline, "input did not start");
            thread::sleep(POLL);
        }
    }
    fn wait_result(
        job: &mut CoordinatedJob,
        callback: &mut sample_recording::Callback,
        map: &hex_engine::PreparedTempoMap,
        offset: &mut u32,
    ) -> CoordinatedOutcome {
        let deadline = Instant::now() + Duration::from_millis(700);
        loop {
            if let Some(outcome) = job.try_result_on_control().unwrap() {
                return outcome;
            }
            if emit(callback, map, *offset, true) {
                *offset += 64;
            }
            assert!(Instant::now() < deadline, "coordinator did not finish");
            thread::sleep(POLL);
        }
    }
    fn spawn(input: InputCapture, reader: Reader, path: &Path) -> CoordinatedJob {
        spawn_with_timeouts(
            input,
            reader,
            path.to_owned(),
            Duration::from_millis(500),
            Duration::from_millis(500),
        )
        .unwrap()
    }

    fn failed(outcome: CoordinatedOutcome) -> (CoordinatedFailure, Option<InputTake>) {
        match outcome {
            CoordinatedOutcome::Failed { reason, input, .. } => (reason, input),
            CoordinatedOutcome::Completed { .. } => panic!("unexpected successful recording"),
        }
    }

    #[test]
    fn captures_both_owners_and_returns_a_complete_clock_candidate() {
        let temp = Temp::new();
        let (input, started) = mock_input(false);
        let (_control, mut callback, reader, map) = output();
        let mut job = spawn(input, reader, &temp.0);
        let mut offset = 0;
        wait_started(&mut callback, &map, &started, &mut offset);
        job.stop_on_control();
        let result = wait_result(&mut job, &mut callback, &map, &mut offset);
        let (take, output) = match result {
            CoordinatedOutcome::Completed { input, output, .. } => (input, output),
            CoordinatedOutcome::Failed { reason, .. } => panic!("unexpected failure: {reason}"),
        };
        assert_eq!(take.frames, 64);
        assert!(matches!(take.end, InputTakeEnd::Stopped));
        assert_eq!(output.completion().unwrap(), Some(End::Stopped));
        let placement =
            input_capture::align_complete_take(input_capture::SharedClock, &take, &output).unwrap();
        assert_eq!((placement.timeline_start, placement.frames), (0, 64));
        assert!(std::fs::metadata(&temp.0).unwrap().len() > 44);
    }

    #[test]
    fn early_stop_cancels_before_creating_a_take() {
        let temp = Temp::new();
        let (input, _) = mock_input(false);
        let (_control, mut callback, reader, map) = output();
        let mut job = spawn(input, reader, &temp.0);
        job.stop_on_control();
        let mut offset = 0;
        let outcome = wait_result(&mut job, &mut callback, &map, &mut offset);
        assert!(matches!(
            Some(failed(outcome).0),
            Some(CoordinatedFailure::CancelledBeforeInput)
        ));
        assert!(!temp.0.exists());
    }

    #[test]
    fn input_failure_and_existing_file_are_explicit() {
        let temp = Temp::new();
        let (input, started) = mock_input(true);
        let (_control, mut callback, reader, map) = output();
        let mut job = spawn(input, reader, &temp.0);
        let mut offset = 0;
        wait_started(&mut callback, &map, &started, &mut offset);
        let outcome = wait_result(&mut job, &mut callback, &map, &mut offset);
        let (reason, input) = failed(outcome);
        assert!(matches!(reason, CoordinatedFailure::InputInterrupted));
        assert!(matches!(
            input.unwrap().end,
            InputTakeEnd::Capture(input_capture::InputEnd::DeviceFailed)
        ));
        drop(job);
        let existing = Temp::new();
        std::fs::write(&existing.0, b"existing").unwrap();
        let (input, _) = mock_input(false);
        let (_control, mut callback, reader, map) = output();
        let mut job = spawn(input, reader, &existing.0);
        let mut offset = 0;
        let outcome = wait_result(&mut job, &mut callback, &map, &mut offset);
        assert!(matches!(
            Some(failed(outcome).0),
            Some(CoordinatedFailure::InputWrite(_))
        ));
        assert_eq!(std::fs::read(&existing.0).unwrap(), b"existing");
    }

    #[test]
    fn output_failure_stops_input_and_is_not_success() {
        let temp = Temp::new();
        let (input, started) = mock_input(false);
        let (_control, mut callback, reader, map) = output();
        let mut job = spawn(input, reader, &temp.0);
        let mut offset = 0;
        wait_started(&mut callback, &map, &started, &mut offset);
        assert!(emit(&mut callback, &map, offset, false));
        let outcome = wait_result(&mut job, &mut callback, &map, &mut offset);
        assert!(matches!(
            Some(failed(outcome).0),
            Some(CoordinatedFailure::OutputStopped(End::OutputFailed))
        ));
    }

    #[test]
    fn output_preparation_and_shutdown_timeouts_are_explicit() {
        let temp = Temp::new();
        let (input, _) = mock_input(false);
        let (_control, _callback, reader, _map) = output();
        let mut job = spawn_with_timeouts(
            input,
            reader,
            temp.0.clone(),
            Duration::from_millis(20),
            Duration::from_millis(100),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_millis(500);
        let outcome = loop {
            if let Some(outcome) = job.try_result_on_control().unwrap() {
                break outcome;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(POLL);
        };
        assert!(matches!(
            Some(failed(outcome).0),
            Some(CoordinatedFailure::OutputPrepareTimeout)
        ));
        assert!(!temp.0.exists());

        let temp = Temp::new();
        let (input, started) = mock_input(false);
        let (_control, mut callback, reader, map) = output();
        let mut job = spawn_with_timeouts(
            input,
            reader,
            temp.0.clone(),
            Duration::from_millis(500),
            Duration::from_millis(25),
        )
        .unwrap();
        let mut offset = 0;
        wait_started(&mut callback, &map, &started, &mut offset);
        job.stop_on_control();
        // Deliberately stop invoking the output callback; it cannot acknowledge
        // Stop, but control must get an explicit timeout and retained input.
        let deadline = Instant::now() + Duration::from_millis(500);
        let outcome = loop {
            if let Some(outcome) = job.try_result_on_control().unwrap() {
                break outcome;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(POLL);
        };
        let (reason, input) = failed(outcome);
        assert!(matches!(reason, CoordinatedFailure::OutputStopTimeout));
        assert_eq!(input.unwrap().frames, 64);
    }

    #[test]
    fn missing_output_clock_and_mismatched_rates_are_refused() {
        let temp = Temp::new();
        let (input, _) = mock_input(false);
        let (_control, mut callback, reader, map) = output();
        let mut job = spawn(input, reader, &temp.0);
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            callback.begin(64);
            if let Some((layout, stage)) = callback.observation() {
                stage.observe(layout, ProjectSignal::Master, 0, &[[0.1, 0.1]; 64]);
                stage.observe_timing(0, 0, 64, &map);
                callback.finish(true); // no native clock
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(POLL);
        }
        let outcome = loop {
            if let Some(outcome) = job.try_result_on_control().unwrap() {
                break outcome;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(POLL);
        };
        assert!(matches!(
            Some(failed(outcome).0),
            Some(CoordinatedFailure::OutputIndex(
                OutputClockIndexError::MissingClock
            ))
        ));
        assert!(!temp.0.exists());

        let (input, _) = mock_input(false);
        let (mut control, _callback) = sample_recording::channel(44_100);
        let reader = control.start(1, &[], true).unwrap();
        let error = match spawn_coordinated_take(input, reader, temp.0.clone()) {
            Ok(_) => panic!("mismatched rates were accepted"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn an_already_started_output_reader_is_refused() {
        let temp = Temp::new();
        let (input, _) = mock_input(false);
        let (_control, mut callback, reader, map) = output();
        assert!(reader.ready());
        let mut job = spawn(input, reader, &temp.0);
        let mut offset = 0;
        let outcome = wait_result(&mut job, &mut callback, &map, &mut offset);
        assert!(matches!(
            Some(failed(outcome).0),
            Some(CoordinatedFailure::OutputNotReady)
        ));
        assert!(!temp.0.exists());
    }

    #[test]
    fn input_open_failure_is_reported_even_before_output_timing_arrives() {
        let temp = Temp::new();
        let (mut input, _) = mock_input(false);
        let state = Arc::clone(&input.shared);
        let worker = input.worker.thread().clone();
        let (error_sender, error_receiver) = mpsc::sync_channel(1);
        input.error = error_receiver;
        let (_control, _callback, reader, _map) = output();
        let mut job = spawn(input, reader, &temp.0);
        error_sender
            .try_send("microphone unavailable".into())
            .unwrap();
        state.fail(input_capture::OPEN_FAILED);
        worker.unpark();
        let deadline = Instant::now() + Duration::from_millis(500);
        let outcome = loop {
            if let Some(outcome) = job.try_result_on_control().unwrap() {
                break outcome;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(POLL);
        };
        match failed(outcome).0 {
            CoordinatedFailure::InputBeforeStart { reason, detail } => {
                assert_eq!(reason, input_capture::InputEnd::OpenFailed);
                assert_eq!(detail.as_deref(), Some("microphone unavailable"));
            }
            _ => panic!("input open failure was not preserved"),
        }
        assert!(!temp.0.exists());
    }

    #[test]
    fn writer_exit_and_stalled_stop_are_explicit() {
        let temp = Temp::new();
        let (input, _) = mock_input(false);
        let (_control, mut callback, reader, map) = output();
        let mut job = spawn_with_writer(
            input,
            reader,
            temp.0.clone(),
            Duration::from_millis(500),
            Duration::from_millis(100),
            |_, _| panic!("simulated writer failure"),
        )
        .unwrap();
        let mut offset = 0;
        let outcome = wait_result(&mut job, &mut callback, &map, &mut offset);
        assert!(matches!(
            Some(failed(outcome).0),
            Some(CoordinatedFailure::InputWorkerExited)
        ));

        let temp = Temp::new();
        let (input, _) = mock_input(false);
        let (_control, mut callback, reader, map) = output();
        let (entered, writer_entered) = mpsc::sync_channel(1);
        let (release, wait_release) = mpsc::sync_channel(1);
        let (exited, writer_exited) = mpsc::sync_channel(1);
        let mut job = spawn_with_writer(
            input,
            reader,
            temp.0.clone(),
            Duration::from_millis(500),
            Duration::from_millis(25),
            move |input, _| {
                entered.send(()).unwrap();
                wait_release.recv().unwrap();
                drop(input);
                exited.send(()).unwrap();
                Err(io::Error::other("simulated stalled writer"))
            },
        )
        .unwrap();
        let mut offset = 0;
        wait_started(&mut callback, &map, &writer_entered, &mut offset);
        job.stop_on_control();
        let outcome = wait_result(&mut job, &mut callback, &map, &mut offset);
        assert!(matches!(
            Some(failed(outcome).0),
            Some(CoordinatedFailure::InputStopTimeout)
        ));
        release.send(()).unwrap();
        writer_exited
            .recv_timeout(Duration::from_millis(500))
            .unwrap();
    }
}
