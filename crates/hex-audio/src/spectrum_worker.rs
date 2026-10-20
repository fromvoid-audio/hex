//! Bounded off-callback capture and rolling spectrum analysis.
//!
//! This module consumes already-rendered stereo output. It does not own an
//! engine tap, device lifecycle, project model, or UI. The callback-facing
//! [`SpectrumCapture::submit`] path copies at most 256 frames into fixed storage
//! and attempts one nonblocking SPSC push.

use crate::spectrum::{FFT_SIZE, SpectrumFrame, SpectrumKernel};
mod native;
mod project;
mod stop;
use crate::native_callback_owner::{
    NativePreparedConsumer, NativePreparedProducer, native_prepared_ring,
};
pub(crate) use native::constructor_cost;
#[cfg(test)]
pub(crate) use native::spawn_native_spectrum_worker;
pub(crate) use native::{NativeSpectrumIdentity, NativeSpectrumReceipt};
pub(crate) use project::ProjectSelectionMailbox;
pub use project::{
    MAX_PROJECT_CONTRIBUTION_FRAMES, MAX_PROJECT_LAYERS, MAX_PROJECT_PAYLOAD_BYTES,
    MAX_PROJECT_TRACKS, PROJECT_DISPLAY_STALE_AFTER, ProjectSelection, ProjectSpectrumRefresh,
    PublishedProjectSpectrum,
};
use rtrb::RingBuffer;
use std::{
    error::Error,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
pub(crate) use stop::SpectrumStop;

pub const CAPTURE_QUEUE_CAPACITY: usize = 32;
pub const MAX_CAPTURE_FRAMES: usize = 256;
pub const MAX_BATCHES_PER_POLL: usize = 32;
pub const ANALYSIS_INTERVAL: Duration = Duration::from_nanos(33_333_334);
pub const DISPLAY_STALE_AFTER: Duration = Duration::from_millis(100);
const DECAY_DB_PER_SECOND: f32 = 48.0;

const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(5);
pub(crate) const RESULT_QUEUE_CAPACITY: usize = 1;
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
    producer: NativePreparedProducer<CaptureBatch>,
    next_sequence: u64,
    exhausted: bool,
    native: Option<NativeSpectrumReceipt>,
    pub(crate) project: Option<project::ProjectCapture>,
}

impl SpectrumCapture {
    pub(crate) fn matches_capture_ring(
        &self,
        record: &hex_engine::sampler_host_fields::NativeFixedRingRegistration,
    ) -> bool {
        self.native.is_some() && self.producer.matches_constructor_record(record)
    }
    pub(crate) fn matches_project_rings(
        &self,
        records: [Option<&hex_engine::sampler_host_fields::NativeFixedRingRegistration>; 3],
        packets: &[Option<hex_engine::sampler_host_fields::NativeSpectrumPacketRegistration>],
    ) -> bool {
        self.project
            .as_ref()
            .is_some_and(|project| project.matches_rings(records, packets))
    }
    pub(crate) fn is_legacy_capture(&self) -> bool {
        self.native.is_none() && self.producer.is_unregistered()
    }

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
        self.submit_signals_at(stamp, None, frames, captured_start)
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
        self.submit_signals_at(stamp, Some(input), output, captured_start)
    }

    fn submit_signals_at(
        &mut self,
        stamp: SpectrumStamp,
        input: Option<&[[f32; 2]]>,
        frames: &[[f32; 2]],
        captured_start: Instant,
    ) -> CaptureOutcome {
        if self.exhausted {
            return CaptureOutcome::SequenceExhausted;
        }
        let sequence = self.next_sequence;
        let Some(next_sequence) = sequence.checked_add(1) else {
            self.exhausted = true;
            return CaptureOutcome::SequenceExhausted;
        };
        self.next_sequence = next_sequence;

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
        if self.exhausted {
            return CaptureOutcome::SequenceExhausted;
        }
        let sequence = self.next_sequence;
        let Some(next_sequence) = sequence.checked_add(1) else {
            self.exhausted = true;
            return CaptureOutcome::SequenceExhausted;
        };
        self.next_sequence = next_sequence;
        CaptureOutcome::Skipped { sequence }
    }
}

