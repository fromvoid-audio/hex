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
fn real_seek_waits_without_commit_busy_and_pause_cancel_then_playfrom_reads_exact_page() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let project = readiness::project_with_frames(12288);
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
    assert_eq!(output, [[0.25; 2]; 8]);
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
    assert_eq!(output, [[0.5; 2]; 8]);
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
    assert_eq!(output, [[0.5; 2]; 8]);
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
    assert_eq!(control.status.performance_frame.load(Acquire), 4008);
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
