//! Protocol/model oracles only. Native SDK application and canonical production
//! publication are verified separately at their actual host boundaries.
use super::*;
use hex_lattice_contract::NativeDeviceState;
use hex_project::{Command, Created};

fn value(value: f64) -> PluginParameterValue {
    PluginParameterValue::new(value).unwrap()
}
fn state(byte: u8) -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![byte]).unwrap()
}
fn fixture() -> (Session, PluginParameterOrigin, Arc<PreparedRevision>) {
    let mut session = Session::new(48_000, 32, 0).unwrap();
    let (Created::Track(track), prepared) = session
        .command_prepared_revision(
            Command::AddTrack {
                name: "protocol fixture".into(),
            },
            None,
        )
        .unwrap()
    else {
        panic!("track")
    };
    // No fake foreign processor is compiled. This retained revision is only
    // the preparation callback's ordering oracle, never native acceptance.
    let Created::Device(device) = session
        .editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state(1),
        })
        .unwrap()
    else {
        panic!("device")
    };
    (
        session,
        PluginParameterOrigin {
            context: hex_engine::AudioSourceContext {
                project_generation: 9,
                editor_revision: 3,
            },
            device,
            parameter: 0,
            page_first: 0,
            metadata_revision: 17,
            initial: value(0.25),
        },
        prepared,
    )
}
fn ack(request: GestureRequest, accepted: bool) -> GestureAck {
    GestureAck {
        preview_retained: false,
        source_lost: false,
        key: request.key(),
        phase: request.phase(),
        sequence: request.sequence(),
        accepted,
        reason: None,
    }
}

#[test]
fn values_and_coalesced_terminal_preserve_raw_zero_and_f64() {
    let (_, origin, _) = fixture();
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 1.1] {
        assert!(PluginParameterValue::new(invalid).is_err());
    }
    assert_eq!(value(-0.0).normalized().to_bits(), 0);
    let precise = value(0.123_456_789_012_345_66);
    assert_ne!(precise.normalized(), f64::from(precise.normalized() as f32));
    let mut client = GestureClient::default();
    let key = client.begin_plugin(origin).unwrap();
    let begin = client.next_request().unwrap();
    assert!(matches!(begin, GestureRequest::BeginPlugin { origin, .. } if origin.parameter == 0));
    client.mark_submitted();
    assert!(client.change(ParameterValue::Float(0.5)).is_err());
    assert_eq!(
        client.snapshot().unwrap().plugin_value,
        Some(origin.initial)
    );
    client.change_plugin(value(0.3)).unwrap();
    client.change_plugin(precise).unwrap();
    let mut stale = ack(begin, true);
    stale.key.token += 1;
    assert!(!client.acknowledge(&stale));
    assert!(client.acknowledge(&ack(begin, true)));
    let change = client.next_request().unwrap();
    assert!(
        matches!(change, GestureRequest::ChangePlugin { key: actual, sequence: 2, value } if actual == key && value == precise)
    );
    client.mark_submitted();
    let final_value = value(0.987_654_321_012_345_6);
    client.change_plugin(final_value).unwrap();
    client.finish_current().unwrap();
    assert!(client.change_plugin(value(0.5)).is_err());
    assert!(client.change(ParameterValue::Float(0.5)).is_err());
    assert!(client.acknowledge(&ack(change, true)));
    let terminal = client.next_request().unwrap();
    assert!(
        matches!(terminal, GestureRequest::CommitPlugin { sequence: 4, value, .. } if value == final_value)
    );
    client.mark_submitted();
    let mut wrong = ack(terminal, true);
    wrong.sequence -= 1;
    assert!(!client.acknowledge(&wrong));
    assert!(client.acknowledge(&ack(terminal, true)));
    assert!(client.snapshot().is_none());
    assert!(client.next_request().is_none());
}

