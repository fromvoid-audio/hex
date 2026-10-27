//! Closed original runtime leaf observations for native constructor/group owners.
//! A typed borrowed view is not source custody. Callers first retain the real
//! constructor origin; only these private-field provider families are audited.
use crate::AudioStorageError;
use hex_lattice::NativeRuntimeEntryStorage;
pub(crate) fn pair_bytes(
    entry: &NativeRuntimeEntryStorage<'_>,
) -> Result<Option<(usize, usize)>, AudioStorageError> {
    if let Some((primary, scratch)) = entry.runtime_pair::<hex_devices::reverb::ReverbRuntime>() {
        return Ok(Some((
            primary
                .control_storage_bytes()
                .ok_or(AudioStorageError::Overflow)?,
            scratch
                .control_storage_bytes()
                .ok_or(AudioStorageError::Overflow)?,
        )));
    }
    match (entry.primary_payload_bytes(), entry.scratch_payload_bytes()) {
        (Some(a), Some(b))
            if entry
                .runtime_types_are::<hex_devices::utility_filters::UtilityFilterProcessor>()
                || entry.runtime_types_are::<hex_devices::saturator::SaturatorRuntime>()
                || entry.runtime_types_are::<hex_devices::auto_pan::AutoPanRuntime>() =>
        {
            Ok(Some((a, b)))
        }
        (None, None) if !entry.is_owned_transaction() => Ok(None),
        _ => Err(AudioStorageError::UnsupportedGraph),
    }
}

/// Recognition only; constructor origin and strict state validation remain separate.
pub(crate) fn is_closed_type(id: hex_lattice_contract::DeviceTypeId) -> bool {
    id == hex_devices::utility::UTILITY_TYPE_ID
        || id == hex_devices::reverb::REVERB_TYPE_ID
        || id == hex_devices::saturator::SATURATOR_TYPE_ID
        || id == hex_devices::auto_pan::AUTO_PAN_TYPE_ID
}
