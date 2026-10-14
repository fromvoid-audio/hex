//! Control-only physical lease custody; not graph retirement/admission authority.
use super::*;
#[path = "catalog_custody/coverage.rs"]
mod coverage;
pub use coverage::{
    CatalogCoverageAck, CatalogCoverageBinding, CatalogCoverageCovered, CatalogCoveragePending,
    CatalogCoverageProgress, CatalogCoverageRefusal, CatalogCoverageReservation,
    CatalogCoverageStorageReport, VerifiedCoverageRefusal,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogCustodyError {
    Allocation,
    Capacity,
    Overflow,
    Busy,
    WrongPair,
    Stale,
    Exhausted,
    Incomplete,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatalogCustodyCapacity {
    pub records: usize,
    pub leases: usize,
    pub bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordState {
    Empty,
    Reserved,
    Committed,
    CoverageReserved,
    CoverageSourceReserved,
    CoverageHeld,
    RedundantPending,
    DisposalPending,
    AckReady,
    AckIssued,
    Failed,
}
struct Record {
    nonce: u64,
    state: RecordState,
    lease_cell: usize,
    target: Option<(usize, u64)>,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
    #[cfg(test)]
    fail_disposal: bool,
    #[cfg(test)]
    disposal_attempts: usize,
}
#[derive(Default)]
pub(super) struct CatalogCustody {
    records: Vec<Record>,
    leases: Vec<Option<CatalogLease>>,
    coverage: Option<(usize, u64)>,
    next_nonce: u64,
    configured: bool,
}
impl CatalogCustody {
    pub(super) fn configured(&self) -> bool {
        self.configured
    }
    pub(super) fn outstanding(&self) -> bool {
        self.records.iter().any(|r| r.state != RecordState::Empty)
    }
    pub(super) fn reserved(&self) -> bool {
        self.records.iter().any(|r| {
            matches!(
                r.state,
                RecordState::Reserved
                    | RecordState::CoverageReserved
                    | RecordState::CoverageSourceReserved
                    | RecordState::RedundantPending
                    | RecordState::DisposalPending
                    | RecordState::AckReady
                    | RecordState::AckIssued
            )
        })
    }
    pub(super) fn failed(&self) -> bool {
        self.records.iter().any(|r| r.state == RecordState::Failed)
    }
    pub(super) fn lease_count(&self) -> usize {
        self.leases.iter().filter(|lease| lease.is_some()).count()
    }
    pub(super) fn matches_catalog(&self, catalog: &CatalogLease) -> bool {
        self.leases
            .iter()
            .flatten()
            .all(|lease| lease.same_verified_catalog(catalog))
    }
    pub(super) fn release_leases(&mut self) {
        for lease in &mut self.leases {
            drop(lease.take());
        }
    }
}
/// Nonclone control handle. Dropping it leaves its reservation occupied.
pub struct CatalogCustodyReservation {
    seal: Arc<()>,
    index: usize,
    incarnation: u64,
    record: usize,
    nonce: u64,
}
/// Awaiting actual successful shutdown; no caller can mint cleanup proof.
pub struct CatalogCustodyAwaiter(CatalogCustodyReservation);
pub struct CatalogCustodyReceipt {
    reservation: CatalogCustodyReservation,
    report: ReportedShutdown,
}
impl CatalogCustodyReceipt {
    pub fn report(&self) -> ReportedShutdown {
        self.report
    }
    /// Borrowed actual reservation identity, not a public scalar constructor.
    pub fn same_reservation(&self, other: &CatalogCustodyReceipt) -> bool {
        Arc::ptr_eq(&self.reservation.seal, &other.reservation.seal)
            && self.reservation.index == other.reservation.index
            && self.reservation.incarnation == other.reservation.incarnation
            && self.reservation.record == other.reservation.record
            && self.reservation.nonce == other.reservation.nonce
    }
}
pub struct CatalogCustodyRefusal {
    pub reason: CatalogCustodyError,
    pub reservation: CatalogCustodyReservation,
    pub lease: CatalogLease,
}
pub struct VerifiedCatalogCustodyRefusal {
    pub reason: CatalogCustodyError,
    pub reservation: CatalogCustodyReservation,
    pub verified: crate::VerifiedOwnedStreamRevision,
}
impl MediaDomainReaper {
    /// Opt-in caller reservation; legacy constructors have zero late-lease cells.
    /// Actual backing capacities are charged before replacement. Box/fatal OOM
    /// and allocator bookkeeping are outside this data-capacity contract.
    pub fn configure_catalog_custody(
        &mut self,
        id: DomainId,
        capacity: usize,
        max_data_bytes: usize,
    ) -> Result<CatalogCustodyCapacity, CatalogCustodyError> {
        let slot = self
            .slots
            .iter_mut()
            .find(|s| !s.vacant && s.id == id)
            .ok_or(CatalogCustodyError::Stale)?;
        if slot.custody.configured || slot.report.is_some() || slot.terminal_error.is_some() {
            return Err(CatalogCustodyError::Busy);
        }
        let mut records = Vec::new();
        let mut leases = Vec::new();
        records
            .try_reserve_exact(capacity)
            .map_err(|_| CatalogCustodyError::Allocation)?;
        leases
            .try_reserve_exact(capacity)
            .map_err(|_| CatalogCustodyError::Allocation)?;
        let bytes = records
            .capacity()
            .checked_mul(std::mem::size_of::<Record>())
            .and_then(|n| {
                leases
                    .capacity()
                    .checked_mul(std::mem::size_of::<Option<CatalogLease>>())
                    .and_then(|b| n.checked_add(b))
            })
            .ok_or(CatalogCustodyError::Overflow)?;
        if bytes > max_data_bytes {
            return Err(CatalogCustodyError::Capacity);
        }
        let domain_bytes = slot
            .data_bytes
            .checked_add(bytes)
            .ok_or(CatalogCustodyError::Overflow)?;
        let total = self
            .footprint
            .domain_data_bytes
            .and_then(|n| n.checked_add(bytes))
            .ok_or(CatalogCustodyError::Overflow)?;
        for lease_cell in 0..capacity {
            records.push(Record {
                nonce: 0,
                state: RecordState::Empty,
                lease_cell,
                target: None,
                first_panic: None,
                #[cfg(test)]
                fail_disposal: false,
                #[cfg(test)]
                disposal_attempts: 0,
            });
        }
        leases.resize_with(capacity, || None);
        let report = CatalogCustodyCapacity {
            records: records.capacity(),
            leases: leases.capacity(),
            bytes,
        };
        slot.custody = CatalogCustody {
            records,
            leases,
            next_nonce: slot.custody.next_nonce,
            configured: true,
            coverage: None,
        };
        slot.data_bytes = domain_bytes;
        self.footprint.domain_data_bytes = Some(total);
        Ok(report)
    }
    /// Explicitly release unused opt-in backing storage before legacy rollback.
    /// Outstanding/abandoned handles refuse; their storage is never silently freed.
    pub fn release_catalog_custody_capacity(
        &mut self,
        id: DomainId,
    ) -> Result<(), CatalogCustodyError> {
        let slot = self
            .slots
            .iter_mut()
            .find(|s| !s.vacant && s.id == id)
            .ok_or(CatalogCustodyError::Stale)?;
        if slot.custody.outstanding() || slot.custody.lease_count() != 0 {
            return Err(CatalogCustodyError::Busy);
        }
        let bytes = slot
            .custody
            .records
            .capacity()
            .checked_mul(std::mem::size_of::<Record>())
            .and_then(|n| {
                slot.custody
                    .leases
                    .capacity()
                    .checked_mul(std::mem::size_of::<Option<CatalogLease>>())
                    .and_then(|b| n.checked_add(b))
            })
            .ok_or(CatalogCustodyError::Overflow)?;
        let domain_bytes = slot
            .data_bytes
            .checked_sub(bytes)
            .ok_or(CatalogCustodyError::Overflow)?;
        let total = self
            .footprint
            .domain_data_bytes
            .and_then(|n| n.checked_sub(bytes))
            .ok_or(CatalogCustodyError::Overflow)?;
        slot.custody = CatalogCustody {
            next_nonce: slot.custody.next_nonce,
            ..Default::default()
        }; // control-only, no graph/payload ownership
        slot.data_bytes = domain_bytes;
        self.footprint.domain_data_bytes = Some(total);
        Ok(())
    }
    pub fn reserve_verified_catalog_custody(
        &mut self,
        id: DomainId,
        verified: &crate::VerifiedOwnedStreamRevision,
    ) -> Result<CatalogCustodyReservation, CatalogCustodyError> {
        self.reserve_catalog_custody(id, verified.catalog_on_control())
    }
    pub fn reserve_catalog_custody(
        &mut self,
        id: DomainId,
        catalog: &CatalogLease,
    ) -> Result<CatalogCustodyReservation, CatalogCustodyError> {
        let index = self
            .slots
            .iter()
            .position(|s| !s.vacant && s.id == id)
            .ok_or(CatalogCustodyError::Stale)?;
        let slot = &mut self.slots[index];
        if slot.report.is_some() || slot.terminal_error.is_some() {
            return Err(CatalogCustodyError::Busy);
        }
        if !slot
            .control
            .as_ref()
            .is_some_and(|c| c.matches_catalog(catalog))
        {
            return Err(CatalogCustodyError::WrongPair);
        }
        let record = slot
            .custody
            .records
            .iter()
            .position(|r| r.state == RecordState::Empty)
            .ok_or(CatalogCustodyError::Capacity)?;
        let occupied = slot
            .custody
            .records
            .iter()
            .filter(|r| r.state != RecordState::Empty)
            .count();
        if occupied >= slot.custody.leases.capacity() {
            return Err(CatalogCustodyError::Capacity);
        }
        let nonce = slot
            .custody
            .next_nonce
            .checked_add(1)
            .ok_or(CatalogCustodyError::Exhausted)?;
        // Control clone of existing seal: no new allocation; no catalog clone.
        let seal = Arc::clone(&self.seal);
        slot.custody.next_nonce = nonce;
        slot.custody.records[record] = Record {
            nonce,
            state: RecordState::Reserved,
            lease_cell: record,
            target: None,
            first_panic: None,
            #[cfg(test)]
            fail_disposal: false,
            #[cfg(test)]
            disposal_attempts: 0,
        };
        Ok(CatalogCustodyReservation {
            seal,
            index,
            incarnation: slot.incarnation,
            record,
            nonce,
        })
    }
    fn custody_record(
        &self,
        r: &CatalogCustodyReservation,
        state: RecordState,
    ) -> Result<(), CatalogCustodyError> {
        if !Arc::ptr_eq(&self.seal, &r.seal) {
            return Err(CatalogCustodyError::Stale);
        }
        let slot = self.slots.get(r.index).ok_or(CatalogCustodyError::Stale)?;
        if slot.vacant
            || slot.incarnation != r.incarnation
            || !slot
                .custody
                .records
                .get(r.record)
                .is_some_and(|x| x.nonce == r.nonce && x.state == state)
        {
            return Err(CatalogCustodyError::Stale);
        }
        Ok(())
    }
    fn custody_commit_preflight(
        &self,
        r: &CatalogCustodyReservation,
        lease: &CatalogLease,
    ) -> Result<(), CatalogCustodyError> {
        self.custody_record(r, RecordState::Reserved)?;
        let slot = &self.slots[r.index];
        if slot.report.is_some() || slot.terminal_error.is_some() {
            return Err(CatalogCustodyError::Busy);
        }
        if !slot
            .control
            .as_ref()
            .is_some_and(|c| c.matches_catalog(lease))
        {
            return Err(CatalogCustodyError::WrongPair);
        }
        let cell = slot.custody.records[r.record].lease_cell;
        if !slot.custody.leases.get(cell).is_some_and(Option::is_none) {
            return Err(CatalogCustodyError::Capacity);
        }
        Ok(())
    }
    pub fn cancel_catalog_custody(
        &mut self,
        r: CatalogCustodyReservation,
    ) -> Result<(), (CatalogCustodyError, CatalogCustodyReservation)> {
        if let Err(e) = self.custody_record(&r, RecordState::Reserved) {
            return Err((e, r));
        }
        self.slots[r.index].custody.records[r.record].state = RecordState::Empty;
        Ok(()) // control-only seal disposal
    }
    /// Physical catalog transfer only: NOT proof that a graph is retired.
    pub fn commit_catalog_custody(
        &mut self,
        r: CatalogCustodyReservation,
        lease: CatalogLease,
    ) -> Result<CatalogCustodyAwaiter, Box<CatalogCustodyRefusal>> {
        if let Err(reason) = self.custody_commit_preflight(&r, &lease) {
            return Err(Box::new(CatalogCustodyRefusal {
                reason,
                reservation: r,
                lease,
            }));
        }
        let slot = &mut self.slots[r.index];
        let cell = slot.custody.records[r.record].lease_cell;
        slot.custody.leases[cell] = Some(lease); // validated empty, no allocation/drop
        slot.custody.records[r.record].state = RecordState::Committed;
        Ok(CatalogCustodyAwaiter(r))
    }
    pub fn commit_verified_catalog_custody(
        &mut self,
        r: CatalogCustodyReservation,
        verified: crate::VerifiedOwnedStreamRevision,
    ) -> Result<
        (hex_engine::OwnedPreparedRevision, CatalogCustodyAwaiter),
        Box<VerifiedCatalogCustodyRefusal>,
    > {
        if let Err(reason) = self.custody_commit_preflight(&r, verified.catalog_on_control()) {
            return Err(Box::new(VerifiedCatalogCustodyRefusal {
                reason,
                reservation: r,
                verified,
            }));
        }
        let (prepared, lease) = verified.into_parts();
        let slot = &mut self.slots[r.index];
        let cell = slot.custody.records[r.record].lease_cell;
        slot.custody.leases[cell] = Some(lease);
        slot.custody.records[r.record].state = RecordState::Committed;
        Ok((prepared, CatalogCustodyAwaiter(r)))
    }
    /// Actual accounted report only. A live receipt blocks domain-slot reuse.
    pub fn catalog_cleanup_receipt(
        &self,
        waiter: CatalogCustodyAwaiter,
    ) -> Result<CatalogCustodyReceipt, (CatalogCustodyError, CatalogCustodyAwaiter)> {
        if let Err(e) = self.custody_record(&waiter.0, RecordState::Committed) {
            return Err((e, waiter));
        }
        let Some(report) = self.slots[waiter.0.index]
            .report
            .filter(|r| r.successful_report().is_some())
        else {
            return Err((CatalogCustodyError::Incomplete, waiter));
        };
        Ok(CatalogCustodyReceipt {
            reservation: waiter.0,
            report,
        })
    }
    /// Control must retain/copy actual report before consuming sealed proof.
    pub fn consume_catalog_cleanup_receipt(
        &mut self,
        receipt: CatalogCustodyReceipt,
    ) -> Result<ReportedShutdown, (CatalogCustodyError, CatalogCustodyReceipt)> {
        if let Err(e) = self.custody_record(&receipt.reservation, RecordState::Committed) {
            return Err((e, receipt));
        }
        let r = &receipt.reservation;
        if self.slots[r.index].report != Some(receipt.report)
            || receipt.report.successful_report().is_none()
        {
            return Err((CatalogCustodyError::Incomplete, receipt));
        }
        self.slots[r.index].custody.records[r.record].state = RecordState::Empty;
        if self.slots[r.index].custody.coverage == Some((r.record, r.nonce)) {
            self.slots[r.index].custody.coverage = None;
        }
        self.update_phase();
        Ok(receipt.report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn input(id: u64) -> MediaDomainInput {
        let mut runtime =
            crate::StreamRuntime::new(crate::StreamConfig::with_pool_pages(4), Vec::new()).unwrap();
        runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
        let (control, pages) = runtime.split().ok().unwrap();
        let endpoint =
            crate::VerifiedPageEndpoint::new(pages, control.catalog_lease().unwrap()).unwrap();
        MediaDomainInput {
            id: DomainId::new(id).unwrap(),
            endpoint: PageReadiness::new(endpoint, 3).unwrap(),
            retained_leases: Vec::new(),
            control,
        }
    }
    fn setup() -> (
        Box<CallbackDomainBatch>,
        TerminalBatchSender,
        MediaDomainReaper,
        DomainAdmission,
    ) {
        let (batch, terminal, mut reaper) = reusable_domain_escrow(2).unwrap();
        let admission = reaper.reserve_domain(input(1)).ok().unwrap();
        (batch, terminal, reaper, admission)
    }
    fn end(
        batch: Box<CallbackDomainBatch>,
        mut terminal: TerminalBatchSender,
        reaper: &mut MediaDomainReaper,
    ) {
        terminal.try_return_stopped_batch(batch).unwrap();
        while reaper.poll_one().unwrap() != DomainPoll::Empty {}
    }
    #[test]
    fn preflighted_physical_commit_allocates_frees_and_clones_no_catalog_owner() {
        let (mut batch, terminal, mut r, admission) = setup();
        let id = admission.id();
        r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
        let mut lease = Some(r.catalog_lease(id).unwrap());
        let references = r.slots[0].control.as_ref().unwrap().owner_references();
        let mut token = Some(
            r.reserve_catalog_custody(id, lease.as_ref().unwrap())
                .unwrap(),
        );
        let mut waiter = None;
        let info = allocation_counter::measure(|| {
            waiter = Some(
                r.commit_catalog_custody(token.take().unwrap(), lease.take().unwrap())
                    .ok()
                    .unwrap(),
            );
        });
        assert_eq!(info.count_total, 0);
        assert_eq!(info.bytes_total, 0);
        assert_eq!(info.count_current, 0);
        assert_eq!(info.bytes_current, 0);
        assert_eq!(
            r.slots[0].control.as_ref().unwrap().owner_references(),
            references
        );
        let mut positive = None;
        let probe = allocation_counter::measure(|| {
            positive = Some(vec![7_u8; 128]);
            std::hint::black_box(&positive);
        });
        assert!(probe.count_total > 0 && probe.bytes_total >= 128);
        drop(positive);
        batch.install_domain(admission).ok().unwrap();
        end(batch, terminal, &mut r);
        assert!(matches!(
            r.try_reap_domain(id),
            DomainCleanupProgress::Finished(_)
        ));
        let receipt = r.catalog_cleanup_receipt(waiter.unwrap()).ok().unwrap();
        r.consume_catalog_cleanup_receipt(receipt).ok().unwrap();
        assert!(r.take_reclaimed_report(id).is_some());
    }
    #[test]
    fn actual_lease_transfer_cleanup_proof_and_capacity_report() {
        let (mut batch, terminal, mut r, admission) = setup();
        let id = admission.id();
        let base = r.allocation_footprint().domain_data_bytes.unwrap();
        let capacity = r.configure_catalog_custody(id, 2, usize::MAX).unwrap();
        assert!(capacity.records >= 2 && capacity.leases >= 2);
        assert_eq!(
            r.allocation_footprint().domain_data_bytes,
            Some(base + capacity.bytes)
        );
        let lease = r.catalog_lease(id).unwrap();
        let control = r.slots[0].control.as_ref().unwrap();
        let references = control.owner_references();
        let token = r.reserve_catalog_custody(id, &lease).unwrap();
        assert_eq!(
            r.slots[0].control.as_ref().unwrap().owner_references(),
            references
        );
        let waiter = r.commit_catalog_custody(token, lease).ok().unwrap();
        assert_eq!(
            r.slots[0].control.as_ref().unwrap().owner_references(),
            references
        );
        let waiter = r.catalog_cleanup_receipt(waiter).err().unwrap().1;
        assert!(r.take_reclaimed_report(id).is_none());
        let external = r.catalog_lease(id).unwrap();
        batch.install_domain(admission).ok().unwrap();
        assert!(!r.domain_has_only_escrow_references(id));
        end(batch, terminal, &mut r);
        assert!(matches!(
            r.try_reap_domain(id),
            DomainCleanupProgress::Busy {
                external_references: 1
            }
        ));
        assert!(r.report(id).is_none());
        assert!(r.take_reclaimed_report(id).is_none());
        drop(external);
        let DomainCleanupProgress::Finished(report) = r.try_reap_domain(id) else {
            panic!("actual shutdown");
        };
        assert!(report.successful_report().unwrap().accounting_complete);
        assert_eq!(r.slots[0].custody.lease_count(), 0);
        assert_eq!(r.phase(), MediaCleanupPhase::Pending); // proof still anchored
        assert!(r.take_reclaimed_report(id).is_none());
        let receipt = r.catalog_cleanup_receipt(waiter).ok().unwrap();
        assert_eq!(receipt.report(), report);
        assert_eq!(
            r.consume_catalog_cleanup_receipt(receipt).ok(),
            Some(report)
        );
        assert_eq!(r.phase(), MediaCleanupPhase::Complete);
        assert_eq!(r.take_reclaimed_report(id), Some(report));
        assert_eq!(r.allocation_footprint().domain_data_bytes, Some(0));
    }
    #[test]
    fn reserve_cancel_pressure_rollback_barrier_and_abandoned_handle() {
        let (batch, terminal, mut r, admission) = setup();
        let id = admission.id();
        r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
        let lease = r.catalog_lease(id).unwrap();
        let token = r.reserve_catalog_custody(id, &lease).unwrap();
        assert!(matches!(
            r.reserve_catalog_custody(id, &lease),
            Err(CatalogCustodyError::Capacity)
        ));
        let admission = r.rollback_domain(admission).err().unwrap();
        assert!(matches!(
            r.release_catalog_custody_capacity(id),
            Err(CatalogCustodyError::Busy)
        ));
        assert!(!r.domain_has_only_escrow_references(id));
        r.cancel_catalog_custody(token).ok().unwrap();
        r.release_catalog_custody_capacity(id).unwrap();
        let original = r.rollback_domain(admission).ok().unwrap();
        drop(lease);
        let (pages, lease) = original.endpoint.into_endpoint().into_parts();
        drop(lease);
        assert!(
            original
                .control
                .shutdown(pages)
                .unwrap()
                .accounting_complete
        );
        end(batch, terminal, &mut r);
        assert_eq!(r.phase(), MediaCleanupPhase::Complete);
        let (mut batch, terminal, mut r, admission) = setup();
        let id = admission.id();
        r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
        let lease = r.catalog_lease(id).unwrap();
        drop(r.reserve_catalog_custody(id, &lease).unwrap()); // abandoned token does not cancel
        drop(lease);
        batch.install_domain(admission).ok().unwrap();
        end(batch, terminal, &mut r);
        assert_eq!(
            r.try_reap_domain(id),
            DomainCleanupProgress::WaitingForReturn
        );
        assert_eq!(r.phase(), MediaCleanupPhase::Pending);
        assert!(r.take_reclaimed_report(id).is_none());
        // Test owns durable root until teardown; production lifecycle must retain it.
    }
    #[test]
    fn real_wrong_catalog_and_root_refusal_return_exact_lease_and_token() {
        let (mut batch, terminal, mut r, admission) = setup();
        let id = admission.id();
        let other = input(2);
        let other_lease = other.control.catalog_lease().unwrap();
        r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
        assert!(matches!(
            r.reserve_catalog_custody(id, &other_lease),
            Err(CatalogCustodyError::WrongPair)
        ));
        let lease = r.catalog_lease(id).unwrap();
        let token = r.reserve_catalog_custody(id, &lease).unwrap();
        let original_nonce = token.nonce;
        let original_inc = token.incarnation;
        let refused = r.commit_catalog_custody(token, other_lease).err().unwrap();
        assert_eq!(refused.reason, CatalogCustodyError::WrongPair);
        assert_eq!(refused.reservation.nonce, original_nonce);
        assert_eq!(refused.reservation.incarnation, original_inc);
        assert!(other.control.matches_catalog(&refused.lease));
        drop(refused.lease);
        let (mut different_batch, different_terminal, mut different, different_admission) = setup();
        let token = different
            .cancel_catalog_custody(refused.reservation)
            .err()
            .unwrap()
            .1;
        r.cancel_catalog_custody(token).ok().unwrap();
        different_batch
            .install_domain(different_admission)
            .ok()
            .unwrap();
        end(different_batch, different_terminal, &mut different);
        assert!(matches!(
            different.try_reap_domain(DomainId::new(1).unwrap()),
            DomainCleanupProgress::Finished(_)
        ));
        drop(lease);
        let (pages, catalog) = other.endpoint.into_endpoint().into_parts();
        drop(catalog);
        assert!(other.control.shutdown(pages).unwrap().accounting_complete);
        batch.install_domain(admission).ok().unwrap();
        end(batch, terminal, &mut r);
        assert!(matches!(
            r.try_reap_domain(id),
            DomainCleanupProgress::Finished(_)
        ));
    }
    #[test]
    fn stale_incarnation_nonce_and_checked_exhaustion_never_mutate() {
        let (mut batch, terminal, mut r, admission) = setup();
        let id = admission.id();
        r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
        let lease = r.catalog_lease(id).unwrap();
        let mut token = r.reserve_catalog_custody(id, &lease).unwrap();
        let nonce = token.nonce;
        token.incarnation += 1;
        let mut token = r.cancel_catalog_custody(token).err().unwrap().1;
        assert_eq!(r.slots[0].custody.records[0].nonce, nonce);
        token.incarnation -= 1;
        token.nonce += 1;
        let mut token = r.cancel_catalog_custody(token).err().unwrap().1;
        token.nonce -= 1;
        r.cancel_catalog_custody(token).ok().unwrap();
        r.slots[0].custody.next_nonce = u64::MAX;
        assert!(matches!(
            r.reserve_catalog_custody(id, &lease),
            Err(CatalogCustodyError::Exhausted)
        ));
        drop(lease);
        batch.install_domain(admission).ok().unwrap();
        end(batch, terminal, &mut r);
        assert!(matches!(
            r.try_reap_domain(id),
            DomainCleanupProgress::Finished(_)
        ));
    }
    fn verified(lease: CatalogLease) -> crate::VerifiedOwnedStreamRevision {
        let project = hex_project::Project::new(48_000).unwrap();
        let prepared = hex_engine::PreparedRevision::prepare(
            &project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap()
        .into_owned_shared();
        crate::VerifiedOwnedStreamRevision::new(prepared, lease).unwrap()
    }
    #[test]
    fn typed_verifier_refusal_preserves_actual_root_and_success_moves_only_lease() {
        let (mut batch, terminal, mut r, admission) = setup();
        let id = admission.id();
        r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
        let lease = r.catalog_lease(id).unwrap();
        let token = r.reserve_catalog_custody(id, &lease).unwrap();
        drop(lease);
        let other = input(2);
        let wrong = verified(other.control.catalog_lease().unwrap());
        let refused = r
            .commit_verified_catalog_custody(token, wrong)
            .err()
            .unwrap();
        assert_eq!(refused.reason, CatalogCustodyError::WrongPair);
        let (root, wronglease) = refused.verified.into_parts();
        assert!(other.control.matches_catalog(&wronglease));
        drop((root, wronglease));
        let source = verified(r.catalog_lease(id).unwrap());
        let (prepared, waiter) = r
            .commit_verified_catalog_custody(refused.reservation, source)
            .ok()
            .unwrap();
        assert_eq!(prepared.metadata().sample_rate(), 48_000);
        batch.install_domain(admission).ok().unwrap();
        end(batch, terminal, &mut r);
        assert!(matches!(
            r.try_reap_domain(id),
            DomainCleanupProgress::Finished(_)
        ));
        // Graph owner remains live after catalog cleanup; proof remains anchored.
        assert_eq!(prepared.metadata().sample_rate(), 48_000);
        let receipt = r.catalog_cleanup_receipt(waiter).ok().unwrap();
        r.consume_catalog_cleanup_receipt(receipt).ok().unwrap();
        assert!(r.take_reclaimed_report(id).is_some());
        drop(prepared);
        let (pages, lease) = other.endpoint.into_endpoint().into_parts();
        drop(lease);
        assert!(other.control.shutdown(pages).unwrap().accounting_complete);
    }
    #[test]
    fn actual_domain_reuse_preserves_incarnation_and_stale_cleanup_cannot_free_replacement() {
        let (mut batch, terminal, mut r, admission) = setup();
        let id = admission.id();
        r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
        let lease = r.catalog_lease(id).unwrap();
        let token = r.reserve_catalog_custody(id, &lease).unwrap();
        // Private negative-control copy only; public handle is nonclone.
        let stale = CatalogCustodyReservation {
            seal: Arc::clone(&token.seal),
            index: token.index,
            incarnation: token.incarnation,
            record: token.record,
            nonce: token.nonce,
        };
        let waiter = r.commit_catalog_custody(token, lease).ok().unwrap();
        batch.install_domain(admission).ok().unwrap();
        assert_eq!(batch.try_return_domain(id), DomainReturnStatus::Accepted);
        assert_eq!(r.poll_one().unwrap(), DomainPoll::DomainReturned(id));
        assert!(matches!(
            r.try_reap_domain(id),
            DomainCleanupProgress::Finished(_)
        ));
        let receipt = r.catalog_cleanup_receipt(waiter).ok().unwrap();
        r.consume_catalog_cleanup_receipt(receipt).ok().unwrap();
        r.take_reclaimed_report(id).unwrap();
        let next = r.reserve_domain(input(1)).ok().unwrap();
        r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
        let lease = r.catalog_lease(id).unwrap();
        let token = r.reserve_catalog_custody(id, &lease).unwrap();
        assert_ne!(token.incarnation, stale.incarnation);
        assert!(matches!(
            r.cancel_catalog_custody(stale),
            Err((CatalogCustodyError::Stale, _))
        ));
        assert!(r.slots[token.index].custody.reserved());
        r.cancel_catalog_custody(token).ok().unwrap();
        drop(lease);
        batch.install_domain(next).ok().unwrap();
        end(batch, terminal, &mut r);
        assert!(matches!(
            r.try_reap_domain(id),
            DomainCleanupProgress::Finished(_)
        ));
    }
    #[test]
    fn actual_panicked_or_imbalanced_shutdown_never_mints_cleanup_receipt() {
        for panic_worker in [false, true] {
            let source = super::super::tests::damaged_input(panic_worker);
            let id = source.id;
            let lease = source.control.catalog_lease().unwrap();
            let (batch, terminal, mut r) = crate::domain_escrow(vec![source], 1).unwrap();
            r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
            let token = r.reserve_catalog_custody(id, &lease).unwrap();
            let waiter = r.commit_catalog_custody(token, lease).ok().unwrap();
            end(batch, terminal, &mut r);
            let DomainCleanupProgress::Finished(report) = r.try_reap_domain(id) else {
                panic!("actual damaged shutdown report");
            };
            assert!(report.successful_report().is_none());
            assert_eq!(r.phase(), MediaCleanupPhase::FailedIncomplete);
            assert!(matches!(
                r.catalog_cleanup_receipt(waiter),
                Err((CatalogCustodyError::Incomplete, _))
            ));
            assert!(r.take_reclaimed_report(id).is_none());
            assert!(r.slots[0].custody.outstanding());
            assert_eq!(r.report(id), Some(report));
        }
    }
    #[test]
    fn release_reconfigure_preserves_nonce_history_and_domain_exhaustion_recovers_inputs() {
        let (mut batch, terminal, mut r, admission) = setup();
        let id = admission.id();
        let lease = r.catalog_lease(id).unwrap();
        r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
        let token = r.reserve_catalog_custody(id, &lease).unwrap();
        let nonce = token.nonce;
        r.cancel_catalog_custody(token).ok().unwrap();
        r.release_catalog_custody_capacity(id).unwrap();
        r.configure_catalog_custody(id, 1, usize::MAX).unwrap();
        let token = r.reserve_catalog_custody(id, &lease).unwrap();
        assert!(token.nonce > nonce);
        r.cancel_catalog_custody(token).ok().unwrap();
        drop(lease);
        batch.install_domain(admission).ok().unwrap();
        assert_eq!(batch.try_return_domain(id), DomainReturnStatus::Accepted);
        r.poll_one().unwrap();
        assert!(matches!(
            r.try_reap_domain(id),
            DomainCleanupProgress::Finished(_)
        ));
        r.take_reclaimed_report(id).unwrap();
        r.slots[0].incarnation = u64::MAX;
        let mut original = input(1);
        let mut spare = Vec::with_capacity(8);
        spare.push(original.control.catalog_lease().unwrap());
        original.retained_leases = spare;
        let pointer = original.retained_leases.as_ptr();
        let capacity = original.retained_leases.capacity();
        let refused = r.reserve_domain(original).err().unwrap();
        assert_eq!(refused.reason, DomainRefusal::IdentityExhausted);
        assert_eq!(refused.input.retained_leases.as_ptr(), pointer);
        assert_eq!(refused.input.retained_leases.capacity(), capacity);
        let source = refused.input;
        let (pages, lease) = source.endpoint.into_endpoint().into_parts();
        drop((lease, source.retained_leases));
        assert!(source.control.shutdown(pages).unwrap().accounting_complete);
        end(batch, terminal, &mut r);
        assert_eq!(r.phase(), MediaCleanupPhase::Complete);
    }
    #[test]
    fn configuration_refusal_preserves_domain_and_actual_original_storage() {
        let (mut batch, terminal, mut r, admission) = setup();
        let id = admission.id();
        let pointer = r.slots[0].retained_leases.as_ptr();
        let footprint = r.allocation_footprint();
        assert_eq!(
            r.configure_catalog_custody(id, 2, 0),
            Err(CatalogCustodyError::Capacity)
        );
        assert_eq!(
            r.configure_catalog_custody(id, usize::MAX, usize::MAX),
            Err(CatalogCustodyError::Allocation)
        );
        assert_eq!(r.slots[0].retained_leases.as_ptr(), pointer);
        assert_eq!(r.allocation_footprint(), footprint);
        batch.install_domain(admission).ok().unwrap();
        end(batch, terminal, &mut r);
        assert!(matches!(
            r.try_reap_domain(id),
            DomainCleanupProgress::Finished(_)
        ));
    }
}
