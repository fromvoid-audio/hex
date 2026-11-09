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
