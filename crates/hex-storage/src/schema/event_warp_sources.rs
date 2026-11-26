//! Schema124 extends a complete earlier document with live event marker
//! forks. Earlier projects have no forks, so their original writers and
//! readers still use the same event IDs, geometry, tempo and media.
use super::{audio_event_roots::walk, device_automation::bounded};
use crate::{EVENT_WARP_SOURCE_SCHEMA_VERSION, Project, StorageError};
use hex_project::{AudioEventWarpSource, BeatTime, WarpMarker};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    source_frame: u64,
    beat_raw: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Source {
    id: u64,
    #[serde(deserialize_with = "read_markers")]
    markers: Vec<Marker>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Read {
    event: u64,
    source: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Row {
    #[serde(deserialize_with = "read_path")]
    path: Vec<u64>,
    #[serde(deserialize_with = "read_events")]
    sources: Vec<Source>,
    #[serde(deserialize_with = "read_events")]
    reads: Vec<Read>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document<'a> {
    format: &'a str,
    version: u32,
    #[serde(borrow)]
    base: &'a RawValue,
    #[serde(deserialize_with = "read_rows")]
    event_warp_sources: Vec<Row>,
}

const MAX_ROWS: usize = 2 * hex_project::MAX_CLIPS;

fn read_markers<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Vec<Marker>, D::Error> {
    bounded(value, hex_project::MAX_AUDIO_POINTS)
}

fn read_events<'de, D, T>(value: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    bounded(value, hex_project::MAX_AUDIO_EVENTS)
}

fn read_path<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Vec<u64>, D::Error> {
    bounded(value, hex_project::MAX_RETAINED_AUDIO_DEPTH + 1)
}

fn read_rows<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Vec<Row>, D::Error> {
    bounded(value, MAX_ROWS)
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
                        .and_then(|edit| edit.domain.events.as_ref())
                        .is_some_and(|events| !events.warp_sources.is_empty())
                })
        })
}