#[test]
fn plugin_cancel_signals_exact_inflight_operation_without_displacing_its_ack() {
    let (_, origin, _) = fixture();
    let mut client = GestureClient::default();
    let key = client.begin_plugin(origin).unwrap();
    let begin = client.next_request().unwrap();
    client.mark_submitted();
    client.cancel().unwrap();
    assert_eq!(client.urgent_plugin_cancel(), Some(begin));
    assert_eq!(
        client.urgent_plugin_cancel(),
        Some(begin),
        "full FIFO retries the same signal"
    );
    client.mark_urgent_plugin_cancel_submitted();
    assert_eq!(client.urgent_plugin_cancel(), None);
    assert_eq!(
        client.next_request(),
        None,
        "original ACK still owns the slot"
    );
    let mut stale = ack(begin, true);
    stale.key.token += 1;
    assert!(!client.acknowledge(&stale));
    assert!(client.acknowledge(&ack(begin, true)));
    assert_eq!(client.next_request(), Some(GestureRequest::Cancel { key }));
}

#[test]
fn plugin_cancel_during_commit_waits_for_real_terminal_then_restores_refused_preview() {
    let (_, origin, _) = fixture();
    let mut client = GestureClient::default();
    let key = client.begin_plugin(origin).unwrap();
    let begin = client.next_request().unwrap();
    client.mark_submitted();
    assert!(client.acknowledge(&ack(begin, true)));
    client.finish_plugin(value(0.75)).unwrap();
    let commit = client.next_request().unwrap();
    client.mark_submitted();
    client.cancel().unwrap();
    assert_eq!(client.urgent_plugin_cancel(), Some(commit));
    client.mark_urgent_plugin_cancel_submitted();
    let mut refused = ack(commit, false);
    refused.preview_retained = true;
    let retained = client.snapshot().unwrap();
    assert!(client.acknowledge_with_snapshot(&refused, Some(retained)));
    assert_eq!(client.next_request(), Some(GestureRequest::Cancel { key }));

    let mut completed = GestureClient::default();
    completed.begin_plugin(origin).unwrap();
    let begin = completed.next_request().unwrap();
    completed.mark_submitted();
    assert!(completed.acknowledge(&ack(begin, true)));
    completed.finish_plugin(value(0.75)).unwrap();
    let commit = completed.next_request().unwrap();
    completed.mark_submitted();
    completed.cancel().unwrap();
    assert!(
        completed.acknowledge(&ack(commit, true)),
        "accepted commit wins the race"
    );
    assert!(completed.snapshot().is_none());
    assert!(completed.next_request().is_none());
    assert!(completed.urgent_plugin_cancel().is_none());
}

#[test]
fn hostless_begin_refuses_without_canonical_or_gesture_mutation() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let mut machine = GestureMachine::default();
    let request = GestureRequest::BeginPlugin {
        key: GestureKey {
            generation: 9,
            token: 1,
        },
        origin,
    };
    let outcome = machine.handle(&mut session, request, Some(&prepared), 9);
    assert!(!outcome.ack.accepted);
    assert!(machine.snapshot().is_none());
    assert_eq!(session.project(), &before);
}

#[test]
fn stale_retirement_preserves_the_key_fence_and_does_not_retire_another_gesture() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let key = GestureKey {
        generation: 9,
        token: 4,
    };
    let mut machine = GestureMachine::default();
    let begin = GestureRequest::BeginPlugin { key, origin };
    assert!(
        machine
            .handle_prepared(&mut session, begin, Some(&prepared), 9, |_, _| Ok(None))
            .ack
            .accepted
    );
    machine.retire_if_key(GestureKey {
        generation: 9,
        token: 3,
    });
    assert_eq!(machine.snapshot().unwrap().key, key);
    machine.retire_if_key(key);
    assert!(machine.snapshot().is_none());
    assert!(
        !machine
            .handle_prepared(&mut session, begin, Some(&prepared), 9, |_, _| Ok(None))
            .ack
            .accepted
    );
    assert_eq!(session.project(), &before);
}

