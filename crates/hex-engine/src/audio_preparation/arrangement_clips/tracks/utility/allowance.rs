//! Conservative requested new child/backing allowance BEFORE construction.
//! Not observed instantaneous peak, caller ownership, a DATA permit or RSS.
use super::*;
use std::mem::size_of;

#[derive(Clone, Copy, Debug)]
pub(in crate::audio_preparation::arrangement_clips) struct ConstructionAllowance {
    pub(in crate::audio_preparation::arrangement_clips) retained_bytes: usize,
    pub(in crate::audio_preparation::arrangement_clips) construction_peak_bytes: usize,
    pub(in crate::audio_preparation::arrangement_clips) entry_capacity: usize,
}
fn parts(parts: Option<(usize, usize)>) -> Result<usize, AudioStorageError> {
    let (body, header) = parts.ok_or(AudioStorageError::UnsupportedConstructor)?;
    body.checked_add(header).ok_or(AudioStorageError::Overflow)
}
impl UtilityCustody {
    pub(in crate::audio_preparation::arrangement_clips) fn construction_allowance(
        &self,
        devices: &[hex_project::Device],
        owner: ProcessorChainOwner,
    ) -> Result<ConstructionAllowance, AudioStorageError> {
        if !self.entries.is_empty()
            || match owner {
                ProcessorChainOwner::Track(id) | ProcessorChainOwner::Return(id) => id == 0,
            }
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        for device in devices {
            device
                .state()
                .require_asset_free_processing()
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            // The actual closed constructor below still selects the concrete
            // production implementation. These scalar facts supply cost only.
            hex_devices::utility::UTILITY_DEFINITION
                .read_parameter(device.state(), hex_devices::utility::parameter::GAIN)
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        }
        let count = devices.len();
        let entry_capacity = self.entries.capacity().max(count);
        let entries = entry_capacity
            .checked_mul(size_of::<Entry>())
            .ok_or(AudioStorageError::Overflow)?;
        let chain =
            parts(OwnedShared::<[crate::PreparedEffectInstance]>::declared_slice_parts(count))?;
        let child = [
            parts(OwnedShared::<[u8]>::declared_slice_parts(
                hex_devices::utility::UTILITY_STATE_PAYLOAD_BYTES,
            ))?,
            parts(OwnedShared::<hex_lattice_contract::NativeDeviceState>::declared_parts())?,
            parts(OwnedShared::<hex_lattice::DeviceMeter>::declared_parts())?,
            // Reserve the concrete optional filter even for a memoryless or
            // bypassed state; this is explicitly a conservative ceiling.
            parts(OwnedShared::<
                hex_devices::utility::PreparedUtilityFilterRuntime,
            >::declared_parts())?,
        ]
        .into_iter()
        .try_fold(0usize, |sum, bytes| {
            sum.checked_add(bytes).ok_or(AudioStorageError::Overflow)
        })?;
        let retained_bytes = child
            .checked_mul(count)
            .and_then(|bytes| bytes.checked_add(chain))
            .and_then(|bytes| bytes.checked_add(entries))
            .ok_or(AudioStorageError::Overflow)?;
        // Old Entry backing may coexist with replacement; completed chain
        // children coexist with the Vec->slice input, and one codec input Vec
        // may coexist with its new payload Arc. No input PCM is copied here.
        let replaced = if count > self.entries.capacity() {
            self.metadata_bytes()?
        } else {
            0
        };
        let scratch = count
            .checked_mul(size_of::<crate::PreparedEffectInstance>())
            .and_then(|bytes| bytes.checked_add(replaced))
            .and_then(|bytes| {
                bytes.checked_add(if count == 0 {
                    0
                } else {
                    hex_devices::utility::UTILITY_STATE_PAYLOAD_BYTES
                })
            })
            .ok_or(AudioStorageError::Overflow)?;
        Ok(ConstructionAllowance {
            retained_bytes,
            construction_peak_bytes: retained_bytes
                .checked_add(scratch)
                .ok_or(AudioStorageError::Overflow)?,
            entry_capacity,
        })
    }
}

#[cfg(test)]
mod tests;
