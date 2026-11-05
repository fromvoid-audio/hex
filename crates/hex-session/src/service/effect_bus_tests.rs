use super::*;
use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
use hex_engine::NativeDeviceClass;
use hex_lattice_contract::{DeviceTypeId, ParameterValue};

fn receive(service: &Service) -> Event {
    // Blocks on the event channel; bounded only against hangs.
    match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
        Ok(event) => event,
        Err(error) => panic!("receive: worker event absent: {error}"),
    }
}

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    receive(service)
}

fn insert(revision: u64, destination: EffectBusDestination) -> Request {
    Request::InsertEffectBusDevice {
        project_generation: 0,
        revision,
        destination,
        class: NativeDeviceClass::Effect,
        type_id: DeviceTypeId::from_static("hex.utility"),
        before: None,
    }
}

#[test]
fn effect_bus_worker_main_and_return_use_selected_defaults_and_one_undo_redo() {
    let service = Service::new(48_000, 32, 0).unwrap();
    receive(&service);
    let initial = run(
        &service,
        Request::Edit(Command::AddReturnTrack {
            name: "return".into(),
        }),
    );
    let return_id = initial.state.project.return_tracks()[0].id();
    for destination in [
        EffectBusDestination::Main,
        EffectBusDestination::Return(return_id),
    ] {
        let before = run(
            &service,
            Request::Edit(Command::AddTrack {
                name: "empty".into(),
            }),
        )
        .state;
        let added = run(&service, insert(before.revision, destination));
        assert!(added.error.is_none(), "{:?}", added.error);
        let device = match destination {
            EffectBusDestination::Main => &added.state.project.main_devices()[0],
            EffectBusDestination::Return(_) => &added.state.project.return_tracks()[0].devices()[0],
        };
        assert_eq!(device.id().value(), before.project.next_identity());
        assert!(matches!(added.created, Some(Created::Device(id)) if id == device.id()));
        assert_eq!(
            device.state(),
            &hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Effect,
                DeviceTypeId::from_static("hex.utility")
            )
            .unwrap()
        );
        assert!(added.playback.is_some());
        assert!(
            added
                .state
                .project
                .tracks()
                .iter()
                .all(|track| track.ordered_device_chain().next().is_none())
        );
        let undone = run(&service, Request::Undo);
        let mut expected = before.project.snapshot();
        expected.next_id = added.state.project.next_identity();
        assert!(expected.next_id > before.project.next_identity());
        assert_eq!(undone.state.project.snapshot(), expected);
        let redone = run(&service, Request::Redo);
        assert_eq!(redone.state.project, added.state.project);
        run(&service, Request::Undo);
    }
}

#[test]
fn effect_bus_worker_invalid_identity_target_anchor_and_stamps_are_atomic() {
    let service = Service::new(48_000, 32, 0).unwrap();
    receive(&service);
    let returned = run(
        &service,
        Request::Edit(Command::AddReturnTrack {
            name: "removed".into(),
        }),
    );
    let missing = returned.state.project.return_tracks()[0].id();
    run(&service, Request::Edit(Command::RemoveReturnTrack(missing)));
    let returned = run(
        &service,
        Request::Edit(Command::AddReturnTrack {
            name: "live".into(),
        }),
    );
    let live = returned.state.project.return_tracks()[0].id();
    let before = run(
        &service,
        insert(returned.state.revision, EffectBusDestination::Return(live)),
    )
    .state;
    let foreign = before.project.return_tracks()[0].devices()[0].id();
    for (generation, revision, destination, class, kind, anchor) in [
        (
            0,
            before.revision,
            EffectBusDestination::Main,
            NativeDeviceClass::Instrument,
            "hex.instrument.sine",
            None,
        ),
        (
            0,
            before.revision,
            EffectBusDestination::Main,
            NativeDeviceClass::Effect,
            "hex.instrument.sine",
            None,
        ),
        (
            0,
            before.revision,
            EffectBusDestination::Main,
            NativeDeviceClass::Effect,
            "missing.effect",
            None,
        ),
        (
            1,
            before.revision,
            EffectBusDestination::Main,
            NativeDeviceClass::Effect,
            "hex.utility",
            None,
        ),
        (
            0,
            before.revision + 1,
            EffectBusDestination::Main,
            NativeDeviceClass::Effect,
            "hex.utility",
            None,
        ),
        (
            0,
            before.revision,
            EffectBusDestination::Return(missing),
            NativeDeviceClass::Effect,
            "hex.utility",
            None,
        ),
        (
            0,
            before.revision,
            EffectBusDestination::Main,
            NativeDeviceClass::Effect,
            "hex.utility",
            Some(foreign),
        ),
    ] {
        let refused = run(
            &service,
            Request::InsertEffectBusDevice {
                project_generation: generation,
                revision,
                destination,
                class,
                type_id: DeviceTypeId::from_static(kind),
                before: anchor,
            },
        );
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, before.project);
        assert_eq!(
            refused.state.project.next_identity(),
            before.project.next_identity()
        );
        assert_eq!(refused.state.revision, before.revision);
        assert_eq!(refused.state.unsaved, before.unsaved);
        assert_eq!(refused.state.can_undo, before.can_undo);
        assert_eq!(refused.state.can_redo, before.can_redo);
        assert!(refused.created.is_none());
        assert!(refused.playback.is_none());
        assert!(Arc::ptr_eq(
            refused.state.canonical_audio_source.as_ref().unwrap(),
            before.canonical_audio_source.as_ref().unwrap()
        ));
    }
    let added = run(
        &service,
        insert(before.revision, EffectBusDestination::Main),
    );
    assert!(added.error.is_none());
    assert_eq!(
        added.state.project.main_devices()[0].id().value(),
        before.project.next_identity()
    );
}

