//! Singleton Main metadata; provider class admission stays in prepared effect compilation.
use crate::{Command, Created, Device, DeviceId, Invalid, Project};

pub(crate) fn apply(project: &mut Project, command: Command) -> Result<Created, Invalid> {
    match command {
        Command::InsertMainDevice { before, state } => {
            if project.main_devices.len() >= crate::MAX_DEVICES_PER_TRACK {
                return Err(Invalid("main device limit reached"));
            }
            if crate::returns::device_count(project)? >= crate::MAX_DEVICES {
                return Err(Invalid("project device limit reached"));
            }
            let index = anchor(project, before)?;
            let id = DeviceId(project.allocate_id()?);
            project
                .main_devices
                .insert(index, Device::fresh_serial_leaf(id, state));
            Ok(Created::Device(id))
        }
        Command::ReplaceMainDevice { replacing, state } => {
            let index = project
                .main_devices
                .iter()
                .position(|device| device.id == replacing && device.parent.is_none())
                .ok_or(Invalid("main device replacement target changed"))?;
            if crate::device_inventory::serial_unit_span(&project.main_devices, index)?.len() != 1 {
                return Err(Invalid("main device replacement requires a single effect"));
            }
            let id = DeviceId(project.allocate_id()?);
            project.main_devices[index] = Device::fresh_serial_leaf(id, state);
            Ok(Created::Device(id))
        }
        Command::MoveMainDevice { device, before } => {
            let source = project
                .main_devices
                .iter()
                .position(|item| item.id == device)
                .ok_or(Invalid("main device no longer exists"))?;
            let destination = anchor(project, before)?;
            crate::device::move_serial_unit(&mut project.main_devices, source, destination)?;
            Ok(Created::None)
        }
        _ => Err(Invalid("not a main device command")),
    }
}

fn anchor(project: &Project, before: Option<DeviceId>) -> Result<usize, Invalid> {
    match before {
        Some(id) => project
            .main_devices
            .iter()
            .position(|item| item.id == id)
            .ok_or(Invalid("main device insertion anchor no longer exists")),
        None => Ok(project.main_devices.len()),
    }
}

#[cfg(test)]
mod tests;
