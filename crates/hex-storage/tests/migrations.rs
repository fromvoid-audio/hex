//! Historical files emitted by the unmodified public encoders at the full Git
//! revisions below. The artifact harness created 44.1kHz audio referencing absent
//! Media/Missing.WAV, Arrangement/Session MIDI, host trims and opaque effects.
//! Only workspace membership and the example harness differed from those sources.
//! Rebuilt owning crates between sources (shared dependency cache only).
//! Schema36 reused a version in two branches: absence of sections identifies the
//! valid tempo branch; deletion of that whole inventory cannot be distinguished.
use hex_project::{Command, Editor};
use hex_storage::{CURRENT_SCHEMA_VERSION, decode, encode, load_with_token, save_new};
use serde_json::{Value, json};

// 85e6dfe2181b9b1d9ad5bc3af34843b179a536a5
const V34: &[u8] = include_bytes!("fixtures/migrations/schema34-85e6dfe2.json");
// 8d2e61abe85712683961c9980bdfbb593bb6bdfc
const V35: &[u8] = include_bytes!("fixtures/migrations/schema35-8d2e61ab.json");
// aea1275ca05fc3553815b384567b772473527d3f (global tempo branch)
const V36_TEMPO: &[u8] = include_bytes!("fixtures/migrations/schema36-aea1275c.json");
// 97e885eb322e6c75c88a10dbf3d35ce994746099 (sections branch)
const V36_SECTIONS: &[u8] = include_bytes!("fixtures/migrations/schema36-97e885eb.json");
// Same original encoders, with their own persistence/section fixture builders:
// edited/looped audio, automation and retained section processing contexts.
const AUDIO_FIXTURES: [&[u8]; 4] = [
    include_bytes!("fixtures/migrations/schema34-audio-85e6dfe2.json"),
    include_bytes!("fixtures/migrations/schema35-audio-8d2e61ab.json"),
    include_bytes!("fixtures/migrations/schema36-audio-aea1275c.json"),
    include_bytes!("fixtures/migrations/schema36-audio-97e885eb.json"),
];
const FIXTURES: [&[u8]; 4] = [V34, V35, V36_TEMPO, V36_SECTIONS];
fn value(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn assert_centered_fade_defaults(current: &mut Value, old: &Value) {
    let expected: Vec<_> = old["fade_shapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| json!({"clip":row["clip"],"in_position":500,"out_position":500}))
        .collect();
    assert_eq!(
        current
            .as_object_mut()
            .unwrap()
            .remove("fade_curve_positions"),
        Some(json!(expected))
    );
}

#[test]
fn historical_encoders_upgrade_without_losing_any_saved_field() {
    for original in FIXTURES {
        let old = value(original);
        let project = decode(original).unwrap();
        let current_bytes = encode(&project).unwrap();
        assert_eq!(decode(&current_bytes).unwrap(), project);
        let mut current = value(&current_bytes);
        assert_eq!(current["version"], CURRENT_SCHEMA_VERSION);
        current["version"] = old["version"].clone();
        assert_centered_fade_defaults(&mut current, &old);
        if old.get("arrangement_sections").is_none() {
            assert_eq!(
                current
                    .as_object_mut()
                    .unwrap()
                    .remove("arrangement_sections"),
                Some(json!([]))
            );
        }
        if old.get("midi_clip_volumes").is_none() {
            let volumes = current
                .as_object_mut()
                .unwrap()
                .remove("midi_clip_volumes")
                .unwrap();
            assert_eq!(volumes["clips"].as_array().unwrap().len(), 2);
            assert!(
                volumes["clips"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|row| row["points"] == json!([]))
            );
        }
        assert_eq!(
            current, old,
            "every old field including opaque bytes, source paths, IDs and inventories survives"
        );
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        editor.execute(Command::MasterGain { gain: 0.25 }).unwrap();
        assert!(editor.undo());
        assert_eq!(*editor.project(), project);
        assert!(editor.redo());
        assert_eq!(
            decode(&encode(editor.project()).unwrap()).unwrap(),
            *editor.project()
        );
    }
}

#[test]
fn supplied_volume_and_section_state_survives_upgrade() {
    let mut old = value(V36_SECTIONS);
    let next = old["next_id"].as_u64().unwrap();
    old["arrangement_sections"] = json!([{"id":next,"name":"Original Chorus","color":"rose","start_raw":0,"end_raw":4_u64<<32}]);
    old["next_id"] = (next + 1).into();
    for row in old["midi_clip_volumes"]["clips"].as_array_mut().unwrap() {
        row["points"] = json!([{"beat_raw":0,"incoming":0.375,"value":0.375}]);
    }
    let mut encoded = value(&encode(&decode(&bytes(&old)).unwrap()).unwrap());
    assert_centered_fade_defaults(&mut encoded, &old);
    old["version"] = CURRENT_SCHEMA_VERSION.into();
    assert_eq!(encoded, old);
}

#[test]
fn migration_never_defaults_missing_required_fields_or_discards_unknown_data() {
    for fixture in FIXTURES {
        let original = value(fixture);
        for field in [
            "tuning",
            "asset_sources",
            "device_assets",
            "main_devices",
            "tracks",
            "audio_clip_state",
        ] {
            let mut bad = original.clone();
            assert!(bad.as_object_mut().unwrap().remove(field).is_some());
            assert!(decode(&bytes(&bad)).is_err(), "missing {field}");
        }
        let mut bad = original.clone();
        bad["future_data"] = json!({"retain":42});
        assert!(decode(&bytes(&bad)).is_err());
        for field in ["tuning", "asset_sources", "device_assets", "main_devices"] {
            let mut bad = original.clone();
            bad[field] = Value::Null;
            assert!(decode(&bytes(&bad)).is_err(), "null {field}");
        }
        let text = std::str::from_utf8(fixture).unwrap();
        let duplicate = text.replacen(
            "\"sample_rate\":44100",
            "\"sample_rate\":44100,\"sample_rate\":44100",
            1,
        );
        assert_ne!(duplicate, text);
        assert!(decode(duplicate.as_bytes()).is_err());
        assert!(decode(&fixture[..fixture.len() / 2]).is_err());
    }
    let mut bad = value(V35);
    bad.as_object_mut().unwrap().remove("midi_clip_volumes");
    assert!(decode(&bytes(&bad)).is_err());
    let mut bad = value(V36_SECTIONS);
    bad["arrangement_sections"] = Value::Null;
    assert!(decode(&bytes(&bad)).is_err());
    let mut wrong_lineage = value(V34);
    wrong_lineage["midi_clip_volumes"] = json!({"clips":[]});
    assert!(decode(&bytes(&wrong_lineage)).is_err());
    let mut future = value(V35);
    future["version"] = (CURRENT_SCHEMA_VERSION + 1).into();
    assert!(
        decode(&bytes(&future))
            .unwrap_err()
            .to_string()
            .contains("newer schema")
    );
}

#[test]
fn open_and_save_as_preserve_original_and_migration_token_custody() {
    let dir = std::env::temp_dir().join(format!(
        "hex-migrations-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let original = dir.join("original.hex");
    let destination = dir.join("new-version.hex");
    std::fs::write(&original, V34).unwrap();
    let (project, token) = load_with_token(&original).unwrap();
    assert!(token.requires_format_upgrade());
    token.check_unchanged(&original).unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), V34);
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        1,
        "open never writes a conversion or media"
    );
    save_new(&destination, &project).unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), V34);
    let (reopened, current_token) = load_with_token(&destination).unwrap();
    assert_eq!(reopened, project);
    assert!(!current_token.requires_format_upgrade());
    assert!(save_new(&original, &project).is_err());
    assert_eq!(std::fs::read(&original).unwrap(), V34);
    std::fs::write(&original, b"external edit").unwrap();
    assert!(token.check_unchanged(&original).is_err());
    assert!(load_with_token(&original).is_err());
    assert_eq!(std::fs::read(&original).unwrap(), b"external edit");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn historical_edited_audio_and_automation_keep_every_value() {
    for fixture in AUDIO_FIXTURES {
        let mut expected = value(fixture);
        let sections = expected.get("arrangement_sections").is_some();
        for row in expected["audio_clip_state"].as_array_mut().unwrap() {
            let edit = &mut row["audio_edit"];
            if edit.is_null() {
                continue;
            }
            let edit = edit.as_object_mut().unwrap();
            if let Some(start) = edit.remove("musical_start_raw") {
                edit.insert(
                    "timing".into(),
                    if start.is_null() {
                        json!({"kind":"frames"})
                    } else {
                        json!({"kind":"musical","start_raw":start})
                    },
                );
            }
            if !sections {
                edit.insert("processing_projection".into(), Value::Null);
                edit.insert("placement_start_offset".into(), 0.into());
                edit.insert("placement_end_offset".into(), 0.into());
            }
        }
        if !sections {
            expected["arrangement_sections"] = json!([]);
            for track in expected["tracks"].as_array_mut().unwrap() {
                for point in track["gain_envelope"].as_array_mut().unwrap() {
                    point["incoming_gain"] = Value::Null;
                }
                for span in track["gain_touch_spans"].as_array_mut().unwrap() {
                    span["source_start_raw"] = span["start_raw"].clone();
                }
            }
        }
        if expected.get("midi_clip_volumes").is_none() {
            expected["midi_clip_volumes"] = json!({"clips":[]});
        }
        expected["version"] = CURRENT_SCHEMA_VERSION.into();
        let project = decode(fixture).unwrap();
        let mut current = value(&encode(&project).unwrap());
        assert_centered_fade_defaults(&mut current, &expected);
        assert_eq!(current, expected);
        assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        editor.execute(Command::MasterGain { gain: 0.25 }).unwrap();
        assert!(editor.undo());
        assert_eq!(*editor.project(), project);
        assert!(editor.redo());
        assert_eq!(
            decode(&encode(editor.project()).unwrap()).unwrap(),
            *editor.project()
        );
    }
}

