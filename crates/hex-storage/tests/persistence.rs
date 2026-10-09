use hex_project::{BeatTime, Command, Created, Editor, Meter, Project, Tempo};
use hex_storage::{MAX_DOCUMENT_BYTES, decode, encode, load, save_new};
use std::fs;

fn project() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let (track, clip) = match editor
        .execute(Command::ImportAudioTrack {
            name: "Case Preserved".into(),
            path: "Media/Drum.WAV".into(),
            sample_rate: 48_000,
            frames: 200,
        })
        .unwrap()
    {
        Created::Imported { track, asset, clip } => {
            let loop_clip = match editor
                .execute(Command::PlaceClip {
                    track,
                    asset,
                    start: 300,
                    duration: 9,
                    source_offset: 20,
                    source_length: 3,
                    looping: true,
                })
                .unwrap()
            {
                Created::Clip(id) => id,
                _ => panic!("wrong loop clip result"),
            };
            editor
                .execute(Command::SplitClip {
                    clip: loop_clip,
                    frame: 302,
                })
                .unwrap();
            (track, clip)
        }
        _ => panic!("wrong import result"),
    };
    editor
        .execute(Command::Gain { track, gain: 0.375 })
        .unwrap();
    editor
        .execute(Command::Mute { track, muted: true })
        .unwrap();
    editor
        .execute(Command::TrimClip {
            clip,
            start: 123,
            duration: 77,
            source_offset: 11,
            source_length: 99,
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(8).unwrap(),
            tempo: Tempo::new(93_500_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetMeterPoint {
            beat: BeatTime::from_ratio(5, 2).unwrap(),
            meter: Meter::new(7, 8).unwrap(),
        })
        .unwrap();
    // Deleted IDs must not be reused when reopening.
    editor
        .execute(Command::AddTrack {
            name: "discarded".into(),
        })
        .unwrap();
    editor.undo();
    editor.project().clone()
}

fn assert_empty_session(project: &Project) {
    assert!(project.scenes().is_empty());
    assert!(project.session_cells().is_empty());
}
#[test]
fn roundtrip_complete_current_state_and_highwater() {
    let project = project();
    assert_eq!(decode(&encode(&project).unwrap()).unwrap(), project);
    let mut editor = Editor::new(decode(&encode(&project).unwrap()).unwrap(), 8).unwrap();
    let id = match editor
        .execute(Command::AddTrack { name: "new".into() })
        .unwrap()
    {
        Created::Track(id) => id.value(),
        _ => panic!("wrong track result"),
    };
    assert_eq!(id, project.snapshot().next_id);
}

#[test]
fn schema_twelve_persists_arrangement_midi_and_session_audio_sources_strictly() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "instrument".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(3).unwrap(),
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::from_ratio(1, 2).unwrap(),
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 2,
            velocity: 99,
        })
        .unwrap();
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: BeatTime::from_quarters(6).unwrap(),
            source_start: BeatTime::from_quarters(1).unwrap(),
            loop_start: BeatTime::from_quarters(2).unwrap(),
            loop_end: BeatTime::from_quarters(5).unwrap(),
            looping: true,
            length: BeatTime::from_quarters(3).unwrap(),
        })
        .unwrap();
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene one".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let audio_track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("audio track"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "fixtures/session.wav".into(),
            sample_rate: 48_000,
            frames: 128,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        _ => panic!("asset"),
    };
    let audio_clip = match editor
        .execute(Command::CreateSessionAudioClip {
            track: audio_track,
            scene,
            name: "one shot".into(),
            asset,
            source_start: 13,
            source_end: 77,
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("audio clip"),
    };
    let session_clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "session name".into(),
            source_length: BeatTime::from_quarters(5).unwrap(),
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("session clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: session_clip,
            start: BeatTime::from_quarters(1).unwrap(),
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 72,
            channel: 1,
            velocity: 111,
        })
        .unwrap();
    let stop_scene = match editor
        .execute(Command::AddScene {
            name: "stop scene".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    editor
        .execute(Command::SetSessionStopCell {
            track,
            scene: stop_scene,
        })
        .unwrap();
    let project = editor.project().clone();
    let bytes = encode(&project).unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["version"], 13);
    assert!(
        json["tracks"][0]["audio_clips"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let dto = &json["tracks"][0]["midi_clips"][0];
    assert_eq!(dto["length_raw"], 3_u64 << 32);
    assert_eq!(dto["source_length_raw"], 6_u64 << 32);
    assert_eq!(dto["source_start_raw"], 1_u64 << 32);
    assert_eq!(dto["loop_start_raw"], 2_u64 << 32);
    assert_eq!(dto["loop_end_raw"], 5_u64 << 32);
    assert_eq!(dto["looping"], true);
    assert_eq!(json["session"]["scenes"][0]["name"], "scene one");
    let cells = json["session"]["cells"].as_array().unwrap();
    let audio_cell = cells
        .iter()
        .find(|cell| cell["content"]["kind"] == "audio")
        .unwrap();
    let midi_cell = cells
        .iter()
        .find(|cell| cell["content"]["kind"] == "midi")
        .unwrap();
    assert_eq!(midi_cell["content"]["kind"], "midi");
    assert_eq!(
        cells
            .iter()
            .filter(|cell| cell["content"]["kind"] == "stop")
            .count(),
        1
    );
    assert_eq!(audio_cell["content"]["clip"]["id"], audio_clip.value());
    assert_eq!(audio_cell["content"]["clip"]["asset"], asset.value());
    assert_eq!(audio_cell["content"]["clip"]["source_start"], 13);
    assert_eq!(audio_cell["content"]["clip"]["source_end"], 77);
    assert_eq!(json["session"]["cells"][2]["content"]["kind"], "stop");
    assert_eq!(decode(&bytes).unwrap(), project);
    json["tracks"][0]["midi_clips"][0]["notes"][0]["unexpected"] = true.into();
    assert!(decode(&serde_json::to_vec(&json).unwrap()).is_err());
    let mut missing_session = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
    missing_session.as_object_mut().unwrap().remove("session");
    assert!(decode(&serde_json::to_vec(&missing_session).unwrap()).is_err());
    let mut malformed_session = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
    let malformed_cells = malformed_session["session"]["cells"]
        .as_array_mut()
        .unwrap();
    let malformed_midi = malformed_cells
        .iter_mut()
        .find(|cell| cell["content"]["kind"] == "midi")
        .unwrap();
    malformed_midi["content"]["clip"]["unexpected"] = true.into();
    assert!(decode(&serde_json::to_vec(&malformed_session).unwrap()).is_err());
    let mut bad_range = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
    let audio = bad_range["session"]["cells"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|cell| cell["content"]["kind"] == "audio")
        .unwrap();
    audio["content"]["clip"]["source_end"] = 129.into();
    assert!(decode(&serde_json::to_vec(&bad_range).unwrap()).is_err());
    let mut missing_asset = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
    let audio = missing_asset["session"]["cells"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|cell| cell["content"]["kind"] == "audio")
        .unwrap();
    audio["content"]["clip"]["asset"] = 999.into();
    assert!(decode(&serde_json::to_vec(&missing_asset).unwrap()).is_err());
    json["tracks"][0]["midi_clips"][0]["notes"][0]
        .as_object_mut()
        .unwrap()
        .remove("unexpected");
    json["tracks"][0]["midi_clips"][0]
        .as_object_mut()
        .unwrap()
        .remove("source_length_raw");
    assert!(decode(&serde_json::to_vec(&json).unwrap()).is_err());
    json["tracks"][0]["midi_clips"][0]["source_length_raw"] = 2_u64.into();
    assert!(decode(&serde_json::to_vec(&json).unwrap()).is_err());
}

#[test]
fn schema_nine_literal_migrates_midi_source_to_strict_one_shot_defaults() {
    let legacy = br#"{
        "format":"hex-project","version":9,"sample_rate":48000,"next_id":4,
        "assets":[],"tracks":[{"id":1,"name":"keys","gain":1.0,
          "balance":0.0,"muted":false,"soloed":false,
          "content":{"kind":"instrument","loop_policy":"one_shot",
            "clips":[{"id":2,"start_raw":0,"length_raw":17179869184,
              "notes":[{"id":3,"start_raw":4294967296,"duration_raw":4294967296,
                "pitch":60,"channel":0,"velocity":100}]}]}}],
        "loop_region":null,"master_gain":1.0,
        "tempo_points":[{"beat_raw":0,"micro_bpm":120000000}],
        "meter_points":[{"beat_raw":0,"numerator":4,"denominator":4}]
    }"#;
    let project = decode(legacy).expect("strict schema nine fixture migrates");
    assert_empty_session(&project);
    let clip = &project.tracks()[0].midi_clips()[0];
    assert_eq!(clip.length(), BeatTime::from_quarters(4).unwrap());
    assert_eq!(clip.source_length(), clip.length());
    assert_eq!(clip.source_start(), BeatTime::ZERO);
    assert_eq!(clip.loop_start(), BeatTime::ZERO);
    assert_eq!(clip.loop_end(), clip.length());
    assert!(!clip.looping());

    let mut json: serde_json::Value = serde_json::from_slice(legacy).unwrap();
    json["tracks"][0]["content"]["loop_policy"] = "loop".into();
    assert!(decode(&serde_json::to_vec(&json).unwrap()).is_err());
    json["tracks"][0]["content"]["loop_policy"] = "one_shot".into();
    json["tracks"][0]["content"]["clips"][0]["unexpected"] = true.into();
    assert!(decode(&serde_json::to_vec(&json).unwrap()).is_err());
}

#[test]
fn schema_ten_migrates_to_an_empty_session_grid() {
    let literal = br#"{"format":"hex-project","version":10,"sample_rate":48000,"next_id":2,"assets":[],"tracks":[{"id":1,"name":"keys","gain":1.0,"balance":0.0,"muted":false,"soloed":false,"content":{"kind":"instrument","clips":[]}}],"loop_region":null,"master_gain":1.0,"tempo_points":[{"beat_raw":0,"micro_bpm":120000000}],"meter_points":[{"beat_raw":0,"numerator":4,"denominator":4}]}"#;
    let migrated = decode(literal).unwrap();
    assert_empty_session(&migrated);
}
#[test]
fn malformed_unknown_future_and_oversized_documents_refused() {
    let bytes = encode(&project()).unwrap();
    assert!(decode(&bytes[..bytes.len() - 1]).is_err());
    let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["version"], 13);
    json["version"] = 14.into();
    assert!(decode(&serde_json::to_vec(&json).unwrap()).is_err());
    json["version"] = 13.into();
    json["unrecognized"] = true.into();
    assert!(decode(&serde_json::to_vec(&json).unwrap()).is_err());
    json.as_object_mut().unwrap().remove("unrecognized");
    let mut missing_master = json.clone();
    missing_master
        .as_object_mut()
        .unwrap()
        .remove("master_gain");
    assert!(decode(&serde_json::to_vec(&missing_master).unwrap()).is_err());
    let mut missing_balance = json.clone();
    missing_balance["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("balance");
    assert!(decode(&serde_json::to_vec(&missing_balance).unwrap()).is_err());
    let mut missing_phase = json.clone();
    missing_phase["tracks"][0]["audio_clips"][2]
        .as_object_mut()
        .unwrap()
        .remove("source_phase");
    assert!(decode(&serde_json::to_vec(&missing_phase).unwrap()).is_err());
    let mut invalid_phase = json.clone();
    invalid_phase["tracks"][0]["audio_clips"][2]["source_phase"] = 3.into();
    assert!(decode(&serde_json::to_vec(&invalid_phase).unwrap()).is_err());
    json["tracks"][0]["audio_clips"][0]["asset"] = 999.into();
    assert!(decode(&serde_json::to_vec(&json).unwrap()).is_err());
    assert!(decode(&vec![b' '; MAX_DOCUMENT_BYTES + 1]).is_err());
}

#[test]
fn legacy_schemas_migrate_and_schema_twelve_roundtrips_complete_state() {
    let legacy = br#"{
        "format":"hex-project","version":1,"sample_rate":48000,"next_id":4,
        "assets":[{"id":1,"path":"Media/Legacy.WAV","frames":200}],
        "tracks":[{"id":2,"name":"legacy","gain":1.0,"muted":false,
          "clips":[{"id":3,"asset":1,"start":0,"duration":200,"source_offset":0,
            "source_length":200,"looping":false}]}]
    }"#;
    let migrated = decode(legacy).expect("schema one is supported");
    assert_empty_session(&migrated);
    let asset = migrated.assets().next().expect("legacy asset");
    assert_eq!(asset.source_sample_rate(), 48_000);
    assert_eq!(asset.source_frames(), 200);
    assert_eq!(asset.playback_frames(), 200);
    assert_eq!(asset.conversion_version(), 0);
    assert_eq!(migrated.tracks()[0].clips()[0].source_phase(), 0);
    assert_eq!(migrated.tracks()[0].balance(), 0.0);
    assert_eq!(migrated.master_gain(), 1.0);
    assert_eq!(decode(&encode(&migrated).unwrap()).unwrap(), migrated);
    assert!(!migrated.tracks()[0].soloed());
    let mut invalid_v1: serde_json::Value = serde_json::from_slice(legacy).unwrap();
    invalid_v1["tracks"][0]["unknown"] = true.into();
    assert!(decode(&serde_json::to_vec(&invalid_v1).unwrap()).is_err());

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let (track, asset) = match editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "converted".into(),
            path: "Media/Source-44100.wav".into(),
            source_sample_rate: 44_100,
            source_frames: 44_100,
            playback_frames: 48_000,
            conversion_version: 1,
        })
        .unwrap()
    {
        Created::Imported { track, asset, .. } => (track, asset),
        _ => panic!("wrong import result"),
    };
    let loop_clip = match editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 1_000,
            duration: 9,
            source_offset: 20,
            source_length: 3,
            looping: true,
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("wrong loop clip result"),
    };
    editor
        .execute(Command::SplitClip {
            clip: loop_clip,
            frame: 1_002,
        })
        .unwrap();
    editor
        .execute(Command::ToggleSolo {
            track,
            exclusive: false,
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 100,
            end: 300,
            enabled: true,
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(8).unwrap(),
            tempo: Tempo::new(93_500_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetMeterPoint {
            beat: BeatTime::from_ratio(5, 2).unwrap(),
            meter: Meter::new(7, 8).unwrap(),
        })
        .unwrap();
    let project = editor.project().clone();
    let bytes = encode(&project).unwrap();
    let current_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(current_json["version"], 13);
    assert_eq!(decode(&bytes).unwrap(), project);
    let mut json = current_json.clone();
    json.as_object_mut().unwrap().remove("session");
    json["version"] = 8.into();
    for track in json["tracks"].as_array_mut().unwrap() {
        let clips = track["audio_clips"].take();
        let object = track.as_object_mut().unwrap();
        object.remove("audio_clips");
        object.remove("midi_clips");
        track["clips"] = clips;
    }
    assert_eq!(json["assets"][0]["source_sample_rate"], 44_100);
    assert_eq!(json["assets"][0]["source_frames"], 44_100);
    assert_eq!(json["assets"][0]["playback_frames"], 48_000);
    assert_eq!(json["assets"][0]["conversion_version"], 1);
    assert_eq!(json["tracks"][0]["soloed"], true);
    assert_eq!(json["tracks"][0]["balance"], 0.0);
    assert_eq!(json["master_gain"], 1.0);
    assert_eq!(json["loop_region"]["start"], 100);
    assert_eq!(json["loop_region"]["end"], 300);
    assert_eq!(json["tracks"][0]["clips"][0]["fade"]["span"], 48_000);
    assert_eq!(json["tracks"][0]["clips"][2]["source_phase"], 2);
    assert_eq!(json["tempo_points"][1]["beat_raw"], 8_u64 << 32);
    assert_eq!(json["tempo_points"][1]["micro_bpm"], 93_500_000);
    assert_eq!(json["meter_points"][1]["beat_raw"], (5_u64 << 31));
    assert_eq!(json["meter_points"][1]["numerator"], 7);
    let migrated_v8 = decode(&serde_json::to_vec(&json).unwrap()).unwrap();
    assert_empty_session(&migrated_v8);
    assert_eq!(decode(&bytes).unwrap(), project);

    let mut invalid_mix = json.clone();
    invalid_mix["tracks"][0]["balance"] = 1.01.into();
    assert!(decode(&serde_json::to_vec(&invalid_mix).unwrap()).is_err());
    invalid_mix["tracks"][0]["balance"] = 0.0.into();
    invalid_mix["master_gain"] = 2.01.into();
    assert!(decode(&serde_json::to_vec(&invalid_mix).unwrap()).is_err());

    let mut legacy_v2 = json.clone();
    legacy_v2["version"] = 2.into();
    legacy_v2.as_object_mut().unwrap().remove("tempo_points");
    legacy_v2.as_object_mut().unwrap().remove("meter_points");
    legacy_v2.as_object_mut().unwrap().remove("loop_region");
    legacy_v2.as_object_mut().unwrap().remove("master_gain");
    for track in legacy_v2["tracks"].as_array_mut().unwrap() {
        track.as_object_mut().unwrap().remove("soloed");
        track.as_object_mut().unwrap().remove("balance");
        for clip in track["clips"].as_array_mut().unwrap() {
            clip.as_object_mut().unwrap().remove("fade");
            clip.as_object_mut().unwrap().remove("source_phase");
        }
    }
    let migrated_v2 = decode(&serde_json::to_vec(&legacy_v2).unwrap()).unwrap();
    assert_empty_session(&migrated_v2);
    assert!(
        migrated_v2
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .all(|clip| clip.source_phase() == 0)
    );
    assert_eq!(
        migrated_v2.assets().next().unwrap().source_sample_rate(),
        44_100
    );
    assert!(!migrated_v2.tracks()[0].soloed());
    assert_eq!(migrated_v2.tracks()[0].balance(), 0.0);
    assert_eq!(migrated_v2.master_gain(), 1.0);
    legacy_v2["tracks"][0]["unknown"] = true.into();
    assert!(decode(&serde_json::to_vec(&legacy_v2).unwrap()).is_err());

    let mut legacy_v3 = json.clone();
    legacy_v3["version"] = 3.into();
    legacy_v3.as_object_mut().unwrap().remove("tempo_points");
    legacy_v3.as_object_mut().unwrap().remove("meter_points");
    legacy_v3.as_object_mut().unwrap().remove("loop_region");
    legacy_v3.as_object_mut().unwrap().remove("master_gain");
    legacy_v3["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("balance");
    for track in legacy_v3["tracks"].as_array_mut().unwrap() {
        for clip in track["clips"].as_array_mut().unwrap() {
            clip.as_object_mut().unwrap().remove("fade");
            clip.as_object_mut().unwrap().remove("source_phase");
        }
    }
    let migrated_v3 = decode(&serde_json::to_vec(&legacy_v3).unwrap()).unwrap();
    assert_empty_session(&migrated_v3);
    assert!(
        migrated_v3
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .all(|clip| clip.source_phase() == 0)
    );
    assert_eq!(migrated_v3.loop_region(), None);
    assert!(migrated_v3.tracks()[0].soloed());
    assert_eq!(migrated_v3.tracks()[0].balance(), 0.0);
    assert_eq!(migrated_v3.master_gain(), 1.0);
    assert_eq!(migrated_v3.assets().next().unwrap().conversion_version(), 1);

    let mut legacy_v4 = json.clone();
    legacy_v4["version"] = 4.into();
    legacy_v4.as_object_mut().unwrap().remove("tempo_points");
    legacy_v4.as_object_mut().unwrap().remove("meter_points");
    legacy_v4.as_object_mut().unwrap().remove("master_gain");
    legacy_v4["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("balance");
    for track in legacy_v4["tracks"].as_array_mut().unwrap() {
        for clip in track["clips"].as_array_mut().unwrap() {
            clip.as_object_mut().unwrap().remove("fade");
            clip.as_object_mut().unwrap().remove("source_phase");
        }
    }
    let migrated_v4 = decode(&serde_json::to_vec(&legacy_v4).unwrap()).unwrap();
    assert_empty_session(&migrated_v4);
    assert!(
        migrated_v4
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .all(|clip| clip.source_phase() == 0)
    );
    assert_eq!(migrated_v4.loop_region(), project.loop_region());
    assert!(migrated_v4.tracks()[0].soloed());
    assert_eq!(migrated_v4.tracks()[0].balance(), 0.0);
    assert_eq!(migrated_v4.master_gain(), 1.0);

    let mut legacy_v5 = json.clone();
    legacy_v5["version"] = 5.into();
    legacy_v5.as_object_mut().unwrap().remove("tempo_points");
    legacy_v5.as_object_mut().unwrap().remove("meter_points");
    for track in legacy_v5["tracks"].as_array_mut().unwrap() {
        for clip in track["clips"].as_array_mut().unwrap() {
            clip.as_object_mut().unwrap().remove("fade");
            clip.as_object_mut().unwrap().remove("source_phase");
        }
    }
    let migrated_v5 = decode(&serde_json::to_vec(&legacy_v5).unwrap()).unwrap();
    assert_empty_session(&migrated_v5);
    assert!(
        migrated_v5
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .all(|clip| clip.source_phase() == 0)
    );
    assert_eq!(migrated_v5.tracks()[0].clips()[0].fade().span(), 48_000);
    assert_eq!(migrated_v5.tracks()[0].clips()[0].fade().offset(), 0);
    assert_eq!(migrated_v5.tracks()[0].clips()[0].fade().fade_in(), 0);
    assert_eq!(migrated_v5.tracks()[0].clips()[0].fade().fade_out(), 0);
    assert_eq!(decode(&encode(&migrated_v5).unwrap()).unwrap(), migrated_v5);

    let mut legacy_v6 = json.clone();
    legacy_v6["version"] = 6.into();
    legacy_v6.as_object_mut().unwrap().remove("tempo_points");
    legacy_v6.as_object_mut().unwrap().remove("meter_points");
    for track in legacy_v6["tracks"].as_array_mut().unwrap() {
        for clip in track["clips"].as_array_mut().unwrap() {
            clip.as_object_mut().unwrap().remove("source_phase");
        }
    }
    let migrated_v6 = decode(&serde_json::to_vec(&legacy_v6).unwrap()).unwrap();
    assert_empty_session(&migrated_v6);
    assert!(
        migrated_v6
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .all(|clip| clip.source_phase() == 0)
    );

    let mut legacy_v7 = json.clone();
    legacy_v7["version"] = 7.into();
    legacy_v7.as_object_mut().unwrap().remove("tempo_points");
    legacy_v7.as_object_mut().unwrap().remove("meter_points");
    let migrated_v7 = decode(&serde_json::to_vec(&legacy_v7).unwrap()).unwrap();
    assert_empty_session(&migrated_v7);
    assert_eq!(migrated_v7.tempo_map().points().len(), 1);
    assert_eq!(migrated_v7.tempo_map().points()[0].tempo(), Tempo::DEFAULT);
    assert_eq!(migrated_v7.meter_map().points().len(), 1);
    assert_eq!(
        migrated_v7.meter_map().points()[0].meter(),
        Meter::COMMON_TIME
    );
    assert_eq!(migrated_v7.tracks()[0].clips()[0].start(), 0);

    let mut invalid = json.clone();
    invalid["assets"][0]["conversion_version"] = 90.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid["assets"][0]["conversion_version"] = 1.into();
    invalid["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("soloed");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid.as_object_mut().unwrap().remove("loop_region");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["tracks"][0]["clips"][0]
        .as_object_mut()
        .unwrap()
        .remove("fade");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["tracks"][0]["clips"][0]["fade"]["offset"] = 48_000.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["tracks"][0]["clips"][0]["fade"]
        .as_object_mut()
        .unwrap()
        .remove("fade_out");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["tracks"][0]["clips"][0]["fade"]["unrecognized"] = true.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["tracks"][0]["clips"][0]["fade"]["fade_in"] = 48_001.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["tracks"][0]["clips"][0]["duration"] = u64::MAX.into();
    invalid["tracks"][0]["clips"][0]["fade"]["offset"] = 1.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["loop_region"]["end"] = 100.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid.as_object_mut().unwrap().remove("tempo_points");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid.as_object_mut().unwrap().remove("meter_points");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["tempo_points"][0]["micro_bpm"] = 0.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["meter_points"][0]["denominator"] = 3.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["tempo_points"][0]["unknown"] = true.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = json.clone();
    invalid["meter_points"][0]["beat_raw"] = 1.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
}

#[test]
fn schema_eleven_session_midi_and_stop_migrate_to_schema_twelve_model() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "keys".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "session midi".into(),
            source_length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        _ => panic!("session clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(1).unwrap(),
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 64,
            channel: 0,
            velocity: 90,
        })
        .unwrap();
    let stop_scene = match editor
        .execute(Command::AddScene {
            name: "stop marker scene".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        _ => panic!("stop scene"),
    };
    editor
        .execute(Command::SetSessionStopCell {
            track,
            scene: stop_scene,
        })
        .unwrap();
    let expected = editor.project().clone();
    let current = encode(&expected).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&current).unwrap();
    let midi_clips = value["tracks"][0]["midi_clips"].take();
    let track_object = value["tracks"][0].as_object_mut().unwrap();
    track_object.remove("audio_clips");
    track_object.remove("midi_clips");
    value["tracks"][0]["content"] = serde_json::json!({
        "kind": "instrument",
        "clips": midi_clips,
    });
    value["version"] = 11.into();
    let migrated = decode(&serde_json::to_vec(&value).unwrap()).expect("schema eleven migrates");
    assert_eq!(migrated, expected);
    assert_eq!(
        migrated
            .session_midi_clip(clip)
            .expect("schema eleven Session MIDI clip")
            .source()
            .notes()
            .len(),
        1
    );
    assert!(matches!(
        migrated.session_cell(track, stop_scene),
        Some(hex_project::SessionCellContent::Stop)
    ));
}

#[test]
fn literal_schema_eleven_migrates_session_midi_and_stop_cells() {
    let literal = br#"{
        "format":"hex-project","version":11,"sample_rate":48000,"next_id":6,
        "assets":[],
        "tracks":[{"id":1,"name":"keys","gain":1.0,"balance":0.0,
          "muted":false,"soloed":false,"content":{"kind":"audio","clips":[]}}],
        "loop_region":null,"master_gain":1.0,
        "tempo_points":[{"beat_raw":0,"micro_bpm":120000000}],
        "meter_points":[{"beat_raw":0,"numerator":4,"denominator":4}],
        "session":{"scenes":[{"id":2,"name":"midi"},{"id":3,"name":"stop"}],
          "cells":[
            {"track":1,"scene":2,"content":{"kind":"midi","clip":{
              "id":4,"name":"legacy midi","source":{
                "source_length_raw":4294967296,"source_start_raw":0,
                "loop_start_raw":0,"loop_end_raw":4294967296,"looping":false,
                "notes":[{"id":5,"start_raw":0,"duration_raw":4294967296,
                  "pitch":60,"channel":0,"velocity":100}]}}}},
            {"track":1,"scene":3,"content":{"kind":"stop"}}
          ]}
    }"#;
    let migrated = decode(literal).expect("literal schema eleven");
    let track = migrated.tracks()[0].id();
    let midi_scene = migrated.scenes()[0].id();
    let stop_scene = migrated.scenes()[1].id();
    let cell = migrated
        .session_cell(track, midi_scene)
        .expect("migrated midi cell");
    assert!(matches!(cell, hex_project::SessionCellContent::Midi(_)));
    assert!(matches!(
        migrated.session_cell(track, stop_scene),
        Some(hex_project::SessionCellContent::Stop)
    ));
    assert_eq!(decode(&encode(&migrated).unwrap()).unwrap(), migrated);
}

#[test]
fn literal_schema_twelve_migrates_session_audio_cell_and_asset_window() {
    let literal = br#"{
        "format":"hex-project","version":12,"sample_rate":48000,"next_id":5,
        "assets":[{"id":2,"path":"Media/one-shot.wav","source_sample_rate":48000,
          "source_frames":8,"playback_frames":8,"conversion_version":0}],
        "tracks":[{"id":1,"name":"hybrid","gain":1.0,"balance":0.0,
          "muted":false,"soloed":false,"content":{"kind":"audio","clips":[]}}],
        "loop_region":null,"master_gain":1.0,
        "tempo_points":[{"beat_raw":0,"micro_bpm":120000000}],
        "meter_points":[{"beat_raw":0,"numerator":4,"denominator":4}],
        "session":{"scenes":[{"id":3,"name":"audio"}],
          "cells":[{"track":1,"scene":3,"content":{"kind":"audio","clip":{
            "id":4,"name":"legacy one shot","asset":2,
            "source_start":2,"source_end":7}}}]}
    }"#;
    let migrated = decode(literal).expect("literal schema twelve");
    let track = migrated.tracks()[0].id();
    let scene = migrated.scenes()[0].id();
    let Some(hex_project::SessionCellContent::Audio(clip)) = migrated.session_cell(track, scene)
    else {
        panic!("session audio cell was not migrated");
    };
    assert_eq!(clip.source_start(), 2);
    assert_eq!(clip.source_end(), 7);
    assert_eq!(migrated.asset(clip.asset()).expect("asset").frames(), 8);
    assert_eq!(decode(&encode(&migrated).unwrap()).unwrap(), migrated);
}
#[test]
fn new_file_is_complete_and_old_file_survives_failures() {
    let directory = std::env::temp_dir().join(format!("hex-storage-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let path = directory.join("Project.hex");
    let project = project();
    save_new(&path, &project).unwrap();
    assert_eq!(load(&path).unwrap(), project);
    let old = fs::read(&path).unwrap();
    assert!(save_new(&path, &Project::new(44_100).unwrap()).is_err());
    assert_eq!(fs::read(&path).unwrap(), old);
    assert!(save_new(directory.join("missing/sub.hex"), &project).is_err());
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}
#[cfg(unix)]
#[test]
fn non_utf8_path_refused_without_lossy_rewrite() {
    use std::os::unix::ffi::OsStringExt;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: std::ffi::OsString::from_vec(vec![b'a', 255]).into(),
            sample_rate: 48_000,
            frames: 1,
        })
        .unwrap();
    assert!(encode(editor.project()).is_err());
}

