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
    std::fs::remove_dir_all(dir).unwrap();
}
