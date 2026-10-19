//! Purpose-specific native compiled records for the existing Session family.
//! No queue, registry, lock, raw Arc adoption or admission flag is created here.
//! These methods require exclusive record access supplied by the family; the existing
//! SessionExportFamily record lock supplies structural exclusion.
use super::ExportRecordError as Error;
use hex_engine::{AudioCompiledRevision, AudioRootStorageLedger, AudioStorageError};

// Current and constructing compiled references. Export/seek reader fields live
// in their own concrete work records. Exhaustion retains the producer payload.
const COMPILED_RECORDS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Vacant,
    Reserved,
    Queued,
    Claimed,
    Completed,
    Retiring,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompiledStamp {
    family: u64,
    index: usize,
    generation: u64,
}

struct CompiledRecord {
    generation: u64,
    phase: Phase,
    payload: Option<AudioCompiledRevision>,
    cleanup_pending: bool,
}
impl Default for CompiledRecord {
    fn default() -> Self {
        Self {
            generation: 0,
            phase: Phase::Vacant,
            payload: None,
            cleanup_pending: false,
        }
    }
}

pub(super) struct NativeCompiledRecords {
    family: u64,
    closed: bool,
    records: [CompiledRecord; COMPILED_RECORDS],
}
impl NativeCompiledRecords {
    // Inline record bodies have no backing allocation or hidden OS primitive.
    // The enclosing registered-family constructor must charge this actual size.
    pub(super) const fn inline_bytes() -> usize {
        std::mem::size_of::<Self>()
    }

    pub(super) fn new(family: u64) -> Self {
        Self {
            family,
            closed: false,
            records: std::array::from_fn(|_| CompiledRecord::default()),
        }
    }

