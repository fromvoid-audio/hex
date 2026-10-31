//! Rattle is an additive native device type: it inserts through canonical
//! commands, automates drive and round-trips in the current saved container.
use hex_engine::{NativeDeviceClass, default_native_device_state};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
};
use hex_session::Session;

const DRIVE: ParameterId = ParameterId::from_static(1);

#[test]
fn rattle_drive_edit_automation_history_and_save_reopen() {
    let mut session = Session::new(48_000, 32, 0).unwrap();
    let Created::Track(track) = session
        .command(Command::AddTrack {
            name: "drums".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let state = default_native_device_state(DeviceTypeId::from_static("hex.rattle")).unwrap();
    let Created::Device(device) = session
        .command(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("rattle")
    };
    let before = session.project().clone();
    session
        .commit_device_parameter(device, DRIVE, ParameterValue::Float(0.75), None)
        .unwrap();
    let edited = session.project().clone();
    assert_ne!(edited, before);
    assert!(session.undo());
    assert_eq!(session.project(), &before);
    assert!(session.redo());
    assert_eq!(session.project(), &edited);

    session
        .command(Command::ReplaceDeviceAutomation {
            device,
            parameter: DRIVE,
            points: vec![
                AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(0.0).unwrap(),
                    outgoing: AutomationInterpolation::Linear,
                },
                AutomationPoint {
                    beat: BeatTime::from_quarters(4).unwrap(),
                    value: AutomationValue::new(1.0).unwrap(),
                    outgoing: AutomationInterpolation::Linear,
                },
            ],
        })
        .unwrap();
    assert_eq!(
        session.project().device(device).unwrap().automation().len(),
        1
    );

    let bytes = hex_storage::encode(session.project()).unwrap();
    let container: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        container["version"].as_u64(),
        Some(u64::from(hex_storage::CURRENT_SCHEMA_VERSION)),
        "rattle uses the current container without adding saved container fields"
    );
    let reopened = hex_storage::decode(&bytes).unwrap();
    assert_eq!(&reopened, session.project());
    let saved = reopened.device(device).unwrap();
    assert_eq!(saved.automation().len(), 1);
    assert_eq!(
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            saved.state(),
            DRIVE
        ),
        Ok(ParameterValue::Float(0.75))
    );
    let view =
        hex_engine::native_device_view_for_class(NativeDeviceClass::Effect, saved.state()).unwrap();
    assert_eq!(view.name, "rattle");
    assert_eq!(view.parameters.len(), 1);
}
