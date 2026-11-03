//! Bounded off-callback capture and rolling spectrum analysis.
//!
//! This module consumes already-rendered stereo output. It does not own an
//! engine tap, device lifecycle, project model, or UI. The callback-facing
//! [`SpectrumCapture::submit`] path copies at most 256 frames into fixed storage
//! and attempts one nonblocking SPSC push.

use crate::spectrum::{SpectrumBallistics, SpectrumFrame, SpectrumKernel, SpectrumWindow};
use crate::spectrum_target::{MAX_SPECTRUM_TARGETS, SpectrumTargets};
mod native;
pub(crate) use native::constructor_storage_quote;
mod project;
pub(crate) use native::{
    NativeSpectrumIdentity, NativeSpectrumReceipt, spawn_native_spectrum_worker,
};
pub(crate) use project::ProjectSelectionMailbox;
pub use project::{
    MAX_PROJECT_CONTRIBUTION_FRAMES, MAX_PROJECT_LAYERS, MAX_PROJECT_PAYLOAD_BYTES,
    MAX_PROJECT_TRACKS, PROJECT_DISPLAY_STALE_AFTER, ProjectSelection, PublishedProjectSpectrum,
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    error::Error,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TryRecvError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

// Thirty-two complete visible-chain quanta absorb off-thread FFT scheduling.
pub const CAPTURE_QUEUE_CAPACITY: usize = 32 * MAX_SPECTRUM_TARGETS;
pub const MAX_CAPTURE_FRAMES: usize = 256;
pub const MAX_BATCHES_PER_POLL: usize = CAPTURE_QUEUE_CAPACITY;
pub const ANALYSIS_INTERVAL: Duration = Duration::from_nanos(33_333_334);
pub const DISPLAY_STALE_AFTER: Duration = Duration::from_millis(100);

const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(5);
const RESULT_QUEUE_CAPACITY: usize = MAX_SPECTRUM_TARGETS;
const MIN_CAPTURE_SAMPLE_RATE_HZ: u32 = 8_000;
const MAX_CAPTURE_SAMPLE_RATE_HZ: u32 = 192_000;

/// A stable backend-assigned identity for the observed output target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpectrumTargetId(pub u64);

/// Context captured with every bounded output batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpectrumStamp {
    pub target: SpectrumTargetId,
    pub generation: u64,
    pub sample_rate_hz: u32,
    pub applied_revision: u64,
}

/// Result freshness and canonical context for one analyzed output window.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishedSpectrum {
    pub stamp: SpectrumStamp,
    pub frame: SpectrumFrame,
    /// Conservative caller timestamp for the newest accepted capture batch.
    /// The CPAL adapter uses callback entry time, so it is never a predicted
    /// future playback timestamp.
    pub captured_at: Instant,
    /// Time the worker completed the transform.
    pub processed_at: Instant,
}

/// Aligned spectra from the same capture sequence and sample window. Legacy
/// output-only submissions leave input absent; no input signal is synthesized.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishedSpectra {
    pub output: PublishedSpectrum,
    pub input: Option<SpectrumFrame>,
}

