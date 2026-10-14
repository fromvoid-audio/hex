//! Headless domain return paths; no CPAL stop/disposal or graph providers.
use crate::{
    AllocationCapacity, CatalogLease, DomainCleanupProgress, DomainEscrowFootprint, DomainId,
    DomainPoll, DomainRefusal, DomainReturnStatus, DomainSetupFailure, MediaCleanupPhase,
    MediaDomainInput, PageReadiness, ReportedShutdown, ShutdownError, StreamControl,
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

#[path = "domain_admission.rs"]
mod admission;
pub use admission::{DomainAdmission, DomainAdmissionFailure, reusable_domain_escrow};

struct BatchSlot {
    vacant: bool,
    id: DomainId,
    endpoint: Option<Box<PageReadiness>>,
    admission_seal: Option<Arc<()>>,
}
#[path = "catalog_custody.rs"]
mod custody;
pub use custody::{
    CatalogCoverageAck, CatalogCoverageBinding, CatalogCoverageCovered, CatalogCoveragePending,
    CatalogCoverageProgress, CatalogCoverageRefusal, CatalogCoverageReservation,
    CatalogCoverageStorageReport, CatalogCustodyAwaiter, CatalogCustodyCapacity,
    CatalogCustodyError, CatalogCustodyReceipt, CatalogCustodyRefusal, CatalogCustodyReservation,
    CurrentCatalogCoverage, VerifiedCatalogCustodyRefusal, VerifiedCoverageRefusal,
};

struct ControlSlot {
    incarnation: u64,
    custody: custody::CatalogCustody,
    vacant: bool,
    data_bytes: usize,
    id: DomainId,
    control: Option<StreamControl>,
    retained_leases: Vec<CatalogLease>,
    endpoint: Option<Box<PageReadiness>>,
    report: Option<ReportedShutdown>,
    // Defensive ownership retention after a post-commit invariant failure.
    terminal_error: Option<ShutdownError>,
}

/// Private transport payload; recovery is control-only. No Clone.
pub struct ReturnedMediaDomain {
    admission_seal: Option<Arc<()>>,
    index: usize,
    id: DomainId,
    endpoint: Box<PageReadiness>,
}
impl ReturnedMediaDomain {
    pub fn into_parts(self) -> (DomainId, PageReadiness) {
        (self.id, *self.endpoint)
    }
}

/// Control allocates this Box/Vec once. Rendering/return methods borrow or move
/// actual endpoint owners; final batch disposal belongs on control. No Clone.
pub struct CallbackDomainBatch {
    reusable: bool,
    slots: Vec<BatchSlot>,
    returns: Producer<ReturnedMediaDomain>,
    seal: Arc<()>,
}
impl CallbackDomainBatch {
    /// Borrowed identity lookup in fixed slot storage, not graph ownership.
    pub fn domain_slot(&self, id: DomainId) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| !slot.vacant && slot.id == id)
    }
    pub fn readiness_mut(&mut self, id: DomainId) -> Option<&mut PageReadiness> {
        self.slots
            .iter_mut()
            .find(|slot| !slot.vacant && slot.id == id)?
            .endpoint
            .as_deref_mut()
    }
    pub fn try_return_domain(&mut self, id: DomainId) -> DomainReturnStatus {
        if self.returns.is_abandoned() {
            return DomainReturnStatus::Refused(DomainRefusal::QueueClosed);
        }
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.id == id && (!slot.vacant || !self.reusable))
        else {
            return DomainReturnStatus::Refused(DomainRefusal::UnknownDomain);
        };
        let Some(endpoint) = self.slots[index].endpoint.take() else {
            return DomainReturnStatus::Refused(DomainRefusal::AlreadyReturned);
        };
        match self.returns.push(ReturnedMediaDomain {
            admission_seal: self.slots[index].admission_seal.take(),
            index,
            id,
            endpoint,
        }) {
            Ok(()) => {
                self.slots[index].vacant = true;
                DomainReturnStatus::Accepted
            }
            Err(PushError::Full(returned)) => {
                self.slots[index].endpoint = Some(returned.endpoint);
                self.slots[index].admission_seal = returned.admission_seal;
                DomainReturnStatus::Pending
            }
        }
    }
}

