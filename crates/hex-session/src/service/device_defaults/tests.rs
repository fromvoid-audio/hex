use super::*;
use crate::service::service_tests::{TestDir, next_event};
use crate::service::{Request, Service};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Created, DeviceComparison, DeviceVariant};

fn saved(
    root: &Path,
    class: NativeDeviceClass,
    type_id: DeviceTypeId,
) -> hex_storage::preset::Preset {
    let factory = hex_engine::default_native_device_state_for_class(class, type_id).unwrap();
    let state = hex_engine::patch_native_device_parameter_for_class(
        class,
        &factory,
        ParameterId::from_static(1),
        ParameterValue::Float(0.5),
    )
    .unwrap();
    let preset = hex_storage::preset::Preset {
        class: match class {
            NativeDeviceClass::Instrument => hex_storage::preset::DeviceClass::Instrument,
            NativeDeviceClass::Effect => hex_storage::preset::DeviceClass::Effect,
        },
        state,
        comparison: Some(DeviceComparison::new(DeviceVariant::B, factory)),
    };
    hex_storage::preset::defaults::save(root, &preset).unwrap();
    preset
}

#[test]
fn all_fresh_catalog_routes_use_saved_banks_with_one_undo_and_redo() {
    let directory = TestDir::new();
    let effect_type = DeviceTypeId::from_static("hex.utility");
    let instrument_type = DeviceTypeId::from_static("hex.instrument.operator");
    let effect = saved(&directory.0, NativeDeviceClass::Effect, effect_type);
    let instrument = saved(&directory.0, NativeDeviceClass::Instrument, instrument_type);
    for route in 0..12 {
        let mut service = Service::new(48_000, 16, 0).unwrap();
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "fixture".into(),
            }))
            .unwrap();
        let event = next_event(&service);
        let track = event.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::AddReturnTrack {
                name: "return".into(),
            }))
            .unwrap();
        let event = next_event(&service);
        let bus = event.state.project.return_tracks()[0].id();
        if matches!(route, 1 | 4 | 7)
            && event
                .state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .is_some()
        {
            service
                .submit(Request::Edit(Command::RemoveInstrument { track }))
                .unwrap();
            next_event(&service);
        }
        if route == 8 {
            service
                .submit(Request::Edit(Command::InsertDevice {
                    track,
                    before: None,
                    state: hex_engine::default_native_device_state_for_class(
                        NativeDeviceClass::Effect,
                        effect_type,
                    )
                    .unwrap(),
                }))
                .unwrap();
            next_event(&service);
        }
        if matches!(route, 9..=11)
            && event
                .state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        {
            service
                .submit(Request::Edit(Command::InsertInstrument {
                    track,
                    state: hex_engine::default_native_device_state_for_class(
                        NativeDeviceClass::Instrument,
                        instrument_type,
                    )
                    .unwrap(),
                }))
                .unwrap();
            next_event(&service);
        }
        service
            .set_device_defaults_directory(directory.0.clone())
            .unwrap();
        let class = if matches!(route, 1 | 4 | 6 | 7 | 9 | 10 | 11) {
            NativeDeviceClass::Instrument
        } else {
            NativeDeviceClass::Effect
        };
        let type_id = if class == NativeDeviceClass::Instrument {
            instrument_type
        } else {
            effect_type
        };
        let event = {
            service
                .submit(Request::Edit(Command::Gain { track, gain: 0.7 }))
                .unwrap();
            next_event(&service)
        };
        let original = event.state.project.clone();
        let request = match route {
            0 | 1 => Request::AddNativeDevice {
                track,
                class,
                type_id: Box::new(type_id),
                before: None,
            },
            2 => Request::InsertEffectBusDevice {
                project_generation: 0,
                revision: event.state.revision,
                destination: crate::service::EffectBusDestination::Main,
                class,
                type_id,
                before: None,
            },
            3 => Request::InsertEffectBusDevice {
                project_generation: 0,
                revision: event.state.revision,
                destination: crate::service::EffectBusDestination::Return(bus),
                class,
                type_id,
                before: None,
            },
            4 => Request::DropNativeDevice {
                project_generation: 0,
                revision: event.state.revision,
                source: crate::service::NativeDeviceDropSource::Browser { class, type_id },
                track,
                before: None,
                replace: None,
            },
            5 | 6 => Request::DropBrowserDeviceOnArrangement {
                project_generation: 0,
                revision: event.state.revision,
                class,
                type_id,
                destination: crate::service::ArrangementDeviceDestination::NewTrack {
                    before_track: None,
                },
                new_track_gain: Some(0.5),
            },
            7 => Request::InsertBrowserInstrumentTrack {
                project_generation: 0,
                revision: event.state.revision,
                class,
                type_id,
                new_track_gain: Some(0.5),
            },
            8 | 11 => Request::ReplaceNativeDevice {
                device: if route == 8 {
                    original.track(track).unwrap().devices()[0].id()
                } else {
                    original.track(track).unwrap().instrument().unwrap().id()
                },
                class,
                type_id: Box::new(type_id),
            },
            9 => Request::DropBrowserDeviceOnArrangement {
                project_generation: 0,
                revision: event.state.revision,
                class,
                type_id,
                destination: crate::service::ArrangementDeviceDestination::ExistingTrack {
                    track,
                    instrument: original
                        .track(track)
                        .unwrap()
                        .instrument()
                        .map(hex_project::Device::id),
                },
                new_track_gain: None,
            },
            10 => Request::DropNativeDevice {
                project_generation: 0,
                revision: event.state.revision,
                source: crate::service::NativeDeviceDropSource::Browser { class, type_id },
                track,
                before: None,
                replace: original
                    .track(track)
                    .unwrap()
                    .instrument()
                    .map(hex_project::Device::id),
            },
            _ => unreachable!(),
        };
        service.submit(request).unwrap();
        let changed = next_event(&service);
        assert!(changed.error.is_none(), "route{route}: {:?}", changed.error);
        let device = if matches!(route, 4 | 5 | 6 | 8 | 9 | 10 | 11) {
            assert!(changed.created.is_none(), "drop/replacement retains focus");
            let selected = if matches!(route, 5 | 6) {
                assert_eq!(
                    changed.state.project.tracks().len(),
                    original.tracks().len() + 1
                );
                changed.state.project.tracks().last().unwrap()
            } else {
                changed.state.project.track(track).unwrap()
            };
            if class == NativeDeviceClass::Instrument {
                selected.instrument().unwrap()
            } else {
                selected.devices().last().unwrap()
            }
        } else {
            match changed.created.expect("selection-bearing insertion result") {
                Created::Device(id) => changed.state.project.device(id).unwrap(),
                Created::Track(id) => changed
                    .state
                    .project
                    .track(id)
                    .unwrap()
                    .instrument()
                    .unwrap(),
                other => panic!("route{route}: {other:?}"),
            }
        };
        let expected = if class == NativeDeviceClass::Instrument {
            &instrument
        } else {
            &effect
        };
        assert_eq!(device.state(), &expected.state, "route{route}");
        assert_eq!(
            device.comparison(),
            expected.comparison.as_ref(),
            "route{route}"
        );
        service.submit(Request::Undo).unwrap();
        let undo = next_event(&service);
        assert!(undo.error.is_none());
        assert_eq!(
            undo.state.project.tracks(),
            original.tracks(),
            "route{route}"
        );
        assert_eq!(undo.state.project.main_devices(), original.main_devices());
        assert_eq!(undo.state.project.return_tracks(), original.return_tracks());
        service.submit(Request::Redo).unwrap();
        let redo = next_event(&service);
        assert!(redo.error.is_none());
        assert_eq!(redo.state.project, changed.state.project);
    }
}

