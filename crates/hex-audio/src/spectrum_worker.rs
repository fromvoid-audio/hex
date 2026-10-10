//! Bounded off-callback capture and rolling spectrum analysis.
//!
//! This module consumes already-rendered stereo output. It does not own an
//! engine tap, device lifecycle, project model, or UI. The callback-facing
//! [`SpectrumCapture::submit`] path copies at most 256 frames into fixed storage
//! and attempts one nonblocking SPSC push.

use crate::spectrum::{FFT_SIZE, SpectrumFrame, SpectrumKernel};
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

pub const CAPTURE_QUEUE_CAPACITY: usize = 32;
pub const MAX_CAPTURE_FRAMES: usize = 256;
pub const MAX_BATCHES_PER_POLL: usize = 32;
pub const ANALYSIS_INTERVAL: Duration = Duration::from_nanos(33_333_334);
pub const DISPLAY_STALE_AFTER: Duration = Duration::from_millis(100);

const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(5);
const RESULT_QUEUE_CAPACITY: usize = 1;
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
    /// Worker receive time of the newest frame represented by this window.
    pub captured_at: Instant,
    /// Time the worker completed the transform.
    pub processed_at: Instant,
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
    SequenceExhausted,
}

/// Capture-side producer. Construct it on control, then move it into the
/// callback owner. It must be returned to control after callback quiescence.
pub struct SpectrumCapture {
    producer: Producer<CaptureBatch>,
    next_sequence: u64,
    exhausted: bool,
}

impl SpectrumCapture {
    /// Submit one output batch without allocating, blocking, waking, or logging.
    ///
    /// An invalid or full submission still consumes its sequence number, so
    /// the next accepted batch makes the discontinuity visible to the worker.
    /// Sequence numbers never wrap; after exhaustion, create a new capture
    /// producer with a new generation.
    pub fn submit(&mut self, stamp: SpectrumStamp, frames: &[[f32; 2]]) -> CaptureOutcome {
        if self.exhausted {
            return CaptureOutcome::SequenceExhausted;
        }
        let sequence = self.next_sequence;
        let Some(next_sequence) = sequence.checked_add(1) else {
            self.exhausted = true;
            return CaptureOutcome::SequenceExhausted;
        };
        self.next_sequence = next_sequence;

        if frames.is_empty()
            || frames.len() > MAX_CAPTURE_FRAMES
            || !(MIN_CAPTURE_SAMPLE_RATE_HZ..=MAX_CAPTURE_SAMPLE_RATE_HZ)
                .contains(&stamp.sample_rate_hz)
            || frames.iter().flatten().any(|sample| !sample.is_finite())
        {
            return CaptureOutcome::InvalidBatch { sequence };
        }

        let mut samples = [[0.0; 2]; MAX_CAPTURE_FRAMES];
        samples[..frames.len()].copy_from_slice(frames);
        let batch = CaptureBatch {
            stamp,
            sequence,
            frame_count: frames.len() as u16,
            samples,
        };
        match self.producer.push(batch) {
            Ok(()) => CaptureOutcome::Queued { sequence },
            Err(_) => CaptureOutcome::QueueFull { sequence },
        }
    }
}

/// Control-side worker owner and newest-result cache.
pub struct SpectrumWorker {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    results: Receiver<PublishedSpectrum>,
    latest: Option<PublishedSpectrum>,
}

impl SpectrumWorker {
    /// Return the newest result, clearing cached display data after 100 ms.
    pub fn try_latest(&mut self) -> Option<PublishedSpectrum> {
        for _ in 0..RESULT_QUEUE_CAPACITY {
            match self.results.try_recv() {
                Ok(result) => self.latest = Some(result),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let now = Instant::now();
        if self
            .latest
            .as_ref()
            .is_some_and(|result| !result.is_fresh_at(now))
        {
            self.latest = None;
        }
        self.latest.clone()
    }

    /// Stop and join on a control thread after the callback has quiesced and
    /// returned its capture producer. Consuming the producer ensures its queue
    /// storage is reclaimed here, never in an audio callback destructor.
    pub fn shutdown_and_join(
        mut self,
        capture: SpectrumCapture,
    ) -> Result<(), SpectrumWorkerError> {
        drop(capture);
        self.stop.store(true, Ordering::Release);
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        join.join().map_err(|_| SpectrumWorkerError::ThreadPanicked)
    }
}

impl Drop for SpectrumWorker {
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
            Self::ThreadPanicked => formatter.write_str("spectrum worker exited unexpectedly"),
        }
    }
}