pub(crate) fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    walk(project, &mut snapshot, |path, edit| {
        let Some(events) = edit
            .domain
            .events
            .as_ref()
            .filter(|events| !events.warp_sources.is_empty())
        else {
            return Ok(false);
        };
        if rows.len() == MAX_ROWS {
            return Err(StorageError::Invalid(
                "event warp source inventory limit exceeded",
            ));
        }
        let sources = events
            .warp_sources
            .iter()
            .map(|source| Source {
                id: source.id,
                markers: source
                    .markers
                    .iter()
                    .map(|marker| Marker {
                        source_frame: marker.source_frame,
                        beat_raw: marker.beat.raw(),
                    })
                    .collect(),
            })
            .collect();
        let reads = events
            .events
            .iter()
            .filter(|event| event.source != 0)
            .map(|event| Read {
                event: event.id,
                source: event.source,
            })
            .collect();
        rows.push(Row {
            path: path.to_vec(),
            sources,
            reads,
        });
        let mut events = events.as_ref().clone();
        events.warp_sources.clear();
        for event in &mut events.events {
            event.source = 0;
        }
        Arc::make_mut(&mut edit.domain).events = Some(Arc::new(events));
        Ok(true)
    })?;
    let base = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    if rows.is_empty() || contains(&base) {
        return Err(StorageError::Invalid(
            "incomplete event warp source inventory",
        ));
    }
    let bytes = crate::encode(&base)?;
    crate::encode_document(&Document {
        format: "hex-project",
        version: EVENT_WARP_SOURCE_SCHEMA_VERSION,
        base: serde_json::from_slice(&bytes)?,
        event_warp_sources: rows,
    })
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.len() > crate::MAX_PROJECT_DOCUMENT_BYTES {
        return Err(StorageError::Invalid(
            "project exceeds the document size limit",
        ));
    }
    let document: Document<'_> = serde_json::from_slice(bytes)?;
    #[derive(Deserialize)]
    struct Header {
        version: u32,
    }
    let base = document.base.get().as_bytes();
    let header: Header = serde_json::from_slice(base)?;
    if document.format != "hex-project"
        || document.version != EVENT_WARP_SOURCE_SCHEMA_VERSION
        || header.version >= document.version
        || document.event_warp_sources.is_empty()
    {
        return Err(StorageError::Invalid("invalid event warp source envelope"));
    }
    let base = crate::decode(base)?;
    let mut inventory = BTreeMap::new();
    for row in document.event_warp_sources {
        if row.path.is_empty()
            || row.sources.is_empty()
            || inventory
                .insert(row.path, (row.sources, row.reads))
                .is_some()
        {
            return Err(StorageError::Invalid(
                "invalid or duplicate event warp source owner",
            ));
        }
    }
    let mut snapshot = base.snapshot();
    walk(&base, &mut snapshot, |path, edit| {
        let Some((sources, reads)) = inventory.remove(path) else {
            return Ok(false);
        };
        let events = Arc::make_mut(&mut edit.domain)
            .events
            .as_mut()
            .filter(|events| events.sources.is_empty() && events.warp_sources.is_empty())
            .ok_or(StorageError::Invalid(
                "event warp source owner is not an ordinary root",
            ))?;
        let events = Arc::make_mut(events);
        events.warp_sources = sources
            .into_iter()
            .map(|source| AudioEventWarpSource {
                id: source.id,
                markers: source
                    .markers
                    .into_iter()
                    .map(|marker| WarpMarker {
                        source_frame: marker.source_frame,
                        beat: BeatTime::from_raw(marker.beat_raw),
                    })
                    .collect(),
            })
            .collect();
        for read in reads {
            let event = events
                .events
                .iter_mut()
                .find(|event| event.id == read.event && event.source == 0 && read.source != 0)
                .ok_or(StorageError::Invalid(
                    "invalid or duplicate event warp read",
                ))?;
            event.source = read.source;
        }
        Ok(true)
    })?;
    if !inventory.is_empty() {
        return Err(StorageError::Invalid("event warp source owner is missing"));
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{AudioEventEdit, AudioViewResizeOperation, Command, Created, Editor};
    use serde_json::Value;

    #[test]
    fn live_event_warp_inventory_round_trips_over_a_complete_schema123_document() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut import = || {
            let Created::Imported { asset, track, clip } = editor
                .execute(Command::ImportAudioTrack {
                    name: "audio".into(),
                    path: "source.wav".into(),
                    sample_rate: 48_000,
                    frames: 96_000,
                })
                .unwrap()
            else {
                panic!("audio fixture");
            };
            (track, asset, clip)
        };
        let (_, _, scoped) = import();
        let (track, asset, split) = import();
        let beat = BeatTime::ONE.raw();
        for command in [
            Command::SetAudioClipMusical {
                clip: scoped,
                enabled: true,
            },
            Command::ResizeAudioClipView {
                clip: scoped,
                operation: AudioViewResizeOperation::ScopeRange {
                    start: beat,
                    end: 3 * beat,
                    looping: true,
                },
            },
            Command::EditAudioEvents {
                clip: scoped,
                operation: AudioEventEdit::Place {
                    event: 1,
                    start: -(beat as i64),
                },
            },
        ] {
            editor.execute(command).unwrap();
        }
        let mut warp = editor
            .project()
            .audio_warp_settings_for_enable(split)
            .unwrap();
        assert!(warp.insert_marker(24_000).unwrap());
        editor
            .execute(Command::SetAudioClipWarp {
                clip: split,
                settings: Some(warp.clone()),
            })
            .unwrap();
        editor
            .execute(Command::EditAudioEvents {
                clip: split,
                operation: AudioEventEdit::Split { at: 2 * beat },
            })
            .unwrap();
        let prior = crate::encode_stacked_legacy(editor.project()).unwrap();
        assert_eq!(
            crate::decode(&prior).unwrap().snapshot(),
            editor.project().snapshot()
        );
        assert_eq!(
            crate::decode(&crate::encode(editor.project()).unwrap())
                .unwrap()
                .snapshot(),
            editor.project().snapshot()
        );

        assert!(
            warp.move_marker(24_000, BeatTime::from_raw(beat + beat / 4))
                .unwrap()
        );
        editor
            .execute(Command::EditAudioEvents {
                clip: split,
                operation: AudioEventEdit::SetRetainedWarp {
                    event: 1,
                    settings: warp,
                },
            })
            .unwrap();
        let project = editor.project();
        let encoded = crate::encode_stacked_legacy(project).unwrap();
        assert_eq!(
            crate::schema_version_for(project),
            crate::CAPABILITY_MANIFEST_SCHEMA_VERSION
        );
        assert_eq!(
            crate::decode(&encoded).unwrap().snapshot(),
            project.snapshot()
        );
        assert_eq!(
            crate::decode(&crate::encode(project).unwrap())
                .unwrap()
                .snapshot(),
            project.snapshot()
        );
        let document: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            document["base"],
            serde_json::from_slice::<Value>(&prior).unwrap()
        );
        let mut repeated = document.clone();
        let reads = repeated["event_warp_sources"][0]["reads"]
            .as_array_mut()
            .unwrap();
        reads.push(reads[0].clone());
        assert!(crate::decode(&serde_json::to_vec(&repeated).unwrap()).is_err());
        let mut missing = document;
        missing["event_warp_sources"][0]["reads"][0]["source"] = 9_000.into();
        assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());

        let split_end = project.clip(split).unwrap().duration();
        let Created::Clip(neighbor) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: split_end,
                duration: 1_000,
                source_offset: 0,
                source_length: 1_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("adjacent audio");
        };
        let Created::Clip(group) = editor
            .execute(Command::GroupClips(vec![split, neighbor]))
            .unwrap()
        else {
            panic!("grouped audio");
        };
        let project = editor.project();
        let root = &project.clip(group).unwrap().audio_events().sources[0];
        assert_eq!(root.clip.audio_events().warp_sources.len(), 1);
        let encoded = crate::encode_stacked_legacy(project).unwrap();
        let document: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            document["event_warp_sources"][0]["path"],
            serde_json::json!([group.value(), root.id])
        );
        assert_eq!(
            crate::decode(&encoded).unwrap().snapshot(),
            project.snapshot()
        );
        assert_eq!(
            crate::decode(&crate::encode(project).unwrap())
                .unwrap()
                .snapshot(),
            project.snapshot()
        );
    }
}
