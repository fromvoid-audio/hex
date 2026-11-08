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

#[cfg(test)]
#[path = "device_unit_parent_tests.rs"]
mod tests;