#[test]
fn pending_plugin_operations_retain_the_exact_request_and_do_not_commit() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let key = GestureKey {
        generation: 9,
        token: 1,
    };
    let mut machine = GestureMachine::default();
    let begin = GestureRequest::BeginPlugin { key, origin };
    let pending = machine.handle_prepared(&mut session, begin, Some(&prepared), 9, |_, _| {
        Err(SessionError::PluginPending)
    });
    assert!(pending.pending && !pending.ack.accepted);
    assert!(machine.snapshot().is_none());
    let accepted =
        machine.handle_prepared(&mut session, begin, Some(&prepared), 9, |_, _| Ok(None));
    assert!(!accepted.pending && accepted.ack.accepted);

    let change = GestureRequest::ChangePlugin {
        key,
        sequence: 1,
        value: value(0.5),
    };
    let pending = machine.handle_prepared(&mut session, change, Some(&prepared), 9, |_, _| {
        Err(SessionError::PluginPending)
    });
    assert!(pending.pending && !pending.ack.accepted);
    assert_eq!(machine.snapshot().unwrap().sequence, 0);
    let accepted =
        machine.handle_prepared(&mut session, change, Some(&prepared), 9, |_, _| Ok(None));
    assert!(!accepted.pending && accepted.ack.accepted);

    let commit = GestureRequest::CommitPlugin {
        key,
        sequence: 2,
        value: value(0.75),
    };
    let mut preview_calls = 0;
    let pending = machine.handle_prepared(
        &mut session,
        commit,
        Some(&prepared),
        9,
        |session, op| match op {
            GesturePreparation::Plugin(PluginGesturePreparation::Preview(_)) => {
                preview_calls += 1;
                Ok(Some((
                    Arc::new(session.project().clone()),
                    Arc::clone(&prepared),
                )))
            }
            GesturePreparation::Plugin(PluginGesturePreparation::Commit(_)) => {
                Err(SessionError::PluginPending)
            }
            _ => panic!("unexpected plugin operation"),
        },
    );
    assert!(pending.pending && !pending.ack.accepted);
    assert!(
        pending
            .playback
            .as_ref()
            .is_some_and(|actual| Arc::ptr_eq(actual, &prepared))
    );
    assert_eq!(preview_calls, 1);
    assert_eq!(machine.snapshot().unwrap().sequence, 2);
    assert_eq!(session.project(), &before);
    let committed =
        machine.handle_prepared(&mut session, commit, Some(&prepared), 9, |session, op| {
            assert!(matches!(
                op,
                GesturePreparation::Plugin(PluginGesturePreparation::Commit(_))
            ));
            session.editor.execute(Command::ReplaceDeviceState {
                device: origin.device,
                state: state(2),
            })?;
            Ok(Some((
                Arc::new(session.project().clone()),
                Arc::clone(&prepared),
            )))
        });
    assert!(!committed.pending && committed.ack.accepted && committed.canonical_changed);
    assert!(machine.snapshot().is_none());
}

#[test]
fn pending_plugin_restore_keeps_the_accepted_preview_until_terminal() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let key = GestureKey {
        generation: 9,
        token: 1,
    };
    let mut machine = GestureMachine::default();
    for request in [
        GestureRequest::BeginPlugin { key, origin },
        GestureRequest::ChangePlugin {
            key,
            sequence: 1,
            value: value(0.75),
        },
    ] {
        let outcome =
            machine.handle_prepared(&mut session, request, Some(&prepared), 9, |_, _| Ok(None));
        assert!(!outcome.pending && outcome.ack.accepted);
    }
    let preview = machine.snapshot().unwrap();
    let cancel = GestureRequest::Cancel { key };
    let pending =
        machine.handle_prepared(&mut session, cancel, Some(&prepared), 9, |_, operation| {
            assert!(matches!(
                operation,
                GesturePreparation::Plugin(PluginGesturePreparation::Restore { .. })
            ));
            Err(SessionError::PluginPending)
        });
    assert!(pending.pending && !pending.ack.accepted);
    assert_eq!(machine.snapshot(), Some(preview));
    assert_eq!(session.project(), &before);
    let restored =
        machine.handle_prepared(&mut session, cancel, Some(&prepared), 9, |_, operation| {
            assert!(matches!(
                operation,
                GesturePreparation::Plugin(PluginGesturePreparation::Restore { .. })
            ));
            Ok(None)
        });
    assert!(!restored.pending && restored.ack.accepted);
    assert!(machine.snapshot().is_none());
    assert_eq!(session.project(), &before);
}

