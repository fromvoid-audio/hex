use crate::{
    endpoint::{Endpoint, EndpointQueues, PageBuffer},
    types::{
        CatalogFootprint, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind,
        StreamRuntimeError, VerifiedCatalog,
    },
    worker::{WorkerQueues, run_worker},
};
use hex_engine::{MediaPages, StreamSourceKey};
mod storage;
use rtrb::RingBuffer;
use std::{
    fmt,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread::{self, JoinHandle, Thread},
    time::Duration,
};
pub use storage::{
    AudioStreamStorageError, AudioStreamStorageLedger, AudioStreamStorageReservation,
    AudioStreamStorageSummary, OpaqueStreamCost, OpaqueStreamHandleGroup, StreamHandleLocation,
    StreamOwningHandleObligation, StreamSourceDomainObligation,
};
mod disposal;
pub use disposal::{PreparedStreamDisposal, StreamDisposalError, StreamDisposalPhase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagePoolReport {
    pub page_buffers: usize,
    pub page_bytes: usize,
    pub cached_pages_returned: usize,
    pub queued_pages_reclaimed: usize,
    pub held_return_pages_reclaimed: usize,
    pub worker_pages_reclaimed: usize,
    pub worker_joined: bool,
    pub accounting_complete: bool,
}

/// Control-side owner of worker lifecycle and the verified source catalog.
/// Keep it alive until every graph and callback endpoint using its generation
/// has been retired.
pub struct StreamControl {
    pair_id: Arc<()>,
    owner: Arc<CatalogOwner>,
}

/// Render-side borrowed page endpoint. It owns bounded cache state and retains
/// the shared catalog owner for its lifetime, but it exposes no worker
/// scheduling or join operations. It may be borrowed by the engine renderer;
/// its destruction, including a possible final owner drop/join, belongs on a
/// control thread.
pub struct StreamCallbackPages {
    pair_id: Arc<()>,
    endpoint: Endpoint,
    max_ready_pages_per_pump: usize,
    _owner: Arc<CatalogOwner>,
}

struct CatalogOwner {
    transport_data: crate::TransportData,
    config: StreamConfig,
    stop: Arc<AtomicBool>,
    worker_wake: Thread,
    worker: Mutex<Option<JoinHandle<Vec<Box<PageBuffer>>>>>,
    catalog_state: Mutex<CatalogState>,
    #[cfg(test)]
    worker_finished: Arc<AtomicBool>,
}

struct CatalogState {
    receiver: Receiver<Result<VerifiedCatalog, StreamFailureKind>>,
    result: Option<Result<Arc<VerifiedCatalog>, StreamFailureKind>>,
}

/// Opaque lifetime lease for a successfully verified source catalog. It must
/// be retained and reclaimed on a control thread, never in callback rendering.
pub struct CatalogLease {
    owner: Arc<CatalogOwner>,
    catalog: Arc<VerifiedCatalog>,
    pair_id: Arc<()>,
}

impl Clone for CatalogLease {
    fn clone(&self) -> Self {
        Self {
            owner: Arc::clone(&self.owner),
            catalog: Arc::clone(&self.catalog),
            pair_id: Arc::clone(&self.pair_id),
        }
    }
}

impl CatalogLease {
    /// True only when the complete verified metadata value, including
    /// generation and fingerprint, is present in this catalog.
    pub fn covers_source(&self, source: &hex_engine::StreamedSourceMetadata) -> bool {
        self.catalog
            .sources
            .binary_search_by_key(&source.key(), hex_engine::StreamedSourceMetadata::key)
            .ok()
            .and_then(|index| self.catalog.sources.get(index))
            .is_some_and(|member| member == source)
    }

    /// Distinguishes separate catalogs even when they reuse identical
    /// generation, asset IDs, and source metadata.
    pub fn same_catalog(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner) && Arc::ptr_eq(&self.pair_id, &other.pair_id)
    }

    pub fn footprint(&self) -> CatalogFootprint {
        self.catalog.footprint
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Typed ownership-preserving error returned when control/pages from different
/// runtimes are paired during shutdown.
pub struct StreamPairMismatch {
    control: StreamControl,
    pages: StreamCallbackPages,
}

/// A matching pair cannot shut down while any catalog lease remains. Both
/// halves are retained so the caller can retry after releasing graph leases.
pub struct StreamBusy {
    control: StreamControl,
    pages: StreamCallbackPages,
    external_references: usize,
}

impl StreamBusy {
    pub fn external_references(&self) -> usize {
        self.external_references
    }

    pub fn into_parts(self) -> (StreamControl, StreamCallbackPages) {
        (self.control, self.pages)
    }
}

impl StreamPairMismatch {
    pub fn into_parts(self) -> (StreamControl, StreamCallbackPages) {
        (self.control, self.pages)
    }
}

/// Shutdown can fail either before touching a mismatched pair, or after
/// matching shutdown encounters a worker/accounting failure.
pub enum ShutdownError {
    PairMismatch(Box<StreamPairMismatch>),
    Busy(Box<StreamBusy>),
    Runtime(StreamRuntimeError),
}

impl ShutdownError {
    /// Recover both halves for a mismatch or active catalog leases. Runtime
    /// teardown errors occur after shutdown has begun and cannot return parts.
    pub fn into_parts(self) -> Result<(StreamControl, StreamCallbackPages), Self> {
        match self {
            Self::PairMismatch(mismatch) => Ok((*mismatch).into_parts()),
            Self::Busy(busy) => Ok((*busy).into_parts()),
            other => Err(other),
        }
    }
}

impl fmt::Debug for ShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PairMismatch(_) => formatter.write_str("ShutdownError::PairMismatch"),
            Self::Busy(busy) => formatter
                .debug_tuple("ShutdownError::Busy")
                .field(&busy.external_references)
                .finish(),
            Self::Runtime(error) => formatter
                .debug_tuple("ShutdownError::Runtime")
                .field(error)
                .finish(),
        }
    }
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PairMismatch(_) => formatter.write_str("stream control and pages do not match"),
            Self::Busy(busy) => write!(
                formatter,
                "stream catalog still has {} external lease(s)",
                busy.external_references
            ),
            Self::Runtime(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ShutdownError {}

/// Compatibility facade owning both the control and callback halves.
pub struct StreamRuntime {
    control: Option<StreamControl>,
    pages: Option<StreamCallbackPages>,
}

impl StreamRuntime {
    pub fn new(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
    ) -> Result<Self, StreamRuntimeError> {
        let config = config.validate()?;
        if definitions.len() > config.max_sources {
            return Err(StreamRuntimeError::InvalidConfiguration);
        }
        let mut source_keys = Vec::new();
        source_keys
            .try_reserve_exact(definitions.len())
            .map_err(|_| StreamRuntimeError::AllocationFailed)?;
        for definition in &definitions {
            if definition.key != definition.metadata.key() || source_keys.contains(&definition.key)
            {
                return Err(StreamRuntimeError::InvalidConfiguration);
            }
            source_keys.push(definition.key);
        }
        source_keys.sort_unstable();
        let mut free_pages = Vec::new();
        free_pages
            .try_reserve_exact(config.page_buffers)
            .map_err(|_| StreamRuntimeError::AllocationFailed)?;
        let mut transport_data = crate::TransportData::default();
        for _ in 0..config.page_buffers {
            let (page, old_capacity) = PageBuffer::new_with_capacity()
                .map_err(|_| StreamRuntimeError::AllocationFailed)?;
            transport_data.page_objects = crate::footprint::add(
                transport_data.page_objects,
                std::mem::size_of_val(page.as_ref()),
            )?;
            let PageBuffer {
                key: _,
                frames,
                valid_frames: _,
            } = page.as_ref();
            transport_data.pcm_boxes = crate::footprint::add(
                transport_data.pcm_boxes,
                frames
                    .len()
                    .checked_mul(std::mem::size_of::<hex_engine::Frame>())
                    .ok_or(StreamRuntimeError::AllocationFailed)?,
            )?;
            transport_data.page_boxing_overlap = transport_data.page_boxing_overlap.max(
                old_capacity
                    .checked_mul(std::mem::size_of::<hex_engine::Frame>())
                    .ok_or(StreamRuntimeError::AllocationFailed)?,
            );
            free_pages.push(page);
        }
        let (request_tx, request_rx) = RingBuffer::new(config.request_capacity);
        let (ready_tx, ready_rx) = RingBuffer::new(config.ready_capacity);
        let (return_tx, return_rx) = RingBuffer::new(config.return_capacity);
        let (failure_tx, failure_rx) = RingBuffer::new(config.failure_capacity);
        transport_data.queue_slot_data_declared = crate::footprint::sum([
            request_tx
                .buffer()
                .capacity()
                .checked_mul(std::mem::size_of::<crate::endpoint::PageRequest>())
                .ok_or(StreamRuntimeError::AllocationFailed)?,
            ready_tx
                .buffer()
                .capacity()
                .checked_mul(std::mem::size_of::<Box<PageBuffer>>())
                .ok_or(StreamRuntimeError::AllocationFailed)?,
            return_tx
                .buffer()
                .capacity()
                .checked_mul(std::mem::size_of::<Box<PageBuffer>>())
                .ok_or(StreamRuntimeError::AllocationFailed)?,
            failure_tx
                .buffer()
                .capacity()
                .checked_mul(std::mem::size_of::<StreamFailure>())
                .ok_or(StreamRuntimeError::AllocationFailed)?,
        ])
        .ok_or(StreamRuntimeError::AllocationFailed)?;
        transport_data.queue_object_data = crate::footprint::sum([
            std::mem::size_of_val(request_tx.buffer()),
            std::mem::size_of_val(ready_tx.buffer()),
            std::mem::size_of_val(return_tx.buffer()),
            std::mem::size_of_val(failure_tx.buffer()),
        ])
        .ok_or(StreamRuntimeError::AllocationFailed)?;
        transport_data.queue_constructor_supported = crate::footprint::queue_family_supported();
        transport_data.catalog_owner_data = std::mem::size_of::<CatalogOwner>();
        transport_data.stop_flag_data = std::mem::size_of::<AtomicBool>();
        transport_data.pair_owner_data = std::mem::size_of::<()>();
        let (catalog_sender, catalog_result) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let worker_queues = WorkerQueues {
            requests: request_rx,
            ready: ready_tx,
            returns: return_rx,
            failures: failure_tx,
        };
        #[cfg(test)]
        let worker_finished = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let worker_finished_signal = Arc::clone(&worker_finished);
        #[cfg(test)]
        let test_panic = disposal::take_worker_test_panic();
        let worker = thread::Builder::new()
            .name("hex-stream-worker".to_owned())
            .spawn(move || {
                #[cfg(test)]
                if let Some(payload) = test_panic {
                    std::panic::resume_unwind(payload);
                }
                let pages = run_worker(
                    config,
                    definitions,
                    free_pages,
                    worker_queues,
                    thread_stop,
                    catalog_sender,
                );
                #[cfg(test)]
                worker_finished_signal.store(true, Ordering::Release);
                pages
            })
            .map_err(|_| StreamRuntimeError::WorkerSpawnFailed)?;
        let worker_wake = worker.thread().clone();
        let pair_id = Arc::new(());
        let endpoint = match Endpoint::new(
            config.page_buffers,
            config.request_capacity,
            source_keys,
            EndpointQueues {
                request_tx,
                ready_rx,
                return_tx,
                failure_rx,
            },
        ) {
            Ok(endpoint) => endpoint,
            Err(()) => {
                stop.store(true, Ordering::Release);
                worker_wake.unpark();
                let _ = worker.join();
                return Err(StreamRuntimeError::AllocationFailed);
            }
        };
        let owner = Arc::new(CatalogOwner {
            transport_data,
            config,
            stop,
            worker_wake,
            worker: Mutex::new(Some(worker)),
            catalog_state: Mutex::new(CatalogState {
                receiver: catalog_result,
                result: None,
            }),
            #[cfg(test)]
            worker_finished,
        });
        Ok(Self {
            control: Some(StreamControl {
                pair_id: Arc::clone(&pair_id),
                owner: Arc::clone(&owner),
            }),
            pages: Some(StreamCallbackPages {
                pair_id,
                endpoint,
                max_ready_pages_per_pump: config.max_ready_pages_per_pump,
                _owner: owner,
            }),
        })
    }

    /// Separate control lifecycle from the callback-borrowed page endpoint.
    /// A failed extraction returns this still-owning facade intact.
    // Returning the intact facade on an impossible partial state is part of
    // the consuming extraction contract, so this error intentionally owns both
    // large halves rather than boxing them or dropping one.
    #[allow(clippy::result_large_err)]
    pub fn split(mut self) -> Result<(StreamControl, StreamCallbackPages), Self> {
        match (self.control.take(), self.pages.take()) {
            (Some(control), Some(pages)) => Ok((control, pages)),
            (control, pages) => {
                self.control = control;
                self.pages = pages;
                Err(self)
            }
        }
    }

    pub fn wait_until_ready(&mut self, timeout: Duration) -> Result<(), StreamRuntimeError> {
        self.control
            .as_mut()
            .ok_or(StreamRuntimeError::WorkerUnavailable)?
            .wait_until_ready(timeout)
    }

    pub fn catalog_ready(&self) -> bool {
        self.control
            .as_ref()
            .is_some_and(StreamControl::catalog_ready)
    }

    pub fn catalog_footprint(&self) -> Option<CatalogFootprint> {
        self.control
            .as_ref()
            .and_then(StreamControl::catalog_footprint)
    }

    pub fn pump_ready(&mut self, limit: usize) -> usize {
        self.pages
            .as_mut()
            .map_or(0, |pages| pages.pump_ready(limit))
    }

    pub fn take_failures(&mut self, output: &mut [StreamFailure]) -> usize {
        self.pages
            .as_mut()
            .map_or(0, |pages| pages.take_failures(output))
    }

    pub fn stats(&self) -> crate::endpoint::EndpointStats {
        self.pages.as_ref().map_or_else(
            crate::endpoint::EndpointStats::default,
            StreamCallbackPages::stats,
        )
    }

    pub fn wake_worker(&self) {
        if let Some(control) = self.control.as_ref() {
            control.wake_worker();
        }
    }

    pub fn pin_pages(&mut self, keys: &[crate::PageKey]) -> bool {
        self.pages
            .as_mut()
            .is_some_and(|pages| pages.pin_pages(keys))
    }

    pub fn unpin_all(&mut self) {
        if let Some(pages) = self.pages.as_mut() {
            pages.unpin_all();
        }
    }

    /// Stops and joins the worker and explicitly accounts for every pool box.
    pub fn shutdown(mut self) -> Result<PagePoolReport, StreamRuntimeError> {
        let Some(control) = self.control.take() else {
            return Err(StreamRuntimeError::WorkerUnavailable);
        };
        let Some(pages) = self.pages.take() else {
            drop(control);
            return Err(StreamRuntimeError::WorkerUnavailable);
        };
        control.shutdown(pages).map_err(|error| match error {
            ShutdownError::PairMismatch(_) => StreamRuntimeError::InvalidConfiguration,
            ShutdownError::Busy(_) => StreamRuntimeError::CatalogBusy,
            ShutdownError::Runtime(error) => error,
        })
    }
}

impl StreamControl {
    /// Waits for verified descriptor opens and complete finite-signal admission.
    /// Call on a control thread, never from `StreamCallbackPages` rendering.
    pub fn wait_until_ready(&mut self, timeout: Duration) -> Result<(), StreamRuntimeError> {
        let mut state = lock_unpoisoned(&self.owner.catalog_state);
        if let Some(result) = state.result.as_ref() {
            return result
                .as_ref()
                .map(|_| ())
                .map_err(|error| StreamRuntimeError::CatalogFailed(*error));
        }
        match state.receiver.recv_timeout(timeout) {
            Ok(result) => {
                state.result = Some(result.map(Arc::new));
                state
                    .result
                    .as_ref()
                    .ok_or(StreamRuntimeError::WorkerUnavailable)?
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| StreamRuntimeError::CatalogFailed(*error))
            }
            Err(RecvTimeoutError::Timeout) => Err(StreamRuntimeError::CatalogNotReady),
            Err(RecvTimeoutError::Disconnected) => Err(StreamRuntimeError::WorkerUnavailable),
        }
    }

    pub fn catalog_ready(&self) -> bool {
        matches!(
            lock_unpoisoned(&self.owner.catalog_state).result.as_ref(),
            Some(Ok(_))
        )
    }

    pub fn catalog_footprint(&self) -> Option<CatalogFootprint> {
        lock_unpoisoned(&self.owner.catalog_state)
            .result
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map(|catalog| catalog.footprint)
    }

    /// Retain this exact verified catalog generation for a prepared graph.
    /// Catalog leases can only be created after complete worker admission.
    pub fn catalog_lease(&self) -> Result<CatalogLease, StreamRuntimeError> {
        let state = lock_unpoisoned(&self.owner.catalog_state);
        let catalog = match state.result.as_ref() {
            Some(Ok(catalog)) => Arc::clone(catalog),
            Some(Err(error)) => return Err(StreamRuntimeError::CatalogFailed(*error)),
            None => return Err(StreamRuntimeError::CatalogNotReady),
        };
        Ok(CatalogLease {
            owner: Arc::clone(&self.owner),
            catalog,
            pair_id: Arc::clone(&self.pair_id),
        })
    }

    /// Explicit worker wake for setup, preparation and shutdown paths.
    pub fn wake_worker(&self) {
        self.owner.worker_wake.unpark();
    }

    /// Stop and reclaim a matching callback page endpoint. Pair validation is
    /// performed before cached pages, queues or worker state are mutated.
    pub fn shutdown(self, mut pages: StreamCallbackPages) -> Result<PagePoolReport, ShutdownError> {
        if !Arc::ptr_eq(&self.pair_id, &pages.pair_id) {
            return Err(ShutdownError::PairMismatch(Box::new(StreamPairMismatch {
                control: self,
                pages,
            })));
        }
        // Exactly two references belong to this pair: control and pages. No
        // Weak references are exposed, and cloning a lease already requires
        // an external strong reference, so any other strong reference makes
        // shutdown Busy without a separate clone gate.
        let references = Arc::strong_count(&self.owner);
        if references > 2 {
            return Err(ShutdownError::Busy(Box::new(StreamBusy {
                control: self,
                pages,
                external_references: references - 2,
            })));
        }
        let cached_pages_returned = pages.endpoint.return_cached_pages();
        self.owner.stop.store(true, Ordering::Release);
        self.wake_worker();
        let (worker_pages, worker_joined) = {
            let mut worker_slot = lock_unpoisoned(&self.owner.worker);
            if let Some(worker) = worker_slot.take() {
                let pages = worker
                    .join()
                    .map_err(|_| ShutdownError::Runtime(StreamRuntimeError::WorkerJoinFailed))?;
                (pages, true)
            } else {
                (Vec::new(), false)
            }
        };
        let queued_pages_reclaimed = pages.endpoint.drain_ready_after_join();
        let held_return_pages_reclaimed = pages.endpoint.drop_held_returns_after_join();
        let worker_pages_reclaimed = worker_pages.len();
        drop(worker_pages);
        let accounted = queued_pages_reclaimed
            .saturating_add(held_return_pages_reclaimed)
            .saturating_add(worker_pages_reclaimed);
        let accounting_complete = accounted == self.owner.config.page_buffers;
        let report = PagePoolReport {
            page_buffers: self.owner.config.page_buffers,
            page_bytes: self
                .owner
                .config
                .page_buffers
                .saturating_mul(hex_engine::STREAM_PAGE_FRAMES)
                .saturating_mul(std::mem::size_of::<hex_engine::Frame>()),
            cached_pages_returned,
            queued_pages_reclaimed,
            held_return_pages_reclaimed,
            worker_pages_reclaimed,
            worker_joined,
            accounting_complete,
        };
        if !accounting_complete {
            return Err(ShutdownError::Runtime(StreamRuntimeError::PoolAccounting));
        }
        Ok(report)
    }
}

