//! Schema80 retains serial membership over the exact compatible flat envelope.
//! Older projects keep their existing bytes/layout and migrate with root nodes.
//! A complete ordered membership row for every canonical node introduces no
//! second state/media codec. This control-side wrapper materializes and
//! reserializes one compatible flat envelope; it performs no file or PCM work.
use crate::{Project, StorageError};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Membership {
    device: u64,
    #[serde(deserialize_with = "required_parent")]
    parent: Option<u64>,
}

fn required_parent<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Option<u64>, D::Error> {
    Option::deserialize(decoder)
}

#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "rows")] Vec<Membership>);

fn rows<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<Membership>, D::Error> {
    struct Inventory;
    impl<'de> serde::de::Visitor<'de> for Inventory {
        type Value = Vec<Membership>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded serial device membership")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == hex_project::MAX_DEVICES {
                    return Err(serde::de::Error::custom("serial membership limit reached"));
                }
                rows.try_reserve(1).map_err(serde::de::Error::custom)?;
                rows.push(row);
            }
            Ok(rows)
        }
    }
    decoder.deserialize_seq(Inventory)
}

pub(crate) fn contains(project: &Project) -> bool {
    project.devices().any(|device| device.parent().is_some())
}

pub(crate) fn refuse_legacy(project: &Project) -> Result<(), StorageError> {
    if contains(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save serial device units",
        ));
    }
    Ok(())
}

pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let mut snapshot = project.snapshot();
    let rows = super::device_automation::devices(&mut snapshot)
        .map(|device| Membership {
            device: device.id,
            parent: device.parent.take(),
        })
        .collect::<Vec<_>>();
    let flat = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value: serde_json::Value = serde_json::from_slice(&crate::encode(&flat)?)?;
    let base = value["version"].clone();
    value["version"] = crate::SERIAL_DEVICE_SCHEMA_VERSION.into();
    value["serial_base_version"] = base;
    value["serial_devices"] = serde_json::to_value(rows)?;
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid serial device document"))?;
    let base = object
        .remove("serial_base_version")
        .and_then(|version| version.as_u64())
        .filter(|version| {
            (crate::CURRENT_SCHEMA_VERSION as u64..crate::SERIAL_DEVICE_SCHEMA_VERSION as u64)
                .contains(version)
        })
        .ok_or(StorageError::Invalid("invalid serial base schema"))?;
    let Rows(rows) = serde_json::from_value(
        object
            .remove("serial_devices")
            .ok_or(StorageError::Invalid("serial membership inventory missing"))?,
    )?;
    if !rows.iter().any(|row| row.parent.is_some()) {
        return Err(StorageError::Invalid(
            "serial membership inventory must be nonempty",
        ));
    }
    object.insert("version".into(), base.into());
    let flat = crate::decode(&serde_json::to_vec(&value)?)?;
    let mut snapshot = flat.snapshot();
    let mut rows = rows.into_iter();
    for device in super::device_automation::devices(&mut snapshot) {
        let row = rows
            .next()
            .ok_or(StorageError::Invalid("serial member entry is missing"))?;
        if row.device != device.id {
            return Err(StorageError::Invalid(
                "serial membership must match canonical device order",
            ));
        }
        device.parent = row.parent;
    }
    if rows.next().is_some() {
        return Err(StorageError::Invalid(
            "serial membership references a missing device",
        ));
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests;
