use super::*;
use crate::{Command, Editor};

fn source(id: u64) -> Modulator {
    Modulator {
        id: ModulationSourceId::new(id).unwrap(),
        enabled: true,
        state: NativeDeviceState::new(
            DeviceTypeId::from_static("test.modulator"),
            2,
            vec![0, 255, 1],
        )
        .unwrap(),
    }
}
fn route(id: u64, from: u64, destination: ModulationDestination) -> DeviceModulationRoute {
    DeviceModulationRoute {
        id: ModulationRouteId::new(id).unwrap(),
        source: ModulationSourceId::new(from).unwrap(),
        output: ModulationOutputId::MAIN,
        destination,
        amount: ModulationAmount::new(-0.25).unwrap(),
        range: ModulationRange {
            start: ModulationAmount::new(0.0).unwrap(),
            end: ModulationAmount::new(1.0).unwrap(),
        },
        enabled: true,
    }
}
fn fixture() -> (Editor, DeviceId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(id) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: crate::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    (editor, id)
}
fn bank_at(first: u64) -> DeviceModulation {
    let destination = ModulationDestination::Device {
        device_type: crate::DEFAULT_INSTRUMENT_TYPE_ID,
        target: DeviceAutomationTarget::HostOutputGain,
        vst3_class: None,
    };
    DeviceModulation::new(
        vec![source(first + 1), source(first)],
        vec![
            route(first + 2, first, destination),
            route(first + 3, first + 1, destination),
        ],
    )
    .unwrap()
}

fn bank() -> DeviceModulation {
    bank_at(2)
}

#[test]
fn authored_banks_survive_history_snapshots_and_reordering_without_retargeting() {
    let (mut editor, id) = fixture();
    let mut empty = editor.project().snapshot();
    let expected = bank();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: expected.clone(),
        })
        .unwrap();
    let saved = editor.project().snapshot();
    empty.next_id = saved.next_id;
    assert_eq!(
        Project::from_snapshot(saved.clone())
            .unwrap()
            .device(id)
            .unwrap()
            .modulation(),
        &expected
    );
    let mut order = expected.sources().to_vec();
    order.reverse();
    let reordered = DeviceModulation::new(order, expected.routes().to_vec()).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: reordered.clone(),
        })
        .unwrap();
    assert_eq!(
        editor.project().device(id).unwrap().modulation(),
        &reordered
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), saved);
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), empty);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), saved);
    assert!(editor.redo());
    assert_eq!(
        editor.project().device(id).unwrap().modulation(),
        &reordered
    );
    editor.execute(Command::RemoveDevice(id)).unwrap();
    assert!(editor.project().device(id).is_none());
    assert!(editor.undo());
    assert_eq!(
        editor.project().device(id).unwrap().modulation(),
        &reordered
    );
}

