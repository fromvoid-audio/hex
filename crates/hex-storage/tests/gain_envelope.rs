use hex_project::{
    BeatTime, Command, Created, Editor, GainInterpolation, GainPoint, ImportProvenance, Project,
};
use hex_storage::{decode, encode, encode_schema20};
use serde_json::{Value, json};
fn fixture() -> Project {
    let mut e = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "gain".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    e.execute(Command::ReplaceTrackGainEnvelope {
        track,
        points: vec![
            GainPoint {
                beat: BeatTime::ZERO,
                gain: 0.25,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                beat: BeatTime::ONE,
                gain: 1.5,
                outgoing: GainInterpolation::Step,
            },
        ],
    })
    .unwrap();
    let mut buses = Vec::new();
    for _ in 0..2 {
        let Created::ReturnTrack(id) = e
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        buses.push(id);
    }
    e.execute(Command::SetReturnSend {
        track,
        return_track: buses[0],
        gain: 0.5,
    })
    .unwrap();
    e.execute(Command::SetReturnFeed {
        source: buses[0],
        destination: buses[1],
        gain: 0.5,
    })
    .unwrap();
    let mut s = e.project().snapshot();
    s.import_provenance =
        Some(ImportProvenance::new("ableton-live-set", 1, 1, vec![255, 0, 42]).unwrap());
    s.tracks[0].devices.push(hex_project::DeviceSnapshot {
        id: s.next_id,
        bypassed: false,
        state: hex_lattice_contract::NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::new("vendor.future").unwrap(),
            10,
            vec![0, 255, 42],
        )
        .unwrap(),
    });
    s.next_id += 1;
    Project::from_snapshot(s).unwrap()
}
#[test]
fn schema21_roundtrip_retains_every_lane_feed_send_provenance_and_opaque_payload() {
    let p = fixture();
    let bytes = encode(&p).unwrap();
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["version"], 21);
    assert_eq!(decode(&bytes).unwrap(), p);
    assert!(encode_schema20(&p).is_err());
    let dir = std::env::temp_dir().join(format!(
        "hex-gain-schema-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("gain.hex");
    hex_storage::save_new(&path, &p).unwrap();
    assert_eq!(hex_storage::load(&path).unwrap(), p);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn strict_new_lane_fields_order_modes_roles_limits_and_backports_refuse() {
    let base: Value = serde_json::from_slice(&encode(&fixture()).unwrap()).unwrap();
    for (key, value) in [
        ("beat_raw", json!(-1)),
        ("gain", json!(-0.1)),
        ("gain", json!(2.1)),
        ("gain", json!(null)),
        ("outgoing", json!("curve")),
        ("parameter", json!("device.gain")),
        ("extra", json!(true)),
    ] {
        let mut invalid = base.clone();
        invalid["tracks"][0]["gain_envelope"][0][key] = value;
        assert!(
            decode(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{key}"
        );
    }
    let mut invalid = base.clone();
    invalid["tracks"][0]["gain_envelope"][1]["beat_raw"] = json!(0);
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = base.clone();
    invalid["tracks"][0]["gain_envelope"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = base.clone();
    invalid["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("gain_envelope");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = base.clone();
    invalid["tracks"][0]["gain_envelope"] =
        json!(vec![base["tracks"][0]["gain_envelope"][0].clone(); 1025]);
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    for version in [19, 20, 22] {
        invalid = base.clone();
        invalid["version"] = json!(version);
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
}
#[test]
fn legacy20_and19_migrate_empty_lanes_and_keep_default_byte_selection() {
    let mut s = fixture().snapshot();
    for track in &mut s.tracks {
        track.gain_envelope.clear();
    }
    let p = Project::from_snapshot(s).unwrap();
    let bytes = encode(&p).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
        20
    );
    assert_eq!(decode(&bytes).unwrap(), p);
    let mut s = p.snapshot();
    s.return_feeds.clear();
    let p = Project::from_snapshot(s).unwrap();
    let bytes = encode(&p).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
        19
    );
    assert_eq!(decode(&bytes).unwrap(), p);
}
