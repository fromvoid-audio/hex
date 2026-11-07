//! Shared gesture intent and one canonical manual/read transition. No device UI
//! or callback timestamps are inferred from these control-side assertions.
use super::*;
use hex_lattice_contract::DeviceAutomationTarget as Target;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationReadSelection as Read, AutomationValue,
    BeatTime, Command, Created,
};

fn fixture() -> (Session, DeviceId, Arc<PreparedRevision>) {
    let mut session = Session::new(48_000, 32, 0).unwrap();
    let (Created::Track(track), _) = session
        .command_prepared_revision(
            Command::AddTrack {
                name: "manual".into(),
            },
            None,
        )
        .unwrap()
    else {
        panic!("track");
    };
    let (Created::Device(device), mut prepared) = session
        .command_prepared_revision(
            Command::InsertDevice {
                track,
                before: None,
                state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                    "hex.utility",
                ))
                .unwrap(),
            },
            None,
        )
        .unwrap()
    else {
        panic!("device");
    };
    for target in [
        Target::Parameter(ParameterId::from_static(1)),
        Target::Parameter(ParameterId::from_static(2)),
        Target::Parameter(ParameterId::from_static(3)),
        Target::HostInputGain,
        Target::HostOutputGain,
    ] {
        prepared = session
            .command_prepared_revision(
                Command::ReplaceDeviceAutomationTarget {
                    device,
                    target,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(0.4).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                },
                Some(&prepared),
            )
            .unwrap()
            .1;
    }
    (session, device, prepared)
}
fn selections(session: &Session, device: DeviceId) -> Vec<(Target, Read)> {
    session
        .project()
        .device(device)
        .unwrap()
        .automation()
        .iter()
        .map(|lane| (lane.target, lane.read_selection))
        .collect()
}
fn deliver(
    client: &mut GestureClient,
    machine: &mut GestureMachine,
    session: &mut Session,
    prepared: &Arc<PreparedRevision>,
) -> GestureOutcome {
    let request = client.next_request().unwrap();
    client.mark_submitted();
    let result = machine.handle(session, request, Some(prepared), 0);
    assert!(result.ack.accepted, "{:?}", result.ack.reason);
    assert!(client.acknowledge(&result.ack));
    result
}
#[test]
fn single_equal_update_survives_coalescing_but_begin_and_untouched_terminal_do_not_override() {
    let (mut session, device, prepared) = fixture();
    let original = session.project().clone();
    let gain = hex_engine::native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Effect,
        session.project().device(device).unwrap().state(),
        ParameterId::from_static(1),
    )
    .unwrap();
    let mut client = GestureClient::default();
    let mut machine = GestureMachine::default();
    let target = GestureTarget::DeviceParameter {
        device,
        parameter: ParameterId::from_static(1),
    };
    client.begin(0, target, gain).unwrap();
    let begin = deliver(&mut client, &mut machine, &mut session, &prepared);
    assert!(begin.playback.is_none());
    client.finish(gain).unwrap();
    deliver(&mut client, &mut machine, &mut session, &prepared);
    assert_eq!(session.project(), &original);
    client.begin(0, target, gain).unwrap();
    deliver(&mut client, &mut machine, &mut session, &prepared);
    client.change(gain).unwrap();
    client.finish(gain).unwrap();
    let committed = deliver(&mut client, &mut machine, &mut session, &prepared);
    assert!(committed.canonical_changed);
    assert_eq!(
        selections(&session, device)
            .iter()
            .filter(|(_, s)| *s == Read::Base)
            .map(|(t, _)| *t)
            .collect::<Vec<_>>(),
        vec![Target::Parameter(ParameterId::from_static(1))]
    );
    assert_eq!(
        session.project().device(device).unwrap().automation()[0].points,
        original.device(device).unwrap().automation()[0].points
    );
    assert!(session.undo());
    assert_eq!(session.project(), &original);
}
#[test]
fn changed_terminal_without_change_and_paired_host_intent_select_precise_targets() {
    let (mut session, device, prepared) = fixture();
    let mut client = GestureClient::default();
    let mut machine = GestureMachine::default();
    client
        .begin(
            0,
            GestureTarget::DeviceParameter {
                device,
                parameter: ParameterId::from_static(1),
            },
            ParameterValue::Float(1.0),
        )
        .unwrap();
    deliver(&mut client, &mut machine, &mut session, &prepared);
    client.finish(ParameterValue::Float(0.5)).unwrap();
    deliver(&mut client, &mut machine, &mut session, &prepared);
    assert_eq!(
        session.project().device(device).unwrap().automation()[0].read_selection,
        Read::Base
    );
    assert!(session.undo());
    client
        .begin_pair(
            0,
            device,
            hex_engine::DEVICE_INPUT_GAIN.id,
            hex_engine::DEVICE_OUTPUT_GAIN.id,
            ParameterPairValue {
                first: ParameterValue::Float(0.0),
                second: ParameterValue::Float(0.0),
            },
        )
        .unwrap();
    deliver(&mut client, &mut machine, &mut session, &prepared);
    let equal = ParameterPairValue {
        first: ParameterValue::Float(0.0),
        second: ParameterValue::Float(0.0),
    };
    client.change_pair(equal).unwrap();
    client.finish_pair(equal).unwrap();
    deliver(&mut client, &mut machine, &mut session, &prepared);
    assert_eq!(
        selections(&session, device)
            .iter()
            .filter(|(_, s)| *s == Read::Base)
            .map(|(t, _)| *t)
            .collect::<Vec<_>>(),
        vec![Target::HostInputGain, Target::HostOutputGain]
    );
}
#[test]
fn snapshot_union_preserves_move_back_without_overriding_validation_fields() {
    let (mut session, device, prepared) = fixture();
    let original = session.project().clone();
    let initial = DeviceParameterSnapshot::new(&[
        (ParameterId::from_static(1), ParameterValue::Float(1.0)),
        (ParameterId::from_static(2), ParameterValue::Float(0.0)),
        (ParameterId::from_static(3), ParameterValue::Float(1.0)),
    ])
    .unwrap();
    let changed = DeviceParameterSnapshot::new(&[
        (ParameterId::from_static(1), ParameterValue::Float(0.5)),
        (ParameterId::from_static(2), ParameterValue::Float(0.0)),
        (ParameterId::from_static(3), ParameterValue::Float(1.0)),
    ])
    .unwrap();
    let mut client = GestureClient::default();
    let mut machine = GestureMachine::default();
    client.begin_snapshot(0, device, initial).unwrap();
    deliver(&mut client, &mut machine, &mut session, &prepared);
    client.change_snapshot(changed).unwrap();
    client.change_snapshot(initial).unwrap();
    client.finish_snapshot(initial).unwrap();
    assert_eq!(
        client
            .snapshot()
            .unwrap()
            .touched_parameters
            .ids(&initial)
            .unwrap()
            .collect::<Vec<_>>(),
        vec![ParameterId::from_static(1)]
    );
    deliver(&mut client, &mut machine, &mut session, &prepared);
    assert_eq!(
        selections(&session, device)
            .iter()
            .filter(|(_, s)| *s == Read::Base)
            .map(|(t, _)| *t)
            .collect::<Vec<_>>(),
        vec![Target::Parameter(ParameterId::from_static(1))]
    );
    assert!(session.undo());
    assert_eq!(session.project(), &original);
    client.begin_snapshot(0, device, initial).unwrap();
    deliver(&mut client, &mut machine, &mut session, &prepared);
    client.change_snapshot(initial).unwrap();
    client.finish_snapshot(initial).unwrap();
    deliver(&mut client, &mut machine, &mut session, &prepared);
    assert_eq!(session.project(), &original);
}
#[test]
fn cancel_and_invalid_change_preserve_read_selection_and_authored_points() {
    let (mut session, device, prepared) = fixture();
    let original = session.project().clone();
    let mut machine = GestureMachine::default();
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    let begin = machine.handle(
        &mut session,
        GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter {
                device,
                parameter: ParameterId::from_static(1),
            },
        },
        Some(&prepared),
        0,
    );
    assert!(begin.ack.accepted);
    let invalid = machine.handle(
        &mut session,
        GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(f32::NAN),
        },
        Some(&prepared),
        0,
    );
    assert!(!invalid.ack.accepted);
    assert!(!machine.snapshot().unwrap().manual_edit);
    let preview = machine.handle(
        &mut session,
        GestureRequest::Change {
            key,
            sequence: 2,
            value: ParameterValue::Float(0.5),
        },
        Some(&prepared),
        0,
    );
    assert!(preview.ack.accepted);
    assert_eq!(
        preview
            .preview_project
            .unwrap()
            .device(device)
            .unwrap()
            .automation()[0]
            .read_selection,
        Read::Base
    );
    assert_eq!(session.project(), &original);
    let cancel = machine.handle(
        &mut session,
        GestureRequest::Cancel { key },
        Some(&prepared),
        0,
    );
    assert!(cancel.ack.accepted);
    assert_eq!(session.project(), &original);
}

