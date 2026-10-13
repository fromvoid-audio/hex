//! Bounded callback-owned intent service; no native/engine-demand readiness.
use crate::{
    AllocationCapacity, CatalogLease, PageExtent, ReadinessRefusal, ReadinessStamp,
    StreamAllocationFootprint, VerifiedPageEndpoint, WantedPageManifest,
};

/// Three protection roles, not a graph-domain admission policy. Overlapping
/// roles share the same cached buffer; releasing one never clears another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectionRole {
    Active,
    Candidate,
    Transition,
}
impl ProtectionRole {
    fn index(self) -> usize {
        match self {
            Self::Active => 0,
            Self::Candidate => 1,
            Self::Transition => 2,
        }
    }
    fn mask(self) -> u8 {
        1 << self.index()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageRequestStatus {
    Accepted,
    Pending,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentStatus {
    Accepted,
    Pending,
    Refused(ReadinessRefusal),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestProgress {
    pub accepted: usize,
    pub pending: usize,
    pub refused: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessProgress {
    Pending(RequestProgress),
    InstalledProtected,
    Refused(ReadinessRefusal),
}

/// Borrowed actual catalog/pair identity plus exact installed tokens. Does not
/// authorize playback or certify engine demand. Borrowing prevents concurrent
/// endpoint mutation; retain a control-owned manifest to revalidate later.
pub struct InstalledProtectedReceipt<'a> {
    catalog: &'a CatalogLease,
    stamp: ReadinessStamp,
    role: ProtectionRole,
}
impl InstalledProtectedReceipt<'_> {
    pub fn stamp(&self) -> ReadinessStamp {
        self.stamp
    }
    pub fn role(&self) -> ProtectionRole {
        self.role
    }
    pub fn same_catalog(&self, catalog: &CatalogLease) -> bool {
        self.catalog.same_catalog(catalog)
    }
}

#[derive(Default)]
struct RoleSlot {
    wanted: Vec<PageExtent>,
    protected: Vec<PageExtent>,
    pending: Option<ReadinessStamp>,
    installed: Option<ReadinessStamp>,
    /// Reserved by submission before any protection commit. Sticky until
    /// acknowledged/cancelled; subsequent intents cannot overwrite the receipt.
    receipt_reserved: bool,
    unread_receipt: bool,
}

/// Actual endpoint-exclusive reserved capacities. Inline metadata lives in the
/// caller's PageReadiness value; no implicit heap-object charge. Manifest Vecs
/// and graph owners remain caller reservations, shared catalog/pool charged once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadinessAllocationFootprint {
    pub wanted: [AllocationCapacity; 3],
    pub protected: [AllocationCapacity; 3],
}
impl ReadinessAllocationFootprint {
    pub fn bytes(self) -> Option<usize> {
        self.wanted
            .iter()
            .chain(&self.protected)
            .try_fold(0_usize, |sum, item| sum.checked_add(item.bytes()?))
    }
}

/// No default policy/cap: caller chooses a finite per-role extent limit no
/// larger than this endpoint's existing cache ceiling. Control constructs and
/// reclaims the owner; callback service only borrows preallocated data. No Clone.
pub struct PageReadiness {
    endpoint: VerifiedPageEndpoint,
    roles: [RoleSlot; 3],
    max_extents: usize,
}

pub struct ReadinessSetupFailure {
    endpoint: Box<VerifiedPageEndpoint>,
    reason: ReadinessRefusal,
    observation: ReadinessAllocationFootprint,
}
impl std::fmt::Debug for ReadinessSetupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadinessSetupFailure")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
impl ReadinessSetupFailure {
    pub fn into_parts(
        self,
    ) -> (
        VerifiedPageEndpoint,
        ReadinessRefusal,
        ReadinessAllocationFootprint,
    ) {
        (*self.endpoint, self.reason, self.observation)
    }
}

impl PageReadiness {
    pub fn new(
        endpoint: VerifiedPageEndpoint,
        max_extents: usize,
    ) -> Result<Self, ReadinessSetupFailure> {
        Self::new_with_reserve_check(endpoint, max_extents, || Ok(()))
    }

    fn new_with_reserve_check(
        endpoint: VerifiedPageEndpoint,
        max_extents: usize,
        mut before_reserve: impl FnMut() -> Result<(), ()>,
    ) -> Result<Self, ReadinessSetupFailure> {
        let mut roles: [RoleSlot; 3] = std::array::from_fn(|_| RoleSlot::default());
        let mut observation = ReadinessAllocationFootprint::default();
        let reason = if max_extents == 0
            || max_extents > endpoint.allocation_footprint().endpoint.max_cached_pages
        {
            Some(ReadinessRefusal::Capacity)
        } else {
            let mut failure = None;
            for (index, role) in roles.iter_mut().enumerate() {
                if before_reserve().is_err() {
                    failure = Some(ReadinessRefusal::Allocation);
                    break;
                }
                let reserved = role.wanted.try_reserve_exact(max_extents);
                observation.wanted[index] =
                    AllocationCapacity::of::<PageExtent>(role.wanted.capacity());
                if reserved.is_err() {
                    failure = Some(ReadinessRefusal::Allocation);
                    break;
                }
                if before_reserve().is_err() {
                    failure = Some(ReadinessRefusal::Allocation);
                    break;
                }
                let reserved = role.protected.try_reserve_exact(max_extents);
                observation.protected[index] =
                    AllocationCapacity::of::<PageExtent>(role.protected.capacity());
                if reserved.is_err() {
                    failure = Some(ReadinessRefusal::Allocation);
                    break;
                }
            }
            failure
        };
        if let Some(reason) = reason {
            return Err(ReadinessSetupFailure {
                endpoint: Box::new(endpoint),
                reason,
                observation,
            });
        }
        Ok(Self {
            endpoint,
            roles,
            max_extents,
        })
    }

    pub fn allocation_footprint(&self) -> ReadinessAllocationFootprint {
        ReadinessAllocationFootprint {
            wanted: std::array::from_fn(|index| {
                AllocationCapacity::of::<PageExtent>(self.roles[index].wanted.capacity())
            }),
            protected: std::array::from_fn(|index| {
                AllocationCapacity::of::<PageExtent>(self.roles[index].protected.capacity())
            }),
        }
    }
    pub fn stream_footprint(&self) -> StreamAllocationFootprint {
        self.endpoint.allocation_footprint()
    }
    /// Reserved stream data plus endpoint-exclusive readiness vectors. Caller
    /// manifests/graphs/inline object placement and opaque overhead excluded.
    pub fn retained_data_bytes(&self) -> Option<usize> {
        self.stream_footprint()
            .retained_bytes()?
            .checked_add(self.allocation_footprint().bytes()?)
    }
    pub fn catalog(&self) -> &CatalogLease {
        self.endpoint.catalog()
    }

    /// Borrow-only submission. Refusal never changes owners, protections,
    /// epoch high-water mark, or an earlier unread receipt. Same exact pending
    /// submission coalesces; changed tokens/extents do not.
    pub fn submit(&mut self, role: ProtectionRole, manifest: &WantedPageManifest) -> IntentStatus {
        if !self.catalog().same_catalog(manifest.catalog()) {
            return IntentStatus::Refused(ReadinessRefusal::WrongCatalog);
        }
        let slot = &self.roles[role.index()];
        if let Some(stamp) = slot.pending {
            return if stamp == manifest.stamp() && slot.wanted == manifest.extents() {
                IntentStatus::Pending
            } else {
                IntentStatus::Refused(ReadinessRefusal::IntentPending)
            };
        }
        if slot.receipt_reserved {
            return IntentStatus::Refused(ReadinessRefusal::ReceiptPending);
        }
        let last_epoch = self.endpoint.pages().readiness_endpoint().readiness_epoch();
        if last_epoch == u64::MAX {
            return IntentStatus::Refused(ReadinessRefusal::EpochExhausted);
        }
        if manifest.stamp().request_epoch <= last_epoch {
            return IntentStatus::Refused(ReadinessRefusal::StaleEpoch);
        }
        if manifest.extents().len() > self.max_extents || !self.union_fits(manifest.extents()) {
            return IntentStatus::Refused(ReadinessRefusal::Capacity);
        }
        let slot = &mut self.roles[role.index()];
        slot.wanted.clear();
        slot.wanted.extend_from_slice(manifest.extents());
        slot.pending = Some(manifest.stamp());
        slot.receipt_reserved = true;
        self.endpoint
            .pages()
            .readiness_endpoint()
            .commit_readiness_epoch(manifest.stamp().request_epoch);
        IntentStatus::Accepted
    }

    // Conservative ownership overlap includes old protection until commit and
    // all pending roles. Exact distinct-page union, not sum of duplicated keys.
    // Work is bounded by six role arrays and the endpoint's existing <=1023 cap.
    fn union_fits(&self, extra: &[PageExtent]) -> bool {
        let sets = [
            self.roles[0].protected.as_slice(),
            self.roles[1].protected.as_slice(),
            self.roles[2].protected.as_slice(),
            self.roles[0].wanted.as_slice(),
            self.roles[1].wanted.as_slice(),
            self.roles[2].wanted.as_slice(),
            extra,
        ];
        let mut unique = 0;
        let ceiling = self.stream_footprint().endpoint.max_cached_pages;
        for (set_index, set) in sets.iter().enumerate() {
            for extent in *set {
                let key = (extent.key.source, extent.key.page_index);
                if sets[..set_index].iter().any(|previous| {
                    previous
                        .binary_search_by_key(&key, |item| (item.key.source, item.key.page_index))
                        .is_ok()
                }) {
                    continue;
                }
                unique += 1;
                if unique > ceiling {
                    return false;
                }
            }
        }
        true
    }

    /// The sole callback page consumer/producer does bounded pumping/request
    /// work. Caller supplies its current exact window tokens. A stale window
    /// never installs; explicit cancellation/retry remains caller-owned.
    pub fn service(
        &mut self,
        role: ProtectionRole,
        current: ReadinessStamp,
        pump_limit: usize,
        request_limit: usize,
    ) -> ReadinessProgress {
        let slot = &mut self.roles[role.index()];
        if slot.pending != Some(current) {
            return ReadinessProgress::Refused(ReadinessRefusal::StaleWindow);
        }
        self.endpoint.pages().pump_ready(pump_limit);
        let pages = self.endpoint.pages().readiness_endpoint();
        if pages.install_role(role.mask(), &slot.wanted) {
            slot.protected.clear();
            slot.protected.extend_from_slice(&slot.wanted);
            slot.wanted.clear();
            slot.installed = Some(current);
            slot.pending = None;
            slot.unread_receipt = true;
            return ReadinessProgress::InstalledProtected;
        }
        let mut progress = RequestProgress::default();
        let mut attempted = 0;
        for extent in &slot.wanted {
            if pages.extent_resident(*extent) {
                continue;
            }
            if attempted == request_limit {
                break;
            }
            attempted += 1;
            match pages.queue_request(extent.key) {
                PageRequestStatus::Accepted => progress.accepted += 1,
                PageRequestStatus::Pending => progress.pending += 1,
                PageRequestStatus::Refused => progress.refused += 1,
            }
        }
        ReadinessProgress::Pending(progress)
    }

    /// Number of decoded buffers awaiting this sole consumer, not a manifest
    /// readiness acknowledgment. Reading this counter installs/protects nothing.
    pub fn decoded_pages_waiting(&mut self) -> usize {
        self.endpoint
            .pages()
            .readiness_endpoint()
            .decoded_pages_waiting()
    }

    /// Consume the sticky acknowledgment only for the exact current manifest.
    /// A wrong/stale acknowledgment leaves its reserved slot intact.
    pub fn acknowledge(
        &mut self,
        role: ProtectionRole,
        manifest: &WantedPageManifest,
    ) -> Option<InstalledProtectedReceipt<'_>> {
        if !self.catalog().same_catalog(manifest.catalog()) {
            return None;
        }
        let slot = &mut self.roles[role.index()];
        if !slot.unread_receipt
            || slot.installed != Some(manifest.stamp())
            || slot.protected != manifest.extents()
        {
            return None;
        }
        slot.unread_receipt = false;
        slot.receipt_reserved = false;
        Some(InstalledProtectedReceipt {
            catalog: self.endpoint.catalog(),
            stamp: manifest.stamp(),
            role,
        })
    }

    pub fn installed(&self, role: ProtectionRole, manifest: &WantedPageManifest) -> bool {
        let slot = &self.roles[role.index()];
        self.catalog().same_catalog(manifest.catalog())
            && slot.installed == Some(manifest.stamp())
            && slot.protected == manifest.extents()
    }

    /// Cancellation must name the exact role/request epoch. A stale cancel
    /// cannot release a newer role. Other roles and legacy pins are untouched.
    pub fn cancel(&mut self, role: ProtectionRole, epoch: u64) -> bool {
        let slot = &mut self.roles[role.index()];
        // Pending replacement may coexist with older installed protection:
        // cancel that replacement only; retain the old installed role.
        if slot
            .pending
            .is_some_and(|stamp| stamp.request_epoch == epoch)
        {
            slot.pending = None;
            slot.wanted.clear();
            slot.receipt_reserved = false;
        } else if slot
            .installed
            .is_some_and(|stamp| stamp.request_epoch == epoch)
        {
            self.endpoint
                .pages()
                .readiness_endpoint()
                .release_role(role.mask());
            slot.protected.clear();
            slot.installed = None;
            slot.unread_receipt = false;
            // A newer pending replacement still owns its reserved receipt.
            if slot.pending.is_none() {
                slot.receipt_reserved = false;
            }
        } else {
            return false;
        }
        true
    }

    /// Borrow existing pages; pumping/legacy unpin cannot clear role protection.
    /// Does not enforce engine page-demand coverage; production integration is
    /// intentionally absent. No callbacks retain borrowed observers/pages.
    pub fn pages(&mut self) -> &mut crate::StreamCallbackPages {
        self.endpoint.pages()
    }

    /// Control-only recovery/reclamation; returns the original endpoint owner.
    /// Vec storage is released here, never from callback service/cancellation.
    pub fn into_endpoint(mut self) -> VerifiedPageEndpoint {
        for role in [
            ProtectionRole::Active,
            ProtectionRole::Candidate,
            ProtectionRole::Transition,
        ] {
            self.endpoint
                .pages()
                .readiness_endpoint()
                .release_role(role.mask());
        }
        self.endpoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StreamConfig, StreamRuntime};
    use std::time::Duration;

    #[test]
    fn every_readiness_reservation_refusal_recovers_the_original_pair() {
        for failure in 0..6 {
            let mut runtime =
                StreamRuntime::new(StreamConfig::with_pool_pages(4), Vec::new()).unwrap();
            runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
            let (control, pages) = runtime.split().ok().unwrap();
            let catalog = control.catalog_lease().unwrap();
            let witness = control.catalog_lease().unwrap();
            let endpoint = VerifiedPageEndpoint::new(pages, catalog).unwrap();
            let original = endpoint.allocation_footprint();
            let mut call = 0;
            let error = PageReadiness::new_with_reserve_check(endpoint, 3, || {
                let index = call;
                call += 1;
                if index == failure { Err(()) } else { Ok(()) }
            })
            .err()
            .unwrap();
            let (endpoint, reason, observed) = error.into_parts();
            assert_eq!(reason, ReadinessRefusal::Allocation);
            assert_eq!(call, failure + 1);
            assert!(endpoint.catalog().same_catalog(&witness));
            assert_eq!(endpoint.allocation_footprint(), original);
            let reservations = observed
                .wanted
                .iter()
                .zip(observed.protected)
                .flat_map(|(wanted, protected)| [*wanted, protected])
                .collect::<Vec<_>>();
            assert_eq!(
                reservations
                    .iter()
                    .filter(|value| value.capacity != 0)
                    .count(),
                failure
            );
            assert!(observed.bytes().is_some());
            let (pages, catalog) = endpoint.into_parts();
            drop((catalog, witness));
            assert!(control.shutdown(pages).unwrap().accounting_complete);
        }
    }

    #[test]
    fn readiness_capacity_reporting_checks_actual_capacities_and_overflow() {
        let mut runtime = StreamRuntime::new(StreamConfig::with_pool_pages(4), Vec::new()).unwrap();
        runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
        let (control, pages) = runtime.split().ok().unwrap();
        let endpoint = VerifiedPageEndpoint::new(pages, control.catalog_lease().unwrap()).unwrap();
        let error = PageReadiness::new(endpoint, 4).err().unwrap();
        let (endpoint, reason, observation) = error.into_parts();
        assert_eq!(reason, ReadinessRefusal::Capacity);
        assert_eq!(observation.bytes(), Some(0));
        let readiness = PageReadiness::new(endpoint, 3).unwrap();
        let report = readiness.allocation_footprint();
        eprintln!(
            "readiness fixture capacity: {report:?}; metadata_bytes={:?}; stream_reserved_data_bytes={:?}; combined_reserved_data_bytes={:?}",
            report.bytes(),
            readiness.stream_footprint().retained_bytes(),
            readiness.retained_data_bytes()
        );
        for (index, role) in readiness.roles.iter().enumerate() {
            assert_eq!(report.wanted[index].capacity, role.wanted.capacity());
            assert_eq!(report.protected[index].capacity, role.protected.capacity());
        }
        assert_eq!(
            readiness.retained_data_bytes(),
            readiness
                .stream_footprint()
                .retained_bytes()
                .unwrap()
                .checked_add(report.bytes().unwrap())
        );
        let mut overflow = report;
        overflow.wanted[0].capacity = usize::MAX;
        assert_eq!(overflow.bytes(), None);
        overflow = ReadinessAllocationFootprint::default();
        overflow.wanted[0] = AllocationCapacity {
            capacity: usize::MAX,
            element_bytes: 1,
        };
        overflow.protected[0] = AllocationCapacity {
            capacity: 1,
            element_bytes: 1,
        };
        assert_eq!(overflow.bytes(), None);
        let (pages, catalog) = readiness.into_endpoint().into_parts();
        drop(catalog);
        assert!(control.shutdown(pages).unwrap().accounting_complete);
    }
}
