//! Actual early public writers, plus frozen original-reader oracles for1–12.
//! Fixture provenance distinguishes these two forms of historical evidence.
use hex_project::{Command, Editor};
use hex_storage::{decode, encode};
use serde_json::{Value, json};
mod common;

const WRITERS: &[&[u8]] = &[
    include_bytes!("fixtures/early/schema13-hybrid-89674b36.json"),
    include_bytes!("fixtures/early/schema14-effects-9efed7e1.json"),
    include_bytes!("fixtures/early/schema15-instrument-a5de1baa.json"),
    include_bytes!("fixtures/early/schema16-position-65410c68.json"),
    include_bytes!("fixtures/early/schema17-returns-583bfdc1.json"),
    include_bytes!("fixtures/early/schema18-provenance-173482a3.json"),
    include_bytes!("fixtures/early/schema18-output-dc6c050a.json"),
    include_bytes!("fixtures/early/schema20-inline-gain-b9274930.json"),
    include_bytes!("fixtures/early/schema21-inline-edit-a9da374e.json"),
    include_bytes!("fixtures/early/schema20-session-audio-b9274930.json"),
    include_bytes!("fixtures/early/schema21-session-audio-a9da374e.json"),
    include_bytes!("fixtures/early/schema22-touch-60c88034.json"),
    include_bytes!("fixtures/early/schema23-two-trim-c3076a24.json"),
    include_bytes!("fixtures/early/schema23-combined-3c7c786b.json"),
    include_bytes!("fixtures/early/schema24-routes-f7c73dd0.json"),
    include_bytes!("fixtures/early/schema25-audio-e9736fce.json"),
    include_bytes!("fixtures/early/schema26-names-9f381a6f.json"),
    include_bytes!("fixtures/early/schema26-events-309d41ae.json"),
    include_bytes!("fixtures/early/schema27-appearance-e409a8f2.json"),
    include_bytes!("fixtures/early/schema27-named-events-d8922776.json"),
    include_bytes!("fixtures/early/schema27-named-fades-0298bf88.json"),
    include_bytes!("fixtures/early/schema28-appearance-events-819a4cc4.json"),
    include_bytes!("fixtures/early/schema28-appearance-fades-e50ffea3.json"),
    include_bytes!("fixtures/early/schema29-combined-caed5306.json"),
    include_bytes!("fixtures/early/schema29-sample-bpm-a0cedd1d.json"),
    include_bytes!("fixtures/lineages/main19.json"),
    include_bytes!("fixtures/lineages/foundation19.json"),
    include_bytes!("fixtures/lineages/foundation20.json"),
    include_bytes!("fixtures/lineages/foundation21.json"),
    include_bytes!("fixtures/lineages/foundation22.json"),
];
const READERS: &[(&[u8], &[u8])] = &[
    (
        include_bytes!("fixtures/early/schema1-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema1-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema2-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema2-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema3-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema3-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema4-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema4-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema5-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema5-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema6-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema6-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema7-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema7-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema8-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema8-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema9-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema9-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema10-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema10-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema11-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema11-oracle13-89674b36.json"),
    ),
    (
        include_bytes!("fixtures/early/schema12-reader-89674b36.json"),
        include_bytes!("fixtures/early/schema12-oracle13-89674b36.json"),
    ),
];
fn value(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

// Compare every authored field. Historical timing is the sole renamed leaf;
// inline audio state is checked separately at its current owning inventory.
fn saved_values(old: &Value, current: &Value, path: &str) {
    match old {
        Value::Object(fields) => {
            for (key, before) in fields {
                if path.is_empty() && matches!(key.as_str(), "version" | "next_id") {
                    continue;
                }
                if key == "musical_start_raw" {
                    assert_eq!(
                        current["timing"],
                        if before.is_null() {
                            json!({"kind":"frames"})
                        } else {
                            json!({"kind":"musical","start_raw":before})
                        },
                        "timing {path}"
                    );
                } else {
                    let after = current
                        .get(key)
                        .unwrap_or_else(|| panic!("missing {path}/{key}"));
                    saved_values(before, after, &format!("{path}/{key}"));
                }
            }
        }
        Value::Array(rows) => {
            let after = current.as_array().unwrap();
            assert_eq!(rows.len(), after.len(), "inventory {path}");
            for (i, (before, after)) in rows.iter().zip(after).enumerate() {
                saved_values(before, after, &format!("{path}/{i}"));
            }
        }
        Value::Number(_)
            if matches!(
                path.rsplit('/').next(),
                Some("gain" | "balance" | "master_gain")
            ) =>
        {
            // Some original Value-based writers serialized widened f32 values;
            // the current direct writer uses the shortest round-tripping f32.
            // These model fields are f32: compare their exact bits, with no
            // tolerance or precision conversion for any other numeric field.
            assert_eq!(
                (old.as_f64().unwrap() as f32).to_bits(),
                (current.as_f64().unwrap() as f32).to_bits(),
                "saved f32 value {path}"
            );
        }
        _ => assert_eq!(old, current, "saved value {path}"),
    }
}
fn check_history_and_roundtrip(project: hex_project::Project) {
    assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor.execute(Command::MasterGain { gain: 0.625 }).unwrap();
    assert!(editor.undo());
    assert_eq!(*editor.project(), project);
    assert!(editor.redo());
    assert_eq!(
        decode(&encode(editor.project()).unwrap()).unwrap(),
        *editor.project()
    );
}
#[test]
fn original_writers_preserve_complete_saved_values_roundtrip_and_history() {
    for fixture in WRITERS {
        let mut old = value(fixture);
        let version = old["version"].as_u64().unwrap();
        let project = decode(fixture).unwrap_or_else(|err| panic!("schema{version}: {err}"));
        let mut current = value(&encode(&project).unwrap());
        common::restore_pre_binding_instruments(&mut current);
        if matches!(version, 20 | 21) && old.get("import_provenance").is_none() {
            for track in old["tracks"].as_array_mut().unwrap() {
                for clip in track["audio_clips"].as_array_mut().unwrap() {
                    let state = current["audio_clip_state"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|row| row["clip"] == clip["id"])
                        .unwrap();
                    assert_eq!(clip["gain_millidb"], state["gain_millidb"]);
                    clip.as_object_mut().unwrap().remove("gain_millidb");
                    if let Some(edit) = clip.as_object_mut().unwrap().remove("audio_edit") {
                        saved_values(&edit, &state["audio_edit"], "inline audio edit");
                    }
                }
            }
            for cell in old["session"]["cells"].as_array_mut().unwrap() {
                if cell["content"]["kind"] == "audio" {
                    let clip = &mut cell["content"]["clip"];
                    let state = current["audio_clip_state"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|row| row["clip"] == clip["id"])
                        .unwrap();
                    assert_eq!(clip["gain_millidb"], state["gain_millidb"]);
                    assert!(state["audio_edit"].is_null());
                    clip.as_object_mut().unwrap().remove("gain_millidb");
                }
            }
        }
        saved_values(&old, &current, "");
        if version <= 14 {
            let first = old["next_id"].as_u64().unwrap();
            let tracks = current["tracks"].as_array().unwrap();
            assert_eq!(current["next_id"], first + tracks.len() as u64);
            for (i, track) in tracks.iter().enumerate() {
                let instrument = &track["instrument"];
                assert_eq!(instrument["id"], first + i as u64);
                assert_eq!(instrument["type_id"], "hex.instrument.sine");
                assert_eq!(instrument["state_version"], 1);
                assert_eq!(instrument["payload"], json!([]));
                assert_eq!(track["instrument_position"], 0);
            }
        } else {
            assert_eq!(old["next_id"], current["next_id"]);
        }
        check_history_and_roundtrip(project);
    }
}
#[test]
fn schemas_one_to_twelve_match_the_frozen_original_reader_semantics() {
    for (original, oracle) in READERS {
        let project = decode(original).unwrap();
        assert_eq!(
            project,
            decode(oracle).unwrap(),
            "schema{}",
            value(original)["version"]
        );
        check_history_and_roundtrip(project);
    }
}
#[test]
fn historical_files_refuse_unknown_duplicate_missing_and_partial_inventories() {
    for fixture in WRITERS
        .iter()
        .copied()
        .chain(READERS.iter().map(|pair| pair.0))
    {
        let original = value(fixture);
        let version = original["version"].clone();
        for field in ["assets", "tracks"] {
            let mut bad = original.clone();
            bad.as_object_mut().unwrap().remove(field);
            assert!(
                decode(&bytes(&bad)).is_err(),
                "schema{version} missing {field}"
            );
            bad[field] = Value::Null;
            assert!(
                decode(&bytes(&bad)).is_err(),
                "schema{version} null {field}"
            );
        }
        let mut bad = original.clone();
        bad["unknown"] = json!([]);
        assert!(decode(&bytes(&bad)).is_err());
        let mut bad = original.clone();
        bad["tracks"][0]["future"] = json!(1);
        assert!(decode(&bytes(&bad)).is_err());
        let text = std::str::from_utf8(fixture).unwrap();
        let duplicate = text.replacen("\"version\":", "\"version\":1,\"version\":", 1);
        assert_ne!(duplicate, text);
        assert!(decode(duplicate.as_bytes()).is_err());
        assert!(decode(&fixture[..fixture.len() / 2]).is_err());
        if original.get("audio_clip_state").is_some() {
            for field in [
                "audio_clip_state",
                "audio_event_state",
                "fade_shapes",
                "clip_names",
                "track_colors",
                "accented_clips",
            ] {
                if original.get(field).is_none() {
                    continue;
                }
                let mut bad = original.clone();
                bad.as_object_mut().unwrap().remove(field);
                assert!(
                    decode(&bytes(&bad)).is_err(),
                    "schema{version} missing {field}"
                );
                bad[field] = Value::Null;
                assert!(decode(&bytes(&bad)).is_err());
            }
        }
    }
    let mut mixed = value(include_bytes!(
        "fixtures/early/schema26-names-9f381a6f.json"
    ));
    mixed["audio_event_state"] = json!([]);
    assert!(decode(&bytes(&mixed)).is_err());
    let mut mixed = value(include_bytes!(
        "fixtures/early/schema27-named-events-d8922776.json"
    ));
    mixed["fade_shapes"] = json!([]);
    assert!(decode(&bytes(&mixed)).is_err());
    let mut mixed = value(include_bytes!(
        "fixtures/early/schema28-appearance-fades-e50ffea3.json"
    ));
    mixed["audio_event_state"] = json!([]);
    assert!(decode(&bytes(&mixed)).is_err());
    let mut mixed = value(include_bytes!("fixtures/lineages/foundation19.json"));
    mixed.as_object_mut().unwrap().remove("routing");
    assert!(decode(&bytes(&mixed)).is_err());
    let mut partial = value(include_bytes!(
        "fixtures/early/schema22-touch-60c88034.json"
    ));
    partial["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("gain_touch_epoch");
    assert!(decode(&bytes(&partial)).is_err());
}
#[test]
fn historical_load_and_save_as_keep_original_bytes_and_tokens() {
    let dir = std::env::temp_dir().join(format!(
        "hex-early-migrations-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    for (i, fixture) in WRITERS
        .iter()
        .copied()
        .chain(READERS.iter().map(|pair| pair.0))
        .enumerate()
    {
        let path = dir.join(format!("original{i}.hex"));
        let copy = dir.join(format!("upgraded{i}.hex"));
        std::fs::write(&path, fixture).unwrap();
        let (project, token) = hex_storage::load_with_token(&path).unwrap();
        assert!(token.requires_format_upgrade());
        token.check_unchanged(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), fixture);
        hex_storage::save_new(&copy, &project).unwrap();
        assert_eq!(hex_storage::load(&copy).unwrap(), project);
        assert!(hex_storage::save_new(&path, &project).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), fixture);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_bearing_project_refuses_schema22_and_current_format_preserves_gain_history() {
    use hex_project::{BeatTime, GainTouchContext, GainTouchSpan, GainTouchStep, Project};
    let base = decode(include_bytes!(
        "fixtures/early/schema22-touch-60c88034.json"
    ))
    .unwrap();
    let b = |n| BeatTime::from_quarters(n).unwrap();
    let mut snapshot = base.snapshot();
    snapshot.tracks[0].gain_envelope[0].incoming_gain = Some(0.75);
    snapshot.tracks[0].gain_touch_epoch = 1;
    snapshot.tracks[0].gain_touch_spans = vec![GainTouchSpan {
        source_start: b(0),
        start: b(1),
        end: b(3),
        context: GainTouchContext {
            recording_epoch: 1,
            project_generation: 0,
            canonical_revision: 1,
            audio_revision: 1,
            transport_epoch: 1,
            gesture_epoch: 1,
            gesture_serial: 1,
            sample_rate: 48000,
            loop_pass: 0,
        },
        steps: vec![
            GainTouchStep {
                beat: b(1),
                gain: 0.8,
                origin_beat: b(0),
                source_sequence: 1,
            },
            GainTouchStep {
                beat: b(2),
                gain: 0.4,
                origin_beat: b(1),
                source_sequence: 2,
            },
        ],
    }];
    let project = Project::from_snapshot(snapshot).unwrap();
    assert!(hex_storage::encode_schema22(&project).is_err());
    let encoded = encode(&project).unwrap();
    assert_eq!(decode(&encoded).unwrap(), project);
    let original = value(&encoded);
    for field in ["incoming_gain", "source_start_raw"] {
        let mut partial = original.clone();
        let row = if field == "incoming_gain" {
            &mut partial["tracks"][0]["gain_envelope"][1]
        } else {
            &mut partial["tracks"][0]["gain_touch_spans"][0]
        };
        row.as_object_mut().unwrap().remove(field);
        assert!(
            decode(&bytes(&partial)).is_err(),
            "mixed modern gain metadata: {field}"
        );
    }
    check_history_and_roundtrip(project);
}

#[test]
fn early_nested_metadata_requires_the_exact_evidenced_wire_shape() {
    let fixture = include_bytes!("fixtures/early/schema21-inline-edit-a9da374e.json");
    let original = value(fixture);
    for field in ["domain", "view_start", "view_end", "musical_start_raw"] {
        let mut bad = original.clone();
        bad["tracks"][0]["audio_clips"][0]["audio_edit"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(decode(&bytes(&bad)).is_err(), "missing edit {field}");
    }
    for (field, entry) in [("sample_micro_bpm", json!(null)), ("future", json!(1))] {
        let mut bad = original.clone();
        bad["tracks"][0]["audio_clips"][0]["audio_edit"]["domain"][field] = entry;
        assert!(decode(&bytes(&bad)).is_err(), "unexpected domain {field}");
    }
    let mut bad = original.clone();
    bad["tracks"][0]["audio_clips"][0]["audio_edit"]["domain"]
        .as_object_mut()
        .unwrap()
        .remove("fade_projection");
    assert!(decode(&bytes(&bad)).is_err());
    let text = std::str::from_utf8(fixture).unwrap();
    let duplicate = text.replacen(
        "\"source_length\":",
        "\"source_length\":0,\"source_length\":",
        1,
    );
    assert!(decode(duplicate.as_bytes()).is_err());
    let mut mixed = value(include_bytes!(
        "fixtures/early/schema29-sample-bpm-a0cedd1d.json"
    ));
    let mut old_edit = mixed["audio_clip_state"][0]["audio_edit"].clone();
    old_edit["domain"]
        .as_object_mut()
        .unwrap()
        .remove("sample_micro_bpm");
    mixed["audio_clip_state"][1]["audio_edit"] = old_edit;
    assert!(
        decode(&bytes(&mixed))
            .unwrap_err()
            .to_string()
            .contains("mixed historical sample tempo")
    );
    let mut bad = original.clone();
    bad["tracks"][0]["devices"][0]
        .as_object_mut()
        .unwrap()
        .remove("input_gain_millidb");
    assert!(decode(&bytes(&bad)).is_err(), "partial or mixed host trims");
    let mut bad = original.clone();
    bad["tracks"] = json!(vec![Value::Null; hex_project::MAX_TRACKS + 1]);
    assert!(decode(&bytes(&bad)).is_err());
    let mut bad = original.clone();
    bad["assets"] = json!(vec![Value::Null; hex_project::MAX_ASSETS + 1]);
    assert!(decode(&bytes(&bad)).is_err());
    let mut bad = value(include_bytes!(
        "fixtures/early/schema22-touch-60c88034.json"
    ));
    bad["tracks"][0]["gain_envelope"] = json!(vec![
        json!({"beat_raw":0,"gain":1.0,"outgoing":"step"});
        hex_project::MAX_GAIN_POINTS_PER_TRACK + 1
    ]);
    assert!(decode(&bytes(&bad)).is_err());
}

#[test]
fn original_empty_inline_audio_writers_have_unambiguous_complete_envelopes() {
    for fixture in [
        include_bytes!("fixtures/early/schema20-empty-b9274930.json").as_slice(),
        include_bytes!("fixtures/early/schema21-empty-a9da374e.json").as_slice(),
    ] {
        let project = decode(fixture).unwrap();
        assert!(project.tracks().is_empty());
        check_history_and_roundtrip(project);
        let mut bad = value(fixture);
        bad["tracks"] = Value::Null;
        assert!(decode(&bytes(&bad)).is_err());
    }
}
