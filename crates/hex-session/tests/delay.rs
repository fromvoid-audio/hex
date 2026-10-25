use hex_engine::{NativeDeviceClass, default_native_device_state, patch_native_device_parameter};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Command, Created};
use hex_session::Session;
#[test]
fn delay_canonical_track_return_edit_bypass_reorder_history_and_save_reopen() {
    let mut session = Session::new(48000, 32, 0).unwrap();
    let Created::Track(track) = session
        .command(Command::AddTrack {
            name: "delay".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let state = default_native_device_state(DeviceTypeId::from_static("hex.delay")).unwrap();
    let Created::Device(first) = session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("delay")
    };
    let Created::Device(second) = session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("second")
    };
    let Created::ReturnTrack(bus) = session
        .command(Command::AddReturnTrack {
            name: "echoes".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    session
        .command(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let Created::Device(ret) = session
        .command(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("return delay")
    };
    for device in [first, ret] {
        for (id, value) in [(1, 765.5), (2, 0.75), (3, 0.9)] {
            let before = session.project().clone();
            session
                .commit_device_parameter(
                    device,
                    ParameterId::from_static(id),
                    ParameterValue::Float(value),
                    None,
                )
                .unwrap();
            let after = session.project().clone();
            assert_ne!(after, before);
            assert!(session.undo());
            assert_eq!(session.project(), &before);
            assert!(session.redo());
            assert_eq!(session.project(), &after);
        }
        // Exercise every new saved control through canonical history, including
        // linked timing side effects and the independently stored offsets.
        for descriptor in hex_engine::native_device_view(&state).unwrap().parameters {
            use hex_lattice_contract::ParameterKind;
            let value = match descriptor.descriptor.kind {
                ParameterKind::Float { min, max, .. } => {
                    ParameterValue::Float(min + (max - min) * 0.37)
                }
                ParameterKind::Bool => ParameterValue::Bool(false),
                ParameterKind::Choice { labels } => {
                    ParameterValue::Choice((labels.len() - 1) as u32)
                }
            };
            let before = session.project().clone();
            session
                .commit_device_parameter(device, descriptor.descriptor.id, value, None)
                .unwrap();
            let after = session.project().clone();
            if before != after {
                assert!(session.undo());
                assert_eq!(session.project(), &before);
                assert!(session.redo());
                assert_eq!(session.project(), &after);
            }
        }
        session
            .command(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
    }
    session
        .command(Command::MoveDevice {
            device: second,
            track,
            before: Some(first),
        })
        .unwrap();
    assert_eq!(
        session.project().track(track).unwrap().devices()[0].id(),
        second
    );
    let before = session.project().clone();
    session.command(Command::RemoveDevice(first)).unwrap();
    assert!(session.project().device(first).is_none());
    assert!(session.undo());
    assert_eq!(session.project(), &before);
    let bytes = hex_storage::encode(session.project()).unwrap();
    let container: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        container["version"].as_u64(),
        Some(u64::from(hex_storage::CURRENT_SCHEMA_VERSION)),
        "delay uses the current container without adding saved container fields"
    );
    let reopened = hex_storage::decode(&bytes).unwrap();
    assert_eq!(&reopened, session.project());
    assert_eq!(
        reopened.device(ret).unwrap().state(),
        session.project().device(ret).unwrap().state()
    );
    let view = hex_engine::native_device_view_for_class(
        NativeDeviceClass::Effect,
        reopened.device(ret).unwrap().state(),
    )
    .unwrap();
    assert_eq!(view.name, "delay");
    assert_eq!(view.parameters.len(), 27);
    for value in [f32::NAN, f32::INFINITY, -1.0, 5001.0] {
        assert!(
            patch_native_device_parameter(
                &state,
                ParameterId::from_static(1),
                ParameterValue::Float(value)
            )
            .is_err()
        );
        assert_eq!(session.project(), &reopened);
    }
}
