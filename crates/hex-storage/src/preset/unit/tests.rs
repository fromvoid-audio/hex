use super::*;
use crate::preset;
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceAutomationTarget,
};
use hex_project::{
    DeviceComparison, DeviceModulation, DeviceModulationRoute, DeviceVariant, ElementColor,
    ModulationAmount, ModulationDestination, ModulationOutputId, ModulationRange,
    ModulationRouteId, ModulationSourceId, Modulator,
};
use serde_json::{Value, json};

fn fixture() -> PortableDeviceUnit {
    let generator = DeviceTypeId::from_static("hex.instrument.sampler");
    let effect = DeviceTypeId::from_static("hex.effect.delay");
    let sampler_state = |version, asset| {
        NativeDeviceState::new(generator, version, vec![2, 255, 0])
            .unwrap()
            .with_output_gain_millidb(-2500)
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(asset).unwrap(),
            }])
            .unwrap()
    };
    let source = ModulationSourceId::new(1).unwrap();
    let modulation = DeviceModulation::new(
        vec![Modulator {
            id: source,
            color: ElementColor::Cyan,
            enabled: true,
            state: NativeDeviceState::new(
                DeviceTypeId::from_static("hex.modulator.macro"),
                1,
                0.25_f32.to_le_bytes().to_vec(),
            )
            .unwrap(),
        }],
        vec![DeviceModulationRoute {
            id: ModulationRouteId::new(2).unwrap(),
            source,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Device {
                device_type: generator,
                target: DeviceAutomationTarget::HostOutputGain,
                vst3_class: None,
            },
            amount: ModulationAmount::new(0.5).unwrap(),
            range: ModulationRange::default(),
            enabled: false,
        }],
    )
    .unwrap();
    PortableDeviceUnit::new(
        DeviceUnitClass::Instrument,
        vec![
            PortableDeviceNode {
                parent: None,
                state: sampler_state(2, 1),
                comparison: Some(DeviceComparison::new(DeviceVariant::B, sampler_state(1, 2))),
                bypassed: false,
                modulation,
            },
            PortableDeviceNode {
                parent: Some(0),
                state: NativeDeviceState::new(effect, 3, vec![9]).unwrap(),
                comparison: None,
                bypassed: true,
                modulation: DeviceModulation::default(),
            },
        ],
        [3, 8]
            .into_iter()
            .map(|tag| PortableDeviceAsset {
                source_rate: 48_000,
                source_frames: 32,
                facts: OriginalSourceFacts::new(OriginalSourceFacts::LAW_VERSION, [tag; 32])
                    .unwrap(),
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn complete_unit_preserves_nested_state_relative_assets_and_colored_mappings() {
    let unit = fixture();
    let bytes = encode_unit(&unit).unwrap();
    let document: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(document["schema"], UNIT_PRESET_SCHEMA);
    assert_eq!(document["nodes"][0]["references"][0]["asset"], 1);
    assert_eq!(
        document["nodes"][0]["comparison"]["references"][0]["asset"],
        2
    );
    assert_eq!(document["nodes"][1]["parent"], 0);
    assert!(document["assets"][0].get("path").is_none());
    assert!(document["assets"][0].get("id").is_none());
    assert_eq!(decode_contents(&bytes).unwrap(), PresetContents::Unit(unit));
    assert!(matches!(
        preset::decode(&bytes),
        Err(Error::Invalid(
            "complete device preset requires a unit load"
        ))
    ));
}

#[test]
fn rejects_wrong_structure_references_metadata_and_unknown_or_duplicate_keys() {
    let bytes = encode_unit(&fixture()).unwrap();
    let document: Value = serde_json::from_slice(&bytes).unwrap();
    for (pointer, bad) in [
        ("/format", json!("foreign")),
        ("/schema", json!(4)),
        ("/nodes/1/parent", json!(1)),
        ("/nodes/0/references/0/asset", json!(3)),
        ("/nodes/0/comparison/type_id", json!("foreign.provider")),
        ("/nodes/0/modulation/sources/0/color", json!("unknown")),
        (
            "/nodes/0/modulation/routes/0/destination/device_type",
            json!("invalid provider"),
        ),
        ("/assets/0/law_version", json!(0)),
        ("/assets/0/source_frames", json!(0)),
    ] {
        let mut changed = document.clone();
        *changed.pointer_mut(pointer).unwrap() = bad;
        assert!(
            decode_contents(&serde_json::to_vec(&changed).unwrap()).is_err(),
            "{pointer} must refuse"
        );
    }
    let mut missing = document.clone();
    missing["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("parent");
    assert!(decode_contents(&serde_json::to_vec(&missing).unwrap()).is_err());
    let mut unknown = document.clone();
    unknown["assets"][0]["path"] = json!("original.wav");
    assert!(decode_contents(&serde_json::to_vec(&unknown).unwrap()).is_err());
    let mut external = document.clone();
    external["nodes"][0]["modulation"]["routes"][0]["destination"] = json!({
        "kind": "device",
        "device_type": "hex.effect.delay",
        "target": {"kind": "parameter", "parameter": 1},
        "vst3_class": null,
    });
    assert!(decode_contents(&serde_json::to_vec(&external).unwrap()).is_err());
    let mut excess = document.clone();
    excess["nodes"] = json!(vec![
        document["nodes"][0].clone();
        MAX_PORTABLE_DEVICE_UNIT_NODES + 1
    ]);
    assert!(decode_contents(&serde_json::to_vec(&excess).unwrap()).is_err());
    let mut deep = document.clone();
    let mut nodes = document["nodes"].as_array().unwrap().clone();
    for index in 1..9 {
        let mut child = document["nodes"][1].clone();
        child["parent"] = json!(index);
        nodes.push(child);
    }
    deep["nodes"] = json!(nodes);
    assert!(decode_contents(&serde_json::to_vec(&deep).unwrap()).is_err());
    let duplicate = String::from_utf8(bytes).unwrap().replacen(
        r#""parent":null"#,
        r#""parent":null,"parent":null"#,
        1,
    );
    assert!(decode_contents(duplicate.as_bytes()).is_err());
}

#[test]
fn legacy_schemas_open_as_state_and_publication_never_overwrites() {
    let legacy = br#"{"format":"hex-device-preset","schema":1,"class":"effect","type_id":"hex.effect.delay","state_version":1,"payload":[3]}"#;
    let state = preset::decode(legacy).unwrap();
    assert_eq!(
        decode_contents(legacy).unwrap(),
        PresetContents::State(Box::new(state.clone()))
    );
    let latest_state = preset::encode(&state).unwrap();
    assert_eq!(
        decode_contents(&latest_state).unwrap(),
        PresetContents::State(Box::new(state))
    );

    let directory = std::env::temp_dir().join(format!(
        "hex-unit-preset-{}-{}",
        std::process::id(),
        preset::NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join(format!("chain.{}", preset::EXTENSION));
    let unit = fixture();
    save_unit_new(&path, &unit).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert_eq!(
        load_contents(&path).unwrap(),
        PresetContents::Unit(unit.clone())
    );
    assert!(preset::load(&path).is_err());
    assert!(matches!(
        save_unit_new(&path, &unit),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists
    ));
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    std::fs::remove_dir_all(directory).unwrap();
}
