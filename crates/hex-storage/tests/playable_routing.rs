use hex_project::{Command, Created, Editor, Project, RoutingOwner};
use hex_storage::{decode, encode, encode_schema20, encode_schema22};
use serde_json::{Value, json};

fn fixture() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let Created::Track(id) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "track".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track");
        };
        ids.push(id);
    }
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack { name: "bus".into() })
        .unwrap()
    else {
        panic!("return");
    };
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(ids[0]),
            destination: ids[1],
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnSend {
            track: ids[1],
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Return(bus),
            destination: ids[2],
            gain: 0.25,
        })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination: ids[1],
            source: Some(ids[0]),
        })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination: ids[2],
            source: Some(ids[1]),
        })
        .unwrap();
    editor.project().clone()
}

#[test]
fn schema24_roundtrip_preserves_typed_routes_ids_gain_and_midi_bus_selection() {
    let project = fixture();
    let bytes = encode(&project).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["version"], hex_storage::CURRENT_SCHEMA_VERSION);
    assert_eq!(value["audio_feeds"][0]["source"]["kind"], "track");
    assert_eq!(value["audio_feeds"][1]["source"]["kind"], "return");
    let loaded = decode(&bytes).unwrap();
    assert_eq!(loaded, project);
    assert_eq!(
        loaded.midi_source_track(loaded.tracks()[2].id()),
        loaded.tracks()[0].id()
    );
    assert!(encode_schema20(&project).is_err());
    assert!(encode_schema22(&project).is_err());
}