#[test]
fn missing_defaults_use_factory_errors_preserve_project_and_supplied_state_ignores_disk() {
    let directory = TestDir::new();
    let class = NativeDeviceClass::Effect;
    let type_id = DeviceTypeId::from_static("hex.utility");
    let factory = hex_engine::default_native_device_state_for_class(class, type_id).unwrap();
    let mut service = Service::new(48_000, 8, 0).unwrap();
    next_event(&service);
    assert!(
        service
            .set_device_defaults_directory(Path::new("").to_path_buf())
            .is_err()
    );
    assert!(
        service
            .set_device_defaults_directory(Path::new(&"a".repeat(4097)).to_path_buf())
            .is_err()
    );
    service
        .set_device_defaults_directory(directory.0.clone())
        .unwrap();
    service
        .submit(Request::InsertEffectBusDevice {
            project_generation: 0,
            revision: 0,
            destination: crate::service::EffectBusDestination::Main,
            class,
            type_id,
            before: None,
        })
        .unwrap();
    let initial = next_event(&service);
    assert!(initial.error.is_none(), "{:?}", initial.error);
    assert_eq!(initial.state.project.main_devices()[0].state(), &factory);
    assert!(
        initial.state.project.main_devices()[0]
            .comparison()
            .is_none()
    );
    let preset = saved(&directory.0, class, type_id);
    let path = hex_storage::preset::defaults::path(&directory.0, preset.class, type_id).unwrap();
    let valid = std::fs::read(&path).unwrap();
    for bytes in [b"broken".to_vec(), {
        let mut json: serde_json::Value = serde_json::from_slice(&valid).unwrap();
        json["schema"] = serde_json::json!(u32::MAX);
        serde_json::to_vec(&json).unwrap()
    }] {
        std::fs::write(&path, &bytes).unwrap();
        service
            .submit(Request::InsertEffectBusDevice {
                project_generation: 0,
                revision: initial.state.revision,
                destination: crate::service::EffectBusDestination::Main,
                class,
                type_id,
                before: None,
            })
            .unwrap();
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert!(refused.playback.is_none());
        assert_eq!(refused.state.project, initial.state.project);
        assert_eq!(refused.state.revision, initial.state.revision);
        assert_eq!(
            (refused.state.can_undo, refused.state.can_redo),
            (initial.state.can_undo, initial.state.can_redo)
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    // Supplied state is a different contract: even an unreadable default cannot
    // replace bytes restored by clipboard, preset, project or direct commands.
    service
        .submit(Request::Edit(Command::InsertMainDevice {
            before: None,
            state: factory.clone(),
        }))
        .unwrap();
    let supplied = next_event(&service);
    assert!(supplied.error.is_none(), "{:?}", supplied.error);
    assert_eq!(supplied.state.project.main_devices()[1].state(), &factory);
    std::fs::write(&path, valid).unwrap();
    service
        .submit(Request::ReplaceNativeDevice {
            device: supplied.state.project.main_devices()[1].id(),
            class,
            type_id: Box::new(type_id),
        })
        .unwrap();
    let changed = next_event(&service);
    assert!(changed.error.is_none(), "{:?}", changed.error);
    assert_eq!(
        changed.state.project.main_devices()[1].state(),
        &preset.state
    );
    assert_eq!(
        changed.state.project.main_devices()[1].comparison(),
        preset.comparison.as_ref()
    );
}
