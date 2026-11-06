//! Schema 78 adds exact half-open native touch spans. Older schemas migrate
//! with an empty overlay and preserve the original authored curve and read
//! selection. The schema 77 codec still owns all other project data.
use super::{automation_read::Selection, device_automation};
use crate::{DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION, Project, StorageError};
use hex_lattice_contract::ParameterId;
use hex_project::{
    AutomationValue, BeatTime, DeviceAutomation, DeviceAutomationTarget, DeviceAutomationTouchSpan,
    DeviceAutomationTouchStep,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Target {
    Parameter(u32),
    HostInputGain,
    HostOutputGain,
}
impl Target {
    fn from_target(target: DeviceAutomationTarget) -> Result<Self, StorageError> {
        Ok(match target {
            DeviceAutomationTarget::Parameter(id) => Self::Parameter(id.value()),
            DeviceAutomationTarget::HostInputGain => Self::HostInputGain,
            DeviceAutomationTarget::HostOutputGain => Self::HostOutputGain,
            DeviceAutomationTarget::Vst3 { .. } => {
                return Err(StorageError::Invalid("vst3 touch overlays are unsupported"));
            }
        })
    }

    fn into_target(self) -> Result<DeviceAutomationTarget, StorageError> {
        Ok(match self {
            Self::Parameter(id) => DeviceAutomationTarget::Parameter(
                ParameterId::new(id)
                    .map_err(|_| StorageError::Invalid("invalid touch parameter id"))?,
            ),
            Self::HostInputGain => DeviceAutomationTarget::HostInputGain,
            Self::HostOutputGain => DeviceAutomationTarget::HostOutputGain,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lane {
    device: u64,
    target: Target,
    read_selection: Selection,
    spans: Vec<Span>,
}
#[derive(Deserialize)]
struct Rows(#[serde(deserialize_with = "rows")] Vec<Lane>);

fn rows<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Lane>, D::Error> {
    device_automation::bounded_lanes(deserializer, |lane: &Lane| {
        lane.spans
            .iter()
            .fold(0usize, |sum, span| sum.saturating_add(span.steps.len()))
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Span {
    start_raw: u64,
    end_raw: u64,
    steps: Vec<Step>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    beat_raw: u64,
    value: f32,
}

pub(crate) fn contains(project: &Project) -> bool {
    project.automation_devices().any(|device| {
        device
            .automation()
            .iter()
            .any(|lane| !lane.touch.is_empty())
    })
}

pub(crate) fn encode(project: &Project) -> Result<Value, StorageError> {
    let mut snapshot = project.snapshot();
    let mut rows = Vec::new();
    for device in device_automation::devices(&mut snapshot) {
        for lane in &mut device.automation {
            if !lane.touch.is_empty() {
                rows.push((
                    (device.id, lane.target),
                    Lane {
                        device: device.id,
                        target: Target::from_target(lane.target)?,
                        read_selection: lane.read_selection.into(),
                        spans: std::mem::take(&mut lane.touch)
                            .into_iter()
                            .map(|span| Span {
                                start_raw: span.start.raw(),
                                end_raw: span.end.raw(),
                                steps: span
                                    .steps
                                    .into_iter()
                                    .map(|step| Step {
                                        beat_raw: step.beat.raw(),
                                        value: step.value.get(),
                                    })
                                    .collect(),
                            })
                            .collect(),
                    },
                ));
            }
        }
        device.automation.retain(|lane| !lane.points.is_empty());
    }
    rows.sort_unstable_by_key(|(key, _)| *key);
    let core = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let mut value = super::vst3_automation::encode(&core)?;
    value["version"] = DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION.into();
    value["device_automation_touch"] =
        serde_json::to_value(rows.into_iter().map(|(_, lane)| lane).collect::<Vec<_>>())?;
    Ok(value)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    let mut value = super::audio_events::parse_unique(bytes)?;
    let Rows(rows) = serde_json::from_value(
        value
            .as_object_mut()
            .and_then(|root| root.remove("device_automation_touch"))
            .ok_or(StorageError::Invalid(
                "device automation touch inventory is missing",
            ))?,
    )?;
    if rows.is_empty() || rows.len() > hex_project::MAX_DEVICE_AUTOMATION_POINTS {
        return Err(StorageError::Invalid(
            "device automation touch inventory is outside its bound",
        ));
    }
    let core = super::vst3_automation::decode_value(value, false)?;
    let mut snapshot = core.snapshot();
    let mut by_device = BTreeMap::<u64, Vec<DeviceAutomation>>::new();
    let mut previous = None;
    for row in rows {
        let target = row.target.into_target()?;
        let key = (row.device, target);
        if previous.is_some_and(|previous| previous >= key)
            || row.spans.is_empty()
            || row.spans.len() > hex_project::MAX_DEVICE_AUTOMATION_TOUCH_SPANS
        {
            return Err(StorageError::Invalid(
                "device automation touch inventory is invalid",
            ));
        }
        previous = Some(key);
        let mut points = 0usize;
        let touch = row
            .spans
            .into_iter()
            .map(|span| {
                points = points
                    .checked_add(span.steps.len())
                    .filter(|&n| n <= hex_project::MAX_DEVICE_AUTOMATION_POINTS_PER_LANE)
                    .ok_or(StorageError::Invalid(
                        "device automation touch points exceed the lane budget",
                    ))?;
                let steps = span
                    .steps
                    .into_iter()
                    .map(|step| {
                        Ok(DeviceAutomationTouchStep {
                            beat: BeatTime::from_raw(step.beat_raw),
                            value: AutomationValue::new(step.value)
                                .map_err(|error| StorageError::Invalid(error.0))?,
                        })
                    })
                    .collect::<Result<Vec<_>, StorageError>>()?;
                Ok(DeviceAutomationTouchSpan {
                    start: BeatTime::from_raw(span.start_raw),
                    end: BeatTime::from_raw(span.end_raw),
                    steps,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        by_device
            .entry(row.device)
            .or_default()
            .push(DeviceAutomation {
                read_selection: row.read_selection.into(),
                target,
                vst3_class: None,
                points: Vec::new(),
                touch,
            });
    }
    for device in device_automation::devices(&mut snapshot) {
        let Some(lanes) = by_device.remove(&device.id) else {
            continue;
        };
        for row in lanes {
            match device
                .automation
                .binary_search_by_key(&row.target, |lane| lane.target)
            {
                Ok(index) => {
                    let base = &mut device.automation[index];
                    if base.read_selection != row.read_selection {
                        return Err(StorageError::Invalid(
                            "device automation touch and base read selections differ",
                        ));
                    }
                    base.touch = row.touch;
                }
                Err(index) => device.automation.insert(index, row),
            }
        }
    }
    if !by_device.is_empty() {
        return Err(StorageError::Invalid(
            "device automation touch device no longer exists",
        ));
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    use hex_project::{Command, Created, Editor};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn touch_only_lane_round_trips_and_older_project_keeps_original_layout() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("test.recorded"),
                    1,
                    vec![7],
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        let legacy = crate::encode(editor.project()).unwrap();
        assert_eq!(
            crate::schema_version_for(editor.project()),
            crate::CURRENT_SCHEMA_VERSION
        );
        assert_eq!(crate::decode(&legacy).unwrap(), *editor.project());
        let target = DeviceAutomationTarget::HostInputGain;
        editor
            .execute(Command::SpliceDeviceAutomationTouch {
                device,
                target,
                spans: vec![DeviceAutomationTouchSpan {
                    start: BeatTime::ZERO,
                    end: BeatTime::ONE,
                    steps: vec![DeviceAutomationTouchStep {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.25).unwrap(),
                    }],
                }],
            })
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "hex-touch-roundtrip-{}-{}.hex",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        crate::files::save_new(&path, editor.project()).unwrap();
        let reopened = crate::files::load(&path);
        std::fs::remove_file(path).unwrap();
        let reopened = reopened.unwrap();
        assert_eq!(&reopened, editor.project());
        assert_eq!(
            crate::schema_version_for(&reopened),
            DEVICE_AUTOMATION_TOUCH_SCHEMA_VERSION
        );
        let bytes = crate::encode(&reopened).unwrap();
        let mut corrupted: Value = serde_json::from_slice(&bytes).unwrap();
        corrupted["device_automation_touch"][0]["spans"][0]["end_raw"] = 0.into();
        assert!(crate::decode(&serde_json::to_vec(&corrupted).unwrap()).is_err());
        assert!(editor.undo());
        assert_eq!(crate::encode(editor.project()).unwrap(), legacy);
        assert!(editor.redo());
        assert_eq!(crate::encode(editor.project()).unwrap(), bytes);
    }

    #[test]
    fn schema77_vst3_and_native_curve_survive_a_mixed_touch_round_trip() {
        use hex_project::{AutomationInterpolation, AutomationPoint};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let state = hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new([9; 16]).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [6; 32],
                    info_sha256: [7; 32],
                },
            },
            component: Some(&[42; 12]),
            controller: Some(&[]),
        })
        .unwrap();
        let Created::Device(foreign) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("foreign device");
        };
        let Created::Device(native) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("test.recorded"),
                    1,
                    vec![7],
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("native device");
        };
        let target = DeviceAutomationTarget::HostOutputGain;
        for (device, target) in [
            (foreign, DeviceAutomationTarget::Vst3 { parameter: 0 }),
            (native, target),
        ] {
            editor
                .execute(Command::ReplaceDeviceAutomationTarget {
                    device,
                    target,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.25).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
        }
        let previous = crate::encode(editor.project()).unwrap();
        assert_eq!(
            crate::schema_version_for(editor.project()),
            crate::VST3_AUTOMATION_SCHEMA_VERSION
        );
        assert_eq!(crate::decode(&previous).unwrap(), *editor.project());
        editor
            .execute(Command::SpliceDeviceAutomationTouch {
                device: native,
                target,
                spans: vec![DeviceAutomationTouchSpan {
                    start: BeatTime::ZERO,
                    end: BeatTime::ONE,
                    steps: vec![DeviceAutomationTouchStep {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.875).unwrap(),
                    }],
                }],
            })
            .unwrap();
        let reopened = crate::decode(&crate::encode(editor.project()).unwrap()).unwrap();
        assert_eq!(&reopened, editor.project());
        assert!(editor.undo());
        assert_eq!(crate::encode(editor.project()).unwrap(), previous);
    }
}
