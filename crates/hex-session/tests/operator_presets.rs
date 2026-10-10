use hex_engine::NativeDeviceClass;
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Command, Created};
use hex_session::Session;

#[test]
fn operator_preset_is_one_canonical_edit_with_exact_undo_redo() {
    let mut session = Session::new(48_000, 16, 0).expect("session");
    let (Created::Track(track), _) = session
        .command_prepared_revision(
            Command::AddMidiTrack {
                name: "operator".into(),
            },
            None,
        )
        .expect("track")
    else {
        panic!("expected track")
    };
    let device = session
        .project()
        .track(track)
        .expect("track")
        .instrument()
        .expect("instrument")
        .id();
    let original = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .expect("operator defaults");
    let (_, prepared) = session
        .command_prepared_revision(
            Command::ReplaceDeviceState {
                device,
                state: original.clone(),
            },
            None,
        )
        .expect("operator");
    let preset = ParameterId::from_static(61);
    let changed = session
        .commit_device_parameter(device, preset, ParameterValue::Choice(3), Some(&prepared))
        .expect("two pairs preset");
    let selected = session
        .project()
        .device(device)
        .expect("device")
        .state()
        .clone();
    assert_ne!(selected, original);
    assert_eq!(selected.payload().len(), 240);
    let undone = session
        .undo_prepared_revision(Some(&changed))
        .expect("undo")
        .expect("one edit");
    assert_eq!(
        session.project().device(device).expect("device").state(),
        &original
    );
    session
        .redo_prepared_revision(Some(&undone))
        .expect("redo")
        .expect("one edit");
    assert_eq!(
        session.project().device(device).expect("device").state(),
        &selected
    );
}
