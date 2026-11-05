//! Files emitted by original public schema20/22/30–33 encoders. Full source
//! identities, unmodified-source hashes and harnesses accompany the local raw
//! artifact provenance. Earlier reader-backed fixtures are tested separately.
use hex_project::{Command, Editor};
use hex_storage::{CURRENT_SCHEMA_VERSION, decode, encode, load_with_token, save_new};
use serde_json::{Value, json};
mod common;

const FIXTURES: [&[u8]; 13] = [
    include_bytes!("fixtures/earlier/schema30-tuning-ac29a1ad.json"),
    include_bytes!("fixtures/earlier/schema30-device-assets-d34e5a13.json"),
    include_bytes!("fixtures/earlier/schema31-combined-20cd1de3.json"),
    include_bytes!("fixtures/earlier/schema32-comparison-5165e305.json"),
    include_bytes!("fixtures/earlier/schema33-main-7b647514.json"),
    include_bytes!("fixtures/earlier/schema30-assets-bpm-a0cedd1d.json"),
    include_bytes!("fixtures/earlier/schema31-combined-bpm-1bab7b84.json"),
    include_bytes!("fixtures/earlier/schema31-midi-source-5d6c5f73.json"),
    include_bytes!("fixtures/earlier/schema31-placement-midi-4043b1fa.json"),
    include_bytes!("fixtures/earlier/schema32-placement-a9278234.json"),
    include_bytes!("fixtures/earlier/schema32-integrated-e09aa1ac.json"),
    include_bytes!("fixtures/earlier/schema32-wide-origin-13bd9fa3.json"),
    include_bytes!("fixtures/earlier/schema31-midi-loop-5d6c5f73.json"),
];
const EXPLICIT_MIDI: [&[u8]; 4] = [
    include_bytes!("fixtures/earlier/schema20-midi-source-5d6c5f73.json"),
    include_bytes!("fixtures/earlier/schema20-midi-loop-5d6c5f73.json"),
    include_bytes!("fixtures/earlier/schema22-midi-source-5d6c5f73.json"),
    include_bytes!("fixtures/earlier/schema22-midi-loop-5d6c5f73.json"),
];
fn value(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

// Compare every field actually authored by the old encoder. Extra current
// metadata is verified below; additions in later current schemas remain owned
// by their independent prior-version migration tests.
fn saved_values(old: &Value, current: &Value, path: &str) {
    match old {
        Value::Object(fields) => {
            for (key, before) in fields {
                if path.is_empty() && key == "version" {
                    continue;
                }
                // Original31 embeds this metadata in each MIDI source. Its
                // current typed inventory is verified separately below.
                if key == "one_shot_boundary" {
                    assert!(matches!(before.as_str(), Some("source_end" | "loop_end")));
                    continue;
                }
                if key == "musical_start_raw" {
                    let expected = if before.is_null() {
                        json!({"kind":"frames"})
                    } else {
                        json!({"kind":"musical","start_raw":before})
                    };
                    assert_eq!(current["timing"], expected, "timing at {path}");
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
        _ => assert_eq!(old, current, "saved value {path}"),
    }
}

fn saved_boundaries(old: &Value, current: &Value) {
    let mut boundaries = std::collections::BTreeMap::new();
    for clip in old["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|track| track["midi_clips"].as_array().unwrap())
    {
        boundaries.insert(
            clip["id"].as_u64().unwrap(),
            clip.get("one_shot_boundary")
                .cloned()
                .unwrap_or(json!("source_end")),
        );
    }
    for cell in old["session"]["cells"].as_array().unwrap() {
        if cell["content"]["kind"] == "midi" {
            let clip = &cell["content"]["clip"];
            boundaries.insert(
                clip["id"].as_u64().unwrap(),
                clip["source"]
                    .get("one_shot_boundary")
                    .cloned()
                    .unwrap_or(json!("source_end")),
            );
        }
    }
    let current_boundaries: std::collections::BTreeMap<_, _> = current["midi_one_shot_boundaries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["clip"].as_u64().unwrap(), row["boundary"].clone()))
        .collect();
    assert_eq!(current_boundaries, boundaries);
}

#[test]
fn original_30_to_33_writers_keep_all_saved_values_roundtrip_and_history() {
    for fixture in FIXTURES {
        let old = value(fixture);
        let project = decode(fixture).unwrap();
        let encoded = encode(&project).unwrap();
        let mut current = value(&encoded);
        common::restore_pre_binding_instruments(&mut current);
        assert_eq!(current["version"], CURRENT_SCHEMA_VERSION);
        saved_values(&old, &current, "");
        assert_eq!(decode(&encoded).unwrap(), project);
        saved_boundaries(&old, &current);
        assert_eq!(current["arrangement_sections"], json!([]));
        assert!(
            current["asset_sources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["original"].is_null())
        );
        let volumes = current["midi_clip_volumes"]["clips"].as_array().unwrap();
        let mut expected: Vec<_> = old["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|track| {
                track["midi_clips"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|clip| clip["id"].as_u64().unwrap())
            })
            .collect();
        expected.extend(
            old["session"]["cells"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|cell| cell["content"]["kind"] == "midi")
                .map(|cell| cell["content"]["clip"]["id"].as_u64().unwrap()),
        );
        let mut actual: Vec<_> = volumes
            .iter()
            .map(|row| {
                assert_eq!(row["points"], json!([]));
                row["clip"].as_u64().unwrap()
            })
            .collect();
        expected.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, expected);
        for track in current["tracks"].as_array().unwrap() {
            assert!(
                track["gain_envelope"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|point| point["incoming_gain"].is_null())
            );
            assert!(
                track["gain_touch_spans"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|span| span["source_start_raw"] == span["start_raw"])
            );
        }
        for edit in current["audio_clip_state"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| &row["audio_edit"])
            .filter(|edit| !edit.is_null())
        {
            assert!(edit["processing_projection"].is_null());
            assert_eq!(edit["placement_start_offset"], 0);
            assert_eq!(edit["placement_end_offset"], 0);
        }
        if old.get("tuning").is_none() {
            assert_eq!(project.tuning(), hex_tuning::ProjectTuning::default());
        }
        if old.get("main_devices").is_none() {
            assert!(project.main_devices().is_empty());
        }
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        editor.execute(Command::MasterGain { gain: 0.375 }).unwrap();
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
fn historical_30_to_33_refuse_missing_unknown_duplicate_mixed_or_partial_data() {
    for fixture in FIXTURES {
        let original = value(fixture);
        for field in [
            "tracks",
            "assets",
            "audio_clip_state",
            "audio_event_state",
            "fade_shapes",
            "clip_names",
        ] {
            let mut bad = original.clone();
            assert!(bad.as_object_mut().unwrap().remove(field).is_some());
            assert!(decode(&bytes(&bad)).is_err(), "missing {field}");
        }
        for field in ["tuning", "device_assets", "main_devices"] {
            if original.get(field).is_some() {
                let mut bad = original.clone();
                bad.as_object_mut().unwrap().remove(field);
                assert!(decode(&bytes(&bad)).is_err(), "missing {field}");
            }
        }
        for field in [
            "asset_sources",
            "midi_clip_volumes",
            "arrangement_sections",
            "unknown",
        ] {
            let mut bad = original.clone();
            bad[field] = json!([]);
            assert!(decode(&bytes(&bad)).is_err());
        }
        let mut bad = original.clone();
        bad["audio_clip_state"][0]["audio_edit"]["future"] = json!(1);
        assert!(decode(&bytes(&bad)).is_err());
        let mut bad = original.clone();
        let edit = bad["audio_clip_state"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| !row["audio_edit"].is_null())
            .unwrap();
        edit["audio_edit"].as_object_mut().unwrap().remove("domain");
        assert!(decode(&bytes(&bad)).is_err());
        let text = std::str::from_utf8(fixture).unwrap();
        let duplicate = text.replacen("\"version\":", "\"version\":30,\"version\":", 1);
        assert_ne!(duplicate, text);
        assert!(decode(duplicate.as_bytes()).is_err());
        assert!(decode(&fixture[..fixture.len() / 2]).is_err());
    }
    let mut mixed = value(FIXTURES[0]);
    mixed["device_assets"] = json!([]);
    assert!(decode(&bytes(&mixed)).is_err());
    let mut mixed = value(FIXTURES[3]);
    mixed["device_assets"][0]
        .as_object_mut()
        .unwrap()
        .remove("comparison");
    assert!(decode(&bytes(&mixed)).is_err(), "partial inventory layout");
    for field in ["placement", "sample_micro_bpm", "one_shot_boundary"] {
        let mut mixed = value(FIXTURES[8]);
        if field == "one_shot_boundary" {
            mixed["tracks"][0]["midi_clips"][0]
                .as_object_mut()
                .unwrap()
                .remove(field);
        } else {
            let edit = &mut mixed["audio_clip_state"][0]["audio_edit"];
            let object = if field == "sample_micro_bpm" {
                edit["domain"].as_object_mut().unwrap()
            } else {
                edit.as_object_mut().unwrap()
            };
            assert!(object.remove(field).is_some());
        }
        assert!(decode(&bytes(&mixed)).is_err(), "mixed {field} layout");
    }
}

#[test]
fn historical_open_is_no_write_and_save_as_preserves_source_bytes() {
    let dir = std::env::temp_dir().join(format!(
        "hex-earlier-migrations-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    for (i, fixture) in FIXTURES.into_iter().chain(EXPLICIT_MIDI).enumerate() {
        let path = dir.join(format!("original-{i}.hex"));
        let copy = dir.join(format!("upgraded-{i}.hex"));
        std::fs::write(&path, fixture).unwrap();
        let (project, token) = load_with_token(&path).unwrap();
        assert!(token.requires_format_upgrade());
        token.check_unchanged(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), fixture);
        save_new(&copy, &project).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), fixture);
        assert_eq!(hex_storage::load(&copy).unwrap(), project);
        assert!(save_new(&path, &project).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), fixture);
    }
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        (FIXTURES.len() + EXPLICIT_MIDI.len()) * 2
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn original31_loop_end_keeps_late_notes_and_validates_each_selected_boundary() {
    use hex_project::{BeatTime, MidiOneShotBoundary, SessionCellContent};
    let fixture = FIXTURES[12];
    let project = decode(fixture).unwrap();
    let sources: Vec<_> = project
        .tracks()
        .iter()
        .flat_map(|track| track.midi_clips())
        .map(|clip| clip.source())
        .chain(project.session_cells().iter().filter_map(|cell| {
            if let SessionCellContent::Midi(clip) = cell.content() {
                Some(clip.source())
            } else {
                None
            }
        }))
        .filter(|source| source.one_shot_boundary() == MidiOneShotBoundary::LoopEnd)
        .collect();
    assert_eq!(sources.len(), 2, "Arrangement and Session source owners");
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    for source in sources {
        assert!(source.notes().iter().any(|note| note.start() == beat(10)));
        let playback = source.playback_for_duration(beat(20)).unwrap();
        assert_eq!(
            playback
                .note_occurrences(beat(10), beat(2))
                .unwrap()
                .count(),
            0
        );
        assert!(playback.source_phase_at_elapsed(beat(3)).is_some());
        assert_eq!(playback.source_phase_at_elapsed(beat(4)), None);
        assert_eq!(source.source_length(), beat(12));
    }
    // Both edits remain valid for ordinary SourceEnd. Only reinstating the
    // authored LoopEnd policy before final typed validation makes them refuse.
    for session in [false, true] {
        let mut bad = value(fixture);
        let source = if session {
            &mut bad["session"]["cells"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|cell| cell["content"]["clip"]["source"]["one_shot_boundary"] == "loop_end")
                .unwrap()["content"]["clip"]["source"]
        } else {
            &mut bad["tracks"][0]["midi_clips"][0]
        };
        source["source_start_raw"] = source["loop_end_raw"].clone();
        assert!(decode(&bytes(&bad)).is_err());
    }
}

#[test]
fn original_explicit20_and22_midi_writers_preserve_policy_and_reject_mixed_layouts() {
    for fixture in EXPLICIT_MIDI {
        let original = value(fixture);
        let project = decode(fixture).unwrap();
        let encoded = encode(&project).unwrap();
        let mut current = value(&encoded);
        common::restore_pre_binding_instruments(&mut current);
        saved_values(&original, &current, "");
        saved_boundaries(&original, &current);
        assert_eq!(decode(&encoded).unwrap(), project);
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        editor.execute(Command::MasterGain { gain: 0.375 }).unwrap();
        assert!(editor.undo());
        assert_eq!(*editor.project(), project);
        assert!(editor.redo());
        assert_eq!(
            decode(&encode(editor.project()).unwrap()).unwrap(),
            *editor.project()
        );
        let mut bad = original.clone();
        let instrument = bad["tracks"][0]["instrument"].as_object_mut().unwrap();
        instrument.remove("input_gain_millidb");
        instrument.remove("output_gain_millidb");
        assert!(
            decode(&bytes(&bad)).is_err(),
            "policy with older no-trim DTO"
        );
        let mut bad = original.clone();
        bad["tracks"][0]["midi_clips"][0]
            .as_object_mut()
            .unwrap()
            .remove("one_shot_boundary");
        assert!(decode(&bytes(&bad)).is_err(), "partial policy inventory");
        let mut bad = original.clone();
        bad["tracks"][0]["midi_clips"][0]["one_shot_boundary"] = json!("unknown");
        assert!(decode(&bytes(&bad)).is_err());
        if original["version"] == 22 {
            let mut bad = original.clone();
            bad["tracks"][0]["gain_envelope"] = json!([{
                "beat_raw":0,"gain":0.5,"outgoing":"linear","incoming_gain":null
            }]);
            assert!(
                decode(&bytes(&bad)).is_err(),
                "policy with later gain seams"
            );
        }
    }
}

#[test]
fn inline20_cannot_acquire_a_foundation_midi_policy_layout() {
    let fixture = include_bytes!("fixtures/early/schema20-inline-gain-b9274930.json");
    assert!(decode(fixture).is_ok());
    let mut bad = value(fixture);
    for track in bad["tracks"].as_array_mut().unwrap() {
        for clip in track["midi_clips"].as_array_mut().unwrap() {
            clip["one_shot_boundary"] = json!("source_end");
        }
    }
    for cell in bad["session"]["cells"].as_array_mut().unwrap() {
        if cell["content"]["kind"] == "midi" {
            cell["content"]["clip"]["source"]["one_shot_boundary"] = json!("source_end");
        }
    }
    assert!(decode(&bytes(&bad)).is_err());
}
