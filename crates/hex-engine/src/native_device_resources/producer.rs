//! Concrete borrowed original-source inputs for the existing bank producer.
//! No PCM owner, source cache, callback, publication permit or caller byte claim.
use super::*;
use crate::{Audio, AudioStorageReservation, OwnedAudioInputs, OwnedAudioSource};
use std::mem::size_of;

#[derive(Clone, Copy, Debug)]
pub struct OriginalResourcePlanReservation {
    pub inputs: AudioStorageReservation,
    pub catalog: ResourceCatalogReservation,
    pub max_preparation_metadata_bytes: usize,
    pub max_retained_data_bytes: usize,
    pub max_construction_peak_bytes: usize,
}

enum Source<'a> {
    Stereo(&'a Audio),
    Native(&'a OwnedAudioSource),
}
impl Source<'_> {
    fn facts(&self) -> Result<(u32, u64, OriginalResourceLayout), ResourcePreparationError> {
        match self {
            Self::Stereo(audio) => Ok((
                audio.sample_rate(),
                audio.frames().len() as u64,
                OriginalResourceLayout::Stereo,
            )),
            Self::Native(source) => {
                let view = source.view();
                let layout = if view.resident_mono_samples().is_some() {
                    OriginalResourceLayout::Mono
                } else if view.resident_frames().is_some() {
                    OriginalResourceLayout::Stereo
                } else {
                    return Err(ResourcePreparationError::InvalidSource);
                };
                Ok((source.sample_rate(), source.frames(), layout))
            }
        }
    }
}
/// One actual original borrow. Public raw/derived Audio remains Unknown at the
/// existing alias constructor; this wrapper cannot attach or upgrade provenance.
pub struct OriginalResourceInput<'a> {
    asset: AssetId,
    source: Source<'a>,
}
impl<'a> OriginalResourceInput<'a> {
    pub fn stereo(asset: AssetId, source: &'a Audio) -> Self {
        Self {
            asset,
            source: Source::Stereo(source),
        }
    }
    pub fn native(asset: AssetId, source: &'a OwnedAudioSource) -> Self {
        Self {
            asset,
            source: Source::Native(source),
        }
    }
    /// Fixed requested metadata for the caller's borrowed input Vec only. The
    /// other producer vectors/catalog/root and global coexistence remain separate.
    pub fn requested_input_metadata_bytes(count: usize) -> Result<usize, ResourcePreparationError> {
        count
            .checked_mul(size_of::<Self>())
            .and_then(|bytes| bytes.checked_add(size_of::<Vec<Self>>()))
            .ok_or(AudioStorageError::Overflow.into())
    }
}
/// Borrows the actual Vec field/capacity. No allocation, owning clone or input
/// mutation is possible through this view. It is not current-model authority.
/// ```compile_fail
/// use hex_engine::native_device_resources::OriginalResourceBankPlan;
/// fn fabricate() { let _ = OriginalResourceBankPlan { entries: &Vec::new() }; }
/// ```
/// ```compile_fail
/// use hex_engine::native_device_resources::{OriginalResourceBankPlan, OriginalResourceInput};
/// fn escape<'a>(entries: &'a Vec<OriginalResourceInput<'a>>) -> OriginalResourceBankPlan<'static> {
///     OriginalResourceBankPlan::borrow(entries)
/// }
/// ```
/// ```compile_fail
/// use hex_engine::native_device_resources::{OriginalResourceBankPlan, OriginalResourceInput};
/// fn change(entries: &mut Vec<OriginalResourceInput<'_>>) {
///     let plan = OriginalResourceBankPlan::borrow(entries);
///     entries.clear();
///     drop(plan);
/// }
/// ```
pub struct OriginalResourceBankPlan<'a> {
    entries: &'a Vec<OriginalResourceInput<'a>>,
}
impl<'a> OriginalResourceBankPlan<'a> {
    pub fn borrow(entries: &'a Vec<OriginalResourceInput<'a>>) -> Self {
        Self { entries }
    }
    /// The existing control producer, shared by Session's legacy bank-returning
    /// API and the sealed preparation route. No supplied bank is adopted. The
    /// native install operation must reserve/validate its actual destination
    /// before invoking this and move success there before any further work.
    pub fn prepare_on_control(
        &self,
        reservation: OriginalResourcePlanReservation,
        cancel: &dyn crate::CancellationQuery,
    ) -> Result<DeviceResourceBank, ResourcePreparationError> {
        cancellation(cancel)?;
        if self.entries.len() > hex_project::MAX_ASSETS
            || self
                .entries
                .windows(2)
                .any(|pair| pair[0].asset.value() >= pair[1].asset.value())
        {
            return Err(ResourcePreparationError::InvalidSource);
        }
        let input_bytes = Self::input_bytes(self.entries)?;
        let minimum_metadata = self
            .entries
            .len()
            .checked_mul(
                size_of::<(AssetId, OwnedAudioSource)>() + size_of::<OriginalResourceSource>(),
            )
            .and_then(|n| n.checked_add(size_of::<Vec<(AssetId, OwnedAudioSource)>>()))
            .and_then(|n| n.checked_add(size_of::<Vec<OriginalResourceSource>>()))
            .ok_or(AudioStorageError::Overflow)?;
        if minimum_metadata
            .checked_add(input_bytes)
            .ok_or(AudioStorageError::Overflow)?
            > reservation.max_preparation_metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        let mut sources = Vec::new();
        let mut expected = Vec::new();
        sources
            .try_reserve_exact(self.entries.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        expected
            .try_reserve_exact(self.entries.len())
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
            .and_then(|bytes| bytes.checked_add(input_bytes))
            .ok_or(AudioStorageError::Overflow)?
            > reservation.max_preparation_metadata_bytes
        {
            return Err(ResourcePreparationError::MetadataCapacity);
        }
        // Capture request facts and temporary capacities before creating aliases.
        // Source constructors check actual lineage; failure drops only new
        // unpublished control aliases. Unknown/derived Audio stays unknown.
        for entry in self.entries {
            cancellation(cancel)?;
            let original = &entry.source;
            let (sample_rate, frames, layout) = original.facts()?;
            expected.push(OriginalResourceSource {
                asset: entry.asset,
                sample_rate,
                frames,
                layout,
            });
        }
        for (entry, fact) in self.entries.iter().zip(&expected) {
            cancellation(cancel)?;
            let original = &entry.source;
            let source = match original {
                Source::Stereo(audio) => OwnedAudioSource::alias_native_audio_on_control(
                    audio,
                    size_of::<OwnedAudioSource>(),
                )?,
                Source::Native(source) => {
                    source.alias_for_resource_plan_on_control(size_of::<OwnedAudioSource>())?
                }
            };
            if source.sample_rate() != fact.sample_rate || source.frames() != fact.frames {
                return Err(ResourcePreparationError::InvalidSource);
            }
            sources.push((entry.asset, source));
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
            .and_then(|n| n.checked_add(input_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        if peak > reservation.max_construction_peak_bytes {
            return Err(AudioStorageError::ReservationExceeded.into());
        }
        drop(expected);
        let bank_peak = reservation
            .max_construction_peak_bytes
            .checked_sub(input_bytes)
            .ok_or(AudioStorageError::ReservationExceeded)?;
        DeviceResourceBank::try_new_on_control(
            inputs,
            catalog,
            reservation.max_retained_data_bytes,
            bank_peak,
            cancel,
        )
        .map_err(|refused| refused.reason)
    }
    fn input_bytes(
        entries: &Vec<OriginalResourceInput<'_>>,
    ) -> Result<usize, ResourcePreparationError> {
        OriginalResourceInput::requested_input_metadata_bytes(entries.capacity())
    }
}

#[cfg(test)]
mod tests;
