//! Required bounded device asset manifest in the coherent current envelope.
use super::*;
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, MAX_DEVICE_ASSET_REFERENCES,
    NativeDeviceState,
};
use std::collections::BTreeMap;

pub(crate) fn authored_states(project: &Project) -> impl Iterator<Item = &NativeDeviceState> {
    project
        .devices()
        .flat_map(hex_project::Device::authored_states)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceAssets {
    device: u64,
    #[serde(deserialize_with = "bounded_references")]
    references: Vec<Reference>,
    #[serde(deserialize_with = "required_comparison")]
    comparison: Option<ComparisonDto>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reference {
    slot: u32,
    asset: u64,
}

impl Reference {
    pub(crate) fn from_model(reference: &DeviceAssetReference) -> Self {
        Self {
            slot: reference.slot.value(),
            asset: reference.asset.value(),
        }
    }

    pub(crate) fn into_model(self) -> Result<DeviceAssetReference, StorageError> {
        Ok(DeviceAssetReference {
            slot: DeviceAssetSlot::new(self.slot)
                .map_err(|_| StorageError::Invalid("invalid device asset slot"))?,
            asset: DeviceAssetKey::new(self.asset)
                .map_err(|_| StorageError::Invalid("invalid device asset identity"))?,
        })
    }
}

pub(crate) fn bounded_references<'de, D: serde::Deserializer<'de>>(
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
        .chain(project.main_devices())
}
pub(crate) fn has_references(project: &Project) -> bool {
    devices(project).any(|device| {
        device
            .authored_states()
            .any(|state| !state.asset_references().is_empty())
    })
}
pub(crate) fn refuse_legacy(project: &Project) -> Result<(), StorageError> {
    if has_references(project) || devices(project).any(|device| device.comparison().is_some()) {
        return Err(StorageError::Invalid(
            "older schema cannot save device comparison or asset references",
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
            .map(|device| {
                Ok(DeviceAssets {
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
                    comparison: device
                        .comparison()
                        .map(ComparisonDto::from_comparison)
                        .transpose()?,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
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
            if bindings
                .insert(row.device, (references, row.comparison))
                .is_some()
            {
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
        for device in &mut snapshot.main_devices {
            apply(device, &mut bindings)?;
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
    bindings: &mut BTreeMap<u64, (Vec<DeviceAssetReference>, Option<ComparisonDto>)>,
) -> Result<(), StorageError> {
    let (references, comparison) = bindings.remove(&device.id).ok_or(StorageError::Invalid(
        "device asset manifest entry is missing",
    ))?;
    device.state = device
        .state
        .clone()
        .with_asset_references(references)
        .map_err(|_| StorageError::Invalid("invalid device asset references"))?;
    device.comparison = comparison
        .map(|value| value.into_comparison(&device.state))
        .transpose()?;
    Ok(())
}

/// Shared opaque comparison codec. Host trims are supplied by the active device,
/// never restored from an inactive provider snapshot.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ComparisonDto {
    active: VariantDto,
    type_id: String,
    state_version: u32,
    #[serde(deserialize_with = "bounded_comparison_payload")]
    payload: Vec<u8>,
    #[serde(deserialize_with = "bounded_references")]
    references: Vec<Reference>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum VariantDto {
    A,
    B,
}

fn bounded_comparison_payload<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Vec<u8>, D::Error> {
    super::device_automation::bounded(decoder, hex_lattice_contract::MAX_VST3_DEVICE_STATE_BYTES)
}

pub(crate) fn required_comparison<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Option<ComparisonDto>, D::Error> {
    Option::deserialize(decoder)
}
impl ComparisonDto {
    pub(crate) fn from_comparison(
        value: &hex_project::DeviceComparison,
    ) -> Result<Self, StorageError> {
        let state = value.inactive();
        Ok(Self {
            active: match value.active() {
                hex_project::DeviceVariant::A => VariantDto::A,
                hex_project::DeviceVariant::B => VariantDto::B,
            },
            type_id: state
                .type_id()
                .as_str()
                .map_err(|_| StorageError::Invalid("invalid comparison provider"))?
                .into(),
            state_version: state.state_version(),
            payload: state.payload().to_vec(),
            references: state
                .asset_references()
                .iter()
                .map(Reference::from_model)
                .collect(),
        })
    }
    pub(crate) fn into_comparison(
        self,
        active: &hex_lattice_contract::NativeDeviceState,
    ) -> Result<hex_project::DeviceComparison, StorageError> {
        let type_id = hex_lattice_contract::DeviceTypeId::new(&self.type_id)
            .map_err(|_| StorageError::Invalid("invalid comparison provider"))?;
        if type_id != active.type_id() {
            return Err(StorageError::Invalid(
                "comparison provider does not match device",
            ));
        }
        let references = self
            .references
            .into_iter()
            .map(Reference::into_model)
            .collect::<Result<Vec<_>, StorageError>>()?;
        let inactive =
            hex_lattice_contract::NativeDeviceState::new(type_id, self.state_version, self.payload)
                .and_then(|state| state.with_asset_references(references))
                .map_err(|_| StorageError::Invalid("invalid comparison state"))?;
        hex_project::DeviceComparison::new(
            match self.active {
                VariantDto::A => hex_project::DeviceVariant::A,
                VariantDto::B => hex_project::DeviceVariant::B,
            },
            inactive,
        )
        .with_global_trims_from(active)
        .map_err(|e| StorageError::Invalid(e.0))
    }
}
