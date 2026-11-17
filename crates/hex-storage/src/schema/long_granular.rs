//! Schema120 stores long active Tones/Texture roots. Schema121 stores long
//! neutral inactive Warp roots, including their markers and algorithm.
//! The base is an independently valid older document with these clips' edits
//! removed; rows restore the complete existing audio-edit DTO, never PCM.
//! Supported older documents migrate in memory through their existing codecs
//! without changes. The actual format comparison in FileToken retains Save As,
//! cancel and explicit overwrite custody. No original file/media is rewritten.
use super::s_curve_fades::Curve;
use crate::{
    LONG_GRANULAR_SCHEMA_VERSION, LONG_INACTIVE_WARP_SCHEMA_VERSION, Project, StorageError,
    audio_edit::AudioEditDto,
};
use hex_project::{
    AudioClipEdit, MAX_AUDIO_DOMAIN_FRAMES, MAX_CLIPS, TempoMap, TrackContentSnapshot,
};
use serde::{Deserialize, Serialize, de};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(crate) enum LongWarpSchema {
    Granular,
    Inactive,
}

impl LongWarpSchema {
    fn version(self) -> u32 {
        match self {
            Self::Granular => LONG_GRANULAR_SCHEMA_VERSION,
            Self::Inactive => LONG_INACTIVE_WARP_SCHEMA_VERSION,
        }
    }

