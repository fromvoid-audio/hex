use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, NativeDeviceState,
};
use hex_project::{AssetId, Command, Created, DeviceId, Editor, Invalid, Project};

fn fixture() -> (Editor, DeviceId, AssetId) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "Samples/Original.WAV".into(),
            sample_rate: 48000,
            frames: 100,
        })
        .unwrap()
    else {
        panic!("asset fixture")
    };
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "sample source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track fixture")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    (editor, device, asset)
}
fn reference(asset: AssetId) -> DeviceAssetReference {
    DeviceAssetReference {
        slot: DeviceAssetSlot::new(1).unwrap(),
        asset: DeviceAssetKey::new(asset.value()).unwrap(),
    }
}
fn replacement(
    editor: &Editor,
    device: DeviceId,
    references: Vec<DeviceAssetReference>,
) -> Command {
    let expected_state = editor.project().device(device).unwrap().state().clone();
    let state = expected_state
        .clone()
        .with_asset_references(references)
        .unwrap();
    Command::ReplaceDeviceStateWithIdentityReservation {
        device,
        expected_state,
        expected_variant: editor.project().device(device).unwrap().active_variant(),
        state,
        expected_next_id: editor.project().snapshot().next_id,
        reserved_ids: 0,
    }
}

#[test]
fn device_asset_binding_roundtrips_history_and_all_device_owners_without_payload_branches() {
    let (mut editor, instrument, asset) = fixture();
    let legacy = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&legacy).unwrap()["version"],
        hex_storage::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(
        hex_storage::decode(&legacy).unwrap().snapshot(),
        editor.project().snapshot()
    );
    let before = editor.project().snapshot();
    editor
        .execute(replacement(&editor, instrument, vec![reference(asset)]))
        .unwrap();
    let bound = editor.project().snapshot();
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), bound);
    let mut scale = hex_tuning::Scale::equal(7);
    scale.set_reference(60, 440.000_000_011_720_83, 2).unwrap();
    scale.set_period(1901.955);
    let tuning =
        hex_tuning::ProjectTuning::new(true, scale, hex_tuning::Audition::default()).unwrap();
    editor.execute(Command::SetTuning(tuning)).unwrap();
    assert!(editor.undo());
    assert_eq!(
        editor.project().snapshot(),
        bound,
        "tuning Undo preserves asset bindings"
    );
    assert!(editor.redo());
    assert_eq!(editor.project().tuning(), tuning);
    let track = editor.project().tracks()[0].id();
    let opaque = NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::new("test.asset-effect").unwrap(),
        7,
        vec![255, 17],
    )
    .unwrap()
    .with_asset_references(vec![reference(asset)])
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: opaque.clone(),
        })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack { name: "bus".into() })
        .unwrap()
    else {
        panic!("return fixture")
    };
    editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: opaque,
        })
        .unwrap();
    let bytes = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        hex_storage::CURRENT_SCHEMA_VERSION
    );
    let reopened = hex_storage::decode(&bytes).unwrap();
    assert_eq!(reopened.snapshot(), editor.project().snapshot());
    assert_eq!(
        reopened.asset(asset).unwrap().path().to_str(),
        Some("Samples/Original.WAV")
    );
    assert!(hex_storage::encode_schema22(editor.project()).is_err());
    assert!(hex_storage::encode_schema20(editor.project()).is_err());
    let preset = hex_storage::preset::Preset {
        comparison: None,
        class: hex_storage::preset::DeviceClass::Instrument,
        state: editor.project().device(instrument).unwrap().state().clone(),
    };
    assert!(hex_storage::preset::encode(&preset).is_err());
    assert!(
        editor.execute(Command::RemoveAsset(asset)).is_err(),
        "device-only references retain catalog entries"
    );
}