/// Separate once-only terminal producer. Kept on lifecycle/control, never in
/// the batch sent into its own ring (avoids a producer/payload ownership cycle).
pub struct TerminalBatchSender {
    sender: Producer<Box<CallbackDomainBatch>>,
    seal: Arc<()>,
    sent: bool,
}
pub struct TerminalBatchFailure {
    pub reason: DomainRefusal,
    batch: Box<CallbackDomainBatch>,
}
impl std::fmt::Debug for TerminalBatchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalBatchFailure")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
impl TerminalBatchFailure {
    pub fn into_batch(self) -> Box<CallbackDomainBatch> {
        self.batch
    }
}
impl TerminalBatchSender {
    /// Explicit headless transfer after all borrows/callback use of this batch
    /// have ended. This cannot establish a native device-stop guarantee.
    pub fn try_return_stopped_batch(
        &mut self,
        batch: Box<CallbackDomainBatch>,
    ) -> Result<(), TerminalBatchFailure> {
        let reason = if self.sent {
            Some(DomainRefusal::TerminalAlreadyReturned)
        } else if !Arc::ptr_eq(&self.seal, &batch.seal) {
            Some(DomainRefusal::WrongEscrow)
        } else if self.sender.is_abandoned() {
            Some(DomainRefusal::QueueClosed)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(TerminalBatchFailure { reason, batch });
        }
        match self.sender.push(batch) {
            Ok(()) => {
                self.sent = true;
                Ok(())
            }
            Err(PushError::Full(batch)) => Err(TerminalBatchFailure {
                reason: DomainRefusal::InvalidCapacity,
                batch,
            }),
        }
    }
}

pub enum RejectedMediaReturn {
    Domain(Box<ReturnedMediaDomain>),
    Batch(Box<CallbackDomainBatch>),
}
pub struct MediaReturnFailure {
    pub reason: DomainRefusal,
    returned: RejectedMediaReturn,
}
impl std::fmt::Debug for MediaReturnFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaReturnFailure")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
impl MediaReturnFailure {
    pub fn into_returned(self) -> RejectedMediaReturn {
        self.returned
    }
}

/// Observation-only interest; dropping it cannot abandon/cancel root cleanup.
/// Clones are control-side only, never created by callback return methods.
pub struct MediaCleanupObserver {
    phase: Arc<AtomicU8>,
}
impl MediaCleanupObserver {
    pub fn phase(&self) -> MediaCleanupPhase {
        phase_from(self.phase.load(Ordering::Acquire))
    }
}
fn phase_from(value: u8) -> MediaCleanupPhase {
    match value {
        1 => MediaCleanupPhase::Complete,
        2 => MediaCleanupPhase::FailedIncomplete,
        _ => MediaCleanupPhase::Pending,
    }
}