#[test]
fn schema24_rejects_malformed_ports_identity_cycles_and_unbounded_lists() {
    let base: Value = serde_json::from_slice(&encode(&fixture()).unwrap()).unwrap();
    for (field, key, value) in [
        ("audio_feeds", "gain", json!(0)),
        ("audio_feeds", "gain", json!(2.1)),
        ("audio_feeds", "gain", Value::Null),
        ("audio_feeds", "id", base["midi_routes"][0]["id"].clone()),
        ("audio_feeds", "destination", json!(0)),
        ("audio_feeds", "extra", json!(true)),
        ("midi_routes", "source", base["tracks"][1]["id"].clone()),
        ("midi_routes", "destination", json!(0)),
        ("midi_routes", "id", base["tracks"][0]["id"].clone()),
        ("midi_routes", "extra", json!(true)),
    ] {
        let mut invalid = base.clone();
        invalid[field][0][key] = value;
        assert!(
            decode(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{field}:{key}"
        );
    }
    for field in ["audio_feeds", "midi_routes"] {
        let mut invalid = base.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        let mut invalid = base.clone();
        invalid[field] = Value::Null;
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        let limit = if field == "audio_feeds" {
            hex_project::MAX_AUDIO_FEEDS
        } else {
            hex_project::MAX_MIDI_ROUTES
        };
        let mut invalid = base.clone();
        invalid[field] = json!(vec![base[field][0].clone(); limit + 1]);
        assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let mut invalid = base.clone();
    invalid["audio_feeds"][0]["source"]["kind"] = json!("main");
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    let mut invalid = base.clone();
    invalid["audio_feeds"][1]["destination"] = base["tracks"][0]["id"].clone();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    let mut invalid = base.clone();
    invalid["midi_routes"][1]["destination"] = base["midi_routes"][0]["destination"].clone();
    assert!(decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    // Duplicate keys must remain rejected, including the fields added in24.
    let bytes = serde_json::to_string(&base).unwrap();
    let duplicate = bytes.replacen("\"audio_feeds\":", "\"audio_feeds\":[],\"audio_feeds\":", 1);
    assert!(decode(duplicate.as_bytes()).is_err());
}

#[test]
fn current_playable_route_inventories_are_required_even_when_empty() {
    let project = Project::new(48_000).unwrap();
    let base: Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    assert_eq!(
        decode(&serde_json::to_vec(&base).unwrap()).unwrap(),
        project
    );
    for key in ["audio_feeds", "midi_routes"] {
        let mut missing = base.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    }
}

fn content_fixture() -> Project {
    let mut editor = Editor::new(fixture(), 16).unwrap();
    let ids: Vec<_> = editor
        .project()
        .tracks()
        .iter()
        .map(|track| track.id())
        .collect();
    editor
        .execute(Command::ImportPreparedAudioClip {
            track: ids[0],
            start: 0,
            path: "original.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 48_000,
            playback_frames: 48_000,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap();
    editor
        .execute(Command::PlaceMidiClip {
            track: ids[1],
            start: hex_project::BeatTime::from_quarters(4).unwrap(),
            length: hex_project::BeatTime::ONE,
        })
        .unwrap();
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track: ids[0],
            points: vec![hex_project::GainPoint {
                beat: hex_project::BeatTime::ZERO,
                gain: 0.5,
                incoming_gain: None,
                outgoing: hex_project::GainInterpolation::Step,
            }],
        })
        .unwrap();
    let device = editor
        .project()
        .track(ids[1])
        .unwrap()
        .instrument()
        .unwrap()
        .clone();
    let other =
        hex_lattice_contract::NativeDeviceState::new(device.state().type_id(), 1, vec![1, 2, 3])
            .unwrap();
    editor
        .execute(Command::ReplaceDeviceComparison {
            device: device.id(),
            state: device.state().clone(),
            comparison: Some(hex_project::DeviceComparison::new(
                hex_project::DeviceVariant::B,
                other,
            )),
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let asset = editor.project().tracks()[0].clips()[0].asset();
    editor
        .execute(Command::CreateSessionAudioClip {
            track: ids[0],
            scene,
            name: "session audio".into(),
            asset,
            source_start: 4,
            source_end: 128,
        })
        .unwrap();
    editor
        .execute(Command::CreateSessionMidiClip {
            track: ids[1],
            scene,
            name: "session midi".into(),
            source_length: hex_project::BeatTime::ONE,
        })
        .unwrap();
    editor
        .execute(Command::SetSessionStopCell {
            track: ids[2],
            scene,
        })
        .unwrap();
    editor.project().clone()
}

#[test]
fn track_clipboard_roundtrips_full_and_setup_only_copies_with_routes_and_history() {
    let original = content_fixture();
    for without_clips in [false, true] {
        let mut editor = Editor::new(original.clone(), 16).unwrap();
        let ids: Vec<_> = original.tracks().iter().map(|t| t.id()).collect();
        let clipboard = std::sync::Arc::new(
            original
                .capture_tracks(
                    &hex_project::TrackSelectionIds::new(&ids).unwrap(),
                    without_clips,
                )
                .unwrap(),
        );
        editor
            .execute(Command::PasteTracks {
                clipboard,
                destination: hex_project::TrackDestination {
                    parent_group: None,
                    before: None,
                },
            })
            .unwrap();
        let saved = encode(editor.project()).unwrap();
        let opened = decode(&saved).unwrap();
        assert_eq!(opened.snapshot(), editor.project().snapshot());
        assert_eq!(&opened.tracks()[..3], original.tracks());
        assert_eq!(opened.tracks()[3].clips().is_empty(), without_clips);
        assert_eq!(opened.tracks()[4].midi_clips().is_empty(), without_clips);
        assert!(editor.undo());
        assert!(editor.redo());
        assert_eq!(encode(editor.project()).unwrap(), saved);
    }
}

#[test]
fn reordered_track_inventory_roundtrips_complete_content_automation_and_stable_routes() {
    let original = content_fixture();
    let ids: Vec<_> = original.tracks().iter().map(|t| t.id()).collect();
    for (command, order) in [
        (
            Command::MoveTrackBefore {
                track: ids[2],
                before: Some(ids[0]),
            },
            vec![ids[2], ids[0], ids[1]],
        ),
        (
            Command::MoveTrackBefore {
                track: ids[0],
                before: None,
            },
            vec![ids[1], ids[2], ids[0]],
        ),
        (
            Command::MoveTracksBefore {
                tracks: hex_project::TrackSelectionIds::new(&[ids[2], ids[0]]).unwrap(),
                before: None,
            },
            vec![ids[1], ids[0], ids[2]],
        ),
    ] {
        let mut editor = Editor::new(original.clone(), 16).unwrap();
        editor.execute(command).unwrap();
        let restored = decode(&encode(editor.project()).unwrap()).unwrap();
        assert_eq!(restored, *editor.project());
        assert_eq!(
            restored
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect::<Vec<_>>(),
            order
        );
        assert_eq!(restored.return_tracks(), original.return_tracks());
        assert_eq!(restored.audio_feeds(), original.audio_feeds());
        assert_eq!(restored.midi_routes(), original.midi_routes());
        assert_eq!(restored.snapshot().next_id, original.snapshot().next_id);
        assert!(editor.undo());
        assert_eq!(editor.project(), &original);
    }
}
