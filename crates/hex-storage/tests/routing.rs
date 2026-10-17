use hex_project::{Command, Created, Editor, ImportProvenance, Project};
use hex_storage::{StorageError, decode, encode};
use serde_json::{Value, json};

fn fixture() -> Value {
    let mut snapshot = Project::new(48_000).unwrap().snapshot();
    snapshot.import_provenance =
        Some(ImportProvenance::new("ableton-live-set", 1, 1, vec![0, 255, 42]).unwrap());
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut returns = Vec::new();
    for name in ["a", "b"] {
        let Created::ReturnTrack(id) = editor
            .execute(Command::AddReturnTrack { name: name.into() })
            .unwrap()
        else {
            panic!("return")
        };
        returns.push(id);
    }
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: returns[0],
            gain: 0.375,
        })
        .unwrap();
    let mut document: Value = serde_json::from_slice(&encode(editor.project()).unwrap()).unwrap();
    document["version"] = json!(19);
    document.as_object_mut().unwrap().remove("audio_clip_state");
    document.as_object_mut().unwrap().remove("fade_shapes");
    document.as_object_mut().unwrap().remove("clip_names");
    document.as_object_mut().unwrap().remove("audio_feeds");
    document.as_object_mut().unwrap().remove("midi_routes");
    for track in document["tracks"].as_array_mut().unwrap() {
        for key in ["gain_envelope", "gain_touch_spans", "gain_touch_epoch"] {
            track.as_object_mut().unwrap().remove(key);
        }
    }
    document
}
fn historical(version: u64, provenance: bool, routing: bool) -> Value {
    let mut document = fixture();
    document["version"] = json!(version);
    document.as_object_mut().unwrap().remove("audio_clip_state");
    document.as_object_mut().unwrap().remove("fade_shapes");
    document.as_object_mut().unwrap().remove("clip_names");
    document.as_object_mut().unwrap().remove("audio_feeds");
    document.as_object_mut().unwrap().remove("midi_routes");
    if !provenance {
        document
            .as_object_mut()
            .unwrap()
            .remove("import_provenance");
    }
    if !routing {
        document.as_object_mut().unwrap().remove("routing");
    }
    document
}
fn remove_host_trim_fields(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                remove_host_trim_fields(value);
            }
        }
        Value::Object(object) => {
            object.remove("input_gain_millidb");
            object.remove("output_gain_millidb");
            for value in object.values_mut() {
                remove_host_trim_fields(value);
            }
        }
        _ => {}
    }
}
fn edge(source: Value, target: Value) -> Value {
    json!({"source":source,"target":target,"signal":{"kind":"audio","channels":2}})
}
fn owner(kind: &str, id: &Value) -> Value {
    json!({"kind":kind,"id":id})
}
fn refusal(document: &Value, expected: &str) {
    assert!(
        matches!(decode(&serde_json::to_vec(document).unwrap()), Err(StorageError::Invalid(reason)) if reason == expected)
    );
}

