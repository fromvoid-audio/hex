use super::*;

#[test]
#[cfg(feature = "allocation-probe")]
fn reliable_output_accepts_ordinary_stream_configuration() {
    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let _reader = wire_action_outcomes(&mut control, &mut callback);
    control.enable_native_custody().unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
        )
        .unwrap();
    assert!(
        control
            .configure_ordinary_streaming_on_control(
                crate::AudioCandidateDomainReservation {
                    max_retained_data_bytes: 8 * 1024 * 1024,
                    max_construction_peak_data_bytes: 8 * 1024 * 1024,
                },
                3,
                4096,
            )
            .is_ok()
    );
    assert!(control.candidate_root.ordinary_streams_configured());
}

#[test]
#[cfg(feature = "allocation-probe")]
fn verified_streaming_accepts_both_outcome_modes() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = super::candidate_tests::serial();
    for reliable in [false, true] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let _reader = reliable.then(|| wire_action_outcomes(&mut control, &mut callback));
        let root = control.native_candidate_root();
        let (data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        let worker = publication.control().unwrap();
        worker.enable_native_custody().unwrap();
        worker
            .configure_audio_candidate_publication(crate::AudioCandidatePublicationReservation {
                runtime_slots: 2,
                archive_slots: 2,
                member_rows: 256,
                transfer_rows: 256,
                receipt_rows: 3,
                data_rows: 4096,
                handle_rows: 2048,
                max_retained_data_bytes: 96 * 1024 * 1024,
                max_construction_peak_data_bytes: 96 * 1024 * 1024,
            })
            .unwrap();
        assert!(worker.configure_audio_candidate_streaming(3).is_ok());
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
}

#[test]
fn initial_stale_fence_rejects_play_without_active_playback() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    #[cfg(not(feature = "allocation-probe"))]
    callback.begin_block();
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(|| callback.begin_block());
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
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    assert_eq!(
        control.status.rejected_revision().map(|r| r.0),
        Some(revision)
    );
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::NoActivePlayback))
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn priority_stop_discards_play_without_active_playback() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    control.stop();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    assert_eq!(control.status.rejected_action(), None);
    assert!(!control.status.playing.load(Acquire));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn pre_fault_session_launch_cannot_restart_or_change_route() {
    use hex_engine::{LaunchQuantization, PlaybackAction, SessionPlaybackCommand};
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, _, clip, _) = session_audio_graph();
    control.publish(playback).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 16]).unwrap();
    let ticket = control
        .action_with_ticket(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 16);
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
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::OutputFailed))
    );
    let mut routes = [SessionRouteEntry::default(); 1];
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
    assert!(routes[0].pending.is_none());
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn priority_stop_clears_pending_and_queued_session_launches_before_later_play() {
    use hex_engine::{LaunchQuantization, PlaybackAction, SessionPlaybackCommand};
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, weak, clip, track) = session_audio_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 1]).unwrap();
    let quantized = SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    };
    control
        .action_with_ticket(PlaybackAction::Session(quantized))
        .unwrap();
    callback.begin_block();
    let mut routes = [SessionRouteEntry::default(); 1];
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(routes[0].track_id, track.value());
    assert!(routes[0].pending.is_some());
    assert_eq!(
        routes[0].pending.unwrap().target,
        SessionRouteTarget::Session {
            clip_id: clip.value()
        }
    );
    let old_due = routes[0].pending.unwrap().performance_frame;
    assert_eq!(old_due, 96_000);

    let old_ticket = control
        .action_with_ticket(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    control.stop();
    let play_ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    assert!(play_ticket > old_ticket);
    let mut output = [[0.0; 2]; 128];
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
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
    assert_eq!(output, [[0.0; 2]; 128]);
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.consumed_command.load(Acquire), play_ticket);
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
    assert_eq!(routes[0].pending, None);

    // Cross the old due frame with real callback-sized blocks: no stale
    // Session audio or deferred launch may reappear after the Stop.
    let mut rendered = output.len() as u64;
    while rendered <= old_due {
        let n = ((old_due + 1 - rendered) as usize).min(output.len());
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output[..n]).unwrap();
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
        assert!(output[..n].iter().all(|frame| *frame == [0.0; 2]));
        rendered += n as u64;
    }
    assert_eq!(control.status.position.load(Acquire), old_due + 1);
    assert_eq!(control.status.performance_frame.load(Acquire), old_due + 1);
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
    assert_eq!(routes[0].pending, None);
    control
        .action_with_ticket(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output[..4]).unwrap();
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
    assert_eq!(
        &output[..4],
        &[[0.25, -0.5], [0.75, -1.0], [0.0; 2], [0.0; 2]]
    );
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(
        routes[0].owner,
        SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );
    control.stop();
    let stopped = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output[..4]).unwrap();
    });
    assert_eq!(
        (
            stopped.count_total,
            stopped.bytes_total,
            stopped.count_current,
            stopped.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(&output[..4], &[[0.0; 2]; 4]);
    control.request_shutdown();
    let shutdown = allocation_counter::measure(|| callback.begin_block());
    assert_eq!(
        (
            shutdown.count_total,
            shutdown.bytes_total,
            shutdown.count_current,
            shutdown.bytes_current
        ),
        (0, 0, 0, 0)
    );
    control.reclaim();
    assert!(weak.upgrade().is_none());
}

#[test]
fn retired_session_audio_pcm_stays_owned_until_control_reclaims_publication() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, weak_revision, _, _) = session_audio_graph();
    callback.held_retirement = Some(ReturnedPublication::Legacy(Publication {
        pcm_frames: 0,
        forwarded_pcm: false,
        revision: 1,
        control_context_generation: 0,
        explicit_control_context: false,
        source_context: None,
        policy: PublicationPolicy::InheritRuntime,
        fresh_runtime_cancellation: None,
        scheduled_cut_frame: None,
        streams: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        native_references: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        native_continuation: None,
        playback: Box::new(playback),
    }));
    assert!(weak_revision.upgrade().is_some());
    callback.begin_block();
    assert!(weak_revision.upgrade().is_some());
    assert_eq!(control.reclaim(), 1);
    assert!(weak_revision.upgrade().is_none());
}

