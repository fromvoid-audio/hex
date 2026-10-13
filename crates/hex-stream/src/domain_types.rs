//! Headless ownership identities, observations and reserved-data accounting.
use crate::{AllocationCapacity, CatalogLease, PageReadiness, ReportedShutdown, StreamControl};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DomainId(u64);
impl DomainId {
    pub fn new(id: u64) -> Result<Self, DomainRefusal> {
        if id == 0 {
            Err(DomainRefusal::InvalidIdentity)
        } else {
            Ok(Self(id))
        }
    }
    pub fn value(self) -> u64 {
        self.0
    }
}
#[derive(Default)]
pub struct DomainIds(u64);
impl DomainIds {
    pub fn advance(&mut self) -> Result<DomainId, DomainRefusal> {
        let next = self
            .0
            .checked_add(1)
            .ok_or(DomainRefusal::IdentityExhausted)?;
        self.0 = next;
        Ok(DomainId(next))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainRefusal {
    InvalidIdentity,
    IdentityExhausted,
    InvalidCapacity,
    Allocation,
    DuplicateIdentity,
    DuplicateCatalog,
    WrongPair,
    WrongLease,
    UnknownDomain,
    AlreadyReturned,
    QueueClosed,
    WrongEscrow,
    TerminalAlreadyReturned,
    UnexpectedShutdownRefusal,
}

/// Moves only. Setup borrows/preallocates before splitting actual owners.
/// Retained leases are physical control-owned references, not guessed graphs.
pub struct MediaDomainInput {
    pub id: DomainId,
    pub control: StreamControl,
    pub endpoint: PageReadiness,
    pub retained_leases: Vec<CatalogLease>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainReturnStatus {
    Accepted,
    Pending,
    Refused(DomainRefusal),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainPoll {
    Empty,
    DomainReturned(DomainId),
    TerminalReturned,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainCleanupProgress {
    WaitingForReturn,
    Busy { external_references: usize },
    Finished(ReportedShutdown),
    Refused(DomainRefusal),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCleanupPhase {
    Pending,
    Complete,
    FailedIncomplete,
}

/// Actual endpoint-independent escrow data capacities. Inline outcome/owner
/// metadata is included in control slot size. Shared queue objects and observer
/// byte are charged once; allocator/Arc headers and caller placement excluded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DomainEscrowFootprint {
    pub control_slots: AllocationCapacity,
    pub callback_slots: AllocationCapacity,
    pub normal_return_slots: AllocationCapacity,
    pub terminal_return_slots: AllocationCapacity,
    pub queue_object_bytes: usize,
    pub callback_batch_object_bytes: usize,
    pub observer_object_bytes: usize,
    /// Original input Vec overlaps output storage during construction only.
    pub construction_input_slots: AllocationCapacity,
    pub domain_data_bytes: Option<usize>,
}
impl DomainEscrowFootprint {
    pub fn retained_data_bytes(self) -> Option<usize> {
        [
            self.control_slots.bytes()?,
            self.callback_slots.bytes()?,
            self.normal_return_slots.bytes()?,
            self.terminal_return_slots.bytes()?,
            self.queue_object_bytes,
            self.callback_batch_object_bytes,
            self.observer_object_bytes,
            self.domain_data_bytes?,
        ]
        .into_iter()
        .try_fold(0_usize, |sum, value| sum.checked_add(value))
    }
    pub fn construction_data_bound(self) -> Option<usize> {
        self.retained_data_bytes()?
            .checked_add(self.construction_input_slots.bytes()?)
    }
}

pub struct DomainSetupFailure {
    pub(crate) inputs: Vec<MediaDomainInput>,
    pub(crate) reason: DomainRefusal,
    pub(crate) observed: DomainEscrowFootprint,
}
impl std::fmt::Debug for DomainSetupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DomainSetupFailure")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
impl DomainSetupFailure {
    pub fn into_parts(self) -> (Vec<MediaDomainInput>, DomainRefusal, DomainEscrowFootprint) {
        (self.inputs, self.reason, self.observed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_and_capacity_arithmetic_never_wrap() {
        assert_eq!(DomainId::new(0), Err(DomainRefusal::InvalidIdentity));
        let mut ids = DomainIds(u64::MAX - 1);
        assert_eq!(ids.advance().unwrap().value(), u64::MAX);
        assert_eq!(ids.advance(), Err(DomainRefusal::IdentityExhausted));
        assert_eq!(ids.advance(), Err(DomainRefusal::IdentityExhausted));
        let mut footprint = DomainEscrowFootprint {
            domain_data_bytes: Some(usize::MAX),
            ..Default::default()
        };
        footprint.observer_object_bytes = 1;
        assert_eq!(footprint.retained_data_bytes(), None);
        footprint.observer_object_bytes = 0;
        footprint.construction_input_slots = AllocationCapacity {
            capacity: 1,
            element_bytes: 1,
        };
        assert_eq!(footprint.construction_data_bound(), None);
    }
}
