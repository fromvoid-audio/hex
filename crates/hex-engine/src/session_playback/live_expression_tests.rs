//! Controller ownership and outer-block rendering, without native hardware.
use super::*;
use crate::live_expression::{LiveExpressionInput, Snapshot as Expression};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::{Command, Created, DeviceId, Editor, Project};
use std::collections::BTreeMap;

struct Fixture {
    editor: Editor,
    track: TrackId,
    instrument: DeviceId,
    clip: ClipId,
}
fn fixture() -> Fixture {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "controller".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let mut state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        hex_devices::operator_state::OPERATOR_TYPE_ID,
    )
    .unwrap();
    for (parameter, value) in [
        (608, ParameterValue::Choice(1)),
        (29, ParameterValue::Float(0.5)),
        (33, ParameterValue::Float(1.0)),
        (34, ParameterValue::Float(1.0)),
        (35, ParameterValue::Float(1.0)),
        (36, ParameterValue::Float(0.0)),
        (37, ParameterValue::Float(0.001)),
        (628, ParameterValue::Choice(10)),
        (629, ParameterValue::Float(50.0)),
        (636, ParameterValue::Choice(11)),
        (637, ParameterValue::Float(50.0)),
    ] {
        state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(parameter),
            value,
        )
        .unwrap();
    }
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state,
        })
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: crate::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID)
                .unwrap(),
        })
        .unwrap();
    let Created::Clip(arrangement) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_ratio(8, 1).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: arrangement,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(4, 1).unwrap(),
            pitch: 69,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "controllers".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CopyArrangementMidiToSession {
            source: arrangement,
            scene,
        })
        .unwrap()
    else {
        panic!("session clip")
    };
    Fixture {
        editor,
        track,
        instrument,
        clip,
    }
}
fn revision(f: &Fixture) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &BTreeMap::new(),
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}
fn lease(f: &Fixture) -> Arc<LiveExpressionInput> {
    Arc::new(LiveExpressionInput::new(7, f.track, f.instrument))
}
fn start(
    f: &Fixture,
    revision: Arc<PreparedRevision>,
    input: Arc<LiveExpressionInput>,
    session: bool,
) -> SessionPlayback {
    let mut runtime = SessionPlayback::new(revision).unwrap();
    runtime.attach_live_expression(input).unwrap();
    if session {
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: f.clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
    }
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime
}
#[test]
fn controller_snapshot_is_frozen_across_subsegments_and_source_value_survives_cancel() {
    let f = fixture();
    for session in [false, true] {
        let revision = revision(&f);
        let input = lease(&f);
        let oracle_input = lease(&f);
        let first = Expression::new(90, 3000, 60).unwrap();
        let second = Expression::new(127, -8192, 127).unwrap();
        input.publish(first);
        oracle_input.publish(first);
        let mut tested = start(&f, Arc::clone(&revision), Arc::clone(&input), session);
        let mut oracle = start(&f, revision, Arc::clone(&oracle_input), session);
        let mut actual = [[0.0; 2]; 1024];
        let mut expected = actual;
        let mut observed = 0;
        tested
            .render_with_post_node_observer(
                &mut actual,
                Some(&mut |_, _, _: &[Frame]| {
                    observed += 1;
                    input.publish(second);
                }),
            )
            .unwrap();
        oracle.render(&mut expected).unwrap();
        assert!(
            observed > 1,
            "multiple native render segments must be observed"
        );
        assert_eq!(
            actual, expected,
            "a controller write inside a block must wait until the next outer block"
        );
        oracle_input.publish(second);
        tested.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(
            tested.mutable_instruments[0].as_ref().unwrap().expression,
            Some(second)
        );
        input.cancel();
        oracle_input.cancel();
        input.publish(first);
        tested.render(&mut actual).unwrap();
        oracle.render(&mut expected).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(input.snapshot(), Expression::default());
        assert_eq!(
            tested.mutable_instruments[0].as_ref().unwrap().expression,
            Some(second)
        );
    }
}
#[test]
fn same_source_handoff_and_deselection_retain_last_published_expression() {
    let f = fixture();
    let revision = revision(&f);
    let input = lease(&f);
    input.publish(Expression::new(100, 8191, 80).unwrap());
    let mut active = start(&f, Arc::clone(&revision), Arc::clone(&input), false);
    active.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    candidate
        .attach_live_expression(Arc::clone(&input))
        .unwrap();
    candidate.try_inherit_state_from(&mut active).unwrap();
    candidate.render(&mut [[0.0; 2]; 64]).unwrap();
    assert!(!input.is_cancelled());
    assert_eq!(
        candidate.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .expression,
        Some(input.snapshot())
    );
    let mut deselected = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    deselected.try_inherit_state_from(&mut candidate).unwrap();
    deselected.render(&mut [[0.0; 2]; 64]).unwrap();
    assert_eq!(
        deselected.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .expression,
        Some(input.snapshot())
    );
}
#[test]
fn fresh_and_cancelled_input_do_not_zero_shared_source_expression() {
    let f = fixture();
    let revision = revision(&f);
    let input = lease(&f);
    let value = Expression::new(100, 8191, 80).unwrap();
    input.publish(value);
    let mut active = start(&f, Arc::clone(&revision), input, false);
    active.render(&mut [[0.0; 2]; 64]).unwrap();
    let fresh = lease(&f);
    active.attach_live_expression(Arc::clone(&fresh)).unwrap();
    active.render(&mut [[0.0; 2]; 64]).unwrap();
    assert_eq!(
        active.mutable_instruments[0].as_ref().unwrap().expression,
        Some(value)
    );
    fresh.cancel();
    active.render(&mut [[0.0; 2]; 64]).unwrap();
    assert_eq!(
        active.mutable_instruments[0].as_ref().unwrap().expression,
        Some(value)
    );
}

