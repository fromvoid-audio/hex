//! Complete authored device initialization in one canonical history transaction.
//! This bounded wrapper reuses existing creation, naming and source ownership
//! commands. It accepts no arbitrary batches, nested wrappers, filesystem or DSP.
use crate::{Command, Created, DeviceComparison, Invalid, Project, TrackDeviceSlot};
use hex_lattice_contract::NativeDeviceState;

/// Validated creation/replacement plus its initial comparison bank. Fields are
/// private so an unrelated or recursively nested command cannot reach apply.
#[derive(Clone, Debug)]
pub struct DeviceInitialization {
    command: Command,
    comparison: Option<DeviceComparison>,
}

impl DeviceInitialization {
    /// Accept only device creation/replacement. `None` clears an inherited bank
    /// on replacement. Provider payload/DSP admission remains with the Session.
    pub fn new(command: Command, comparison: Option<DeviceComparison>) -> Result<Self, Invalid> {
        let state = state(&command)?;
        if let Some(comparison) = &comparison {
            comparison.validate(state)?;
        }
        Ok(Self {
            command,
            comparison,
        })
    }

    /// The original bounded command, for control-side inspection/diagnostics.
    pub fn command(&self) -> &Command {
        &self.command
    }

    pub fn comparison(&self) -> Option<&DeviceComparison> {
        self.comparison.as_ref()
    }

    pub(crate) fn apply(self, project: &mut Project) -> Result<Created, Invalid> {
        let replacement = match &self.command {
            Command::ReplaceDeviceState { device, .. }
            | Command::LoadInstrument {
                replacing: Some(device),
                ..
            } => Some(*device),
            _ => None,
        };
        let slot = match &self.command {
            Command::InsertTrackWithDevice { slot, .. }
            | Command::InsertDefaultTrackWithDevice { slot, .. }
            | Command::InsertDefaultTrackWithDeviceAndGain { slot, .. } => Some(*slot),
            _ => None,
        };
        let created = project.apply(self.command)?;
        let device = match created {
            Created::Device(id) => id,
            Created::Track(id) => {
                let track = project
                    .track(id)
                    .ok_or(Invalid("created device track is missing"))?;
                match slot {
                    Some(TrackDeviceSlot::Instrument) => track.instrument().map(crate::Device::id),
                    Some(TrackDeviceSlot::Effect) => track.devices().first().map(crate::Device::id),
                    None => None,
                }
                .ok_or(Invalid("created track device is missing"))?
            }
            Created::None => replacement.ok_or(Invalid("initialized device is missing"))?,
            _ => return Err(Invalid("device initialization returned an invalid result")),
        };
        let entry = project
            .device(device)
            .ok_or(Invalid("initialized device is missing"))?;
        if entry.comparison() == self.comparison.as_ref() {
            return Ok(created);
        }
        let state = entry.state().clone();
        project.apply(Command::ReplaceDeviceComparison {
            device,
            state,
            comparison: self.comparison,
        })?;
        Ok(created)
    }
}

fn state(command: &Command) -> Result<&NativeDeviceState, Invalid> {
    match command {
        Command::InsertDevice { state, .. }
        | Command::InsertInstrument { state, .. }
        | Command::LoadInstrument { state, .. }
        | Command::InsertMainDevice { state, .. }
        | Command::InsertReturnDevice { state, .. }
        | Command::InsertTrackWithDevice { state, .. }
        | Command::InsertDefaultTrackWithDevice { state, .. }
        | Command::InsertDefaultTrackWithDeviceAndGain { state, .. }
        | Command::ReplaceDeviceState { state, .. } => Ok(state),
        _ => Err(Invalid(
            "device initialization requires a device creation or replacement",
        )),
    }
}

#[cfg(test)]
mod tests;
