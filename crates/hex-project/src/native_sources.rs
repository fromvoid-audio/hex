//! Retained, project-owned native source definitions, independent of tracks.
//!
//! This inventory owns authored chains and their media references, not live DSP.
//! Entries are ordered by creation, have globally unique source/device identities,
//! and survive track removal and bounded history expiry. Only explicit removal
//! deletes one (Undo of creation deliberately restores its prior state). Binding
//! and shared-runtime admission are separate increments: every entry here is
//! unassigned and cannot acquire references through this API. When bindings are
//! introduced, removal must refuse referenced entries until explicitly unlinked.
//! Source device state/bypass/comparison edits use the existing DeviceId commands.
use crate::{Command, Created, Device, DeviceId, DeviceSnapshot, Invalid, Project, SourceId};

pub const MAX_NATIVE_SOURCES: usize = crate::MAX_TRACKS;
pub const MAX_SOURCE_DEVICES: usize = crate::MAX_DEVICES_PER_TRACK + 1;

/// An ordered chain with one designated instrument and source-owned processing.
/// The neutral model preserves provider state without constructing or validating
/// DSP; native provider class/admission remains the session/engine's boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSource {
    pub(crate) id: SourceId,
    pub(crate) name: String,
    pub(crate) devices: Vec<Device>,
    pub(crate) instrument_index: usize,
}
impl NativeSource {
    pub fn id(&self) -> SourceId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn devices(&self) -> &[Device] {
        &self.devices
    }
    pub fn instrument_index(&self) -> usize {
        self.instrument_index
    }
}

/// Complete authored source transport, including inactive comparison state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSourceSnapshot {
    pub id: u64,
    pub name: String,
    pub devices: Vec<DeviceSnapshot>,
    pub instrument_index: usize,
}
impl From<&NativeSource> for NativeSourceSnapshot {
    fn from(source: &NativeSource) -> Self {
        Self {
            id: source.id.value(),
            name: source.name.clone(),
            instrument_index: source.instrument_index,
            devices: source
                .devices
                .iter()
                .map(|d| DeviceSnapshot {
                    id: d.id().value(),
                    state: d.state().clone(),
                    bypassed: d.bypassed(),
                    comparison: d.comparison().cloned(),
                })
                .collect(),
        }
    }
}

pub(crate) fn validate_chain(name: &str, count: usize, instrument: usize) -> Result<(), Invalid> {
    crate::edit::name_valid(name)?;
    if count == 0 || count > MAX_SOURCE_DEVICES || instrument >= count {
        return Err(Invalid(
            "native source needs a bounded chain and an instrument index",
        ));
    }
    Ok(())
}
impl Project {
    pub fn native_sources(&self) -> &[NativeSource] {
        &self.native_sources
    }
    pub fn native_source(&self, source: SourceId) -> Option<&NativeSource> {
        self.native_sources.iter().find(|s| s.id == source)
    }
}

pub(crate) fn apply(project: &mut Project, command: Command) -> Result<Created, Invalid> {
    match command {
        Command::CreateNativeSource {
            name,
            devices,
            instrument_index,
        } => {
            validate_chain(&name, devices.len(), instrument_index)?;
            if project.native_sources.len() >= MAX_NATIVE_SOURCES {
                return Err(Invalid("native source limit reached"));
            }
            if crate::returns::device_count(project)?
                .checked_add(devices.len())
                .is_none_or(|count| count > crate::MAX_DEVICES)
            {
                return Err(Invalid("project device limit reached"));
            }
            let count = devices
                .len()
                .checked_add(1)
                .ok_or(Invalid("source identity count overflow"))?;
            let (first, _) = project.allocate_id_range(count)?;
            let id = SourceId(first);
            let devices = devices
                .into_iter()
                .enumerate()
                .map(|(index, state)| {
                    // allocate_id_range proved this complete inclusive range fits.
                    Device {
                        id: DeviceId(first + 1 + index as u64),
                        state,
                        bypassed: false,
                        comparison: None,
                    }
                })
                .collect();
            project.native_sources.push(NativeSource {
                id,
                name,
                devices,
                instrument_index,
            });
            Ok(Created::NativeSource(id))
        }
        Command::RenameNativeSource { source, name } => {
            crate::edit::name_valid(&name)?;
            project
                .native_sources
                .iter_mut()
                .find(|s| s.id == source)
                .ok_or(Invalid("native source no longer exists"))?
                .name = name;
            Ok(Created::None)
        }
        Command::RemoveNativeSource(source) => {
            let index = project
                .native_sources
                .iter()
                .position(|s| s.id == source)
                .ok_or(Invalid("native source no longer exists"))?;
            project.native_sources.remove(index);
            Ok(Created::None)
        }
        _ => Err(Invalid("unsupported native source command")),
    }
}

#[cfg(test)]
mod tests;
