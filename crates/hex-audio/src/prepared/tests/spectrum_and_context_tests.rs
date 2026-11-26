use super::*;

#[test]
fn playback_audition_routes_to_arrangement_session_runtime_and_return_owner() {
    let (mut playback, _) = spectrum_eq8_graph();
    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    let return_identity = PostNodeIdentity::new(
        hex_engine::ProcessorChainOwner::Return(3),
        12,
        hex_engine::DeviceTypeId::from_static("hex.eq8"),
    );
    assert!(playback.select_processor_audition(return_identity, Some(selection)));
    assert!(playback.processor_audition_is_active(return_identity, selection));
    let wrong_owner = PostNodeIdentity::new(
        hex_engine::ProcessorChainOwner::Track(3),
        12,
        hex_engine::DeviceTypeId::from_static("hex.eq8"),
    );
    assert!(!playback.select_processor_audition(wrong_owner, Some(selection)));
    playback.cancel_all_processor_auditions();
    assert!(!playback.processor_audition_is_active(return_identity, selection));
}

#[test]
fn spectrum_stage_requires_independent_exact_phase_coverage() {
    use hex_engine::NodeSignalPhase::{Input, Output};
    let target = spectrum_test_target(hex_engine::ProcessorChainOwner::Track(3), 0, 1, 48_000);
    let wrong_owner =
        spectrum_test_target(hex_engine::ProcessorChainOwner::Return(3), 0, 1, 48_000);
    let input = [[0.1, -0.2], [0.3, -0.4], [0.5, -0.6], [0.7, -0.8]];
    let output = [[0.2, -0.4], [0.6, -0.8], [1.0, -1.2], [1.4, -1.6]];
    let mut stage = SpectrumStage::default();
    stage.begin(target, 1, 4);
    stage.observe_signal(target.identity, Input, 0, &input);
    assert!(!stage.complete(), "input alone cannot publish");
    stage.observe_signal(target.identity, Output, 2, &output[2..]);
    stage.observe_signal(wrong_owner.identity, Output, 0, &output[..2]);
    assert!(
        !stage.complete(),
        "the other owner's samples cannot complete a pair"
    );
    stage.observe_signal(target.identity, Output, 0, &output[..2]);
    assert!(stage.complete());
    assert_eq!(stage.input_frames[..4], input);
    assert_eq!(stage.frames[..4], output);
    stage.clear();
    assert!(!stage.complete());

    for phase in [Input, Output] {
        for (offset, count) in [(1, 1), (4, 1), (usize::MAX, 1)] {
            stage.begin(target, 2, 4);
            stage.observe_signal(target.identity, Input, 0, &input);
            stage.observe_signal(target.identity, Output, 0, &output);
            assert!(stage.complete());
            stage.observe_signal(target.identity, phase, offset, &input[..count]);
            assert!(
                !stage.complete(),
                "overlap or out-of-bounds in either phase invalidates the pair"
            );
        }
    }
    stage.begin(target, 3, 257);
    assert!(!stage.complete());
}

#[test]
fn prepared_eq_captures_actual_input_and_processed_output_without_changing_pcm() {
    let (mut control, mut callback, mut reader, reaper) = spectrum_test_channel();
    let (playback, identity) = spectrum_eq8_graph_with_params(true, 0, 12.0, 1.0);
    let (mut oracle, _) = spectrum_eq8_graph_with_params(true, 0, 12.0, 1.0);
    let revision = control.publish(playback).unwrap();
    control
        .spectrum_target
        .select(identity, 0, revision, 48_000)
        .unwrap();
    let mut actual = [[0.0; 2]; 256];
    let mut expected = actual;
    oracle.render(&mut expected).unwrap();
    let mut render = || {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(&mut render);
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
    #[cfg(not(feature = "allocation-probe"))]
    render();
    assert_eq!(actual, expected);
    assert!(callback.spectrum_stages.get(0).unwrap().complete());
    assert_eq!(
        callback.spectrum_stages.get(0).unwrap().input_frames,
        [[0.25, -0.4]; 256]
    );
    assert_eq!(callback.spectrum_stages.get(0).unwrap().frames, actual);
    assert!(actual.iter().any(|frame| (frame[0] - 0.25).abs() > 0.001));
    // Even a fully covered pair must be discarded if final caller validation fails.
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), false),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
    );
    assert!(!callback.spectrum_stages.get(0).unwrap().complete());
    assert!(reader.try_latest_pair().is_none());
    shutdown_spectrum_test_channel(control, callback, reaper);
}