impl Drop for CatalogOwner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker_wake.unpark();
        let worker_slot = match self.worker.get_mut() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(worker) = worker_slot.take() {
            let _ = worker.join();
        }
    }
}

impl StreamCallbackPages {
    /// Pair a retained real catalog lease with this endpoint without cloning
    /// owners or upgrading metadata equality into allocation identity.
    pub fn matches_catalog(&self, lease: &CatalogLease) -> bool {
        Arc::ptr_eq(&self.pair_id, &lease.pair_id) && Arc::ptr_eq(&self._owner, &lease.owner)
    }
    /// Drain a bounded number of completed pages into the preallocated cache.
    pub fn pump_ready(&mut self, limit: usize) -> usize {
        self.endpoint
            .pump_ready(limit.min(self.max_ready_pages_per_pump))
    }

    pub fn take_failures(&mut self, output: &mut [StreamFailure]) -> usize {
        self.endpoint.take_failures(output)
    }

    pub fn stats(&self) -> crate::endpoint::EndpointStats {
        self.endpoint.stats()
    }

    pub fn pin_pages(&mut self, keys: &[crate::PageKey]) -> bool {
        self.endpoint.pin_pages(keys)
    }

    pub fn unpin_all(&mut self) {
        self.endpoint.unpin_all();
    }
}

