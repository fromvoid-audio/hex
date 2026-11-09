use super::*;
use hex_project::{Command, Created, Editor};

fn legacy_fixture() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    for _ in 0..2 {
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
    }
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.main_devices[1].parent = Some(snapshot.main_devices[0].id);
    snapshot.main_devices[1].bypassed = true;
    Project::from_snapshot(snapshot).unwrap()
}
fn bank(device_type: DeviceTypeId, first: u64) -> DeviceModulation {
    let module_type = DeviceTypeId::from_static("test.original.modulator");
    let sources = (first..first + 2)
        .map(|id| Modulator {
            color: Default::default(),
            id: ModulationSourceId::new(id).unwrap(),
            enabled: id == first,
            state: NativeDeviceState::new(module_type, 7, vec![0, 127, 255])
                .unwrap()
                .with_output_gain_millidb(-1350)
                .unwrap(),
        })
        .collect();
    let route = |id, source, destination| DeviceModulationRoute {
        id: ModulationRouteId::new(id).unwrap(),
        source: ModulationSourceId::new(source).unwrap(),
        output: ModulationOutputId::MAIN,
        destination,
        amount: ModulationAmount::new(-0.4).unwrap(),
        range: ModulationRange {
            start: ModulationAmount::new(0.0).unwrap(),
            end: ModulationAmount::new(0.8).unwrap(),
        },
        enabled: true,
    };
    DeviceModulation::new(
        sources,
        vec![
            route(
                first + 2,
                first,
                ModulationDestination::Modulator {
                    source: ModulationSourceId::new(first + 1).unwrap(),
                    module_type,
                    parameter: ParameterId::from_static(5),
                },
            ),
            route(
                first + 3,
                first + 1,
                ModulationDestination::Device {
                    device_type,
                    target: DeviceAutomationTarget::HostOutputGain,
                    vst3_class: None,
                },
            ),
        ],
    )
    .unwrap()
}
fn authored() -> Project {
    let mut editor = Editor::new(legacy_fixture(), 8).unwrap();
    let devices = editor
        .project()
        .devices()
        .map(|device| (device.id(), device.state().type_id()))
        .collect::<Vec<_>>();
    for (device, device_type) in devices {
        editor
            .execute(Command::ReplaceDeviceModulation {
                device,
                modulation: bank(device_type, editor.project().snapshot().next_id),
            })
            .unwrap();
    }
    editor.project().clone()
}
fn temp_dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "hex-modulation-schema-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    path
}
#[test]
fn complete_banks_round_trip_and_save_reopen_with_serial_and_retained_source_owners() {
    let project = authored();
    let bytes = crate::encode(&project).unwrap();
    assert_eq!(crate::schema_version_for(&project), 81);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["modulation_base_version"],
        80
    );
    assert_eq!(
        crate::decode(&bytes).unwrap().snapshot(),
        project.snapshot()
    );
    assert!(crate::encode_schema20(&project).is_err());
    assert!(crate::encode_schema22(&project).is_err());
    let dir = temp_dir();
    let path = dir.join("modulated.hex");
    crate::save_new(&path, &project).unwrap();
    assert_eq!(crate::load(&path).unwrap().snapshot(), project.snapshot());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn schema81_keeps_gray_slots_and_schema82_round_trips_complete_colored_slots() {
    let gray = authored();
    let bytes = crate::encode(&gray).unwrap();
    let old: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(old["version"], 81);
    assert!(
        old["device_modulation"][0]["sources"][0]
            .get("color")
            .is_none()
    );

    let dir = temp_dir();
    let path = dir.join("schema81.hex");
    std::fs::write(&path, &bytes).unwrap();
    let (loaded, old_token) = crate::load_with_token(&path).unwrap();
    assert_eq!(loaded.snapshot(), gray.snapshot());
    assert!(!old_token.requires_save_confirmation(&loaded).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);

    let device = loaded.devices().next().unwrap();
    let bank = device.modulation();
    let mut slots = bank.sources().to_vec();
    slots[0].color = ElementColor::Cyan;
    slots.reverse();
    let replacement = DeviceModulation::new(slots, bank.routes().to_vec()).unwrap();
    let edit = Command::ReplaceDeviceModulation {
        device: device.id(),
        modulation: replacement,
    };
    let mut editor = Editor::new(loaded, 8).unwrap();
    editor.execute(edit).unwrap();
    let colored = editor.project();
    assert_eq!(crate::schema_version_for(colored), 82);
    assert!(old_token.requires_save_confirmation(colored).unwrap());
    let path = dir.join("schema82.hex");
    crate::save_new(&path, colored).unwrap();
    assert_eq!(crate::load(&path).unwrap().snapshot(), colored.snapshot());
    let current: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        current["device_modulation"][0]["sources"][0]["color"],
        "gray"
    );
    assert_eq!(
        current["device_modulation"][0]["sources"][1]["color"],
        "cyan"
    );
    assert_eq!(
        current["device_modulation"][1]["sources"][0]["color"],
        "gray"
    );

    let mut unexpected = old;
    unexpected["device_modulation"][0]["sources"][0]["color"] = "cyan".into();
    assert!(crate::decode(&serde_json::to_vec(&unexpected).unwrap()).is_err());

    let mut missing = current.clone();
    missing["device_modulation"][0]["sources"][0]
        .as_object_mut()
        .unwrap()
        .remove("color");
    assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    for invalid in [serde_json::Value::Null, "unknown".into(), 1.into()] {
        let mut malformed = current.clone();
        malformed["device_modulation"][0]["sources"][0]["color"] = invalid;
        assert!(crate::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn schema80_fixture_migrates_in_memory_without_overwriting_its_file() {
    let legacy = legacy_fixture();
    // The unchanged prior-schema owner produces this historical-layout fixture.
    let bytes =
        crate::encode_document(&super::super::serial_units::encode(&legacy).unwrap()).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        80
    );
    let dir = temp_dir();
    let path = dir.join("schema80.hex");
    std::fs::write(&path, &bytes).unwrap();
    let loaded = crate::load(&path).unwrap();
    assert!(
        loaded
            .devices()
            .all(|device| device.modulation().is_empty())
    );
    assert_eq!(loaded.snapshot(), legacy.snapshot());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn malformed_future_incomplete_and_cyclic_documents_refuse_instead_of_losing_routes() {
    let project = authored();
    let original: serde_json::Value =
        serde_json::from_slice(&crate::encode(&project).unwrap()).unwrap();
    let mut variants = Vec::new();
    let mut bad = original.clone();
    bad["version"] = (crate::NEWEST_SCHEMA_VERSION + 1).into();
    variants.push(bad);
    let mut bad = original.clone();
    bad["modulation_base_version"] = 81.into();
    variants.push(bad);
    let mut bad = original.clone();
    bad.as_object_mut().unwrap().remove("device_modulation");
    variants.push(bad);
    let mut bad = original.clone();
    bad["device_modulation"].as_array_mut().unwrap().pop();
    variants.push(bad);
    let mut bad = original.clone();
    bad["device_modulation"][0]["sources"][0]["id"] = 0.into();
    variants.push(bad);
    let mut bad = original.clone();
    bad["device_modulation"][0]["routes"][0]["output"] = 0.into();
    variants.push(bad);
    let mut bad = original.clone();
    bad["device_modulation"][0]["routes"][0]["amount"] = 1.01.into();
    variants.push(bad);
    let mut bad = original.clone();
    bad["device_modulation"][0]["routes"][0]["source"] = 999.into();
    variants.push(bad);
    let mut bad = original.clone();
    bad["device_modulation"][0]["routes"][0]["destination"]["source"] =
        original["device_modulation"][0]["sources"][0]["id"].clone();
    variants.push(bad);
    let mut bad = original.clone();
    bad["device_modulation"][0]["routes"][1]["destination"]
        .as_object_mut()
        .unwrap()
        .remove("vst3_class");
    variants.push(bad);
    let mut duplicate = serde_json::to_string(&original).unwrap();
    let index = duplicate.find("\"amount\":").unwrap();
    duplicate.insert_str(index, "\"amount\":0,");
    assert!(crate::decode(duplicate.as_bytes()).is_err());
    for bad in variants {
        assert!(crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut missing = original.clone();
    missing["device_modulation"][0]["sources"][0]
        .as_object_mut()
        .unwrap()
        .insert("unknown".into(), true.into());
    assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    assert_eq!(
        crate::decode(&serde_json::to_vec(&original).unwrap())
            .unwrap()
            .snapshot(),
        project.snapshot()
    );
}

#[test]
fn bound_source_effect_schema_preserves_colored_slots_and_requires_upgrade_consent() {
    let original = authored();
    let older = crate::FileToken::from_validated_bytes(crate::encode(&original).unwrap().into());
    let mut editor = Editor::new(original, 8).unwrap();
    let main = &editor.project().main_devices()[0];
    let id = main.id();
    let mut slots = main.modulation().sources().to_vec();
    slots[0].color = ElementColor::Cyan;
    let modulation = DeviceModulation::new(slots, main.modulation().routes().to_vec()).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation,
        })
        .unwrap();

    let parent = editor.project().tracks()[0].instrument().unwrap().id();
    editor
        .execute(Command::InsertNestedDevice {
            parent,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let project = editor.project();
    assert!(older.requires_save_confirmation(project).unwrap());
    let saved = crate::encode(project).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(json["version"], 83);
    assert_eq!(json["source_effect_base_version"], 82);
    assert_eq!(
        crate::decode(&saved).unwrap().snapshot(),
        project.snapshot()
    );
    let mut missing = json.clone();
    missing
        .as_object_mut()
        .unwrap()
        .remove("source_effect_base_version");
    assert!(crate::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    let mut recursive = json;
    recursive["source_effect_base_version"] = 83.into();
    assert!(crate::decode(&serde_json::to_vec(&recursive).unwrap()).is_err());
}

#[test]
fn shaped_lfo_uses_its_own_schema_and_retains_bound_units_and_slot_metadata() {
    let mut editor = Editor::new(authored(), 8).unwrap();
    let parent = editor.project().tracks()[0].instrument().unwrap().id();
    editor
        .execute(Command::InsertNestedDevice {
            parent,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let device = &editor.project().main_devices()[0];
    let id = device.id();
    let mut slots = device.modulation().sources().to_vec();
    let routes = device.modulation().routes().to_vec();
    let mut payload = vec![0];
    payload.extend(1.0_f64.to_le_bytes());
    payload.extend(0.0_f64.to_le_bytes());
    let lfo = DeviceTypeId::from_static("hex.modulator.lfo");
    slots.push(Modulator {
        id: ModulationSourceId::new(editor.project().next_identity()).unwrap(),
        enabled: true,
        color: ElementColor::Cyan,
        state: NativeDeviceState::new(lfo, 1, payload.clone()).unwrap(),
    });
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: DeviceModulation::new(slots.clone(), routes.clone()).unwrap(),
        })
        .unwrap();

    let dir = temp_dir();
    let legacy = dir.join("sine.hex");
    crate::save_new(&legacy, editor.project()).unwrap();
    let original = std::fs::read(&legacy).unwrap();
    let (loaded, token) = crate::load_with_token(&legacy).unwrap();
    assert_eq!(loaded, *editor.project());
    assert_eq!(
        crate::schema_version_for(&loaded),
        crate::BOUND_SOURCE_EFFECT_SCHEMA_VERSION
    );
    assert!(!token.requires_save_confirmation(&loaded).unwrap());

    payload.push(3);
    slots.last_mut().unwrap().state = NativeDeviceState::new(lfo, 2, payload).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: DeviceModulation::new(slots, routes).unwrap(),
        })
        .unwrap();
    let shaped = editor.project();
    assert_eq!(
        crate::schema_version_for(shaped),
        crate::LFO_SHAPE_SCHEMA_VERSION
    );
    assert!(token.requires_save_confirmation(shaped).unwrap());
    let upgraded = dir.join("square.hex");
    crate::save_new(&upgraded, shaped).unwrap();
    assert_eq!(crate::load(&upgraded).unwrap(), *shaped);
    assert_eq!(std::fs::read(&legacy).unwrap(), original);

    let mut wrong_schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&upgraded).unwrap()).unwrap();
    wrong_schema["version"] = crate::MODULATION_SLOT_COLOR_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong_schema).unwrap()).is_err());

    let mut missing_shape: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&upgraded).unwrap()).unwrap();
    let slot = missing_shape["device_modulation"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["device"] == id.value())
        .unwrap()["sources"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap();
    slot["state_version"] = 1.into();
    slot["payload"].as_array_mut().unwrap().pop();
    assert!(crate::decode(&serde_json::to_vec(&missing_shape).unwrap()).is_err());

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn lfo_rate_route_upgrades_the_exact_previous_format_and_preserves_the_complete_bank() {
    let mut editor = Editor::new(authored(), 8).unwrap();
    let parent = editor.project().tracks()[0].instrument().unwrap().id();
    editor
        .execute(Command::InsertNestedDevice {
            parent,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let device = &editor.project().main_devices()[0];
    let id = device.id();
    let mut slots = device.modulation().sources().to_vec();
    let mut routes = device.modulation().routes().to_vec();
    let lfo = ModulationSourceId::new(editor.project().next_identity()).unwrap();
    let mut payload = vec![0];
    payload.extend(1.0_f64.to_le_bytes());
    payload.extend(0.0_f64.to_le_bytes());
    payload.push(2);
    slots.push(Modulator {
        id: lfo,
        color: ElementColor::Cyan,
        enabled: true,
        state: NativeDeviceState::new(LFO, 2, payload).unwrap(),
    });
    let set = |editor: &mut Editor, routes: Vec<DeviceModulationRoute>| {
        editor
            .execute(Command::ReplaceDeviceModulation {
                device: id,
                modulation: DeviceModulation::new(slots.clone(), routes).unwrap(),
            })
            .unwrap();
    };
    set(&mut editor, routes.clone());
    assert_eq!(
        crate::schema_version_for(editor.project()),
        crate::LFO_SHAPE_SCHEMA_VERSION
    );
    let dir = temp_dir();
    let old = dir.join("shaped.hex");
    crate::save_new(&old, editor.project()).unwrap();
    let original = std::fs::read(&old).unwrap();
    let (reopened, token) = crate::load_with_token(&old).unwrap();
    assert_eq!(reopened, *editor.project());
    assert!(!token.requires_save_confirmation(&reopened).unwrap());

    routes.push(DeviceModulationRoute {
        id: ModulationRouteId::new(editor.project().next_identity()).unwrap(),
        source: slots[0].id,
        output: ModulationOutputId::MAIN,
        destination: ModulationDestination::Modulator {
            source: lfo,
            module_type: LFO,
            parameter: ParameterId::from_static(1),
        },
        amount: ModulationAmount::new(-0.45).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    });
    set(&mut editor, routes);
    let project = editor.project();
    assert_eq!(
        crate::schema_version_for(project),
        crate::LFO_RATE_ROUTE_SCHEMA_VERSION
    );
    assert!(token.requires_save_confirmation(project).unwrap());
    let new = dir.join("rate-route.hex");
    crate::save_new(&new, project).unwrap();
    assert_eq!(crate::load(&new).unwrap(), *project);
    assert_eq!(std::fs::read(&old).unwrap(), original);

    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&new).unwrap()).unwrap();
    document["version"] = crate::LFO_SHAPE_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&document).unwrap()).is_err());

    document["version"] = crate::LFO_RATE_ROUTE_SCHEMA_VERSION.into();
    let rate = document["device_modulation"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["device"] == id.value())
        .unwrap()["routes"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap();
    rate["destination"]["parameter"] = 2.into();
    assert!(crate::decode(&serde_json::to_vec(&document).unwrap()).is_err());
    let (free_project, free_token) = crate::load_with_token(&new).unwrap();
    assert_eq!(free_project, *editor.project());
    assert!(
        !free_token
            .requires_save_confirmation(&free_project)
            .unwrap()
    );
    let free_bytes = std::fs::read(&new).unwrap();
    let mut wrongly_new: serde_json::Value = serde_json::from_slice(&free_bytes).unwrap();
    wrongly_new["version"] = crate::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrongly_new).unwrap()).is_err());

    let free_bank = free_project.device(id).unwrap().modulation();
    let mut synced_slots = free_bank.sources().to_vec();
    let target = synced_slots
        .iter_mut()
        .find(|source| source.id == lfo)
        .unwrap();
    let mut payload = target.state.payload().to_vec();
    payload[0] = 1;
    target.state = NativeDeviceState::new(LFO, 2, payload).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: DeviceModulation::new(synced_slots, free_bank.routes().to_vec()).unwrap(),
        })
        .unwrap();
    let synced = editor.project();
    assert_eq!(
        crate::schema_version_for(synced),
        crate::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION
    );
    assert!(free_token.requires_save_confirmation(synced).unwrap());
    let synced_path = dir.join("synced-rate-route.hex");
    crate::save_new(&synced_path, synced).unwrap();
    assert_eq!(crate::load(&synced_path).unwrap(), *synced);
    assert_eq!(std::fs::read(&new).unwrap(), free_bytes);

    let mut wrong_version: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&synced_path).unwrap()).unwrap();
    wrong_version["version"] = crate::LFO_RATE_ROUTE_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong_version).unwrap()).is_err());
    let legacy_bytes = std::fs::read(&synced_path).unwrap();
    let (previous, previous_token) = crate::load_with_token(&synced_path).unwrap();
    assert!(
        !previous_token
            .requires_save_confirmation(&previous)
            .unwrap()
    );
    let mut wrong_version: serde_json::Value = serde_json::from_slice(&legacy_bytes).unwrap();
    wrong_version["version"] = crate::AUDIO_RATE_MODULATOR_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong_version).unwrap()).is_err());

    let next = previous.next_identity();
    let audio_rate = ModulationSourceId::new(next).unwrap();
    let bank = previous.device(id).unwrap().modulation();
    let mut slots = bank.sources().to_vec();
    let mut routes = bank.routes().to_vec();
    slots.push(Modulator {
        id: audio_rate,
        color: ElementColor::Cyan,
        enabled: false,
        state: NativeDeviceState::new(AUDIO_RATE, 1, vec![]).unwrap(),
    });
    routes.push(DeviceModulationRoute {
        id: ModulationRouteId::new(next + 1).unwrap(),
        source: audio_rate,
        output: ModulationOutputId::MAIN,
        destination: ModulationDestination::Modulator {
            source: lfo,
            module_type: LFO,
            parameter: ParameterId::from_static(1),
        },
        amount: ModulationAmount::new(-0.25).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    });
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: DeviceModulation::new(slots, routes).unwrap(),
        })
        .unwrap();
    let upgraded = editor.project();
    assert_eq!(
        crate::schema_version_for(upgraded),
        crate::AUDIO_RATE_MODULATOR_SCHEMA_VERSION
    );
    assert!(previous_token.requires_save_confirmation(upgraded).unwrap());
    let audio_path = dir.join("audio-rate.hex");
    crate::save_new(&audio_path, upgraded).unwrap();
    assert_eq!(crate::load(&audio_path).unwrap(), *upgraded);
    assert_eq!(std::fs::read(&synced_path).unwrap(), legacy_bytes);

    let mut wrong_version: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&audio_path).unwrap()).unwrap();
    wrong_version["version"] = crate::TEMPO_LFO_RATE_ROUTE_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong_version).unwrap()).is_err());
    let prior_audio_bytes = std::fs::read(&audio_path).unwrap();
    let (prior_audio, prior_token) = crate::load_with_token(&audio_path).unwrap();
    let instrument = prior_audio.tracks()[0].instrument().unwrap();
    let instrument_id = instrument.id();
    let instrument_type = DeviceTypeId::from_static("hex.instrument.fm4");
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument_id,
            state: NativeDeviceState::new(instrument_type, 1, vec![1]).unwrap(),
        })
        .unwrap();
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let state = prior_audio.device(id).unwrap().modulation().sources()[0]
        .state
        .clone();
    let bank = DeviceModulation::new(
        vec![Modulator {
            id: source,
            color: ElementColor::Cyan,
            enabled: true,
            state,
        }],
        vec![DeviceModulationRoute {
            id: ModulationRouteId::new(next + 1).unwrap(),
            source,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Device {
                device_type: instrument_type,
                target: DeviceAutomationTarget::Parameter(ParameterId::from_static(29)),
                vst3_class: None,
            },
            amount: ModulationAmount::new(0.2).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        }],
    )
    .unwrap();
    let mut wrongly_new: serde_json::Value = serde_json::from_slice(&prior_audio_bytes).unwrap();
    wrongly_new["version"] = crate::INSTRUMENT_MODULATION_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrongly_new).unwrap()).is_err());
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: instrument_id,
            modulation: bank,
        })
        .unwrap();
    let instrument_project = editor.project();
    assert_eq!(
        crate::schema_version_for(instrument_project),
        crate::INSTRUMENT_MODULATION_SCHEMA_VERSION
    );
    assert!(
        prior_token
            .requires_save_confirmation(instrument_project)
            .unwrap()
    );
    let instrument_path = dir.join("instrument-modulation.hex");
    crate::save_new(&instrument_path, instrument_project).unwrap();
    assert_eq!(crate::load(&instrument_path).unwrap(), *instrument_project);
    assert_eq!(std::fs::read(&audio_path).unwrap(), prior_audio_bytes);
    let mut old_label: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&instrument_path).unwrap()).unwrap();
    // The previous codec admitted this opaque shape. Load it unchanged
    // and require consent before saving it with the new playback floor.
    old_label["version"] = crate::AUDIO_RATE_MODULATOR_SCHEMA_VERSION.into();
    let legacy_instrument = dir.join("opaque-instrument-bank.hex");
    let bytes = serde_json::to_vec(&old_label).unwrap();
    std::fs::write(&legacy_instrument, &bytes).unwrap();
    let (migrated, token) = crate::load_with_token(&legacy_instrument).unwrap();
    assert_eq!(migrated, *instrument_project);
    assert!(token.requires_save_confirmation(&migrated).unwrap());
    assert_eq!(std::fs::read(&legacy_instrument).unwrap(), bytes);
    use hex_project::BeatTime;
    editor
        .execute(Command::SpliceDeviceAutomationTouch {
            device: instrument_id,
            target: DeviceAutomationTarget::Parameter(ParameterId::from_static(29)),
            vst3_class: None,
            spans: vec![hex_project::DeviceAutomationTouchSpan {
                start: BeatTime::ZERO,
                end: BeatTime::ONE,
                steps: vec![hex_project::DeviceAutomationTouchStep {
                    beat: BeatTime::ZERO,
                    value: hex_project::AutomationValue::new(0.75).unwrap(),
                }],
            }],
        })
        .unwrap();
    let combined = editor.project();
    assert_eq!(
        crate::schema_version_for(combined),
        crate::INSTRUMENT_MODULATION_SCHEMA_VERSION
    );
    let combined_path = dir.join("instrument-modulation-and-touch.hex");
    crate::save_new(&combined_path, combined).unwrap();
    let bytes = std::fs::read(&combined_path).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        document["modulation_base_version"],
        crate::NATIVE_INSTRUMENT_TOUCH_SCHEMA_VERSION
    );
    assert_eq!(crate::load(&combined_path).unwrap(), *combined);
    editor
        .execute(Command::SpliceDeviceAutomationTouch {
            device: instrument_id,
            target: DeviceAutomationTarget::Parameter(ParameterId::from_static(29)),
            vst3_class: None,
            spans: vec![hex_project::DeviceAutomationTouchSpan {
                start: BeatTime::ZERO,
                end: BeatTime::ONE,
                steps: (0..=hex_project::MAX_DEVICE_AUTOMATION_POINTS_PER_LANE)
                    .map(|index| hex_project::DeviceAutomationTouchStep {
                        beat: BeatTime::from_raw(index as u64),
                        value: hex_project::AutomationValue::new((index % 8) as f32 / 8.0).unwrap(),
                    })
                    .collect(),
            }],
        })
        .unwrap();
    let extended = editor.project();
    let extended_path = dir.join("instrument-modulation-and-long-touch.hex");
    crate::save_new(&extended_path, extended).unwrap();
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&extended_path).unwrap()).unwrap();
    assert_eq!(
        document["version"],
        crate::INSTRUMENT_MODULATION_SCHEMA_VERSION
    );
    assert_eq!(
        document["modulation_base_version"],
        crate::EXTENDED_AUTOMATION_TOUCH_SCHEMA_VERSION
    );
    assert_eq!(crate::load(&extended_path).unwrap(), *extended);
    let original_version = std::fs::read(&extended_path).unwrap();
    let old_token = crate::load_with_token(&extended_path).unwrap().1;
    let previous_bank = extended.device(instrument_id).unwrap().modulation();
    let mut slots = previous_bank.sources().to_vec();
    slots.push(Modulator {
        id: ModulationSourceId::new(extended.next_identity()).unwrap(),
        enabled: true,
        color: ElementColor::Cyan,
        state: NativeDeviceState::new(
            DeviceTypeId::from_static("hex.modulator.note-pressure"),
            1,
            Vec::new(),
        )
        .unwrap(),
    });
    let pressure = DeviceModulation::new(slots, previous_bank.routes().to_vec()).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: instrument_id,
            modulation: pressure,
        })
        .unwrap();
    let expression = editor.project();
    assert_eq!(
        crate::schema_version_for(expression),
        crate::NOTE_PRESSURE_MODULATOR_SCHEMA_VERSION
    );
    assert!(old_token.requires_save_confirmation(expression).unwrap());
    let pressure_path = dir.join("note-pressure-and-long-touch.hex");
    crate::save_new(&pressure_path, expression).unwrap();
    assert_eq!(crate::load(&pressure_path).unwrap(), *expression);
    assert_eq!(std::fs::read(&extended_path).unwrap(), original_version);
    let mut malformed = document;
    malformed["version"] = crate::NOTE_PRESSURE_MODULATOR_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&malformed).unwrap()).is_err());
    let pressure_bytes = std::fs::read(&pressure_path).unwrap();
    let bank = expression.device(instrument_id).unwrap().modulation();
    let mut slots = bank.sources().to_vec();
    slots.push(Modulator {
        id: ModulationSourceId::new(expression.next_identity()).unwrap(),
        enabled: true,
        color: ElementColor::Cyan,
        state: NativeDeviceState::new(
            DeviceTypeId::from_static("hex.modulator.button"),
            1,
            vec![0],
        )
        .unwrap(),
    });
    let combined = DeviceModulation::new(slots, bank.routes().to_vec()).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: instrument_id,
            modulation: combined,
        })
        .unwrap();
    let current = editor.project();
    assert_eq!(
        crate::schema_version_for(current),
        crate::BUTTON_MODULATOR_SCHEMA_VERSION
    );
    let combined_path = dir.join("button-pressure-and-long-touch.hex");
    crate::save_new(&combined_path, current).unwrap();
    assert_eq!(crate::load(&combined_path).unwrap(), *current);
    assert_eq!(std::fs::read(&pressure_path).unwrap(), pressure_bytes);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn button_schema_preserves_effect_only_banks_and_reader_consent() {
    let mut editor = Editor::new(legacy_fixture(), 8).unwrap();
    let device = editor.project().main_devices()[0].id();
    let root = temp_dir();
    let prior_path = root.join("prior.hex");
    crate::save_new(&prior_path, editor.project()).unwrap();
    let old_bytes = std::fs::read(&prior_path).unwrap();
    let old_token = crate::load_with_token(&prior_path).unwrap().1;
    let prior_version = crate::schema_version_for(editor.project());
    let next = editor.project().next_identity();
    let button = Modulator {
        id: ModulationSourceId::new(next).unwrap(),
        enabled: true,
        color: ElementColor::Cyan,
        state: NativeDeviceState::new(
            DeviceTypeId::from_static("hex.modulator.button"),
            1,
            vec![1],
        )
        .unwrap(),
    };
    let modulation = DeviceModulation::new(vec![button], Vec::new()).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();
    let current = editor.project();
    assert_eq!(
        crate::schema_version_for(current),
        crate::BUTTON_MODULATOR_SCHEMA_VERSION
    );
    assert!(old_token.requires_save_confirmation(current).unwrap());
    let current_path = root.join("button.hex");
    crate::save_new(&current_path, current).unwrap();
    assert_eq!(crate::load(&current_path).unwrap(), *current);
    assert_eq!(std::fs::read(&prior_path).unwrap(), old_bytes);
    assert_eq!(
        crate::schema_version_for(&crate::load(&prior_path).unwrap()),
        prior_version
    );
    let mut wrong: serde_json::Value =
        serde_json::from_slice(&crate::encode(current).unwrap()).unwrap();
    wrong["version"] = crate::MODULATION_SLOT_COLOR_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong).unwrap()).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn macro_four_schema_preserves_values_outputs_and_prior_reader_consent() {
    let mut editor = Editor::new(legacy_fixture(), 8).unwrap();
    let unit = &editor.project().main_devices()[0];
    let device = unit.id();
    let device_type = unit.state().type_id();
    let root = temp_dir();
    let prior_path = root.join("prior.hex");
    crate::save_new(&prior_path, editor.project()).unwrap();
    let old_bytes = std::fs::read(&prior_path).unwrap();
    let old_token = crate::load_with_token(&prior_path).unwrap().1;
    let prior_version = crate::schema_version_for(editor.project());
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let module = Modulator {
        id: source,
        enabled: true,
        color: ElementColor::Cyan,
        state: NativeDeviceState::new(
            DeviceTypeId::from_static("hex.modulator.macro-four"),
            1,
            [0.125_f32, 0.25, 0.5, 0.75]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect(),
        )
        .unwrap(),
    };
    let routes = (1..=4)
        .map(|index| DeviceModulationRoute {
            id: ModulationRouteId::new(next + u64::from(index)).unwrap(),
            source,
            output: ModulationOutputId::new(index).unwrap(),
            destination: ModulationDestination::Device {
                device_type,
                target: DeviceAutomationTarget::Parameter(ParameterId::from_static(1)),
                vst3_class: None,
            },
            amount: ModulationAmount::new(index as f32 / 8.0).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        })
        .collect();
    let modulation = DeviceModulation::new(vec![module], routes).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();
    let current = editor.project();
    assert_eq!(
        crate::schema_version_for(current),
        crate::MACRO_FOUR_MODULATOR_SCHEMA_VERSION
    );
    assert!(old_token.requires_save_confirmation(current).unwrap());
    let current_path = root.join("macro-four.hex");
    crate::save_new(&current_path, current).unwrap();
    assert_eq!(crate::load(&current_path).unwrap(), *current);
    assert_eq!(std::fs::read(&prior_path).unwrap(), old_bytes);
    assert_eq!(
        crate::schema_version_for(&crate::load(&prior_path).unwrap()),
        prior_version
    );
    let mut wrong: serde_json::Value =
        serde_json::from_slice(&crate::encode(current).unwrap()).unwrap();
    wrong["version"] = crate::BUTTON_MODULATOR_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong).unwrap()).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn loaded_sampler_bank_migrates_opaque_inventory_and_saves_with_its_new_reader_floor() {
    const HISTORICAL: &[u8] =
        include_bytes!("../../../tests/fixtures/sampler-crossfade/schema66.json");
    let mut editor = Editor::new(crate::decode(HISTORICAL).unwrap(), 8).unwrap();
    let loaded = editor
        .project()
        .tracks()
        .iter()
        .filter_map(|track| track.instrument())
        .find(|instrument| !instrument.state().asset_references().is_empty())
        .unwrap();
    let device = loaded.id();
    let device_type = loaded.state().type_id();
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let modulation = DeviceModulation::new(
        vec![Modulator {
            id: source,
            enabled: true,
            color: ElementColor::Cyan,
            state: NativeDeviceState::new(
                DeviceTypeId::from_static("hex.modulator.macro"),
                1,
                0.25_f32.to_le_bytes().to_vec(),
            )
            .unwrap(),
        }],
        vec![DeviceModulationRoute {
            id: ModulationRouteId::new(next + 1).unwrap(),
            source,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Device {
                device_type,
                target: DeviceAutomationTarget::Parameter(ParameterId::from_static(1)),
                vst3_class: None,
            },
            amount: ModulationAmount::new(0.5).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        }],
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();
    let project = editor.project();
    assert_eq!(
        crate::schema_version_for(project),
        crate::SAMPLER_MODULATION_SCHEMA_VERSION
    );

    let mut old_document: serde_json::Value =
        serde_json::from_slice(&crate::encode(project).unwrap()).unwrap();
    // The old colored-bank wrapper admitted this opaque provider without a
    // playback grant. Its layout and embedded schema66 stay identical.
    old_document["version"] = crate::MODULATION_SLOT_COLOR_SCHEMA_VERSION.into();
    let old_bytes = serde_json::to_vec(&old_document).unwrap();
    let root = temp_dir();
    let old_path = root.join("legacy-sampler-bank.hex");
    std::fs::write(&old_path, &old_bytes).unwrap();
    let (opened, token) = crate::load_with_token(&old_path).unwrap();
    assert_eq!(&opened, project);
    assert!(token.requires_save_confirmation(&opened).unwrap());
    let new_path = root.join("sampler-modulation.hex");
    crate::save_new(&new_path, &opened).unwrap();
    assert_eq!(crate::load(&new_path).unwrap(), opened);
    assert_eq!(std::fs::read(&old_path).unwrap(), old_bytes);

    let mut missing_bank: serde_json::Value = serde_json::from_slice(HISTORICAL).unwrap();
    missing_bank["version"] = crate::SAMPLER_MODULATION_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&missing_bank).unwrap()).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn math_schema_preserves_two_named_inputs_and_requires_reader_upgrade() {
    let mut editor = Editor::new(legacy_fixture(), 8).unwrap();
    let device = editor.project().main_devices()[0].id();
    let root = temp_dir();
    let prior_path = root.join("prior.hex");
    crate::save_new(&prior_path, editor.project()).unwrap();
    let original = std::fs::read(&prior_path).unwrap();
    let token = crate::load_with_token(&prior_path).unwrap().1;
    let next = editor.project().next_identity();
    let id = |offset| ModulationSourceId::new(next + offset).unwrap();
    let math_type = DeviceTypeId::from_static("hex.modulator.math");
    let mut payload = vec![2];
    payload.extend_from_slice(&(-0.25_f32).to_le_bytes());
    payload.extend_from_slice(&0.625_f32.to_le_bytes());
    let slot = |id, state| Modulator {
        id,
        state,
        color: ElementColor::Cyan,
        enabled: true,
    };
    let slots = vec![
        slot(
            id(0),
            NativeDeviceState::new(math_type, 1, payload).unwrap(),
        ),
        slot(
            id(1),
            NativeDeviceState::new(
                DeviceTypeId::from_static("hex.modulator.macro"),
                1,
                0.25_f32.to_le_bytes().to_vec(),
            )
            .unwrap(),
        ),
    ];
    let routes = (1..=2)
        .map(|index| DeviceModulationRoute {
            id: ModulationRouteId::new(next + u64::from(index) + 1).unwrap(),
            source: id(1),
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Modulator {
                source: id(0),
                module_type: math_type,
                parameter: ParameterId::from_static(index),
            },
            amount: ModulationAmount::new(if index == 1 { 0.25 } else { -0.25 }).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        })
        .collect();
    let modulation = DeviceModulation::new(slots, routes).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();
    let project = editor.project();
    assert_eq!(
        crate::schema_version_for(project),
        crate::MATH_MODULATOR_SCHEMA_VERSION
    );
    assert!(token.requires_save_confirmation(project).unwrap());
    let path = root.join("math.hex");
    crate::save_new(&path, project).unwrap();
    assert_eq!(crate::load(&path).unwrap(), *project);
    assert_eq!(std::fs::read(&prior_path).unwrap(), original);
    let mut wrong: serde_json::Value =
        serde_json::from_slice(&crate::encode(project).unwrap()).unwrap();
    wrong["version"] = crate::MODULATION_SLOT_COLOR_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong).unwrap()).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn buttons_reader_floor_round_trips_two_switches_and_distinct_outputs() {
    let mut editor = Editor::new(legacy_fixture(), 8).unwrap();
    let device = editor.project().main_devices()[0].id();
    let device_type = editor.project().device(device).unwrap().state().type_id();
    let root = temp_dir();
    let previous = root.join("previous.hex");
    crate::save_new(&previous, editor.project()).unwrap();
    let original = std::fs::read(&previous).unwrap();
    let token = crate::load_with_token(&previous).unwrap().1;
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let slot = Modulator {
        id: source,
        color: ElementColor::Cyan,
        enabled: true,
        state: NativeDeviceState::new(
            DeviceTypeId::from_static("hex.modulator.buttons"),
            1,
            vec![1, 0],
        )
        .unwrap(),
    };
    let routes = (1..=2)
        .map(|number| DeviceModulationRoute {
            id: ModulationRouteId::new(next + u64::from(number)).unwrap(),
            source,
            output: ModulationOutputId::new(number).unwrap(),
            destination: ModulationDestination::Device {
                device_type,
                target: DeviceAutomationTarget::Parameter(ParameterId::from_static(number)),
                vst3_class: None,
            },
            amount: ModulationAmount::new(0.5).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        })
        .collect();
    let modulation = DeviceModulation::new(vec![slot], routes).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();
    let project = editor.project();
    assert_eq!(
        crate::schema_version_for(project),
        crate::BUTTONS_MODULATOR_SCHEMA_VERSION
    );
    assert!(token.requires_save_confirmation(project).unwrap());
    let current = root.join("buttons.hex");
    crate::save_new(&current, project).unwrap();
    assert_eq!(crate::load(&current).unwrap(), *project);
    assert_eq!(std::fs::read(&previous).unwrap(), original);
    let mut wrong: serde_json::Value =
        serde_json::from_slice(&crate::encode(project).unwrap()).unwrap();
    wrong["version"] = crate::MATH_MODULATOR_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong).unwrap()).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mix_reader_floor_keeps_previous_buttons_and_three_input_assignments() {
    let mut editor = Editor::new(legacy_fixture(), 8).unwrap();
    let device = editor.project().main_devices()[0].id();
    let next = editor.project().next_identity();
    let slot = |offset, type_name, payload| Modulator {
        id: ModulationSourceId::new(next + offset).unwrap(),
        color: ElementColor::Cyan,
        enabled: true,
        state: NativeDeviceState::new(DeviceTypeId::from_static(type_name), 1, payload).unwrap(),
    };
    let buttons = slot(0, "hex.modulator.buttons", vec![1, 0]);
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: DeviceModulation::new(vec![buttons.clone()], vec![]).unwrap(),
        })
        .unwrap();
    let root = temp_dir();
    let previous = root.join("buttons.hex");
    crate::save_new(&previous, editor.project()).unwrap();
    let original = std::fs::read(&previous).unwrap();
    let (opened, token) = crate::load_with_token(&previous).unwrap();
    assert_eq!(opened, *editor.project());
    assert_eq!(
        crate::schema_version_for(&opened),
        crate::BUTTONS_MODULATOR_SCHEMA_VERSION
    );

    let mix = slot(
        1,
        "hex.modulator.mix",
        [-0.25_f32, 0.625, 0.25]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect(),
    );
    let routes = (1..=3)
        .map(|input| DeviceModulationRoute {
            id: ModulationRouteId::new(next + 1 + u64::from(input)).unwrap(),
            source: buttons.id,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Modulator {
                source: mix.id,
                module_type: mix.state.type_id(),
                parameter: ParameterId::from_static(input),
            },
            amount: ModulationAmount::new(0.25).unwrap(),
            range: ModulationRange::default(),
            enabled: true,
        })
        .collect();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: DeviceModulation::new(vec![buttons, mix], routes).unwrap(),
        })
        .unwrap();
    let project = editor.project();
    assert_eq!(
        crate::schema_version_for(project),
        crate::MIX_MODULATOR_SCHEMA_VERSION
    );
    assert!(token.requires_save_confirmation(project).unwrap());
    let path = root.join("mix.hex");
    crate::save_new(&path, project).unwrap();
    assert_eq!(crate::load(&path).unwrap(), *project);
    assert_eq!(std::fs::read(&previous).unwrap(), original);
    let mut wrong: serde_json::Value =
        serde_json::from_slice(&crate::encode(project).unwrap()).unwrap();
    wrong["version"] = crate::BUTTONS_MODULATOR_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong).unwrap()).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn note_counter_and_ramp_reader_floors_preserve_settings_and_instrument_routes() {
    let mut editor = Editor::new(legacy_fixture(), 8).unwrap();
    let instrument_type = DeviceTypeId::from_static("hex.instrument.fm4");
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "counter instrument".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: NativeDeviceState::new(instrument_type, 1, vec![0; 144]).unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument track");
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let root = temp_dir();
    let previous = root.join("previous.hex");
    crate::save_new(&previous, editor.project()).unwrap();
    let (opened, token) = crate::load_with_token(&previous).unwrap();
    assert_eq!(opened, *editor.project());
    let original = std::fs::read(&previous).unwrap();
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let slot = Modulator {
        id: source,
        color: ElementColor::Cyan,
        enabled: true,
        state: NativeDeviceState::new(
            DeviceTypeId::from_static("hex.modulator.note-counter"),
            1,
            vec![12, 0, 5, 0, 1],
        )
        .unwrap(),
    };
    let route = DeviceModulationRoute {
        id: ModulationRouteId::new(next + 1).unwrap(),
        source,
        output: ModulationOutputId::MAIN,
        destination: ModulationDestination::Device {
            device_type: instrument_type,
            target: DeviceAutomationTarget::Parameter(ParameterId::from_static(29)),
            vst3_class: None,
        },
        amount: ModulationAmount::new(0.25).unwrap(),
        range: ModulationRange::default(),
        enabled: true,
    };
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: DeviceModulation::new(vec![slot], vec![route]).unwrap(),
        })
        .unwrap();
    let project = editor.project();
    assert_eq!(
        crate::schema_version_for(project),
        crate::NOTE_COUNTER_MODULATOR_SCHEMA_VERSION
    );
    assert!(token.requires_save_confirmation(project).unwrap());
    let path = root.join("counter.hex");
    crate::save_new(&path, project).unwrap();
    assert_eq!(crate::load(&path).unwrap(), *project);
    assert_eq!(std::fs::read(&previous).unwrap(), original);
    let mut wrong: serde_json::Value =
        serde_json::from_slice(&crate::encode(project).unwrap()).unwrap();
    wrong["version"] = crate::MIX_MODULATOR_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong).unwrap()).is_err());
    let counter_bytes = std::fs::read(&path).unwrap();
    let counter_token = crate::load_with_token(&path).unwrap().1;
    let bank = project.device(device).unwrap().modulation();
    let ramp_id = ModulationSourceId::new(project.next_identity()).unwrap();
    let mut ramp_payload = 50_000_u32.to_le_bytes().to_vec();
    ramp_payload.extend_from_slice(&[2, 1, 0]);
    let ramp = Modulator {
        id: ramp_id,
        color: ElementColor::Cyan,
        enabled: true,
        state: NativeDeviceState::new(
            DeviceTypeId::from_static("hex.modulator.ramp"),
            1,
            ramp_payload,
        )
        .unwrap(),
    };
    let mut sources = bank.sources().to_vec();
    sources.push(ramp);
    let mut routes = bank.routes().to_vec();
    routes.push(DeviceModulationRoute {
        id: ModulationRouteId::new(project.next_identity() + 1).unwrap(),
        source: ramp_id,
        ..route
    });
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: DeviceModulation::new(sources, routes).unwrap(),
        })
        .unwrap();
    let project = editor.project();
    assert_eq!(
        crate::schema_version_for(project),
        crate::RAMP_MODULATOR_SCHEMA_VERSION
    );
    assert!(counter_token.requires_save_confirmation(project).unwrap());
    let ramp_path = root.join("counter-and-ramp.hex");
    crate::save_new(&ramp_path, project).unwrap();
    assert_eq!(crate::load(&ramp_path).unwrap(), *project);
    assert_eq!(std::fs::read(&path).unwrap(), counter_bytes);
    let mut wrong: serde_json::Value =
        serde_json::from_slice(&crate::encode(project).unwrap()).unwrap();
    wrong["version"] = crate::NOTE_COUNTER_MODULATOR_SCHEMA_VERSION.into();
    assert!(crate::decode(&serde_json::to_vec(&wrong).unwrap()).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
