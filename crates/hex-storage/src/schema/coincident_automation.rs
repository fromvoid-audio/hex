//! Schema115 retains the incoming points of vertical device automation transitions.
//! Its base is an exact older document containing the outgoing points. Older
//! documents retain their strict single-point-per-beat contract.

use crate::{COINCIDENT_AUTOMATION_SCHEMA_VERSION, Project, StorageError};
use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<'a> {
    format: &'a str,
    version: u32,
    #[serde(borrow)]
    base: &'a RawValue,
    #[serde(deserialize_with = "bounded_rows")]
    incoming: Vec<Row>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    device: u64,
    lane: usize,
    beat_raw: u64,
    value: f32,
    step: bool,
}

fn bounded_rows<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Row>, D::Error> {
    super::device_automation::bounded_lanes(d, |_: &Row| 1)
}

pub(crate) fn contains(project: &Project) -> bool {
    project.automation_devices().any(|device| {
        device.automation().iter().any(|lane| {
            lane.points
                .windows(2)
                .any(|pair| pair[0].beat == pair[1].beat)
        })
    })
}

pub(crate) fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    let mut snapshot = project.snapshot();
    let mut incoming = Vec::new();
    for device in super::device_automation::devices(&mut snapshot) {
        for (lane_index, lane) in device.automation.iter_mut().enumerate() {
            let mut points = std::mem::take(&mut lane.points).into_iter().peekable();
            while let Some(point) = points.next() {
                if points.peek().is_some_and(|next| next.beat == point.beat) {
                    incoming.push(Row {
                        device: device.id,
                        lane: lane_index,
                        beat_raw: point.beat.raw(),
                        value: point.value.get(),
                        step: point.outgoing == AutomationInterpolation::Step,
                    });
                } else {
                    lane.points.push(point);
                }
            }
        }
    }
    let prior = Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))?;
    let base = crate::encode(&prior)?;
    crate::encode_document(&Document {
        format: "hex-project",
        version: COINCIDENT_AUTOMATION_SCHEMA_VERSION,
        base: serde_json::from_slice(&base)?,
        incoming,
    })
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.len() > crate::MAX_PROJECT_DOCUMENT_BYTES {
        return Err(StorageError::Invalid(
            "project exceeds the document size limit",
        ));
    }
    let document: Document<'_> = serde_json::from_slice(bytes)?;
    #[derive(Deserialize)]
    struct Header {
        version: u32,
    }
    let base = document.base.get().as_bytes();
    let header: Header = serde_json::from_slice(base)?;
    if header.version >= COINCIDENT_AUTOMATION_SCHEMA_VERSION || document.incoming.is_empty() {
        return Err(StorageError::Invalid(
            "invalid coincident automation envelope",
        ));
    }
    let mut incoming = BTreeMap::new();
    for row in document.incoming {
        let point = AutomationPoint {
            beat: BeatTime::from_raw(row.beat_raw),
            value: AutomationValue::new(row.value)
                .map_err(|error| StorageError::Invalid(error.0))?,
            outgoing: if row.step {
                AutomationInterpolation::Step
            } else {
                AutomationInterpolation::Linear
            },
        };
        if incoming
            .insert((row.device, row.lane, point.beat), point)
            .is_some()
        {
            return Err(StorageError::Invalid("duplicate incoming automation point"));
        }
    }
    let mut snapshot = crate::decode(base)?.snapshot();
    for device in super::device_automation::devices(&mut snapshot) {
        for (lane_index, lane) in device.automation.iter_mut().enumerate() {
            let first = (device.id, lane_index, BeatTime::ZERO);
            let last = (device.id, lane_index, BeatTime::from_raw(u64::MAX));
            if incoming.range(first..=last).next().is_none() {
                continue;
            }
            for point in std::mem::take(&mut lane.points) {
                if let Some(before) = incoming.remove(&(device.id, lane_index, point.beat)) {
                    lane.points.push(before);
                }
                lane.points.push(point);
            }
        }
    }
    if !incoming.is_empty() {
        return Err(StorageError::Invalid(
            "incoming automation point has no matching outgoing point",
        ));
    }
    Project::from_snapshot(snapshot).map_err(|error| StorageError::Invalid(error.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
    use hex_project::{Command, Created, DeviceAutomationTarget, Editor};

    #[test]
    fn shared_beat_round_trips_over_an_unchanged_historical_envelope() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "automation".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("test.automation"),
                    1,
                    vec![1],
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        let target = DeviceAutomationTarget::HostInputGain;
        let outgoing = AutomationPoint {
            beat: BeatTime::ONE,
            value: AutomationValue::new(0.2).unwrap(),
            outgoing: AutomationInterpolation::Linear,
        };
        let edit = |points| Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points,
        };
        editor.execute(edit(vec![outgoing])).unwrap();
        editor
            .execute(Command::AddArrangementLocator { frame: 500 })
            .unwrap();
        let prior = editor.project().clone();
        let prior_bytes = crate::encode(&prior).unwrap();
        assert_eq!(
            crate::schema_version_for(&prior),
            crate::ARRANGEMENT_LOCATOR_SCHEMA_VERSION
        );
        assert_eq!(crate::decode(&prior_bytes).unwrap(), prior);

        let incoming = AutomationPoint {
            value: AutomationValue::new(0.9).unwrap(),
            outgoing: AutomationInterpolation::Step,
            ..outgoing
        };
        editor.execute(edit(vec![incoming, outgoing])).unwrap();
        let expected = editor.project();
        assert_eq!(
            crate::schema_version_for(expected),
            COINCIDENT_AUTOMATION_SCHEMA_VERSION
        );
        let bytes = crate::encode(expected).unwrap();
        let document: Document<'_> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(document.base.get().as_bytes(), prior_bytes);
        assert_eq!(crate::decode(&bytes).unwrap(), *expected);

        let folder =
            std::env::temp_dir().join(format!("hex-coincident-automation-{}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        let old_path = folder.join("prior.hex");
        std::fs::write(&old_path, &prior_bytes).unwrap();
        let (_, token) = crate::load_with_token(&old_path).unwrap();
        assert!(token.requires_save_confirmation(expected).unwrap());
        let new_path = folder.join("vertical.hex");
        let loaded = crate::save_new(&new_path, expected).and_then(|()| crate::load(&new_path));
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(loaded.unwrap(), *expected);

        let mut invalid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        invalid["incoming"][0]["beat_raw"] = 0.into();
        assert!(crate::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
}