/// Control-side newest-result cache. It can be polled by the UI without
/// owning or joining the worker thread.
pub struct SpectrumReader {
    results: NativePreparedConsumer<PublishedSpectra>,
    latest: Option<PublishedSpectra>,
    native: Option<NativeSpectrumIdentity>,
    project: Option<project::ProjectReader>,
}

impl SpectrumReader {
    pub(crate) fn matches_result_ring(
        &self,
        record: &hex_engine::sampler_host_fields::NativeFixedRingRegistration,
    ) -> bool {
        self.native.is_some() && self.results.matches_constructor_record(record)
    }
    pub(crate) fn matches_project_rings(
        &self,
        records: [Option<&hex_engine::sampler_host_fields::NativeFixedRingRegistration>; 4],
        results: &[Option<hex_engine::sampler_host_fields::NativeSpectrumResultRegistration>],
    ) -> bool {
        self.project
            .as_ref()
            .is_some_and(|project| project.matches_rings(records, results))
    }
    pub(crate) fn is_legacy_reader(&self) -> bool {
        self.native.is_none() && self.results.is_unregistered()
    }

    pub fn project_available(&self) -> bool {
        self.project.is_some()
    }
    pub fn try_latest_project_at(
        &mut self,
        now: Instant,
        selection: ProjectSelection,
    ) -> Option<&PublishedProjectSpectrum> {
        self.project.as_mut()?.latest_at(now, selection)
    }
    pub(crate) fn refresh_project(
        &mut self,
        slot: &mut Option<PublishedProjectSpectrum>,
        now: Instant,
        selection: Option<ProjectSelection>,
    ) -> project::ProjectSpectrumRefresh {
        match self.project.as_mut() {
            Some(project) => project.refresh(slot, now, selection),
            None => {
                drop(slot.take());
                project::ProjectSpectrumRefresh::Cleared
            }
        }
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
            match self.results.pop() {
                Ok(result) => self.latest = Some(result),
                Err(rtrb::PopError::Empty) => break,
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
    stop: SpectrumStop,
    join: Option<JoinHandle<()>>,
    native: Option<NativeSpectrumIdentity>,
    phase: ReaperPhase,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
    captured_join: Option<hex_engine::sampler_host_fields::NativeSpectrumJoinLease>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReaperPhase {
    Ready,
    Pending,
    Complete,
    Failed,
}
impl SpectrumReaper {
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_join_completed(&self) -> bool {
        self.phase == ReaperPhase::Complete && self.join.is_none()
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_first_panic(&self) -> Option<&(dyn std::any::Any + Send)> {
        self.first_panic.as_deref()
    }
    pub(crate) fn validate_actual_stop_on_control(
        &self,
        parent: &crate::native_callback_owner::NativeRetirement,
    ) -> Result<(), SpectrumWorkerError> {
        parent
            .matches_spectrum_stop(&self.stop)
            .then_some(())
            .ok_or(SpectrumWorkerError::StorageRefused)
    }
    // Only the actual owning lifecycle parent invokes this AFTER advancing
    // this SAME Reaper. The affine association cannot be copied from identity.
    pub(crate) fn finalize_captured_frame_on_control(
        &mut self,
        parent: &crate::native_callback_owner::NativeRetirement,
    ) -> Result<(), SpectrumWorkerError> {
        if self.phase != ReaperPhase::Complete || self.join.is_some() || self.first_panic.is_some()
        {
            return Err(SpectrumWorkerError::ThreadPanicked);
        }
        if let Some(association) = self.captured_join.as_mut() {
            parent
                .finish_spectrum_capture_on_control(association)
                .map_err(|_| SpectrumWorkerError::StorageOverflow)?;
            // Metadata alias closure is checked before disposing this paired
            // field. Root's registration itself remains retained afterwards.
            drop(self.captured_join.take());
        }
        Ok(())
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn join_finished_for_capture_test(&self) -> bool {
        self.join.as_ref().is_some_and(JoinHandle::is_finished)
    }
    /// Request exit without joining. Safe to use while retaining a quarantined
    /// stream and callback; all destruction still waits for successful pause.
    pub fn request_stop(&self) {
        let _requested = self.stop.request_stop();
    }

    /// Stop and join after the callback producer has been destroyed by stream
    /// disposal. The worker consumer and queue allocation are reclaimed here.
    pub fn shutdown_and_join(mut self) -> Result<(), SpectrumWorkerError> {
        self.advance_shutdown_on_control()
    }
    /// Persistent lifecycle seam. No owner take before Pending, no ordinary
    /// mutex is held, and a worker panic keeps its exact first payload here.
    pub(crate) fn advance_shutdown_on_control(&mut self) -> Result<(), SpectrumWorkerError> {
        if self.stop.request_stop().is_err() {
            self.phase = ReaperPhase::Failed;
            return Err(SpectrumWorkerError::StorageRefused);
        }
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
                if let Some(association) = self.captured_join.as_mut() {
                    association.record_actual_worker_failure_on_control();
                }
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
        let _requested = self.stop.request_stop();
    }
}

#[derive(Debug)]
pub enum SpectrumWorkerError {
    InvalidSampleRate(u32),
    ThreadSpawn(std::io::Error),
    ThreadPanicked,
    StorageOverflow,
    StorageRefused,
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
            Self::StorageRefused => {
                formatter.write_str("spectrum storage binding is unsupported or failed")
            }
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
    spawn_worker_with_parent(initial_stamp, fixed_native, entry, None)
}
pub(crate) fn spawn_worker_with_parent(
    initial_stamp: SpectrumStamp,
    fixed_native: bool,
    entry: Option<fn()>,
    parent: Option<&crate::native_callback_owner::NativeRetirement>,
) -> Result<(SpectrumCapture, SpectrumReader, SpectrumReaper), SpectrumWorkerError> {
    let kernel = SpectrumKernel::new(initial_stamp.sample_rate_hz)
        .map_err(|_| SpectrumWorkerError::InvalidSampleRate(initial_stamp.sample_rate_hz))?;

    let core = SpectrumCore::new(kernel, initial_stamp.sample_rate_hz);
    let (project_capture, project_reader, project_worker) = if fixed_native {
        let (capture, reader, worker) =
            project::channel_for_parent(initial_stamp.sample_rate_hz, parent)?;
        (Some(capture), Some(reader), Some(worker))
    } else {
        (None, None, None)
    };
    let (producer, consumer) = if fixed_native && let Some(parent) = parent {
        parent
            .spectrum_capture_ring_on_control()
            .map_err(|_| SpectrumWorkerError::StorageOverflow)?
    } else {
        native_prepared_ring(CAPTURE_QUEUE_CAPACITY)
    };
    let (result_sender, result_receiver) = if fixed_native && let Some(parent) = parent {
        parent
            .spectrum_result_ring_on_control()
            .map_err(|_| SpectrumWorkerError::StorageOverflow)?
    } else {
        native_prepared_ring(RESULT_QUEUE_CAPACITY)
    };
    let [stop, worker_stop] = SpectrumStop::pair(if fixed_native { parent } else { None })?;
    let mut receipt = if fixed_native {
        Some(
            NativeSpectrumReceipt::new(
                &producer,
                &result_sender,
                &stop,
                &core,
                initial_stamp.sample_rate_hz,
                project_capture.as_ref(),
                project_worker.as_ref(),
            )
            .ok_or(SpectrumWorkerError::StorageOverflow)?,
        )
    } else {
        None
    };
    let mut core = CapturedSpectrumCore {
        payload: Some(core),
        project: project_worker,
        consumer,
        results: result_sender,
        stop: worker_stop,
        entry,
        lease: None,
        #[cfg(all(test, feature = "allocation-probe"))]
        panic_on_drop: TEST_CAPTURE_DROP_PANIC.with(|fault| fault.replace(false)),
    };
    let mut captured_join = if fixed_native && let Some(parent) = parent {
        let fields = core.fields().ok_or(SpectrumWorkerError::StorageOverflow)?;
        if let Some(receipt) = receipt.as_mut() {
            receipt.storage.data_bytes = receipt.storage.data_bytes.checked_add(
                hex_engine::sampler_host_fields::NativeSpectrumCapturedRegistration::metadata_requested_bytes_on_control()
                    .map_err(|_| SpectrumWorkerError::StorageOverflow)?)
                .and_then(|bytes| bytes.checked_add(hex_engine::sampler_host_fields::NativeFixedRingRegistration::metadata_requested_bytes_on_control().ok()?.checked_mul(2)?))
                .ok_or(SpectrumWorkerError::StorageOverflow)?;
        }
        let (lease, join) = parent
            .pair_spectrum_capture_on_control(fields)
            .map_err(|_| SpectrumWorkerError::StorageOverflow)?;
        core.lease = Some(lease);
        Some(join)
    } else {
        None
    };
    let join = spawn_analyzer_thread(move || worker_loop(core));
    let join = match join {
        Ok(join) => join,
        Err(error) => {
            // std returned the actual closure after normal disposal. Buffers
            // must report normal completion; Preparing unwind never reaches
            // this separate no-worker constructor-abort branch.
            if let (Some(parent), Some(association)) = (parent, captured_join.as_mut()) {
                parent
                    .abort_spectrum_capture_on_control(association)
                    .map_err(|_| SpectrumWorkerError::StorageOverflow)?;
            }
            return Err(SpectrumWorkerError::ThreadSpawn(error));
        }
    };

    Ok((
        SpectrumCapture {
            producer,
            next_sequence: 0,
            exhausted: false,
            native: receipt,
            project: project_capture,
        },
        SpectrumReader {
            results: result_receiver,
            latest: None,
            native: receipt.map(|receipt| receipt.identity),
            project: project_reader,
        },
        SpectrumReaper {
            stop,
            join: Some(join),
            native: receipt.map(|receipt| receipt.identity),
            phase: ReaperPhase::Ready,
            first_panic: None,
            captured_join,
        },
    ))
}

#[derive(Clone, Copy)]
pub(crate) struct CaptureBatch {
    stamp: SpectrumStamp,
    sequence: u64,
    captured_start: Instant,
    frame_count: u16,
    samples: [[f32; 2]; MAX_CAPTURE_FRAMES],
    input_samples: [[f32; 2]; MAX_CAPTURE_FRAMES],
    paired: bool,
}

// Both existing result paths are SPSC, capacity one and drop the NEW result
// on a full/disconnected consumer. A reader can disappear after this check;
// the one already-queued owner then stays charged until worker retirement.
fn try_publish_native_result(
    producer: &mut NativePreparedProducer<PublishedSpectra>,
    value: PublishedSpectra,
) -> Option<PublishedSpectra> {
    // Actual queued values are entirely inline. Exhaustive field types make
    // a future owning descendant a compiler/source-contract change.
    let PublishedSpectra { output, input } = &value;
    let PublishedSpectrum {
        stamp,
        frame,
        captured_at,
        processed_at,
    } = output;
    let SpectrumStamp {
        target,
        generation,
        sample_rate_hz,
        applied_revision,
    } = stamp;
    let _: (&SpectrumTargetId, &u64, &u32, &u64, &Instant, &Instant) = (
        target,
        generation,
        sample_rate_hz,
        applied_revision,
        captured_at,
        processed_at,
    );
    frame.validate_native_inline_fields();
    if let Some(frame) = input {
        frame.validate_native_inline_fields();
    }
    if producer.is_abandoned() {
        return Some(value);
    }
    producer
        .push(value)
        .err()
        .map(|rtrb::PushError::Full(value)| value)
}

#[cfg(all(test, feature = "allocation-probe"))]
thread_local! {
    static TEST_SPECTRUM_SPAWN_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_CAPTURE_DROP_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn spawn_analyzer_thread(
    worker: impl FnOnce() + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    #[cfg(all(test, feature = "allocation-probe"))]
    if TEST_SPECTRUM_SPAWN_FAILURE.with(|fault| fault.replace(false)) {
        // Exercise the SAME constructor Err branch after actual normal closure
        // disposal; this injection is not a real OS spawn-failure claim.
        drop(worker);
        return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock));
    }
    thread::Builder::new()
        .name("hex-spectrum".into())
        .spawn(worker)
}

fn worker_loop(mut captured: CapturedSpectrumCore) {
    if let Some(entry) = captured.entry {
        entry();
    }
    loop {
        match captured.stop.stopped() {
            Ok(false) => {}
            Ok(true) => break,
            Err(_) => {
                captured.record_capture_failure();
                return;
            }
        }
        let Some(core) = captured.payload.as_mut() else {
            captured.record_capture_failure();
            return;
        };
        let mut drained = 0;
        while drained < MAX_BATCHES_PER_POLL {
            let Ok(batch) = captured.consumer.pop() else {
                break;
            };
            core.ingest(batch);
            drained += 1;
        }
        let now = Instant::now();
        if let Some(result) = core.analyze_pair_if_due(now) {
            let _ = try_publish_native_result(&mut captured.results, result);
        }
        if let Some(project) = captured.project.as_mut() {
            drained += project.poll(now);
        }
        if drained == 0 {
            thread::sleep(IDLE_POLL_INTERVAL);
        }
    }
}

// The existing worker owns this payload directly. No new Box/lock/custodian.
// Its inline frame outlives buffer destruction until the actual thread joins.
struct CapturedSpectrumCore {
    payload: Option<SpectrumCore>,
    project: Option<project::ProjectWorker>,
    consumer: NativePreparedConsumer<CaptureBatch>,
    results: NativePreparedProducer<PublishedSpectra>,
    stop: SpectrumStop,
    entry: Option<fn()>,
    lease: Option<hex_engine::sampler_host_fields::NativeSpectrumCaptureLease>,
    #[cfg(all(test, feature = "allocation-probe"))]
    panic_on_drop: bool,
}
impl CapturedSpectrumCore {
    fn record_capture_failure(&self) {
        if let Some(lease) = self.lease.as_ref() {
            lease.record_actual_capture_failure_on_control();
        }
    }
    fn fields(&self) -> Option<hex_engine::sampler_host_fields::NativeSpectrumCaptureFields<'_>> {
        Some(
            hex_engine::sampler_host_fields::NativeSpectrumCaptureFields {
                body: hex_engine::sampler_host_fields::BodyRef::of(self),
                core: self.payload.as_ref()?.native_core_fields()?,
                project: self.project.as_ref()?.native_core_fields(),
            },
        )
    }
}
impl Drop for CapturedSpectrumCore {
    fn drop(&mut self) {
        let Some(lease) = self.lease.as_ref() else {
            // Unpaired generic constructor, or an already failed disappeared
            // lease: no registration completion is possible from this branch.
            return;
        };
        let Some(fields) = self.fields() else {
            self.record_capture_failure();
            return;
        };
        if lease.begin_actual_buffers_drop_on_control(fields).is_err() {
            self.record_capture_failure();
            return;
        }
        #[cfg(all(test, feature = "allocation-probe"))]
        if self.panic_on_drop {
            std::panic::panic_any(31655_u64);
        }
        let Some(payload) = self.payload.take() else {
            self.record_capture_failure();
            return;
        };
        drop(payload);
        drop(self.project.take());
        if let Some(lease) = self.lease.as_mut() {
            let _completed = lease.complete_actual_buffers_drop_on_control();
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
    rolling: Box<[[f32; 2]; FFT_SIZE]>,
    ordered: Box<[[f32; 2]; FFT_SIZE]>,
    input_rolling: Box<[[f32; 2]; FFT_SIZE]>,
    input_ordered: Box<[[f32; 2]; FFT_SIZE]>,
    write_index: usize,
    filled: usize,
    last_sequence: Option<u64>,
    latest_stamp: Option<SpectrumStamp>,
    captured_at: Option<Instant>,
    last_analysis_at: Option<Instant>,
    output_envelope: Option<SpectrumFrame>,
    input_envelope: Option<SpectrumFrame>,
    envelope_at: Option<Instant>,
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
            input_rolling: Box::new([[0.0; 2]; FFT_SIZE]),
            input_ordered: Box::new([[0.0; 2]; FFT_SIZE]),
            write_index: 0,
            filled: 0,
            last_sequence: None,
            latest_stamp: None,
            captured_at: None,
            last_analysis_at: None,
            output_envelope: None,
            input_envelope: None,
            envelope_at: None,
            dirty_since_analysis: false,
        }
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
        self.captured_at = Some(batch.captured_start);
        for (offset, frame) in batch.samples[..frame_count].iter().enumerate() {
            self.rolling[self.write_index] = *frame;
            if batch.paired {
                self.input_rolling[self.write_index] = batch.input_samples[offset];
            }
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

    #[cfg(test)]
    fn analyze_if_due(&mut self, now: Instant) -> Option<PublishedSpectrum> {
        self.analyze_pair_if_due(now).map(|pair| pair.output)
    }

    fn analyze_pair_if_due(&mut self, now: Instant) -> Option<PublishedSpectra> {
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
            self.input_ordered[offset] = self.input_rolling[(oldest + offset) % FFT_SIZE];
        }
        let mut frame = self.kernel.as_mut()?.analyze(&self.ordered[..]).ok()?;
        let mut input = if self.context?.paired {
            Some(
                self.kernel
                    .as_mut()?
                    .analyze(&self.input_ordered[..])
                    .ok()?,
            )
        } else {
            None
        };
        let decay = self.envelope_at.map_or(0.0, |previous| {
            now.saturating_duration_since(previous).as_secs_f32() * DECAY_DB_PER_SECOND
        });
        apply_decay(&mut frame, self.output_envelope.as_ref(), decay);
        if let Some(input) = input.as_mut() {
            apply_decay(input, self.input_envelope.as_ref(), decay);
        }
        self.output_envelope = Some(frame.clone());
        self.input_envelope = input.clone();
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
        self.rolling.fill([0.0; 2]);
        self.input_rolling.fill([0.0; 2]);
        self.write_index = 0;
        self.filled = 0;
        self.latest_stamp = None;
        self.captured_at = None;
        self.output_envelope = None;
        self.input_envelope = None;
        self.envelope_at = None;
        self.dirty_since_analysis = false;
    }
}

// Attack follows actual measured power immediately. Decay affects display-only
// worker results and never refreshes their capture timestamp or cadence.
fn apply_decay(frame: &mut SpectrumFrame, previous: Option<&SpectrumFrame>, decay: f32) {
    if let Some(previous) = previous {
        for (current, old) in frame.bins_dbfs.iter_mut().zip(previous.bins_dbfs.iter()) {
            *current = current.max((old - decay).max(crate::spectrum::FLOOR_DBFS));
        }
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
    fn paired_decay_uses_shared_elapsed_time_and_gap_clears_both_envelopes() {
        let context = stamp(3, 7, 48_000, 11);
        let mut analysis = SpectrumCore::new(SpectrumKernel::new(48_000).unwrap(), 48_000);
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
            for (before, after) in old.bins_dbfs.iter().zip(new.bins_dbfs.iter()) {
                assert!((after - (before - 4.8).max(crate::spectrum::FLOOR_DBFS)).abs() < 0.001);
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
        let mut analysis = SpectrumCore::new(SpectrumKernel::new(48_000).unwrap(), 48_000);
        let now = Instant::now();
        for sequence in 0..16 {
            let mut capture = batch(sequence, context, 0.0, 256);
            capture.paired = true;
            capture.captured_start = now;
            for i in 0..256 {
                let position = sequence as usize * 256 + i;
                let signal =
                    (std::f32::consts::TAU * 32.0 * position as f32 / FFT_SIZE as f32).sin() * 0.5;
                capture.input_samples[i] = [signal, -signal];
                capture.samples[i] = [signal * 0.25, -signal * 0.25];
            }
            analysis.ingest(capture);
        }
        let pair = analysis.analyze_pair_if_due(now).unwrap();
        let peak = |frame: &SpectrumFrame| {
            frame
                .bins_dbfs
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
        assert_eq!(analysis.filled, 256);
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
        assert_eq!(analysis.filled, 256);
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
        let (producer, mut consumer) = native_prepared_ring(CAPTURE_QUEUE_CAPACITY);
        let mut capture = SpectrumCapture {
            producer,
            next_sequence: 0,
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
            stamp,
            sequence,
            captured_start: Instant::now(),
            frame_count: count as u16,
            samples,
            input_samples: [[0.0; 2]; MAX_CAPTURE_FRAMES],
            paired: false,
        }
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn malformed_captured_payload_does_not_mask_first_unwind_or_complete_storage() {
        let _serial = crate::prepared::candidate_tests::serial();
        for missing_core in [true, false] {
            let root = crate::candidate_custody::CandidateRoot::new(3).unwrap();
            let parent =
                crate::native_callback_owner::NativeRetirement::begin_construction(root.clone());
            let (_capture, consumer) = native_prepared_ring(CAPTURE_QUEUE_CAPACITY);
            let (results, _reader) = native_prepared_ring(RESULT_QUEUE_CAPACITY);
            let [_stop, worker_stop] = SpectrumStop::pair(None).unwrap();
            let (_project_capture, _project_reader, project) = project::channel(48000).unwrap();
            let mut captured = CapturedSpectrumCore {
                payload: Some(SpectrumCore::new(
                    SpectrumKernel::new(48000).unwrap(),
                    48000,
                )),
                project: Some(project),
                consumer,
                results,
                stop: worker_stop,
                entry: None,
                lease: None,
                panic_on_drop: false,
            };
            let (lease, join) = parent
                .pair_spectrum_capture_on_control(captured.fields().unwrap())
                .unwrap();
            captured.lease = Some(lease);
            if missing_core {
                drop(captured.payload.take());
            } else if let Some(core) = captured.payload.as_mut() {
                drop(core.kernel.take());
            }
            let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _captured = captured;
                std::panic::panic_any(31715_u64);
            }))
            .unwrap_err();
            assert_eq!(first.downcast_ref::<u64>(), Some(&31715));
            assert!(matches!(
                root.test_spectrum_capture_facts(),
                Err(hex_engine::AudioStorageError::UnknownStorage)
            ));
            drop(join);
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
        let (producer, mut consumer) = native_prepared_ring(CAPTURE_QUEUE_CAPACITY);
        let mut capture = SpectrumCapture {
            producer,
            next_sequence: 0,
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
        assert_eq!(analysis.filled, MAX_CAPTURE_FRAMES);
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
        assert_eq!(analysis.filled, 512);
        assert_eq!(
            analysis.ingest(batch(2, stamp(1, 2, 8_000, 2), 0.2, 256)),
            IngestOutcome::Reset
        );
        assert_eq!(analysis.filled, 256);
        assert_eq!(
            analysis.ingest(batch(3, stamp(2, 2, 8_000, 3), 0.2, 256)),
            IngestOutcome::Reset
        );
        assert_eq!(analysis.filled, 256);
        assert_eq!(
            analysis.ingest(batch(4, stamp(2, 2, 48_000, 4), 0.2, 256)),
            IngestOutcome::Reset
        );
        assert_eq!(analysis.filled, 256);
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
        assert_eq!(first.frame.bins_dbfs.len(), 96);
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

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn actual_scalar_result_ring_preserves_oldest_full_and_freshness_without_allocating() {
        let now = Instant::now();
        let make = |revision| PublishedSpectra {
            output: PublishedSpectrum {
                stamp: stamp(1, 1, 48_000, revision),
                frame: SpectrumFrame {
                    sample_rate_hz: 48_000,
                    bins_dbfs: [crate::spectrum::FLOOR_DBFS; crate::spectrum::SPECTRUM_BANDS],
                },
                captured_at: now,
                processed_at: now,
            },
            input: None,
        };
        let (mut sender, receiver) = native_prepared_ring(RESULT_QUEUE_CAPACITY);
        assert_eq!(sender.buffer().capacity(), 1);
        let mut reader = SpectrumReader {
            results: receiver,
            latest: None,
            native: None,
            project: None,
        };
        let measured = allocation_counter::measure(|| {
            assert!(try_publish_native_result(&mut sender, make(1)).is_none());
            let returned = try_publish_native_result(&mut sender, make(2)).unwrap();
            assert_eq!(returned.output.stamp.applied_revision, 2);
            assert_eq!(reader.try_latest_at(now).unwrap().stamp.applied_revision, 1);
            assert!(try_publish_native_result(&mut sender, make(3)).is_none());
            assert_eq!(reader.try_latest_at(now).unwrap().stamp.applied_revision, 3);
            // Producer shutdown never refreshes a cached capture or fabricates
            // an output; the same original capture-age law clears it.
            drop(sender);
            assert!(
                reader
                    .try_latest_at(now + DISPLAY_STALE_AFTER + Duration::from_nanos(1))
                    .is_none()
            );
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        // A vanished reader cannot block the worker's one push attempt. Stop
        // and join remain controlled by the existing independent lifecycle.
        let (mut sender, receiver) = native_prepared_ring(RESULT_QUEUE_CAPACITY);
        drop(receiver);
        let measured = allocation_counter::measure(|| {
            sender.push(make(4)).unwrap();
            assert!(matches!(
                sender.push(make(5)),
                Err(rtrb::PushError::Full(_))
            ));
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
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

        let (mut sender, receiver) = native_prepared_ring(RESULT_QUEUE_CAPACITY);
        sender
            .push(PublishedSpectra {
                output: result,
                input: None,
            })
            .expect("publish worker result");
        let mut reader = SpectrumReader {
            results: receiver,
            latest: None,
            native: None,
            project: None,
        };
        // Receipt happens now, but the channel carries the original capture
        // timestamp so expensive/queued processing cannot refresh stale audio.
        assert!(reader.try_latest_at(now).is_none());
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
        assert_eq!(analysis.filled, 0);
        let bad_rate = batch(2, stamp(1, 1, 7_999, 1), 0.5, 256);
        assert_eq!(analysis.ingest(bad_rate), IngestOutcome::Rejected);
        assert_eq!(analysis.filled, 0);
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
        let (producer, _consumer) = native_prepared_ring(CAPTURE_QUEUE_CAPACITY);
        let mut capture = SpectrumCapture {
            producer,
            next_sequence: u64::MAX,
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
        let (producer, mut consumer) = native_prepared_ring(CAPTURE_QUEUE_CAPACITY);
        let mut capture = SpectrumCapture {
            producer,
            next_sequence: 0,
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
