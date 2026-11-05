//! Schema70 gives musical events an independently retained source span.
//! Schemas1–69 derive both clocks from the same root. An inventory beside
//! schema69 identifies the arrangement clip and nested source path. The
//! existing event codec still owns all event IDs, geometry and media. Schema71
//! reuses this layout, allowing an empty musical inventory for extended frame
//! roots. The top-level codec checks the schema-specific frame extent.
use crate::{MUSICAL_EVENT_SOURCE_SCHEMA_VERSION, Project, SIDECHAIN_SCHEMA_VERSION, StorageError};
use hex_project::{AudioClipEdit, BeatTime, MusicalEventSource, Snapshot, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Row {
    path: Vec<u64>,
    zero_in_destination: u64,
    length_raw: u64,
}

pub(crate) fn contains(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(|clip| {
            std::iter::once(clip)
                .chain(clip.retained_audio_roots().map(|root| &root.clip))
                .any(|clip| {
                    clip.audio_edit()
                        .is_some_and(|edit| edit.domain.musical_event_source.is_some())
                })
        })
}

fn walk_edit(
    project: &Project,
    edit: &mut AudioClipEdit,
    path: &mut Vec<u64>,
    visit: &mut impl FnMut(&[u64], &mut AudioClipEdit) -> bool,
) -> Result<bool, StorageError> {
    let mut changed = visit(path, edit);
    if let Some(events) = &edit.domain.events
        && !events.sources.is_empty()
    {
        let mut roots = None;
        for (index, root) in events.sources.iter().enumerate() {
            let mut snapshot = root.clip.retained_snapshot();
            let Some(child) = &mut snapshot.edit else {
                continue;
            };
            path.push(root.id);
            let child_changed = walk_edit(project, child, path, visit)?;
            path.pop();
            if child_changed {
                let restored = project
                    .restore_retained_audio_root(root.id, snapshot, Arc::clone(&root.tempo))
                    .map_err(|error| StorageError::Invalid(error.0))?;
                let replacement = roots.get_or_insert_with(|| events.sources.clone());
                replacement[index] = restored;
                changed = true;
            }
        }
        if let Some(roots) = roots
            && let Some(events) = Arc::make_mut(&mut edit.domain).events.as_mut()
        {
            Arc::make_mut(events).sources = roots;
        }
    }
    Ok(changed)
}

fn walk(
    project: &Project,
    snapshot: &mut Snapshot,
    mut visit: impl FnMut(&[u64], &mut AudioClipEdit) -> bool,
) -> Result<(), StorageError> {
    let mut path = Vec::with_capacity(hex_project::MAX_RETAINED_AUDIO_DEPTH + 1);
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            if let Some(edit) = &mut clip.edit {
                path.push(clip.id);
                walk_edit(project, edit, &mut path, &mut visit)?;
                path.pop();
            }
        }
    }
    super::audio_alias_projection::synchronize(snapshot)
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    walk(project, &mut snapshot, |path, edit| {
        if let Some(source) = edit.domain.musical_event_source {
            rows.push(Row {
                path: path.to_vec(),
                zero_in_destination: source.zero_in_destination,
                length_raw: source.length.raw(),
            });
            Arc::make_mut(&mut edit.domain).musical_event_source = None;
            true
        } else {
            false
        }
    })?;
    let neutral =
        Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value = super::sidechain::encode(&neutral)?;
    value["musical_event_sources"] = serde_json::to_value(rows)?;
    value["version"] = MUSICAL_EVENT_SOURCE_SCHEMA_VERSION.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    if value.get("version").and_then(Value::as_u64) != Some(version.into()) {
        return Err(StorageError::Invalid("invalid musical event source schema"));
    }
    let rows = value
        .as_object_mut()
        .and_then(|value| value.remove("musical_event_sources"))
        .ok_or(StorageError::Invalid(
            "musical event source inventory missing",
        ))?;
    if rows.as_array().is_none_or(|rows| {
        (rows.is_empty() && version == MUSICAL_EVENT_SOURCE_SCHEMA_VERSION)
            || rows.len() > 2 * hex_project::MAX_CLIPS
    }) {
        return Err(StorageError::Invalid(
            "invalid musical event source inventory",
        ));
    }
    let mut inventory = BTreeMap::new();
    for Row {
        path,
        zero_in_destination,
        length_raw,
    } in serde_json::from_value::<Vec<Row>>(rows)?
    {
        if path.is_empty()
            || path.len() > hex_project::MAX_RETAINED_AUDIO_DEPTH + 1
            || path.contains(&0)
            || inventory
                .insert(
                    path,
                    MusicalEventSource {
                        zero_in_destination,
                        length: BeatTime::from_raw(length_raw),
                    },
                )
                .is_some()
        {
            return Err(StorageError::Invalid(
                "invalid or duplicate musical event source owner",
            ));
        }
    }
    value["version"] = SIDECHAIN_SCHEMA_VERSION.into();
    let neutral = super::sidechain::decode(&serde_json::to_vec(&value)?)?;
    let mut snapshot = neutral.snapshot();
    walk(&neutral, &mut snapshot, |path, edit| {
        if let Some(source) = inventory.remove(path) {
            Arc::make_mut(&mut edit.domain).musical_event_source = Some(source);
            true
        } else {
            false
        }
    })?;
    if !inventory.is_empty() {
        return Err(StorageError::Invalid("musical event source owner missing"));
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}