/// Durable in-process control root. Keep it independently of any UI observer
/// until terminal return and actual cleanup; Drop/reclamation is control-only.
/// No hidden worker, automatic stop, timeout completion or native reopen API.
pub struct MediaDomainReaper {
    slots: Vec<ControlSlot>,
    returns: Consumer<ReturnedMediaDomain>,
    terminal: Consumer<Box<CallbackDomainBatch>>,
    terminal_batch: Option<Box<CallbackDomainBatch>>,
    seal: Arc<()>,
    phase: Arc<AtomicU8>,
    footprint: DomainEscrowFootprint,
    reusable: bool,
}
impl MediaDomainReaper {
    pub fn observer(&self) -> MediaCleanupObserver {
        MediaCleanupObserver {
            phase: Arc::clone(&self.phase),
        }
    }
    pub fn phase(&self) -> MediaCleanupPhase {
        phase_from(self.phase.load(Ordering::Acquire))
    }
    pub fn allocation_footprint(&self) -> DomainEscrowFootprint {
        self.footprint
    }
    pub fn retained_lease_count(&self, id: DomainId) -> Option<usize> {
        self.slots
            .iter()
            .find(|slot| !slot.vacant && slot.id == id)
            .map(|slot| slot.retained_leases.len())
    }
    pub fn returned_readiness(&mut self, id: DomainId) -> Option<&mut PageReadiness> {
        self.slots
            .iter_mut()
            .find(|slot| !slot.vacant && slot.id == id)?
            .endpoint
            .as_deref_mut()
    }
    pub fn report(&self, id: DomainId) -> Option<ReportedShutdown> {
        self.slots
            .iter()
            .find(|slot| !slot.vacant && slot.id == id)?
            .report
    }
    fn validate_return(
        &self,
        index: usize,
        id: DomainId,
        endpoint: &PageReadiness,
    ) -> Result<(), DomainRefusal> {
        let slot = self.slots.get(index).ok_or(DomainRefusal::UnknownDomain)?;
        if slot.vacant || slot.id != id {
            return Err(DomainRefusal::UnknownDomain);
        }
        if slot.endpoint.is_some() || slot.report.is_some() || slot.terminal_error.is_some() {
            return Err(DomainRefusal::AlreadyReturned);
        }
        if !slot
            .control
            .as_ref()
            .is_some_and(|control| control.matches_catalog(endpoint.catalog()))
        {
            return Err(DomainRefusal::WrongPair);
        }
        Ok(())
    }
    /// At most one payload per call. Invalid/duplicate payloads return owning
    /// failure storage on control, never overwrite an existing domain.
    pub fn poll_one(&mut self) -> Result<DomainPoll, MediaReturnFailure> {
        if let Ok(returned) = self.returns.pop() {
            if let Err(reason) =
                self.validate_return(returned.index, returned.id, &returned.endpoint)
            {
                return Err(MediaReturnFailure {
                    reason,
                    returned: RejectedMediaReturn::Domain(Box::new(returned)),
                });
            }
            let id = returned.id;
            self.slots[returned.index].endpoint = Some(returned.endpoint);
            self.update_phase();
            return Ok(DomainPoll::DomainReturned(id));
        }
        if let Ok(mut batch) = self.terminal.pop() {
            let reason = if self.terminal_batch.is_some() {
                Some(DomainRefusal::TerminalAlreadyReturned)
            } else if !Arc::ptr_eq(&self.seal, &batch.seal) || batch.slots.len() != self.slots.len()
            {
                Some(DomainRefusal::WrongEscrow)
            } else {
                batch.slots.iter().enumerate().find_map(|(index, slot)| {
                    if slot.endpoint.is_some()
                        && (self.slots[index].vacant || slot.id != self.slots[index].id)
                    {
                        return Some(DomainRefusal::UnknownDomain);
                    }
                    slot.endpoint
                        .as_ref()
                        .and_then(|endpoint| self.validate_return(index, slot.id, endpoint).err())
                })
            };
            if let Some(reason) = reason {
                return Err(MediaReturnFailure {
                    reason,
                    returned: RejectedMediaReturn::Batch(batch),
                });
            }
            for (index, slot) in batch.slots.iter_mut().enumerate() {
                if let Some(endpoint) = slot.endpoint.take() {
                    self.slots[index].endpoint = Some(endpoint);
                }
            }
            self.terminal_batch = Some(batch);
            self.update_phase();
            return Ok(DomainPoll::TerminalReturned);
        }
        self.update_phase();
        Ok(DomainPoll::Empty)
    }
    /// Whole-domain preflight before any lease release, unpin or metadata clear.
    /// Counts only actual references physically owned in this domain. External
    /// graph/manifest references cause Busy; no inferred graph retirement.
    pub fn try_reap_domain(&mut self, id: DomainId) -> DomainCleanupProgress {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| !slot.vacant && slot.id == id)
        else {
            return DomainCleanupProgress::Refused(DomainRefusal::UnknownDomain);
        };
        if let Some(report) = slot.report {
            return DomainCleanupProgress::Finished(report);
        }
        if slot.terminal_error.is_some() {
            return DomainCleanupProgress::Refused(DomainRefusal::UnexpectedShutdownRefusal);
        }
        if slot.custody.failed() {
            return DomainCleanupProgress::Refused(DomainRefusal::UnexpectedShutdownRefusal);
        }
        if slot.custody.reserved() {
            return DomainCleanupProgress::WaitingForReturn;
        }
        let (Some(control), Some(endpoint)) = (slot.control.as_ref(), slot.endpoint.as_ref())
        else {
            return DomainCleanupProgress::WaitingForReturn;
        };
        if !control.matches_catalog(endpoint.catalog()) {
            return DomainCleanupProgress::Refused(DomainRefusal::WrongPair);
        }
        if slot
            .retained_leases
            .iter()
            .any(|lease| !endpoint.catalog().same_catalog(lease))
        {
            return DomainCleanupProgress::Refused(DomainRefusal::WrongLease);
        }
        if !slot.custody.matches_catalog(endpoint.catalog()) {
            return DomainCleanupProgress::Refused(DomainRefusal::WrongLease);
        }
        let Some(known) = slot
            .retained_leases
            .len()
            .checked_add(slot.custody.lease_count())
            .and_then(|n| n.checked_add(3))
        else {
            return DomainCleanupProgress::Refused(DomainRefusal::InvalidCapacity);
        };
        let references = control.owner_references();
        if references > known {
            return DomainCleanupProgress::Busy {
                external_references: references - known,
            };
        }
        if references != known {
            return DomainCleanupProgress::Refused(DomainRefusal::WrongLease);
        }
        // No public Weak or clonable control/pages exist. At exactly known
        // references, no external lease can concurrently manufacture a clone.
        slot.retained_leases.clear();
        slot.custody.release_leases();
        let (Some(control), Some(endpoint)) = (slot.control.take(), slot.endpoint.take()) else {
            return DomainCleanupProgress::Refused(DomainRefusal::UnexpectedShutdownRefusal);
        };
        let (pages, lease) = endpoint.into_endpoint().into_parts();
        drop(lease);
        match control.shutdown_reported(pages) {
            Ok(report) => {
                slot.report = Some(report);
                self.update_phase();
                DomainCleanupProgress::Finished(report)
            }
            Err(error) => {
                slot.terminal_error = Some(error);
                self.phase.store(2, Ordering::Release);
                DomainCleanupProgress::Refused(DomainRefusal::UnexpectedShutdownRefusal)
            }
        }
    }
    fn update_phase(&self) {
        let failed = self.slots.iter().any(|slot| {
            slot.terminal_error.is_some()
                || slot.custody.failed()
                || slot
                    .report
                    .is_some_and(|report| report.successful_report().is_none())
        });
        let complete = self.terminal_batch.is_some()
            && self.returns.is_empty()
            && self.slots.iter().all(|slot| {
                slot.vacant
                    || (!slot.custody.outstanding()
                        && slot
                            .report
                            .is_some_and(|report| report.successful_report().is_some()))
            });
        self.phase.store(
            if failed {
                2
            } else if complete {
                1
            } else {
                0
            },
            Ordering::Release,
        );
    }
}

