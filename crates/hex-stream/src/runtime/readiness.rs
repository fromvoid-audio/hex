//! Exact-pair installed/protected extents, separate from engine demand or publication.
//!
//! A readiness receipt covers the actual endpoint/catalog incarnation and exact engine-generated
//! chunk extents, including short final pages. Decoded or queued PCM alone is not readiness.
//! Active, Candidate and Transition roles retain preallocated wanted/protected buffers; promotion
//! prevalidates every affected role before the nonfallible commit. Stale epochs/windows and
//! capacity refusal leave active protection intact. Native callers buffer only the uncommitted
//! remainder on missing readiness; already committed chunks and their clocks remain authoritative.
//! Complete pages for a pending intent are held separately from installed-role protection until
//! its atomic receipt or cancellation; a partially arrived set is never exposed as ready.
use super::*;
use crate::{StreamPageExtent, StreamResourceExtent};
use hex_engine::StreamResourceKey;

#[derive(Clone, Copy)]
enum ExtentSlice<'a> {
    Audio(&'a [StreamPageExtent]),
    Resources(&'a [StreamResourceExtent]),
}
impl ExtentSlice<'_> {
    fn len(self) -> usize {
        match self {
            Self::Audio(s) => s.len(),
            Self::Resources(s) => s.len(),
        }
    }
    fn is_empty(self) -> bool {
        self.len() == 0
    }
    fn get(self, index: usize) -> Option<StreamResourceExtent> {
        match self {
            Self::Audio(s) => s.get(index).copied().map(Into::into),
            Self::Resources(s) => s.get(index).copied(),
        }
    }
    fn iter(self) -> impl Iterator<Item = StreamResourceExtent> {
        (0..self.len()).filter_map(move |index| self.get(index))
    }
}
fn extent_eq(actual: &[StreamResourceExtent], expected: ExtentSlice<'_>) -> bool {
    actual.iter().copied().eq(expected.iter())
}

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
    fn pending_mask(self) -> u8 {
        1 << (self.index() + 4)
    }
}
/// Nonowning identity for page-intent correlation. Neither variant grants graph,
/// endpoint or source custody. Ordinary publication numbers are scoped by the
/// actual device Root epoch in ReadinessStamp; zero is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessArtifact {
    Constructor(hex_engine::AudioCandidateIdentity),
    Ordinary { publication: u64 },
}
impl From<hex_engine::AudioCandidateIdentity> for ReadinessArtifact {
    fn from(identity: hex_engine::AudioCandidateIdentity) -> Self {
        Self::Constructor(identity)
    }
}
impl ReadinessArtifact {
    fn valid(self) -> bool {
        !matches!(self, Self::Ordinary { publication: 0 })
    }
}
/// Correlation fields only. Native authorization additionally borrows the actual
/// musical owner, covering Root/domain and exact engine-generated extents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadinessStamp {
    pub request_epoch: u64,
    pub artifact: ReadinessArtifact,
    pub project_generation: u64,
    pub editor_revision: u64,
    pub device_epoch: u64,
    pub device_rate: u32,
    pub predecessor: Option<ReadinessArtifact>,
    pub admitted_command_prefix: u64,
    pub window: hex_engine::StreamChunkWindow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessRefusal {
    WrongPair,
    InvalidExtents,
    Capacity,
    StaleEpoch,
    IntentPending,
    ReceiptPending,
    StaleWindow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessProgress {
    Pending,
    InstalledProtected,
}
/// Finite caller reservation, bounded further by the actual endpoint cache.
#[derive(Clone, Copy, Debug)]
pub struct StreamReadinessReservation {
    pub extents_per_role: usize,
}
#[derive(Default)]
struct Role {
    wanted: Vec<StreamResourceExtent>,
    protected: Vec<StreamResourceExtent>,
    pending: Option<ReadinessStamp>,
    installed: Option<ReadinessStamp>,
    unread: bool,
}
pub(super) struct PreparedReadiness {
    catalog: CatalogLease,
    roles: [Role; 3],
    limit: usize,
    epoch: u64,
}
impl PreparedReadiness {
    pub(super) fn try_new(
        catalog: &CatalogLease,
        pages: &StreamCallbackPages,
        reservation: StreamReadinessReservation,
    ) -> Result<Self, AudioStreamStorageError> {
        if !pages.matches_catalog(catalog) {
            return Err(AudioStreamStorageError::PairMismatch);
        }
        if reservation.extents_per_role == 0
            || reservation.extents_per_role > pages.endpoint.cache_capacity()
        {
            return Err(AudioStreamStorageError::ReservationExceeded);
        }
        let mut roles: [Role; 3] = std::array::from_fn(|_| Role::default());
        for role in &mut roles {
            role.wanted
                .try_reserve_exact(reservation.extents_per_role)
                .map_err(|_| AudioStreamStorageError::Capacity)?;
            role.protected
                .try_reserve_exact(reservation.extents_per_role)
                .map_err(|_| AudioStreamStorageError::Capacity)?;
        }
        Ok(Self {
            catalog: catalog.clone(),
            roles,
            limit: reservation.extents_per_role,
            epoch: 0,
        })
    }
    pub(super) fn backing_bytes(&self) -> Option<usize> {
        self.roles.iter().try_fold(0usize, |bytes, role| {
            role.wanted
                .capacity()
                .checked_add(role.protected.capacity())?
                .checked_mul(std::mem::size_of::<StreamResourceExtent>())?
                .checked_add(bytes)
        })
    }
    fn valid(&self, stamp: ReadinessStamp, extents: ExtentSlice<'_>) -> bool {
        if !stamp.artifact.valid()
            || stamp.predecessor.is_some_and(|identity| !identity.valid())
            || stamp.request_epoch == 0
            || stamp.device_epoch == 0
            || stamp.device_rate == 0
            || stamp
                .window
                .performance_frame
                .checked_add(stamp.window.frames as u64)
                .is_none()
            || stamp
                .window
                .arrangement_frame
                .checked_add(stamp.window.frames as u64)
                .is_none()
            || (stamp.window.frames == 0 && !extents.is_empty())
            || extents.len() > self.limit
            || extents
                .iter()
                .zip(extents.iter().skip(1))
                .any(|(a, b)| (a.source, a.page_index) >= (b.source, b.page_index))
        {
            return false;
        }
        extents.iter().all(|extent| {
            let frame_end = extent
                .page_index
                .checked_mul(hex_engine::STREAM_PAGE_FRAMES as u64)
                .and_then(|start| start.checked_add(extent.end as u64));
            let frame_start = extent
                .page_index
                .checked_mul(hex_engine::STREAM_PAGE_FRAMES as u64)
                .and_then(|start| start.checked_add(extent.start as u64));
            let metadata = match extent.source {
                StreamResourceKey::Audio(key) => self
                    .catalog
                    .catalog
                    .sources
                    .binary_search_by_key(&key, hex_engine::StreamedSourceMetadata::key)
                    .ok()
                    .and_then(|index| self.catalog.catalog.sources.get(index))
                    .map(|source| (source.project_rate(), 0, source.playback_frames())),
                StreamResourceKey::Scalar(key) => self
                    .catalog
                    .catalog
                    .scalar_sources
                    .binary_search_by_key(&key, hex_engine::ScalarSourceMetadata::key)
                    .ok()
                    .and_then(|index| self.catalog.catalog.scalar_sources.get(index))
                    .map(|source| {
                        (
                            source.project_rate(),
                            source.start_frame(),
                            source.end_frame(),
                        )
                    }),
            };
            metadata.is_some_and(|(rate, start, end)| {
                extent.start < extent.end
                    && extent.end <= hex_engine::STREAM_PAGE_FRAMES
                    && rate == stamp.device_rate
                    && frame_start.is_some_and(|value| value >= start)
                    && frame_end.is_some_and(|value| value <= end)
            })
        })
    }
    /// Count distinct pages among retained protections and outstanding intents,
    /// including the new request. No allocation or synthetic pool enlargement.
    fn fits_pool(&self, replacement: usize, extents: ExtentSlice<'_>, capacity: usize) -> bool {
        let slices = [
            ExtentSlice::Resources(self.roles[0].protected.as_slice()),
            ExtentSlice::Resources(self.roles[1].protected.as_slice()),
            ExtentSlice::Resources(self.roles[2].protected.as_slice()),
            if replacement == 0 {
                ExtentSlice::Resources(&[])
            } else {
                ExtentSlice::Resources(self.roles[0].wanted.as_slice())
            },
            if replacement == 1 {
                ExtentSlice::Resources(&[])
            } else {
                ExtentSlice::Resources(self.roles[1].wanted.as_slice())
            },
            if replacement == 2 {
                ExtentSlice::Resources(&[])
            } else {
                ExtentSlice::Resources(self.roles[2].wanted.as_slice())
            },
            extents,
        ];
        let mut cursors = [0usize; 7];
        let mut count = 0usize;
        while let Some(key) = slices
            .iter()
            .zip(cursors)
            .filter_map(|(slice, cursor)| slice.get(cursor))
            .map(|extent| (extent.source, extent.page_index))
            .min()
        {
            count += 1;
            if count > capacity {
                return false;
            }
            for (slice, cursor) in slices.iter().zip(&mut cursors) {
                while slice
                    .get(*cursor)
                    .is_some_and(|extent| (extent.source, extent.page_index) == key)
                {
                    *cursor += 1;
                }
            }
        }
        true
    }
    fn submit(
        &mut self,
        pages: &StreamCallbackPages,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> Result<(), ReadinessRefusal> {
        if !pages.matches_catalog(&self.catalog) {
            return Err(ReadinessRefusal::WrongPair);
        }
        if !self.valid(stamp, extents) {
            return Err(ReadinessRefusal::InvalidExtents);
        }
        let index = role.index();
        let slot = &self.roles[index];
        if slot.unread {
            return Err(ReadinessRefusal::ReceiptPending);
        }
        if let Some(old) = slot.pending {
            return if old == stamp && extent_eq(&slot.wanted, extents) {
                Ok(())
            } else {
                Err(ReadinessRefusal::IntentPending)
            };
        }
        if stamp.request_epoch <= self.epoch {
            return Err(ReadinessRefusal::StaleEpoch);
        }
        if !self.fits_pool(index, extents, pages.endpoint.cache_capacity()) {
            return Err(ReadinessRefusal::Capacity);
        }
        let slot = &mut self.roles[index];
        slot.wanted.clear();
        slot.wanted.extend(extents.iter());
        slot.pending = Some(stamp);
        self.epoch = stamp.request_epoch;
        Ok(())
    }
    pub(super) fn service(
        &mut self,
        pages: &mut StreamCallbackPages,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        pump_limit: usize,
        request_limit: usize,
    ) -> Result<ReadinessProgress, ReadinessRefusal> {
        if !pages.matches_catalog(&self.catalog) {
            return Err(ReadinessRefusal::WrongPair);
        }
        if self.roles[role.index()].pending != Some(stamp) {
            return Err(ReadinessRefusal::StaleWindow);
        }
        // Pending pins protect only complete extents already present. They do
        // not publish a receipt or replace any installed role. Mark new pages
        // during the same bounded pump, before a later insertion can evict them.
        let roles = [
            ProtectionRole::Active,
            ProtectionRole::Candidate,
            ProtectionRole::Transition,
        ];
        for current in roles {
            let slot = &self.roles[current.index()];
            if slot.pending.is_some() {
                pages
                    .endpoint
                    .protect_present(current.pending_mask(), &slot.wanted);
            }
        }
        let pending = roles.map(|current| {
            let slot = &self.roles[current.index()];
            (
                current.pending_mask(),
                if slot.pending.is_some() {
                    slot.wanted.as_slice()
                } else {
                    &[]
                },
            )
        });
        pages
            .endpoint
            .pump_ready_for_pending(pump_limit.min(pages.max_ready_pages_per_pump), &pending);
        let slot = &mut self.roles[role.index()];
        let missing = slot
            .wanted
            .iter()
            .filter(|extent| !pages.endpoint.has_extent(**extent))
            .count();
        // A full cache otherwise feeds the worker one returned page per pump.
        // Release only stale, unprotected victims before submitting demand.
        pages.endpoint.reclaim_for_missing(missing, request_limit);
        let mut requests = 0;
        let mut complete = true;
        for extent in &slot.wanted {
            if !pages.endpoint.has_extent(*extent) {
                complete = false;
                if requests < request_limit {
                    match extent.source {
                        StreamResourceKey::Audio(key) => pages.request_page(key, extent.page_index),
                        StreamResourceKey::Scalar(key) => {
                            pages.request_scalar_page(key, extent.page_index)
                        }
                    };
                    requests += 1;
                }
            }
        }
        if !complete {
            return Ok(ReadinessProgress::Pending);
        }
        // All validation precedes the role mask/backing/stamp commit. Other role
        // protections survive this replacement, including overlapping pages.
        pages.endpoint.replace_protection(role.mask(), &slot.wanted);
        pages.endpoint.replace_protection(role.pending_mask(), &[]);
        slot.protected.clear();
        slot.protected.extend_from_slice(&slot.wanted);
        slot.installed = Some(stamp);
        slot.pending = None;
        slot.unread = true;
        Ok(ReadinessProgress::InstalledProtected)
    }
    fn acknowledge(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> bool {
        let slot = &mut self.roles[role.index()];
        if !slot.unread || slot.installed != Some(stamp) || !extent_eq(&slot.protected, extents) {
            return false;
        }
        slot.unread = false;
        true
    }
    fn installed(
        &self,
        pages: &StreamCallbackPages,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> bool {
        let slot = &self.roles[role.index()];
        pages.matches_catalog(&self.catalog)
            && !slot.unread
            && slot.installed == Some(stamp)
            && extent_eq(&slot.protected, extents)
            && extents
                .iter()
                .all(|extent| pages.endpoint.has_extent(extent))
    }
    pub(super) fn cancel(
        &mut self,
        pages: &mut StreamCallbackPages,
        role: ProtectionRole,
        epoch: u64,
    ) -> bool {
        let slot = &mut self.roles[role.index()];
        if slot
            .pending
            .or(slot.installed)
            .is_none_or(|stamp| stamp.request_epoch != epoch)
        {
            return false;
        }
        slot.pending = None;
        slot.installed = None;
        slot.unread = false;
        slot.wanted.clear();
        slot.protected.clear();
        pages.endpoint.replace_protection(role.mask(), &[]);
        pages.endpoint.replace_protection(role.pending_mask(), &[]);
        true
    }
}

/// One actual factory endpoint, moved once into the participating callback's
/// domain slot. Generations borrow it; none receive another request producer.
/// Return it to its constructor domain before normal joining disposal. Dropping
/// this owning capsule is control-only, like StreamCallbackPages itself.
pub struct ConstructorPageLoan {
    pub(super) pages: StreamCallbackPages,
    pub(super) readiness: PreparedReadiness,
}
/// Exact protected borrowed samples. No endpoint ownership, pin mutation,
/// scheduling, catalog clone or disposal operation is exposed.
pub struct ProtectedStreamPages<'a> {
    pages: &'a mut StreamCallbackPages,
    extents: &'a [StreamResourceExtent],
}
impl MediaPages for ProtectedStreamPages<'_> {
    fn page(
        &mut self,
        key: StreamSourceKey,
        index: u64,
    ) -> Option<hex_engine::BorrowedMediaPage<'_>> {
        self.extents
            .iter()
            .any(|extent| {
                extent.source == StreamResourceKey::Audio(key) && extent.page_index == index
            })
            .then_some(())?;
        self.pages.page(key, index)
    }
    fn scalar_page(
        &mut self,
        key: hex_engine::ScalarSourceKey,
        index: u64,
    ) -> Option<hex_engine::BorrowedScalarPage<'_>> {
        self.extents
            .iter()
            .any(|extent| {
                extent.source == StreamResourceKey::Scalar(key) && extent.page_index == index
            })
            .then_some(())?;
        self.pages.scalar_page(key, index)
    }
    fn request_scalar_page(&mut self, _key: hex_engine::ScalarSourceKey, _index: u64) {}
    fn request_page(&mut self, _key: StreamSourceKey, _index: u64) {
        // This borrow contains already protected pages only. Actual missing
        // demand must return to readiness service, never schedule from render.
    }
    fn prefetch_page(&mut self, key: StreamSourceKey, index: u64) {
        // Bounded speculative lookahead only (`Endpoint::prefetch`): it never
        // pins, never satisfies or changes readiness, and the page it may
        // load stays evictable, so readiness keeps sole custody of demand.
        self.pages.prefetch_page(key, index);
    }
}
/// Exclusive borrowed promotion prepared against exact held protection. It
/// owns no endpoint and grants no engine/publication authority; dropping it has
/// no effect. The exclusive borrow prevents changes before its fixed commit.
pub struct PreparedRolePromotion<'a> {
    loan: &'a mut ConstructorPageLoan,
    from: ProtectionRole,
}
impl PreparedRolePromotion<'_> {
    pub fn pages(&mut self) -> ProtectedStreamPages<'_> {
        ProtectedStreamPages {
            pages: &mut self.loan.pages,
            extents: &self.loan.readiness.roles[self.from.index()].protected,
        }
    }
    pub fn commit(self) {
        self.loan.commit_promotion(self.from);
    }
}
impl ConstructorPageLoan {
    pub fn submit(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> Result<(), ReadinessRefusal> {
        self.submit_view(role, stamp, ExtentSlice::Audio(extents))
    }
    pub fn submit_resources(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamResourceExtent],
    ) -> Result<(), ReadinessRefusal> {
        self.submit_view(role, stamp, ExtentSlice::Resources(extents))
    }
    pub fn acknowledge(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> bool {
        self.acknowledge_view(role, stamp, ExtentSlice::Audio(extents))
    }
    pub fn acknowledge_resources(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamResourceExtent],
    ) -> bool {
        self.acknowledge_view(role, stamp, ExtentSlice::Resources(extents))
    }
    pub fn protected_pages(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> Option<ProtectedStreamPages<'_>> {
        self.protected_pages_view(role, stamp, ExtentSlice::Audio(extents))
    }
    pub fn protected_resource_pages(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamResourceExtent],
    ) -> Option<ProtectedStreamPages<'_>> {
        self.protected_pages_view(role, stamp, ExtentSlice::Resources(extents))
    }
    pub fn matches_intent(
        &self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> bool {
        self.matches_intent_view(role, stamp, ExtentSlice::Audio(extents))
    }
    pub fn matches_resource_intent(
        &self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamResourceExtent],
    ) -> bool {
        self.matches_intent_view(role, stamp, ExtentSlice::Resources(extents))
    }
    pub fn is_protected(
        &self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> bool {
        self.is_protected_view(role, stamp, ExtentSlice::Audio(extents))
    }
    pub fn is_resource_protected(
        &self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamResourceExtent],
    ) -> bool {
        self.is_protected_view(role, stamp, ExtentSlice::Resources(extents))
    }
    pub fn can_promote(
        &self,
        from: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> bool {
        self.can_promote_view(from, stamp, ExtentSlice::Audio(extents))
    }
    pub fn can_promote_resources(
        &self,
        from: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamResourceExtent],
    ) -> bool {
        self.can_promote_view(from, stamp, ExtentSlice::Resources(extents))
    }
    pub fn prepare_promotion(
        &mut self,
        from: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> Option<PreparedRolePromotion<'_>> {
        self.prepare_promotion_view(from, stamp, ExtentSlice::Audio(extents))
    }
    pub fn prepare_resource_promotion(
        &mut self,
        from: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamResourceExtent],
    ) -> Option<PreparedRolePromotion<'_>> {
        self.prepare_promotion_view(from, stamp, ExtentSlice::Resources(extents))
    }
    pub fn promote_to_active(
        &mut self,
        from: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> bool {
        self.promote_to_active_view(from, stamp, ExtentSlice::Audio(extents))
    }
    pub fn promote_resources_to_active(
        &mut self,
        from: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamResourceExtent],
    ) -> bool {
        self.promote_to_active_view(from, stamp, ExtentSlice::Resources(extents))
    }

    fn submit_view(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> Result<(), ReadinessRefusal> {
        self.readiness.submit(&self.pages, role, stamp, extents)
    }
    pub fn service(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        pump_limit: usize,
        request_limit: usize,
    ) -> Result<ReadinessProgress, ReadinessRefusal> {
        self.readiness
            .service(&mut self.pages, role, stamp, pump_limit, request_limit)
    }
    /// Test-only observation of a resident extent; it does not grant protection.
    #[cfg(feature = "test-support")]
    pub fn test_has_resource_extent(&self, extent: StreamResourceExtent) -> bool {
        self.pages.endpoint.has_extent(extent)
    }
    /// Test-only observation of the next worker-ready page; it grants no custody.
    #[cfg(feature = "test-support")]
    pub fn test_ready_page(&self, key: crate::ResourcePageKey) -> bool {
        self.pages.endpoint.test_ready_page(key)
    }
    fn acknowledge_view(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> bool {
        self.readiness.acknowledge(role, stamp, extents)
    }
    fn protected_pages_view(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> Option<ProtectedStreamPages<'_>> {
        self.readiness
            .installed(&self.pages, role, stamp, extents)
            .then_some(())?;
        Some(ProtectedStreamPages {
            pages: &mut self.pages,
            extents: &self.readiness.roles[role.index()].protected,
        })
    }
    /// Pure exact intent observation; neither a receipt nor engine demand proof.
    fn matches_intent_view(
        &self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> bool {
        let slot = &self.readiness.roles[role.index()];
        (slot.pending == Some(stamp) && extent_eq(&slot.wanted, extents))
            || (slot.installed == Some(stamp) && extent_eq(&slot.protected, extents))
    }
    fn is_protected_view(
        &self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> bool {
        self.readiness.installed(&self.pages, role, stamp, extents)
    }
    /// After the host's actual musical commit, move an already acknowledged
    /// held role to Active without allocating, cloning or scheduling. A host
    /// with multiple domains prevalidates every can_promote before any commit.
    fn can_promote_view(
        &self,
        from: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> bool {
        from != ProtectionRole::Active
            && self.is_protected_view(from, stamp, extents)
            && self.readiness.roles[ProtectionRole::Active.index()]
                .pending
                .is_none()
            && !self.readiness.roles[ProtectionRole::Active.index()].unread
    }
    fn prepare_promotion_view(
        &mut self,
        from: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> Option<PreparedRolePromotion<'_>> {
        self.can_promote_view(from, stamp, extents).then_some(())?;
        Some(PreparedRolePromotion { loan: self, from })
    }
    fn promote_to_active_view(
        &mut self,
        from: ProtectionRole,
        stamp: ReadinessStamp,
        extents: ExtentSlice<'_>,
    ) -> bool {
        let Some(prepared) = self.prepare_promotion_view(from, stamp, extents) else {
            return false;
        };
        prepared.commit();
        true
    }
    fn commit_promotion(&mut self, from: ProtectionRole) {
        self.readiness
            .roles
            .swap(from.index(), ProtectionRole::Active.index());
        self.pages.endpoint.replace_protection(
            ProtectionRole::Active.mask(),
            &self.readiness.roles[ProtectionRole::Active.index()].protected,
        );
        self.pages.endpoint.replace_protection(from.mask(), &[]);
        let old = &mut self.readiness.roles[from.index()];
        old.wanted.clear();
        old.protected.clear();
        old.pending = None;
        old.installed = None;
        old.unread = false;
    }
    /// Catalog membership only. The native host separately validates exact
    /// metadata through the actual retained constructor domain before splitting.
    pub fn covers_resource_key(&self, key: StreamResourceKey) -> bool {
        match key {
            StreamResourceKey::Audio(key) => self.covers_key(key),
            StreamResourceKey::Scalar(key) => self
                .readiness
                .catalog
                .catalog
                .scalar_sources
                .binary_search_by_key(&key, hex_engine::ScalarSourceMetadata::key)
                .is_ok(),
        }
    }
    pub fn covers_key(&self, key: StreamSourceKey) -> bool {
        self.readiness
            .catalog
            .catalog
            .sources
            .binary_search_by_key(&key, hex_engine::StreamedSourceMetadata::key)
            .is_ok()
    }
    pub fn covers_scalar_source(&self, source: &hex_engine::ScalarSourceMetadata) -> bool {
        self.readiness.catalog.covers_scalar_source(source)
    }
    pub fn cancel(&mut self, role: ProtectionRole, epoch: u64) -> bool {
        self.readiness.cancel(&mut self.pages, role, epoch)
    }
    pub fn covers_source(&self, source: &hex_engine::StreamedSourceMetadata) -> bool {
        self.readiness.catalog.covers_source(source)
    }
    /// Consume one actual worker failure and release only its pending request
    /// key. The caller retains/reports the bounded diagnostic; no page or role
    /// protection is dropped or certified by this observation.
    pub fn take_failure(&mut self) -> Option<StreamFailure> {
        self.pages.endpoint.take_failure()
    }
    pub fn take_failures(&mut self, output: &mut [StreamFailure]) -> usize {
        self.pages.take_failures(output)
    }
    pub fn same_catalog(&self, other: &Self) -> bool {
        self.readiness
            .catalog
            .same_catalog(&other.readiness.catalog)
    }
}

#[cfg(test)]
mod tests;
