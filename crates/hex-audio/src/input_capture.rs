//! Bounded physical-input capture on an independent control worker.
//!
//! The worker owns the CPAL stream and the paired callback terminals. Dropping
//! the public handle requests stop without joining; the worker retains the
//! exclusive input lease until the stream and both callback owners return.
//! One 32-packet SPSC queue holds at most 8192 stereo frames. Input callbacks
//! validate and copy at most 8192 frames without allocation, waiting, logging,
//! file work or final endpoint destruction. A full queue ends the take with
//! its accepted prefix. Stop is observed at callback entry, so an already
//! started callback can finish its bounded block. Input capture times are not
//! Arrangement clock mappings.
use crate::input_devices::InputDeviceId;
mod coordinated;
mod take;
mod take_alignment;
mod timeline_overlap;
pub use coordinated::{
    AlignedAudioTake, ArrangementTakeError, CoordinatedFailure, CoordinatedJob, CoordinatedOutcome,
    spawn_coordinated_take,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{
            AtomicBool, AtomicU8,
            Ordering::{AcqRel, Acquire, Release},
        },
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
pub use take::{InputTake, InputTakeEnd, InputTiming, write_input_take};
pub use take_alignment::{
    CandidatePlacement, TakeAlignmentError, align_audible_take, align_complete_take,
};
pub use timeline_overlap::{
    ClockOverlapError, OutputWindow, SharedClock, TimelineOverlap, timeline_overlap,
};

const FRAMES: usize = 256;
const PACKETS: usize = 32;
const MAX_CALLBACK_FRAMES: usize = FRAMES * PACKETS;
const MAX_DEVICES: usize = 256;
const OPENING: u8 = 0;
const READY: u8 = 1;
const STARTING: u8 = 2;
const CAPTURING: u8 = 3;
const STOPPING: u8 = 4;
const FINISHED: u8 = 5;
const NO_FAULT: u8 = 0;
const BACKPRESSURE: u8 = 1;
const INVALID_BUFFER: u8 = 2;
const CLOCK_CHANGED: u8 = 3;
const DEVICE_FAILED: u8 = 4;
const OPEN_FAILED: u8 = 5;
const WORKER_FAILED: u8 = 6;
static INPUT_OCCUPIED: AtomicBool = AtomicBool::new(false);

/// The requested device format. Only mono and stereo f32 at 8–192 kHz are
/// admitted; the selected device must report this exact configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputConfig {
    pub sample_rate: u32,
    pub channels: u16,
}

/// How capture ended. A terminal is observable only after accepted packets
/// have been read and the original callback and stream have returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEnd {
    Stopped,
    Backpressure,
    InvalidBuffer,
    ClockChanged,
    DeviceFailed,
    OpenFailed,
    WorkerFailed,
}

/// Control-side state; a failed or stopping source never implies quiescence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputPhase {
    Opening,
    Ready,
    Starting,
    Capturing,
    Stopping,
    Finished(InputEnd),
}

/// One accepted PCM block. `captured_at` is the original callback's CPAL
/// capture instant; `offset_frames` locates this block within that callback.
/// It is not automatically comparable with another stream's clock. Mono
/// inputs are copied into both channels without scaling.
pub struct InputPacket {
    pub captured_at: cpal::StreamInstant,
    pub offset_frames: u32,
    pub sequence: u64,
    frames: usize,
    pcm: [[f32; 2]; FRAMES],
}
impl InputPacket {
    pub fn frames(&self) -> &[[f32; 2]] {
        &self.pcm[..self.frames]
    }
}

#[derive(Debug)]
pub enum InputCaptureError {
    Busy,
    InvalidConfiguration,
    Spawn(std::io::Error),
}
impl fmt::Display for InputCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("audio input is open or still stopping"),
            Self::InvalidConfiguration => f.write_str("unsupported audio input configuration"),
            Self::Spawn(error) => write!(f, "audio input worker could not start: {error}"),
        }
    }
}
impl std::error::Error for InputCaptureError {}

struct Shared {
    phase: AtomicU8,
    stop: AtomicBool,
    fault: AtomicU8,
    terminal: AtomicU8,
}
impl Shared {
    fn fail(&self, fault: u8) {
        let _ = self
            .fault
            .compare_exchange(NO_FAULT, fault, AcqRel, Acquire);
        self.stop.store(true, Release);
    }
}
struct Lease;
impl Drop for Lease {
    fn drop(&mut self) {
        INPUT_OCCUPIED.store(false, Release);
    }
}

