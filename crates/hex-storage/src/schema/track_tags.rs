//! Schema70 adds canonical tag assignments above schema69's complete project.
//! Schemas1–69 migrate with no tags; their existing layout remains unchanged.
//! Only tagged projects need70. Reading never edits original bytes or media;
//! existing publication and newer-format overwrite consent keep their owners.
use crate::{Project, StorageError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Assignment {
    track: u64,
    tags: Vec<String>,
}
pub(crate) fn contains(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .any(|track| !track.tags().is_empty())
}
pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let assignments: Vec<_> = snapshot
        .tracks
        .iter_mut()
        .filter_map(|track| {
            (!track.tags.is_empty()).then(|| Assignment {
                track: track.id,
                tags: std::mem::take(&mut track.tags),
            })
        })
        .collect();
    let neutral =
        Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value = super::sidechain::encode(&neutral)?;
    value["track_tags"] = serde_json::to_value(assignments)?;
    value["version"] = crate::TRACK_TAGS_SCHEMA_VERSION.into();
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    if value.get("version").and_then(Value::as_u64) != Some(crate::TRACK_TAGS_SCHEMA_VERSION.into())
    {
        return Err(StorageError::Invalid("invalid track tag schema"));
    }
    let assignments = value
        .as_object_mut()
        .and_then(|object| object.remove("track_tags"))
        .ok_or(StorageError::Invalid("track tag inventory missing"))?;
    let rows = assignments
        .as_array()
        .ok_or(StorageError::Invalid("invalid track tag inventory"))?;
    if rows.len() > hex_project::MAX_TRACKS
        || rows.iter().any(|row| {
            row.get("tags")
                .and_then(Value::as_array)
                .is_none_or(|tags| {
                    tags.is_empty()
                        || tags.len() > hex_project::MAX_TRACK_TAGS
                        || tags.iter().any(|tag| {
                            tag.as_str()
                                .is_none_or(|s| s.len() > hex_project::MAX_TRACK_TAG_BYTES)
                        })
                })
        })
    {
        return Err(StorageError::Invalid("invalid track tag inventory"));
    }
    let assignments: Vec<Assignment> = serde_json::from_value(assignments)?;
    value["version"] = crate::SIDECHAIN_SCHEMA_VERSION.into();
    let neutral = super::sidechain::decode(&serde_json::to_vec(&value)?)?;
    let mut snapshot = neutral.snapshot();
    let mut seen = BTreeSet::new();
    for assignment in assignments {
        if !seen.insert(assignment.track) {
            return Err(StorageError::Invalid("duplicate track tag owner"));
        }
        let track = snapshot
            .tracks
            .iter_mut()
            .find(|track| track.id == assignment.track)
            .ok_or(StorageError::Invalid("track tag owner is missing"))?;
        track.tags = assignment.tags;
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, TrackTag};
    #[test]
    fn prior_schema_migrates_empty_and_tagged_project_round_trips_complete_state() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "drums".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let original = editor.project().clone();
        let historical = super::super::sidechain::encode(&original).unwrap();
        assert_eq!(
            crate::decode(&serde_json::to_vec(&historical).unwrap()).unwrap(),
            original
        );
        editor
            .execute(Command::SetTrackTags {
                track,
                tags: vec![
                    TrackTag::new("Drums").unwrap(),
                    TrackTag::new("lowend").unwrap(),
                ],
            })
            .unwrap();
        let tagged = editor.project();
        assert_eq!(crate::schema_version_for(tagged), 70);
        let bytes = crate::encode(tagged).unwrap();
        assert_eq!(crate::decode(&bytes).unwrap(), *tagged);
        assert!(crate::encode_schema22(tagged).is_err());
        assert!(crate::encode_schema20(tagged).is_err());
        assert_eq!(historical["version"], 69);
        assert!(historical.get("track_tags").is_none());
        let mut old = historical;
        old["track_tags"] = serde_json::json!([]);
        assert!(crate::decode(&serde_json::to_vec(&old).unwrap()).is_err());
    }
    #[test]
    fn malformed_or_noncanonical_assignments_refuse_without_rewriting_input() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "tagged".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(Command::SetTrackTags {
                track,
                tags: vec![TrackTag::new("drums").unwrap()],
            })
            .unwrap();
        let valid = encode(editor.project()).unwrap();
        for tags in [
            serde_json::json!([]),
            serde_json::json!(["Drums"]),
            serde_json::json!(["drums", "drums"]),
            serde_json::json!(["z", "a"]),
            serde_json::json!(["a,b"]),
            serde_json::json!(["x".repeat(65)]),
        ] {
            let mut value = valid.clone();
            value["track_tags"][0]["tags"] = tags;
            let bytes = serde_json::to_vec(&value).unwrap();
            let original = bytes.clone();
            assert!(crate::decode(&bytes).is_err());
            assert_eq!(bytes, original);
        }
        let mut duplicate = valid.clone();
        let row = duplicate["track_tags"][0].clone();
        duplicate["track_tags"].as_array_mut().unwrap().push(row);
        assert!(crate::decode(&serde_json::to_vec(&duplicate).unwrap()).is_err());
        let mut unknown = valid;
        unknown["track_tags"][0]["track"] = 99999.into();
        assert!(crate::decode(&serde_json::to_vec(&unknown).unwrap()).is_err());
    }
}
