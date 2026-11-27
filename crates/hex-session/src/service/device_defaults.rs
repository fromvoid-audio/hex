//! Fresh catalog construction only. Disk I/O and provider validation run on the
//! existing control worker before canonical preparation; supplied state never
//! enters this resolver. The queue captures application-directory configuration.
use super::{TrackCatalogPlacement, native_device_name, preset};
use crate::Session;
use hex_engine::NativeDeviceClass;
use hex_lattice_contract::DeviceTypeId;
use hex_project::{Command, DeviceInitialization, TrackId};
use hex_storage::preset::PresetContents;
use std::path::Path;

pub(super) fn track_picker_command(
    session: &Session,
    track: TrackId,
    class: NativeDeviceClass,
    type_id: DeviceTypeId,
    placement: TrackCatalogPlacement,
    root: Option<&Path>,
) -> Result<preset::PreparedCommand, String> {
    if !session
        .project()
        .track(track)
        .is_some_and(|track| placement.matches_track(class, track))
    {
        return Err("device picker target changed".into());
    }
    let state = hex_engine::default_native_device_state_for_class(class, type_id)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?;
    let command = match placement {
        TrackCatalogPlacement::ReplaceInstrument(device) => {
            Command::ReplaceDeviceState { device, state }
        }
        TrackCatalogPlacement::ReplaceMidiProcessor(device) => Command::ReplaceTrackMidiProcessor {
            track,
            replacing: device,
            state,
        },
        TrackCatalogPlacement::Insert { .. } if class == NativeDeviceClass::Instrument => {
            Command::LoadInstrument {
                track,
                replacing: None,
                state,
                device_name: native_device_name(class, type_id)?,
            }
        }
        TrackCatalogPlacement::Insert { .. } if class == NativeDeviceClass::MidiProcessor => {
            Command::InsertMidiProcessor { track, state }
        }
        TrackCatalogPlacement::Insert { before } => Command::InsertDevice {
            track,
            before,
            state,
        },
    };
    initialize(session, command, class, root)
}

pub(super) fn initialize(
    session: &Session,
    mut command: Command,
    class: NativeDeviceClass,
    root: Option<&Path>,
) -> Result<preset::PreparedCommand, String> {
    let state = match &mut command {
        Command::InsertDevice { state, .. }
        | Command::InsertNestedDevice { state, .. }
        | Command::InsertMidiProcessor { state, .. }
        | Command::ReplaceTrackMidiProcessor { state, .. }
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
        .map(|root| preset::load_default_contents(root, class, state.type_id()))
        .transpose()?
        .flatten()
    {
        Some(PresetContents::State(preset)) => {
            *state = preset.state;
            preset.comparison
        }
        Some(PresetContents::Unit(unit)) => {
            return preset::prepare_unit_command(session, command, unit, true, root);
        }
        None => None,
    };
    DeviceInitialization::new(command, comparison)
        .map(|initialization| {
            preset::PreparedCommand::plain(Command::InitializeDevice(Box::new(initialization)))
        })
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
