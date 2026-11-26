use super::*;

#[test]
fn action_outcomes_require_a_vacant_matching_constructor_and_account_storage() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (_, mut foreign) = channel(48_000).unwrap();
    assert!(
        control
            .enable_action_outcomes_on_control(&mut foreign)
            .is_err()
    );
    let before = callback.constructor_channel_storage().unwrap();
    let reader = control
        .enable_action_outcomes_on_control(&mut callback)
        .unwrap();
    let after = callback.constructor_channel_storage().unwrap();
    assert_eq!(
        after.data_bytes - before.data_bytes,
        reader.retained_data_bytes()
    );
    assert_eq!(after.shared_groups - before.shared_groups, 1);
    assert_eq!(after.shared_handles - before.shared_handles, 3);
    assert!(
        control
            .enable_action_outcomes_on_control(&mut callback)
            .is_err()
    );
    let (mut late, mut late_callback) = channel(48_000).unwrap();
    late_callback.begin_block();
    assert!(
        late.enable_action_outcomes_on_control(&mut late_callback)
            .is_err()
    );
}

#[test]
fn legacy_quiescent_retirement_refuses_foreign_callback_and_interrupts_pending() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (_, mut foreign) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    control.request_shutdown();
    assert!(
        control
            .retire_action_outcomes_on_control(&mut foreign)
            .is_err()
    );
    assert_eq!(reader.try_take(), None);
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    assert_eq!(reader.failure(), None);
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    callback.begin_block();
    control.reclaim();
}

#[test]
fn abandoning_legacy_endpoint_reports_unknown_instead_of_a_terminal() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    control.command_with_ticket(PlaybackCommand::Play).unwrap();
    drop(control);
    assert_eq!(
        reader.failure(),
        Some(ActionOutcomeFailure::EndpointAbandoned)
    );
    assert_eq!(reader.try_take(), None);
    drop(callback);
}

#[test]
fn resident_action_outcomes_preserve_distinct_rejections_before_polling() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    let fence = control.capture_fresh_runtime_fence().unwrap();
    control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let no_graph = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    begin_outcome_test_block(&mut callback);
    control.reclaim();
    control.publish(graph().0).unwrap();
    let metronome = control
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    begin_outcome_test_block(&mut callback);
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: no_graph,
            outcome: action_outcomes::Outcome::Rejected(PlaybackActionRejection::NoActivePlayback)
        })
    );
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: metronome,
            outcome: action_outcomes::Outcome::Rejected(
                PlaybackActionRejection::MetronomeUnavailable
            )
        })
    );
    assert_eq!(reader.try_take(), None);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn resident_outcomes_cross_concurrent_admission_and_callback_handoff() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
    std::thread::scope(|scope| {
        let audio = scope.spawn(|| callback.begin_block());
        barrier.wait();
        let session = control
            .session_command_with_ticket(hex_engine::SessionPlaybackCommand::Play)
            .unwrap();
        let metronome = control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        barrier.wait();
        audio.join().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: session,
                outcome: action_outcomes::Outcome::Rejected(
                    PlaybackActionRejection::SessionNotPrepared
                ),
            })
        );
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: metronome,
                outcome: action_outcomes::Outcome::Rejected(
                    PlaybackActionRejection::MetronomeUnavailable
                ),
            })
        );
    });
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn action_receipt_waits_for_transport_and_consumed_status() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    callback.test_before_action_outcome_status = Some(Arc::clone(&barrier));
    std::thread::scope(|scope| {
        let audio = scope.spawn(|| callback.begin_block());
        barrier.wait();
        let early = reader.try_take();
        let playing = control.status.playing.load(Acquire);
        let consumed = control.status.consumed_command.load(Acquire);
        barrier.wait();
        audio.join().unwrap();
        assert_eq!(early, None);
        assert!(!playing);
        assert!(consumed < ticket);
    });
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket,
            outcome: action_outcomes::Outcome::Applied,
        })
    );
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn resident_outcome_backpressure_preserves_priority_stop_and_retries() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let mut tickets = Vec::new();
    for _ in 0..COMMANDS {
        tickets.push(control.command_with_ticket(PlaybackCommand::Play).unwrap());
    }
    assert!(control.command_with_ticket(PlaybackCommand::Play).is_err());
    control.stop();
    begin_outcome_test_block(&mut callback);
    assert!(!control.status.playing.load(Acquire));
    assert!(control.command_with_ticket(PlaybackCommand::Play).is_err());
    for ticket in tickets {
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket,
                outcome: action_outcomes::Outcome::Superseded
            })
        );
    }
    let retried = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    begin_outcome_test_block(&mut callback);
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: retried,
            outcome: action_outcomes::Outcome::Applied
        })
    );
    assert!(control.status.playing.load(Acquire));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn resident_outcomes_report_each_pre_fault_action_and_stop() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let pause = control.command_with_ticket(PlaybackCommand::Pause).unwrap();
    let stop = control.command_with_ticket(PlaybackCommand::Stop).unwrap();
    control.status.device_fault.store(true, Release);
    begin_outcome_test_block(&mut callback);
    for ticket in [play, pause] {
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket,
                outcome: action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
            })
        );
    }
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: stop,
            outcome: action_outcomes::Outcome::Applied,
        })
    );
    assert!(!control.status.playing.load(Acquire));
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn action_outcomes_survive_graph_and_output_replacement_without_ticket_aliasing() {
    let (mut old, mut old_callback) = channel(48_000).unwrap();
    let mut old_reader = wire_action_outcomes(&mut old, &mut old_callback);
    old.publish(graph().0).unwrap();
    old_callback.begin_block();
    let first = old.command_with_ticket(PlaybackCommand::Play).unwrap();
    old.publish(graph().0).unwrap();
    old_callback.begin_block();
    let pending = old.command_with_ticket(PlaybackCommand::Pause).unwrap();
    assert!(!old_reader.is_retired());
    old.request_shutdown();
    old.retire_action_outcomes_on_control(&mut old_callback)
        .unwrap();
    assert!(old_reader.is_retired());

    let (mut replacement, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut replacement, &mut callback);
    assert_ne!(old_reader.output_identity(), reader.output_identity());
    replacement.publish(graph().0).unwrap();
    callback.begin_block();
    let replacement_ticket = replacement
        .command_with_ticket(PlaybackCommand::Play)
        .unwrap();
    callback.begin_block();
    assert_eq!(replacement_ticket, first);
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: replacement_ticket,
            outcome: action_outcomes::Outcome::Applied,
        })
    );
    assert_eq!(
        old_reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: first,
            outcome: action_outcomes::Outcome::Applied,
        })
    );
    assert_eq!(
        old_reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: pending,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    old_callback.begin_block();
    old.reclaim();
    replacement.request_shutdown();
    replacement
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    replacement.reclaim();
}

