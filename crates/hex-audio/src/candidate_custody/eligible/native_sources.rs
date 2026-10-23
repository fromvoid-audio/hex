//! Actual native preparation custody in the existing Root, before endpoint
//! admission. Fixed cells reserve control metadata before helper start. Their
//! variable native/provenance resources remain explicitly UNSEALED: occupied
//! cells cannot close a graph/publication resource witness. Final completion
//! performs native startup/capture in the helper, but no PCM exchange, callback
//! loan, source reload or canonical acceptance occurs here.
use super::*;
use hex_lattice::ProcessorInstanceIdentity;
use hex_lattice_contract::{ForeignProcessorBinding as Binding, NativeDeviceState};
use hex_plugin_host::native::{
    self, Cancellation, Pending, PreparationDisposal, PreparationFailure,
};
use hex_plugin_scan::macos::build_identity::InstanceBuild;

mod completion;

const MAX_SOURCES: usize = hex_lattice::MAX_STATEFUL_PROCESSORS;
type Cancellations = [Option<Cancellation>; MAX_SOURCES];
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
    Retiring,
    /// Source-owner disposal finished; the original return awaits recovery.
    /// This is never prepared-instance or graph acceptance.
    Complete,
    Failed,
}
/// Original construction outcome after actual native disposal AND normal
/// control-side source-owner destruction. Report metadata remains until
/// recovery. No endpoint was ever loaned by this path.
pub struct NativeSourcePreparationReturn {
    pub binding: Binding,
    pub disposal: PreparationDisposal,
    pub error: Option<String>,
    pub cleanup_error: Option<String>,
}
struct Slot {
    binding: Option<Binding>,
    identity: Option<ProcessorInstanceIdentity>,
    phase: Phase,
    starting: bool,
    retiring: bool,
    facts: Option<hex_plugin_host::protocol::Facts>,
    presentation: Option<hex_plugin_host::protocol::Presentation>,
    cancellation: Option<Cancellation>,
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
            cancellation: None,
        }
    }
}
#[derive(Default)]
pub(super) struct Bank {
    slots: Vec<Slot>,
    next_epoch: u64,
}
impl Bank {
    pub(super) fn occupied(&self) -> bool {
        self.slots.iter().any(|s| s.phase != Phase::Vacant)
    }
    pub(super) fn has_work(&self, abandoned: bool) -> bool {
        self.slots.iter().any(|s| {
            s.phase == Phase::Failed
                || s.phase == Phase::Cleaning
                || (s.retiring && s.phase != Phase::Vacant && s.phase != Phase::Complete)
                || (abandoned && s.phase == Phase::Complete)
        })
    }
    pub(super) fn close(&mut self, cancellations: &mut Cancellations) {
        for (slot, target) in self.slots.iter_mut().zip(cancellations) {
            if slot.phase != Phase::Vacant {
                slot.retiring = true;
                *target = slot.cancellation.clone();
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
    Refused(PreparationFailure),
}
#[derive(Default)]
struct Execution {
    owner: Option<Owner>,
    error: Option<String>,
    cleanup_error: Option<String>,
    disposal: Option<PreparationDisposal>,
    pending: bool,
    first_panic: Option<Box<dyn Any + Send>>,
}
#[derive(Default)]
pub(super) struct Work {
    cells: Vec<Execution>,
}
impl Custody {
    pub(in crate::candidate_custody) fn configure_native_sources(
        &self,
        capacity: usize,
    ) -> Result<(), &'static str> {
        if capacity == 0 || capacity > MAX_SOURCES {
            return Err("native preparation capacity is invalid");
        }
        let Ok(mut work) = self.work.try_lock() else {
            return Err("native preparation is busy");
        };
        let quote = capacity
            .checked_mul(std::mem::size_of::<Slot>() + std::mem::size_of::<Execution>())
            .ok_or("native preparation metadata overflow")?;
        {
            let bank = lock(&self.bank);
            native_pool_preflight(&bank, quote)?;
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
        if let Err(error) = native_pool_preflight(&bank, quote) {
            drop(bank); // Refused temporary allocation drops outside the ordinary lock.
            return Err(error);
        }
        bank.sources = Bank {
            slots,
            next_epoch: 1,
        };
        bank.metadata_bytes += quote;
        work.sources = Work { cells };
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Complete typed native request, with Root-derived initial binding.
    pub(in crate::candidate_custody) fn begin_native_source(
        &self,
        identity: ProcessorInstanceIdentity,
        graph: u64,
        build: &InstanceBuild,
        source: native::Source,
        preparation: native::Preparation,
        restore: Option<NativeDeviceState>,
        cancellation: native::PreparationCancellation,
        trust: native::TrustUserCode,
    ) -> Result<Binding, &'static str> {
        self.begin_native_source_between(
            identity,
            graph,
            build,
            source,
            preparation,
            restore,
            cancellation,
            trust,
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
        cancellation: native::PreparationCancellation,
        trust: native::TrustUserCode,
        after_reservation: impl FnOnce(),
    ) -> Result<Binding, &'static str> {
        let Ok(mut work) = self.work.try_lock() else {
            return Err("native preparation is busy");
        };
        let (index, binding) = {
            let mut bank = lock(&self.bank);
            if !self.enabled() || bank.closing || graph == 0 {
                return Err("native preparation is closing or invalid");
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
            slot.cancellation = Some(cancellation.token_on_control());
            slot.starting = true;
            slot.retiring = false;
            slot.phase = Phase::Reserved;
            bank.sources.next_epoch = next;
            (index, binding)
        };
        // This is an initial request, never an existing endpoint/epoch retag.
        preparation.binding = binding;
        work.sources.cells[index].pending = true;
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
                cell.error = Some(owner.to_string());
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
            _ if slot.presentation.is_some() => NativeSourcePreparationProgress::Prepared {
                facts: slot.facts?,
                presentation: slot.presentation?,
            },
            Phase::Reserved => NativeSourcePreparationProgress::Preparing,
            _ => NativeSourcePreparationProgress::Facts(slot.facts?),
        })
    }
    pub(in crate::candidate_custody) fn retire_native_source(&self, binding: Binding) -> bool {
        let cancellation = {
            let mut bank = lock(&self.bank);
            let Some(index) = bank.sources.index(binding) else {
                return false;
            };
            let slot = &mut bank.sources.slots[index];
            if matches!(slot.phase, Phase::Complete | Phase::Failed) {
                return false;
            }
            slot.retiring = true;
            slot.cancellation.clone()
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
        let work = self.work.try_lock().ok()?;
        let bank = self.bank.try_lock().ok()?;
        let index = bank.sources.index(binding)?;
        let cell = work.sources.cells.get(index)?;
        match (&cell.error, &cell.cleanup_error) {
            (Some(error), Some(cleanup)) => Some(format!("{error} · cleanup: {cleanup}")),
            (Some(error), None) | (None, Some(error)) => Some(error.clone()),
            (None, None) => None,
        }
    }

    pub(super) fn advance_native_sources(&self, work: &mut Work, abandoned: bool) {
        for (index, cell) in work.cells.iter_mut().enumerate() {
            let eligible = {
                let bank = lock(&self.bank);
                let slot = &bank.sources.slots[index];
                !slot.starting
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
                    let Some(Owner::Refused(owner)) = cell.owner.as_mut() else {
                        return Err(hex_plugin_host::Error::Protocol(
                            "native preparation owner missing",
                        ));
                    };
                    let disposed = owner.poll_disposal_on_control()?;
                    if disposed.is_some() {
                        let cancellation =
                            lock(&self.bank).sources.slots[index].cancellation.take();
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
                        if cell.cleanup_error.is_none() {
                            cell.cleanup_error = Some(error.to_string());
                        }
                        if !matches!(error, hex_plugin_host::Error::Timeout) {
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
        if bank.sources.slots[index].phase != Phase::Complete {
            return None;
        }
        let cell = &mut work.sources.cells[index];
        if cell.pending || cell.owner.is_some() || cell.first_panic.is_some() {
            return None;
        }
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
mod tests;

fn native_pool_preflight(bank: &NativeBank, quote: usize) -> Result<(), &'static str> {
    if bank.root == 0 || bank.closing || !bank.sources.slots.is_empty() {
        return Err("native preparation reservation is unavailable");
    }
    let data = bank
        .domains
        .iter()
        .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.data))
        .and_then(|n| n.checked_add(bank.publication.retained_bytes()?))
        .and_then(|n| n.checked_add(quote))
        .ok_or("native preparation metadata overflow")?;
    let peak = bank
        .domains
        .iter()
        .try_fold(bank.metadata_bytes, |n, d| n.checked_add(d.peak))
        .and_then(|n| n.checked_add(bank.publication.peak_bytes()?))
        .and_then(|n| n.checked_add(quote))
        .ok_or("native preparation metadata overflow")?;
    if data > bank.reservation.max_retained_data_bytes
        || peak > bank.reservation.max_construction_peak_data_bytes
    {
        return Err("native preparation metadata reservation exceeded");
    }
    Ok(())
}

impl CandidateRoot {
    pub(crate) fn configure_native_source_preparation(
        &self,
        capacity: usize,
    ) -> Result<(), &'static str> {
        self.0.eligible.configure_native_sources(capacity)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin_native_source_preparation(
        &self,
        identity: ProcessorInstanceIdentity,
        graph: u64,
        build: &InstanceBuild,
        source: native::Source,
        preparation: native::Preparation,
        restore: Option<NativeDeviceState>,
        cancellation: native::PreparationCancellation,
        trust: native::TrustUserCode,
    ) -> Result<Binding, &'static str> {
        let result = self.0.eligible.begin_native_source(
            identity,
            graph,
            build,
            source,
            preparation,
            restore,
            cancellation,
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
    pub(crate) fn retire_native_source_preparation(&self, binding: Binding) -> bool {
        let accepted = self.0.eligible.retire_native_source(binding);
        crate::lifecycle::request_candidate_cleanup(self);
        accepted
    }
    pub(crate) fn native_source_preparation_error(&self, binding: Binding) -> Option<String> {
        self.0.eligible.native_source_error(binding)
    }
    pub(crate) fn recover_native_source_preparation(
        &self,
        binding: Binding,
    ) -> Option<NativeSourcePreparationReturn> {
        self.0.eligible.recover_native_source(binding)
    }
}
