use crate::{decode, encode, load, load_with_token, save_new};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use hex_project::{Command, Created, Editor, Project, default_instrument_state};

#[test]
fn larger_unbound_vst3_instrument_round_trips_without_group_mutes() {
    let state = hex_lattice_contract::NativeDeviceState::new(
        hex_lattice_contract::VST3_DEVICE_TYPE_ID,
        1,
        vec![42; 9_357],
    )
    .unwrap();
    let authority = hex_lattice_contract::Vst3StateAuthority::new_on_control().unwrap();
    let verifier = authority.verifier();
    let state = authority.approve_on_control(state).unwrap();
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
    editor
        .execute(Command::CreateNativeSource {
            name: "unbound instrument".into(),
            devices: vec![state.clone()],
            instrument_index: 0,
        })
        .unwrap();
    let project = editor.project();
    let saved = encode(project).unwrap();
    assert_eq!(crate::schema_version_for(project), 75);
    let reopened = decode(&saved).unwrap();
    assert_eq!(reopened.snapshot(), project.snapshot());
    let reopened_state = reopened.native_sources()[0].instrument().state();
    assert!(verifier.approves(&state));
    assert!(!verifier.approves(reopened_state));
    assert_eq!(encode(&reopened).unwrap(), saved);
}

/// Historical schemas could describe only identity and sinc v1 conversion, so
/// a document derived from the current fixture records its v2 rows as v1.
fn historical_conversions(document: &mut serde_json::Value) {
    for asset in document["assets"].as_array_mut().unwrap() {
        if asset["conversion_version"] == hex_project::SINC_CONVERSION_V2 {
            asset["conversion_version"] = hex_project::SINC_CONVERSION_V1.into();
        }
    }
}

/// The same project as a historical file would have recorded it.
fn with_v1_conversions(project: &Project) -> Project {
    let mut snapshot = project.snapshot();
    for asset in &mut snapshot.assets {
        if asset.conversion_version == hex_project::SINC_CONVERSION_V2 {
            asset.conversion_version = hex_project::SINC_CONVERSION_V1;
        }
    }
    Project::from_snapshot(snapshot).unwrap()
}

