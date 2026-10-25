use crate::{decode, encode, load, load_with_token, save_new};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use hex_project::{Command, Created, Editor, Project, default_instrument_state};

fn fixture() -> Project {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "Original Sample.wav".into(),
            sample_rate: 44100,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let instrument = default_instrument_state()
        .unwrap()
        .with_input_gain_millidb(1500)
        .unwrap()
        .with_output_gain_millidb(-3000)
        .unwrap();
    let reference = DeviceAssetReference {
        slot: DeviceAssetSlot::new(1).unwrap(),
        asset: DeviceAssetKey::new(asset.value()).unwrap(),
    };
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "Preserved Source".into(),
            devices: vec![instrument.clone(), instrument.clone()],
            instrument_index: 1,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let device = editor.project().native_source(source).unwrap().devices()[1].id();
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceComparison {
            device,
            state: instrument.clone(),
            comparison: Some(hex_project::DeviceComparison::new(
                hex_project::DeviceVariant::B,
                instrument.with_asset_references(vec![reference]).unwrap(),
            )),
        })
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "existing track".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: default_instrument_state().unwrap(),
        })
        .unwrap();
    editor.project().clone()
}
#[test]
fn current_source_roundtrip_preserves_complete_chain_host_metadata_comparison_and_resources() {
    let project = fixture();
    let bytes = encode(&project).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        45
    );
    assert_eq!(decode(&bytes).unwrap(), project);
    assert!(
        crate::encode_schema22(&project).is_err(),
        "explicit older encoding must not discard sources"
    );
}
#[test]
fn schema44_migrates_empty_inventory_without_changing_existing_state_or_original_bytes() {
    // Frozen output of the preserved schema44 encoder from baseline3a2ff464,
    // captured before source bindings. No historical source inventory is invented.
    let bytes =
        include_bytes!("../../../tests/fixtures/native-sources/schema44-predecessor.json").to_vec();
    let original = bytes.clone();
    let migrated = decode(&bytes).unwrap();
    assert!(migrated.native_sources().is_empty());
    assert_eq!(
        super::super::track_groups::encode(&migrated).unwrap(),
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
    );
    assert_eq!(bytes, original);
    assert_eq!(decode(&encode(&migrated).unwrap()).unwrap(), migrated);
}
#[test]
fn native_source_envelope_refuses_missing_unknown_duplicate_and_invalid_fields() {
    let project = fixture();
    let value: serde_json::Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    for case in 0..14 {
        let mut bad = value.clone();
        match case {
            0 => {
                bad.as_object_mut().unwrap().remove("native_sources");
            }
            1 => {
                bad["native_sources"] = serde_json::json!(null);
            }
            2 => {
                bad["native_sources"][0]["unknown"] = true.into();
            }
            3 => {
                bad["native_sources"][0]["instrument_index"] = 8.into();
            }
            4 => {
                bad["native_sources"][0]["devices"] = serde_json::json!([]);
            }
            5 => {
                bad["native_sources"][0]["devices"][0]["device"]["id"] =
                    bad["native_sources"][0]["id"].clone();
            }
            6 => {
                bad["native_sources"][0]["devices"][1]
                    .as_object_mut()
                    .unwrap()
                    .remove("comparison");
            }
            7 => {
                bad["native_sources"][0]["devices"][1]["comparison"]["references"][0]["asset"] =
                    99999.into();
            }
            8 => {
                bad["native_sources"][0]["id"] = bad["tracks"][0]["id"].clone();
            }
            9 => {
                bad["native_sources"][0]["name"] = "".into();
            }
            11 => {
                bad["native_sources"][0]["devices"][0]["device"]
                    .as_object_mut()
                    .unwrap()
                    .remove("input_gain_millidb");
            }
            12 => {
                bad["native_sources"][0]["devices"][0]["device"]
                    .as_object_mut()
                    .unwrap()
                    .remove("output_gain_millidb");
            }
            13 => {
                bad["native_sources"][0]["devices"][0]["device"]["payload"] =
                    serde_json::json!(vec![
                        0;
                        hex_lattice_contract::MAX_NATIVE_DEVICE_STATE_BYTES + 1
                    ]);
            }
            _ => {
                bad["native_sources"] =
                    serde_json::Value::Array(vec![
                        bad["native_sources"][0].clone();
                        hex_project::MAX_NATIVE_SOURCES + 1
                    ]);
            }
        }
        assert!(
            decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "case {case}"
        );
    }
    let bytes = String::from_utf8(encode(&project).unwrap()).unwrap();
    let duplicated = bytes.replacen(
        "\"native_sources\":",
        "\"native_sources\":[],\"native_sources\":",
        1,
    );
    assert!(decode(duplicated.as_bytes()).is_err());
}
#[test]
fn saved_unassigned_source_reopens_without_history_and_failed_publication_preserves_original() {
    let project = fixture();
    let dir = std::env::temp_dir().join(format!("hex-native-sources-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("retained.hex");
    save_new(&path, &project).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let (loaded, token) = load_with_token(&path).unwrap();
    assert_eq!(loaded, project);
    let mut editor = Editor::new(loaded, 8).unwrap();
    assert!(!editor.can_undo(), "retention must not rely on history");
    let source = editor.project().native_sources()[0].id();
    editor
        .execute(Command::RenameNativeSource {
            source,
            name: "Changed".into(),
        })
        .unwrap();
    assert!(save_new(&path, editor.project()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let mut foreign = bytes.clone();
    foreign.push(b' ');
    std::fs::write(&path, &foreign).unwrap();
    assert!(crate::replace_existing(&path, &token, editor.project()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), foreign);
    assert_eq!(load(&path).unwrap(), project);
    assert!(editor.undo());
    assert_eq!(editor.project().native_sources(), project.native_sources());
    assert!(editor.redo());
    let copy = dir.join("edited.hex");
    save_new(&copy, editor.project()).unwrap();
    assert_eq!(&load(&copy).unwrap(), editor.project());
    std::fs::remove_dir_all(dir).unwrap();
}
