//! Schema42 preserves event-owned source-clock fade domains over the genuine
//! schema41 directed-view envelope. Supported older decoders explicitly acquire
//! absent event fades. Decoding/migration is control-side and in-memory only;
//! existing format warnings and atomic Save As/overwrite custody remain owners.
use super::{Project, StorageError};
use hex_project::{AudioEventFade, ClipFade, ClipSnapshot, Snapshot, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, marker::PhantomData, sync::Arc};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fade {
    source_start: u64,
    span: u64,
    fade_in: u64,
    fade_out: u64,
    in_start: u64,
    out_end_offset: u64,
    in_curve: i16,
    out_curve: i16,
    in_position: u16,
    out_position: u16,
}
impl From<AudioEventFade> for Fade {
    fn from(fade: AudioEventFade) -> Self {
        let shape = fade.shape();
        Self {
            source_start: fade.source_start(),
            span: shape.span(),
            fade_in: shape.fade_in(),
            fade_out: shape.fade_out(),
            in_start: shape.in_start(),
            out_end_offset: shape.out_end_offset(),
            in_curve: shape.in_curve(),
            out_curve: shape.out_curve(),
            in_position: shape.in_position(),
            out_position: shape.out_position(),
        }
    }
}
impl Fade {
    pub(crate) fn checked(self) -> Result<AudioEventFade, StorageError> {
        let shape = ClipFade::new(self.span, 0, self.fade_in, self.fade_out)
            .and_then(|shape| {
                shape.with_shape(
                    self.in_start,
                    self.out_end_offset,
                    self.in_curve,
                    self.out_curve,
                )
            })
            .and_then(|shape| shape.with_curve_positions(self.in_position, self.out_position))
            .map_err(|e| StorageError::Invalid(e.0))?;
        AudioEventFade::new(self.source_start, shape).map_err(|e| StorageError::Invalid(e.0))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    event: u64,
    #[serde(deserialize_with = "required_fade")]
    fade: Option<Fade>,
}
pub(crate) fn required_fade<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Fade>, D::Error> {
    Option::deserialize(d)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    #[serde(deserialize_with = "bounded_events")]
    events: Vec<Event>,
}
#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "bounded_rows")] Vec<Row>);
fn bounded_rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    bounded(d, hex_project::MAX_CLIPS)
}
fn bounded_events<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Event>, D::Error> {
    bounded(d, hex_project::MAX_AUDIO_EVENTS)
}
fn bounded<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
    limit: usize,
) -> Result<Vec<T>, D::Error> {
    struct Visitor<T> {
        limit: usize,
        marker: PhantomData<T>,
    }
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded audio event fade inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == self.limit {
                    return Err(serde::de::Error::custom(
                        "audio event fade inventory limit reached",
                    ));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Visitor::<T> {
        limit,
        marker: PhantomData,
    })
}
fn visit(
    snapshot: &mut Snapshot,
    mut action: impl FnMut(&mut ClipSnapshot) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            action(clip)?;
        }
    }
    Ok(())
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    visit(&mut snapshot, |clip| {
        let mut events = Vec::new();
        if let Some(edit) = clip.edit.as_mut()
            && edit.domain.events.is_some()
        {
            let root = Arc::make_mut(&mut edit.domain);
            if let Some(sequence) = root.events.as_mut() {
                for event in &mut Arc::make_mut(sequence).events {
                    events.push(Event {
                        event: event.id,
                        fade: event.fade.take().map(Into::into),
                    });
                }
            }
        }
        rows.push(Row {
            clip: clip.id,
            events,
        });
        Ok(())
    })?;
    // The frozen predecessor sees only state it can represent. It still owns
    // its complete directed views, native devices, musical roots and histories.
    let legacy = Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
    let mut value = super::content_views::encode(&legacy)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid audio event fade envelope"))?;
    object.insert("version".into(), 42.into());
    object.insert("audio_event_fades".into(), serde_json::to_value(rows)?);
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: serde_json::Value) -> Result<Project, StorageError> {
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid audio event fade envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(42) {
        return Err(StorageError::Invalid("invalid audio event fade version"));
    }
    let inventory: Rows = serde_json::from_value(object.remove("audio_event_fades").ok_or(
        StorageError::Invalid("audio event fade inventory is missing"),
    )?)?;
    object.insert("version".into(), 41.into());
    let legacy = super::content_views::decode_value(value)?;
    let mut rows = BTreeMap::new();
    for row in inventory.0 {
        if rows.insert(row.clip, row.events).is_some() {
            return Err(StorageError::Invalid(
                "duplicate audio event fade clip identity",
            ));
        }
    }
    let mut snapshot = legacy.snapshot();
    visit(&mut snapshot, |clip| {
        let inventory = rows.remove(&clip.id).ok_or(StorageError::Invalid(
            "audio event fade clip row is missing",
        ))?;
        let mut events = BTreeMap::new();
        for row in inventory {
            if events
                .insert(row.event, row.fade.map(Fade::checked).transpose()?)
                .is_some()
            {
                return Err(StorageError::Invalid("duplicate audio event fade identity"));
            }
        }
        if let Some(edit) = clip.edit.as_mut()
            && edit.domain.events.is_some()
        {
            let root = Arc::make_mut(&mut edit.domain);
            if let Some(sequence) = root.events.as_mut() {
                for event in &mut Arc::make_mut(sequence).events {
                    event.fade = events
                        .remove(&event.id)
                        .ok_or(StorageError::Invalid("audio event fade row is missing"))?;
                }
            }
        }
        if !events.is_empty() {
            return Err(StorageError::Invalid(
                "audio event fade references a missing event",
            ));
        }
        Ok(())
    })?;
    if !rows.is_empty() {
        return Err(StorageError::Invalid(
            "audio event fades reference a missing arrangement clip",
        ));
    }
    Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

#[cfg(test)]
#[path = "event_fades_tests.rs"]
mod tests;