#[derive(Clone, Copy)]
struct Block<T> {
    stamp: T,
    offset_frames: u32,
    sequence: u64,
    frames: usize,
    pcm: [[f32; 2]; FRAMES],
}
struct MonitorWriter<T> {
    output: Producer<Block<T>>,
    overrun: Arc<AtomicBool>,
}
struct Writer<T> {
    output: Producer<Block<T>>,
    shared: Arc<Shared>,
    channels: usize,
    sequence: u64,
    previous: Option<T>,
    monitor: Option<MonitorWriter<T>>,
}
impl<T: Copy + Ord> Writer<T> {
    fn submit(&mut self, input: &[f32], stamp: T) {
        let phase = self.shared.phase.load(Acquire);
        if self.shared.stop.load(Acquire)
            || !matches!(phase, STARTING | CAPTURING)
            || input.is_empty()
        {
            return;
        }
        if !input.len().is_multiple_of(self.channels)
            || input.len() / self.channels > MAX_CALLBACK_FRAMES
            || input.iter().any(|sample| !sample.is_finite())
        {
            self.shared.fail(INVALID_BUFFER);
            return;
        }
        if self.previous.is_some_and(|previous| stamp < previous) {
            self.shared.fail(CLOCK_CHANGED);
            return;
        }
        self.previous = Some(stamp);
        for (part, samples) in input.chunks(FRAMES * self.channels).enumerate() {
            let Some(next) = self.sequence.checked_add(1) else {
                self.shared.fail(INVALID_BUFFER);
                return;
            };
            let mut block = Block {
                stamp,
                offset_frames: (part * FRAMES) as u32,
                sequence: self.sequence,
                frames: samples.len() / self.channels,
                pcm: [[0.0; 2]; FRAMES],
            };
            for (frame, input) in block
                .pcm
                .iter_mut()
                .zip(samples.chunks_exact(self.channels))
            {
                *frame = if self.channels == 1 {
                    [input[0], input[0]]
                } else {
                    [input[0], input[1]]
                };
            }
            if let Some(monitor) = &mut self.monitor {
                let live = block;
                if self.output.push(block).is_err() {
                    self.shared.fail(BACKPRESSURE);
                    return;
                }
                if !monitor.overrun.load(Acquire)
                    && !monitor.output.is_abandoned()
                    && monitor.output.push(live).is_err()
                {
                    // A monitor discontinuity must never stop or truncate a take.
                    monitor.overrun.store(true, Release);
                }
            } else if self.output.push(block).is_err() {
                self.shared.fail(BACKPRESSURE);
                return;
            }
            self.sequence = next;
        }
    }
}

/// Separately bounded live PCM feed from an input capture. It contains at most
/// 32 packets of 256 frames. A full queue permanently latches an overrun;
/// the consumer must stop using this feed rather than play discontinuous PCM.
/// An empty queue means no packet is currently available. A requested stop or
/// input fault refuses further packets; a concurrent read may already be in flight.
///
/// The endpoint may be read from an output callback, but its owner must retain
/// and destroy it on control only after that callback has quiesced. In
/// particular, dropping this endpoint on an audio callback is forbidden.
pub struct InputMonitor {
    input: Consumer<Block<cpal::StreamInstant>>,
    overrun: Arc<AtomicBool>,
    shared: Arc<Shared>,
    id: InputDeviceId,
    config: InputConfig,
}

/// A monitor feed cannot provide more continuous input. This is not evidence
/// that the native input callback has quiesced; only `InputPhase::Finished` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMonitorEnd {
    Overrun,
    StopRequested,
    InputFailure(InputEnd),
}
impl fmt::Display for InputMonitorEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overrun => f.write_str("audio input monitor queue overran"),
            Self::StopRequested => f.write_str("audio input monitoring stop requested"),
            Self::InputFailure(reason) => {
                let detail = match reason {
                    InputEnd::Stopped => "stopped",
                    InputEnd::Backpressure => "recording queue full",
                    InputEnd::InvalidBuffer => "invalid input buffer",
                    InputEnd::ClockChanged => "input clock changed",
                    InputEnd::DeviceFailed => "input device failed",
                    InputEnd::OpenFailed => "input device could not open",
                    InputEnd::WorkerFailed => "input worker failed",
                };
                write!(f, "audio input monitoring failed · {detail}")
            }
        }
    }
}
impl std::error::Error for InputMonitorEnd {}

impl InputMonitor {
    /// The requested input and configuration; opening may still fail on the worker.
    pub fn source(&self) -> (&InputDeviceId, InputConfig) {
        (&self.id, self.config)
    }

    fn status(&self) -> Result<(), InputMonitorEnd> {
        if self.overrun.load(Acquire) {
            return Err(InputMonitorEnd::Overrun);
        }
        let fault = self.shared.fault.load(Acquire);
        if fault != NO_FAULT {
            return Err(InputMonitorEnd::InputFailure(end(fault.saturating_add(1))));
        }
        if self.shared.stop.load(Acquire) {
            return Err(InputMonitorEnd::StopRequested);
        }
        Ok(())
    }

    /// Read at most one fixed-size packet without allocation or waiting.
    /// Once an overrun is observed, later reads always refuse it. A read that
    /// races the overrun can complete at its final check before the overrun.
    pub fn try_read(&mut self) -> Result<Option<InputPacket>, InputMonitorEnd> {
        self.status()?;
        let next = self.input.pop().ok();
        self.status()?;
        Ok(next.map(packet))
    }
}

/// A control/worker-side handle, never an audio-callback owner. The queue is
/// finite; consumers must drain promptly. Dropping it requests asynchronous
/// stop and abandons unread packets. The input lease remains held until
/// callback quiescence; an uncertain native owner stays quarantined.
pub struct InputCapture {
    id: InputDeviceId,
    config: InputConfig,
    input: Consumer<Block<cpal::StreamInstant>>,
    shared: Arc<Shared>,
    worker: JoinHandle<()>,
    error: Receiver<String>,
    prefetched: Option<Block<cpal::StreamInstant>>,
    preroll_frames: u64,
    // Bit 0 means ready, bit 1 means stopped. One atomic orders stop against
    // the writer's ready transition, so a cancelled preparation cannot start.
    recording_ready: Option<Arc<AtomicU8>>,
}

/// A control-side stop request for one capture. It remains bound to that
/// capture after the reader moves to a writer worker or the capture finishes.
/// Dropping a clone does not stop recording. Never call it from an audio callback.
#[derive(Clone)]
pub struct InputStop {
    shared: Arc<Shared>,
    worker: thread::Thread,
}
impl InputStop {
    /// Request asynchronous shutdown independently of packet queue capacity.
    pub fn stop_on_control(&self) {
        request_stop(&self.shared, &self.worker);
    }
}

