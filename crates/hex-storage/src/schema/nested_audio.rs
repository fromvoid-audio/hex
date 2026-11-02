//! Schema58 adds a bounded retained-audio tree to the strict schema57 envelope.
//! Each immediate source has a mandatory row. Nested sources retain isolated
//! schema57 leaf documents, captured clocks and explicit event activation.
use crate::{Project, StorageError};
use hex_project::{AudioEvent, AudioEvents, ClipSnapshot, RetainedAudioRoot, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

fn invalid(error: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(error.0)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    active: bool,
    source: u64,
    id: u64,
    start: u64,
    end: u64,
    source_start: u64,
    #[serde(deserialize_with = "super::event_fades::required_fade")]
    fade: Option<super::event_fades::Fade>,
}
impl From<&AudioEvent> for Event {
    fn from(event: &AudioEvent) -> Self {
        Self {
            active: event.active,
            source: event.source,
            id: event.id,
            start: event.start,
            end: event.end,
            source_start: event.source_start,
            fade: event.fade.map(Into::into),
        }
    }
}
impl Event {
    fn decode(self) -> Result<AudioEvent, StorageError> {
        Ok(AudioEvent {
            active: self.active,
            gain: hex_project::ClipGain::UNITY,
            source: self.source,
            id: self.id,
            start: self.start,
            end: self.end,
            source_start: self.source_start,
            fade: self.fade.map(|fade| fade.checked()).transpose()?,
        })
    }
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
struct Root {
    id: u64,
    document: Value,
    #[serde(deserialize_with = "required_state")]
    state: Option<Box<State>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    source: u64,
    #[serde(deserialize_with = "required_state")]
    state: Option<Box<State>>,
}
fn required_state<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Box<State>>, D::Error> {
    Option::deserialize(d)
}
fn bounded<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    items::<D, T, { hex_project::MAX_AUDIO_EVENTS }>(d)
}
fn bounded_rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    items::<D, Row, { hex_project::MAX_CLIPS }>(d)
}
fn items<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>, const MAX: usize>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    struct Visitor<T, const MAX: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const MAX: usize> serde::de::Visitor<'de> for Visitor<T, MAX> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded retained audio inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == MAX {
                    return Err(serde::de::Error::custom(
                        "retained audio inventory limit reached",
                    ));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Visitor::<T, MAX>(std::marker::PhantomData))
}
#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "bounded_rows")] Vec<Row>);

// Bound the complete tree before deserializing its owned vectors. A per-row
// bound alone permits a much larger product of sibling source inventories.
fn preflight(value: &Value) -> Result<(), StorageError> {
    let rows = value
        .as_array()
        .ok_or(StorageError::Invalid("invalid nested audio inventory"))?;
    if rows.len() > hex_project::MAX_CLIPS {
        return Err(StorageError::Invalid(
            "project retained audio root limit reached",
        ));
    }
    let mut count = rows.len();
    let mut pending: Vec<_> = rows.iter().map(|row| (&row["state"], 1usize)).collect();
    while let Some((state, depth)) = pending.pop() {
        if state.is_null() {
            continue;
        }
        if depth >= hex_project::MAX_RETAINED_AUDIO_DEPTH {
            return Err(StorageError::Invalid(
                "retained audio nesting limit reached",
            ));
        }
        let events = state
            .get("events")
            .and_then(Value::as_array)
            .ok_or(StorageError::Invalid("invalid nested audio events"))?;
        let sources = state
            .get("sources")
            .and_then(Value::as_array)
            .ok_or(StorageError::Invalid("invalid nested audio sources"))?;
        if events.len() > hex_project::MAX_AUDIO_EVENTS
            || sources.len() > hex_project::MAX_AUDIO_EVENTS
        {
            return Err(StorageError::Invalid("nested audio event limit reached"));
        }
        count = count
            .checked_add(sources.len())
            .filter(|count| *count <= hex_project::MAX_CLIPS)
            .ok_or(StorageError::Invalid(
                "project retained audio root limit reached",
            ))?;
        pending.extend(sources.iter().map(|source| (&source["state"], depth + 1)));
    }
    Ok(())
}

