//! Complete authored device initialization in one canonical history transaction.
//! This bounded wrapper reuses existing creation, naming and source ownership
//! commands. It accepts no arbitrary batches, nested wrappers, filesystem or DSP.
use crate::{
    AssetId, Command, Created, DeviceComparison, DeviceId, Invalid, OriginalSourceFacts,
    PortableDeviceUnit, Project, TrackDeviceSlot,
};
use hex_lattice_contract::NativeDeviceState;
use std::path::PathBuf;

mod unit;

/// Worker-verified original audio metadata registered as a fresh destination
/// asset inside one complete-unit initialization edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterUnitOriginal {
    pub path: PathBuf,
    pub source_rate: u32,
    pub source_frames: u64,
    pub facts: OriginalSourceFacts,
}

/// How one entry in a `PortableDeviceUnit`'s asset manifest resolves in the
/// destination project during complete-unit initialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitAssetResolution {
    /// Reuse an existing destination original whose canonical facts, rate and
    /// frame count match the manifest entry.
    Existing(AssetId),
    /// Register a fresh destination original inside the same project edit using
    /// worker-supplied path, rate, frame count and exact content identity.
    Register(RegisterUnitOriginal),
}

/// One fresh destination original reserved by complete-unit initialization for
/// the prepared audio stage before the edit commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReservedUnitOriginal {
    /// Zero-based index into `PortableDeviceUnit::assets()`.
    pub manifest_index: usize,
    /// Exact fresh destination `AssetId` reserved and registered by this edit.
    pub asset: AssetId,
}

/// Validated creation/replacement plus its initial comparison bank. Fields are
/// private so an unrelated or recursively nested command cannot reach apply.
#[derive(Clone, Debug)]
pub struct DeviceInitialization {
    command: Command,
    authored: AuthoredInitialization,
}

#[derive(Clone, Debug)]
enum AuthoredInitialization {
    Comparison(Option<DeviceComparison>),
    Unit(unit::CompleteUnit),
}

impl DeviceInitialization {
    /// Accept only device creation/replacement. `None` clears an inherited bank
    /// on replacement. Provider payload/DSP admission remains with the Session.
    pub fn new(
        mut command: Command,
        comparison: Option<DeviceComparison>,
    ) -> Result<Self, Invalid> {
        let state = state_mut(&mut command)?;
        if let Some(comparison) = &comparison {
            comparison.validate(state)?;
        }
        Ok(Self {
            command,
            authored: AuthoredInitialization::Comparison(comparison),
        })
    }

    /// Initialize one complete portable subtree with verified destination asset
    /// identities. Replacement targets must already share the unit root's
    /// provider type (for same-provider preset loading).
    pub fn new_unit(
        mut command: Command,
        template: PortableDeviceUnit,
        assets: Vec<AssetId>,
    ) -> Result<Self, Invalid> {
        let unit = unit::CompleteUnit::new(
            template,
            assets,
            state_mut(&mut command)?,
            unit::ReplacementPolicy::SameProvider,
        )?;
        Ok(Self {
            command,
            authored: AuthoredInitialization::Unit(unit),
        })
    }

    /// Initialize one complete portable subtree for fresh or cross-type catalog
    /// default replacement. Slot class must match and the command's new state
    /// must match the unit root, while the replaced device may have a different
    /// provider type.
    pub fn new_replacement_unit(
        mut command: Command,
        template: PortableDeviceUnit,
        assets: Vec<AssetId>,
    ) -> Result<Self, Invalid> {
        let unit = unit::CompleteUnit::new(
            template,
            assets,
            state_mut(&mut command)?,
            unit::ReplacementPolicy::AllowTypeChange,
        )?;
        Ok(Self {
            command,
            authored: AuthoredInitialization::Unit(unit),
        })
    }

