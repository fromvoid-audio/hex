use hex_project::{Command, Created, Editor, Project, RoutingOwner};
use hex_storage::{decode, encode, encode_schema20, encode_schema22};
use serde_json::{Value, json};

fn fixture() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let Created::Track(id) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "track".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track");
        };
        ids.push(id);
    }
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack { name: "bus".into() })
        .unwrap()
    else {
        panic!("return");
    };
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(ids[0]),
            destination: ids[1],
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnSend {
            track: ids[1],
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Return(bus),
            destination: ids[2],
            gain: 0.25,
        })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination: ids[1],
            source: Some(ids[0]),
        })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination: ids[2],
            source: Some(ids[1]),
        })
        .unwrap();
    editor.project().clone()
}

#[test]
fn schema24_roundtrip_preserves_typed_routes_ids_gain_and_midi_bus_selection() {
    let project = fixture();
    let bytes = encode(&project).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["version"], 34);
    assert_eq!(value["audio_feeds"][0]["source"]["kind"], "track");
    assert_eq!(value["audio_feeds"][1]["source"]["kind"], "return");
    let loaded = decode(&bytes).unwrap();
    assert_eq!(loaded, project);
    assert_eq!(
        loaded.midi_source_track(loaded.tracks()[2].id()),
        loaded.tracks()[0].id()
    );
    assert!(encode_schema20(&project).is_err());
    assert!(encode_schema22(&project).is_err());
}

#[test]
fn schema24_rejects_malformed_ports_identity_cycles_and_unbounded_lists() {
    let base: Value = serde_json::from_slice(&encode(&fixture()).unwrap()).unwrap();
    for (field, key, value) in [
        ("audio_feeds", "gain", json!(0)),
        ("audio_feeds", "gain", json!(2.1)),
        ("audio_feeds", "gain", Value::Null),
        ("audio_feeds", "id", base["midi_routes"][0]["id"].clone()),
        ("audio_feeds", "destination", json!(0)),
        ("audio_feeds", "extra", json!(true)),
        ("midi_routes", "source", base["tracks"][1]["id"].clone()),
        ("midi_routes", "destination", json!(0)),
        ("midi_routes", "id", base["tracks"][0]["id"].clone()),
        ("midi_routes", "extra", json!(true)),
    ] {
        let mut invalid = base.clone();
        invalid[field][0][key] = value;
        assert!(
            decode(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{field}:{key}"
        );
    }
    for field in ["audio_feeds", "midi_routes"] {
        let mut invalid = base.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        let mut invalid = base.clone();
        invalid[field] = Value::Null;
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        let limit = if field == "audio_feeds" {
            hex_project::MAX_AUDIO_FEEDS
        } else {
            hex_project::MAX_MIDI_ROUTES
        };
        let mut invalid = base.clone();
        invalid[field] = json!(vec![base[field][0].clone(); limit + 1]);
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut invalid = base.clone();
    invalid["audio_feeds"][0]["source"]["kind"] = json!("main");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    let mut invalid = base.clone();
    invalid["audio_feeds"][1]["destination"] = base["tracks"][0]["id"].clone();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    let mut invalid = base.clone();
    invalid["midi_routes"][1]["destination"] = base["midi_routes"][0]["destination"].clone();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    // Duplicate keys must remain rejected, including the fields added in24.
    let bytes = serde_json::to_string(&base).unwrap();
    let duplicate = bytes.replacen("\"audio_feeds\":", "\"audio_feeds\":[],\"audio_feeds\":", 1);
    assert!(decode(duplicate.as_bytes()).is_err());
}

#[test]
fn schema23_migrates_empty_inputs_and_rejects_playable_field_backports() {
    let project = Project::new(48_000).unwrap();
    let mut old: Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    old["version"] = json!(23);
    old.as_object_mut().unwrap().remove("main_devices");
    old.as_object_mut().unwrap().remove("tuning");
    old.as_object_mut().unwrap().remove("device_assets");
    old.as_object_mut().unwrap().remove("asset_sources");
    old.as_object_mut().unwrap().remove("track_colors");
    old.as_object_mut().unwrap().remove("accented_clips");
    old.as_object_mut().unwrap().remove("audio_clip_state");
    old.as_object_mut().unwrap().remove("fade_shapes");
    old.as_object_mut().unwrap().remove("audio_event_state");

    old.as_object_mut().unwrap().remove("clip_names");
    assert!(decode(&serde_json::to_vec(&old).unwrap()).is_err());
    old.as_object_mut().unwrap().remove("audio_feeds");
    old.as_object_mut().unwrap().remove("midi_routes");
    assert_eq!(decode(&serde_json::to_vec(&old).unwrap()).unwrap(), project);
}