fn request_stop(shared: &Shared, worker: &thread::Thread) {
    shared.stop.store(true, Release);
    worker.unpark();
}

impl InputCapture {
    /// Begin opening the exact selected source on its dedicated worker.
    /// No PCM is admitted before a start request. CPAL backends can differ
    /// in whether a newly constructed stream starts automatically.
    pub fn open_on_control(
        id: InputDeviceId,
        config: InputConfig,
    ) -> Result<Self, InputCaptureError> {
        Self::open_inner(id, config, None)
    }

    /// Open one capture with an independent optional monitor feed. The feed
    /// starts with capture, never consumes take packets, and does not open a
    /// second input stream or alter the writer's stop/backpressure behavior.
    pub fn open_with_monitor_on_control(
        id: InputDeviceId,
        config: InputConfig,
    ) -> Result<(Self, InputMonitor), InputCaptureError> {
        let (output, input) = RingBuffer::new(PACKETS);
        let overrun = Arc::new(AtomicBool::new(false));
        let monitor = MonitorWriter {
            output,
            overrun: Arc::clone(&overrun),
        };
        let capture = Self::open_inner(id, config, Some(monitor))?;
        let shared = Arc::clone(&capture.shared);
        let id = capture.id.clone();
        Ok((
            capture,
            InputMonitor {
                input,
                overrun,
                shared,
                id,
                config,
            },
        ))
    }

    fn open_inner(
        id: InputDeviceId,
        config: InputConfig,
        monitor: Option<MonitorWriter<cpal::StreamInstant>>,
    ) -> Result<Self, InputCaptureError> {
        if !(8_000..=192_000).contains(&config.sample_rate) || !(1..=2).contains(&config.channels) {
            return Err(InputCaptureError::InvalidConfiguration);
        }
        if INPUT_OCCUPIED
            .compare_exchange(false, true, AcqRel, Acquire)
            .is_err()
        {
            return Err(InputCaptureError::Busy);
        }
        let lease = Lease;
        let (producer, input) = RingBuffer::new(PACKETS);
        let shared = Arc::new(Shared {
            phase: AtomicU8::new(OPENING),
            stop: AtomicBool::new(false),
            fault: AtomicU8::new(NO_FAULT),
            terminal: AtomicU8::new(0),
        });
        let (error_sender, error) = mpsc::sync_channel(1);
        let selected = id.clone();
        let worker_state = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("hex-audio-input".into())
            .spawn(move || {
                let _lease = lease;
                let mut custody = Custody::default();
                let result = catch_unwind(AssertUnwindSafe(|| {
                    run_worker(&mut custody, &worker_state, producer, id, config, monitor)
                }));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(reason)) => {
                        worker_state.fail(OPEN_FAILED);
                        let _ = error_sender.try_send(reason);
                    }
                    Err(_) => {
                        worker_state.fail(WORKER_FAILED);
                        worker_state.phase.store(STOPPING, Release);
                        // Uncertain native ownership stays on this worker.
                        loop {
                            thread::park_timeout(Duration::from_secs(60));
                        }
                    }
                }
                finish_worker(custody, &worker_state);
            })
            .map_err(InputCaptureError::Spawn)?;
        Ok(Self {
            id: selected,
            config,
            input,
            shared,
            worker,
            error,
            prefetched: None,
            preroll_frames: 0,
            recording_ready: None,
        })
    }

    /// The exact selected source and requested stream configuration.
    /// The worker may still reject it while opening.
    pub fn source(&self) -> (&InputDeviceId, InputConfig) {
        (&self.id, self.config)
    }

    pub fn phase(&self) -> InputPhase {
        match capture_phase(&self.input, &self.shared) {
            InputPhase::Finished(_) if self.prefetched.is_some() => InputPhase::Stopping,
            phase => phase,
        }
    }

    /// Queue a start for a prepared stream. `Capturing` confirms that CPAL
    /// accepted it; a concurrent Stop may cancel an admitted start.
    pub fn start_on_control(&self) -> bool {
        if self.shared.stop.load(Acquire) {
            return false;
        }
        let accepted = self
            .shared
            .phase
            .compare_exchange(READY, STARTING, AcqRel, Acquire)
            .is_ok();
        if accepted {
            self.worker.thread().unpark();
        }
        accepted && !self.shared.stop.load(Acquire)
    }

    pub fn stop_on_control(&self) {
        request_stop(&self.shared, self.worker.thread());
    }

    /// Retain an independent stop request before moving this reader to a
    /// writer. Creating or dropping the handle does not start or stop capture.
    pub fn stop_handle_on_control(&self) -> InputStop {
        InputStop {
            shared: Arc::clone(&self.shared),
            worker: self.worker.thread().clone(),
        }
    }

    /// A reported failure while teardown may still be in progress. Only
    /// `Finished` or a read terminal proves callback quiescence.
    pub fn failure(&self) -> Option<InputEnd> {
        let fault = self.shared.fault.load(Acquire);
        (fault != NO_FAULT).then(|| end(fault.saturating_add(1)))
    }

    /// One original open error, if the asynchronous open failed.
    pub fn take_open_error_on_control(&mut self) -> Option<String> {
        self.error.try_recv().ok()
    }

    /// Return an accepted packet before reporting the terminal. None means
    /// no packet is ready yet; the producer may still be active.
    pub fn try_read_on_control(&mut self) -> Result<Option<InputPacket>, InputEnd> {
        self.read_block().map(|block| block.map(packet))
    }
    fn read_block(&mut self) -> Result<Option<Block<cpal::StreamInstant>>, InputEnd> {
        if let Some(block) = self.prefetched.take() {
            return Ok(Some(block));
        }
        read(&mut self.input, &self.shared)
    }
}
impl Drop for InputCapture {
    fn drop(&mut self) {
        self.stop_on_control();
    }
}
fn capture_phase<T>(input: &Consumer<Block<T>>, shared: &Shared) -> InputPhase {
    let phase = shared.phase.load(Acquire);
    if phase == FINISHED {
        return if input.is_empty() {
            InputPhase::Finished(end(shared.terminal.load(Acquire)))
        } else {
            InputPhase::Stopping
        };
    }
    if shared.stop.load(Acquire) {
        return InputPhase::Stopping;
    }
    match phase {
        OPENING => InputPhase::Opening,
        READY => InputPhase::Ready,
        STARTING => InputPhase::Starting,
        CAPTURING => InputPhase::Capturing,
        _ => InputPhase::Stopping,
    }
}