#[test]
fn exact_source_loss_retires_plugin_input_without_claiming_restore_or_touching_new_keys() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let mut client = GestureClient::default();
    let key = client.begin_plugin(origin).unwrap();
    let mut machine = GestureMachine::default();
    let begin = client.next_request().unwrap();
    client.mark_submitted();
    let accepted =
        machine.handle_prepared(&mut session, begin, Some(&prepared), 9, |_, _| Ok(None));
    assert!(client.acknowledge(&accepted.ack));
    client.change_plugin(value(0.75)).unwrap();
    let change = client.next_request().unwrap();
    client.mark_submitted();
    let lost = machine.handle_prepared(&mut session, change, Some(&prepared), 9, |_, _| {
        Err(SessionError::PluginSourceLost("source loss fixture".into()))
    });
    assert!(!lost.ack.accepted && lost.ack.source_lost && !lost.ack.preview_retained);
    assert!(machine.snapshot().is_none() && lost.playback.is_none());
    assert_eq!(session.project(), &before);
    let mut unrelated = lost.ack.clone();
    unrelated.key.token += 1;
    assert!(!client.acknowledge(&unrelated));
    let mut inconsistent = lost.ack.clone();
    inconsistent.preview_retained = true;
    assert!(!client.acknowledge(&inconsistent));
    assert_eq!(client.snapshot().unwrap().key, key);
    assert!(client.acknowledge(&lost.ack));
    assert!(client.snapshot().is_none() && !client.is_recovery_pending());
    assert!(client.next_request().is_none());
    let next = client.begin_plugin(origin).unwrap();
    assert_ne!(next, key);
    client.next_request().unwrap();
    client.mark_submitted();
    assert!(!client.acknowledge(&lost.ack));
    assert_eq!(client.snapshot().unwrap().key, next);
}

#[test]
fn canonical_rebase_revalidates_plugin_without_replaying_input_or_fabricating_a_revision() {
    let (mut session, origin, prepared) = fixture();
    let key = GestureKey {
        generation: 9,
        token: 1,
    };
    let mut machine = GestureMachine::default();
    assert!(
        machine
            .handle_prepared(
                &mut session,
                GestureRequest::BeginPlugin { key, origin },
                Some(&prepared),
                9,
                |_, _| Ok(None)
            )
            .ack
            .accepted
    );
    let active = machine.snapshot().unwrap();
    session
        .editor
        .execute(Command::AddTrack {
            name: "independent edit".into(),
        })
        .unwrap();
    let canonical = session.project().clone();
    let mut validations = 0;
    let rebased = machine
        .rebase_prepared(&mut session, Some(&prepared), &mut |_, operation| {
            assert!(matches!(operation,
                GesturePreparation::Plugin(PluginGesturePreparation::Preview(snapshot)) if snapshot == active));
            validations += 1;
            Ok(None)
        })
        .unwrap();
    assert!(rebased.ack.accepted && !rebased.ack.source_lost);
    assert!(rebased.playback.is_none() && rebased.preview_project.is_none());
    assert_eq!(validations, 1);
    assert_eq!(machine.snapshot(), Some(active));
    assert_eq!(session.project(), &canonical);

    let pending = machine
        .rebase_prepared(&mut session, Some(&prepared), &mut |_, _| {
            Err(SessionError::PluginPending)
        })
        .unwrap();
    assert!(pending.pending && !pending.ack.accepted && pending.ack.reason.is_none());
    assert_eq!(machine.snapshot(), Some(active));
    assert_eq!(session.project(), &canonical);

    let refused = machine
        .rebase_prepared(&mut session, Some(&prepared), &mut |_, _| {
            Err(SessionError::NativeAudio(
                "controller validation is temporarily busy".into(),
            ))
        })
        .unwrap();
    assert!(!refused.ack.accepted && !refused.ack.source_lost);
    assert!(refused.playback.is_none() && refused.preview_project.is_none());
    assert_eq!(
        machine.snapshot(),
        Some(active),
        "ordinary refusal retains the actual SDK preview"
    );
    assert_eq!(session.project(), &canonical);

    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device: origin.device,
            state: state(3),
        })
        .unwrap();
    let replaced = session.project().clone();
    let retired = machine
        .rebase_prepared(&mut session, Some(&prepared), &mut |_, _| {
            panic!("replaced canonical target cannot reach its old source")
        })
        .unwrap();
    assert!(!retired.ack.accepted && retired.ack.source_lost && !retired.ack.preview_retained);
    assert!(machine.snapshot().is_none());
    assert_eq!(session.project(), &replaced);
}

