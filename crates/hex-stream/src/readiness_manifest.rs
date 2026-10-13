//! Control-owned exact page extents and caller-supplied lifecycle identities.
use crate::{CatalogLease, PageKey};
use hex_engine::STREAM_PAGE_FRAMES;

/// One canonical page's required half-open extent, in project-rate frames.
/// Manifests require strictly sorted, unique (source, page_index) entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageExtent {
    pub key: PageKey,
    pub start: usize,
    pub end: usize,
}

/// Opaque caller identities, compared exactly, never interpreted as engine
/// demand or an ordered editor revision. The caller must bind artifact to its
/// actual prepared graph and derive the exact window from engine planning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadinessStamp {
    pub request_epoch: u64,
    pub artifact: u64,
    pub project_generation: u64,
    pub editor_revision: u64,
    pub device_epoch: u64,
    pub project_rate: u32,
    pub timeline_epoch: u64,
    pub performance_frame: u64,
    pub arrangement_frame: u64,
    pub window_frames: u64,
    pub active_revision: u64,
    pub route_generation: u64,
    pub admitted_command_prefix: u64,
}

/// Non-wrapping control-side sequence. One sequence per endpoint's intents;
/// rejected submissions may skip epochs. Exhaustion never resets identities.
#[derive(Debug, Default)]
pub struct ReadinessEpochs(u64);
impl ReadinessEpochs {
    pub fn advance(&mut self) -> Result<u64, ReadinessRefusal> {
        let next = self
            .0
            .checked_add(1)
            .ok_or(ReadinessRefusal::EpochExhausted)?;
        self.0 = next;
        Ok(next)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadinessRefusal {
    WrongCatalog,
    InvalidManifest,
    Capacity,
    Allocation,
    StaleEpoch,
    EpochExhausted,
    IntentPending,
    ReceiptPending,
    StaleWindow,
}

/// Held/reclaimed on control, never moved into callback service. Submit borrows
/// it and copies only finite extents/tokens into preallocated endpoint storage.
/// This validates catalog coverage, not that the engine actually demands these
/// pages. No native publication capability and no Clone implementation.
pub struct WantedPageManifest {
    catalog: CatalogLease,
    stamp: ReadinessStamp,
    extents: Vec<PageExtent>,
}

/// Control-side owning refusal; exact input owners/capacity remain recoverable.
pub struct ManifestRefusal {
    reason: ReadinessRefusal,
    catalog: CatalogLease,
    extents: Vec<PageExtent>,
}
impl std::fmt::Debug for ManifestRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManifestRefusal")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
impl ManifestRefusal {
    pub fn into_parts(self) -> (CatalogLease, Vec<PageExtent>, ReadinessRefusal) {
        (self.catalog, self.extents, self.reason)
    }
}

impl WantedPageManifest {
    pub fn new(
        catalog: CatalogLease,
        stamp: ReadinessStamp,
        extents: Vec<PageExtent>,
    ) -> Result<Self, ManifestRefusal> {
        if !valid_manifest(&catalog, stamp, &extents) {
            return Err(ManifestRefusal {
                reason: ReadinessRefusal::InvalidManifest,
                catalog,
                extents,
            });
        }
        Ok(Self {
            catalog,
            stamp,
            extents,
        })
    }
    pub fn catalog(&self) -> &CatalogLease {
        &self.catalog
    }
    pub fn stamp(&self) -> ReadinessStamp {
        self.stamp
    }
    pub fn extents(&self) -> &[PageExtent] {
        &self.extents
    }
    pub fn allocation_capacity(&self) -> crate::AllocationCapacity {
        crate::AllocationCapacity::of::<PageExtent>(self.extents.capacity())
    }
    pub fn into_parts(self) -> (CatalogLease, ReadinessStamp, Vec<PageExtent>) {
        (self.catalog, self.stamp, self.extents)
    }
}

fn valid_manifest(catalog: &CatalogLease, stamp: ReadinessStamp, extents: &[PageExtent]) -> bool {
    if stamp.request_epoch == 0
        || stamp.project_rate == 0
        || stamp
            .performance_frame
            .checked_add(stamp.window_frames)
            .is_none()
        || stamp
            .arrangement_frame
            .checked_add(stamp.window_frames)
            .is_none()
        || extents.windows(2).any(|pair| {
            (pair[0].key.source, pair[0].key.page_index)
                >= (pair[1].key.source, pair[1].key.page_index)
        })
    {
        return false;
    }
    extents.iter().all(|extent| {
        let Some(source) = catalog.source_metadata(extent.key.source) else {
            return false;
        };
        extent.start < extent.end
            && extent.end <= STREAM_PAGE_FRAMES
            && source.project_rate() == stamp.project_rate
            && extent
                .key
                .page_index
                .checked_mul(STREAM_PAGE_FRAMES as u64)
                .and_then(|start| start.checked_add(extent.end as u64))
                .is_some_and(|end| end <= source.playback_frames())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn epoch_exhaustion_is_sticky_and_never_wraps() {
        let mut epochs = ReadinessEpochs(u64::MAX - 1);
        assert_eq!(epochs.advance(), Ok(u64::MAX));
        assert_eq!(epochs.advance(), Err(ReadinessRefusal::EpochExhausted));
        assert_eq!(epochs.advance(), Err(ReadinessRefusal::EpochExhausted));
    }
}