#[test]
#[cfg(feature = "allocation-probe")]
fn split_client_admission_reaches_the_actual_callback_writer() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};

    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let session = client
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let metronome = client
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    let ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    {
        let measured = allocation_counter::measure(|| {
            data.owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block());
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: session,
            outcome: action_outcomes::Outcome::Rejected(
                PlaybackActionRejection::SessionNotPrepared
            ),
        })
    );
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: metronome,
            outcome: action_outcomes::Outcome::Rejected(
                PlaybackActionRejection::MetronomeUnavailable
            ),
        })
    );
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket,
            outcome: action_outcomes::Outcome::Applied
        })
    );
    let pending = client.command_with_ticket(PlaybackCommand::Pause).unwrap();
    client.close();
    publication.close();
    assert!(!retirement.advance().unwrap());
    assert_eq!(reader.try_take(), None);
    drop(data);
    assert!(!retirement.advance().unwrap());
    assert_eq!(reader.try_take(), None);
    drop(errors);
    // Both guard carriers are back, but the stream handle is not yet disposed.
    assert!(!retirement.advance().unwrap());
    assert_eq!(reader.try_take(), None);
    assert!(
        crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    assert!(reader.is_retired());
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: pending,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    assert_eq!(reader.try_take(), None);
    assert_eq!(reader.failure(), None);
    super::candidate_tests::finish(&mut retirement);
}

#[test]
#[cfg(feature = "allocation-probe")]
fn action_outcomes_wait_for_client_after_stream_and_callback_retirement() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    publication.close();
    drop(errors);
    drop(data);
    assert!(
        !crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    assert_eq!(reader.try_take(), None);
    assert!(
        !reader.is_retired(),
        "callback return alone is not quiescence"
    );
    client.close();
    assert!(retirement.advance().unwrap());
    assert!(
        reader.is_retired(),
        "retirement frontier must follow exact endpoint return"
    );
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    super::candidate_tests::finish(&mut retirement);
}

#[test]
#[cfg(feature = "allocation-probe")]
fn retired_client_reader_stays_bound_to_its_output_after_replacement() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut old_reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let old_ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    client.close();
    publication.close();
    drop(data);
    drop(errors);
    assert!(
        crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    super::candidate_tests::finish(&mut retirement);

    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut new_reader = wire_action_outcomes(&mut control, &mut callback);
    assert_ne!(old_reader.output_identity(), new_reader.output_identity());
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let new_ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    assert_eq!(new_ticket, old_ticket);
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(
        new_reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: new_ticket,
            outcome: action_outcomes::Outcome::Applied,
        })
    );
    assert_eq!(
        old_reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: old_ticket,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    client.close();
    publication.close();
    drop(data);
    drop(errors);
    assert!(
        crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    super::candidate_tests::finish(&mut retirement);
}

#[test]
#[cfg(feature = "allocation-probe")]
fn action_outcomes_report_retirement_failure_without_inventing_a_terminal() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    retirement.fail_retained_receipts();
    assert!(!reader.is_retired());
    assert_eq!(
        reader.failure(),
        Some(ActionOutcomeFailure::RetirementUnavailable)
    );
    assert_eq!(reader.try_take(), None);
    assert!(client.command_with_ticket(PlaybackCommand::Pause).is_err());
    client.close();
    publication.close();
    drop(data);
    drop(errors);
    assert!(
        crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    assert!(reader.is_retired());
    assert_eq!(
        reader.failure(),
        Some(ActionOutcomeFailure::RetirementUnavailable)
    );
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    super::candidate_tests::finish(&mut retirement);
}

#[test]
#[cfg(feature = "allocation-probe")]
fn abandoned_action_reader_does_not_destroy_ledger_on_callback_guard_drop() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    client.command_with_ticket(PlaybackCommand::Play).unwrap();
    drop(reader);
    client.close();
    publication.close();
    let measured = allocation_counter::measure(|| drop(data));
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    drop(errors);
    assert!(
        crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    super::candidate_tests::finish(&mut retirement);
}
