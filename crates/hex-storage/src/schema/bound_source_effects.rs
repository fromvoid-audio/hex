//! Schema83 adds an explicit compatibility boundary for retained source chains
//! bound to a track. The exact prior codec still owns all state and memberships.
use crate::{BOUND_SOURCE_EFFECT_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION, Project, StorageError};

pub(crate) fn contains(project: &Project) -> bool {
    project.native_sources().iter().any(|source| {
        !source.post_instrument_effects().is_empty()
            && project.source_contributors(source.id()).next().is_some()
    })
}

pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let mut value: serde_json::Value = serde_json::from_slice(&crate::encode_base(project)?)?;
    let base = value["version"].clone();
    value["version"] = BOUND_SOURCE_EFFECT_SCHEMA_VERSION.into();
    value["source_effect_base_version"] = base;
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid source effect project"))?;
    let base = object
        .remove("source_effect_base_version")
        .and_then(|value| value.as_u64())
        .filter(|version| {
            (u64::from(CURRENT_SCHEMA_VERSION)..u64::from(BOUND_SOURCE_EFFECT_SCHEMA_VERSION))
                .contains(version)
        })
        .ok_or(StorageError::Invalid("invalid source effect base schema"))?;
    object.insert("version".into(), base.into());
    let project = crate::decode_base(&serde_json::to_vec(&value)?)?;
    if !contains(&project) {
        return Err(StorageError::Invalid(
            "bound source effect schema has no bound unit",
        ));
    }
    Ok(project)
}
