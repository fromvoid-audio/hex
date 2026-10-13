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
    serde_json::from_slice(&encode(editor.project()).unwrap()).unwrap()
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
fn schema19_roundtrip_and_schema18_migration_preserve_sends_provenance_and_opaque_states() {
    let mut document = fixture();
    assert_eq!(document["version"], 19);
    assert_eq!(document["routing"], json!([]));
    // Unknown vendor formats and reserved device-state versions are opaque here.
    document["return_tracks"][0]["devices"] = json!([
        {"id":100,"type_id":"vendor.future","state_version":9,"payload":[1,2,255],"bypassed":true},
        {"id":101,"type_id":"vendor.future","state_version":10,"payload":[0,42],"bypassed":false}
    ]);
    document["next_id"] = json!(102);
    let project = decode(&serde_json::to_vec(&document).unwrap()).unwrap();
    assert!(project.authored_routing().is_empty());
    assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
    let current: Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    assert_eq!(current["return_tracks"], document["return_tracks"]);
    assert_eq!(current["tracks"], document["tracks"]);
    assert_eq!(current["import_provenance"], document["import_provenance"]);
    document["version"] = json!(18);
    assert!(decode(&serde_json::to_vec(&document).unwrap()).is_err());
    document.as_object_mut().unwrap().remove("routing");
    assert_eq!(
        decode(&serde_json::to_vec(&document).unwrap()).unwrap(),
        project
    );
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
