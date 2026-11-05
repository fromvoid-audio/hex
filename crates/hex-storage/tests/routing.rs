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
fn current_routing_preserves_sends_provenance_and_refuses_duplicate_or_null_keys() {
    let base = fixture();
    let project = decode(&serde_json::to_vec(&base).unwrap()).unwrap();
    assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
    assert_eq!(project.tracks()[0].sends()[0].gain(), 0.375);
    assert_eq!(
        project.import_provenance().unwrap().original_bytes(),
        &[0, 255, 42]
    );
    let mut null_routes = base.clone();
    null_routes["routing"] = Value::Null;
    assert!(decode(&serde_json::to_vec(&null_routes).unwrap()).is_err());
    let document = serde_json::to_string(&base).unwrap();
    for field in ["routing", "import_provenance"] {
        let duplicate = format!("{},\"{field}\":null}}", &document[..document.len() - 1]);
        assert!(decode(duplicate.as_bytes()).is_err());
    }
}

#[test]
fn current_routing_checks_feed_owners_duplicates_and_cycles_before_adoption() {
    let mut base = fixture();
    let a = base["return_tracks"][0]["id"].clone();
    let b = base["return_tracks"][1]["id"].clone();
    let id = base["next_id"].as_u64().unwrap();
    base["next_id"] = (id + 2).into();
    let feed = json!({"kind":"return_feed","id":id,"source":a,"destination":b,"gain":0.5});
    let mut document = base.clone();
    document["routing"] = json!([feed.clone()]);
    assert_eq!(
        decode(&serde_json::to_vec(&document).unwrap())
            .unwrap()
            .authored_routing()
            .len(),
        1
    );
    document["routing"][0]["destination"] = base["tracks"][0]["id"].clone();
    refusal(&document, "return feed owner no longer exists");
    document["routing"] = json!([feed.clone(), {"kind":"return_feed","id":id+1,"source":b,"destination":a,"gain":0.5}]);
    refusal(&document, "routing cycles are unsupported");
    document["routing"][1]["source"] = a;
    document["routing"][1]["destination"] = b;
    refusal(&document, "duplicate return feed");
    document["routing"] = json!([feed]);
    document["routing"][0]["source"] = 0.into();
    refusal(&document, "return feed owner no longer exists");
}

#[test]
fn current_routing_refuses_missing_unbounded_unknown_and_wrong_type_fields() {
    let mut base = fixture();
    let id = base["next_id"].as_u64().unwrap();
    base["next_id"] = (id + 1).into();
    let route = json!({"kind":"return_feed","id":id,
        "source":base["return_tracks"][0]["id"],
        "destination":base["return_tracks"][1]["id"],"gain":0.5});
    base["routing"] = json!([route.clone()]);
    decode(&serde_json::to_vec(&base).unwrap()).unwrap();
    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("routing");
    assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    for value in [json!(null), json!({}), json!([{"extra":true}])] {
        let mut malformed = base.clone();
        malformed["routing"] = value;
        assert!(decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
    }
    let mut unbounded = base.clone();
    unbounded["routing"] = json!(vec![route.clone(); hex_project::MAX_RETURN_FEEDS + 1]);
    let error = decode(&serde_json::to_vec(&unbounded).unwrap()).unwrap_err();
    assert!(error.to_string().contains("return feed limit reached"));
    for (field, value) in [
        ("source", json!({"kind":"track","id":1})),
        ("destination", json!("return")),
        ("kind", json!("midi")),
        ("signal", json!({"kind":"audio","channels":2})),
        ("unknown", json!(true)),
    ] {
        let mut malformed = base.clone();
        malformed["routing"][0][field] = value;
        assert!(matches!(
            decode(&serde_json::to_vec(&malformed).unwrap()),
            Err(StorageError::Json(_))
        ));
    }
    // Generic schema19 ports are unsupported shapes, not current serialized feeds.
    let mut generic = base;
    generic["routing"] = json!([edge(owner("track", &json!(1)), owner("return", &json!(2)))]);
    assert!(matches!(
        decode(&serde_json::to_vec(&generic).unwrap()),
        Err(StorageError::Json(_))
    ));
}
