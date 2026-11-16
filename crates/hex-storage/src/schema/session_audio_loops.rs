//! Schema48 adds a mandatory ordered cycle row for every Console audio cell.
//! Schema49 retains these cycles and adds explicit nullable source receivers.
//! Schemas1–47 keep their frozen codecs and migrate as one-shots, in memory.
//! The private native-source cores contain no loop state; this is not a public lossy
//! downgrade. Original documents/media are never rewritten during decoding.
use crate::{Project, StorageError};
use hex_project::{SessionAudioLoop, SessionCellContentSnapshot};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cycle {
    start_frame: u64,
    end_frame: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    #[serde(deserialize_with = "required_cycle")]
    cycle: Option<Cycle>,
}
fn required_cycle<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Cycle>, D::Error> {
    Option::deserialize(d)
}
#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "bounded_rows")] Vec<Row>);
fn bounded_rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<Row>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded console audio cycle inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<Row>, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() >= hex_project::MAX_SESSION_CELLS {
                    return Err(serde::de::Error::custom(
                        "console audio cycle limit exceeded",
                    ));
                }
                rows.try_reserve(1).map_err(serde::de::Error::custom)?;
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Visitor)
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    for cell in &mut snapshot.session.cells {
        if let SessionCellContentSnapshot::Audio(clip) = &mut cell.content {
            rows.push(Row {
                clip: clip.id,
                cycle: clip.loop_range.take().map(|range| Cycle {
                    start_frame: range.start(),
                    end_frame: range.end(),
                }),
            });
        }
    }
    let core = Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
    let mut value = super::native_sources::encode(&core)?;
    value["version"] = 49.into();
    value["session_audio_cycles"] = serde_json::to_value(rows)?;
    Ok(value)
}
pub(crate) fn decode48(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_version(bytes, 48)
}

pub(crate) fn decode49(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_version(bytes, 49)
}

/// Decode a schema49 envelope parsed once by an upper migration layer.
pub(crate) fn decode49_value(value: serde_json::Value) -> Result<Project, StorageError> {
    decode_version_value(value, 49)
}

fn decode_version(bytes: &[u8], version: u64) -> Result<Project, StorageError> {
    decode_version_value(super::audio_events::parse_unique(bytes)?, version)
}

fn decode_version_value(
    mut value: serde_json::Value,
    version: u64,
) -> Result<Project, StorageError> {
    let object = value.as_object_mut().ok_or(StorageError::Invalid(
        "invalid console audio cycle envelope",
    ))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(version) {
        return Err(StorageError::Invalid("invalid console audio cycle schema"));
    }
    let Rows(rows) = serde_json::from_value(object.remove("session_audio_cycles").ok_or(
        StorageError::Invalid("console audio cycle inventory missing"),
    )?)?;
    // Schema48's source layout is exactly the frozen schema47 receiverless
    // singleton contract. Only49 admits explicit receivers and shared bindings.
    let core = if version == 48 {
        object.insert("version".into(), 47.into());
        super::native_sources::decode47_value(value)?
    } else {
        super::native_sources::decode49_value(value)?
    };
    let mut snapshot = core.snapshot();
    let mut rows = rows.into_iter();
    for cell in &mut snapshot.session.cells {
        if let SessionCellContentSnapshot::Audio(clip) = &mut cell.content {
            let row =
                rows.next()
                    .filter(|row| row.clip == clip.id)
                    .ok_or(StorageError::Invalid(
                        "console audio cycle inventory is incomplete or out of order",
                    ))?;
            clip.loop_range = row
                .cycle
                .map(|cycle| SessionAudioLoop::new(cycle.start_frame, cycle.end_frame))
                .transpose()
                .map_err(|e| StorageError::Invalid(e.0))?;
        }
    }
    if rows.next().is_some() {
        return Err(StorageError::Invalid(
            "console audio cycle inventory has extra rows",
        ));
    }
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}
pub(crate) fn refuse_legacy(project: &Project) -> Result<(), StorageError> {
    if project.session_cells().iter().any(|cell| matches!(cell.content(), hex_project::SessionCellContent::Audio(clip) if clip.loop_range().is_some())) {
        return Err(StorageError::Invalid("older schema cannot save console audio loops"));
    }
    Ok(())
}
