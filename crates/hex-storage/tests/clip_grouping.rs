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
    assert_eq!(json["version"], 41);
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
            5 => state["sources"][0]["document"]["version"] = 41.into(),
            6 => state["events"][0]["source"] = 0.into(),
            _ => state["extra"] = true.into(),
        }
        assert!(
            decode(&bytes(&bad)).is_err(),
            "accepted malformed inventory {kind}"
        );
    }
}
