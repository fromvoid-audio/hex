//! Exact control-request ownership for the shared canonical A/B commands.
use super::preset::{self, Owner};
use crate::Session;
use hex_project::{Command, DeviceComparisonAction, DeviceVariant};

pub(super) fn preflight(
    session: &Session,
    generation: u64,
    revision: u64,
    owner: Owner,
    expected: DeviceVariant,
) -> Result<(), String> {
    preset::validate_owner(session, generation, revision, owner)?;
    let device = session
        .project()
        .device(owner.device)
        .ok_or("device no longer exists")?;
    if device.active_variant() != expected {
        return Err("device comparison changed · retry from its current slot".into());
    }
    crate::device_comparison::validate_states(owner.class, device.state(), device.comparison())
        .map_err(|error| format!("device comparison refused · {error}"))
}

pub(super) fn command(
    session: &Session,
    generation: u64,
    revision: u64,
    owner: Owner,
    expected: DeviceVariant,
    action: DeviceComparisonAction,
) -> Result<Command, String> {
    // Recheck after the existing gesture barrier. An intervening accepted edit
    // invalidates the original owner instead of replaying on another revision.
    preflight(session, generation, revision, owner, expected)?;
    Ok(Command::CompareDevice {
        device: owner.device,
        expected,
        action,
    })
}

#[cfg(test)]
mod tests;
