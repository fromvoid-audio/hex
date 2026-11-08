//! Schema69 stores post-mixer track keys for native Compressor instances.
//! Schemas1–68 have no authored sidechains. Projects without routes still
//! use their original codec and can be read by the same older builds.
use crate::{Project, StorageError};
use hex_project::{MAX_SIDECHAIN_ROUTES, SidechainRouteSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Route {
    device: u64,
    source: u64,
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let routes: Vec<_> = std::mem::take(&mut snapshot.sidechains)
        .into_iter()
        .map(|route| Route {
            device: route.device,
            source: route.source,
        })
        .collect();
    let neutral =
        Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value = super::retained_receiver::encode(&neutral)?;
    value["sidechains"] = serde_json::to_value(routes)?;
    value["version"] = crate::SIDECHAIN_SCHEMA_VERSION.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    if value.get("version").and_then(Value::as_u64) != Some(crate::SIDECHAIN_SCHEMA_VERSION.into())
    {
        return Err(StorageError::Invalid("invalid compressor sidechain schema"));
    }
    let routes = value
        .as_object_mut()
        .and_then(|object| object.remove("sidechains"))
        .ok_or(StorageError::Invalid("sidechain inventory missing"))?;
    if routes
        .as_array()
        .is_none_or(|rows| rows.len() > MAX_SIDECHAIN_ROUTES)
    {
        return Err(StorageError::Invalid("invalid sidechain inventory"));
    }
    let routes: Vec<Route> = serde_json::from_value(routes)?;
    value["version"] = crate::RETAINED_RECEIVER_SCHEMA_VERSION.into();
    let neutral = super::retained_receiver::decode(&serde_json::to_vec(&value)?)?;
    let mut snapshot = neutral.snapshot();
    snapshot.sidechains = routes
        .into_iter()
        .map(|route| SidechainRouteSnapshot {
            device: route.device,
            source: route.source,
        })
        .collect();
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema68_still_loads_without_sidechains_and_refuses_new_fields() {
        let original = Project::new(48_000).unwrap();
        let mut old = super::super::retained_receiver::encode(&original).unwrap();
        assert_eq!(old["version"], crate::RETAINED_RECEIVER_SCHEMA_VERSION);
        let bytes = serde_json::to_vec(&old).unwrap();
        assert_eq!(crate::decode(&bytes).unwrap(), original);
        old["sidechains"] = serde_json::json!([]);
        assert!(crate::decode(&serde_json::to_vec(&old).unwrap()).is_err());
    }
}
