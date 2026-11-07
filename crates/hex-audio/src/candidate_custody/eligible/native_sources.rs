//! Actual native sources in the existing Root. Fixed cells and per-source DATA
//! capacity are reserved before helper start. Preparation alone grants no graph
//! authority: accepted original worker/callback construction, unique endpoint
//! loans, actual node matching and original graph references close that witness.
//! Source reports retain custody through native disposal and recovery.
use super::*;
use crate::plugin_effects::NativeEffectReference;
use hex_lattice::ProcessorInstanceIdentity;
use hex_lattice_contract::{ForeignProcessorBinding as Binding, NativeDeviceState};
use hex_plugin_host::native::{
    self, Cancellation, Pending, PreparationDisposal, PreparationFailure,
};
use hex_plugin_scan::macos::build_identity::InstanceBuild;

mod capture;
mod completion;
mod definition;
mod effect;
mod endpoints;
mod epoch;
pub(super) mod ordinary;
mod references;
pub(crate) use ordinary::OrdinaryNativeReferences;
mod resources;
pub(crate) use endpoints::{NativeEndpointBank, NativeEndpointEnvelope};
pub(super) use references::References;

const MAX_SOURCES: usize = hex_lattice::MAX_STATEFUL_PROCESSORS;
type Cancellations = [Option<Cancellation>; MAX_SOURCES];
struct EndpointSignal {
    requested: AtomicBool,
    // Written only by actual callback installation/return. Queue admission is
    // not an endpoint ACK and this observation grants no graph admission.
    installed: AtomicBool,
    // Published by the owning callback before a predecessor graph can return.
    // An ordinary successor can still own that source in an inherited tail.
    runtime_required: AtomicBool,
}
impl EndpointSignal {
    fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
            installed: AtomicBool::new(false),
            runtime_required: AtomicBool::new(false),
        }
    }
    fn load(&self, order: Ordering) -> bool {
        self.requested.load(order)
    }
    fn store(&self, value: bool, order: Ordering) {
        self.requested.store(value, order);
    }
}
/// Per-cell allowances charged for the whole configured pool before any child
/// or cancellation pair starts. These are reserved capacity, not allocated bytes
/// or a producer/resource witness. The same fixed pool also reserves its
/// constructor-derived shared-handle bound. Actual endpoint/resource sealing
/// remains necessary before native publication.
#[derive(Clone, Copy, Debug)]
pub struct NativeSourceDataReservation {
    pub max_retained_data_bytes: usize,
    pub max_construction_peak_data_bytes: usize,
}
impl NativeSourceDataReservation {
    /// Whole fixed pool's listed metadata and per-source DATA allowances.
    /// These are reserved capacities, not actual allocations or opaque native
    /// RSS. The output owner adds them to its existing domain reservation before
    /// configuration; Root still checks actual construction and every admission.
    pub fn pool_data_bytes(self, capacity: usize) -> Option<(usize, usize)> {
        self.validate().ok()?;
        if capacity == 0 || capacity > MAX_SOURCES {
            return None;
        }
        let metadata =
            capacity.checked_mul(std::mem::size_of::<Slot>() + std::mem::size_of::<Execution>())?;
        Some((
            metadata.checked_add(capacity.checked_mul(self.max_retained_data_bytes)?)?,
            metadata.checked_add(capacity.checked_mul(self.max_construction_peak_data_bytes)?)?,
        ))
    }
    fn validate(self) -> Result<(), &'static str> {
        let file = usize::try_from(hex_plugin_scan::MAX_BINARY)
            .map_err(|_| "native validation file bound overflow")?;
        if self.max_retained_data_bytes == 0
            || self
                .max_retained_data_bytes
                .checked_add(file)
                .is_none_or(|minimum| minimum > self.max_construction_peak_data_bytes)
        {
            return Err("native source data reservation is invalid");
        }
        Ok(())
    }
}
pub(super) fn empty_cancellations() -> Cancellations {
    std::array::from_fn(|_| None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeSourcePreparationProgress {
    Preparing,
    Facts(hex_plugin_host::protocol::Facts),
    /// Final native ACK retained by Root; endpoints/resources remain unsealed.
    Prepared {
        facts: hex_plugin_host::protocol::Facts,
        presentation: hex_plugin_host::protocol::Presentation,
    },
    /// Actual unsplit worker/endpoint pair retained by Root, not publication.
    Effect {
        prepared: hex_lattice::PreparedForeignProcessor,
    },
    Retiring,
    /// Source-owner disposal finished; the original return awaits recovery.
    /// This is never prepared-instance or graph acceptance.
    Complete,
    Failed,
}
/// Original construction outcome after actual native disposal AND normal
/// control-side source-owner destruction. Report metadata remains until
/// recovery. Original typed diagnostics move with their backing; presentation
/// is caller-owned. Any unique endpoint loan must have returned first.
pub struct NativeSourcePreparationReturn {
    pub binding: Binding,
    pub disposal: PreparationDisposal,
    pub error: Option<hex_plugin_host::Error>,
    pub cleanup_error: Option<hex_plugin_host::Error>,
}
struct Slot {
    binding: Option<Binding>,
    identity: Option<ProcessorInstanceIdentity>,
    phase: Phase,
    starting: bool,
    retiring: bool,
    facts: Option<hex_plugin_host::protocol::Facts>,
    presentation: Option<(
        hex_plugin_host::protocol::Presentation,
        hex_plugin_host::protocol::PresentationPlan,
    )>,
    processor: Option<hex_lattice::PreparedForeignProcessor>,
    cancellation: Option<Cancellation>,
    cancellation_requested: bool,
    endpoint_loaned: bool,
    endpoint_return: Option<Arc<EndpointSignal>>,
}
impl Slot {
    fn vacant() -> Self {
        Self {
            binding: None,
            identity: None,
            phase: Phase::Vacant,
            starting: false,
            retiring: false,
            facts: None,
            presentation: None,
            processor: None,
            cancellation: None,
            cancellation_requested: false,
            endpoint_loaned: false,
            endpoint_return: None,
        }
    }
    // One Root-owned in-flight request per source. Mark only after the actual
    // token exists; a close before token construction is serviced on install.
    fn request_cancellation(&mut self) -> Option<Cancellation> {
        if self.cancellation_requested {
            return None;
        }
        let cancellation = self.cancellation.as_ref()?.clone();
        self.cancellation_requested = true;
        Some(cancellation)
    }
}
#[derive(Default)]
pub(super) struct Bank {
    slots: Vec<Slot>,
    next_epoch: u64,
    limits: Option<NativeSourceDataReservation>,
    pub(super) retained_reserved: usize,
    pub(super) peak_reserved: usize,
    resources: Option<resources::PoolResources>,
}
impl Bank {
    pub(super) fn cleanup_failed(&self) -> bool {
        self.slots.iter().any(|s| s.phase == Phase::Failed)
    }

    pub(super) fn occupied(&self) -> bool {
        self.slots.iter().any(|s| s.phase != Phase::Vacant)
    }
    pub(super) fn has_work(&self, abandoned: bool, publication: &publication::Bank) -> bool {
        self.slots.iter().enumerate().any(|(index, s)| {
            s.phase == Phase::Failed
                || s.phase == Phase::Cleaning
                || (s.retiring
                    && s.phase != Phase::Vacant
                    && s.phase != Phase::Complete
                    && !publication.references_native_source(index))
                || (abandoned && s.phase == Phase::Complete)
        })
    }
    pub(super) fn request_unreferenced_returns(&self, publication: &publication::Bank) {
        for (index, source) in self.slots.iter().enumerate() {
            if source.retiring
                && !publication.references_native_source(index)
                && let Some(signal) = &source.endpoint_return
                && !signal.runtime_required.load(Ordering::Acquire)
            {
                signal.store(true, Ordering::Release);
            }
        }
    }
    pub(super) fn close(&mut self, cancellations: &mut Cancellations) {
        for (slot, target) in self.slots.iter_mut().zip(cancellations) {
            if slot.phase != Phase::Vacant {
                slot.retiring = true;
                if let Some(signal) = &slot.endpoint_return {
                    signal.store(true, Ordering::Release);
                }
                // A fully constructed effect retires through its existing
                // joining worker/SDK stop. In-flight preparation cancels now.
                if slot.processor.is_none() || slot.phase == Phase::Failed {
                    // Failed/unwound custody cannot enter normal retirement;
                    // wake its original guardian without releasing any owner.
                    *target = slot.request_cancellation();
                }
            }
        }
    }
    fn index(&self, binding: Binding) -> Option<usize> {
        self.slots.iter().position(|s| {
            s.binding == Some(binding)
                && s.identity
                    .is_some_and(|identity| identity.device_id() == binding.instance)
        })
    }
}
#[allow(clippy::large_enum_variant)] // Reserve the entire pending packet in each Root cell before SDK start.
enum Owner {
    Pending(Pending),
    Prepared(native::Prepared),
    Quiescent(native::Quiescent),
    Effect(crate::plugin_effects::NativeEffect),
    Loaned(crate::plugin_effects::NativeEffectControl),
    WorkerRefused(crate::plugin_worker::NativeWorkerStartFailure),
    Refused(PreparationFailure),
}
#[derive(Default)]
struct Execution {
    owner: Option<Owner>,
    error: Option<hex_plugin_host::Error>,
    cleanup_error: Option<hex_plugin_host::Error>,
    // Constructor refusals retain their original cause in the actual owner
    // until disposal. Explicit cancellation of a valid source is not a failure.
    constructor_refused: bool,
    disposal: Option<PreparationDisposal>,
    pending: bool,
    first_panic: Option<Box<dyn Any + Send>>,
}
impl Execution {
    fn cause(&self) -> Option<&hex_plugin_host::Error> {
        self.error.as_ref().or_else(|| {
            if !self.constructor_refused {
                return None;
            }
            match self.owner.as_ref()? {
                Owner::Refused(owner) => Some(owner.cause()),
                Owner::WorkerRefused(owner) => Some(owner.cause()),
                _ => None,
            }
        })
    }
    fn take_constructor_cause(&mut self) {
        if self.constructor_refused {
            let cause = match self.owner.as_mut() {
                Some(Owner::Refused(owner)) => Some(owner.take_cause_on_control()),
                Some(Owner::WorkerRefused(owner)) => Some(owner.take_cause_on_control()),
                _ => None,
            };
            if cause.is_some() {
                self.error = cause;
                self.constructor_refused = false;
            }
        }
    }
}
#[derive(Default)]
pub(super) struct Work {
    cells: Vec<Execution>,
}
impl Custody {
    pub(in crate::candidate_custody) fn configure_native_sources(
        &self,
        capacity: usize,
        limits: NativeSourceDataReservation,
    ) -> Result<(), &'static str> {
        limits.validate()?;
        if capacity == 0 || capacity > MAX_SOURCES {
            return Err("native preparation capacity is invalid");
        }
        let resources = resources::PoolResources::new(capacity)
            .ok_or("native source resource reservation overflow")?;
        let Ok(mut work) = self.work.try_lock() else {
            return Err("native preparation is busy");
        };
        let quote = capacity
            .checked_mul(std::mem::size_of::<Slot>() + std::mem::size_of::<Execution>())
            .ok_or("native preparation metadata overflow")?;
        let retained = capacity
            .checked_mul(limits.max_retained_data_bytes)
            .ok_or("native source reservation overflow")?;
        let peak = capacity
            .checked_mul(limits.max_construction_peak_data_bytes)
            .ok_or("native source reservation overflow")?;
        let (retained_quote, peak_quote) = limits
            .pool_data_bytes(capacity)
            .ok_or("native source reservation overflow")?;
        {
            let bank = lock(&self.bank);
            native_pool_preflight(&bank, retained_quote, peak_quote, resources.shared_handles)?;
        }
        let mut slots = Vec::new();
        let mut cells = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(|_| "native preparation allocation refused")?;
        cells
            .try_reserve_exact(capacity)
            .map_err(|_| "native preparation allocation refused")?;
        if slots.capacity() != capacity || cells.capacity() != capacity {
            return Err("native preparation allocator capacity differs");
        }
        slots.resize_with(capacity, Slot::vacant);
        cells.resize_with(capacity, Execution::default);
        let mut bank = lock(&self.bank);
        if let Err(error) =
            native_pool_preflight(&bank, retained_quote, peak_quote, resources.shared_handles)
        {
            drop(bank); // Refused temporary allocation drops outside the ordinary lock.
            return Err(error);
        }
        bank.sources = Bank {
            slots,
            next_epoch: 1,
            limits: Some(limits),
            retained_reserved: retained,
            peak_reserved: peak,
            resources: Some(resources),
        };
        bank.metadata_bytes += quote;
        work.sources = Work { cells };
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Complete typed native request, with Root-derived binding and cancellation.
    pub(in crate::candidate_custody) fn begin_native_source(
        &self,
        identity: ProcessorInstanceIdentity,
        graph: u64,
        build: &InstanceBuild,
        source: native::Source,
        preparation: native::Preparation,
        restore: Option<NativeDeviceState>,
        trust: native::TrustUserCode,
    ) -> Result<Binding, &'static str> {
        self.begin_native_source_between(
            identity,
            graph,
            build,
            source,
            preparation,
            restore,
            trust,
            native::PreparationCancellation::new_on_control,
            || {},
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_native_source_between(
        &self,
        identity: ProcessorInstanceIdentity,
        graph: u64,
        build: &InstanceBuild,
        source: native::Source,
        mut preparation: native::Preparation,
        restore: Option<NativeDeviceState>,
        trust: native::TrustUserCode,
        make_cancellation: impl FnOnce() -> Result<
            native::PreparationCancellation,
            hex_plugin_host::Error,
        >,
        after_reservation: impl FnOnce(),
    ) -> Result<Binding, &'static str> {
        let Ok(mut work) = self.work.try_lock() else {
            return Err("native preparation is busy");
        };
        let storage_bound = source
            .preparation_storage_bound_on_control(build, restore.as_ref())
            .ok_or("native source input storage is invalid or overflowing")?;
        let (index, binding) = {
            let mut bank = lock(&self.bank);
            if !self.enabled() || bank.closing || graph == 0 {
                return Err("native preparation is closing or invalid");
            }
            let limits = bank
                .sources
                .limits
                .ok_or("native source data is not reserved")?;
            if storage_bound.input_data_bytes > limits.max_retained_data_bytes {
                return Err("native source input exceeds retained data reservation");
            }
            if storage_bound.max_retained_data_bytes() > limits.max_retained_data_bytes {
                return Err("native preparation exceeds retained data reservation");
            }
            if storage_bound
                .base_peak_data_bytes()
                .ok_or("native preparation peak storage overflow")?
                > limits.max_construction_peak_data_bytes
            {
                return Err("native preparation exceeds known peak data reservation");
            }
            let index = bank
                .sources
                .slots
                .iter()
                .position(|s| s.phase == Phase::Vacant)
                .ok_or("native preparation cells are full")?;
            let binding = Binding {
                root: bank.root,
                instance: identity.device_id(),
                graph,
                epoch: bank.sources.next_epoch,
            };
            let next = bank
                .sources
                .next_epoch
                .checked_add(1)
                .ok_or("native preparation epoch exhausted")?;
            if work
                .sources
                .cells
                .get(index)
                .is_none_or(|c| c.owner.is_some() || c.pending)
            {
                return Err("native preparation destination is occupied");
            }
            let slot = &mut bank.sources.slots[index];
            slot.binding = Some(binding);
            slot.identity = Some(identity);
            // Root creates and retains its own pair after reserving this cell.
            slot.cancellation = None;
            slot.starting = true;
            slot.retiring = false;
            slot.phase = Phase::Reserved;
            bank.sources.next_epoch = next;
            (index, binding)
        };
        // This is an initial request, never an existing endpoint/epoch retag.
        preparation.binding = binding;
        work.sources.cells[index].pending = true;
        let cancellation = match make_cancellation() {
            Ok(cancellation) => cancellation,
            Err(error) => {
                // This exact production factory failed before the native
                // constructor was invoked. No instance was ever started.
                let cell = &mut work.sources.cells[index];
                cell.error = Some(error);
                drop(source);
                drop(restore);
                cell.disposal = Some(PreparationDisposal::NoInstance);
                cell.pending = false;
                let mut bank = lock(&self.bank);
                let slot = &mut bank.sources.slots[index];
                slot.starting = false;
                slot.retiring = true;
                slot.phase = Phase::Complete;
                return Ok(binding);
            }
        };
        let cancel = {
            let mut bank = lock(&self.bank);
            let closing = bank.closing;
            let slot = &mut bank.sources.slots[index];
            slot.cancellation = Some(cancellation.token_on_control());
            if closing || slot.retiring {
                slot.request_cancellation()
            } else {
                None
            }
        };
        if let Some(cancel) = cancel {
            cancel.cancel_on_control();
        }
        after_reservation();
        // Dedicated existing execution custody spans bounded control I/O. No
        // ordinary bank lock spans construction, native calls or destruction.
        let result = catch_unwind(AssertUnwindSafe(|| match restore {
            Some(state) => Pending::prepare_restored_retained_on_control(
                build,
                source,
                preparation,
                state,
                cancellation,
                trust,
            ),
            None => Pending::prepare_retained_on_control(
                build,
                source,
                preparation,
                cancellation,
                trust,
            ),
        }));
        let cell = &mut work.sources.cells[index];
        let facts = match result {
            Ok(Ok(owner)) => {
                let facts = owner.facts();
                cell.owner = Some(Owner::Pending(owner));
                Some(facts)
            }
            Ok(Err(owner)) => {
                cell.constructor_refused = true;
                cell.owner = Some(Owner::Refused(owner));
                None
            }
            Err(payload) => {
                cell.first_panic = Some(payload);
                self.fail_native_source(index);
                return Ok(binding);
            }
        };
        cell.pending = false;
        let mut bank = lock(&self.bank);
        let closing = bank.closing;
        let slot = &mut bank.sources.slots[index];
        slot.starting = false;
        slot.facts = facts;
        slot.retiring |= closing || facts.is_none();
        slot.phase = if facts.is_some() {
            Phase::Accepted
        } else {
            Phase::Cleaning
        };
        Ok(binding) // Accepted custody, including close/refusal; never graph admission.
    }

    pub(in crate::candidate_custody) fn native_source_progress(
        &self,
        binding: Binding,
    ) -> Option<NativeSourcePreparationProgress> {
        let bank = self.bank.try_lock().ok()?;
        let slot = &bank.sources.slots[bank.sources.index(binding)?];
        Some(match slot.phase {
            Phase::Failed => NativeSourcePreparationProgress::Failed,
            Phase::Complete => NativeSourcePreparationProgress::Complete,
            _ if slot.retiring => NativeSourcePreparationProgress::Retiring,
            _ if slot.starting => NativeSourcePreparationProgress::Preparing,
            _ if slot.processor.is_some() => NativeSourcePreparationProgress::Effect {
                prepared: slot.processor?,
            },
            _ if slot.presentation.is_some() => NativeSourcePreparationProgress::Prepared {
                facts: slot.facts?,
                presentation: slot.presentation?.0,
            },
            Phase::Reserved => NativeSourcePreparationProgress::Preparing,
            _ => NativeSourcePreparationProgress::Facts(slot.facts?),
        })
    }
    pub(in crate::candidate_custody) fn retire_native_source(&self, binding: Binding) -> bool {
        // The original Work guard also spans graph preflight/split/commit.
        // A busy control request refuses without requesting endpoint return.
        let Ok(_work) = self.work.try_lock() else {
            return false;
        };
        let cancellation = {
            let mut bank = lock(&self.bank);
            let Some(index) = bank.sources.index(binding) else {
                return false;
            };
            let referenced = bank.publication.references_native_source(index);
            let slot = &mut bank.sources.slots[index];
            if matches!(slot.phase, Phase::Complete | Phase::Failed) {
                return false;
            }
            slot.retiring = true;
            if !referenced
                && let Some(signal) = &slot.endpoint_return
                && !signal.runtime_required.load(Ordering::Acquire)
            {
                signal.store(true, Ordering::Release);
            }
            if slot.processor.is_none() {
                slot.request_cancellation()
            } else {
                None
            }
        };
        if let Some(cancellation) = cancellation {
            cancellation.cancel_on_control();
        }
        true
    }
    pub(in crate::candidate_custody) fn native_source_error(
        &self,
        binding: Binding,
    ) -> Option<String> {
        self.try_native_source_error(binding).ok().flatten()
    }
    pub(in crate::candidate_custody) fn try_native_source_error(
        &self,
        binding: Binding,
    ) -> Result<Option<String>, &'static str> {
        let work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        let bank = self
            .bank
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        let index = bank.sources.index(binding).ok_or("native source differs")?;
        let cell = work
            .sources
            .cells
            .get(index)
            .ok_or("native source cell unavailable")?;
        Ok(match (cell.cause(), &cell.cleanup_error) {
            (Some(error), Some(cleanup)) => Some(format!("{error} · cleanup: {cleanup}")),
            (Some(error), None) | (None, Some(error)) => Some(error.to_string()),
            (None, None) => None,
        })
    }

    pub(super) fn advance_native_sources(&self, work: &mut Work, abandoned: bool) {
        // Ordinary tails can release their last runtime requirement after the
        // introducing graph has already returned. Re-observe that callback
        // frontier on this existing control pump, not only on constructor-slot
        // release or the original retirement request.
        {
            let bank = lock(&self.bank);
            bank.sources.request_unreferenced_returns(&bank.publication);
        }
        for (index, cell) in work.cells.iter_mut().enumerate() {
            let eligible = {
                let bank = lock(&self.bank);
                let slot = &bank.sources.slots[index];
                !slot.starting
                    && !bank.publication.references_native_source(index)
                    && !slot.endpoint_loaned
                    && slot.retiring
                    && slot.phase != Phase::Vacant
                    && slot.phase != Phase::Complete
                    && slot.phase != Phase::Failed
            };
            if eligible && !cell.pending && cell.first_panic.is_none() {
                cell.pending = true;
                let result = catch_unwind(AssertUnwindSafe(|| {
                    if let Some(Owner::Pending(_)) = cell.owner.as_ref()
                        && let Some(Owner::Pending(owner)) = cell.owner.take()
                    {
                        cell.owner = Some(Owner::Refused(owner.cancel_retained_on_control()));
                    }
                    if let Some(Owner::Prepared(_)) = cell.owner.as_ref()
                        && let Some(Owner::Prepared(owner)) = cell.owner.take()
                    {
                        cell.owner = Some(Owner::Refused(owner.cancel_retained_on_control()));
                    }
                    if let Some(Owner::Quiescent(_)) = cell.owner.as_ref()
                        && let Some(Owner::Quiescent(owner)) = cell.owner.take()
                    {
                        cell.owner = Some(Owner::Refused(owner.cancel_retained_on_control()));
                    }
                    let disposed = match cell.owner.as_mut() {
                        Some(Owner::Refused(owner)) => owner.poll_disposal_on_control()?,
                        Some(Owner::WorkerRefused(owner)) => owner.poll_disposal_on_control()?,
                        Some(Owner::Effect(owner)) => {
                            let retired = owner.retire_on_control()?;
                            if let Some(error) = retired.failure {
                                cell.error = Some(error);
                            }
                            Some(PreparationDisposal::Reaped(retired.native))
                        }
                        _ => {
                            return Err(hex_plugin_host::Error::Protocol(
                                "native preparation owner missing",
                            ));
                        }
                    };
                    if disposed.is_some() {
                        let cancellation =
                            lock(&self.bank).sources.slots[index].cancellation.take();
                        // Preserve the actual diagnostic before any owner or
                        // cancellation destruction can unwind. No new backing
                        // is allocated and disposal custody stays in the owner.
                        cell.take_constructor_cause();
                        drop(cell.owner.take());
                        drop(cancellation);
                    }
                    Ok(disposed)
                }));
                cell.pending = false;
                match result {
                    Ok(Ok(Some(disposal))) => {
                        cell.disposal = Some(disposal);
                        lock(&self.bank).sources.slots[index].phase = Phase::Complete;
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        let timed_out = matches!(error, hex_plugin_host::Error::Timeout);
                        if cell.cleanup_error.is_none() {
                            cell.cleanup_error = Some(error);
                        }
                        if !timed_out {
                            self.fail_native_source(index);
                        }
                    }
                    Err(payload) => {
                        cell.first_panic = Some(payload);
                        cell.pending = true;
                        self.fail_native_source(index);
                    }
                }
            }
            if abandoned
                && !cell.pending
                && cell.first_panic.is_none()
                && lock(&self.bank).sources.slots[index].phase == Phase::Complete
            {
                let cancellation = lock(&self.bank).sources.slots[index].cancellation.take();
                cell.pending = true;
                let result = catch_unwind(AssertUnwindSafe(|| {
                    drop(cancellation);
                    drop(cell.error.take());
                    drop(cell.cleanup_error.take());
                }));
                match result {
                    Ok(()) => {
                        cell.disposal = None;
                        cell.constructor_refused = false;
                        cell.pending = false;
                        lock(&self.bank).sources.slots[index] = Slot::vacant();
                    }
                    Err(payload) => {
                        cell.first_panic = Some(payload);
                        self.fail_native_source(index);
                    }
                }
            }
        }
    }
    fn fail_native_source(&self, index: usize) {
        {
            let mut bank = lock(&self.bank);
            bank.sources.slots[index].phase = Phase::Failed;
            self.mark_closing(&mut bank);
        }
        self.close(); // Cancel other actual sources outside the ordinary lock.
    }
    pub(in crate::candidate_custody) fn recover_native_source(
        &self,
        binding: Binding,
    ) -> Option<NativeSourcePreparationReturn> {
        let mut work = self.work.try_lock().ok()?;
        let mut bank = lock(&self.bank);
        let index = bank.sources.index(binding)?;
        if bank.publication.references_native_source(index)
            || bank.sources.slots[index].phase != Phase::Complete
        {
            return None;
        }
        let cell = &mut work.sources.cells[index];
        if cell.pending || cell.owner.is_some() || cell.first_panic.is_some() {
            return None;
        }
        cell.constructor_refused = false;
        let result = NativeSourcePreparationReturn {
            binding,
            disposal: cell.disposal.take()?,
            error: cell.error.take(),
            cleanup_error: cell.cleanup_error.take(),
        };
        let cancellation = bank.sources.slots[index].cancellation.take();
        bank.sources.slots[index] = Slot::vacant();
        drop(bank);
        drop(cancellation);
        Some(result)
    }
}

#[cfg(test)]
pub(crate) mod tests;

fn native_pool_preflight(
    bank: &NativeBank,
    retained_quote: usize,
    peak_quote: usize,
    shared_handles: usize,
) -> Result<(), &'static str> {
    if bank.root == 0 || bank.closing || !bank.sources.slots.is_empty() {
        return Err("native preparation reservation is unavailable");
    }
    if shared_handles
        > publication::remaining_handle_reservation(bank)
            .ok_or("native source handle reservation overflow")?
    {
        return Err("native preparation resource reservation exceeded");
    }
    let data = bank
        .domains
        .iter()
        .try_fold(bank.accounted_base(false)?, |n, d| n.checked_add(d.data))
        .and_then(|n| n.checked_add(bank.publication.retained_bytes()?))
        .and_then(|n| n.checked_add(retained_quote))
        .ok_or("native preparation metadata overflow")?;
    let peak = bank
        .domains
        .iter()
        .try_fold(bank.accounted_base(true)?, |n, d| n.checked_add(d.peak))
        .and_then(|n| n.checked_add(bank.publication.peak_bytes()?))
        .and_then(|n| n.checked_add(peak_quote))
        .ok_or("native preparation metadata overflow")?;
    if data > bank.reservation.max_retained_data_bytes
        || peak > bank.reservation.max_construction_peak_data_bytes
    {
        return Err("native preparation pool reservation exceeded");
    }
    Ok(())
}

