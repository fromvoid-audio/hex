//! Retained, project-owned native source definitions, independent of tracks.
//!
//! This inventory owns authored chains and their media references, not live DSP.
//! Entries are ordered by creation, have globally unique source/device identities,
//! and survive track removal and bounded history expiry. Immutable track binding
//! handles refer to the same registry definition, not a copy or runtime. Only explicit removal
//! deletes one (Undo of creation deliberately restores its prior state). Source
//! mutations replace the registry and binding views together. A singleton source
//! can contribute to many tracks, while one explicitly selected contributor
//! receives its audio. Duplicating a track makes an independent source instance.
use crate::{
    Command, Created, Device, DeviceId, DeviceSnapshot, Invalid, Project, SourceId, TrackId,
};
use std::{ops::Deref, sync::Arc};

// The preceding format allowed256 retained sources PLUS256 track instruments.
// Migration retains both; the existing global device budget is unchanged.
pub const MAX_NATIVE_SOURCES: usize = crate::MAX_TRACKS * 2;
pub const MAX_SOURCE_DEVICES: usize = crate::MAX_DEVICES_PER_TRACK + 1;

/// An ordered chain with one designated instrument and source-owned processing.
/// The neutral model preserves provider state without constructing or validating
/// DSP; native provider class/admission remains the session/engine's boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSource {
    definition: Arc<SourceDefinition>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceDefinition {
    id: SourceId,
    name: String,
    instrument: Device,
    effects: Vec<Device>,
    instrument_index: usize,
    receiver: Option<TrackId>,
}
/// Immutable control-side binding view of a retained definition. This is not a
/// DSP instance or resource lease. Only the registry can replace authored state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceBinding(NativeSource);
impl Deref for SourceBinding {
    type Target = Device;
    fn deref(&self) -> &Device {
        self.0.instrument()
    }
}
impl SourceBinding {
    pub(crate) fn source_id(&self) -> SourceId {
        self.0.id()
    }
    pub(crate) fn receiver(&self) -> Option<TrackId> {
        self.0.receiver()
    }
}
impl NativeSource {
    pub fn id(&self) -> SourceId {
        self.definition.id
    }
    pub fn name(&self) -> &str {
        &self.definition.name
    }
    pub fn instrument(&self) -> &Device {
        &self.definition.instrument
    }
    pub fn instrument_index(&self) -> usize {
        self.definition.instrument_index
    }
    pub fn receiver(&self) -> Option<TrackId> {
        self.definition.receiver
    }
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        let index = self.instrument_index();
        self.definition
            .effects
            .iter()
            .take(index)
            .chain(std::iter::once(self.instrument()))
            .chain(self.definition.effects.iter().skip(index))
    }
    pub(crate) fn from_parts(
        id: SourceId,
        name: String,
        devices: Vec<Device>,
        instrument_index: usize,
        receiver: Option<TrackId>,
    ) -> Result<Self, Invalid> {
        validate_chain(&name, devices.len(), instrument_index)?;
        let mut devices = devices.into_iter();
        let mut effects: Vec<_> = devices.by_ref().take(instrument_index).collect();
        let instrument = devices
            .next()
            .ok_or(Invalid("native source instrument is missing"))?;
        effects.extend(devices);
        Ok(Self {
            definition: Arc::new(SourceDefinition {
                id,
                name,
                instrument,
                effects,
                instrument_index,
                receiver,
            }),
        })
    }
    pub(crate) fn binding(&self) -> Result<SourceBinding, Invalid> {
        if !self.is_singleton() {
            return Err(Invalid(
                "binding multi-device source chains is not supported yet",
            ));
        }
        Ok(SourceBinding(self.clone()))
    }
    pub(crate) fn is_singleton(&self) -> bool {
        self.definition.effects.is_empty()
    }
}

