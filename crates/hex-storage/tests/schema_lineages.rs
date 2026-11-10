use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{Command, Created, Editor, Project};
use hex_storage::{decode, encode};
use serde_json::{Value, json};

const MAIN19: &[u8] = include_bytes!("fixtures/lineages/main19.json");
const FOUNDATION: [&[u8]; 4] = [
    include_bytes!("fixtures/lineages/foundation19.json"),
    include_bytes!("fixtures/lineages/foundation20.json"),
    include_bytes!("fixtures/lineages/foundation21.json"),
    include_bytes!("fixtures/lineages/foundation22.json"),
];
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn fixture() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "Case Preserved".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: NativeDeviceState::new(
                DeviceTypeId::new("fixture.opaque").unwrap(),
                7,
                vec![0, 255, 42],
            )
            .unwrap()
            .with_input_gain_millidb(3000)
            .unwrap()
            .with_output_gain_millidb(-9500)
            .unwrap(),
        })
        .unwrap();
    let state = editor.project().track(track).unwrap().devices()[0]
        .state()
        .clone();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: state.clone(),
        })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: state.clone(),
        })
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    editor
        .execute(Command::AddAsset {
            path: "Media/Original.WAV".into(),
            sample_rate: 48_000,
            frames: 128,
        })
        .unwrap();
    editor.project().clone()
}
#[test]
fn original_lineages_roundtrip_and_malformed_relabelled_envelopes_refuse_without_rewriting_input() {
    for original in std::iter::once(MAIN19).chain(FOUNDATION) {
        let before = original.to_vec();
        let project = decode(original).unwrap();
        assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
        assert_eq!(original, before);
    }
    let current: Value = serde_json::from_slice(&encode(&fixture()).unwrap()).unwrap();
    for version in (0..34).chain([hex_storage::NEWEST_SCHEMA_VERSION + 1, u32::MAX]) {
        let mut old = current.clone();
        old["version"] = version.into();
        assert!(
            decode(&bytes(&old)).is_err(),
            "malformed relabelled envelope {version}"
        );
    }
}
#[test]
fn current_host_gains_require_both_fields_and_refuse_invalid_duplicate_and_unknown_fields() {
    let base: Value = serde_json::from_slice(&encode(&fixture()).unwrap()).unwrap();
    assert!(decode(&bytes(&base)).is_ok());
    for field in ["input_gain_millidb", "output_gain_millidb"] {
        for bad in [
            json!(-60001),
            json!(24001),
            json!(0.5),
            Value::Null,
            json!("0"),
        ] {
            let mut doc = base.clone();
            doc["tracks"][0]["devices"][0][field] = bad;
            assert!(decode(&bytes(&doc)).is_err());
        }
        let mut doc = base.clone();
        doc["tracks"][0]["devices"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(decode(&bytes(&doc)).is_err());
        let text = String::from_utf8(bytes(&base)).unwrap();
        let existing = format!("\"{field}\":{}", base["tracks"][0]["devices"][0][field]);
        let duplicate = text.replace(&existing, &format!("{existing},\"{field}\":0"));
        assert_ne!(text, duplicate);
        assert!(decode(duplicate.as_bytes()).is_err());
    }
    for field in ["routing", "import_provenance", "loop_region"] {
        let mut doc = base.clone();
        doc.as_object_mut().unwrap().remove(field);
        assert!(decode(&bytes(&doc)).is_err());
    }
    let mut doc = base.clone();
    doc["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("gain_envelope");
    assert!(decode(&bytes(&doc)).is_err());
    let mut doc = base.clone();
    doc["tracks"][0]["devices"][0]["unknown"] = true.into();
    assert!(decode(&bytes(&doc)).is_err());
    let mut doc = base.clone();
    doc["version"] = 32.into();
    doc.as_object_mut().unwrap().remove("main_devices");
    assert!(decode(&bytes(&doc)).is_err());
}

#[test]
fn current_opaque_host_gains_roundtrip_preserving_history_and_destination() {
    let project = fixture();
    assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor.execute(Command::MasterGain { gain: 0.25 }).unwrap();
    assert_eq!(
        decode(&encode(editor.project()).unwrap()).unwrap(),
        *editor.project()
    );
    assert!(editor.undo());
    assert_eq!(*editor.project(), project);
    assert!(editor.redo());
    assert_eq!(
        decode(&encode(editor.project()).unwrap()).unwrap(),
        *editor.project()
    );
    let dir = std::env::temp_dir().join(format!(
        "hex-schema-lineage-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("project.hex");
    hex_storage::save_new(&path, &project).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(hex_storage::save_new(&path, &Project::new(48000).unwrap()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    assert_eq!(hex_storage::load(&path).unwrap(), project);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn current_host_gain_fields_are_required_for_each_device_owner_not_asset_or_comparison_rows() {
    let base: Value = serde_json::from_slice(&encode(&fixture()).unwrap()).unwrap();
    decode(&bytes(&base)).unwrap();
    assert!(base["assets"][0].get("input_gain_millidb").is_none());
    for owner in 0..4 {
        for field in ["input_gain_millidb", "output_gain_millidb"] {
            let mut missing = base.clone();
            let device = match owner {
                0 => &mut missing["tracks"][0]["devices"][0],
                1 => {
                    let source = missing["track_source_bindings"][0]["source"]
                        .as_u64()
                        .unwrap();
                    missing["native_sources"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|row| row["id"].as_u64() == Some(source))
                        .unwrap()
                        .get_mut("devices")
                        .unwrap()
                        .as_array_mut()
                        .unwrap()
                        .get_mut(0)
                        .unwrap()
                        .get_mut("device")
                        .unwrap()
                }
                2 => &mut missing["return_tracks"][0]["devices"][0],
                _ => &mut missing["main_devices"][0],
            };
            device.as_object_mut().unwrap().remove(field);
            assert!(decode(&bytes(&missing)).is_err(), "owner {owner}, {field}");
        }
    }
}
