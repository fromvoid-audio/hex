//! Atomic same-owner serial membership edits; runtime admission stays external.
use crate::{Device, DeviceId, Invalid, Project};

// Indices are resolved from this candidate's inventories and never cross edits.
#[derive(Clone, Copy)]
enum Owner {
    Track(usize),
    Return(usize),
    Main,
}
fn locate(project: &Project, device: DeviceId) -> Result<(Owner, usize), Invalid> {
    for (track, item) in project.tracks.iter().enumerate() {
        if let Some(index) = item.devices.iter().position(|item| item.id() == device) {
            return Ok((Owner::Track(track), index));
        }
    }
    for (track, item) in project.returns.iter().enumerate() {
        if let Some(index) = item.devices.iter().position(|item| item.id() == device) {
            return Ok((Owner::Return(track), index));
        }
    }
    project
        .main_devices
        .iter()
        .position(|item| item.id() == device)
        .map(|index| (Owner::Main, index))
        .ok_or(Invalid("device is not in an editable effect chain"))
}
fn devices(project: &Project, owner: Owner) -> &[Device] {
    match owner {
        Owner::Track(index) => &project.tracks[index].devices,
        Owner::Return(index) => &project.returns[index].devices,
        Owner::Main => &project.main_devices,
    }
}
fn devices_mut(project: &mut Project, owner: Owner) -> &mut [Device] {
    match owner {
        Owner::Track(index) => &mut project.tracks[index].devices,
        Owner::Return(index) => &mut project.returns[index].devices,
        Owner::Main => &mut project.main_devices,
    }
}

pub(super) fn apply(
    project: &mut Project,
    device: DeviceId,
    parent: Option<DeviceId>,
) -> Result<(), Invalid> {
    if let Some(source) = project.source_owning_device(device) {
        return project.reparent_source_device(source, device, parent);
    }
    let (owner, index) = locate(project, device)?;
    let chain = devices(project, owner);
    if chain[index].parent() == parent {
        return Ok(());
    }
    let span = crate::device_inventory::serial_unit_span(chain, index)?;
    let target = if let Some(parent) = parent {
        let target = chain
            .iter()
            .position(|item| item.id() == parent)
            .ok_or(Invalid(
                "serial parent must belong to the same effect chain",
            ))?;
        if span.contains(&target) {
            return Err(Invalid("a device cannot contain itself or an ancestor"));
        }
        target
    } else {
        chain
            .get(..=index)
            .and_then(|prefix| prefix.iter().rposition(|item| item.parent().is_none()))
            .ok_or(Invalid("serial root no longer exists"))?
    };
    let destination = crate::device_inventory::serial_unit_span(chain, target)?.end;
    match owner {
        Owner::Track(track) => {
            // Appending a child to a unit ending at the instrument still belongs
            // before that slot, even though the local effect index is identical.
            let before_instrument = project.tracks[track]
                .instrument_position
                .is_some_and(|gap| target < gap);
            super::move_local_unit(
                &mut project.tracks[track],
                index,
                destination,
                before_instrument,
            )?;
        }
        _ => super::move_serial_unit(devices_mut(project, owner), index, destination)?,
    }
    let moved = devices_mut(project, owner)
        .iter_mut()
        .find(|item| item.id() == device)
        .ok_or(Invalid("moving device no longer exists"))?;
    moved.parent = parent;
    // The existing history candidate validates preorder/depth/slot boundaries
    // before preparation. Any refusal leaves canonical state and history intact.
    Ok(())
}

pub(super) fn insert(
    project: &mut Project,
    parent: DeviceId,
    state: hex_lattice_contract::NativeDeviceState,
) -> Result<crate::Created, Invalid> {
    if let Some(source) = project.source_owning_device(parent) {
        return project.insert_source_child(source, parent, state);
    }
    let (owner, index) = locate(project, parent)?;
    let chain = devices(project, owner);
    let end = crate::device_inventory::serial_unit_span(chain, index)?.end;
    let before = chain.get(end).map(Device::id);
    let command = match owner {
        Owner::Track(index) => {
            let track = &project.tracks[index];
            let before = if track.instrument_position == Some(end) {
                track.instrument().map(Device::id)
            } else {
                before
            };
            crate::Command::InsertDevice {
                track: track.id,
                before,
                state,
            }
        }
        Owner::Return(index) => crate::Command::InsertReturnDevice {
            return_track: project.returns[index].id,
            before,
            state,
        },
        Owner::Main => crate::Command::InsertMainDevice { before, state },
    };
    let crate::Created::Device(id) = project.apply(command)? else {
        return Err(Invalid(
            "nested device insertion returned an invalid result",
        ));
    };
    apply(project, id, Some(parent))?;
    Ok(crate::Created::Device(id))
}

#[cfg(test)]
#[path = "device_unit_parent_tests.rs"]
mod tests;