impl PublishedSpectrum {
    /// Whether the represented output is recent enough to display.
    pub fn is_fresh_at(&self, now: Instant) -> bool {
        now.checked_duration_since(self.captured_at)
            .is_some_and(|age| age <= DISPLAY_STALE_AFTER)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureOutcome {
    Queued { sequence: u64 },
    QueueFull { sequence: u64 },
    InvalidBatch { sequence: u64 },
    Skipped { sequence: u64 },
    SequenceExhausted,
}

/// Capture-side producer. Construct it on control, then move it into the
/// callback owner. It must be returned to control after callback quiescence.
pub struct SpectrumCapture {
    producer: Producer<CaptureBatch>,
    next_sequences: [u64; MAX_SPECTRUM_TARGETS],
    exhausted: bool,
    native: Option<NativeSpectrumReceipt>,
    pub(crate) project: Option<project::ProjectCapture>,
}

impl SpectrumCapture {
    /// Submit one output batch without allocating, blocking, waking, or logging.
    ///
    /// An invalid or full submission still consumes its sequence number, so
    /// the next accepted batch makes the discontinuity visible to the worker.
    /// Sequence numbers never wrap; after exhaustion, create a new capture
    /// producer with a new generation.
    pub fn submit(&mut self, stamp: SpectrumStamp, frames: &[[f32; 2]]) -> CaptureOutcome {
        self.submit_at(stamp, frames, Instant::now())
    }

    /// Submit output with a caller-supplied capture time. The CPAL adapter uses
    /// callback entry time for every chunk from one invocation, avoiding a
    /// predicted timestamp that may lie ahead of fast offline rendering.
    pub fn submit_at(
        &mut self,
        stamp: SpectrumStamp,
        frames: &[[f32; 2]],
        captured_start: Instant,
    ) -> CaptureOutcome {
        self.submit_signals_at(0, stamp, None, frames, captured_start)
    }

    /// Submit aligned actual node input and output in one bounded packet.
    pub fn submit_pair(
        &mut self,
        stamp: SpectrumStamp,
        input: &[[f32; 2]],
        output: &[[f32; 2]],
    ) -> CaptureOutcome {
        self.submit_pair_at(stamp, input, output, Instant::now())
    }

    /// Paired counterpart with the caller's conservative callback timestamp.
    pub fn submit_pair_at(
        &mut self,
        stamp: SpectrumStamp,
        input: &[[f32; 2]],
        output: &[[f32; 2]],
        captured_start: Instant,
    ) -> CaptureOutcome {
        self.submit_signals_at(0, stamp, Some(input), output, captured_start)
    }

    /// Fixed membership slot from the callback's admitted target set. Each slot
    /// has its own sequence, so interleaved EQs cannot reset each other's FFT.
    pub(crate) fn submit_pair_for_slot_at(
        &mut self,
        slot: usize,
        stamp: SpectrumStamp,
        input: &[[f32; 2]],
        output: &[[f32; 2]],
        captured_start: Instant,
    ) -> CaptureOutcome {
        self.submit_signals_at(slot, stamp, Some(input), output, captured_start)
    }

    fn submit_signals_at(
        &mut self,
        slot: usize,
        stamp: SpectrumStamp,
        input: Option<&[[f32; 2]]>,
        frames: &[[f32; 2]],
        captured_start: Instant,
    ) -> CaptureOutcome {
        let sequence = match self.advance(slot) {
            Ok(sequence) => sequence,
            Err(outcome) => return outcome,
        };

        if self
            .native
            .is_some_and(|receipt| receipt.identity.sample_rate != stamp.sample_rate_hz)
            || frames.is_empty()
            || frames.len() > MAX_CAPTURE_FRAMES
            || !(MIN_CAPTURE_SAMPLE_RATE_HZ..=MAX_CAPTURE_SAMPLE_RATE_HZ)
                .contains(&stamp.sample_rate_hz)
            || frames.iter().flatten().any(|sample| !sample.is_finite())
            || input.is_some_and(|input| {
                input.len() != frames.len()
                    || input.iter().flatten().any(|sample| !sample.is_finite())
            })
        {
            return CaptureOutcome::InvalidBatch { sequence };
        }

        let mut samples = [[0.0; 2]; MAX_CAPTURE_FRAMES];
        samples[..frames.len()].copy_from_slice(frames);
        let mut input_samples = [[0.0; 2]; MAX_CAPTURE_FRAMES];
        if let Some(input) = input {
            input_samples[..input.len()].copy_from_slice(input);
        }
        let batch = CaptureBatch {
            slot: slot as u8,
            stamp,
            sequence,
            captured_start,
            frame_count: frames.len() as u16,
            samples,
            input_samples,
            paired: input.is_some(),
        };
        match self.producer.push(batch) {
            Ok(()) => CaptureOutcome::Queued { sequence },
            Err(_) => CaptureOutcome::QueueFull { sequence },
        }
    }

    /// Record an unavailable, failed or rolled-back render attempt so the next
    /// accepted capture resets the worker's rolling window at the gap.
    pub fn skip(&mut self) -> CaptureOutcome {
        self.skip_slot(0)
    }
    pub(crate) fn skip_slot(&mut self, slot: usize) -> CaptureOutcome {
        match self.advance(slot) {
            Ok(sequence) => CaptureOutcome::Skipped { sequence },
            Err(outcome) => outcome,
        }
    }
    fn advance(&mut self, slot: usize) -> Result<u64, CaptureOutcome> {
        if self.exhausted {
            return Err(CaptureOutcome::SequenceExhausted);
        }
        let Some(next) = self.next_sequences.get_mut(slot) else {
            return Err(CaptureOutcome::InvalidBatch { sequence: 0 });
        };
        let sequence = *next;
        let Some(successor) = sequence.checked_add(1) else {
            self.exhausted = true;
            return Err(CaptureOutcome::SequenceExhausted);
        };
        *next = successor;
        Ok(sequence)
    }
}

/// Control-side newest-result cache. It can be polled by the UI without
/// owning or joining the worker thread.
pub struct SpectrumReader {
    results: Receiver<PublishedSpectra>,
    latest: Option<PublishedSpectra>,
    latest_targets: Box<[Option<PublishedSpectra>]>,
    native: Option<NativeSpectrumIdentity>,
    project: Option<project::ProjectReader>,
}

impl SpectrumReader {
    pub fn project_available(&self) -> bool {
        self.project.is_some()
    }
    pub fn try_latest_project_at(
        &mut self,
        now: Instant,
        selection: ProjectSelection,
    ) -> Option<PublishedProjectSpectrum> {
        self.project.as_mut()?.latest_at(now, selection)
    }
    /// Fresh independent pairs for exactly the admitted membership. Cache slots
    /// are stable across focus/order changes; removed tokens never reappear.
    pub fn try_latest_pairs_at(
        &mut self,
        now: Instant,
        targets: &SpectrumTargets,
    ) -> Vec<PublishedSpectra> {
        let mut pairs = Vec::new();
        self.try_latest_pairs_into(now, targets, &mut pairs);
        pairs
    }

    /// Like [`Self::try_latest_pairs_at`], but replaces the contents of a
    /// caller-retained buffer. Once `out` has capacity for every target
    /// ([`MAX_SPECTRUM_TARGETS`]), a call allocates nothing.
    pub fn try_latest_pairs_into(
        &mut self,
        now: Instant,
        targets: &SpectrumTargets,
        out: &mut Vec<PublishedSpectra>,
    ) {
        out.clear();
        for _ in 0..RESULT_QUEUE_CAPACITY {
            let result = match self.results.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            };
            let slot = targets.iter().find_map(|(slot, target)| {
                (target.token == result.output.stamp.target).then_some(slot)
            });
            if let Some(entry) = slot.and_then(|slot| self.latest_targets.get_mut(slot)) {
                *entry = Some(result);
            }
        }
        for (slot, result) in self.latest_targets.iter_mut().enumerate() {
            if result.as_ref().is_some_and(|result| {
                !result.output.is_fresh_at(now)
                    || !targets.get(slot).is_some_and(|target| {
                        target.token == result.output.stamp.target
                            && target.project_generation == result.output.stamp.generation
                            && target.sample_rate_hz == result.output.stamp.sample_rate_hz
                            && result.output.stamp.applied_revision >= target.activation_floor
                    })
            }) {
                *result = None;
            }
        }
        out.extend(self.latest_targets.iter().flatten().cloned());
    }

    /// Return the newest result, clearing cached display data after 100 ms.
    pub fn try_latest(&mut self) -> Option<PublishedSpectrum> {
        self.try_latest_at(Instant::now())
    }

    pub fn try_latest_at(&mut self, now: Instant) -> Option<PublishedSpectrum> {
        self.try_latest_pair_at(now).map(|pair| pair.output)
    }

    /// Return one aligned input/output result through the same freshness cache.
    pub fn try_latest_pair(&mut self) -> Option<PublishedSpectra> {
        self.try_latest_pair_at(Instant::now())
    }

    pub fn try_latest_pair_at(&mut self, now: Instant) -> Option<PublishedSpectra> {
        for _ in 0..RESULT_QUEUE_CAPACITY {
            match self.results.try_recv() {
                Ok(result) => self.latest = Some(result),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if self
            .latest
            .as_ref()
            .is_some_and(|result| !result.output.is_fresh_at(now))
        {
            self.latest = None;
        }
        self.latest.clone()
    }
}

/// Lifecycle-side worker shutdown owner. The stream owner must first pause and
/// drop the stream, which destroys its callback-held producer, then join here.
pub struct SpectrumReaper {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    native: Option<NativeSpectrumIdentity>,
    phase: ReaperPhase,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReaperPhase {
    Ready,
    Pending,
    Complete,
    Failed,
}
impl SpectrumReaper {
    /// Request exit without joining. Safe to use while retaining a quarantined
    /// stream and callback; all destruction still waits for successful pause.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Stop and join after the callback producer has been destroyed by stream
    /// disposal. The worker consumer and queue allocation are reclaimed here.
    pub fn shutdown_and_join(mut self) -> Result<(), SpectrumWorkerError> {
        self.advance_shutdown_on_control()
    }
    /// Persistent lifecycle seam. No owner take before Pending, no ordinary
    /// mutex is held, and a worker panic keeps its exact first payload here.
    pub(crate) fn advance_shutdown_on_control(&mut self) -> Result<(), SpectrumWorkerError> {
        self.request_stop();
        match self.phase {
            ReaperPhase::Complete => return Ok(()),
            ReaperPhase::Pending | ReaperPhase::Failed => {
                return Err(SpectrumWorkerError::ThreadPanicked);
            }
            ReaperPhase::Ready => {}
        }
        self.phase = ReaperPhase::Pending;
        let Some(join) = self.join.take() else {
            return Err(SpectrumWorkerError::ThreadPanicked);
        };
        match join.join() {
            Ok(()) => {
                self.phase = ReaperPhase::Complete;
                Ok(())
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = ReaperPhase::Failed;
                Err(SpectrumWorkerError::ThreadPanicked)
            }
        }
    }
}

impl Drop for SpectrumReaper {
    fn drop(&mut self) {
        // Dropping the control handle requests worker exit but never joins it.
        // Owners needing lifecycle confirmation must call shutdown_and_join.
        self.stop.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
pub enum SpectrumWorkerError {
    InvalidSampleRate(u32),
    ThreadSpawn(std::io::Error),
    ThreadPanicked,
    StorageOverflow,
    PoolCapacity,
    Allocation,
}

impl fmt::Display for SpectrumWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate(rate) => {
                write!(formatter, "unsupported spectrum sample rate: {rate} hz")
            }
            Self::ThreadSpawn(error) => {
                write!(formatter, "could not start spectrum worker: {error}")
            }
            Self::PoolCapacity => formatter.write_str("spectrum packet pool capacity refused"),
            Self::StorageOverflow => formatter.write_str("spectrum storage size overflow"),
            Self::Allocation => formatter.write_str("could not reserve spectrum storage"),
            Self::ThreadPanicked => formatter.write_str("spectrum worker exited unexpectedly"),
        }
    }
}

impl Error for SpectrumWorkerError {}

/// Construct the bounded capture queue and its worker on a control thread.
pub fn spawn_spectrum_worker(
    initial_stamp: SpectrumStamp,
) -> Result<(SpectrumCapture, SpectrumReader, SpectrumReaper), SpectrumWorkerError> {
    spawn_worker(initial_stamp, false, None)
}
fn spawn_worker(
    initial_stamp: SpectrumStamp,
    fixed_native: bool,
    entry: Option<fn()>,
) -> Result<(SpectrumCapture, SpectrumReader, SpectrumReaper), SpectrumWorkerError> {
    let count = if fixed_native {
        MAX_SPECTRUM_TARGETS
    } else {
        1
    };
    let mut cores = Vec::new();
    cores
        .try_reserve_exact(count)
        .map_err(|_| SpectrumWorkerError::Allocation)?;
    for _ in 0..count {
        cores.push(SpectrumCore::for_rate(initial_stamp.sample_rate_hz)?);
    }
    let cores = cores.into_boxed_slice();
    let (project_capture, project_reader, project_worker) = if fixed_native {
        let (capture, reader, worker) = project::channel(initial_stamp.sample_rate_hz)?;
        (Some(capture), Some(reader), Some(worker))
    } else {
        (None, None, None)
    };
    let mut latest_targets = Vec::new();
    latest_targets
        .try_reserve_exact(MAX_SPECTRUM_TARGETS)
        .map_err(|_| SpectrumWorkerError::Allocation)?;
    latest_targets.resize_with(MAX_SPECTRUM_TARGETS, || None);
    let latest_targets = latest_targets.into_boxed_slice();
    let (producer, consumer) = RingBuffer::new(CAPTURE_QUEUE_CAPACITY);
    let (result_sender, result_receiver) = sync_channel(RESULT_QUEUE_CAPACITY);
    let stop = Arc::new(AtomicBool::new(false));
    let receipt = if fixed_native {
        Some(
            NativeSpectrumReceipt::new(
                &producer,
                &stop,
                &cores,
                &latest_targets,
                initial_stamp.sample_rate_hz,
                project_capture.as_ref(),
                project_worker.as_ref(),
            )
            .ok_or(SpectrumWorkerError::StorageOverflow)?,
        )
    } else {
        None
    };
    let worker_stop = Arc::clone(&stop);
    let join = thread::Builder::new()
        .name("hex-spectrum".into())
        .spawn(move || {
            if let Some(entry) = entry {
                entry();
            }
            worker_loop(consumer, result_sender, worker_stop, cores, project_worker)
        })
        .map_err(SpectrumWorkerError::ThreadSpawn)?;

    Ok((
        SpectrumCapture {
            producer,
            next_sequences: [0; MAX_SPECTRUM_TARGETS],
            exhausted: false,
            native: receipt,
            project: project_capture,
        },
        SpectrumReader {
            results: result_receiver,
            latest: None,
            latest_targets,
            native: receipt.map(|receipt| receipt.identity),
            project: project_reader,
        },
        SpectrumReaper {
            stop,
            join: Some(join),
            native: receipt.map(|receipt| receipt.identity),
            phase: ReaperPhase::Ready,
            first_panic: None,
        },
    ))
}

#[derive(Clone, Copy)]
struct CaptureBatch {
    slot: u8,
    stamp: SpectrumStamp,
    sequence: u64,
    captured_start: Instant,
    frame_count: u16,
    samples: [[f32; 2]; MAX_CAPTURE_FRAMES],
    input_samples: [[f32; 2]; MAX_CAPTURE_FRAMES],
    paired: bool,
}

fn worker_loop(
    mut consumer: Consumer<CaptureBatch>,
    result_sender: SyncSender<PublishedSpectra>,
    stop: Arc<AtomicBool>,
    mut cores: Box<[SpectrumCore]>,
    mut project: Option<project::ProjectWorker>,
) {
    while !stop.load(Ordering::Acquire) {
        let mut drained = 0;
        while drained < MAX_BATCHES_PER_POLL {
            let Ok(batch) = consumer.pop() else {
                break;
            };
            if let Some(core) = cores.get_mut(usize::from(batch.slot)) {
                core.ingest(batch);
            }
            drained += 1;
        }
        let now = Instant::now();
        for core in &mut cores {
            if let Some(result) = core.analyze_pair_if_due(now) {
                let _ = result_sender.try_send(result);
            }
        }
        if let Some(project) = project.as_mut() {
            drained += project.poll(now);
        }
        if drained == 0 {
            thread::sleep(IDLE_POLL_INTERVAL);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StreamContext {
    target: SpectrumTargetId,
    generation: u64,
    sample_rate_hz: u32,
    paired: bool,
}

struct SpectrumCore {
    context: Option<StreamContext>,
    kernel: Option<SpectrumKernel>,
    kernel_rate: Option<u32>,
    output_window: SpectrumWindow,
    input_window: SpectrumWindow,
    last_sequence: Option<u64>,
    latest_stamp: Option<SpectrumStamp>,
    captured_at: Option<Instant>,
    last_analysis_at: Option<Instant>,
    output_ballistics: SpectrumBallistics,
    input_ballistics: SpectrumBallistics,
    envelope_at: Option<Instant>,
    dirty_since_analysis: bool,
}

impl SpectrumCore {
    fn new(
        kernel: SpectrumKernel,
        output_window: SpectrumWindow,
        input_window: SpectrumWindow,
    ) -> Self {
        Self {
            context: None,
            kernel_rate: Some(kernel.sample_rate_hz()),
            kernel: Some(kernel),
            output_window,
            input_window,
            last_sequence: None,
            latest_stamp: None,
            captured_at: None,
            last_analysis_at: None,
            output_ballistics: SpectrumBallistics::default(),
            input_ballistics: SpectrumBallistics::default(),
            envelope_at: None,
            dirty_since_analysis: false,
        }
    }

    /// Fallible control-side construction of one core and its fixed windows.
    fn for_rate(sample_rate_hz: u32) -> Result<Self, SpectrumWorkerError> {
        let invalid = |_| SpectrumWorkerError::InvalidSampleRate(sample_rate_hz);
        Ok(Self::new(
            SpectrumKernel::new(sample_rate_hz).map_err(invalid)?,
            SpectrumWindow::new(sample_rate_hz).map_err(invalid)?,
            SpectrumWindow::new(sample_rate_hz).map_err(invalid)?,
        ))
    }

    fn ingest(&mut self, batch: CaptureBatch) -> IngestOutcome {
        let frame_count = usize::from(batch.frame_count);
        if frame_count == 0
            || frame_count > MAX_CAPTURE_FRAMES
            || batch.samples[..frame_count]
                .iter()
                .flatten()
                .any(|sample| !sample.is_finite())
            || (batch.paired
                && batch.input_samples[..frame_count]
                    .iter()
                    .flatten()
                    .any(|sample| !sample.is_finite()))
        {
            self.clear_window();
            self.last_sequence = Some(batch.sequence);
            return IngestOutcome::Rejected;
        }

        let next_context = StreamContext {
            target: batch.stamp.target,
            generation: batch.stamp.generation,
            sample_rate_hz: batch.stamp.sample_rate_hz,
            paired: batch.paired,
        };
        let context_changed = self.context.is_some_and(|context| context != next_context);
        let sequence_gap = self
            .last_sequence
            .and_then(|sequence| sequence.checked_add(1))
            .is_some_and(|expected| expected != batch.sequence);
        let needs_kernel = self.kernel_rate != Some(batch.stamp.sample_rate_hz);

        if context_changed || sequence_gap {
            self.clear_window();
        }
        if needs_kernel {
            self.clear_window();
            // Rate changes are rare control events; windows keep their storage.
            let rate = batch.stamp.sample_rate_hz;
            self.kernel = SpectrumKernel::new(rate).ok().filter(|_| {
                self.output_window.reconfigure(rate).is_ok()
                    && self.input_window.reconfigure(rate).is_ok()
            });
            self.kernel_rate = self.kernel.as_ref().map(|_| rate);
        }
        if self.kernel.is_none() {
            self.context = None;
            self.last_sequence = Some(batch.sequence);
            return IngestOutcome::Rejected;
        }

        self.context = Some(next_context);
        self.last_sequence = Some(batch.sequence);
        self.latest_stamp = Some(batch.stamp);
        self.captured_at = Some(batch.captured_start);
        // Finiteness was checked above, so neither push can refuse.
        let _ = self.output_window.push(&batch.samples[..frame_count]);
        if batch.paired {
            let _ = self.input_window.push(&batch.input_samples[..frame_count]);
        }
        self.dirty_since_analysis = true;

        if sequence_gap || context_changed {
            IngestOutcome::Reset
        } else {
            IngestOutcome::Accepted
        }
    }

    #[cfg(test)]
    fn analyze_if_due(&mut self, now: Instant) -> Option<PublishedSpectrum> {
        self.analyze_pair_if_due(now).map(|pair| pair.output)
    }

    fn analyze_pair_if_due(&mut self, now: Instant) -> Option<PublishedSpectra> {
        if !self.output_window.is_ready()
            || !self.dirty_since_analysis
            || self
                .last_analysis_at
                .is_some_and(|last| now.saturating_duration_since(last) < ANALYSIS_INTERVAL)
        {
            return None;
        }
        let kernel = self.kernel.as_mut()?;
        let mut frame = kernel.analyze(&self.output_window).ok()?;
        let mut input = if self.context?.paired {
            Some(kernel.analyze(&self.input_window).ok()?)
        } else {
            None
        };
        // Ballistics advance by actual elapsed analysis time, so the display
        // is independent of worker cadence. Captures still own freshness.
        let elapsed = self.envelope_at.map_or(Duration::ZERO, |previous| {
            now.saturating_duration_since(previous)
        });
        self.output_ballistics.apply(&mut frame, elapsed);
        if let Some(input) = input.as_mut() {
            self.input_ballistics.apply(input, elapsed);
        }
        self.envelope_at = Some(now);
        let processed_at = Instant::now();
        let result = PublishedSpectrum {
            stamp: self.latest_stamp?,
            frame,
            captured_at: self.captured_at?,
            processed_at,
        };
        self.last_analysis_at = Some(now);
        self.dirty_since_analysis = false;
        Some(PublishedSpectra {
            output: result,
            input,
        })
    }

    fn clear_window(&mut self) {
        self.output_window.clear();
        self.input_window.clear();
        self.latest_stamp = None;
        self.captured_at = None;
        self.output_ballistics.reset();
        self.input_ballistics.reset();
        self.envelope_at = None;
        self.dirty_since_analysis = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IngestOutcome {
    Accepted,
    Reset,
    Rejected,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_worker_keeps_interleaved_eq_windows_and_reader_membership_independent() {
        use crate::spectrum_target::{SpectrumTargetRequest, channel};
        let (mut control, _callback) = channel();
        let requests = [10, 20].map(|id| SpectrumTargetRequest {
            identity: hex_engine::PostNodeIdentity::new(
                hex_engine::ProcessorChainOwner::Track(1),
                id,
                hex_engine::DeviceTypeId::from_static("hex.eq"),
            ),
            project_generation: 7,
            activation_floor: 3,
            sample_rate_hz: 48_000,
        });
        control.set_targets(&requests).unwrap();
        let targets = *control.desired_targets();
        let (mut capture, mut reader, reaper) =
            spawn_native_spectrum_worker(stamp(0, 0, 48_000, 0)).unwrap();
        let now = Instant::now();
        for sequence in 0..16 {
            for (slot, target) in targets.iter() {
                let level = if target.identity.device_id == 10 {
                    0.125
                } else {
                    0.5
                };
                assert_eq!(
                    capture.submit_pair_for_slot_at(
                        slot,
                        SpectrumStamp {
                            target: target.token,
                            generation: 7,
                            sample_rate_hz: 48_000,
                            applied_revision: 3,
                        },
                        &[[level * 2.0; 2]; 256],
                        &[[level; 2]; 256],
                        now
                    ),
                    CaptureOutcome::Queued { sequence }
                );
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let pairs = loop {
            let pairs = reader.try_latest_pairs_at(now, &targets);
            if pairs.len() == 2 {
                break pairs;
            }
            assert!(
                Instant::now() < deadline,
                "both actual rolling FFTs must publish"
            );
            std::thread::yield_now();
        };
        let peak = |frame: &SpectrumFrame| {
            crate::spectrum::without_default_slope(frame)
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
        };
        assert!(
            (peak(&pairs[1].output.frame) - peak(&pairs[0].output.frame) - 12.0412).abs() < 0.05
        );
        for pair in &pairs {
            assert!(
                (peak(pair.input.as_ref().unwrap()) - peak(&pair.output.frame) - 6.0206).abs()
                    < 0.05
            );
        }
        // A caller-retained buffer reproduces the same pairs without allocating
        // once it holds every target; the Vec-returning call allocates each time.
        let mut retained_buffer = Vec::with_capacity(MAX_SPECTRUM_TARGETS);
        #[cfg(feature = "allocation-probe")]
        {
            let fresh = allocation_counter::measure(|| {
                assert_eq!(reader.try_latest_pairs_at(now, &targets).len(), 2);
            });
            // Each Vec-returning call grows a fresh buffer (two allocations for
            // these two large pairs with the current std growth policy).
            assert!(fresh.count_total >= 1 && fresh.count_current == 0);
            let reused = allocation_counter::measure(|| {
                reader.try_latest_pairs_into(now, &targets, &mut retained_buffer);
            });
            assert_eq!(
                (
                    reused.count_total,
                    reused.bytes_total,
                    reused.count_current,
                    reused.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        reader.try_latest_pairs_into(now, &targets, &mut retained_buffer);
        assert_eq!(retained_buffer, pairs);
        control.set_targets(&requests[1..]).unwrap();
        let retained = reader.try_latest_pairs_at(now, control.desired_targets());
        assert_eq!(retained.len(), 1);
        assert_eq!(
            retained[0].output.stamp.target,
            targets.get(1).unwrap().token
        );
        assert!(
            reader
                .try_latest_pairs_at(
                    now + DISPLAY_STALE_AFTER + Duration::from_nanos(1),
                    control.desired_targets()
                )
                .is_empty()
        );
        drop(capture);
        reaper.shutdown_and_join().unwrap();
    }

    #[test]
    fn paired_release_uses_shared_elapsed_time_and_gap_clears_both_envelopes() {
        let context = stamp(3, 7, 48_000, 11);
        let mut analysis = SpectrumCore::for_rate(48_000).unwrap();
        let now = Instant::now();
        for sequence in 0..16 {
            let mut capture = batch(sequence, context, 0.5, 256);
            capture.paired = true;
            capture.input_samples.fill([0.25; 2]);
            capture.captured_start = now;
            analysis.ingest(capture);
        }
        let first = analysis.analyze_pair_if_due(now).unwrap();
        let later = now + Duration::from_millis(100);
        for sequence in 16..32 {
            let mut capture = batch(sequence, context, 0.0, 256);
            capture.paired = true;
            capture.captured_start = later;
            analysis.ingest(capture);
        }
        let falling = analysis.analyze_pair_if_due(later).unwrap();
        for (old, new) in [
            (&first.output.frame, &falling.output.frame),
            (
                first.input.as_ref().unwrap(),
                falling.input.as_ref().unwrap(),
            ),
        ] {
            // 100 ms of release toward the silent floor, then a held peak.
            let release = 1.0 - (-0.1f32 / 0.25).exp();
            for (index, (before, after)) in
                old.bins_dbfs.iter().zip(new.bins_dbfs.iter()).enumerate()
            {
                let floor = crate::spectrum::FLOOR_DBFS;
                assert!((after - (before + (floor - before) * release)).abs() < 0.01);
                assert_eq!(new.peak_dbfs[index], old.peak_dbfs[index]);
            }
        }
        assert!(
            analysis
                .analyze_pair_if_due(later + ANALYSIS_INTERVAL)
                .is_none(),
            "decay cannot refresh data without captures"
        );
        for sequence in 33..49 {
            let mut capture = batch(sequence, context, 0.0, 256);
            capture.paired = true;
            analysis.ingest(capture);
        }
        let reset = analysis
            .analyze_pair_if_due(later + ANALYSIS_INTERVAL)
            .unwrap();
        assert!(
            reset
                .output
                .frame
                .bins_dbfs
                .iter()
                .all(|db| *db == crate::spectrum::FLOOR_DBFS)
        );
        assert!(
            reset
                .input
                .unwrap()
                .bins_dbfs
                .iter()
                .all(|db| *db == crate::spectrum::FLOOR_DBFS)
        );
    }

    #[test]
    fn paired_windows_keep_real_levels_stereo_energy_and_reset_together() {
        let context = stamp(3, 7, 48_000, 11);
        let mut analysis = SpectrumCore::for_rate(48_000).unwrap();
        let now = Instant::now();
        for sequence in 0..16 {
            let mut capture = batch(sequence, context, 0.0, 256);
            capture.paired = true;
            capture.captured_start = now;
            for i in 0..256 {
                let position = sequence as usize * 256 + i;
                let signal = (std::f32::consts::TAU * 32.0 * position as f32
                    / crate::spectrum::FFT_SIZE as f32)
                    .sin()
                    * 0.5;
                capture.input_samples[i] = [signal, -signal];
                capture.samples[i] = [signal * 0.25, -signal * 0.25];
            }
            analysis.ingest(capture);
        }
        let pair = analysis.analyze_pair_if_due(now).unwrap();
        let peak = |frame: &SpectrumFrame| {
            crate::spectrum::without_default_slope(frame)
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
        };
        let input_peak = peak(pair.input.as_ref().unwrap());
        let output_peak = peak(&pair.output.frame);
        assert!(
            (input_peak + 6.0206).abs() < 0.01,
            "anti-phase input retains stereo energy"
        );
        assert!((input_peak - output_peak - 12.0412).abs() < 0.01);
        assert_eq!(pair.output.stamp, context);
        assert_eq!(pair.output.captured_at, now);

        // A missing capture resets both phases, never joining samples across a gap.
        let mut after_gap = batch(17, context, 0.0, 256);
        after_gap.paired = true;
        assert_eq!(analysis.ingest(after_gap), IngestOutcome::Reset);
        assert_eq!(analysis.output_window.filled(), 256);
        assert!(
            analysis
                .analyze_pair_if_due(now + ANALYSIS_INTERVAL)
                .is_none()
        );
        for sequence in 18..33 {
            let mut silence = batch(sequence, context, 0.0, 256);
            silence.paired = true;
            analysis.ingest(silence);
        }
        let silent = analysis
            .analyze_pair_if_due(now + ANALYSIS_INTERVAL)
            .unwrap();
        assert_eq!(
            peak(silent.input.as_ref().unwrap()),
            crate::spectrum::FLOOR_DBFS
        );
        assert_eq!(peak(&silent.output.frame), crate::spectrum::FLOOR_DBFS);

        // Legacy output-only input cannot inherit the previous paired input.
        assert_eq!(
            analysis.ingest(batch(33, context, 0.2, 256)),
            IngestOutcome::Reset
        );
        assert_eq!(analysis.output_window.filled(), 256);
        for sequence in 34..49 {
            analysis.ingest(batch(sequence, context, 0.2, 256));
        }
        assert!(
            analysis
                .analyze_pair_if_due(now + ANALYSIS_INTERVAL * 2)
                .unwrap()
                .input
                .is_none()
        );
    }

    #[test]
    fn paired_capture_refuses_mismatched_or_nonfinite_input_and_preserves_sequence_gaps() {
        let (producer, mut consumer) = RingBuffer::new(CAPTURE_QUEUE_CAPACITY);
        let mut capture = SpectrumCapture {
            producer,
            next_sequences: [0; MAX_SPECTRUM_TARGETS],
            exhausted: false,
            native: None,
            project: None,
        };
        let context = stamp(1, 2, 48_000, 3);
        assert_eq!(
            capture.submit_pair(context, &[[0.5; 2]], &[[0.1; 2]; 2]),
            CaptureOutcome::InvalidBatch { sequence: 0 }
        );
        assert_eq!(
            capture.submit_pair(context, &[[f32::NAN; 2]], &[[0.1; 2]]),
            CaptureOutcome::InvalidBatch { sequence: 1 }
        );
        assert!(consumer.pop().is_err());
        let mut submit = || {
            assert_eq!(
                capture.submit_pair(context, &[[0.5, -0.5]; 256], &[[0.1, -0.1]; 256]),
                CaptureOutcome::Queued { sequence: 2 }
            )
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(&mut submit);
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            submit();
        }
        let queued = consumer.pop().unwrap();
        assert_eq!(queued.sequence, 2);
        assert!(queued.paired);
        assert_eq!(queued.input_samples, [[0.5, -0.5]; 256]);
        assert_eq!(queued.samples, [[0.1, -0.1]; 256]);
    }

    fn stamp(target: u64, generation: u64, rate: u32, revision: u64) -> SpectrumStamp {
        SpectrumStamp {
            target: SpectrumTargetId(target),
            generation,
            sample_rate_hz: rate,
            applied_revision: revision,
        }
    }

    fn batch(sequence: u64, stamp: SpectrumStamp, value: f32, count: usize) -> CaptureBatch {
        let mut samples = [[0.0; 2]; MAX_CAPTURE_FRAMES];
        samples[..count].fill([value, value]);
        CaptureBatch {
            slot: 0,
            stamp,
            sequence,
            captured_start: Instant::now(),
            frame_count: count as u16,
            samples,
            input_samples: [[0.0; 2]; MAX_CAPTURE_FRAMES],
            paired: false,
        }
    }

    fn core() -> SpectrumCore {
        SpectrumCore::for_rate(8_000).unwrap()
    }

    #[test]
    fn full_queue_drops_only_capture_and_next_sequence_exposes_gap() {
        let (producer, mut consumer) = RingBuffer::new(CAPTURE_QUEUE_CAPACITY);
        let mut capture = SpectrumCapture {
            producer,
            next_sequences: [0; MAX_SPECTRUM_TARGETS],
            exhausted: false,
            native: None,
            project: None,
        };
        let frames = [[0.25; 2]; MAX_CAPTURE_FRAMES];
        let context = stamp(1, 2, 8_000, 10);
        for sequence in 0..CAPTURE_QUEUE_CAPACITY as u64 {
            assert_eq!(
                capture.submit(context, &frames),
                CaptureOutcome::Queued { sequence }
            );
        }
        assert_eq!(
            capture.submit(context, &frames),
            CaptureOutcome::QueueFull {
                sequence: CAPTURE_QUEUE_CAPACITY as u64
            }
        );
        let first = consumer.pop().expect("queued batch");
        let mut analysis = core();
        assert_eq!(analysis.ingest(first), IngestOutcome::Accepted);
        while consumer.pop().is_ok() {}
        assert_eq!(
            capture.submit(context, &frames),
            CaptureOutcome::Queued {
                sequence: CAPTURE_QUEUE_CAPACITY as u64 + 1
            }
        );
        let next = consumer.pop().expect("batch after dropped sequence");
        assert_eq!(analysis.ingest(next), IngestOutcome::Reset);
        assert_eq!(analysis.output_window.filled(), MAX_CAPTURE_FRAMES);
    }

    #[test]
    fn ordinary_revisions_continue_but_target_generation_and_rate_reset_window() {
        let mut analysis = core();
        assert_eq!(
            analysis.ingest(batch(0, stamp(1, 1, 8_000, 1), 0.1, 256)),
            IngestOutcome::Accepted
        );
        assert_eq!(
            analysis.ingest(batch(1, stamp(1, 1, 8_000, 2), 0.2, 256)),
            IngestOutcome::Accepted
        );
        assert_eq!(analysis.output_window.filled(), 512);
        assert_eq!(
            analysis.ingest(batch(2, stamp(1, 2, 8_000, 2), 0.2, 256)),
            IngestOutcome::Reset
        );
        assert_eq!(analysis.output_window.filled(), 256);
        assert_eq!(
            analysis.ingest(batch(3, stamp(2, 2, 8_000, 3), 0.2, 256)),
            IngestOutcome::Reset
        );
        assert_eq!(analysis.output_window.filled(), 256);
        assert_eq!(
            analysis.ingest(batch(4, stamp(2, 2, 48_000, 4), 0.2, 256)),
            IngestOutcome::Reset
        );
        assert_eq!(analysis.output_window.filled(), 256);
        assert_eq!(analysis.kernel_rate, Some(48_000));
    }

    #[test]
    fn cadence_freshness_and_window_publication_are_deterministic() {
        let mut analysis = core();
        let start = Instant::now();
        for sequence in 0..32 {
            // 128 frames at 8 khz model approximately 60 hz parameter changes
            // throughout one rolling window, with caller capture timestamps.
            let mut capture = batch(sequence, stamp(3, 1, 8_000, sequence + 1), 0.5, 128);
            capture.captured_start = start - Duration::from_millis((31 - sequence) * 16);
            analysis.ingest(capture);
        }
        let first = analysis.analyze_if_due(start).expect("full initial window");
        assert_eq!(first.frame.bins_dbfs.len(), crate::spectrum::SPECTRUM_BANDS);
        assert_eq!(first.stamp.applied_revision, 32);
        assert!(first.is_fresh_at(first.captured_at + DISPLAY_STALE_AFTER));
        assert!(
            !first.is_fresh_at(first.captured_at + DISPLAY_STALE_AFTER + Duration::from_nanos(1))
        );
        analysis.ingest(batch(32, stamp(3, 1, 8_000, 33), 0.25, 128));
        assert!(
            analysis
                .analyze_if_due(start + Duration::from_millis(33))
                .is_none()
        );
        let next = analysis
            .analyze_if_due(start + Duration::from_millis(34))
            .expect("cadence elapsed");
        assert_eq!(next.stamp.applied_revision, 33);
    }

    #[test]
    fn reader_drops_worker_result_by_capture_age_not_processing_or_receipt_time() {
        let mut analysis = core();
        let now = Instant::now();
        let stale_capture = now - DISPLAY_STALE_AFTER - Duration::from_millis(1);
        let context = stamp(5, 2, 8_000, 40);
        for sequence in 0..16 {
            let mut captured = batch(sequence, context, 0.4, 256);
            captured.captured_start = stale_capture;
            analysis.ingest(captured);
        }
        let result = analysis.analyze_if_due(now).expect("complete fft window");
        assert!(result.processed_at >= now);
        assert!(!result.is_fresh_at(now));

        let (sender, receiver) = sync_channel(RESULT_QUEUE_CAPACITY);
        sender
            .try_send(PublishedSpectra {
                output: result,
                input: None,
            })
            .expect("publish worker result");
        let mut reader = SpectrumReader {
            results: receiver,
            latest: None,
            latest_targets: vec![None; MAX_SPECTRUM_TARGETS].into_boxed_slice(),
            native: None,
            project: None,
        };
        // Receipt happens now, but the channel carries the original capture
        // timestamp so expensive/queued processing cannot refresh stale audio.
        assert!(reader.try_latest_at(now).is_none());
    }

    #[test]
    fn context_reset_keeps_global_cadence_and_idle_windows_are_not_republished() {
        let mut analysis = SpectrumCore::for_rate(192_000).unwrap();
        let start = Instant::now();
        let first_context = stamp(4, 1, 192_000, 1);
        for sequence in 0..16 {
            let mut captured = batch(sequence, first_context, 0.3, 256);
            captured.captured_start = start;
            analysis.ingest(captured);
        }
        assert!(analysis.analyze_if_due(start).is_some());

        let next_context = stamp(4, 2, 192_000, 2);
        let next_capture_at = start + Duration::from_millis(21);
        for sequence in 16..32 {
            let mut captured = batch(sequence, next_context, 0.4, 256);
            captured.captured_start = next_capture_at;
            analysis.ingest(captured);
        }
        assert!(analysis.analyze_if_due(next_capture_at).is_none());
        let second = analysis
            .analyze_if_due(start + Duration::from_millis(34))
            .expect("global cadence elapsed");
        assert_eq!(second.stamp, next_context);
        assert_eq!(second.captured_at, next_capture_at);
        assert!(second.is_fresh_at(next_capture_at + DISPLAY_STALE_AFTER));
        assert!(
            !second.is_fresh_at(next_capture_at + DISPLAY_STALE_AFTER + Duration::from_nanos(1))
        );
        assert!(!second.is_fresh_at(start + Duration::from_millis(122)));
        assert!(
            analysis
                .analyze_if_due(start + Duration::from_millis(200))
                .is_none()
        );
    }

    #[test]
    fn delayed_capture_is_fresh_by_capture_age_not_fixture_start() {
        let mut analysis = core();
        let start = Instant::now();
        let captured_at = start + Duration::from_millis(40);
        let context = stamp(4, 2, 8_000, 2);
        for sequence in 0..16 {
            let mut captured = batch(sequence, context, 0.4, 256);
            captured.captured_start = captured_at;
            analysis.ingest(captured);
        }
        let result = analysis
            .analyze_if_due(start + Duration::from_millis(54))
            .expect("complete delayed capture window");
        assert_eq!(result.captured_at, captured_at);
        // The old absolute +122 ms query is only 82 ms after this capture.
        let query_at = start + Duration::from_millis(122);
        assert!(query_at.duration_since(start) > DISPLAY_STALE_AFTER);
        assert_eq!(
            query_at.duration_since(result.captured_at),
            Duration::from_millis(82)
        );
        assert!(result.is_fresh_at(query_at));
        assert!(result.is_fresh_at(captured_at + DISPLAY_STALE_AFTER));
        assert!(!result.is_fresh_at(captured_at + DISPLAY_STALE_AFTER + Duration::from_nanos(1)));
    }

    #[test]
    fn malformed_batches_clear_window_without_partial_ingest() {
        let mut analysis = core();
        assert_eq!(
            analysis.ingest(batch(0, stamp(1, 1, 8_000, 1), 0.5, 256)),
            IngestOutcome::Accepted
        );
        let mut invalid = batch(1, stamp(1, 1, 8_000, 1), 0.5, 256);
        invalid.frame_count = (MAX_CAPTURE_FRAMES + 1) as u16;
        assert_eq!(analysis.ingest(invalid), IngestOutcome::Rejected);
        assert_eq!(analysis.output_window.filled(), 0);
        let bad_rate = batch(2, stamp(1, 1, 7_999, 1), 0.5, 256);
        assert_eq!(analysis.ingest(bad_rate), IngestOutcome::Rejected);
        assert_eq!(analysis.output_window.filled(), 0);
        assert_eq!(analysis.context, None);
    }

    #[test]
    fn worker_lifecycle_returns_capture_to_control_before_join() {
        let context = stamp(7, 1, 8_000, 0);
        let (mut capture, _reader, reaper) = spawn_spectrum_worker(context).expect("valid worker");
        assert_eq!(
            capture.submit(context, &[[0.0, 0.0]; 128]),
            CaptureOutcome::Queued { sequence: 0 }
        );
        drop(capture);
        reaper
            .shutdown_and_join()
            .expect("worker joined after producer drop");
    }

    #[test]
    fn unsupported_initial_rate_is_rejected_before_worker_creation() {
        assert!(matches!(
            spawn_spectrum_worker(stamp(1, 1, 7_999, 0)),
            Err(SpectrumWorkerError::InvalidSampleRate(7_999))
        ));
    }

    #[test]
    fn sequence_exhaustion_requires_a_new_capture_generation() {
        let (producer, _consumer) = RingBuffer::new(CAPTURE_QUEUE_CAPACITY);
        let mut capture = SpectrumCapture {
            producer,
            next_sequences: [u64::MAX; MAX_SPECTRUM_TARGETS],
            exhausted: false,
            native: None,
            project: None,
        };
        assert_eq!(
            capture.submit(stamp(1, 2, 8_000, 1), &[[0.0, 0.0]]),
            CaptureOutcome::SequenceExhausted
        );
        assert_eq!(
            capture.submit(stamp(1, 3, 8_000, 2), &[[0.0, 0.0]]),
            CaptureOutcome::SequenceExhausted
        );
    }

    #[test]
    fn invalid_capture_is_refused_and_consumes_a_sequence() {
        let (producer, mut consumer) = RingBuffer::new(CAPTURE_QUEUE_CAPACITY);
        let mut capture = SpectrumCapture {
            producer,
            next_sequences: [0; MAX_SPECTRUM_TARGETS],
            exhausted: false,
            native: None,
            project: None,
        };
        let context = stamp(1, 1, 8_000, 1);
        assert_eq!(
            capture.submit(context, &[[f32::NAN, 0.0]]),
            CaptureOutcome::InvalidBatch { sequence: 0 }
        );
        assert_eq!(
            capture.submit(stamp(1, 1, 7_999, 1), &[[0.0, 0.0]]),
            CaptureOutcome::InvalidBatch { sequence: 1 }
        );
        assert!(consumer.pop().is_err());
        assert_eq!(
            capture.submit(context, &[[0.1, 0.2]]),
            CaptureOutcome::Queued { sequence: 2 }
        );
        let queued = consumer.pop().expect("valid capture after malformed input");
        assert_eq!(queued.sequence, 2);
    }
}