impl CandidateRoot {
    pub(crate) fn has_vacant_native_source_slots(&self, needed: usize) -> bool {
        self.0.eligible.bank.try_lock().is_ok_and(|bank| {
            !bank.closing
                && bank.sources.limits.is_some()
                && bank
                    .sources
                    .slots
                    .iter()
                    .filter(|slot| slot.phase == Phase::Vacant)
                    .take(needed)
                    .count()
                    == needed
        })
    }

    fn require_native_source_control_open(&self) -> Result<(), &'static str> {
        let bank = lock(&self.0.bank);
        if bank.closing || bank.retiring {
            return Err("native source control is closing");
        }
        Ok(())
    }
    pub(crate) fn configure_native_source_preparation(
        &self,
        capacity: usize,
        limits: NativeSourceDataReservation,
    ) -> Result<(), &'static str> {
        self.require_native_source_control_open()?;
        self.0.eligible.configure_native_sources(capacity, limits)
    }
    #[allow(clippy::too_many_arguments)] // Same complete source request as the public control boundary.
    pub(crate) fn begin_native_source_preparation(
        &self,
        identity: ProcessorInstanceIdentity,
        graph: u64,
        build: &InstanceBuild,
        source: native::Source,
        preparation: native::Preparation,
        restore: Option<NativeDeviceState>,
        trust: native::TrustUserCode,
    ) -> Result<Binding, &'static str> {
        self.require_native_source_control_open()?;
        let result = self.0.eligible.begin_native_source(
            identity,
            graph,
            build,
            source,
            preparation,
            restore,
            trust,
        );
        // A failed or late-closing construction already belongs to Root. Its
        // disposal must progress even when the preparing caller stops polling.
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
    pub(crate) fn native_source_preparation_progress(
        &self,
        binding: Binding,
    ) -> Option<NativeSourcePreparationProgress> {
        self.0.eligible.native_source_progress(binding)
    }
    pub(crate) fn native_source_endpoint_installed(&self, binding: Binding) -> Option<bool> {
        let bank = self.0.eligible.bank.try_lock().ok()?;
        let slot = bank.sources.slots.get(bank.sources.index(binding)?)?;
        if bank.closing
            || slot.phase != Phase::Accepted
            || slot.retiring
            || slot.cancellation_requested
        {
            return None;
        }
        Some(
            slot.endpoint_loaned
                && slot
                    .endpoint_return
                    .as_ref()
                    .is_some_and(|signal| signal.installed.load(Ordering::Acquire)),
        )
    }
    pub(crate) fn native_source_endpoint_status(&self, binding: Binding) -> Option<(bool, bool)> {
        self.with_native_source_endpoint(binding, |loaned, endpoint| {
            (loaned, endpoint.transport_is_live_on_control())
        })
    }
    pub(crate) fn native_source_received_program(
        &self,
        binding: Binding,
    ) -> Option<crate::plugin_worker::ReceivedProgram> {
        self.with_native_source_endpoint(binding, |_, endpoint| {
            endpoint.received_program_on_control()
        })?
    }
    pub(crate) fn native_source_processed_program(
        &self,
        binding: Binding,
    ) -> Option<crate::plugin_worker::ReceivedProgram> {
        self.with_native_source_endpoint(binding, |_, endpoint| {
            endpoint.processed_program_on_control()
        })?
    }
    pub(crate) fn take_native_source_failure_diagnostic(&self, binding: Binding) -> Option<String> {
        self.with_native_source_endpoint(binding, |_, endpoint| {
            endpoint.take_failure_diagnostic_on_control()
        })?
    }
    fn with_native_source_endpoint<T>(
        &self,
        binding: Binding,
        read: impl FnOnce(bool, NativeEffectReference<'_>) -> T,
    ) -> Option<T> {
        let work = self.0.eligible.work.try_lock().ok()?;
        let bank = self.0.eligible.bank.try_lock().ok()?;
        let index = bank.sources.index(binding)?;
        let slot = bank.sources.slots.get(index)?;
        if bank.closing
            || slot.phase != Phase::Accepted
            || slot.retiring
            || slot.starting
            || slot.cancellation_requested
        {
            return None;
        }
        let endpoint = match work.sources.cells.get(index)?.owner.as_ref()? {
            Owner::Effect(effect) => NativeEffectReference::Unsplit(effect),
            Owner::Loaned(control) => NativeEffectReference::Loaned(control),
            _ => return None,
        };
        Some(read(slot.endpoint_loaned, endpoint))
    }
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn native_source_completed_blocks(&self, binding: Binding) -> Option<u64> {
        let work = self.0.eligible.work.try_lock().ok()?;
        let bank = self.0.eligible.bank.try_lock().ok()?;
        let index = bank.sources.index(binding)?;
        match work.sources.cells.get(index)?.owner.as_ref()? {
            Owner::Effect(effect) => {
                Some(NativeEffectReference::Unsplit(effect).completed_blocks_on_control())
            }
            Owner::Loaned(control) => {
                Some(NativeEffectReference::Loaned(control).completed_blocks_on_control())
            }
            _ => None,
        }
    }
    /// Headless control fixture only: retain the actual Work exclusion while
    /// another control request observes contention. This runs outside a callback
    /// entry and changes no slot, source, native state or acknowledgement.
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn with_native_source_work_held_on_control<R>(
        &self,
        run: impl FnOnce() -> R,
    ) -> Option<R> {
        let _work = self.0.eligible.work.lock().ok()?;
        Some(run())
    }
    pub(crate) fn retire_native_source_preparation(&self, binding: Binding) -> bool {
        let accepted = self.0.eligible.retire_native_source(binding);
        crate::lifecycle::request_candidate_cleanup(self);
        accepted
    }
    pub(crate) fn native_source_preparation_error(&self, binding: Binding) -> Option<String> {
        self.0.eligible.native_source_error(binding)
    }
    pub(crate) fn try_native_source_preparation_error(
        &self,
        binding: Binding,
    ) -> Result<Option<String>, &'static str> {
        self.0.eligible.try_native_source_error(binding)
    }
    pub(crate) fn recover_native_source_preparation(
        &self,
        binding: Binding,
    ) -> Option<NativeSourcePreparationReturn> {
        self.0.eligible.recover_native_source(binding)
    }
}
