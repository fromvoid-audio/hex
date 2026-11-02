//! Schema63 stores which track names are automatic (hex-generated default or
//! device-derived) beside the schema62 layout. Earlier supported schemas never
//! recorded name origin, so every migrated name is user-set: hex cannot prove a
//! historical name was generated, and a user may have typed `track 3` themself.
use crate::{Project, StorageError};
use serde_json::Value;

pub(crate) fn require_user_names(project: &Project) -> Result<(), StorageError> {
    if project
        .tracks()
        .iter()
        .any(hex_project::Track::name_is_automatic)
    {
        return Err(StorageError::Invalid(
            "older schema cannot save automatic track names",
        ));
    }
    Ok(())
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let automatic = std::mem::take(&mut snapshot.automatic_track_names);
    let neutral = Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
    let mut value = super::midi_clip_gain::encode(&neutral)?;
    value["automatic_track_names"] = serde_json::to_value(automatic)?;
    value["version"] = 63.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: Value) -> Result<Project, StorageError> {
    if value.get("version").and_then(Value::as_u64) != Some(63) {
        return Err(StorageError::Invalid("invalid track name origin schema"));
    }
    let raw = value
        .as_object_mut()
        .and_then(|v| v.remove("automatic_track_names"))
        .ok_or(StorageError::Invalid(
            "automatic track name inventory missing",
        ))?;
    value["version"] = 62.into();
    let project = super::midi_clip_gain::decode_value(value)?;
    let rows = raw.as_array().ok_or(StorageError::Invalid(
        "invalid automatic track name inventory",
    ))?;
    if rows.len() > project.tracks().len() {
        return Err(StorageError::Invalid(
            "automatic track name inventory exceeds track count",
        ));
    }
    let mut snapshot = project.snapshot();
    // The model refuses unordered, duplicate and ownerless IDs.
    snapshot.automatic_track_names = serde_json::from_value(raw)?;
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};

    #[test]
    fn historical_schema62_names_migrate_as_user_set_and_survive_instrument_loads() {
        let bytes = include_bytes!("../../tests/fixtures/track-name-origin/schema62.json");
        let project = crate::decode(bytes).unwrap();
        let names: Vec<_> = project.tracks().iter().map(|t| t.name()).collect();
        assert_eq!(names, ["track 1", "audio 2", "lead", "new track"]);
        assert!(!project.tracks().iter().any(|t| t.name_is_automatic()));
        assert!(project.tracks()[3].instrument().is_some());
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        let track = project.tracks()[0].id();
        editor
            .execute(Command::LoadInstrument {
                track,
                replacing: None,
                state: hex_project::default_instrument_state().unwrap(),
                device_name: "sine".into(),
            })
            .unwrap();
        assert_eq!(editor.project().track(track).unwrap().name(), "track 1");
        let saved = crate::encode(&project).unwrap();
        let value: Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(value["version"], crate::CURRENT_SCHEMA_VERSION);
        assert_eq!(value["automatic_track_names"], serde_json::json!([]));
        assert_eq!(
            crate::decode(&saved).unwrap().snapshot(),
            project.snapshot()
        );
    }

    #[test]
    fn automatic_names_roundtrip_refuse_older_writers_and_strict_inventory() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(Command::AddTrack {
                name: "user".into(),
            })
            .unwrap();
        for _ in 0..2 {
            let Created::Track(_) = editor
                .execute(Command::InsertDefaultTrack { before_track: None })
                .unwrap()
            else {
                panic!("default track")
            };
        }
        let project = editor.project();
        assert!(crate::encode_schema20(project).is_err());
        let value = encode(project).unwrap();
        let ids = value["automatic_track_names"].clone();
        assert_eq!(ids.as_array().unwrap().len(), 2);
        let reopened = crate::decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(reopened.snapshot(), project.snapshot());
        assert!(reopened.tracks()[1].name_is_automatic());
        let user_track = project.tracks()[0].id().value();
        let malformed = [
            serde_json::json!([ids[1], ids[0]]),
            serde_json::json!([ids[0], ids[0]]),
            serde_json::json!([ids[0], ids[1], user_track]),
            serde_json::json!([u64::MAX]),
            serde_json::json!({ "track": ids[0] }),
            serde_json::json!(["1"]),
        ];
        for inventory in malformed {
            let mut bad = value.clone();
            bad["automatic_track_names"] = inventory;
            assert!(crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        let mut missing = value;
        missing
            .as_object_mut()
            .unwrap()
            .remove("automatic_track_names");
        assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    }
}
