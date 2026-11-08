//! Source-stamped lane edits use the ordinary worker publication and history.
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId, ParameterId};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
};
use hex_session::service::{Event, Request, Service};
use std::time::Instant;
fn event(service: &Service) -> Event {
    let until = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(1))
            }
            Err(reason) => panic!("event: {reason}"),
        }
    }
}
#[test]
fn delay_feedback_lane_edits_preserve_base_value_history_and_refuse_stale_requests() {
    for parameter in 1..=27 {
        lane_case("hex.delay", parameter);
    }
}
#[test]
fn limiter_value_and_structural_lane_edits_preserve_history_and_refuse_stale_requests() {
    for parameter in [1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12] {
        lane_case("hex.limiter", parameter);
    }
}
fn lane_case(type_id: &'static str, raw_parameter: u32) {
    let service = Service::new(48000, 16, 0).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "automation".into(),
        }))
        .unwrap();
    let Created::Track(track) = event(&service).created.unwrap() else {
        panic!("track")
    };
    service
        .submit(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(type_id))
                .unwrap(),
        }))
        .unwrap();
    let added = event(&service);
    let Created::Device(device) = added.created.unwrap() else {
        panic!("device")
    };
    let parameter = ParameterId::new(raw_parameter).unwrap();
    let points = vec![AutomationPoint {
        beat: BeatTime::ZERO,
        value: AutomationValue::new(0.8 / 0.95).unwrap(),
        outgoing: AutomationInterpolation::Step,
    }];
    let request = |revision, generation, points| Request::DeviceAutomation {
        project_generation: generation,
        revision,
        device,
        target: DeviceAutomationTarget::Parameter(parameter),
        vst3_class: None,
        points,
    };
    service
        .submit(request(added.state.revision, 0, points.clone()))
        .unwrap();
    service
        .submit(request(added.state.revision, 0, Vec::new()))
        .unwrap();
    let committed = event(&service);
    assert!(committed.error.is_none(), "{:?}", committed.error);
    assert_eq!(
        committed
            .state
            .project
            .device(device)
            .unwrap()
            .automation_lane(parameter)
            .unwrap()
            .points,
        points
    );
    assert_eq!(
        committed.state.project.device(device).unwrap().state(),
        added.state.project.device(device).unwrap().state()
    );
    let stale = event(&service);
    assert!(stale.error.unwrap().contains("project changed"));
    assert_eq!(stale.state.project, committed.state.project);
    service
        .submit(request(stale.state.revision, 1, Vec::new()))
        .unwrap();
    assert!(event(&service).error.unwrap().contains("project changed"));
    service.submit(Request::Undo).unwrap();
    assert!(
        event(&service)
            .state
            .project
            .device(device)
            .unwrap()
            .automation()
            .is_empty()
    );
    service.submit(Request::Redo).unwrap();
    let redone = event(&service);
    assert_eq!(
        redone
            .state
            .project
            .device(device)
            .unwrap()
            .automation_lane(parameter)
            .unwrap()
            .points,
        points
    );
    service
        .submit(request(redone.state.revision, 0, Vec::new()))
        .unwrap();
    let cleared = event(&service);
    assert!(cleared.error.is_none());
    assert!(
        cleared
            .state
            .project
            .device(device)
            .unwrap()
            .automation()
            .is_empty()
    );
    service.submit(Request::Undo).unwrap();
    assert_eq!(event(&service).state.project, redone.state.project);
    service
        .submit(Request::Edit(Command::RemoveDevice(device)))
        .unwrap();
    let removed = event(&service);
    service
        .submit(request(removed.state.revision, 0, points.clone()))
        .unwrap();
    let refused = event(&service);
    assert!(
        refused
            .error
            .unwrap()
            .contains("target no longer available")
    );
    assert_eq!(refused.state.project, removed.state.project);
    service.submit(Request::Undo).unwrap();
    assert_eq!(
        event(&service).state.project,
        redone.state.project,
        "refusal cannot add or consume history"
    );
}
