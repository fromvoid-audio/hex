//! Schema50 adds a required canonical device-automation inventory to schema49.
//! Old schemas migrate in memory with no authored device envelopes. This wrapper
//! strips only its owned inventory before invoking the frozen49 codec; no files
//! or media are modified. Unknown/deleted parameters retain their stable identity
//! and normalized points for provider admission to validate without retargeting.
use crate::{Project, StorageError};
use hex_lattice_contract::ParameterId;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, DeviceAutomation, DeviceSnapshot,
    Snapshot,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Point {
    pub(super) beat_raw: u64,
    pub(super) value: f32,
    pub(super) outgoing: Interpolation,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Interpolation {
    Step,
    Linear,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lane {
    device: u64,
    parameter: u32,
    #[serde(deserialize_with = "points")]
    points: Vec<Point>,
}
#[derive(Deserialize)]
struct Lanes(#[serde(deserialize_with = "lanes")] Vec<Lane>);
fn bounded<'de, D, T>(d: D, limit: usize) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Items<T>(usize, std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Items<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded device automation inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == self.0 {
                    return Err(serde::de::Error::custom(
                        "device automation inventory limit reached",
                    ));
                }
                values.try_reserve(1).map_err(serde::de::Error::custom)?;
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Items(limit, std::marker::PhantomData))
}
pub(super) fn points<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Point>, D::Error> {
    bounded(d, hex_project::MAX_DEVICE_AUTOMATION_POINTS_PER_LANE)
}
fn lanes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Lane>, D::Error> {
    bounded_lanes(d, |lane: &Lane| lane.points.len())
}

