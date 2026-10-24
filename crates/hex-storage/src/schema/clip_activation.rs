//! Schema40 adds a complete clip-activation inventory and elapsed MIDI playback
//! windows over frozen schema39. Older readers default all clips active and all
//! windows to zero; explicit older writers refuse state they cannot represent.
//! Decoding is in-memory only; existing atomic save custody preserves originals.
use super::{Project, StorageError};
use hex_project::{SessionCellContent, SessionCellContentSnapshot, Snapshot, TrackContentSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Activation {
    clip: u64,
    active: bool,
    /// Always zero for audio, elapsed quarter-note beat bits for MIDI.
    playback_offset_raw: u64,
}
#[derive(Deserialize)]
struct Inventory(#[serde(deserialize_with = "bounded")] Vec<Activation>);
fn bounded<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Activation>, D::Error> {
    struct Rows;
    impl<'de> serde::de::Visitor<'de> for Rows {
        type Value = Vec<Activation>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("bounded clip activation inventory") }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == hex_project::MAX_CLIPS { return Err(serde::de::Error::custom("clip activation inventory limit reached")); }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows)
}
fn visit(snapshot: &mut Snapshot, mut action: impl FnMut(u64, &mut bool, Option<&mut u64>) -> Result<(), StorageError>) -> Result<(), StorageError> {
    for track in &mut snapshot.tracks {
        match &mut track.content {
            TrackContentSnapshot::Hybrid {audio_clips, midi_clips} => {
                for clip in audio_clips { action(clip.id, &mut clip.active, None)?; }
                for clip in midi_clips { action(clip.id, &mut clip.active, Some(&mut clip.playback_offset_raw))?; }
            }
            TrackContentSnapshot::Audio(clips) => for clip in clips { action(clip.id, &mut clip.active, None)?; },
            TrackContentSnapshot::Instrument(clips) => for clip in clips { action(clip.id, &mut clip.active, Some(&mut clip.playback_offset_raw))?; },
        }
    }
    for cell in &mut snapshot.session.cells {
        match &mut cell.content {
            SessionCellContentSnapshot::Audio(clip) => action(clip.id, &mut clip.active, None)?,
            SessionCellContentSnapshot::Midi(clip) => action(clip.id, &mut clip.active, Some(&mut clip.playback_offset_raw))?,
            SessionCellContentSnapshot::Stop => (),
        }
    }
    Ok(())
}
pub(crate) fn require_legacy_compatible(project: &Project) -> Result<(), StorageError> {
    let incompatible = project.tracks().iter().any(|track| track.clips().iter().any(|clip| !clip.active())
        || track.midi_clips().iter().any(|clip| !clip.active() || clip.playback_offset() != hex_project::BeatTime::ZERO))
        || project.session_cells().iter().any(|cell| match cell.content() {
            SessionCellContent::Audio(clip) => !clip.active(),
            SessionCellContent::Midi(clip) => !clip.active() || clip.playback_offset() != hex_project::BeatTime::ZERO,
            SessionCellContent::Stop => false,
        });
    if incompatible { Err(StorageError::Invalid("older schema cannot save clip activation or midi playback windows")) } else { Ok(()) }
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    visit(&mut snapshot, |clip, active, offset| {
        rows.push(Activation {clip, active: *active, playback_offset_raw: offset.as_deref().copied().unwrap_or(0)});
        *active = true;
        if let Some(offset) = offset { *offset = 0; }
        Ok(())
    })?;
    let legacy = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let base = crate::encode_document(&super::midi_one_shot::encode(&legacy)?)?;
    let mut value: serde_json::Value = serde_json::from_slice(&base)?;
    let object = value.as_object_mut().ok_or(StorageError::Invalid("invalid clip activation envelope"))?;
    object.insert("version".into(), 40.into());
    object.insert("clip_activation".into(), serde_json::to_value(rows)?);
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    let object = value.as_object_mut().ok_or(StorageError::Invalid("invalid clip activation envelope"))?;
    if object.get("version").and_then(|version| version.as_u64()) != Some(40) { return Err(StorageError::Invalid("invalid clip activation version")); }
    let inventory: Inventory = serde_json::from_value(object.remove("clip_activation").ok_or(StorageError::Invalid("clip activation inventory is missing"))?)?;
    object.insert("version".into(), 39.into());
    let project = super::midi_one_shot::decode(&serde_json::to_vec(&value)?)?;
    let mut rows = BTreeMap::new();
    for row in inventory.0 {
        if rows.insert(row.clip, row).is_some() { return Err(StorageError::Invalid("duplicate clip activation identity")); }
    }
    let mut snapshot = project.snapshot();
    visit(&mut snapshot, |clip, active, offset| {
        let row = rows.remove(&clip).ok_or(StorageError::Invalid("clip activation row is missing"))?;
        *active = row.active;
        if let Some(offset) = offset { *offset = row.playback_offset_raw; }
        else if row.playback_offset_raw != 0 { return Err(StorageError::Invalid("audio clip cannot have a midi playback window")); }
        Ok(())
    })?;
    if !rows.is_empty() { return Err(StorageError::Invalid("clip activation references a missing clip")); }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}