impl Error for SpectrumWorkerError {}

/// Construct the bounded capture queue and its worker on a control thread.
pub fn spawn_spectrum_worker(
    initial_stamp: SpectrumStamp,
) -> Result<(SpectrumCapture, SpectrumWorker), SpectrumWorkerError> {
    let kernel = SpectrumKernel::new(initial_stamp.sample_rate_hz)
        .map_err(|_| SpectrumWorkerError::InvalidSampleRate(initial_stamp.sample_rate_hz))?;

    let (producer, consumer) = RingBuffer::new(CAPTURE_QUEUE_CAPACITY);
    let (result_sender, result_receiver) = sync_channel(RESULT_QUEUE_CAPACITY);
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let join = thread::Builder::new()
        .name("hex-spectrum".into())
        .spawn(move || worker_loop(consumer, result_sender, worker_stop, initial_stamp, kernel))
        .map_err(SpectrumWorkerError::ThreadSpawn)?;

    Ok((
        SpectrumCapture {
            producer,
            next_sequence: 0,
            exhausted: false,
        },
        SpectrumWorker {
            stop,
            join: Some(join),
            results: result_receiver,
            latest: None,
        },
    ))
}

#[derive(Clone, Copy)]
struct CaptureBatch {
    stamp: SpectrumStamp,
    sequence: u64,
    frame_count: u16,
    samples: [[f32; 2]; MAX_CAPTURE_FRAMES],
}

