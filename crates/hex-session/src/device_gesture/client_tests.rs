//! Unit tests for `GestureClient` coalescing, terminal custody, and paired edits.
use super::*;

fn device() -> DeviceId {
    let mut session = Session::new(48_000, 4, 0).unwrap();
    let (track, _) = session
        .command_prepared_revision(
            hex_project::Command::AddTrack {
                name: "test".into(),
            },
            None,
        )
        .unwrap();
    let track = match track {
        hex_project::Created::Track(track) => track,
        _ => unreachable!(),
    };
    let type_id = DeviceTypeId::new("hex.utility").unwrap();
    let state = hex_engine::default_native_device_state(type_id).unwrap();
    let (created, _) = session
        .command_prepared_revision(
            hex_project::Command::InsertDevice {
                track,
                before: None,
                state,
            },
            None,
        )
        .unwrap();
    match created {
        hex_project::Created::Device(device) => device,
        _ => unreachable!(),
    }
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
fn retained_terminal_uses_worker_snapshot_and_next_begin_only_requests_cancel() {
    let device = device();
    let parameter = ParameterId::from_static(1);
    let target = GestureTarget::DeviceParameter { device, parameter };
    for next_kind in 0..3 {
        let mut client = GestureClient::default();
        client.begin(7, target, ParameterValue::Float(1.0)).unwrap();
        let begin = client.next_request().unwrap();
        assert!(client.acknowledge(&ack(begin, true)));
        client.change(ParameterValue::Float(0.5)).unwrap();
        let change = client.next_request().unwrap();
        assert!(client.acknowledge(&ack(change, true)));
        let accepted = client.snapshot().unwrap();
        client.finish(ParameterValue::Float(0.9)).unwrap();
        let terminal = client.next_request().unwrap();
        client.mark_submitted();
        let mut refused = ack(terminal, false);
        refused.preview_retained = true;
        assert!(!client.acknowledge(&refused));
        assert!(!client.acknowledge_with_snapshot(&refused, None));
        let mut foreign = accepted;
        foreign.key.token += 1;
        assert!(!client.acknowledge_with_snapshot(&refused, Some(foreign)));
        assert!(client.has_pending_terminal());
        assert!(client.acknowledge_with_snapshot(&refused, Some(accepted)));
        assert_eq!(client.snapshot(), Some(accepted));
        assert!(client.is_recovery_pending());
        assert!(
            client.next_request().is_none(),
            "permanent refusal cannot spin Commit"
        );
        client.change(ParameterValue::Float(0.6)).unwrap();
        let change = client.next_request().unwrap();
        assert!(client.acknowledge(&ack(change, true)));
        assert!(
            client.is_recovery_pending(),
            "accepted overlay changes cannot release terminal dependents"
        );
        let accepted = client.snapshot().unwrap();
        let declined = match next_kind {
            0 => client.begin(7, target, ParameterValue::Float(0.8)),
            1 => client.begin_pair(
                7,
                device,
                parameter,
                ParameterId::from_static(2),
                ParameterPairValue {
                    first: ParameterValue::Float(0.8),
                    second: ParameterValue::Float(0.9),
                },
            ),
            _ => client.begin_snapshot(
                7,
                device,
                DeviceParameterSnapshot::new(&[(parameter, ParameterValue::Float(0.8))]).unwrap(),
            ),
        };
        assert!(declined.unwrap_err().contains("retry after"));
        assert_eq!(client.snapshot(), Some(accepted));
        assert!(client.change(ParameterValue::Float(0.8)).is_err());
        let cancel = client.next_request().unwrap();
        assert!(matches!(cancel, GestureRequest::Cancel { .. }));
        let mut cancel_refused = ack(cancel, false);
        cancel_refused.preview_retained = true;
        assert!(client.acknowledge_with_snapshot(&cancel_refused, Some(accepted)));
        assert!(client.next_request().is_none());
        client.cancel().unwrap();
        let retry = client.next_request().unwrap();
        assert!(client.acknowledge(&ack(retry, true)));
        assert!(client.snapshot().is_none());
        assert!(!client.is_recovery_pending());
        assert!(client.begin(7, target, ParameterValue::Float(1.0)).is_ok());
    }
}

#[test]
fn stranded_commit_overlays_released_pair_values_while_refused_cancel_stays_unreconciled() {
    let mut session = Session::new(48_000, 4, 0).unwrap();
    let (track, _) = session
        .command_prepared_revision(
            hex_project::Command::AddTrack {
                name: "eq-track".into(),
            },
            None,
        )
        .unwrap();
    let hex_project::Created::Track(track) = track else {
        unreachable!();
    };
    let mut eq = hex_engine::eq::EqState::decode(
        &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.eq")).unwrap(),
    )
    .unwrap();
    let node_id = if let Some(first_node) = eq.nodes.first() {
        first_node.id
    } else {
        let id = hex_engine::eq::NodeId::new(1).unwrap();
        eq.add(hex_engine::eq::Node::new(id, 100.0, 0.0, 0).unwrap())
            .unwrap();
        id
    };
    let state = eq.encode().unwrap();
    let (created, _) = session
        .command_prepared_revision(
            hex_project::Command::InsertDevice {
                track,
                before: None,
                state,
            },
            None,
        )
        .unwrap();
    let hex_project::Created::Device(device) = created else {
        unreachable!();
    };
    let first = node_id
        .parameter(hex_engine::eq::NodeParameter::Frequency)
        .unwrap();
    let second = node_id
        .parameter(hex_engine::eq::NodeParameter::Gain)
        .unwrap();
    let mut client = GestureClient::default();
    client
        .begin_pair(
            7,
            device,
            first,
            second,
            ParameterPairValue {
                first: ParameterValue::Float(100.0),
                second: ParameterValue::Float(0.0),
            },
        )
        .unwrap();
    let begin = client.next_request().unwrap();
    assert!(client.acknowledge(&ack(begin, true)));
    client
        .change_pair(ParameterPairValue {
            first: ParameterValue::Float(320.0),
            second: ParameterValue::Float(3.5),
        })
        .unwrap();
    let change = client.next_request().unwrap();
    assert!(client.acknowledge(&ack(change, true)));
    let accepted = client.snapshot().unwrap();
    assert!(!client.has_stranded_commit());
    assert!(client.reconcilable_snapshot().is_none());

    client
        .finish_pair(ParameterPairValue {
            first: ParameterValue::Float(480.0),
            second: ParameterValue::Float(-4.5),
        })
        .unwrap();
    let commit = client.next_request().unwrap();
    client.mark_submitted();
    let mut refused = ack(commit, false);
    refused.preview_retained = true;
    assert!(client.acknowledge_with_snapshot(&refused, Some(accepted)));
    assert!(client.is_recovery_pending());
    assert!(!client.has_pending_requests());
    assert!(client.has_stranded_commit());

    let reconcilable = client
        .reconcilable_snapshot()
        .expect("released pair values must survive refused CommitPair");
    assert_eq!(reconcilable.value, ParameterValue::Float(480.0));
    assert_eq!(
        reconcilable.secondary_value,
        Some(ParameterValue::Float(-4.5))
    );
    assert!(reconcilable.manual_edit);
    let command = reconcile_command_for_project(session.project(), reconcilable)
        .unwrap()
        .expect("native EQ pair must produce canonical command");
    session.command_prepared_revision(command, None).unwrap();
    let decoded =
        hex_engine::eq::EqState::decode(session.project().device(device).unwrap().state()).unwrap();
    assert!((decoded.nodes[0].frequency_hz - 480.0).abs() < 1e-3);
    assert!((decoded.nodes[0].gain_db - (-4.5)).abs() < 1e-3);

    // An explicit Cancel refused with preview_retained clears stranded_commit.
    client.cancel().unwrap();
    let cancel = client.next_request().unwrap();
    client.mark_submitted();
    let mut cancel_refused = ack(cancel, false);
    cancel_refused.preview_retained = true;
    assert!(client.acknowledge_with_snapshot(&cancel_refused, Some(accepted)));
    assert!(client.is_recovery_pending());
    assert!(!client.has_stranded_commit());
    assert!(client.reconcilable_snapshot().is_none());

    // Host-owned PluginParameter snapshots return Ok(None) so callers account for them.
    let mut plugin_snapshot = reconcilable;
    plugin_snapshot.target = GestureTarget::PluginParameter {
        device,
        parameter: 0,
    };
    plugin_snapshot.secondary_value = None;
    plugin_snapshot.plugin_value = Some(PluginParameterValue::new(0.5).unwrap());
    assert!(
        reconcile_command_for_project(session.project(), plugin_snapshot)
            .unwrap()
            .is_none()
    );
}

#[test]
fn coalesces_changes_and_preserves_final_value_until_terminal_ack() {
    let mut client = GestureClient::default();
    let target = GestureTarget::DeviceParameter {
        device: device(),
        parameter: ParameterId::new(1).unwrap(),
    };
    client.begin(0, target, ParameterValue::Float(1.0)).unwrap();
    let begin = client.next_request().unwrap();
    assert!(matches!(begin, GestureRequest::Begin { .. }));
    client.mark_submitted();
    assert_eq!(client.next_request(), None);

    client.change(ParameterValue::Float(2.0)).unwrap();
    client.change(ParameterValue::Float(3.0)).unwrap();
    client.finish(ParameterValue::Float(4.0)).unwrap();
    assert_eq!(
        client.change(ParameterValue::Float(5.0)),
        Err("device gesture is finishing")
    );
    assert!(client.acknowledge(&ack(begin, true)));
    let terminal = client.next_request().unwrap();
    assert_eq!(
        terminal,
        GestureRequest::Commit {
            manual_edit: true,
            key: begin.key(),
            sequence: 3,
            value: ParameterValue::Float(4.0),
        }
    );
    client.mark_submitted();
    assert_eq!(client.next_request(), None);
    assert!(client.acknowledge(&ack(terminal, true)));
    assert!(client.snapshot().is_none());
}

#[test]
fn admitted_terminal_cannot_be_replaced_before_its_ack() {
    let mut client = GestureClient::default();
    let target = GestureTarget::DeviceParameter {
        device: device(),
        parameter: ParameterId::new(1).unwrap(),
    };
    client.begin(0, target, ParameterValue::Float(1.0)).unwrap();
    let begin = client.next_request().unwrap();
    client.mark_submitted();
    assert!(client.acknowledge(&ack(begin, true)));
    client.finish(ParameterValue::Float(2.0)).unwrap();
    let terminal = client.next_request().unwrap();
    client.mark_submitted();

    assert_eq!(
        client.finish(ParameterValue::Float(3.0)),
        Err("device gesture is already finishing")
    );
    assert_eq!(client.cancel(), Err("device gesture is already finishing"));
    assert_eq!(client.next_request(), None);
    assert_eq!(
        terminal,
        GestureRequest::Commit {
            manual_edit: true,
            key: begin.key(),
            sequence: 1,
            value: ParameterValue::Float(2.0),
        }
    );
    assert!(client.acknowledge(&ack(terminal, true)));
    assert!(client.snapshot().is_none());
}

#[test]
fn queue_full_retry_keeps_exact_reserved_request_and_ignores_stale_ack() {
    let mut client = GestureClient::default();
    let target = GestureTarget::DeviceParameter {
        device: device(),
        parameter: ParameterId::new(1).unwrap(),
    };
    client.begin(2, target, ParameterValue::Float(1.0)).unwrap();
    let begin = client.next_request().unwrap();
    assert_eq!(client.next_request(), Some(begin));
    let wrong = GestureAck {
        preview_retained: false,
        source_lost: false,
        key: GestureKey {
            generation: 2,
            token: 99,
        },
        phase: GesturePhase::Begin,
        sequence: 0,
        accepted: true,
        reason: None,
    };
    assert!(!client.acknowledge(&wrong));
    assert_eq!(client.next_request(), Some(begin));
    client.mark_submitted();
    client.change(ParameterValue::Float(2.0)).unwrap();
    client.finish(ParameterValue::Float(3.0)).unwrap();
    assert!(client.acknowledge(&ack(begin, true)));
    let terminal = client.next_request().unwrap();
    assert!(matches!(
        terminal,
        GestureRequest::Commit {
            value: ParameterValue::Float(3.0),
            ..
        }
    ));
}

#[test]
fn paired_client_coalesces_two_values_as_one_terminal_command() {
    let mut client = GestureClient::default();
    let device = device();
    let target_first = ParameterId::new(10).unwrap();
    let target_second = ParameterId::new(11).unwrap();
    let initial = ParameterPairValue {
        first: ParameterValue::Float(1_000.0),
        second: ParameterValue::Float(0.0),
    };
    client
        .begin_pair(3, device, target_first, target_second, initial)
        .unwrap();
    let begin = client.next_request().unwrap();
    client.mark_submitted();
    client
        .change_pair(ParameterPairValue {
            first: ParameterValue::Float(2_000.0),
            second: ParameterValue::Float(2.0),
        })
        .unwrap();
    client
        .change_pair(ParameterPairValue {
            first: ParameterValue::Float(3_000.0),
            second: ParameterValue::Float(3.0),
        })
        .unwrap();
    client
        .finish_pair(ParameterPairValue {
            first: ParameterValue::Float(4_000.0),
            second: ParameterValue::Float(4.0),
        })
        .unwrap();
    assert_eq!(
        client.change_pair(initial),
        Err("device gesture is finishing")
    );
    assert!(client.acknowledge(&ack(begin, true)));
    let terminal = client.next_request().unwrap();
    assert_eq!(
        terminal,
        GestureRequest::CommitPair {
            manual_edit: true,
            key: begin.key(),
            sequence: 3,
            value: ParameterPairValue {
                first: ParameterValue::Float(4_000.0),
                second: ParameterValue::Float(4.0),
            },
        }
    );
    assert_eq!(client.next_request(), Some(terminal));
    client.mark_submitted();
    assert!(client.acknowledge(&ack(terminal, true)));
    assert!(client.snapshot().is_none());
}

#[test]
fn paired_commit_is_atomic_and_undoes_as_one_state_change() {
    let mut session = Session::new(48_000, 4, 0).unwrap();
    let (created_track, _) = session
        .command_prepared_revision(
            hex_project::Command::AddTrack {
                name: "test".into(),
            },
            None,
        )
        .unwrap();
    let hex_project::Created::Track(track) = created_track else {
        panic!("track command returned an unexpected object");
    };
    let eq8_type = DeviceTypeId::new("hex.eq8").unwrap();
    let state = hex_engine::default_native_device_state(eq8_type).unwrap();
    let (created_device, prepared) = session
        .command_prepared_revision(
            hex_project::Command::InsertDevice {
                track,
                before: None,
                state,
            },
            None,
        )
        .unwrap();
    let hex_project::Created::Device(device) = created_device else {
        panic!("device command returned an unexpected object");
    };
    let original = session.project().device(device).unwrap().state().clone();
    let first = ParameterId::new(18).unwrap();
    let second = ParameterId::new(19).unwrap();
    assert!(
        session
            .preview_device_parameter_pair(
                device,
                (first, ParameterValue::Float(2_000.0)),
                (first, ParameterValue::Float(2.0)),
                Some(&prepared),
            )
            .is_err()
    );
    assert!(
        session
            .commit_device_parameter_pair(
                device,
                (first, ParameterValue::Float(2_000.0)),
                (first, ParameterValue::Float(2.0)),
                Some(&prepared),
            )
            .is_err()
    );
    assert_eq!(session.project().device(device).unwrap().state(), &original);
    let key = GestureKey {
        generation: 1,
        token: 1,
    };
    let mut machine = GestureMachine::default();
    let begun = machine.handle(
        &mut session,
        GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameterPair {
                device,
                first,
                second,
            },
        },
        Some(&prepared),
        1,
    );
    assert!(begun.ack.accepted);
    let changed = machine.handle(
        &mut session,
        GestureRequest::ChangePair {
            key,
            sequence: 1,
            value: ParameterPairValue {
                first: ParameterValue::Float(1_500.0),
                second: ParameterValue::Float(6.0),
            },
        },
        Some(&prepared),
        1,
    );
    assert!(changed.ack.accepted);
    let preview_state = changed
        .preview_project
        .as_ref()
        .unwrap()
        .device(device)
        .unwrap()
        .state();
    assert_eq!(
        hex_engine::native_device_parameter(preview_state, first).unwrap(),
        ParameterValue::Float(1_500.0)
    );
    assert_eq!(
        hex_engine::native_device_parameter(preview_state, second).unwrap(),
        ParameterValue::Float(6.0)
    );
    assert_eq!(session.project().device(device).unwrap().state(), &original);

    let committed = machine.handle(
        &mut session,
        GestureRequest::CommitPair {
            manual_edit: true,
            key,
            sequence: 2,
            value: ParameterPairValue {
                first: ParameterValue::Float(1_500.0),
                second: ParameterValue::Float(6.0),
            },
        },
        Some(&prepared),
        1,
    );
    assert!(committed.ack.accepted);
    assert!(committed.canonical_changed);
    let committed_state = session.project().device(device).unwrap().state();
    assert_eq!(
        hex_engine::native_device_parameter(committed_state, first).unwrap(),
        ParameterValue::Float(1_500.0)
    );
    assert_eq!(
        hex_engine::native_device_parameter(committed_state, second).unwrap(),
        ParameterValue::Float(6.0)
    );
    assert!(session.undo());
    assert_eq!(session.project().device(device).unwrap().state(), &original);

    let before = session.project().clone();
    let result = session.commit_device_parameter_pair(
        device,
        (first, ParameterValue::Float(2_000.0)),
        (second, ParameterValue::Float(f32::NAN)),
        Some(&prepared),
    );
    assert!(result.is_err());
    assert_eq!(session.project(), &before);
    assert_eq!(session.project().device(device).unwrap().state(), &original);
}