fn strip_nested(
    project: &Project,
    root: &RetainedAudioRoot,
    depth: usize,
    count: &mut usize,
) -> Result<(RetainedAudioRoot, Option<Box<State>>), StorageError> {
    if depth > hex_project::MAX_RETAINED_AUDIO_DEPTH {
        return Err(StorageError::Invalid(
            "retained audio nesting limit reached",
        ));
    }
    *count = count
        .checked_add(1)
        .filter(|count| *count <= hex_project::MAX_CLIPS)
        .ok_or(StorageError::Invalid(
            "project retained audio root limit reached",
        ))?;
    let mut snapshot = root.clip.retained_snapshot();
    let sequence = snapshot
        .edit
        .as_mut()
        .and_then(|edit| Arc::make_mut(&mut edit.domain).events.as_mut())
        .filter(|events| !events.sources.is_empty())
        .map(|events| Arc::clone(events));
    let Some(sequence) = sequence else {
        return Ok((root.clone(), None));
    };
    let mut sources = Vec::new();
    sources
        .try_reserve_exact(sequence.sources.len())
        .map_err(|_| StorageError::Invalid("retained audio allocation failed"))?;
    for child in &sequence.sources {
        let (neutral, state) = strip_nested(project, child, depth + 1, count)?;
        let isolated = super::grouped_audio::isolated_root(
            project,
            neutral.clip.retained_snapshot(),
            &neutral.tempo,
        )?;
        sources.push(Root {
            id: child.id,
            document: super::event_activation::encode(&isolated)?,
            state,
        });
    }
    if let Some(edit) = snapshot.edit.as_mut() {
        Arc::make_mut(&mut edit.domain).events = None;
    }
    let neutral = project
        .restore_retained_audio_root(root.id, snapshot, Arc::clone(&root.tempo))
        .map_err(invalid)?;
    Ok((
        neutral,
        Some(Box::new(State {
            next_id: sequence.next_id,
            events: sequence.events.iter().map(Event::from).collect(),
            sources,
        })),
    ))
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    let mut count = 0;
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            if let Some(events) = clip
                .edit
                .as_mut()
                .and_then(|edit| Arc::make_mut(&mut edit.domain).events.as_mut())
            {
                for root in &mut Arc::make_mut(events).sources {
                    let (neutral, state) = strip_nested(project, root, 1, &mut count)?;
                    rows.push(Row {
                        clip: clip.id,
                        source: root.id,
                        state,
                    });
                    *root = neutral;
                }
            }
        }
    }
    super::audio_alias_projection::synchronize(&mut snapshot)?;
    let neutral = Project::from_snapshot(snapshot).map_err(invalid)?;
    let mut value = super::event_activation::encode(&neutral)?;
    value["version"] = 58.into();
    value["nested_audio"] = serde_json::to_value(rows)?;
    Ok(value)
}

