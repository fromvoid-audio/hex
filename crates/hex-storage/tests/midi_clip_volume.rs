use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint,
    Project,
};
use hex_storage::{decode, encode};
use serde_json::{Value, json};

fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn curve(incoming: f64, value: f64) -> MidiClipVolume {
    MidiClipVolume::new(
        vec![
            MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.25).unwrap()),
            MidiClipVolumePoint {
                beat: beat(3),
                incoming: MidiClipGain::new(incoming).unwrap(),
                value: MidiClipGain::new(value).unwrap(),
            },
            MidiClipVolumePoint::linear(beat(8), MidiClipGain::new(4.0).unwrap()),
        ],
        beat(8),
    )
    .unwrap()
}
fn fixture() -> (Editor, ClipId, ClipId, ClipId, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported {
        track, clip: audio, ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "hybrid".into(),
            path: "Media/Original.WAV".into(),
            sample_rate: 48_000,
            frames: 128,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    let mut clips = Vec::new();
    for start in [4, 20] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(start),
                length: beat(8),
            })
            .unwrap()
        else {
            panic!("midi fixture")
        };
        clips.push(clip);
    }
    let arrangement = clips[0];
    editor
        .execute(Command::AddMidiNote {
            clip: arrangement,
            start: beat(1),
            duration: beat(2),
            pitch: 60,
            channel: 2,
            velocity: 93,
        })
        .unwrap();
    editor
        .execute(Command::SetMidiClipSource {
            clip: arrangement,
            source_length: beat(8),
            source_start: beat(1),
            loop_start: beat(2),
            loop_end: beat(7),
            looping: true,
            length: beat(8),
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene fixture")
    };
    let Created::SessionClip(console) = editor
        .execute(Command::CopyArrangementMidiToSession {
            source: arrangement,
            scene,
        })
        .unwrap()
    else {
        panic!("console fixture")
    };
    (editor, arrangement, console, clips[1], audio)
}
fn document() -> (Value, ClipId, ClipId) {
    let (mut editor, arrangement, console, _, audio) = fixture();
    for (clip, volume) in [(arrangement, curve(0.0, 2.0)), (console, curve(2.0, 0.0))] {
        editor
            .execute(Command::ReplaceMidiClipVolume { clip, volume })
            .unwrap();
    }
    (
        serde_json::from_slice(&encode(editor.project()).unwrap()).unwrap(),
        console,
        audio,
    )
}
fn refuses(document: &Value) {
    assert!(decode(&serde_json::to_vec(document).unwrap()).is_err());
}

