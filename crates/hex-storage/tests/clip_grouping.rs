//! Current-format retained sources, strict inventories and prior-schema migration.
use hex_project::{AudioEventEdit, ClipId, Command, Created, Editor, Project};
use hex_storage::{decode, encode};
use serde_json::{Value, json};
fn fixture() -> (Editor, Vec<ClipId>) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { track, clip: a, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "a".into(),
            path: "Media/A.wav".into(),
            sample_rate: 48_000,
            frames: 200,
        })
        .unwrap()
    else {
        panic!()
    };
    e.execute(Command::EditAudioEvents {
        clip: a,
        operation: AudioEventEdit::Split { at: 70 },
    })
    .unwrap();
    e.execute(Command::SetAudioClipGain {
        clip: a,
        millidb: -3000,
    })
    .unwrap();
    let Created::Asset(asset) = e
        .execute(Command::AddAsset {
            path: "Media/B.wav".into(),
            sample_rate: 48_000,
            frames: 150,
        })
        .unwrap()
    else {
        panic!()
    };
    let Created::Clip(b) = e
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 400,
            duration: 200,
            source_offset: 20,
            source_length: 100,
            looping: true,
        })
        .unwrap()
    else {
        panic!()
    };
    e.execute(Command::SetClipFades {
        clip: b,
        fade_in: 30,
        fade_out: 40,
    })
    .unwrap();
    (Editor::new(e.project().clone(), 16).unwrap(), vec![a, b])
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
#[test]
fn grouped_sources_roundtrip_complete_original_state_and_missing_media_paths() {
    let (mut e, ids) = fixture();
    let original = e.project().clip(ids[0]).unwrap().retained_snapshot();
    let Created::Clip(group) = e.execute(Command::GroupClips(ids)).unwrap() else {
        panic!()
    };
    let encoded = encode(e.project()).unwrap();
    let json: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(json["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    let reopened = decode(&encoded).unwrap();
    assert_eq!(reopened, *e.project());
    let roots = reopened.clip(group).unwrap().audio_events().sources;
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0].clip.retained_snapshot(), original);
    assert_eq!(reopened.assets().count(), 2);
    assert!(hex_storage::encode_schema22(&reopened).is_err());
    let dir = std::env::temp_dir().join(format!(
        "hex-grouping-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("group.hex");
    {
        hex_storage::save_new(&path, &reopened).unwrap();
        assert_eq!(hex_storage::load(&path).unwrap(), reopened);
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(dir).unwrap();
}
#[test]
fn schema40_migrates_in_memory_with_single_root_defaults() {
    let (e, _) = fixture();
    let mut value: Value = serde_json::from_slice(&encode(e.project()).unwrap()).unwrap();
    value["version"] = 40.into();
    value.as_object_mut().unwrap().remove("grouped_audio");
    value.as_object_mut().unwrap().remove("content_views");
    let original = bytes(&value);
    let loaded = decode(&original).unwrap();
    assert_eq!(loaded, *e.project());
    assert_eq!(bytes(&value), original);
    for clip in loaded.tracks().iter().flat_map(|t| t.clips()) {
        let events = clip.audio_events();
        assert!(events.sources.is_empty());
        assert!(events.events.iter().all(|e| e.source == 0));
    }
    assert_eq!(decode(&encode(&loaded).unwrap()).unwrap(), loaded);
}
#[test]
fn malformed_group_sources_refuse_missing_foreign_or_unrelated_state() {
    let (mut e, ids) = fixture();
    e.execute(Command::GroupClips(ids)).unwrap();
    let value: Value = serde_json::from_slice(&encode(e.project()).unwrap()).unwrap();
    for kind in 0..8 {
        let mut bad = value.clone();
        let state = &mut bad["grouped_audio"][0];
        match kind {
            0 => state["events"][0]["source"] = 999.into(),
            1 => state["events"][0]["source_start"] = u64::MAX.into(),
            2 => state["sources"][1]["id"] = state["sources"][0]["id"].clone(),
            3 => state["sources"][0]["document"]["assets"][0]["path"] = json!("other.wav"),
            4 => state["sources"][0]["document"]["master_gain"] = json!(0.5),
            5 => state["sources"][0]["document"]["version"] = 42.into(),
            6 => state["events"][0]["source"] = 0.into(),
            _ => state["extra"] = true.into(),
        }
        assert!(
            decode(&bytes(&bad)).is_err(),
            "accepted malformed inventory {kind}"
        );
    }
}

#[test]
fn grouping_preserves_directed_child_and_container_views_through_current_codec() {
    let (mut e, ids) = fixture();
    e.execute(Command::ReverseClips(ids.clone())).unwrap();
    let original = e.project().snapshot();
    let Created::Clip(group) = e.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group");
    };
    e.execute(Command::ReverseClips(vec![group])).unwrap();
    let data = encode(e.project()).unwrap();
    assert_eq!(decode(&data).unwrap().snapshot(), e.project().snapshot());
    assert!(e.undo());
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = original.next_id; // IDs remain monotonic across Undo.
    assert_eq!(restored, original);
}

#[test]
fn both_evidenced_schema41_producers_migrate_without_losing_inventory() {
    for data in [
        include_bytes!("fixtures/grouped-audio/schema41-group-04d5ee56.json").as_slice(),
        include_bytes!("fixtures/grouped-audio/schema41-content-08dfb3a0.json").as_slice(),
    ] {
        let original = data.to_vec();
        let old: Value = serde_json::from_slice(data).unwrap();
        let project = decode(data).unwrap();
        let current = encode(&project).unwrap();
        assert_eq!(decode(&current).unwrap(), project);
        let mut upgraded: Value = serde_json::from_slice(&current).unwrap();
        upgraded["version"] = 41.into();
        if old.get("grouped_audio").is_some() {
            upgraded.as_object_mut().unwrap().remove("content_views");
            // Current retained roots use strict41; the historical writer used
            // strict40. The actual original fixtures validate both envelopes.
            for row in upgraded["grouped_audio"].as_array_mut().unwrap() {
                for root in row["sources"].as_array_mut().unwrap() {
                    root["document"]["version"] = 40.into();
                    root["document"]
                        .as_object_mut()
                        .unwrap()
                        .remove("content_views");
                }
            }
        } else {
            upgraded.as_object_mut().unwrap().remove("grouped_audio");
        }
        assert_eq!(upgraded, old, "migration changed existing saved state");
        assert_eq!(data, original);
        let mut ambiguous = old.clone();
        if ambiguous.get("grouped_audio").is_some() {
            ambiguous["content_views"] = json!([]);
        } else {
            ambiguous["grouped_audio"] = json!([]);
        }
        assert!(decode(&bytes(&ambiguous)).is_err());
    }
}

#[test]
fn embedded_root_schema_matches_the_actual_outer_writer_lineage() {
    let (mut e, ids) = fixture();
    e.execute(Command::GroupClips(ids)).unwrap();
    let current: Value = serde_json::from_slice(&encode(e.project()).unwrap()).unwrap();
    let mut old_root_in_current = current.clone();
    let document = &mut old_root_in_current["grouped_audio"][0]["sources"][0]["document"];
    document["version"] = 40.into();
    document.as_object_mut().unwrap().remove("content_views");
    assert!(decode(&bytes(&old_root_in_current)).is_err());
    let mut old: Value = serde_json::from_slice(include_bytes!(
        "fixtures/grouped-audio/schema41-group-04d5ee56.json"
    ))
    .unwrap();
    // Construct a valid main41 child using the complete decoded historical root.
    // It remains invalid in the draft41 outer envelope, whose writer used40.
    let old_document = &old["grouped_audio"][0]["sources"][0]["document"];
    let root = decode(&bytes(old_document)).unwrap();
    let mut newer_document: Value = serde_json::from_slice(&encode(&root).unwrap()).unwrap();
    newer_document["version"] = 41.into();
    newer_document
        .as_object_mut()
        .unwrap()
        .remove("grouped_audio");
    assert!(decode(&bytes(&newer_document)).is_ok());
    old["grouped_audio"][0]["sources"][0]["document"] = newer_document;
    assert!(decode(&bytes(&old)).is_err());
}
