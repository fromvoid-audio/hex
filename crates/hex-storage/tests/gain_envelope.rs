use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
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
                incoming_gain: None,
                beat: BeatTime::ZERO,
                gain: 0.25,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                incoming_gain: None,
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
        automation: Vec::new(),
        comparison: None,
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
    assert_eq!(doc["version"], hex_storage::CURRENT_SCHEMA_VERSION);
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
    for version in [19, 20, 24] {
        invalid = base.clone();
        invalid["version"] = json!(version);
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
}
#[test]
fn new_writer_uses_complete_schema_even_without_lanes_or_feeds() {
    let mut s = fixture().snapshot();
    for track in &mut s.tracks {
        track.gain_envelope.clear();
    }
    let p = Project::from_snapshot(s).unwrap();
    let bytes = encode(&p).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
        hex_storage::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(decode(&bytes).unwrap(), p);
    let mut s = p.snapshot();
    s.return_feeds.clear();
    let p = Project::from_snapshot(s).unwrap();
    let bytes = encode(&p).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
        hex_storage::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(decode(&bytes).unwrap(), p);
}
#[test]
fn schema22_epoch_only_save_history_retains_base_feeds_sends_provenance_and_opaque_states() {
    // Author a schema22-compatible baseline without changing ordinary fixtures.
    let mut baseline = Editor::new(fixture(), 8).unwrap();
    let track = baseline.project().tracks()[0].id();
    baseline
        .execute(Command::SetTrackColor {
            track,
            color: hex_project::ElementColor::Gray,
        })
        .unwrap();
    let previous = baseline.project().clone();
    let mut snapshot = previous.snapshot();
    snapshot.tracks[0].gain_touch_epoch = 17;
    let project = Project::from_snapshot(snapshot).unwrap();
    let bytes = encode(&project).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["version"],
        hex_storage::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(decode(&bytes).unwrap(), project);
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    let owner = project.tracks()[0].id();
    editor
        .execute(Command::ClearTrackGainEnvelope { track: owner })
        .unwrap();
    assert_eq!(
        editor.project().tracks()[0].gain_lane().recording_epoch(),
        17
    );
    assert!(editor.undo());
    assert_eq!(editor.project(), &project);
    assert!(encode_schema20(&project).is_err());
    let dir = std::env::temp_dir().join(format!(
        "hex-touch-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("touch.hex");
    hex_storage::save_new(&path, &project).unwrap();
    assert_eq!(hex_storage::load(&path).unwrap(), project);
    std::fs::remove_dir_all(dir).unwrap();
    let p = decode(&encode(&previous).unwrap()).unwrap();
    assert_eq!(p, previous);
    assert!(p.tracks()[0].gain_lane().spans().is_empty());
    assert_eq!(p.tracks()[0].gain_lane().recording_epoch(), 0);
    assert_eq!(
        decode(&hex_storage::encode_schema22(&previous).unwrap()).unwrap(),
        previous
    );
}

#[test]
fn schema24_combined_model_roundtrip_keeps_trimmed_devices_routes_automation_and_opaque_data() {
    let mut snapshot = Project::new(48_000).unwrap().snapshot();
    snapshot.import_provenance =
        Some(ImportProvenance::new("ableton-live-set", 1, 1, vec![0, 255, 42, 7]).unwrap());
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "combined".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::ReturnTrack(first_return) = editor
        .execute(Command::AddReturnTrack {
            name: "first return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::ReturnTrack(second_return) = editor
        .execute(Command::AddReturnTrack {
            name: "second return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: first_return,
            gain: 0.375,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnFeed {
            source: first_return,
            destination: second_return,
            gain: 0.625,
        })
        .unwrap();

    let hosted_effect = |kind: &'static str, input: f32, output: f32| {
        NativeDeviceState::new(DeviceTypeId::new(kind).unwrap(), 9, vec![0, 255, 17])
            .unwrap()
            .with_input_gain_db(input)
            .unwrap()
            .with_output_gain_db(output)
            .unwrap()
    };
    let Created::Device(track_effect) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hosted_effect("vendor.track-effect", 2.5, -4.0),
        })
        .unwrap()
    else {
        panic!("track effect")
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
            state: hosted_effect("vendor.instrument", -3.5, 1.25),
        })
        .unwrap();
    let Created::Device(return_effect) = editor
        .execute(Command::InsertReturnDevice {
            return_track: first_return,
            before: None,
            state: hosted_effect("vendor.return-effect", 5.0, -7.25),
        })
        .unwrap()
    else {
        panic!("return effect")
    };

    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].gain_envelope = vec![
        GainPoint {
            incoming_gain: None,
            beat: BeatTime::ZERO,
            gain: 0.2,
            outgoing: GainInterpolation::Linear,
        },
        GainPoint {
            incoming_gain: None,
            beat: BeatTime::from_raw(480),
            gain: 1.6,
            outgoing: GainInterpolation::Step,
        },
    ];
    snapshot.tracks[0].gain_touch_epoch = 23;
    let project = Project::from_snapshot(snapshot).unwrap();
    let bytes = encode(&project).unwrap();
    let document: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(document["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(document["tracks"][0]["gain_touch_epoch"], 23);
    assert!(
        document["tracks"][0]["gain_touch_spans"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        document["import_provenance"]["original_bytes"],
        json!([0, 255, 42, 7])
    );
    assert_eq!(document["routing"].as_array().unwrap().len(), 1);
    assert_eq!(document["tracks"][0]["sends"][0]["gain"], json!(0.375));
    assert_eq!(
        document["tracks"][0]["devices"][0]["payload"],
        json!([0, 255, 17])
    );
    assert_eq!(decode(&bytes).unwrap(), project);
    for (id, input_millidb, output_millidb) in [
        (track_effect, 2_500, -4_000),
        (instrument, -3_500, 1_250),
        (return_effect, 5_000, -7_250),
    ] {
        let restored = decode(&bytes).unwrap();
        assert_eq!(
            restored.device(id).unwrap().state(),
            project.device(id).unwrap().state()
        );
        assert_eq!(
            restored.device(id).unwrap().state().input_gain_millidb(),
            input_millidb
        );
        assert_eq!(
            restored.device(id).unwrap().state().output_gain_millidb(),
            output_millidb
        );
    }
}