impl MediaPages for StreamCallbackPages {
    fn page(
        &mut self,
        key: StreamSourceKey,
        page_index: u64,
    ) -> Option<hex_engine::BorrowedMediaPage<'_>> {
        self.endpoint.page(key, page_index)
    }

    fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
        self.endpoint.request_page(key, page_index);
    }
}

impl MediaPages for StreamRuntime {
    fn page(
        &mut self,
        key: StreamSourceKey,
        page_index: u64,
    ) -> Option<hex_engine::BorrowedMediaPage<'_>> {
        self.pages.as_mut()?.page(key, page_index)
    }

    fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
        if let Some(pages) = self.pages.as_mut() {
            pages.request_page(key, page_index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_catalog_lease_joins_worker_on_its_control_reclamation_thread() {
        let mut runtime =
            StreamRuntime::new(StreamConfig::with_pool_pages(3), Vec::new()).expect("runtime");
        runtime
            .wait_until_ready(Duration::from_secs(2))
            .expect("catalog ready");
        let (control, pages) = runtime.split().ok().expect("runtime split");
        let lease = control.catalog_lease().expect("verified catalog lease");
        let worker_finished = Arc::clone(&control.owner.worker_finished);

        drop(control);
        drop(pages);
        assert!(!worker_finished.load(Ordering::Acquire));

        thread::Builder::new()
            .name("hex-stream-control-reclaim-test".to_owned())
            .spawn(move || drop(lease))
            .expect("start control reclamation thread")
            .join()
            .expect("control reclamation thread joins");
        assert!(worker_finished.load(Ordering::Acquire));
    }
}
