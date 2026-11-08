//! Fresh catalog construction only. Disk I/O and provider validation run on the
//! existing control worker before canonical preparation; supplied state never
//! enters this resolver. The queue captures application-directory configuration.
use super::preset;
use hex_engine::NativeDeviceClass;
use hex_project::{Command, DeviceInitialization};
use std::path::Path;

pub(super) fn initialize(
    mut command: Command,
    class: NativeDeviceClass,
    root: Option<&Path>,
) -> Result<Command, String> {
    let state = match &mut command {
        Command::InsertDevice { state, .. }
        | Command::InsertInstrument { state, .. }
        | Command::LoadInstrument { state, .. }
        | Command::InsertMainDevice { state, .. }
        | Command::InsertReturnDevice { state, .. }
        | Command::InsertTrackWithDevice { state, .. }
        | Command::InsertDefaultTrackWithDevice { state, .. }
        | Command::InsertDefaultTrackWithDeviceAndGain { state, .. }
        | Command::ReplaceDeviceState { state, .. } => state,
        _ => return Err("device default requires a fresh catalog construction".into()),
    };
    let comparison = match root
        .map(|root| preset::load_default(root, class, state.type_id()))
        .transpose()?
        .flatten()
    {
        Some(preset) => {
            *state = preset.state;
            preset.comparison
        }
        None => None,
    };
    DeviceInitialization::new(command, comparison)
        .map(|initialization| Command::InitializeDevice(Box::new(initialization)))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