#[test]
fn effect_bus_worker_refusal_does_not_finalize_an_unrelated_live_gesture() {
    let service = Service::new(48_000, 32, 0).unwrap();
    receive(&service);
    let before = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "empty".into(),
        }),
    )
    .state;
    let key = GestureKey {
        generation: 0,
        token: 91,
    };
    let begun = run(
        &service,
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain {
                track: before.project.tracks()[0].id(),
            },
        }),
    );
    assert!(begun.gesture_ack.unwrap().accepted);
    let changed = run(
        &service,
        Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.25),
        }),
    );
    assert!(changed.gesture_ack.unwrap().accepted);
    for revision in [before.revision, before.revision + 1] {
        let refused = run(&service, insert(revision, EffectBusDestination::Main));
        assert!(refused.error.is_some());
        assert!(refused.gesture_ack.is_none());
        assert!(refused.active_device_gesture.is_some());
        assert_eq!(refused.state.project, before.project);
        assert_eq!(refused.state.revision, before.revision);
        assert_eq!(refused.state.can_undo, before.can_undo);
        assert_eq!(refused.state.unsaved, before.unsaved);
        assert!(refused.created.is_none());
        assert!(refused.playback.is_none());
    }
    let cancelled = run(
        &service,
        Request::DeviceGesture(GestureRequest::Cancel { key }),
    );
    assert!(cancelled.gesture_ack.unwrap().accepted);
    assert_eq!(cancelled.state.project, before.project);
    let added = run(
        &service,
        insert(cancelled.state.revision, EffectBusDestination::Main),
    );
    assert!(added.error.is_none());
    let mut expected = before.project.snapshot();
    expected.next_id = added.state.project.next_identity();
    assert!(expected.next_id > before.project.next_identity());
    assert_eq!(
        run(&service, Request::Undo).state.project.snapshot(),
        expected
    );
}

#[test]
fn overdrive_track_return_main_parameters_history_and_file_roundtrip() {
    use hex_lattice_contract::ParameterId;
    let directory = super::service_tests::TestDir::new();
    let path = directory.0.join("overdrive.hex");
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    receive(&service);
    let added = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "overdrive".into(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let added = run(
        &service,
        Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(DeviceTypeId::from_static("hex.overdrive")),
            before: None,
        },
    );
    assert!(added.error.is_none(), "{:?}", added.error);
    let track_device = added.state.project.tracks()[0].devices()[0].id();
    let added = run(
        &service,
        Request::Edit(Command::AddReturnTrack {
            name: "overdrive return".into(),
        }),
    );
    let return_id = added.state.project.return_tracks()[0].id();
    let mut devices = vec![track_device];
    let mut revision = added.state.revision;
    for destination in [
        EffectBusDestination::Return(return_id),
        EffectBusDestination::Main,
    ] {
        let added = run(
            &service,
            Request::InsertEffectBusDevice {
                project_generation: 0,
                revision,
                destination,
                class: NativeDeviceClass::Effect,
                type_id: DeviceTypeId::from_static("hex.overdrive"),
                before: None,
            },
        );
        assert!(added.error.is_none(), "{:?}", added.error);
        let Some(Created::Device(device)) = added.created else {
            panic!("created device");
        };
        revision = added.state.revision;
        devices.push(device);
    }
    let mut last = None;
    for device in devices {
        for (id, value) in [
            (1, 2_400.0),
            (2, 3.0),
            (3, 0.9),
            (4, 0.2),
            (5, 0.7),
            (6, 0.8),
        ] {
            let changed = run(
                &service,
                Request::SetDeviceParameter {
                    device,
                    parameter: ParameterId::from_static(id),
                    value: ParameterValue::Float(value),
                },
            );
            assert!(changed.error.is_none(), "{:?}", changed.error);
            assert!(changed.playback.is_some());
            assert_eq!(
                hex_engine::native_device_parameter(
                    changed.state.project.device(device).unwrap().state(),
                    ParameterId::from_static(id)
                )
                .unwrap(),
                ParameterValue::Float(value)
            );
            last = Some(changed.state.project.clone());
        }
        let bypassed = run(
            &service,
            Request::Edit(Command::SetDeviceBypass {
                device,
                bypassed: true,
            }),
        );
        assert!(bypassed.error.is_none(), "{:?}", bypassed.error);
        let undone = run(&service, Request::Undo);
        assert_eq!(undone.state.project, last.clone().unwrap());
        let redone = run(&service, Request::Redo);
        assert_eq!(redone.state.project, bypassed.state.project);
        last = Some(redone.state.project.clone());
    }
    let saved = run(&service, Request::SaveNew(path.clone()));
    assert!(saved.error.is_none(), "{:?}", saved.error);
    let opened = run(&service, Request::Open(path));
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert_eq!(opened.state.project, last.unwrap());
    assert!(opened.playback.is_some());
}
