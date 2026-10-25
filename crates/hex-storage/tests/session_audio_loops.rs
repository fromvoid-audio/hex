//! Strict schema48 cycles and factual schema47 one-shot migration.
use hex_project::{Command, Editor, SessionAudioLoop, SessionCellContent};
use hex_storage::{decode, encode, load_with_token, save_new};
use serde_json::{Value, json};
const OLD: &[u8] = include_bytes!("fixtures/native-sources/schema47-console-one-shot.json");
fn fixture() -> Editor {
    Editor::new(decode(OLD).unwrap(), 16).unwrap()
}
#[test]
fn schema47_migrates_every_field_and_preserves_original_document_and_media() {
    let project = decode(OLD).unwrap();
    let mut current: Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    assert_eq!(current["version"], 48);
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
