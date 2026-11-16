//! Schema43 adds retained audio roots over schema42 event fades/directed views.
//! Main41 and the evidenced draft grouped41 lineage use disjoint complete
//! inventories; mixed layouts refuse. Draft grouped41 came from04d5ee56 and
//! embeds strict40 roots. Current43 embeds strict42 faded content roots, preserving
//! directed reads without recursively accepting grouped documents. Earlier
//! formats migrate in memory; no original bytes/media are rewritten. Schema43
//! has one explicit nullable group row per arranged audio clip so an omitted
//! container cannot silently fall back to its stripped predecessor root.
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
    #[serde(deserialize_with = "super::event_fades::required_fade")]
    fade: Option<super::event_fades::Fade>,
}
impl From<&AudioEvent> for Event {
    fn from(e: &AudioEvent) -> Self {
        Self {
            id: e.id,
            source: e.source,
            start: e.start,
            end: e.end,
            source_start: e.source_start,
            fade: e.fade.map(Into::into),
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
    next_id: u64,
    #[serde(deserialize_with = "bounded")]
    events: Vec<Event>,
    #[serde(deserialize_with = "bounded")]
    sources: Vec<Root>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    #[serde(deserialize_with = "required_group")]
    group: Option<State>,
}
fn required_group<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<State>, D::Error> {
    Option::deserialize(d)
}
/// Schema71 saves extended coordinates alongside the grouped inventory.
/// The historical base decoder sees a bounded, source-free carrier. Restore
/// the exact crop and fade origin together with its group, before validation.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtendedFrameGeometry {
    length: u64,
    view_start: u64,
    view_end: u64,
    fade_start: u64,
}

#[derive(Deserialize)]
struct Inventory(#[serde(deserialize_with = "bounded_inventory")] Vec<Row>);
fn bounded_inventory<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    items::<D, Row, { hex_project::MAX_CLIPS }>(d)
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
pub(super) fn isolated_root(
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
    Project::from_legacy_snapshot(snapshot).map_err(invalid)
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
    super::clip_aliases::require_legacy_compatible(project)?;
    super::track_groups::require_legacy_compatible(project)?;
    Project::from_legacy_snapshot(project.snapshot()).map_err(invalid)?;
    let mut snapshot = project.snapshot();
    let mut inventory = Vec::new();
    let mut extended = BTreeMap::new();
    for track in &mut snapshot.tracks {
        let clips = track.audio_clips_mut().map_err(invalid)?;
        for clip in clips {
            let Some(edit) = clip.edit.as_mut() else {
                inventory.push(Row {
                    clip: clip.id,
                    group: None,
                });
                continue;
            };
            let Some(events) = edit
                .domain
                .events
                .as_ref()
                .filter(|events| !events.sources.is_empty())
            else {
                inventory.push(Row {
                    clip: clip.id,
                    group: None,
                });
                continue;
            };
            let mut roots = Vec::new();
            for root in &events.sources {
                let isolated = isolated_root(project, root.clip.retained_snapshot(), &root.tempo)?;
                roots.push(Root {
                    id: root.id,
                    document: super::event_fades::encode(&isolated)?,
                });
            }
            inventory.push(Row {
                clip: clip.id,
                group: Some(State {
                    next_id: events.next_id,
                    events: events.events.iter().map(Event::from).collect(),
                    sources: roots,
                }),
            });
            let domain = Arc::make_mut(&mut edit.domain);
            domain.events = None;
            if let hex_project::AudioDomainClock::Frames(length) = domain.clock
                && length > hex_project::MAX_AUDIO_DOMAIN_FRAMES
            {
                extended.insert(
                    clip.id,
                    ExtendedFrameGeometry {
                        length,
                        view_start: edit.view_start,
                        view_end: edit.view_end,
                        fade_start: domain.fade_start,
                    },
                );
                clip.duration = clip.duration.min(hex_project::MAX_AUDIO_DOMAIN_FRAMES);
                edit.view_start = 0;
                edit.view_end = clip.duration;
                domain.fade_start = 0;
                domain.clock =
                    hex_project::AudioDomainClock::Frames(hex_project::MAX_AUDIO_DOMAIN_FRAMES);
            }
        }
    }
    let legacy = Project::from_legacy_snapshot(snapshot).map_err(invalid)?;
    let mut value = super::event_fades::encode(&legacy)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid grouped audio envelope"))?;
    object.insert("version".into(), 43.into());
    if !extended.is_empty() {
        let rows: BTreeMap<_, _> = inventory
            .iter()
            .map(|row| (row.clip, extended.remove(&row.clip)))
            .collect();
        object.insert(
            "extended_frame_geometry".into(),
            serde_json::to_value(rows)?,
        );
    }
    object.insert("grouped_audio".into(), serde_json::to_value(inventory)?);
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_version(super::audio_events::parse_unique(bytes)?, 43)
}
/// Decode a schema43 envelope parsed once by an upper migration layer.
pub(crate) fn decode_value(value: serde_json::Value) -> Result<Project, StorageError> {
    decode_version(value, 43)
}
/// Actual41 producers are selected by their distinct mandatory inventory.
/// The underlying frozen reader still validates every field and rejects extras.
pub(crate) fn decode41(bytes: &[u8]) -> Result<Project, StorageError> {
    let value = super::audio_events::parse_unique(bytes)?;
    if value.get("grouped_audio").is_some() {
        if value.get("content_views").is_some() || value.get("audio_event_fades").is_some() {
            return Err(StorageError::Invalid(
                "ambiguous schema41 branch inventories; original unchanged",
            ));
        }
        decode_version(value, 41)
    } else {
        super::content_views::decode_value(value)
    }
}
fn decode_version(mut value: serde_json::Value, version: u64) -> Result<Project, StorageError> {
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid grouped audio envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(version) {
        return Err(StorageError::Invalid("invalid grouped audio version"));
    }
    if version == 41
        && let Some(states) = object
            .get_mut("grouped_audio")
            .and_then(|v| v.as_array_mut())
    {
        for state in states {
            let row = state
                .as_object_mut()
                .ok_or(StorageError::Invalid("invalid draft grouped41 row"))?;
            let clip = row.remove("clip").ok_or(StorageError::Invalid(
                "draft grouped41 clip identity missing",
            ))?;
            if let Some(events) = row.get_mut("events").and_then(|v| v.as_array_mut()) {
                for event in events {
                    if let Some(event) = event.as_object_mut() {
                        if event.contains_key("fade") {
                            return Err(StorageError::Invalid(
                                "draft grouped41 cannot contain event fade fields",
                            ));
                        }
                        event.insert("fade".into(), serde_json::Value::Null);
                    }
                }
            }
            let group = std::mem::take(row);
            row.insert("clip".into(), clip);
            row.insert("group".into(), serde_json::Value::Object(group));
        }
    }
    let Inventory(states) = serde_json::from_value(
        object
            .remove("grouped_audio")
            .ok_or(StorageError::Invalid("grouped audio inventory missing"))?,
    )?;
    let mut extended = if let Some(rows) = object.remove("extended_frame_geometry") {
        if version != 43
            || rows
                .as_object()
                .is_none_or(|rows| rows.len() != states.len())
        {
            return Err(StorageError::Invalid(
                "invalid extended frame geometry inventory",
            ));
        }
        let rows: BTreeMap<u64, Option<ExtendedFrameGeometry>> = serde_json::from_value(rows)?;
        if !rows.values().any(Option::is_some)
            || rows.values().flatten().any(|geometry| {
                geometry.length <= hex_project::MAX_AUDIO_DOMAIN_FRAMES
                    || geometry.length > hex_project::MAX_SPARSE_AUDIO_TIMELINE_FRAMES
            })
        {
            return Err(StorageError::Invalid("invalid extended frame geometry"));
        }
        Some(rows)
    } else {
        None
    };
    object.insert(
        "version".into(),
        if version == 43 { 42.into() } else { 40.into() },
    );
    let project = if version == 43 {
        super::event_fades::decode_value(value)?
    } else {
        super::clip_activation::decode_value(value)?
    };
    let mut inventory = BTreeMap::new();
    let mut count = 0usize;
    for row in states {
        count = count
            .checked_add(row.group.as_ref().map_or(0, |state| state.sources.len()))
            .filter(|n| *n <= hex_project::MAX_CLIPS)
            .ok_or(StorageError::Invalid(
                "project retained audio root limit reached",
            ))?;
        if row
            .group
            .as_ref()
            .is_some_and(|state| state.sources.is_empty())
            || inventory.insert(row.clip, row.group).is_some()
        {
            return Err(StorageError::Invalid(
                "empty or duplicate grouped audio inventory",
            ));
        }
    }
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        for clip in track.audio_clips_mut().map_err(invalid)? {
            let group = match inventory.remove(&clip.id) {
                Some(group) => group,
                None if version == 41 => None,
                None => return Err(StorageError::Invalid("grouped audio clip row is missing")),
            };
            let geometry = if let Some(rows) = &mut extended {
                rows.remove(&clip.id)
                    .ok_or(StorageError::Invalid("extended frame geometry row missing"))?
            } else {
                None
            };
            let Some(state) = group else {
                if geometry.is_some() {
                    return Err(StorageError::Invalid(
                        "extended frame geometry needs grouped audio",
                    ));
                }
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
                let root_version = root.document.get("version").and_then(|v| v.as_u64());
                let expected = if version == 43 { 42 } else { 40 };
                if root_version != Some(expected) {
                    return Err(StorageError::Invalid(
                        "retained root schema differs from its owning envelope",
                    ));
                }
                let root_project = if version == 43 {
                    super::event_fades::decode_value(root.document)?
                } else {
                    super::clip_activation::decode_value(root.document)?
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
            let domain = Arc::make_mut(&mut edit.domain);
            if let Some(geometry) = geometry {
                let duration = geometry
                    .view_end
                    .checked_sub(geometry.view_start)
                    .filter(|duration| *duration > 0)
                    .ok_or(StorageError::Invalid("invalid extended frame crop"))?;
                if domain.clock
                    != hex_project::AudioDomainClock::Frames(hex_project::MAX_AUDIO_DOMAIN_FRAMES)
                    || domain.fade_start != 0
                    || edit.view_start != 0
                    || edit.view_end != duration.min(hex_project::MAX_AUDIO_DOMAIN_FRAMES)
                    || clip.duration != edit.view_end
                {
                    return Err(StorageError::Invalid(
                        "extended frame carrier does not match its crop",
                    ));
                }
                clip.duration = duration;
                domain.clock = hex_project::AudioDomainClock::Frames(geometry.length);
                domain.fade_start = geometry.fade_start;
                edit.view_start = geometry.view_start;
                edit.view_end = geometry.view_end;
            }
            domain.events = Some(Arc::new(AudioEvents {
                next_id: state.next_id,
                sources,
                events: state
                    .events
                    .into_iter()
                    .map(|e| {
                        Ok(AudioEvent {
                            active: true,
                            gain: hex_project::ClipGain::UNITY,
                            id: e.id,
                            source: e.source,
                            start: e.start,
                            end: e.end,
                            source_start: e.source_start,
                            fade: e.fade.map(super::event_fades::Fade::checked).transpose()?,
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?,
            }));
        }
    }
    if !inventory.is_empty() {
        return Err(StorageError::Invalid(
            "grouped audio references a missing clip",
        ));
    }
    if extended.is_some_and(|rows| !rows.is_empty()) {
        return Err(StorageError::Invalid(
            "extended frame geometry owner missing",
        ));
    }
    Project::from_legacy_snapshot(snapshot).map_err(invalid)
}