#[test]
fn keyboard_monitor_receives_current_snapshot_without_starting_transport() {
    let f = fixture();
    let mut playback =
        crate::playback::Playback::try_from_prepared_with_session(revision(&f)).unwrap();
    let input = lease(&f);
    input.publish(Expression::new(127, 8191, 127).unwrap());
    playback
        .prepare_live_expression(f.editor.project(), Arc::clone(&input))
        .unwrap();
    let keys = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
    playback
        .prepare_computer_keyboard(f.editor.project(), f.track, Arc::clone(&keys))
        .unwrap();
    keys.set_key(0, Some((69, 100, 1))).unwrap();
    let mut output = [[0.0; 2]; 1024];
    playback.render(&mut output).unwrap();
    assert!(!playback.playing());
    assert!(output.iter().any(|frame| frame[0] != 0.0));
    input.cancel();
    playback.render(&mut output).unwrap();
    assert!(
        output
            .iter()
            .all(|frame| frame.iter().all(|sample| sample.is_finite()))
    );
}
#[test]
fn packed_controller_bounds_and_flood_remain_one_latest_snapshot() {
    let f = fixture();
    let input = lease(&f);
    for index in 0..100_000 {
        input.publish(
            Expression::new(
                (index % 128) as u8,
                (index % 16384 - 8192) as i16,
                (index % 128) as u8,
            )
            .unwrap(),
        );
    }
    assert_eq!(input.snapshot(), Expression::new(31, -6497, 31).unwrap());
    assert!(Expression::new(128, 0, 0).is_none());
    assert!(Expression::new(0, 8192, 0).is_none());
    input.cancel();
    input.publish(Expression::new(127, 8191, 127).unwrap());
    assert_eq!(input.snapshot(), Expression::default());
}