#[test]
fn current_volume_roundtrips_both_owners_unity_discontinuities_history_and_file_publication() {
    let (mut editor, arrangement, console, unity, _) = fixture();
    let baseline = editor.project().clone();
    let arrangement_source = baseline.midi_clip(arrangement).unwrap().source_identity();
    for clip in [arrangement, console] {
        let mut single_owner = Editor::new(baseline.clone(), 1).unwrap();
        single_owner
            .execute(Command::ReplaceMidiClipVolume {
                clip,
                volume: curve(0.0, 2.0),
            })
            .unwrap();
        for encoded in [
            hex_storage::encode_schema20(single_owner.project()),
            hex_storage::encode_schema22(single_owner.project()),
        ] {
            assert!(matches!(
                encoded,
                Err(hex_storage::StorageError::Invalid(
                    "older schema cannot save midi clip volume"
                ))
            ));
        }
    }
    for (clip, volume) in [(arrangement, curve(0.0, 2.0)), (console, curve(2.0, 0.0))] {
        let before = editor.project().clone();
        editor
            .execute(Command::ReplaceMidiClipVolume { clip, volume })
            .unwrap();
        let changed = editor.project().clone();
        assert!(editor.undo());
        assert_eq!(decode(&encode(editor.project()).unwrap()).unwrap(), before);
        assert!(editor.redo());
        assert_eq!(decode(&encode(editor.project()).unwrap()).unwrap(), changed);
    }
    assert_eq!(
        editor
            .project()
            .midi_clip(arrangement)
            .unwrap()
            .source_identity(),
        arrangement_source
    );
    assert!(
        editor
            .project()
            .midi_clip_volume(unity)
            .unwrap()
            .points()
            .is_empty()
    );
    let encoded = encode(editor.project()).unwrap();
    let value: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    let rows = value["midi_clip_volumes"]["clips"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "unity is an explicit complete inventory row");
    assert!(
        rows.iter()
            .any(|row| row["clip"] == unity.value() && row["points"] == json!([]))
    );
    let reopened = decode(&encoded).unwrap();
    assert_eq!(reopened, *editor.project());
    for clip in [arrangement, console] {
        let volume = reopened.midi_clip_volume(clip).unwrap();
        assert_ne!(volume.gain_before(beat(3)), volume.gain_at(beat(3)));
        assert_eq!(
            volume.points().last().unwrap().beat,
            beat(8),
            "source endpoint is legal"
        );
    }
    assert!(hex_storage::encode_schema20(editor.project()).is_err());
    assert!(hex_storage::encode_schema22(editor.project()).is_err());
    let dir = std::env::temp_dir().join(format!(
        "hex-midi-volume-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("saved.hex");
    hex_storage::save_new(&path, editor.project()).unwrap();
    assert_eq!(hex_storage::load(&path).unwrap(), reopened);
    assert_eq!(std::fs::read(&path).unwrap(), encoded);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        baseline.midi_clip(arrangement).unwrap().volume(),
        &MidiClipVolume::default()
    );
}

#[test]
fn current_volume_inventory_requires_every_midi_owner_exactly_once_and_refuses_other_ids() {
    let (base, console, audio) = document();
    decode(&serde_json::to_vec(&base).unwrap()).unwrap();
    for owner in [0, 1, 2] {
        let mut bad = base.clone();
        bad["midi_clip_volumes"]["clips"]
            .as_array_mut()
            .unwrap()
            .remove(owner);
        refuses(&bad);
    }
    for id in [0, audio.value(), console.value(), u64::MAX] {
        let mut bad = base.clone();
        bad["midi_clip_volumes"]["clips"][0]["clip"] = id.into();
        refuses(&bad);
    }
    let mut extra = base.clone();
    extra["midi_clip_volumes"]["clips"]
        .as_array_mut()
        .unwrap()
        .push(json!({"clip":audio.value(),"points":[]}));
    refuses(&extra);
    let mut missing = base;
    missing.as_object_mut().unwrap().remove("midi_clip_volumes");
    refuses(&missing);
}

#[test]
fn current_volume_points_require_finite_bounded_gains_sorted_source_beats_and_strict_fields() {
    let (base, _, _) = document();
    for index in [0, 2] {
        // Arrangement and Console custom curves.
        for (key, value) in [
            ("beat_raw", json!(beat(8).raw() + 1)),
            ("beat_raw", json!(-1)),
            ("beat_raw", json!(1.5)),
            ("incoming", json!(-0.1)),
            ("incoming", json!(4.1)),
            ("value", json!(-0.1)),
            ("value", json!(4.1)),
            ("value", Value::Null),
            ("incoming", json!("nan")),
            ("unknown", json!(1)),
        ] {
            let mut bad = base.clone();
            bad["midi_clip_volumes"]["clips"][index]["points"][1][key] = value;
            refuses(&bad);
        }
        for key in ["beat_raw", "incoming", "value"] {
            let mut bad = base.clone();
            bad["midi_clip_volumes"]["clips"][index]["points"][1]
                .as_object_mut()
                .unwrap()
                .remove(key);
            refuses(&bad);
        }
        for beat_raw in [0, beat(8).raw()] {
            let mut bad = base.clone();
            bad["midi_clip_volumes"]["clips"][index]["points"][1]["beat_raw"] = beat_raw.into();
            refuses(&bad);
        }
    }
    for path in [
        vec!["midi_clip_volumes"],
        vec!["midi_clip_volumes", "clips"],
    ] {
        let mut bad = base.clone();
        let mut at = &mut bad;
        for key in path {
            at = &mut at[key];
        }
        *at = Value::Null;
        refuses(&bad);
    }
    for key in ["clip", "points"] {
        let mut bad = base.clone();
        bad["midi_clip_volumes"]["clips"][0]
            .as_object_mut()
            .unwrap()
            .remove(key);
        refuses(&bad);
    }
    let mut bad = base.clone();
    bad["midi_clip_volumes"]["unknown"] = true.into();
    refuses(&bad);
    bad = base;
    bad["midi_clip_volumes"]["clips"][0]["unknown"] = true.into();
    refuses(&bad);
}

#[test]
fn current_volume_raw_duplicate_keys_and_numeric_overflow_never_become_valid_data() {
    let (base, _, _) = document();
    let raw = serde_json::to_string(&base).unwrap();
    for (needle, replacement) in [
        (
            "\"midi_clip_volumes\":",
            "\"midi_clip_volumes\":{},\"midi_clip_volumes\":",
        ),
        ("\"clips\":[", "\"clips\":[],\"clips\":["),
        ("\"points\":[", "\"points\":[],\"points\":["),
        (
            "\"beat_raw\":0,\"incoming\":",
            "\"beat_raw\":0,\"incoming\":1,\"incoming\":",
        ),
        ("\"incoming\":0.25", "\"incoming\":1e999"),
    ] {
        let bad = raw.replacen(needle, replacement, 1);
        assert_ne!(bad, raw, "fixture must reach intended raw token");
        assert!(decode(bad.as_bytes()).is_err());
    }
}

#[test]
fn current_volume_limits_refuse_at_the_inventory_boundary() {
    let (base, _, _) = document();
    let points: Vec<_> = (0..hex_project::MAX_MIDI_VOLUME_POINTS_PER_CLIP)
        .map(|raw| json!({"beat_raw":raw,"incoming":1.0,"value":1.0}))
        .collect();
    let mut at_limit = base.clone();
    at_limit["midi_clip_volumes"]["clips"][0]["points"] = json!(points);
    decode(&serde_json::to_vec(&at_limit).unwrap()).unwrap();
    at_limit["midi_clip_volumes"]["clips"][0]["points"]
        .as_array_mut()
        .unwrap()
        .push(json!({"beat_raw":1024,"incoming":1.0,"value":1.0}));
    let error = decode(&serde_json::to_vec(&at_limit).unwrap()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("clip volume point limit reached")
    );
    let mut global = base.clone();
    let rows: Vec<_> = (0..=hex_project::MAX_MIDI_VOLUME_POINTS / points.len())
        .map(|id| json!({"clip":id+1,"points":points}))
        .collect();
    global["midi_clip_volumes"]["clips"] = json!(rows);
    let error = decode(&serde_json::to_vec(&global).unwrap()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("project clip volume point limit reached")
    );
    let mut clips = base;
    clips["midi_clip_volumes"]["clips"] = json!(vec![
        json!({"clip":1,"points":[]});
        hex_project::MAX_CLIPS + 1
    ]);
    let error = decode(&serde_json::to_vec(&clips).unwrap()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("clip volume inventory limit reached")
    );
}

#[test]
fn consolidated_legacy_gains_and_curves_roundtrip_with_boundary_jumps() {
    let (mut editor, first, _, second, _) = fixture();
    for (clip, db, volume) in [
        (first, -6000, curve(0.0, 2.0)),
        (second, 6000, curve(2.0, 0.0)),
    ] {
        editor
            .execute(Command::ReplaceMidiClipVolume { clip, volume })
            .unwrap();
        editor
            .execute(Command::SetMidiClipGain {
                clip,
                gain: hex_project::ClipGain::new(db).unwrap(),
            })
            .unwrap();
    }
    assert_eq!(
        decode(&encode(editor.project()).unwrap()).unwrap(),
        *editor.project()
    );
    let Created::Clip(group) = editor
        .execute(Command::GroupClips(vec![first, second]))
        .unwrap()
    else {
        panic!("group")
    };
    let reopened = decode(&encode(editor.project()).unwrap()).unwrap();
    assert_eq!(reopened, *editor.project());
    let volume = reopened.midi_clip(group).unwrap().volume();
    assert_ne!(volume.gain_before(beat(16)), volume.gain_at(beat(16)));
}
