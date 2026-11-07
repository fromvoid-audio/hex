//! Source-stamped control edits use canonical publication/history. These actor
//! regressions do not claim callback timestamps or installed native acceptance.
use hex_lattice_contract::{
    DeviceAutomationTarget as Target, DeviceTypeId, ParameterId, ParameterKind, ParameterValue,
};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationReadSelection as Read, AutomationValue,
    BeatTime, Command, Created, DeviceId,
};
use hex_session::service::{DeviceAutomationControlEdit as Edit, Event, Request, Service};
use std::time::Instant;
fn next(service: &Service) -> Event {
    let until = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(1))
            }
            Err(error) => panic!("actor event: {error}"),
        }
    }
}
fn send(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn stamped(event: &Event, edit: Edit) -> Request {
    Request::DeviceAutomationControl {
        project_generation: 0,
        revision: event.state.revision,
        edit,
    }
}
fn effect(service: &Service, kind: &'static str) -> (DeviceId, Event) {
    let event = send(
        service,
        Request::Edit(Command::AddTrack {
            name: "read selection".into(),
        }),
    );
    let Created::Track(track) = event.created.unwrap() else {
        panic!("track")
    };
    let event = send(
        service,
        Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(kind))
                .unwrap(),
        }),
    );
    let Created::Device(device) = event.created.unwrap() else {
        panic!("device")
    };
    (device, event)
}
#[test]
fn manual_read_reenable_global_and_stale_request_keep_points_and_one_undo() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next(&service);
    let (device, mut event) = effect(&service, "hex.utility");
    let points = vec![AutomationPoint {
        beat: BeatTime::ZERO,
        value: AutomationValue::new(0.25).unwrap(),
        outgoing: AutomationInterpolation::Step,
    }];
    let target = Target::Parameter(ParameterId::from_static(1));
    event = send(
        &service,
        Request::DeviceAutomation {
            project_generation: 0,
            revision: event.state.revision,
            device,
            target,
            vst3_class: None,
            points: points.clone(),
        },
    );
    let authored = event.state.project.clone();
    // Equal base is still an explicit manual write.
    let base = hex_engine::native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Effect,
        authored.device(device).unwrap().state(),
        ParameterId::from_static(1),
    )
    .unwrap();
    event = send(
        &service,
        stamped(
            &event,
            Edit::ManualParameter {
                device,
                parameter: ParameterId::from_static(1),
                value: base,
            },
        ),
    );
    assert_eq!(
        event
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .read_selection,
        Read::Base
    );
    assert_eq!(
        event
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .points,
        points
    );
    let manual = event.state.project.clone();
    event = send(&service, Request::Undo);
    assert_eq!(event.state.project, authored);
    event = send(&service, Request::Redo);
    assert_eq!(event.state.project, manual);
    event = send(
        &service,
        stamped(
            &event,
            Edit::ReadSelection {
                device,
                target,
                vst3_class: None,
                selection: Read::Authored,
            },
        ),
    );
    assert_eq!(event.state.project, authored);
    event = send(&service, Request::Undo);
    assert_eq!(event.state.project, manual);
    let stale = stamped(&event, Edit::Reenable);
    let (_, new_event) = effect(&service, "hex.overdrive");
    event = new_event;
    service.submit(stale).unwrap();
    let refused = next(&service);
    assert!(refused.error.unwrap().contains("project changed"));
    assert_eq!(refused.state.project, event.state.project);
    event = send(&service, stamped(&event, Edit::Reenable));
    assert!(!event.state.project.has_overridden_device_automation());
    assert_eq!(
        event
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap()
            .points,
        points
    );
}
#[test]
fn stamped_base_edits_keep_nonautomatable_effects_and_retained_instruments_editable() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next(&service);
    let (effect, event) = effect(&service, "hex.overdrive");
    check_base_edit(
        &service,
        effect,
        event,
        hex_engine::NativeDeviceClass::Effect,
    );
    let event = send(
        &service,
        Request::Edit(Command::InsertTrackWithDevice {
            name: "instrument".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Instrument,
                DeviceTypeId::from_static("hex.instrument.operator"),
            )
            .unwrap(),
        }),
    );
    let device = event
        .state
        .project
        .native_sources()
        .last()
        .unwrap()
        .instrument()
        .id();
    check_base_edit(
        &service,
        device,
        event,
        hex_engine::NativeDeviceClass::Instrument,
    );
}
fn check_base_edit(
    service: &Service,
    device: DeviceId,
    event: Event,
    class: hex_engine::NativeDeviceClass,
) {
    let before = event.state.project.clone();
    let state = before.device(device).unwrap().state();
    let metadata = hex_engine::native_device_view_for_class(class, state).unwrap();
    let parameter = metadata
        .parameters
        .iter()
        .find(|p| p.enabled && matches!(p.descriptor.kind, ParameterKind::Float { .. }))
        .unwrap();
    let ParameterKind::Float { min, max, .. } = parameter.descriptor.kind else {
        panic!("float")
    };
    let value = ParameterValue::Float(if parameter.value == ParameterValue::Float(min) {
        min + (max - min) * 0.25
    } else {
        min
    });
    let changed = send(
        service,
        stamped(
            &event,
            Edit::ManualParameter {
                device,
                parameter: parameter.descriptor.id,
                value,
            },
        ),
    );
    assert_ne!(changed.state.project.device(device).unwrap().state(), state);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            class,
            changed.state.project.device(device).unwrap().state(),
            parameter.descriptor.id
        )
        .unwrap(),
        value
    );
    assert!(!changed.state.project.has_overridden_device_automation());
    let undo = send(service, Request::Undo);
    assert_eq!(undo.state.project, before);
}
