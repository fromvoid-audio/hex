//! Required bounded device asset manifest in the coherent current envelope.
use super::*;
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, MAX_DEVICE_ASSET_REFERENCES,
};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceAssets {
    device: u64,
    #[serde(deserialize_with = "bounded_references")]
    references: Vec<Reference>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    slot: u32,
    asset: u64,
}

fn bounded_references<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Vec<Reference>, D::Error> {
    struct References;
    impl<'de> serde::de::Visitor<'de> for References {
        type Value = Vec<Reference>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded device asset references")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut references = Vec::new();
            while let Some(reference) = seq.next_element()? {
                if references.len() == MAX_DEVICE_ASSET_REFERENCES {
                    return Err(serde::de::Error::custom(
                        "device asset reference limit reached",
                    ));
                }
                references.push(reference);
            }
            Ok(references)
        }
    }
    decoder.deserialize_seq(References)
}
#[derive(Serialize, Deserialize)]
pub(super) struct Manifest(#[serde(deserialize_with = "bounded_devices")] Vec<DeviceAssets>);
fn bounded_devices<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Vec<DeviceAssets>, D::Error> {
    struct Devices;
    impl<'de> serde::de::Visitor<'de> for Devices {
        type Value = Vec<DeviceAssets>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded device asset manifest")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut devices = Vec::new();
            while let Some(device) = seq.next_element()? {
                if devices.len() == hex_project::MAX_DEVICES {
                    return Err(serde::de::Error::custom(
                        "device asset manifest limit reached",
                    ));
                }
                devices.push(device);
            }
            Ok(devices)
        }
    }
    decoder.deserialize_seq(Devices)
}

fn devices(project: &Project) -> impl Iterator<Item = &hex_project::Device> {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.devices().iter().chain(track.instrument()))
        .chain(
            project
                .return_tracks()
                .iter()
                .flat_map(|track| track.devices()),
        )
}
pub(crate) fn has_references(project: &Project) -> bool {
    devices(project).any(|device| !device.state().asset_references().is_empty())
}
pub(crate) fn refuse_legacy(project: &Project) -> Result<(), StorageError> {
    if has_references(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save device asset references",
        ));
    }
    Ok(())
}
impl Manifest {
    pub(super) fn from_project(project: &Project) -> Result<Self, StorageError> {
        project
            .validate_device_asset_references()
            .map_err(|e| StorageError::Invalid(e.0))?;
        let device_assets = devices(project)
            .map(|device| DeviceAssets {
                device: device.id().value(),
                references: device
                    .state()
                    .asset_references()
                    .iter()
                    .map(|reference| Reference {
                        slot: reference.slot.value(),
                        asset: reference.asset.value(),
                    })
                    .collect(),
            })
            .collect();
        Ok(Self(device_assets))
    }
    pub(super) fn apply(self, snapshot: &mut Snapshot) -> Result<(), StorageError> {
        let mut bindings = BTreeMap::new();
        for row in self.0 {
            let references = row
                .references
                .into_iter()
                .map(|reference| {
                    Ok(DeviceAssetReference {
                        slot: DeviceAssetSlot::new(reference.slot)
                            .map_err(|_| StorageError::Invalid("invalid device asset slot"))?,
                        asset: DeviceAssetKey::new(reference.asset)
                            .map_err(|_| StorageError::Invalid("invalid device asset identity"))?,
                    })
                })
                .collect::<Result<Vec<_>, StorageError>>()?;
            if bindings.insert(row.device, references).is_some() {
                return Err(StorageError::Invalid(
                    "duplicate device asset manifest entry",
                ));
            }
        }
        for track in &mut snapshot.tracks {
            for device in track.devices.iter_mut().chain(track.instrument.iter_mut()) {
                apply(device, &mut bindings)?;
            }
        }
        for track in &mut snapshot.return_tracks {
            for device in &mut track.devices {
                apply(device, &mut bindings)?;
            }
        }
        if !bindings.is_empty() {
            return Err(StorageError::Invalid(
                "device asset manifest references a missing device",
            ));
        }
        Ok(())
    }
}
fn apply(
    device: &mut DeviceSnapshot,
    bindings: &mut BTreeMap<u64, Vec<DeviceAssetReference>>,
) -> Result<(), StorageError> {
    let references = bindings.remove(&device.id).ok_or(StorageError::Invalid(
        "device asset manifest entry is missing",
    ))?;
    device.state = device
        .state
        .clone()
        .with_asset_references(references)
        .map_err(|_| StorageError::Invalid("invalid device asset references"))?;
    Ok(())
}
