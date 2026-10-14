//! Opt-in control storage only. Actual callback retirement producers are unwired.
use super::*;
use hex_engine::storage_report::{ClaimIdentity, ClaimReservation, ClaimedPlaybackOwner};
use std::sync::atomic::{AtomicU64, Ordering};

// Identity allocator only: no owners/rows/global custodian registry. Never reset.
static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);
fn next_root(counter: &AtomicU64) -> Result<u64, ClaimSlotError> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| ClaimSlotError::IncarnationExhausted)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimSlotError {
    Allocation,
    Capacity,
    Overflow,
    ReservationExceeded,
    Incomplete,
    Busy,
    Stale,
    Duplicate,
    PairMismatch,
    IncarnationExhausted,
    RetirementPending,
}
/// Move-only non-owning association. No public constructor/Clone/Copy or owner.
/// Root identity plus slot incarnation prevent recreation/reuse ABA.
pub struct ClaimAssociation {
    root: u64,
    slot: usize,
    incarnation: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClaimUnionCapacityReport {
    pub slots: usize,
    pub record_capacity: usize,
    pub bytes: usize,
    pub prepared_bytes: Option<usize>,
    pub construction_bytes: Option<usize>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Vacant,
    Reserved,
    Staged,
    Accepted,
}
struct Slot {
    incarnation: u64,
    phase: Phase,
    identity: Option<ClaimIdentity>,
    anchors: Option<PublicationAnchors>,
    returned: Option<ClaimedPlaybackOwner>,
    facts: Facts,
    #[cfg(test)]
    probe: Option<Box<Probe>>,
}
impl Slot {
    fn vacant() -> Self {
        Self {
            incarnation: 0,
            phase: Phase::Vacant,
            identity: None,
            anchors: None,
            returned: None,
            facts: Facts::default(),
            #[cfg(test)]
            probe: None,
        }
    }
}
#[derive(Default)]
struct Facts {
    inherited_roles: u8,
    catalog_accounted: bool,
    custody_returned: bool,
    quiescence_required: bool,
    quiescent: bool,
    incomplete: bool,
}
impl Facts {
    fn ready(&self) -> bool {
        self.inherited_roles == 0b1_1111
            && self.catalog_accounted
            && self.custody_returned
            && (!self.quiescence_required || self.quiescent)
            && !self.incomplete
    }
}
// No production/public receipt factory exists in this unit. A future producer
// must prove ALL actual owners of a role retired, not voice/silence completion.
// CatalogAccounted requires actual lease release/covered transfer and cleanup;
// it must not be invented while the retained verifier itself causes Busy.
pub enum Fact {
    Role(u8),
    CatalogAccounted,
    Quiescent,
}
pub struct ClaimReceipt {
    root: u64,
    slot: usize,
    incarnation: u64,
    fact: Fact,
}
#[derive(Clone, Copy)]
struct Record {
    root: usize,
    item: usize,
}
pub(crate) struct ClaimUnionSlots {
    root: u64,
    slots: Vec<Slot>,
    records: Vec<Record>,
    reservation: ClaimReservation,
    report: ClaimUnionCapacityReport,
    #[cfg(test)]
    drop_observer: Option<Box<dyn FnOnce() + Send>>,
}
pub struct ClaimStageRefusal {
    pub reason: ClaimSlotError,
    pub association: ClaimAssociation,
    pub owner: ClaimedPlaybackOwner,
    pub anchors: PublicationAnchors,
}
pub struct ClaimReturnedOwnerRefusal {
    pub reason: ClaimSlotError,
    pub association: ClaimAssociation,
    pub owner: ClaimedPlaybackOwner,
}
/// Move only; control consumes/drops this OUTSIDE the media-root mutex.
pub struct ClaimReclamation {
    pub owner: ClaimedPlaybackOwner,
    pub anchors: PublicationAnchors,
}
impl ClaimUnionSlots {
    pub(crate) fn new(reservation: ClaimReservation) -> Result<Self, ClaimSlotError> {
        let mut slots = Vec::new();
        let mut records = Vec::new();
        slots
            .try_reserve_exact(reservation.items)
            .map_err(|_| ClaimSlotError::Allocation)?;
        records
            .try_reserve_exact(reservation.items)
            .map_err(|_| ClaimSlotError::Allocation)?;
        let bytes = slots
            .capacity()
            .checked_mul(std::mem::size_of::<Slot>())
            .and_then(|n| {
                records
                    .capacity()
                    .checked_mul(std::mem::size_of::<Record>())
                    .and_then(|r| n.checked_add(r))
            })
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
            .ok_or(ClaimSlotError::Overflow)?;
        let prepared = bytes
            .checked_add(reservation.other_retained_bytes)
            .ok_or(ClaimSlotError::Overflow)?;
        let construction = prepared
            .checked_add(reservation.other_construction_bytes)
            .ok_or(ClaimSlotError::Overflow)?;
        if prepared > reservation.max_prepared_data_bytes
            || construction > reservation.max_construction_data_bytes
        {
            return Err(ClaimSlotError::ReservationExceeded);
        }
        slots.resize_with(reservation.items, Slot::vacant);
        let root = next_root(&NEXT_ROOT)?;
        let report = ClaimUnionCapacityReport {
            slots: slots.capacity(),
            record_capacity: records.capacity(),
            bytes,
            prepared_bytes: Some(prepared),
            construction_bytes: Some(construction),
        };
        Ok(Self {
            root,
            slots,
            records,
            reservation,
            report,
            #[cfg(test)]
            drop_observer: None,
        })
    }
    #[cfg(test)]
    pub(crate) fn observe_drop(&mut self, observer: Box<dyn FnOnce() + Send>) {
        self.drop_observer = Some(observer);
    }
    pub(crate) fn report(&self) -> ClaimUnionCapacityReport {
        self.report
    }
    pub(crate) fn phase(&self) -> hex_stream::MediaCleanupPhase {
        if self.slots.iter().any(|s| s.facts.incomplete) {
            hex_stream::MediaCleanupPhase::FailedIncomplete
        } else if self.slots.iter().any(|s| s.phase != Phase::Vacant) {
            hex_stream::MediaCleanupPhase::Pending
        } else {
            hex_stream::MediaCleanupPhase::Complete
        }
    }
    fn slot(&self, a: &ClaimAssociation) -> Result<&Slot, ClaimSlotError> {
        if a.root != self.root {
            return Err(ClaimSlotError::Stale);
        }
        self.slots
            .get(a.slot)
            .filter(|s| s.phase != Phase::Vacant && s.incarnation == a.incarnation)
            .ok_or(ClaimSlotError::Stale)
    }
    fn slot_mut(&mut self, a: &ClaimAssociation) -> Result<&mut Slot, ClaimSlotError> {
        self.slot(a)?;
        Ok(&mut self.slots[a.slot])
    }
    /// Reserves fixed storage only, never graph/catalog admission. No owners
    /// move. The future complete preflight must validate before staging/queueing.
    pub(crate) fn reserve_storage(&mut self) -> Result<ClaimAssociation, ClaimSlotError> {
        if self
            .slots
            .iter()
            .any(|s| matches!(s.phase, Phase::Reserved | Phase::Staged))
        {
            return Err(ClaimSlotError::Busy);
        }
        let index = self
            .slots
            .iter()
            .position(|s| s.phase == Phase::Vacant)
            .ok_or(ClaimSlotError::Capacity)?;
        let s = &mut self.slots[index];
        let incarnation = s
            .incarnation
            .checked_add(1)
            .ok_or(ClaimSlotError::IncarnationExhausted)?;
        s.incarnation = incarnation;
        s.phase = Phase::Reserved;
        s.identity = None;
        s.facts = Facts::default();
        Ok(ClaimAssociation {
            root: self.root,
            slot: index,
            incarnation,
        })
    }
    pub(crate) fn close_unstaged(&mut self) {
        for s in &mut self.slots {
            if s.phase == Phase::Reserved && s.anchors.is_none() && s.returned.is_none() {
                s.phase = Phase::Vacant;
                s.identity = None;
            }
        }
    }
    pub(crate) fn reserve(
        &mut self,
        input: &PublicationClaims,
    ) -> Result<ClaimAssociation, ClaimSlotError> {
        if self
            .slots
            .iter()
            .any(|s| matches!(s.phase, Phase::Reserved | Phase::Staged))
        {
            return Err(ClaimSlotError::Busy);
        }
        let index = self
            .slots
            .iter()
            .position(|s| s.phase == Phase::Vacant)
            .ok_or(ClaimSlotError::Capacity)?;
        let incarnation = self.slots[index]
            .incarnation
            .checked_add(1)
            .ok_or(ClaimSlotError::IncarnationExhausted)?;
        self.preflight(input)?;
        let slot = &mut self.slots[index];
        slot.incarnation = incarnation;
        slot.phase = Phase::Reserved;
        slot.identity = Some(input.claims.identity_on_control());
        slot.facts = Facts::default();
        Ok(ClaimAssociation {
            root: self.root,
            slot: index,
            incarnation,
        })
    }
    // Conservative complete-scope refusal remains in force. Scalar scratch is
    // preallocated; no borrowed addresses survive a preflight or root mutation.
    fn preflight(&mut self, input: &PublicationClaims) -> Result<(), ClaimSlotError> {
        self.records.clear();
        let mut prepared = self
            .report
            .bytes
            .checked_add(self.reservation.other_retained_bytes)
            .ok_or(ClaimSlotError::Overflow)?;
        let mut anchors = 0_usize;
        let roots = self.slots.len();
        for root in 0..=roots {
            let (report, count, anchor_count) = if root == roots {
                (
                    input.claims.report(),
                    input.claims.item_count(),
                    input.claims.anchored_allocation_count(),
                )
            } else if let Some(a) = &self.slots[root].anchors {
                (
                    a.claims.report(),
                    a.claims.item_count(),
                    a.claims.anchor_count(),
                )
            } else {
                continue;
            };
            // Until actual catalog/association reservation accounting and the
            // lifetime producer are available, no partial footprint is admitted.
            let graph = report.graph_data_bytes.ok_or(ClaimSlotError::Incomplete)?;
            let containers = report
                .prepared_container_bytes
                .ok_or(ClaimSlotError::Incomplete)?;
            anchors = anchors
                .checked_add(anchor_count)
                .ok_or(ClaimSlotError::Overflow)?;
            if anchors > self.reservation.anchors {
                return Err(ClaimSlotError::Capacity);
            }
            for item in 0..count {
                if self.records.len() == self.reservation.items {
                    return Err(ClaimSlotError::Capacity);
                }
                self.records.push(Record { root, item });
            }
            prepared = prepared
                .checked_add(graph)
                .and_then(|n| {
                    containers
                        .checked_sub(report.inline_claim_bytes)
                        .and_then(|c| n.checked_add(c))
                })
                .ok_or(ClaimSlotError::Overflow)?;
        }
        // No naked identity cache: resolve indices while actual roots are live.
        for record in &self.records {
            let item = if record.root == roots {
                input.claims.archived_item_on_control(record.item)
            } else {
                self.slots[record.root]
                    .anchors
                    .as_ref()
                    .and_then(|a| a.claims.archived_item_on_control(record.item))
            };
            if item.is_none() {
                return Err(ClaimSlotError::Incomplete);
            }
        }
        let construction = prepared
            .checked_add(self.reservation.other_construction_bytes)
            .ok_or(ClaimSlotError::Overflow)?;
        if prepared > self.reservation.max_prepared_data_bytes
            || construction > self.reservation.max_construction_data_bytes
        {
            return Err(ClaimSlotError::ReservationExceeded);
        }
        self.report.prepared_bytes = None;
        self.report.construction_bytes = None;
        // Catalog link/lease accounting cannot be omitted or inferred from DSP
        // or graph declarations. This first unit seals/reserves storage only.
        Err(ClaimSlotError::Incomplete)
    }
    pub(crate) fn rollback_reserved(
        &mut self,
        a: ClaimAssociation,
    ) -> Result<(), (ClaimSlotError, ClaimAssociation)> {
        match self.slot(&a) {
            Ok(s) if s.phase == Phase::Reserved => {
                let s = &mut self.slots[a.slot];
                s.phase = Phase::Vacant;
                s.identity = None;
                Ok(())
            }
            Ok(_) => Err((ClaimSlotError::Busy, a)),
            Err(e) => Err((e, a)),
        }
    }
    pub(crate) fn stage(
        &mut self,
        a: ClaimAssociation,
        owner: ClaimedPlaybackOwner,
        anchors: PublicationAnchors,
    ) -> Result<(ClaimedPlaybackOwner, ClaimAssociation), Box<ClaimStageRefusal>> {
        let reason = match self.slot(&a) {
            Ok(s) if s.phase != Phase::Reserved => Some(ClaimSlotError::Busy),
            Ok(s) if s.identity.is_none() => Some(ClaimSlotError::Incomplete),
            Ok(s) if s.identity != Some(owner.identity()) || !anchors.matches_owner(&owner) => {
                Some(ClaimSlotError::PairMismatch)
            }
            Ok(_) => None,
            Err(e) => Some(e),
        };
        if let Some(reason) = reason {
            return Err(Box::new(ClaimStageRefusal {
                reason,
                association: a,
                owner,
                anchors,
            }));
        }
        let s = &mut self.slots[a.slot];
        s.anchors = Some(anchors);
        s.phase = Phase::Staged;
        Ok((owner, a))
    }
    pub(crate) fn accepted(&mut self, a: &ClaimAssociation) -> Result<(), ClaimSlotError> {
        let s = self.slot_mut(a)?;
        if s.phase != Phase::Staged {
            return Err(ClaimSlotError::Duplicate);
        }
        s.phase = Phase::Accepted;
        Ok(())
    }
    pub(crate) fn returned(
        &mut self,
        a: ClaimAssociation,
        owner: ClaimedPlaybackOwner,
    ) -> Result<ClaimAssociation, Box<ClaimReturnedOwnerRefusal>> {
        let reason = match self.slot(&a) {
            Ok(s) if s.returned.is_some() => Some(ClaimSlotError::Duplicate),
            Ok(s)
                if s.phase != Phase::Accepted
                    || s.anchors.as_ref().is_none_or(|p| !p.matches_owner(&owner)) =>
            {
                Some(ClaimSlotError::PairMismatch)
            }
            Ok(_) => None,
            Err(e) => Some(e),
        };
        if let Some(reason) = reason {
            return Err(Box::new(ClaimReturnedOwnerRefusal {
                reason,
                association: a,
                owner,
            }));
        }
        let s = &mut self.slots[a.slot];
        s.returned = Some(owner);
        s.facts.custody_returned = true;
        Ok(a)
    }
    pub(crate) fn receipt(
        &mut self,
        receipt: ClaimReceipt,
    ) -> Result<(), (ClaimSlotError, ClaimReceipt)> {
        let a = ClaimAssociation {
            root: receipt.root,
            slot: receipt.slot,
            incarnation: receipt.incarnation,
        };
        let s = match self.slot_mut(&a) {
            Ok(s) => s,
            Err(e) => return Err((e, receipt)),
        };
        if s.phase != Phase::Accepted {
            return Err((ClaimSlotError::Busy, receipt));
        }
        // Accepted custody alone cannot freeze a role: a later owner transfer
        // could invalidate an earlier retirement fact. No sealed final-transfer
        // proof producer exists yet, so ALL facts conservatively remain refused.
        // Inspect the kind only; never mutate protection/retirement state.
        match receipt.fact {
            Fact::Role(role) if role >= 5 => Err((ClaimSlotError::Incomplete, receipt)),
            Fact::Role(_) | Fact::CatalogAccounted | Fact::Quiescent => {
                Err((ClaimSlotError::Incomplete, receipt))
            }
        }
    }