pub(super) fn bounded_lanes<'de, D, T>(
    d: D,
    point_count: impl Fn(&T) -> usize,
) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Inventory<T, F>(F, std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, F: Fn(&T) -> usize> serde::de::Visitor<'de> for Inventory<T, F> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded device automation points")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut rows = Vec::new();
            let mut count = 0usize;
            while let Some(row) = seq.next_element::<T>()? {
                count = count
                    .checked_add(self.0(&row))
                    .ok_or_else(|| serde::de::Error::custom("automation point count overflow"))?;
                if rows.len() == hex_project::MAX_DEVICE_AUTOMATION_POINTS
                    || count > hex_project::MAX_DEVICE_AUTOMATION_POINTS
                {
                    return Err(serde::de::Error::custom(
                        "project device automation point limit reached",
                    ));
                }
                rows.try_reserve(1).map_err(serde::de::Error::custom)?;
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(Inventory(point_count, std::marker::PhantomData))
}
/// Visit only canonical envelope owners. Track instrument projections remain
/// empty even when many bindings reference the same retained source envelope.
pub(super) fn devices(snapshot: &mut Snapshot) -> impl Iterator<Item = &mut DeviceSnapshot> {
    snapshot
        .tracks
        .iter_mut()
        .flat_map(|track| track.devices.iter_mut())
        .chain(
            snapshot
                .return_tracks
                .iter_mut()
                .flat_map(|track| track.devices.iter_mut()),
        )
        .chain(snapshot.main_devices.iter_mut())
        .chain(
            snapshot
                .native_sources
                .iter_mut()
                .flat_map(|source| source.devices.iter_mut()),
        )
}
pub(crate) fn encode(project: &Project) -> Result<serde_json::Value, StorageError> {
    super::automation_read::require_authored(project)?;
    let mut rows = Vec::new();
    for device in project.automation_devices() {
        for lane in device.automation() {
            rows.push(Lane {
                device: device.id().value(),
                parameter: lane
                    .target
                    .provider_parameter()
                    .ok_or(StorageError::Invalid(
                        "older schema cannot save host automation targets",
                    ))?
                    .value(),
                points: lane
                    .points
                    .iter()
                    .map(|point| Point {
                        beat_raw: point.beat.raw(),
                        value: point.value.get(),
                        outgoing: match point.outgoing {
                            AutomationInterpolation::Step => Interpolation::Step,
                            AutomationInterpolation::Linear => Interpolation::Linear,
                        },
                    })
                    .collect(),
            });
        }
    }
    rows.sort_by_key(|row| (row.device, row.parameter));
    let mut snapshot = project.snapshot();
    for device in devices(&mut snapshot) {
        device.automation.clear();
    }
    let core = Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))?;
    let mut value = super::session_audio_loops::encode(&core)?;
    value["version"] = 50.into();
    value["device_automation"] = serde_json::to_value(rows)?;
    Ok(value)
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_value(super::audio_events::parse_unique(bytes)?)
}
/// Decode an envelope parsed once by [`super::audio_events::parse_unique`].
pub(crate) fn decode_value(mut value: serde_json::Value) -> Result<Project, StorageError> {
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid device automation document"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(50) {
        return Err(StorageError::Invalid("invalid device automation schema"));
    }
    let Lanes(rows) = serde_json::from_value(
        object
            .remove("device_automation")
            .ok_or(StorageError::Invalid("device automation inventory missing"))?,
    )?;
    object.insert("version".into(), 49.into());
    let core = super::session_audio_loops::decode49_value(value)?;
    let mut snapshot = core.snapshot();
    let known: std::collections::BTreeSet<_> = core
        .automation_devices()
        .map(|device| device.id().value())
        .collect();
    let mut authored: std::collections::BTreeMap<u64, Vec<DeviceAutomation>> =
        std::collections::BTreeMap::new();
    let mut previous = None;
    let mut count = 0usize;
    for row in rows {
        let key = (row.device, row.parameter);
        if previous.is_some_and(|old| old >= key) {
            return Err(StorageError::Invalid(
                "device automation targets must strictly increase",
            ));
        }
        previous = Some(key);
        count = count
            .checked_add(row.points.len())
            .ok_or(StorageError::Invalid("automation point count overflow"))?;
        if count > hex_project::MAX_DEVICE_AUTOMATION_POINTS {
            return Err(StorageError::Invalid(
                "project device automation point limit reached",
            ));
        }
        let parameter = ParameterId::new(row.parameter)
            .map_err(|_| StorageError::Invalid("invalid automation parameter identity"))?;
        let points = row
            .points
            .into_iter()
            .map(|point| {
                Ok(AutomationPoint {
                    beat: hex_project::BeatTime::from_raw(point.beat_raw),
                    value: AutomationValue::new(point.value)
                        .map_err(|e| StorageError::Invalid(e.0))?,
                    outgoing: match point.outgoing {
                        Interpolation::Step => AutomationInterpolation::Step,
                        Interpolation::Linear => AutomationInterpolation::Linear,
                    },
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let lane = DeviceAutomation {
            read_selection: hex_project::AutomationReadSelection::Authored,
            target: hex_lattice_contract::DeviceAutomationTarget::Parameter(parameter),
            points,
        };
        hex_project::validate_device_automation(std::slice::from_ref(&lane))
            .map_err(|e| StorageError::Invalid(e.0))?;
        if !known.contains(&row.device) {
            return Err(StorageError::Invalid("automation device no longer exists"));
        }
        authored.entry(row.device).or_default().push(lane);
    }
    for device in devices(&mut snapshot) {
        if let Some(lanes) = authored.get(&device.id) {
            device.automation = lanes.clone();
        }
    }
    Project::from_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};
    fn fixture() -> Project {
        let mut e = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = e
            .execute(Command::AddTrack {
                name: "automation".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let state = hex_project::default_instrument_state().unwrap();
        let Created::Device(device) = e
            .execute(Command::InsertInstrument {
                track,
                state: state.clone(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let Created::Device(main) = e
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("main")
        };
        for device in [device, main] {
            e.execute(Command::ReplaceDeviceAutomation {
                device,
                parameter: ParameterId::from_static(7),
                points: vec![
                    AutomationPoint {
                        beat: hex_project::BeatTime::ZERO,
                        value: AutomationValue::new(0.25).unwrap(),
                        outgoing: AutomationInterpolation::Linear,
                    },
                    AutomationPoint {
                        beat: hex_project::BeatTime::ONE,
                        value: AutomationValue::new(1.0).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    },
                ],
            })
            .unwrap();
        }
        let source = e.project().track(track).unwrap().source_id().unwrap();
        for name in ["shared one", "shared two"] {
            let Created::Track(shared) =
                e.execute(Command::AddTrack { name: name.into() }).unwrap()
            else {
                panic!("track")
            };
            e.execute(Command::BindTrackSource {
                track: shared,
                source: Some(source),
            })
            .unwrap();
        }
        e.project().clone()
    }
    #[test]
    fn current_format_preserves_complete_device_and_source_automation() {
        let project = fixture();
        let bytes = crate::encode(&project).unwrap();
        let snapshot = project.snapshot();
        assert_eq!(
            snapshot
                .tracks
                .iter()
                .filter(|track| track.instrument.is_some())
                .count(),
            3
        );
        assert!(
            snapshot
                .tracks
                .iter()
                .filter_map(|track| track.instrument.as_ref())
                .all(|instrument| instrument.automation.is_empty())
        );
        assert_eq!(
            snapshot.native_sources[0].devices[0].automation[0]
                .points
                .len(),
            2
        );
        assert_eq!(crate::decode(&bytes).unwrap().snapshot(), snapshot);
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["version"], crate::CURRENT_SCHEMA_VERSION);
        assert_eq!(
            value["device_automation"].as_array().unwrap().len(),
            2,
            "source projection is serialized once"
        );
    }
    #[test]
    fn legacy_writers_refuse_to_discard_authored_device_envelopes() {
        let mut e = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = e
            .execute(Command::AddTrack {
                name: "legacy".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        e.execute(Command::SetTrackColor {
            track,
            color: hex_project::ElementColor::Gray,
        })
        .unwrap();
        let Created::Device(device) = e
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        crate::encode_schema20(e.project()).unwrap();
        crate::encode_schema22(e.project()).unwrap();
        e.execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::from_static(1),
            points: vec![AutomationPoint {
                beat: hex_project::BeatTime::ZERO,
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
        .unwrap();
        for result in [
            crate::encode_schema20(e.project()),
            crate::encode_schema22(e.project()),
        ] {
            assert!(matches!(
                result,
                Err(StorageError::Invalid(
                    "older schema cannot save device automation"
                ))
            ));
        }
    }
    #[test]
    fn schema49_migrates_without_inventing_authored_envelopes() {
        let mut snapshot = fixture().snapshot();
        for device in devices(&mut snapshot) {
            device.automation.clear();
        }
        let project = Project::from_snapshot(snapshot).unwrap();
        let old_value = super::super::session_audio_loops::encode(&project).unwrap();
        assert_eq!(old_value["version"], 49);
        let old_bytes = serde_json::to_vec(&old_value).unwrap();
        let retained = old_bytes.clone();
        let migrated = crate::decode(&old_bytes).unwrap();
        assert_eq!(migrated, project);
        assert_eq!(
            old_bytes, retained,
            "migration leaves source bytes untouched"
        );
        assert!(!migrated.has_device_automation());
        assert_eq!(
            crate::decode(&crate::encode(&migrated).unwrap()).unwrap(),
            project
        );
    }
    #[test]
    fn typed_decoder_refuses_per_lane_and_aggregate_point_overflow() {
        let mut value = encode(&fixture()).unwrap();
        let mut row = value["device_automation"][0].clone();
        row["points"] = serde_json::Value::Array(
            (0..1024)
                .map(|beat| serde_json::json!({"beat_raw":beat,"value":0.5,"outgoing":"linear"}))
                .collect(),
        );
        value["device_automation"] = serde_json::Value::Array(
            (1..=65)
                .map(|parameter| {
                    let mut lane = row.clone();
                    lane["parameter"] = parameter.into();
                    lane
                })
                .collect(),
        );
        let error = crate::decode(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("project device automation point limit reached")
        );
        row["points"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"beat_raw":1024,"value":0.5,"outgoing":"step"}));
        value["device_automation"] = serde_json::json!([row]);
        let error = crate::decode(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("device automation inventory limit reached")
        );
    }
    #[test]
    fn current_format_refuses_missing_duplicate_stale_and_malformed_envelopes() {
        let value = encode(&fixture()).unwrap();
        let changes: [fn(&mut serde_json::Value); 9] = [
            |v| {
                v.as_object_mut().unwrap().remove("device_automation");
            },
            |v| {
                v["device_automation"][0]["device"] = 999999.into();
            },
            |v| {
                v["device_automation"][0]["parameter"] = 0.into();
            },
            |v| {
                v["device_automation"][0]["points"][0]["value"] = (-0.01).into();
            },
            |v| {
                v["device_automation"][0]["points"][0]["value"] = 1.01.into();
            },
            |v| {
                v["device_automation"][0]["points"] = serde_json::json!([]);
            },
            |v| {
                v["device_automation"][0]["points"][1]["beat_raw"] = 0.into();
            },
            |v| {
                v["device_automation"][0]["unknown"] = true.into();
            },
            |v| {
                let duplicate = v["device_automation"][0].clone();
                v["device_automation"]
                    .as_array_mut()
                    .unwrap()
                    .insert(0, duplicate);
            },
        ];
        for change in changes {
            let mut malformed = value.clone();
            change(&mut malformed);
            assert!(crate::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
        }
        let bytes = serde_json::to_string(&value).unwrap();
        let duplicate_key = bytes.replacen(
            "\"device_automation\":",
            "\"device_automation\":[],\"device_automation\":",
            1,
        );
        assert!(crate::decode(duplicate_key.as_bytes()).is_err());
    }
}