pub fn domain_escrow(
    inputs: Vec<MediaDomainInput>,
    return_capacity: usize,
) -> Result<
    (
        Box<CallbackDomainBatch>,
        TerminalBatchSender,
        MediaDomainReaper,
    ),
    Box<DomainSetupFailure>,
> {
    setup(inputs, return_capacity, || Ok(()))
}

fn setup(
    inputs: Vec<MediaDomainInput>,
    return_capacity: usize,
    mut before_reserve: impl FnMut() -> Result<(), ()>,
) -> Result<
    (
        Box<CallbackDomainBatch>,
        TerminalBatchSender,
        MediaDomainReaper,
    ),
    Box<DomainSetupFailure>,
> {
    let slots = inputs.len();
    build(inputs, return_capacity, slots, false, &mut before_reserve)
}

fn build(
    inputs: Vec<MediaDomainInput>,
    return_capacity: usize,
    slot_capacity: usize,
    reusable: bool,
    before_reserve: &mut impl FnMut() -> Result<(), ()>,
) -> Result<
    (
        Box<CallbackDomainBatch>,
        TerminalBatchSender,
        MediaDomainReaper,
    ),
    Box<DomainSetupFailure>,
> {
    let mut footprint = DomainEscrowFootprint {
        construction_input_slots: AllocationCapacity::of::<MediaDomainInput>(inputs.capacity()),
        ..Default::default()
    };
    let validation = if reusable {
        if slot_capacity == 0
            || return_capacity != slot_capacity
            || slot_capacity
                .checked_mul(std::mem::size_of::<ControlSlot>())
                .is_none()
            || slot_capacity
                .checked_mul(std::mem::size_of::<BatchSlot>())
                .is_none()
            || slot_capacity
                .checked_mul(std::mem::size_of::<ReturnedMediaDomain>())
                .is_none()
        {
            Err(DomainRefusal::InvalidCapacity)
        } else {
            validate_inputs(&inputs, 1)
        }
    } else {
        validate_inputs(&inputs, return_capacity)
    };
    if let Err(reason) = validation {
        return Err(Box::new(DomainSetupFailure {
            inputs,
            reason,
            observed: footprint,
        }));
    }
    let domain_data = inputs.iter().try_fold(0_usize, |sum, input| {
        let leases =
            AllocationCapacity::of::<CatalogLease>(input.retained_leases.capacity()).bytes()?;
        sum.checked_add(input.endpoint.retained_data_bytes()?)?
            .checked_add(leases)?
            .checked_add(std::mem::size_of::<PageReadiness>())
    });
    let Some(domain_data_bytes) = domain_data else {
        return Err(Box::new(DomainSetupFailure {
            inputs,
            reason: DomainRefusal::InvalidCapacity,
            observed: footprint,
        }));
    };
    footprint.domain_data_bytes = Some(domain_data_bytes);
    let mut controls = Vec::new();
    let mut callbacks = Vec::new();
    let reserved =
        before_reserve().and_then(|()| controls.try_reserve_exact(slot_capacity).map_err(|_| ()));
    footprint.control_slots = AllocationCapacity::of::<ControlSlot>(controls.capacity());
    if reserved.is_err() {
        return Err(Box::new(DomainSetupFailure {
            inputs,
            reason: DomainRefusal::Allocation,
            observed: footprint,
        }));
    }
    let reserved =
        before_reserve().and_then(|()| callbacks.try_reserve_exact(slot_capacity).map_err(|_| ()));
    footprint.callback_slots = AllocationCapacity::of::<BatchSlot>(callbacks.capacity());
    if reserved.is_err() {
        return Err(Box::new(DomainSetupFailure {
            inputs,
            reason: DomainRefusal::Allocation,
            observed: footprint,
        }));
    }
    // rtrb/Box/Arc offer no recoverable allocation API. Fatal OOM/abort excluded.
    // Their checked size reservations precede consuming any input owner.
    let mut planned = footprint;
    planned.normal_return_slots = AllocationCapacity::of::<ReturnedMediaDomain>(return_capacity);
    planned.terminal_return_slots = AllocationCapacity::of::<Box<CallbackDomainBatch>>(1);
    planned.queue_object_bytes = std::mem::size_of::<RingBuffer<ReturnedMediaDomain>>()
        + std::mem::size_of::<RingBuffer<Box<CallbackDomainBatch>>>();
    planned.callback_batch_object_bytes = std::mem::size_of::<CallbackDomainBatch>();
    planned.observer_object_bytes = std::mem::size_of::<AtomicU8>();
    if planned.construction_data_bound().is_none() {
        return Err(Box::new(DomainSetupFailure {
            inputs,
            reason: DomainRefusal::InvalidCapacity,
            observed: footprint,
        }));
    }
    let (returns, received) = RingBuffer::new(return_capacity);
    let (sender, terminal) = RingBuffer::new(1);
    footprint.queue_object_bytes = planned.queue_object_bytes;
    footprint.normal_return_slots =
        AllocationCapacity::of::<ReturnedMediaDomain>(returns.buffer().capacity());
    footprint.terminal_return_slots =
        AllocationCapacity::of::<Box<CallbackDomainBatch>>(sender.buffer().capacity());
    let seal = Arc::new(());
    let phase = Arc::new(AtomicU8::new(0));
    footprint.observer_object_bytes = planned.observer_object_bytes;
    footprint.callback_batch_object_bytes = planned.callback_batch_object_bytes;
    // All API-reported fallible work is now complete. Original lease Vecs move
    // unchanged; no graph/source owners are cloned or page producers duplicated.
    for input in inputs {
        controls.push(ControlSlot {
            incarnation: 1,
            custody: Default::default(),
            vacant: false,
            data_bytes: input
                .endpoint
                .retained_data_bytes()
                .and_then(|bytes| {
                    bytes.checked_add(
                        AllocationCapacity::of::<CatalogLease>(input.retained_leases.capacity())
                            .bytes()?,
                    )
                })
                .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PageReadiness>()))
                .expect("checked domain bytes"),
            id: input.id,
            control: Some(input.control),
            retained_leases: input.retained_leases,
            endpoint: None,
            report: None,
            terminal_error: None,
        });
        callbacks.push(BatchSlot {
            vacant: false,
            admission_seal: None,
            id: input.id,
            endpoint: Some(Box::new(input.endpoint)),
        });
    }
    while controls.len() < slot_capacity {
        controls.push(ControlSlot {
            incarnation: 0,
            custody: Default::default(),
            vacant: true,
            data_bytes: 0,
            id: DomainId::new(1).expect("positive placeholder"),
            control: None,
            retained_leases: Vec::new(),
            endpoint: None,
            report: None,
            terminal_error: None,
        });
        callbacks.push(BatchSlot {
            vacant: true,
            admission_seal: None,
            id: DomainId::new(1).expect("positive placeholder"),
            endpoint: None,
        });
    }
    Ok((
        Box::new(CallbackDomainBatch {
            reusable,
            slots: callbacks,
            returns,
            seal: Arc::clone(&seal),
        }),
        TerminalBatchSender {
            sender,
            seal: Arc::clone(&seal),
            sent: false,
        },
        MediaDomainReaper {
            slots: controls,
            returns: received,
            terminal,
            terminal_batch: None,
            seal,
            phase,
            footprint,
            reusable,
        },
    ))
}