fn attach(
    project: &Project,
    root: RetainedAudioRoot,
    state: Option<Box<State>>,
    depth: usize,
    count: &mut usize,
) -> Result<RetainedAudioRoot, StorageError> {
    if depth > hex_project::MAX_RETAINED_AUDIO_DEPTH {
        return Err(StorageError::Invalid(
            "retained audio nesting limit reached",
        ));
    }
    *count = count
        .checked_add(1)
        .filter(|count| *count <= hex_project::MAX_CLIPS)
        .ok_or(StorageError::Invalid(
            "project retained audio root limit reached",
        ))?;
    let Some(state) = state else {
        return Ok(root);
    };
    if state.sources.is_empty() {
        return Err(StorageError::Invalid("empty nested audio source inventory"));
    }
    let mut sources = Vec::new();
    sources
        .try_reserve_exact(state.sources.len())
        .map_err(|_| StorageError::Invalid("retained audio allocation failed"))?;
    for child in state.sources {
        if child.id == 0 || child.document.get("version").and_then(Value::as_u64) != Some(57) {
            return Err(StorageError::Invalid(
                "invalid nested audio source document",
            ));
        }
        let isolated = super::event_activation::decode_value(child.document.clone())?;
        let clips: Vec<_> = isolated
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .collect();
        if isolated.tracks().len() != 1 || clips.len() != 1 {
            return Err(StorageError::Invalid(
                "invalid isolated retained audio source",
            ));
        }
        let snapshot = clips[0].retained_snapshot();
        let canonical =
            super::grouped_audio::isolated_root(project, snapshot.clone(), isolated.tempo_map())?;
        if super::event_activation::encode(&canonical)? != child.document {
            return Err(StorageError::Invalid(
                "nested audio source differs from project media",
            ));
        }
        let child_root = project
            .restore_retained_audio_root(child.id, snapshot, Arc::new(isolated.tempo_map().clone()))
            .map_err(invalid)?;
        sources.push(attach(project, child_root, child.state, depth + 1, count)?);
    }
    let mut snapshot: ClipSnapshot = root.clip.retained_snapshot();
    let edit = snapshot.edit.as_mut().ok_or(StorageError::Invalid(
        "nested audio processing root missing",
    ))?;
    if edit.domain.events.is_some() {
        return Err(StorageError::Invalid(
            "competing nested audio event inventory",
        ));
    }
    Arc::make_mut(&mut edit.domain).events = Some(Arc::new(AudioEvents {
        next_id: state.next_id,
        events: state
            .events
            .into_iter()
            .map(Event::decode)
            .collect::<Result<_, _>>()?,
        sources,
    }));
    project
        .restore_retained_audio_root(root.id, snapshot, Arc::clone(&root.tempo))
        .map_err(invalid)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: Value) -> Result<Project, StorageError> {
    if value.get("version").and_then(Value::as_u64) != Some(58) {
        return Err(StorageError::Invalid("invalid nested audio schema"));
    }
    let rows = value
        .as_object_mut()
        .and_then(|value| value.remove("nested_audio"))
        .ok_or(StorageError::Invalid("nested audio inventory missing"))?;
    preflight(&rows)?;
    let rows: Rows = serde_json::from_value(rows)?;
    let mut inventory = BTreeMap::new();
    for row in rows.0 {
        if row.clip == 0
            || row.source == 0
            || inventory
                .insert((row.clip, row.source), row.state)
                .is_some()
        {
            return Err(StorageError::Invalid(
                "invalid or duplicate nested audio row",
            ));
        }
    }
    value["version"] = 57.into();
    let project = super::event_activation::decode_value(value)?;
    let mut snapshot = project.snapshot();
    let mut count = 0;
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            if let Some(events) = clip
                .edit
                .as_mut()
                .and_then(|edit| Arc::make_mut(&mut edit.domain).events.as_mut())
            {
                for root in &mut Arc::make_mut(events).sources {
                    let state = inventory
                        .remove(&(clip.id, root.id))
                        .ok_or(StorageError::Invalid("nested audio row missing"))?;
                    *root = attach(&project, root.clone(), state, 1, &mut count)?;
                }
            }
        }
    }
    if !inventory.is_empty() {
        return Err(StorageError::Invalid("nested audio row owner missing"));
    }
    super::audio_alias_projection::synchronize(&mut snapshot)?;
    Project::from_snapshot(snapshot).map_err(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preflight_refuses_excess_depth_and_aggregate_roots() {
        let mut state = Value::Null;
        for _ in 0..hex_project::MAX_RETAINED_AUDIO_DEPTH {
            state = json!({"events": [], "sources": [{"state": state}]});
        }
        assert!(preflight(&json!([{"state": state}])).is_err());
        let rows = vec![json!({"state": null}); hex_project::MAX_CLIPS + 1];
        assert!(preflight(&Value::Array(rows)).is_err());
    }
}
