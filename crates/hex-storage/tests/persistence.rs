use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{BeatTime, Command, Created, Editor, Meter, Project, Tempo};
use hex_storage::{LEGACY_READER_DOCUMENT_BYTES, decode, encode, load, save_new};
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
fn current_format_roundtrips_return_devices_sends_and_strict_fields() {
    let mut editor = Editor::new(project(), 32).unwrap();
    let track = editor.project().tracks()[0].id();
    let return_track = match editor
        .execute(Command::AddReturnTrack {
            name: "Verb return".into(),
        })
        .unwrap()
    {
        Created::ReturnTrack(id) => id,
        other => panic!("wrong return result: {other:?}"),
    };
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track,
            gain: 0.625,
        })
        .unwrap();
    let effect_state = NativeDeviceState::new(
        DeviceTypeId::new("vendor.future-return-effect").unwrap(),
        27,
        vec![0, 9, 255],
    )
    .unwrap();
    let device = match editor
        .execute(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: effect_state.clone(),
        })
        .unwrap()
    {
        Created::Device(id) => id,
        other => panic!("wrong return device result: {other:?}"),
    };
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let with_returns = editor.project().clone();
    let bytes = encode(&with_returns).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(json["return_tracks"][0]["name"], "Verb return");
    assert_eq!(
        json["return_tracks"][0]["devices"][0]["type_id"],
        "vendor.future-return-effect"
    );
    assert_eq!(
        json["tracks"][0]["sends"][0]["return_track"],
        return_track.value()
    );
    assert_eq!(decode(&bytes).unwrap(), with_returns);
    assert_eq!(
        decode(&bytes).unwrap().device(device).unwrap().state(),
        &effect_state
    );

    let mut missing = json.clone();
    missing["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("sends");
    assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    let mut invalid_target = json.clone();
    invalid_target["tracks"][0]["sends"][0]["return_track"] = u64::MAX.into();
    assert!(decode(&serde_json::to_vec(&invalid_target).unwrap()).is_err());
    let mut unknown_return_field = json;
    unknown_return_field["return_tracks"][0]["surprise"] = true.into();
    assert!(decode(&serde_json::to_vec(&unknown_return_field).unwrap()).is_err());
}

#[test]
fn eq8_nondefault_independent_banks_roundtrip_as_generic_opaque_state() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::AddTrack {
            name: "eq8 storage".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let return_track = match editor
        .execute(Command::AddReturnTrack {
            name: "eq8 return".into(),
        })
        .unwrap()
    {
        Created::ReturnTrack(id) => id,
        _ => panic!("return track"),
    };

    // The EQ codec owns this valid version-1 payload; storage deliberately sees
    // only an opaque type/version/byte envelope and preserves every channel bank.
    let mut payload = Vec::with_capacity(565);
    payload.push(2); // mid/side banks selected
    payload.extend_from_slice(&1.25_f32.to_le_bytes());
    for bank in 0..5 {
        for band in 0..8 {
            let enabled = (bank == 1 && band == 2) || (bank == 4 && band == 6);
            let shape = if bank == 1 { 7 } else { 0 };
            let frequency: f32 = if bank == 1 && band == 2 {
                1_337.0
            } else if bank == 4 && band == 6 {
                8_765.0
            } else {
                [
                    40.0, 100.0, 250.0, 500.0, 1_000.0, 2_500.0, 6_000.0, 12_000.0,
                ][band]
            };
            let gain: f32 = if bank == 1 && band == 2 { 9.5 } else { 0.0 };
            let q = if bank == 4 && band == 6 {
                3.25
            } else {
                std::f32::consts::FRAC_1_SQRT_2
            };
            payload.push(u8::from(enabled));
            payload.push(shape);
            payload.extend_from_slice(&frequency.to_le_bytes());
            payload.extend_from_slice(&gain.to_le_bytes());
            payload.extend_from_slice(&q.to_le_bytes());
        }
    }
    let eq8 = NativeDeviceState::new(DeviceTypeId::from_static("hex.eq8"), 1, payload).unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: eq8.clone(),
        })
        .unwrap();
    editor
        .execute(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: eq8.clone(),
        })
        .unwrap();

    let project = editor.project().clone();
    let encoded = encode(&project).unwrap();
    let decoded = decode(&encoded).unwrap();
    assert_eq!(decoded, project);
    assert_eq!(decoded.track(track).unwrap().devices()[0].state(), &eq8);
    assert_eq!(
        decoded.return_tracks()[0].devices()[0].state(),
        &eq8,
        "return-track storage preserves the same independent bank payload"
    );
}

