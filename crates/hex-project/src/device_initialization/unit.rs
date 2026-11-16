//! Complete canonical install. File reading and runtime preparation stay with
//! the caller; all authored changes take place inside the one editor candidate.

use super::{apply_root, replacement, state_mut};
use crate::{AssetId, Command, Created, DeviceComparison, Invalid, PortableDeviceUnit, Project};
use hex_lattice_contract::NativeDeviceState;

mod remap;

#[derive(Clone, Debug)]
pub(super) struct CompleteUnit {
    template: PortableDeviceUnit,
    assets: Box<[AssetId]>,
}

impl CompleteUnit {
    pub(super) fn new(
        template: PortableDeviceUnit,
        assets: Vec<AssetId>,
        target: &NativeDeviceState,
    ) -> Result<Self, Invalid> {
        if template.assets().len() != assets.len() {
            return Err(Invalid("portable device asset resolution is incomplete"));
        }
        if template.nodes()[0].state.type_id() != target.type_id() {
            return Err(Invalid("portable unit does not match the target provider"));
        }
        Ok(Self {
            template,
            assets: assets.into_boxed_slice(),
        })
    }

    pub(super) fn comparison(&self) -> Option<&DeviceComparison> {
        self.template.nodes()[0].comparison.as_ref()
    }

    pub(super) fn apply(
        self,
        project: &mut Project,
        mut command: Command,
    ) -> Result<Created, Invalid> {
        remap::validate_assets(project, &self.template, &self.assets)?;
        let nodes = self.template.nodes();
        let root = &nodes[0];
        if let Some(target) = replacement(&command) {
            let location = project
                .device_location(target)
                .ok_or(Invalid("portable unit replacement target no longer exists"))?;
            if location.class != self.template.class()
                || location.device.state().type_id() != root.state.type_id()
            {
                return Err(Invalid(
                    "portable unit does not match the replacement target",
                ));
            }
            // Existing remove commands own subtree and target cleanup. Keep the
            // actual root, owner, and any compatible root automation intact.
            let children: Vec<_> = project
                .devices()
                .filter(|item| item.parent() == Some(target))
                .map(crate::Device::id)
                .collect();
            for child in children {
                project.apply(Command::RemoveDevice(child))?;
            }
        }

        *state_mut(&mut command)? = remap::map_state(&root.state, &self.assets)?;
        let (created, root_id) = apply_root(project, command)?;
        if project
            .device_location(root_id)
            .is_none_or(|location| location.class != self.template.class())
        {
            return Err(Invalid("portable unit does not match the destination slot"));
        }

        let mut device_ids = Vec::with_capacity(nodes.len());
        for node in nodes {
            let device = if let Some(parent) = node.parent {
                let parent = *device_ids
                    .get(parent)
                    .ok_or(Invalid("portable unit parent is missing"))?;
                let state = remap::map_state(&node.state, &self.assets)?;
                let Created::Device(child) =
                    project.apply(Command::InsertNestedDevice { parent, state })?
                else {
                    return Err(Invalid("portable unit child was not created"));
                };
                child
            } else {
                root_id
            };
            device_ids.push(device);
            let comparison = remap::map_comparison(node.comparison.as_ref(), &self.assets)?;
            project.edit_device(device, |entry| {
                entry.bypassed = node.bypassed;
                entry.comparison = comparison;
                Ok(())
            })?;
            let modulation = remap::remap_bank(&node.modulation, project.next_id)?;
            project.apply(Command::ReplaceDeviceModulation { device, modulation })?;
        }
        Ok(created)
    }
}

#[cfg(test)]
mod tests;
