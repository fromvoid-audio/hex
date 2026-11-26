//! Schema125 stores complete VST3 envelopes as compact base64, keyed by the
//! canonical device and comparison slot. Its predecessor carries all other
//! project fields, including the provider identity used by automation lanes.
use super::device_automation::{bounded, devices};
use crate::{LARGE_VST3_STATE_SCHEMA_VERSION, MAX_PROJECT_DOCUMENT_BYTES, Project, StorageError};
use base64::{Engine, engine::general_purpose::STANDARD};
use hex_lattice_contract::{
    MAX_STORED_VST3_DEVICE_STATE_BYTES, MAX_VST3_DEVICE_STATE_BYTES, NativeDeviceState,
};
use hex_project::DeviceComparison;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::{borrow::Cow, collections::BTreeMap};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Row<'a> {
    device: u64,
    inactive: bool,
    sha256: [u8; 32],
    #[serde(borrow)]
    payload: Cow<'a, str>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document<'a> {
    format: &'a str,
    version: u32,
    #[serde(borrow)]
    base: &'a RawValue,
    #[serde(borrow, deserialize_with = "read_rows")]
    vst3_states: Vec<Row<'a>>,
}

fn read_rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row<'de>>, D::Error> {
    bounded(d, 2 * hex_project::MAX_DEVICES)
}

pub(crate) fn contains(project: &Project) -> bool {
    super::device_assets::authored_states(project)
        .any(|state| state.payload().len() > MAX_VST3_DEVICE_STATE_BYTES)
}

fn placeholder(state: &NativeDeviceState) -> Result<NativeDeviceState, StorageError> {
    let empty = match hex_plugin_state::decode(state) {
        Ok(view) => hex_plugin_state::encode(hex_plugin_state::State {
            identity: view.identity,
            component: view.component.map(|_| &[][..]),
            controller: view.controller.map(|_| &[][..]),
        })
        .map_err(|_| StorageError::Invalid("invalid vst3 state identity"))?,
        // Unknown and malformed provider envelopes still belong to the user.
        Err(_) => NativeDeviceState::new(state.type_id(), state.state_version(), Vec::new())
            .map_err(|_| StorageError::Invalid("invalid opaque vst3 state"))?,
    };
    empty
        .with_host_metadata_from(state)
        .map_err(|_| StorageError::Invalid("invalid vst3 host metadata"))
}

fn map_states(
    project: &Project,
    mut visit: impl FnMut(u64, bool, &NativeDeviceState) -> Result<NativeDeviceState, StorageError>,
) -> Result<Project, StorageError> {
    let mut snapshot = project.snapshot();
    for device in
        devices(&mut snapshot).filter(|device| device.state.type_id() == hex_plugin_state::TYPE_ID)
    {
        device.state = visit(device.id, false, &device.state)?;
        if let Some(comparison) = device.comparison.as_mut() {
            *comparison = DeviceComparison::new(
                comparison.active(),
                visit(device.id, true, comparison.inactive())?,
            );
        }
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

// Include the owner, comparison slot and provider version so two independently
// valid payloads cannot be silently exchanged between instances of one class.
fn checksum(device: u64, inactive: bool, version: u32, payload: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(device.to_le_bytes())
        .chain_update([u8::from(inactive)])
        .chain_update(version.to_le_bytes())
        .chain_update(payload)
        .finalize()
        .into()
}

pub(crate) fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    let mut rows = Vec::new();
    let mut remaining = MAX_PROJECT_DOCUMENT_BYTES;
    let base = map_states(project, |device, inactive, state| {
        let bytes = state.payload();
        remaining =
            remaining
                .checked_sub(bytes.len().div_ceil(3) * 4)
                .ok_or(StorageError::Invalid(
                    "project exceeds the document size limit",
                ))?;
        rows.push(Row {
            device,
            inactive,
            sha256: checksum(device, inactive, state.state_version(), bytes),
            payload: Cow::Owned(STANDARD.encode(bytes)),
        });
        placeholder(state)
    })?;
    let base = crate::encode(&base)?;
    if base.len() > remaining {
        return Err(StorageError::Invalid(
            "project exceeds the document size limit",
        ));
    }
    crate::encode_document(&Document {
        format: "hex-project",
        version: LARGE_VST3_STATE_SCHEMA_VERSION,
        base: serde_json::from_slice(&base)?,
        vst3_states: rows,
    })
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.len() > MAX_PROJECT_DOCUMENT_BYTES {
        return Err(StorageError::Invalid(
            "project exceeds the document size limit",
        ));
    }
    #[derive(Deserialize)]
    struct Header {
        version: u32,
    }
    let document: Document<'_> = serde_json::from_slice(bytes)?;
    let base = document.base.get().as_bytes();
    let header: Header = serde_json::from_slice(base)?;
    if header.version >= LARGE_VST3_STATE_SCHEMA_VERSION {
        return Err(StorageError::Invalid(
            "invalid large vst3 state base version",
        ));
    }
    let mut slots = BTreeMap::new();
    for row in document.vst3_states {
        if slots.insert((row.device, row.inactive), row).is_some() {
            return Err(StorageError::Invalid("duplicate vst3 state slot"));
        }
    }
    let project = map_states(&crate::decode(base)?, |device, inactive, neutral| {
        let row = slots
            .remove(&(device, inactive))
            .ok_or(StorageError::Invalid("missing vst3 state slot"))?;
        if row.payload.len() > MAX_STORED_VST3_DEVICE_STATE_BYTES.div_ceil(3) * 4 {
            return Err(StorageError::Invalid(
                "vst3 project state exceeds the size limit",
            ));
        }
        let payload = STANDARD
            .decode(row.payload.as_bytes())
            .map_err(|_| StorageError::Invalid("invalid vst3 state encoding"))?;
        if payload.len() > MAX_STORED_VST3_DEVICE_STATE_BYTES
            || checksum(device, inactive, neutral.state_version(), &payload) != row.sha256
        {
            return Err(StorageError::Invalid("invalid vst3 project state payload"));
        }
        let state = NativeDeviceState::new(neutral.type_id(), neutral.state_version(), payload)
            .and_then(|state| state.with_host_metadata_from(neutral))
            .map_err(|_| StorageError::Invalid("invalid vst3 project state"))?;
        if placeholder(&state)?.payload() != neutral.payload() {
            return Err(StorageError::Invalid(
                "vst3 state identity does not match its owner",
            ));
        }
        Ok(state)
    })?;
    if !slots.is_empty() || !contains(&project) {
        return Err(StorageError::Invalid("invalid large vst3 state inventory"));
    }
    Ok(project)
}

#[cfg(test)]
mod tests;