#[test]
fn schema_fourteen_roundtrips_generic_opaque_device_envelopes() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::AddTrack {
            name: "devices".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let first_state = NativeDeviceState::new(
        DeviceTypeId::new("hex.test-first").unwrap(),
        1,
        vec![1, 2, 3],
    )
    .unwrap();
    let first = match editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: first_state,
        })
        .unwrap()
    {
        Created::Device(id) => id,
        _ => panic!("device"),
    };
    let second_state = NativeDeviceState::new(
        DeviceTypeId::new("vendor.future-device").unwrap(),
        19,
        vec![0, 255, 7, 8],
    )
    .unwrap();
    let second = match editor
        .execute(Command::InsertDevice {
            track,
            before: Some(first),
            state: second_state.clone(),
        })
        .unwrap()
    {
        Created::Device(id) => id,
        _ => panic!("device"),
    };
    editor
        .execute(Command::SetDeviceBypass {
            device: first,
            bypassed: true,
        })
        .unwrap();
    let project = editor.project().clone();
    let bytes = encode(&project).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(json["tracks"][0]["devices"][0]["id"], second.value());
    assert_eq!(json["tracks"][0]["devices"][1]["id"], first.value());
    assert_eq!(
        json["tracks"][0]["devices"][0]["type_id"],
        "vendor.future-device"
    );
    assert_eq!(json["tracks"][0]["devices"][0]["state_version"], 19);
    assert_eq!(
        json["tracks"][0]["devices"][0]["payload"],
        serde_json::json!([0, 255, 7, 8])
    );
    assert!(
        json["tracks"][0]["devices"][1]["bypassed"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(decode(&bytes).unwrap(), project);
    assert_eq!(
        decode(&bytes).unwrap().device(second).unwrap().state(),
        &second_state
    );

    let mut bad = json.clone();
    bad["tracks"][0]["devices"][0]["extra"] = true.into();
    assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    let mut invalid = json.clone();
    invalid["tracks"][0]["devices"][0]["state_version"] = 0.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    let mut duplicate = json;
    duplicate["tracks"][0]["devices"][0]["id"] = duplicate["tracks"][0]["id"].clone();
    assert!(decode(&serde_json::to_vec(&duplicate).unwrap()).is_err());
}

#[test]
fn registered_operator_nondefault_envelope_roundtrips_as_opaque_state() {
    check_operator_opaque_roundtrip(1);
}

#[test]
fn registered_operator_waveform_choices_roundtrip_as_opaque_state() {
    check_operator_opaque_roundtrip(2);
}

#[test]
fn registered_operator_fixed_tuning_roundtrips_as_opaque_state() {
    check_operator_opaque_roundtrip(3);
}

#[test]
fn registered_operator_velocity_sensitivities_roundtrip_as_opaque_state() {
    check_operator_opaque_roundtrip(4);
}

#[test]
fn registered_operator_authored_feedback_roundtrips_as_opaque_state() {
    check_operator_opaque_roundtrip(5);
}

fn check_operator_opaque_roundtrip(state_version: u32) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "operator state".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };

    // Valid Operator envelopes and waveform choices use only the existing
    // opaque state contract; storage must preserve it without a device-codec edge.
    let mut fields = [0.0_f32; 56];
    fields[0] = 1.0;
    fields[1] = 1.0;
    fields[3] = 2.0;
    fields[4] = 1.0;
    fields[6] = 3.0;
    fields[7] = 1.0;
    fields[9] = 4.0;
    fields[10] = 1.0;
    fields[28] = 0.5;
    if state_version >= 5 {
        // Incoming op1 modulation disables op2 feedback but must not erase
        // its authored value from this generic persistence boundary.
        fields[13] = 0.375;
    }
    for operator in 0..4 {
        let offset = 32 + operator * 6;
        fields[offset..offset + 6].copy_from_slice(&[0.0, 1.0, 0.7, 0.005, 0.1, 0.1]);
    }
    let mut payload: Vec<u8> = fields.into_iter().flat_map(f32::to_le_bytes).collect();
    if state_version >= 2 {
        for choice in [1_u32, 2, 3, 0] {
            payload.extend_from_slice(&choice.to_le_bytes());
        }
    }
    if state_version >= 3 {
        for index in 0..4 {
            payload.extend_from_slice(&u32::from(index % 2 == 0).to_le_bytes());
            payload.extend_from_slice(&(100.0_f32 * (index + 1) as f32).to_le_bytes());
            payload.extend_from_slice(&2.0_f32.to_le_bytes());
        }
    }
    if state_version >= 4 {
        for amount in [0.0_f32, 0.25, 0.75, 1.0] {
            payload.extend_from_slice(&amount.to_le_bytes());
        }
    }
    if state_version >= 5 {
        for amount in [0.4_f32, 0.8, 0.2, 0.0] {
            payload.extend_from_slice(&amount.to_le_bytes());
        }
    }
    let state = NativeDeviceState::new(
        DeviceTypeId::new("hex.instrument.operator").unwrap(),
        state_version,
        payload,
    )
    .unwrap();
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state: state.clone(),
        })
        .unwrap();

    let encoded = encode(editor.project()).unwrap();
    let reopened = decode(&encoded).unwrap();
    assert_eq!(reopened.device(instrument).unwrap().state(), &state);
}

