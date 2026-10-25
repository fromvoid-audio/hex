//! Schema45 adds mandatory retained native source definitions to strict schema44.
//! Schemas1–44 migrate to an empty inventory, keeping existing track instruments
//! unchanged; adoption into source bindings is a later model change. Decoding is
//! in-memory only. The privately stripped schema44 core composes the existing
//! codec, not a public downgrade that may discard retained source state.
use crate::{Project, StorageError};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use hex_project::{DeviceSnapshot, NativeSourceSnapshot};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    id: u64,
    name: String,
    instrument_index: usize,
    #[serde(deserialize_with = "devices")]
    devices: Vec<SourceDevice>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceDevice {
    device: SourceState,
    #[serde(deserialize_with = "references")]
    references: Vec<Reference>,
    #[serde(deserialize_with = "super::device_assets::required_comparison")]
    comparison: Option<super::device_assets::ComparisonDto>,
}
// New current-format envelopes require every host field; legacy DTO defaults
// apply only to their evidenced older schemas, never to a retained source.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceState {
    id: u64,
    bypassed: bool,
    type_id: String,
    state_version: u32,
    #[serde(deserialize_with = "payload")]
    payload: Vec<u8>,
    input_gain_millidb: i32,
    output_gain_millidb: i32,
}
fn payload<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    bounded(d, hex_lattice_contract::MAX_NATIVE_DEVICE_STATE_BYTES)
}
impl From<super::DeviceDtoV14> for SourceState {
    fn from(d: super::DeviceDtoV14) -> Self {
        Self {
            id: d.id,
            bypassed: d.bypassed,
            type_id: d.type_id,
            state_version: d.state_version,
            payload: d.payload,
            input_gain_millidb: d.input_gain_millidb,
            output_gain_millidb: d.output_gain_millidb,
        }
    }
}
impl From<SourceState> for super::DeviceDtoV14 {
    fn from(d: SourceState) -> Self {
        Self {
            id: d.id,
            bypassed: d.bypassed,
            type_id: d.type_id,
            state_version: d.state_version,
            payload: d.payload,
            input_gain_millidb: d.input_gain_millidb,
            output_gain_millidb: d.output_gain_millidb,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    slot: u32,
    asset: u64,
}
#[derive(Deserialize)]
struct Inventory(#[serde(deserialize_with = "sources")] Vec<Source>);

fn bounded<'de, D, T>(d: D, limit: usize) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Items<T>(usize, std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Items<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded native source inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == self.0 {
                    return Err(serde::de::Error::custom(
                        "native source inventory limit reached",
                    ));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Items(limit, std::marker::PhantomData))
}
fn sources<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Source>, D::Error> {
    bounded(d, hex_project::MAX_NATIVE_SOURCES)
}
fn devices<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SourceDevice>, D::Error> {
    bounded(d, hex_project::MAX_SOURCE_DEVICES)
}
fn references<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Reference>, D::Error> {
    bounded(d, hex_lattice_contract::MAX_DEVICE_ASSET_REFERENCES)
}
fn invalid(e: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(e.0)
}

impl SourceDevice {
    fn from_device(d: &DeviceSnapshot) -> Result<Self, StorageError> {
        Ok(Self {
            device: super::device_to_dto(d.id, d.bypassed, &d.state)?.into(),
            references: d
                .state
                .asset_references()
                .iter()
                .map(|r| Reference {
                    slot: r.slot.value(),
                    asset: r.asset.value(),
                })
                .collect(),
            comparison: d
                .comparison
                .as_ref()
                .map(super::device_assets::ComparisonDto::from_comparison)
                .transpose()?,
        })
    }
    fn into_device(self) -> Result<DeviceSnapshot, StorageError> {
        let mut d = super::device_dto_into_snapshot(self.device.into())?;
        let refs = self
            .references
            .into_iter()
            .map(|r| {
                Ok(DeviceAssetReference {
                    slot: DeviceAssetSlot::new(r.slot)
                        .map_err(|_| StorageError::Invalid("invalid source asset slot"))?,
                    asset: DeviceAssetKey::new(r.asset)
                        .map_err(|_| StorageError::Invalid("invalid source asset identity"))?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        d.state = d
            .state
            .with_asset_references(refs)
            .map_err(|_| StorageError::Invalid("invalid source asset references"))?;
        d.comparison = self
            .comparison
            .map(|c| c.into_comparison(&d.state))
            .transpose()?;
        Ok(d)
    }
}

pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let mut snapshot = project.snapshot();
    Project::from_snapshot(snapshot.clone()).map_err(invalid)?;
    let sources = std::mem::take(&mut snapshot.native_sources)
        .into_iter()
        .map(|s| {
            Ok(Source {
                id: s.id,
                name: s.name,
                instrument_index: s.instrument_index,
                devices: s
                    .devices
                    .iter()
                    .map(SourceDevice::from_device)
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    let core = Project::from_snapshot(snapshot).map_err(invalid)?;
    let mut value = super::track_groups::encode(&core)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid native source envelope"))?;
    object.insert("version".into(), 45.into());
    object.insert("native_sources".into(), serde_json::to_value(sources)?);
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid native source envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(45) {
        return Err(StorageError::Invalid("invalid native source schema"));
    }
    let Inventory(sources) = serde_json::from_value(
        object
            .remove("native_sources")
            .ok_or(StorageError::Invalid("native source inventory missing"))?,
    )?;
    object.insert("version".into(), 44.into());
    let mut snapshot = super::track_groups::decode(&serde_json::to_vec(&value)?)?.snapshot();
    snapshot.native_sources = sources
        .into_iter()
        .map(|s| {
            Ok(NativeSourceSnapshot {
                id: s.id,
                name: s.name,
                instrument_index: s.instrument_index,
                devices: s
                    .devices
                    .into_iter()
                    .map(SourceDevice::into_device)
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<_, StorageError>>()?;
    Project::from_snapshot(snapshot).map_err(invalid)
}

#[cfg(test)]
mod tests;
