use super::*;
fn zero(measured: allocation_counter::AllocationInfo) {
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

#[test]
fn current_streamed_audio_continues_while_active_span_is_pending() {
    use hex_stream::{ProtectionRole, ReadinessProgress, ReadinessStamp, StreamPageExtent};

    let _serial = serial();
    assert!(hex_engine::audio_storage_constructor_layout_supported());
    let project = readiness::project_with_frames(12_288);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(12_288);
    let definition = wav.definition(asset, 11);
    let key = definition.metadata.key();
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 7,
        }),
    );
    let identity = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![definition], 5),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .unwrap_or_else(|refused| panic!("{}", refused.reason));
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut baseline = [[0.0; 2]; 8];
    while baseline.iter().all(|frame| *frame == [0.0; 2]) {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut baseline).unwrap();
            })
        }));
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }

    let extent = |page_index| StreamPageExtent {
        source: key,
        page_index,
        start: 0,
        end: 8,
    };
    let candidate = [extent(2)];
    let span = [extent(0), extent(1)];
    let mut candidate_stamp = None;
    let mut pending_stamp = None;
    data.owner_mut().unwrap().run(|c, _| {
        let prefix = c.stream_prefix;
        let pages = c.domain_pages.iter_mut().flatten().next().unwrap();
        let (loan, epoch, root) = pages.test_loan_and_epoch().unwrap();
        *epoch += 1;
        let stamp = ReadinessStamp {
            request_epoch: *epoch,
            artifact: identity.into(),
            project_generation: 0,
            editor_revision: 7,
            device_epoch: root,
            device_rate: 48_000,
            predecessor: None,
            admitted_command_prefix: prefix,
            window: hex_engine::StreamChunkWindow {
                performance_frame: 0,
                arrangement_frame: 0,
                route_generation: 0,
                frames: 8,
            },
        };
        loan.submit(ProtectionRole::Candidate, stamp, &candidate)
            .unwrap();
        candidate_stamp = Some(stamp);
    });
    let candidate_stamp = candidate_stamp.unwrap();
    loop {
        let mut ready = false;
        data.owner_mut().unwrap().run(|c, _| {
            let pages = c.domain_pages.iter_mut().flatten().next().unwrap();
            let (loan, _, _) = pages.test_loan_and_epoch().unwrap();
            ready = loan
                .service(ProtectionRole::Candidate, candidate_stamp, 3, 3)
                .unwrap()
                == ReadinessProgress::InstalledProtected;
            if ready {
                assert!(loan.acknowledge(ProtectionRole::Candidate, candidate_stamp, &candidate));
            }
        });
        if ready {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    data.owner_mut().unwrap().run(|c, _| {
        let pages = c.domain_pages.iter_mut().flatten().next().unwrap();
        let (loan, epoch, _) = pages.test_loan_and_epoch().unwrap();
        *epoch += 1;
        let mut pending = candidate_stamp;
        pending.request_epoch = *epoch;
        loan.submit(ProtectionRole::Active, pending, &span).unwrap();
        pending_stamp = Some(pending);
    });
    let pending_stamp = pending_stamp.unwrap();

    let before = control.status.performance_frame.load(Acquire);
    let mut actual = [[0.0; 2]; 8];
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut actual).unwrap();
        })
    }));
    let after = control.status.performance_frame.load(Acquire);
    data.owner_mut().unwrap().run(|c, _| {
        let pages = c.domain_pages.iter_mut().flatten().next().unwrap();
        let (loan, _, _) = pages.test_loan_and_epoch().unwrap();
        assert!(loan.matches_intent(ProtectionRole::Active, pending_stamp, &span));
        assert!(loan.is_protected(ProtectionRole::Candidate, candidate_stamp, &candidate));
        assert!(loan.cancel(ProtectionRole::Active, pending_stamp.request_epoch));
        assert!(loan.cancel(ProtectionRole::Candidate, candidate_stamp.request_epoch));
    });
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    crate::lifecycle::test_dispose_backend(
        &mut backend,
        &mut pending,
        &mut disposed,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    let ready = recover(&mut control, &ticket);
    assert_eq!(ready.disposed.identity(), identity);
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(ready);
    drop(control);
    finish(&mut retirement);
    assert_eq!(
        (actual, after),
        (baseline, before + 8),
        "pending span must not silence or hold the current window"
    );
}
#[test]
fn split_client_streams_and_retires_its_domain_after_quiescence() {
    split_client_stream_case(StreamCase::Legacy);
}
#[test]
fn split_client_staged_stream_commit_reports_exact_ticket() {
    split_client_stream_case(StreamCase::Staged);
}
#[test]
fn split_client_receives_busy_then_older_cancelled_seek() {
    split_client_stream_case(StreamCase::BusyCancel);
}
#[test]
fn split_client_stop_survives_full_outcome_capacity_and_retry() {
    split_client_stream_case(StreamCase::Pressure);
}
#[test]
fn split_client_shutdown_without_another_callback_interrupts_pending_tickets() {
    split_client_stream_case(StreamCase::Shutdown);
}
#[test]
fn split_client_reports_schedule_once_before_activation_and_cancel() {
    split_client_stream_case(StreamCase::Schedule);
}
#[test]
fn split_client_replacement_cancels_pending_stream_action() {
    split_client_stream_case(StreamCase::Replacement);
}
#[test]
fn split_client_device_fault_finishes_pending_and_queued_actions() {
    split_client_stream_case(StreamCase::Fault);
}
#[test]
fn split_client_receipt_waits_for_stream_status_publication() {
    split_client_stream_case(StreamCase::StatusVisibility);
}
#[test]
fn split_client_reports_changed_source_for_pending_seek() {
    split_client_stream_case(StreamCase::SourceChanged);
}
#[test]
fn split_client_rejects_second_play_while_first_waits_for_stream() {
    split_client_stream_case(StreamCase::DoublePlay);
}
#[test]
fn streamed_terminal_precedes_resident_consumption_in_same_block() {
    split_client_stream_case(StreamCase::CrossMode);
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamCase {
    Legacy,
    Staged,
    BusyCancel,
    Pressure,
    Shutdown,
    Schedule,
    Replacement,
    Fault,
    StatusVisibility,
    SourceChanged,
    DoublePlay,
    CrossMode,
}
fn split_client_stream_case(case: StreamCase) {
    let outcomes = case != StreamCase::Legacy;
    let busy_cancel = case == StreamCase::BusyCancel;
    let pressure = case == StreamCase::Pressure;
    let shutdown = case == StreamCase::Shutdown;
    let schedule = case == StreamCase::Schedule;
    let replacement = case == StreamCase::Replacement;
    let fault = case == StreamCase::Fault;
    let visibility = case == StreamCase::StatusVisibility;
    let source_changed = case == StreamCase::SourceChanged;
    let double_play = case == StreamCase::DoublePlay;
    let cross_mode = case == StreamCase::CrossMode;
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let mut project = readiness::project_with_frames(12_288);
    if schedule {
        let mut snapshot = project.snapshot();
        snapshot.loop_region = Some(hex_project::LoopRegion::new(0, 11487, true).unwrap());
        let scene = snapshot.next_id;
        let clip = scene + 1;
        snapshot.next_id += 2;
        snapshot.session.scenes.push(hex_project::SceneSnapshot {
            id: scene,
            name: "launch".into(),
        });
        snapshot
            .session
            .cells
            .push(hex_project::SessionCellSnapshot {
                track: snapshot.tracks[0].id,
                scene,
                content: hex_project::SessionCellContentSnapshot::Audio(
                    hex_project::SessionAudioClipSnapshot {
                        loop_range: None,
                        active: true,
                        flavor: hex_project::ClipFlavor::default(),
                        gain: hex_project::ClipGain::UNITY,
                        id: clip,
                        name: "future".into(),
                        asset: snapshot.assets[0].id,
                        source_start: 8192,
                        source_end: 12288,
                    },
                ),
            });
        project = hex_project::Project::from_snapshot(snapshot).unwrap();
    }
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(12_288);
    let definition = wav.definition(asset, 11);
    let source_path = definition.path.clone();
    let replacement_metadata = definition.metadata.clone();
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 25,
        }),
    );
    let identity = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = if outcomes {
        let reader = control
            .enable_action_outcomes_on_control(&mut callback)
            .unwrap();
        Some(reader)
    } else {
        None
    };
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        crate::native_callback_owner::reserve_native_worker(control, callback, None);
    let mut retirement =
        crate::native_callback_owner::NativeRetirement::new(terminals, root.clone());
    let control = publication.control().unwrap();
    control.enable_native_custody().unwrap();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(definition),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    client.activate();
    let play = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    let second_play =
        double_play.then(|| client.command_with_ticket(PlaybackCommand::Play).unwrap());
    let mut output = [[0.0; 2]; 8];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while client.status().unwrap().applied_revision.load(Acquire) == 0 {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
        }));
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output, [[0.25; 2]; 8]);
    if cross_mode {
        let reader = reader.as_mut().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: play,
                outcome: ActionOutcome::Applied
            })
        );
        let seek = client
            .command_with_ticket(PlaybackCommand::Seek(4096))
            .unwrap();
        data.owner_mut().unwrap().run(|c, _| c.begin_block());
        let pause = client.command_with_ticket(PlaybackCommand::Pause).unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.cancel_active_stream_action();
                c.consume_commands_through(command_cut(pause, 0));
            })
        }));
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: seek,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::StreamActionCancelled)
            })
        );
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: pause,
                outcome: ActionOutcome::Applied
            })
        );
        assert_eq!(reader.failure(), None);
    }
    if let Some(second) = second_play {
        let reader = reader.as_mut().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: second,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::StreamActionBusy)
            })
        );
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: play,
                outcome: ActionOutcome::Applied
            })
        );
        assert_eq!(reader.failure(), None);
    }
    if source_changed {
        let reader = reader.as_mut().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: play,
                outcome: ActionOutcome::Applied
            })
        );
        std::fs::write(source_path, b"changed source bytes").unwrap();
        let seek = client
            .command_with_ticket(PlaybackCommand::Seek(4096))
            .unwrap();
        let receipt = loop {
            zero(allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|c, _| {
                    c.begin_block();
                    c.render(&mut output).unwrap();
                })
            }));
            if let Some(receipt) = reader.try_take() {
                break receipt;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(
            receipt,
            ActionOutcomeReceipt {
                ticket: seek,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::StreamSourceChanged)
            }
        );
        assert_eq!(reader.failure(), None);
    }
    if visibility {
        let reader = reader.as_mut().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: play,
                outcome: ActionOutcome::Applied
            })
        );
        let pause = client.command_with_ticket(PlaybackCommand::Pause).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        data.owner_mut().unwrap().run(|callback, _| {
            callback.test_before_action_outcome_status = Some(Arc::clone(&barrier))
        });
        std::thread::scope(|scope| {
            let audio = scope.spawn(|| data.owner_mut().unwrap().run(|c, _| c.begin_block()));
            barrier.wait();
            assert_eq!(reader.try_take(), None);
            barrier.wait();
            audio.join().unwrap();
        });
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: pause,
                outcome: ActionOutcome::Applied
            })
        );
        // A streamed Pause is Applied at once but plays through its fade (#4151).
        assert!(client.status().unwrap().playing.load(Acquire));
        assert_eq!(
            client.status().unwrap().consumed_command.load(Acquire),
            pause
        );
    }
    if fault {
        let reader = reader.as_mut().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: play,
                outcome: ActionOutcome::Applied
            })
        );
        let seek = client
            .command_with_ticket(PlaybackCommand::Seek(4096))
            .unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        let queued = client
            .command_with_ticket(PlaybackCommand::PlayFrom(0))
            .unwrap();
        client.status().unwrap().device_fault.store(true, Release);
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.fence_device_fault())
        }));
        for ticket in [seek, queued] {
            assert_eq!(
                reader.try_take(),
                Some(ActionOutcomeReceipt {
                    ticket,
                    outcome: ActionOutcome::Rejected(PlaybackActionRejection::OutputFailed)
                })
            );
        }
        assert_eq!(reader.failure(), None);
    }
    if replacement {
        let reader = reader.as_mut().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: play,
                outcome: ActionOutcome::Applied
            })
        );
        let seek = client
            .command_with_ticket(PlaybackCommand::Seek(4096))
            .unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        let next = stream_fixture::owner_with_context(
            &project,
            replacement_metadata,
            Some(hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 26,
            }),
        );
        let _next_ticket = publication
            .control()
            .unwrap()
            .stage_eligible_audio_candidate(
                next,
                vec![domain],
                PublicationPolicy::InheritRuntime,
                0,
            )
            .ok()
            .unwrap();
        let receipt = loop {
            if let Some(receipt) = reader.try_take() {
                break receipt;
            }
            zero(allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|c, _| {
                    c.begin_block();
                    c.render(&mut output).unwrap();
                })
            }));
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(
            receipt,
            ActionOutcomeReceipt {
                ticket: seek,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::StreamActionCancelled)
            }
        );
        assert_eq!(reader.try_take(), None);
        assert_eq!(reader.failure(), None);
        while client.status().unwrap().applied_revision.load(Acquire) < 2 {
            zero(allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|c, _| {
                    c.begin_block();
                    c.render(&mut output).unwrap();
                })
            }));
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(reader.try_take(), None);
    }
    if schedule {
        let reader = reader.as_mut().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: play,
                outcome: ActionOutcome::Applied
            })
        );
        let launched = client
            .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchScene {
                scene: project.scenes()[0].id(),
                quantization: hex_engine::LaunchQuantization::FourQuarter,
            })
            .unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        assert_eq!(
            client
                .status()
                .unwrap()
                .stream_scheduled_command
                .load(Acquire),
            launched
        );
        assert_eq!(
            client
                .status()
                .unwrap()
                .stream_activated_command
                .load(Acquire),
            0
        );
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: launched,
                outcome: ActionOutcome::Applied
            })
        );
        let cancel = client
            .session_command_with_ticket(hex_engine::SessionPlaybackCommand::CancelPending {
                track: Some(project.tracks()[0].id()),
            })
            .unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: cancel,
                outcome: ActionOutcome::Applied
            })
        );
        assert_eq!(reader.try_take(), None);
        assert_eq!(reader.failure(), None);
        let later = client
            .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchScene {
                scene: project.scenes()[0].id(),
                quantization: hex_engine::LaunchQuantization::FourQuarter,
            })
            .unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: later,
                outcome: ActionOutcome::Applied
            })
        );
        let mut long_output = [[0.0; 2]; 1024];
        while client
            .status()
            .unwrap()
            .stream_activated_command
            .load(Acquire)
            != later
        {
            zero(allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|c, _| {
                    c.begin_block();
                    c.render(&mut long_output).unwrap();
                })
            }));
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(reader.try_take(), None);
        assert_eq!(reader.failure(), None);
    }
    if shutdown {
        let reader = reader.as_mut().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: play,
                outcome: ActionOutcome::Applied
            })
        );
        let pending = client
            .command_with_ticket(PlaybackCommand::Seek(4096))
            .unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        let queued = client.command_with_ticket(PlaybackCommand::Pause).unwrap();
        let mut backend = Some(());
        let mut dispose_pending = false;
        let mut disposed = false;
        crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut dispose_pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
        .unwrap();
        drop(data);
        drop(errors);
        drop(client);
        drop(publication);
        finish(&mut retirement);
        let mut got = [reader.try_take().unwrap(), reader.try_take().unwrap()];
        got.sort_by_key(|receipt| receipt.ticket);
        assert_eq!(
            got,
            [
                ActionOutcomeReceipt {
                    ticket: pending,
                    outcome: ActionOutcome::Interrupted
                },
                ActionOutcomeReceipt {
                    ticket: queued,
                    outcome: ActionOutcome::Interrupted
                },
            ]
        );
        assert_eq!(reader.failure(), None);
        return;
    }
    if busy_cancel {
        let seek = client
            .command_with_ticket(PlaybackCommand::Seek(4096))
            .unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
        }));
        assert_eq!(output, [[0.0; 2]; 8]);
        let busy = client
            .command_with_ticket(PlaybackCommand::PlayFrom(0))
            .unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        let pause = client.command_with_ticket(PlaybackCommand::Pause).unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        let reader = reader.as_mut().unwrap();
        for (ticket, outcome) in [
            (play, ActionOutcome::Applied),
            (
                busy,
                ActionOutcome::Rejected(PlaybackActionRejection::StreamActionBusy),
            ),
            (
                seek,
                ActionOutcome::Rejected(PlaybackActionRejection::StreamActionCancelled),
            ),
            (pause, ActionOutcome::Applied),
        ] {
            assert_eq!(
                reader.try_take(),
                Some(ActionOutcomeReceipt { ticket, outcome })
            );
        }
    }
    if pressure {
        let reader = reader.as_mut().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: play,
                outcome: ActionOutcome::Applied
            })
        );
        let pending = client
            .command_with_ticket(PlaybackCommand::Seek(4096))
            .unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        let mut tickets = vec![pending];
        for _ in 1..ACTION_OUTCOME_CAPACITY {
            tickets.push(client.command_with_ticket(PlaybackCommand::Pause).unwrap());
        }
        assert!(client.command_with_ticket(PlaybackCommand::Pause).is_err());
        client.stop();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        assert!(!client.status().unwrap().playing.load(Acquire));
        assert_eq!(client.status().unwrap().position.load(Acquire), 0);
        for ticket in tickets {
            assert_eq!(
                reader.try_take(),
                Some(ActionOutcomeReceipt {
                    ticket,
                    outcome: ActionOutcome::Superseded
                })
            );
        }
        let retry = client.command_with_ticket(PlaybackCommand::Pause).unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| c.begin_block())
        }));
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: retry,
                outcome: ActionOutcome::Applied
            })
        );
        assert_eq!(reader.failure(), None);
    }
    if let Some(reader) = reader.as_mut().filter(|_| {
        !busy_cancel
            && !pressure
            && !schedule
            && !replacement
            && !fault
            && !visibility
            && !source_changed
            && !double_play
            && !cross_mode
    }) {
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: play,
                outcome: ActionOutcome::Applied,
            })
        );
        let from = client
            .command_with_ticket(PlaybackCommand::PlayFrom(4096))
            .unwrap();
        while client.status().unwrap().position.load(Acquire) != 4104 {
            zero(allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|c, _| {
                    c.begin_block();
                    c.render(&mut output).unwrap();
                })
            }));
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket: from,
                outcome: ActionOutcome::Applied,
            })
        );
    }
    client.stop();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
    }));
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    crate::lifecycle::test_dispose_backend(
        &mut backend,
        &mut pending,
        &mut disposed,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    if outcomes {
        drop(client);
        drop(publication);
        finish(&mut retirement);
        while !root.test_eligible_complete() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        return;
    }
    retirement.advance().unwrap();
    let control = publication.control().unwrap();
    let ready = loop {
        control.reclaim();
        if let Some(ready) = control.recover_audio_candidate(&ticket) {
            break ready;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    };
    assert_eq!(ready.disposed.identity(), identity);
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            assert_eq!(report.page_buffers, 4);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(ready);
    drop(client);
    drop(publication);
    finish(&mut retirement);
}
#[test]
fn changed_source_rejects_streamed_seek_with_recovery_guidance() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let project = readiness::project_with_frames(12_288);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(12_288);
    let definition = wav.definition(asset, 11);
    let source_path = definition.path.clone();
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 25,
        }),
    );
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(definition),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 8];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while control.status.applied_revision.load(Acquire) == 0 {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
        }));
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output, [[0.25; 2]; 8]);
    std::fs::write(source_path, b"changed source bytes").unwrap();
    let seek = control
        .command_with_ticket(PlaybackCommand::Seek(4096))
        .unwrap();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
    }));
    assert_eq!(output, [[0.0; 2]; 8]);
    assert_eq!(control.status.position.load(Acquire), 8);
    let rejection = loop {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
        }));
        assert!(output == [[0.0; 2]; 8] || output == [[0.25; 2]; 8]);
        if let Some((observed, reason)) = control.status.rejected_action()
            && observed == seek
        {
            break reason;
        }
        assert!(std::time::Instant::now() < deadline, "seek must be refused");
        std::thread::yield_now();
    };
    assert!(control.status.position.load(Acquire) < 4096);
    control.stop();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
    }));
    assert_eq!(control.status.position.load(Acquire), 0);
    assert!(!control.status.playing.load(Acquire));
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    crate::lifecycle::test_dispose_backend(
        &mut backend,
        &mut pending,
        &mut disposed,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    retirement.advance().unwrap();
    let returned = recover(&mut control, &ticket);
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(returned);
    drop(control);
    finish(&mut retirement);
    assert_eq!(
        rejection.message(),
        "original media changed · reopen or relink it"
    );
}

