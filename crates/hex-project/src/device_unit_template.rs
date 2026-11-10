//! Portable authored subtrees. These values retain neither project identities,
//! file locations, live instances nor authority to use an original PCM source.
use crate::{
    AssetId, DeviceComparison, DeviceId, DeviceModulation, Invalid, ModulationDestination,
    ModulationRouteId, ModulationSourceId, OriginalSourceFacts, Project,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, NativeDeviceState, VST3_DEVICE_TYPE_ID,
};

pub const MAX_PORTABLE_DEVICE_UNIT_NODES: usize = crate::MAX_DEVICES_PER_TRACK + 1;
pub const MAX_PORTABLE_DEVICE_UNIT_ASSETS: usize = 256;
pub const MAX_PORTABLE_DEVICE_UNIT_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceUnitClass {
    Instrument,
    Effect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortableDeviceAsset {
    pub source_rate: u32,
    pub source_frames: u64,
    pub facts: OriginalSourceFacts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortableDeviceNode {
    /// Zero-based index in the same unit. Only the root has no parent.
    pub parent: Option<usize>,
    pub state: NativeDeviceState,
    pub comparison: Option<DeviceComparison>,
    pub bypassed: bool,
    pub modulation: DeviceModulation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortableDeviceUnit {
    class: DeviceUnitClass,
    nodes: Box<[PortableDeviceNode]>,
    assets: Box<[PortableDeviceAsset]>,
}

impl PortableDeviceUnit {
    /// Validate raw authored data from any transport before admitting it.
    /// Asset keys are one-based indexes into this unit's asset manifest.
    pub fn new(
        class: DeviceUnitClass,
        mut nodes: Vec<PortableDeviceNode>,
        assets: Vec<PortableDeviceAsset>,
    ) -> Result<Self, Invalid> {
        if nodes.is_empty()
            || nodes.len() > MAX_PORTABLE_DEVICE_UNIT_NODES
            || assets.len() > MAX_PORTABLE_DEVICE_UNIT_ASSETS
        {
            return Err(Invalid(
                "portable device unit inventory is outside its bounds",
            ));
        }
        for (index, asset) in assets.iter().enumerate() {
            if asset.source_rate == 0
                || asset.source_frames == 0
                || asset.source_frames > crate::MAX_AUDIO_DOMAIN_FRAMES
                || assets[..index]
                    .iter()
                    .any(|other| other.facts == asset.facts)
            {
                return Err(Invalid("invalid or repeated portable device asset"));
            }
        }
        let mut used = [false; MAX_PORTABLE_DEVICE_UNIT_ASSETS];
        let mut stack = [0; crate::device_inventory::MAX_SERIAL_DEVICE_DEPTH];
        let mut depth = 0;
        let mut bytes = 0usize;
        let mut modulators = 0usize;
        let mut routes = 0usize;
        let mut modulator_bytes = 0usize;
        for (index, node) in nodes.iter().enumerate() {
            let slot = match node.parent {
                None if index == 0 => 0,
                Some(parent) => stack[..depth]
                    .iter()
                    .position(|&candidate| candidate == parent)
                    .map(|level| level + 1)
                    .ok_or(Invalid("portable device parent is outside its subtree"))?,
                None => return Err(Invalid("portable device unit has more than one root")),
            };
            *stack
                .get_mut(slot)
                .ok_or(Invalid("portable device nesting exceeds its limit"))? = index;
            depth = slot + 1;

            if let Some(comparison) = &node.comparison {
                comparison.validate(&node.state)?;
            }
            validate_relative_modulation(node)?;
            modulators += node.modulation.sources().len();
            routes += node.modulation.routes().len();
            for source in node.modulation.sources() {
                modulator_bytes = modulator_bytes
                    .checked_add(
                        source
                            .state
                            .retained_control_metadata_bytes()
                            .ok_or(Invalid("portable modulator size overflow"))?,
                    )
                    .ok_or(Invalid("portable modulator size overflow"))?;
            }
            if modulators > crate::MAX_PROJECT_MODULATORS
                || routes > crate::MAX_PROJECT_MODULATION_ROUTES
                || modulator_bytes > crate::MAX_PROJECT_MODULATOR_STATE_BYTES
            {
                return Err(Invalid(
                    "portable device modulation inventory exceeds its limits",
                ));
            }
            let states = std::iter::once(&node.state)
                .chain(node.comparison.iter().map(DeviceComparison::inactive))
                .chain(node.modulation.sources().iter().map(|source| &source.state));
            for state in states {
                bytes = bytes
                    .checked_add(state.payload().len())
                    .filter(|&size| size <= MAX_PORTABLE_DEVICE_UNIT_PAYLOAD_BYTES)
                    .ok_or(Invalid("portable device state exceeds its byte limit"))?;
                for reference in state.asset_references() {
                    let index = usize::try_from(reference.asset.value() - 1)
                        .map_err(|_| Invalid("portable device asset key is invalid"))?;
                    if index >= assets.len() {
                        return Err(Invalid("portable device asset key is missing"));
                    }
                    used[index] = true;
                }
            }
        }
        if used[..assets.len()].contains(&false) {
            return Err(Invalid("portable device unit contains unused assets"));
        }
        for node in &mut nodes {
            strip_vst3_approvals(node)?;
        }
        Ok(Self {
            class,
            nodes: nodes.into_boxed_slice(),
            assets: assets.into_boxed_slice(),
        })
    }

    pub fn class(&self) -> DeviceUnitClass {
        self.class
    }

    pub fn nodes(&self) -> &[PortableDeviceNode] {
        &self.nodes
    }

    pub fn assets(&self) -> &[PortableDeviceAsset] {
        &self.assets
    }
}

fn strip_vst3_approvals(node: &mut PortableDeviceNode) -> Result<(), Invalid> {
    fn clear(state: &NativeDeviceState) -> Result<NativeDeviceState, Invalid> {
        // Rebinding the same references drops process-local approval while
        // keeping the immutable provider bytes and all authored fields.
        state
            .clone()
            .with_asset_references(state.asset_references().to_vec())
            .map_err(|_| Invalid("could not detach portable device state"))
    }
    if node.state.type_id() == VST3_DEVICE_TYPE_ID {
        node.state = clear(&node.state)?;
        if let Some(value) = &node.comparison {
            node.comparison = Some(DeviceComparison::new(
                value.active(),
                clear(value.inactive())?,
            ));
        }
    }
    if node
        .modulation
        .sources()
        .iter()
        .any(|slot| slot.state.type_id() == VST3_DEVICE_TYPE_ID)
    {
        let mut sources = node.modulation.sources().to_vec();
        for slot in &mut sources {
            if slot.state.type_id() == VST3_DEVICE_TYPE_ID {
                slot.state = clear(&slot.state)?;
            }
        }
        node.modulation = DeviceModulation::new(sources, node.modulation.routes().to_vec())?;
    }
    Ok(())
}

impl Project {
    /// Capture one authored serial subtree. The project inventory visits every
    /// canonical source once, so aliases never duplicate a device or its media.
    pub fn capture_portable_device_unit(
        &self,
        root: DeviceId,
    ) -> Result<PortableDeviceUnit, Invalid> {
        let class = if self
            .native_sources()
            .iter()
            .any(|source| source.instrument().id() == root)
        {
            DeviceUnitClass::Instrument
        } else {
            DeviceUnitClass::Effect
        };
        let mut ids = Vec::new();
        let mut nodes = Vec::new();
        let mut assets = Vec::new();
        for device in self.devices().skip_while(|device| device.id() != root) {
            let parent = if nodes.is_empty() {
                None
            } else {
                let Some(parent) = device
                    .parent()
                    .and_then(|parent| ids.iter().position(|&id| id == parent))
                else {
                    break;
                };
                Some(parent)
            };
            if nodes.len() == MAX_PORTABLE_DEVICE_UNIT_NODES {
                return Err(Invalid("portable device unit node limit reached"));
            }
            let state = portable_state(self, &mut assets, device.state())?;
            let comparison = device
                .comparison()
                .map(|comparison| {
                    portable_state(self, &mut assets, comparison.inactive())
                        .map(|inactive| DeviceComparison::new(comparison.active(), inactive))
                })
                .transpose()?;
            let modulation = relative_modulation(device.modulation())?;
            ids.push(device.id());
            nodes.push(PortableDeviceNode {
                parent,
                state,
                comparison,
                modulation,
                bypassed: device.bypassed(),
            });
        }
        if nodes.is_empty() {
            return Err(Invalid("device no longer exists"));
        }
        PortableDeviceUnit::new(class, nodes, assets)
    }
}

fn portable_state(
    project: &Project,
    assets: &mut Vec<PortableDeviceAsset>,
    original: &NativeDeviceState,
) -> Result<NativeDeviceState, Invalid> {
    if original.asset_references().is_empty() {
        return Ok(original.clone());
    }
    let mut references = Vec::with_capacity(original.asset_references().len());
    for reference in original.asset_references() {
        let source = project
            .asset(AssetId(reference.asset.value()))
            .ok_or(Invalid("device original asset is missing"))?;
        let facts = source.original_source().ok_or(Invalid(
            "device asset has no verified original content identity",
        ))?;
        let asset = PortableDeviceAsset {
            source_rate: source.source_sample_rate(),
            source_frames: source.source_frames(),
            facts,
        };
        let index = if let Some(index) = assets
            .iter()
            .position(|candidate| candidate.facts == asset.facts)
        {
            if assets[index] != asset {
                return Err(Invalid(
                    "device original content identity disagrees with its metadata",
                ));
            }
            index
        } else {
            if assets.len() == MAX_PORTABLE_DEVICE_UNIT_ASSETS {
                return Err(Invalid("portable device asset count exceeds its limit"));
            }
            assets.push(asset);
            assets.len() - 1
        };
        let key = DeviceAssetKey::new(index as u64 + 1)
            .map_err(|_| Invalid("portable device asset key is invalid"))?;
        references.push(DeviceAssetReference {
            asset: key,
            slot: reference.slot,
        });
    }
    // Provider/resource approval is project-local. Reconstruct only the
    // authored state, preserving its exact payload and host trims.
    NativeDeviceState::new(
        original.type_id(),
        original.state_version(),
        original.payload().to_vec(),
    )
    .and_then(|state| state.with_input_gain_millidb(original.input_gain_millidb()))
    .and_then(|state| state.with_output_gain_millidb(original.output_gain_millidb()))
    .and_then(|state| state.with_asset_references(references))
    .map_err(|_| Invalid("could not remap portable device assets"))
}

fn relative_modulation(bank: &DeviceModulation) -> Result<DeviceModulation, Invalid> {
    let source_key = |index: usize| {
        ModulationSourceId::new(index as u64 + 1)
            .map_err(|_| Invalid("portable modulator identity is out of bounds"))
    };
    let relative_source = |id| {
        let index = bank
            .sources()
            .iter()
            .position(|source| source.id == id)
            .ok_or(Invalid("portable modulation source is missing"))?;
        source_key(index)
    };
    let mut sources = bank.sources().to_vec();
    for (index, source) in sources.iter_mut().enumerate() {
        source.id = source_key(index)?;
    }
    let mut routes = bank.routes().to_vec();
    for (index, route) in routes.iter_mut().enumerate() {
        route.id = ModulationRouteId::new((sources.len() + index) as u64 + 1)
            .map_err(|_| Invalid("portable modulation route identity is out of bounds"))?;
        route.source = relative_source(route.source)?;
        if let ModulationDestination::Modulator { source, .. } = &mut route.destination {
            *source = relative_source(*source)?;
        }
    }
    DeviceModulation::new(sources, routes)
}

fn validate_relative_modulation(node: &PortableDeviceNode) -> Result<(), Invalid> {
    let sources = node.modulation.sources();
    let routes = node.modulation.routes();
    if sources
        .iter()
        .enumerate()
        .any(|(index, source)| source.id.value() != index as u64 + 1)
        || routes
            .iter()
            .enumerate()
            .any(|(index, route)| route.id.value() != (sources.len() + index) as u64 + 1)
    {
        return Err(Invalid(
            "portable modulation identities must be local to the device",
        ));
    }
    for route in routes {
        if let ModulationDestination::Device {
            device_type,
            target,
            vst3_class,
        } = route.destination
            && ((!target.is_host() && device_type != node.state.type_id())
                || crate::device_automation::automation_class(target, &node.state)? != vst3_class)
        {
            return Err(Invalid(
                "portable modulation destination belongs to a different provider",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
