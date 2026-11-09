//! Complete hierarchy persistence and evidenced schema43 migration.
use hex_project::{Command, Created, Editor, Project, TrackKind, TrackSelectionIds};
use hex_storage::{decode, encode};
use serde_json::{Value, json};
mod common;
const HISTORICAL: &[u8] =
    include_bytes!("fixtures/track-groups/schema43-flat-tracks-15957517.json");
fn fixture() -> Editor {
    let mut e = Editor::new(decode(HISTORICAL).unwrap(), 32).unwrap();
    let tracks = e
        .project()
        .tracks()
        .iter()
        .map(|t| t.id())
        .collect::<Vec<_>>();
    let Created::Track(group) = e
        .execute(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&tracks[..2]).unwrap(),
            name: "submix".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    e.execute(Command::GroupTracks {
        tracks: TrackSelectionIds::new(&[group]).unwrap(),
        name: "outer".into(),
    })
    .unwrap();
    e
}
#[test]
fn actual_schema43_migrates_all_fields_without_rewriting_sources() {
    let original: Value = serde_json::from_slice(HISTORICAL).unwrap();
    assert_eq!(original["version"], 43);
    let project = decode(HISTORICAL).unwrap();
    assert!(
        project
            .tracks()
            .iter()
            .all(|t| t.kind() == TrackKind::Ordinary && t.parent_group().is_none())
    );
    let mut current: Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    assert_eq!(current["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(
        common::restore_pre_binding_instruments(&mut current),
        original["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|track| !track["instrument"].is_null())
            .count()
    );
    let rows = current
        .as_object_mut()
        .unwrap()
        .remove("track_hierarchy")
        .unwrap();
    assert_eq!(rows.as_array().unwrap().len(), project.tracks().len());
    current["version"] = 43.into();
    assert_eq!(
        current, original,
        "every prior-format field must survive migration"
    );
    assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
}
#[test]
fn nested_groups_roundtrip_complete_state_and_explicit_downgrade_refuses() {
    let mut e = fixture();
    let original = e.project().clone();
    let saved = encode(&original).unwrap();
    assert_eq!(decode(&saved).unwrap(), original);
    assert!(hex_storage::encode_schema20(&original).is_err());
    assert!(hex_storage::encode_schema22(&original).is_err());
    assert!(e.undo());
    assert!(e.redo());
    assert_eq!(*e.project(), original);
    assert_eq!(decode(&encode(e.project()).unwrap()).unwrap(), original);
}
#[test]
fn hierarchy_inventory_is_required_exact_ordered_bounded_and_strict() {
    let original: Value = serde_json::from_slice(&encode(fixture().project()).unwrap()).unwrap();
    let mut cases = Vec::new();
    let mut v = original.clone();
    v.as_object_mut().unwrap().remove("track_hierarchy");
    cases.push(v);
    let mut v = original.clone();
    v["track_hierarchy"].as_array_mut().unwrap().pop();
    cases.push(v);
    let mut v = original.clone();
    v["track_hierarchy"].as_array_mut().unwrap().swap(0, 1);
    cases.push(v);
    let mut v = original.clone();
    v["track_hierarchy"][0]["parent"] = v["track_hierarchy"][0]["track"].clone();
    cases.push(v);
    let mut v = original.clone();
    v["track_hierarchy"][0]["group"] = false.into();
    cases.push(v);
    let mut v = original.clone();
    v["track_hierarchy"][1]
        .as_object_mut()
        .unwrap()
        .remove("parent");
    cases.push(v);
    let mut v = original.clone();
    v["track_hierarchy"][0]["extra"] = true.into();
    cases.push(v);
    let mut v = original.clone();
    v["track_hierarchy"][1]["track"] = v["track_hierarchy"][0]["track"].clone();
    cases.push(v);
    let mut v = original.clone();
    v["track_hierarchy"] = Value::Array(vec![
        json!({"track":1,"group":false,"parent":null});
        hex_project::MAX_TRACKS + 1
    ]);
    cases.push(v);
    let mut v = original.clone();
    v["version"] = (hex_storage::NEWEST_SCHEMA_VERSION + 1).into();
    cases.push(v);
    for (index, v) in cases.into_iter().enumerate() {
        assert!(
            decode(&serde_json::to_vec(&v).unwrap()).is_err(),
            "malformed case {index}"
        );
    }
    // Nullable parent must occur exactly once, too.
    let raw = String::from_utf8(serde_json::to_vec(&original).unwrap()).unwrap();
    assert!(
        decode(
            raw.replacen("\"parent\":null", "\"parent\":null,\"parent\":null", 1)
                .as_bytes()
        )
        .is_err()
    );
}
#[test]
fn leaf_content_cannot_be_reinterpreted_as_a_group() {
    let project = decode(HISTORICAL).unwrap();
    let mut value: Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    value["track_hierarchy"][0]["group"] = true.into();
    assert!(decode(&serde_json::to_vec(&value).unwrap()).is_err());
    assert_eq!(decode(HISTORICAL).unwrap(), project);
    assert!(Project::from_snapshot(fixture().project().snapshot()).is_ok());
}