#[test]
fn current_format_preserves_opaque_instrument_state_and_requires_its_field() {
    let mut current = Project::new(48_000).unwrap();
    let mut editor = Editor::new(current.clone(), 8).unwrap();
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "opaque".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    current = editor.project().clone();
    let instrument = current.track(track).unwrap().instrument().unwrap().id();
    let opaque = NativeDeviceState::new(
        DeviceTypeId::new("future.instrument").unwrap(),
        27,
        vec![9, 0, 255, 17],
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state: opaque.clone(),
        })
        .unwrap();
    let bytes = encode(editor.project()).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(
        json["native_sources"][0]["devices"][0]["device"]["type_id"],
        "future.instrument"
    );
    assert_eq!(
        decode(&bytes).unwrap().device(instrument).unwrap().state(),
        &opaque
    );
    let mut missing = json;
    missing["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("instrument");
    assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());
}

#[test]
fn schema_sixteen_roundtrips_chain_position_and_rejects_malformed_positions() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "chain".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let effect = match editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: NativeDeviceState::new(DeviceTypeId::new("test.effect").unwrap(), 1, vec![1])
                .unwrap(),
        })
        .unwrap()
    {
        Created::Device(id) => id,
        _ => panic!("effect"),
    };
    editor
        .execute(Command::MoveTrackDevice {
            device: instrument,
            track,
            before: None,
        })
        .unwrap();
    assert_eq!(
        editor.project().track(track).unwrap().instrument_position(),
        Some(1)
    );
    let project = editor.project().clone();
    let bytes = encode(&project).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(json["track_source_bindings"][0]["instrument_position"], 1);
    assert_eq!(decode(&bytes).unwrap(), project);

    let mut missing = json.clone();
    missing["track_source_bindings"][0]
        .as_object_mut()
        .unwrap()
        .remove("instrument_position");
    assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());

    let mut out_of_range = json.clone();
    out_of_range["track_source_bindings"][0]["instrument_position"] = 2.into();
    assert!(decode(&serde_json::to_vec(&out_of_range).unwrap()).is_err());

    let mut orphaned = json.clone();
    orphaned["track_source_bindings"][0]["source"] = serde_json::Value::Null;
    orphaned["track_source_bindings"][0]["instrument_position"] = 0.into();
    assert!(decode(&serde_json::to_vec(&orphaned).unwrap()).is_err());

    let mut missing_instrument = json;
    missing_instrument["track_source_bindings"][0]
        .as_object_mut()
        .unwrap()
        .remove("source");
    assert!(decode(&serde_json::to_vec(&missing_instrument).unwrap()).is_err());
    assert!(editor.project().device(effect).is_some());
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
    assert_eq!(json["version"], hex_storage::CURRENT_SCHEMA_VERSION);
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
fn malformed_unknown_future_and_large_invalid_documents_refused() {
    let bytes = encode(&project()).unwrap();
    assert!(decode(&bytes[..bytes.len() - 1]).is_err());
    let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    let mut future = json.clone();
    future["version"] = (hex_storage::NEWEST_SCHEMA_VERSION + 1).into();
    assert!(decode(&serde_json::to_vec(&future).unwrap()).is_err());
    let mut unknown = json.clone();
    unknown["unrecognized"] = true.into();
    assert!(decode(&serde_json::to_vec(&unknown).unwrap()).is_err());
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
    assert!(decode(&vec![b' '; LEGACY_READER_DOCUMENT_BYTES + 1]).is_err());
}

