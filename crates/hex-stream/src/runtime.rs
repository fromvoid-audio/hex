use crate::{
    endpoint::{Endpoint, EndpointQueues, PageBuffer},
    types::{
        CatalogFootprint, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind,
        StreamRuntimeError, VerifiedCatalog,
    },
    worker::{WorkerQueues, run_worker},
};
use hex_engine::{MediaPages, StreamSourceKey};
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
    config: StreamConfig,
    allocations: crate::TransportAllocationFootprint,
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
    pub(crate) fn same_verified_catalog(&self, other: &Self) -> bool {
        self.same_catalog(other) && Arc::ptr_eq(&self.catalog, &other.catalog)
    }
    pub(crate) fn source_metadata(
        &self,
        key: StreamSourceKey,
    ) -> Option<&hex_engine::StreamedSourceMetadata> {
        self.catalog
            .sources
            .binary_search_by_key(&key, hex_engine::StreamedSourceMetadata::key)
            .ok()
            .and_then(|index| self.catalog.sources.get(index))
    }
    pub(crate) fn matches_pages(&self, pages: &StreamCallbackPages) -> bool {
        Arc::ptr_eq(&self.owner, &pages._owner) && Arc::ptr_eq(&self.pair_id, &pages.pair_id)
    }

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

    /// Shared storage report, charged once per actual catalog, not per lease.
    pub fn allocation_footprint(&self) -> crate::CatalogAllocationFootprint {
        self.catalog.allocations
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
    pub(crate) fn from_construction(
        config: StreamConfig,
        allocations: crate::TransportAllocationFootprint,
        endpoint: Endpoint,
        stop: Arc<AtomicBool>,
        worker: JoinHandle<Vec<Box<PageBuffer>>>,
        catalog: VerifiedCatalog,
    ) -> Self {
        let worker_wake = worker.thread().clone();
        let (_, receiver) = mpsc::sync_channel(1);
        let owner = Arc::new(CatalogOwner {
            config,
            allocations,
            stop,
            worker_wake,
            worker: Mutex::new(Some(worker)),
            catalog_state: Mutex::new(CatalogState {
                receiver,
                result: Some(Ok(Arc::new(catalog))),
            }),
            #[cfg(test)]
            worker_finished: Arc::new(AtomicBool::new(false)),
        });
        let pair_id = Arc::new(());
        Self {
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
        }
    }
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
        let mut page_boxing_capacity = 0;
        for _ in 0..config.page_buffers {
            let (page, capacity) = PageBuffer::new_with_capacity()
                .map_err(|_| StreamRuntimeError::AllocationFailed)?;
            page_boxing_capacity = page_boxing_capacity.max(capacity);
            free_pages.push(page);
        }
        let (request_tx, request_rx) = RingBuffer::new(config.request_capacity);
        let (ready_tx, ready_rx) = RingBuffer::new(config.ready_capacity);
        let (return_tx, return_rx) = RingBuffer::new(config.return_capacity);
        let (failure_tx, failure_rx) = RingBuffer::new(config.failure_capacity);
        let pcm_frames = free_pages
            .iter()
            .try_fold(0_usize, |total, page| total.checked_add(page.frames.len()))
            .ok_or(StreamRuntimeError::InvalidConfiguration)?;
        let allocations = crate::TransportAllocationFootprint {
            page_objects: crate::AllocationCapacity::of::<PageBuffer>(free_pages.len()),
            pcm_frames: crate::AllocationCapacity::of::<hex_engine::Frame>(pcm_frames),
            request_slots: crate::AllocationCapacity::of::<crate::endpoint::PageRequest>(
                request_tx.buffer().capacity(),
            ),
            ready_slots: crate::AllocationCapacity::of::<Box<PageBuffer>>(
                ready_tx.buffer().capacity(),
            ),
            return_slots: crate::AllocationCapacity::of::<Box<PageBuffer>>(
                return_tx.buffer().capacity(),
            ),
            failure_slots: crate::AllocationCapacity::of::<StreamFailure>(
                failure_tx.buffer().capacity(),
            ),
            queue_object_bytes: std::mem::size_of::<RingBuffer<crate::endpoint::PageRequest>>()
                .checked_add(std::mem::size_of::<RingBuffer<Box<PageBuffer>>>())
                .and_then(|v| v.checked_add(std::mem::size_of::<RingBuffer<Box<PageBuffer>>>()))
                .and_then(|v| v.checked_add(std::mem::size_of::<RingBuffer<StreamFailure>>()))
                .ok_or(StreamRuntimeError::InvalidConfiguration)?,
            catalog_owner_object_bytes: std::mem::size_of::<CatalogOwner>(),
            stop_flag_object_bytes: std::mem::size_of::<AtomicBool>(),
            page_boxing_peak_extra: crate::AllocationCapacity::of::<hex_engine::Frame>(
                page_boxing_capacity,
            ),
        };
        allocations
            .bytes()
            .ok_or(StreamRuntimeError::InvalidConfiguration)?;
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
        let worker = thread::Builder::new()
            .name("hex-stream-worker".to_owned())
            .spawn(move || {
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
            config,
            allocations,
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

    /// Control-side snapshot of actual reserved capacities after verified
    /// catalog admission. Takes the existing catalog lock but creates no lease
    /// or owner. Use the verified endpoint's lock-free query when borrowing pages.
    pub fn allocation_footprint(&self) -> Option<crate::StreamAllocationFootprint> {
        let control = self.control.as_ref()?;
        let pages = self.pages.as_ref()?;
        let state = lock_unpoisoned(&control.owner.catalog_state);
        let catalog = state.result.as_ref()?.as_ref().ok()?;
        Some(crate::StreamAllocationFootprint {
            endpoint: pages.allocation_footprint(),
            transport: control.owner.allocations,
            catalog: catalog.allocations,
            construction: catalog.construction,
            legacy_catalog: catalog.footprint,
        })
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

pub(crate) fn catalog_owner_data_bytes() -> usize {
    std::mem::size_of::<CatalogOwner>()
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
    pub fn shutdown(self, pages: StreamCallbackPages) -> Result<PagePoolReport, ShutdownError> {
        let result = self.shutdown_reported(pages)?;
        result.successful_report().ok_or(ShutdownError::Runtime(
            result.failure.unwrap_or(StreamRuntimeError::PoolAccounting),
        ))
    }

    /// Actual borrowed owner/pair identity; creates no lease or owner.
    pub(crate) fn matches_catalog(&self, lease: &CatalogLease) -> bool {
        Arc::ptr_eq(&self.owner, &lease.owner) && Arc::ptr_eq(&self.pair_id, &lease.pair_id)
    }

    /// Control-only exact immutable catalog check, without manufacturing another
    /// lease. This briefly borrows the existing catalog-state mutex; never call
    /// it from callback demand/render or retain the guard across cleanup.
    pub(crate) fn matches_verified_catalog(&self, lease: &CatalogLease) -> bool {
        self.matches_catalog(lease)
            && self.owner.catalog_state.lock().ok().is_some_and(|state| {
                state.result.as_ref().is_some_and(|result| match result {
                    Ok(catalog) => Arc::ptr_eq(catalog, &lease.catalog),
                    Err(_) => false,
                })
            })
    }

    pub(crate) fn owner_references(&self) -> usize {
        Arc::strong_count(&self.owner)
    }

    /// Owning preflight errors remain transactional. Terminal results retain
    /// observations even after panic/imbalance; unknown worker inventory is None.
    /// Control-only: may stop, wait for IO cancellation and join without deadline.
    pub fn shutdown_reported(
        self,
        mut pages: StreamCallbackPages,
    ) -> Result<crate::ReportedShutdown, ShutdownError> {
        if !Arc::ptr_eq(&self.pair_id, &pages.pair_id) || !Arc::ptr_eq(&self.owner, &pages._owner) {
            return Err(ShutdownError::PairMismatch(Box::new(StreamPairMismatch {
                control: self,
                pages,
            })));
        }
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
        let (worker_pages, join) = {
            let mut slot = lock_unpoisoned(&self.owner.worker);
            match slot.take() {
                Some(worker) => match worker.join() {
                    Ok(pages) => (Some(pages), crate::WorkerJoinObservation::Joined),
                    Err(_) => (None, crate::WorkerJoinObservation::Panicked),
                },
                None => (None, crate::WorkerJoinObservation::MissingHandle),
            }
        };
        // The joined/panicked worker has stopped. The ready/held inventories can
        // still be observed/reclaimed, including after worker unwind. With no
        // handle this is only known endpoint inventory, never proof of a join.
        let queued_pages_reclaimed = pages.endpoint.drain_ready_after_join();
        let held_return_pages_reclaimed = pages.endpoint.drop_held_returns_after_join();
        let worker_pages_reclaimed = worker_pages.as_ref().map(Vec::len);
        drop(worker_pages);
        let conservation = worker_pages_reclaimed.and_then(|worker| {
            queued_pages_reclaimed
                .checked_add(held_return_pages_reclaimed)?
                .checked_add(worker)
                .map(|count| count == self.owner.config.page_buffers)
        });
        let failure = match join {
            crate::WorkerJoinObservation::Panicked => Some(StreamRuntimeError::WorkerJoinFailed),
            _ if conservation != Some(true) => Some(StreamRuntimeError::PoolAccounting),
            _ => None,
        };
        Ok(crate::ReportedShutdown {
            page_buffers: self.owner.config.page_buffers,
            page_bytes: self.owner.config.page_buffers
                * hex_engine::STREAM_PAGE_FRAMES
                * std::mem::size_of::<hex_engine::Frame>(),
            cached_pages_returned,
            queued_pages_reclaimed,
            held_return_pages_reclaimed,
            worker_pages_reclaimed,
            join,
            conservation,
            failure,
        })
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
    pub(crate) fn readiness_endpoint(&mut self) -> &mut Endpoint {
        &mut self.endpoint
    }
    /// Endpoint-exclusive reserved capacities. Shared pages/queue payloads are
    /// reported separately and must not be charged again for cached pointers.
    pub fn allocation_footprint(&self) -> crate::EndpointAllocationFootprint {
        self.endpoint.allocation_footprint()
    }

    pub(crate) fn complete_footprint(
        &self,
        lease: &CatalogLease,
    ) -> crate::StreamAllocationFootprint {
        crate::StreamAllocationFootprint {
            endpoint: self.allocation_footprint(),
            transport: self._owner.allocations,
            catalog: lease.catalog.allocations,
            construction: lease.catalog.construction,
            legacy_catalog: lease.catalog.footprint,
        }
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
    fn endpoint_binding_checks_owner_and_pair_independently_without_mutation() {
        let mut runtime = StreamRuntime::new(StreamConfig::with_pool_pages(3), Vec::new()).unwrap();
        runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
        let (control, mut pages) = runtime.split().ok().unwrap();
        let lease = control.catalog_lease().unwrap();
        let original_pair = Arc::clone(&pages.pair_id);
        pages.pair_id = Arc::new(());
        let error = crate::VerifiedPageEndpoint::new(pages, lease)
            .err()
            .unwrap();
        let (mut pages, lease) = error.into_parts();
        assert!(!lease.matches_pages(&pages));
        assert!(!control.owner.stop.load(Ordering::Acquire));
        assert_eq!(pages.stats(), Default::default());
        pages.pair_id = original_pair;
        let mut other = StreamRuntime::new(StreamConfig::with_pool_pages(3), Vec::new()).unwrap();
        other.wait_until_ready(Duration::from_secs(2)).unwrap();
        let (other_control, other_pages) = other.split().ok().unwrap();
        let original_owner = Arc::clone(&pages._owner);
        pages._owner = Arc::clone(&other_control.owner);
        let error = crate::VerifiedPageEndpoint::new(pages, lease)
            .err()
            .unwrap();
        let (mut pages, lease) = error.into_parts();
        assert!(Arc::ptr_eq(&pages.pair_id, &control.pair_id));
        assert!(!lease.matches_pages(&pages));
        pages._owner = original_owner;
        let references = Arc::strong_count(&control.owner);
        let endpoint = crate::VerifiedPageEndpoint::new(pages, lease).unwrap();
        assert_eq!(Arc::strong_count(&control.owner), references);
        let (pages, lease) = endpoint.into_parts();
        let error = control.shutdown(pages).unwrap_err();
        let (control, pages) = error.into_parts().unwrap();
        assert!(!control.owner.stop.load(Ordering::Acquire));
        drop(lease);
        let report = control.shutdown(pages).unwrap();
        assert!(report.worker_joined && report.accounting_complete);
        let report = other_control.shutdown(other_pages).unwrap();
        assert!(report.worker_joined && report.accounting_complete);
    }

    #[test]
    fn final_graph_catalog_binding_joins_worker_on_its_control_reclamation_thread() {
        let mut runtime =
            StreamRuntime::new(StreamConfig::with_pool_pages(3), Vec::new()).expect("runtime");
        runtime
            .wait_until_ready(Duration::from_secs(2))
            .expect("catalog ready");
        let (control, pages) = runtime.split().ok().expect("runtime split");
        let lease = control.catalog_lease().expect("verified catalog lease");
        let project = hex_project::Project::new(48_000).expect("project");
        let graph = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .expect("graph"),
        );
        let binding = crate::VerifiedStreamRevision::new(graph, lease).expect("binding");
        let worker_finished = Arc::clone(&control.owner.worker_finished);

        drop(control);
        drop(pages);
        assert!(!worker_finished.load(Ordering::Acquire));

        thread::Builder::new()
            .name("hex-stream-control-reclaim-test".to_owned())
            .spawn(move || drop(binding))
            .expect("start control reclamation thread")
            .join()
            .expect("control reclamation thread joins");
        assert!(worker_finished.load(Ordering::Acquire));
    }
}
