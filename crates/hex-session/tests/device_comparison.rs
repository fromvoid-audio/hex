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
    snapshot.native_sources[0].devices[0].comparison = snapshot.tracks[0]
        .instrument
        .as_ref()
        .unwrap()
        .comparison
        .clone();
    let malformed = Project::from_snapshot(snapshot).unwrap();
    let bytes = hex_storage::encode(&malformed).unwrap();
    let decoded = hex_storage::decode(&bytes).unwrap();
    assert!(
        Session::from_prepared(decoded, 16, 0, vec![]).is_err(),
        "opening the current-format malformed pair must refuse before publication"
    );
}

fn main_fixture() -> (Session, hex_project::DeviceId) {
    let mut session = Session::new(48_000, 16, 0).unwrap();
    session
        .command(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Effect,
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        })
        .unwrap();
    let id = session.project().main_devices()[0].id();
    (session, id)
}

#[test]
fn main_comparison_malformed_inactive_refuses_atomically_and_keeps_history() {
    let (mut session, device) = main_fixture();
    let before = session.project().clone();
    let state = before.device(device).unwrap().state().clone();
    let invalid =
        NativeDeviceState::new(state.type_id(), u32::MAX, state.payload().to_vec()).unwrap();
    assert!(
        session
            .command(Command::ReplaceDeviceComparison {
                device,
                state,
                comparison: Some(DeviceComparison::new(DeviceVariant::A, invalid)),
            })
            .is_err(),
        "Main inactive provider must obey the same validation as Track/Return"
    );
    assert_eq!(session.project(), &before);
    assert!(session.undo());
    assert!(
        session.project().main_devices().is_empty(),
        "refusal must not add history"
    );
    assert!(session.redo());
    assert_eq!(session.project(), &before);
}

#[test]
fn main_comparison_saved_malformed_inactive_refuses_session_open() {
    let (session, device) = main_fixture();
    let before = session.project().clone();
    let state = before.device(device).unwrap().state();
    let invalid =
        NativeDeviceState::new(state.type_id(), u32::MAX, state.payload().to_vec()).unwrap();
    let mut snapshot = before.snapshot();
    snapshot.main_devices[0].comparison = Some(DeviceComparison::new(DeviceVariant::A, invalid));
    let malformed = Project::from_snapshot(snapshot).unwrap();
    // Actual create-new and load, not an in-memory lookalike of the codec.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "hex-main-comparison-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    // Own a fresh directory before arming cleanup; a create-new collision must
    // never remove a file from a previous process or another actor.
    std::fs::create_dir(&directory).unwrap();
    struct Saved(std::path::PathBuf);
    impl Drop for Saved {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let saved = Saved(directory);
    let path = saved.0.join("main.hex");
    hex_storage::save_new(&path, &malformed).unwrap();
    let original_bytes = std::fs::read(&path).unwrap();
    let decoded = hex_storage::load(&path).unwrap();
    assert_eq!(decoded, malformed);
    assert!(
        Session::from_prepared(decoded, 16, 0, vec![]).is_err(),
        "valid active Main state must not conceal malformed inactive saved payload"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original_bytes,
        "refusal must preserve original project bytes"
    );
}

#[test]
fn main_comparison_switch_copy_both_directions_roundtrip_and_undo_keep_global_trims() {
    use hex_project::{DeviceComparisonAction as Action, DeviceVariant as Slot};
    let (mut session, device) = main_fixture();
    session
        .command(Command::CompareDevice {
            device,
            expected: Slot::A,
            action: Action::Switch,
        })
        .unwrap();
    let original_b = session.project().device(device).unwrap().state().clone();
    let changed_b = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Effect,
        &original_b,
        hex_lattice_contract::ParameterId::from_static(1),
        hex_lattice_contract::ParameterValue::Float(0.25),
    )
    .unwrap()
    .with_input_gain_db(-3.0)
    .unwrap();
    session
        .command(Command::ReplaceDeviceState {
            device,
            state: changed_b.clone(),
        })
        .unwrap();
    let before_copy = session.project().clone();
    session
        .command(Command::CompareDevice {
            device,
            expected: Slot::B,
            action: Action::CopyToInactive,
        })
        .unwrap();
    let copied = session.project().clone();
    assert_eq!(copied.device(device).unwrap().active_variant(), Slot::B);
    assert_eq!(copied.device(device).unwrap().state(), &changed_b);
    assert_eq!(
        copied
            .device(device)
            .unwrap()
            .comparison()
            .unwrap()
            .inactive(),
        &changed_b
    );
    assert!(session.undo());
    assert_eq!(session.project(), &before_copy);
    assert!(session.redo());
    assert_eq!(session.project(), &copied);
    let bytes = hex_storage::encode(&copied).unwrap();
    let decoded = hex_storage::decode(&bytes).unwrap();
    assert_eq!(decoded, copied);
    let mut reopened = Session::from_prepared(decoded, 16, 0, vec![]).unwrap();
    reopened
        .command(Command::CompareDevice {
            device,
            expected: Slot::B,
            action: Action::Switch,
        })
        .unwrap();
    assert_eq!(
        reopened.project().device(device).unwrap().active_variant(),
        Slot::A
    );
    assert_eq!(
        reopened.project().device(device).unwrap().state(),
        &changed_b
    );
    let changed_a = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Effect,
        &changed_b,
        hex_lattice_contract::ParameterId::from_static(1),
        hex_lattice_contract::ParameterValue::Float(0.75),
    )
    .unwrap();
    reopened
        .command(Command::ReplaceDeviceState {
            device,
            state: changed_a.clone(),
        })
        .unwrap();
    let before_copy = reopened.project().clone();
    reopened
        .command(Command::CompareDevice {
            device,
            expected: Slot::A,
            action: Action::CopyToInactive,
        })
        .unwrap();
    assert_eq!(
        reopened.project().device(device).unwrap().active_variant(),
        Slot::A
    );
    assert_eq!(
        reopened.project().device(device).unwrap().state(),
        &changed_a
    );
    assert_eq!(
        reopened
            .project()
            .device(device)
            .unwrap()
            .comparison()
            .unwrap()
            .inactive(),
        &changed_a
    );
    assert!(reopened.undo());
    assert_eq!(reopened.project(), &before_copy);
    assert!(reopened.redo());
    let final_project = reopened.project().clone();
    let roundtrip = hex_storage::decode(&hex_storage::encode(&final_project).unwrap()).unwrap();
    assert_eq!(roundtrip, final_project);
    Session::from_prepared(roundtrip, 16, 0, vec![]).unwrap();
}