#[test]
fn current_format_roundtrips_converted_media_phase_loop_tempo_and_meter() {
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
            start: 48_000,
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
            frame: 48_002,
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
    assert_eq!(current_json["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(decode(&bytes).unwrap(), project);
    assert_eq!(current_json["assets"][0]["source_sample_rate"], 44_100);
    assert_eq!(current_json["assets"][0]["playback_frames"], 48_000);
    assert_eq!(
        current_json["tracks"][0]["audio_clips"][2]["source_phase"],
        2
    );
    for (field, value) in [("balance", 1.01), ("gain", -0.1)] {
        let mut invalid = current_json.clone();
        invalid["tracks"][0][field] = value.into();
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut invalid = current_json.clone();
    invalid["master_gain"] = 2.01.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    let mut invalid = current_json.clone();
    invalid["assets"][0]["conversion_version"] = 90.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid["assets"][0]["conversion_version"] = 1.into();
    invalid["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("soloed");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid.as_object_mut().unwrap().remove("loop_region");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["tracks"][0]["audio_clips"][0]
        .as_object_mut()
        .unwrap()
        .remove("fade");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["tracks"][0]["audio_clips"][0]["fade"]["offset"] = 48_000.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["tracks"][0]["audio_clips"][0]["fade"]
        .as_object_mut()
        .unwrap()
        .remove("fade_out");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["tracks"][0]["audio_clips"][0]["fade"]["unrecognized"] = true.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["tracks"][0]["audio_clips"][0]["fade"]["fade_in"] = 48_001.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["tracks"][0]["audio_clips"][0]["duration"] = u64::MAX.into();
    invalid["tracks"][0]["audio_clips"][0]["fade"]["offset"] = 1.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["loop_region"]["end"] = 100.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid.as_object_mut().unwrap().remove("tempo_points");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid.as_object_mut().unwrap().remove("meter_points");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["tempo_points"][0]["micro_bpm"] = 0.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["meter_points"][0]["denominator"] = 3.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["tempo_points"][0]["unknown"] = true.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    invalid = current_json.clone();
    invalid["meter_points"][0]["beat_raw"] = 1.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
}

#[test]
fn current_session_midi_and_stop_cells_roundtrip() {
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
    let reopened = decode(&current).unwrap();
    assert_eq!(reopened, expected);
    assert_eq!(
        reopened
            .session_midi_clip(clip)
            .unwrap()
            .source()
            .notes()
            .len(),
        1
    );
    assert!(matches!(
        reopened.session_cell(track, stop_scene),
        Some(hex_project::SessionCellContent::Stop)
    ));
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
    assert_eq!(resized.source_start(), BeatTime::ZERO);
    assert_eq!(resized.playback_offset(), beat(3));
    assert_eq!(
        resized
            .source_playback()
            .unwrap()
            .source_phase_at_elapsed(BeatTime::ZERO),
        Some(beat(3))
    );
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

#[test]
fn host_output_trim_roundtrips_and_rejects_invalid_gain() {
    let mut editor = Editor::new(project(), 8).unwrap();
    let track = editor.project().tracks()[0].id();
    let state = NativeDeviceState::new(DeviceTypeId::from_static("test.device"), 1, vec![1, 2])
        .unwrap()
        .with_input_gain_db(3.0)
        .unwrap()
        .with_output_gain_db(-9.5)
        .unwrap();
    let Created::Device(id) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("device");
    };
    let bytes = encode(editor.project()).unwrap();
    let restored = decode(&bytes).unwrap();
    assert_eq!(
        restored.device(id).unwrap().state().output_gain_millidb(),
        -9500
    );
    assert_eq!(restored.device(id).unwrap().state().input_gain_db(), 3.0);
    let mut invalid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    invalid["tracks"][0]["devices"][0]["output_gain_millidb"] = 24_001.into();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    assert!(
        restored
            .device(id)
            .unwrap()
            .state()
            .clone()
            .with_output_gain_db(f32::NAN)
            .is_err()
    );
}

#[test]
fn invalid_host_input_gain_refuses_loading() {
    let mut editor = Editor::new(project(), 8).unwrap();
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: NativeDeviceState::new(DeviceTypeId::from_static("test.device"), 1, vec![])
                .unwrap(),
        })
        .unwrap();
    let mut json: serde_json::Value =
        serde_json::from_slice(&encode(editor.project()).unwrap()).unwrap();
    json["tracks"][0]["devices"][0]["input_gain_millidb"] = (-60_001).into();
    assert!(decode(&serde_json::to_vec(&json).unwrap()).is_err());
}

#[test]
fn current_audio_clip_gains_roundtrip_and_refuse_invalid_values() {
    let mut editor = Editor::new(project(), 8).unwrap();
    let clip = editor.project().tracks()[0].clips()[0].id();
    editor
        .execute(Command::SetAudioClipGain {
            clip,
            millidb: -6_000,
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "gain".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session) = editor
        .execute(Command::CopyArrangementAudioClipToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("session")
    };
    editor
        .execute(Command::SetAudioClipGain {
            clip: session,
            millidb: 3_000,
        })
        .unwrap();
    let bytes = encode(editor.project()).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(decode(&bytes).unwrap(), *editor.project());
    for value in [
        serde_json::json!(-60_001),
        serde_json::json!(24_001),
        serde_json::json!(1.5),
        serde_json::Value::Null,
    ] {
        for session in [false, true] {
            let mut invalid = json.clone();
            if session {
                invalid["audio_clip_state"][1]["gain_millidb"] = value.clone();
            } else {
                invalid["audio_clip_state"][0]["gain_millidb"] = value.clone();
            }
            assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
    }
}

#[test]
fn historical_layout_admission_has_no_blanket_document_byte_ceiling() {
    let project = Project::new(48_000).unwrap();
    let mut historical = hex_storage::encode_schema20(&project).unwrap();
    historical.resize(LEGACY_READER_DOCUMENT_BYTES + 1, b' ');
    assert_eq!(decode(&historical).unwrap(), project);
}