    fn matches(self, edit: &AudioClipEdit, tempo: &TempoMap) -> bool {
        match self {
            Self::Granular => {
                edit.is_streamed_granular_root()
                    && edit
                        .projected_domain(tempo)
                        .is_ok_and(|(_, length, _, _)| length > MAX_AUDIO_DOMAIN_FRAMES)
            }
            Self::Inactive => {
                !edit.domain.warp_enabled
                    && edit.domain.warp.is_some()
                    && edit
                        .projected_domain(tempo)
                        .is_ok_and(|(_, length, _, _)| edit.is_long_inactive_warp_root(length))
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<'a> {
    format: &'a str,
    version: u32,
    #[serde(borrow)]
    base: &'a RawValue,
    #[serde(deserialize_with = "bounded_rows")]
    clips: Vec<Row>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    clip: u64,
    #[serde(deserialize_with = "required_edit")]
    edit: Option<AudioEditDto>,
    #[serde(deserialize_with = "required_alias")]
    alias_source: Option<u64>,
    #[serde(deserialize_with = "required_curve")]
    curve: Option<Curve>,
}
fn required_curve<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Curve>, D::Error> {
    Option::deserialize(d)
}
fn required_edit<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<AudioEditDto>, D::Error> {
    Option::deserialize(d)
}
fn required_alias<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Option::deserialize(d)
}
fn bounded_rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    struct Rows;
    impl<'de> de::Visitor<'de> for Rows {
        type Value = Vec<Row>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded long warp clip edits")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            while let Some(row) = seq.next_element()? {
                if rows.len() == MAX_CLIPS {
                    return Err(de::Error::custom("long warp clip limit reached"));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Rows)
}
fn is_long(project: &Project, clip: &hex_project::Clip, schema: LongWarpSchema) -> bool {
    clip.audio_edit()
        .is_some_and(|edit| schema.matches(edit, project.tempo_map()))
}
pub(crate) fn contains(project: &Project, schema: LongWarpSchema) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(|clip| is_long(project, clip, schema))
}
pub(crate) fn encode(project: &Project, schema: LongWarpSchema) -> Result<Vec<u8>, StorageError> {
    let mut snapshot = project.snapshot();
    let mut ids: BTreeSet<_> = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter(|clip| is_long(project, clip, schema))
        .map(|clip| clip.id().value())
        .collect();
    let aliases: BTreeSet<_> = snapshot
        .aliases
        .members
        .iter()
        .filter(|member| ids.contains(&member.clip))
        .map(|member| member.source)
        .collect();
    let members: BTreeMap<_, _> = snapshot
        .aliases
        .members
        .iter()
        .filter(|member| aliases.contains(&member.source))
        .map(|member| (member.clip, member.source))
        .collect();
    let long_ids = ids.clone();
    ids.extend(members.keys());
    if ids.is_empty() || ids.len() > MAX_CLIPS {
        return Err(StorageError::Invalid("invalid long warp inventory"));
    }
    snapshot
        .aliases
        .sources
        .retain(|source| !aliases.contains(&source.id));
    snapshot
        .aliases
        .members
        .retain(|member| !aliases.contains(&member.source));
    let mut rows = Vec::new();
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            if ids.contains(&clip.id) {
                let curve = if long_ids.contains(&clip.id) {
                    clip.edit
                        .as_ref()
                        .map(|edit| Curve::capture(edit.domain.fade))
                } else {
                    None
                };
                let edit = if long_ids.contains(&clip.id) {
                    Some(
                        clip.edit
                            .take()
                            .ok_or(StorageError::Invalid("long warp edit missing"))?
                            .into(),
                    )
                } else {
                    None
                };
                rows.push(Row {
                    clip: clip.id,
                    edit,
                    alias_source: members.get(&clip.id).copied(),
                    curve,
                });
            }
        }
    }
    rows.sort_unstable_by_key(|row| row.clip);
    let prior = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let base = crate::encode(&prior)?;
    crate::encode_document(&Document {
        format: "hex-project",
        version: schema.version(),
        base: serde_json::from_slice(&base)?,
        clips: rows,
    })
}
pub(crate) fn decode(bytes: &[u8], schema: LongWarpSchema) -> Result<Project, StorageError> {
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
    if document.version != schema.version()
        || header.version >= schema.version()
        || document.clips.is_empty()
        || document
            .clips
            .windows(2)
            .any(|pair| pair[0].clip >= pair[1].clip)
    {
        return Err(StorageError::Invalid("invalid long warp envelope"));
    }
    let mut wanted: BTreeMap<_, _> = document
        .clips
        .into_iter()
        .map(|row| (row.clip, (row.edit, row.alias_source, row.curve)))
        .collect();
    let project = crate::decode(base)?;
    let tempo = project.tempo_map().clone();
    let mut snapshot = project.snapshot();
    let mut restored_aliases = BTreeMap::new();
    let mut long_aliases = BTreeSet::new();
    for track in &mut snapshot.tracks {
        let clips = match &mut track.content {
            TrackContentSnapshot::Audio(clips) => clips,
            TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
            TrackContentSnapshot::Instrument(_) => continue,
        };
        for clip in clips {
            if let Some((edit, alias, curve)) = wanted.remove(&clip.id) {
                let edit = if let Some(edit) = edit {
                    if clip.edit.is_some() {
                        return Err(StorageError::Invalid("long warp base clip is not plain"));
                    }
                    let mut edit = hex_project::AudioClipEdit::try_from(edit)?;
                    let curve =
                        curve.ok_or(StorageError::Invalid("long warp fade curve missing"))?;
                    std::sync::Arc::make_mut(&mut edit.domain).fade =
                        curve.restore(edit.domain.fade)?;
                    if !schema.matches(&edit, &tempo) {
                        return Err(StorageError::Invalid(
                            "invalid long warp root for project schema",
                        ));
                    }
                    if let Some(source) = alias {
                        long_aliases.insert(source);
                    }
                    edit
                } else {
                    if alias.is_none() || curve.is_some() {
                        return Err(StorageError::Invalid("missing long warp edit"));
                    }
                    clip.edit
                        .take()
                        .ok_or(StorageError::Invalid("long warp alias base edit missing"))?
                };
                if let Some(source) = alias {
                    if source == 0 || snapshot.aliases.sources.iter().any(|old| old.id == source) {
                        return Err(StorageError::Invalid("invalid long warp alias source"));
                    }
                    let content = hex_project::AliasContentSnapshot::Audio {
                        asset: clip.asset,
                        content: std::sync::Arc::clone(&edit.domain.content),
                    };
                    if let Some(old) = restored_aliases.get(&source) {
                        if old != &content {
                            return Err(StorageError::Invalid("long warp alias content differs"));
                        }
                    } else {
                        restored_aliases.insert(source, content);
                    }
                    snapshot
                        .aliases
                        .members
                        .push(hex_project::AliasMemberSnapshot {
                            clip: clip.id,
                            source,
                        });
                }
                clip.edit = Some(edit);
            }
        }
    }
    if !wanted.is_empty() {
        return Err(StorageError::Invalid("long warp clip is missing"));
    }
    if restored_aliases
        .keys()
        .any(|source| !long_aliases.contains(source))
    {
        return Err(StorageError::Invalid("long warp alias has no long root"));
    }
    snapshot.aliases.sources.extend(
        restored_aliases
            .into_iter()
            .map(|(id, content)| hex_project::AliasSourceSnapshot { id, content }),
    );
    snapshot
        .aliases
        .sources
        .sort_unstable_by_key(|source| source.id);
    snapshot
        .aliases
        .members
        .sort_unstable_by_key(|member| member.clip);
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}
#[cfg(test)]
mod tests;
