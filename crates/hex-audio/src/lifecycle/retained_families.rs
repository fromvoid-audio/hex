//! Bounded application-control jobs in the existing lifecycle custodian.
//! Routing and cleanup are not storage/admission witnesses.
use std::{
    any::Any,
    marker::PhantomData,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Mutex, OnceLock, RwLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

static NEXT_TABLE: AtomicU64 = AtomicU64::new(1);
static TABLE: OnceLock<Result<RetainedFamilies, RetainedFamilyError>> = OnceLock::new();

/// Component bytes must come from the existing joint retained/peak allowance.
#[derive(Clone, Copy, Debug)]
pub struct RetainedFamilyReservation {
    pub family_slots: usize,
    pub max_table_bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedFamilyError {
    Reservation,
    Allocation,
    Exhausted,
    Busy,
    Stale,
    Failed,
}
pub use hex_engine::RetainedControlJobProgress as RetainedFamilyProgress;
/// Reserve before starting an export-capable Session. No owning shutdown send.
#[derive(Debug)]
pub struct RetainedFamilySlot {
    table: u64,
    index: usize,
    generation: u64,
}
/// Nonowning route. Exact type dispatch does not establish provenance.
#[derive(Debug)]
pub struct RetainedFamilyId<T> {
    table: u64,
    index: usize,
    generation: u64,
    marker: PhantomData<fn() -> T>,
}
impl<T> Copy for RetainedFamilyId<T> {}
impl<T> Clone for RetainedFamilyId<T> {
    fn clone(&self) -> Self {
        *self
    }
}

enum Phase {
    Vacant,
    Reserved,
    Active,
    Failed,
}
struct Slot {
    generation: u64,
    phase: Phase,
    job: Option<hex_engine::RetainedControlJob>,
    first_panic: Option<Mutex<Box<dyn Any + Send>>>,
    // Constructor component reservation survives a consumed final-Drop fault.
    // It is not a claim about partially freed physical bytes/payload children.
    body_reservation_bytes: usize,
}
/// Stable per-family guards, never a service-wide mutex around render work.
/// The lifecycle scan only uses try_write; busy/failed families cannot block
/// unrelated native work. A read protects the actual boxed family allocation.
struct RetainedFamilies {
    identity: u64,
    slots: Vec<RwLock<Slot>>,
    next_scan: AtomicUsize,
}
impl RetainedFamilies {
    fn quote(limits: RetainedFamilyReservation) -> Result<usize, RetainedFamilyError> {
        if limits.family_slots == 0 {
            return Err(RetainedFamilyError::Reservation);
        }
        std::mem::size_of::<Self>()
            .checked_add(
                limits
                    .family_slots
                    .checked_mul(std::mem::size_of::<RwLock<Slot>>())
                    .ok_or(RetainedFamilyError::Reservation)?,
            )
            .filter(|n| *n <= limits.max_table_bytes)
            .ok_or(RetainedFamilyError::Reservation)
    }
    fn new(limits: RetainedFamilyReservation) -> Result<Self, RetainedFamilyError> {
        Self::quote(limits)?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(limits.family_slots)
            .map_err(|_| RetainedFamilyError::Allocation)?;
        let actual = std::mem::size_of::<Self>()
            .checked_add(
                slots
                    .capacity()
                    .checked_mul(std::mem::size_of::<RwLock<Slot>>())
                    .ok_or(RetainedFamilyError::Reservation)?,
            )
            .ok_or(RetainedFamilyError::Reservation)?;
        if actual > limits.max_table_bytes {
            return Err(RetainedFamilyError::Reservation);
        }
        slots.resize_with(limits.family_slots, || {
            RwLock::new(Slot {
                generation: 0,
                phase: Phase::Vacant,
                job: None,
                first_panic: None,
                body_reservation_bytes: 0,
            })
        });
        let identity = NEXT_TABLE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| RetainedFamilyError::Exhausted)?;
        Ok(Self {
            identity,
            slots,
            next_scan: AtomicUsize::new(0),
        })
    }
    fn reserve(&self) -> Result<RetainedFamilySlot, RetainedFamilyError> {
        for (index, slot) in self.slots.iter().enumerate() {
            let Ok(mut slot) = slot.try_write() else {
                continue;
            };
            if !matches!(slot.phase, Phase::Vacant) {
                continue;
            }
            let Some(generation) = slot.generation.checked_add(1) else {
                continue;
            };
            slot.generation = generation;
            slot.phase = Phase::Reserved;
            return Ok(RetainedFamilySlot {
                table: self.identity,
                index,
                generation,
            });
        }
        Err(RetainedFamilyError::Reservation)
    }
    fn slot(&self, token: &RetainedFamilySlot) -> Result<&RwLock<Slot>, RetainedFamilyError> {
        if token.table != self.identity {
            return Err(RetainedFamilyError::Stale);
        }
        self.slots
            .get(token.index)
            .ok_or(RetainedFamilyError::Stale)
    }
    fn cancel(&self, token: &RetainedFamilySlot) -> Result<(), RetainedFamilyError> {
        let mut slot = self
            .slot(token)?
            .try_write()
            .map_err(|_| RetainedFamilyError::Busy)?;
        if slot.generation != token.generation || !matches!(slot.phase, Phase::Reserved) {
            return Err(RetainedFamilyError::Stale);
        }
        slot.phase = Phase::Vacant;
        Ok(())
    }
    fn register<T: Send + Sync + 'static>(
        &self,
        token: &RetainedFamilySlot,
        source: &mut Option<T>,
        max_job_body_bytes: usize,
        step: fn(&mut T) -> Result<RetainedFamilyProgress, RetainedFamilyError>,
    ) -> Result<RetainedFamilyId<T>, RetainedFamilyError> {
        // Fresh requested Box body only. Child owners and old/new/input peak
        // are separate obligations; this number never certifies either.
        if hex_engine::RetainedControlJob::body_reservation_quote::<T, RetainedFamilyError>()
            > max_job_body_bytes
        {
            return Err(RetainedFamilyError::Reservation);
        }
        let mut slot = self
            .slot(token)?
            .try_write()
            .map_err(|_| RetainedFamilyError::Busy)?;
        if slot.generation != token.generation || !matches!(slot.phase, Phase::Reserved) {
            return Err(RetainedFamilyError::Stale);
        }
        let job = hex_engine::RetainedControlJob::try_from_owned_on_control(
            source,
            max_job_body_bytes,
            step,
        )
        .map_err(|error| match error {
            hex_engine::RetainedControlJobBuildError::Reservation => {
                RetainedFamilyError::Reservation
            }
            hex_engine::RetainedControlJobBuildError::MissingSource => RetainedFamilyError::Stale,
        })?;
        slot.job = Some(job);
        slot.body_reservation_bytes =
            hex_engine::RetainedControlJob::body_reservation_quote::<T, RetainedFamilyError>();
        slot.phase = Phase::Active;
        Ok(RetainedFamilyId {
            table: self.identity,
            index: token.index,
            generation: token.generation,
            marker: PhantomData,
        })
    }
    fn read<T: Send + Sync + 'static, R>(
        &self,
        id: RetainedFamilyId<T>,
        read: impl for<'a> FnOnce(&'a T) -> R,
    ) -> Result<R, RetainedFamilyError> {
        let token = RetainedFamilySlot {
            table: id.table,
            index: id.index,
            generation: id.generation,
        };
        let slot = self
            .slot(&token)?
            .try_read()
            .map_err(|_| RetainedFamilyError::Busy)?;
        if slot.generation != id.generation {
            return Err(RetainedFamilyError::Stale);
        }
        if matches!(slot.phase, Phase::Failed) {
            return Err(RetainedFamilyError::Failed);
        }
        if !matches!(slot.phase, Phase::Active) {
            return Err(RetainedFamilyError::Stale);
        }
        slot.job
            .as_ref()
            .and_then(|job| job.inspect_payload::<T, R>(read))
            .ok_or(RetainedFamilyError::Stale)
    }
    fn inspect_on_control<T: Send + Sync + 'static, R>(
        &self,
        id: RetainedFamilyId<T>,
        inspect: impl for<'a> FnOnce(&'a hex_engine::RetainedControlJob, &'a T) -> R,
    ) -> Result<R, RetainedFamilyError> {
        let token = RetainedFamilySlot {
            table: id.table,
            index: id.index,
            generation: id.generation,
        };
        let slot = self
            .slot(&token)?
            .try_read()
            .map_err(|_| RetainedFamilyError::Busy)?;
        if slot.generation != id.generation || !matches!(slot.phase, Phase::Active | Phase::Failed)
        {
            return Err(RetainedFamilyError::Stale);
        }
        // A failed STEP retains the actual holder for accounting. A consumed
        // final-Drop failure has no inspectable object: unresolved charge stays.
        let job = slot.job.as_ref().ok_or(RetainedFamilyError::Failed)?;
        job.inspect_payload::<T, _>(|payload| inspect(job, payload))
            .ok_or(RetainedFamilyError::Stale)
    }
    fn unresolved_body_reservation<T>(
        &self,
        id: RetainedFamilyId<T>,
    ) -> Result<Option<usize>, RetainedFamilyError> {
        let token = RetainedFamilySlot {
            table: id.table,
            index: id.index,
            generation: id.generation,
        };
        let slot = self
            .slot(&token)?
            .try_read()
            .map_err(|_| RetainedFamilyError::Busy)?;
        if slot.generation != id.generation {
            return Err(RetainedFamilyError::Stale);
        }
        Ok((matches!(slot.phase, Phase::Failed) && slot.job.is_none())
            .then_some(slot.body_reservation_bytes))
    }
    fn acknowledge<T: Send + Sync + 'static>(
        &self,
        id: RetainedFamilyId<T>,
        acknowledge: impl FnOnce(&T) -> Result<(), RetainedFamilyError>,
    ) -> Result<(), RetainedFamilyError> {
        let token = RetainedFamilySlot {
            table: id.table,
            index: id.index,
            generation: id.generation,
        };
        // A valid read must not lose its durable release merely because a
        // bounded control step currently has the family writer. This shared
        // guard does not wait for another rendering reader or native barrier.
        // The concrete adapter's outstanding read forbids an owning destructor.
        let slot = self
            .slot(&token)?
            .read()
            .map_err(|_| RetainedFamilyError::Failed)?;
        if slot.generation != id.generation {
            return Err(RetainedFamilyError::Stale);
        }
        if matches!(slot.phase, Phase::Failed) {
            return Err(RetainedFamilyError::Failed);
        }
        if !matches!(slot.phase, Phase::Active) {
            return Err(RetainedFamilyError::Stale);
        }
        slot.job
            .as_ref()
            .and_then(|job| job.inspect_payload::<T, _>(acknowledge))
            .ok_or(RetainedFamilyError::Stale)?
    }
    fn advance_one(&self) {
        let index = self.next_scan.fetch_add(1, Ordering::Relaxed) % self.slots.len();
        let Ok(mut slot) = self.slots[index].try_write() else {
            return;
        };
        if !matches!(slot.phase, Phase::Active) {
            return;
        }
        let Some(job) = slot.job.as_mut() else {
            slot.phase = Phase::Failed;
            return;
        };
        match catch_unwind(AssertUnwindSafe(|| job.step_on_control())) {
            Ok(Ok(RetainedFamilyProgress::Pending)) => {}
            Ok(Ok(RetainedFamilyProgress::Disposed)) => {
                slot.phase = Phase::Failed;
                let job = slot.job.take();
                match catch_unwind(AssertUnwindSafe(|| drop(job))) {
                    Ok(()) => {
                        slot.body_reservation_bytes = 0;
                        slot.phase = Phase::Vacant;
                    }
                    Err(panic) => slot.first_panic = Some(Mutex::new(panic)),
                }
            }
            Ok(Err(_)) => slot.phase = Phase::Failed,
            Err(panic) => {
                slot.phase = Phase::Failed;
                slot.first_panic = Some(Mutex::new(panic));
            }
        }
    }
}

