//! Schema44 adds a mandatory ordered hierarchy inventory over strict schema43.
//! Every track has exactly one kind/parent row. Earlier schemas contain only
//! ordinary root tracks. Migration is in memory; original bytes/media are never
//! rewritten by decoding. The private flattened core below is codec composition,
//! never a playable project or a public downgrade operation.
use crate::{Project, StorageError};
use hex_project::TrackKind;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    track: u64,
    group: bool,
    #[serde(deserialize_with = "required_parent")]
    parent: Option<u64>,
}
fn required_parent<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Option::deserialize(d)
}
#[derive(Deserialize)]
struct Inventory(#[serde(deserialize_with = "bounded")] Vec<Row>);
fn bounded<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    struct Rows;
    impl<'de> serde::de::Visitor<'de> for Rows {
        type Value = Vec<Row>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded track hierarchy inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == hex_project::MAX_TRACKS {
                    return Err(serde::de::Error::custom("track hierarchy limit reached"));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows)
}
fn invalid(e: hex_project::Invalid) -> StorageError {
    StorageError::Invalid(e.0)
}
pub(crate) fn require_legacy_compatible(project: &Project) -> Result<(), StorageError> {
    if project.has_group_tracks() {
        return Err(StorageError::Invalid(
            "older schema cannot save group tracks",
        ));
    }
    Ok(())
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    if project.native_sources().iter().any(|source| {
        project
            .tracks()
            .iter()
            .all(|track| track.source_id() != Some(source.id()))
    }) {
        return Err(StorageError::Invalid(
            "schema44 core cannot represent unassigned native sources",
        ));
    }
    Project::from_legacy_snapshot(project.snapshot()).map_err(invalid)?;
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    for track in &mut snapshot.tracks {
        rows.push(Row {
            track: track.id,
            group: track.kind == TrackKind::Group,
            parent: track.parent_group,
        });
        track.kind = TrackKind::Ordinary;
        track.parent_group = None;
    }
    let core = Project::from_legacy_snapshot(snapshot).map_err(invalid)?;
    let mut value = super::grouped_audio::encode(&core)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid track hierarchy envelope"))?;
    object.insert("version".into(), 44.into());
    object.insert("track_hierarchy".into(), serde_json::to_value(rows)?);
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: serde_json::Value) -> Result<Project, StorageError> {
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid track hierarchy envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(44) {
        return Err(StorageError::Invalid("invalid track hierarchy version"));
    }
    let Inventory(rows) = serde_json::from_value(
        object
            .remove("track_hierarchy")
            .ok_or(StorageError::Invalid("track hierarchy inventory missing"))?,
    )?;
    object.insert("version".into(), 43.into());
    let mut snapshot = super::grouped_audio::decode_value(value)?.snapshot();
    if rows.len() != snapshot.tracks.len() {
        return Err(StorageError::Invalid(
            "track hierarchy inventory is incomplete",
        ));
    }
    for (track, row) in snapshot.tracks.iter_mut().zip(rows) {
        if row.track != track.id {
            return Err(StorageError::Invalid(
                "track hierarchy identities or order differ",
            ));
        }
        track.kind = if row.group {
            TrackKind::Group
        } else {
            TrackKind::Ordinary
        };
        track.parent_group = row.parent;
    }
    Project::from_legacy_snapshot(snapshot).map_err(invalid)
}
