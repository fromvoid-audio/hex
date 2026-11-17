//! Joining control owner for a dedicated admitted granular source-bank worker.
use crate::{
    AllocationData, GranularBankData, GranularWorkerBank, GranularWorkerExit,
    GranularWorkerOutcome, GranularWorkerQueues, GranularWorkerRequest, GranularWorkerResult,
    StreamRuntimeError, run_granular_worker,
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

/// Listed backing DATA only. Queue slots follow the same pinned constructor
/// declaration as StreamRuntime. Excludes inline control/loop values, caller
/// page pools, earlier source construction, Arc/allocator metadata and native
/// thread/OS/stack costs. The caller must admit those separate obligations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GranularWorkerData {
    pub bank: GranularBankData,
    pub requests: AllocationData,
    pub results: AllocationData,
    pub queue_objects: usize,
    pub stop_flag: usize,
    pub demand_epoch: usize,
}
impl GranularWorkerData {
    pub fn bytes(self) -> Option<usize> {
        crate::footprint::sum([
            self.bank.bytes()?,
            self.requests.bytes()?,
            self.results.bytes()?,
            self.queue_objects,
            self.stop_flag,
            self.demand_epoch,
        ])
    }
    /// Four queue halves, two Stop and two epoch handles, plus each source's
    /// manifest and kernel handles. File readers and one thread remain native
    /// lifetime obligations, not shared-handle or byte estimates.
    pub fn shared_handle_bound(self) -> Option<usize> {
        8usize
            .checked_add(self.bank.manifest_handles)?
            .checked_add(self.bank.kernel_handles)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct GranularWorkerAllowance {
    pub max_data_bytes: usize,
    pub max_shared_handles: usize,
}

/// Returned after explicit control-side join. Drain the result consumer and
/// the worker's request consumer, interrupted request and held outcome before
/// returning pages to their original pool; destroy queues and sources on control.
pub struct GranularWorkerStopped {
    pub worker: GranularWorkerExit,
    pub requests: Producer<GranularWorkerRequest>,
    pub results: Consumer<GranularWorkerOutcome>,
}

/// Owns one thread and its finite queues, never a playback/catalog lease.
///
/// All methods and final destruction belong off callback. Only this joining
/// owner wakes or joins its thread. Explicit shutdown is required to observe
/// join/custody failures and reclaim accepted buffers; Drop stops and joins as
/// a fallback, destroying retained objects on control. OS I/O can delay join.
/// StreamRuntime will retain this component during live integration; this type
/// does not implement MediaPages or create a competing publication path.
pub struct GranularWorkerControl {
    requests: Option<Producer<GranularWorkerRequest>>,
    results: Option<Consumer<GranularWorkerOutcome>>,
    stop: Arc<AtomicBool>,
    demand: Arc<AtomicU64>,
    epoch: u64,
    worker: Option<JoinHandle<GranularWorkerExit>>,
    data: GranularWorkerData,
}
impl GranularWorkerControl {
    pub fn data_bound(
        bank: &GranularWorkerBank,
        queue_capacity: usize,
    ) -> Result<GranularWorkerData, StreamRuntimeError> {
        if !(1..=1024).contains(&queue_capacity) || !crate::footprint::queue_family_supported() {
            return Err(StreamRuntimeError::InvalidConfiguration);
        }
        let data = GranularWorkerData {
            bank: bank.data(),
            requests: AllocationData::of::<GranularWorkerRequest>(queue_capacity),
            results: AllocationData::of::<GranularWorkerOutcome>(queue_capacity),
            queue_objects: std::mem::size_of::<RingBuffer<GranularWorkerRequest>>()
                .checked_add(std::mem::size_of::<RingBuffer<GranularWorkerOutcome>>())
                .ok_or(StreamRuntimeError::InvalidConfiguration)?,
            stop_flag: std::mem::size_of::<AtomicBool>(),
            demand_epoch: std::mem::size_of::<AtomicU64>(),
        };
        data.bytes()
            .ok_or(StreamRuntimeError::InvalidConfiguration)?;
        data.shared_handle_bound()
            .ok_or(StreamRuntimeError::InvalidConfiguration)?;
        Ok(data)
    }
    /// Refusal drops prepared inputs on this control thread, as the existing
    /// StreamRuntime constructor does. No page buffer is accepted before start.
    pub fn try_start_on_control(
        bank: GranularWorkerBank,
        queue_capacity: usize,
        allowance: GranularWorkerAllowance,
    ) -> Result<Self, StreamRuntimeError> {
        let data = Self::data_bound(&bank, queue_capacity)?;
        if data
            .bytes()
            .is_none_or(|bytes| bytes > allowance.max_data_bytes)
            || data
                .shared_handle_bound()
                .is_none_or(|handles| handles > allowance.max_shared_handles)
        {
            return Err(StreamRuntimeError::InvalidConfiguration);
        }
        let (requests, request_rx) = RingBuffer::new(queue_capacity);
        let (result_tx, results) = RingBuffer::new(queue_capacity);
        let stop = Arc::new(AtomicBool::new(false));
        let demand = Arc::new(AtomicU64::new(1));
        let worker_stop = Arc::clone(&stop);
        let worker_demand = Arc::clone(&demand);
        let worker = thread::Builder::new()
            .name("hex-granular-worker".into())
            .spawn(move || {
                run_granular_worker(
                    bank,
                    GranularWorkerQueues {
                        requests: request_rx,
                        results: result_tx,
                    },
                    &worker_stop,
                    &worker_demand,
                )
            })
            .map_err(|_| StreamRuntimeError::WorkerSpawnFailed)?;
        Ok(Self {
            requests: Some(requests),
            results: Some(results),
            stop,
            demand,
            epoch: 1,
            worker: Some(worker),
            data,
        })
    }
    pub fn data(&self) -> GranularWorkerData {
        self.data
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn available_request_slots(&self) -> usize {
        self.requests.as_ref().map_or(0, Producer::slots)
    }
    pub fn queued_results(&self) -> usize {
        self.results.as_ref().map_or(0, Consumer::slots)
    }
    pub fn worker_finished(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }
    /// A full queue, invalid size/epoch or finished worker returns the original
    /// request. At most 2*queue_capacity+1 accepted fixed pages can be in flight;
    /// the caller must also enforce its own finite page-pool admission.
    pub fn try_submit_on_control(
        &mut self,
        request: GranularWorkerRequest,
    ) -> Result<(), GranularWorkerRequest> {
        if request.frames.len() != hex_engine::STREAM_PAGE_FRAMES
            || request.key.epoch != self.epoch
            || self.worker_finished()
        {
            return Err(request);
        }
        let Some(requests) = self.requests.as_mut() else {
            return Err(request);
        };
        match requests.push(request) {
            Ok(()) => {
                self.wake();
                Ok(())
            }
            Err(PushError::Full(request)) => Err(request),
        }
    }
    /// Serialize this with result installation on the same control owner.
    /// Retire before epoch exhaustion; wrapping or reusing epochs is forbidden.
    pub fn supersede_on_control(&mut self) -> Result<u64, StreamRuntimeError> {
        let next = self
            .epoch
            .checked_add(1)
            .ok_or(StreamRuntimeError::WorkerUnavailable)?;
        self.epoch = next;
        self.demand.store(next, Ordering::Release);
        self.wake();
        Ok(next)
    }
    /// Fence outcomes already queued when demand changed. This never publishes
    /// PCM; the runtime must still bind the outcome to its current page owner.
    pub fn poll_on_control(&mut self) -> Option<GranularWorkerOutcome> {
        let mut outcome = self.results.as_mut()?.pop().ok()?;
        if outcome.key.epoch != self.epoch {
            let frames = match outcome.result {
                GranularWorkerResult::Ready(page) => page.frames,
                GranularWorkerResult::Cancelled { frames }
                | GranularWorkerResult::Failed { frames, .. } => frames,
            };
            outcome.result = GranularWorkerResult::Cancelled { frames };
        }
        self.wake();
        Some(outcome)
    }
    pub fn shutdown_on_control(mut self) -> Result<GranularWorkerStopped, StreamRuntimeError> {
        let worker = self.stop_and_join()?;
        let requests = self
            .requests
            .take()
            .ok_or(StreamRuntimeError::WorkerUnavailable)?;
        let results = self
            .results
            .take()
            .ok_or(StreamRuntimeError::WorkerUnavailable)?;
        Ok(GranularWorkerStopped {
            worker,
            requests,
            results,
        })
    }
    fn wake(&self) {
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
    fn stop_and_join(&mut self) -> Result<GranularWorkerExit, StreamRuntimeError> {
        self.stop.store(true, Ordering::Release);
        let worker = self
            .worker
            .take()
            .ok_or(StreamRuntimeError::WorkerUnavailable)?;
        worker.thread().unpark();
        worker
            .join()
            .map_err(|_| StreamRuntimeError::WorkerJoinFailed)
    }
}
impl Drop for GranularWorkerControl {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.stop_and_join();
        }
    }
}
