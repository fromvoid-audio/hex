use super::*;
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState, ParameterId};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceComparisonAction, DeviceVariant, Editor,
};

fn fixture() -> Project {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "read selection".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let state = NativeDeviceState::new(
        DeviceTypeId::from_static("test.read-selection"),
        17,
        vec![0, 1, 255],
    )
    .unwrap();
    let Created::Device(effect) = e
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("effect")
    };
    let Created::Device(instrument) = e
        .execute(Command::InsertInstrument {
            track,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("instrument")
    };
    let source = e.project().track(track).unwrap().source_id().unwrap();
    let Created::Track(alias) = e
        .execute(Command::AddTrack {
            name: "shared".into(),
        })
        .unwrap()
    else {
        panic!("alias")
    };
    e.execute(Command::BindTrackSource {
        track: alias,
        source: Some(source),
    })
    .unwrap();
    let Created::Device(main) = e
        .execute(Command::InsertMainDevice {
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("main")
    };
    let Created::ReturnTrack(bus) = e
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::Device(ret) = e
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("return effect")
    };
    for device in [effect, instrument, main, ret] {
        e.execute(Command::CompareDevice {
            device,
            expected: DeviceVariant::A,
            action: DeviceComparisonAction::Switch,
        })
        .unwrap();
        for target in [
            DeviceAutomationTarget::Parameter(ParameterId::from_static(7)),
            DeviceAutomationTarget::HostInputGain,
            DeviceAutomationTarget::HostOutputGain,
        ] {
            e.execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target,
                points: vec![
                    AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.25).unwrap(),
                        outgoing: AutomationInterpolation::Linear,
                    },
                    AutomationPoint {
                        beat: BeatTime::ONE,
                        value: AutomationValue::new(0.75).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    },
                ],
            })
            .unwrap();
        }
    }
    let mut snapshot = e.project().snapshot();
    for (index, device) in devices(&mut snapshot).enumerate() {
        for (lane_index, lane) in device.automation.iter_mut().enumerate() {
            lane.read_selection = if (index + lane_index) % 2 == 0 {
                AutomationReadSelection::Base
            } else {
                AutomationReadSelection::Authored
            };
        }
    }
    Project::from_snapshot(snapshot).unwrap()
}
fn all_authored(project: &Project) -> Project {
    let mut snapshot = project.snapshot();
    for device in devices(&mut snapshot) {
        for lane in &mut device.automation {
            lane.read_selection = AutomationReadSelection::Authored;
        }
    }
    Project::from_snapshot(snapshot).unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

#[test]
fn complete_selection_roundtrip_keeps_points_comparisons_and_single_source_owner() {
    let project = fixture();
    let value = encode(&project).unwrap();
    assert_eq!(value["version"], 55);
    assert_eq!(value["device_automation"].as_array().unwrap().len(), 4);
    assert_eq!(value["host_automation"].as_array().unwrap().len(), 8);
    assert_eq!(
        crate::decode(&crate::encode(&project).unwrap()).unwrap(),
        project
    );
    assert!(project.has_overridden_device_automation());
    let snapshot = project.snapshot();
    assert_eq!(snapshot.native_sources.len(), 1);
    assert_eq!(
        snapshot
            .tracks
            .iter()
            .filter(|t| t.instrument.is_some())
            .count(),
        2
    );
    assert!(
        snapshot
            .tracks
            .iter()
            .filter_map(|t| t.instrument.as_ref())
            .all(|d| d.automation.is_empty())
    );
    assert!(
        project
            .automation_devices()
            .all(|d| d.comparison().is_some())
    );
    // Point edits keep read selection; normal history and serialization retain it.
    let device = project.tracks()[0].devices()[0].id();
    let target = DeviceAutomationTarget::Parameter(ParameterId::from_static(7));
    let original = project
        .device(device)
        .unwrap()
        .automation_target(target)
        .unwrap()
        .clone();
    assert_eq!(original.read_selection, AutomationReadSelection::Base);
    let mut e = Editor::new(project.clone(), 8).unwrap();
    let mut points = original.points.clone();
    points[0].value = AutomationValue::new(0.5).unwrap();
    e.execute(Command::ReplaceDeviceAutomationTarget {
        device,
        target,
        points,
    })
    .unwrap();
    assert_eq!(
        e.project()
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .read_selection,
        AutomationReadSelection::Base
    );
    assert_eq!(
        crate::decode(&crate::encode(e.project()).unwrap()).unwrap(),
        *e.project()
    );
    assert!(e.undo());
    assert_eq!(e.project(), &project);
    assert!(e.redo());
    assert_eq!(
        crate::decode(&crate::encode(e.project()).unwrap()).unwrap(),
        *e.project()
    );
}

#[test]
fn prior53_migrates_authored_in_memory_and_frozen_writers_never_drop_base_selection() {
    let project = fixture();
    for encoded in [
        super::super::host_automation::encode(&project),
        super::super::clip_aliases::encode(&project),
        super::super::device_automation::encode(&project),
    ] {
        assert!(encoded.is_err());
    }
    assert!(crate::encode_schema22(&project).is_err());
    assert!(crate::encode_schema20(&project).is_err());
    let authored = all_authored(&project);
    let old = bytes(&super::super::host_automation::encode(&authored).unwrap());
    let original = old.clone();
    assert_eq!(crate::decode(&old).unwrap(), authored);
    assert_eq!(old, original);
    assert!(
        crate::decode(&old)
            .unwrap()
            .automation_devices()
            .flat_map(|d| d.automation())
            .all(|l| l.read_selection == AutomationReadSelection::Authored)
    );
    // Host-only Base must be refused before53 strips the host inventory.
    let mut snapshot = authored.snapshot();
    for device in devices(&mut snapshot) {
        for lane in &mut device.automation {
            if lane.target.is_host() {
                lane.read_selection = AutomationReadSelection::Base;
            }
        }
    }
    assert!(
        super::super::host_automation::encode(&Project::from_snapshot(snapshot).unwrap()).is_err()
    );
}

#[test]
fn current_read_selection_is_required_typed_unique_and_strict_on_both_row_kinds() {
    let valid = encode(&fixture()).unwrap();
    for inventory in ["device_automation", "host_automation"] {
        for malformed in [
            Value::Null,
            Value::Bool(false),
            Value::String("off".into()),
            Value::Number(0.into()),
        ] {
            let mut value = valid.clone();
            value[inventory][0]["read_selection"] = malformed;
            assert!(crate::decode(&bytes(&value)).is_err());
        }
        let mut value = valid.clone();
        value[inventory][0]
            .as_object_mut()
            .unwrap()
            .remove("read_selection");
        assert!(crate::decode(&bytes(&value)).is_err());
        let mut value = valid.clone();
        value[inventory][0]["unknown"] = true.into();
        assert!(crate::decode(&bytes(&value)).is_err());
        let mut value = valid.clone();
        value[inventory][0]["device"] = 999999.into();
        assert!(crate::decode(&bytes(&value)).is_err());
        // Authored-only reads also validate canonical ownership before taking
        // the no-selection-change path.
        let mut value = encode(&all_authored(&fixture())).unwrap();
        value[inventory][0]["device"] = 999999.into();
        assert!(crate::decode(&bytes(&value)).is_err());
    }
    let text = serde_json::to_string(&valid).unwrap();
    let duplicate = text.replacen(
        "\"read_selection\":",
        "\"read_selection\":\"authored\",\"read_selection\":",
        1,
    );
    assert!(crate::decode(duplicate.as_bytes()).is_err());
    let mut branch54 = valid;
    branch54["version"] = 54.into();
    let error = crate::decode(&bytes(&branch54)).unwrap_err().to_string();
    assert!(error.contains("matching hex build") && error.contains("original file unchanged"));
}

#[test]
fn save_reopen_preserves_selection_and_prior53_tokens_require_upgrade_without_writes() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "hex-read-selection-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    let current = root.join("current.hex");
    let prior = root.join("prior.hex");
    let project = fixture();
    crate::save_new(&current, &project).unwrap();
    let (reopened, token) = crate::load_with_token(&current).unwrap();
    assert_eq!(reopened, project);
    assert!(!token.requires_format_upgrade());
    let authored = all_authored(&project);
    let old = bytes(&super::super::host_automation::encode(&authored).unwrap());
    std::fs::write(&prior, &old).unwrap();
    let (migrated, token) = crate::load_with_token(&prior).unwrap();
    assert_eq!(migrated, authored);
    assert!(token.requires_format_upgrade());
    assert_eq!(std::fs::read(&prior).unwrap(), old);
    std::fs::remove_dir_all(root).unwrap();
}
