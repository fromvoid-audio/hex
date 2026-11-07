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

#[test]
fn provider_and_host_targets_coexist_clear_undo_and_save_reopen_without_base_edits() {
    use hex_lattice_contract::HostGain;
    let service = Service::new(48000, 32, 0).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "coexist".into(),
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
    let mut current = event(&service);
    let Created::Device(device) = current.created.unwrap() else {
        panic!("device")
    };
    let provider = DeviceAutomationTarget::Parameter(ParameterId::from_static(1));
    for (target, value) in [
        (provider, 0.25),
        (
            DeviceAutomationTarget::HostInputGain,
            HostGain::from_db(-6.0).unwrap().normalized(),
        ),
        (
            DeviceAutomationTarget::HostOutputGain,
            HostGain::from_db(3.0).unwrap().normalized(),
        ),
    ] {
        service
            .submit(Request::DeviceAutomation {
                project_generation: 0,
                revision: current.state.revision,
                device,
                target,
                vst3_class: None,
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(value).unwrap(),
                    outgoing: AutomationInterpolation::Linear,
                }],
            })
            .unwrap();
        current = event(&service);
        assert!(current.error.is_none(), "{:?}", current.error);
    }
    let authored = current.state.project.clone();
    assert_eq!(authored.device(device).unwrap().automation().len(), 3);
    service
        .submit(Request::DeviceAutomation {
            project_generation: 0,
            revision: current.state.revision,
            device,
            target: DeviceAutomationTarget::HostInputGain,
            vst3_class: None,
            points: Vec::new(),
        })
        .unwrap();
    let cleared = event(&service);
    assert!(cleared.error.is_none());
    assert_eq!(
        cleared
            .state
            .project
            .device(device)
            .unwrap()
            .automation()
            .len(),
        2
    );
    service.submit(Request::Undo).unwrap();
    current = event(&service);
    assert_eq!(current.state.project, authored);
    let path = std::env::temp_dir().join(format!(
        "hex-host-lanes-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    service.submit(Request::SaveNew(path.clone())).unwrap();
    let saved = event(&service);
    assert!(saved.save_complete && saved.error.is_none());
    let reopen = Service::new(48000, 16, 0).unwrap();
    event(&reopen);
    reopen.submit(Request::Open(path.clone())).unwrap();
    let reopened = event(&reopen);
    assert!(
        reopened.project_replaced && reopened.error.is_none(),
        "{:?}",
        reopened.error
    );
    let actual = reopened.state.project.device(device).unwrap();
    assert_eq!(
        actual.automation(),
        authored.device(device).unwrap().automation()
    );
    assert_eq!(actual.state().input_gain_millidb(), 0);
    assert_eq!(actual.state().output_gain_millidb(), 0);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn actual_source_owner_and_valid_foreign_envelope_do_not_gain_native_host_capability() {
    let service = Service::new(48000, 16, 0).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "source".into(),
            devices: vec![
                hex_project::default_instrument_state().unwrap(),
                hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                    .unwrap(),
            ],
            instrument_index: 0,
        }))
        .unwrap();
    let source = event(&service);
    assert!(source.error.is_none());
    let device = source
        .state
        .project
        .native_sources()
        .iter()
        .flat_map(|source| source.devices())
        .find(|device| device.state().type_id() == DeviceTypeId::from_static("hex.utility"))
        .unwrap()
        .id();
    service
        .submit(Request::DeviceAutomation {
            project_generation: 0,
            revision: source.state.revision,
            device,
            target: DeviceAutomationTarget::HostInputGain,
            vst3_class: None,
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Linear,
            }],
        })
        .unwrap();
    let refused = event(&service);
    assert!(
        refused
            .error
            .unwrap()
            .contains("target no longer available")
    );
    assert_eq!(refused.state.project, source.state.project);
    let foreign = hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [2; 32],
                info_sha256: [3; 32],
            },
        },
        component: Some(&[]),
        controller: Some(&[]),
    })
    .unwrap();
    assert!(hex_plugin_state::decode(&foreign).is_ok());
    let mut model = hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
    let Created::Device(foreign) = model
        .execute(Command::InsertMainDevice {
            before: None,
            state: foreign,
        })
        .unwrap()
    else {
        panic!("device")
    };
    for target in [
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ] {
        assert!(!hex_engine::project_device_automation_target_supported(
            model.project(),
            foreign,
            target
        ));
    }
}
