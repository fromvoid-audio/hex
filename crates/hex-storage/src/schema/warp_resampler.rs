//! Schema65 names each audio content's warp resampler law beside the schema64
//! (or schema63) layout. A domain carries `warp_resampler` (1 = v1, 2 = v2) only
//! when the law differs from [`hex_project::WarpResampler::implied`]; every other domain is
//! byte-identical to its predecessor layout. The tags are never reused.
//!
//! Migration: schemas up to 64 have no such field. Content that reads
//! fractional source positions (nonzero pitch or warp enabled) was rendered by
//! the frozen v1 law and migrates to v1, so saved v1 warped clips render
//! bit-identically. All other content reads integer positions at unity rate,
//! which v1 and v2 reproduce exactly, and takes v2.
//!
//! Only projects with at least one explicit law are written as schema65; all
//! others keep the schema64/63 layout older readers open. A schema65 document
//! without an explicit law is still read and saved back in the older layout.
//! The field lives in the shared audio domain record, so visible clips, alias
//! representatives (the law is content data, not an alias payload key) and
//! retained event roots all carry it through their existing codecs. An older
//! document containing the field is refused. Directed content views name their
//! own read law in schema66 (`schema::content_view_resampler`).
use crate::{Project, StorageError};
use hex_project::{AudioClipContent, Clip};
use serde_json::Value;

/// Every audio content in the project, including retained event roots. Alias
/// sources share their representative members' content.
fn contents(project: &Project) -> impl Iterator<Item = &AudioClipContent> {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .flat_map(|clip| {
            std::iter::once(clip).chain(clip.retained_audio_roots().map(|root| &root.clip))
        })
        .filter_map(Clip::audio_edit)
        .map(|edit| &*edit.domain.content)
}

pub(crate) fn contains_explicit(project: &Project) -> bool {
    contents(project).any(AudioClipContent::has_explicit_warp_resampler)
}

pub(crate) fn require_implied(project: &Project) -> Result<(), StorageError> {
    if contains_explicit(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save a warp resampler version; save with the current format",
        ));
    }
    Ok(())
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut value = if super::conversion_v2::contains_v2(project) {
        super::conversion_v2::encode(project)?
    } else {
        super::track_name_origin::encode(project)?
    };
    value["version"] = crate::WARP_RESAMPLER_SCHEMA_VERSION.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: Value = serde_json::from_slice(bytes)?;
    if value.get("version").and_then(Value::as_u64)
        != Some(crate::WARP_RESAMPLER_SCHEMA_VERSION.into())
    {
        return Err(StorageError::Invalid("invalid warp resampler schema"));
    }
    // Schema64 is the same layout without the optional domain field; its
    // decoder accepts identity, v1 and v2 conversion rows alike.
    value["version"] = crate::SINC_V2_SCHEMA_VERSION.into();
    super::conversion_v2::decode(&serde_json::to_vec(&value)?)
}

/// Test-only model of a project authored before schema65: every visible and
/// alias content takes its implied law, preserving alias sharing. Retained
/// roots must already be implied.
#[cfg(test)]
pub(crate) fn with_implied_laws(project: &Project) -> Project {
    use hex_project::{AliasContentSnapshot, TrackContentSnapshot, WarpResampler};
    use std::{collections::HashMap, sync::Arc};
    let mut snapshot = project.snapshot();
    let mut replaced = HashMap::<*const AudioClipContent, Arc<AudioClipContent>>::new();
    let mut implied = |content: &mut Arc<AudioClipContent>| {
        let key = Arc::as_ptr(content);
        let next = replaced.entry(key).or_insert_with(|| {
            let mut next = (**content).clone();
            next.warp_resampler = WarpResampler::implied(next.pitch_cents, next.warp_enabled);
            Arc::new(next)
        });
        *content = Arc::clone(next);
    };
    for source in &mut snapshot.aliases.sources {
        if let AliasContentSnapshot::Audio { content, .. } = &mut source.content {
            implied(content);
        }
    }
    for track in &mut snapshot.tracks {
        if let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut track.content {
            for edit in audio_clips.iter_mut().filter_map(|clip| clip.edit.as_mut()) {
                implied(&mut Arc::make_mut(&mut edit.domain).content);
            }
        }
    }
    let project = Project::from_snapshot(snapshot).expect("implied laws stay valid");
    assert!(
        !contains_explicit(&project),
        "retained roots need implied laws"
    );
    project
}

#[cfg(test)]
mod tests;
