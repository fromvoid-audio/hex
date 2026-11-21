//! Schema70 gives musical events an independently retained source span.
//! Schemas1–69 derive both clocks from the same root. An inventory beside
//! schema69 identifies the arrangement clip and nested source path. The
//! existing event codec still owns all event IDs, geometry and media. Schema71
//! reuses this layout, allowing an empty musical inventory for extended frame
//! roots. Schema72 also uses this inventory. The top-level codec checks the
//! schema-specific frame extent. Schema123 wraps a complete earlier document
//! and restores the independent source spans of scoped musical clips. Earlier
//! formats continue to admit these spans only on unplaced clips.
use crate::{
    MUSICAL_EVENT_SOURCE_SCHEMA_VERSION, Project, SCOPED_MUSICAL_EVENTS_SCHEMA_VERSION,
    SIDECHAIN_SCHEMA_VERSION, StorageError,
};
use hex_project::{AudioClipEdit, BeatTime, MusicalEventSource, Snapshot, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use std::{collections::BTreeMap, sync::Arc};

const MAX_ROWS: usize = 2 * hex_project::MAX_CLIPS;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Row {
    #[serde(deserialize_with = "read_path")]
    path: Vec<u64>,
    zero_in_destination: u64,
    length_raw: u64,
}

fn read_path<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Vec<u64>, D::Error> {
    super::device_automation::bounded(value, hex_project::MAX_RETAINED_AUDIO_DEPTH + 1)
}

fn read_rows<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Vec<Row>, D::Error> {
    super::device_automation::bounded(value, MAX_ROWS)
}

pub(crate) fn contains(project: &Project) -> bool {
    contains_matching(project, |_| true)
}

pub(crate) fn contains_scoped(project: &Project) -> bool {
    contains_matching(project, |edit| edit.placement.is_some())
}

fn contains_matching(project: &Project, matches_view: impl Fn(&AudioClipEdit) -> bool) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(|clip| {
            std::iter::once(clip)
                .chain(clip.retained_audio_roots().map(|root| &root.clip))
                .any(|clip| {
                    clip.audio_edit().is_some_and(|edit| {
                        edit.domain.musical_event_source.is_some() && matches_view(edit)
                    })
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

fn capture(project: &Project, scoped: bool) -> Result<(Project, Vec<Row>), StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    let mut exceeded = false;
    walk(project, &mut snapshot, |path, edit| {
        if edit.placement.is_some() == scoped
            && let Some(source) = edit.domain.musical_event_source
        {
            if rows.len() == MAX_ROWS {
                exceeded = true;
                return false;
            }
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
    if exceeded {
        return Err(StorageError::Invalid(
            "musical event source inventory limit exceeded",
        ));
    }
    let neutral =
        Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    Ok((neutral, rows))
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    if contains_scoped(project) {
        return Err(StorageError::Invalid(
            "scoped musical events need their current schema",
        ));
    }
    // Newer schemas retain this field even when the project has no corresponding state.
    let (neutral, rows) = if contains(project) {
        let (neutral, rows) = capture(project, false)?;
        (Some(neutral), rows)
    } else {
        (None, Vec::new())
    };
    let mut value = super::sidechain::encode(neutral.as_ref().unwrap_or(project))?;
    value["musical_event_sources"] = serde_json::to_value(rows)?;
    value["version"] = MUSICAL_EVENT_SOURCE_SCHEMA_VERSION.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?, version)
}

pub(crate) fn decode_value(mut value: Value, version: u32) -> Result<Project, StorageError> {
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
        (rows.is_empty() && version == MUSICAL_EVENT_SOURCE_SCHEMA_VERSION) || rows.len() > MAX_ROWS
    }) {
        return Err(StorageError::Invalid(
            "invalid musical event source inventory",
        ));
    }
    let rows = serde_json::from_value(rows)?;
    value["version"] = SIDECHAIN_SCHEMA_VERSION.into();
    let neutral = super::sidechain::decode(&serde_json::to_vec(&value)?)?;
    restore(neutral, rows, false)
}

fn restore(neutral: Project, rows: Vec<Row>, scoped: bool) -> Result<Project, StorageError> {
    let mut inventory = BTreeMap::new();
    for Row {
        path,
        zero_in_destination,
        length_raw,
    } in rows
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
    let mut snapshot = neutral.snapshot();
    walk(&neutral, &mut snapshot, |path, edit| {
        if edit.placement.is_some() == scoped
            && edit.domain.musical_event_source.is_none()
            && let Some(source) = inventory.remove(path)
        {
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ScopedDocument<'a> {
    format: &'a str,
    version: u32,
    #[serde(borrow)]
    base: &'a RawValue,
    #[serde(deserialize_with = "read_rows")]
    musical_event_sources: Vec<Row>,
}

pub(crate) fn encode_scoped(project: &Project) -> Result<Vec<u8>, StorageError> {
    let (neutral, rows) = capture(project, true)?;
    if rows.is_empty() || contains_scoped(&neutral) {
        return Err(StorageError::Invalid(
            "incomplete scoped musical event inventory",
        ));
    }
    let base = crate::encode(&neutral)?;
    crate::encode_document(&ScopedDocument {
        format: "hex-project",
        version: SCOPED_MUSICAL_EVENTS_SCHEMA_VERSION,
        base: serde_json::from_slice(&base)?,
        musical_event_sources: rows,
    })
}

pub(crate) fn decode_scoped(bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.len() > crate::MAX_PROJECT_DOCUMENT_BYTES {
        return Err(StorageError::Invalid(
            "project exceeds the document size limit",
        ));
    }
    let document: ScopedDocument<'_> = serde_json::from_slice(bytes)?;
    #[derive(Deserialize)]
    struct Header {
        version: u32,
    }
    let base = document.base.get().as_bytes();
    let header: Header = serde_json::from_slice(base)?;
    if document.format != "hex-project"
        || document.version != SCOPED_MUSICAL_EVENTS_SCHEMA_VERSION
        || header.version >= document.version
        || document.musical_event_sources.is_empty()
        || document.musical_event_sources.len() > MAX_ROWS
    {
        return Err(StorageError::Invalid(
            "invalid scoped musical event envelope",
        ));
    }
    restore(crate::decode(base)?, document.musical_event_sources, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{
        AudioEventEdit, AudioViewResizeOperation, ClipFade, Command, Created, Editor,
    };

    #[test]
    fn scoped_sources_preserve_prior_outer_fades_and_round_trip_at_their_own_version() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "audio".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .unwrap()
        else {
            panic!("import");
        };
        let beat = BeatTime::ONE.raw();
        for command in [
            Command::SetAudioClipMusical {
                clip,
                enabled: true,
            },
            Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::ScopeRange {
                    start: beat,
                    end: 3 * beat,
                    looping: true,
                },
            },
        ] {
            editor.execute(command).unwrap();
        }
        let length = editor.project().clip(clip).unwrap().duration();
        editor
            .execute(Command::SetClipFadeEnvelope {
                clip,
                fade: ClipFade::new(length, 0, 1_400, 1_200).unwrap(),
            })
            .unwrap();
        let prior = editor.project().clone();
        let historical = super::super::outer_clip_fades::encode(&prior).unwrap();
        assert_eq!(
            crate::decode(&historical).unwrap().snapshot(),
            prior.snapshot()
        );
        assert_eq!(crate::encode(&prior).unwrap(), historical);

        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Place {
                    event: 1,
                    start: -(beat as i64),
                },
            })
            .unwrap();
        let project = editor.project();
        let document = crate::encode(project).unwrap();
        let mut json: Value = serde_json::from_slice(&document).unwrap();
        assert_eq!(json["version"], SCOPED_MUSICAL_EVENTS_SCHEMA_VERSION);
        assert_eq!(
            json["base"]["version"],
            crate::OUTER_CLIP_FADE_SCHEMA_VERSION
        );
        assert_eq!(
            crate::decode(&document).unwrap().snapshot(),
            project.snapshot()
        );
        let duplicate = json["musical_event_sources"][0].clone();
        json["musical_event_sources"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(crate::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    }
}
