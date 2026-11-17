use crate::{
    endpoint::{Endpoint, EndpointQueues, PageBuffer},
    types::{
        CatalogFootprint, ScalarSourceDefinition, SourceDefinition, StreamConfig, StreamFailure,
        StreamFailureKind, StreamRuntimeError, VerifiedCatalog,
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
mod readiness;
pub use readiness::{
    ConstructorPageLoan, CurrentQuantumRefusal, CurrentQuantumReservation, PreparedRolePromotion,
    ProtectedStreamPages, ProtectionRole, ReadinessArtifact, ReadinessProgress, ReadinessRefusal,
    ReadinessStamp, StreamReadinessReservation,
};
mod native_custody;
pub use native_custody::{
    ConstructorStreamDomain, StreamDomainConstructorSummary, StreamDomainSealRefusal,
};
mod pending;
pub use pending::{
    GranularStreamPreparationAllowance, PendingStreamDomain, StreamPreparationAllowance,
    StreamPreparationBound,
};
mod disposal;
#[cfg(test)]
mod read_outcome_tests;
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
    worker: Mutex<Option<JoinHandle<crate::worker::WorkerExit>>>,
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

    /// Exact verified scalar facts, including content and projection identity.
    pub fn covers_scalar_source(&self, source: &hex_engine::ScalarSourceMetadata) -> bool {
        self.catalog
            .scalar_sources
            .binary_search_by_key(&source.key(), hex_engine::ScalarSourceMetadata::key)
            .ok()
            .and_then(|index| self.catalog.scalar_sources.get(index))
            .is_some_and(|actual| actual == source)
    }
    pub fn scalar_sources(&self) -> &[hex_engine::ScalarSourceMetadata] {
        &self.catalog.scalar_sources
    }
    pub fn scalar_shared_data(&self) -> &[crate::ScalarSharedData] {
        &self.catalog.scalar_shared_data
    }

    /// Key membership for ambiguity checks after complete fact admission.
    pub fn covers_resource_key(&self, key: hex_engine::StreamResourceKey) -> bool {
        match key {
            hex_engine::StreamResourceKey::Audio(key) => self
                .catalog
                .sources
                .binary_search_by_key(&key, hex_engine::StreamedSourceMetadata::key)
                .is_ok(),
            hex_engine::StreamResourceKey::Scalar(key) => self
                .catalog
                .scalar_sources
                .binary_search_by_key(&key, hex_engine::ScalarSourceMetadata::key)
                .is_ok(),
        }
    }
    /// Distinguishes actual catalogs even when all source facts are identical.
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
        Self::new_inner(
            config,
            definitions,
            #[cfg(test)]
            None,
            #[cfg(feature = "test-support")]
            None,
        )
    }

    /// Construct a real stream worker with one optional test-only page hold.
    #[cfg(feature = "test-support")]
    pub fn new_with_page_gate(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        gate: crate::test_support::PageGate,
    ) -> Result<Self, StreamRuntimeError> {
        Self::new_inner(
            config,
            definitions,
            #[cfg(test)]
            None,
            Some(gate),
        )
    }

    fn new_inner(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        #[cfg(test)] test_hooks: Option<crate::worker::TestWorkerHooks>,
        #[cfg(feature = "test-support")] page_gate: Option<crate::test_support::PageGate>,
    ) -> Result<Self, StreamRuntimeError> {
        Self::new_factory(
            config,
            definitions,
            false,
            #[cfg(test)]
            test_hooks,
            #[cfg(feature = "test-support")]
            page_gate,
        )
        .map(|(runtime, _)| runtime)
    }
    /// Mixed resources use the same worker, queues, fixed page pool and exact
    /// pair disposal contract. Scalar readers/projections move to the worker.
    pub fn spawn_with_scalar_sources(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        scalar_definitions: Vec<ScalarSourceDefinition>,
    ) -> Result<Self, StreamRuntimeError> {
        Self::new_factory_with_scalar_sources(
            config,
            definitions,
            scalar_definitions,
            false,
            #[cfg(test)]
            None,
            #[cfg(feature = "test-support")]
            None,
        )
        .map(|(runtime, _)| runtime)
    }
    fn new_factory(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        reserve_disposal: bool,
        #[cfg(test)] test_hooks: Option<crate::worker::TestWorkerHooks>,
        #[cfg(feature = "test-support")] page_gate: Option<crate::test_support::PageGate>,
    ) -> Result<(Self, Option<PreparedStreamDisposal>), StreamRuntimeError> {
        Self::new_factory_with_scalar_sources(
            config,
            definitions,
            Vec::new(),
            reserve_disposal,
            #[cfg(test)]
            test_hooks,
            #[cfg(feature = "test-support")]
            page_gate,
        )
    }
    fn new_factory_with_scalar_sources(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        scalar_definitions: Vec<ScalarSourceDefinition>,
        reserve_disposal: bool,
        #[cfg(test)] test_hooks: Option<crate::worker::TestWorkerHooks>,
        #[cfg(feature = "test-support")] page_gate: Option<crate::test_support::PageGate>,
    ) -> Result<(Self, Option<PreparedStreamDisposal>), StreamRuntimeError> {
        Self::new_factory_with_granular(
            config,
            definitions,
            scalar_definitions,
            None,
            reserve_disposal,
            #[cfg(test)]
            test_hooks,
            #[cfg(feature = "test-support")]
            page_gate,
        )
    }
    /// Route an already prepared/admitted granular bank through this runtime's
    /// existing endpoint and original page pool, alongside raw/scalar sources.
    /// The allowance covers the dedicated worker's listed DATA/known handles;
    /// ordinary runtime and loan metadata DATA remain separate. This blocking
    /// control constructor is not pending aggregate admission. Hosts should use
    /// PendingStreamDomain's combined granular preparation allowance.
    /// At least three pool buffers are required: one granular loan, cached
    /// content and ordinary turnover. Granular admission keeps only one loan
    /// active and reduces the endpoint cache limit by one.
    pub fn spawn_with_granular_sources(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        scalar_definitions: Vec<ScalarSourceDefinition>,
        bank: crate::GranularWorkerBank,
        allowance: crate::GranularWorkerAllowance,
    ) -> Result<Self, StreamRuntimeError> {
        let config = config.validate()?;
        let granular = crate::worker::granular::PreparedGranular::new(bank, config, allowance)?;
        Self::new_factory_with_granular(
            config,
            definitions,
            scalar_definitions,
            Some(granular),
            false,
            #[cfg(test)]
            None,
            #[cfg(feature = "test-support")]
            None,
        )
        .map(|(runtime, _)| runtime)
    }
    /// Hold the dedicated granular worker before its first active step. Stop
    /// releases this real-worker test hold; raw/scalar service remains live.
    #[cfg(feature = "test-support")]
    pub fn new_with_granular_page_gate(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        scalar_definitions: Vec<ScalarSourceDefinition>,
        bank: crate::GranularWorkerBank,
        allowance: crate::GranularWorkerAllowance,
        gate: crate::test_support::PageGate,
    ) -> Result<Self, StreamRuntimeError> {
        let config = config.validate()?;
        let mut granular = crate::worker::granular::PreparedGranular::new(bank, config, allowance)?;
        granular.gate = Some(gate);
        Self::new_factory_with_granular(
            config,
            definitions,
            scalar_definitions,
            Some(granular),
            false,
            #[cfg(test)]
            None,
            None,
        )
        .map(|(runtime, _)| runtime)
    }
    #[allow(clippy::too_many_arguments)] // Closed source owners plus test-only gates.
    fn new_factory_with_granular(
        config: StreamConfig,
        definitions: Vec<SourceDefinition>,
        scalar_definitions: Vec<ScalarSourceDefinition>,
        granular: Option<crate::worker::granular::PreparedGranular>,
        reserve_disposal: bool,
        #[cfg(test)] test_hooks: Option<crate::worker::TestWorkerHooks>,
        #[cfg(feature = "test-support")] page_gate: Option<crate::test_support::PageGate>,
    ) -> Result<(Self, Option<PreparedStreamDisposal>), StreamRuntimeError> {
        let config = config.validate()?;
        let granular_count = granular.as_ref().map_or(0, |g| g.metadata().len());
        if definitions
            .len()
            .checked_add(scalar_definitions.len())
            .and_then(|n| n.checked_add(granular_count))
            .is_none_or(|count| count > config.max_sources)
        {
            return Err(StreamRuntimeError::InvalidConfiguration);
        }
        crate::worker::preparation_data_bound_with_scalar_sources(
            config,
            &definitions,
            definitions.capacity(),
            &scalar_definitions,
            scalar_definitions.capacity(),
        )
        .ok_or(StreamRuntimeError::InvalidConfiguration)?;
        let mut source_keys = Vec::new();
        source_keys
            .try_reserve_exact(definitions.len() + scalar_definitions.len() + granular_count)
            .map_err(|_| StreamRuntimeError::AllocationFailed)?;
        for definition in &definitions {
            if definition.key != definition.metadata.key()
                || source_keys.contains(&hex_engine::StreamResourceKey::from(definition.key))
            {
                return Err(StreamRuntimeError::InvalidConfiguration);
            }
            source_keys.push(definition.key.into());
        }
        for definition in &scalar_definitions {
            let key = hex_engine::StreamResourceKey::from(definition.metadata().key());
            if source_keys.contains(&key) {
                return Err(StreamRuntimeError::InvalidConfiguration);
            }
            source_keys.push(key);
        }
        if let Some(granular) = &granular {
            for source in granular.metadata() {
                let key = hex_engine::StreamResourceKey::from(source.key());
                if source_keys.contains(&key) {
                    return Err(StreamRuntimeError::InvalidConfiguration);
                }
                source_keys.push(key);
            }
        }
        source_keys.sort_unstable();
        let mut free_pages = Vec::new();
        free_pages
            .try_reserve_exact(config.page_buffers)
            .map_err(|_| StreamRuntimeError::AllocationFailed)?;
        let mut transport_data = crate::TransportData::default();
        for _ in 0..config.page_buffers {
            let (page, old_capacity) =
                PageBuffer::new_with_capacity(!scalar_definitions.is_empty())
                    .map_err(|_| StreamRuntimeError::AllocationFailed)?;
            transport_data.page_objects = crate::footprint::add(
                transport_data.page_objects,
                std::mem::size_of_val(page.as_ref()),
            )?;
            let PageBuffer {
                key: _,
                frames,
                scalar_values,
                scalar_coverage,
                valid_frames: _,
            } = page.as_ref();
            transport_data.pcm_boxes = crate::footprint::add(
                transport_data.pcm_boxes,
                frames
                    .len()
                    .checked_mul(std::mem::size_of::<hex_engine::Frame>())
                    .ok_or(StreamRuntimeError::AllocationFailed)?,
            )?;
            transport_data.scalar_boxes = crate::footprint::add(
                transport_data.scalar_boxes,
                scalar_values
                    .len()
                    .checked_mul(std::mem::size_of::<f32>())
                    .ok_or(StreamRuntimeError::AllocationFailed)?,
            )?;
            transport_data.scalar_coverage_boxes = crate::footprint::add(
                transport_data.scalar_coverage_boxes,
                scalar_coverage
                    .len()
                    .checked_mul(std::mem::size_of::<u64>())
                    .ok_or(StreamRuntimeError::AllocationFailed)?,
            )?;
            transport_data.page_boxing_overlap =
                transport_data.page_boxing_overlap.max(old_capacity);
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
        let pair_id = Arc::new(());
        let mut endpoint = Endpoint::new(
            config.page_buffers,
            config.request_capacity,
            source_keys,
            EndpointQueues {
                request_tx,
                ready_rx,
                return_tx,
                failure_rx,
            },
        )
        .map_err(|_| StreamRuntimeError::AllocationFailed)?;
        if granular.is_some() {
            endpoint
                .reserve_granular_loan_on_control()
                .map_err(|_| StreamRuntimeError::InvalidConfiguration)?;
        }
        let disposal = reserve_disposal
            .then(|| {
                PreparedStreamDisposal::reserve_for_pair(Arc::clone(&pair_id), config.page_buffers)
            })
            .transpose()
            .map_err(|_| StreamRuntimeError::AllocationFailed)?;
        if reserve_disposal {
            // The caller's requested allowance was checked before allocation.
            // Now validate the actual control-side capacities before a live
            // JoinHandle can exist. Worker payload is separately capped by its
            // exact source/manifest/kernel/scratch constructor limits.
            let bound = PendingStreamDomain::preparation_bound_inner(
                config,
                &definitions,
                definitions.capacity(),
                &scalar_definitions,
                scalar_definitions.capacity(),
                granular.as_ref().map(|prepared| prepared.bank()),
                None,
            )?;
            let (worker_data, worker_peak) =
                crate::worker::preparation_data_bound_with_scalar_sources(
                    config,
                    &definitions,
                    definitions.capacity(),
                    &scalar_definitions,
                    scalar_definitions.capacity(),
                )
                .ok_or(StreamRuntimeError::InvalidConfiguration)?;
            let free_handles = free_pages
                .capacity()
                .checked_sub(config.page_buffers)
                .and_then(|n| n.checked_mul(std::mem::size_of::<Box<PageBuffer>>()))
                .ok_or(StreamRuntimeError::AllocationFailed)?;
            let common = crate::footprint::sum([
                endpoint
                    .data_footprint()
                    .bytes()
                    .ok_or(StreamRuntimeError::InvalidConfiguration)?,
                transport_data
                    .bytes()
                    .ok_or(StreamRuntimeError::InvalidConfiguration)?,
                disposal
                    .as_ref()
                    .and_then(PreparedStreamDisposal::metadata_bytes)
                    .ok_or(StreamRuntimeError::InvalidConfiguration)?,
                free_handles,
            ])
            .ok_or(StreamRuntimeError::AllocationFailed)?;
            let granular_extra = granular
                .as_ref()
                .map_or(Some(0), |prepared| {
                    prepared.data().bytes()?.checked_add(
                        prepared.metadata().len().checked_mul(std::mem::size_of::<
                            hex_engine::StreamedSourceMetadata,
                        >())?,
                    )
                })
                .ok_or(StreamRuntimeError::AllocationFailed)?;
            let actual = common
                .checked_add(granular_extra)
                .and_then(|n| n.checked_add(worker_data))
                .ok_or(StreamRuntimeError::AllocationFailed)?;
            let peak = common
                .checked_add(granular_extra)
                .and_then(|n| n.checked_add(worker_peak))
                .and_then(|n| n.checked_add(transport_data.page_boxing_overlap))
                .ok_or(StreamRuntimeError::AllocationFailed)?;
            if actual > bound.retained_data_bytes || peak > bound.construction_peak_data_bytes {
                return Err(StreamRuntimeError::InvalidConfiguration);
            }
        }
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
                    scalar_definitions,
                    granular,
                    free_pages,
                    worker_queues,
                    thread_stop,
                    catalog_sender,
                    #[cfg(test)]
                    test_hooks,
                    #[cfg(feature = "test-support")]
                    page_gate,
                );
                #[cfg(test)]
                worker_finished_signal.store(true, Ordering::Release);
                pages
            })
            .map_err(|_| StreamRuntimeError::WorkerSpawnFailed)?;
        let worker_wake = worker.thread().clone();
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
        Ok((
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
            },
            disposal,
        ))
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

    /// Actual simultaneous cache capacity. The worker retains a turnover page;
    /// this scalar is only a bound, never source readiness or native authority.
    pub fn page_capacity(&self) -> usize {
        self.pages
            .as_ref()
            .map_or(0, |pages| pages.endpoint.cache_capacity())
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
                    .map_err(|_| ShutdownError::Runtime(StreamRuntimeError::WorkerJoinFailed))?
                    .map_err(ShutdownError::Runtime)?;
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
                .transport_data
                .pcm_boxes
                .saturating_add(self.owner.transport_data.scalar_boxes)
                .saturating_add(self.owner.transport_data.scalar_coverage_boxes),
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

    fn prefetch_page(&mut self, key: StreamSourceKey, page_index: u64) {
        self.endpoint.prefetch_page(key, page_index);
    }
    fn scalar_page(
        &mut self,
        key: hex_engine::ScalarSourceKey,
        page_index: u64,
    ) -> Option<hex_engine::BorrowedScalarPage<'_>> {
        self.endpoint.scalar_page(key, page_index)
    }
    fn request_scalar_page(&mut self, key: hex_engine::ScalarSourceKey, page_index: u64) {
        self.endpoint.request_scalar_page(key, page_index);
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

    fn prefetch_page(&mut self, key: StreamSourceKey, page_index: u64) {
        if let Some(pages) = self.pages.as_mut() {
            pages.prefetch_page(key, page_index);
        }
    }
    fn scalar_page(
        &mut self,
        key: hex_engine::ScalarSourceKey,
        page_index: u64,
    ) -> Option<hex_engine::BorrowedScalarPage<'_>> {
        self.pages.as_mut()?.scalar_page(key, page_index)
    }
    fn request_scalar_page(&mut self, key: hex_engine::ScalarSourceKey, page_index: u64) {
        if let Some(pages) = self.pages.as_mut() {
            pages.request_scalar_page(key, page_index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shutdown_queue_fixture(wait_for_publication: bool) -> PagePoolReport {
        use hex_engine::StreamedSourceMetadata;
        use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
        use hex_project::{Command, Created, Editor, Project};

        let wav = crate::worker::tests::Fixture::pcm16_frames(12_001);
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(2 * 1024 * 1024, 12_001, 128, 32_768),
            1024 * 1024,
        );
        let verified = VerifiedWavReader::open(&wav.0, limits).expect("verified source");
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "shutdown-source.wav".into(),
                sample_rate: 48_000,
                frames: 12_001,
            })
            .expect("asset")
        else {
            panic!("expected asset")
        };
        let key = StreamSourceKey::new(104, asset);
        let metadata = StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            12_001,
            12_001,
            hex_project::IDENTITY_CONVERSION,
            *verified.fingerprint(),
        )
        .expect("identity metadata");
        let definition = SourceDefinition::new(wav.0.clone(), metadata, limits);
        let mut config = StreamConfig::with_pool_pages(8);
        config.request_capacity = 8;
        config.ready_capacity = 1;
        config.max_ready_pages_per_pump = 1;
        config.return_capacity = 1;
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (publication_tx, publication_rx) = mpsc::sync_channel(2);
        let hooks = crate::worker::TestWorkerHooks {
            read_gate: None,
            scalar_steps: None,
            entered: entered_tx,
            release: release_rx,
            publications: publication_tx,
        };
        let mut runtime = StreamRuntime::new_inner(
            config,
            vec![definition],
            Some(hooks),
            #[cfg(feature = "test-support")]
            None,
        )
        .expect("runtime");
        runtime
            .wait_until_ready(Duration::from_secs(3))
            .expect("catalog ready");
        entered_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("worker paused after catalog preparation");
        for page_index in 0..3 {
            runtime.request_page(key, page_index);
        }
        runtime.wake_worker();
        if wait_for_publication {
            release_tx.send(()).expect("release delayed worker");
            assert_eq!(
                publication_rx
                    .recv_timeout(Duration::from_secs(3))
                    .expect("first page published"),
                0
            );
            assert_eq!(
                publication_rx
                    .recv_timeout(Duration::from_secs(3))
                    .expect("second page held behind full queue"),
                1
            );
        } else {
            // Negative control: the old fixed delay proves no worker progress.
            // The worker remains deterministically paused until shutdown.
            thread::sleep(Duration::from_millis(30));
            assert!(matches!(
                publication_rx.try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
        }
        runtime
            .shutdown()
            .expect("shutdown joins the delayed worker")
    }

    #[test]
    fn full_ready_queue_drains_held_real_worker_page_and_continues() {
        use hex_engine::StreamedSourceMetadata;
        use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
        use hex_project::{Command, Created, Editor, Project};
        use std::time::Instant;

        let wav = crate::worker::tests::Fixture::pcm16_frames(12_001);
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(2 * 1024 * 1024, 12_001, 128, 32_768),
            1024 * 1024,
        );
        let reader = VerifiedWavReader::open(&wav.0, limits).unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "ready-queue-source.wav".into(),
                sample_rate: 48_000,
                frames: 12_001,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let key = StreamSourceKey::new(106, asset);
        let metadata = StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            12_001,
            12_001,
            hex_project::IDENTITY_CONVERSION,
            *reader.fingerprint(),
        )
        .unwrap();
        drop(reader);
        let mut config = StreamConfig::with_pool_pages(4);
        config.ready_capacity = 1;
        config.max_ready_pages_per_pump = 1;
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (publication_tx, publication_rx) = mpsc::sync_channel(3);
        let hooks = crate::worker::TestWorkerHooks {
            read_gate: None,
            entered: entered_tx,
            release: release_rx,
            publications: publication_tx,
            scalar_steps: None,
        };
        let mut runtime = StreamRuntime::new_inner(
            config,
            vec![SourceDefinition::new(wav.0.clone(), metadata, limits)],
            Some(hooks),
            #[cfg(feature = "test-support")]
            None,
        )
        .unwrap();
        runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        #[cfg(feature = "allocation-probe")]
        let initial_requests = allocation_counter::measure(|| {
            runtime.request_page(key, 0);
            runtime.request_page(key, 1);
        });
        #[cfg(not(feature = "allocation-probe"))]
        {
            runtime.request_page(key, 0);
            runtime.request_page(key, 1);
        }
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                initial_requests.count_total,
                initial_requests.count_current,
                initial_requests.bytes_total,
                initial_requests.bytes_current
            ),
            (0, 0, 0, 0)
        );
        release_tx.send(()).unwrap();
        runtime.wake_worker();
        assert_eq!(
            publication_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            0
        );
        assert_eq!(
            publication_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            1
        );

        // Pump and request are callback-side operations. Construction, waking,
        // fixture verification and all destruction stay outside the probe.
        let pump = |runtime: &mut StreamRuntime| {
            let count = std::cell::Cell::new(0);
            #[cfg(feature = "allocation-probe")]
            let measured = allocation_counter::measure(|| count.set(runtime.pump_ready(1)));
            #[cfg(not(feature = "allocation-probe"))]
            {
                count.set(runtime.pump_ready(1));
            }
            #[cfg(feature = "allocation-probe")]
            assert_eq!(
                (
                    measured.count_total,
                    measured.count_current,
                    measured.bytes_total,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            count.get()
        };
        assert_eq!(pump(&mut runtime), 1);
        assert!(runtime.page(key, 0).is_some());
        runtime.wake_worker();
        let deadline = Instant::now() + Duration::from_secs(2);
        while pump(&mut runtime) == 0 {
            assert!(Instant::now() < deadline, "held ready page did not drain");
            std::thread::yield_now();
        }
        assert!(runtime.page(key, 1).is_some());
        #[cfg(feature = "allocation-probe")]
        let measured = allocation_counter::measure(|| runtime.request_page(key, 2));
        #[cfg(not(feature = "allocation-probe"))]
        runtime.request_page(key, 2);
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        runtime.wake_worker();
        assert_eq!(
            publication_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            0
        );
        assert_eq!(pump(&mut runtime), 1);
        assert_eq!(pump(&mut runtime), 0);
        for index in 0..3 {
            let page = runtime.page(key, index).expect("each page arrives once");
            let start = index as usize * hex_engine::STREAM_PAGE_FRAMES;
            assert_eq!(
                page.frames.len(),
                (12_001 - start).min(hex_engine::STREAM_PAGE_FRAMES)
            );
            for (offset, frame) in page.frames.iter().enumerate() {
                let sample = (start + offset) as f32 / 32768.0;
                assert_eq!(*frame, [sample, -sample]);
            }
        }
        let report = runtime.shutdown().unwrap();
        assert!(report.worker_joined && report.accounting_complete);
        assert_eq!(report.page_buffers, 4);
        assert_eq!(
            report.queued_pages_reclaimed
                + report.held_return_pages_reclaimed
                + report.worker_pages_reclaimed,
            4
        );
    }

    #[test]
    fn full_request_queue_retries_with_real_worker_and_preserves_pcm_and_custody() {
        use hex_engine::StreamedSourceMetadata;
        use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
        use hex_project::{Command, Created, Editor, Project};

        let wav = crate::worker::tests::Fixture::pcm16_frames(12_001);
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(2 * 1024 * 1024, 12_001, 128, 32_768),
            1024 * 1024,
        );
        let reader = VerifiedWavReader::open(&wav.0, limits).unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "queue-source.wav".into(),
                sample_rate: 48_000,
                frames: 12_001,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let key = StreamSourceKey::new(105, asset);
        let metadata = StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            12_001,
            12_001,
            hex_project::IDENTITY_CONVERSION,
            *reader.fingerprint(),
        )
        .unwrap();
        drop(reader);
        let mut config = StreamConfig::with_pool_pages(4);
        config.request_capacity = 2;
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (publication_tx, publication_rx) = mpsc::sync_channel(3);
        let hooks = crate::worker::TestWorkerHooks {
            read_gate: None,
            entered: entered_tx,
            release: release_rx,
            publications: publication_tx,
            scalar_steps: None,
        };
        let mut runtime = StreamRuntime::new_inner(
            config,
            vec![SourceDefinition::new(wav.0.clone(), metadata, limits)],
            Some(hooks),
            #[cfg(feature = "test-support")]
            None,
        )
        .unwrap();
        runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // The worker cannot consume the queue until released. A rejected page
        // must not acquire a pending entry or be mistaken for a coalesced one.
        #[cfg(feature = "allocation-probe")]
        let measured = allocation_counter::measure(|| {
            runtime.request_page(key, 0);
            runtime.request_page(key, 1);
            runtime.request_page(key, 2);
            runtime.request_page(key, 0);
        });
        #[cfg(not(feature = "allocation-probe"))]
        {
            runtime.request_page(key, 0);
            runtime.request_page(key, 1);
            runtime.request_page(key, 2);
            runtime.request_page(key, 0);
        }
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        let stats = runtime.stats();
        assert_eq!(
            (
                stats.queued_requests,
                stats.rejected_requests,
                stats.coalesced_requests
            ),
            (2, 1, 1)
        );
        release_tx.send(()).unwrap();
        runtime.wake_worker();
        for _ in 0..2 {
            assert_eq!(
                publication_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                0
            );
        }
        assert_eq!(runtime.pump_ready(2), 2);
        runtime.request_page(key, 2);
        runtime.wake_worker();
        assert_eq!(
            publication_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            0
        );
        assert_eq!(runtime.pump_ready(1), 1);
        let stats = runtime.stats();
        assert_eq!(
            (
                stats.queued_requests,
                stats.rejected_requests,
                stats.coalesced_requests
            ),
            (3, 1, 1)
        );
        for index in 0..3 {
            let page = runtime.page(key, index).expect("accepted or retried page");
            let start = index as usize * hex_engine::STREAM_PAGE_FRAMES;
            for (offset, frame) in page.frames.iter().enumerate() {
                let sample = (start + offset) as f32 / 32768.0;
                assert_eq!(*frame, [sample, -sample]);
            }
            assert_eq!(
                page.frames.len(),
                (12_001 - start).min(hex_engine::STREAM_PAGE_FRAMES)
            );
        }
        let report = runtime
            .shutdown()
            .expect("joined worker and recovered page pool");
        assert!(report.worker_joined && report.accounting_complete);
        assert_eq!(report.page_buffers, 4);
        assert_eq!(
            report.queued_pages_reclaimed
                + report.held_return_pages_reclaimed
                + report.worker_pages_reclaimed,
            4
        );
    }

    #[test]
    fn shutdown_reclaims_undrained_ready_queue_and_worker_held_pages() {
        let report = shutdown_queue_fixture(true);
        assert!(report.worker_joined);
        assert!(report.accounting_complete);
        assert_eq!(report.queued_pages_reclaimed, 1);
        assert_eq!(
            report.queued_pages_reclaimed
                + report.held_return_pages_reclaimed
                + report.worker_pages_reclaimed,
            8
        );
    }

    #[test]
    fn delayed_worker_exposes_the_fixed_delay_readiness_assumption() {
        let report = shutdown_queue_fixture(false);
        assert!(report.worker_joined);
        assert!(report.accounting_complete);
        assert_eq!(report.queued_pages_reclaimed, 0);
        assert_eq!(report.worker_pages_reclaimed, 8);
    }

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

#[cfg(test)]
mod scalar_tests;
