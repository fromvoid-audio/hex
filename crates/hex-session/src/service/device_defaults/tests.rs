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
            NativeDeviceClass::MidiProcessor => hex_storage::preset::DeviceClass::MidiProcessor,
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
                control: None,
            },
            3 => Request::InsertEffectBusDevice {
                project_generation: 0,
                revision: event.state.revision,
                destination: crate::service::EffectBusDestination::Return(bus),
                class,
                type_id,
                before: None,
                control: None,
            },
            4 => Request::DropNativeDevice {
                project_generation: 0,
                revision: event.state.revision,
                source: crate::service::NativeDeviceDropSource::Browser { class, type_id },
                track,
                before: None,
                replace: None,
                control: None,
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
                control: None,
            },
            7 => Request::InsertBrowserInstrumentTrack {
                project_generation: 0,
                revision: event.state.revision,
                class,
                type_id,
                new_track_gain: Some(0.5),
                control: None,
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
                control: None,
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
                control: None,
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
            control: None,
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
                control: None,
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

#[test]
fn cross_type_replacement_uses_saved_unit_default_and_prunes_compressor_sidechain() {
    let directory = TestDir::new();
    let operator_type = DeviceTypeId::from_static("hex.instrument.operator");
    let sine_type = DeviceTypeId::from_static("hex.instrument.sine");
    let delay_type = DeviceTypeId::from_static("hex.delay");
    let utility_type = DeviceTypeId::from_static("hex.utility");
    let compressor_type = DeviceTypeId::from_static("hex.compressor");

    let mut service = Service::new(48_000, 16, 0).unwrap();
    next_event(&service);
    service
        .set_device_defaults_directory(directory.0.clone())
        .unwrap();

    // Create two tracks: `source` (for sidechain) and `track` (for device replacements).
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "source".into(),
        }))
        .unwrap();
    let source_track = next_event(&service).state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "work".into(),
        }))
        .unwrap();
    let track = next_event(&service).state.project.tracks()[1].id();

    // Save a complete unit default for `hex.instrument.operator` (with a nested `hex.utility` child)
    // and for `hex.delay` (with a nested `hex.utility` child).
    let op_factory = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        operator_type,
    )
    .unwrap();
    let op_authored = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &op_factory,
        ParameterId::from_static(1),
        ParameterValue::Float(0.5),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertInstrument {
            track,
            state: op_authored.clone(),
        }))
        .unwrap();
    let op_ev = next_event(&service);
    assert!(op_ev.error.is_none(), "{:?}", op_ev.error);
    let op_id = op_ev
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let child_state = hex_engine::default_native_device_state(utility_type).unwrap();
    service
        .submit(Request::Edit(Command::InsertNestedDevice {
            parent: op_id,
            state: child_state.clone(),
        }))
        .unwrap();
    let op_child_ev = next_event(&service);
    assert!(op_child_ev.error.is_none(), "{:?}", op_child_ev.error);
    service
        .submit(Request::SaveDeviceUnitDefault {
            owner: crate::service::preset::Owner {
                project_generation: 0,
                revision: op_child_ev.state.revision,
                device: op_id,
                class: NativeDeviceClass::Instrument,
                type_id: operator_type,
            },
            root: directory.0.clone(),
        })
        .unwrap();
    let saved_op = next_event(&service);
    assert!(saved_op.error.is_none(), "{:?}", saved_op.error);

    let delay_factory = hex_engine::default_native_device_state(delay_type).unwrap();
    service
        .submit(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: delay_factory,
        }))
        .unwrap();
    let delay_ev = next_event(&service);
    assert!(delay_ev.error.is_none(), "{:?}", delay_ev.error);
    let Created::Device(delay_id) = delay_ev.created.unwrap() else {
        panic!("delay id");
    };
    service
        .submit(Request::Edit(Command::InsertNestedDevice {
            parent: delay_id,
            state: child_state,
        }))
        .unwrap();
    let delay_child_ev = next_event(&service);
    assert!(delay_child_ev.error.is_none(), "{:?}", delay_child_ev.error);
    service
        .submit(Request::SaveDeviceUnitDefault {
            owner: crate::service::preset::Owner {
                project_generation: 0,
                revision: delay_child_ev.state.revision,
                device: delay_id,
                class: NativeDeviceClass::Effect,
                type_id: delay_type,
            },
            root: directory.0.clone(),
        })
        .unwrap();
    let saved_delay = next_event(&service);
    assert!(saved_delay.error.is_none(), "{:?}", saved_delay.error);

    // 1. Replace `hex.instrument.sine` with `hex.instrument.operator` across all three replacement routes.
    for route in 0..3 {
        let sine_state = hex_engine::default_native_device_state_for_class(
            NativeDeviceClass::Instrument,
            sine_type,
        )
        .unwrap();
        service
            .submit(Request::Edit(Command::RemoveInstrument { track }))
            .unwrap();
        next_event(&service);
        service
            .submit(Request::Edit(Command::InsertInstrument {
                track,
                state: sine_state,
            }))
            .unwrap();
        let before = next_event(&service);
        assert!(before.error.is_none(), "{:?}", before.error);
        let sine_id = before
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();

        let request = match route {
            0 => Request::ReplaceNativeDevice {
                device: sine_id,
                class: NativeDeviceClass::Instrument,
                type_id: Box::new(operator_type),
            },
            1 => Request::LoadTrackCatalogDevice {
                project_generation: 0,
                revision: before.state.revision,
                track,
                class: NativeDeviceClass::Instrument,
                type_id: operator_type,
                placement: TrackCatalogPlacement::ReplaceInstrument(sine_id),
                control: std::sync::Arc::default(),
            },
            _ => Request::DropNativeDevice {
                project_generation: 0,
                revision: before.state.revision,
                source: crate::service::NativeDeviceDropSource::Browser {
                    class: NativeDeviceClass::Instrument,
                    type_id: operator_type,
                },
                track,
                before: None,
                replace: Some(sine_id),
                control: None,
            },
        };
        service.submit(request).unwrap();
        let after = next_event(&service);
        assert!(
            after.error.is_none(),
            "cross-type instrument replacement route {route} failed: {:?}",
            after.error
        );
        let replaced = after
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap();
        assert_eq!(replaced.state(), &op_authored);
        assert_eq!(
            after
                .state
                .project
                .devices()
                .filter(|d| d.parent() == Some(replaced.id()))
                .count(),
            1,
            "route {route} must install the saved unit default's child device"
        );
    }

    // 2. Replace `hex.utility` with `hex.delay` via `ReplaceNativeDevice`.
    service
        .submit(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(utility_type).unwrap(),
        }))
        .unwrap();
    let util_ev = next_event(&service);
    let Created::Device(util_id) = util_ev.created.unwrap() else {
        panic!("utility id");
    };
    service
        .submit(Request::ReplaceNativeDevice {
            device: util_id,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(delay_type),
        })
        .unwrap();
    let replaced_fx = next_event(&service);
    assert!(replaced_fx.error.is_none(), "{:?}", replaced_fx.error);
    assert_eq!(
        replaced_fx
            .state
            .project
            .device(util_id)
            .unwrap()
            .state()
            .type_id(),
        delay_type
    );
    assert_eq!(
        replaced_fx
            .state
            .project
            .devices()
            .filter(|d| d.parent() == Some(util_id))
            .count(),
        1
    );

    // 3. Same-provider `LoadDevicePreset` must still refuse loading a `hex.delay` unit preset onto `hex.utility`.
    let preset_path = directory.0.join("delay-unit.hex-device-preset.json");
    service
        .submit(Request::SaveDeviceUnitPreset {
            owner: crate::service::preset::Owner {
                project_generation: 0,
                revision: replaced_fx.state.revision,
                device: util_id,
                class: NativeDeviceClass::Effect,
                type_id: delay_type,
            },
            path: preset_path.clone(),
        })
        .unwrap();
    assert!(next_event(&service).error.is_none());

    service
        .submit(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(utility_type).unwrap(),
        }))
        .unwrap();
    let other_util_ev = next_event(&service);
    let Created::Device(other_util_id) = other_util_ev.created.unwrap() else {
        panic!("other utility id");
    };
    service
        .submit(Request::LoadDevicePreset {
            owner: crate::service::preset::Owner {
                project_generation: 0,
                revision: other_util_ev.state.revision,
                device: other_util_id,
                class: NativeDeviceClass::Effect,
                type_id: utility_type,
            },
            path: preset_path,
        })
        .unwrap();
    let mismatch = next_event(&service);
    assert!(
        mismatch.error.is_some(),
        "LoadDevicePreset must still refuse a mismatched provider type"
    );

    // 4. Replacing a sidechained `hex.compressor` with `hex.utility` prunes the sidechain and Undo restores it.
    service
        .submit(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(compressor_type).unwrap(),
        }))
        .unwrap();
    let comp_ev = next_event(&service);
    let Created::Device(comp_id) = comp_ev.created.unwrap() else {
        panic!("compressor id");
    };
    service
        .submit(Request::Edit(Command::SetSidechain {
            device: comp_id,
            source: Some(source_track),
        }))
        .unwrap();
    let sc_ev = next_event(&service);
    assert!(sc_ev.error.is_none(), "{:?}", sc_ev.error);
    assert_eq!(sc_ev.state.project.sidechains().len(), 1);

    service
        .submit(Request::ReplaceNativeDevice {
            device: comp_id,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(utility_type),
        })
        .unwrap();
    let replaced_sc = next_event(&service);
    assert!(replaced_sc.error.is_none(), "{:?}", replaced_sc.error);
    assert!(replaced_sc.state.project.sidechains().is_empty());
    assert_eq!(
        replaced_sc
            .state
            .project
            .device(comp_id)
            .unwrap()
            .state()
            .type_id(),
        utility_type
    );

    service.submit(Request::Undo).unwrap();
    let undone = next_event(&service);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(
        undone
            .state
            .project
            .device(comp_id)
            .unwrap()
            .state()
            .type_id(),
        compressor_type
    );
    assert_eq!(undone.state.project.sidechains().len(), 1);
}

