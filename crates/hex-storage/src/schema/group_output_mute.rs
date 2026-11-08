//! Schema74 adds exact Arrangement output mute ranges keyed by group identity.
//! It extends the complete track-tag and event layout used by schemas72/73.
//! Schema75 uses the same layout with a larger VST3 state bound. It can
//! carry an empty mute inventory. No project is changed during open.
use crate::{GROUP_OUTPUT_MUTE_SCHEMA_VERSION, Project, StorageError};
use hex_project::{GroupOutputMuteRange, MAX_GROUP_OUTPUT_MUTE_RANGES, MAX_TRACKS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Interval {
    start: u64,
    end: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Assignment {
    group: u64,
    ranges: Vec<Interval>,
}

pub(crate) fn contains(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .any(|track| !track.group_output_mutes().is_empty())
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    // Lower codecs temporarily flatten track hierarchy. Keep the group-only
    // fields in this envelope while the lower core composes its document.
    let mut snapshot = project.snapshot();
    let rows: Vec<_> = snapshot
        .tracks
        .iter_mut()
        .filter(|track| !track.group_output_mutes.is_empty())
        .map(|track| Assignment {
            group: track.id,
            ranges: std::mem::take(&mut track.group_output_mutes)
                .into_iter()
                .map(|range| Interval {
                    start: range.start(),
                    end: range.end(),
                })
                .collect(),
        })
        .collect();
    let core = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value = super::track_tags::encode(&core)?;
    value["version"] = GROUP_OUTPUT_MUTE_SCHEMA_VERSION.into();
    value["group_output_mutes"] = serde_json::to_value(rows)?;
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?, version)
}

pub(crate) fn decode_value(mut value: Value, version: u32) -> Result<Project, StorageError> {
    let rows = value
        .as_object_mut()
        .and_then(|map| map.remove("group_output_mutes"))
        .ok_or(StorageError::Invalid(
            "group output mute inventory is missing",
        ))?;
    if rows.as_array().is_none_or(|rows| {
        (rows.is_empty() && version == GROUP_OUTPUT_MUTE_SCHEMA_VERSION)
            || rows.len() > MAX_TRACKS
            || rows.iter().any(|row| {
                row.get("ranges")
                    .and_then(Value::as_array)
                    .is_none_or(|ranges| {
                        ranges.is_empty() || ranges.len() > MAX_GROUP_OUTPUT_MUTE_RANGES
                    })
            })
    }) {
        return Err(StorageError::Invalid("invalid group output mute inventory"));
    }
    let mut by_group = BTreeMap::new();
    for Assignment { group, ranges } in serde_json::from_value::<Vec<Assignment>>(rows)? {
        let ranges = ranges
            .into_iter()
            .map(|range| GroupOutputMuteRange::new(range.start, range.end))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| StorageError::Invalid(error.0))?;
        if by_group.insert(group, ranges).is_some() {
            return Err(StorageError::Invalid("duplicate group output mute owner"));
        }
    }
    let project = super::track_tags::decode_value(value, version)?;
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        if let Some(ranges) = by_group.remove(&track.id) {
            track.group_output_mutes = ranges;
        }
    }
    if !by_group.is_empty() {
        return Err(StorageError::Invalid("group output mute owner is missing"));
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}