    pub(crate) fn fault(
        &mut self,
        a: &ClaimAssociation,
        incomplete: bool,
    ) -> Result<(), ClaimSlotError> {
        let s = self.slot_mut(a)?;
        s.facts.quiescence_required = true;
        s.facts.incomplete |= incomplete;
        Ok(())
    }
    pub(crate) fn take_ready(
        &mut self,
        a: ClaimAssociation,
    ) -> Result<ClaimReclamation, (ClaimSlotError, ClaimAssociation)> {
        let s = match self.slot(&a) {
            Ok(s) => s,
            Err(e) => return Err((e, a)),
        };
        if !s.facts.ready() || s.returned.is_none() || s.anchors.is_none() {
            return Err((ClaimSlotError::RetirementPending, a));
        }
        let s = &mut self.slots[a.slot];
        let mut vacant = Slot::vacant();
        vacant.incarnation = s.incarnation;
        match std::mem::replace(s, vacant) {
            Slot {
                returned: Some(owner),
                anchors: Some(anchors),
                ..
            } => Ok(ClaimReclamation { owner, anchors }),
            unchanged => {
                *s = unchanged;
                Err((ClaimSlotError::RetirementPending, a))
            }
        }
    }
}

#[cfg(test)]
impl Drop for ClaimUnionSlots {
    fn drop(&mut self) {
        if let Some(observer) = self.drop_observer.take() {
            observer();
        }
    }
}

#[cfg(test)]
struct Probe {
    drop_thread: std::sync::Arc<std::sync::Mutex<Option<std::thread::ThreadId>>>,
    outside_mutex: std::sync::Arc<std::sync::atomic::AtomicBool>,
    root: std::sync::Weak<std::sync::Mutex<Option<Box<ClaimUnionSlots>>>>,
}
#[cfg(test)]
impl Drop for Probe {
    fn drop(&mut self) {
        *self.drop_thread.lock().unwrap() = Some(std::thread::current().id());
        self.outside_mutex.store(
            self.root
                .upgrade()
                .is_some_and(|root| root.try_lock().is_ok()),
            std::sync::atomic::Ordering::SeqCst,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reservation() -> ClaimReservation {
        ClaimReservation {
            items: 1,
            anchors: 8,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            other_retained_bytes: 0,
            other_construction_bytes: 0,
        }
    }
    // Closed protocol facts only: no actual current Unknown report is admitted.
    fn token(root: &ClaimUnionSlots, slot: usize) -> ClaimAssociation {
        ClaimAssociation {
            root: root.root,
            slot,
            incarnation: root.slots[slot].incarnation,
        }
    }
    fn receipt(a: &ClaimAssociation, fact: Fact) -> ClaimReceipt {
        ClaimReceipt {
            root: a.root,
            slot: a.slot,
            incarnation: a.incarnation,
            fact,
        }
    }
    #[test]
    fn actual_capacity_and_reservation_overflow_refusal() {
        let slots = ClaimUnionSlots::new(reservation()).unwrap();
        assert_eq!(slots.report.slots, slots.slots.capacity());
        assert_eq!(slots.report.record_capacity, slots.records.capacity());
        assert_eq!(
            slots.report.bytes,
            slots.slots.capacity() * std::mem::size_of::<Slot>()
                + slots.records.capacity() * std::mem::size_of::<Record>()
                + std::mem::size_of::<ClaimUnionSlots>()
        );
        assert!(matches!(
            ClaimUnionSlots::new(ClaimReservation {
                items: usize::MAX,
                ..reservation()
            }),
            Err(ClaimSlotError::Allocation)
        ));
        assert!(matches!(
            ClaimUnionSlots::new(ClaimReservation {
                other_retained_bytes: usize::MAX,
                ..reservation()
            }),
            Err(ClaimSlotError::Overflow)
        ));
        assert!(matches!(
            ClaimUnionSlots::new(ClaimReservation {
                max_prepared_data_bytes: 0,
                ..reservation()
            }),
            Err(ClaimSlotError::ReservationExceeded)
        ));
        let counter = AtomicU64::new(u64::MAX);
        assert_eq!(
            next_root(&counter),
            Err(ClaimSlotError::IncarnationExhausted)
        );
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }
    #[test]
    fn storage_rollback_incarnation_root_aba_and_exhaustion() {
        let mut first = ClaimUnionSlots::new(reservation()).unwrap();
        let a = first.reserve_storage().unwrap();
        let stale = token(&first, a.slot);
        assert!(matches!(first.reserve_storage(), Err(ClaimSlotError::Busy)));
        assert!(first.rollback_reserved(a).is_ok());
        let new = first.reserve_storage().unwrap();
        assert_ne!(new.incarnation, stale.incarnation);
        assert!(matches!(first.slot(&stale), Err(ClaimSlotError::Stale)));
        assert!(first.rollback_reserved(new).is_ok());
        first.slots[0].incarnation = u64::MAX;
        assert!(matches!(
            first.reserve_storage(),
            Err(ClaimSlotError::IncarnationExhausted)
        ));
        let mut recreated = ClaimUnionSlots::new(reservation()).unwrap();
        let live = recreated.reserve_storage().unwrap();
        assert_ne!(first.root, recreated.root);
        assert!(matches!(recreated.slot(&stale), Err(ClaimSlotError::Stale)));
        assert!(recreated.rollback_reserved(live).is_ok());
    }
    #[test]
    fn ack_boxreturn_missing_duplicate_stale_and_incomplete_facts_never_free() {
        let mut root = ClaimUnionSlots::new(reservation()).unwrap();
        let a = root.reserve_storage().unwrap();
        let bytes = root.report.bytes;
        // Pre-acceptance receipt is refused before any fact mutation.
        assert!(matches!(
            root.receipt(receipt(&a, Fact::Role(0))),
            Err((ClaimSlotError::Busy, _))
        ));
        root.slots[a.slot].phase = Phase::Accepted; // private model, no graph admission
        root.slots[a.slot].facts.custody_returned = true;
        for fact in [
            Fact::Role(0),
            Fact::Role(4),
            Fact::CatalogAccounted,
            Fact::Quiescent,
        ] {
            assert!(matches!(
                root.receipt(receipt(&a, fact)),
                Err((ClaimSlotError::Incomplete, _))
            ));
            assert_eq!(root.slots[a.slot].facts.inherited_roles, 0);
            assert!(!root.slots[a.slot].facts.catalog_accounted);
            assert!(!root.slots[a.slot].facts.quiescent);
            assert_eq!(root.report.bytes, bytes);
        }
        // Repeated assertions cannot acquire retirement authority.
        assert!(matches!(
            root.receipt(receipt(&a, Fact::Role(0))),
            Err((ClaimSlotError::Incomplete, _))
        ));
        assert!(!root.slots[a.slot].facts.ready());
        assert!(matches!(
            root.take_ready(token(&root, a.slot)),
            Err((ClaimSlotError::RetirementPending, _))
        ));
        root.fault(&a, true).unwrap();
        root.close_unstaged();
        assert_eq!(
            root.phase(),
            hex_stream::MediaCleanupPhase::FailedIncomplete
        );
        let mut stale = receipt(&a, Fact::Quiescent);
        stale.incarnation += 1;
        assert!(matches!(
            root.receipt(stale),
            Err((ClaimSlotError::Stale, _))
        ));
    }
    #[test]
    fn occupied_payload_capacity_retention_and_real_drop_outside_root_lock() {
        use std::sync::{Arc, Mutex, atomic::AtomicBool};
        let thread = Arc::new(Mutex::new(None));
        let outside = Arc::new(AtomicBool::new(false));
        let root = Arc::new(Mutex::new(Some(Box::new(
            ClaimUnionSlots::new(reservation()).unwrap(),
        ))));
        {
            let mut guard = root.lock().unwrap();
            let slots = guard.as_mut().unwrap();
            let a = slots.reserve_storage().unwrap();
            slots.slots[a.slot].phase = Phase::Accepted;
            slots.slots[a.slot].probe = Some(Box::new(Probe {
                drop_thread: Arc::clone(&thread),
                outside_mutex: Arc::clone(&outside),
                root: Arc::downgrade(&root),
            }));
            assert!(matches!(
                slots.reserve_storage(),
                Err(ClaimSlotError::Capacity)
            ));
            slots.close_unstaged();
            assert_eq!(slots.phase(), hex_stream::MediaCleanupPhase::Pending);
            assert!(thread.lock().unwrap().is_none());
        }
        let local = { root.lock().unwrap().take().unwrap() };
        drop(local); // explicit control teardown test, not an actual native receipt
        assert_eq!(*thread.lock().unwrap(), Some(std::thread::current().id()));
        assert!(outside.load(Ordering::SeqCst));
    }
}
