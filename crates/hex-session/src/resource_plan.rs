//! Closed control-side media plans and borrowed actual PCM inventory.
//! No second source cache, publication authority or callback preparation.
use crate::{OriginalAudio, Session, service::AudioClipWaveform};
use hex_engine::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageReservation, AudioStorageRole,
    OwnedAudioInputs, OwnedAudioSource,
    native_device_resources::{
        DeviceResourceBank, DeviceResourceCatalog, OriginalResourceLayout, OriginalResourceSource,
        ResourceCatalogReservation, ResourcePreparationError,
    },
};
use hex_project::{AssetId, ClipId};
use std::{
    collections::BTreeMap,
    mem::size_of,
    sync::atomic::{AtomicBool, Ordering},
};
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
pub struct ResidentResourcePlanReservation {
    pub inputs: AudioStorageReservation,
    pub catalog: ResourceCatalogReservation,
    pub max_preparation_metadata_bytes: usize,
    pub max_retained_data_bytes: usize,
    pub max_construction_peak_bytes: usize,
}

/// Holds the actual immutable media and waveform borrow while the ledger reads
/// their fields. It owns no PCM; no RefCell guard escapes into a callback.
pub struct ResidentPcmInventory<'a> {
    session: &'a Session,
    waveforms: &'a BTreeMap<ClipId, AudioClipWaveform>,
    transforms: &'a hex_engine::audio_transform::AudioTransformPcmInventory<'a>,
}
impl ResidentPcmInventory<'_> {
    /// PCM fields only. Enclosing maps, transform caches, graph metadata and
    /// retirement/publication proof remain separate and must not be inferred.
    pub fn visit_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        for resident in self.session.media.values() {
            match &resident.source_audio {
                OriginalAudio::Stereo(audio) => {
                    ledger.borrow_native_audio_on_control(audio, role)?
                }
                OriginalAudio::Mono(source) => {
                    ledger.borrow_native_source_on_control(source, role)?
                }
            }
            if let Some(audio) = &resident.audio {
                ledger.borrow_native_audio_on_control(audio, role)?;
            }
            visit_waveform(ledger, &resident.waveform, role)?;
        }
        for waveform in self.waveforms.values() {
            visit_waveform(ledger, &waveform.waveform, role)?;
        }
        self.transforms.visit_on_control(ledger, role)?;
        Ok(())
    }
}
fn visit_waveform<'a>(
    ledger: &mut AudioRootStorageLedger<'a>,
    waveform: &'a hex_media::waveform::Waveform,
    role: AudioStorageRole,
) -> Result<(), AudioStorageError> {
    if let Some(audio) = waveform.audio_source() {
        ledger.borrow_native_audio_on_control(audio, role)
    } else if let Some(source) = waveform.original_source() {
        ledger.borrow_original_source_lease_on_control(source, role)
    } else {
        Err(AudioStorageError::UnknownStorage)
    }
}
fn check(cancel: &AtomicBool) -> Result<(), ResourcePreparationError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ResourcePreparationError::Cancelled)
    } else {
        Ok(())
    }
}
impl Session {
    /// The scoped inventory/ledger cannot escape transaction preparation or
    /// retain a borrow across cache commit, eviction or canonical mutation.
    pub fn with_resident_pcm_inventory_on_control<T>(
        &self,
        visit: impl for<'scope> FnOnce(
            &'scope ResidentPcmInventory<'scope>,
        ) -> Result<T, AudioStorageError>,
    ) -> Result<T, AudioStorageError> {
        let waveforms = self
            .audio_clip_waveforms
            .try_borrow()
            .map_err(|_| AudioStorageError::UnknownStorage)?;
        self.audio_transforms
            .with_pcm_inventory_on_control(|transforms| {
                let inventory = ResidentPcmInventory {
                    session: self,
                    waveforms: &waveforms,
                    transforms,
                };
                visit(&inventory)
            })
    }

    /// Prepare requested sorted source identities from ACTUAL retained media.
    /// History-only media may resolve here; freshness/publication still belongs
    /// to the canonical worker and actual graph/root admission.
    pub fn prepare_resident_resource_bank_on_control(
        &self,
        assets: &[AssetId],
        reservation: ResidentResourcePlanReservation,
        cancel: &AtomicBool,
    ) -> Result<DeviceResourceBank, ResourcePreparationError> {
        self.prepare_resident_resource_bank_with_pending_on_control(
            assets,
            None,
            reservation,
            cancel,
        )
    }

    /// Borrow one actual unpublished decoded original while preparing the
    /// candidate's catalog. The canonical asset must be fresh and requested;
    /// this never inserts media or publishes a model/audio revision.
    pub(crate) fn prepare_resident_resource_bank_with_pending_on_control(
        &self,
        assets: &[AssetId],
        pending: Option<(&hex_project::Asset, &OriginalAudio)>,
        reservation: ResidentResourcePlanReservation,
        cancel: &AtomicBool,
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