#[test]
fn cycles_missing_sources_stale_providers_and_invalid_values_refuse_atomically() {
    let target = |id| ModulationDestination::Modulator {
        source: ModulationSourceId::new(id).unwrap(),
        module_type: DeviceTypeId::from_static("test.modulator"),
        parameter: ParameterId::from_static(1),
    };
    assert!(DeviceModulation::new(vec![source(1)], vec![route(1, 1, target(1))]).is_err());
    assert!(
        DeviceModulation::new(
            vec![source(1), source(2)],
            vec![route(1, 1, target(2)), route(2, 2, target(1))]
        )
        .is_err()
    );
    assert!(DeviceModulation::new(vec![source(1)], vec![route(1, 1, target(2))]).is_err());
    let mut disabled = route(2, 2, target(1));
    disabled.enabled = false;
    assert!(
        DeviceModulation::new(
            vec![source(1), source(2)],
            vec![route(1, 1, target(2)), disabled]
        )
        .is_ok()
    );
    for value in [f32::NAN, f32::INFINITY, -1.01, 1.01] {
        assert!(ModulationAmount::new(value).is_err());
    }
    let (mut editor, id) = fixture();
    let original = bank();
    let mut routes = original.routes().to_vec();
    routes[0].destination = ModulationDestination::Device {
        device_type: crate::DEFAULT_INSTRUMENT_TYPE_ID,
        target: DeviceAutomationTarget::Parameter(ParameterId::from_static(1)),
        vst3_class: None,
    };
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: DeviceModulation::new(original.sources().to_vec(), routes).unwrap(),
        })
        .unwrap();
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::ReplaceDeviceState {
                device: id,
                state: NativeDeviceState::new(
                    DeviceTypeId::from_static("test.replacement"),
                    1,
                    vec![]
                )
                .unwrap()
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert!(
        editor
            .execute(Command::ReplaceDeviceModulation {
                device: DeviceId(u64::MAX),
                modulation: bank()
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn retained_instrument_has_one_canonical_modulation_bank_and_borrowed_bindings() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Device(id) = editor
        .execute(Command::InsertInstrument {
            track,
            state: crate::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument")
    };
    let expected = bank_at(editor.project().snapshot().next_id);
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: expected.clone(),
        })
        .unwrap();
    let snapshot = editor.project().snapshot();
    assert!(
        snapshot.tracks[0]
            .instrument
            .as_ref()
            .unwrap()
            .modulation
            .is_empty()
    );
    assert_eq!(snapshot.native_sources[0].devices[0].modulation, expected);
    let restored = Project::from_snapshot(snapshot).unwrap();
    assert_eq!(
        restored
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .modulation(),
        &expected
    );
    let mut invalid = restored.snapshot();
    invalid.tracks[0].instrument.as_mut().unwrap().modulation = expected;
    assert!(Project::from_snapshot(invalid).is_err());
}

#[test]
fn device_copy_and_move_preserve_local_mapping_identity_and_host_targets() {
    let (mut editor, id) = fixture();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device: id,
            modulation: bank(),
        })
        .unwrap();
    let clipboard = std::sync::Arc::new(
        editor
            .project()
            .capture_devices(
                crate::DeviceChainOwner::Main,
                &crate::DeviceSelectionIds::new(&[id]).unwrap(),
            )
            .unwrap(),
    );
    editor
        .execute(Command::PasteDevices {
            clipboard,
            owner: crate::DeviceChainOwner::Main,
            before: None,
        })
        .unwrap();
    let copied = editor.project().main_devices()[1].id();
    assert_ne!(copied, id);
    assert_eq!(
        editor.project().device(copied).unwrap().modulation(),
        &bank()
    );
    editor
        .execute(Command::MoveMainDevice {
            device: copied,
            before: Some(id),
        })
        .unwrap();
    assert_eq!(editor.project().main_devices()[0].id(), copied);
    assert_eq!(
        editor.project().device(copied).unwrap().modulation(),
        &bank()
    );
    // Intrinsic host trim targets remain meaningful across provider changes.
    editor
        .execute(Command::ReplaceDeviceState {
            device: copied,
            state: NativeDeviceState::new(DeviceTypeId::from_static("test.replacement"), 1, vec![])
                .unwrap(),
        })
        .unwrap();
    assert_eq!(
        editor.project().device(copied).unwrap().modulation(),
        &bank()
    );
    assert_eq!(
        editor.project().device(id).unwrap().state().type_id(),
        crate::DEFAULT_INSTRUMENT_TYPE_ID
    );
}

#[test]
fn per_device_and_project_capacity_refusals_preserve_the_last_valid_project() {
    assert!(
        DeviceModulation::new(Vec::with_capacity(MAX_MODULATORS_PER_DEVICE + 1), vec![]).is_err()
    );
    assert!(
        DeviceModulation::new(
            vec![],
            Vec::with_capacity(MAX_MODULATION_ROUTES_PER_DEVICE + 1)
        )
        .is_err()
    );
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
    let mut devices = Vec::new();
    for _ in 0..=MAX_PROJECT_MODULATORS / MAX_MODULATORS_PER_DEVICE {
        let Created::Device(id) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: crate::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        devices.push(id);
    }
    let full = |first| {
        DeviceModulation::new(
            (first..first + MAX_MODULATORS_PER_DEVICE as u64)
                .map(source)
                .collect(),
            vec![],
        )
        .unwrap()
    };
    for &device in &devices[..devices.len() - 1] {
        let modulation = full(editor.project().snapshot().next_id);
        editor
            .execute(Command::ReplaceDeviceModulation { device, modulation })
            .unwrap();
    }
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::ReplaceDeviceModulation {
                device: *devices.last().unwrap(),
                modulation: full(editor.project().snapshot().next_id)
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn removed_source_and_route_ids_cannot_be_reused_after_edits_or_history_branching() {
    let (mut editor, device) = fixture();
    let old = bank();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: old.clone(),
        })
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: DeviceModulation::default(),
        })
        .unwrap();
    let removed = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::ReplaceDeviceModulation {
                device,
                modulation: old
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), removed);
    let fresh = bank_at(removed.next_id);
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: fresh.clone(),
        })
        .unwrap();
    let next = editor.project().snapshot().next_id;
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot().next_id, next);
    assert!(
        editor
            .execute(Command::ReplaceDeviceModulation {
                device,
                modulation: fresh.clone()
            })
            .is_err()
    );
    assert!(editor.redo());
    assert_eq!(
        editor.project().device(device).unwrap().modulation(),
        &fresh
    );
    let mut invalid = editor.project().snapshot();
    invalid.next_id = fresh.sources()[0].id.value();
    assert!(Project::from_snapshot(invalid).is_err());
}