#[test]
fn plugin_preview_then_captured_commit_is_one_model_history_edge() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let key = GestureKey {
        generation: 9,
        token: 1,
    };
    let mut machine = GestureMachine::default();
    let mut observed = Vec::new();
    let mut prepare = |session: &mut Session, operation| {
        let GesturePreparation::Plugin(operation) = operation else {
            panic!("plugin preparation")
        };
        observed.push(operation);
        match operation {
            PluginGesturePreparation::Begin { .. } => Ok(None),
            PluginGesturePreparation::Preview(snapshot) => {
                assert!(matches!(
                    snapshot.target,
                    GestureTarget::PluginParameter { parameter: 0, .. }
                ));
                assert_eq!(session.project(), &before);
                Ok(None)
            }
            PluginGesturePreparation::Commit(_) => {
                session.editor.execute(Command::ReplaceDeviceState {
                    device: origin.device,
                    state: state(2),
                })?;
                Ok(Some((
                    Arc::new(session.project().clone()),
                    Arc::clone(&prepared),
                )))
            }
            PluginGesturePreparation::Restore { .. } => panic!("different value must commit"),
        }
    };
    for request in [
        GestureRequest::BeginPlugin { key, origin },
        GestureRequest::ChangePlugin {
            key,
            sequence: 1,
            value: value(0.333_333_333_333_333_3),
        },
        GestureRequest::CommitPlugin {
            key,
            sequence: 2,
            value: value(0.666_666_666_666_666_6),
        },
    ] {
        let outcome =
            machine.handle_prepared(&mut session, request, Some(&prepared), 9, &mut prepare);
        assert!(outcome.ack.accepted, "{:?}", outcome.ack.reason);
    }
    assert!(matches!(
        observed.as_slice(),
        [
            PluginGesturePreparation::Begin { .. },
            PluginGesturePreparation::Preview(_),
            PluginGesturePreparation::Preview(_),
            PluginGesturePreparation::Commit(_)
        ]
    ));
    assert!(machine.snapshot().is_none());
    let committed = session.project().clone();
    assert_ne!(committed, before);
    // This is the Editor history oracle; Session's playback-aware undo needs
    // an actual foreign host and is covered by production integration tests.
    assert!(session.editor.undo());
    assert_eq!(session.project(), &before);
    assert!(session.editor.redo());
    assert_eq!(session.project(), &committed);
    assert!(!session.editor.redo());
}

