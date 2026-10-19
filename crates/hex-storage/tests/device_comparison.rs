use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};
use hex_project::{
    Command, Created, DeviceComparisonAction as Action, DeviceVariant, Editor, Project,
};

fn state(value: u8) -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::from_static("test.device"), 1, vec![value]).unwrap()
}
fn fixture() -> (Editor, Vec<hex_project::DeviceId>) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state: state(1),
        })
        .unwrap();
    let Created::Device(effect) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state(1),
        })
        .unwrap()
    else {
        panic!("effect")
    };
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::Device(return_effect) = editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: state(1),
        })
        .unwrap()
    else {
        panic!("return effect")
    };
    (editor, vec![instrument, effect, return_effect])
}

#[test]
fn all_device_roles_preserve_both_slots_global_metadata_and_inactive_asset_bindings() {
    let (mut editor, devices) = fixture();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "Samples/ExactCase.WAV".into(),
            sample_rate: 48_000,
            frames: 128,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    for device in devices {
        let bound = state(1)
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(asset.value()).unwrap(),
            }])
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: bound,
            })
            .unwrap();
        editor
            .execute(Command::CompareDevice {
                device,
                expected: DeviceVariant::A,
                action: Action::Switch,
            })
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: state(2)
                    .with_input_gain_millidb(-1200)
                    .unwrap()
                    .with_output_gain_millidb(2300)
                    .unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
    }
    let bytes = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(
        hex_storage::decode(&bytes).unwrap().snapshot(),
        editor.project().snapshot()
    );
    let reopened = hex_storage::decode(&bytes).unwrap();
    assert_eq!(
        reopened.asset(asset).unwrap().path().to_str(),
        Some("Samples/ExactCase.WAV")
    );
    assert!(hex_storage::encode_schema22(editor.project()).is_err());
    let mut restored = Editor::new(reopened, 8).unwrap();
    assert!(
        restored.execute(Command::RemoveAsset(asset)).is_err(),
        "inactive references retain their actual media"
    );
    let mut malformed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    malformed["device_assets"][0]["comparison"]["references"][0]["asset"] =
        serde_json::json!(u64::MAX);
    assert!(hex_storage::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
}

#[test]
fn missing_comparison_wrong_slot_provider_or_state_size_refuses_current_document() {
    let (mut editor, devices) = fixture();
    editor
        .execute(Command::CompareDevice {
            device: devices[0],
            expected: DeviceVariant::A,
            action: Action::Switch,
        })
        .unwrap();
    let original: serde_json::Value =
        serde_json::from_slice(&hex_storage::encode(editor.project()).unwrap()).unwrap();
    for (field, value) in [
        ("active", serde_json::json!("c")),
        ("type_id", serde_json::json!("other.provider")),
        ("state_version", serde_json::json!(0)),
        (
            "payload",
            serde_json::json!(vec![
                0u8;
                hex_lattice_contract::MAX_NATIVE_DEVICE_STATE_BYTES + 1
            ]),
        ),
    ] {
        let mut changed = original.clone();
        changed["device_assets"][0]["comparison"][field] = value;
        assert!(
            hex_storage::decode(&serde_json::to_vec(&changed).unwrap()).is_err(),
            "{field}"
        );
    }
    let mut changed = original.clone();
    changed["device_assets"][0]
        .as_object_mut()
        .unwrap()
        .remove("comparison");
    assert!(hex_storage::decode(&serde_json::to_vec(&changed).unwrap()).is_err());
    let mut changed = original;
    changed["version"] = 31.into();
    assert!(
        hex_storage::decode(&serde_json::to_vec(&changed).unwrap()).is_err(),
        "old alpha envelope is explicitly unsupported"
    );
}

#[test]
fn preset_pair_roundtrip_and_both_slot_portable_asset_refusal() {
    use hex_storage::preset::{self, DeviceClass, Preset};
    let (mut editor, devices) = fixture();
    let device = devices[0];
    editor
        .execute(Command::CompareDevice {
            device,
            expected: DeviceVariant::A,
            action: Action::Switch,
        })
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: state(2),
        })
        .unwrap();
    let authored = editor.project().device(device).unwrap();
    let mut preset = Preset {
        class: DeviceClass::Instrument,
        state: authored.state().clone(),
        comparison: authored.comparison().cloned(),
    };
    assert_eq!(
        preset::decode(&preset::encode(&preset).unwrap()).unwrap(),
        preset
    );
    let bound = state(1)
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(1).unwrap(),
        }])
        .unwrap();
    preset.comparison = Some(hex_project::DeviceComparison::new(
        DeviceVariant::B,
        bound.clone(),
    ));
    assert!(
        preset::encode(&preset).is_err(),
        "inactive assets require portable manifest too"
    );
    preset.comparison = None;
    preset.state = bound;
    assert!(preset::encode(&preset).is_err());
}