    /// Initialize one complete portable subtree whose manifest entries may each
    /// reuse an existing destination original or register a fresh original
    /// inside the same project edit. Fresh `AssetId`s are reserved against
    /// `project` without mutating it and exposed via `reserved_unit_originals`
    /// and `resolved_unit_assets` for the prepared audio stage.
    pub fn new_unit_with_originals(
        project: &Project,
        mut command: Command,
        template: PortableDeviceUnit,
        resolutions: Vec<UnitAssetResolution>,
    ) -> Result<Self, Invalid> {
        let unit = unit::CompleteUnit::new_with_originals(
            project,
            template,
            resolutions,
            state_mut(&mut command)?,
            unit::ReplacementPolicy::SameProvider,
        )?;
        Ok(Self {
            command,
            authored: AuthoredInitialization::Unit(unit),
        })
    }

    /// Cross-type replacement variant of `new_unit_with_originals` for catalog
    /// default replacement while reserving fresh destination original IDs.
    pub fn new_replacement_unit_with_originals(
        project: &Project,
        mut command: Command,
        template: PortableDeviceUnit,
        resolutions: Vec<UnitAssetResolution>,
    ) -> Result<Self, Invalid> {
        let unit = unit::CompleteUnit::new_with_originals(
            project,
            template,
            resolutions,
            state_mut(&mut command)?,
            unit::ReplacementPolicy::AllowTypeChange,
        )?;
        Ok(Self {
            command,
            authored: AuthoredInitialization::Unit(unit),
        })
    }

    /// Destination `AssetId` for every entry in the unit's asset manifest
    /// (combining reused existing IDs and reserved fresh IDs).
    pub fn resolved_unit_assets(&self) -> &[AssetId] {
        match &self.authored {
            AuthoredInitialization::Comparison(_) => &[],
            AuthoredInitialization::Unit(unit) => unit.resolved_assets(),
        }
    }

    /// Exact fresh destination `AssetId`s reserved for manifest entries that
    /// register new originals, in manifest order.
    pub fn reserved_unit_originals(&self) -> &[ReservedUnitOriginal] {
        match &self.authored {
            AuthoredInitialization::Comparison(_) => &[],
            AuthoredInitialization::Unit(unit) => unit.reserved_originals(),
        }
    }

    /// The original bounded command, for control-side inspection/diagnostics.
    pub fn command(&self) -> &Command {
        &self.command
    }

    pub fn comparison(&self) -> Option<&DeviceComparison> {
        match &self.authored {
            AuthoredInitialization::Comparison(comparison) => comparison.as_ref(),
            AuthoredInitialization::Unit(unit) => unit.comparison(),
        }
    }

    pub(crate) fn apply(self, project: &mut Project) -> Result<Created, Invalid> {
        let comparison = match self.authored {
            AuthoredInitialization::Comparison(comparison) => comparison,
            AuthoredInitialization::Unit(unit) => return unit.apply(project, self.command),
        };
        let (created, device) = apply_root(project, self.command)?;
        let entry = project
            .device(device)
            .ok_or(Invalid("initialized device is missing"))?;
        if entry.comparison() == comparison.as_ref() {
            return Ok(created);
        }
        let state = entry.state().clone();
        project.apply(Command::ReplaceDeviceComparison {
            device,
            state,
            comparison,
        })?;
        Ok(created)
    }
}

fn replacement(command: &Command) -> Option<DeviceId> {
    match command {
        Command::ReplaceDeviceState { device, .. }
        | Command::LoadInstrument {
            replacing: Some(device),
            ..
        } => Some(*device),
        _ => None,
    }
}

fn apply_root(project: &mut Project, command: Command) -> Result<(Created, DeviceId), Invalid> {
    let replacement = replacement(&command);
    let slot = match &command {
        Command::InsertTrackWithDevice { slot, .. }
        | Command::InsertDefaultTrackWithDevice { slot, .. }
        | Command::InsertDefaultTrackWithDeviceAndGain { slot, .. } => Some(*slot),
        _ => None,
    };
    let created = project.apply(command)?;
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
    Ok((created, device))
}

fn state_mut(command: &mut Command) -> Result<&mut NativeDeviceState, Invalid> {
    match command {
        Command::InsertDevice { state, .. }
        | Command::InsertNestedDevice { state, .. }
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