#[test]
fn real_seek_waits_without_commit_busy_and_pause_cancel_then_playfrom_reads_exact_page() {
    seek_pause_stop_case(false);
}
#[test]
fn grouped_stream_seek_pause_and_priority_stop_preserve_pages_and_clocks() {
    seek_pause_stop_case(true);
}
fn seek_pause_stop_case(grouped: bool) {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let project = readiness::project_with_frames(12288);
    let project = if grouped {
        let mut editor = hex_project::Editor::new(project, 8).unwrap();
        let track = editor.project().tracks()[0].id();
        let hex_project::Created::Track(group) = editor
            .execute(hex_project::Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        editor
            .execute(hex_project::Command::Gain {
                track: group,
                gain: 0.5,
            })
            .unwrap();
        editor.project().clone()
    } else {
        project
    };
    let asset = project.assets().next().unwrap().id();
    let wav =
        stream_fixture::Wav::with_samples(12288, |frame| if frame < 4096 { 8192 } else { 16384 });
    let definition = wav.definition(asset, 11);
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 25,
        }),
    );
    let id = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(definition),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 8];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while control.status.applied_revision.load(Acquire) == 0 {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        });
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output, [[if grouped { 0.125 } else { 0.25 }; 2]; 8]);
    let seek = control
        .command_with_ticket(PlaybackCommand::Seek(4096))
        .unwrap();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
    }));
    assert_eq!(output, [[0.0; 2]; 8]);
    assert_eq!(control.status.position.load(Acquire), 8);
    assert_eq!(control.status.performance_frame.load(Acquire), 8);
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        play
    );
    assert_eq!(control.status.consumed_command.load(Acquire), seek);
    let busy = control
        .command_with_ticket(PlaybackCommand::PlayFrom(0))
        .unwrap();
    // Begin consumes the conflicting command before any ready seek can commit.
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| c.begin_block())
    }));
    assert_eq!(
        control.status.rejected_action(),
        Some((busy, PlaybackActionRejection::StreamActionBusy))
    );
    let pause = control.command_with_ticket(PlaybackCommand::Pause).unwrap();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
    }));
    assert_eq!(
        control.status.rejected_action(),
        Some((seek, PlaybackActionRejection::StreamActionCancelled))
    );
    assert_eq!(control.status.position.load(Acquire), 8);
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        pause
    );
    let from = control
        .command_with_ticket(PlaybackCommand::PlayFrom(4096))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while control.status.position.load(Acquire) != 4104 {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
        }));
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output, [[if grouped { 0.25 } else { 0.5 }; 2]; 8]);
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        from
    );
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    control.command(PlaybackCommand::Seek(8192)).unwrap();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
    }));
    assert_eq!(control.status.position.load(Acquire), 4104);
    for _ in 0..COMMANDS {
        control.command(PlaybackCommand::Seek(1)).unwrap();
    }
    control.stop();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
    }));
    assert_eq!(control.status.position.load(Acquire), 0);
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    assert!(!control.status.playing.load(Acquire));
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    let _ = crate::lifecycle::test_dispose_backend(
        &mut backend,
        &mut pending,
        &mut disposed,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    let ready = recover(&mut control, &ticket);
    assert_eq!(ready.disposed.identity(), id);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            assert_eq!(report.page_buffers, 4);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(ready);
    drop(control);
    finish(&mut retirement);
}

