//! Schema72 adds canonical track tags to the complete event layout. Projects
//! saved without tags keep their existing codec. The earlier tag draft at
//! 3a50aa81 used schema70 over schema69, with a disjoint tag inventory and no
//! musical event inventory. It remains readable and upgrades only on save.
//! Schema73 reuses the complete layout and may have no tagged tracks.
use crate::{Project, StorageError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

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
    let assignments: Vec<_> = project
        .tracks()
        .iter()
        .filter(|track| !track.tags().is_empty())
        .map(|track| Assignment {
            track: track.id().value(),
            tags: track
                .tags()
                .iter()
                .map(|tag| tag.as_str().to_owned())
                .collect(),
        })
        .collect();
    let mut value = super::musical_event_source::encode(project)?;
    value["track_tags"] = serde_json::to_value(assignments)?;
    value["version"] = crate::TRACK_TAGS_SCHEMA_VERSION.into();
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_with_version(bytes, crate::TRACK_TAGS_SCHEMA_VERSION)
}

pub(crate) fn decode_with_version(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?, version)
}

pub(crate) fn decode_value(mut value: Value, version: u32) -> Result<Project, StorageError> {
    if value.get("version").and_then(Value::as_u64) != Some(version.into()) {
        return Err(StorageError::Invalid("invalid track tag schema"));
    }
    let assignments = take_assignments(&mut value)?;
    if version == crate::TRACK_TAGS_SCHEMA_VERSION && assignments.is_empty() {
        return Err(StorageError::Invalid("track tag inventory is empty"));
    }
    let project = super::musical_event_source::decode_value(value, version)?;
    restore_assignments(project, assignments)
}

pub(crate) fn decode_schema70(bytes: &[u8]) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    if value.get("track_tags").is_none() {
        return super::musical_event_source::decode_value(
            value,
            crate::MUSICAL_EVENT_SOURCE_SCHEMA_VERSION,
        );
    }
    let assignments = take_assignments(&mut value)?;
    value["version"] = crate::SIDECHAIN_SCHEMA_VERSION.into();
    let project = super::sidechain::decode(&serde_json::to_vec(&value)?)?;
    restore_assignments(project, assignments)
}

fn take_assignments(value: &mut Value) -> Result<BTreeMap<u64, Vec<String>>, StorageError> {
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
    let mut by_track = BTreeMap::new();
    for Assignment { track, tags } in serde_json::from_value::<Vec<Assignment>>(assignments)? {
        if by_track.insert(track, tags).is_some() {
            return Err(StorageError::Invalid("duplicate track tag owner"));
        }
    }
    Ok(by_track)
}

fn restore_assignments(
    project: Project,
    mut assignments: BTreeMap<u64, Vec<String>>,
) -> Result<Project, StorageError> {
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        if let Some(tags) = assignments.remove(&track.id) {
            track.tags = tags;
        }
    }
    if !assignments.is_empty() {
        return Err(StorageError::Invalid("track tag owner is missing"));
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
        assert_eq!(
            crate::schema_version_for(tagged),
            crate::TRACK_TAGS_SCHEMA_VERSION
        );
        let bytes = crate::encode(tagged).unwrap();
        assert_eq!(crate::decode(&bytes).unwrap(), *tagged);
        assert!(crate::encode_schema22(tagged).is_err());
        assert!(crate::encode_schema20(tagged).is_err());
        assert_eq!(historical["version"], 69);
        assert!(historical.get("track_tags").is_none());
        // The draft layout was schema69 plus only the track inventory.
        let mut draft = historical.clone();
        draft["version"] = 70.into();
        draft["track_tags"] = encode(tagged).unwrap()["track_tags"].clone();
        assert_eq!(
            crate::decode(&serde_json::to_vec(&draft).unwrap()).unwrap(),
            *tagged
        );
        draft["musical_event_sources"] = serde_json::json!([]);
        assert!(crate::decode(&serde_json::to_vec(&draft).unwrap()).is_err());

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
