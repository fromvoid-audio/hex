use hex_project::{Command, DeviceComparison, DeviceVariant, Editor, Project};

fn fixture() -> Project {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "retained.wav".into(),
            sample_rate: 48_000,
            frames: 16,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    for _ in 0..3 {
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
    }
    let mut snapshot = editor.project().snapshot();
    snapshot.main_devices[1].parent = Some(snapshot.main_devices[0].id);
    snapshot.main_devices[2].parent = Some(snapshot.main_devices[1].id);
    snapshot.main_devices[1].bypassed = true;
    let state = snapshot.main_devices[2]
        .state
        .clone()
        .with_output_gain_millidb(-3500)
        .unwrap();
    snapshot.main_devices[2].state = state.clone();
    let inactive = state
        .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
            slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
            asset: hex_lattice_contract::DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    snapshot.main_devices[2].comparison = Some(DeviceComparison::new(DeviceVariant::B, inactive));
    snapshot.main_devices[2]
        .automation
        .push(hex_project::DeviceAutomation {
            target: hex_project::DeviceAutomationTarget::HostOutputGain,
            points: vec![hex_project::AutomationPoint {
                beat: hex_project::BeatTime::from_quarters(1).unwrap(),
                value: hex_project::AutomationValue::new(0.5).unwrap(),
                outgoing: hex_project::AutomationInterpolation::Linear,
            }],
            read_selection: hex_project::AutomationReadSelection::Authored,
            touch: Vec::new(),
            vst3_class: None,
        });
    Project::from_snapshot(snapshot).unwrap()
}

#[test]
fn every_owner_and_unbound_source_roundtrips_without_cross_owner_parenting() {
    let mut editor = Editor::new(fixture(), 16).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let hex_project::Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    for _ in 0..2 {
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
    }
    editor
        .execute(Command::CreateNativeSource {
            name: "retained".into(),
            devices: vec![hex_project::default_instrument_state().unwrap(); 2],
            instrument_index: 0,
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].devices[1].parent = Some(snapshot.tracks[0].devices[0].id);
    snapshot.return_tracks[0].devices[1].parent = Some(snapshot.return_tracks[0].devices[0].id);
    snapshot.native_sources[0].devices[1].parent = Some(snapshot.native_sources[0].devices[0].id);
    let project = Project::from_snapshot(snapshot.clone()).unwrap();
    assert_eq!(
        crate::decode(&crate::encode(&project).unwrap())
            .unwrap()
            .snapshot(),
        snapshot
    );
    let source = project.native_sources()[0].id();
    let parent = project.native_sources()[0].instrument().id();
    let mut bound = Editor::new(project, 4).unwrap();
    bound
        .execute(Command::BindTrackSource {
            track,
            source: Some(source),
        })
        .unwrap();
    let bound = bound.project();
    assert_eq!(
        crate::schema_version_for(bound),
        crate::BOUND_SOURCE_EFFECT_SCHEMA_VERSION
    );
    assert!(crate::decode(&crate::encode_base(bound).unwrap()).is_err());
    assert!(crate::encode_schema22(bound).is_err());
    let saved = crate::encode(bound).unwrap();
    let restored = crate::decode(&saved).unwrap();
    assert_eq!(crate::encode(&restored).unwrap(), saved);
    assert_eq!(restored.snapshot(), bound.snapshot());
    let restored_unit = restored.native_source(source).unwrap();
    assert_eq!(restored_unit.receiver(), Some(track));
    assert_eq!(
        restored_unit.post_instrument_effects()[0].parent(),
        Some(parent)
    );

    let mut bad = snapshot.clone();
    bad.tracks[0].devices[1].parent = Some(bad.return_tracks[0].devices[0].id);
    assert!(Project::from_snapshot(bad).is_err());
    let mut bad = snapshot;
    bad.native_sources[0].instrument_index = 1;
    assert!(
        Project::from_snapshot(bad).is_err(),
        "instrument cannot become a child"
    );
}

#[test]
fn serial_units_roundtrip_complete_state_and_old_writers_refuse_loss() {
    let project = fixture();
    assert_eq!(
        crate::schema_version_for(&project),
        crate::SERIAL_DEVICE_SCHEMA_VERSION
    );
    let bytes = crate::encode(&project).unwrap();
    let loaded = crate::decode(&bytes).unwrap();
    assert_eq!(loaded.snapshot(), project.snapshot());
    assert_eq!(crate::encode(&loaded).unwrap(), bytes);
    assert!(crate::encode_schema20(&project).is_err());
    assert!(crate::encode_schema22(&project).is_err());
    let older = crate::FileToken::from_validated_bytes(
        crate::encode(&Project::new(48_000).unwrap())
            .unwrap()
            .into(),
    );
    assert!(older.requires_save_confirmation(&project).unwrap());
}

#[test]
fn malformed_incomplete_and_future_membership_refuses_without_changing_bytes() {
    let original = crate::encode(&fixture()).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    for case in 0..9 {
        let mut bad = value.clone();
        match case {
            0 => {
                bad.as_object_mut().unwrap().remove("serial_devices");
            }
            1 => {
                bad["serial_devices"].as_array_mut().unwrap().pop();
            }
            2 => {
                bad["serial_devices"][1]["parent"] = u64::MAX.into();
            }
            3 => {
                bad["serial_devices"][1]["device"] = bad["serial_devices"][0]["device"].clone();
            }
            4 => {
                bad["serial_devices"][1]
                    .as_object_mut()
                    .unwrap()
                    .remove("parent");
            }
            5 => {
                bad["serial_devices"][1]["unknown"] = true.into();
            }
            6 => {
                bad["serial_base_version"] = crate::SERIAL_DEVICE_SCHEMA_VERSION.into();
            }
            7 => {
                bad["version"] = (crate::NEWEST_SCHEMA_VERSION + 1).into();
            }
            _ => {
                bad["serial_devices"][0]["parent"] = bad["serial_devices"][1]["device"].clone();
            }
        }
        assert!(
            crate::decode(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "case {case}"
        );
    }
    assert_eq!(crate::encode(&fixture()).unwrap(), original);
}

#[test]
fn authentic_flat_schema66_migrates_as_roots() {
    let bytes = include_bytes!("../../../tests/fixtures/sampler-crossfade/schema66.json");
    let project = crate::decode(bytes).unwrap();
    assert!(project.devices().all(|device| device.parent().is_none()));
    assert_eq!(crate::encode(&project).unwrap(), bytes);
}

#[test]
fn legacy_writers_accept_the_flat_control_but_refuse_serial_membership() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "flat".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::RenameTrack {
            track,
            name: "flat".into(),
        })
        .unwrap();
    for _ in 0..2 {
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
    }
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].color = hex_project::ElementColor::default();
    let flat = Project::from_snapshot(snapshot.clone()).unwrap();
    snapshot.tracks[0].devices[1].parent = Some(snapshot.tracks[0].devices[0].id);
    let nested = Project::from_snapshot(snapshot).unwrap();
    for writer in [crate::encode_schema20, crate::encode_schema22] {
        writer(&flat).expect("flat positive control");
        assert!(writer(&nested).is_err(), "membership cannot be discarded");
    }
}
