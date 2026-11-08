//! Schema66 names each directed audio content view's read resampler law beside
//! the schema65 (or older) layout. A view row carries `read_resampler` (the
//! [`hex_project::WarpResampler`] tags: 1 = v1, 2 = v2) only when its law
//! differs from [`hex_project::AudioContentView::implied_read_resampler`];
//! every other view is byte-identical to its predecessor layout.
//!
//! Migration: schemas 41..=65 have no such field. Musical-clock views read
//! fractional base positions and were rendered by the frozen v1 law, so they
//! migrate to v1 and render bit-identically. Frame-clock views copy integer
//! samples, which every law reproduces exactly, and take v2. New views take v2;
//! a musical-clock v2 view therefore makes its project schema66.
//!
//! Only projects with at least one explicit view law are written as schema66;
//! all others keep the schema65/64/63 layout older readers open. A schema66
//! document without an explicit law is still read and saved back in the older
//! layout. An older document containing the field is refused.
use crate::{Project, StorageError};
use serde_json::Value;

pub(crate) fn contains_explicit(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter_map(hex_project::Clip::audio_edit)
        .any(|edit| {
            edit.content_view
                .is_some_and(|view| view.has_explicit_read_resampler(edit.domain.clock))
        })
}

pub(crate) fn require_implied(project: &Project) -> Result<(), StorageError> {
    if contains_explicit(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save a directed view resampler version; save with the current format",
        ));
    }
    Ok(())
}

/// The schema65-or-older layout `project` would otherwise use, renumbered.
pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut value = if super::warp_resampler::contains_explicit(project) {
        super::warp_resampler::encode(project)?
    } else if super::conversion_v2::contains_v2(project) {
        super::conversion_v2::encode(project)?
    } else {
        super::track_name_origin::encode(project)?
    };
    value["version"] = crate::CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: Value = serde_json::from_slice(bytes)?;
    if value.get("version").and_then(Value::as_u64)
        != Some(crate::CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION.into())
    {
        return Err(StorageError::Invalid(
            "invalid directed view resampler schema",
        ));
    }
    // Schema65 is the same layout without the optional view field and accepts
    // schema64 and schema63 content alike.
    value["version"] = crate::WARP_RESAMPLER_SCHEMA_VERSION.into();
    super::warp_resampler::decode(&serde_json::to_vec(&value)?)
}

#[cfg(test)]
mod tests;
