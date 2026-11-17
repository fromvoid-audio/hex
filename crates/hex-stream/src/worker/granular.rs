//! Pool-buffer loans to the dedicated granular worker, owned by the ordinary
//! stream worker. No callback endpoint or alternate publication path is created.
use super::*;
use crate::{
    AllocationData, GranularWorkerAllowance, GranularWorkerBank, GranularWorkerControl,
    GranularWorkerData, GranularWorkerKey, GranularWorkerOutcome, GranularWorkerRequest,
    GranularWorkerResult, GranularWorkerSeek, ResourcePageKey, StreamRuntimeError,
};

/// Listed granular DATA retained beside the ordinary worker. Earlier source
/// preparation, caller PCM pool and opaque/native/thread costs remain separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GranularRuntimeData {
    pub worker: GranularWorkerData,
    pub metadata: AllocationData,
    pub loans: AllocationData,
}
impl GranularRuntimeData {
    pub fn bytes(self) -> Option<usize> {
        crate::footprint::sum([
            self.worker.bytes()?,
            self.metadata.bytes()?,
            self.loans.bytes()?,
        ])
    }
}
struct Loan {
    key: GranularWorkerKey,
    pointer: usize,
    page: Box<PageBuffer>,
}
pub(crate) struct PreparedGranular {
    bank: GranularWorkerBank,
    capacity: usize,
    allowance: GranularWorkerAllowance,
    metadata: Vec<StreamedSourceMetadata>,
    loans: Vec<Loan>,
    loan_limit: usize,
    data: GranularRuntimeData,
    #[cfg(feature = "test-support")]
    pub(crate) gate: Option<crate::test_support::PageGate>,
}
impl PreparedGranular {
    pub(crate) fn new(
        bank: GranularWorkerBank,
        config: StreamConfig,
        allowance: GranularWorkerAllowance,
    ) -> Result<Self, StreamRuntimeError> {
        if config.page_buffers < 3 {
            return Err(StreamRuntimeError::InvalidConfiguration);
        }
        let capacity = config.page_buffers;
        let worker = GranularWorkerControl::data_bound(&bank, capacity)?;
        if worker.bytes().is_none_or(|n| n > allowance.max_data_bytes)
            || worker
                .shared_handle_bound()
                .is_none_or(|n| n > allowance.max_shared_handles)
        {
            return Err(StreamRuntimeError::InvalidConfiguration);
        }
        let mut metadata = Vec::new();
        metadata
            .try_reserve_exact(bank.metadata().len())
            .map_err(|_| StreamRuntimeError::AllocationFailed)?;
        metadata.extend(bank.metadata().cloned());
        metadata.sort_unstable_by_key(StreamedSourceMetadata::key);
        let mut loans = Vec::new();
        loans
            .try_reserve_exact(config.page_buffers)
            .map_err(|_| StreamRuntimeError::AllocationFailed)?;
        let data = GranularRuntimeData {
            worker,
            metadata: AllocationData::of::<StreamedSourceMetadata>(metadata.capacity()),
            loans: AllocationData::of::<Loan>(loans.capacity()),
        };
        data.bytes()
            .ok_or(StreamRuntimeError::InvalidConfiguration)?;
        Ok(Self {
            bank,
            capacity,
            allowance,
            metadata,
            loans,
            loan_limit: 1,
            data,
            #[cfg(feature = "test-support")]
            gate: None,
        })
    }
    pub(crate) fn metadata(&self) -> &[StreamedSourceMetadata] {
        &self.metadata
    }
    pub(crate) fn start(mut self, stop: &AtomicBool) -> Result<GranularRouting, StreamFailureKind> {
        self.bank.validate_for_catalog(stop)?;
        let control = GranularWorkerControl::try_start_inner(
            self.bank,
            self.capacity,
            self.allowance,
            #[cfg(feature = "test-support")]
            self.gate,
        )
        .map_err(|_| StreamFailureKind::CapacityExhausted)?;
        Ok(GranularRouting {
            control,
            metadata: self.metadata,
            loans: self.loans,
            loan_limit: self.loan_limit,
            data: self.data,
            fault: false,
        })
    }
}
pub(crate) struct GranularRouting {
    control: GranularWorkerControl,
    metadata: Vec<StreamedSourceMetadata>,
    loans: Vec<Loan>,
    loan_limit: usize,
    data: GranularRuntimeData,
    fault: bool,
}
impl GranularRouting {
    pub(crate) fn metadata(&self) -> &[StreamedSourceMetadata] {
        &self.metadata
    }
    pub(crate) fn data(&self) -> GranularRuntimeData {
        self.data
    }
    pub(crate) fn faulted(&self) -> bool {
        self.fault || self.control.worker_finished()
    }
    pub(crate) fn contains(&self, key: hex_engine::StreamSourceKey) -> bool {
        self.metadata
            .binary_search_by_key(&key, StreamedSourceMetadata::key)
            .is_ok()
    }
    pub(crate) fn submit(
        &mut self,
        key: ResourcePageKey,
        mut page: Box<PageBuffer>,
    ) -> Result<(), Box<PageBuffer>> {
        let StreamResourceKey::Audio(source) = key.source else {
            return Err(page);
        };
        // One original buffer remains outside granular custody so the
        // ordinary worker can continue raw/scalar service during replay.
        if self.loans.len() >= self.loan_limit {
            return Err(page);
        }
        let frames = std::mem::take(&mut page.frames);
        let request = GranularWorkerRequest {
            key: GranularWorkerKey {
                source,
                page_index: key.page_index,
                epoch: self.control.epoch(),
                seek: GranularWorkerSeek::Segmented {
                    segment_frames: crate::MAX_GRANULAR_PREROLL_FRAMES,
                },
            },
            frames,
        };
        let pointer = request.frames.as_ptr() as usize;
        let worker_key = request.key;
        match self.control.try_submit_on_control(request) {
            Ok(()) => {
                self.loans.push(Loan {
                    key: worker_key,
                    pointer,
                    page,
                });
                Ok(())
            }
            Err(request) => {
                page.frames = request.frames;
                Err(page)
            }
        }
    }
    pub(crate) fn poll(&mut self) -> Option<(Box<PageBuffer>, Result<(), StreamFailureKind>)> {
        let outcome = self.control.poll_on_control()?;
        let key = outcome.key;
        let (frames, valid, status) = match outcome.result {
            GranularWorkerResult::Ready(page) => {
                let valid = page.valid_frames;
                let matches = page.metadata.key() == key.source
                    && page.page_index == key.page_index
                    && valid > 0
                    && valid <= page.frames.len();
                (
                    page.frames,
                    valid,
                    if matches {
                        Ok(())
                    } else {
                        Err(StreamFailureKind::DecodeFailed)
                    },
                )
            }
            GranularWorkerResult::Cancelled { frames } => {
                (frames, 0, Err(StreamFailureKind::Cancelled))
            }
            GranularWorkerResult::Failed { frames, error } => (frames, 0, Err(map_error(error))),
        };
        let mut page = self.restore(key, frames)?;
        page.key = Some(ResourcePageKey {
            source: key.source.into(),
            page_index: key.page_index,
        });
        page.valid_frames = valid;
        Some((page, status))
    }
    fn restore(
        &mut self,
        key: GranularWorkerKey,
        frames: Box<[hex_engine::Frame]>,
    ) -> Option<Box<PageBuffer>> {
        let pointer = frames.as_ptr() as usize;
        let Some(index) = self
            .loans
            .iter()
            .position(|loan| loan.key == key && loan.pointer == pointer)
        else {
            self.fault = true;
            return None;
        };
        let mut page = self.loans.swap_remove(index).page;
        page.frames = frames;
        Some(page)
    }
    pub(crate) fn stop_into(
        mut self,
        free: &mut Vec<Box<PageBuffer>>,
    ) -> Result<(), StreamRuntimeError> {
        // The child joins before any original pool header is destroyed. The
        // parent worker returns all restored pages to the original pair's join.
        let mut stopped = self.control.shutdown_on_control()?;
        let mut returned = |key, frames: Box<[hex_engine::Frame]>| {
            let pointer = frames.as_ptr() as usize;
            if let Some(index) = self
                .loans
                .iter()
                .position(|loan| loan.key == key && loan.pointer == pointer)
            {
                let mut page = self.loans.swap_remove(index).page;
                page.frames = frames;
                page.key = None;
                page.valid_frames = 0;
                free.push(page);
            }
        };
        while let Ok(outcome) = stopped.results.pop() {
            let (key, frames) = outcome_frames(outcome);
            returned(key, frames);
        }
        if let Some(outcome) = stopped.worker.held.take() {
            let (key, frames) = outcome_frames(outcome);
            returned(key, frames);
        }
        if let Some(request) = stopped.worker.interrupted.take() {
            returned(request.key, request.frames);
        }
        while let Ok(request) = stopped.worker.queues.requests.pop() {
            returned(request.key, request.frames);
        }
        if self.fault
            || !self.loans.is_empty()
            || stopped.worker.reason != crate::GranularWorkerExitReason::Stopped
        {
            return Err(StreamRuntimeError::PoolAccounting);
        }
        Ok(())
    }
}
fn outcome_frames(outcome: GranularWorkerOutcome) -> (GranularWorkerKey, Box<[hex_engine::Frame]>) {
    let frames = match outcome.result {
        GranularWorkerResult::Ready(page) => page.frames,
        GranularWorkerResult::Cancelled { frames }
        | GranularWorkerResult::Failed { frames, .. } => frames,
    };
    (outcome.key, frames)
}
fn map_error(error: crate::GranularWorkerError) -> StreamFailureKind {
    match error {
        crate::GranularWorkerError::UnknownSource => StreamFailureKind::UnknownSource,
        crate::GranularWorkerError::Page(
            crate::GranularPageError::Step(crate::VerifiedGranularStepError::Source(kind))
            | crate::GranularPageError::Cache(crate::CheckpointCacheError::Seek(
                crate::VerifiedGranularStepError::Source(kind),
            )),
        ) => kind,
        crate::GranularWorkerError::Page(
            crate::GranularPageError::Warp(error)
            | crate::GranularPageError::Step(crate::VerifiedGranularStepError::Warp(error))
            | crate::GranularPageError::Cache(
                crate::CheckpointCacheError::Warp(error)
                | crate::CheckpointCacheError::Seek(crate::VerifiedGranularStepError::Warp(error)),
            ),
        ) => super::map_pitch_error(error),
        _ => StreamFailureKind::DecodeFailed,
    }
}