#[test]
fn existing_publication_methods_use_control_context_generation_zero() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(0)
    );
    control
        .publish_with_policy(graph().0, PublicationPolicy::InstallFreshRuntime)
        .unwrap();
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(0)
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn analyzer_waits_for_exact_publication_generation_and_activation_floor() {
    let (mut control, mut callback, _reader, reaper) = spectrum_test_channel();
    let generation_a = 41;
    let generation_b = 42;
    let (playback_a, identity) = spectrum_eq8_graph();
    let revision_a = control
        .publish_with_policy_and_context_generation(
            playback_a,
            PublicationPolicy::InstallFreshRuntime,
            generation_a,
        )
        .unwrap();
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(generation_a)
    );

    let target_a = spectrum_test_target(identity.owner, generation_a, revision_a, 48_000);
    control
        .spectrum_target
        .select(identity, generation_a, revision_a, 48_000)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_a));

    let mut output = [[0.0; 2]; 16];
    callback.render(&mut output).unwrap();
    assert!(callback.spectrum_stages.get(0).unwrap().complete());

    let (playback_b, identity_b) = spectrum_eq8_graph();
    assert_eq!(identity_b, identity);
    let revision_b = control
        .publish_with_policy_and_context_generation(
            playback_b,
            PublicationPolicy::InstallFreshRuntime,
            generation_b,
        )
        .unwrap();
    assert!(revision_b > target_a.activation_floor);
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(generation_b)
    );

    // A project can switch after render but before the adapter submits its
    // block. The final admission check must reject the old project's tap.
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
    );

    let (target_b, _) = control
        .spectrum_target
        .select(identity, generation_b, revision_b + 1, 48_000)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_b));
    callback.render(&mut output).unwrap();
    assert!(
        !callback.spectrum_stages.get(0).unwrap().valid,
        "activation floor is not reached"
    );
    assert_eq!(
        callback.spectrum_stages.get(0).unwrap().target,
        Some(target_b)
    );
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 1 })
    );

    let (playback_c, identity_c) = spectrum_eq8_graph();
    assert_eq!(identity_c, identity);
    let revision_c = control
        .publish_with_policy_and_context_generation(
            playback_c,
            PublicationPolicy::InstallFreshRuntime,
            generation_b,
        )
        .unwrap();
    assert_eq!(revision_c, revision_b + 1);
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert!(callback.spectrum_stages.get(0).unwrap().complete());
    assert!(matches!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 2 })
            | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 2 })
    ));

    // A later ordinary publication in the same project must keep the
    // selected token and its original lower-bound activation revision.
    let revision_d = control
        .publish_with_policy_and_context_generation(
            spectrum_eq8_graph().0,
            PublicationPolicy::InheritRuntime,
            generation_b,
        )
        .unwrap();
    assert!(revision_d > revision_c);
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_b));
    callback.render(&mut output).unwrap();
    assert!(matches!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 3 })
            | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 3 })
    ));

    let (target_wrong_rate, _) = control
        .spectrum_target
        .select(identity, generation_b, revision_d, 44_100)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_wrong_rate));
    callback.render(&mut output).unwrap();
    assert!(!callback.spectrum_stages.get(0).unwrap().valid);
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 4 })
    );
    shutdown_spectrum_test_channel(control, callback, reaper);
}