fn worker_loop(
    mut consumer: Consumer<CaptureBatch>,
    result_sender: SyncSender<PublishedSpectrum>,
    stop: Arc<AtomicBool>,
    initial_stamp: SpectrumStamp,
    kernel: SpectrumKernel,
) {
    let mut core = SpectrumCore::new(kernel, initial_stamp.sample_rate_hz);
    while !stop.load(Ordering::Acquire) {
        let mut drained = 0;
        while drained < MAX_BATCHES_PER_POLL {
            let Ok(batch) = consumer.pop() else {
                break;
            };
            core.ingest(batch, Instant::now());
            drained += 1;
        }
        let now = Instant::now();
        if let Some(result) = core.analyze_if_due(now) {
            let _ = result_sender.try_send(result);
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
}

struct SpectrumCore {
    context: Option<StreamContext>,
    kernel: Option<SpectrumKernel>,
    kernel_rate: Option<u32>,
    rolling: Box<[[f32; 2]; FFT_SIZE]>,
    ordered: Box<[[f32; 2]; FFT_SIZE]>,
    write_index: usize,
    filled: usize,
    last_sequence: Option<u64>,
    latest_stamp: Option<SpectrumStamp>,
    received_at: Option<Instant>,
    last_analysis_at: Option<Instant>,
    dirty_since_analysis: bool,
}

impl SpectrumCore {
    fn new(kernel: SpectrumKernel, sample_rate_hz: u32) -> Self {
        Self {
            context: None,
            kernel: Some(kernel),
            kernel_rate: Some(sample_rate_hz),
            rolling: Box::new([[0.0; 2]; FFT_SIZE]),
            ordered: Box::new([[0.0; 2]; FFT_SIZE]),
            write_index: 0,
            filled: 0,
            last_sequence: None,
            latest_stamp: None,
            received_at: None,
            last_analysis_at: None,
            dirty_since_analysis: false,
        }
    }

    fn ingest(&mut self, batch: CaptureBatch, received_at: Instant) -> IngestOutcome {
        let frame_count = usize::from(batch.frame_count);
        if frame_count == 0
            || frame_count > MAX_CAPTURE_FRAMES
            || batch.samples[..frame_count]
                .iter()
                .flatten()
                .any(|sample| !sample.is_finite())
        {
            self.clear_window();
            self.last_sequence = Some(batch.sequence);
            return IngestOutcome::Rejected;
        }

        let next_context = StreamContext {
            target: batch.stamp.target,
            generation: batch.stamp.generation,
            sample_rate_hz: batch.stamp.sample_rate_hz,
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
            self.kernel = SpectrumKernel::new(batch.stamp.sample_rate_hz).ok();
            self.kernel_rate = self.kernel.as_ref().map(|_| batch.stamp.sample_rate_hz);
        }
        if self.kernel.is_none() {
            self.context = None;
            self.last_sequence = Some(batch.sequence);
            return IngestOutcome::Rejected;
        }

        self.context = Some(next_context);
        self.last_sequence = Some(batch.sequence);
        self.latest_stamp = Some(batch.stamp);
        self.received_at = Some(received_at);
        for frame in &batch.samples[..frame_count] {
            self.rolling[self.write_index] = *frame;
            self.write_index = (self.write_index + 1) % FFT_SIZE;
            self.filled = (self.filled + 1).min(FFT_SIZE);
        }
        self.dirty_since_analysis = true;

        if sequence_gap || context_changed {
            IngestOutcome::Reset
        } else {
            IngestOutcome::Accepted
        }
    }

    fn analyze_if_due(&mut self, now: Instant) -> Option<PublishedSpectrum> {
        if self.filled < FFT_SIZE
            || !self.dirty_since_analysis
            || self
                .last_analysis_at
                .is_some_and(|last| now.saturating_duration_since(last) < ANALYSIS_INTERVAL)
        {
            return None;
        }
        let oldest = self.write_index;
        for (offset, destination) in self.ordered.iter_mut().enumerate() {
            *destination = self.rolling[(oldest + offset) % FFT_SIZE];
        }
        let frame = self.kernel.as_mut()?.analyze(&self.ordered[..]).ok()?;
        let processed_at = Instant::now();
        let result = PublishedSpectrum {
            stamp: self.latest_stamp?,
            frame,
            captured_at: self.received_at?,
            processed_at,
        };
        self.last_analysis_at = Some(now);
        self.dirty_since_analysis = false;
        Some(result)
    }

    fn clear_window(&mut self) {
        self.rolling.fill([0.0; 2]);
        self.write_index = 0;
        self.filled = 0;
        self.latest_stamp = None;
        self.received_at = None;
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
            stamp,
            sequence,
            frame_count: count as u16,
            samples,
        }
    }

    fn core() -> SpectrumCore {
        SpectrumCore::new(
            SpectrumKernel::new(8_000).expect("supported sample rate"),
            8_000,
        )
    }

    #[test]
    fn full_queue_drops_only_capture_and_next_sequence_exposes_gap() {
        let (producer, mut consumer) = RingBuffer::new(CAPTURE_QUEUE_CAPACITY);
        let mut capture = SpectrumCapture {
            producer,
            next_sequence: 0,
            exhausted: false,
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
        assert_eq!(
            analysis.ingest(first, Instant::now()),
            IngestOutcome::Accepted
        );
        while consumer.pop().is_ok() {}
        assert_eq!(
            capture.submit(context, &frames),
            CaptureOutcome::Queued {
                sequence: CAPTURE_QUEUE_CAPACITY as u64 + 1
            }
        );
        let next = consumer.pop().expect("batch after dropped sequence");
        assert_eq!(analysis.ingest(next, Instant::now()), IngestOutcome::Reset);
        assert_eq!(analysis.filled, MAX_CAPTURE_FRAMES);
    }

    #[test]
    fn ordinary_revisions_continue_but_target_generation_and_rate_reset_window() {
        let mut analysis = core();
        let at = Instant::now();
        assert_eq!(
            analysis.ingest(batch(0, stamp(1, 1, 8_000, 1), 0.1, 256), at),
            IngestOutcome::Accepted
        );
        assert_eq!(
            analysis.ingest(batch(1, stamp(1, 1, 8_000, 2), 0.2, 256), at),
            IngestOutcome::Accepted
        );
        assert_eq!(analysis.filled, 512);
        assert_eq!(
            analysis.ingest(batch(2, stamp(1, 2, 8_000, 2), 0.2, 256), at),
            IngestOutcome::Reset
        );
        assert_eq!(analysis.filled, 256);
        assert_eq!(
            analysis.ingest(batch(3, stamp(2, 2, 8_000, 3), 0.2, 256), at),
            IngestOutcome::Reset
        );
        assert_eq!(analysis.filled, 256);
        assert_eq!(
            analysis.ingest(batch(4, stamp(2, 2, 48_000, 4), 0.2, 256), at),
            IngestOutcome::Reset
        );
        assert_eq!(analysis.filled, 256);
        assert_eq!(analysis.kernel_rate, Some(48_000));
    }

    #[test]
    fn cadence_freshness_and_window_publication_are_deterministic() {
        let mut analysis = core();
        let start = Instant::now();
        let context = stamp(3, 1, 8_000, 1);
        for sequence in 0..16 {
            analysis.ingest(batch(sequence, context, 0.5, 256), start);
        }
        let first = analysis.analyze_if_due(start).expect("full initial window");
        assert_eq!(first.frame.bins_dbfs.len(), 96);
        assert_eq!(first.stamp.applied_revision, 1);
        assert!(first.is_fresh_at(start + DISPLAY_STALE_AFTER));
        assert!(!first.is_fresh_at(start + DISPLAY_STALE_AFTER + Duration::from_nanos(1)));

        analysis.ingest(
            batch(16, stamp(3, 1, 8_000, 2), 0.25, 256),
            start + Duration::from_millis(1),
        );
        assert!(
            analysis
                .analyze_if_due(start + Duration::from_millis(33))
                .is_none()
        );
        let next = analysis
            .analyze_if_due(start + Duration::from_millis(34))
            .expect("cadence elapsed");
        assert_eq!(next.stamp.applied_revision, 2);
    }

    #[test]
    fn context_reset_keeps_global_cadence_and_idle_windows_are_not_republished() {
        let mut analysis = SpectrumCore::new(
            SpectrumKernel::new(192_000).expect("supported sample rate"),
            192_000,
        );
        let start = Instant::now();
        let first_context = stamp(4, 1, 192_000, 1);
        for sequence in 0..16 {
            analysis.ingest(batch(sequence, first_context, 0.3, 256), start);
        }
        assert!(analysis.analyze_if_due(start).is_some());

        let next_context = stamp(4, 2, 192_000, 2);
        let next_capture_at = start + Duration::from_millis(21);
        for sequence in 16..32 {
            analysis.ingest(batch(sequence, next_context, 0.4, 256), next_capture_at);
        }
        assert!(analysis.analyze_if_due(next_capture_at).is_none());
        let second = analysis
            .analyze_if_due(start + Duration::from_millis(34))
            .expect("global cadence elapsed");
        assert!(!second.is_fresh_at(start + Duration::from_millis(122)));
        assert!(
            analysis
                .analyze_if_due(start + Duration::from_millis(200))
                .is_none()
        );
    }

    #[test]
    fn malformed_batches_clear_window_without_partial_ingest() {
        let mut analysis = core();
        let at = Instant::now();
        assert_eq!(
            analysis.ingest(batch(0, stamp(1, 1, 8_000, 1), 0.5, 256), at),
            IngestOutcome::Accepted
        );
        let mut invalid = batch(1, stamp(1, 1, 8_000, 1), 0.5, 256);
        invalid.frame_count = (MAX_CAPTURE_FRAMES + 1) as u16;
        assert_eq!(analysis.ingest(invalid, at), IngestOutcome::Rejected);
        assert_eq!(analysis.filled, 0);
        let bad_rate = batch(2, stamp(1, 1, 7_999, 1), 0.5, 256);
        assert_eq!(analysis.ingest(bad_rate, at), IngestOutcome::Rejected);
        assert_eq!(analysis.filled, 0);
        assert_eq!(analysis.context, None);
    }

    #[test]
    fn worker_lifecycle_returns_capture_to_control_before_join() {
        let context = stamp(7, 1, 8_000, 0);
        let (mut capture, worker) = spawn_spectrum_worker(context).expect("valid worker");
        assert_eq!(
            capture.submit(context, &[[0.0, 0.0]; 128]),
            CaptureOutcome::Queued { sequence: 0 }
        );
        worker
            .shutdown_and_join(capture)
            .expect("control-side capture reclamation and join");
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
            next_sequence: u64::MAX,
            exhausted: false,
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
            next_sequence: 0,
            exhausted: false,
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
