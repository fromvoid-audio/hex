//! Strict schema49 shared receivers/cycles and factual schema47/48 migration.
use hex_project::{Command, Editor, SessionAudioLoop, SessionCellContent};
use hex_storage::{decode, encode, load_with_token, save_new};
use serde_json::{Value, json};
#[allow(dead_code)]
mod common;
const OLD: &[u8] = include_bytes!("fixtures/native-sources/schema47-console-one-shot.json");
fn fixture() -> Editor {
    Editor::new(decode(OLD).unwrap(), 16).unwrap()
}
#[test]
fn schema47_migrates_every_field_and_preserves_original_document_and_media() {
    let project = decode(OLD).unwrap();
    let mut current: Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    assert_eq!(current["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    let rows = current
        .as_object_mut()
        .unwrap()
        .remove("session_audio_cycles")
        .unwrap();
    assert!(!rows.as_array().unwrap().is_empty());
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .all(|row| row["cycle"].is_null())
    );
    for source in current["native_sources"].as_array_mut().unwrap() {
        source.as_object_mut().unwrap().remove("receiver");
    }
    assert!(
        current
            .as_object_mut()
            .unwrap()
            .remove("device_automation")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    common::remove_empty_host_automation(&mut current);
    common::remove_empty_aliases(&mut current);
    current["version"] = 47.into();
    assert_eq!(
        current,
        serde_json::from_slice::<Value>(OLD).unwrap(),
        "every schema47 field must survive"
    );
    assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
    let folder = std::env::temp_dir().join(format!("hex-cycle-migration-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("old.hex");
    std::fs::write(&path, OLD).unwrap();
    let (loaded, _) = load_with_token(&path).unwrap();
    assert_eq!(loaded, project);
    assert_eq!(std::fs::read(&path).unwrap(), OLD);
    std::fs::remove_dir_all(folder).unwrap();
}
#[test]
fn looping_audio_roundtrips_complete_state_and_history_without_losing_tail() {
    let mut editor = fixture();
    let clip = editor
        .project()
        .session_cells()
        .iter()
        .find_map(|cell| match cell.content() {
            SessionCellContent::Audio(c) => Some(c.id()),
            _ => None,
        })
        .unwrap();
    let original = editor.project().clone();
    let range = Some(SessionAudioLoop::new(17, 71).unwrap());
    editor
        .execute(Command::SetSessionAudioClipLoop {
            clip,
            loop_range: range,
        })
        .unwrap();
    let changed = editor.project().clone();
    let source = changed.session_audio_clip(clip).unwrap();
    assert_eq!(
        (
            source.source_start(),
            source.source_end(),
            source.loop_range()
        ),
        (5, 100, range)
    );
    assert_eq!(decode(&encode(&changed).unwrap()).unwrap(), changed);
    assert!(hex_storage::encode_schema22(&changed).is_err());
    assert!(editor.undo());
    assert_eq!(editor.project(), &original);
    assert!(editor.redo());
    assert_eq!(editor.project(), &changed);
    let path = std::env::temp_dir().join(format!("hex-cycle-roundtrip-{}.hex", std::process::id()));
    save_new(&path, &changed).unwrap();
    assert_eq!(load_with_token(&path).unwrap().0, changed);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn current_cycle_inventory_is_mandatory_ordered_complete_and_bounded_by_source() {
    let valid: Value = serde_json::from_slice(&encode(fixture().project()).unwrap()).unwrap();
    for mutation in 0..9 {
        let mut value = valid.clone();
        match mutation {
            0 => {
                value
                    .as_object_mut()
                    .unwrap()
                    .remove("session_audio_cycles");
            }
            1 => value["session_audio_cycles"] = json!([]),
            2 => value["session_audio_cycles"][0]["clip"] = json!(999999),
            3 => {
                value["session_audio_cycles"][0]["cycle"] = json!({"start_frame":17,"end_frame":17})
            }
            4 => {
                value["session_audio_cycles"][0]["cycle"] =
                    json!({"start_frame":17,"end_frame":101})
            }
            5 => value["session_audio_cycles"][0]["cycle"] = json!({"start_frame":0,"end_frame":5}),
            6 => {
                value["session_audio_cycles"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("cycle");
            }
            7 => value["session_audio_cycles"][0]["extra"] = json!(0),
            8 => {
                let row = value["session_audio_cycles"][0].clone();
                value["session_audio_cycles"]
                    .as_array_mut()
                    .unwrap()
                    .push(row);
            }
            _ => panic!("case"),
        }
        assert!(
            decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "mutation {mutation}"
        );
    }
    let raw = String::from_utf8(encode(fixture().project()).unwrap()).unwrap();
    let duplicate = raw.replacen(
        "\"session_audio_cycles\":",
        "\"session_audio_cycles\":[],\"session_audio_cycles\":",
        1,
    );
    assert!(decode(duplicate.as_bytes()).is_err());
}

const OLD48: &[u8] = include_bytes!("fixtures/native-sources/schema48-console-loop-677a00f4.json");

#[test]
fn authentic_schema48_migrates_complete_loop_and_singleton_source_without_rewriting_bytes() {
    let old: Value = serde_json::from_slice(OLD48).unwrap();
    assert_eq!(old["version"], 48);
    assert_eq!(
        old["session_audio_cycles"][0]["cycle"],
        json!({"start_frame":17,"end_frame":71})
    );
    let project = decode(OLD48).unwrap();
    let source = &project.native_sources()[0];
    let contributor = project
        .tracks()
        .iter()
        .find(|track| track.source_id() == Some(source.id()))
        .unwrap();
    assert_eq!(source.receiver(), Some(contributor.id()));
    assert_eq!(project.source_contributors(source.id()).count(), 1);
    let mut current: Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    assert_eq!(current["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    for source in current["native_sources"].as_array_mut().unwrap() {
        source.as_object_mut().unwrap().remove("receiver");
    }
    assert!(
        current
            .as_object_mut()
            .unwrap()
            .remove("device_automation")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    common::remove_empty_host_automation(&mut current);
    common::remove_empty_aliases(&mut current);
    current["version"] = 48.into();
    assert_eq!(
        current, old,
        "every historical field, including cycles, must survive"
    );
    assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);

    let folder = std::env::temp_dir().join(format!(
        "hex-schema48-cycle-migration-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(folder.join("Media")).unwrap();
    let path = folder.join("old.hex");
    let media = folder.join("Media/metadata-only.wav");
    std::fs::write(&path, OLD48).unwrap();
    std::fs::write(&media, b"original media sentinel").unwrap();
    let (loaded, token) = load_with_token(&path).unwrap();
    assert_eq!(loaded, project);
    assert!(token.requires_format_upgrade());
    assert!(token.requires_save_confirmation(&loaded).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), OLD48);
    assert_eq!(std::fs::read(&media).unwrap(), b"original media sentinel");
    let reopened = folder.join("new49.hex");
    save_new(&reopened, &loaded).unwrap();
    assert_eq!(load_with_token(&reopened).unwrap().0, project);
    assert_eq!(std::fs::read(&path).unwrap(), OLD48);
    std::fs::remove_dir_all(folder).unwrap();

    let mut receiver_forgery = old.clone();
    receiver_forgery["native_sources"][0]["receiver"] = Value::Null;
    assert!(decode(&serde_json::to_vec(&receiver_forgery).unwrap()).is_err());
    let mut shared_forgery = old;
    shared_forgery["track_source_bindings"][2]["source"] =
        shared_forgery["native_sources"][0]["id"].clone();
    shared_forgery["track_source_bindings"][2]["instrument_position"] = 0.into();
    assert!(
        decode(&serde_json::to_vec(&shared_forgery).unwrap()).is_err(),
        "schema48 remains singleton-only"
    );
}

#[test]
fn schema49_shared_source_receiver_and_console_cycle_roundtrip_together_with_history() {
    let mut editor = Editor::new(decode(OLD48).unwrap(), 16).unwrap();
    let source = editor.project().native_sources()[0].id();
    let original_receiver = editor.project().native_source(source).unwrap().receiver();
    let contributor = editor.project().tracks()[2].id();
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: Some(contributor),
        })
        .unwrap();
    let shared = editor.project().clone();
    assert_eq!(shared.source_contributors(source).count(), 2);
    let bytes = encode(&shared).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(value["native_sources"][0]["receiver"], contributor.value());
    assert_eq!(
        value["session_audio_cycles"][0]["cycle"],
        json!({"start_frame":17,"end_frame":71})
    );
    assert_eq!(decode(&bytes).unwrap(), shared);
    assert!(editor.undo());
    assert_eq!(
        editor.project().native_source(source).unwrap().receiver(),
        original_receiver
    );
    assert!(editor.redo());
    assert_eq!(editor.project(), &shared);

    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    let unassigned = editor.project().clone();
    let bytes = encode(&unassigned).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value["native_sources"][0]["receiver"].is_null());
    assert_eq!(unassigned.source_contributors(source).count(), 2);
    assert_eq!(decode(&bytes).unwrap(), unassigned);
}

#[test]
fn schema49_receiver_is_mandatory_and_must_be_a_contributor_alongside_strict_cycles() {
    let valid: Value = serde_json::from_slice(&encode(&decode(OLD48).unwrap()).unwrap()).unwrap();
    for mutation in 0..5 {
        let mut value = valid.clone();
        match mutation {
            0 => {
                value["native_sources"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("receiver");
            }
            1 => value["native_sources"][0]["receiver"] = 999999.into(),
            2 => value["native_sources"][0]["receiver"] = value["tracks"][0]["id"].clone(),
            3 => value["native_sources"][0]["receiver"] = json!("7"),
            4 => value["version"] = 48.into(),
            _ => panic!("case"),
        }
        assert!(
            decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "mutation {mutation}"
        );
    }
    let raw = String::from_utf8(encode(&decode(OLD48).unwrap()).unwrap()).unwrap();
    let duplicate = raw.replacen("\"receiver\":", "\"receiver\":null,\"receiver\":", 1);
    assert!(decode(duplicate.as_bytes()).is_err());
}
