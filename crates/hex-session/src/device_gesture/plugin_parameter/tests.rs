//! Protocol/model oracles only. Native SDK application and canonical production
//! publication are verified separately at their actual host boundaries.
use super::*;
use hex_lattice_contract::NativeDeviceState;
use hex_project::{Command, Created};

fn value(value: f64) -> PluginParameterValue {
    PluginParameterValue::new(value).unwrap()
}
fn state(byte: u8) -> NativeDeviceState {
    hex_plugin_state::encode(hex_plugin_state::State {
        identity: hex_plugin_state::Identity {
            class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
            binary: hex_plugin_state::BinaryIdentity {
                executable_sha256: [2; 32],
                info_sha256: [3; 32],
            },
        },
        component: Some(&[byte]),
        controller: Some(&[byte]),
    })
    .unwrap()
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
fn hostless_begin_refuses_without_canonical_or_gesture_mutation() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let mut machine = GestureMachine::default();
    let request = GestureRequest::BeginPlugin {
        recording: false,
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
fn released_recording_refusal_ends_only_its_matching_terminal_without_source_loss() {
    for recording in [false, true] {
        for cancel in [false, true] {
            let (mut session, origin, prepared) = fixture();
            let before = session.project().clone();
            let mut client = GestureClient::default();
            let key = if recording {
                client.begin_plugin_recording(origin).unwrap()
            } else {
                client.begin_plugin(origin).unwrap()
            };
            let mut machine = GestureMachine::default();
            let begin = client.next_request().unwrap();
            client.mark_submitted();
            let began =
                machine.handle_prepared(&mut session, begin, Some(&prepared), 9, |_, _| Ok(None));
            assert!(client.acknowledge_with_snapshot(&began.ack, machine.snapshot()));
            client.change_plugin(value(0.75)).unwrap();
            let change = client.next_request().unwrap();
            client.mark_submitted();
            let changed =
                machine.handle_prepared(&mut session, change, Some(&prepared), 9, |_, _| Ok(None));
            assert!(client.acknowledge_with_snapshot(&changed.ack, machine.snapshot()));
            if cancel {
                client.cancel().unwrap();
            } else {
                client.finish_plugin(value(0.75)).unwrap();
            }
            let terminal = client.next_request().unwrap();
            client.mark_submitted();
            let refused = machine.handle_prepared(
                &mut session,
                terminal,
                Some(&prepared),
                9,
                |_, operation| match operation {
                    GesturePreparation::Plugin(PluginGesturePreparation::Preview(_)) => Ok(None),
                    GesturePreparation::Plugin(
                        PluginGesturePreparation::Commit(_)
                        | PluginGesturePreparation::Restore { .. },
                    ) => Err(SessionError::PluginRecordingReleased(
                        "recording retained after publication refusal".into(),
                    )),
                    _ => panic!("expected plugin terminal"),
                },
            );
            assert!(!refused.ack.accepted && !refused.ack.source_lost);
            assert_eq!(
                refused.ack.reason.as_deref(),
                Some("recording retained after publication refusal")
            );
            assert_eq!(refused.ack.preview_retained, !recording);
            assert_eq!(machine.snapshot().is_none(), recording);
            assert!(refused.playback.is_none() && !refused.canonical_changed);
            assert_eq!(session.project(), &before);
            assert!(client.acknowledge_with_snapshot(&refused.ack, machine.snapshot()));
            assert_eq!(client.is_recovery_pending(), !recording);
            if recording {
                assert!(client.snapshot().is_none());
                let next = client.begin_plugin_recording(origin).unwrap();
                let begin = client.next_request().unwrap();
                client.mark_submitted();
                let began =
                    machine
                        .handle_prepared(&mut session, begin, Some(&prepared), 9, |_, _| Ok(None));
                let stale =
                    machine.handle_prepared(&mut session, terminal, Some(&prepared), 9, |_, _| {
                        panic!("stale terminal cannot reach the host")
                    });
                assert!(!stale.ack.accepted && !stale.ack.source_lost);
                assert_eq!(machine.snapshot().unwrap().key, next);
                assert!(!client.acknowledge_with_snapshot(&refused.ack, None));
                assert_eq!(client.snapshot().unwrap().key, next);
                assert_ne!(next, key);
                assert!(client.acknowledge_with_snapshot(&began.ack, machine.snapshot()));
            }
        }
    }
}

#[test]
fn released_recording_marker_cannot_retire_a_nonterminal_preview() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let key = GestureKey {
        generation: 9,
        token: 1,
    };
    let mut machine = GestureMachine::default();
    assert!(
        machine
            .handle_prepared(
                &mut session,
                GestureRequest::BeginPlugin {
                    key,
                    origin,
                    recording: true
                },
                Some(&prepared),
                9,
                |_, _| Ok(None),
            )
            .ack
            .accepted
    );
    let active = machine.snapshot();
    let refused = machine.handle_prepared(
        &mut session,
        GestureRequest::ChangePlugin {
            key,
            sequence: 1,
            value: value(0.75),
        },
        Some(&prepared),
        9,
        |_, _| {
            Err(SessionError::PluginRecordingReleased(
                "invalid nonterminal marker".into(),
            ))
        },
    );
    assert!(!refused.ack.accepted && !refused.ack.source_lost);
    assert_eq!(machine.snapshot(), active);
    assert_eq!(session.project(), &before);
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
                GestureRequest::BeginPlugin {
                    key,
                    origin,
                    recording: false
                },
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
        GestureRequest::BeginPlugin {
            key,
            origin,
            recording: false,
        },
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
    let saved = hex_storage::encode(&committed).unwrap();
    let reopened = hex_storage::decode(&saved).unwrap();
    assert_eq!(reopened, committed);
    assert_eq!(reopened.device(origin.device).unwrap().state(), &state(2));
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
                GestureRequest::BeginPlugin {
                    key,
                    origin,
                    recording: false
                },
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
                GestureRequest::BeginPlugin {
                    key,
                    origin,
                    recording: false
                },
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

#[test]
fn recorded_return_to_initial_preserves_the_trajectory_terminal() {
    for recording in [false, true] {
        let (mut session, origin, prepared) = fixture();
        let before = session.project().clone();
        let mut client = GestureClient::default();
        let key = if recording {
            client.begin_plugin_recording(origin).unwrap()
        } else {
            client.begin_plugin(origin).unwrap()
        };
        let mut machine = GestureMachine::default();
        let mut observed = Vec::new();
        for step in 0..3 {
            match step {
                1 => {
                    client.change_plugin(value(0.75)).unwrap();
                }
                2 => client.finish_plugin(origin.initial).unwrap(),
                _ => {}
            }
            let request = client.next_request().unwrap();
            client.mark_submitted();
            let outcome = machine.handle_prepared(
                &mut session,
                request,
                Some(&prepared),
                9,
                |session, operation| {
                    let GesturePreparation::Plugin(operation) = operation else {
                        panic!("plugin")
                    };
                    observed.push(operation);
                    Ok(matches!(operation, PluginGesturePreparation::Commit(_))
                        .then(|| (Arc::new(session.project().clone()), Arc::clone(&prepared))))
                },
            );
            assert!(outcome.ack.accepted, "{:?}", outcome.ack.reason);
            assert!(client.acknowledge(&outcome.ack));
        }
        assert!(
            matches!(observed.first(), Some(PluginGesturePreparation::Begin { recording: actual, .. }) if *actual == recording)
        );
        if recording {
            assert!(
                matches!(observed.last(), Some(PluginGesturePreparation::Commit(snapshot)) if snapshot.key == key && snapshot.sequence == 2 && snapshot.plugin_value == Some(origin.initial))
            );
        } else {
            assert!(
                matches!(observed.last(), Some(PluginGesturePreparation::Restore { snapshot, .. }) if snapshot.key == key && snapshot.sequence == 2)
            );
        }
        assert!(machine.snapshot().is_none());
        assert!(client.snapshot().is_none());
        assert_eq!(
            session.project(),
            &before,
            "protocol oracle creates no history itself"
        );
    }
}

#[test]
fn refused_recorded_return_keeps_the_exact_final_preview_until_cancel() {
    let (mut session, origin, prepared) = fixture();
    let before = session.project().clone();
    let mut client = GestureClient::default();
    let key = client.begin_plugin_recording(origin).unwrap();
    let mut machine = GestureMachine::default();
    for step in 0..3 {
        match step {
            1 => {
                client.change_plugin(value(0.75)).unwrap();
            }
            2 => client.finish_plugin(origin.initial).unwrap(),
            _ => {}
        }
        let request = client.next_request().unwrap();
        client.mark_submitted();
        let outcome =
            machine.handle_prepared(&mut session, request, Some(&prepared), 9, |_, operation| {
                match operation {
                    GesturePreparation::Plugin(PluginGesturePreparation::Commit(_)) => Err(
                        SessionError::NativeAudio("writer still owns the recorded take".into()),
                    ),
                    GesturePreparation::Plugin(PluginGesturePreparation::Restore { .. }) => {
                        panic!("recorded trajectory must reach commit")
                    }
                    _ => Ok(None),
                }
            });
        if step < 2 {
            assert!(outcome.ack.accepted);
            assert!(client.acknowledge(&outcome.ack));
        } else {
            assert!(!outcome.ack.accepted && outcome.ack.preview_retained);
            assert_eq!(
                machine.snapshot().unwrap().plugin_value,
                Some(origin.initial)
            );
            assert_eq!(machine.snapshot().unwrap().sequence, 2);
            let mut stale = outcome.ack.clone();
            stale.key.token += 1;
            assert!(!client.acknowledge_with_snapshot(&stale, machine.snapshot()));
            assert!(client.acknowledge_with_snapshot(&outcome.ack, machine.snapshot()));
            assert!(client.is_recovery_pending());
        }
    }
    client.cancel().unwrap();
    let request = client.next_request().unwrap();
    assert_eq!(request.key(), key);
    let cancelled = machine.handle_prepared(&mut session, request, Some(&prepared), 9, |_, operation| {
        assert!(matches!(operation, GesturePreparation::Plugin(PluginGesturePreparation::Restore { snapshot, .. }) if snapshot.key == key && snapshot.plugin_value == Some(origin.initial)));
        Ok(None)
    });
    assert!(cancelled.ack.accepted);
    assert!(client.acknowledge(&cancelled.ack));
    assert!(machine.snapshot().is_none());
    assert!(client.snapshot().is_none());
    assert_eq!(session.project(), &before);
}
