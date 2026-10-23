use hex_project::{Command, Editor, Project};
use hex_storage::{decode, encode, encode_schema20, encode_schema22};
use hex_tuning::{Audition, ProjectTuning, Scale, Waveform};
use serde_json::{Value, json};

fn tuned(enabled: bool, waveform: Waveform) -> Project {
    let scale = Scale::new(7, *Scale::default().cents(), 57, 432.0, 3, 1300.0).unwrap();
    let tuning = ProjectTuning::new(
        enabled,
        scale,
        Audition::new(waveform, 0.7, 15.0, 550.0).unwrap(),
    )
    .unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor.execute(Command::SetTuning(tuning)).unwrap();
    editor.project().clone()
}
fn document() -> Value {
    serde_json::from_slice(&encode(&tuned(true, Waveform::Saw)).unwrap()).unwrap()
}
fn refuses(value: &Value) {
    assert!(decode(&serde_json::to_vec(value).unwrap()).is_err());
}

#[test]
fn required_current_tuning_roundtrips_every_waveform_enabled_and_disabled() {
    for enabled in [false, true] {
        for waveform in [Waveform::Triangle, Waveform::Square, Waveform::Saw] {
            let project = tuned(enabled, waveform);
            let bytes = encode(&project).unwrap();
            let doc: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(doc["version"], hex_storage::CURRENT_SCHEMA_VERSION);
            assert_eq!(
                doc["tuning"]["scale"]["cents"].as_array().unwrap().len(),
                48
            );
            assert_eq!(decode(&bytes).unwrap(), project);
            assert!(encode_schema20(&project).is_err());
            assert!(encode_schema22(&project).is_err());
        }
    }
    let neutral = Project::new(48_000).unwrap();
    assert_eq!(decode(&encode(&neutral).unwrap()).unwrap(), neutral);
    assert!(encode_schema20(&neutral).is_ok());
    assert!(encode_schema22(&neutral).is_ok());
}

#[test]
fn older_alpha_tuning_documents_are_explicitly_unsupported() {
    for version in 1..34 {
        let mut value = document();
        value["version"] = json!(version);
        refuses(&value);
    }
}

#[test]
fn malformed_tuning_is_refused_without_defaults_or_clamping() {
    let base = document();
    for path in ["tuning", "enabled", "scale", "audition"] {
        let mut bad = base.clone();
        if path == "tuning" {
            bad.as_object_mut().unwrap().remove(path);
        } else {
            bad["tuning"].as_object_mut().unwrap().remove(path);
        }
        refuses(&bad);
    }
    for (path, value) in [
        (vec!["tuning"], Value::Null),
        (vec!["tuning", "enabled"], json!(1)),
        (vec!["tuning", "scale", "count"], json!(2)),
        (vec!["tuning", "scale", "count"], json!(49)),
        (vec!["tuning", "scale", "root_key"], json!(128)),
        (vec!["tuning", "scale", "root_hz"], json!(0)),
        (vec!["tuning", "scale", "root_degree"], json!(7)),
        (vec!["tuning", "scale", "period_cents"], json!(-1)),
        (vec!["tuning", "scale", "cents"], json!([0, 100, 200])),
        (vec!["tuning", "scale", "cents"], json!(vec![0; 49])),
        (vec!["tuning", "audition", "waveform"], json!("sine")),
        (vec!["tuning", "audition", "level"], json!(1.1)),
        (vec!["tuning", "audition", "attack_ms"], json!(0)),
        (vec!["tuning", "audition", "release_ms"], json!(-1)),
    ] {
        let mut bad = base.clone();
        let mut target = &mut bad;
        for key in path {
            target = &mut target[key];
        }
        *target = value;
        refuses(&bad);
    }
    for object in [
        vec!["tuning"],
        vec!["tuning", "scale"],
        vec!["tuning", "audition"],
    ] {
        let mut bad = base.clone();
        let mut target = &mut bad;
        for key in object {
            target = &mut target[key];
        }
        target["library_name"] = json!("local preset");
        refuses(&bad);
    }
    let mut bad = base;
    bad["tuning"]["scale"]["cents"][1] = json!(0);
    refuses(&bad);
}

#[test]
fn original_token_duplicates_are_refused_at_every_tuning_level() {
    let raw = String::from_utf8(encode(&tuned(true, Waveform::Saw)).unwrap()).unwrap();
    for (needle, replacement) in [
        ("\"tuning\":{", "\"tuning\":null,\"tuning\":{"),
        ("\"enabled\":true", "\"enabled\":false,\"enabled\":true"),
        ("\"count\":7", "\"count\":12,\"count\":7"),
        ("\"level\":0.7", "\"level\":0.2,\"level\":0.7"),
    ] {
        let duplicate = raw.replace(needle, replacement);
        assert_ne!(duplicate, raw);
        assert!(decode(duplicate.as_bytes()).is_err());
    }
}

#[test]
fn fractional_reference_frequencies_roundtrip_without_changing_the_tuning() {
    // Authored decimal references, equal-tempered note references, and a
    // one-ulp adjustment must survive saving without shifting canonical state.
    for root_hz in [
        432.123_456_789,
        261.625_565_300_598_6,
        466.163_761_518_089_9,
        440.000_000_000_000_06,
        440.000_000_011_720_83,
        123.456_789_101_112_13,
    ] {
        let mut scale = Scale::default();
        scale.set_reference(60, root_hz, 0).unwrap();
        let tuning = ProjectTuning::new(true, scale, Audition::default()).unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor.execute(Command::SetTuning(tuning)).unwrap();
        let reopened = decode(&encode(editor.project()).unwrap()).unwrap();
        assert_eq!(reopened.tuning().scale().root_hz(), root_hz);
        assert_eq!(reopened, *editor.project());
    }
}
