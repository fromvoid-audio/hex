use super::service_tests::{TestDir, next_event};
use super::*;
use hex_engine::NativeDeviceClass;
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

const LIMITER: DeviceTypeId = DeviceTypeId::from_static("hex.limiter");
const LINK: ParameterId = ParameterId::from_static(5);
const LOOKAHEAD: ParameterId = ParameterId::from_static(6);
const ROUTING: ParameterId = ParameterId::from_static(7);
const LINK_MID_SIDE: ParameterId = ParameterId::from_static(12);

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).expect("submit Limiter request");
    next_event(service)
}

fn insert_bus(service: &Service, state: &ViewState, destination: EffectBusDestination) -> Event {
    run(
        service,
        Request::InsertEffectBusDevice {
            // This fixture inserts buses before any project replacement.
            project_generation: 0,
            revision: state.revision,
            destination,
            class: NativeDeviceClass::Effect,
            type_id: LIMITER,
            before: None,
        },
    )
}

fn set_parameter(
    service: &Service,
    device: hex_project::DeviceId,
    parameter: ParameterId,
    value: ParameterValue,
) -> Event {
    run(
        service,
        Request::SetDeviceParameter {
            device,
            parameter,
            value,
        },
    )
}

fn value(device: &hex_project::Device, parameter: ParameterId) -> ParameterValue {
    hex_engine::native_device_parameter_for_class(
        NativeDeviceClass::Effect,
        device.state(),
        parameter,
    )
    .expect("read Limiter parameter")
}

#[test]
fn limiter_uses_track_main_and_return_chains_and_round_trips_inactive_state() {
    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    let track_event = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "limiter track".into(),
        }),
    );
    assert!(track_event.error.is_none(), "{:?}", track_event.error);
    let track = track_event.state.project.tracks()[0].id();
    let track_state =
        hex_engine::default_native_device_state_for_class(NativeDeviceClass::Effect, LIMITER)
            .expect("Limiter defaults");
    let track_event = run(
        &service,
        Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: track_state,
        }),
    );
    assert!(track_event.error.is_none(), "{:?}", track_event.error);
    let track_limiter = track_event.state.project.track(track).unwrap().devices()[0].id();
    assert_eq!(
        track_event
            .state
            .project
            .device(track_limiter)
            .unwrap()
            .state()
            .type_id(),
        LIMITER
    );
    assert_eq!(
        track_event
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .unwrap()
            .arrangement()
            .main_latency_frames(),
        72
    );

    let main_event = insert_bus(&service, &track_event.state, EffectBusDestination::Main);
    assert!(main_event.error.is_none(), "{:?}", main_event.error);
    let main_limiter = main_event.state.project.main_devices()[0].id();
    let main_state = main_event
        .state
        .project
        .device(main_limiter)
        .unwrap()
        .state();
    assert_eq!(main_state.type_id(), LIMITER);
    assert_eq!(
        main_event
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .unwrap()
            .arrangement()
            .main_latency_frames(),
        144
    );

    let return_event = run(
        &service,
        Request::Edit(Command::AddReturnTrack {
            name: "limiter return".into(),
        }),
    );
    assert!(return_event.error.is_none(), "{:?}", return_event.error);
    let return_id = return_event.state.project.return_tracks()[0].id();
    let return_event = insert_bus(
        &service,
        &return_event.state,
        EffectBusDestination::Return(return_id),
    );
    assert!(return_event.error.is_none(), "{:?}", return_event.error);
    let return_limiter = return_event.state.project.return_tracks()[0].devices()[0].id();
    assert_eq!(
        return_event
            .state
            .project
            .device(return_limiter)
            .unwrap()
            .state()
            .type_id(),
        LIMITER
    );

    let return_event = set_parameter(
        &service,
        return_limiter,
        LOOKAHEAD,
        ParameterValue::Choice(1),
    );
    assert!(return_event.error.is_none(), "{:?}", return_event.error);
    let main_event = set_parameter(&service, main_limiter, LOOKAHEAD, ParameterValue::Choice(2));
    assert!(main_event.error.is_none(), "{:?}", main_event.error);
    assert_eq!(
        main_event
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .unwrap()
            .arrangement()
            .main_latency_frames(),
        432
    );

    let main_event = set_parameter(&service, main_limiter, LINK, ParameterValue::Float(0.375));
    assert!(main_event.error.is_none(), "{:?}", main_event.error);
    let main_event = set_parameter(&service, main_limiter, ROUTING, ParameterValue::Choice(1));
    assert!(main_event.error.is_none(), "{:?}", main_event.error);
    let main_event = set_parameter(
        &service,
        main_limiter,
        LINK_MID_SIDE,
        ParameterValue::Float(0.625),
    );
    assert!(main_event.error.is_none(), "{:?}", main_event.error);
    let main_event = set_parameter(&service, main_limiter, ROUTING, ParameterValue::Choice(0));
    assert!(main_event.error.is_none(), "{:?}", main_event.error);
    let main_device = main_event.state.project.device(main_limiter).unwrap();
    assert_eq!(value(main_device, LINK), ParameterValue::Float(0.375));
    assert_eq!(
        value(main_device, LINK_MID_SIDE),
        ParameterValue::Float(0.625)
    );
    assert_eq!(value(main_device, ROUTING), ParameterValue::Choice(0));

    let undone = run(&service, Request::Undo);
    assert_eq!(
        value(undone.state.project.device(main_limiter).unwrap(), ROUTING),
        ParameterValue::Choice(1)
    );
    let redone = run(&service, Request::Redo);
    assert_eq!(
        value(redone.state.project.device(main_limiter).unwrap(), ROUTING),
        ParameterValue::Choice(0)
    );
    assert_eq!(
        value(
            redone.state.project.device(main_limiter).unwrap(),
            LINK_MID_SIDE
        ),
        ParameterValue::Float(0.625)
    );

    let directory = TestDir::new();
    let path = directory.0.join("limiter.hex");
    let saved = run(&service, Request::SaveNew(path.clone()));
    assert!(saved.error.is_none(), "{:?}", saved.error);
    let reopened_service = Service::new(48_000, 16, 0).expect("reopen service");
    next_event(&reopened_service);
    let reopened = run(&reopened_service, Request::Open(path));
    assert!(reopened.error.is_none(), "{:?}", reopened.error);
    assert_eq!(
        value(reopened.state.project.device(main_limiter).unwrap(), LINK),
        ParameterValue::Float(0.375)
    );
    assert_eq!(
        value(
            reopened.state.project.device(main_limiter).unwrap(),
            LINK_MID_SIDE,
        ),
        ParameterValue::Float(0.625)
    );
    assert_eq!(
        value(
            reopened.state.project.device(main_limiter).unwrap(),
            ROUTING
        ),
        ParameterValue::Choice(0)
    );
    assert_eq!(
        value(
            reopened.state.project.device(return_limiter).unwrap(),
            LOOKAHEAD
        ),
        ParameterValue::Choice(1)
    );
    assert_eq!(
        value(
            reopened.state.project.device(track_limiter).unwrap(),
            LOOKAHEAD
        ),
        ParameterValue::Choice(0)
    );
    assert_eq!(
        reopened
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .unwrap()
            .arrangement()
            .main_latency_frames(),
        432
    );

    drop(service);
    drop(reopened_service);
}
