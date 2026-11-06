//! Schema52 adds explicit shared clip sources and a complete member reference
//! inventory over strict schema51 complete project state. Aliased fields carry required
//! null markers, never a second editable copy of the source. One audio member
//! validates each source through the predecessor codec; neutral wrappers keep
//! that private composition from expanding event/warp payloads per alias. Final
//! snapshot admission installs one shared Arc and validates the complete project.
//! Older documents acquire empty alias inventories in memory without file I/O.
use crate::{Project, StorageError};
use hex_project::{
    AliasContentSnapshot, AliasMemberSnapshot, AliasSourceSnapshot, AudioClipContent,
    ClipAliasesSnapshot, MidiNoteSnapshot, TrackContentSnapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, sync::Arc};

const SHARED_DOMAIN_KEYS: [&str; 8] = [
    "source_offset",
    "source_length",
    "clock",
    "pitch_cents",
    "sample_micro_bpm",
    "warp_enabled",
    "warp",
    "source_gain",
];
fn invalid(e: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(e.0)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    id: u64,
    content: Content,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Content {
    Midi {
        source_length_raw: u64,
        #[serde(deserialize_with = "bounded_notes")]
        notes: Vec<Note>,
    },
    Audio {
        asset: u64,
        payload: Box<AudioPayload>,
    },
    ConsoleAudio {
        asset: u64,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioPayload {
    domain: Value,
    next_id: Value,
    events: Value,
    event_fades: Value,
    group: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Note {
    id: u64,
    start_raw: u64,
    duration_raw: u64,
    pitch: u8,
    channel: u8,
    velocity: u8,
}
impl From<MidiNoteSnapshot> for Note {
    fn from(note: MidiNoteSnapshot) -> Self {
        Self {
            id: note.id,
            start_raw: note.start_raw,
            duration_raw: note.duration_raw,
            pitch: note.pitch,
            channel: note.channel,
            velocity: note.velocity,
        }
    }
}
impl From<Note> for MidiNoteSnapshot {
    fn from(note: Note) -> Self {
        Self {
            id: note.id,
            active: true,
            start_raw: note.start_raw,
            duration_raw: note.duration_raw,
            pitch: note.pitch,
            channel: note.channel,
            velocity: note.velocity,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    clip: u64,
    source: u64,
}
#[derive(Deserialize)]
struct Sources(#[serde(deserialize_with = "bounded_sources")] Vec<Source>);
#[derive(Deserialize)]
struct Members(#[serde(deserialize_with = "bounded_members")] Vec<Member>);
fn bounded_sources<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Source>, D::Error> {
    bounded::<D, Source, { hex_project::MAX_CLIPS }>(d)
}
fn bounded_members<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Member>, D::Error> {
    bounded::<D, Member, { hex_project::MAX_CLIPS }>(d)
}
fn bounded_notes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Note>, D::Error> {
    bounded::<D, Note, { hex_project::MAX_NOTES }>(d)
}
fn bounded<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>, const MAX: usize>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    struct Rows<T, const MAX: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const MAX: usize> serde::de::Visitor<'de> for Rows<T, MAX> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded clip alias inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == MAX {
                    return Err(serde::de::Error::custom(
                        "clip alias inventory limit reached",
                    ));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows::<T, MAX>(std::marker::PhantomData))
}

pub(crate) fn require_legacy_compatible(project: &Project) -> Result<(), StorageError> {
    if project.has_clip_aliases() {
        return Err(StorageError::Invalid(
            "older schema cannot save linked clip sources",
        ));
    }
    Ok(())
}
fn neutral(content: &AudioClipContent) -> Arc<AudioClipContent> {
    Arc::new(AudioClipContent {
        events: None,
        musical_event_source: None,
        source_offset: content.source_offset,
        source_length: content.source_length,
        clock: content.clock,
        pitch_cents: content.pitch_cents,
        sample_bpm: content.sample_bpm,
        warp: None,
        warp_enabled: false,
        source_gain: Vec::new(),
        // Placeholder only; the representative carries the shared law.
        warp_resampler: hex_project::WarpResampler::implied(content.pitch_cents, false),
    })
}
fn representatives(
    members: &[Member],
    sources: &BTreeMap<u64, Source>,
) -> Result<BTreeMap<u64, u64>, StorageError> {
    let mut seen_members = BTreeMap::new();
    let mut representatives = BTreeMap::new();
    for member in members {
        if member.clip == 0
            || !sources.contains_key(&member.source)
            || seen_members.insert(member.clip, member.source).is_some()
        {
            return Err(StorageError::Invalid(
                "invalid or duplicate clip alias member",
            ));
        }
        representatives.entry(member.source).or_insert(member.clip);
    }
    if representatives.len() != sources.len() {
        return Err(StorageError::Invalid("clip alias source has no member"));
    }
    Ok(representatives)
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    Project::from_snapshot(project.snapshot()).map_err(invalid)?;
    let mut snapshot = project.snapshot();
    let aliases = std::mem::take(&mut snapshot.aliases);
    let members: Vec<Member> = aliases
        .members
        .iter()
        .map(|m| Member {
            clip: m.clip,
            source: m.source,
        })
        .collect();
    let mut first = BTreeMap::new();
    for member in &members {
        first.entry(member.source).or_insert(member.clip);
    }
    // Only the representative retains expensive source payloads during the old
    // codec's private composition. All final bytes still use explicit markers.
    let member_sources: BTreeMap<_, _> = members.iter().map(|m| (m.clip, m.source)).collect();
    for track in &mut snapshot.tracks {
        let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut track.content else {
            continue;
        };
        for clip in audio_clips {
            if let Some(source) = member_sources.get(&clip.id)
                && first.get(source) != Some(&clip.id)
            {
                let edit = clip
                    .edit
                    .as_mut()
                    .ok_or(StorageError::Invalid("alias audio member has no root"))?;
                let content = neutral(&edit.domain.content);
                Arc::make_mut(&mut edit.domain).content = content;
            }
        }
    }
    let core = Project::from_snapshot(snapshot).map_err(invalid)?;
    let mut value = super::crossfades::encode(&core)?;
    let index = CodecIndex::new(&value)?;
    let mut sources = BTreeMap::new();
    for source in aliases.sources {
        let content = match source.content {
            AliasContentSnapshot::Midi {
                source_length_raw,
                notes,
            } => Content::Midi {
                source_length_raw,
                notes: notes.into_iter().map(Into::into).collect(),
            },
            AliasContentSnapshot::Audio { asset, .. } => {
                let representative = *first
                    .get(&source.id)
                    .ok_or(StorageError::Invalid("alias audio source has no member"))?;
                Content::Audio {
                    asset,
                    payload: Box::new(extract_audio(&value, &index, representative)?),
                }
            }
            AliasContentSnapshot::ConsoleAudio { asset } => Content::ConsoleAudio { asset },
        };
        if sources
            .insert(
                source.id,
                Source {
                    id: source.id,
                    content,
                },
            )
            .is_some()
        {
            return Err(StorageError::Invalid("duplicate clip alias source"));
        }
    }
    representatives(&members, &sources)?;
    for member in &members {
        let source = sources
            .get(&member.source)
            .ok_or(StorageError::Invalid("clip alias source missing"))?;
        mark_member(&mut value, &index, member.clip, &source.content)?;
    }
    let object = object_mut(&mut value)?;
    object.insert("version".into(), 52.into());
    object.insert(
        "alias_sources".into(),
        serde_json::to_value(sources.into_values().collect::<Vec<_>>())?,
    );
    object.insert("alias_members".into(), serde_json::to_value(members)?);
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: Value) -> Result<Project, StorageError> {
    let object = object_mut(&mut value)?;
    if object.get("version").and_then(Value::as_u64) != Some(52) {
        return Err(StorageError::Invalid("invalid clip alias version"));
    }
    let Sources(sources) = serde_json::from_value(
        object
            .remove("alias_sources")
            .ok_or(StorageError::Invalid("clip alias sources missing"))?,
    )?;
    let Members(members) = serde_json::from_value(
        object
            .remove("alias_members")
            .ok_or(StorageError::Invalid("clip alias members missing"))?,
    )?;
    let mut by_source = BTreeMap::new();
    for source in sources {
        if source.id == 0 {
            return Err(StorageError::Invalid("invalid clip alias source identity"));
        }
        if let Content::Audio { payload, .. } = &source.content {
            validate_shared_domain(&payload.domain)?;
        }
        if by_source.insert(source.id, source).is_some() {
            return Err(StorageError::Invalid("duplicate clip alias source"));
        }
    }
    let first = representatives(&members, &by_source)?;
    let index = CodecIndex::new(&value)?;
    for member in &members {
        let source = by_source
            .get(&member.source)
            .ok_or(StorageError::Invalid("clip alias source missing"))?;
        restore_member(
            &mut value,
            &index,
            member.clip,
            &source.content,
            first.get(&member.source) == Some(&member.clip),
        )?;
    }
    object_mut(&mut value)?.insert("version".into(), 51.into());
    let core = super::crossfades::decode_value(value)?;
    let mut snapshot = core.snapshot();
    let mut decoded = Vec::new();
    for (id, source) in by_source {
        let content = match source.content {
            Content::Midi {
                source_length_raw,
                notes,
            } => AliasContentSnapshot::Midi {
                source_length_raw,
                notes: notes.into_iter().map(Into::into).collect(),
            },
            Content::Audio { asset, .. } => {
                let clip = *first
                    .get(&id)
                    .ok_or(StorageError::Invalid("alias audio source has no member"))?;
                let content = core
                    .tracks()
                    .iter()
                    .flat_map(|track| track.clips())
                    .find(|c| c.id().value() == clip)
                    .and_then(|c| c.audio_edit())
                    .map(|edit| Arc::clone(&edit.domain.content))
                    .ok_or(StorageError::Invalid("alias audio representative missing"))?;
                AliasContentSnapshot::Audio { asset, content }
            }
            Content::ConsoleAudio { asset } => AliasContentSnapshot::ConsoleAudio { asset },
        };
        decoded.push(AliasSourceSnapshot { id, content });
    }
    let source_index: BTreeMap<_, _> = decoded.iter().map(|s| (s.id, &s.content)).collect();
    let member_index: BTreeMap<_, _> = members.iter().map(|m| (m.clip, m.source)).collect();
    for track in &mut snapshot.tracks {
        let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut track.content else {
            continue;
        };
        for clip in audio_clips {
            if let Some(AliasContentSnapshot::Audio { content, .. }) = member_index
                .get(&clip.id)
                .and_then(|id| source_index.get(id))
                .copied()
            {
                let edit = clip
                    .edit
                    .as_mut()
                    .ok_or(StorageError::Invalid("alias audio wrapper missing"))?;
                Arc::make_mut(&mut edit.domain).content = Arc::clone(content);
            }
        }
    }
    snapshot.aliases = ClipAliasesSnapshot {
        sources: decoded,
        members: members
            .into_iter()
            .map(|m| AliasMemberSnapshot {
                clip: m.clip,
                source: m.source,
            })
            .collect(),
    };
    Project::from_snapshot(snapshot).map_err(invalid)
}

#[derive(Clone, Copy)]
enum Location {
    Audio(usize, usize),
    Midi(usize, usize),
    ConsoleAudio(usize),
    ConsoleMidi(usize),
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, StorageError> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or(StorageError::Invalid(
            "clip alias wrapper inventory is malformed",
        ))
}
struct CodecIndex {
    clips: BTreeMap<u64, Location>,
    rows: BTreeMap<&'static str, BTreeMap<u64, usize>>,
}
impl CodecIndex {
    fn new(value: &Value) -> Result<Self, StorageError> {
        let mut clips = BTreeMap::new();
        for (ti, track) in array(value, "tracks")?.iter().enumerate() {
            for (key, audio) in [("audio_clips", true), ("midi_clips", false)] {
                for (ci, clip) in array(track, key)?.iter().enumerate() {
                    let id =
                        clip.get("id")
                            .and_then(Value::as_u64)
                            .ok_or(StorageError::Invalid(
                                "clip alias wrapper identity is malformed",
                            ))?;
                    let location = if audio {
                        Location::Audio(ti, ci)
                    } else {
                        Location::Midi(ti, ci)
                    };
                    if clips.insert(id, location).is_some() {
                        return Err(StorageError::Invalid(
                            "duplicate clip alias wrapper identity",
                        ));
                    }
                }
            }
        }
        for (ci, cell) in array(
            value
                .get("session")
                .ok_or(StorageError::Invalid("session inventory missing"))?,
            "cells",
        )?
        .iter()
        .enumerate()
        {
            let content = cell
                .get("content")
                .ok_or(StorageError::Invalid("session content missing"))?;
            let candidate = match content.get("kind").and_then(Value::as_str) {
                Some("audio") => Some(Location::ConsoleAudio(ci)),
                Some("midi") => Some(Location::ConsoleMidi(ci)),
                Some("stop") => None,
                _ => return Err(StorageError::Invalid("clip alias session kind is invalid")),
            };
            if let Some(location) = candidate {
                let id = content
                    .get("clip")
                    .and_then(|c| c.get("id"))
                    .and_then(Value::as_u64)
                    .ok_or(StorageError::Invalid(
                        "session clip alias identity is malformed",
                    ))?;
                if clips.insert(id, location).is_some() {
                    return Err(StorageError::Invalid(
                        "duplicate clip alias wrapper identity",
                    ));
                }
            }
        }
        let mut rows = BTreeMap::new();
        for name in [
            "audio_clip_state",
            "audio_event_state",
            "audio_event_fades",
            "grouped_audio",
        ] {
            let mut by_clip = BTreeMap::new();
            for (index, row) in array(value, name)?.iter().enumerate() {
                let id = row
                    .get("clip")
                    .and_then(Value::as_u64)
                    .ok_or(StorageError::Invalid(
                        "clip alias state identity is malformed",
                    ))?;
                if by_clip.insert(id, index).is_some() {
                    return Err(StorageError::Invalid("duplicate clip alias state row"));
                }
            }
            rows.insert(name, by_clip);
        }
        Ok(Self { clips, rows })
    }
    fn location(&self, id: u64) -> Result<Location, StorageError> {
        self.clips
            .get(&id)
            .copied()
            .ok_or(StorageError::Invalid("clip alias member has no clip"))
    }
    fn row(&self, inventory: &'static str, id: u64) -> Result<usize, StorageError> {
        self.rows
            .get(inventory)
            .and_then(|rows| rows.get(&id))
            .copied()
            .ok_or(StorageError::Invalid("clip alias state row is missing"))
    }
}
fn wrapper_mut(value: &mut Value, location: Location) -> Result<&mut Value, StorageError> {
    match location {
        Location::Audio(t, c) => value
            .get_mut("tracks")
            .and_then(|v| v.get_mut(t))
            .and_then(|v| v.get_mut("audio_clips"))
            .and_then(|v| v.get_mut(c)),
        Location::Midi(t, c) => value
            .get_mut("tracks")
            .and_then(|v| v.get_mut(t))
            .and_then(|v| v.get_mut("midi_clips"))
            .and_then(|v| v.get_mut(c)),
        Location::ConsoleAudio(c) => value
            .get_mut("session")
            .and_then(|v| v.get_mut("cells"))
            .and_then(|v| v.get_mut(c))
            .and_then(|v| v.get_mut("content"))
            .and_then(|v| v.get_mut("clip")),
        Location::ConsoleMidi(c) => value
            .get_mut("session")
            .and_then(|v| v.get_mut("cells"))
            .and_then(|v| v.get_mut(c))
            .and_then(|v| v.get_mut("content"))
            .and_then(|v| v.get_mut("clip"))
            .and_then(|v| v.get_mut("source")),
    }
    .ok_or(StorageError::Invalid("clip alias wrapper is missing"))
}
fn object_mut(value: &mut Value) -> Result<&mut Map<String, Value>, StorageError> {
    value
        .as_object_mut()
        .ok_or(StorageError::Invalid("clip alias object is malformed"))
}
fn row<'a>(
    value: &'a Value,
    index: &CodecIndex,
    inventory: &'static str,
    id: u64,
) -> Result<&'a Value, StorageError> {
    value
        .get(inventory)
        .and_then(|rows| rows.get(index.row(inventory, id).ok()?))
        .ok_or(StorageError::Invalid("clip alias state row is missing"))
}
fn row_mut<'a>(
    value: &'a mut Value,
    index: &CodecIndex,
    inventory: &'static str,
    id: u64,
) -> Result<&'a mut Value, StorageError> {
    let position = index.row(inventory, id)?;
    value
        .get_mut(inventory)
        .and_then(|rows| rows.get_mut(position))
        .ok_or(StorageError::Invalid("clip alias state row is missing"))
}
fn domain<'a>(value: &'a Value, index: &CodecIndex, id: u64) -> Result<&'a Value, StorageError> {
    row(value, index, "audio_clip_state", id)?
        .get("audio_edit")
        .and_then(|v| v.get("domain"))
        .ok_or(StorageError::Invalid("alias audio domain missing"))
}
fn domain_mut<'a>(
    value: &'a mut Value,
    index: &CodecIndex,
    id: u64,
) -> Result<&'a mut Value, StorageError> {
    row_mut(value, index, "audio_clip_state", id)?
        .get_mut("audio_edit")
        .and_then(|v| v.get_mut("domain"))
        .ok_or(StorageError::Invalid("alias audio domain missing"))
}
fn validate_shared_domain(domain: &Value) -> Result<(), StorageError> {
    let object = domain
        .as_object()
        .ok_or(StorageError::Invalid("alias audio content is malformed"))?;
    if object.len() != SHARED_DOMAIN_KEYS.len()
        || SHARED_DOMAIN_KEYS
            .iter()
            .any(|key| !object.contains_key(*key))
    {
        return Err(StorageError::Invalid("alias audio content fields differ"));
    }
    Ok(())
}
fn extract_audio(value: &Value, index: &CodecIndex, id: u64) -> Result<AudioPayload, StorageError> {
    let source = domain(value, index, id)?
        .as_object()
        .ok_or(StorageError::Invalid("alias audio domain is malformed"))?;
    let mut shared = Map::new();
    for key in SHARED_DOMAIN_KEYS {
        shared.insert(
            key.into(),
            source
                .get(key)
                .cloned()
                .ok_or(StorageError::Invalid("alias audio content field missing"))?,
        );
    }
    Ok(AudioPayload {
        domain: shared.into(),
        next_id: row(value, index, "audio_event_state", id)?
            .get("next_id")
            .cloned()
            .ok_or(StorageError::Invalid("alias event allocator missing"))?,
        events: row(value, index, "audio_event_state", id)?
            .get("events")
            .cloned()
            .ok_or(StorageError::Invalid("alias events missing"))?,
        event_fades: row(value, index, "audio_event_fades", id)?
            .get("events")
            .cloned()
            .ok_or(StorageError::Invalid("alias event fades missing"))?,
        group: row(value, index, "grouped_audio", id)?
            .get("group")
            .cloned()
            .ok_or(StorageError::Invalid("alias event roots missing"))?,
    })
}
fn marker(
    object: &mut Value,
    key: &str,
    replacement: Value,
    restoring: bool,
) -> Result<(), StorageError> {
    let map = object_mut(object)?;
    let field = map
        .get_mut(key)
        .ok_or(StorageError::Invalid("clip alias shared field missing"))?;
    if restoring && !field.is_null() {
        return Err(StorageError::Invalid(
            "clip alias member contains inline shared content",
        ));
    }
    *field = replacement;
    Ok(())
}
fn mark_member(
    value: &mut Value,
    index: &CodecIndex,
    id: u64,
    content: &Content,
) -> Result<(), StorageError> {
    let location = index.location(id)?;
    match (content, location) {
        (Content::Midi { .. }, Location::Midi(..) | Location::ConsoleMidi(..)) => {
            let wrapper = wrapper_mut(value, location)?;
            marker(wrapper, "source_length_raw", Value::Null, false)?;
            marker(wrapper, "notes", Value::Null, false)?;
        }
        (Content::Audio { .. }, Location::Audio(..)) => {
            marker(wrapper_mut(value, location)?, "asset", Value::Null, false)?;
            for key in SHARED_DOMAIN_KEYS {
                marker(domain_mut(value, index, id)?, key, Value::Null, false)?;
            }
            marker(
                row_mut(value, index, "audio_event_state", id)?,
                "next_id",
                Value::Null,
                false,
            )?;
            marker(
                row_mut(value, index, "audio_event_state", id)?,
                "events",
                Value::Null,
                false,
            )?;
            marker(
                row_mut(value, index, "audio_event_fades", id)?,
                "events",
                Value::Null,
                false,
            )?;
            marker(
                row_mut(value, index, "grouped_audio", id)?,
                "group",
                Value::Null,
                false,
            )?;
        }
        (Content::ConsoleAudio { .. }, Location::ConsoleAudio(..)) => {
            marker(wrapper_mut(value, location)?, "asset", Value::Null, false)?;
        }
        _ => {
            return Err(StorageError::Invalid(
                "clip alias source kind differs from member",
            ));
        }
    }
    Ok(())
}
fn restore_member(
    value: &mut Value,
    index: &CodecIndex,
    id: u64,
    content: &Content,
    representative: bool,
) -> Result<(), StorageError> {
    let location = index.location(id)?;
    match (content, location) {
        (
            Content::Midi {
                source_length_raw, ..
            },
            Location::Midi(..) | Location::ConsoleMidi(..),
        ) => {
            let wrapper = wrapper_mut(value, location)?;
            marker(
                wrapper,
                "source_length_raw",
                (*source_length_raw).into(),
                true,
            )?;
            marker(wrapper, "notes", Value::Array(Vec::new()), true)?;
        }
        (Content::Audio { asset, payload }, Location::Audio(..)) => {
            marker(
                wrapper_mut(value, location)?,
                "asset",
                (*asset).into(),
                true,
            )?;
            for key in SHARED_DOMAIN_KEYS {
                let source = payload
                    .domain
                    .get(key)
                    .cloned()
                    .ok_or(StorageError::Invalid("alias audio content field missing"))?;
                let replacement = if representative {
                    source
                } else {
                    match key {
                        "warp_enabled" => false.into(),
                        "warp" => Value::Null,
                        "source_gain" => Value::Array(Vec::new()),
                        _ => source,
                    }
                };
                marker(domain_mut(value, index, id)?, key, replacement, true)?;
            }
            marker(
                row_mut(value, index, "audio_event_state", id)?,
                "next_id",
                if representative {
                    payload.next_id.clone()
                } else {
                    Value::Null
                },
                true,
            )?;
            marker(
                row_mut(value, index, "audio_event_state", id)?,
                "events",
                if representative {
                    payload.events.clone()
                } else {
                    Value::Array(Vec::new())
                },
                true,
            )?;
            marker(
                row_mut(value, index, "audio_event_fades", id)?,
                "events",
                if representative {
                    payload.event_fades.clone()
                } else {
                    Value::Array(Vec::new())
                },
                true,
            )?;
            marker(
                row_mut(value, index, "grouped_audio", id)?,
                "group",
                if representative {
                    payload.group.clone()
                } else {
                    Value::Null
                },
                true,
            )?;
        }
        (Content::ConsoleAudio { asset }, Location::ConsoleAudio(..)) => {
            marker(
                wrapper_mut(value, location)?,
                "asset",
                (*asset).into(),
                true,
            )?;
        }
        _ => {
            return Err(StorageError::Invalid(
                "clip alias source kind differs from member",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
