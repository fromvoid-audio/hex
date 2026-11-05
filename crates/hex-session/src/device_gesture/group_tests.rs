//! Frozen gain cohorts reuse the ordered gesture/prepare/commit owner.
use super::*;
use hex_project::{Command, Created, RelativeTrackGains};

fn fixture() -> (Session, Vec<TrackId>, RelativeTrackGains) {
    let mut session = Session::new(48_000, 16, 0).unwrap();
    let mut ids = Vec::new();
    for gain in [0.5, 1.5, 0.0, 0.25] {
        let (Created::Track(track), _) = session
            .command_prepared_revision(
                Command::AddTrack {
                    name: "gain".into(),
                },
                None,
            )
            .unwrap()
        else {
            panic!("track")
        };
        session
            .command_prepared_revision(Command::Gain { track, gain }, None)
            .unwrap();
        ids.push(track);
    }
    let group = RelativeTrackGains::capture(session.project(), ids[0], &ids[..3]).unwrap();
    (session, ids, group)
}
fn gains(project: &Project) -> Vec<f32> {
    project
        .tracks()
        .iter()
        .map(hex_project::Track::gain)
        .collect()
}
fn key(token: u64) -> GestureKey {
    GestureKey {
        generation: 0,
        token,
    }
}

#[test]
fn group_preview_reverse_cancel_and_commit_have_one_atomic_history_boundary() {
    let (mut session, _, group) = fixture();
    let before = session.project().clone();
    let mut machine = GestureMachine::default();
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::BeginTrackGains { key: key(1), group },
                None,
                0
            )
            .ack
            .accepted
    );
    for (sequence, value, expected) in [
        (1, 1.0, [1.0, 2.0, 0.0, 0.25]),
        (2, 0.25, [0.25, 0.75, 0.0, 0.25]),
    ] {
        let outcome = machine.handle(
            &mut session,
            GestureRequest::Change {
                key: key(1),
                sequence,
                value: ParameterValue::Float(value),
            },
            None,
            0,
        );
        assert!(outcome.ack.accepted, "{:?}", outcome.ack.reason);
        assert_eq!(gains(outcome.preview_project.as_ref().unwrap()), expected);
        assert_eq!(session.project(), &before);
    }
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::Cancel { key: key(1) },
                None,
                0
            )
            .ack
            .accepted
    );
    assert!(machine.snapshot().is_none());
    assert_eq!(session.project(), &before);
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::BeginTrackGains { key: key(2), group },
                None,
                0
            )
            .ack
            .accepted
    );
    let committed = machine.handle(
        &mut session,
        GestureRequest::Commit {
            key: key(2),
            sequence: 1,
            manual_edit: true,
            value: ParameterValue::Float(0.75),
        },
        None,
        0,
    );
    assert!(committed.ack.accepted, "{:?}", committed.ack.reason);
    assert_eq!(gains(session.project()), [0.75, 2.0, 0.0, 0.25]);
    assert!(session.undo());
    assert_eq!(session.project(), &before);
    assert!(session.redo());
    assert_eq!(gains(session.project()), [0.75, 2.0, 0.0, 0.25]);
}

#[test]
fn stale_cohorts_refuse_together_and_failed_native_terminal_keeps_the_whole_preview() {
    let (mut session, ids, group) = fixture();
    session
        .command_prepared_revision(
            Command::Gain {
                track: ids[1],
                gain: 1.0,
            },
            None,
        )
        .unwrap();
    let before = session.project().clone();
    let mut machine = GestureMachine::default();
    let rejected = machine.handle(
        &mut session,
        GestureRequest::BeginTrackGains { key: key(1), group },
        None,
        0,
    );
    assert!(!rejected.ack.accepted);
    assert!(machine.snapshot().is_none());
    assert_eq!(session.project(), &before);
    let group = RelativeTrackGains::capture(session.project(), ids[0], &ids[..3]).unwrap();
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::BeginTrackGains { key: key(2), group },
                None,
                0
            )
            .ack
            .accepted
    );
    let changed = machine.handle(
        &mut session,
        GestureRequest::Change {
            key: key(2),
            sequence: 1,
            value: ParameterValue::Float(1.0),
        },
        None,
        0,
    );
    assert!(changed.ack.accepted);
    let refused = machine.handle_prepared(
        &mut session,
        GestureRequest::Commit {
            key: key(2),
            sequence: 2,
            manual_edit: true,
            value: ParameterValue::Float(1.0),
        },
        None,
        0,
        |_, _| Err(SessionError::Invalid("injected publication refusal")),
    );
    assert!(!refused.ack.accepted);
    assert!(refused.ack.preview_retained);
    let retained = machine.snapshot().unwrap();
    assert_eq!(retained.track_gains, Some(group));
    assert_eq!(retained.value, ParameterValue::Float(1.0));
    assert_eq!(session.project(), &before);
    let invalid = machine.handle(
        &mut session,
        GestureRequest::Change {
            key: key(2),
            sequence: 3,
            value: ParameterValue::Float(f32::NAN),
        },
        None,
        0,
    );
    assert!(!invalid.ack.accepted);
    assert_eq!(session.project(), &before);
    assert_eq!(
        machine.snapshot().unwrap().value,
        ParameterValue::Float(1.0)
    );
}

#[test]
fn group_client_coalesces_scalar_values_and_reserves_one_terminal_without_losing_origins() {
    let (_, ids, group) = fixture();
    let mut client = GestureClient::default();
    let key = client.begin_track_gains(0, group).unwrap();
    let begin = client.next_request().unwrap();
    assert!(matches!(begin, GestureRequest::BeginTrackGains { .. }));
    client.mark_submitted();
    client.change(ParameterValue::Float(1.0)).unwrap();
    client.change(ParameterValue::Float(0.25)).unwrap();
    client.finish(ParameterValue::Float(0.75)).unwrap();
    assert_eq!(client.preview_track_gain(0, ids[1]), Some(2.0));
    assert_eq!(client.preview_track_gain(0, ids[2]), Some(0.0));
    assert_eq!(client.preview_track_gain(1, ids[1]), None);
    assert!(client.acknowledge(&GestureAck {
        preview_retained: false,
        key,
        phase: GesturePhase::Begin,
        sequence: 0,
        accepted: true,
        reason: None
    }));
    let terminal = client.next_request().unwrap();
    assert!(matches!(
        terminal,
        GestureRequest::Commit {
            value: ParameterValue::Float(0.75),
            ..
        }
    ));
    assert_eq!(client.snapshot().unwrap().track_gains, Some(group));
    println!(
        "control bytes: request={} snapshot={} client={} cohort={}",
        std::mem::size_of::<GestureRequest>(),
        std::mem::size_of::<GestureSnapshot>(),
        std::mem::size_of::<GestureClient>(),
        std::mem::size_of::<RelativeTrackGains>()
    );
}