/// Explicitly reserve the persistent backing before any capability escape.
/// No default capacity, queue-size inference or allowance per output reopen.
pub fn retained_family_table_reservation_quote(
    limits: RetainedFamilyReservation,
) -> Result<usize, RetainedFamilyError> {
    RetainedFamilies::quote(limits)
}
/// Fresh job Box body component only, not child storage or old/new/input peak.
pub fn retained_family_job_body_quote<T>() -> usize {
    hex_engine::RetainedControlJob::body_reservation_quote::<T, RetainedFamilyError>()
}
// This exact facade sequence is shared by production's public calls and
// injected startup/exit/concurrent-construction tests. OnceLock serializes the
// entire constructor, not just installation of an already allocated backing.
fn configure_table(
    table: &OnceLock<Result<RetainedFamilies, RetainedFamilyError>>,
    readiness: &super::ServiceReadiness,
    construct: impl FnOnce() -> Result<RetainedFamilies, RetainedFamilyError>,
    start: impl FnOnce() -> Result<(), RetainedFamilyError>,
) -> Result<(), RetainedFamilyError> {
    let mut constructed = false;
    let result = table.get_or_init(|| {
        constructed = true;
        construct()
    });
    result.as_ref().map_err(|error| *error)?;
    if !constructed {
        return Err(RetainedFamilyError::Reservation);
    }
    if let Err(error) = start() {
        readiness.failed();
        return Err(error);
    }
    readiness.spawned();
    if !readiness.ready() {
        return Err(RetainedFamilyError::Failed);
    }
    Ok(())
}
fn persistent_table(
    table: &OnceLock<Result<RetainedFamilies, RetainedFamilyError>>,
) -> Result<&RetainedFamilies, RetainedFamilyError> {
    table
        .get()
        .ok_or(RetainedFamilyError::Reservation)?
        .as_ref()
        .map_err(|error| *error)
}
fn reserve_slot(
    table: &OnceLock<Result<RetainedFamilies, RetainedFamilyError>>,
    readiness: &super::ServiceReadiness,
) -> Result<RetainedFamilySlot, RetainedFamilyError> {
    let table = persistent_table(table)?;
    if !readiness.ready() {
        return Err(RetainedFamilyError::Failed);
    }
    table.reserve()
}
fn register_slot<T: Send + Sync + 'static>(
    table: &OnceLock<Result<RetainedFamilies, RetainedFamilyError>>,
    readiness: &super::ServiceReadiness,
    slot: &RetainedFamilySlot,
    source: &mut Option<T>,
    max_job_body_bytes: usize,
    step: fn(&mut T) -> Result<RetainedFamilyProgress, RetainedFamilyError>,
) -> Result<RetainedFamilyId<T>, RetainedFamilyError> {
    let table = persistent_table(table)?;
    if !readiness.ready() {
        return Err(RetainedFamilyError::Failed);
    }
    table.register(slot, source, max_job_body_bytes, step)
}
pub fn reserve_retained_families_on_control(
    limits: RetainedFamilyReservation,
) -> Result<(), RetainedFamilyError> {
    configure_table(
        &TABLE,
        &super::SERVICE_READINESS,
        || RetainedFamilies::new(limits),
        || {
            super::service()
                .map(|_| ())
                .map_err(|_| RetainedFamilyError::Failed)
        },
    )
}
pub fn reserve_retained_family_slot_on_control() -> Result<RetainedFamilySlot, RetainedFamilyError>
{
    reserve_slot(&TABLE, &super::SERVICE_READINESS)
}
pub fn cancel_retained_family_slot_on_control(
    slot: &RetainedFamilySlot,
) -> Result<(), RetainedFamilyError> {
    // Cancellation remains operational without a worker: it contains no owner.
    persistent_table(&TABLE)?.cancel(slot)
}
/// Neutral custody hook only. Desktop must bind the audited concrete Session
/// adapter and actual joint inventory. Arbitrary jobs remain storage-Unknown.
pub fn register_retained_family_on_control<T: Send + Sync + 'static>(
    slot: &RetainedFamilySlot,
    source: &mut Option<T>,
    max_job_body_bytes: usize,
    step: fn(&mut T) -> Result<RetainedFamilyProgress, RetainedFamilyError>,
) -> Result<RetainedFamilyId<T>, RetainedFamilyError> {
    register_slot(
        &TABLE,
        &super::SERVICE_READINESS,
        slot,
        source,
        max_job_body_bytes,
        step,
    )
}
/// Read callbacks hold only the individual family guard. The concrete Session
/// adapter must expose restricted queries, never public ViewState/child owners.
///
/// ```compile_fail
/// use hex_audio::{RetainedFamilyId, read_retained_family};
/// fn escape(id: RetainedFamilyId<String>) {
///     let _escaped = read_retained_family(id, |value| value.as_str());
/// }
/// ```
pub fn read_retained_family<T: Send + Sync + 'static, R>(
    id: RetainedFamilyId<T>,
    read: impl for<'a> FnOnce(&'a T) -> R,
) -> Result<R, RetainedFamilyError> {
    persistent_table(&TABLE)?.read(id, read)
}
/// Actual accounting access also admits a failed-step holder; it does not
/// reopen UI reads or claim a consumed final-Drop object is still inspectable.
/// The concrete adapter must visit the payload, and hold this guard through
/// publication preflight. This single-family operation is not a whole union.
/// ```compile_fail
/// use hex_audio::{RetainedFamilyId, inspect_retained_family_on_control};
/// fn escape(id: RetainedFamilyId<u32>) {
///     let _escaped = inspect_retained_family_on_control(id, |job, _| job);
/// }
/// ```
pub fn inspect_retained_family_on_control<T: Send + Sync + 'static, R>(
    id: RetainedFamilyId<T>,
    inspect: impl for<'a> FnOnce(&'a hex_engine::RetainedControlJob, &'a T) -> R,
) -> Result<R, RetainedFamilyError> {
    persistent_table(&TABLE)?.inspect_on_control(id, inspect)
}
/// Unresolved constructor BODY reservation after consumed final-Drop failure.
/// Not physical freed/retained bytes or a substitute for payload/peak charges.
/// This reservation cannot be released or used to mark the family known.
pub fn unresolved_retained_family_body_reservation_on_control<T>(
    id: RetainedFamilyId<T>,
) -> Result<Option<usize>, RetainedFamilyError> {
    persistent_table(&TABLE)?.unresolved_body_reservation(id)
}
/// Durable final read acknowledgment through the already registered family.
/// Bind only the concrete bounded bookkeeping adapter, never a render callback.
/// Its live read prevents an owning cleanup step; no owning payload is returned.
pub fn acknowledge_retained_family<T: Send + Sync + 'static>(
    id: RetainedFamilyId<T>,
    acknowledge: impl FnOnce(&T) -> Result<(), RetainedFamilyError>,
) -> Result<(), RetainedFamilyError> {
    persistent_table(&TABLE)?.acknowledge(id, acknowledge)
}
/// Actual service Vec backing only. Enclosing/static storage and every live job
/// body/payload/panic owner need their separate typed traversal. This produces
/// no storage-completeness witness and never infers bytes from a caller quote.
pub fn visit_retained_family_table_backing_on_control<'a>(
    ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
) -> Result<(), hex_engine::AudioStorageError> {
    let table =
        persistent_table(&TABLE).map_err(|_| hex_engine::AudioStorageError::UnknownStorage)?;
    ledger.borrow_metadata_vec_backing_on_control(&table.slots)
}
pub(super) fn advance_registered_family_on_control() {
    if let Ok(table) = persistent_table(&TABLE) {
        table.advance_one();
    }
}

#[cfg(test)]
mod tests;
