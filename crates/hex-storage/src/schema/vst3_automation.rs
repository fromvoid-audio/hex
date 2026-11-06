//! Schema77 retains class-bound VST3 targets independently of native IDs.
//! The remaining project uses the complete schema75 layout plus an explicit
//! sample-rate mode. Earlier schemas have no VST3 target inventory and keep
//! their original native/host lane meanings and authored sample-rate mode.
use super::{automation_read::Selection, device_automation};
use crate::{
    Project, StorageError, VST3_AUTOMATION_SCHEMA_VERSION, VST3_STATE_SIZE_SCHEMA_VERSION,
};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, DeviceAutomation,
    DeviceAutomationTarget, SampleRateMode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lane {
    device: u64,
    class: [u8; 16],
    parameter: u32,
    read_selection: Selection,
    #[serde(deserialize_with = "device_automation::points")]
    points: Vec<device_automation::Point>,
}

#[derive(Deserialize)]
struct Lanes(#[serde(deserialize_with = "lanes")] Vec<Lane>);

fn lanes<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Lane>, D::Error> {
    device_automation::bounded_lanes(deserializer, |lane: &Lane| lane.points.len())
}

pub(crate) fn contains(project: &Project) -> bool {
    project.automation_devices().any(|device| {
        device
            .automation()
            .iter()
            .any(|lane| matches!(lane.target, DeviceAutomationTarget::Vst3 { .. }))
    })
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    for device in device_automation::devices(&mut snapshot) {
        for lane in std::mem::take(&mut device.automation) {
            let DeviceAutomationTarget::Vst3 { parameter } = lane.target else {
                device.automation.push(lane);
                continue;
            };
            let class = lane.vst3_class.ok_or(StorageError::Invalid(
                "vst3 automation class stamp is missing",
            ))?;
            rows.push(Lane {
                device: device.id,
                class,
                parameter,
                read_selection: lane.read_selection.into(),
                points: lane
                    .points
                    .into_iter()
                    .map(|point| device_automation::Point {
                        beat_raw: point.beat.raw(),
                        value: point.value.get(),
                        outgoing: match point.outgoing {
                            AutomationInterpolation::Step => device_automation::Interpolation::Step,
                            AutomationInterpolation::Linear => {
                                device_automation::Interpolation::Linear
                            }
                        },
                    })
                    .collect(),
            });
        }
    }
    rows.sort_by_key(|row| (row.device, row.class, row.parameter));
    let core = Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
    let mut value = super::group_output_mute::encode(&core)?;
    value["version"] = VST3_AUTOMATION_SCHEMA_VERSION.into();
    value["sample_rate_mode"] = match project.sample_rate_mode() {
        SampleRateMode::Fixed => "fixed",
        SampleRateMode::FollowInterface => "follow_interface",
    }
    .into();
    value["vst3_automation"] = serde_json::to_value(rows)?;
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?, true)
}

