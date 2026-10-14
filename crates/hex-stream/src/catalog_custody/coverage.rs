//! Stable domain coverage, not graph/runtime/role/admission authority.
use super::*;
#[cfg(test)]
#[path = "coverage/tests.rs"]
mod tests;
struct Key {
    seal: Arc<()>,
    domain: usize,
    incarnation: u64,
    source: usize,
    source_nonce: u64,
    target: usize,
    target_nonce: u64,
    created: bool,
}
/// Dropping any handle preserves occupied source/destination records.
pub struct CatalogCoverageReservation(Key);
pub struct CatalogCoveragePending(Key);
pub struct CatalogCoverageAck(Key);
pub struct CatalogCoverageCovered(Key);
/// Borrowed current responsibility, not a durable receipt. The exclusive reaper
/// borrow prevents retirement, shutdown, report consumption or domain reuse
/// until the control transaction releases this permit.
/// ```compile_fail
/// fn retire_while_borrowed(
///     reaper: &mut hex_stream::MediaDomainReaper,
///     binding: &hex_stream::CatalogCoverageBinding,
///     covered: &hex_stream::CatalogCoverageCovered,
/// ) {
///     let permit = reaper.validate_current_catalog_coverage(binding, covered).unwrap();
///     reaper.try_reap_domain(hex_stream::DomainId::new(1).unwrap());
///     assert!(permit.matches_binding(binding));
/// }
/// ```
/// Never hold it across
/// destructors, provider calls, joins or callback work.
pub struct CurrentCatalogCoverage<'a> {
    _reaper: &'a mut MediaDomainReaper,
    key: &'a Key,
}
impl CurrentCatalogCoverage<'_> {
    /// Identity check for the control owner's private claim association. The
    /// caller must also validate its own lifecycle/runtime/slot incarnation.
    pub fn matches_binding(&self, binding: &CatalogCoverageBinding) -> bool {
        self.key.same(&binding.0)
    }
}
/// Control-only identity anchor for audio's private claim association binding.
/// No public scalar factory, catalog clone or graph retirement assertion.
pub struct CatalogCoverageBinding(Key);
pub struct CatalogCoverageRefusal {
    pub reason: CatalogCustodyError,
    pub reservation: CatalogCoverageReservation,
    pub lease: CatalogLease,
}
pub struct VerifiedCoverageRefusal {
    pub reason: CatalogCustodyError,
    pub reservation: CatalogCoverageReservation,
    pub verified: crate::VerifiedOwnedStreamRevision,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogCoverageProgress {
    Idle,
    Advanced(usize),
    Incomplete,
}
/// Data-capacity contract only. Indexed backing is ALREADY part of the domain
/// charge; handle/carrier sizes describe separate caller-owned storage, not
/// additional allocations or allocator/RSS usage. Failure payload is Unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogCoverageStorageReport {
    pub records: crate::AllocationCapacity,
    pub lease_cells: crate::AllocationCapacity,
    pub indexed_bytes_already_in_domain: usize,
    pub whole_domain_data_bytes: usize,
    pub reservation_header_bytes: usize,
    pub binding_header_bytes: usize,
    pub pending_header_bytes: usize,
    pub ack_header_bytes: usize,
    pub covered_header_bytes: usize,
    pub refusal_pointee_bytes: usize,
    pub caller_retained_metadata_bytes: usize,
    pub caller_construction_metadata_bytes: usize,
    pub retained_metadata_bound: usize,
    pub construction_metadata_overlap: usize,
    pub failure_payload_unknown: bool,
}
impl Key {
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.seal, &other.seal)
            && self.domain == other.domain
            && self.incarnation == other.incarnation
            && self.source == other.source
            && self.source_nonce == other.source_nonce
            && self.target == other.target
            && self.target_nonce == other.target_nonce
    }
}
impl CatalogCoverageReservation {
    pub fn binding_on_control(&self) -> CatalogCoverageBinding {
        let k = &self.0;
        CatalogCoverageBinding(Key {
            seal: Arc::clone(&k.seal),
            domain: k.domain,
            incarnation: k.incarnation,
            source: k.source,
            source_nonce: k.source_nonce,
            target: k.target,
            target_nonce: k.target_nonce,
            created: k.created,
        })
    }
}
impl CatalogCoverageBinding {
    pub fn matches_ack(&self, ack: &CatalogCoverageAck) -> bool {
        self.0.same(&ack.0)
    }
    pub fn matches_covered(&self, covered: &CatalogCoverageCovered) -> bool {
        self.0.same(&covered.0)
    }
}
impl MediaDomainReaper {
    /// Validate CURRENT physical custody rather than the historical source
    /// transition. Source record reuse is allowed; the stable target and actual
    /// domain incarnation may not have retired or been replaced. No allocation
    /// or catalog/Arc clone is performed, and no lasting authority is minted.
    pub fn validate_current_catalog_coverage<'a>(
        &'a mut self,
        binding: &CatalogCoverageBinding,
        covered: &'a CatalogCoverageCovered,
    ) -> Result<CurrentCatalogCoverage<'a>, CatalogCustodyError> {
        let key = &covered.0;
        if !binding.0.same(key) || !Arc::ptr_eq(&self.seal, &key.seal) {
            return Err(CatalogCustodyError::Stale);
        }
        let slot = self
            .slots
            .get(key.domain)
            .filter(|s| !s.vacant && s.incarnation == key.incarnation)
            .ok_or(CatalogCustodyError::Stale)?;
        if slot.report.is_some() || slot.terminal_error.is_some() || slot.custody.failed() {
            return Err(CatalogCustodyError::Incomplete);
        }
        if slot.custody.coverage != Some((key.target, key.target_nonce)) {
            return Err(CatalogCustodyError::Stale);
        }
        let record = slot
            .custody
            .records
            .get(key.target)
            .filter(|r| r.nonce == key.target_nonce && r.state == RecordState::CoverageHeld)
            .ok_or(CatalogCustodyError::Stale)?;
        let lease = slot
            .custody
            .leases
            .get(record.lease_cell)
            .and_then(Option::as_ref)
            .ok_or(CatalogCustodyError::Incomplete)?;
        let control = slot
            .control
            .as_ref()
            .ok_or(CatalogCustodyError::Incomplete)?;
        if !control.matches_verified_catalog(lease) {
            return Err(CatalogCustodyError::WrongPair);
        }
        Ok(CurrentCatalogCoverage { _reaper: self, key })
    }
    /// Borrowed control report; declarations must be disjoint from existing
    /// domain/graph/claim storage. No admission or unknown-owner completion.
    pub fn catalog_coverage_storage_report(
        &self,
        id: DomainId,
        caller_retained_metadata_bytes: usize,
        caller_construction_metadata_bytes: usize,
    ) -> Result<CatalogCoverageStorageReport, CatalogCustodyError> {
        let slot = self
            .slots
            .iter()
            .find(|s| !s.vacant && s.id == id)
            .ok_or(CatalogCustodyError::Stale)?;
        let records = crate::AllocationCapacity::of::<Record>(slot.custody.records.capacity());
        let lease_cells =
            crate::AllocationCapacity::of::<Option<CatalogLease>>(slot.custody.leases.capacity());
        let indexed = records
            .bytes()
            .and_then(|n| n.checked_add(lease_cells.bytes()?))
            .ok_or(CatalogCustodyError::Overflow)?;
        let retained = indexed
            .checked_add(caller_retained_metadata_bytes)
            .ok_or(CatalogCustodyError::Overflow)?;
        let construction = retained
            .checked_add(caller_construction_metadata_bytes)
            .ok_or(CatalogCustodyError::Overflow)?;
        Ok(CatalogCoverageStorageReport {
            records,
            lease_cells,
            indexed_bytes_already_in_domain: indexed,
            whole_domain_data_bytes: slot.data_bytes,
            reservation_header_bytes: std::mem::size_of::<CatalogCoverageReservation>(),
            binding_header_bytes: std::mem::size_of::<CatalogCoverageBinding>(),
            pending_header_bytes: std::mem::size_of::<CatalogCoveragePending>(),
            ack_header_bytes: std::mem::size_of::<CatalogCoverageAck>(),
            covered_header_bytes: std::mem::size_of::<CatalogCoverageCovered>(),
            refusal_pointee_bytes: std::mem::size_of::<CatalogCoverageRefusal>(),
            caller_retained_metadata_bytes,
            caller_construction_metadata_bytes,
            retained_metadata_bound: retained,
            construction_metadata_overlap: construction,
            failure_payload_unknown: slot.custody.failed(),
        })
    }
    pub fn reserve_catalog_coverage(
        &mut self,
        id: DomainId,
        catalog: &CatalogLease,
    ) -> Result<CatalogCoverageReservation, CatalogCustodyError> {
        let domain = self
            .slots
            .iter()
            .position(|s| !s.vacant && s.id == id)
            .ok_or(CatalogCustodyError::Stale)?;
        let slot = &mut self.slots[domain];
        if slot.report.is_some() || slot.terminal_error.is_some() || slot.custody.failed() {
            return Err(CatalogCustodyError::Busy);
        }
        if !slot
            .control
            .as_ref()
            .is_some_and(|c| c.matches_catalog(catalog))
        {
            return Err(CatalogCustodyError::WrongPair);
        }
        let mut empty = slot
            .custody
            .records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.state == RecordState::Empty);
        let source = empty
            .next()
            .map(|(i, _)| i)
            .ok_or(CatalogCustodyError::Capacity)?;
        let (target, target_nonce, created) = if let Some((target, nonce)) = slot.custody.coverage {
            let r = slot
                .custody
                .records
                .get(target)
                .ok_or(CatalogCustodyError::Stale)?;
            if r.nonce != nonce || r.state != RecordState::CoverageHeld {
                return Err(CatalogCustodyError::Busy);
            }
            if !slot
                .custody
                .leases
                .get(r.lease_cell)
                .and_then(Option::as_ref)
                .is_some_and(|held| held.same_verified_catalog(catalog))
            {
                return Err(CatalogCustodyError::WrongPair);
            }
            (target, nonce, false)
        } else {
            (
                empty
                    .next()
                    .map(|(i, _)| i)
                    .ok_or(CatalogCustodyError::Capacity)?,
                0,
                true,
            )
        };
        let source_nonce = slot
            .custody
            .next_nonce
            .checked_add(1)
            .ok_or(CatalogCustodyError::Exhausted)?;
        let next = source_nonce
            .checked_add(u64::from(created))
            .ok_or(CatalogCustodyError::Exhausted)?;
        let target_nonce = if created { next } else { target_nonce };
        for record in [source, target] {
            let cell = slot.custody.records[record].lease_cell;
            if (record == source || created)
                && !slot.custody.leases.get(cell).is_some_and(Option::is_none)
            {
                return Err(CatalogCustodyError::Busy);
            }
        }
        // Every fallible check is complete. Reserve metadata, not input owners.
        slot.custody.next_nonce = next;
        let r = &mut slot.custody.records[source];
        r.nonce = source_nonce;
        r.state = RecordState::CoverageSourceReserved;
        r.target = Some((target, target_nonce));
        if created {
            let r = &mut slot.custody.records[target];
            r.nonce = target_nonce;
            r.state = RecordState::CoverageReserved;
            slot.custody.coverage = Some((target, target_nonce));
        }
        Ok(CatalogCoverageReservation(Key {
            seal: Arc::clone(&self.seal),
            domain,
            incarnation: slot.incarnation,
            source,
            source_nonce,
            target,
            target_nonce,
            created,
        }))
    }
    pub fn reserve_verified_catalog_coverage(
        &mut self,
        id: DomainId,
        verified: &crate::VerifiedOwnedStreamRevision,
    ) -> Result<CatalogCoverageReservation, CatalogCustodyError> {
        self.reserve_catalog_coverage(id, verified.catalog_on_control())
    }
    fn coverage_slot(
        &self,
        key: &Key,
        state: RecordState,
    ) -> Result<&ControlSlot, CatalogCustodyError> {
        if !Arc::ptr_eq(&self.seal, &key.seal) {
            return Err(CatalogCustodyError::Stale);
        }
        let slot = self
            .slots
            .get(key.domain)
            .filter(|s| !s.vacant && s.incarnation == key.incarnation)
            .ok_or(CatalogCustodyError::Stale)?;
        if slot.custody.coverage != Some((key.target, key.target_nonce))
            || !slot.custody.records.get(key.source).is_some_and(|r| {
                r.nonce == key.source_nonce
                    && r.state == state
                    && r.target == Some((key.target, key.target_nonce))
            })
            || slot
                .custody
                .records
                .get(key.target)
                .is_none_or(|r| r.nonce != key.target_nonce)
        {
            return Err(CatalogCustodyError::Stale);
        }
        if slot.report.is_some() || slot.terminal_error.is_some() || slot.custody.failed() {
            return Err(CatalogCustodyError::Busy);
        }
        Ok(slot)
    }
    fn coverage_commit_preflight(
        &self,
        key: &Key,
        lease: &CatalogLease,
    ) -> Result<(), CatalogCustodyError> {
        let slot = self.coverage_slot(key, RecordState::CoverageSourceReserved)?;
        if !slot
            .control
            .as_ref()
            .is_some_and(|c| c.matches_catalog(lease))
        {
            return Err(CatalogCustodyError::WrongPair);
        }
        let target = &slot.custody.records[key.target];
        let source = &slot.custody.records[key.source];
        if !slot
            .custody
            .leases
            .get(source.lease_cell)
            .is_some_and(Option::is_none)
        {
            return Err(CatalogCustodyError::Busy);
        }
        if key.created {
            if target.state != RecordState::CoverageReserved
                || !slot
                    .custody
                    .leases
                    .get(target.lease_cell)
                    .is_some_and(Option::is_none)
            {
                return Err(CatalogCustodyError::Busy);
            }
        } else if target.state != RecordState::CoverageHeld
            || !slot
                .custody
                .leases
                .get(target.lease_cell)
                .and_then(Option::as_ref)
                .is_some_and(|held| held.same_verified_catalog(lease))
        {
            return Err(CatalogCustodyError::WrongPair);
        }
        Ok(())
    }
    pub fn cancel_catalog_coverage(
        &mut self,
        reservation: CatalogCoverageReservation,
    ) -> Result<(), (CatalogCustodyError, CatalogCoverageReservation)> {
        if let Err(error) = self.coverage_slot(&reservation.0, RecordState::CoverageSourceReserved)
        {
            return Err((error, reservation));
        }
        let k = &reservation.0;
        let custody = &mut self.slots[k.domain].custody;
        custody.records[k.source].state = RecordState::Empty;
        custody.records[k.source].target = None;
        if k.created {
            custody.records[k.target].state = RecordState::Empty;
            custody.coverage = None;
        }
        Ok(()) // no lease ever moved; control-only seal release
    }
    fn coverage_commit(&mut self, key: Key, lease: CatalogLease) -> CatalogCoveragePending {
        let custody = &mut self.slots[key.domain].custody;
        if key.created {
            let cell = custody.records[key.target].lease_cell;
            custody.leases[cell] = Some(lease); // destination actual custody FIRST
            custody.records[key.target].state = RecordState::CoverageHeld;
            custody.records[key.source].state = RecordState::AckReady;
        } else {
            let cell = custody.records[key.source].lease_cell;
            custody.leases[cell] = Some(lease); // persistent redundant disposal owner
            custody.records[key.source].state = RecordState::RedundantPending;
        }
        CatalogCoveragePending(key)
    }
    pub fn commit_catalog_coverage(
        &mut self,
        reservation: CatalogCoverageReservation,
        lease: CatalogLease,
    ) -> Result<CatalogCoveragePending, Box<CatalogCoverageRefusal>> {
        if let Err(reason) = self.coverage_commit_preflight(&reservation.0, &lease) {
            return Err(Box::new(CatalogCoverageRefusal {
                reason,
                reservation,
                lease,
            }));
        }
        Ok(self.coverage_commit(reservation.0, lease))
    }
    pub fn commit_verified_catalog_coverage(
        &mut self,
        reservation: CatalogCoverageReservation,
        verified: crate::VerifiedOwnedStreamRevision,
    ) -> Result<
        (hex_engine::OwnedPreparedRevision, CatalogCoveragePending),
        Box<VerifiedCoverageRefusal>,
    > {
        if let Err(reason) =
            self.coverage_commit_preflight(&reservation.0, verified.catalog_on_control())
        {
            return Err(Box::new(VerifiedCoverageRefusal {
                reason,
                reservation,
                verified,
            }));
        }
        let (root, lease) = verified.into_parts();
        Ok((root, self.coverage_commit(reservation.0, lease)))
    }
    /// Borrow the persistent reaper ONLY outside ordinary/root mutexes. Native
    /// caller integration must use its existing extracted cleanup capsule.
    /// Pending precedes take; panic preserves its payload/tombstone and target.
    pub fn advance_catalog_coverage_on_control(&mut self, id: DomainId) -> CatalogCoverageProgress {
        let Some(slot) = self.slots.iter_mut().find(|s| !s.vacant && s.id == id) else {
            return CatalogCoverageProgress::Incomplete;
        };
        if slot.custody.failed() {
            return CatalogCoverageProgress::Incomplete;
        }
        let mut advanced = 0;
        for index in 0..slot.custody.records.len() {
            if slot.custody.records[index].state != RecordState::RedundantPending {
                continue;
            }
            let Some((target, nonce)) = slot.custody.records[index].target else {
                return CatalogCoverageProgress::Incomplete;
            };
            let Some(destination) = slot
                .custody
                .records
                .get(target)
                .filter(|r| r.nonce == nonce && r.state == RecordState::CoverageHeld)
            else {
                return CatalogCoverageProgress::Incomplete;
            };
            let cell = slot.custody.records[index].lease_cell;
            if !slot
                .custody
                .leases
                .get(destination.lease_cell)
                .and_then(Option::as_ref)
                .zip(slot.custody.leases.get(cell).and_then(Option::as_ref))
                .is_some_and(|(a, b)| a.same_verified_catalog(b))
            {
                return CatalogCoverageProgress::Incomplete;
            }
            slot.custody.records[index].state = RecordState::DisposalPending;
            // Sole owner stays in this persistent cell until after pending.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                drop(slot.custody.leases[cell].take());
                #[cfg(test)]
                {
                    slot.custody.records[index].disposal_attempts += 1;
                    assert!(
                        !slot.custody.records[index].fail_disposal,
                        "negative control after actual redundant lease destruction"
                    );
                }
            }));
            match outcome {
                Ok(()) => {
                    slot.custody.records[index].state = RecordState::AckReady;
                    advanced += 1;
                }
                Err(payload) => {
                    slot.custody.records[index].first_panic = Some(payload);
                    slot.custody.records[index].state = RecordState::Failed;
                    self.phase.store(2, Ordering::Release);
                    return CatalogCoverageProgress::Incomplete;
                }
            }
        }
        if advanced == 0 {
            CatalogCoverageProgress::Idle
        } else {
            CatalogCoverageProgress::Advanced(advanced)
        }
    }
    pub fn catalog_coverage_ack(
        &mut self,
        pending: CatalogCoveragePending,
    ) -> Result<CatalogCoverageAck, (CatalogCustodyError, CatalogCoveragePending)> {
        if let Err(error) = self.coverage_slot(&pending.0, RecordState::AckReady) {
            return Err((error, pending));
        }
        self.slots[pending.0.domain].custody.records[pending.0.source].state =
            RecordState::AckIssued;
        Ok(CatalogCoverageAck(pending.0))
    }
    pub fn consume_catalog_coverage_ack(
        &mut self,
        ack: CatalogCoverageAck,
    ) -> Result<CatalogCoverageCovered, (CatalogCustodyError, CatalogCoverageAck)> {
        if let Err(error) = self.coverage_slot(&ack.0, RecordState::AckIssued) {
            return Err((error, ack));
        }
        let custody = &mut self.slots[ack.0.domain].custody;
        custody.records[ack.0.source].state = RecordState::Empty;
        custody.records[ack.0.source].target = None;
        Ok(CatalogCoverageCovered(ack.0))
    }
    /// Distinct FINAL shutdown receipt. Live coverage ack can never mint this.
    pub fn stable_catalog_cleanup_receipt(
        &mut self,
        id: DomainId,
    ) -> Result<CatalogCustodyReceipt, CatalogCustodyError> {
        let index = self
            .slots
            .iter()
            .position(|s| !s.vacant && s.id == id)
            .ok_or(CatalogCustodyError::Stale)?;
        let slot = &mut self.slots[index];
        let report = slot
            .report
            .filter(|r| r.successful_report().is_some())
            .ok_or(CatalogCustodyError::Incomplete)?;
        let (record, nonce) = slot.custody.coverage.ok_or(CatalogCustodyError::Stale)?;
        let r = &mut slot.custody.records[record];
        if r.nonce != nonce || r.state != RecordState::CoverageHeld {
            return Err(CatalogCustodyError::Busy);
        }
        if slot
            .custody
            .leases
            .get(r.lease_cell)
            .is_none_or(Option::is_some)
        {
            return Err(CatalogCustodyError::Incomplete);
        }
        r.state = RecordState::Committed; // consume through existing report check
        Ok(CatalogCustodyReceipt {
            reservation: CatalogCustodyReservation {
                seal: Arc::clone(&self.seal),
                index,
                incarnation: slot.incarnation,
                record,
                nonce,
            },
            report,
        })
    }
}
