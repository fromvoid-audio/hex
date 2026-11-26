use super::*;

#[test]
fn running_transport_fence_rejects_stale_and_pending_command_observations() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.capture_running_transport_fence().is_none());
    control
        .publish_with_policy_and_context_generation(
            session_playback(1),
            PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    callback.begin_block();
    assert!(control.capture_running_transport_fence().is_none());
    let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    assert!(control.capture_running_transport_fence().is_none());
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 8]).unwrap();
    let (observed, fence) = control.capture_running_transport_fence().unwrap();
    assert_eq!(observed.consumed_command_ticket, play);
    assert_eq!(fence.admitted_command_ticket(), play);

    let seek = control
        .command_with_ticket(PlaybackCommand::Seek(0))
        .unwrap();
    assert!(control.capture_running_transport_fence().is_none());
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 8]).unwrap();
    let current = control.status.transport_observation().unwrap();
    assert_eq!(current.consumed_command_ticket, seek);
    control.status.transport_observation.publish(Some(observed));
    assert!(control.capture_running_transport_fence().is_none());
    control.status.transport_observation.publish(Some(current));
    assert!(control.capture_running_transport_fence().is_some());

    control
        .status
        .transport_observation
        .publish(Some(TransportObservation {
            audio_revision: current.audio_revision + 1,
            ..current
        }));
    assert!(control.capture_running_transport_fence().is_none());
    control
        .status
        .transport_observation
        .publish(Some(TransportObservation {
            control_generation: 0,
            ..current
        }));
    assert!(control.capture_running_transport_fence().is_none());
    control.status.transport_observation.publish(Some(current));
    control.status.device_fault.store(true, Release);
    assert!(control.capture_running_transport_fence().is_none());
    control.status.device_fault.store(false, Release);
    assert!(control.capture_running_transport_fence().is_some());
}