fn read<T>(input: &mut Consumer<Block<T>>, shared: &Shared) -> Result<Option<Block<T>>, InputEnd> {
    if let Ok(block) = input.pop() {
        return Ok(Some(block));
    }
    let terminal = shared.terminal.load(Acquire);
    if terminal == 0 {
        return Ok(None);
    }
    if let Ok(block) = input.pop() {
        return Ok(Some(block));
    }
    Err(end(terminal))
}

fn packet(block: Block<cpal::StreamInstant>) -> InputPacket {
    InputPacket {
        captured_at: block.stamp,
        offset_frames: block.offset_frames,
        sequence: block.sequence,
        frames: block.frames,
        pcm: block.pcm,
    }
}
fn end(terminal: u8) -> InputEnd {
    match terminal.saturating_sub(1) {
        NO_FAULT => InputEnd::Stopped,
        BACKPRESSURE => InputEnd::Backpressure,
        INVALID_BUFFER => InputEnd::InvalidBuffer,
        CLOCK_CHANGED => InputEnd::ClockChanged,
        DEVICE_FAILED => InputEnd::DeviceFailed,
        OPEN_FAILED => InputEnd::OpenFailed,
        _ => InputEnd::WorkerFailed,
    }
}

#[derive(Default)]
struct Custody {
    stream: Option<cpal::Stream>,
    data: Option<crate::native_callback_owner::Terminal<Writer<cpal::StreamInstant>>>,
    errors: Option<crate::native_callback_owner::Terminal<Arc<Shared>>>,
}
impl Custody {
    fn ready(&mut self) -> bool {
        self.data.as_mut().is_none_or(|terminal| terminal.ready())
            && self.errors.as_mut().is_none_or(|terminal| terminal.ready())
    }
}

fn finish_worker(mut custody: Custody, shared: &Shared) {
    shared.phase.store(STOPPING, Release);
    // A blocking or panicking native destructor is contained on this worker.
    // An uncertain owner is retained with the lease held.
    if catch_unwind(AssertUnwindSafe(|| drop(custody.stream.take()))).is_err() {
        shared.fail(WORKER_FAILED);
        loop {
            thread::park_timeout(Duration::from_secs(60));
        }
    }
    while !custody.ready() {
        thread::park_timeout(Duration::from_millis(5));
    }
    drop(custody);
    let fault = shared.fault.load(Acquire);
    shared.terminal.store(fault.saturating_add(1), Release);
    shared.phase.store(FINISHED, Release);
}

