use super::*;
use hex_engine::eq::{EqState, Node, NodeId, NodeKind};
use hex_project::{Command, Created, DeviceVariant};

fn fixture() -> (
    Session,
    DeviceId,
    Arc<PreparedRevision>,
    EqNodeCreationSnapshot,
) {
    let mut session = Session::new(48_000, 16, 0).unwrap();
    let (Created::Track(track), _) = session
        .command_prepared_revision(Command::AddTrack { name: "eq".into() }, None)
        .unwrap()
    else {
        panic!("track")
    };
    let state = EqState::default()
        .encode()
        .unwrap()
        .with_input_gain_millidb(1200)
        .unwrap()
        .with_output_gain_millidb(-2300)
        .unwrap();
    let (Created::Device(device), prepared) = session
        .command_prepared_revision(
            Command::InsertDevice {
                track,
                before: None,
                state,
            },
            None,
        )
        .unwrap()
    else {
        panic!("device")
    };
    let node = Node::new(
        NodeId::new(session.project().next_identity()).unwrap(),
        1200.0,
        12.0,
        0,
    )
    .unwrap();
    let snapshot =
        EqNodeCreationSnapshot::new(node, DeviceVariant::A, session.project().next_identity())
            .unwrap();
    (session, device, prepared, snapshot)
}
fn key() -> GestureKey {
    GestureKey {
        generation: 1,
        token: 1,
    }
}

#[test]
fn creation_previews_without_canonical_ids_and_commits_one_complete_history_edge() {
    for kind in [NodeKind::Bell, NodeKind::LowCut, NodeKind::Notch] {
        let (mut session, device, prepared, mut snapshot) = fixture();
        snapshot.node.kind = kind;
        let before = session.project().snapshot();
        let origin = session.project().device(device).unwrap().state().clone();
        let mut machine = GestureMachine::default();
        let begun = machine.handle(
            &mut session,
            GestureRequest::BeginEqCreation {
                key: key(),
                device,
                snapshot,
            },
            Some(&prepared),
            1,
        );
        assert!(begun.ack.accepted, "{:?}", begun.ack.reason);
        assert!(begun.playback.is_none());
        for sequence in 1..=3 {
            snapshot.node.frequency_hz = 900.0 + sequence as f32 * 100.0;
            let outcome = machine.handle(
                &mut session,
                GestureRequest::ChangeEqCreation {
                    key: key(),
                    sequence,
                    snapshot,
                },
                Some(&prepared),
                1,
            );
            assert!(outcome.ack.accepted, "{:?}", outcome.ack.reason);
            assert!(outcome.playback.is_some());
            let project = outcome.preview_project.unwrap();
            let entry = project.device(device).unwrap();
            assert_eq!(
                EqState::decode(entry.state()).unwrap().nodes,
                vec![snapshot.node]
            );
            assert_eq!(
                entry.state().input_gain_millidb(),
                origin.input_gain_millidb()
            );
            assert_eq!(
                entry.state().output_gain_millidb(),
                origin.output_gain_millidb()
            );
            assert_eq!(session.project().snapshot(), before);
        }
        let mut changed_binding = snapshot;
        changed_binding.node.bank = 1;
        let refused = machine.handle(
            &mut session,
            GestureRequest::ChangeEqCreation {
                key: key(),
                sequence: 4,
                snapshot: changed_binding,
            },
            Some(&prepared),
            1,
        );
        assert!(!refused.ack.accepted);
        assert_eq!(machine.snapshot().unwrap().sequence, 3);
        let committed = machine.handle(
            &mut session,
            GestureRequest::CommitEqCreation {
                key: key(),
                sequence: 4,
                snapshot,
            },
            Some(&prepared),
            1,
        );
        assert!(committed.ack.accepted, "{:?}", committed.ack.reason);
        assert!(committed.canonical_changed);
        assert!(machine.snapshot().is_none());
        let after = session.project().snapshot();
        assert_eq!(session.project().next_identity(), before.next_id + 1);
        session
            .undo_prepared_revision(committed.playback.as_deref())
            .unwrap();
        let mut undone = session.project().snapshot();
        undone.next_id = before.next_id;
        assert_eq!(undone, before);
        session.redo_prepared_revision(None).unwrap();
        assert_eq!(session.project().snapshot(), after);
        let replay = machine.handle(
            &mut session,
            GestureRequest::CommitEqCreation {
                key: key(),
                sequence: 4,
                snapshot,
            },
            Some(&prepared),
            1,
        );
        assert!(!replay.ack.accepted);
        assert_eq!(session.project().snapshot(), after);
    }
}

