//! Source-owned membership edits use the retained registry's atomic replacement.
use crate::{Created, Device, DeviceId, Invalid, Project, SourceId};

impl Project {
    pub(crate) fn source_owning_device(&self, device: DeviceId) -> Option<SourceId> {
        self.native_sources()
            .iter()
            .find(|source| source.devices().any(|item| item.id() == device))
            .map(super::NativeSource::id)
    }

    /// Preserve the designated instrument while editing one bounded flat chain.
    /// The registry publishes its new immutable definition and binding views only
    /// after the edit succeeds; the Editor validates/prepares before committing.
    fn edit_source_chain(
        &mut self,
        source: SourceId,
        edit: impl FnOnce(&mut Vec<Device>) -> Result<(), Invalid>,
    ) -> Result<(), Invalid> {
        self.edit_native_source(source, |definition| {
            let instrument = definition.instrument.id();
            let mut chain = std::mem::take(&mut definition.effects);
            if definition.instrument_index > chain.len() {
                return Err(Invalid("source instrument position is out of bounds"));
            }
            chain
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve source device chain"))?;
            chain.insert(definition.instrument_index, definition.instrument.clone());
            edit(&mut chain)?;
            let position = chain
                .iter()
                .position(|device| device.id() == instrument)
                .ok_or(Invalid("source instrument is missing"))?;
            definition.instrument = chain.remove(position);
            definition.instrument_index = position;
            definition.effects = chain;
            Ok(())
        })
    }

    pub(crate) fn reparent_source_device(
        &mut self,
        source: SourceId,
        device: DeviceId,
        parent: Option<DeviceId>,
    ) -> Result<(), Invalid> {
        let definition = self
            .native_source(source)
            .ok_or(Invalid("native source no longer exists"))?;
        let current = definition
            .devices()
            .find(|item| item.id() == device)
            .ok_or(Invalid("source device no longer exists"))?;
        if current.parent() == parent {
            return Ok(());
        }
        if device == definition.instrument().id() {
            return Err(Invalid("source instrument must be a serial root"));
        }
        self.edit_source_chain(source, |chain| {
            let index = chain
                .iter()
                .position(|item| item.id() == device)
                .ok_or(Invalid("source device no longer exists"))?;
            let span = crate::device_inventory::serial_unit_span(chain, index)?;
            let target = match parent {
                Some(parent) => chain
                    .iter()
                    .position(|item| item.id() == parent)
                    .ok_or(Invalid("serial parent must belong to the same source"))?,
                None => chain[..=index]
                    .iter()
                    .rposition(|item| item.parent().is_none())
                    .ok_or(Invalid("serial root no longer exists"))?,
            };
            if parent.is_some() && span.contains(&target) {
                return Err(Invalid("a device cannot contain itself or an ancestor"));
            }
            let destination = crate::device_inventory::serial_unit_span(chain, target)?.end;
            crate::device::move_serial_unit(chain, index, destination)?;
            let moved = chain
                .iter_mut()
                .find(|item| item.id() == device)
                .ok_or(Invalid("moving device no longer exists"))?;
            moved.parent = parent;
            Ok(())
        })
    }

    pub(crate) fn insert_source_child(
        &mut self,
        source: SourceId,
        parent: DeviceId,
        state: hex_lattice_contract::NativeDeviceState,
    ) -> Result<Created, Invalid> {
        let count = self
            .native_source(source)
            .ok_or(Invalid("native source no longer exists"))?
            .devices()
            .count();
        if count >= super::MAX_SOURCE_DEVICES {
            return Err(Invalid("source device limit reached"));
        }
        if crate::returns::device_count(self)? >= crate::MAX_DEVICES {
            return Err(Invalid("project device limit reached"));
        }
        let id = DeviceId(self.allocate_id()?);
        self.edit_source_chain(source, |chain| {
            let position = chain
                .iter()
                .position(|item| item.id() == parent)
                .ok_or(Invalid("source parent no longer exists"))?;
            let end = crate::device_inventory::serial_unit_span(chain, position)?.end;
            chain
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve source child"))?;
            chain.insert(
                end,
                Device {
                    modulation: Default::default(),
                    id,
                    parent: Some(parent),
                    state,
                    bypassed: false,
                    comparison: None,
                    automation: Vec::new(),
                },
            );
            Ok(())
        })?;
        Ok(Created::Device(id))
    }

    pub(crate) fn remove_source_device(
        &mut self,
        source: SourceId,
        device: DeviceId,
    ) -> Result<(), Invalid> {
        let definition = self
            .native_source(source)
            .ok_or(Invalid("native source no longer exists"))?;
        if definition.instrument().id() == device {
            return Err(Invalid(
                "source instrument requires explicit source removal or unlinking",
            ));
        }
        self.edit_source_chain(source, |chain| {
            let index = chain
                .iter()
                .position(|item| item.id() == device)
                .ok_or(Invalid("source device no longer exists"))?;
            let span = crate::device_inventory::serial_unit_span(chain, index)?;
            drop(chain.drain(span));
            Ok(())
        })
    }
    pub(crate) fn move_source_device(
        &mut self,
        source: SourceId,
        device: DeviceId,
        before: Option<DeviceId>,
    ) -> Result<(), Invalid> {
        let definition = self
            .native_source(source)
            .ok_or(Invalid("native source no longer exists"))?;
        if !definition.devices().any(|item| item.id() == device) {
            return Err(Invalid("device does not belong to the source"));
        }
        // These unchanged placements also remain harmless on a bound singleton.
        if before == Some(device)
            || (before.is_none()
                && definition
                    .devices()
                    .last()
                    .is_some_and(|item| item.id() == device))
        {
            return Ok(());
        }
        self.edit_source_chain(source, |chain| {
            let index = chain
                .iter()
                .position(|item| item.id() == device)
                .ok_or(Invalid("source device no longer exists"))?;
            let destination = match before {
                Some(anchor) => chain
                    .iter()
                    .position(|item| item.id() == anchor)
                    .ok_or(Invalid("source device anchor no longer exists"))?,
                None => chain.len(),
            };
            crate::device::move_serial_unit(chain, index, destination)
        })
    }
}

#[cfg(test)]
mod order_tests;
#[cfg(test)]
mod removal_tests;
#[cfg(test)]
mod tests;
