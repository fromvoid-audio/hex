//! An unprepared device envelope must not dirty canonical state or take an Undo
//! entry away from the last audible edit.
use hex_lattice_contract::{DeviceTypeId, ParameterId};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
};
use hex_session::Session;

#[test]
fn unsupported_envelope_refuses_before_canonical_commit_and_preserves_history() {
    let mut session = Session::new(48_000, 16, 0).unwrap();
    let Created::Track(track) = session
        .command(Command::AddTrack {
            name: "track".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Device(device) = session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.compressor",
            ))
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("device");
    };
    let before = session.project().snapshot();
    let error = session
        .command(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::new(8).unwrap(),
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.25).unwrap(),
                outgoing: AutomationInterpolation::Linear,
            }],
        })
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains(hex_engine::DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED)
    );
    assert_eq!(session.project().snapshot(), before);
    assert!(session.undo());
    assert!(session.project().device(device).is_none());
    assert!(session.redo());
    assert_eq!(session.project().snapshot(), before);
}