#[test]
fn reset_publication_drops_same_id_session_owner_after_stop_and_reclaim() {
    use hex_engine::{LaunchQuantization, SessionPlaybackCommand};

    let (mut control, mut callback) = channel(48_000).unwrap();
    let (old, old_weak, clip, track) = session_audio_graph();
    assert!(control.publish(old).is_ok());
    callback.begin_block();
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    let mut first_audio = [[0.0; 2]; 8];
    callback.render(&mut first_audio).unwrap();
    assert_eq!(control.status.performance_frame.load(Acquire), 8);
    let mut old_routes = [SessionRouteEntry::default(); 1];
    let old_snapshot = control
        .status
        .session_routes(&mut old_routes)
        .expect("old active route");
    assert_eq!(old_routes[0].track_id, track.value());
    assert_eq!(
        old_routes[0].owner,
        SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );

    let (replacement, _, same_clip, same_track) = session_audio_graph();
    assert_eq!(same_clip, clip);
    assert_eq!(same_track, track);
    assert!(
        control
            .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
            .is_ok()
    );
    callback.test_transient_handoff_refusals = 1;
    callback.begin_block();
    assert!(callback.deferred.is_some());
    assert!(old_weak.upgrade().is_some());
    assert_eq!(
        old_routes[0].owner,
        SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );
    control.stop();
    callback.begin_block();
    assert!(callback.deferred.is_none());
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    let mut new_routes = [SessionRouteEntry::default(); 1];
    let new_snapshot = control
        .status
        .session_routes(&mut new_routes)
        .expect("replacement active route");
    assert!(new_snapshot.audio_revision > old_snapshot.audio_revision);
    assert_eq!(new_routes[0].owner, SessionRouteOwner::Arrangement);
    assert_eq!(new_routes[0].pending, None);
    assert!(old_weak.upgrade().is_some());

    let mut paused = [[1.0; 2]; 4];
    callback.render(&mut paused).unwrap();
    assert_eq!(paused, [[0.0; 2]; 4]);
    assert_eq!(control.reclaim(), 1);
    assert!(old_weak.upgrade().is_none());
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_runtime_waits_for_retirement_space_without_delaying_stop() {
    use hex_engine::{LaunchQuantization, SessionPlaybackCommand};

    let (mut control, mut callback) = channel(48_000).unwrap();
    let (old, old_weak, clip, _) = session_audio_graph();
    assert!(control.publish(old).is_ok());
    callback.begin_block();
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 8]).unwrap();

    // Fill the real retirement ring: publication must wait for control-side
    // reclamation rather than dropping the active graph on the callback.
    let mut retired_weak = Vec::new();
    for revision in 10..10 + RETIRED as u64 {
        let (playback, weak, _, _) = session_audio_graph();
        assert!(
            callback
                .retired
                .push(ReturnedPublication::Legacy(Publication {
                    pcm_frames: 0,
                    forwarded_pcm: false,
                    revision,
                    control_context_generation: 0,
                    explicit_control_context: false,
                    source_context: None,
                    policy: PublicationPolicy::InheritRuntime,
                    fresh_runtime_cancellation: None,
                    scheduled_cut_frame: None,
                    streams: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_references: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation: None,
                    playback: Box::new(playback),
                }))
                .is_ok()
        );
        retired_weak.push(weak);
    }
    let (replacement, replacement_weak, _, _) = session_audio_graph();
    assert!(
        control
            .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
            .is_ok()
    );
    control.stop();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(control.status.publication_pending.load(Acquire));
    assert!(!control.status.playing.load(Acquire));
    let mut routes = [SessionRouteEntry::default(); 1];
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(
        routes[0].owner,
        SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );
    let mut output = [[9.0; 2]; 8];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 8]);
    assert!(old_weak.upgrade().is_some());
    assert!(replacement_weak.upgrade().is_some());
    assert!(retired_weak.iter().all(|weak| weak.upgrade().is_some()));

    assert_eq!(control.reclaim(), RETIRED);
    assert!(retired_weak.iter().all(|weak| weak.upgrade().is_none()));
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    assert!(!control.status.publication_pending.load(Acquire));
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
    assert_eq!(routes[0].pending, None);
    assert!(old_weak.upgrade().is_some());
    assert_eq!(control.reclaim(), 1);
    assert!(old_weak.upgrade().is_none());
    control.request_shutdown();
    callback.begin_block();
    assert!(replacement_weak.upgrade().is_some());
    assert_eq!(control.reclaim(), 1);
    assert!(replacement_weak.upgrade().is_none());
}