#[test]
fn historical_nested_fields_refuse_missing_unknown_or_mixed_layouts() {
    for fixture in AUDIO_FIXTURES {
        let original = value(fixture);
        let edit_index = original["audio_clip_state"]
            .as_array()
            .unwrap()
            .iter()
            .position(|row| !row["audio_edit"].is_null())
            .unwrap();
        let fields: Vec<_> = original["audio_clip_state"][edit_index]["audio_edit"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for field in fields {
            let mut bad = original.clone();
            bad["audio_clip_state"][edit_index]["audio_edit"]
                .as_object_mut()
                .unwrap()
                .remove(&field);
            assert!(
                decode(&bytes(&bad)).is_err(),
                "missing historical edit {field}"
            );
        }
        for pointer in [
            format!("/audio_clip_state/{edit_index}/audio_edit"),
            "/tracks/0/gain_envelope/0".into(),
            "/tracks/0/gain_touch_spans/0".into(),
        ] {
            let mut bad = original.clone();
            bad.pointer_mut(&pointer).unwrap()["unrecognized"] = json!(42);
            assert!(decode(&bytes(&bad)).is_err(), "unknown field at {pointer}");
        }
        let mut bad = original.clone();
        let edit = &mut bad["audio_clip_state"][edit_index]["audio_edit"];
        if edit.get("timing").is_some() {
            edit["musical_start_raw"] = Value::Null;
        } else {
            edit["timing"] = json!({"kind":"frames"});
        }
        assert!(decode(&bytes(&bad)).is_err(), "mixed timing schemas");
        let mut bad = original.clone();
        bad["tracks"][0]["gain_touch_spans"][0]["steps"][0]["gain"] = json!(-1);
        assert!(decode(&bytes(&bad)).is_err());
        let duplicate = std::str::from_utf8(fixture).unwrap().replacen(
            "\"gain_envelope\":",
            "\"gain_envelope\":[],\"gain_envelope\":",
            1,
        );
        assert_ne!(duplicate.as_bytes(), fixture);
        assert!(decode(duplicate.as_bytes()).is_err());
    }
}
