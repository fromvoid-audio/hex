use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{Command, Created, Editor, ImportProvenance, Project};
use hex_storage::{StorageError, decode, encode, encode_schema20, load};
use serde_json::{Value, json};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn fixture() -> Project {
    let mut snapshot = Project::new(48_000).unwrap().snapshot();
    snapshot.import_provenance =
        Some(ImportProvenance::new("ableton-live-set", 1, 1, vec![255, 0, 42]).unwrap());
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    for version in [9, 10] {
        let Created::ReturnTrack(id) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        editor
            .execute(Command::InsertReturnDevice {
                return_track: id,
                before: None,
                state: NativeDeviceState::new(
                    DeviceTypeId::new("vendor.future").unwrap(),
                    version,
                    vec![0, 255, 42],
                )
                .unwrap(),
            })
            .unwrap();
    }
    editor.project().clone()
}
fn document() -> Value {
    serde_json::from_slice(&encode(&fixture()).unwrap()).unwrap()
}
fn with_feed(mut doc: Value) -> Value {
    let id = doc["next_id"].as_u64().unwrap();
    doc["next_id"] = json!(id + 1);
    doc["routing"] = json!([{"kind":"return_feed","id":id,
        "source":doc["return_tracks"][0]["id"],"destination":doc["return_tracks"][1]["id"],"gain":0.5}]);
    doc
}

#[test]
fn current_feeds_keep_provenance_and_opaque_device_states() {
    let project = fixture();
    let bytes = encode(&project).unwrap();
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(doc["routing"], json!([]));
    assert_eq!(decode(&bytes).unwrap(), project);
    assert_eq!(
        decode(&encode_schema20(&project).unwrap()).unwrap(),
        project
    );
}

#[test]
fn valid_feed_decode_load_and_save_preserve_canonical_state_and_input() {
    let doc = with_feed(document());
    let bytes = serde_json::to_vec(&doc).unwrap();
    let decoded = decode(&bytes).unwrap();
    assert_eq!(decoded.authored_routing().len(), 1);
    assert_eq!(decode(&encode(&decoded).unwrap()).unwrap(), decoded);
    assert_eq!(
        serde_json::from_slice::<Value>(&encode(&decoded).unwrap()).unwrap()["version"],
        hex_storage::CURRENT_SCHEMA_VERSION
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hex-feed-{}-{stamp}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("candidate.hex");
    fs::write(&path, &bytes).unwrap();
    assert_eq!(load(&path).unwrap(), decoded);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let saved = dir.join("saved.hex");
    hex_storage::save_new(&saved, &decoded).unwrap();
    assert_eq!(load(&saved).unwrap(), decoded);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema20_refuses_cycles_id_collisions_zero_gain_unknown_fields_and_capacity() {
    let base = with_feed(document());
    for (key, value) in [
        ("id", json!(0)),
        ("id", base["return_tracks"][0]["id"].clone()),
        ("id", base["return_tracks"][0]["devices"][0]["id"].clone()),
        ("gain", json!(0)),
        ("gain", json!(-1)),
        ("gain", json!(2.1)),
        ("gain", json!(null)),
        ("kind", json!("midi")),
        ("kind", json!("sidechain")),
        ("extra", json!(true)),
        ("source", json!(0)),
        ("destination", json!(0)),
    ] {
        let mut invalid = base.clone();
        invalid["routing"][0][key] = value;
        assert!(
            decode(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{key}"
        );
    }
    let mut invalid = base.clone();
    invalid["routing"][0]["destination"] = invalid["routing"][0]["source"].clone();
    assert!(matches!(
        decode(&serde_json::to_vec(&invalid).unwrap()),
        Err(StorageError::Invalid("routing cycles are unsupported"))
    ));
    invalid = base.clone();
    invalid["routing"] = json!(vec![base["routing"][0].clone(); 121]);
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = base.clone();
    invalid.as_object_mut().unwrap().remove("routing");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = base;
    invalid["version"] = json!(19);
    invalid.as_object_mut().unwrap().remove("main_devices");
    invalid.as_object_mut().unwrap().remove("tuning");
    invalid.as_object_mut().unwrap().remove("device_assets");
    invalid.as_object_mut().unwrap().remove("asset_sources");
    invalid.as_object_mut().unwrap().remove("track_colors");
    invalid.as_object_mut().unwrap().remove("accented_clips");
    invalid.as_object_mut().unwrap().remove("audio_clip_state");
    invalid.as_object_mut().unwrap().remove("fade_shapes");
    invalid.as_object_mut().unwrap().remove("audio_event_state");

    invalid.as_object_mut().unwrap().remove("clip_names");
    invalid.as_object_mut().unwrap().remove("audio_feeds");
    invalid.as_object_mut().unwrap().remove("midi_routes");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
}
