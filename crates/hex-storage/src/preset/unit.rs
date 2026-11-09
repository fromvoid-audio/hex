//! Portable preset transport. Asset digests describe the required originals;
//! decoding grants no access to media and makes no project or audio changes.

use super::{DeviceClass, Error, Header, PRESET_FORMAT, Preset, decode, publish_new, read_bounded};
use crate::schema::{
    device_assets::{ComparisonDto, Reference, bounded_references, required_comparison},
    device_automation::bounded,
    device_modulation::BankDto,
};
use hex_lattice_contract::{DeviceTypeId, MAX_VST3_DEVICE_STATE_BYTES, NativeDeviceState};
use hex_project::{
    DeviceUnitClass, MAX_PORTABLE_DEVICE_UNIT_ASSETS, MAX_PORTABLE_DEVICE_UNIT_NODES,
    MAX_PORTABLE_DEVICE_UNIT_PAYLOAD_BYTES, OriginalSourceFacts, PortableDeviceAsset,
    PortableDeviceNode, PortableDeviceUnit,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const UNIT_PRESET_SCHEMA: u32 = 3;
/// Compact byte arrays need up to four bytes per payload byte. The additional
/// space covers the bounded device, asset, source and route metadata.
pub const MAX_UNIT_PRESET_BYTES: usize =
    4 * MAX_PORTABLE_DEVICE_UNIT_PAYLOAD_BYTES + 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetContents {
    State(Box<Preset>),
    Unit(PortableDeviceUnit),
}

impl PresetContents {
    /// The slot class and root provider, independent of the envelope version.
    pub fn identity(&self) -> (DeviceClass, DeviceTypeId) {
        match self {
            Self::State(preset) => (preset.class, preset.state.type_id()),
            Self::Unit(unit) => (unit.class().into(), unit.nodes()[0].state.type_id()),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    schema: u32,
    class: DeviceClass,
    #[serde(deserialize_with = "nodes")]
    nodes: Vec<Node>,
    #[serde(deserialize_with = "assets")]
    assets: Vec<Asset>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Node {
    #[serde(deserialize_with = "parent")]
    parent: Option<usize>,
    bypassed: bool,
    type_id: String,
    state_version: u32,
    #[serde(deserialize_with = "payload")]
    payload: Vec<u8>,
    input_gain_millidb: i32,
    output_gain_millidb: i32,
    #[serde(deserialize_with = "bounded_references")]
    references: Vec<Reference>,
    #[serde(deserialize_with = "required_comparison")]
    comparison: Option<ComparisonDto>,
    modulation: BankDto,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    source_rate: u32,
    source_frames: u64,
    law_version: u32,
    content_digest: [u8; 32],
}

fn nodes<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<Node>, D::Error> {
    bounded(decoder, MAX_PORTABLE_DEVICE_UNIT_NODES)
}

fn assets<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<Asset>, D::Error> {
    bounded(decoder, MAX_PORTABLE_DEVICE_UNIT_ASSETS)
}

fn parent<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Option<usize>, D::Error> {
    Option::deserialize(decoder)
}

fn payload<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<u8>, D::Error> {
    bounded(decoder, MAX_VST3_DEVICE_STATE_BYTES)
}

fn invalid(error: crate::StorageError) -> Error {
    match error {
        crate::StorageError::Invalid(reason) => Error::Invalid(reason),
        _ => Error::Invalid("invalid portable device preset"),
    }
}

impl Node {
    fn from_model(node: &PortableDeviceNode) -> Result<Self, Error> {
        let state = &node.state;
        Ok(Self {
            parent: node.parent,
            bypassed: node.bypassed,
            type_id: state
                .type_id()
                .as_str()
                .map_err(|_| Error::Invalid("invalid portable device provider"))?
                .into(),
            state_version: state.state_version(),
            payload: state.payload().to_vec(),
            input_gain_millidb: state.input_gain_millidb(),
            output_gain_millidb: state.output_gain_millidb(),
            references: state
                .asset_references()
                .iter()
                .map(Reference::from_model)
                .collect(),
            comparison: node
                .comparison
                .as_ref()
                .map(ComparisonDto::from_comparison)
                .transpose()
                .map_err(invalid)?,
            modulation: BankDto::from_model(&node.modulation, true).map_err(invalid)?,
        })
    }

    fn into_model(self) -> Result<PortableDeviceNode, Error> {
        let type_id = DeviceTypeId::new(&self.type_id)
            .map_err(|_| Error::Invalid("invalid portable device provider"))?;
        let references = self
            .references
            .into_iter()
            .map(Reference::into_model)
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid)?;
        let state = NativeDeviceState::new(type_id, self.state_version, self.payload)
            .and_then(|state| state.with_input_gain_millidb(self.input_gain_millidb))
            .and_then(|state| state.with_output_gain_millidb(self.output_gain_millidb))
            .and_then(|state| state.with_asset_references(references))
            .map_err(|_| Error::Invalid("invalid portable device state"))?;
        let comparison = self
            .comparison
            .map(|value| value.into_comparison(&state))
            .transpose()
            .map_err(invalid)?;
        Ok(PortableDeviceNode {
            parent: self.parent,
            state,
            comparison,
            bypassed: self.bypassed,
            modulation: self.modulation.into_model(true).map_err(invalid)?,
        })
    }
}

impl From<&PortableDeviceAsset> for Asset {
    fn from(asset: &PortableDeviceAsset) -> Self {
        Self {
            source_rate: asset.source_rate,
            source_frames: asset.source_frames,
            law_version: asset.facts.law_version(),
            content_digest: asset.facts.content_digest(),
        }
    }
}

impl TryFrom<Asset> for PortableDeviceAsset {
    type Error = Error;

    fn try_from(asset: Asset) -> Result<Self, Error> {
        Ok(Self {
            source_rate: asset.source_rate,
            source_frames: asset.source_frames,
            facts: OriginalSourceFacts::new(asset.law_version, asset.content_digest)
                .map_err(|error| Error::Invalid(error.0))?,
        })
    }
}

pub fn encode_unit(unit: &PortableDeviceUnit) -> Result<Vec<u8>, Error> {
    let envelope = Envelope {
        format: PRESET_FORMAT.into(),
        schema: UNIT_PRESET_SCHEMA,
        class: unit.class().into(),
        nodes: unit
            .nodes()
            .iter()
            .map(Node::from_model)
            .collect::<Result<_, _>>()?,
        assets: unit.assets().iter().map(Asset::from).collect(),
    };
    let bytes = serde_json::to_vec(&envelope)?;
    if bytes.len() > MAX_UNIT_PRESET_BYTES {
        return Err(Error::Invalid("portable device preset exceeds byte limit"));
    }
    Ok(bytes)
}

/// Read both historical state-only presets and current complete units. The
/// state-only APIs refuse a unit so callers cannot accidentally flatten it.
pub fn decode_contents(bytes: &[u8]) -> Result<PresetContents, Error> {
    if bytes.len() > MAX_UNIT_PRESET_BYTES {
        return Err(Error::Invalid("portable device preset exceeds byte limit"));
    }
    let header: Header = serde_json::from_slice(bytes)?;
    if header.format != PRESET_FORMAT {
        return Err(Error::Invalid("not a hex device preset file"));
    }
    if header.schema != UNIT_PRESET_SCHEMA {
        return decode(bytes).map(|state| PresetContents::State(Box::new(state)));
    }
    let envelope: Envelope = serde_json::from_slice(bytes)?;
    let class = match envelope.class {
        DeviceClass::Instrument => DeviceUnitClass::Instrument,
        DeviceClass::Effect => DeviceUnitClass::Effect,
    };
    let nodes = envelope
        .nodes
        .into_iter()
        .map(Node::into_model)
        .collect::<Result<_, _>>()?;
    let assets = envelope
        .assets
        .into_iter()
        .map(PortableDeviceAsset::try_from)
        .collect::<Result<_, _>>()?;
    PortableDeviceUnit::new(class, nodes, assets)
        .map(PresetContents::Unit)
        .map_err(|error| Error::Invalid(error.0))
}

pub fn load_contents(path: &Path) -> Result<PresetContents, Error> {
    decode_contents(&read_bounded(path, MAX_UNIT_PRESET_BYTES)?)
}

/// Atomically publish a complete authored unit; never overwrite a destination.
pub fn save_unit_new(path: &Path, unit: &PortableDeviceUnit) -> Result<(), Error> {
    publish_new(path, || encode_unit(unit))
}

#[cfg(test)]
pub(super) mod tests;
