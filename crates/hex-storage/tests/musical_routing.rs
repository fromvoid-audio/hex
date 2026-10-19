//! Combined routed schema and isolated development-envelope preservation.
use hex_project::{
    BeatTime, Command, Created, Editor, GainInterpolation, GainPoint, ImportProvenance, Project,
    RoutingOwner, SourceGainPoint, WarpMode, default_audio_warp,
};
use hex_storage::{decode, encode, encode_schema20, encode_schema22};
use serde_json::{Value, json};
fn fixture(transformed: bool) -> Project {
    let mut e = Editor::new(Project::new(16_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "original.wav".into(),
            sample_rate: 16_000,
            frames: 8_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session) = e
        .execute(Command::CopyArrangementAudioClipToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("session")
    };
    e.execute(Command::SetAudioClipGain {
        clip: session,
        millidb: 3_000,
    })
    .unwrap();
    e.execute(Command::SetAudioClipGain {
        clip,
        millidb: -6_000,
    })
    .unwrap();
    if transformed {
        e.execute(Command::SetAudioClipPitch { clip, cents: 137 })
            .unwrap();
        let warp = default_audio_warp(
            e.project().clip(clip).unwrap(),
            e.project().tempo_map(),
            WarpMode::ComplexPro {
                formants: 75,
                envelope: 128,
            },
        )
        .unwrap();
        e.execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        })
        .unwrap();
        e.execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 0,
                },
                SourceGainPoint {
                    frame: 8_000,
                    gain_q16: 65_536,
                },
            ],
        })
        .unwrap();
    }
    e.project().clone()
}
#[test]
fn schema25_combines_roots_session_gain_routes_lanes_provenance_and_host_trims() {
    let mut e = Editor::new(fixture(true), 32).unwrap();
    let source = e.project().tracks()[0].id();
    let Created::Track(target) = e
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    e.execute(Command::SetAudioFeed {
        source: RoutingOwner::Track(source),
        destination: target,
        gain: 0.5,
    })
    .unwrap();
    e.execute(Command::SetMidiSource {
        destination: target,
        source: Some(source),
    })
    .unwrap();
    let mut returns = Vec::new();
    for _ in 0..2 {
        let Created::ReturnTrack(bus) = e
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        returns.push(bus);
    }
    e.execute(Command::SetReturnSend {
        track: source,
        return_track: returns[0],
        gain: 0.25,
    })
    .unwrap();
    e.execute(Command::SetReturnFeed {
        source: returns[0],
        destination: returns[1],
        gain: 0.5,
    })
    .unwrap();
    e.execute(Command::ReplaceTrackGainEnvelope {
        track: source,
        points: vec![
            GainPoint {
                beat: BeatTime::ZERO,
                gain: 0.25,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                beat: BeatTime::ONE,
                gain: 0.75,
                outgoing: GainInterpolation::Step,
            },
        ],
    })
    .unwrap();
    let mut snapshot = e.project().snapshot();
    snapshot.import_provenance =
        Some(ImportProvenance::new("ableton-live-set", 1, 1, vec![0, 42, 255]).unwrap());
    let device = snapshot.tracks[0].instrument.as_mut().unwrap();
    device.state = device
        .state
        .clone()
        .with_input_gain_millidb(3_000)
        .unwrap()
        .with_output_gain_millidb(-2_000)
        .unwrap();
    let project = Project::from_snapshot(snapshot).unwrap();
    let bytes = encode(&project).unwrap();
    assert_eq!(decode(&bytes).unwrap(), project);
    assert!(encode_schema20(&project).is_err());
    assert!(encode_schema22(&project).is_err());
}
#[test]
fn state_table_requires_exact_audio_id_bijection_and_strict_nullable_fields() {
    let base: Value = serde_json::from_slice(&encode(&fixture(true)).unwrap()).unwrap();
    for edit in 0..8 {
        let mut bad = base.clone();
        match edit {
            0 => {
                bad.as_object_mut().unwrap().remove("audio_clip_state");
                bad.as_object_mut().unwrap().remove("audio_event_state");
                bad.as_object_mut().unwrap().remove("track_colors");
                bad.as_object_mut().unwrap().remove("accented_clips");

                bad.as_object_mut().unwrap().remove("clip_names");
            }
            1 => {
                bad["audio_clip_state"].as_array_mut().unwrap().pop();
            }
            2 => {
                let duplicate = bad["audio_clip_state"][0].clone();
                bad["audio_clip_state"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            3 => bad["audio_clip_state"][0]["clip"] = json!(999_999),
            4 => {
                bad["audio_clip_state"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("audio_edit");
            }
            5 => {
                bad["audio_clip_state"][1]["audio_edit"] =
                    bad["audio_clip_state"][0]["audio_edit"].clone()
            }
            6 => bad["audio_clip_state"][0]["gain_millidb"] = Value::Null,
            _ => bad["tracks"][0]["audio_clips"][0]["audio_edit"] = Value::Null,
        }
        assert!(
            decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "case {edit}"
        );
    }
    let text = serde_json::to_string(&base).unwrap();
    assert!(
        decode(
            text.replacen(
                "\"gain_millidb\":",
                "\"gain_millidb\":0,\"gain_millidb\":",
                1
            )
            .as_bytes()
        )
        .is_err()
    );
    for version in 1..=24 {
        let mut old = base.clone();
        old["version"] = json!(version);
        old.as_object_mut().unwrap().remove("tuning");
        old.as_object_mut().unwrap().remove("device_assets");
        old.as_object_mut().unwrap().remove("track_colors");
        old.as_object_mut().unwrap().remove("accented_clips");
        assert!(decode(&serde_json::to_vec(&old).unwrap()).is_err());
    }
}
fn development(version: u32) -> Value {
    let mut doc: Value = serde_json::from_slice(&encode(&fixture(version == 21)).unwrap()).unwrap();
    doc.as_object_mut().unwrap().remove("tuning");
    doc.as_object_mut().unwrap().remove("device_assets");
    doc.as_object_mut().unwrap().remove("clip_names");
    doc.as_object_mut().unwrap().remove("fade_shapes");
    let states = doc
        .as_object_mut()
        .unwrap()
        .remove("audio_clip_state")
        .unwrap();
    for state in states.as_array().unwrap() {
        let id = state["clip"].as_u64().unwrap();
        let mut found = false;
        for track in doc["tracks"].as_array_mut().unwrap() {
            for clip in track["audio_clips"].as_array_mut().unwrap() {
                if clip["id"] == id {
                    clip["gain_millidb"] = state["gain_millidb"].clone();
                    if version == 21 {
                        clip["audio_edit"] = state["audio_edit"].clone();
                    }
                    found = true;
                }
            }
        }
        if !found {
            for cell in doc["session"]["cells"].as_array_mut().unwrap() {
                if cell["content"]["kind"] == "audio" && cell["content"]["clip"]["id"] == id {
                    cell["content"]["clip"]["gain_millidb"] = state["gain_millidb"].clone();
                }
            }
        }
    }
    doc.as_object_mut().unwrap().remove("audio_event_state");
    doc.as_object_mut().unwrap().remove("track_colors");
    doc.as_object_mut().unwrap().remove("accented_clips");
    for key in ["import_provenance", "routing", "audio_feeds", "midi_routes"] {
        doc.as_object_mut().unwrap().remove(key);
    }
    for track in doc["tracks"].as_array_mut().unwrap() {
        for key in ["gain_envelope", "gain_touch_epoch", "gain_touch_spans"] {
            track.as_object_mut().unwrap().remove(key);
        }
    }
    doc["version"] = json!(version);
    doc.as_object_mut().unwrap().remove("track_colors");
    doc.as_object_mut().unwrap().remove("accented_clips");
    doc
}
#[test]
fn private20_21_dispatch_is_explicit_strict_and_migrates_both_audio_kinds() {
    for version in [20, 21] {
        // These private envelopes omit track colors; their authored expected
        // project is neutral while the current-format fixtures stay colored.
        let mut expected = Editor::new(fixture(version == 21), 8).unwrap();
        let track = expected.project().tracks()[0].id();
        expected
            .execute(Command::SetTrackColor {
                track,
                color: hex_project::ElementColor::Gray,
            })
            .unwrap();
        let doc = development(version);
        let restored = decode(&serde_json::to_vec(&doc).unwrap()).unwrap();
        assert_eq!(restored, *expected.project());
        for edit in 0..5 {
            let mut bad = doc.clone();
            match edit {
                0 => bad["routing"] = Value::Null,
                1 => {
                    bad["routing"] = json!([]);
                    bad["import_provenance"] = Value::Null;
                }
                2 => {
                    bad["tracks"][0]["audio_clips"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("gain_millidb");
                }
                3 => {
                    if version == 21 {
                        bad["tracks"][0]["audio_clips"][0]
                            .as_object_mut()
                            .unwrap()
                            .remove("audio_edit");
                    } else {
                        bad["tracks"][0]["audio_clips"][0]["audio_edit"] = Value::Null;
                    }
                }
                _ => bad["tracks"][0]["gain_envelope"] = json!([]),
            }
            assert!(
                decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "{version} case {edit}"
            );
        }
        let mut empty = doc;
        empty["tracks"][0]["audio_clips"] = json!([]);
        empty["session"]["cells"] = json!([]);
        assert!(decode(&serde_json::to_vec(&empty).unwrap()).is_err());
    }
    // A real schema24 document keeps its strict historical nested shape and neutral gain.
    let neutral = Project::new(16_000).unwrap();
    let mut old: Value = serde_json::from_slice(&encode(&neutral).unwrap()).unwrap();
    old["version"] = json!(24);
    old.as_object_mut().unwrap().remove("tuning");
    old.as_object_mut().unwrap().remove("device_assets");
    old.as_object_mut().unwrap().remove("track_colors");
    old.as_object_mut().unwrap().remove("accented_clips");
    old.as_object_mut().unwrap().remove("audio_clip_state");
    old.as_object_mut().unwrap().remove("fade_shapes");
    old.as_object_mut().unwrap().remove("audio_event_state");

    old.as_object_mut().unwrap().remove("clip_names");
    assert_eq!(decode(&serde_json::to_vec(&old).unwrap()).unwrap(), neutral);
}
