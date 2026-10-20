use hex_engine::NativeDeviceClass;
use hex_lattice_contract::NativeDeviceState;
use hex_project::{Command, Created, DeviceComparison, DeviceVariant, Project};
use hex_session::Session;

fn fixture() -> (Session, hex_project::DeviceId) {
    let mut session = Session::new(48_000, 16, 0).unwrap();
    let Created::Track(track) = session
        .command(Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    (session, device)
}
#[test]
fn malformed_inactive_provider_refuses_before_preparation_and_preserves_history() {
    let (mut session, device) = fixture();
    let before = session.project().clone();
    let state = before.device(device).unwrap().state().clone();
    let invalid =
        NativeDeviceState::new(state.type_id(), u32::MAX, state.payload().to_vec()).unwrap();
    assert!(
        session
            .command(Command::ReplaceDeviceComparison {
                device,
                state: state.clone(),
                comparison: Some(DeviceComparison::new(DeviceVariant::A, invalid.clone())),
            })
            .is_err(),
        "inactive state must not bypass registered provider validation"
    );
    assert_eq!(session.project(), &before);
    assert!(session.undo());
    assert!(
        session.project().tracks().is_empty(),
        "refusal must not add an Undo"
    );
    assert!(session.redo());
    assert_eq!(session.project(), &before);
}
#[test]
fn class_specific_pair_refusal_does_not_accept_effect_payload_as_instrument() {
    let (mut session, device) = fixture();
    let before = session.project().clone();
    let wrong = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    assert!(
        session
            .command(Command::ReplaceDeviceComparison {
                device,
                state: wrong.clone(),
                comparison: Some(DeviceComparison::new(DeviceVariant::B, wrong)),
            })
            .is_err()
    );
    assert_eq!(session.project(), &before);
}

#[test]
fn malformed_current_format_inactive_provider_refuses_new_session() {
    let (session, device) = fixture();
    let before = session.project().clone();
    let state = before.device(device).unwrap().state();
    let invalid =
        NativeDeviceState::new(state.type_id(), u32::MAX, state.payload().to_vec()).unwrap();
    let mut snapshot = before.snapshot();
    snapshot.tracks[0].instrument.as_mut().unwrap().comparison =
        Some(DeviceComparison::new(DeviceVariant::A, invalid));
    let malformed = Project::from_snapshot(snapshot).unwrap();
    let bytes = hex_storage::encode(&malformed).unwrap();
    let decoded = hex_storage::decode(&bytes).unwrap();
    assert!(
        Session::from_prepared(decoded, 16, 0, vec![]).is_err(),
        "opening the current-format malformed pair must refuse before publication"
    );
}
