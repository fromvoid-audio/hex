//! Control-worker preset I/O and exact dialog ownership. Project mutation always
//! uses the existing prepared ReplaceDeviceState transaction in the service.
use crate::Session;
use hex_engine::NativeDeviceClass;
use hex_lattice_contract::DeviceTypeId;
use hex_project::{Command, DeviceId};
use hex_storage::preset::{DeviceClass, Preset};
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub struct Owner {
    pub project_generation: u64,
    pub revision: u64,
    pub device: DeviceId,
    pub class: NativeDeviceClass,
    pub type_id: DeviceTypeId,
}

fn storage_class(class: NativeDeviceClass) -> DeviceClass {
    match class {
        NativeDeviceClass::Instrument => DeviceClass::Instrument,
        NativeDeviceClass::Effect => DeviceClass::Effect,
    }
}

fn validate_owner(
    session: &Session,
    generation: u64,
    revision: u64,
    owner: Owner,
) -> Result<(), String> {
    if generation != owner.project_generation || revision != owner.revision {
        return Err(
            "device preset canceled · project changed; open the preset dialog again".into(),
        );
    }
    let entry = session
        .project()
        .device(owner.device)
        .ok_or("device preset target no longer exists")?;
    if session
        .native_device_class(owner.device)
        .map_err(|error| error.to_string())?
        != owner.class
        || entry.state().type_id() != owner.type_id
    {
        return Err(
            "device preset canceled · target device changed; open the preset dialog again".into(),
        );
    }
    Ok(())
}

pub(super) fn save(
    session: &Session,
    generation: u64,
    revision: u64,
    owner: Owner,
    path: &Path,
) -> Result<(), String> {
    if path.as_os_str().len() > 4096 {
        return Err("device preset path exceeds the 4096-byte limit".into());
    }
    validate_owner(session, generation, revision, owner)?;
    let state = session
        .project()
        .device(owner.device)
        .ok_or("device is missing")?
        .state()
        .clone();
    hex_engine::native_device_view_for_class(owner.class, &state)
        .map_err(|error| format!("device preset refused · {error}"))?;
    hex_storage::preset::save_new(
        path,
        &Preset {
            class: storage_class(owner.class),
            state,
        },
    )
    .map_err(|error| error.to_string())
}

pub(super) fn load_command(
    session: &Session,
    generation: u64,
    revision: u64,
    owner: Owner,
    path: &Path,
) -> Result<Command, String> {
    if path.as_os_str().len() > 4096 {
        return Err("device preset path exceeds the 4096-byte limit".into());
    }
    validate_owner(session, generation, revision, owner)?;
    let preset = hex_storage::preset::load(path).map_err(|error| error.to_string())?;
    if preset.class != storage_class(owner.class) || preset.state.type_id() != owner.type_id {
        return Err(
            "incompatible device preset · choose a preset for this device type and class".into(),
        );
    }
    // The native provider rejects unknown/newer/malformed states. Its supported
    // legacy migrations remain authoritative; retain the original opaque bytes.
    hex_engine::native_device_view_for_class(owner.class, &preset.state)
        .map_err(|error| format!("device preset refused · {error}"))?;
    Ok(Command::ReplaceDeviceState {
        device: owner.device,
        state: preset.state,
    })
}

#[cfg(test)]
mod tests;