#[test]
fn fresh_runtime_fence_accepts_empty_and_exact_active_snapshots() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let empty = control.capture_fresh_runtime_fence().unwrap();
    assert_eq!(empty.expected_active_revision(), 0);
    assert_eq!(empty.route_state_generation(), 0);
    assert_eq!(empty.admitted_command_ticket(), 0);
    let first_revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(empty),
        )
        .unwrap();
    assert_eq!(control.status.admitted_ticket.load(Acquire), 0);
    callback.begin_block();
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        first_revision
    );

    let current = control.capture_fresh_runtime_fence().unwrap();
    let second_revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(current),
        )
        .unwrap();
    callback.begin_block();
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        second_revision
    );
    assert_eq!(control.status.rejected_revision(), None);
    assert!(!control.status.publication_pending.load(Acquire));
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn cancelled_fresh_publication_returns_candidate_and_allows_successor() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let active_revision = control.status.applied_revision.load(Acquire);
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let candidate_revision = control
        .publish_with_policy(
            graph_with_frame([0.91, -0.83]).0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();

    assert_eq!(
        control.cancel_fresh_runtime_publication(candidate_revision + 1),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    assert_eq!(
        control.cancel_fresh_runtime_publication(candidate_revision),
        FreshRuntimeCancellationResult::Cancelled
    );
    callback.begin_block();
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        active_revision
    );
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            candidate_revision,
            PlaybackHandoffRejection::FreshRuntimeCancelled
        ))
    );
    assert!(!control.status.publication_pending.load(Acquire));
    let mut output = [[0.0; 2]; 1];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.25; 2]], "the old runtime remains active");
    assert_eq!(control.reclaim(), 1, "the candidate retires to control");

    let successor = control.publish(graph_with_frame([0.5, -0.4]).0).unwrap();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), successor);
    assert_eq!(
        control.cancel_fresh_runtime_publication(candidate_revision),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    assert_eq!(
        control.reclaim(),
        1,
        "the replaced graph retires to control"
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn cancellation_and_final_apply_cas_have_one_winner() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let active_revision = control.status.applied_revision.load(Acquire);
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let cancelled_revision = control
        .publish_with_policy(
            graph_with_frame([0.91, -0.83]).0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let cancelled = control.fresh_runtime_receipt(cancelled_revision).unwrap();
    let before_apply = Arc::new(Barrier::new(2));
    callback.test_before_fresh_runtime_apply = Some(Arc::clone(&before_apply));
    std::thread::scope(|scope| {
        let callback_thread = scope.spawn(|| callback.begin_block());
        before_apply.wait();
        assert_eq!(
            control.cancel_fresh_runtime_publication(cancelled_revision),
            FreshRuntimeCancellationResult::Cancelled
        );
        assert_eq!(cancelled.state(), PublicationReceiptState::Cancelled);
        control.stop();
        before_apply.wait();
        callback_thread.join().unwrap();
    });
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        active_revision
    );
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            cancelled_revision,
            PlaybackHandoffRejection::FreshRuntimeCancelled
        ))
    );
    assert_eq!(control.reclaim(), 1);

    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let applying_revision = control
        .publish_with_policy(
            graph_with_frame([0.5, -0.4]).0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let installing = control.fresh_runtime_receipt(applying_revision).unwrap();
    let after_apply_decision = Arc::new(Barrier::new(2));
    callback.test_after_fresh_runtime_apply_decision = Some(Arc::clone(&after_apply_decision));
    std::thread::scope(|scope| {
        let callback_thread = scope.spawn(|| callback.begin_block());
        after_apply_decision.wait();
        assert_eq!(
            control.cancel_fresh_runtime_publication(applying_revision),
            FreshRuntimeCancellationResult::Applying
        );
        assert_eq!(installing.state(), PublicationReceiptState::Applying);
        control.stop();
        after_apply_decision.wait();
        callback_thread.join().unwrap();
    });
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        applying_revision
    );
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_revision().unwrap().0,
        cancelled_revision
    );
    assert_eq!(
        control.cancel_fresh_runtime_publication(applying_revision),
        FreshRuntimeCancellationResult::Complete
    );
    assert_eq!(installing.state(), PublicationReceiptState::Installed);
    assert_eq!(cancelled.state(), PublicationReceiptState::Cancelled);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn publication_receipts_keep_exact_terminals_after_replacement_and_shutdown() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let output = control.output_identity().unwrap();
    let context = hex_engine::AudioSourceContext {
        project_generation: 4,
        editor_revision: 6,
    };
    let submit = |control: &mut Control, fence| {
        let revision = control
            .publish_unaccounted_pcm_obligation(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                context.project_generation,
                true,
                false,
                Some(context),
            )
            .unwrap();
        let receipt = control.fresh_runtime_receipt(revision).unwrap();
        assert_eq!(receipt.output_identity(), output);
        assert_eq!(receipt.context(), Some(context));
        assert_eq!(receipt.revision(), revision);
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        receipt
    };
    let begin = |callback: &mut Callback| {
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(|| callback.begin_block());
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0),
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        callback.begin_block();
    };

    let old_fence = control.capture_fresh_runtime_fence().unwrap();
    let installed = submit(&mut control, old_fence);
    begin(&mut callback);
    control.reclaim();
    // Both later candidates have the same context. Their revisions and
    // terminals must stay distinct from the installed receipt.
    let refused = submit(&mut control, old_fence);
    begin(&mut callback);
    control.reclaim();
    assert_eq!(
        refused.state(),
        PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
    );
    let current_fence = control.capture_fresh_runtime_fence().unwrap();
    let cancelled = submit(&mut control, current_fence);
    assert_eq!(
        cancelled.cancel(),
        FreshRuntimeCancellationResult::Cancelled
    );
    begin(&mut callback);
    control.reclaim();
    assert_eq!(
        control.status.rejected_revision().unwrap().0,
        cancelled.revision()
    );
    let pending = submit(&mut control, current_fence);
    control.request_shutdown();
    begin(&mut callback);
    control.reclaim();
    assert_eq!(pending.state(), PublicationReceiptState::OutputClosed);
    assert_eq!(installed.state(), PublicationReceiptState::Installed);
    assert_eq!(
        refused.state(),
        PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
    );
    assert_eq!(cancelled.state(), PublicationReceiptState::Cancelled);
}

