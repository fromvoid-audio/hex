//! Immutable projection of the actual media field, disjoint from the editor.
//! No Session clone, source cache, publication authority or public constructor.
use super::*;

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
        let input_bytes = hex_engine::native_device_resources::OriginalResourceInput::requested_input_metadata_bytes(assets.len())?;
        if input_bytes > reservation.max_preparation_metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(assets.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        if hex_engine::native_device_resources::OriginalResourceInput::requested_input_metadata_bytes(entries.capacity())? > reservation.max_preparation_metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        for asset in assets {
            check(cancel)?;
            entries.push(match original(*asset)? {
                OriginalAudio::Stereo(audio) => {
                    hex_engine::native_device_resources::OriginalResourceInput::stereo(
                        *asset, audio,
                    )
                }
                OriginalAudio::Mono(source) => {
                    hex_engine::native_device_resources::OriginalResourceInput::native(
                        *asset, source,
                    )
                }
            });
        }
        hex_engine::native_device_resources::OriginalResourceBankPlan::borrow(&entries)
            .prepare_on_control(reservation, cancel)
    }
}
