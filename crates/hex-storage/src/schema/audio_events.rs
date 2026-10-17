//! Schema26 adds a strict event-state overlay to the complete schema25 envelope.
use crate::StorageError;
use hex_project::{AudioEvent, AudioEvents, Project, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    clip: u64,
    #[serde(deserialize_with = "required_next_id")]
    next_id: Option<u64>,
    #[serde(deserialize_with = "bounded_events")]
    events: Vec<Event>,
}
fn required_next_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Option::deserialize(d)
}
fn bounded_events<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Event>, D::Error> {
    struct Events;
    impl<'de> serde::de::Visitor<'de> for Events {
        type Value = Vec<Event>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded audio event sequence")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut result = Vec::new();
            while let Some(event) = seq.next_element()? {
                if result.len() == hex_project::MAX_AUDIO_EVENTS {
                    return Err(serde::de::Error::custom("audio event limit reached"));
                }
                result.push(event);
            }
            Ok(result)
        }
    }
    d.deserialize_seq(Events)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    id: u64,
    start: u64,
    end: u64,
    source_start: u64,
}

pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let base = super::musical_audio::DocumentV25::from_project(project)?;
    let mut document = serde_json::to_value(base)?;
    let states: Vec<_> = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .map(|clip| {
            let events = clip
                .audio_edit()
                .and_then(|edit| edit.domain.events.as_ref());
            State {
                clip: clip.id().value(),
                next_id: events.map(|e| e.next_id),
                events: events.map_or_else(Vec::new, |e| {
                    e.events
                        .iter()
                        .map(|e| Event {
                            id: e.id,
                            start: e.start,
                            end: e.end,
                            source_start: e.source_start,
                        })
                        .collect()
                }),
            }
        })
        .collect();
    let object = document
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid audio event envelope"))?;
    object.insert("version".into(), 26.into());
    object.insert("audio_event_state".into(), serde_json::to_value(states)?);
    Ok(document)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let _: UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid audio event envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(26) {
        return Err(StorageError::Invalid("invalid audio event version"));
    }
    let states: Vec<State> = serde_json::from_value(
        object
            .remove("audio_event_state")
            .ok_or(StorageError::Invalid("audio event state is missing"))?,
    )?;
    if states.len() > hex_project::MAX_CLIPS
        || states
            .iter()
            .any(|s| s.events.len() > hex_project::MAX_AUDIO_EVENTS)
    {
        return Err(StorageError::Invalid("audio event state limit reached"));
    }
    object.insert("version".into(), 25.into());
    let base = super::musical_audio::DocumentV25::decode(&serde_json::to_vec(&value)?)?;
    let mut snapshot = base.snapshot();
    let mut by_clip = std::collections::BTreeMap::new();
    for state in states {
        let id = state.clip;
        if by_clip.insert(id, state).is_some() {
            return Err(StorageError::Invalid("duplicate audio event state"));
        }
    }
    for track in &mut snapshot.tracks {
        let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut track.content else {
            continue;
        };
        for clip in audio_clips {
            let state = by_clip.remove(&clip.id).ok_or(StorageError::Invalid(
                "audio event state is missing for a clip",
            ))?;
            if let Some(next_id) = state.next_id {
                let edit = clip.edit.as_mut().ok_or(StorageError::Invalid(
                    "audio events require a retained processing root",
                ))?;
                let events = AudioEvents {
                    next_id,
                    events: state
                        .events
                        .into_iter()
                        .map(|e| AudioEvent {
                            id: e.id,
                            start: e.start,
                            end: e.end,
                            source_start: e.source_start,
                        })
                        .collect(),
                };
                events
                    .validate(edit.domain.clock.length())
                    .map_err(|e| StorageError::Invalid(e.0))?;
                Arc::make_mut(&mut edit.domain).events = Some(Arc::new(events));
            } else if !state.events.is_empty() {
                return Err(StorageError::Invalid(
                    "implicit audio event state cannot contain events",
                ));
            }
        }
    }
    if !by_clip.is_empty() {
        return Err(StorageError::Invalid("audio event owner does not exist"));
    }
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

// Value's normal map decoder discards duplicate fields. Validate the original
// token stream before adapting the versioned envelope so strict historical
// parsing cannot accidentally accept an ambiguous document.
struct UniqueKeys;
impl<'de> Deserialize<'de> for UniqueKeys {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueKeys;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("json with unique object fields")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<UniqueKeys, A::Error> {
                let mut keys = std::collections::BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(serde::de::Error::custom("duplicate json field"));
                    }
                    map.next_value::<UniqueKeys>()?;
                }
                Ok(UniqueKeys)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<UniqueKeys, A::Error> {
                while seq.next_element::<UniqueKeys>()?.is_some() {}
                Ok(UniqueKeys)
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueKeys, E> {
                Ok(UniqueKeys)
            }
        }
        d.deserialize_any(Visitor)
    }
}
