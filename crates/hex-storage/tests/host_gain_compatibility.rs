//! Real canonical project fixtures; no engine/native PCM verdict is implied.
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{
    BeatTime, Command, Created, Editor, GainInterpolation, GainPoint, ImportProvenance, Project,
};
use hex_storage::{MAX_DOCUMENT_BYTES, decode, encode, encode_schema20};
use serde_json::{Value, json};
fn fixture(gain: i32) -> Project {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track,
            points: vec![GainPoint {
                beat: BeatTime::ZERO,
                gain: 0.5,
                outgoing: GainInterpolation::Step,
            }],
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.import_provenance =
        Some(ImportProvenance::new("ableton-live-set", 1, 1, vec![255, 0, 42]).unwrap());
    snapshot.tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            id: snapshot.next_id,
            bypassed: true,
            state: NativeDeviceState::new(
                DeviceTypeId::new("vendor.future").unwrap(),
                4,
                vec![1, 255],
            )
            .unwrap()
            .with_output_gain_millidb(gain)
            .unwrap(),
        });
    snapshot.next_id += 1;
    Project::from_snapshot(snapshot).unwrap()
}
#[test]
fn combined22_roundtrip_bypassed_opaque_gain_and_unity_selection() {
    for gain in [-60000, -9500, 24000] {
        let p = fixture(gain);
        let bytes = encode(&p).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
            22
        );
        assert_eq!(decode(&bytes).unwrap(), p);
        assert!(encode_schema20(&p).is_err());
        let mut e = Editor::new(p.clone(), 4).unwrap();
        e.execute(Command::MasterGain { gain: 0.25 }).unwrap();
        assert!(e.undo());
        assert_eq!(e.project(), &p);
        assert!(e.redo());
        assert_eq!(decode(&encode(e.project()).unwrap()).unwrap(), *e.project());
    }
    let p = fixture(0);
    let bytes = encode(&p).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
        21
    );
    assert_eq!(decode(&bytes).unwrap(), p);
}
#[test]
fn gain18_defaults_and_provenance18_disjoint_shapes() {
    let mut base: Value = serde_json::from_slice(&encode(&fixture(0)).unwrap()).unwrap();
    base["version"] = 18.into();
    base.as_object_mut().unwrap().remove("routing");
    base["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("gain_envelope");
    assert!(decode(&serde_json::to_vec(&base).unwrap()).is_ok());
    let mut hybrid = base.clone();
    hybrid["tracks"][0]["devices"][0]["output_gain_millidb"] = json!(0);
    assert!(decode(&serde_json::to_vec(&hybrid).unwrap()).is_err());
    base.as_object_mut().unwrap().remove("import_provenance");
    // This is a valid published gain18 shape: removed evidence is not detectable.
    let unity = decode(&serde_json::to_vec(&base).unwrap()).unwrap();
    assert!(unity.import_provenance().is_none());
    assert_eq!(
        unity.tracks()[0].devices()[0].state().output_gain_millidb(),
        0
    );
    for gain in [-60000, -9500, 24000] {
        base["tracks"][0]["devices"][0]["output_gain_millidb"] = gain.into();
        let p = decode(&serde_json::to_vec(&base).unwrap()).unwrap();
        assert_eq!(
            p.tracks()[0].devices()[0].state().output_gain_millidb(),
            gain
        );
        assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    }
}
#[test]
fn malformed_gains_required_fields_backports_and_duplicate_keys_refuse() {
    let base: Value = serde_json::from_slice(&encode(&fixture(-9500)).unwrap()).unwrap();
    for gain in [
        json!(-60001),
        json!(24001),
        json!(0.5),
        json!(null),
        json!("0"),
    ] {
        let mut bad = base.clone();
        bad["tracks"][0]["devices"][0]["output_gain_millidb"] = gain;
        assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut missing = base.clone();
    missing["tracks"][0]["devices"][0]
        .as_object_mut()
        .unwrap()
        .remove("output_gain_millidb");
    assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    for version in [18, 19, 20, 21, 23] {
        let mut bad = base.clone();
        bad["version"] = version.into();
        assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let text = String::from_utf8(serde_json::to_vec(&base).unwrap()).unwrap();
    let duplicate = text.replace(
        "\"output_gain_millidb\":-9500",
        "\"output_gain_millidb\":-9500,\"output_gain_millidb\":0",
    );
    assert_ne!(duplicate, text);
    assert!(decode(duplicate.as_bytes()).is_err());
    let duplicate = text.replace("\"version\":22", "\"version\":22,\"version\":18");
    assert_ne!(duplicate, text);
    assert!(decode(duplicate.as_bytes()).is_err());
    assert!(decode(&vec![b' '; MAX_DOCUMENT_BYTES + 1]).is_err());
}
#[test]
fn every_device_role_feeds_and_save_load_preserve_complete22_state() {
    let p = fixture(-9500);
    let mut e = Editor::new(p, 8).unwrap();
    let mut buses = Vec::new();
    for _ in 0..2 {
        let Created::ReturnTrack(bus) = e
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        buses.push(bus);
    }
    e.execute(Command::SetReturnFeed {
        source: buses[0],
        destination: buses[1],
        gain: 0.5,
    })
    .unwrap();
    let mut s = e.project().snapshot();
    let state = s.tracks[0].devices[0].state.clone();
    s.tracks[0].instrument = Some(hex_project::DeviceSnapshot {
        id: s.next_id,
        bypassed: false,
        state: state.clone().with_output_gain_millidb(24000).unwrap(),
    });
    s.next_id += 1;
    s.tracks[0].instrument_position = Some(1);
    s.return_tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            id: s.next_id,
            bypassed: false,
            state: state.with_output_gain_millidb(-60000).unwrap(),
        });
    s.next_id += 1;
    let p = Project::from_snapshot(s).unwrap();
    let bytes = encode(&p).unwrap();
    assert_eq!(decode(&bytes).unwrap(), p);
    let dir = std::env::temp_dir().join(format!(
        "hex-storage-compat-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("project.hex");
    hex_storage::save_new(&path, &p).unwrap();
    assert_eq!(hex_storage::load(&path).unwrap(), p);
    assert!(hex_storage::save_new(&path, &Project::new(48000).unwrap()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_dir_all(dir).unwrap();
    let mut doc: Value = serde_json::from_slice(&bytes).unwrap();
    doc["return_tracks"][0]["devices"][0]["output_gain_millidb"] = 24001.into();
    assert!(decode(&serde_json::to_vec(&doc).unwrap()).is_err());
    doc["return_tracks"][0]["devices"][0]["output_gain_millidb"] = 0.into();
    doc["tracks"][0]["instrument"]
        .as_object_mut()
        .unwrap()
        .remove("output_gain_millidb");
    assert!(decode(&serde_json::to_vec(&doc).unwrap()).is_err());
}
#[test]
fn combined22_provenance_ceiling_and_publication_refusal_keep_existing_file() {
    let mut s = fixture(-9500).snapshot();
    s.import_provenance = Some(
        ImportProvenance::new(
            "ableton-live-set",
            1,
            1,
            vec![255; hex_project::MAX_IMPORT_DOCUMENT_BYTES],
        )
        .unwrap(),
    );
    let p = Project::from_snapshot(s).unwrap();
    let bytes = encode(&p).unwrap();
    assert!(bytes.len() < MAX_DOCUMENT_BYTES);
    assert_eq!(decode(&bytes).unwrap(), p);
    let mut e = Editor::new(p, 4).unwrap();
    e.execute(Command::ImportAudioTrack {
        name: "large path".into(),
        path: "x".repeat(MAX_DOCUMENT_BYTES).into(),
        sample_rate: 48000,
        frames: 100,
    })
    .unwrap();
    assert!(encode(e.project()).is_err());
    assert_eq!(
        e.project()
            .import_provenance()
            .unwrap()
            .original_bytes()
            .len(),
        hex_project::MAX_IMPORT_DOCUMENT_BYTES
    );
}
#[test]
fn unknown_and_required22_shape_and_nested_duplicates_refuse() {
    let base: Value = serde_json::from_slice(&encode(&fixture(-9500)).unwrap()).unwrap();
    for field in ["import_provenance", "routing", "loop_region"] {
        let mut bad = base.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut bad = base.clone();
    bad["tracks"][0]["devices"][0]["unknown"] = true.into();
    assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    let mut bad = base.clone();
    bad["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("gain_envelope");
    assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    let text = String::from_utf8(serde_json::to_vec(&base).unwrap()).unwrap();
    let duplicate = text.replace(
        "\"adapter_version\":1",
        "\"adapter_version\":1,\"adapter_version\":1",
    );
    assert_ne!(duplicate, text);
    assert!(decode(duplicate.as_bytes()).is_err());
}
#[test]
fn combined22_track_and_gain_point_budget_refuses_before_canonical_adoption() {
    let base: Value = serde_json::from_slice(&encode(&fixture(-9500)).unwrap()).unwrap();
    let mut bad = base.clone();
    bad["tracks"] = json!(vec![base["tracks"][0].clone(); hex_project::MAX_TRACKS + 1]);
    assert!(
        decode(&serde_json::to_vec(&bad).unwrap())
            .unwrap_err()
            .to_string()
            .contains("track limit reached")
    );
    let mut bad = base.clone();
    bad["tracks"][0]["gain_envelope"] = json!(vec![
        base["tracks"][0]["gain_envelope"][0].clone();
        hex_project::MAX_GAIN_POINTS_PER_TRACK + 1
    ]);
    assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
}
#[test]
fn explicit20_refuses_nonunity_without_automation_and_keeps_unity_feeds() {
    for gain in [0, -9500] {
        let mut snapshot = fixture(gain).snapshot();
        snapshot.tracks[0].gain_envelope.clear();
        let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 4).unwrap();
        let mut buses = Vec::new();
        for _ in 0..2 {
            let Created::ReturnTrack(bus) = editor
                .execute(Command::AddReturnTrack {
                    name: "return".into(),
                })
                .unwrap()
            else {
                panic!("return")
            };
            buses.push(bus);
        }
        editor
            .execute(Command::SetReturnFeed {
                source: buses[0],
                destination: buses[1],
                gain: 0.5,
            })
            .unwrap();
        let project = editor.project();
        assert!(!project.has_gain_automation());
        let encoded = encode(project).unwrap();
        assert_eq!(decode(&encoded).unwrap(), *project);
        assert_eq!(
            serde_json::from_slice::<Value>(&encoded).unwrap()["version"],
            if gain == 0 { 20 } else { 22 }
        );
        let older = encode_schema20(project);
        if gain == 0 {
            assert_eq!(older.unwrap(), encoded);
        } else {
            assert_eq!(
                older.unwrap_err().to_string(),
                "older schema cannot save host gain"
            );
        }
    }
}
