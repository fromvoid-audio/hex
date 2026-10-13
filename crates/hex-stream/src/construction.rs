//! Opt-in, control-owned verified catalog construction. Not page readiness.
use crate::construction_report::{WorkerFaults, WorkerObserver};
use crate::construction_transport::{PreparedTransport, SetupFaults, TransportSetupObservation};
use crate::endpoint::{Endpoint, PageBuffer};
use crate::types::VerifiedCatalog;
use crate::worker::{WorkerQueues, run_worker_observed};
use crate::{
    CompactCopyReport, CompactSourceInput, PagePoolReport, SourceDefinition, StreamConfig,
    StreamFailureKind, StreamRuntime, StreamRuntimeError, WorkerConstructionObservation,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

type CatalogResult = (
    Result<VerifiedCatalog, StreamFailureKind>,
    WorkerConstructionObservation,
);
struct StartupPayload {
    definitions: Vec<SourceDefinition>,
    pages: Vec<Box<PageBuffer>>,
    queues: WorkerQueues,
}
type StartupCell = Arc<Mutex<Option<StartupPayload>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructionState {
    Unstarted,
    Pending,
    CatalogVerified,
    CancellingCleaning,
    Failed,
    Consumed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructionRefusal {
    InvalidState,
    MissingCompactCopy,
    Runtime(StreamRuntimeError),
    Worker(StreamFailureKind),
    WorkerUnavailable,
    Cancelled,
    CapacityExceeded,
    ArithmeticOverflow,
}
impl std::fmt::Display for ConstructionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stream construction refused: {self:?}")
    }
}
impl std::error::Error for ConstructionRefusal {}

/// Explicit lifecycle outcomes. Panic loses the worker-returned inventory;
/// known queue counts must not be represented as complete pool conservation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructionCleanup {
    NotStarted {
        allocated_pages: usize,
        reclaimed_pages: usize,
    },
    Joined(PagePoolReport),
    PoolAccounting(PagePoolReport),
    JoinFailed {
        page_buffers: usize,
        queued_pages_reclaimed: usize,
        held_return_pages_reclaimed: usize,
    },
}

/// Reserved data capacities and conservative construction overlaps only.
/// Opaque allocator/Arc/mpsc/thread/OS costs, stack and caller-owned handles
/// are excluded. Component maxima are not a sampled simultaneous memory peak.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConstructionCapacityReport {
    pub input: Option<CompactCopyReport>,
    pub setup: TransportSetupObservation,
    pub worker: WorkerConstructionObservation,
    pub startup_shared_object_bytes: usize,
    pub max_construction_data_bytes: usize,
}
impl ConstructionCapacityReport {
    /// Available only after an API-reported worker outcome. Panic snapshots
    /// remain partial and must not authorize a complete construction claim.
    pub fn finalized_data_bound_bytes(self) -> Option<usize> {
        if !self.worker.finalized {
            return None;
        }
        self.data_bound_bytes()
    }
    pub fn data_bound_bytes(self) -> Option<usize> {
        let input = self.input?;
        // Worker paths replace the compact report's path charge, not an extra
        // copy. Incoming definition slots are already construction-only there.
        let setup_free = self.setup.free_page_handles.bytes()?;
        let worker_free = self.worker.catalog.free_page_handles.bytes()?;
        let free_extra = setup_free.max(worker_free).checked_sub(worker_free)?;
        input
            .originals
            .bytes()?
            .checked_add(input.reservations.other_retained_bytes)?
            .checked_add(input.reservations.other_construction_bytes)?
            .checked_add(self.setup.endpoint.bytes()?)?
            .checked_add(self.setup.transport.bytes()?)?
            .checked_add(self.setup.transport.page_boxing_peak_extra.bytes()?)?
            .checked_add(self.worker.data_bound_bytes()?)?
            .checked_add(free_extra)?
            .checked_add(self.startup_shared_object_bytes)
    }
    fn check(self) -> Result<(), ConstructionRefusal> {
        let bytes = self
            .data_bound_bytes()
            .ok_or(ConstructionRefusal::ArithmeticOverflow)?;
        if bytes > self.max_construction_data_bytes {
            Err(ConstructionRefusal::CapacityExceeded)
        } else {
            Ok(())
        }
    }
}

