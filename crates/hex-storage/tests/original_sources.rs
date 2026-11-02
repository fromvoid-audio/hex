use hex_project::{Command, Created, Editor, OriginalSourceFacts, Project};
use hex_storage::{decode, encode};
use serde_json::{Value, json};

fn project(original: bool) -> Project {
    let mut editor = Editor::new(Project::new(16000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "original.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let mut snapshot = editor.project().snapshot();
    assert_eq!(snapshot.assets[0].id, asset.value());
    // Storage carries expected metadata only; this synthetic claim is neither
    // an actual source catalog nor an owning native admission capability.
    snapshot.assets[0].original_source =
        original.then(|| OriginalSourceFacts::new(1, [42; 32]).unwrap());
    Project::from_snapshot(snapshot).unwrap()
}
#[test]
fn current_source_inventory_preserves_original_kind_and_exact_expected_digest() {
    for original in [false, true] {
        let project = project(original);
        let bytes = encode(&project).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["version"], hex_storage::schema_version_for(&project));
        let row = &value["asset_sources"][0];
        assert_eq!(row["asset"], project.assets().next().unwrap().id().value());
        if original {
            assert_eq!(row["original"]["law_version"], 1);
            assert_eq!(row["original"]["content_digest"], json!(vec![42; 32]));
        } else {
            assert!(row["original"].is_null());
        }
        assert_eq!(decode(&bytes).unwrap(), project);
        if original {
            assert!(hex_storage::encode_schema20(&project).is_err());
            assert!(hex_storage::encode_schema22(&project).is_err());
        }
    }
}
#[test]
fn absent_ambiguous_unknown_or_malformed_original_source_claims_refuse() {
    let valid: Value = serde_json::from_slice(&encode(&project(true)).unwrap()).unwrap();
    for case in 0..9 {
        let mut bad = valid.clone();
        match case {
            0 => {
                bad.as_object_mut().unwrap().remove("asset_sources");
            }
            1 => bad["asset_sources"] = json!([]),
            2 => {
                bad["asset_sources"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("original");
            }
            3 => bad["asset_sources"][0]["original"]["law_version"] = json!(2),
            4 => bad["asset_sources"][0]["original"]["content_digest"] = json!(vec![42; 31]),
            5 => bad["asset_sources"][0]["original"]["content_digest"] = json!(vec![42; 33]),
            6 => {
                let row = bad["asset_sources"][0].clone();
                bad["asset_sources"].as_array_mut().unwrap().push(row);
            }
            7 => bad["asset_sources"][0]["asset"] = json!(999),
            _ => bad["version"] = json!(33),
        }
        assert!(
            decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "case {case}"
        );
    }
}
#[test]
fn expected_device_original_metadata_cannot_become_an_unaccounted_clip_derivative() {
    let mut editor = Editor::new(project(true), 8).unwrap();
    let asset = editor.project().assets().next().unwrap().id();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: 8,
                source_offset: 0,
                source_length: 8,
                looping: false,
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
}
