use super::*;

#[test]
fn stop_admitted_after_fresh_fence_validation_stops_new_runtime() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    let barrier = std::sync::Arc::new(Barrier::new(2));
    callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
    std::thread::scope(|scope| {
        let callback_thread = scope.spawn(|| callback.begin_block());
        barrier.wait();
        control.stop();
        barrier.wait();
        callback_thread.join().unwrap();
    });
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
    assert_eq!(control.status.rejected_revision(), None);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn stale_fresh_runtime_fence_is_rechecked_after_retirement_backpressure() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    for _ in 0..RETIRED {
        control
            .publish_with_policy(graph().0, PublicationPolicy::InstallFreshRuntime)
            .unwrap();
        callback.begin_block();
    }
    assert!(callback.retired.is_full());
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_policy(
            graph_with_frame([0.91, -0.83]).0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let ticket = control
        .command_with_ticket(PlaybackCommand::Seek(13))
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), revision - 1);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    // A Seek while playing fades out, jumps and ramps in (#4140).
    assert_eq!(control.status.position.load(Acquire), 0);
    assert!(control.status.publication_pending.load(Acquire));
    let mut fade_out = [[0.0; 2]; 128];
    callback.render(&mut fade_out).unwrap();
    assert_eq!(control.status.position.load(Acquire), 13);
    let ramp = |frame: usize| [0.25 * (frame + 1) as f32 / 128.0; 2];
    let mut before_retry = [[0.0; 2]; 8];
    callback.render(&mut before_retry).unwrap();
    assert_eq!(before_retry, std::array::from_fn(ramp));
    assert_eq!(control.status.position.load(Acquire), 21);
    assert_eq!(control.reclaim(), RETIRED);
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), revision - 1);
    assert_fresh_fence_rejection(&mut control, revision);
    let mut after_retry = [[0.0; 2]; 8];
    callback.render(&mut after_retry).unwrap();
    assert_eq!(after_retry, std::array::from_fn(|frame| ramp(frame + 8)));
    assert_eq!(control.status.position.load(Acquire), 29);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn rejected_candidate_is_held_until_control_reclaims_retirement_space() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    for revision in 10..10 + RETIRED as u64 {
        callback
            .retired
            .push(ReturnedPublication::Legacy(Publication {
                pcm_frames: 0,
                forwarded_pcm: false,
                revision,
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
    }
    assert!(callback.retired.is_full());

    let fence = control.capture_fresh_runtime_fence().unwrap();
    let (candidate, weak_candidate, _, _) = session_audio_graph();
    let rejected = control
        .publish_with_policy(
            candidate,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    control.stop();
    callback.begin_block();

    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_fresh_fence_rejection(&mut control, rejected);
    assert!(callback.held_retirement.is_some());
    assert!(weak_candidate.upgrade().is_some());

    assert_eq!(control.reclaim(), RETIRED);
    callback.begin_block();
    assert!(callback.held_retirement.is_none());
    assert!(weak_candidate.upgrade().is_some());
    assert_eq!(control.reclaim(), 1);
    assert!(weak_candidate.upgrade().is_none());

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn command_cut_distinguishes_captured_prefix_and_stop_boundary() {
    assert_eq!(
        command_cut(7, 4),
        CommandCut {
            stop_through: Some(3),
            process_through: 7,
        }
    );
    assert_eq!(
        command_cut(2, 8),
        CommandCut {
            stop_through: Some(7),
            process_through: 7,
        }
    );
    assert_eq!(
        command_cut(7, 0),
        CommandCut {
            stop_through: None,
            process_through: 7,
        }
    );
}

#[test]
fn stop_cut_suppresses_tickets_at_or_before_boundary_only() {
    for (stop_through, process_through, expected_position) in [
        (Some(2), 1, 0),
        (Some(2), 2, 0),
        (Some(2), 3, 33),
        (None, 3, 33),
    ] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        assert!(control.publish(graph().0).is_ok());
        callback.begin_block();
        for frame in [11, 22, 33] {
            control.command(PlaybackCommand::Seek(frame)).unwrap();
        }
        callback.consume_commands_through(CommandCut {
            stop_through,
            process_through,
        });
        assert_eq!(control.status.position.load(Acquire), expected_position);
        assert_eq!(
            control.status.consumed_command.load(Acquire),
            process_through
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn full_queue_does_not_advance_admitted_ticket_and_stop_never_wraps() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(graph().0).is_ok());
    for expected in 1..=COMMANDS as u64 {
        assert_eq!(
            control
                .command_with_ticket(PlaybackCommand::Seek(expected))
                .unwrap(),
            expected
        );
    }
    assert!(
        control
            .command_with_ticket(PlaybackCommand::Seek(99))
            .is_err()
    );
    assert_eq!(
        control.status.admitted_ticket.load(Acquire),
        COMMANDS as u64
    );
    callback.begin_block();
    assert_eq!(
        control.status.consumed_command.load(Acquire),
        COMMANDS as u64
    );

    control.command_ticket = u64::MAX - 1;
    control.status.admitted_ticket.store(u64::MAX - 1, Release);
    assert!(control.command_with_ticket(PlaybackCommand::Play).is_err());
    control.stop();
    assert_eq!(control.status.pending_stop_marker.load(Acquire), u64::MAX);
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn play_from_uses_one_ticket_and_priority_stop_cuts_it_off_under_full_queue() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(graph().0).is_ok());
    callback.begin_block();

    let ticket = control
        .command_with_ticket(PlaybackCommand::PlayFrom(37))
        .unwrap();
    assert_eq!(ticket, 1, "seek and start are admitted as one command");
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.position.load(Acquire), 37);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);

    control.command(PlaybackCommand::Pause).unwrap();
    callback.begin_block();
    for frame in 1..COMMANDS as u64 {
        control.command(PlaybackCommand::Seek(frame)).unwrap();
    }
    let play_from_ticket = control
        .command_with_ticket(PlaybackCommand::PlayFrom(91))
        .unwrap();
    assert_eq!(play_from_ticket, COMMANDS as u64 + 2);
    assert!(control.command(PlaybackCommand::Play).is_err());
    control.stop();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.position.load(Acquire), 0);
    assert_eq!(
        control.status.consumed_command.load(Acquire),
        play_from_ticket,
        "priority Stop consumes the queued prefix while suppressing playback"
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn post_cut_admissions_remain_queued_until_the_next_actual_block() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(graph().0).is_ok());
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));

    // Capture the actual callback cut and marker, then admit Stop twice
    // and Play after that cut. Both Stops coalesce to the same cutoff.
    let entry_ticket = control.status.admitted_ticket.load(Acquire);
    let marker = control.status.pending_stop_marker.swap(0, AcqRel);
    let cut = command_cut(entry_ticket, marker);
    assert_eq!(cut.stop_through, None);
    control.stop();
    control.stop();
    let post_cut_play = control.command_with_ticket(PlaybackCommand::Play).unwrap();

    // Run the same captured cut production uses after the swap. The later
    // Play remains queued and the post-entry Stop marker is still pending.
    callback.consume_commands_through(cut);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.consumed_command.load(Acquire), 0);
    assert_eq!(
        control.status.pending_stop_marker.load(Acquire),
        post_cut_play
    );
    assert_eq!(callback.commands.peek().unwrap().ticket, post_cut_play);

    // The next real block captures Stop + Play and applies them in order.
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.consumed_command.load(Acquire), post_cut_play);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn derived_pcm_render_replace_and_retire_never_allocate_or_drop_on_callback() {
    fn transformed(cents: i16) -> (Playback, Weak<hex_engine::PreparedRevision>) {
        use hex_project::{Command, Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, asset, track } = editor
            .execute(Command::ImportAudioTrack {
                name: "audio".into(),
                path: "derived.wav".into(),
                sample_rate: 48_000,
                frames: 2_048,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor
            .execute(Command::SetAudioClipPitch { clip, cents })
            .unwrap();
        let Created::Track(target) = editor
            .execute(Command::AddTrack {
                name: "routed".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(Command::SetAudioFeed {
                source: hex_project::RoutingOwner::Track(track),
                destination: target,
                gain: 0.5,
            })
            .unwrap();
        editor
            .execute(Command::ReplaceTrackGainEnvelope {
                track,
                points: vec![hex_project::GainPoint {
                    incoming_gain: None,
                    beat: hex_project::BeatTime::ZERO,
                    gain: 0.5,
                    outgoing: hex_project::GainInterpolation::Step,
                }],
            })
            .unwrap();
        let audio = hex_engine::Audio::new(
            48_000,
            (0..2_048)
                .map(|i| {
                    let x = (i as f32 * 0.173).sin() * 0.2;
                    [x, -x]
                })
                .collect(),
        )
        .unwrap();
        // The temporary default preparation cache is destroyed here. Only the
        // returned graph owns derived Audio; no UI/source/cache masks retirement.
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::from([(asset, audio)]),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let weak = Arc::downgrade(&revision);
        (
            Playback::try_from_prepared_with_session(revision).unwrap(),
            weak,
        )
    }
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut roots = Vec::new();
    let mut output = [[0.0; 2]; 31];
    for cents in [137, 211, 337, 411] {
        let (graph, weak) = transformed(cents);
        roots.push(weak);
        control.publish(graph).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let allocation = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                allocation.count_total,
                allocation.bytes_total,
                allocation.count_current,
                allocation.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(
            roots.iter().all(|root| root.strong_count() > 0),
            "retired derived roots remain owned until control reclamation"
        );
    }
    control.request_shutdown();
    for _ in 0..3 {
        let allocation = allocation_counter::measure(|| {
            callback.begin_block();
        });
        assert_eq!(
            (
                allocation.count_total,
                allocation.bytes_total,
                allocation.count_current,
                allocation.bytes_current
            ),
            (0, 0, 0, 0)
        );
        control.reclaim();
    }
    assert!(roots.iter().all(|root| root.strong_count() == 0));
}

#[test]
fn split_gain_and_publication_each_reserve_before_the_other_actor_mutates() {
    for gain_first in [false, true] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.status.split_endpoints.store(true, Release);
        if gain_first {
            control
                .gain_recording
                .begin(admission_gain_target(), 0.5)
                .unwrap();
            let (playback, weak) = graph();
            let refused = control.publish(playback).unwrap_err();
            assert_eq!(control.revision, 0);
            assert_eq!(
                control.publications.slots(),
                control.publications.buffer().capacity()
            );
            assert!(weak.upgrade().is_some());
            drop(refused);
            assert!(weak.upgrade().is_none());
        } else {
            control.publish(graph().0).unwrap();
            assert!(
                control
                    .gain_recording
                    .begin(admission_gain_target(), 0.5)
                    .is_err()
            );
            assert!(control.gain_recording.target().is_none());
            // Callback completion clears only publication; the next Begin
            // succeeds without any UI publication receipt collection.
            callback.begin_block();
            control
                .gain_recording
                .begin(admission_gain_target(), 0.5)
                .unwrap();
        }
    }
}

#[test]
fn unsplit_legacy_control_keeps_publish_then_begin_ordering() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    control
        .gain_recording
        .begin(admission_gain_target(), 0.5)
        .unwrap();
    callback.begin_block();
    assert!(!control.status.publication_pending.load(Acquire));
    assert!(!control.status.publication_pending.reserve_publication());
    // Publication completion did not clear gain's independent bit.
    assert_eq!(
        control.gain_recording.target(),
        Some(admission_gain_target())
    );
}

#[test]
fn arrangement_stopped_gate_holds_play_through_install_and_stop_keeps_priority() {
    for stop_stage in 0..3 {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut old = graph().0;
        old.apply(PlaybackCommand::Seek(8));
        let revision = control
            .publish_with_policy_and_context_generation(
                old,
                PublicationPolicy::InstallFreshRuntime,
                7,
            )
            .unwrap();
        callback.begin_block();
        let gate = control.request_arrangement_edit_fence(7, revision).unwrap();
        callback.begin_block();
        assert_eq!(control.arrangement_edit_position(gate), Some(8));
        assert!(
            control.publish(graph().0).is_err(),
            "unrelated publication cannot cross gate"
        );
        let _ = control.command(PlaybackCommand::Play);
        callback.begin_block();
        assert!(!control.status.playing.load(Acquire));
        if stop_stage == 1 {
            control.stop();
            callback.begin_block();
        }
        let mut next = graph().0;
        next.apply(PlaybackCommand::Seek(16));
        let installed = control
            .publish_with_policy_and_context_generation(
                next,
                PublicationPolicy::InstallUnderStoppedArrangement(gate),
                7,
            )
            .unwrap();
        let _ = control.command(PlaybackCommand::Play);
        if stop_stage == 2 {
            control.stop();
        }
        #[cfg(feature = "allocation-probe")]
        let measured = allocation_counter::measure(|| callback.begin_block());
        #[cfg(not(feature = "allocation-probe"))]
        callback.begin_block();
        #[cfg(feature = "allocation-probe")]
        {
            assert_eq!(measured.count_total, 0);
            assert_eq!(measured.bytes_total, 0);
            assert_eq!(measured.count_current, 0);
            assert_eq!(measured.bytes_current, 0);
        }
        assert_eq!(control.status.applied_revision.load(Acquire), installed);
        assert!(!control.status.playing.load(Acquire));
        let position = callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .playback
            .position();
        assert_eq!(position, if stop_stage == 0 { 16 } else { 0 });
        let _ = control.command(PlaybackCommand::Play);
        callback.begin_block();
        assert!(
            !control.status.playing.load(Acquire),
            "accepted candidate retains gate until ACK owner retires it"
        );
        control.release_stopped_edit_fence(gate);
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        assert!(control.status.playing.load(Acquire));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn live_arrangement_gate_inherits_installation_cursor_and_priority_stop() {
    for stop in [false, true] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let revision = control
            .publish_with_policy_and_context_generation(
                graph().0,
                PublicationPolicy::InstallFreshRuntime,
                7,
            )
            .unwrap();
        callback.begin_block();
        control.command(PlaybackCommand::Play).unwrap();
        let gate = control
            .request_live_arrangement_edit_fence(7, revision)
            .unwrap();
        let mut pcm = [[0.0; 2]; 8];
        callback.begin_block();
        callback.render(&mut pcm).unwrap();
        assert_eq!(control.arrangement_edit_playing(gate), Some(true));
        let captured = control.arrangement_edit_position(gate).unwrap();
        callback.begin_block();
        callback.render(&mut pcm).unwrap();
        callback.begin_block();
        callback.render(&mut pcm).unwrap();
        assert!(control.publish(graph().0).is_err());
        let installed = control
            .publish_with_policy_and_context_generation(
                graph_with_frame([0.5; 2]).0,
                PublicationPolicy::InheritUnderArrangement(gate),
                7,
            )
            .unwrap();
        if stop {
            control.stop();
        }
        #[cfg(feature = "allocation-probe")]
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
        });
        #[cfg(not(feature = "allocation-probe"))]
        {
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
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
        assert_eq!(control.status.applied_revision.load(Acquire), installed);
        assert_eq!(control.status.playing.load(Acquire), !stop);
        let active = callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap();
        if stop {
            assert_eq!(active.playback.position(), 0);
            assert_eq!(pcm, [[0.0; 2]; 8]);
        } else {
            assert!(active.playback.position() > captured + 8);
            assert_eq!(pcm, [[0.5; 2]; 8]);
        }
        control.release_stopped_edit_fence(gate);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn arrangement_stopped_gate_refuses_an_earlier_play_without_changing_runtime() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let revision = control
        .publish_with_policy_and_context_generation(
            graph().0,
            PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    let gate = control.request_arrangement_edit_fence(7, revision).unwrap();
    callback.begin_block();
    assert_eq!(
        control.stopped_edit_fence_state(gate),
        StoppedEditFenceState::Refused
    );
    assert!(control.status.playing.load(Acquire));
    control.release_stopped_edit_fence(gate);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