#[test]
fn historical_main_and_branch_shapes_preserve_their_supported_data() {
    // Main schema18/19 omitted both later fields. The branch added provenance
    // in schema18 and routing in schema19; keep fixtures distinct so dispatch
    // cannot accidentally accept a hybrid or mistake one branch for the other.
    for (version, provenance, routing) in [
        (18, false, false),
        (19, false, false),
        (18, true, false),
        (19, true, true),
    ] {
        let mut document = historical(version, provenance, routing);
        assert_eq!(document["version"], json!(version));
        assert_eq!(document.get("import_provenance").is_some(), provenance);
        assert_eq!(document.get("routing").is_some(), routing);
        // Unknown vendor formats and reserved device-state versions are opaque here.
        document["return_tracks"][0]["devices"] = json!([
            {"id":100,"type_id":"vendor.future","state_version":9,"payload":[1,2,255],"bypassed":true},
            {"id":101,"type_id":"vendor.future","state_version":10,"payload":[0,42],"bypassed":false}
        ]);
        document["next_id"] = json!(102);
        let project = decode(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(project.authored_routing().is_empty());
        assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
        let mut current: Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
        remove_host_trim_fields(&mut current);
        remove_host_trim_fields(&mut document);
        assert_eq!(current["return_tracks"], document["return_tracks"]);
        let mut current_tracks = current["tracks"].clone();
        for track in current_tracks.as_array_mut().unwrap() {
            for key in ["gain_envelope", "gain_touch_spans", "gain_touch_epoch"] {
                track.as_object_mut().unwrap().remove(key);
            }
        }
        assert_eq!(current_tracks, document["tracks"]);
        if provenance {
            assert_eq!(current["import_provenance"], document["import_provenance"]);
        } else {
            assert!(current["import_provenance"].is_null());
        }
    }
}

#[test]
fn historical_shape_collisions_and_duplicate_or_null_keys_are_refused() {
    for document in [
        historical(18, false, true),
        historical(18, true, true),
        historical(19, true, false),
        historical(19, false, true),
    ] {
        assert!(
            decode(&serde_json::to_vec(&document).unwrap()).is_err(),
            "unexpected accepted shape: {document}"
        );
    }
    let mut null_routes = historical(19, true, true);
    null_routes["routing"] = Value::Null;
    assert!(decode(&serde_json::to_vec(&null_routes).unwrap()).is_err());
    let document = serde_json::to_string(&historical(19, true, true)).unwrap();
    let duplicate = format!("{},\"routing\":null}}", &document[..document.len() - 1]);
    assert!(decode(duplicate.as_bytes()).is_err());
    let duplicate = format!(
        "{},\"import_provenance\":null}}",
        &document[..document.len() - 1]
    );
    assert!(decode(duplicate.as_bytes()).is_err());
}

#[test]
fn schema19_checks_graph_before_closed_adoption() {
    let base = fixture();
    let track = owner("track", &base["tracks"][0]["id"]);
    let a = owner("return", &base["return_tracks"][0]["id"]);
    let b = owner("return", &base["return_tracks"][1]["id"]);
    let mut document = base.clone();
    document["routing"] = json!([edge(track.clone(), b.clone())]);
    refusal(&document, hex_project::ROUTING_UNSUPPORTED);
    document["routing"] = json!([edge(a.clone(), b.clone()), edge(b.clone(), a.clone())]);
    refusal(&document, "routing cycles are unsupported");
    document["routing"] = json!([edge(track.clone(), a)]);
    refusal(&document, "duplicate routing edge");
    document["routing"] = json!([edge(owner("track", &json!(0)), b.clone())]);
    refusal(&document, "routing owner no longer exists");
    document["routing"] = json!([edge(b, track)]);
    refusal(&document, "routing port direction is unsupported");
    assert_eq!(
        decode(&serde_json::to_vec(&base).unwrap())
            .unwrap()
            .tracks()[0]
            .sends()[0]
            .gain(),
        0.375
    );
}

#[test]
fn required_strict_bounded_typed_edges_refuse_malformed_and_unsupported_ports() {
    let base = fixture();
    let route = edge(
        owner("track", &base["tracks"][0]["id"]),
        owner("return", &base["return_tracks"][1]["id"]),
    );
    let mut document = base.clone();
    document.as_object_mut().unwrap().remove("routing");
    assert!(decode(&serde_json::to_vec(&document).unwrap()).is_err());
    for value in [
        json!(null),
        json!({}),
        json!([{"extra":true}]),
        json!(vec![route.clone(); hex_project::MAX_AUTHORED_ROUTES + 1]),
    ] {
        document = base.clone();
        document["routing"] = value;
        assert!(decode(&serde_json::to_vec(&document).unwrap()).is_err());
    }
    for signal in [
        json!({"kind":"midi"}),
        json!({"kind":"audio","channels":0}),
        json!({"kind":"audio","channels":1}),
        json!({"kind":"audio","channels":4}),
    ] {
        document = base.clone();
        document["routing"] = json!([route.clone()]);
        document["routing"][0]["signal"] = signal;
        refusal(&document, "routing requires supported stereo audio ports");
    }
    for malformed in [
        json!({"kind":"device","id":1}),
        json!({"kind":"sidechain","id":1}),
        json!({"kind":"track","id":1,"extra":true}),
    ] {
        document = base.clone();
        document["routing"] = json!([route.clone()]);
        document["routing"][0]["source"] = malformed;
        assert!(decode(&serde_json::to_vec(&document).unwrap()).is_err());
    }
    document = base.clone();
    document["routing"] = json!([route]);
    document["routing"][0]["extra"] = json!(true);
    assert!(decode(&serde_json::to_vec(&document).unwrap()).is_err());
}
