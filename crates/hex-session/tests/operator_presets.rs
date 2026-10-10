use hex_engine::NativeDeviceClass;
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Command, Created};
use hex_session::Session;

#[test]
fn fixed_tuning_history_and_invalid_effective_frequency_are_atomic() {
    let mut session = Session::new(48_000, 16, 0).expect("session");
    let (Created::Track(track), _) = session
        .command_prepared_revision(
            Command::AddMidiTrack {
                name: "fixed operator".into(),
            },
            None,
        )
        .expect("track")
    else {
        panic!("track");
    };
    let device = session
        .project()
        .track(track)
        .expect("track")
        .instrument()
        .expect("instrument")
        .id();
    let state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .expect("operator");
    let (_, mut graph) = session
        .command_prepared_revision(Command::ReplaceDeviceState { device, state }, None)
        .expect("state");
    let (Created::Clip(clip), next) = session
        .command_prepared_revision(
            Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("length"),
            },
            Some(&graph),
        )
        .expect("clip")
    else {
        panic!("clip");
    };
    graph = next;
    (_, graph) = session
        .command_prepared_revision(
            Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(2).expect("duration"),
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            Some(&graph),
        )
        .expect("note");
    graph = session
        .commit_device_parameter(
            device,
            ParameterId::from_static(63),
            ParameterValue::Float(12_000.0),
            Some(&graph),
        )
        .expect("inactive frequency");
    graph = session
        .commit_device_parameter(
            device,
            ParameterId::from_static(64),
            ParameterValue::Float(2.0),
            Some(&graph),
        )
        .expect("inactive multiplier");
    let before = session.project().clone();
    assert!(
        session
            .commit_device_parameter(
                device,
                ParameterId::from_static(62),
                ParameterValue::Bool(true),
                Some(&graph)
            )
            .is_err()
    );
    assert_eq!(session.project().snapshot(), before.snapshot());
    graph = session
        .commit_device_parameter(
            device,
            ParameterId::from_static(63),
            ParameterValue::Float(250.0),
            Some(&graph),
        )
        .expect("valid frequency");
    let before_toggle = session
        .project()
        .device(device)
        .expect("device")
        .state()
        .clone();
    let changed = session
        .commit_device_parameter(
            device,
            ParameterId::from_static(62),
            ParameterValue::Bool(true),
            Some(&graph),
        )
        .expect("fixed on");
    let fixed = session
        .project()
        .device(device)
        .expect("device")
        .state()
        .clone();
    assert_ne!(fixed, before_toggle);
    let undone = session
        .undo_prepared_revision(Some(&changed))
        .expect("undo")
        .expect("one edit");
    assert_eq!(
        session.project().device(device).expect("device").state(),
        &before_toggle
    );
    session
        .redo_prepared_revision(Some(&undone))
        .expect("redo")
        .expect("one edit");
    assert_eq!(
        session.project().device(device).expect("device").state(),
        &fixed
    );
}

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
    let original = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &original,
        ParameterId::from_static(75),
        ParameterValue::Float(0.75),
    )
    .expect("nonneutral velocity setting");
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
    assert_eq!(selected.payload().len(), 304);
    assert_eq!(&selected.payload()[288..], &original.payload()[288..]);
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
