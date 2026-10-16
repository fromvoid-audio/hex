//! Exact-pair installed/protected extents, adapted from published177 8d444c06.
//! This layer validates real cached samples, not engine demand or publication.
use super::*;
use crate::StreamPageExtent;

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
/// Correlation fields only. Native authorization additionally borrows the actual
/// constructor owner, covering Root/domain and exact engine-generated extents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadinessStamp {
    pub request_epoch: u64,
    pub artifact: hex_engine::AudioCandidateIdentity,
    pub project_generation: u64,
    pub editor_revision: u64,
    pub device_epoch: u64,
    pub device_rate: u32,
    pub predecessor: Option<hex_engine::AudioCandidateIdentity>,
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
    wanted: Vec<StreamPageExtent>,
    protected: Vec<StreamPageExtent>,
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
                .checked_mul(std::mem::size_of::<StreamPageExtent>())?
                .checked_add(bytes)
        })
    }
    fn valid(&self, stamp: ReadinessStamp, extents: &[StreamPageExtent]) -> bool {
        if stamp.request_epoch == 0
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
                .windows(2)
                .any(|w| (w[0].source, w[0].page_index) >= (w[1].source, w[1].page_index))
        {
            return false;
        }
        extents.iter().all(|extent| {
            self.catalog
                .catalog
                .sources
                .binary_search_by_key(&extent.source, hex_engine::StreamedSourceMetadata::key)
                .ok()
                .and_then(|index| self.catalog.catalog.sources.get(index))
                .is_some_and(|source| {
                    extent.start < extent.end
                        && extent.end <= hex_engine::STREAM_PAGE_FRAMES
                        && source.project_rate() == stamp.device_rate
                        && extent
                            .page_index
                            .checked_mul(hex_engine::STREAM_PAGE_FRAMES as u64)
                            .and_then(|start| start.checked_add(extent.end as u64))
                            .is_some_and(|end| end <= source.playback_frames())
                })
        })
    }
    /// Count distinct pages among retained protections and outstanding intents,
    /// including the new request. No allocation or synthetic pool enlargement.
    fn fits_pool(&self, replacement: usize, extents: &[StreamPageExtent], capacity: usize) -> bool {
        let slices = [
            self.roles[0].protected.as_slice(),
            self.roles[1].protected.as_slice(),
            self.roles[2].protected.as_slice(),
            if replacement == 0 {
                &[]
            } else {
                self.roles[0].wanted.as_slice()
            },
            if replacement == 1 {
                &[]
            } else {
                self.roles[1].wanted.as_slice()
            },
            if replacement == 2 {
                &[]
            } else {
                self.roles[2].wanted.as_slice()
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
    pub(super) fn submit(
        &mut self,
        pages: &StreamCallbackPages,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
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
            return if old == stamp && slot.wanted == extents {
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
        slot.wanted.extend_from_slice(extents);
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
        let slot = &mut self.roles[role.index()];
        if slot.pending != Some(stamp) {
            return Err(ReadinessRefusal::StaleWindow);
        }
        pages.pump_ready(pump_limit);
        let mut requests = 0;
        let mut complete = true;
        for extent in &slot.wanted {
            if !pages.endpoint.has_extent(*extent) {
                complete = false;
                if requests < request_limit {
                    pages.request_page(extent.source, extent.page_index);
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
        slot.protected.clear();
        slot.protected.extend_from_slice(&slot.wanted);
        slot.installed = Some(stamp);
        slot.pending = None;
        slot.unread = true;
        Ok(ReadinessProgress::InstalledProtected)
    }
    pub(super) fn acknowledge(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> bool {
        let slot = &mut self.roles[role.index()];
        if !slot.unread || slot.installed != Some(stamp) || slot.protected != extents {
            return false;
        }
        slot.unread = false;
        true
    }
    pub(super) fn installed(
        &self,
        pages: &StreamCallbackPages,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> bool {
        let slot = &self.roles[role.index()];
        pages.matches_catalog(&self.catalog)
            && !slot.unread
            && slot.installed == Some(stamp)
            && slot.protected == extents
            && extents
                .iter()
                .all(|extent| pages.endpoint.has_extent(*extent))
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
}
impl MediaPages for ProtectedStreamPages<'_> {
    fn page(
        &mut self,
        key: StreamSourceKey,
        index: u64,
    ) -> Option<hex_engine::BorrowedMediaPage<'_>> {
        self.pages.page(key, index)
    }
    fn request_page(&mut self, key: StreamSourceKey, index: u64) {
        self.pages.request_page(key, index);
    }
}
impl ConstructorPageLoan {
    pub fn submit(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
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
    pub fn acknowledge(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> bool {
        self.readiness.acknowledge(role, stamp, extents)
    }
    pub fn protected_pages(
        &mut self,
        role: ProtectionRole,
        stamp: ReadinessStamp,
        extents: &[StreamPageExtent],
    ) -> Option<ProtectedStreamPages<'_>> {
        self.readiness
            .installed(&self.pages, role, stamp, extents)
            .then_some(())?;
        Some(ProtectedStreamPages {
            pages: &mut self.pages,
        })
    }
    pub fn cancel(&mut self, role: ProtectionRole, epoch: u64) -> bool {
        self.readiness.cancel(&mut self.pages, role, epoch)
    }
    pub fn covers_source(&self, source: &hex_engine::StreamedSourceMetadata) -> bool {
        self.readiness.catalog.covers_source(source)
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
