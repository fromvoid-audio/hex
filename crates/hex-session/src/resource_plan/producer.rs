//! Immutable projection of the actual media field, disjoint from the editor.
//! No Session clone, source cache, publication authority or public constructor.
use super::*;
use hex_engine::{
    OwnedAudioInputs, OwnedAudioSource,
    native_device_resources::{
        DeviceResourceCatalog, OriginalResourceLayout, OriginalResourceSource,
    },
};

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
        let minimum_metadata = assets
            .len()
            .checked_mul(
                size_of::<(AssetId, OwnedAudioSource)>() + size_of::<OriginalResourceSource>(),
            )
            .and_then(|n| n.checked_add(size_of::<Vec<(AssetId, OwnedAudioSource)>>()))
            .and_then(|n| n.checked_add(size_of::<Vec<OriginalResourceSource>>()))
            .ok_or(AudioStorageError::Overflow)?;
        if minimum_metadata > reservation.max_preparation_metadata_bytes {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut sources = Vec::new();
        let mut expected = Vec::new();
        sources
            .try_reserve_exact(assets.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        expected
            .try_reserve_exact(assets.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        let source_bytes = sources
            .capacity()
            .checked_mul(size_of::<(AssetId, OwnedAudioSource)>())
            .and_then(|n| n.checked_add(size_of::<Vec<(AssetId, OwnedAudioSource)>>()))
            .ok_or(AudioStorageError::Overflow)?;
        let expected_bytes = expected
            .capacity()
            .checked_mul(size_of::<OriginalResourceSource>())
            .and_then(|n| n.checked_add(size_of::<Vec<OriginalResourceSource>>()))
            .ok_or(AudioStorageError::Overflow)?;
        if source_bytes
            .checked_add(expected_bytes)
            .ok_or(AudioStorageError::Overflow)?
            > reservation.max_preparation_metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        // Capture request facts and temporary capacities before creating aliases.
        // Source constructors check actual lineage; failure drops only new
        // unpublished control aliases. Unknown/derived Audio stays unknown.
        for asset in assets {
            check(cancel)?;
            let original = original(*asset)?;
            let layout = match original {
                OriginalAudio::Stereo(_) => OriginalResourceLayout::Stereo,
                OriginalAudio::Mono(_) => OriginalResourceLayout::Mono,
            };
            expected.push(OriginalResourceSource {
                asset: *asset,
                sample_rate: original.sample_rate(),
                frames: original.frames() as u64,
                layout,
            });
        }
        for (asset, fact) in assets.iter().zip(&expected) {
            check(cancel)?;
            let original = original(*asset)?;
            let source = match original {
                OriginalAudio::Stereo(audio) => OwnedAudioSource::alias_native_audio_on_control(
                    audio,
                    size_of::<OwnedAudioSource>(),
                )?,
                OriginalAudio::Mono(source) => {
                    source.alias_for_resource_plan_on_control(size_of::<OwnedAudioSource>())?
                }
            };
            if source.sample_rate() != fact.sample_rate || source.frames() != fact.frames {
                return Err(ResourcePreparationError::InvalidSource);
            }
            sources.push((*asset, source));
        }
        let inputs = OwnedAudioInputs::try_new(sources, reservation.inputs)
            .map_err(|refused| refused.reason)?;
        let inputs = inputs
            .prepare_input_storage_on_control()
            .map_err(|refused| refused.reason)?;
        let catalog = DeviceResourceCatalog::prepare_on_control(
            &inputs,
            &expected,
            reservation.catalog,
            cancel,
        )?;
        let peak = inputs
            .summary()
            .construction_peak_bytes
            .checked_add(catalog.metadata_bytes_on_control()?)
            .and_then(|n| n.checked_add(expected_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        if peak > reservation.max_construction_peak_bytes {
            return Err(AudioStorageError::ReservationExceeded.into());
        }
        drop(expected);
        DeviceResourceBank::try_new_on_control(
            inputs,
            catalog,
            reservation.max_retained_data_bytes,
            reservation.max_construction_peak_bytes,
            cancel,
        )
        .map_err(|refused| refused.reason)
    }
}