#[test]
fn generation_context_survives_full_queue_retry_and_deferred_handoff() {
    let (mut control, mut callback, _reader, reaper) = spectrum_test_channel();
    let generation_a = 71;
    let generation_b = 72;
    let (playback_a, identity) = spectrum_eq8_graph();
    let revision_a = control
        .publish_with_policy_and_context_generation(
            playback_a,
            PublicationPolicy::InstallFreshRuntime,
            generation_a,
        )
        .unwrap();
    let rejected = control
        .publish_with_policy_and_context_generation(
            spectrum_eq8_graph().0,
            PublicationPolicy::InstallFreshRuntime,
            generation_b,
        )
        .unwrap_err();
    assert_eq!(rejected.reason, "playback publication already pending");
    assert_eq!(control.revision, revision_a);

    let (target_b, _) = control
        .spectrum_target
        .select(identity, generation_b, revision_a + 1, 48_000)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_b));
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(generation_a)
    );
    let mut output = [[0.0; 2]; 16];
    callback.render(&mut output).unwrap();
    assert!(!callback.spectrum_stages.get(0).unwrap().valid);
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
    );

    control
        .publish_with_policy_and_context_generation(
            rejected.playback,
            PublicationPolicy::InstallFreshRuntime,
            generation_b,
        )
        .unwrap();
    callback.test_transient_handoff_refusals = 1;
    callback.begin_block();
    assert_eq!(
        callback
            .deferred
            .as_ref()
            .map(|publication| publication.control_context_generation),
        Some(generation_b)
    );
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(generation_a)
    );
    callback.render(&mut output).unwrap();
    assert!(!callback.spectrum_stages.get(0).unwrap().valid);
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 1 })
    );

    callback.begin_block();
    assert!(callback.deferred.is_none());
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(generation_b)
    );
    callback.render(&mut output).unwrap();
    assert!(callback.spectrum_stages.get(0).unwrap().complete());
    assert_eq!(
        callback.spectrum_stages.get(0).unwrap().target,
        Some(target_b)
    );
    assert!(matches!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 2 })
            | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 2 })
    ));
    shutdown_spectrum_test_channel(control, callback, reaper);
}

#[test]
fn selected_target_is_rejected_when_project_switches_at_block_boundary() {
    let (mut control, mut callback, _reader, reaper) = spectrum_test_channel();
    let (playback_a, identity) = spectrum_eq8_graph();
    control
        .publish_with_policy_and_context_generation(
            playback_a,
            PublicationPolicy::InstallFreshRuntime,
            0,
        )
        .unwrap();
    callback.begin_block();
    let (target_a, _) = control
        .spectrum_target
        .select(identity, 0, 1, 48_000)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_a));

    let after_target_selection = Arc::new(Barrier::new(2));
    callback.test_after_spectrum_target_boundary = Some(Arc::clone(&after_target_selection));
    std::thread::scope(|scope| {
        let callback_thread = scope.spawn(|| callback.begin_block());
        after_target_selection.wait();
        let (playback_b, identity_b) = spectrum_eq8_graph();
        assert_eq!(identity_b, identity);
        let revision_b = control
            .publish_with_policy_and_context_generation(
                playback_b,
                PublicationPolicy::InstallFreshRuntime,
                1,
            )
            .unwrap();
        after_target_selection.wait();
        callback_thread.join().unwrap();
        assert_eq!(revision_b, 2);
    });
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(1)
    );
    let mut output = [[0.0; 2]; 8];
    assert_eq!(callback.spectrum_target.current(), Some(target_a));
    assert_eq!(
        callback.spectrum_target.current(),
        control.spectrum_target.desired()
    );
    callback.render(&mut output).unwrap();
    assert!(!callback.spectrum_stages.get(0).unwrap().valid);
    assert_eq!(
        callback.spectrum_stages.get(0).unwrap().target,
        Some(target_a)
    );
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
    );

    let (target_b, _) = control
        .spectrum_target
        .select(identity, 1, 2, 48_000)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_b));
    assert_eq!(target_b.project_generation, 1);
    callback.render(&mut output).unwrap();
    assert!(callback.spectrum_stages.get(0).unwrap().complete());
    assert_eq!(
        callback.spectrum_stages.get(0).unwrap().target,
        Some(target_b)
    );
    assert!(matches!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 1 })
            | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 1 })
    ));
    shutdown_spectrum_test_channel(control, callback, reaper);
}