#[test]
fn cancel_and_retained_terminal_refusal_preserve_canonical_state_and_exact_overlay() {
    let (mut session, device, prepared, mut snapshot) = fixture();
    let before = session.project().snapshot();
    let mut machine = GestureMachine::default();
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::BeginEqCreation {
                    key: key(),
                    device,
                    snapshot
                },
                Some(&prepared),
                1
            )
            .ack
            .accepted
    );
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::ChangeEqCreation {
                    key: key(),
                    sequence: 1,
                    snapshot
                },
                Some(&prepared),
                1
            )
            .ack
            .accepted
    );
    let accepted = machine.snapshot().unwrap();
    snapshot.node.gain_db = -12.0;
    let refused = machine.handle_prepared(
        &mut session,
        GestureRequest::CommitEqCreation {
            key: key(),
            sequence: 2,
            snapshot,
        },
        Some(&prepared),
        1,
        |_, _| Err(SessionError::Invalid("fixture publication refusal")),
    );
    assert!(!refused.ack.accepted && refused.ack.preview_retained);
    assert_eq!(machine.snapshot(), Some(accepted));
    assert_eq!(session.project().snapshot(), before);
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::Cancel { key: key() },
                Some(&prepared),
                1
            )
            .ack
            .accepted
    );
    assert_eq!(session.project().snapshot(), before);
    session.undo_prepared_revision(None).unwrap();
    assert!(
        session.project().device(device).is_none(),
        "cancel created no history edge"
    );
}

#[test]
fn creation_coalescer_keeps_reserved_terminal_and_worker_rejects_changed_origin() {
    let (mut session, device, prepared, mut snapshot) = fixture();
    let mut client = GestureClient::default();
    let key = client.begin_eq_creation(1, device, snapshot).unwrap();
    let mut machine = GestureMachine::default();
    let begin = client.next_request().unwrap();
    assert_eq!(begin.key(), key);
    assert_eq!(
        client.next_request(),
        Some(begin),
        "queue-full retains the exact Begin"
    );
    client.mark_submitted();
    for i in 0..10 {
        snapshot.node.frequency_hz = 900.0 + i as f32;
        client.change_eq_creation(snapshot).unwrap();
    }
    client.finish_eq_creation(snapshot).unwrap();
    assert!(client.change_eq_creation(snapshot).is_err());
    let ack = machine.handle(&mut session, begin, Some(&prepared), 1).ack;
    assert!(client.acknowledge(&ack));
    let commit = client.next_request().unwrap();
    assert_eq!(commit.phase(), GesturePhase::Commit);
    assert_eq!(commit.sequence(), 11);
    let state = session
        .project()
        .device(device)
        .unwrap()
        .state()
        .clone()
        .with_input_gain_millidb(3400)
        .unwrap();
    session
        .command_prepared_revision(
            Command::ReplaceDeviceState { device, state },
            Some(&prepared),
        )
        .unwrap();
    let outcome = machine.handle_prepared(&mut session, commit, Some(&prepared), 1, |_, _| {
        panic!("changed origin must refuse before preparation")
    });
    assert!(!outcome.ack.accepted && outcome.ack.preview_retained);
    assert_eq!(
        EqState::decode(session.project().device(device).unwrap().state())
            .unwrap()
            .nodes
            .len(),
        0
    );
}