#[test]
fn real_immediate_scene_waits_before_route_and_play_then_restarts_actual_clip() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let mut snapshot = readiness::project_with_frames(8192).snapshot();
    snapshot.assets[0].source_sample_rate = 24_000;
    snapshot.assets[0].source_frames = 4096;
    snapshot.assets[0].conversion_version = hex_project::SINC_CONVERSION_V1;
    let scene = snapshot.next_id;
    let clip = scene + 1;
    snapshot.next_id += 2;
    snapshot.session.scenes.push(hex_project::SceneSnapshot {
        id: scene,
        name: "scene".into(),
    });
    snapshot
        .session
        .cells
        .push(hex_project::SessionCellSnapshot {
            track: snapshot.tracks[0].id,
            scene,
            content: hex_project::SessionCellContentSnapshot::Audio(
                hex_project::SessionAudioClipSnapshot {
                    loop_range: None,
                    active: true,
                    flavor: hex_project::ClipFlavor::default(),
                    gain: hex_project::ClipGain::UNITY,
                    id: clip,
                    name: "launch".into(),
                    asset: snapshot.assets[0].id,
                    source_start: 4608,
                    source_end: 8192,
                },
            ),
        });
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let asset = project.assets().next().unwrap().id();
    let scene = project.scenes()[0].id();
    let hex_project::SessionCellContent::Audio(audio) = project.session_cells()[0].content() else {
        panic!("audio fixture")
    };
    let clip = audio.id();
    let wav = stream_fixture::Wav::with_rate_and_samples(4096, 24_000, |f| {
        if f < 2048 { 8192 } else { 16384 }
    });
    let definition = wav.definition(asset, 11);
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 26,
        }),
    );
    let id = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(definition),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    let mut output = [[1.0; 2]; 8];
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        c.render(&mut output).unwrap();
    });
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    let launch = control
        .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
    }));
    assert_eq!(output, [[0.0; 2]; 8]);
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        0
    );
    data.owner_mut().unwrap().run(|c, _| {
        assert_eq!(
            c.active
                .as_ref()
                .unwrap()
                .current()
                .unwrap()
                .playback
                .session_route_status(0)
                .unwrap()
                .owner,
            hex_engine::SessionPlaybackOwner::Arrangement
        )
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while control.status.performance_frame.load(Acquire) == 0 {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
        }));
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output, [[0.5; 2]; 8]);
    assert_eq!(control.status.performance_frame.load(Acquire), 8);
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        launch
    );
    data.owner_mut().unwrap().run(|c, _| {
        assert_eq!(
            c.active
                .as_ref()
                .unwrap()
                .current()
                .unwrap()
                .playback
                .session_route_status(0)
                .unwrap()
                .owner,
            hex_engine::SessionPlaybackOwner::Session { clip }
        )
    });
    let restart = control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
    }));
    // The restarted clip starts at full level while its previous instance
    // fades out over the shared 128-frame declick.
    let restarted: Vec<_> = (0..8)
        .map(|frame| [0.5 + 0.5 * (128 - frame) as f32 / 128.0; 2])
        .collect();
    assert_eq!(output.as_slice(), restarted.as_slice());
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        restart
    );
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    let _ = crate::lifecycle::test_dispose_backend(
        &mut backend,
        &mut pending,
        &mut disposed,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    let ready = recover(&mut control, &ticket);
    assert_eq!(ready.disposed.identity(), id);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            assert_eq!(report.page_buffers, 4);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(ready);
    drop(control);
    finish(&mut retirement);
}

