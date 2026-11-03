//! Schema67 admits saved native device states newer than any schema66 reader
//! decodes; the document layout is otherwise identical to schema66 (or older).
//! The only such state is a Sampler state v3 (Pulse #4088: an authored loop
//! crossfade). The Sampler writes v2 whenever its crossfade is off, so only a
//! project with at least one crossfaded Sampler (active or inactive comparison
//! variant, on any track, return, main or retained native source) is written
//! as schema67; every other project keeps the layout older readers open.
//!
//! Device payloads stay opaque here: this table records, per device type, the
//! newest state version schema66 readers accept. It is compatibility metadata,
//! not a device codec, and the device registry still validates every state.
//!
//! Migration: schemas up to 66 contain only states their readers accepted, so
//! they decode unchanged. A schema67 document without a newer state is read and
//! saved back in its older layout. An older document containing one is refused.
use crate::{Project, StorageError};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use serde_json::Value;

/// (device type, newest state version a schema66 reader decodes).
const SCHEMA66_STATE_VERSIONS: &[(DeviceTypeId, u32)] =
    &[(DeviceTypeId::from_static("hex.instrument.sampler"), 2)];

fn newer_than_schema66(state: &NativeDeviceState) -> bool {
    SCHEMA66_STATE_VERSIONS
        .iter()
        .any(|(type_id, newest)| state.type_id() == *type_id && state.state_version() > *newest)
}

pub(crate) fn contains_newer_states(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.devices().iter().chain(track.instrument()))
        .chain(
            project
                .return_tracks()
                .iter()
                .flat_map(|track| track.devices()),
        )
        .chain(project.main_devices())
        .chain(
            project
                .native_sources()
                .iter()
                .flat_map(|source| source.devices().chain([source.instrument()])),
        )
        .flat_map(hex_project::Device::authored_states)
        .any(newer_than_schema66)
}

pub(crate) fn require_older_states(project: &Project) -> Result<(), StorageError> {
    if contains_newer_states(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save a newer device state; save with the current format",
        ));
    }
    Ok(())
}

/// The schema66-or-older layout `project` would otherwise use, renumbered.
pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut value = if super::content_view_resampler::contains_explicit(project) {
        super::content_view_resampler::encode(project)?
    } else if super::warp_resampler::contains_explicit(project) {
        super::warp_resampler::encode(project)?
    } else if super::conversion_v2::contains_v2(project) {
        super::conversion_v2::encode(project)?
    } else {
        super::track_name_origin::encode(project)?
    };
    value["version"] = crate::DEVICE_STATE_SCHEMA_VERSION.into();
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let _: super::audio_events::UniqueKeys = serde_json::from_slice(bytes)?;
    let mut value: Value = serde_json::from_slice(bytes)?;
    if value.get("version").and_then(Value::as_u64)
        != Some(crate::DEVICE_STATE_SCHEMA_VERSION.into())
    {
        return Err(StorageError::Invalid("invalid device state schema"));
    }
    // Schema66 is the same layout and accepts schema65/64/63 content alike.
    value["version"] = crate::CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION.into();
    super::content_view_resampler::decode(&serde_json::to_vec(&value)?)
}

#[cfg(test)]
mod tests;