pub(super) fn decode_value(
    mut value: Value,
    require_vst3_lanes: bool,
) -> Result<Project, StorageError> {
    let root = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid vst3 automation document"))?;
    let mode = match root
        .remove("sample_rate_mode")
        .as_ref()
        .and_then(Value::as_str)
    {
        Some("fixed") => SampleRateMode::Fixed,
        Some("follow_interface") => SampleRateMode::FollowInterface,
        _ => return Err(StorageError::Invalid("invalid project sample rate mode")),
    };
    let Lanes(rows) = serde_json::from_value(root.remove("vst3_automation").ok_or(
        StorageError::Invalid("vst3 automation inventory is missing"),
    )?)?;
    if require_vst3_lanes && rows.is_empty() {
        return Err(StorageError::Invalid("vst3 automation inventory is empty"));
    }
    root.insert("version".into(), VST3_STATE_SIZE_SCHEMA_VERSION.into());
    let core = super::group_output_mute::decode_value(value, VST3_STATE_SIZE_SCHEMA_VERSION)?;
    let mut snapshot = core.snapshot();
    let mut by_device = BTreeMap::<u64, Vec<DeviceAutomation>>::new();
    let mut previous = None;
    let mut point_count = core
        .automation_devices()
        .flat_map(|device| device.automation())
        .map(|lane| lane.points.len())
        .sum::<usize>();
    for row in rows {
        let key = (row.device, row.class, row.parameter);
        if previous.is_some_and(|previous| previous >= key) {
            return Err(StorageError::Invalid(
                "vst3 automation targets must strictly increase",
            ));
        }
        previous = Some(key);
        point_count += row.points.len();
        if point_count > hex_project::MAX_DEVICE_AUTOMATION_POINTS {
            return Err(StorageError::Invalid(
                "project device automation point limit reached",
            ));
        }
        let points = row
            .points
            .into_iter()
            .map(|point| {
                Ok(AutomationPoint {
                    beat: hex_project::BeatTime::from_raw(point.beat_raw),
                    value: AutomationValue::new(point.value)
                        .map_err(|e| StorageError::Invalid(e.0))?,
                    outgoing: match point.outgoing {
                        device_automation::Interpolation::Step => AutomationInterpolation::Step,
                        device_automation::Interpolation::Linear => AutomationInterpolation::Linear,
                    },
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        by_device
            .entry(row.device)
            .or_default()
            .push(DeviceAutomation {
                touch: Vec::new(),
                read_selection: row.read_selection.into(),
                target: DeviceAutomationTarget::Vst3 {
                    parameter: row.parameter,
                },
                vst3_class: Some(row.class),
                points,
            });
    }
    for device in device_automation::devices(&mut snapshot) {
        if let Some(lanes) = by_device.remove(&device.id) {
            device.automation.extend(lanes);
        }
    }
    if !by_device.is_empty() {
        return Err(StorageError::Invalid(
            "vst3 automation device no longer exists",
        ));
    }
    Project::from_snapshot(snapshot)
        .map(|project| project.with_sample_rate_mode(mode))
        .map_err(|e| StorageError::Invalid(e.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{AutomationReadSelection, BeatTime, Command, Created, Editor};

    #[test]
    fn schema77_preserves_mixed_targets_and_refuses_corrupt_vst3_identity() {
        let class = [9; 16];
        let component = [42; 8192];
        let state = hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new(class).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [6; 32],
                    info_sha256: [7; 32],
                },
            },
            component: Some(&component),
            controller: Some(&[]),
        })
        .unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Device(native) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("expected a native device");
        };
        let Created::Device(vst3) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("expected a vst3 device");
        };
        let zero = DeviceAutomationTarget::Vst3 { parameter: 0 };
        assert!(
            editor
                .execute(Command::ReplaceDeviceAutomationTarget {
                    device: native,
                    target: zero,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ONE,
                        value: AutomationValue::new(0.75).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .is_err()
        );
        for (device, target) in [
            (
                native,
                DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(
                    1,
                )),
            ),
            (native, DeviceAutomationTarget::HostOutputGain),
            (vst3, zero),
            (
                vst3,
                DeviceAutomationTarget::Vst3 {
                    parameter: i32::MAX as u32,
                },
            ),
        ] {
            editor
                .execute(Command::ReplaceDeviceAutomationTarget {
                    device,
                    target,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ONE,
                        value: AutomationValue::new(0.75).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
        }
        editor
            .execute(Command::SetDeviceAutomationReadSelection {
                device: vst3,
                target: zero,
                selection: AutomationReadSelection::Base,
            })
            .unwrap();
        editor
            .execute(Command::MoveMainDevice {
                device: vst3,
                before: Some(native),
            })
            .unwrap();
        assert_eq!(editor.project().main_devices()[0].id(), vst3);
        let directory =
            std::env::temp_dir().join(format!("hex-vst3-automation-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        for (index, mode) in [SampleRateMode::Fixed, SampleRateMode::FollowInterface]
            .into_iter()
            .enumerate()
        {
            let project = editor.project().clone().with_sample_rate_mode(mode);
            assert_eq!(
                crate::schema_version_for(&project),
                VST3_AUTOMATION_SCHEMA_VERSION
            );
            let path = directory.join(format!("{index}.hex"));
            crate::save_new(&path, &project).unwrap();
            let (reopened, token) = crate::load_with_token(&path).unwrap();
            assert_eq!(reopened, project);
            assert!(!token.requires_format_upgrade());
        }
        std::fs::remove_dir_all(directory).unwrap();
        let value = encode(editor.project()).unwrap();
        let decode_mutation = |mutate: fn(&mut Value)| {
            let mut value = value.clone();
            mutate(&mut value);
            crate::decode(&serde_json::to_vec(&value).unwrap())
        };
        assert!(decode_mutation(|v| v["vst3_automation"][0]["class"][0] = 5.into()).is_err());
        assert!(
            decode_mutation(|v| v["vst3_automation"][0]["parameter"] = u32::MAX.into()).is_err()
        );
        assert!(decode_mutation(|v| v["vst3_automation"][0]["device"] = 99.into()).is_err());
        assert!(
            decode_mutation(|v| v["vst3_automation"][1] = v["vst3_automation"][0].clone()).is_err()
        );
        assert!(decode_mutation(|v| v["vst3_automation"] = serde_json::json!([])).is_err());
        assert!(decode_mutation(|v| v["sample_rate_mode"] = Value::Null).is_err());
        let historical = crate::decode(include_bytes!(
            "../../tests/fixtures/track-name-origin/schema62.json"
        ))
        .unwrap();
        assert!(!contains(&historical));
        assert_eq!(historical.sample_rate_mode(), SampleRateMode::Fixed);
    }
}
