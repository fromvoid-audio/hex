//! Immutable projection of the actual media field, disjoint from the editor.
//! No Session clone, source cache, publication authority or public constructor.
use super::*;
use hex_engine::native_device_resources::{OriginalResourceBankPlan, OriginalResourceInput};

/// Actual borrowed source rows, not PCM owners or a freshness/admission permit.
/// The backing remains live through the affine install call and is charged by
/// the existing Engine producer using its actual Vec capacity.
pub(crate) struct ResolvedOriginalResources<'a> {
    entries: Vec<OriginalResourceInput<'a>>,
}
impl ResolvedOriginalResources<'_> {
    pub(crate) fn as_engine_plan(&self) -> OriginalResourceBankPlan<'_> {
        OriginalResourceBankPlan::borrow(&self.entries)
    }
}

pub(crate) struct ResidentResourceSources<'a> {
    media: &'a SortedEntries<AssetId, crate::ResidentAsset>,
}
impl<'a> ResidentResourceSources<'a> {
    pub(crate) fn new(media: &'a SortedEntries<AssetId, crate::ResidentAsset>) -> Self {
        Self { media }
    }
    pub(crate) fn prepare(
        &self,
        assets: &[AssetId],
        pending: Option<(&hex_project::Asset, &OriginalAudio)>,
        reservation: ResidentResourcePlanReservation,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<DeviceResourceBank, ResourcePreparationError> {
        self.resolve(
            assets,
            pending,
            reservation.max_preparation_metadata_bytes,
            cancel,
        )?
        .as_engine_plan()
        .prepare_on_control(reservation, cancel)
    }
    /// Resolve only actual retained/pending fields. No alias constructor or
    /// resource bank runs here; the caller can pass this concrete plan straight
    /// to the existing nonextracting preparation loan.
    pub(crate) fn resolve<'scope>(
        &'scope self,
        assets: &[AssetId],
        pending: Option<(&hex_project::Asset, &'scope OriginalAudio)>,
        max_preparation_metadata_bytes: usize,
        cancel: &dyn hex_engine::CancellationQuery,
    ) -> Result<ResolvedOriginalResources<'scope>, ResourcePreparationError> {
        check(cancel)?;
        if assets.len() > hex_project::MAX_ASSETS
            || assets.windows(2).any(|a| a[0].value() >= a[1].value())
        {
            return Err(ResourcePreparationError::InvalidSource);
        }
        if pending.is_some_and(|(asset, original)| {
            self.media.contains_key(&asset.id())
                || assets
                    .binary_search_by_key(&asset.id().value(), |id| id.value())
                    .is_err()
                || asset.source_sample_rate() != original.sample_rate()
                || usize::try_from(asset.source_frames()).ok() != Some(original.frames())
        }) {
            return Err(ResourcePreparationError::InvalidSource);
        }
        let original = |asset| {
            pending
                .filter(|(metadata, _)| metadata.id() == asset)
                .map(|(_, original)| original)
                .or_else(|| {
                    self.media
                        .get(&asset)
                        .map(|resident| &resident.source_audio)
                })
                .ok_or(ResourcePreparationError::InvalidSource)
        };
        let input_bytes = OriginalResourceInput::requested_input_metadata_bytes(assets.len())?;
        if input_bytes > max_preparation_metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(assets.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        if OriginalResourceInput::requested_input_metadata_bytes(entries.capacity())?
            > max_preparation_metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        for asset in assets {
            check(cancel)?;
            entries.push(match original(*asset)? {
                OriginalAudio::Stereo(audio) => OriginalResourceInput::stereo(*asset, audio),
                OriginalAudio::Mono(source) => OriginalResourceInput::native(*asset, source),
            });
        }
        Ok(ResolvedOriginalResources { entries })
    }
}
