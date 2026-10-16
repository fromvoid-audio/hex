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
                    source_start: 4096,
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
    let wav = stream_fixture::Wav::with_samples(8192, |f| if f < 4096 { 8192 } else { 16384 });
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