#[test]
fn unresolved_stale_or_refused_asset_candidates_preserve_canonical_state_and_redo() {
    let (mut editor, device, asset) = fixture();
    let command = replacement(&editor, device, vec![reference(asset)]);
    editor.execute(command.clone()).unwrap();
    assert!(editor.undo());
    assert!(editor.can_redo());
    let before = editor.project().snapshot();
    let missing = DeviceAssetReference {
        slot: DeviceAssetSlot::new(1).unwrap(),
        asset: DeviceAssetKey::new(u64::MAX).unwrap(),
    };
    assert!(
        editor
            .execute(replacement(&editor, device, vec![missing]))
            .is_err()
    );
    let refused = editor.execute_prepared(command.clone(), |_| {
        Err::<(), _>(Invalid("resource preparation refused"))
    });
    assert!(refused.is_err());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.can_redo());
    assert!(editor.redo());
    let current = editor.project().snapshot();
    assert!(
        editor.execute(command).is_err(),
        "old expected configuration cannot replace a newer binding"
    );
    assert_eq!(editor.project().snapshot(), current);
    let mut broken = current;
    broken.assets.clear();
    assert!(Project::from_snapshot(broken).is_err());
}

#[test]
fn device_asset_manifest_rejects_incomplete_duplicate_unknown_and_unbounded_bindings() {
    let (mut editor, device, asset) = fixture();
    editor
        .execute(replacement(&editor, device, vec![reference(asset)]))
        .unwrap();
    let track = editor.project().tracks()[0].id();
    let effect = NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::new("test.asset-effect").unwrap(),
        7,
        vec![1],
    )
    .unwrap()
    .with_asset_references(vec![reference(asset)])
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: effect,
        })
        .unwrap();
    let bytes = hex_storage::encode(editor.project()).unwrap();
    let source: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for case in 0..9 {
        let mut value = source.clone();
        match case {
            0 => {
                value.as_object_mut().unwrap().remove("device_assets");
                value.as_object_mut().unwrap().remove("asset_sources");
            }
            1 => {
                value["device_assets"] = serde_json::json!([]);
            }
            2 => {
                let row = value["device_assets"][0].clone();
                value["device_assets"].as_array_mut().unwrap().push(row);
            }
            3 => {
                value["device_assets"][0]["device"] = u64::MAX.into();
            }
            4 => {
                let row = value["device_assets"][0]["references"][0].clone();
                value["device_assets"][0]["references"]
                    .as_array_mut()
                    .unwrap()
                    .push(row);
            }
            5 => {
                value["device_assets"][0]["references"][0]["asset"] = u64::MAX.into();
            }
            6 => {
                value["device_assets"][0]["references"][0]["slot"] = 0.into();
            }
            7 => {
                value["device_assets"][0]["references"] = serde_json::Value::Array(
                    (1..=129)
                        .map(|slot| serde_json::json!({"slot":slot,"asset":asset.value()}))
                        .collect(),
                );
            }
            _ => {
                value["version"] = 29.into();
                value.as_object_mut().unwrap().remove("main_devices");
            }
        }
        assert!(
            hex_storage::decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "case {case}"
        );
    }
    let duplicated = String::from_utf8(bytes).unwrap().replacen(
        "\"device_assets\":",
        "\"device_assets\":[],\"device_assets\":",
        1,
    );
    assert!(hex_storage::decode(duplicated.as_bytes()).is_err());
}

#[test]
fn relabelled_current30_envelopes_refuse_without_rewriting_user_files() {
    let (mut editor, device, asset) = fixture();
    editor
        .execute(replacement(&editor, device, vec![reference(asset)]))
        .unwrap();
    let source: serde_json::Value =
        serde_json::from_slice(&hex_storage::encode(editor.project()).unwrap()).unwrap();
    let directory = std::env::temp_dir().join(format!(
        "hex-alpha-format-refusal-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("user-project.hex");
    for (tuning, assets) in [(true, true), (true, false), (false, true), (false, false)] {
        let mut value = source.clone();
        value["version"] = 30.into();
        value.as_object_mut().unwrap().remove("main_devices");
        if !tuning {
            value.as_object_mut().unwrap().remove("tuning");
        }
        if !assets {
            value.as_object_mut().unwrap().remove("device_assets");
            value.as_object_mut().unwrap().remove("asset_sources");
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(hex_storage::load(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::read_dir(&directory).unwrap().count(),
            1,
            "no conversion/recovery sidecar is written by refusal"
        );
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