#[test]
fn midi_processor_catalog_insert_parameter_patch_bypass_replace_and_undo_redo() {
    let pitch_type = DeviceTypeId::from_static("hex.midi.pitch");
    let service = Service::new(48_000, 16, 1_000_000).unwrap();
    let _initial = next_event(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "midi-pitch".into(),
        }))
        .unwrap();
    let added = next_event(&service);
    assert!(added.error.is_none(), "{:?}", added.error);
    let track = added.state.project.tracks()[0].id();

    // Insert via TrackCatalogPlacement::Insert (same path as UI device picker).
    service
        .submit(Request::LoadTrackCatalogDevice {
            project_generation: 0,
            revision: added.state.revision,
            track,
            class: NativeDeviceClass::MidiProcessor,
            type_id: pitch_type,
            placement: crate::service::TrackCatalogPlacement::Insert { before: None },
            control: std::sync::Arc::default(),
        })
        .unwrap();
    let inserted = next_event(&service);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let proc_device = inserted
        .state
        .project
        .track(track)
        .unwrap()
        .midi_processor()
        .unwrap();
    let proc_id = proc_device.id();
    assert_eq!(proc_device.state().type_id(), pitch_type);

    // Patch parameter via engine class API + canonical ReplaceDeviceState.
    let shifted = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::MidiProcessor,
        proc_device.state(),
        ParameterId::from_static(1),
        ParameterValue::Float(7.0),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::ReplaceDeviceState {
            device: proc_id,
            state: shifted.clone(),
        }))
        .unwrap();
    let patched = next_event(&service);
    assert!(patched.error.is_none(), "{:?}", patched.error);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::MidiProcessor,
            patched.state.project.device(proc_id).unwrap().state(),
            ParameterId::from_static(1),
        )
        .unwrap(),
        ParameterValue::Float(7.0)
    );

    // Bypass toggle + Undo/Redo.
    service
        .submit(Request::Edit(Command::ToggleDeviceBypass {
            device: proc_id,
        }))
        .unwrap();
    let bypassed = next_event(&service);
    assert!(bypassed.error.is_none(), "{:?}", bypassed.error);
    assert!(bypassed.state.project.device(proc_id).unwrap().bypassed());

    service.submit(Request::Undo).unwrap();
    let unbypassed = next_event(&service);
    assert!(!unbypassed.state.project.device(proc_id).unwrap().bypassed());

    // Replace via TrackCatalogPlacement::ReplaceMidiProcessor.
    service
        .submit(Request::LoadTrackCatalogDevice {
            project_generation: 0,
            revision: unbypassed.state.revision,
            track,
            class: NativeDeviceClass::MidiProcessor,
            type_id: pitch_type,
            placement: crate::service::TrackCatalogPlacement::ReplaceMidiProcessor(proc_id),
            control: std::sync::Arc::default(),
        })
        .unwrap();
    let replaced = next_event(&service);
    assert!(replaced.error.is_none(), "{:?}", replaced.error);
    let new_id = replaced
        .state
        .project
        .track(track)
        .unwrap()
        .midi_processor()
        .unwrap()
        .id();
    assert!(new_id > proc_id);

    // RemoveDevice + Undo restores cleanly.
    service
        .submit(Request::Edit(Command::RemoveDevice(new_id)))
        .unwrap();
    let removed = next_event(&service);
    assert!(removed.error.is_none(), "{:?}", removed.error);
    assert!(
        removed
            .state
            .project
            .track(track)
            .unwrap()
            .midi_processor()
            .is_none()
    );
    service.submit(Request::Undo).unwrap();
    let restored = next_event(&service);
    assert_eq!(
        restored
            .state
            .project
            .track(track)
            .unwrap()
            .midi_processor()
            .map(hex_project::Device::id),
        Some(new_id)
    );
}

