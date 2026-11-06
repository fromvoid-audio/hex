//! Schema53 adds typed native host input/output gain envelopes over strict52.
//! Provider parameter rows remain the frozen50 numeric contract. Host targets
//! never masquerade as ParameterIds: only a temporary snapshot passed to the
//! predecessor encoder omits them. Legacy writers refuse this information.
//! Decoding is bounded, in memory, and validates the combined canonical owner
//! inventory before returning; shared source projections remain empty.
use super::device_automation::{Interpolation, Point, devices};
use crate::{Project, StorageError};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, DeviceAutomation,
    DeviceAutomationTarget,
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Target {
    InputGain,
    OutputGain,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lane {
    device: u64,
    target: Target,
    #[serde(deserialize_with = "super::device_automation::points")]
    points: Vec<Point>,
}
#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "rows")] Vec<Lane>);
fn rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Lane>, D::Error> {
    struct Inventory;
    impl<'de> serde::de::Visitor<'de> for Inventory {
        type Value = Vec<Lane>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded host automation inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            let mut count = 0usize;
            while let Some(row) = seq.next_element::<Lane>()? {
                count = count.checked_add(row.points.len()).ok_or_else(|| {
                    serde::de::Error::custom("host automation point count overflow")
                })?;
                if rows.len() == hex_project::MAX_DEVICE_AUTOMATION_POINTS
                    || count > hex_project::MAX_DEVICE_AUTOMATION_POINTS
                {
                    return Err(serde::de::Error::custom(
                        "project host automation point limit reached",
                    ));
                }
                rows.try_reserve(1).map_err(serde::de::Error::custom)?;
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Inventory)
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    super::automation_read::require_authored(project)?;
    let mut rows = Vec::new();
    for device in project.automation_devices() {
        for lane in device.automation() {
            let target = match lane.target {
                DeviceAutomationTarget::Parameter(_) => continue,
                DeviceAutomationTarget::HostInputGain => Target::InputGain,
                DeviceAutomationTarget::HostOutputGain => Target::OutputGain,
                DeviceAutomationTarget::Vst3 { .. } => {
                    return Err(StorageError::Invalid(
                        "older schema cannot save vst3 automation targets",
                    ));
                }
            };
            rows.push(Lane {
                device: device.id().value(),
                target,
                points: lane
                    .points
                    .iter()
                    .map(|p| Point {
                        beat_raw: p.beat.raw(),
                        value: p.value.get(),
                        outgoing: match p.outgoing {
                            AutomationInterpolation::Step => Interpolation::Step,
                            AutomationInterpolation::Linear => Interpolation::Linear,
                        },
                    })
                    .collect(),
            });
        }
    }
    rows.sort_by_key(|row| (row.device, row.target));
    let mut snapshot = project.snapshot();
    for device in devices(&mut snapshot) {
        device.automation.retain(|lane| !lane.target.is_host());
    }
    let legacy = Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
    let mut value = super::clip_aliases::encode(&legacy)?;
    value["version"] = 53.into();
    value["host_automation"] = serde_json::to_value(rows)?;
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: serde_json::Value) -> Result<Project, StorageError> {
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid host automation document"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(53) {
        return Err(StorageError::Invalid("invalid host automation schema"));
    }
    let Rows(rows) = serde_json::from_value(
        object
            .remove("host_automation")
            .ok_or(StorageError::Invalid("host automation inventory missing"))?,
    )?;
    object.insert("version".into(), 52.into());
    let core = super::clip_aliases::decode_value(value)?;
    let known: std::collections::BTreeSet<_> =
        core.automation_devices().map(|d| d.id().value()).collect();
    let mut snapshot = core.snapshot();
    let mut previous = None;
    let mut authored = std::collections::BTreeMap::<u64, Vec<DeviceAutomation>>::new();
    for row in rows {
        let key = (row.device, row.target);
        if previous.is_some_and(|old| old >= key) {
            return Err(StorageError::Invalid(
                "host automation targets must strictly increase",
            ));
        }
        previous = Some(key);
        if !known.contains(&row.device) {
            return Err(StorageError::Invalid(
                "host automation device no longer exists",
            ));
        }
        let points = row
            .points
            .into_iter()
            .map(|p| {
                Ok(AutomationPoint {
                    beat: hex_project::BeatTime::from_raw(p.beat_raw),
                    value: AutomationValue::new(p.value).map_err(|e| StorageError::Invalid(e.0))?,
                    outgoing: match p.outgoing {
                        Interpolation::Step => AutomationInterpolation::Step,
                        Interpolation::Linear => AutomationInterpolation::Linear,
                    },
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        authored
            .entry(row.device)
            .or_default()
            .push(DeviceAutomation {
                vst3_class: None,
                read_selection: hex_project::AutomationReadSelection::Authored,
                target: match row.target {
                    Target::InputGain => DeviceAutomationTarget::HostInputGain,
                    Target::OutputGain => DeviceAutomationTarget::HostOutputGain,
                },
                points,
            });
    }
    for device in devices(&mut snapshot) {
        if let Some(lanes) = authored.remove(&device.id) {
            device.automation.extend(lanes);
        }
    }
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}
#[cfg(test)]
mod tests;
