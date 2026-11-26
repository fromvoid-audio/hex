use super::*;

#[test]
fn seek_cancels_a_waiting_begin_without_blocking_a_later_fresh_press() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let first_revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        first_revision
    );

    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 3, 2, 48_000, selection)
        .unwrap();
    callback.begin_block();
    let mut output = [[0.0; 2]; 64];
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert!(callback.processor_audition.current().is_some());

    control.command(PlaybackCommand::Seek(0)).unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition.current(), None);

    let (replacement, replacement_identity) = fresh_spectrum_eq8_graph();
    assert_eq!(replacement_identity, identity);
    let second_revision = control
        .publish_with_policy_and_context_generation(
            replacement,
            PublicationPolicy::InheritRuntime,
            3,
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        second_revision,
        "publication rejection: {:?}",
        callback.status.rejected_revision()
    );
    assert_eq!(callback.status.rejected_revision(), None);
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition.current(), None);
    assert!(
        !callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    assert_eq!(callback.processor_audition_lease, None);
    assert!(
        !callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control
        .processor_audition
        .select(identity, 3, second_revision, 48_000, selection)
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(
        callback.processor_audition_lease,
        control.processor_audition.desired()
    );
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn audition_waits_for_actual_generation_and_revision_floor_then_activates() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let first_revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        first_revision
    );

    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 3, first_revision + 1, 48_000, selection)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert!(callback.processor_audition.current().is_some());

    let (replacement, replacement_identity) = fresh_spectrum_eq8_graph();
    assert_eq!(replacement_identity, identity);
    let second_revision = control
        .publish_with_policy_and_context_generation(
            replacement,
            PublicationPolicy::InheritRuntime,
            3,
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        second_revision
    );
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .control_context_generation,
        3
    );
    assert_eq!(
        callback.processor_audition_lease,
        control.processor_audition.desired()
    );
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );
    assert!(output.iter().flatten().any(|value| value.abs() > 1.0e-6));

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn sparse_eq_callback_publication_held_stable_audition_release_and_retirement_allocate_nothing() {
    let (mut control, mut callback) = channel(48000).unwrap();
    let (playback, identity) = spectrum_eq_graph_with_params(true, 0, 3.0, 1.0, true, false);
    let first = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    let mut prefix = [[0.0; 2]; 17];
    let admission = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut prefix).unwrap();
    });
    assert_eq!(
        (
            admission.count_total,
            admission.bytes_total,
            admission.count_current,
            admission.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let selection = hex_engine::ProcessorAuditionSelection {
        index: 1000,
        lane: 0,
    };
    control
        .processor_audition
        .select(identity, 2, first, 48000, selection)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    let held = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            held.count_total,
            held.bytes_total,
            held.count_current,
            held.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let (candidate, _) = spectrum_eq_graph_with_params(false, 0, 6.0, 2.0, true, false);
    let revision = control
        .publish_with_policy_and_context_generation(candidate, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    let publication = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            publication.count_total,
            publication.bytes_total,
            publication.count_current,
            publication.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert_eq!(
        callback.processor_audition_lease,
        control.processor_audition.desired()
    );
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );
    control.processor_audition.release().unwrap();
    let mut fade = [[0.0; 2]; 256];
    let release = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut fade).unwrap();
    });
    assert_eq!(
        (
            release.count_total,
            release.bytes_total,
            release.count_current,
            release.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(callback.processor_audition_lease.is_none());
    control
        .processor_audition
        .select(identity, 2, revision, 48000, selection)
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    control.processor_audition.cancel_immediately().unwrap();
    let cancel = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            cancel.count_total,
            cancel.bytes_total,
            cancel.count_current,
            cancel.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(callback.processor_audition_lease.is_none());
    control.request_shutdown();
    let retire = allocation_counter::measure(|| {
        callback.begin_block();
    });
    assert_eq!(
        (
            retire.count_total,
            retire.bytes_total,
            retire.count_current,
            retire.bytes_current
        ),
        (0, 0, 0, 0)
    );
    control.reclaim();
}

#[test]
fn compatible_parameter_publications_preserve_the_held_audition() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let first_revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();
    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 2, first_revision, 48_000, selection)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    for (gain, q) in [(6.0, 1.0), (6.0, 2.0)] {
        let (candidate, candidate_identity) = spectrum_eq8_graph_with_params(false, 0, gain, q);
        assert_eq!(candidate_identity, identity);
        let revision = control
            .publish_with_policy_and_context_generation(
                candidate,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .revision,
            revision
        );
        assert_eq!(
            callback.processor_audition_lease,
            control.processor_audition.desired()
        );
        assert!(
            callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );
    }

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn wrong_sample_rate_target_is_rejected_without_replaying() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();

    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 2, revision, 44_100, selection)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    callback.begin_block();
    let epoch = callback.processor_audition.current_epoch();
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition_rejected_epoch, epoch);
    assert!(
        !callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn mode_change_invalidates_the_old_audition_epoch_and_needs_a_fresh_press() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let first_revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();
    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 2, first_revision, 48_000, selection)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    callback.begin_block();
    let epoch = callback.processor_audition.current_epoch();
    callback.render(&mut output).unwrap();
    assert_eq!(
        callback.processor_audition_lease,
        control.processor_audition.desired()
    );

    // Stereo -> left/right invalidates the prior transient selection while
    // keeping the stable device ID. The old held epoch must stay rejected.
    let (replacement, replacement_identity) = spectrum_eq8_graph_with_mode(false, 1);
    assert_eq!(replacement_identity, identity);
    let second_revision = control
        .publish_with_policy_and_context_generation(
            replacement,
            PublicationPolicy::InheritRuntime,
            2,
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        second_revision
    );
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition_rejected_epoch, epoch);
    assert!(
        !callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition_rejected_epoch, epoch);

    control
        .processor_audition
        .select(identity, 2, second_revision, 48_000, selection)
        .unwrap();
    callback.begin_block();
    let fresh_epoch = callback.processor_audition.current_epoch();
    assert!(fresh_epoch > epoch);
    callback.render(&mut output).unwrap();
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn a_fresh_audition_admitted_after_priority_stop_survives_same_block_consumption() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();

    control.stop();
    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 2, revision, 48_000, selection)
        .unwrap();
    callback.begin_block();
    let mut output = [[0.0; 2]; 64];
    callback.render(&mut output).unwrap();

    assert!(
        !callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .playback
            .playing()
    );
    assert_eq!(
        callback.processor_audition_lease,
        control.processor_audition.desired()
    );
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn render_fault_refuses_new_capture_admissions() {
    let (control, mut callback) = channel(48_000).unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 0);
    assert!(control.status.reserve_midi_begin().is_err());
    assert!(control.status.reserve_gain_begin().is_err());
    assert!(control.status.reserve_native_parameter_input().is_err());
}

