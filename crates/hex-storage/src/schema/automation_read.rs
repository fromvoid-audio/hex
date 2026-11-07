//! Schema55 requires an explicit read selection on every provider/host envelope.
//! Supported schema53 and older migrate to Authored without changing points.
//! Schema54 is reserved by the evidenced, unmerged printed-output branch
//! a4c1a99c236fd4bffa3f8ed32a0de8b22fde6af2: its role/silence inventories are
//! incompatible and are explicitly refused, never reinterpreted or discarded.
//! Only a temporary snapshot passed to the frozen53 writer selects Authored;
//! older writers themselves refuse Base so no published document loses intent.
use super::device_automation::devices;
use crate::{Project, StorageError};
use hex_project::{AutomationReadSelection, DeviceAutomationTarget};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Selection {
    Authored,
    Base,
}
impl From<AutomationReadSelection> for Selection {
    fn from(value: AutomationReadSelection) -> Self {
        match value {
            AutomationReadSelection::Authored => Self::Authored,
            AutomationReadSelection::Base => Self::Base,
        }
    }
}
impl From<Selection> for AutomationReadSelection {
    fn from(value: Selection) -> Self {
        match value {
            Selection::Authored => Self::Authored,
            Selection::Base => Self::Base,
        }
    }
}
/// Frozen writers may not erase the read selection while retaining the curve.
pub(crate) fn require_authored(project: &Project) -> Result<(), StorageError> {
    if project.has_overridden_device_automation() {
        return Err(StorageError::Invalid(
            "older schema cannot save device automation read selection",
        ));
    }
    Ok(())
}
fn key(
    row: &serde_json::Map<String, Value>,
    host: bool,
) -> Result<(u64, DeviceAutomationTarget), StorageError> {
    let device = row
        .get("device")
        .and_then(Value::as_u64)
        .filter(|id| *id != 0)
        .ok_or(StorageError::Invalid("invalid automation device identity"))?;
    let target = if host {
        match row.get("target").and_then(Value::as_str) {
            Some("input_gain") => DeviceAutomationTarget::HostInputGain,
            Some("output_gain") => DeviceAutomationTarget::HostOutputGain,
            _ => return Err(StorageError::Invalid("invalid host automation target")),
        }
    } else {
        let parameter = row
            .get("parameter")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .and_then(|value| hex_lattice_contract::ParameterId::new(value).ok())
            .ok_or(StorageError::Invalid(
                "invalid automation parameter identity",
            ))?;
        DeviceAutomationTarget::Parameter(parameter)
    };
    Ok((device, target))
}
fn rows(value: &mut Value, host: bool) -> Result<&mut Vec<Value>, StorageError> {
    value
        .get_mut(if host {
            "host_automation"
        } else {
            "device_automation"
        })
        .and_then(Value::as_array_mut)
        .ok_or(StorageError::Invalid(
            "automation inventory missing or invalid",
        ))
}
pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    super::note_activation::require_all_active(project)?;
    let selections: BTreeMap<_, _> = project
        .automation_devices()
        .flat_map(|device| {
            device
                .automation()
                .iter()
                .map(move |lane| ((device.id().value(), lane.target), lane.read_selection))
        })
        .collect();
    // Ordinary projects already satisfy the frozen writer. Only an overridden
    // selection needs a neutral temporary model; avoid copying all project state
    // just to rewrite no selections.
    let mut value = if project.has_overridden_device_automation() {
        let mut snapshot = project.snapshot();
        for device in devices(&mut snapshot) {
            for lane in &mut device.automation {
                lane.read_selection = AutomationReadSelection::Authored;
            }
        }
        let core = Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
        super::host_automation::encode(&core)?
    } else {
        super::host_automation::encode(project)?
    };
    for host in [false, true] {
        for row in rows(&mut value, host)? {
            let row = row
                .as_object_mut()
                .ok_or(StorageError::Invalid("invalid automation row"))?;
            let selection = selections
                .get(&key(row, host)?)
                .ok_or(StorageError::Invalid(
                    "automation selection inventory mismatch",
                ))?;
            row.insert(
                "read_selection".into(),
                serde_json::to_value(Selection::from(*selection))?,
            );
        }
    }
    value["version"] = 55.into();
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: Value) -> Result<Project, StorageError> {
    if value.get("version").and_then(Value::as_u64) != Some(55) {
        return Err(StorageError::Invalid("invalid automation read schema"));
    }
    let mut selections = BTreeMap::new();
    for host in [false, true] {
        for row in rows(&mut value, host)? {
            if selections.len() == hex_project::MAX_DEVICE_AUTOMATION_POINTS {
                return Err(StorageError::Invalid(
                    "automation selection inventory limit reached",
                ));
            }
            let row = row
                .as_object_mut()
                .ok_or(StorageError::Invalid("invalid automation row"))?;
            let selection: Selection = serde_json::from_value(
                row.remove("read_selection")
                    .ok_or(StorageError::Invalid("automation read selection missing"))?,
            )?;
            if selections.insert(key(row, host)?, selection).is_some() {
                return Err(StorageError::Invalid("duplicate automation read selection"));
            }
        }
    }
    value["version"] = 53.into();
    let core = super::host_automation::decode_value(value)?;
    // Strict53 has validated and retained every row and its canonical owner,
    // constructing Authored selections. No model copy is needed in that case.
    if selections
        .values()
        .all(|selection| matches!(selection, Selection::Authored))
    {
        return Ok(core);
    }
    let mut snapshot = core.snapshot();
    for device in devices(&mut snapshot) {
        for lane in &mut device.automation {
            lane.read_selection = selections
                .remove(&(device.id, lane.target))
                .ok_or(StorageError::Invalid(
                    "automation selection inventory mismatch",
                ))?
                .into();
        }
    }
    if !selections.is_empty() {
        return Err(StorageError::Invalid("automation selection owner missing"));
    }
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}
#[cfg(test)]
mod tests;
