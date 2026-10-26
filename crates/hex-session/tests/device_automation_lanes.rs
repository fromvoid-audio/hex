//! Source-stamped lane edits use the ordinary worker publication and history.
use hex_lattice_contract::{DeviceTypeId, ParameterId};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
};
use hex_session::service::{Event, Request, Service};
use std::{
    thread,
    time::{Duration, Instant},
};
fn event(service: &Service) -> Event {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < until => {
                thread::yield_now()
            }
            Err(reason) => panic!("event: {reason}"),
        }
    }
}
#[test]
fn stamped_lane_edits_preserve_base_value_and_refuse_stale_queued_requests() {
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
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        }))
        .unwrap();
    let added = event(&service);
    let Created::Device(device) = added.created.unwrap() else {
        panic!("device")
    };
    let parameter = ParameterId::new(1).unwrap();
    let points = vec![AutomationPoint {
        beat: BeatTime::ZERO,
        value: AutomationValue::new(0.25).unwrap(),
        outgoing: AutomationInterpolation::Linear,
    }];
    let request = |revision, generation, points| Request::DeviceAutomation {
        project_generation: generation,
        revision,
        device,
        parameter,
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
            .contains("parameter no longer available")
    );
    assert_eq!(refused.state.project, removed.state.project);
    service.submit(Request::Undo).unwrap();
    assert_eq!(
        event(&service).state.project,
        redone.state.project,
        "refusal cannot add or consume history"
    );
}