/// Complete authored source transport, including inactive comparison state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSourceSnapshot {
    pub id: u64,
    pub name: String,
    pub devices: Vec<DeviceSnapshot>,
    pub instrument_index: usize,
    pub receiver: Option<u64>,
}
impl From<&NativeSource> for NativeSourceSnapshot {
    fn from(source: &NativeSource) -> Self {
        Self {
            id: source.id().value(),
            name: source.name().to_owned(),
            instrument_index: source.instrument_index(),
            receiver: source.receiver().map(TrackId::value),
            devices: source
                .devices()
                .map(|d| DeviceSnapshot {
                    modulation: d.modulation.clone(),
                    parent: d.parent.map(|id| id.value()),
                    automation: d.automation().to_vec(),
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
        self.native_sources.iter().find(|s| s.id() == source)
    }

    /// MIDI-contributing tracks bound to this retained source, in track order.
    pub fn source_contributors(&self, source: SourceId) -> impl Iterator<Item = &crate::Track> {
        self.tracks
            .iter()
            .filter(move |track| track.source_id() == Some(source) && !track.retained_source_path())
    }
}

impl Project {
    fn edit_native_source(
        &mut self,
        id: SourceId,
        edit: impl FnOnce(&mut SourceDefinition) -> Result<(), Invalid>,
    ) -> Result<(), Invalid> {
        let index = self
            .native_sources
            .iter()
            .position(|s| s.id() == id)
            .ok_or(Invalid("native source no longer exists"))?;
        let mut definition = (*self.native_sources[index].definition).clone();
        edit(&mut definition)?;
        let updated = NativeSource {
            definition: Arc::new(definition),
        };
        self.native_sources[index] = updated.clone();
        for track in &mut self.tracks {
            if track.source_id() == Some(id) {
                track.instrument = Some(SourceBinding(updated.clone()));
            }
        }
        Ok(())
    }
    fn set_source_receiver(
        &mut self,
        id: SourceId,
        receiver: Option<TrackId>,
    ) -> Result<(), Invalid> {
        if let Some(track) = receiver
            && self
                .track(track)
                .is_none_or(|track| track.source_id() != Some(id))
        {
            return Err(Invalid("source receiver must be a bound track"));
        }
        self.edit_native_source(id, |definition| {
            definition.receiver = receiver;
            Ok(())
        })
    }
    pub(crate) fn register_instrument(
        &mut self,
        name: String,
        device: Device,
        receiver: TrackId,
    ) -> Result<SourceBinding, Invalid> {
        if self.native_sources.len() >= MAX_NATIVE_SOURCES {
            return Err(Invalid("native source limit reached"));
        }
        let id = SourceId(self.allocate_id()?);
        let source = NativeSource::from_parts(id, name, vec![device], 0, Some(receiver))?;
        self.native_sources.push(source.clone());
        Ok(SourceBinding(source))
    }
    pub(crate) fn copy_source_binding(
        &mut self,
        binding: &SourceBinding,
        receiver: TrackId,
    ) -> Result<SourceBinding, Invalid> {
        let source = &binding.0;
        let count = source.devices().count();
        if self.native_sources.len() >= MAX_NATIVE_SOURCES {
            return Err(Invalid("native source limit reached"));
        }
        if crate::returns::device_count(self)?
            .checked_add(count)
            .is_none_or(|count| count > crate::MAX_DEVICES)
        {
            return Err(Invalid("project device limit reached"));
        }
        let mut devices = Vec::new();
        devices
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve copied source devices"))?;
        devices.extend(source.devices().cloned());
        self.native_sources
            .try_reserve(1)
            .map_err(|_| Invalid("could not reserve copied source"))?;
        let identities = count
            .checked_add(1)
            .ok_or(Invalid("source identity count overflow"))?;
        let (first, _) = self.allocate_id_range(identities)?;
        for (index, device) in devices.iter_mut().enumerate() {
            // The reserved inclusive range covers the source plus every node.
            // Resolve parents against the frozen original, not rewritten IDs.
            device.parent = device
                .parent()
                .map(|parent| {
                    source
                        .devices()
                        .position(|item| item.id() == parent)
                        .map(|index| DeviceId(first + 1 + index as u64))
                        .ok_or(Invalid("copied source parent is missing"))
                })
                .transpose()?;
            device.id = DeviceId(first + 1 + index as u64);
        }
        let copied = NativeSource::from_parts(
            SourceId(first),
            source.name().to_owned(),
            devices,
            source.instrument_index(),
            Some(receiver),
        )?;
        self.native_sources.push(copied.clone());
        Ok(SourceBinding(copied))
    }
    pub(crate) fn bind_track_source(
        &mut self,
        track: crate::TrackId,
        source: Option<SourceId>,
    ) -> Result<(), Invalid> {
        let target = self.track(track).ok_or(Invalid("track no longer exists"))?;
        if target.is_group() {
            return Err(Invalid("group tracks cannot have an instrument source"));
        }
        if target.source_id() == source {
            return Ok(());
        }
        let position = target.instrument_position.unwrap_or(0);
        let old_receiver = target.source_id().filter(|old| {
            self.native_source(*old)
                .is_some_and(|source| source.receiver() == Some(track))
        });
        let assign_first_receiver = if let Some(id) = source {
            let source_definition = self
                .native_source(id)
                .ok_or(Invalid("native source no longer exists"))?;
            if !source_definition.is_singleton() {
                return Err(Invalid(
                    "binding multi-device source chains is not supported yet",
                ));
            }
            self.source_contributors(id).next().is_none() && source_definition.receiver().is_none()
        } else {
            false
        };
        let binding = source
            .map(|id| {
                self.native_source(id)
                    .ok_or(Invalid("native source no longer exists"))?
                    .binding()
            })
            .transpose()?;
        if let Some(old) = old_receiver {
            self.set_source_receiver(old, None)?;
        }
        let target = self.track_mut(track)?;
        target.instrument = binding;
        target.instrument_position = target.instrument.as_ref().map(|_| position);
        if assign_first_receiver {
            self.set_source_receiver(
                source.ok_or(Invalid("native source no longer exists"))?,
                Some(track),
            )?;
        }
        Ok(())
    }
    pub(crate) fn set_native_source_receiver(
        &mut self,
        source: SourceId,
        track: Option<TrackId>,
    ) -> Result<(), Invalid> {
        self.set_source_receiver(source, track)
    }
    pub(crate) fn validate_source_bindings(&self) -> Result<(), Invalid> {
        for track in self.tracks() {
            if track.retained_source_path() && !track.receives_source_audio() {
                return Err(Invalid("retained source path requires its bound receiver"));
            }
            if let Some(binding) = &track.instrument {
                let source = self
                    .native_source(binding.source_id())
                    .ok_or(Invalid("track source is missing"))?;
                if track.is_group()
                    || !source.is_singleton()
                    || !Arc::ptr_eq(&source.definition, &binding.0.definition)
                {
                    return Err(Invalid(
                        "track source binding does not match retained definition",
                    ));
                }
            }
        }
        for source in &self.native_sources {
            if let Some(receiver) = source.receiver() {
                let track = self
                    .track(receiver)
                    .ok_or(Invalid("source receiver track is missing"))?;
                if track.is_group() || track.source_id() != Some(source.id()) {
                    return Err(Invalid("source receiver must be a bound track"));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn edit_source_device(
        &mut self,
        source: SourceId,
        device: DeviceId,
        edit: impl FnOnce(&mut Device) -> Result<(), Invalid>,
    ) -> Result<(), Invalid> {
        self.edit_native_source(source, |definition| {
            let target = if definition.instrument.id == device {
                &mut definition.instrument
            } else {
                definition
                    .effects
                    .iter_mut()
                    .find(|d| d.id == device)
                    .ok_or(Invalid("source device no longer exists"))?
            };
            edit(target)
        })
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
                        modulation: Default::default(),
                        parent: None,
                        automation: Vec::new(),
                        id: DeviceId(first + 1 + index as u64),
                        state,
                        bypassed: false,
                        comparison: None,
                    }
                })
                .collect();
            project.native_sources.push(NativeSource::from_parts(
                id,
                name,
                devices,
                instrument_index,
                None,
            )?);
            Ok(Created::NativeSource(id))
        }
        Command::RenameNativeSource { source, name } => {
            crate::edit::name_valid(&name)?;
            project.edit_native_source(source, |definition| {
                definition.name = name;
                Ok(())
            })?;
            Ok(Created::None)
        }
        Command::BindTrackSource { track, source } => {
            project.bind_track_source(track, source)?;
            Ok(Created::None)
        }
        Command::SetNativeSourceReceiver { source, track } => {
            project.set_native_source_receiver(source, track)?;
            Ok(Created::None)
        }
        Command::RemoveNativeSource(source) => {
            if project
                .tracks()
                .iter()
                .any(|t| t.source_id() == Some(source))
            {
                return Err(Invalid(
                    "native source is still assigned to a track · unlink it first",
                ));
            }
            let index = project
                .native_sources
                .iter()
                .position(|s| s.id() == source)
                .ok_or(Invalid("native source no longer exists"))?;
            project.native_sources.remove(index);
            Ok(Created::None)
        }
        _ => Err(Invalid("unsupported native source command")),
    }
}

#[cfg(test)]
mod binding_tests;
#[cfg(test)]
mod tests;
