use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{Command, DeviceComparisonAction, DeviceVariant, Editor, Project};
use hex_storage::{decode, encode};
use serde_json::json;

fn authored() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let state = NativeDeviceState::new(DeviceTypeId::from_static("hex.utility"), 1, vec![1, 2, 3])
        .unwrap()
        .with_input_gain_millidb(125)
        .unwrap()
        .with_output_gain_millidb(-250)
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let id = editor.project().main_devices()[0].id();
    editor
        .execute(Command::CompareDevice {
            device: id,
            expected: DeviceVariant::A,
            action: DeviceComparisonAction::Switch,
        })
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: id,
            state: NativeDeviceState::new(
                DeviceTypeId::from_static("hex.utility"),
                1,
                vec![9, 8, 7],
            )
            .unwrap()
            .with_input_gain_millidb(125)
            .unwrap()
            .with_output_gain_millidb(-250)
            .unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device: id,
            bypassed: true,
        })
        .unwrap();
    editor.project().clone()
}
#[test]
fn current_main_inventory_roundtrips_order_banks_trims_and_stable_identity() {
    let project = authored();
    let bytes = encode(&project).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(document["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(document["main_devices"].as_array().unwrap().len(), 1);
    let reopened = decode(&bytes).unwrap();
    assert_eq!(reopened.snapshot(), project.snapshot());
    let device = &reopened.main_devices()[0];
    assert_eq!(device.active_variant(), DeviceVariant::B);
    assert_eq!(device.state().payload(), &[9, 8, 7]);
    assert_eq!(
        device.comparison().unwrap().inactive().payload(),
        &[1, 2, 3]
    );
    assert_eq!(device.state().input_gain_millidb(), 125);
    assert!(device.bypassed());
    assert!(reopened.tracks().is_empty());
    assert!(hex_storage::encode_schema22(&project).is_err());
    assert!(hex_storage::encode_schema20(&project).is_err());
}
#[test]
fn main_inventory_is_required_strict_bounded_and_part_of_exact_manifest() {
    let value: serde_json::Value = serde_json::from_slice(&encode(&authored()).unwrap()).unwrap();
    for mutation in 0..7 {
        let mut bad = value.clone();
        match mutation {
            0 => {
                bad.as_object_mut().unwrap().remove("main_devices");
            }
            1 => bad["main_devices"] = json!(null),
            2 => bad["main_devices"][0]["id"] = json!(0),
            3 => bad["main_devices"] = json!(vec![value["main_devices"][0].clone(); 65]),
            4 => bad["main_devices"][0]["unknown"] = json!(true),
            5 => bad["device_assets"] = json!([]),
            6 => bad["version"] = json!(32),
            _ => unreachable!(),
        }
        assert!(
            decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "mutation {mutation}"
        );
    }
}