#[test]
fn refused_commit_keeps_acknowledged_final_preview_until_exact_cancel() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let mut client = GestureClient::default();
    let key = client.begin_plugin(origin).unwrap();
    let mut machine = GestureMachine::default();
    let mut restored = None;
    let mut prepare = |_session: &mut Session, operation| match operation {
        GesturePreparation::Plugin(PluginGesturePreparation::Begin { .. }) => Ok(None),
        GesturePreparation::Plugin(PluginGesturePreparation::Preview(_)) => Ok(None),
        GesturePreparation::Plugin(PluginGesturePreparation::Commit(_)) => {
            Err(SessionError::Invalid("canonical refusal fixture"))
        }
        GesturePreparation::Plugin(PluginGesturePreparation::Restore { initial, .. }) => {
            restored = Some(initial);
            Ok(None)
        }
        _ => panic!("plugin preparation"),
    };
    let begin = client.next_request().unwrap();
    client.mark_submitted();
    let accepted = machine.handle_prepared(&mut session, begin, Some(&prepared), 9, &mut prepare);
    assert!(client.acknowledge(&accepted.ack));
    let final_value = value(0.750_000_000_000_000_1);
    client.finish_plugin(final_value).unwrap();
    let commit = client.next_request().unwrap();
    client.mark_submitted();
    let refused = machine.handle_prepared(&mut session, commit, Some(&prepared), 9, &mut prepare);
    assert!(!refused.ack.accepted && refused.ack.preview_retained);
    assert_eq!(machine.snapshot().unwrap().plugin_value, Some(final_value));
    assert!(client.acknowledge_with_snapshot(&refused.ack, machine.snapshot()));
    assert!(client.is_recovery_pending());
    assert!(client.next_request().is_none());
    let unrelated = machine.handle_prepared(
        &mut session,
        GestureRequest::Cancel {
            key: GestureKey {
                token: key.token + 1,
                ..key
            },
        },
        Some(&prepared),
        9,
        |_, _| panic!("unrelated cancel cannot reach host"),
    );
    assert!(!unrelated.ack.accepted);
    assert_eq!(machine.snapshot().unwrap().key, key);
    client.cancel().unwrap();
    let cancel = client.next_request().unwrap();
    assert!(matches!(cancel, GestureRequest::Cancel { key: actual } if actual == key));
    client.mark_submitted();
    let cancelled = machine.handle_prepared(&mut session, cancel, Some(&prepared), 9, &mut prepare);
    assert!(cancelled.ack.accepted);
    assert!(client.acknowledge(&cancelled.ack));
    assert_eq!(restored, Some(origin.initial));
    assert!(machine.snapshot().is_none() && client.snapshot().is_none());
    assert_eq!(session.project(), &before);
}

#[test]
fn end_at_initial_restores_without_a_counter_only_history_edit_and_replacement_refuses() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let key = GestureKey {
        generation: 9,
        token: 1,
    };
    let mut machine = GestureMachine::default();
    let mut restores = 0;
    let mut prepare = |_session: &mut Session, operation| match operation {
        GesturePreparation::Plugin(PluginGesturePreparation::Begin { .. }) => Ok(None),
        GesturePreparation::Plugin(PluginGesturePreparation::Preview(_)) => Ok(None),
        GesturePreparation::Plugin(PluginGesturePreparation::Restore { initial, .. }) => {
            assert_eq!(initial, origin.initial);
            restores += 1;
            Ok(None)
        }
        _ => panic!("no opaque commit for unchanged parameter"),
    };
    assert!(
        machine
            .handle_prepared(
                &mut session,
                GestureRequest::BeginPlugin { key, origin },
                Some(&prepared),
                9,
                &mut prepare
            )
            .ack
            .accepted
    );
    let terminal = machine.handle_prepared(
        &mut session,
        GestureRequest::CommitPlugin {
            key,
            sequence: 1,
            value: origin.initial,
        },
        Some(&prepared),
        9,
        &mut prepare,
    );
    assert!(terminal.ack.accepted && !terminal.canonical_changed);
    assert_eq!(session.project(), &before);
    assert_eq!(restores, 1);
    let key = GestureKey { token: 2, ..key };
    assert!(
        machine
            .handle_prepared(
                &mut session,
                GestureRequest::BeginPlugin { key, origin },
                Some(&prepared),
                9,
                |_, _| Ok(None)
            )
            .ack
            .accepted
    );
    session
        .editor
        .execute(Command::ReplaceDeviceState {
            device: origin.device,
            state: state(3),
        })
        .unwrap();
    let replaced = session.project().clone();
    let refused = machine.handle_prepared(
        &mut session,
        GestureRequest::ChangePlugin {
            key,
            sequence: 1,
            value: value(0.5),
        },
        Some(&prepared),
        9,
        |_, _| panic!("replaced target cannot reach host"),
    );
    assert!(!refused.ack.accepted);
    assert_eq!(session.project(), &replaced);
}