#[test]
fn original_quantized_due_survives_pause_cancel_and_revocable_prewarm_pool_pressure() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let mut snapshot = readiness::project_with_frames(12288).snapshot();
    snapshot.loop_region = Some(hex_project::LoopRegion::new(0, 11487, true).unwrap());
    let scene = snapshot.next_id;
    let clip = scene + 1;
    let empty = scene + 2;
    snapshot.next_id += 3;
    snapshot.session.scenes.extend([
        hex_project::SceneSnapshot {
            id: scene,
            name: "launch".into(),
        },
        hex_project::SceneSnapshot {
            id: empty,
            name: "empty".into(),
        },
    ]);
    snapshot
        .session
        .cells
        .push(hex_project::SessionCellSnapshot {
            track: snapshot.tracks[0].id,
            scene,
            content: hex_project::SessionCellContentSnapshot::Audio(
                hex_project::SessionAudioClipSnapshot {
                    loop_range: None,
                    active: true,
                    flavor: hex_project::ClipFlavor::default(),
                    gain: hex_project::ClipGain::UNITY,
                    id: clip,
                    name: "future".into(),
                    asset: snapshot.assets[0].id,
                    source_start: 8192,
                    source_end: 12288,
                },
            ),
        });
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let scene = project.scenes()[0].id();
    let empty = project.scenes()[1].id();
    let track = project.tracks()[0].id();
    let hex_project::SessionCellContent::Audio(clip) = project.session_cells()[0].content() else {
        panic!("audio fixture")
    };
    let clip = clip.id();
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_samples(12288, |f| {
        if f < 4096 {
            8192
        } else if f < 8192 {
            16384
        } else {
            24576
        }
    });
    let definition = wav.definition(asset, 11);
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 27,
        }),
    );
    let id = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(2).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![definition], 3),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|refused| panic!("{}", refused.reason));
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    let mut output = [[0.0; 2]; 1024];
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        c.render(&mut output[..8]).unwrap();
    });
    control.command(PlaybackCommand::Seek(4000)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while control.status.performance_frame.load(Acquire) != 4008 {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output[..8]).unwrap();
            })
        }));
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    // Empty scene is a genuine no-op, not a permanently occupied launch slot.
    control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchScene {
            scene: empty,
            quantization: hex_engine::LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let first = control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: hex_engine::LaunchQuantization::FourQuarter,
        })
        .unwrap();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| c.begin_block())
    }));
    assert_eq!(control.status.stream_scheduled_command.load(Acquire), first);
    assert_eq!(control.status.stream_activated_command.load(Acquire), 0);
    let mut due = 0;
    data.owner_mut().unwrap().run(|c, _| {
        due = c
            .active
            .as_ref()
            .unwrap()
            .current()
            .unwrap()
            .playback
            .session_route_status(0)
            .unwrap()
            .pending
            .unwrap()
            .performance_frame;
    });
    assert_eq!(due, 96000);
    control.command(PlaybackCommand::Pause).unwrap();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output[..8]).unwrap();
        })
    }));
    // The streamed Pause keeps the clock running through its fade (#4151).
    assert_eq!(control.status.performance_frame.load(Acquire), 4016);
    data.owner_mut().unwrap().run(|c, _| {
        assert_eq!(
            c.active
                .as_ref()
                .unwrap()
                .current()
                .unwrap()
                .playback
                .session_route_status(0)
                .unwrap()
                .pending
                .unwrap()
                .performance_frame,
            due
        )
    });
    control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::CancelPending {
            track: Some(track),
        })
        .unwrap();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| c.begin_block())
    }));
    data.owner_mut().unwrap().run(|c, _| {
        assert!(
            c.active
                .as_ref()
                .unwrap()
                .current()
                .unwrap()
                .playback
                .session_route_status(0)
                .unwrap()
                .pending
                .is_none()
        )
    });
    assert_eq!(
        control.status.stream_invalidated_schedule.load(Acquire),
        first
    );
    control.command(PlaybackCommand::Play).unwrap();
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        c.render(&mut output[..1]).unwrap();
    });
    let launch = control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::FourQuarter,
        })
        .unwrap();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| c.begin_block())
    }));
    assert_eq!(
        control.status.stream_scheduled_command.load(Acquire),
        launch
    );
    // Let the real worker install the early page, while the old page is Active.
    loop {
        let mut pinned = false;
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                if let Some(ActivePublication::Eligible(active)) = c.active.as_mut() {
                    pinned =
                        active.test_launch_prewarm_protected(&mut c.domain_pages, c.stream_prefix);
                }
            })
        }));
        if pinned {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(control.status.performance_frame.load(Acquire) < 4096);
    let mut crossed = false;
    let mut pressure_observed = false;
    while control.status.performance_frame.load(Acquire) < due {
        let before = control.status.performance_frame.load(Acquire);
        let count = usize::try_from((due - before).min(1024)).unwrap();
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output[..count]).unwrap();
            })
        }));
        if control.status.stream_buffering.load(Acquire)
            && control.status.performance_frame.load(Acquire) < due
        {
            // Actual current decode is pending after future pins revoked.
            // The same production service cannot reissue speculative work
            // before a real old-runtime chunk has committed.
            zero(allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|c, _| {
                    if let Some(ActivePublication::Eligible(active)) = c.active.as_mut() {
                        assert!(
                            !active
                                .test_service_launch_prewarm(&mut c.domain_pages, c.stream_prefix)
                        );
                    }
                })
            }));
            pressure_observed = true;
        }
        crossed |= control.status.performance_frame.load(Acquire) > 4096;
        assert_eq!(control.status.stream_activated_command.load(Acquire), 0);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(crossed && pressure_observed);
    assert_eq!(control.status.performance_frame.load(Acquire), due);
    // The final old chunk crossed page0→1 eight frames before due:
    // 96000 % 11487 == 4104. Page1 evicted speculative page2; blocked
    // prewarm could not reissue it. The actual due request must hold here.
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output[..8]).unwrap();
        })
    }));
    assert_eq!(control.status.performance_frame.load(Acquire), due);
    assert_eq!(control.status.stream_activated_command.load(Acquire), 0);
    assert_eq!(output[..8], [[0.0; 2]; 8]);
    loop {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output[..8]).unwrap();
            })
        }));
        if control.status.stream_activated_command.load(Acquire) == launch {
            break;
        }
        assert_eq!(control.status.performance_frame.load(Acquire), due);
        assert_eq!(output[..8], [[0.0; 2]; 8]);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output[..8], [[0.75; 2]; 8]);
    assert_eq!(control.status.performance_frame.load(Acquire), due + 8);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    data.owner_mut().unwrap().run(|c, _| {
        assert_eq!(
            c.active
                .as_ref()
                .unwrap()
                .current()
                .unwrap()
                .playback
                .session_route_status(0)
                .unwrap()
                .owner,
            hex_engine::SessionPlaybackOwner::Session { clip }
        )
    });
    // A real shared-domain successor invalidates old-owner prewarm correlation,
    // even while the musical PendingRoute may inherit under existing semantics.
    let obsolete = control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::BackToArrangement {
            quantization: hex_engine::LaunchQuantization::FourQuarter,
        })
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    loop {
        let mut pinned = false;
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                if let Some(ActivePublication::Eligible(active)) = c.active.as_mut() {
                    pinned =
                        active.test_launch_prewarm_protected(&mut c.domain_pages, c.stream_prefix);
                }
            })
        }));
        if pinned {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let successor = stream_fixture::owner_with_context(
        &project,
        wav.definition(asset, 11).metadata,
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 28,
        }),
    );
    let successor_id = successor.identity();
    // Actual device-rate mismatch refuses before queue/split, preserving owner.
    control.rate = 44_100;
    let refused = control
        .stage_eligible_audio_candidate(
            successor,
            vec![domain],
            PublicationPolicy::InheritRuntime,
            0,
        )
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), successor_id);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    control.rate = 48_000;
    // The caller cannot relabel the captured document generation either.
    let refused = control
        .stage_eligible_audio_candidate(
            refused.owner,
            refused.domains,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), successor_id);
    let successor_ticket = control
        .stage_eligible_audio_candidate(
            refused.owner,
            refused.domains,
            PublicationPolicy::InheritRuntime,
            0,
        )
        .ok()
        .unwrap();
    while control.status.applied_revision.load(Acquire) != 2 {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output[..8]).unwrap();
            })
        }));
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(
        control.status.stream_activated_command.load(Acquire),
        launch
    );
    assert_ne!(
        control.status.stream_activated_command.load(Acquire),
        obsolete
    );
    assert_ne!(
        control.status.stream_invalidated_schedule.load(Acquire),
        obsolete
    );
    let mut successor_due = 0;
    data.owner_mut().unwrap().run(|c, _| {
        successor_due = c
            .active
            .as_ref()
            .unwrap()
            .current()
            .unwrap()
            .playback
            .session_route_status(0)
            .unwrap()
            .pending
            .unwrap()
            .performance_frame
    });
    assert_eq!(successor_due, 192000);
    while control.status.performance_frame.load(Acquire) <= successor_due {
        let before = control.status.performance_frame.load(Acquire);
        let count = if before == successor_due {
            8
        } else {
            usize::try_from((successor_due - before).min(1024)).unwrap()
        };
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output[..count]).unwrap();
            })
        }));
        if control.status.performance_frame.load(Acquire) <= successor_due {
            assert_ne!(
                control.status.stream_activated_command.load(Acquire),
                obsolete
            );
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(
        control.status.stream_activated_command.load(Acquire),
        obsolete
    );
    data.owner_mut().unwrap().run(|c, _| {
        assert_eq!(
            c.active
                .as_ref()
                .unwrap()
                .current()
                .unwrap()
                .playback
                .session_route_status(0)
                .unwrap()
                .owner,
            hex_engine::SessionPlaybackOwner::Arrangement
        )
    });
    let old = recover(&mut control, &ticket);
    assert_eq!(old.disposed.identity(), id);
    drop(old);
    let ticket = successor_ticket;
    let id = successor_id;
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    let _ = crate::lifecycle::test_dispose_backend(
        &mut backend,
        &mut pending,
        &mut disposed,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    let ready = recover(&mut control, &ticket);
    assert_eq!(ready.disposed.identity(), id);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            assert_eq!(report.page_buffers, 3);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(ready);
    drop(control);
    finish(&mut retirement);
}