#[test]
fn destroying_output_without_another_callback_closes_its_pending_receipt() {
    let (mut control, callback) = channel(48_000).unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let receipt = control.fresh_runtime_receipt(revision).unwrap();
    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    drop(control);
    drop(callback);
    assert_eq!(receipt.state(), PublicationReceiptState::OutputClosed);
}

#[test]
fn failed_enqueue_does_not_leave_a_cancellable_revision_or_block_next_publish() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publications
        .push(QueuedPublication::Legacy(Publication {
            pcm_frames: 0,
            forwarded_pcm: false,
            revision: 99,
            control_context_generation: 0,
            explicit_control_context: false,
            source_context: None,
            policy: PublicationPolicy::InstallFreshRuntime,
            fresh_runtime_cancellation: None,
            scheduled_cut_frame: None,
            streams: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_references: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_continuation: None,
            playback: Box::new(graph().0),
        }))
        .unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let rejected = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap_err();
    assert_eq!(rejected.reason, "playback publication queue full");
    assert_eq!(control.revision, 0);
    assert_eq!(
        control.cancel_fresh_runtime_publication(1),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    let queued = callback.publications.pop().unwrap();
    drop(queued);
    assert_eq!(control.publish(rejected.playback).unwrap(), 1);
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn ordinary_inherited_publications_have_no_fresh_cancellation_decision() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let first = control.publish(graph().0).unwrap();
    callback.begin_block();
    let next = control.publish(graph_with_frame([0.5, -0.4]).0).unwrap();
    assert_eq!(
        control.cancel_fresh_runtime_publication(next),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), next);
    assert_eq!(
        control.cancel_fresh_runtime_publication(first),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    control.reclaim();
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fault_invalidates_prepared_playing_runtime_fence() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let (mut prepared, _) = fresh_spectrum_eq8_graph();
    prepared.apply(PlaybackCommand::Play);
    let revision = control
        .publish_with_policy(
            prepared,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    callback.begin_block();
    assert_eq!(
        control.status.rejected_revision(),
        Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged))
    );
    assert!(!control.status.playing.load(Acquire));
}

#[test]
fn latched_fault_refuses_prepared_runtime_even_after_epoch_exhaustion() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    control.status.transport_epoch.store(u64::MAX, Release);
    control
        .status
        .last_output_fault_epoch
        .store(u64::MAX, Release);
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let (mut prepared, _) = fresh_spectrum_eq8_graph();
    prepared.apply(PlaybackCommand::Play);
    let revision = control
        .publish_with_policy(
            prepared,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    callback.begin_block();
    assert_eq!(
        control.status.rejected_revision(),
        Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged))
    );
    assert!(!control.status.playing.load(Acquire));
}