    pub(super) fn reserve(&mut self) -> Result<CompiledStamp, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let (index, record) = self
            .records
            .iter_mut()
            .enumerate()
            .find(|(_, record)| record.phase == Phase::Vacant)
            .ok_or(Error::Reservation)?;
        if record.payload.is_some() || record.cleanup_pending {
            return Err(Error::Stale);
        }
        let generation = record.generation.checked_add(1).ok_or(Error::Exhausted)?;
        record.generation = generation;
        record.phase = Phase::Reserved;
        Ok(CompiledStamp {
            family: self.family,
            index,
            generation,
        })
    }

    fn record(&self, stamp: CompiledStamp) -> Result<&CompiledRecord, Error> {
        if stamp.family != self.family {
            return Err(Error::Stale);
        }
        self.records
            .get(stamp.index)
            .filter(|record| {
                record.generation == stamp.generation
                    && record.phase != Phase::Vacant
                    && record.phase != Phase::Failed
                    && !record.cleanup_pending
            })
            .ok_or(Error::Stale)
    }

    fn record_mut(&mut self, stamp: CompiledStamp) -> Result<&mut CompiledRecord, Error> {
        self.record(stamp)?;
        Ok(&mut self.records[stamp.index])
    }

    pub(super) fn install(
        &mut self,
        stamp: CompiledStamp,
        source: &mut Option<AudioCompiledRevision>,
    ) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let record = self.record_mut(stamp)?;
        if record.phase != Phase::Reserved || record.payload.is_some() || source.is_none() {
            return Err(Error::Stale);
        }
        // All validation precedes this fixed transfer. No fallible work, clone,
        // callback or allocation can separate payload and state publication.
        record.payload = source.take();
        record.phase = Phase::Queued;
        Ok(())
    }

    pub(super) fn claim(&mut self, stamp: CompiledStamp) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let record = self.record_mut(stamp)?;
        if record.phase != Phase::Queued || record.payload.is_none() {
            return Err(Error::Stale);
        }
        // A claim leaves the actual immutable owner in this inspected field.
        record.phase = Phase::Claimed;
        Ok(())
    }

    pub(super) fn borrowed(&self, stamp: CompiledStamp) -> Result<&AudioCompiledRevision, Error> {
        let record = self.record(stamp)?;
        if !matches!(
            record.phase,
            Phase::Queued | Phase::Claimed | Phase::Completed
        ) {
            return Err(Error::Stale);
        }
        record.payload.as_ref().ok_or(Error::Stale)
    }

    pub(super) fn complete(&mut self, stamp: CompiledStamp) -> Result<(), Error> {
        let record = self.record_mut(stamp)?;
        if record.phase != Phase::Claimed || record.payload.is_none() {
            return Err(Error::Stale);
        }
        record.phase = Phase::Completed;
        Ok(())
    }

    pub(super) fn retire(&mut self, stamp: CompiledStamp) -> Result<(), Error> {
        let record = self.record_mut(stamp)?;
        record.phase = Phase::Retiring;
        Ok(())
    }

    pub(super) fn close(&mut self) {
        self.closed = true;
        for record in &mut self.records {
            if matches!(
                record.phase,
                Phase::Reserved | Phase::Queued | Phase::Claimed | Phase::Completed
            ) {
                record.phase = Phase::Retiring;
            }
        }
    }

    // Fixed transfer to the enclosing family's actual persistent destination.
    // No capability Drop or other-family acquisition under the records writer.
    pub(super) fn begin_cleanup(
        &mut self,
        destination: &mut CompiledRetirement,
    ) -> Result<bool, Error> {
        if !destination.vacant() {
            return Err(Error::Stale);
        }
        let Some(index) = self.records.iter().position(|r| r.phase == Phase::Retiring) else {
            return Ok(false);
        };
        let record = &mut self.records[index];
        if record.cleanup_pending {
            return Err(Error::Stale);
        }
        let stamp = CompiledStamp {
            family: self.family,
            index,
            generation: record.generation,
        };
        record.cleanup_pending = true;
        destination.stamp = Some(stamp);
        destination.payload = record.payload.take();
        Ok(true)
    }
    pub(super) fn finish_cleanup(
        &mut self,
        destination: &mut CompiledRetirement,
    ) -> Result<(), Error> {
        if destination.cleanup_pending || destination.payload.is_some() {
            return Err(Error::Stale);
        }
        let stamp = destination.stamp.ok_or(Error::Stale)?;
        if stamp.family != self.family {
            return Err(Error::Stale);
        }
        let record = self.records.get_mut(stamp.index).ok_or(Error::Stale)?;
        if record.generation != stamp.generation
            || record.phase != Phase::Retiring
            || !record.cleanup_pending
            || record.payload.is_some()
        {
            return Err(Error::Stale);
        }
        record.phase = Phase::Vacant;
        record.cleanup_pending = false;
        destination.stamp = None;
        Ok(())
    }
    pub(super) fn disposed(&self) -> bool {
        self.records
            .iter()
            .all(|r| r.phase == Phase::Vacant && r.payload.is_none() && !r.cleanup_pending)
    }
    pub(super) fn duplicate(
        &mut self,
        source: CompiledStamp,
        destination: CompiledStamp,
    ) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let target = self.record(destination)?;
        if target.phase != Phase::Reserved || target.payload.is_some() {
            return Err(Error::Stale);
        }
        let source = self.borrowed(source)?;
        let reader = source.try_duplicate_on_control().map_err(|e| match e {
            AudioStorageError::ExternalReadersPending => Error::Busy,
            _ => Error::Stale,
        })?;
        // No fallible operation after the paired registration reservation.
        let target = &mut self.records[destination.index];
        target.payload = Some(reader);
        target.phase = Phase::Queued;
        Ok(())
    }
    #[cfg(test)]
    fn cleanup_one(&mut self) -> Result<bool, Error> {
        let mut destination = CompiledRetirement::default();
        if !self.begin_cleanup(&mut destination)? {
            return Ok(false);
        }
        destination.release();
        self.finish_cleanup(&mut destination)?;
        Ok(true)
    }

    /// The enclosing registered-family body must already be observed. Every
    /// paired handle is visited at its actual current physical field location.
    pub(super) fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        for record in &self.records {
            if record.cleanup_pending || record.phase == Phase::Failed {
                return Err(AudioStorageError::UnknownStorage);
            }
            match (record.phase, &record.payload) {
                (Phase::Vacant | Phase::Reserved, None) => {}
                (
                    Phase::Queued | Phase::Claimed | Phase::Completed | Phase::Retiring,
                    Some(reader),
                ) => reader.visit_shared_storage_on_control(ledger)?,
                (Phase::Retiring, None) => {} // cancelled reservation, no owner was constructed
                _ => return Err(AudioStorageError::UnknownStorage),
            }
        }
        Ok(())
    }
}

/// Inline persistent cleanup destination in the existing registered family.
/// The outer lifecycle custodian retains first panic and charge. Pending is
/// latched before taking the actual reader and never reset after unwind.
#[derive(Default)]
pub(super) struct CompiledRetirement {
    stamp: Option<CompiledStamp>,
    payload: Option<AudioCompiledRevision>,
    cleanup_pending: bool,
}
impl CompiledRetirement {
    pub(super) fn vacant(&self) -> bool {
        self.stamp.is_none() && self.payload.is_none() && !self.cleanup_pending
    }
    pub(super) fn release(&mut self) {
        self.cleanup_pending = true;
        drop(self.payload.take());
        #[cfg(test)]
        retirement_test_hook();
        self.cleanup_pending = false;
    }
    pub(super) fn visit<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        if self.cleanup_pending || (self.stamp.is_none() && self.payload.is_some()) {
            return Err(AudioStorageError::UnknownStorage);
        }
        if let Some(reader) = &self.payload {
            reader.visit_shared_storage_on_control(ledger)?;
        }
        Ok(())
    }
}

#[cfg(test)]
type RetirementHook = Box<dyn FnMut()>;
#[cfg(test)]
thread_local! { static RETIREMENT_HOOK: std::cell::RefCell<Option<RetirementHook>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
fn retirement_test_hook() {
    RETIREMENT_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook();
        }
    });
}
#[cfg(test)]
pub(super) mod tests;
