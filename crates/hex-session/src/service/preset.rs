//! Control-worker preset I/O and exact dialog ownership. Project mutation always
//! uses the existing prepared device-comparison replacement transaction in the service.
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

pub(super) fn validate_owner(
    session: &Session,
    generation: u64,
    revision: u64,
    owner: Owner,
) -> Result<(), String> {
    if generation != owner.project_generation || revision != owner.revision {
        return Err(
            "device action canceled · project changed; retry from the current device".into(),
        );
    }
    let entry = session
        .project()
        .device(owner.device)
        .ok_or("device target no longer exists")?;
    if session
        .native_device_class(owner.device)
        .map_err(|error| error.to_string())?
        != owner.class
        || entry.state().type_id() != owner.type_id
    {
        return Err(
            "device action canceled · target device changed; retry from the current device".into(),
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
    let device = session
        .project()
        .device(owner.device)
        .ok_or("device is missing")?;
    let state = device.state().clone();
    let comparison = device.comparison().cloned();
    crate::device_comparison::validate_states(owner.class, &state, comparison.as_ref())
        .map_err(|error| format!("device preset refused · {error}"))?;
    hex_storage::preset::save_new(
        path,
        &Preset {
            class: storage_class(owner.class),
            state,
            comparison,
        },
    )
    .map_err(|error| error.to_string())
}

/// Read only on the worker. Both active and inactive native payloads must be
/// understood by this application before a default can be consumed/replaced.
pub(super) fn load_default(
    root: &Path,
    class: NativeDeviceClass,
    type_id: DeviceTypeId,
) -> Result<Option<Preset>, String> {
    validate_default_root(root)?;
    let preset = hex_storage::preset::defaults::load(root, storage_class(class), type_id)
        .map_err(|error| error.to_string())?;
    if let Some(preset) = &preset {
        crate::device_comparison::validate_states(class, &preset.state, preset.comparison.as_ref())
            .map_err(|error| format!("device default refused · {error}"))?;
    }
    Ok(preset)
}

pub(super) fn save_default(
    session: &Session,
    generation: u64,
    revision: u64,
    owner: Owner,
    root: &Path,
) -> Result<(), String> {
    validate_default_root(root)?;
    validate_owner(session, generation, revision, owner)?;
    let device = session
        .project()
        .device(owner.device)
        .ok_or("device is missing")?;
    crate::device_comparison::validate_states(owner.class, device.state(), device.comparison())
        .map_err(|error| format!("device default refused · {error}"))?;
    // Refuse future or malformed native payloads already on disk before the
    // storage layer's atomic replacement. Opaque envelope validation is not
    // evidence that this version understands a provider's settings.
    load_default(root, owner.class, owner.type_id)?;
    hex_storage::preset::defaults::save(
        root,
        &Preset {
            class: storage_class(owner.class),
            state: device.state().clone(),
            comparison: device.comparison().cloned(),
        },
    )
    .map_err(|error| error.to_string())
}

pub(super) fn validate_default_root(root: &Path) -> Result<(), String> {
    if root.as_os_str().is_empty() || root.as_os_str().len() > 4096 {
        return Err("device default data folder must contain 1–4096 bytes".into());
    }
    Ok(())
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
    // Both opaque variants must be valid for this actual device class before
    // any prepared candidate or publication; retain original authored bytes.
    crate::device_comparison::validate_states(
        owner.class,
        &preset.state,
        preset.comparison.as_ref(),
    )
    .map_err(|error| format!("device preset refused · {error}"))?;
    // Host input/output gains belong to the target, outside the opaque preset.
    let target = session
        .project()
        .device(owner.device)
        .ok_or("device is missing")?
        .state();
    let state = preset
        .state
        .with_input_gain_millidb(target.input_gain_millidb())
        .and_then(|state| state.with_output_gain_millidb(target.output_gain_millidb()))
        .map_err(|error| format!("device preset refused · {error}"))?;
    let comparison = preset
        .comparison
        .map(|comparison| comparison.with_global_trims_from(&state))
        .transpose()
        .map_err(|error| format!("device preset refused · {error}"))?;
    Ok(Command::ReplaceDeviceComparison {
        device: owner.device,
        state,
        comparison,
    })
}

#[cfg(test)]
mod defaults_tests;
#[cfg(test)]
mod tests;
