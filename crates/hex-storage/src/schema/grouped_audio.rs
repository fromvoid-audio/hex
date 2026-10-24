//! Schema42 adds retained audio roots over schema41 directed content views.
//! Main41 and the evidenced draft grouped41 lineage use disjoint complete
//! inventories; mixed layouts refuse. Draft grouped41 came from04d5ee56 and
//! embeds strict40 roots. Current42 embeds strict41 content roots, preserving
//! directed reads without recursively accepting grouped documents. Earlier
//! formats migrate in memory; no original bytes/media are rewritten.
use crate::{Project, StorageError};
use hex_project::{AudioEvent, AudioEvents, ClipSnapshot, Command, Editor, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

fn invalid(e: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(e.0)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    id: u64,
    source: u64,
    start: u64,
    end: u64,
    source_start: u64,
}
impl From<&AudioEvent> for Event {
    fn from(e: &AudioEvent) -> Self {
        Self {
            id: e.id,
            source: e.source,
            start: e.start,
            end: e.end,
            source_start: e.source_start,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Root {
    id: u64,
    document: serde_json::Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    clip: u64,
    next_id: u64,
    #[serde(deserialize_with = "bounded")]
    events: Vec<Event>,
    #[serde(deserialize_with = "bounded")]
    sources: Vec<Root>,
}
#[derive(Deserialize)]
struct Inventory(#[serde(deserialize_with = "bounded_inventory")] Vec<State>);
fn bounded_inventory<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<State>, D::Error> {
    items::<D, State, { hex_project::MAX_CLIPS }>(d)
}
fn bounded<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    items::<D, T, { hex_project::MAX_AUDIO_EVENTS }>(d)
}
fn items<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>, const MAX: usize>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    struct Rows<T, const MAX: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const MAX: usize> serde::de::Visitor<'de> for Rows<T, MAX> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded grouped audio inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == MAX {
                    return Err(serde::de::Error::custom(
                        "grouped audio inventory limit reached",
                    ));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows::<T, MAX>(std::marker::PhantomData))
}
fn isolated_root(
    project: &Project,
    clip: ClipSnapshot,
    tempo: &hex_project::TempoMap,
) -> Result<Project, StorageError> {
    let mut editor =
        Editor::new(Project::new(project.sample_rate()).map_err(invalid)?, 1).map_err(invalid)?;
    editor
        .execute(Command::AddTrack {
            name: "retained audio".into(),
        })
        .map_err(invalid)?;
    let mut snapshot = editor.project().snapshot();
    let source = project
        .assets()
        .find(|asset| asset.id().value() == clip.asset)
        .ok_or(StorageError::Invalid("retained audio media missing"))?;
    let asset = hex_project::AssetSnapshot {
        original_source: source.original_source(),
        id: source.id().value(),
        path: source.path().into(),
        source_sample_rate: source.source_sample_rate(),
        source_frames: source.source_frames(),
        playback_frames: source.playback_frames(),
        conversion_version: source.conversion_version(),
    };
    let track_id = clip
        .id
        .max(asset.id)
        .checked_add(1)
        .ok_or(StorageError::Invalid("retained root identity exhausted"))?;
    snapshot.next_id = track_id
        .checked_add(1)
        .ok_or(StorageError::Invalid("retained root identity exhausted"))?;
    snapshot.assets = vec![asset];
    let track = snapshot
        .tracks
        .first_mut()
        .ok_or(StorageError::Invalid("retained root track missing"))?;
    track.id = track_id;
    track.color = hex_project::ElementColor::Gray;
    track.content = TrackContentSnapshot::Hybrid {
        audio_clips: vec![clip],
        midi_clips: Vec::new(),
    };
    snapshot.tempo_points = tempo.points().to_vec();
    Project::from_snapshot(snapshot).map_err(invalid)
}
pub(crate) fn require_legacy_compatible(project: &Project) -> Result<(), StorageError> {
    if project
        .tracks()
        .iter()
        .flat_map(|t| t.clips())
        .any(|c| c.has_retained_audio_roots())
    {
        Err(StorageError::Invalid(
            "older schema cannot save retained audio event sources",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    Project::from_snapshot(project.snapshot()).map_err(invalid)?;
    let mut snapshot = project.snapshot();
    let mut inventory = Vec::new();
    for track in &mut snapshot.tracks {
        let clips = track.audio_clips_mut().map_err(invalid)?;
        for clip in clips {
            let Some(edit) = clip.edit.as_mut() else {
                continue;
            };
            let Some(events) = edit
                .domain
                .events
                .as_ref()
                .filter(|e| !e.sources.is_empty())
            else {
                continue;
            };
            let mut roots = Vec::new();
            for root in &events.sources {
                let isolated = isolated_root(project, root.clip.retained_snapshot(), &root.tempo)?;
                roots.push(Root {
                    id: root.id,
                    document: super::content_views::encode(&isolated)?,
                });
            }
            inventory.push(State {
                clip: clip.id,
                next_id: events.next_id,
                events: events.events.iter().map(Event::from).collect(),
                sources: roots,
            });
            Arc::make_mut(&mut edit.domain).events = None;
        }
    }
    let legacy = Project::from_snapshot(snapshot).map_err(invalid)?;
    let mut value = super::content_views::encode(&legacy)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid grouped audio envelope"))?;
    object.insert("version".into(), 42.into());
    object.insert("grouped_audio".into(), serde_json::to_value(inventory)?);
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_version(bytes, 42)
}
/// Actual41 producers are selected by their distinct mandatory inventory.
/// The underlying frozen reader still validates every field and rejects extras.
pub(crate) fn decode41(bytes: &[u8]) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    if value.get("grouped_audio").is_some() {
        if value.get("content_views").is_some() {
            return Err(StorageError::Invalid(
                "ambiguous schema41 branch inventories; original unchanged",
            ));
        }
        decode_version(bytes, 41)
    } else {
        super::content_views::decode(bytes)
    }
}
fn decode_version(bytes: &[u8], version: u64) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid grouped audio envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(version) {
        return Err(StorageError::Invalid("invalid grouped audio version"));
    }
    let Inventory(states) = serde_json::from_value(
        object
            .remove("grouped_audio")
            .ok_or(StorageError::Invalid("grouped audio inventory missing"))?,
    )?;
    object.insert(
        "version".into(),
        if version == 42 { 41.into() } else { 40.into() },
    );
    let base = serde_json::to_vec(&value)?;
    let project = if version == 42 {
        super::content_views::decode(&base)?
    } else {
        super::clip_activation::decode(&base)?
    };
    let mut inventory = BTreeMap::new();
    let mut count = 0usize;
    for state in states {
        count = count
            .checked_add(state.sources.len())
            .filter(|n| *n <= hex_project::MAX_CLIPS)
            .ok_or(StorageError::Invalid(
                "project retained audio root limit reached",
            ))?;
        if state.sources.is_empty() || inventory.insert(state.clip, state).is_some() {
            return Err(StorageError::Invalid(
                "empty or duplicate grouped audio inventory",
            ));
        }
    }
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        for clip in track.audio_clips_mut().map_err(invalid)? {
            let Some(state) = inventory.remove(&clip.id) else {
                continue;
            };
            let edit = clip.edit.as_mut().ok_or(StorageError::Invalid(
                "grouped audio processing root missing",
            ))?;
            if edit.domain.events.is_some() {
                return Err(StorageError::Invalid(
                    "grouped audio contains competing event inventories",
                ));
            }
            let mut sources = Vec::new();
            for root in state.sources {
                let encoded = serde_json::to_vec(&root.document)?;
                let root_project = match root.document.get("version").and_then(|v| v.as_u64()) {
                    Some(40) => super::clip_activation::decode(&encoded)?,
                    Some(41) => super::content_views::decode(&encoded)?,
                    _ => {
                        return Err(StorageError::Invalid(
                            "retained root requires a non-grouped schema40 or content41 envelope",
                        ));
                    }
                };
                let source = root_project
                    .tracks()
                    .first()
                    .and_then(|t| t.clips().first())
                    .ok_or(StorageError::Invalid("retained audio root missing"))?;
                let canonical = isolated_root(
                    &root_project,
                    source.retained_snapshot(),
                    root_project.tempo_map(),
                )?;
                if canonical.snapshot() != root_project.snapshot() {
                    return Err(StorageError::Invalid(
                        "retained audio envelope contains unrelated state",
                    ));
                }
                // Resolve against the owning project's catalog too. An embedded
                // root must not replace the canonical asset metadata or path.
                let asset = project
                    .asset(source.asset())
                    .ok_or(StorageError::Invalid("retained audio media missing"))?;
                if root_project.asset(source.asset()) != Some(asset) {
                    return Err(StorageError::Invalid(
                        "retained audio media differs from project catalog",
                    ));
                }
                sources.push(
                    project
                        .restore_retained_audio_root(
                            root.id,
                            source.retained_snapshot(),
                            Arc::new(root_project.tempo_map().clone()),
                        )
                        .map_err(invalid)?,
                );
            }
            Arc::make_mut(&mut edit.domain).events = Some(Arc::new(AudioEvents {
                next_id: state.next_id,
                sources,
                events: state
                    .events
                    .into_iter()
                    .map(|e| AudioEvent {
                        id: e.id,
                        source: e.source,
                        start: e.start,
                        end: e.end,
                        source_start: e.source_start,
                    })
                    .collect(),
            }));
        }
    }
    if !inventory.is_empty() {
        return Err(StorageError::Invalid(
            "grouped audio references a missing clip",
        ));
    }
    Project::from_snapshot(snapshot).map_err(invalid)
}