#[test]
fn insert_nested_catalog_device_applies_unit_default_and_refuses_stale_depth_and_unsupported() {
    let directory = TestDir::new();
    let mut service = Service::new(48_000, 16, 4096).unwrap();
    let _ = next_event(&service);
    service
        .set_device_defaults_directory(directory.0.clone())
        .unwrap();

    let operator_type = DeviceTypeId::from_static("hex.instrument.operator");
    let delay_type = DeviceTypeId::from_static("hex.delay");
    let utility_type = DeviceTypeId::from_static("hex.utility");
    let limiter_type = DeviceTypeId::from_static("hex.limiter");

    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "vector".into(),
        }))
        .unwrap();
    let track_ev = next_event(&service);
    let Created::Track(track) = track_ev.created.unwrap() else {
        panic!("track")
    };
    let vector_state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        operator_type,
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertInstrument {
            track,
            state: vector_state,
        }))
        .unwrap();
    let inst_ev = next_event(&service);
    assert!(inst_ev.error.is_none(), "{:?}", inst_ev.error);
    let Created::Device(vector_id) = inst_ev.created.unwrap() else {
        panic!("vector")
    };

    // Save a whole-unit default for hex.delay containing a nested hex.utility child.
    let delay_state =
        hex_engine::default_native_device_state_for_class(NativeDeviceClass::Effect, delay_type)
            .unwrap();
    let utility_state =
        hex_engine::default_native_device_state_for_class(NativeDeviceClass::Effect, utility_type)
            .unwrap();
    service
        .submit(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: delay_state,
        }))
        .unwrap();
    let donor_ev = next_event(&service);
    let Created::Device(donor_delay) = donor_ev.created.unwrap() else {
        panic!("donor delay")
    };
    service
        .submit(Request::Edit(Command::InsertNestedDevice {
            parent: donor_delay,
            state: utility_state.clone(),
        }))
        .unwrap();
    let donor_child_ev = next_event(&service);
    assert!(donor_child_ev.error.is_none(), "{:?}", donor_child_ev.error);
    service
        .submit(Request::SaveDeviceUnitDefault {
            owner: crate::service::preset::Owner {
                project_generation: 0,
                revision: donor_child_ev.state.revision,
                device: donor_delay,
                class: NativeDeviceClass::Effect,
                type_id: delay_type,
            },
            root: directory.0.clone(),
        })
        .unwrap();
    let saved = next_event(&service);
    assert!(saved.error.is_none(), "{:?}", saved.error);

    // Insert hex.delay under Vector via Request::InsertNestedCatalogDevice: whole-unit
    // default (Delay + nested Utility) lands atomically in one history step.
    let revision_before = saved.state.revision;
    service
        .submit(Request::InsertNestedCatalogDevice {
            project_generation: 0,
            revision: revision_before,
            parent: vector_id,
            class: NativeDeviceClass::Effect,
            type_id: delay_type,
            control: None,
        })
        .unwrap();
    let inserted = next_event(&service);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let source_id = inserted
        .state
        .project
        .track(track)
        .unwrap()
        .source_id()
        .unwrap();
    let suffix = inserted
        .state
        .project
        .native_source(source_id)
        .unwrap()
        .post_instrument_effects();
    assert_eq!(suffix.len(), 2);
    assert_eq!(suffix[0].state().type_id(), delay_type);
    assert_eq!(suffix[0].parent(), Some(vector_id));
    assert_eq!(suffix[1].state().type_id(), utility_type);
    assert_eq!(suffix[1].parent(), Some(suffix[0].id()));

    // One Undo removes the entire inserted default unit cleanly.
    service.submit(Request::Undo).unwrap();
    let undone = next_event(&service);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert!(
        undone
            .state
            .project
            .native_source(source_id)
            .unwrap()
            .post_instrument_effects()
            .is_empty()
    );
    service.submit(Request::Redo).unwrap();
    let redone = next_event(&service);
    assert_eq!(
        redone
            .state
            .project
            .native_source(source_id)
            .unwrap()
            .post_instrument_effects()
            .len(),
        2
    );

    let baseline = redone.state.project.clone();
    let rev = redone.state.revision;

    // Stale revision, non-Effect class, and unsupported source effect (hex.limiter) all refuse cleanly.
    for bad in [
        Request::InsertNestedCatalogDevice {
            project_generation: 0,
            revision: rev.saturating_sub(1),
            parent: vector_id,
            class: NativeDeviceClass::Effect,
            type_id: utility_type,
            control: None,
        },
        Request::InsertNestedCatalogDevice {
            project_generation: 0,
            revision: rev,
            parent: vector_id,
            class: NativeDeviceClass::Instrument,
            type_id: operator_type,
            control: None,
        },
        Request::InsertNestedCatalogDevice {
            project_generation: 0,
            revision: rev,
            parent: vector_id,
            class: NativeDeviceClass::Effect,
            type_id: limiter_type,
            control: None,
        },
    ] {
        service.submit(bad).unwrap();
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert_eq!(*refused.state.project, *baseline);
    }

    // Depth-capped parent refuses cleanly without mutating the project.
    let mut deepest = donor_delay;
    for _ in 0..hex_project::MAX_SERIAL_DEVICE_DEPTH {
        let state = utility_state.clone();
        service
            .submit(Request::Edit(Command::InsertNestedDevice {
                parent: deepest,
                state,
            }))
            .unwrap();
        let ev = next_event(&service);
        if let Some(Created::Device(next)) = ev.created {
            deepest = next;
        } else {
            break;
        }
    }
    let capped_rev = {
        service
            .submit(Request::Edit(Command::InsertNestedDevice {
                parent: deepest,
                state: utility_state,
            }))
            .unwrap();
        let ev = next_event(&service);
        assert!(ev.error.is_some());
        ev.state.revision
    };
    let capped_project = {
        service
            .submit(Request::InsertNestedCatalogDevice {
                project_generation: 0,
                revision: capped_rev,
                parent: deepest,
                class: NativeDeviceClass::Effect,
                type_id: utility_type,
                control: None,
            })
            .unwrap();
        let ev = next_event(&service);
        assert!(ev.error.is_some());
        ev.state.project.clone()
    };
    assert_eq!(
        hex_engine::project_device_nesting_depth(&capped_project, deepest),
        Some(hex_project::MAX_SERIAL_DEVICE_DEPTH - 1)
    );
}
