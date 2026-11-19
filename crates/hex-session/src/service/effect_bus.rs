//! Worker-owned catalog defaults for the two existing effect-only bus destinations.
use super::EffectBusDestination;
use hex_engine::NativeDeviceClass;
use hex_lattice_contract::DeviceTypeId;
use hex_project::{Command, DeviceId, Project};

pub(super) fn insertion_command(
    project: &Project,
    destination: EffectBusDestination,
    class: NativeDeviceClass,
    type_id: DeviceTypeId,
    before: Option<DeviceId>,
) -> Result<Command, String> {
    if class != NativeDeviceClass::Effect {
        return Err("this effect chain accepts effects only".into());
    }
    let devices = match destination {
        EffectBusDestination::Main => project.main_devices(),
        EffectBusDestination::Return(id) => project
            .return_tracks()
            .iter()
            .find(|track| track.id() == id)
            .ok_or("effect destination no longer exists")?
            .devices(),
    };
    if before.is_some_and(|id| !devices.iter().any(|device| device.id() == id)) {
        return Err("effect insertion anchor no longer belongs to its destination".into());
    }
    let state = hex_engine::default_native_device_state_for_class(class, type_id)
        .map_err(|error| hex_storage::text::lowercase_outside_verbatim(&error.to_string()))?;
    Ok(match destination {
        EffectBusDestination::Main => Command::InsertMainDevice { before, state },
        EffectBusDestination::Return(return_track) => Command::InsertReturnDevice {
            return_track,
            before,
            state,
        },
    })
}