#[test]
fn snapshot_touch_mask_resolves_exact_ordered_ids_including_the_last_slot() {
    // IDs deliberately differ from bit indices and are not sorted: identity comes
    // from the sealed snapshot domain, never from interpreting a bit as an ID.
    let entries: Vec<_> = (1..=64)
        .rev()
        .map(|id| (ParameterId::from_static(id * 3), ParameterValue::Float(0.0)))
        .collect();
    let origin = DeviceParameterSnapshot::new(&entries).unwrap();
    let mut edited = entries.clone();
    edited[0].1 = ParameterValue::Float(0.5);
    edited[63].1 = ParameterValue::Float(1.0);
    let edited = DeviceParameterSnapshot::new(&edited).unwrap();
    let mut touched = TouchedParameters::default();
    touched.include_changed(origin, edited).unwrap();
    touched.include_changed(origin, origin).unwrap();
    assert_eq!(touched.len(), 2);
    assert_eq!(
        touched.ids(&origin).unwrap().collect::<Vec<_>>(),
        vec![ParameterId::from_static(192), ParameterId::from_static(3)]
    );
    let mut reordered = entries.clone();
    reordered.swap(0, 63);
    assert!(
        touched
            .include_changed(origin, DeviceParameterSnapshot::new(&reordered).unwrap())
            .is_err()
    );
    let short = DeviceParameterSnapshot::new(&entries[..1]).unwrap();
    assert!(touched.ids(&short).is_err());
    let mut untouched = TouchedParameters::default();
    assert!(untouched.merge(touched, short).is_err());
    assert!(untouched.is_empty());
    assert_eq!(std::mem::size_of::<TouchedParameters>(), 8);
}