#[test]
fn held_retirement_does_not_block_stop_or_drop_ownership() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(graph().0).is_ok());
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));

    // Force the defensive retention path. Normal reserved swaps cannot
    // fill it, but it must retain ownership and honor Stop if exercised.
    let mut retired_weak = Vec::new();
    for revision in 2..=3 {
        let (playback, weak) = graph();
        assert!(
            callback
                .retired
                .push(ReturnedPublication::Legacy(Publication {
                    pcm_frames: 0,
                    forwarded_pcm: false,
                    revision,
                    control_context_generation: 0,
                    explicit_control_context: false,
                    source_context: None,
                    policy: PublicationPolicy::InheritRuntime,
                    fresh_runtime_cancellation: None,
                    scheduled_cut_frame: None,
                    streams: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_references: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation: None,
                    playback: Box::new(playback),
                }))
                .is_ok()
        );
        retired_weak.push(weak);
    }
    let (held, held_weak) = graph();
    callback.held_retirement = Some(ReturnedPublication::Legacy(Publication {
        pcm_frames: 0,
        forwarded_pcm: false,
        revision: 4,
        control_context_generation: 0,
        explicit_control_context: false,
        source_context: None,
        policy: PublicationPolicy::InheritRuntime,
        fresh_runtime_cancellation: None,
        scheduled_cut_frame: None,
        streams: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        native_references: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        native_continuation: None,
        playback: Box::new(held),
    }));
    let mut last_ticket = 0;
    for _ in 0..COMMANDS {
        last_ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    }
    control.stop();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.consumed_command.load(Acquire), last_ticket);
    assert!(callback.held_retirement.is_some());
    assert!(held_weak.upgrade().is_some());
    assert!(retired_weak.iter().all(|weak| weak.upgrade().is_some()));
    let mut output = [[9.0; 2]; 8];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 8]);

    assert_eq!(control.reclaim(), RETIRED);
    assert!(retired_weak.iter().all(|weak| weak.upgrade().is_none()));
    control.request_shutdown();
    callback.begin_block();
    assert!(control.status.shutdown_complete.load(Acquire));
    assert!(held_weak.upgrade().is_some());
    assert_eq!(control.reclaim(), RETIRED);
    assert!(held_weak.upgrade().is_none());
}

