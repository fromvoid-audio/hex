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
    e.execute(Command::InsertInstrument {
        track: source,
        state: hex_project::default_instrument_state().unwrap(),
    })
    .unwrap();
    let Created::Track(target) = e
        .execute(Command::InsertTrackWithDevice {
            name: "target".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
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
                incoming_gain: None,
                beat: BeatTime::ZERO,
                gain: 0.25,
                outgoing: GainInterpolation::Linear,
            },
            GainPoint {
                incoming_gain: None,
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
    let project = Project::from_legacy_snapshot(snapshot).unwrap();
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
        old.as_object_mut().unwrap().remove("main_devices");
        old.as_object_mut().unwrap().remove("tuning");
        old.as_object_mut().unwrap().remove("device_assets");
        old.as_object_mut().unwrap().remove("asset_sources");
        old.as_object_mut().unwrap().remove("track_colors");
        old.as_object_mut().unwrap().remove("accented_clips");
        assert!(decode(&serde_json::to_vec(&old).unwrap()).is_err());
    }
}