fn run_worker(
    custody: &mut Custody,
    shared: &Arc<Shared>,
    producer: Producer<Block<cpal::StreamInstant>>,
    id: InputDeviceId,
    config: InputConfig,
    monitor: Option<MonitorWriter<cpal::StreamInstant>>,
) -> Result<(), String> {
    let host = cpal::default_host();
    if id.cpal_id().0 != host.id() {
        return Err("audio input belongs to a different host".into());
    }
    let mut selected = None;
    let devices = host
        .devices()
        .map_err(|error| format!("audio input enumeration failed: {error}"))?;
    for (index, device) in devices.enumerate() {
        if index == MAX_DEVICES {
            return Err("audio input device limit exceeded".into());
        }
        let current = device
            .id()
            .map_err(|error| format!("audio input identity failed: {error}"))?;
        if &current == id.cpal_id() {
            if selected.is_some() {
                return Err("audio input identity is ambiguous".into());
            }
            selected = Some(device);
        }
    }
    let device = selected.ok_or("selected audio input is no longer available")?;
    let configs = device
        .supported_input_configs()
        .map_err(|error| format!("audio input formats failed: {error}"))?;
    let mut supported = false;
    for (index, range) in configs.enumerate() {
        if index == 128 {
            return Err("audio input format limit exceeded".into());
        }
        if range.channels() == config.channels
            && range.sample_format() == cpal::SampleFormat::F32
            && (range.min_sample_rate()..=range.max_sample_rate()).contains(&config.sample_rate)
        {
            supported = true;
            break;
        }
    }
    if !supported {
        return Err("selected audio input does not support this format".into());
    }
    if shared.stop.load(Acquire) {
        return Ok(());
    }
    let writer = Writer {
        output: producer,
        shared: Arc::clone(shared),
        channels: usize::from(config.channels),
        sequence: 0,
        previous: None,
        monitor,
    };
    let (mut data, terminal) = crate::native_callback_owner::reserve(writer);
    let (errors, error_terminal) = crate::native_callback_owner::reserve(Arc::clone(shared));
    custody.data = Some(terminal);
    custody.errors = Some(error_terminal);
    let stream_config = cpal::StreamConfig {
        channels: config.channels,
        sample_rate: config.sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let stream = device
        .build_input_stream(
            &stream_config,
            move |samples: &[f32], info: &cpal::InputCallbackInfo| {
                if let Some(writer) = data.owner_mut() {
                    writer.submit(samples, info.timestamp().capture);
                }
            },
            move |_error| {
                if let Some(state) = errors.owner_ref() {
                    state.fail(DEVICE_FAILED);
                }
            },
            None,
        )
        .map_err(|error| format!("audio input open failed: {error}"))?;
    custody.stream = Some(stream);
    if shared.stop.load(Acquire) {
        return Ok(());
    }
    shared.phase.store(READY, Release);
    while !shared.stop.load(Acquire) {
        if shared.phase.load(Acquire) == STARTING {
            let stream = custody
                .stream
                .as_ref()
                .ok_or("audio input stream is unavailable")?;
            stream
                .play()
                .map_err(|error| format!("audio input start failed: {error}"))?;
            if !shared.stop.load(Acquire) {
                shared.phase.store(CAPTURING, Release);
            }
        }
        thread::park_timeout(Duration::from_millis(5));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(channels: usize) -> (Writer<u64>, Consumer<Block<u64>>, Arc<Shared>) {
        let (output, input) = RingBuffer::new(PACKETS);
        let shared = Arc::new(Shared {
            phase: AtomicU8::new(CAPTURING),
            stop: AtomicBool::new(false),
            fault: AtomicU8::new(NO_FAULT),
            terminal: AtomicU8::new(0),
        });
        (
            Writer {
                output,
                shared: Arc::clone(&shared),
                channels,
                sequence: 0,
                previous: None,
                monitor: None,
            },
            input,
            shared,
        )
    }

    fn monitored_channel(
        channels: usize,
    ) -> (
        Writer<cpal::StreamInstant>,
        Consumer<Block<cpal::StreamInstant>>,
        InputMonitor,
        Arc<Shared>,
    ) {
        let (output, recording) = RingBuffer::new(PACKETS);
        let (monitor_output, input) = RingBuffer::new(PACKETS);
        let overrun = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            phase: AtomicU8::new(CAPTURING),
            stop: AtomicBool::new(false),
            fault: AtomicU8::new(NO_FAULT),
            terminal: AtomicU8::new(0),
        });
        let writer = Writer {
            output,
            shared: Arc::clone(&shared),
            channels,
            sequence: 0,
            previous: None,
            monitor: Some(MonitorWriter {
                output: monitor_output,
                overrun: Arc::clone(&overrun),
            }),
        };
        (
            writer,
            recording,
            InputMonitor {
                input,
                overrun,
                shared: Arc::clone(&shared),
                id: format!("{}:test-input", cpal::default_host().id())
                    .parse()
                    .unwrap(),
                config: InputConfig {
                    sample_rate: 48_000,
                    channels: channels as u16,
                },
            },
            shared,
        )
    }

    #[test]
    fn monitor_and_recording_receive_identical_mono_and_stereo_packets() {
        for channels in [1, 2] {
            let (mut writer, mut recording, mut monitor, shared) = monitored_channel(channels);
            assert_eq!(
                monitor.source().0.to_string(),
                format!("{}:test-input", cpal::default_host().id())
            );
            assert_eq!(
                monitor.source().1,
                InputConfig {
                    sample_rate: 48_000,
                    channels: channels as u16
                }
            );
            let samples: Vec<f32> = (0..(FRAMES + 7) * channels)
                .map(|n| n as f32 / 1024.0)
                .collect();
            let stamp = cpal::StreamInstant::new(2, 123);
            writer.submit(&samples, stamp);
            for sequence in 0..2 {
                let recorded = read(&mut recording, &shared).unwrap().unwrap();
                let live = monitor.try_read().unwrap().unwrap();
                assert_eq!(live.captured_at, recorded.stamp);
                assert_eq!(live.offset_frames, recorded.offset_frames);
                assert_eq!(live.sequence, sequence);
                assert_eq!(live.sequence, recorded.sequence);
                assert_eq!(live.frames(), &recorded.pcm[..recorded.frames]);
            }
            assert!(monitor.try_read().unwrap().is_none());
            assert_eq!(shared.fault.load(Acquire), NO_FAULT);
        }
    }

    #[test]
    fn monitor_overrun_or_abandonment_never_interrupts_recording() {
        let (mut writer, mut recording, mut monitor, shared) = monitored_channel(1);
        let samples = [0.25; FRAMES];
        let stamp = cpal::StreamInstant::new(3, 0);
        for sequence in 0..=PACKETS {
            writer.submit(&samples, stamp);
            let packet = read(&mut recording, &shared).unwrap().unwrap();
            assert_eq!(packet.sequence, sequence as u64);
            assert!(
                packet.pcm[..packet.frames]
                    .iter()
                    .all(|frame| *frame == [0.25, 0.25])
            );
        }
        assert!(matches!(monitor.try_read(), Err(InputMonitorEnd::Overrun)));
        assert!(matches!(monitor.try_read(), Err(InputMonitorEnd::Overrun)));
        drop(monitor); // control-side release while capture continues
        writer.submit(&samples, stamp);
        assert_eq!(
            read(&mut recording, &shared).unwrap().unwrap().sequence,
            (PACKETS + 1) as u64
        );
        assert_eq!(shared.fault.load(Acquire), NO_FAULT);
        shared.stop.store(true, Release);
        writer.submit(&samples, stamp);
        assert!(read(&mut recording, &shared).unwrap().is_none());
    }

    #[test]
    fn abandoned_monitor_before_overrun_does_not_stop_or_change_recording() {
        let (mut writer, mut recording, monitor, shared) = monitored_channel(1);
        drop(monitor);
        let stamp = cpal::StreamInstant::new(4, 0);
        for sequence in 0..(PACKETS + 2) {
            writer.submit(&[0.5; FRAMES], stamp);
            assert_eq!(
                read(&mut recording, &shared).unwrap().unwrap().sequence,
                sequence as u64
            );
        }
        assert_eq!(shared.fault.load(Acquire), NO_FAULT);
    }

    #[test]
    fn monitor_refuses_buffered_pcm_after_stop_or_input_failure() {
        for failure in [None, Some(DEVICE_FAILED)] {
            let (mut writer, mut recording, mut monitor, shared) = monitored_channel(1);
            writer.submit(&[0.5; FRAMES], cpal::StreamInstant::new(4, 0));
            if let Some(fault) = failure {
                shared.fail(fault);
                assert!(matches!(
                    monitor.try_read(),
                    Err(InputMonitorEnd::InputFailure(InputEnd::DeviceFailed))
                ));
            } else {
                shared.stop.store(true, Release);
                assert!(matches!(
                    monitor.try_read(),
                    Err(InputMonitorEnd::StopRequested)
                ));
            }
            assert_eq!(
                read(&mut recording, &shared).unwrap().unwrap().pcm[0],
                [0.5, 0.5]
            );
        }
    }

    #[test]
    fn monitor_overrun_remains_visible_after_stop() {
        let (mut writer, mut recording, mut monitor, shared) = monitored_channel(1);
        let stamp = cpal::StreamInstant::new(4, 0);
        for _ in 0..=PACKETS {
            writer.submit(&[0.5; FRAMES], stamp);
            assert!(read(&mut recording, &shared).unwrap().is_some());
        }
        shared.stop.store(true, Release);
        assert!(matches!(monitor.try_read(), Err(InputMonitorEnd::Overrun)));
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn callback_guard_returns_monitor_custody_without_freeing_it() {
        let (mut writer, _recording, monitor, _shared) = monitored_channel(1);
        writer.submit(&[0.25; FRAMES], cpal::StreamInstant::new(4, 0));
        let (mut guard, mut terminal) = crate::native_callback_owner::reserve(monitor);
        let measured = allocation_counter::measure(|| {
            assert!(guard.owner_mut().unwrap().try_read().unwrap().is_some());
            guard.close();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(terminal.ready());
        let endpoint = terminal.take().unwrap();
        drop(endpoint); // control after callback guard returned
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn monitored_capture_and_overrun_allocate_and_free_nothing_on_callback() {
        let (mut writer, mut recording, mut monitor, shared) = monitored_channel(1);
        let stamp = cpal::StreamInstant::new(5, 0);
        let input = [0.125; FRAMES];
        let measured = allocation_counter::measure(|| {
            writer.submit(&input, stamp);
            assert!(read(&mut recording, &shared).unwrap().is_some());
            assert!(monitor.try_read().unwrap().is_some());
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        for _ in 0..PACKETS {
            writer.submit(&input, stamp);
            assert!(read(&mut recording, &shared).unwrap().is_some());
        }
        let measured = allocation_counter::measure(|| {
            writer.submit(&input, stamp);
            assert!(read(&mut recording, &shared).unwrap().is_some());
            assert!(matches!(monitor.try_read(), Err(InputMonitorEnd::Overrun)));
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(shared.fault.load(Acquire), NO_FAULT);
    }

    #[test]
    fn copies_mono_and_stereo_with_original_timestamp_and_offsets() {
        let (mut writer, mut reader, state) = channel(1);
        let input: Vec<f32> = (0..FRAMES + 2).map(|value| value as f32 / 512.0).collect();
        writer.submit(&input, 100);
        let first = read(&mut reader, &state).unwrap().unwrap();
        let second = read(&mut reader, &state).unwrap().unwrap();
        assert_eq!(
            (
                first.stamp,
                first.offset_frames,
                first.sequence,
                first.frames
            ),
            (100, 0, 0, FRAMES)
        );
        assert_eq!(
            (
                second.stamp,
                second.offset_frames,
                second.sequence,
                second.frames
            ),
            (100, FRAMES as u32, 1, 2)
        );
        for (frame, sample) in first.pcm.iter().zip(&input) {
            assert_eq!(*frame, [*sample, *sample]);
        }
        assert_eq!(second.pcm[0], [input[FRAMES], input[FRAMES]]);
        assert!(matches!(read(&mut reader, &state), Ok(None)));
        let (mut writer, mut reader, state) = channel(2);
        writer.submit(&[0.25, -0.5, 0.75, -1.0], 17);
        let block = read(&mut reader, &state).unwrap().unwrap();
        assert_eq!(&block.pcm[..block.frames], &[[0.25, -0.5], [0.75, -1.0]]);
    }

    #[test]
    fn overflow_keeps_the_exact_prefix_and_terminal_follows_drain() {
        let (mut writer, mut reader, state) = channel(2);
        let samples = vec![0.25; MAX_CALLBACK_FRAMES * 2];
        writer.submit(&samples, 1);
        writer.submit(&[0.5, 0.5], 2);
        assert_eq!(state.fault.load(Acquire), BACKPRESSURE);
        // Only the stream owner may mark terminal, after the callback returns.
        assert!(state.stop.load(Acquire));
        state.terminal.store(BACKPRESSURE + 1, Release);
        state.phase.store(FINISHED, Release);
        assert_eq!(capture_phase(&reader, &state), InputPhase::Stopping);
        for index in 0..PACKETS {
            let block = read(&mut reader, &state).unwrap().unwrap();
            assert_eq!(
                (block.sequence, block.stamp, block.frames),
                (index as u64, 1, FRAMES)
            );
            assert_eq!(block.offset_frames, (index * FRAMES) as u32);
        }
        assert!(matches!(
            read(&mut reader, &state),
            Err(InputEnd::Backpressure)
        ));
        assert_eq!(
            capture_phase(&reader, &state),
            InputPhase::Finished(InputEnd::Backpressure)
        );
    }

    #[test]
    fn invalid_buffers_and_clock_reversal_end_without_publishing_them() {
        for invalid in [
            vec![0.2],
            vec![0.2, f32::NAN],
            vec![0.0; (MAX_CALLBACK_FRAMES + 1) * 2],
        ] {
            let (mut writer, mut reader, state) = channel(2);
            writer.submit(&[0.1, 0.2], 10);
            writer.submit(&invalid, 11);
            writer.submit(&[0.3, 0.4], 12);
            assert_eq!(state.fault.load(Acquire), INVALID_BUFFER);
            assert_eq!(read(&mut reader, &state).unwrap().unwrap().sequence, 0);
            assert!(matches!(read(&mut reader, &state), Ok(None)));
        }
        let (mut writer, mut reader, state) = channel(1);
        writer.submit(&[0.1], 10);
        writer.submit(&[0.2], 9);
        assert_eq!(state.fault.load(Acquire), CLOCK_CHANGED);
        assert_eq!(read(&mut reader, &state).unwrap().unwrap().stamp, 10);
        assert!(matches!(read(&mut reader, &state), Ok(None)));
    }

    #[test]
    fn no_pcm_is_admitted_before_a_start_request() {
        let (mut writer, mut reader, state) = channel(1);
        state.phase.store(OPENING, Release);
        writer.submit(&[0.25], 1);
        state.phase.store(READY, Release);
        writer.submit(&[0.5], 2);
        assert!(matches!(read(&mut reader, &state), Ok(None)));
        state.phase.store(STARTING, Release);
        writer.submit(&[0.75], 3);
        let block = read(&mut reader, &state).unwrap().unwrap();
        assert_eq!(
            (block.sequence, block.stamp, block.pcm[0]),
            (0, 3, [0.75, 0.75])
        );
    }

    #[test]
    fn stop_is_independent_of_a_full_queue_and_first_fault_is_retained() {
        let (mut writer, mut reader, state) = channel(1);
        writer.submit(&vec![0.5; MAX_CALLBACK_FRAMES], 1);
        state.stop.store(true, Release);
        writer.submit(&[0.1], 2);
        assert_eq!(state.fault.load(Acquire), NO_FAULT);
        state.fail(DEVICE_FAILED);
        state.fail(BACKPRESSURE);
        state.terminal.store(DEVICE_FAILED + 1, Release);
        for _ in 0..PACKETS {
            assert!(matches!(read(&mut reader, &state), Ok(Some(_))));
        }
        assert!(matches!(
            read(&mut reader, &state),
            Err(InputEnd::DeviceFailed)
        ));
    }

    #[test]
    fn independent_stop_wakes_worker_with_a_full_queue_and_preserves_prefix() {
        let (mut writer, mut reader, state) = channel(1);
        writer.submit(&vec![0.5; MAX_CALLBACK_FRAMES], 1);
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            while !worker_state.stop.load(Acquire) {
                thread::park();
            }
            worker_state.terminal.store(NO_FAULT + 1, Release);
        });
        let stop = InputStop {
            shared: Arc::clone(&state),
            worker: worker.thread().clone(),
        };
        drop(stop.clone());
        assert!(!state.stop.load(Acquire));
        let (ready, moved) = mpsc::sync_channel(1);
        let (done, completed) = mpsc::sync_channel(1);
        let reader_state = Arc::clone(&state);
        let reader_worker = thread::spawn(move || {
            ready.send(()).unwrap();
            worker.join().unwrap();
            let mut count = 0;
            loop {
                match read(&mut reader, &reader_state) {
                    Ok(Some(block)) => {
                        assert_eq!(block.sequence, count);
                        count += 1;
                    }
                    Err(InputEnd::Stopped) => break,
                    _ => panic!("unexpected input terminal"),
                }
            }
            done.send(count).unwrap();
        });
        moved.recv_timeout(Duration::from_millis(500)).unwrap();
        stop.stop_on_control();
        stop.stop_on_control();
        assert_eq!(
            completed.recv_timeout(Duration::from_millis(500)).unwrap(),
            PACKETS as u64
        );
        reader_worker.join().unwrap();

        // An old handle only holds its own shared state and worker identity.
        let (_, _, later) = channel(1);
        stop.stop_on_control();
        assert!(!later.stop.load(Acquire));
    }

    #[test]
    fn stop_handle_remains_usable_after_capture_moves_to_a_reader_worker() {
        let (_, input) = RingBuffer::new(PACKETS);
        let state = Arc::new(Shared {
            phase: AtomicU8::new(CAPTURING),
            stop: AtomicBool::new(false),
            fault: AtomicU8::new(NO_FAULT),
            terminal: AtomicU8::new(0),
        });
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            while !worker_state.stop.load(Acquire) {
                thread::park();
            }
            worker_state.terminal.store(NO_FAULT + 1, Release);
            worker_state.phase.store(FINISHED, Release);
        });
        let (_, error) = mpsc::sync_channel(1);
        let capture = InputCapture {
            id: format!("{}:test-input", cpal::default_host().id())
                .parse()
                .unwrap(),
            config: InputConfig {
                sample_rate: 48_000,
                channels: 1,
            },
            input,
            shared: state,
            worker,
            error,
            prefetched: None,
            preroll_frames: 0,
            recording_ready: None,
        };
        let stop = capture.stop_handle_on_control();
        let (done, completed) = mpsc::sync_channel(1);
        let reader_worker = thread::spawn(move || {
            let mut capture = capture;
            loop {
                match capture.try_read_on_control() {
                    Ok(None) => thread::yield_now(),
                    Err(InputEnd::Stopped) => break,
                    _ => panic!("unexpected input terminal"),
                }
            }
            done.send(()).unwrap();
        });
        stop.stop_on_control();
        completed.recv_timeout(Duration::from_millis(500)).unwrap();
        reader_worker.join().unwrap();
    }

    #[test]
    fn custody_waits_for_both_original_callback_owners() {
        let (output, _reader) = RingBuffer::new(PACKETS);
        let shared = Arc::new(Shared {
            phase: AtomicU8::new(OPENING),
            stop: AtomicBool::new(false),
            fault: AtomicU8::new(NO_FAULT),
            terminal: AtomicU8::new(0),
        });
        let writer: Writer<cpal::StreamInstant> = Writer {
            output,
            shared: Arc::clone(&shared),
            channels: 1,
            sequence: 0,
            previous: None,
            monitor: None,
        };
        let (mut data, data_terminal) = crate::native_callback_owner::reserve(writer);
        let (mut errors, error_terminal) = crate::native_callback_owner::reserve(shared);
        let mut custody = Custody {
            stream: None,
            data: Some(data_terminal),
            errors: Some(error_terminal),
        };
        assert!(!custody.ready());
        data.close();
        assert!(!custody.ready());
        errors.close();
        assert!(custody.ready());
    }

    #[test]
    fn worker_reports_failure_only_after_both_callback_owners_return() {
        for cancel_reader in [false, true] {
            let (output, reader) = RingBuffer::new(PACKETS);
            let state = Arc::new(Shared {
                phase: AtomicU8::new(CAPTURING),
                stop: AtomicBool::new(false),
                fault: AtomicU8::new(NO_FAULT),
                terminal: AtomicU8::new(0),
            });
            let writer = Writer::<cpal::StreamInstant> {
                output,
                shared: Arc::clone(&state),
                channels: 1,
                sequence: 0,
                previous: None,
                monitor: None,
            };
            let (data, data_terminal) = crate::native_callback_owner::reserve(writer);
            let (errors, error_terminal) =
                crate::native_callback_owner::reserve(Arc::clone(&state));
            let custody = Custody {
                stream: None,
                data: Some(data_terminal),
                errors: Some(error_terminal),
            };
            errors.owner_ref().unwrap().fail(DEVICE_FAILED);
            let worker_state = Arc::clone(&state);
            let worker = thread::spawn(move || finish_worker(custody, &worker_state));
            let deadline = std::time::Instant::now() + Duration::from_millis(500);
            while state.phase.load(Acquire) != STOPPING {
                assert!(std::time::Instant::now() < deadline, "worker did not stop");
                thread::yield_now();
            }
            assert_eq!(state.terminal.load(Acquire), 0);
            if cancel_reader {
                drop(reader);
            }
            drop(data);
            assert_eq!(state.terminal.load(Acquire), 0);
            drop(errors);
            worker.join().unwrap();
            assert_eq!(state.terminal.load(Acquire), DEVICE_FAILED + 1);
            assert_eq!(state.phase.load(Acquire), FINISHED);
        }
    }

    #[test]
    #[ignore = "queries the actual host device list; no stream is opened"]
    fn missing_device_fails_without_falling_back() {
        let host = cpal::default_host().id();
        let id: InputDeviceId = format!("{host}:hex-nonexistent-input-8227")
            .parse()
            .unwrap();
        let mut capture = InputCapture::open_on_control(
            id,
            InputConfig {
                sample_rate: 48_000,
                channels: 2,
            },
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let InputPhase::Finished(end) = capture.phase() {
                assert_eq!(end, InputEnd::OpenFailed);
                assert!(capture.take_open_error_on_control().is_some());
                assert!(matches!(
                    capture.try_read_on_control(),
                    Err(InputEnd::OpenFailed)
                ));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "input open did not finish"
            );
            thread::yield_now();
        }
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn callback_submit_and_guard_handoff_do_not_allocate_or_free() {
        let (writer, _reader, _state) = channel(2);
        let (mut guard, mut terminal) = crate::native_callback_owner::reserve(writer);
        let samples = [0.25_f32; FRAMES * 2];
        let measured = allocation_counter::measure(|| {
            if let Some(writer) = guard.owner_mut() {
                writer.submit(&samples, 1);
            }
            guard.close();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(terminal.ready());
        drop(terminal.take());

        // A reader can be cancelled before the native callback is released.
        // The paired terminal still prevents a final queue free on callback.
        let (writer, reader, _state) = channel(1);
        let (mut guard, mut terminal) = crate::native_callback_owner::reserve(writer);
        drop(reader);
        let measured = allocation_counter::measure(|| guard.close());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(terminal.ready());
        drop(terminal.take());
    }
}