#[test]
fn resized_midi_clip_roundtrips_retained_notes_and_source_marker() {
    let beat = |n| BeatTime::from_quarters(n).expect("small beat");
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 12).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "hybrid".into(),
        })
        .expect("track")
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(4),
            length: beat(8),
        })
        .expect("clip")
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(2),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .expect("note");
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(6),
            duration: beat(2),
            pitch: 67,
            channel: 0,
            velocity: 90,
        })
        .expect("sustained note");
    let notes = editor
        .project()
        .midi_clip(clip)
        .expect("clip")
        .notes()
        .to_vec();
    editor
        .execute(Command::TrimMidiClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            beat: beat(7),
        })
        .expect("left resize");
    editor
        .execute(Command::TrimMidiClipEdge {
            clip,
            edge: hex_project::TrimEdge::Right,
            beat: beat(15),
        })
        .expect("right resize");
    let Created::Clip(right_id) = editor
        .execute(Command::SplitMidiClip {
            clip,
            beat: beat(11),
        })
        .expect("split")
    else {
        panic!("right clip")
    };
    let restored = decode(&encode(editor.project()).expect("encode")).expect("decode");
    assert_eq!(restored.snapshot(), editor.project().snapshot());
    let resized = restored.midi_clip(clip).expect("retained clip");
    assert_eq!(resized.notes(), notes);
    assert_eq!(resized.start(), beat(7));
    assert_eq!(resized.length(), beat(4));
    assert_eq!(resized.source_start(), beat(3));
    assert_eq!(resized.source_length(), beat(8));
    let right = restored.midi_clip(right_id).expect("split right clip");
    assert_eq!(right.start(), beat(11));
    assert_eq!(right.length(), beat(4));
    assert_eq!(right.source_start(), BeatTime::ZERO);
    assert_eq!(right.source_length(), beat(4));
    assert!(!right.looping());
    assert_eq!(right.notes().len(), 1);
    assert_eq!(right.notes()[0].start(), BeatTime::ZERO);
    assert_eq!(right.notes()[0].duration(), beat(1));
    assert_ne!(right.notes()[0].id(), resized.notes()[1].id());
}