#[test]
fn paired_client_suppresses_unchanged_samples_and_keeps_reversal_retry_invalidation_and_terminals()
{
    let mut client = GestureClient::default();
    let device = device();
    let first = ParameterId::new(10).unwrap();
    let second = ParameterId::new(11).unwrap();
    let initial = ParameterPairValue {
        first: ParameterValue::Float(1_000.0),
        second: ParameterValue::Float(0.0),
    };
    let moved = ParameterPairValue {
        first: ParameterValue::Float(1_500.0),
        second: ParameterValue::Float(3.5),
    };
    let reversed = ParameterPairValue {
        first: ParameterValue::Float(635.8688),
        second: ParameterValue::Float(-9.05508),
    };
    client
        .begin_pair(5, device, first, second, initial)
        .unwrap();
    let begin = client.next_request().unwrap();
    client.mark_submitted();
    assert!(client.acknowledge(&ack(begin, true)));

    // Meaningful first touch with identical initial values must still emit.
    assert_eq!(client.change_pair(initial).unwrap(), 1);
    let first_touch = client.next_request().unwrap();
    assert_eq!(
        first_touch,
        GestureRequest::ChangePair {
            key: begin.key(),
            sequence: 1,
            value: initial,
        }
    );
    // Repeated identical sample while in-flight is suppressed.
    client.mark_submitted();
    assert_eq!(client.change_pair(initial).unwrap(), 1);
    assert!(client.acknowledge(&ack(first_touch, true)));
    assert_eq!(client.next_request(), None);

    // Burst of identical pointer samples after acceptance emits zero requests.
    for _ in 0..10 {
        assert_eq!(client.change_pair(initial).unwrap(), 1);
        assert_eq!(client.next_request(), None);
    }

    // Changed value emits; duplicate while queued does not advance sequence.
    assert_eq!(client.change_pair(moved).unwrap(), 2);
    assert_eq!(client.change_pair(moved).unwrap(), 2);
    let req_moved = client.next_request().unwrap();
    client.mark_submitted();

    // Rapid reversal while `moved` is in-flight queues `initial`.
    assert_eq!(client.change_pair(initial).unwrap(), 3);
    assert!(client.acknowledge(&ack(req_moved, true)));
    let req_back = client.next_request().unwrap();
    assert_eq!(
        req_back,
        GestureRequest::ChangePair {
            key: begin.key(),
            sequence: 3,
            value: initial,
        }
    );
    client.mark_submitted();
    // Failed ChangePair clears acceptance so retrying the same value emits.
    assert!(client.acknowledge(&ack(req_back, false)));
    assert_eq!(client.change_pair(initial).unwrap(), 4);
    let retry = client.next_request().unwrap();
    assert_eq!(retry.sequence(), 4);
    client.mark_submitted();
    assert!(client.acknowledge(&ack(retry, true)));
    assert_eq!(client.next_request(), None);

    // Explicit invalidation (e.g. lost/recreated output) allows re-sending unchanged value.
    client.invalidate_accepted_preview();
    assert_eq!(client.change_pair(initial).unwrap(), 5);
    let after_reset = client.next_request().unwrap();
    assert_eq!(after_reset.sequence(), 5);
    client.mark_submitted();
    assert!(client.acknowledge(&ack(after_reset, true)));

    // Final value, burst of repeats, and terminal CommitPair settle cleanly.
    assert_eq!(client.change_pair(reversed).unwrap(), 6);
    let req_rev = client.next_request().unwrap();
    client.mark_submitted();
    assert!(client.acknowledge(&ack(req_rev, true)));
    for _ in 0..5 {
        assert_eq!(client.change_pair(reversed).unwrap(), 6);
        assert_eq!(client.next_request(), None);
    }
    client.finish_pair(reversed).unwrap();
    let commit = client.next_request().unwrap();
    assert_eq!(
        commit,
        GestureRequest::CommitPair {
            manual_edit: true,
            key: begin.key(),
            sequence: 7,
            value: reversed,
        }
    );
    client.mark_submitted();
    assert!(client.acknowledge(&ack(commit, true)));
    assert!(client.snapshot().is_none());
}

