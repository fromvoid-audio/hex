//! Schema51 admits only model-validated, two-voice Arrangement audio overlaps
//! covered by opposing authored fade ramps. Its fields are exactly schema50's
//! device-automation inventory and receiver/cycle layout; older readers reject that overlap semantics. Keep the
//! frozen schema50 codec and explicitly enforce its disjoint occupancy rule on
//! every older import. No overlap is repaired and no original bytes are written.
use crate::{Project, StorageError};

pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    let mut value = super::device_automation::encode(project)?;
    value["version"] = 51.into();
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: serde_json::Value) -> Result<Project, StorageError> {
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid crossfade project envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(51) {
        return Err(StorageError::Invalid("invalid crossfade project schema"));
    }
    object.insert("version".into(), 50.into());
    super::device_automation::decode_value(value)
}

/// The frozen pre51 contract, independent of the new model's faded-overlap
/// admission. This is a schema validation rule, never a normalization pass.
pub(crate) fn require_legacy_disjoint(project: &Project) -> Result<(), StorageError> {
    for track in project.tracks() {
        let mut intervals = Vec::new();
        intervals
            .try_reserve_exact(track.clips().len())
            .map_err(|_| {
                StorageError::Invalid("could not reserve historical audio occupancy validation")
            })?;
        for clip in track.clips() {
            intervals.push((
                clip.start(),
                clip.start()
                    .checked_add(clip.duration())
                    .ok_or(StorageError::Invalid("historical audio interval overflow"))?,
            ));
        }
        intervals.sort_unstable();
        if intervals.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(StorageError::Invalid(
                "older schema does not support arrangement audio overlaps; retain the original file and use a copy with the hex build that saved it",
            ));
        }
    }
    Ok(())
}