#[test]
fn fresh_runtime_fence_rejects_stale_active_revision() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish(graph().0)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    let stale = control.capture_fresh_runtime_fence().unwrap();
    let intervening = control
        .publish_with_policy(graph().0, PublicationPolicy::InstallFreshRuntime)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), intervening);

    let rejected = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), intervening);
    assert_fresh_fence_rejection(&mut control, rejected);
    assert_eq!(control.reclaim(), 2);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_runtime_fence_rejects_stale_route_generation_independently() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (old, _, clip, _) = session_audio_graph();
    let (mut oracle, _, oracle_clip, _) = session_audio_graph();
    assert_eq!(clip, oracle_clip);
    control
        .publish(old)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    control
        .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    oracle
        .try_apply_action(hex_engine::PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    oracle.apply(PlaybackCommand::Play);

    let stale = control.capture_fresh_runtime_fence().unwrap();
    let ticket_before = control.status.admitted_ticket.load(Acquire);
    callback
        .active
        .as_mut()
        .and_then(ActivePublication::legacy_mut)
        .unwrap()
        .playback
        .try_apply_action(hex_engine::PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::Seek(1),
        ))
        .unwrap();
    oracle
        .try_apply_action(hex_engine::PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::Seek(1),
        ))
        .unwrap();
    callback.publish_active_routes(true);
    assert_eq!(control.status.admitted_ticket.load(Acquire), ticket_before);
    assert_ne!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .playback
            .session_route_state_generation(),
        stale.route_state_generation()
    );

    let (candidate, _) = graph_with_frame([0.91, -0.83]);
    let rejected = control
        .publish_with_policy(
            candidate,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert_fresh_fence_rejection(&mut control, rejected);
    let mut actual = [[0.0; 2]; 2];
    let mut expected = [[0.0; 2]; 2];
    callback.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "stale fresh candidate replaced active PCM"
    );
    assert_ne!(actual[0], [0.91, -0.83]);
    let mut routes = [SessionRouteEntry::default(); 1];
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(
        routes[0].owner,
        SessionRouteOwner::Session {
            clip_id: clip.value()
        },
        "stale rejection must preserve the active Session route owner"
    );
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_runtime_fence_rejects_stale_command_ticket_and_applies_prefix_to_old_graph() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish(graph().0)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    let stale = control.capture_fresh_runtime_fence().unwrap();
    let ticket = control
        .command_with_ticket(PlaybackCommand::Seek(19))
        .unwrap();
    let rejected = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert_eq!(control.status.position.load(Acquire), 19);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    assert_fresh_fence_rejection(&mut control, rejected);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_runtime_fence_rejects_pending_stop_and_stops_the_old_graph() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish(graph().0)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    let stale = control.capture_fresh_runtime_fence().unwrap();
    control.stop();
    let rejected = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
    assert_fresh_fence_rejection(&mut control, rejected);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_runtime_fence_rejects_stop_consumed_before_publication() {
    assert_consumed_stop_invalidates_fresh_fence(false);
}

#[test]
fn resident_fresh_runtime_fence_rejects_stop_consumed_before_publication() {
    assert_consumed_stop_invalidates_fresh_fence(true);
}

#[test]
fn fresh_fence_refuses_stop_between_marker_clear_and_transport_ack() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(session_audio_graph().0).unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    control.stop();
    let barrier = Arc::new(Barrier::new(2));
    callback.test_before_action_outcome_status = Some(Arc::clone(&barrier));
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| callback.begin_block());
        barrier.wait();
        assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
        assert!(control.capture_fresh_runtime_fence().is_none());
        barrier.wait();
        worker.join().unwrap();
    });
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control
            .capture_fresh_runtime_fence()
            .unwrap()
            .consumed_stop_generation(),
        2
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn exhausted_stop_generation_refuses_new_fences_without_blocking_stop() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    control
        .status
        .consumed_stop_generation
        .store(u64::MAX - 1, Release);
    control.stop();
    callback.begin_block();
    assert_eq!(
        control.status.consumed_stop_generation.load(Acquire),
        u64::MAX
    );
    assert!(control.capture_fresh_runtime_fence().is_none());
    control.stop();
    callback.begin_block();
    assert_eq!(
        control.status.consumed_stop_generation.load(Acquire),
        u64::MAX
    );
    assert!(!control.status.playing.load(Acquire));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn command_admitted_after_fresh_fence_validation_applies_to_new_runtime() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish(graph().0)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let barrier = std::sync::Arc::new(Barrier::new(2));
    callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
    std::thread::scope(|scope| {
        let callback_thread = scope.spawn(|| callback.begin_block());
        barrier.wait();
        let ticket = control
            .command_with_ticket(PlaybackCommand::Seek(47))
            .unwrap();
        barrier.wait();
        callback_thread.join().unwrap();
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    });
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert_eq!(control.status.position.load(Acquire), 47);
    assert_eq!(control.status.rejected_revision(), None);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