fn fixture() -> Project {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "Original Sample.wav".into(),
            sample_rate: 44100,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let instrument = default_instrument_state()
        .unwrap()
        .with_input_gain_millidb(1500)
        .unwrap()
        .with_output_gain_millidb(-3000)
        .unwrap();
    let reference = DeviceAssetReference {
        slot: DeviceAssetSlot::new(1).unwrap(),
        asset: DeviceAssetKey::new(asset.value()).unwrap(),
    };
    let Created::NativeSource(source) = editor
        .execute(Command::CreateNativeSource {
            name: "Preserved Source".into(),
            devices: vec![instrument.clone(), instrument.clone()],
            instrument_index: 1,
        })
        .unwrap()
    else {
        panic!("source")
    };
    let device = editor
        .project()
        .native_source(source)
        .unwrap()
        .devices()
        .nth(1)
        .unwrap()
        .id();
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceComparison {
            device,
            state: instrument.clone(),
            comparison: Some(hex_project::DeviceComparison::new(
                hex_project::DeviceVariant::B,
                instrument.with_asset_references(vec![reference]).unwrap(),
            )),
        })
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "existing track".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: default_instrument_state().unwrap(),
        })
        .unwrap();
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::InsertDevice {
            track,
            before: Some(instrument),
            state: default_instrument_state().unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: default_instrument_state().unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::AddMidiTrack {
            name: "unbound track".into(),
        })
        .unwrap();
    editor.project().clone()
}
#[test]
fn current_source_roundtrip_preserves_complete_chain_host_metadata_comparison_and_resources() {
    let project = fixture();
    let bytes = encode(&project).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        crate::schema_version_for(&project)
    );
    assert_eq!(decode(&bytes).unwrap(), project);
    let mut schema46: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        schema46
            .as_object_mut()
            .unwrap()
            .remove("device_automation")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    for field in [
        "alias_sources",
        "alias_members",
        "host_automation",
        "inactive_midi_notes",
        "inactive_audio_events",
        "nested_audio",
        "audio_event_gains",
        "midi_clip_gains",
        "automatic_track_names",
    ] {
        assert!(
            schema46
                .as_object_mut()
                .unwrap()
                .remove(field)
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    schema46["version"] = 46.into();
    historical_conversions(&mut schema46);
    for source in schema46["native_sources"].as_array_mut().unwrap() {
        source.as_object_mut().unwrap().remove("receiver");
    }
    assert!(
        schema46
            .as_object_mut()
            .unwrap()
            .remove("session_audio_cycles")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        decode(&serde_json::to_vec(&schema46).unwrap()).unwrap(),
        with_v1_conversions(&project),
        "schema46 retains its in-memory migration path"
    );
    assert_eq!(project.tracks()[0].devices().len(), 2);
    assert_eq!(project.tracks()[0].instrument_position(), Some(1));
    assert!(
        crate::encode_schema22(&project).is_err(),
        "explicit older encoding must not discard sources"
    );
}

#[test]
fn schema44_promotes_legacy_instrument_and_preserves_original_bytes() {
    // Frozen output of the preserved schema44 encoder from baseline3a2ff464,
    // captured before source bindings. No historical source inventory is invented.
    let bytes =
        include_bytes!("../../../tests/fixtures/native-sources/schema44-predecessor.json").to_vec();
    let original = bytes.clone();
    let migrated = decode(&bytes).unwrap();
    assert_eq!(migrated.native_sources().len(), 1);
    let track = &migrated.tracks()[0];
    let source = migrated.native_source(track.source_id().unwrap()).unwrap();
    assert_eq!(source.instrument().id(), track.instrument().unwrap().id());
    assert_eq!(
        source.instrument().state(),
        track.instrument().unwrap().state()
    );
    assert_eq!(bytes.as_slice(), original.as_slice());
    assert_eq!(decode(&encode(&migrated).unwrap()).unwrap(), migrated);

    let mut schema45: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    schema45["version"] = 45.into();
    schema45["native_sources"] = serde_json::json!([]);
    let migrated45 = decode(&serde_json::to_vec(&schema45).unwrap()).unwrap();
    assert_eq!(migrated45.native_sources().len(), 1);
    assert!(migrated45.tracks()[0].source_id().is_some());

    let path = std::env::temp_dir().join(format!("hex-schema44-upgrade-{}", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    let (loaded, token) = load_with_token(&path).unwrap();
    assert_eq!(loaded, migrated);
    assert!(token.requires_format_upgrade());
    assert!(token.requires_save_confirmation(&loaded).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn authentic_schema45_migration_keeps_wire_sources_before_promoted_track_instruments() {
    // Captured by the schema45 encoder at 201bf8729 before bindings existed.
    // SHA-256: fe30616d9491f59588a2db33af3f569f20466476ad5f2c4bd53659943bbb8dda.
    // It includes an unassigned two-device retained source, an inline instrument
    // with an asset reference, and track-local effects on both sides.
    let bytes =
        include_bytes!("../../../tests/fixtures/native-sources/schema45-prebinding-201bf8729.json");
    let old: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(old["version"], 45);
    let migrated = decode(bytes).unwrap();
    assert_eq!(migrated.native_sources().len(), 2);
    assert_eq!(
        migrated.native_sources()[0].id().value(),
        old["native_sources"][0]["id"].as_u64().unwrap()
    );
    assert_eq!(migrated.native_sources()[0].name(), "Preserved Source");
    assert_eq!(migrated.native_sources()[0].devices().count(), 2);
    assert_eq!(
        migrated.native_sources()[0]
            .devices()
            .map(|device| device.id().value())
            .collect::<Vec<_>>(),
        vec![3, 4]
    );

    let track = &migrated.tracks()[0];
    let source_id = track.source_id().unwrap();
    let source = migrated.native_source(source_id).unwrap();
    assert_eq!(source_id.value(), old["next_id"].as_u64().unwrap());
    assert_eq!(
        source.instrument().id().value(),
        old["tracks"][0]["instrument"]["id"].as_u64().unwrap()
    );
    assert_eq!(
        source.instrument().state(),
        track.instrument().unwrap().state()
    );
    assert_eq!(track.instrument_position(), Some(1));
    assert_eq!(track.devices().len(), 2);
    assert_eq!(
        track
            .devices()
            .iter()
            .map(|device| device.id().value())
            .collect::<Vec<_>>(),
        vec![7, 8]
    );
    assert_eq!(migrated.snapshot().next_id, source_id.value() + 1);
    assert_eq!(decode(&encode(&migrated).unwrap()).unwrap(), migrated);

    let mut colliding_device: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    colliding_device["native_sources"][0]["devices"][0]["device"]["id"] =
        colliding_device["tracks"][0]["instrument"]["id"].clone();
    assert!(decode(&serde_json::to_vec(&colliding_device).unwrap()).is_err());

    let mut colliding_source: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    colliding_source["native_sources"][0]["id"] = colliding_source["next_id"].clone();
    assert!(decode(&serde_json::to_vec(&colliding_source).unwrap()).is_err());

    let mut over_old_limit: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    let retained_source = over_old_limit["native_sources"][0].clone();
    over_old_limit["native_sources"] = serde_json::Value::Array(vec![retained_source; 257]);
    assert!(decode(&serde_json::to_vec(&over_old_limit).unwrap()).is_err());
}
#[test]
fn native_source_envelope_refuses_missing_unknown_duplicate_and_invalid_fields() {
    let project = fixture();
    let value: serde_json::Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    let instrument = project.tracks()[0].instrument().unwrap().id().value();
    assert!(
        value["device_assets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["device"].as_u64() != Some(instrument))
    );
    assert!(
        value["native_sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|device| device["device"]["id"].as_u64() == Some(instrument)))
    );
    for case in 0..14 {
        let mut bad = value.clone();
        match case {
            0 => {
                bad.as_object_mut().unwrap().remove("native_sources");
            }
            1 => {
                bad["native_sources"] = serde_json::json!(null);
            }
            2 => {
                bad["native_sources"][0]["unknown"] = true.into();
            }
            3 => {
                bad["native_sources"][0]["instrument_index"] = 8.into();
            }
            4 => {
                bad["native_sources"][0]["devices"] = serde_json::json!([]);
            }
            5 => {
                bad["native_sources"][0]["devices"][0]["device"]["id"] =
                    bad["native_sources"][0]["id"].clone();
            }
            6 => {
                bad["native_sources"][0]["devices"][1]
                    .as_object_mut()
                    .unwrap()
                    .remove("comparison");
            }
            7 => {
                bad["native_sources"][0]["devices"][1]["comparison"]["references"][0]["asset"] =
                    99999.into();
            }
            8 => {
                bad["native_sources"][0]["id"] = bad["tracks"][0]["id"].clone();
            }
            9 => {
                bad["native_sources"][0]["name"] = "".into();
            }
            11 => {
                bad["native_sources"][0]["devices"][0]["device"]
                    .as_object_mut()
                    .unwrap()
                    .remove("input_gain_millidb");
            }
            12 => {
                bad["native_sources"][0]["devices"][0]["device"]
                    .as_object_mut()
                    .unwrap()
                    .remove("output_gain_millidb");
            }
            13 => {
                bad["native_sources"][0]["devices"][0]["device"]["payload"] =
                    serde_json::json!(vec![
                        0;
                        hex_lattice_contract::MAX_NATIVE_DEVICE_STATE_BYTES + 1
                    ]);
            }
            _ => {
                bad["native_sources"] =
                    serde_json::Value::Array(vec![
                        bad["native_sources"][0].clone();
                        hex_project::MAX_NATIVE_SOURCES + 1
                    ]);
            }
        }
        assert!(
            decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "case {case}"
        );
    }
    let bytes = String::from_utf8(encode(&project).unwrap()).unwrap();
    let duplicated = bytes.replacen(
        "\"native_sources\":",
        "\"native_sources\":[],\"native_sources\":",
        1,
    );
    assert!(decode(duplicated.as_bytes()).is_err());
}

#[test]
fn current_binding_inventory_is_mandatory_ordered_and_resolves_source_projection() {
    let project = fixture();
    let value: serde_json::Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    assert!(
        value["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|track| track["instrument"].is_null() && track["instrument_position"].is_null())
    );
    assert_eq!(
        value["track_source_bindings"].as_array().unwrap().len(),
        project.tracks().len()
    );
    assert_eq!(
        value["track_source_bindings"][1]["source"],
        serde_json::Value::Null
    );
    assert_eq!(
        value["track_source_bindings"][1]["instrument_position"],
        serde_json::Value::Null
    );
    assert_eq!(
        decode(&serde_json::to_vec(&value).unwrap()).unwrap(),
        project
    );

    let source_id = project.tracks()[0].source_id().unwrap().value();
    let source = value["native_sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["id"].as_u64() == Some(source_id))
        .unwrap();
    let instrument_id =
        source["devices"][source["instrument_index"].as_u64().unwrap() as usize]["device"]["id"]
            .as_u64()
            .unwrap();
    let mut forged_asset_row = value.clone();
    forged_asset_row["device_assets"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "device": instrument_id,
            "references": [],
            "comparison": null,
        }));
    assert!(decode(&serde_json::to_vec(&forged_asset_row).unwrap()).is_err());

    for case in 0..12 {
        let mut bad = value.clone();
        match case {
            0 => {
                bad.as_object_mut().unwrap().remove("track_source_bindings");
            }
            1 => {
                bad["track_source_bindings"].as_array_mut().unwrap().pop();
            }
            2 => {
                bad["track_source_bindings"][0]["track"] = 999_999.into();
            }
            3 => {
                bad["track_source_bindings"][0]["source"] = 999_999.into();
            }
            4 => {
                bad["track_source_bindings"][0]["instrument_position"] = serde_json::Value::Null;
            }
            5 => {
                let duplicate = bad["track_source_bindings"][0].clone();
                bad["track_source_bindings"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            6 => {
                bad["track_source_bindings"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("source");
            }
            7 => {
                bad["track_source_bindings"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("instrument_position");
            }
            8 => {
                let source_id = project.tracks()[0].source_id().unwrap().value();
                let source = value["native_sources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|source| source["id"].as_u64() == Some(source_id))
                    .unwrap();
                bad["tracks"][0]["instrument"] = source["devices"][0]["device"].clone();
            }
            9 => {
                let source_id = project.tracks()[0].source_id().unwrap().value();
                let source = value["native_sources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|source| source["id"].as_u64() == Some(source_id))
                    .unwrap();
                let device = source["devices"][0]["device"]["id"].clone();
                bad["device_assets"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({
                        "device": device,
                        "references": [],
                        "comparison": null
                    }));
            }
            10 => {
                bad["track_source_bindings"][1]["source"] =
                    bad["track_source_bindings"][0]["source"].clone();
                bad["track_source_bindings"][1]["instrument_position"] = 0.into();
                let source_id = bad["track_source_bindings"][0]["source"].as_u64().unwrap();
                let receiver = bad["track_source_bindings"][1]["track"].clone();
                let source = bad["native_sources"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|source| source["id"].as_u64() == Some(source_id))
                    .unwrap();
                source["receiver"] = receiver;
                assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_ok());
                continue;
            }
            _ => {
                bad["track_source_bindings"][1]["source"] =
                    value["native_sources"][0]["id"].clone();
                bad["track_source_bindings"][1]["instrument_position"] = 0.into();
            }
        }
        assert!(
            decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "case {case}"
        );
    }
}

#[test]
fn schema49_shared_source_roundtrips_and_schema46_migration_is_single_binding_only() {
    let project = fixture();
    let mut snapshot = project.snapshot();
    let source_id = snapshot.tracks[0].instrument.as_ref().unwrap().id;
    snapshot.tracks[1].instrument = snapshot.tracks[0].instrument.clone();
    snapshot.tracks[1].instrument_position = Some(0);
    let shared = Project::from_snapshot(snapshot).unwrap();
    assert_eq!(
        shared
            .source_contributors(shared.tracks()[0].source_id().unwrap())
            .count(),
        2
    );
    assert_eq!(
        decode(&encode(&shared).unwrap()).unwrap(),
        shared,
        "schema49 preserves one source shared by two contributors"
    );
    let mut no_receiver_snapshot = shared.snapshot();
    let shared_source = shared.tracks()[0].source_id().unwrap().value();
    no_receiver_snapshot
        .native_sources
        .iter_mut()
        .find(|source| source.id == shared_source)
        .unwrap()
        .receiver = None;
    let no_receiver = Project::from_snapshot(no_receiver_snapshot).unwrap();
    assert_eq!(decode(&encode(&no_receiver).unwrap()).unwrap(), no_receiver);

    let mut old46: serde_json::Value = serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    assert!(
        old46
            .as_object_mut()
            .unwrap()
            .remove("device_automation")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    for field in [
        "alias_sources",
        "alias_members",
        "host_automation",
        "inactive_midi_notes",
        "inactive_audio_events",
        "nested_audio",
        "audio_event_gains",
        "midi_clip_gains",
        "automatic_track_names",
    ] {
        assert!(
            old46
                .as_object_mut()
                .unwrap()
                .remove(field)
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    old46["version"] = 46.into();
    historical_conversions(&mut old46);
    old46
        .as_object_mut()
        .unwrap()
        .remove("session_audio_cycles");
    for source in old46["native_sources"].as_array_mut().unwrap() {
        source.as_object_mut().unwrap().remove("receiver");
    }
    let migrated = decode(&serde_json::to_vec(&old46).unwrap()).unwrap();
    assert_eq!(
        migrated.native_sources()[1].instrument().id().value(),
        source_id
    );
    assert_eq!(migrated.native_sources()[0].receiver(), None);
    assert_eq!(
        migrated.native_sources()[1].receiver(),
        Some(migrated.tracks()[0].id())
    );

    let mut malformed46 = old46;
    malformed46["track_source_bindings"][1]["source"] =
        malformed46["track_source_bindings"][0]["source"].clone();
    malformed46["track_source_bindings"][1]["instrument_position"] =
        malformed46["track_source_bindings"][0]["instrument_position"].clone();
    assert!(decode(&serde_json::to_vec(&malformed46).unwrap()).is_err());

    let mut missing_receiver: serde_json::Value =
        serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    missing_receiver["native_sources"][0]
        .as_object_mut()
        .unwrap()
        .remove("receiver");
    assert!(decode(&serde_json::to_vec(&missing_receiver).unwrap()).is_err());

    let mut missing_contributor: serde_json::Value =
        serde_json::from_slice(&encode(&project).unwrap()).unwrap();
    missing_contributor["native_sources"][1]["receiver"] = 999_999.into();
    assert!(decode(&serde_json::to_vec(&missing_contributor).unwrap()).is_err());
}

#[test]
fn authentic_schema46_receiverless_migration_preserves_sources_tracks_assets_and_routes() {
    // Captured by the unmodified schema46 codec at 68b27451d.
    // SHA-256: 1c99667964bbf040c14ac9611c4b7a5d8bd016aee89efef1c06127e7e22eae2f.
    // Contains retained and bound source definitions, source media references,
    // track-local effects on both sides of the instrument, and a MIDI route.
    let bytes = include_bytes!(
        "../../../tests/fixtures/native-sources/schema46-source-receiver-predecessor-68b27451d.json"
    );
    let original = bytes.to_vec();
    let wire: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(wire["version"], 46);
    assert_eq!(wire["native_sources"].as_array().unwrap().len(), 2);
    assert_eq!(wire["midi_routes"].as_array().unwrap().len(), 1);
    let migrated = decode(bytes).unwrap();
    assert_eq!(migrated.native_sources().len(), 2);
    assert_eq!(migrated.native_sources()[0].receiver(), None);
    assert_eq!(
        migrated.native_sources()[1].receiver(),
        Some(migrated.tracks()[0].id())
    );
    assert_eq!(migrated.tracks()[0].instrument_position(), Some(1));
    assert_eq!(migrated.tracks()[0].devices().len(), 2);
    assert_eq!(migrated.midi_routes().len(), 1);
    assert_eq!(bytes.as_slice(), original.as_slice());
    assert_eq!(decode(&encode(&migrated).unwrap()).unwrap(), migrated);
}

#[test]
fn current_bound_sources_roundtrip_at_exhausted_identity_highwater() {
    let mut snapshot = fixture().snapshot();
    snapshot.next_id = u64::MAX;
    let project = Project::from_snapshot(snapshot).unwrap();
    let encoded = encode(&project).unwrap();
    let reopened = decode(&encoded).unwrap();
    assert_eq!(reopened, project);
    assert_eq!(reopened.snapshot().next_id, u64::MAX);
}

#[test]
fn current_midi_routes_validate_after_bound_instruments_are_hydrated() {
    let mut snapshot = fixture().snapshot();
    let route_id = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.midi_routes.push(hex_project::MidiRouteSnapshot {
        id: route_id,
        source: snapshot.tracks[1].id,
        destination: snapshot.tracks[0].id,
    });
    snapshot.next_id = u64::MAX;
    let project = Project::from_snapshot(snapshot).unwrap();
    let encoded = encode(&project).unwrap();
    assert_eq!(decode(&encoded).unwrap(), project);

    let mut unbound_destination: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    unbound_destination["track_source_bindings"][0]["source"] = serde_json::Value::Null;
    unbound_destination["track_source_bindings"][0]["instrument_position"] =
        serde_json::Value::Null;
    assert!(decode(&serde_json::to_vec(&unbound_destination).unwrap()).is_err());
}
#[test]
fn saved_unassigned_source_reopens_without_history_and_failed_publication_preserves_original() {
    let project = fixture();
    let dir = std::env::temp_dir().join(format!("hex-native-sources-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("retained.hex");
    save_new(&path, &project).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let (loaded, token) = load_with_token(&path).unwrap();
    assert_eq!(loaded, project);
    assert!(!token.requires_format_upgrade());
    let mut editor = Editor::new(loaded, 8).unwrap();
    assert!(!editor.can_undo(), "retention must not rely on history");
    let source = editor.project().native_sources()[0].id();
    editor
        .execute(Command::RenameNativeSource {
            source,
            name: "Changed".into(),
        })
        .unwrap();
    assert!(save_new(&path, editor.project()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let mut foreign = bytes.clone();
    foreign.push(b' ');
    std::fs::write(&path, &foreign).unwrap();
    assert!(crate::replace_existing(&path, &token, editor.project()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), foreign);
    assert_eq!(load(&path).unwrap(), project);
    assert!(editor.undo());
    assert_eq!(editor.project().native_sources(), project.native_sources());
    assert!(editor.redo());
    let first = editor.project().tracks()[0].id();
    let second = editor.project().tracks()[1].id();
    let shared = editor.project().track(first).unwrap().source_id().unwrap();
    editor
        .execute(Command::BindTrackSource {
            track: second,
            source: Some(shared),
        })
        .unwrap();
    editor
        .execute(Command::SetNativeSourceReceiver {
            source: shared,
            track: Some(second),
        })
        .unwrap();
    let after = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(
        editor.project().native_source(shared).unwrap().receiver(),
        Some(first)
    );
    assert_eq!(editor.project().source_contributors(shared).count(), 2);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
    let copy = dir.join("edited.hex");
    save_new(&copy, editor.project()).unwrap();
    assert_eq!(&load(&copy).unwrap(), editor.project());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn authentic_schema47_audio_and_source_migration_preserves_complete_state_and_bytes() {
    // Captured by the unmodified schema47 writer at main 1b6095688b40722e20554e463c466026caa3c057.
    // SHA-256: ce5b8fbe5670ad30669370bf9273d47a7456c3e7342889c87c5010a35b3c162e.
    // Includes two retained source definitions, one singleton track binding with local effects,
    // an unbound track, an audio clip with pitch and musical timing, and its media manifests.
    let bytes = include_bytes!(
        "../../../tests/fixtures/native-sources/schema47-audio-and-sources-1b6095688b4.json"
    );
    let original = bytes.to_vec();
    let wire: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(wire["version"], 47);
    assert!(wire["native_sources"][0].get("receiver").is_none());
    assert!(wire["native_sources"][1].get("receiver").is_none());

    let migrated = decode(bytes).unwrap();
    assert_eq!(migrated.native_sources().len(), 2);
    assert_eq!(
        migrated
            .native_sources()
            .iter()
            .map(|source| source.id().value())
            .collect::<Vec<_>>(),
        vec![2, 7]
    );
    assert_eq!(migrated.native_sources()[0].receiver(), None);
    assert_eq!(
        migrated.native_sources()[0]
            .devices()
            .map(|device| device.id().value())
            .collect::<Vec<_>>(),
        vec![3, 4]
    );
    let bound = &migrated.native_sources()[1];
    assert_eq!(
        bound
            .devices()
            .map(|device| device.id().value())
            .collect::<Vec<_>>(),
        vec![6]
    );
    assert_eq!(bound.receiver().map(|track| track.value()), Some(5));
    assert_eq!(migrated.tracks()[0].source_id(), Some(bound.id()));
    assert_eq!(migrated.tracks()[0].instrument_position(), Some(1));
    assert_eq!(migrated.tracks()[0].devices().len(), 2);
    assert!(
        migrated
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .any(|clip| {
                clip.audio_edit()
                    .is_some_and(|edit| edit.domain.pitch_cents == 137)
            })
    );
    let mut shared_legacy: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    shared_legacy["track_source_bindings"][1]["source"] = 7.into();
    shared_legacy["track_source_bindings"][1]["instrument_position"] = 0.into();
    assert!(
        decode(&serde_json::to_vec(&shared_legacy).unwrap()).is_err(),
        "historical schema47 remains one-binding-per-source"
    );
    let mut receiver_forgery: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    receiver_forgery["native_sources"][0]["receiver"] = serde_json::Value::Null;
    assert!(
        decode(&serde_json::to_vec(&receiver_forgery).unwrap()).is_err(),
        "schema47's frozen source DTO has no receiver field"
    );
    let current: serde_json::Value = serde_json::from_slice(&encode(&migrated).unwrap()).unwrap();
    assert_eq!(current["version"], crate::CURRENT_SCHEMA_VERSION);
    assert_eq!(current["audio_clip_state"], wire["audio_clip_state"]);
    assert_eq!(decode(&encode(&migrated).unwrap()).unwrap(), migrated);
    assert_eq!(bytes, original.as_slice());

    let path = std::env::temp_dir().join(format!("hex-schema47-upgrade-{}", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let (loaded, token) = load_with_token(&path).unwrap();
    assert_eq!(loaded, migrated);
    assert!(token.requires_format_upgrade());
    assert!(token.requires_save_confirmation(&loaded).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::remove_file(path).unwrap();
}
