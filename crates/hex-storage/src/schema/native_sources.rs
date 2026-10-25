//! Schema45 adds mandatory retained native source definitions to strict schema44.
//! Schema47 permits retained processing projections on outer musical views and
//! skips inaudible partial first cycles while retaining their authored cursor.
//! Schema46 adds an ordered source-binding row for every track and removes inline
//! instrument copies from its core. Older schemas still migrate through their
//! frozen codecs; decoding is in-memory only. Private stripped cores compose those
//! codecs and are never public lossy downgrades.
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
struct Binding {
    track: u64,
    #[serde(deserialize_with = "required_source")]
    source: Option<u64>,
    #[serde(deserialize_with = "required_position")]
    instrument_position: Option<usize>,
}
#[derive(Deserialize)]
struct Bindings(#[serde(deserialize_with = "bindings")] Vec<Binding>);
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
struct Inventory45(#[serde(deserialize_with = "sources45")] Vec<Source>);
#[derive(Deserialize)]
struct Inventory46(#[serde(deserialize_with = "sources46")] Vec<Source>);

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
fn sources45<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Source>, D::Error> {
    // Schema45 was written when the source limit equaled MAX_TRACKS (256).
    // Keep its wire bound frozen even though schema46 can hold promoted sources.
    bounded(d, 256)
}
fn sources46<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Source>, D::Error> {
    bounded(d, hex_project::MAX_NATIVE_SOURCES)
}
fn bindings<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Binding>, D::Error> {
    bounded(d, hex_project::MAX_TRACKS)
}
fn required_source<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Option::deserialize(d)
}
fn required_position<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<usize>, D::Error> {
    Option::deserialize(d)
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
    let mut value = encode45(project)?;
    let snapshot = project.snapshot();
    let mut bindings = Vec::with_capacity(snapshot.tracks.len());
    let bound_devices = snapshot
        .tracks
        .iter()
        .filter_map(|track| track.instrument.as_ref().map(|device| device.id))
        .collect::<std::collections::BTreeSet<_>>();
    for (track, live_track) in snapshot.tracks.iter().zip(project.tracks()) {
        bindings.push(Binding {
            track: track.id,
            source: live_track.source_id().map(|source| source.value()),
            instrument_position: track.instrument_position,
        });
    }
    let object = value.as_object_mut().ok_or(StorageError::Invalid(
        "invalid native source binding envelope",
    ))?;
    let track_rows = object
        .get_mut("tracks")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or(StorageError::Invalid("track records missing"))?;
    if track_rows.len() != snapshot.tracks.len() {
        return Err(StorageError::Invalid(
            "track records differ from binding inventory",
        ));
    }
    for (row, track) in track_rows.iter_mut().zip(&snapshot.tracks) {
        if row.get("id").and_then(serde_json::Value::as_u64) != Some(track.id) {
            return Err(StorageError::Invalid(
                "track records differ from binding order",
            ));
        }
        row["instrument"] = serde_json::Value::Null;
        row["instrument_position"] = serde_json::Value::Null;
    }
    let asset_rows = object
        .get_mut("device_assets")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or(StorageError::Invalid("device asset manifest missing"))?;
    asset_rows.retain(|row| {
        row.get("device")
            .and_then(serde_json::Value::as_u64)
            .is_none_or(|device| !bound_devices.contains(&device))
    });
    object.insert("version".into(), 47.into());
    object.insert(
        "track_source_bindings".into(),
        serde_json::to_value(bindings)?,
    );
    Ok(value)
}

fn encode45(project: &Project) -> Result<serde_json::Value, StorageError> {
    let snapshot = project.snapshot();
    Project::from_snapshot(snapshot.clone()).map_err(invalid)?;
    let sources = snapshot
        .native_sources
        .iter()
        .map(|s| {
            Ok(Source {
                id: s.id,
                name: s.name.clone(),
                instrument_index: s.instrument_index,
                devices: s
                    .devices
                    .iter()
                    .map(SourceDevice::from_device)
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    let mut core_snapshot = snapshot;
    let bound_sources = project
        .tracks()
        .iter()
        .filter_map(|track| track.source_id().map(|source| source.value()))
        .collect::<std::collections::BTreeSet<_>>();
    core_snapshot
        .native_sources
        .retain(|source| bound_sources.contains(&source.id));
    let core = Project::from_snapshot(core_snapshot).map_err(invalid)?;
    let mut value = super::track_groups::encode(&core)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid native source envelope"))?;
    object.insert("version".into(), 45.into());
    object.insert("native_sources".into(), serde_json::to_value(sources)?);
    Ok(value)
}
/// Decode the schema46 native-source envelope. The outer storage dispatcher
/// applies frozen historical-model restrictions after canonical decoding.
pub(crate) fn decode46(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_version(bytes, 46)
}

/// Schema47 changes only the validity contract; all encoded fields retain their
/// established strict layout and limits.
pub(crate) fn decode47(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_version(bytes, 47)
}

fn decode_version(bytes: &[u8], version: u64) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    let next_id = value
        .get("next_id")
        .and_then(serde_json::Value::as_u64)
        .ok_or(StorageError::Invalid("project next identity missing"))?;
    let object = value.as_object_mut().ok_or(StorageError::Invalid(
        "invalid native source binding envelope",
    ))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(version) {
        return Err(StorageError::Invalid(
            "invalid native source binding schema",
        ));
    }
    let tracks = object
        .get("tracks")
        .and_then(serde_json::Value::as_array)
        .ok_or(StorageError::Invalid("track records missing"))?;
    if tracks.iter().any(|track| {
        track.get("instrument") != Some(&serde_json::Value::Null)
            || track.get("instrument_position") != Some(&serde_json::Value::Null)
    }) {
        return Err(StorageError::Invalid(
            "current track records cannot contain inline instrument projections",
        ));
    }
    let Bindings(bindings) =
        serde_json::from_value(object.remove("track_source_bindings").ok_or(
            StorageError::Invalid("track source binding inventory missing"),
        )?)?;
    let source_value = object
        .get("native_sources")
        .cloned()
        .ok_or(StorageError::Invalid("native source inventory missing"))?;
    let Inventory46(sources) = serde_json::from_value(source_value)?;
    let midi_routes = super::playable_routing::decode_midi_routes(
        object
            .remove("midi_routes")
            .ok_or(StorageError::Invalid("midi route inventory missing"))?,
    )?;
    let track_rows = object
        .get("tracks")
        .and_then(serde_json::Value::as_array)
        .ok_or(StorageError::Invalid("track records missing"))?;
    if bindings.len() != track_rows.len() {
        return Err(StorageError::Invalid(
            "track source binding inventory is incomplete",
        ));
    }
    for (track, binding) in track_rows.iter().zip(&bindings) {
        if track.get("id").and_then(serde_json::Value::as_u64) != Some(binding.track) {
            return Err(StorageError::Invalid(
                "track source bindings differ from track order",
            ));
        }
        match (binding.source, binding.instrument_position) {
            (None, None) => {}
            (Some(source_id), Some(_)) => {
                if !sources.iter().any(|source| source.id == source_id) {
                    return Err(StorageError::Invalid(
                        "track binding references a missing source",
                    ));
                }
            }
            _ => return Err(StorageError::Invalid("invalid track source binding")),
        }
    }
    object.remove("native_sources");
    // Legacy readers validate MIDI destinations against inline instruments. Keep
    // the strict rows for the final hydrated model, but skip that lower-layer
    // check while decoding the core: bindings supply destination instruments.
    object.insert("midi_routes".into(), serde_json::Value::Array(Vec::new()));
    object.insert("version".into(), 44.into());
    let legacy = super::track_groups::decode(&serde_json::to_vec(&value)?)?;
    let mut snapshot = legacy.snapshot();
    if bindings.len() != snapshot.tracks.len() {
        return Err(StorageError::Invalid(
            "track source binding inventory is incomplete",
        ));
    }
    let native_sources = sources
        .into_iter()
        .map(|source| {
            Ok(NativeSourceSnapshot {
                id: source.id,
                name: source.name,
                instrument_index: source.instrument_index,
                devices: source
                    .devices
                    .into_iter()
                    .map(SourceDevice::into_device)
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    for (track, binding) in snapshot.tracks.iter_mut().zip(&bindings) {
        if binding.track != track.id {
            return Err(StorageError::Invalid(
                "track source bindings differ from track order",
            ));
        }
        match (binding.source, binding.instrument_position) {
            (None, None) => {}
            (Some(source_id), Some(position)) => {
                let source = native_sources
                    .iter()
                    .find(|source| source.id == source_id)
                    .ok_or(StorageError::Invalid(
                        "track binding references a missing source",
                    ))?;
                let expected = source
                    .devices
                    .get(source.instrument_index)
                    .ok_or(StorageError::Invalid("source instrument is missing"))?;
                track.instrument = Some(expected.clone());
                track.instrument_position = Some(position);
            }
            _ => return Err(StorageError::Invalid("invalid track source binding")),
        }
    }
    snapshot.native_sources = native_sources;
    snapshot.midi_routes = midi_routes;
    snapshot.next_id = next_id;
    Project::from_snapshot(snapshot).map_err(invalid)
}

pub(crate) fn decode45(bytes: &[u8]) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid native source envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(45) {
        return Err(StorageError::Invalid("invalid native source schema"));
    }
    let wire_sources = object
        .remove("native_sources")
        .ok_or(StorageError::Invalid("native source inventory missing"))?;
    let Inventory45(sources) = serde_json::from_value(wire_sources)?;
    object.insert("version".into(), 44.into());
    let mut snapshot = super::track_groups::decode(&serde_json::to_vec(&value)?)?.snapshot();
    let mut merged = sources
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
        .collect::<Result<Vec<_>, StorageError>>()?;
    merged.extend(snapshot.native_sources);
    snapshot.native_sources = merged;
    Project::from_legacy_snapshot(snapshot).map_err(invalid)
}

#[cfg(test)]
mod tests;
