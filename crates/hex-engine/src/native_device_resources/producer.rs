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

#[derive(Clone, Copy)]
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
#[derive(Clone, Copy)]
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
    inputs: SourceInputs<'a>,
}
enum SourceInputs<'a> {
    Vector(&'a Vec<OriginalResourceInput<'a>>),
    Resident {
        fields: &'a dyn crate::sampler_host_fields::OriginalSamplerResidentFields,
        target: &'a hex_project::Project,
    },
}
impl<'a> OriginalResourceBankPlan<'a> {
    pub(crate) fn visit_storage_on_control<'scope>(
        &'scope self,
        ledger: &mut crate::AudioRootStorageLedger<'scope>,
    ) -> Result<(), AudioStorageError>
    where
        'a: 'scope,
    {
        use crate::audio_storage::{Location, Owner};
        if let SourceInputs::Vector(entries) = self.inputs {
            ledger.vec(entries)?;
            ledger.bytes(
                Owner::exclusive_vec(entries),
                Location("original-resource-input-vec-inline", 0),
                size_of_val(entries),
            )?;
        }
        let body = crate::sampler_host_fields::BodyRef::of(self);
        ledger.bytes(
            body.owner(),
            Location("original-resource-plan-inline", 0),
            body.layout_bytes(),
        )?;
        for index in 0..self.len() {
            let entry = self
                .entry(index)
                .map_err(|_| AudioStorageError::SourceMismatch)?;
            match &entry.source {
                Source::Native(source) => {
                    // A mandatory resident-owner walk may already cover this
                    // exact embedded owning field. Reuse that observation, not
                    // another handle or an unobserved enclosing-body claim.
                    // Arithmetic membership never prunes descendants.
                    if !ledger.skip_observed_root(*source)
                        && !ledger.skip_observed_shared_field(*source)
                    {
                        ledger.borrow_native_source_on_control(
                            source,
                            crate::AudioStorageRole::Caller,
                        )?;
                    }
                }
                Source::Stereo(source) => {
                    // A mandatory resident-owner walk may already cover this
                    // exact embedded owning field. Reuse that observation, not
                    // another handle or an unobserved enclosing-body claim.
                    // Arithmetic membership never prunes descendants.
                    if !ledger.skip_observed_root(*source)
                        && !ledger.skip_observed_shared_field(*source)
                    {
                        ledger.borrow_native_audio_on_control(
                            source,
                            crate::AudioStorageRole::Caller,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
    pub fn borrow(entries: &'a Vec<OriginalResourceInput<'a>>) -> Self {
        Self {
            inputs: SourceInputs::Vector(entries),
        }
    }
    /// Allocation-free resolution from the actual borrowed Session media and
    /// captured target. This is not source-context/admission authority. Actual
    /// owning fields, metadata, old/new coexistence and both caps remain in the
    /// closed constructor. No temporary input Vec is born before that preflight.
    pub fn borrow_resident_fields_on_control(
        fields: &'a dyn crate::sampler_host_fields::OriginalSamplerResidentFields,
        target: &'a hex_project::Project,
    ) -> Result<Self, ResourcePreparationError> {
        fields.validate_remaining_fields()?;
        if !fields.verified_sources().is_empty() {
            return Err(AudioStorageError::UnknownStorage.into());
        }
        let plan = Self {
            inputs: SourceInputs::Resident { fields, target },
        };
        if plan.len() > hex_project::MAX_ASSETS {
            return Err(ResourcePreparationError::InvalidSource);
        }
        // Inspect every actual source before any producer allocation/alias.
        for index in 0..plan.len() {
            plan.entry(index)?;
        }
        Ok(plan)
    }
    fn len(&self) -> usize {
        match self.inputs {
            SourceInputs::Vector(entries) => entries.len(),
            SourceInputs::Resident { target, .. } => target.asset_entries_on_control().len(),
        }
    }
    fn entry(&self, index: usize) -> Result<OriginalResourceInput<'a>, ResourcePreparationError> {
        match self.inputs {
            SourceInputs::Vector(entries) => entries
                .get(index)
                .copied()
                .ok_or(ResourcePreparationError::InvalidSource),
            SourceInputs::Resident { fields, target } => {
                let asset = &target
                    .asset_entries_on_control()
                    .as_slice()
                    .get(index)
                    .ok_or(ResourcePreparationError::InvalidSource)?
                    .1;
                let mut left = 0;
                let mut right = fields.media().len();
                while left < right {
                    let middle = left + (right - left) / 2;
                    let candidate = fields.asset(middle)?;
                    match candidate.asset.value().cmp(&asset.id().value()) {
                        std::cmp::Ordering::Less => left = middle + 1,
                        std::cmp::Ordering::Greater => right = middle,
                        std::cmp::Ordering::Equal => {
                            let source = match candidate.original {
                                crate::sampler_host_fields::ResidentOriginalFields::Stereo(
                                    audio,
                                ) => Source::Stereo(audio),
                                crate::sampler_host_fields::ResidentOriginalFields::Mono(
                                    source,
                                ) => Source::Native(source),
                            };
                            let (rate, frames, _) = source.facts()?;
                            if rate != asset.source_sample_rate() || frames != asset.source_frames()
                            {
                                return Err(ResourcePreparationError::InvalidSource);
                            }
                            return Ok(OriginalResourceInput {
                                asset: asset.id(),
                                source,
                            });
                        }
                    }
                }
                Err(ResourcePreparationError::InvalidSource)
            }
        }
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
        if self.len() > hex_project::MAX_ASSETS {
            return Err(ResourcePreparationError::InvalidSource);
        }
        let mut previous = None;
        for index in 0..self.len() {
            let entry = self.entry(index)?;
            if previous.is_some_and(|id| id >= entry.asset.value()) {
                return Err(ResourcePreparationError::InvalidSource);
            }
            previous = Some(entry.asset.value());
        }
        let input_bytes = self.input_bytes()?;
        let minimum_metadata = self
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
            .try_reserve_exact(self.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        expected
            .try_reserve_exact(self.len())
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
        for index in 0..self.len() {
            let entry = self.entry(index)?;
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
        for (index, fact) in expected.iter().enumerate() {
            let entry = self.entry(index)?;
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
    fn input_bytes(&self) -> Result<usize, ResourcePreparationError> {
        let input = match self.inputs {
            SourceInputs::Vector(entries) => {
                OriginalResourceInput::requested_input_metadata_bytes(entries.capacity())?
            }
            SourceInputs::Resident { .. } => 0,
        };
        input
            .checked_add(size_of_val(self))
            .ok_or(AudioStorageError::Overflow.into())
    }
}

#[cfg(test)]
mod tests;