pub struct FinishedStreamInput {
    originals: Vec<SourceDefinition>,
    runtime: StreamRuntime,
    capacity: ConstructionCapacityReport,
}
impl FinishedStreamInput {
    /// Control transfer only. Catalog verification is not protected-page
    /// installation/prewarm or permission for native streamed publication.
    pub fn into_parts(
        self,
    ) -> (
        Vec<SourceDefinition>,
        StreamRuntime,
        ConstructionCapacityReport,
    ) {
        (self.originals, self.runtime, self.capacity)
    }
}
pub struct RecoveredStreamInput {
    pub originals: Vec<SourceDefinition>,
    pub failure: ConstructionRefusal,
    pub cleanup: ConstructionCleanup,
    pub capacity: ConstructionCapacityReport,
}

/// Non-cloneable control request. No runtime/pages/catalog access before a
/// successful verified and capacity-checked finish. Originals are held here
/// unchanged through Pending, failure and cancellation. All methods and final
/// destruction belong on control; Drop may block joining filesystem work and
/// is best effort, not a successful cleanup receipt. Fatal OOM/abort excluded.
pub struct StreamConstructionRequest {
    originals: Option<Vec<SourceDefinition>>,
    compact: Option<Vec<SourceDefinition>>,
    config: StreamConfig,
    state: ConstructionState,
    failure: Option<ConstructionRefusal>,
    cleanup: Option<ConstructionCleanup>,
    capacity: ConstructionCapacityReport,
    endpoint: Option<Endpoint>,
    payload: Option<StartupCell>,
    telemetry: Option<Arc<Mutex<WorkerConstructionObservation>>>,
    stop: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<Vec<Box<PageBuffer>>>>,
    result: Option<Receiver<CatalogResult>>,
    verified: Option<VerifiedCatalog>,
    setup_faults: SetupFaults,
    worker_faults: WorkerFaults,
}
impl StreamConstructionRequest {
    /// Only moves existing owners. `max_construction_data_bytes` is an explicit
    /// caller reservation for the reported data scope, not a global budget.
    pub fn new(
        input: CompactSourceInput,
        config: StreamConfig,
        max_construction_data_bytes: usize,
    ) -> Self {
        let (originals, compact, report) = input.into_stage_parts();
        let mut worker = WorkerConstructionObservation::default();
        if let Some(report) = report {
            worker.construction.definition_slots = report.compact.source_slots;
            worker.catalog.definition_path_bytes = report.compact.path_capacity_bytes;
        }
        Self {
            originals: Some(originals),
            compact,
            config,
            state: ConstructionState::Unstarted,
            failure: None,
            cleanup: None,
            capacity: ConstructionCapacityReport {
                input: report,
                worker,
                max_construction_data_bytes,
                ..ConstructionCapacityReport::default()
            },
            endpoint: None,
            payload: None,
            telemetry: None,
            stop: None,
            worker: None,
            result: None,
            verified: None,
            setup_faults: SetupFaults::default(),
            worker_faults: WorkerFaults::default(),
        }
    }
    pub fn state(&self) -> ConstructionState {
        self.state
    }
    pub fn originals(&self) -> Option<&[SourceDefinition]> {
        self.originals.as_deref()
    }
    pub fn failure(&self) -> Option<ConstructionRefusal> {
        self.failure
    }
    pub fn cleanup(&self) -> Option<ConstructionCleanup> {
        self.cleanup
    }
    pub fn capacity_report(&self) -> ConstructionCapacityReport {
        let mut report = self.capacity;
        if let Some(telemetry) = &self.telemetry {
            report.worker = *telemetry.lock().unwrap_or_else(|p| p.into_inner());
        }
        report
    }
    fn fail(&mut self, reason: ConstructionRefusal) {
        self.failure.get_or_insert(reason);
        self.verified = None;
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Release);
        }
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
        self.state = ConstructionState::CancellingCleaning;
    }
    pub fn start(&mut self) -> Result<(), ConstructionRefusal> {
        if self.state != ConstructionState::Unstarted {
            return Err(ConstructionRefusal::InvalidState);
        }
        let result = self.start_inner();
        if let Err(reason) = result {
            self.fail(reason);
            self.clean(false);
        }
        result
    }
    fn start_inner(&mut self) -> Result<(), ConstructionRefusal> {
        self.config
            .validate()
            .map_err(ConstructionRefusal::Runtime)?;
        let definitions = self
            .compact
            .as_ref()
            .ok_or(ConstructionRefusal::MissingCompactCopy)?;
        if definitions.len() > self.config.max_sources {
            return Err(ConstructionRefusal::Runtime(
                StreamRuntimeError::InvalidConfiguration,
            ));
        }
        self.capacity.check()?;
        let prepared = PreparedTransport::build(
            self.config,
            definitions,
            &mut self.capacity.setup,
            &self.setup_faults,
        )
        .map_err(ConstructionRefusal::Runtime)?;
        self.endpoint = Some(prepared.endpoint);
        self.capacity.worker.catalog.free_page_handles = self.capacity.setup.free_page_handles;
        let payload = Arc::new(Mutex::new(Some(StartupPayload {
            definitions: self
                .compact
                .take()
                .ok_or(ConstructionRefusal::MissingCompactCopy)?,
            pages: prepared.free_pages,
            queues: prepared.worker_queues,
        })));
        self.payload = Some(Arc::clone(&payload));
        let telemetry = Arc::new(Mutex::new(self.capacity.worker));
        self.telemetry = Some(Arc::clone(&telemetry));
        self.capacity.startup_shared_object_bytes =
            std::mem::size_of::<Mutex<Option<StartupPayload>>>()
                .checked_add(std::mem::size_of::<Mutex<WorkerConstructionObservation>>())
                .ok_or(ConstructionRefusal::ArithmeticOverflow)?;
        self.capacity.check()?;
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(Arc::clone(&stop));
        let (sender, receiver) = mpsc::sync_channel(1);
        self.result = Some(receiver);
        #[cfg(test)]
        if self.setup_faults.fail_spawn {
            return Err(ConstructionRefusal::Runtime(
                StreamRuntimeError::WorkerSpawnFailed,
            ));
        }
        let config = self.config;
        let faults = self.worker_faults.clone();
        let observer = WorkerObserver {
            shared: Some(telemetry),
            data: self.capacity.worker,
            faults,
        };
        let worker = thread::Builder::new()
            .name("hex-stream-construction".into())
            .spawn(move || {
                let start = payload
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                    .expect("once-only startup payload");
                drop(payload);
                run_worker_observed(
                    config,
                    start.definitions,
                    start.pages,
                    start.queues,
                    stop,
                    sender,
                    observer,
                )
            })
            .map_err(|_| ConstructionRefusal::Runtime(StreamRuntimeError::WorkerSpawnFailed))?;
        self.worker = Some(worker);
        self.state = ConstructionState::Pending;
        Ok(())
    }
    pub fn poll(&mut self) -> ConstructionState {
        self.poll_timeout(Duration::ZERO)
    }
    /// Timeout leaves Pending. Cooperative cancellation cannot interrupt an
    /// arbitrary blocking file read, so this is not a cleanup deadline promise.
    pub fn poll_timeout(&mut self, timeout: Duration) -> ConstructionState {
        if self.state == ConstructionState::Pending {
            let received = self
                .result
                .as_ref()
                .map(|result| result.recv_timeout(timeout));
            match received {
                Some(Ok((catalog, observation))) => {
                    self.capacity.worker = observation;
                    match catalog {
                        Ok(catalog) => {
                            self.verified = Some(catalog);
                            self.state = ConstructionState::CatalogVerified;
                        }
                        Err(reason) => self.fail(ConstructionRefusal::Worker(reason)),
                    }
                }
                Some(Err(RecvTimeoutError::Timeout)) => {}
                _ => self.fail(ConstructionRefusal::WorkerUnavailable),
            }
        }
        if self.state == ConstructionState::CancellingCleaning {
            self.clean(false);
        }
        self.state
    }
    /// Cancellation wins even if a successful result is already queued. No
    /// further result can authorize finish after this control transition.
    pub fn cancel(&mut self) -> Result<(), ConstructionRefusal> {
        if self.state == ConstructionState::Consumed {
            return Err(ConstructionRefusal::InvalidState);
        }
        if self.state != ConstructionState::Failed {
            self.fail(ConstructionRefusal::Cancelled);
            self.clean(false);
        }
        Ok(())
    }
    pub fn try_finish(&mut self) -> Result<Option<FinishedStreamInput>, ConstructionRefusal> {
        self.poll();
        if self.state == ConstructionState::Pending {
            return Ok(None);
        }
        if self.state != ConstructionState::CatalogVerified {
            return Err(self.failure.unwrap_or(ConstructionRefusal::InvalidState));
        }
        if self.worker.as_ref().is_none_or(JoinHandle::is_finished) {
            self.fail(ConstructionRefusal::WorkerUnavailable);
            return Err(ConstructionRefusal::WorkerUnavailable);
        }
        let capacity = self.capacity_report();
        if !capacity.worker.finalized {
            self.fail(ConstructionRefusal::WorkerUnavailable);
            return Err(ConstructionRefusal::WorkerUnavailable);
        }
        if let Err(reason) = capacity.check() {
            self.fail(reason);
            return Err(reason);
        }
        let (Some(originals), Some(endpoint), Some(worker), Some(stop), Some(catalog)) = (
            self.originals.take(),
            self.endpoint.take(),
            self.worker.take(),
            self.stop.take(),
            self.verified.take(),
        ) else {
            unreachable!("verified construction retains every private owner");
        };
        let runtime = StreamRuntime::from_construction(
            self.config,
            capacity.setup.transport,
            endpoint,
            stop,
            worker,
            catalog,
        );
        self.state = ConstructionState::Consumed;
        self.result = None;
        self.payload = None;
        self.telemetry = None;
        Ok(Some(FinishedStreamInput {
            originals,
            runtime,
            capacity,
        }))
    }
    /// An active worker keeps request/original owners here. Terminal recovery
    /// can return originals with explicit incomplete cleanup after panic.
    pub fn try_recover(&mut self) -> Result<Option<RecoveredStreamInput>, ConstructionRefusal> {
        if self.state == ConstructionState::Consumed {
            return Err(ConstructionRefusal::InvalidState);
        }
        if self.failure.is_none() {
            self.cancel()?;
        }
        self.clean(false);
        if self.state != ConstructionState::Failed {
            return Ok(None);
        }
        let originals = self
            .originals
            .take()
            .ok_or(ConstructionRefusal::InvalidState)?;
        let recovered = RecoveredStreamInput {
            originals,
            failure: self.failure.unwrap_or(ConstructionRefusal::Cancelled),
            cleanup: self.cleanup.expect("terminal cleanup outcome"),
            capacity: self.capacity_report(),
        };
        self.compact = None;
        self.payload = None;
        self.result = None;
        self.telemetry = None;
        self.stop = None;
        self.state = ConstructionState::Consumed;
        Ok(Some(recovered))
    }
    fn clean(&mut self, blocking: bool) {
        if self.cleanup.is_some() || self.state == ConstructionState::Consumed {
            return;
        }
        if let Some(worker) = &self.worker {
            if !blocking && !worker.is_finished() {
                return;
            }
        }
        let cleanup = if let Some(worker) = self.worker.take() {
            let joined = worker.join();
            let queued = self
                .endpoint
                .as_mut()
                .map_or(0, Endpoint::drain_ready_after_join);
            let held = self
                .endpoint
                .as_mut()
                .map_or(0, Endpoint::drop_held_returns_after_join);
            match joined {
                Ok(pages) => {
                    let worker_pages_reclaimed = pages.len();
                    drop(pages);
                    let accounted = queued
                        .checked_add(held)
                        .and_then(|n| n.checked_add(worker_pages_reclaimed));
                    let report = PagePoolReport {
                        page_buffers: self.config.page_buffers,
                        page_bytes: self.config.page_buffers
                            * hex_engine::STREAM_PAGE_FRAMES
                            * std::mem::size_of::<hex_engine::Frame>(),
                        cached_pages_returned: 0,
                        queued_pages_reclaimed: queued,
                        held_return_pages_reclaimed: held,
                        worker_pages_reclaimed,
                        worker_joined: true,
                        accounting_complete: accounted == Some(self.config.page_buffers),
                    };
                    if report.accounting_complete {
                        ConstructionCleanup::Joined(report)
                    } else {
                        ConstructionCleanup::PoolAccounting(report)
                    }
                }
                Err(_) => {
                    self.capacity.worker.finalized = false;
                    ConstructionCleanup::JoinFailed {
                        page_buffers: self.config.page_buffers,
                        queued_pages_reclaimed: queued,
                        held_return_pages_reclaimed: held,
                    }
                }
            }
        } else {
            if let Some(payload) = &self.payload {
                drop(payload.lock().unwrap_or_else(|p| p.into_inner()).take());
            }
            ConstructionCleanup::NotStarted {
                allocated_pages: self.capacity.setup.allocated_pages,
                reclaimed_pages: self.capacity.setup.allocated_pages,
            }
        };
        self.capacity = self.capacity_report();
        if matches!(cleanup, ConstructionCleanup::JoinFailed { .. }) {
            self.capacity.worker.finalized = false;
            self.capacity.worker.kernel_transient_unobserved = true;
            self.telemetry = None;
        }
        self.endpoint = None;
        self.verified = None;
        self.cleanup = Some(cleanup);
        self.state = ConstructionState::Failed;
    }
}
impl Drop for StreamConstructionRequest {
    fn drop(&mut self) {
        if self.state != ConstructionState::Consumed {
            self.fail(ConstructionRefusal::Cancelled);
            self.clean(true);
        }
    }
}

#[cfg(test)]
#[path = "construction_tests.rs"]
mod tests;
