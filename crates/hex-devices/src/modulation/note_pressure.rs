//! Authored identity for the occurrence-owned note-pressure input. The native
//! instrument runtime owns each held voice's reading and its release lifetime.
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.note-pressure");

pub fn default_state() -> Result<NativeDeviceState, DeviceContractError> {
    NativeDeviceState::new(TYPE_ID, 1, Vec::new())
}

pub fn validate(state: &NativeDeviceState) -> Result<(), DeviceContractError> {
    if state.type_id() != TYPE_ID || state.state_version() != 1 || !state.payload().is_empty() {
        return Err(DeviceContractError::MalformedState);
    }
    state.require_asset_free_processing()
}