#[test]
fn stale_play_queued_before_fault_does_not_restart_resident_graph() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 32]).unwrap();
    let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    let mut output = [[0.0; 2]; 16];
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    #[cfg(not(feature = "allocation-probe"))]
    {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(
        !control.status.playing.load(Acquire),
        "pre-fault queued Play restarted the faulted graph"
    );
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::OutputFailed))
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn admission_delayed_across_fault_cannot_restart_playback() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 32]).unwrap();
    // Model a producer suspended after reading the actual admission fields
    // but before publishing its fixed-size command into the ring.
    let epoch = control.status.transport_epoch.load(Acquire);
    let gate = control.status.sampler_edit.gate();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    let ticket = control.command_ticket + 1;
    assert!(
        control
            .commands
            .push(QueuedCommand {
                ticket,
                action: PlaybackAction::Transport(PlaybackCommand::PlayFrom(45)),
                project_generation: None,
                expected_source: None,
                sampler_edit_gate: gate,
                transport_epoch: epoch,
            })
            .is_ok()
    );
    control.command_ticket = ticket;
    control.status.admitted_ticket.store(ticket, Release);
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.recovery_position(), 32);
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::OutputFailed))
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn stale_play_is_rejected_after_replacement_and_new_play_is_allowed() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = control
        .enable_action_outcomes_on_control(&mut callback)
        .unwrap();
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 32]).unwrap();
    let stale = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let stale_second = control
        .command_with_ticket(PlaybackCommand::PlayFrom(45))
        .unwrap();
    let (replacement, _) = fresh_spectrum_eq8_graph();
    let revision = control
        .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
        .unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(|| callback.begin_block());
    #[cfg(not(feature = "allocation-probe"))]
    callback.begin_block();
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert_eq!(
        control.status.rejected_action(),
        Some((stale_second, PlaybackActionRejection::OutputFailed))
    );
    for ticket in [stale, stale_second] {
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::OutputFailed),
            })
        );
    }
    assert!(!control.status.playing.load(Acquire));
    let fresh = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 16]).unwrap();
    assert!(control.status.playing.load(Acquire));
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: fresh,
            outcome: ActionOutcome::Applied,
        })
    );
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_play_from_after_fault_retains_explicit_cursor() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 32]).unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    let ticket = control
        .command_with_ticket(PlaybackCommand::PlayFrom(45))
        .unwrap();
    let mut output = [[1.0; 2]; 16];
    #[cfg(feature = "allocation-probe")]
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    #[cfg(not(feature = "allocation-probe"))]
    {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.recovery_position(), 45);
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::OutputFailed))
    );
    assert_eq!(output, [[0.0; 2]; 16]);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn priority_stop_preserves_zero_cursor_over_post_fault_play_from() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    control.command(PlaybackCommand::PlayFrom(45)).unwrap();
    control.stop();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.recovery_position(), 0);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn multiple_rejected_actions_are_each_observable_after_one_callback() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = control
        .enable_action_outcomes_on_control(&mut callback)
        .unwrap();
    let (playback, _, _, _) = session_audio_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    let first = control
        .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::Stop)
        .unwrap();
    let second = control
        .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::Seek(4))
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), second);
    assert_eq!(
        control.status.rejected_action(),
        Some((second, PlaybackActionRejection::InvalidSessionAction)),
    );
    for ticket in [first, second] {
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::InvalidSessionAction),
            })
        );
    }
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn first_render_cause_is_latched_before_stop_and_new_admission_clears_it() {
    let (mut control, mut callback) = channel(48_000).expect("channel");
    let (playback, _) = spectrum_eq8_graph();
    let revision = control.publish(playback).expect("publish");
    control.command(PlaybackCommand::Play).expect("play");
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 128]).expect("prefix");
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(|| {
        callback.report_render_fault(Some(&Invalid("ongoing midi query exceeds 256 edges")), 128);
    });
    #[cfg(not(feature = "allocation-probe"))]
    callback.report_render_fault(Some(&Invalid("ongoing midi query exceeds 256 edges")), 128);
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert!(!control.status.playing.load(Relaxed));
    assert_eq!(control.status.position.load(Relaxed), 0);
    assert_eq!(control.status.recovery_position(), 128);
    let fault = control.status.render_fault_snapshot().expect("fault");
    assert_eq!(fault.message, "ongoing midi query exceeds 256 edges");
    assert_eq!((fault.audio_revision, fault.frame), (revision, 128));
    callback.report_render_fault(None, 0);
    assert_eq!(control.status.render_fault_snapshot(), Some(fault));
    assert_eq!(control.status.recovery_position(), 128);
    control.command(PlaybackCommand::Seek(37)).expect("seek");
    callback.begin_block();
    assert_eq!(control.status.position.load(Relaxed), 37);
    assert_eq!(control.status.recovery_position(), 37);
    callback.report_render_fault(None, 37);
    assert_eq!(control.status.recovery_position(), 37);
    control.stop();
    assert_eq!(control.status.recovery_position(), 0);
    control
        .command(PlaybackCommand::Seek(45))
        .expect("later seek");
    callback.begin_block();
    assert_eq!(control.status.recovery_position(), 45);
    let (replacement, _) = spectrum_eq8_graph();
    let replacement_revision = control
        .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
        .expect("replacement");
    callback.begin_block();
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        replacement_revision
    );
    assert!(!control.status.render_fault.load(Acquire));
    assert!(control.status.render_fault_snapshot().is_none());
    assert_eq!(control.status.recovery_position(), 0);
    callback.report_render_fault(None, 256);
    assert_eq!(
        control
            .status
            .render_fault_snapshot()
            .expect("nonfinite")
            .message,
        "audio output contains nonfinite samples"
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn device_fault_keeps_recovery_cursor_across_graph_admission() {
    let (mut control, mut callback) = channel(48_000).expect("channel");
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).expect("publish");
    control.command(PlaybackCommand::Play).expect("play");
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 64]).expect("prefix");
    control.status.device_fault.store(true, Release);
    callback.stop_after_fault();
    assert_eq!(control.status.position.load(Relaxed), 0);
    assert_eq!(control.status.recovery_position(), 64);
    let (replacement, _) = spectrum_eq8_graph();
    control
        .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
        .expect("replacement");
    callback.begin_block();
    assert_eq!(control.status.recovery_position(), 64);
    control.stop();
    assert_eq!(control.status.recovery_position(), 0);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn render_fault_fences_a_waiting_begin_before_a_future_graph_is_published() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let first_revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();

    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 3, first_revision + 1, 48_000, selection)
        .unwrap();
    callback.begin_block();
    let mut output = [[0.0; 2]; 64];
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert!(callback.processor_audition.current().is_some());

    callback.stop_after_fault();
    assert_eq!(callback.processor_audition.current(), None);
    let (replacement, _) = spectrum_eq8_graph();
    let second_revision = control
        .publish_with_policy_and_context_generation(
            replacement,
            PublicationPolicy::InstallFreshRuntime,
            3,
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();

    assert_eq!(second_revision, first_revision + 1);
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        second_revision
    );
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition.current(), None);
    assert!(
        !callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