#[test]
fn compensated_route_and_failed_render_preserve_the_frozen_snapshot_and_transaction() {
    let mut f = fixture();
    let Created::Track(bus) = f
        .editor
        .execute(Command::AddMidiTrack { name: "bus".into() })
        .unwrap()
    else {
        panic!("bus")
    };
    f.editor
        .execute(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.track),
            destination: bus,
            gain: 0.5,
        })
        .unwrap();
    let revision = revision(&f);
    let input = lease(&f);
    let oracle_input = lease(&f);
    let value = Expression::new(80, 2000, 70).unwrap();
    input.publish(value);
    oracle_input.publish(value);
    let mut tested = start(&f, Arc::clone(&revision), Arc::clone(&input), false);
    let mut oracle = start(&f, revision, oracle_input, false);
    assert!(tested.compensation.is_some());
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    let mut observed = 0;
    tested
        .render_with_post_node_observer(
            &mut actual,
            Some(&mut |_, _, _: &[Frame]| {
                observed += 1;
                input.publish(Expression::default());
            }),
        )
        .unwrap();
    oracle.render(&mut expected).unwrap();
    assert!(observed > 1);
    assert_eq!(actual, expected);
    input.publish(value);
    tested.arrangement_frame = u64::MAX - 1;
    assert!(tested.render(&mut actual).is_err());
    assert_eq!(actual, [[0.0; 2]; 128]);
    assert_eq!(tested.performance_frame(), 128);
    tested.arrangement_frame = 128;
    tested.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected, "failed renders preserve native history");
}

#[test]
fn quiescent_global_to_legacy_mutable_handoff_drops_unsupported_expression() {
    let mut f = fixture();
    let mut state = f
        .editor
        .project()
        .track(f.track)
        .unwrap()
        .instrument()
        .unwrap()
        .state()
        .clone();
    for (parameter, value) in [
        (94, ParameterValue::Float(0.25)),
        (38, ParameterValue::Float(0.001)),
    ] {
        state = crate::patch_native_device_parameter_for_class(
            crate::NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(parameter),
            value,
        )
        .unwrap();
    }
    f.editor
        .execute(Command::ReplaceDeviceState {
            device: f.instrument,
            state: state.clone(),
        })
        .unwrap();
    let input = lease(&f);
    input.publish(Expression::new(100, 4096, 70).unwrap());
    let mut active = start(&f, revision(&f), Arc::clone(&input), false);
    let mut block = [[0.0; 2]; 256];
    for _ in 0..500 {
        active.render(&mut block).unwrap();
    }
    assert!(
        active.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    assert!(
        active.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .expression
            .is_some()
    );
    let legacy = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &state,
        ParameterId::from_static(608),
        ParameterValue::Choice(0),
    )
    .unwrap();
    f.editor
        .execute(Command::ReplaceDeviceState {
            device: f.instrument,
            state: legacy,
        })
        .unwrap();
    let mut candidate = SessionPlayback::new(revision(&f)).unwrap();
    assert!(
        candidate.mutable_instruments[0].is_some(),
        "authored spectrum keeps legacy on the mutable path"
    );
    assert!(
        candidate
            .attach_live_expression(Arc::clone(&input))
            .is_err()
    );
    input.cancel();
    candidate.try_inherit_state_from(&mut active).unwrap();
    candidate.render(&mut block).unwrap();
    assert!(
        candidate.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .expression
            .is_none()
    );
}

#[test]
fn live_expression_attachment_rejects_different_state_in_the_same_instrument_slot() {
    let mut f = fixture();
    let mut playback =
        crate::playback::Playback::try_from_prepared_with_session(revision(&f)).unwrap();
    let input = lease(&f);
    let mut other_rate = f.editor.project().snapshot();
    other_rate.sample_rate = 44_100;
    let other_rate = Project::from_snapshot(other_rate).unwrap();
    assert!(
        playback
            .prepare_live_expression(&other_rate, Arc::clone(&input))
            .is_err(),
        "a different canonical sample rate cannot attach to this prepared graph"
    );
    let state = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        f.editor
            .project()
            .track(f.track)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
        ParameterId::from_static(29),
        ParameterValue::Float(0.75),
    )
    .unwrap();
    f.editor
        .execute(Command::ReplaceDeviceState {
            device: f.instrument,
            state,
        })
        .unwrap();
    assert!(
        playback
            .prepare_live_expression(f.editor.project(), input)
            .is_err(),
        "matching instance IDs do not establish matching prepared source"
    );
}
