//! Complete combined envelope after the historical schema18/19 branch collision.
use super::*;

pub(super) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    // Share bounded field decoding and model validation, not a permissive fallback.
    let mut document: gain_touch::DocumentV22 = serde_json::from_slice(bytes)?;
    if document.format != "hex-project" || document.version != 23 {
        return Err(StorageError::Invalid(
            "unsupported project format or version",
        ));
    }
    document.version = 22;
    Project::from_legacy_snapshot(document.into_snapshot()?)
        .map_err(|error| StorageError::Invalid(error.0))
}