#[test]
fn paired_worker_skips_unchanged_pair_until_change_rebase_or_owner_loss() {
    let mut session = Session::new(48_000, 4, 0).unwrap();
    let (hex_project::Created::Track(track), _) = session
        .command_prepared_revision(
            hex_project::Command::AddTrack {
                name: "pair dedup".into(),
            },
            None,
        )
        .unwrap()
    else {
        panic!("track");
    };
    let state =
        hex_engine::default_native_device_state(DeviceTypeId::new("hex.eq8").unwrap()).unwrap();
    let (hex_project::Created::Device(device), prepared) = session
        .command_prepared_revision(
            hex_project::Command::InsertDevice {
                track,
                before: None,
                state,
            },
            None,
        )
        .unwrap()
    else {
        panic!("device");
    };
    let first = ParameterId::new(18).unwrap();
    let second = ParameterId::new(19).unwrap();
    let key = GestureKey {
        generation: 1,
        token: 1,
    };
    let pair_a = ParameterPairValue {
        first: ParameterValue::Float(635.8688),
        second: ParameterValue::Float(-9.05508),
    };
    let pair_b = ParameterPairValue {
        first: ParameterValue::Float(1_200.0),
        second: ParameterValue::Float(3.0),
    };
    let mut machine = GestureMachine::default();
    assert!(
        machine
            .handle(
                &mut session,
                GestureRequest::Begin {
                    key,
                    target: GestureTarget::DeviceParameterPair {
                        device,
                        first,
                        second,
                    },
                },
                Some(&prepared),
                1,
            )
            .ack
            .accepted
    );

    let first_preview = machine.handle(
        &mut session,
        GestureRequest::ChangePair {
            key,
            sequence: 1,
            value: pair_a,
        },
        Some(&prepared),
        1,
    );
    assert!(first_preview.ack.accepted);
    assert!(first_preview.playback.is_some());

    // Identical values against the same canonical prepared owner skip rebuild.
    for sequence in 2..=6 {
        let repeated = machine.handle(
            &mut session,
            GestureRequest::ChangePair {
                key,
                sequence,
                value: pair_a,
            },
            Some(&prepared),
            1,
        );
        assert!(repeated.ack.accepted);
        assert_eq!(repeated.ack.sequence, sequence);
        assert!(repeated.playback.is_none());
        assert!(repeated.preview_project.is_none());
    }

    // Canonical edit (different PreparedRevision) forces preparation even for same pair.
    let (_, prepared_after_mute) = session
        .command_prepared_revision(hex_project::Command::ToggleMute(track), Some(&prepared))
        .unwrap();
    let after_canonical = machine.handle(
        &mut session,
        GestureRequest::ChangePair {
            key,
            sequence: 7,
            value: pair_a,
        },
        Some(&prepared_after_mute),
        1,
    );
    assert!(after_canonical.ack.accepted);
    assert!(after_canonical.playback.is_some());

    // Subsequent repeat at the new canonical revision skips again.
    let skipped_again = machine.handle(
        &mut session,
        GestureRequest::ChangePair {
            key,
            sequence: 8,
            value: pair_a,
        },
        Some(&prepared_after_mute),
        1,
    );
    assert!(skipped_again.ack.accepted);
    assert!(skipped_again.playback.is_none());

    // Changed value always prepares and publishes.
    let changed = machine.handle(
        &mut session,
        GestureRequest::ChangePair {
            key,
            sequence: 9,
            value: pair_b,
        },
        Some(&prepared_after_mute),
        1,
    );
    assert!(changed.ack.accepted);
    assert!(changed.playback.is_some());

    // Cancel restores canonical state cleanly.
    let cancelled = machine.handle(
        &mut session,
        GestureRequest::Cancel { key },
        Some(&prepared_after_mute),
        1,
    );
    assert!(cancelled.ack.accepted);
    assert!(machine.snapshot().is_none());
}