#[test]
fn transient_handoff_keeps_candidate_gated_retries_and_old_graph_renders() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.set_audio_diagnostics(AudioDiagnosticsFlags::READINESS);
    let (old, _) = graph();
    assert_eq!(
        control
            .publish(old)
            .unwrap_or_else(|error| panic!("{}", error.reason)),
        1
    );
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let mut output = [[0.0; 2]; 16];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.25; 2]; 16]);

    let (candidate, _) = graph();
    assert_eq!(
        control
            .publish(candidate)
            .unwrap_or_else(|error| panic!("{}", error.reason)),
        2
    );
    callback.test_transient_handoff_refusals = 2;
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    let waiting = control.take_audio_diagnostic().unwrap();
    assert_eq!(waiting.revision, 2);
    assert_eq!(
        waiting.kind,
        AudioDiagnosticKind::PublicationWaiting {
            reason: PublicationWaitReason::RuntimeHandoff(
                hex_engine::PlaybackHandoffError::Session(
                    hex_engine::SessionPlaybackError::ReleaseBankFull,
                ),
            ),
        }
    );
    assert!(callback.deferred.is_some());
    assert_eq!(
        control.publish(graph().0).unwrap_err().reason,
        "playback publication already pending"
    );
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.25; 2]; 16]);

    control.stop();
    callback.begin_block();
    assert!(callback.deferred.is_some());
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(!control.status.playing.load(Acquire));
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 16]);

    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    assert!(callback.deferred.is_none());
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn rejection_snapshots_never_pair_a_new_reason_with_an_old_ticket() {
    let status = Arc::new(Status::default());
    let writer = Arc::clone(&status);
    let thread = std::thread::spawn(move || {
        for ticket in 1..=50_000 {
            let reason = if ticket & 1 == 0 {
                PlaybackActionRejection::SessionNotPrepared
            } else {
                PlaybackActionRejection::SessionUnavailable
            };
            writer.publish_rejected_action(ticket, reason);
        }
    });
    while !thread.is_finished() {
        if let Some((ticket, reason)) = status.rejected_action() {
            let expected = if ticket & 1 == 0 {
                PlaybackActionRejection::SessionNotPrepared
            } else {
                PlaybackActionRejection::SessionUnavailable
            };
            assert_eq!(reason, expected);
        }
    }
    thread.join().unwrap();
    assert_eq!(
        status.rejected_action(),
        Some((50_000, PlaybackActionRejection::SessionNotPrepared))
    );
}

#[test]
fn shutdown_returns_active_deferred_and_queued_graphs_for_control_drops() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (active, active_weak) = graph();
    assert_eq!(
        control
            .publish(active)
            .unwrap_or_else(|error| panic!("{}", error.reason)),
        1
    );
    callback.begin_block();

    let (deferred, deferred_weak) = graph();
    assert_eq!(
        control
            .publish(deferred)
            .unwrap_or_else(|error| panic!("{}", error.reason)),
        2
    );
    callback.test_transient_handoff_refusals = 1;
    callback.begin_block();
    assert_eq!(
        callback.deferred.as_ref().map(|value| value.revision),
        Some(2)
    );

    // Model an already queued final owner while shutdown admission closes.
    let (queued, queued_weak) = graph();
    assert!(
        control
            .publications
            .push(QueuedPublication::Legacy(Publication {
                pcm_frames: 0,
                forwarded_pcm: false,
                revision: 3,
                control_context_generation: 0,
                explicit_control_context: false,
                source_context: None,
                policy: PublicationPolicy::InheritRuntime,
                fresh_runtime_cancellation: None,
                scheduled_cut_frame: None,
                streams: None,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                native_references: None,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                native_continuation: None,
                playback: Box::new(queued),
            }))
            .is_ok()
    );
    control.request_shutdown();
    callback.begin_block();
    assert!(active_weak.upgrade().is_some());
    assert!(deferred_weak.upgrade().is_some());
    assert!(queued_weak.upgrade().is_some());
    while !control.status.shutdown_complete.load(Acquire) {
        control.reclaim();
        callback.begin_block();
    }
    control.reclaim();
    assert!(active_weak.upgrade().is_none());
    assert!(deferred_weak.upgrade().is_none());
    assert!(queued_weak.upgrade().is_none());
}