fn validate_inputs(
    inputs: &[MediaDomainInput],
    return_capacity: usize,
) -> Result<(), DomainRefusal> {
    if return_capacity == 0
        || return_capacity > inputs.len().max(1)
        || return_capacity
            .checked_mul(std::mem::size_of::<ReturnedMediaDomain>())
            .is_none()
    {
        return Err(DomainRefusal::InvalidCapacity);
    }
    for (index, input) in inputs.iter().enumerate() {
        if !input.control.matches_catalog(input.endpoint.catalog()) {
            return Err(DomainRefusal::WrongPair);
        }
        if input
            .retained_leases
            .iter()
            .any(|lease| !input.endpoint.catalog().same_catalog(lease))
        {
            return Err(DomainRefusal::WrongLease);
        }
        if inputs[..index]
            .iter()
            .any(|previous| previous.id == input.id)
        {
            return Err(DomainRefusal::DuplicateIdentity);
        }
        // One actual endpoint/SPSC pair per catalog owner; separate catalogs
        // with identical keys remain independent and are charged independently.
        if inputs[..index].iter().any(|previous| {
            previous
                .endpoint
                .catalog()
                .same_catalog(input.endpoint.catalog())
        }) {
            return Err(DomainRefusal::DuplicateCatalog);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "domain_escrow_tests.rs"]
mod tests;
