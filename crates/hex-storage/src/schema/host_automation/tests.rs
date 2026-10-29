use super::*;
use hex_project::{BeatTime, Command, Created, Editor};
fn fixture() -> Project {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "host".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
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
    for target in [
        DeviceAutomationTarget::Parameter(hex_lattice_contract::ParameterId::from_static(1)),
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ] {
        e.execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points: vec![
                AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.5).unwrap(),
                    outgoing: AutomationInterpolation::Linear,
                },
                AutomationPoint {
                    beat: BeatTime::ONE,
                    value: AutomationValue::new(1.0).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();
    }
    e.project().clone()
}
#[test]
fn current_typed_targets_roundtrip_and_frozen_writers_refuse_host_information() {
    let project = fixture();
    let value = encode(&project).unwrap();
    assert_eq!(value["version"], 53);
    assert_eq!(value["device_automation"].as_array().unwrap().len(), 1);
    assert_eq!(value["host_automation"].as_array().unwrap().len(), 2);
    assert_eq!(
        crate::decode(&crate::encode(&project).unwrap()).unwrap(),
        project
    );
    assert!(super::super::clip_aliases::encode(&project).is_err());
    assert!(super::super::device_automation::encode(&project).is_err());
    let mut snapshot = project.snapshot();
    for device in devices(&mut snapshot) {
        device.automation.retain(|l| !l.target.is_host());
    }
    let previous = Project::from_snapshot(snapshot).unwrap();
    let old = super::super::clip_aliases::encode(&previous).unwrap();
    let bytes = serde_json::to_vec(&old).unwrap();
    let unchanged = bytes.clone();
    let migrated = crate::decode(&bytes).unwrap();
    assert_eq!(migrated, previous);
    assert_eq!(bytes, unchanged);
    assert_eq!(
        crate::decode(&crate::encode(&migrated).unwrap()).unwrap(),
        previous
    );
}
#[test]
fn host_inventory_is_required_strict_typed_ordered_bounded_and_identity_checked() {
    let value = encode(&fixture()).unwrap();
    let mutations: [fn(&mut serde_json::Value); 9] = [
        |v| {
            v.as_object_mut().unwrap().remove("host_automation");
        },
        |v| v["host_automation"][0]["device"] = 999999.into(),
        |v| v["host_automation"][0]["target"] = "parameter".into(),
        |v| v["host_automation"][0]["parameter"] = 1.into(),
        |v| v["host_automation"][0]["points"] = serde_json::json!([]),
        |v| v["host_automation"][0]["points"][0]["value"] = (-0.1).into(),
        |v| v["host_automation"][0]["points"][1]["beat_raw"] = 0.into(),
        |v| {
            let duplicate = v["host_automation"][0].clone();
            v["host_automation"]
                .as_array_mut()
                .unwrap()
                .insert(0, duplicate);
        },
        |v| {
            let point = v["host_automation"][0]["points"][0].clone();
            v["host_automation"][0]["points"] = serde_json::Value::Array(vec![point; 1025]);
        },
    ];
    for mutate in mutations {
        let mut malformed = value.clone();
        mutate(&mut malformed);
        assert!(crate::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
    }
    let text = serde_json::to_string(&value).unwrap();
    let duplicate = text.replacen(
        "\"host_automation\":",
        "\"host_automation\":[],\"host_automation\":",
        1,
    );
    assert!(crate::decode(duplicate.as_bytes()).is_err());
}
