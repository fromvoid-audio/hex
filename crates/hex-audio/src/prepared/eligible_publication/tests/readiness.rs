//! Actual sealed source/domain, native ring and callback render path.
use super::*;
#[test]
fn real_first_play_buffers_without_ack_clock_or_samples_then_installs_exact_worker_pages() {
    first_play(false, false, false, false);
}
#[test]
fn grouped_closed_constructor_buffers_and_renders_verified_worker_pages() {
    first_play(true, false, false, false);
}
#[test]
fn track_limiter_closed_stream_buffers_then_renders_without_callback_allocations() {
    first_play(false, false, true, false);
}
#[test]
fn group_limiter_closed_stream_buffers_then_renders_without_callback_allocations() {
    first_play(true, false, true, false);
}
#[test]
fn fed_track_limiter_buffers_and_handoffs_without_callback_allocations() {
    first_play(false, false, true, true);
}
#[test]
fn fed_group_limiter_buffers_and_handoffs_without_callback_allocations() {
    first_play(true, false, true, true);
}
#[test]
fn priority_stop_revokes_buffered_play_before_installation() {
    assert!(
        hex_engine::audio_storage_constructor_layout_supported(),
        "this regression requires the real sealed-source constructor"
    );
    first_play(false, true, false, false);
}
fn first_play(grouped: bool, stop_pending: bool, limiter: bool, feed: bool) {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (project, _) = utility_fixture::project();
    let project = if limiter {
        project_with_frames(1024)
    } else {
        project
    };
    let mut snapshot = project.snapshot();
    snapshot.tracks[0].devices.clear();
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
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
        if limiter {
            let state = hex_engine::default_native_device_state(
                hex_engine::DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap();
            editor
                .execute(hex_project::Command::InsertDevice {
                    track: group,
                    before: None,
                    state,
                })
                .unwrap();
        }
        editor.project().clone()
    } else if limiter {
        let mut editor = hex_project::Editor::new(project, 8).unwrap();
        let track = editor.project().tracks()[0].id();
        let state = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
            "hex.limiter",
        ))
        .unwrap();
        editor
            .execute(hex_project::Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap();
        if feed {
            let hex_project::Created::Track(receiver) = editor
                .execute(hex_project::Command::InsertTrackWithDevice {
                    name: "receiver".into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state().unwrap(),
                })
                .unwrap()
            else {
                panic!("receiver")
            };
            editor
                .execute(hex_project::Command::RemoveInstrument { track: receiver })
                .unwrap();
            editor
                .execute(hex_project::Command::SetAudioFeed {
                    source: hex_project::RoutingOwner::Track(track),
                    destination: receiver,
                    gain: 0.5,
                })
                .unwrap();
        }
        editor.project().clone()
    } else {
        project
    };
    let project = if grouped && feed {
        let mut editor = hex_project::Editor::new(project, 8).unwrap();
        let group = editor
            .project()
            .tracks()
            .iter()
            .find(|track| track.is_group())
            .unwrap()
            .id();
        let hex_project::Created::Track(receiver) = editor
            .execute(hex_project::Command::InsertTrackWithDevice {
                name: "receiver".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        editor
            .execute(hex_project::Command::RemoveInstrument { track: receiver })
            .unwrap();
        editor
            .execute(hex_project::Command::SetAudioFeed {
                source: hex_project::RoutingOwner::Track(group),
                destination: receiver,
                gain: 0.5,
            })
            .unwrap();
        editor.project().clone()
    } else {
        project
    };
    let asset = project.assets().next().unwrap().id();
    let wav = if limiter {
        stream_fixture::Wav::with_frames(1024)
    } else {
        stream_fixture::Wav::new()
    };
    let definition = wav.definition(asset, 11);
    let make_owner = if limiter {
        stream_fixture::owner_with_effects
    } else {
        stream_fixture::owner_with_context
    };
    let mut owner = make_owner(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 7,
        }),
    );
    let mut id = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let source_metadata = definition.metadata.clone();
    let domain = control
        .begin_audio_candidate_domain_preparation(
            hex_stream::StreamConfig::with_pool_pages(4),
            vec![definition],
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .unwrap();
    assert_eq!(observed_usage(&control).occupied_domains, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match control
            .poll_audio_candidate_domain_preparation(domain)
            .unwrap()
        {
            crate::AudioCandidateDomainPreparationProgress::Ready => break,
            crate::AudioCandidateDomainPreparationProgress::Verifying => {}
            other => panic!("unexpected source preparation: {other:?}"),
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(control.audio_candidate_domain_covers_source(domain, &source_metadata));
    assert!(
        !control.audio_candidate_domain_covers_source(domain, &wav.definition(asset, 12).metadata)
    );
    assert_eq!(
        control.poll_audio_candidate_domain_preparation(domain),
        Err("candidate publication queue is full")
    );
    assert_eq!(observed_usage(&control).occupied_domains, 1);
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block()); // only exact loan transfer
    let domains = vec![domain];
    let original_domains = (domains.as_ptr(), domains.capacity());
    let refused = control
        .stage_eligible_audio_candidate(owner, domains, PublicationPolicy::InheritRuntime, 1)
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), id);
    assert_eq!(
        (refused.domains.as_ptr(), refused.domains.capacity()),
        original_domains
    );
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    // Caller context cannot relabel this actual prepared owner's document.
    let mut ticket = control
        .stage_eligible_audio_candidate(
            refused.owner,
            refused.domains,
            PublicationPolicy::InheritRuntime,
            0,
        )
        .ok()
        .unwrap();
    let mut play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let mut output = [[1.0; 2]; 8];
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        })
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
    assert_eq!(output, [[0.0; 2]; 8]);
    assert!(control.status.stream_buffering.load(Acquire));
    assert_eq!(control.status.position.load(Acquire), 0);
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_eq!(control.status.consumed_command.load(Acquire), play);
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        0
    );
    if stop_pending {
        control.stop();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while control.status.applied_revision.load(Acquire) == 0 {
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            })
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
        if stop_pending {
            assert_eq!(output, [[0.0; 2]; 8]);
            assert!(!control.status.playing.load(Acquire));
            assert_eq!(control.status.position.load(Acquire), 0);
            assert_eq!(control.status.performance_frame.load(Acquire), 0);
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    if stop_pending {
        for _ in 0..3 {
            let measured = allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|callback, _| {
                    callback.begin_block();
                    callback.render(&mut output).unwrap();
                })
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
            assert_eq!(output, [[0.0; 2]; 8]);
            assert!(!control.status.playing.load(Acquire));
            assert_eq!(control.status.position.load(Acquire), 0);
            assert_eq!(control.status.performance_frame.load(Acquire), 0);
        }
        play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        while control.status.stream_applied_command_prefix.load(Acquire) < play {
            let measured = allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|callback, _| {
                    callback.begin_block();
                    callback.render(&mut output).unwrap();
                })
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
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    if limiter {
        while output.iter().all(|frame| frame[0] == 0.0) {
            let measured = allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|callback, _| {
                    callback.begin_block();
                    callback.render(&mut output).unwrap();
                });
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
            assert!(control.status.position.load(Acquire) < 1024);
        }
        assert!(
            output
                .iter()
                .all(|frame| frame[0] >= 0.0 && frame[0] <= if feed { 0.375 } else { 0.25 })
        );
    } else {
        assert_eq!(output, [[if grouped { 0.125 } else { 0.25 }; 2]; 8]);
        assert_eq!(control.status.position.load(Acquire), 8);
        assert_eq!(control.status.performance_frame.load(Acquire), 8);
    }
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        play
    );
    assert!(!control.status.stream_buffering.load(Acquire));
    if grouped || limiter {
        let successor = make_owner(
            &project,
            wav.definition(asset, 11).metadata,
            Some(hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 8,
            }),
        );
        let next_id = successor.identity();
        let next = control
            .stage_eligible_audio_candidate(
                successor,
                vec![domain],
                PublicationPolicy::InheritRuntime,
                0,
            )
            .ok()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while control.status.applied_revision.load(Acquire) != 2 {
            let measured = allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|callback, _| {
                    callback.begin_block();
                    callback.render(&mut output).unwrap();
                });
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
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        if limiter {
            assert!(
                output.iter().any(|frame| frame[0] > 0.0),
                "handoff retains Limiter history"
            );
        }
        control.reclaim();
        let retired = recover(&mut control, &ticket);
        assert_eq!(retired.disposed.identity(), id);
        drop(retired);
        ticket = next;
        id = next_id;
        let seek = control
            .command_with_ticket(PlaybackCommand::PlayFrom(0))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while control.status.stream_applied_command_prefix.load(Acquire) < seek {
            let measured = allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|callback, _| {
                    callback.begin_block();
                    callback.render(&mut output).unwrap();
                });
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
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        if !limiter {
            assert_eq!(output, [[0.125; 2]; 8]);
        }
    }
    let mut retirement = std::thread::spawn(move || {
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
        retirement
    })
    .join()
    .unwrap();
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    let ready = recover(&mut control, &ticket);
    assert_eq!(ready.disposed.identity(), id);
    assert_eq!(
        ready.return_kind,
        crate::AudioCandidateReturnKind::FinalActive
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
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
}

pub(super) fn project_with_frames(frames: u64) -> hex_project::Project {
    let (project, _) = utility_fixture::project();
    let mut snapshot = project.snapshot();
    snapshot.tracks[0].devices.clear();
    snapshot.assets[0].source_frames = frames;
    snapshot.assets[0].playback_frames = frames;
    let clips = match &mut snapshot.tracks[0].content {
        hex_project::TrackContentSnapshot::Audio(clips) => clips,
        hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
        _ => panic!("audio fixture"),
    };
    clips[0].duration = frames;
    clips[0].source_length = frames;
    clips[0].fade = hex_project::ClipFade::neutral(frames).unwrap();
    hex_project::Project::from_snapshot(snapshot).unwrap()
}
#[cfg(feature = "stream-test-support")]
#[test]
fn later_missing_chunk_keeps_committed_pcm_and_clocks_then_pause_passes_pending_play() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let project = project_with_frames(8192);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(8192);
    let definition = wav.definition(asset, 11);
    // A fast worker may otherwise finish page two within the same callback.
    // Keep its absence explicit under allocation-probe + stream-test-support.
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(hex_stream::PageKey {
        source: definition.metadata.key(),
        page_index: 1,
    });
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 9,
        }),
    );
    let id = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let mut runtime = hex_stream::StreamRuntime::new_with_page_gate(
        hex_stream::StreamConfig::with_pool_pages(4),
        vec![definition],
        gate,
    )
    .unwrap();
    runtime
        .wait_until_ready(std::time::Duration::from_secs(2))
        .unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            runtime.split().ok().unwrap(),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = vec![[1.0; 2]; 8192];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    // First submission requests only the exact first chunk, never a speculative
    // whole-block prefetch. The next uncached page must miss after real commits.
    while control.status.applied_revision.load(Acquire) == 0 {
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            })
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
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(control.status.stream_buffering.load(Acquire));
    assert_eq!(control.status.stream_committed_frames.load(Acquire), 4096);
    assert_eq!(control.status.position.load(Acquire), 4096);
    assert_eq!(control.status.performance_frame.load(Acquire), 4096);
    assert_eq!(&output[..4096], &vec![[0.25; 2]; 4096]);
    assert_eq!(&output[4096..], &vec![[0.0; 2]; 4096]);
    entered
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    // Play is deferred even on an installed runtime; Pause must pass it and run
    // the actual zero-page reset before any more page/clock/DSP commit.
    let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let pause = control.command_with_ticket(PlaybackCommand::Pause).unwrap();
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output[..32]).unwrap();
        })
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
    assert!(pause > play);
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        pause
    );
    assert_eq!(control.status.consumed_command.load(Acquire), pause);
    assert_eq!(control.status.position.load(Acquire), 4096);
    assert_eq!(control.status.performance_frame.load(Acquire), 4096);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(&output[..32], &[[0.0; 2]; 32]);
    // Full admitted command ring cannot starve the independent priority Stop.
    for _ in 0..COMMANDS {
        control.command(PlaybackCommand::Play).unwrap();
    }
    control.stop();
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output[..32]).unwrap();
        })
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
    assert_eq!(control.status.position.load(Acquire), 0);
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    assert!(!control.status.playing.load(Acquire));
    release.try_send(()).unwrap();
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
fn two_source_small_pool_refusal_after_commit_preserves_pcm_clock_and_applied_prefix() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let mut edit = hex_project::Editor::new(project_with_frames(128), 16).unwrap();
    let a = edit.project().assets().next().unwrap().id();
    let track = edit.project().tracks()[0].id();
    let hex_project::Created::Asset(b) = edit
        .execute(hex_project::Command::AddAsset {
            path: "later.wav".into(),
            sample_rate: 48000,
            frames: 8192,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    edit.execute(hex_project::Command::PlaceClip {
        track,
        asset: b,
        start: 128,
        duration: 512,
        source_offset: 3968,
        source_length: 512,
        looping: false,
    })
    .unwrap();
    let a_wav = stream_fixture::Wav::with_frames(128);
    let b_wav = stream_fixture::Wav::with_frames(8192);
    let a_def = a_wav.definition(a, 11);
    let b_def = b_wav.definition(b, 11);
    let mut owner = stream_fixture::owner_sources(
        edit.project(),
        vec![a_def.metadata.clone(), b_def.metadata.clone()],
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 12,
        }),
    );
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(2).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![a_def, b_def], 3),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
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
    let mut output = [[1.0; 2]; 512];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while control.status.applied_revision.load(Acquire) == 0 {
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
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
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    // The first chunk reads a/page0 and b/page0. The next needs b/page1;
    // those three distinct protections cannot fit this actual two-page cache.
    assert_eq!(&output[..256], &[[0.25; 2]; 256]);
    assert_eq!(&output[256..], &[[0.0; 2]; 256]);
    assert!(control.status.stream_buffering.load(Acquire));
    assert!(control.status.stream_readiness_refused.load(Acquire));
    assert_eq!(control.status.stream_committed_frames.load(Acquire), 256);
    assert_eq!(control.status.position.load(Acquire), 256);
    assert_eq!(control.status.performance_frame.load(Acquire), 256);
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        play
    );
    assert!(!control.status.render_fault.load(Acquire));
    // Pause invalidates obsolete protection, then the exact remaining window can
    // make progress without widening the cache or rolling back the old chunk.
    control.command(PlaybackCommand::Pause).unwrap();
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        c.render(&mut output[..256]).unwrap();
    });
    control.command(PlaybackCommand::Play).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while control.status.position.load(Acquire) == 256 {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output[..256]).unwrap();
        });
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(control.status.position.load(Acquire), 512);
    // A ready streamed Play from the paused frame ramps in over 128 frames,
    // like the resident path (#4140).
    for (i, frame) in output[..256].iter().enumerate() {
        let gain = (i + 1).min(128) as f32 / 128.0;
        assert_eq!(*frame, [0.25 * gain; 2], "frame {i}");
    }
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

#[test]
fn ready_candidate_group_retry_releases_speculation_for_old_page_and_remains_cancellable() {
    ready_candidate_group_retry_case(false);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn eligible_candidate_retries_smaller_window_at_two_page_boundary() {
    crate::candidate_window_probe::begin();
    ready_candidate_group_retry_case(true);
    let (failed, next, _) = crate::candidate_window_probe::finish().unwrap();
    assert_eq!((failed, next), (256, 128));
}

fn ready_candidate_group_retry_case(cross_page: bool) {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let mut snapshot = project_with_frames(8192).snapshot();
    let (utility, _) = utility_fixture::project();
    snapshot.tracks[0].devices = utility.snapshot().tracks[0].devices.clone();
    snapshot.next_id = snapshot.next_id.max(utility.snapshot().next_id);
    let a_project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let a_asset = a_project.assets().next().unwrap().id();
    let a_track = a_project.tracks()[0].id();
    let mut edit = hex_project::Editor::new(a_project.clone(), 16).unwrap();
    edit.execute(hex_project::Command::RemoveTrack(a_track))
        .unwrap();
    let hex_project::Created::Track(b_track) = edit
        .execute(hex_project::Command::InsertTrackWithDevice {
            name: "replacement".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track")
    };
    edit.execute(hex_project::Command::RemoveInstrument { track: b_track })
        .unwrap();
    let hex_project::Created::Asset(b_asset) = edit
        .execute(hex_project::Command::AddAsset {
            path: "replacement.wav".into(),
            sample_rate: 48000,
            frames: 8192,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    edit.execute(hex_project::Command::PlaceClip {
        track: b_track,
        asset: b_asset,
        start: 0,
        duration: 8192,
        source_offset: 0,
        source_length: 8192,
        looping: false,
    })
    .unwrap();
    let a_wav = stream_fixture::Wav::with_frames(8192);
    let b_wav = if cross_page {
        stream_fixture::Wav::with_samples(8192, |_| 16_384)
    } else {
        stream_fixture::Wav::with_frames(8192)
    };
    let a_def = a_wav.definition(a_asset, 11);
    let b_def = b_wav.definition(b_asset, 11);
    let context = Some(hex_engine::AudioSourceContext {
        project_generation: 0,
        editor_revision: 20,
    });
    let mut a = stream_fixture::owner_sources(&a_project, vec![a_def.metadata.clone()], context);
    let b = stream_fixture::owner_sources(edit.project(), vec![b_def.metadata.clone()], context);
    let b_id = b.identity();
    let limits = reservation(&mut a);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(2).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![a_def, b_def], 3),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|error| panic!("domain registration refused: {}", error.reason));
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let a_ticket = control
        .stage_eligible_audio_candidate(a, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 256];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while control.status.applied_revision.load(Acquire) == 0 {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        });
        assert!(
            std::time::Instant::now() < deadline,
            "position={} refused={} rejected={:?} pending={}",
            control.status.position.load(Acquire),
            control.status.stream_readiness_refused.load(Acquire),
            control.status.rejected_revision(),
            control.status.publication_pending.load(Acquire)
        );
        std::thread::yield_now();
    }
    // The observer facade uses the same real ready chunks and carries offsets
    // across them. EQ8 capture selection itself remains outside this Utility-
    // only eligible class; no fake EQ8 target is installed for this probe.
    let mut observed = [[false; 512]; 2];
    let mut observed_output = [[0.0; 2]; 512];
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            let Some(ActivePublication::Eligible(active)) = c.active.as_mut() else {
                panic!("active")
            };
            let mut observe = |_identity, phase, offset: usize, frames: &[Frame]| {
                let index = match phase {
                    hex_engine::NodeSignalPhase::Input => 0,
                    hex_engine::NodeSignalPhase::Output => 1,
                };
                assert!(offset + frames.len() <= observed[index].len());
                observed[index][offset..offset + frames.len()].fill(true);
            };
            let outcome = active.stream_view().render_streamed(
                &mut c.domain_pages,
                false,
                c.stream_prefix,
                &mut observed_output,
                Some(&mut observe),
            );
            assert!(matches!(
                outcome.progress,
                crate::candidate_custody::StreamProgress::Complete
            ));
            assert!(outcome.fault.is_none());
            c.publish_transport();
        })
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
    assert!(observed.into_iter().flatten().all(|seen| seen));
    if cross_page {
        control.command(PlaybackCommand::Seek(3840)).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while control.status.position.load(Acquire) != 3968 {
            let measured = allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|c, _| {
                    c.begin_block();
                    c.render(&mut output[..128]).unwrap();
                })
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
            assert!(
                std::time::Instant::now() < deadline,
                "seek to candidate boundary"
            );
            std::thread::yield_now();
        }
    }
    let b_ticket = control
        .stage_eligible_audio_candidate(b, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    if cross_page {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while control.status.applied_revision.load(Acquire) != 2 {
            let measured = allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|c, _| {
                    c.begin_block();
                    c.render(&mut output).unwrap();
                });
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
            assert!(control.status.rejected_revision().is_none());
            assert!(!control.status.stream_buffering.load(Acquire));
            assert!(
                std::time::Instant::now() < deadline,
                "eligible successor did not install"
            );
            std::thread::yield_now();
        }
        assert!(
            output
                .iter()
                .all(|frame| frame[0].is_finite() && frame[0] > 0.0 && frame[0] == frame[1])
        );
        // The predecessor's retained Utility tail overlaps the louder successor.
        // Require audible successor contribution without discarding that tail.
        assert!(output.iter().any(|frame| frame[0] > 0.30));
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
        let ready_a = recover(&mut control, &a_ticket);
        let ready_b = recover(&mut control, &b_ticket);
        drop(ready_a);
        drop(ready_b);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            control.reclaim();
            if let Some(report) = control.recover_audio_candidate_domain(domain) {
                assert!(report.worker_joined && report.accounting_complete);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "eligible domain did not retire"
            );
            std::thread::yield_now();
        }
        drop(control);
        finish(&mut retirement);
        return;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    // Real Utility tail movement cannot fit the deliberately full destination
    // runtime allowance. The old runtime must still reach its next actual page.
    while control.status.position.load(Acquire) < 4352 {
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.test_eligible_transfer_allowance =
                    Some(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
                c.begin_block();
                c.render(&mut output).unwrap();
            })
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
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        assert!(
            std::time::Instant::now() < deadline,
            "position={} refused={} rejected={:?} pending={}",
            control.status.position.load(Acquire),
            control.status.stream_readiness_refused.load(Acquire),
            control.status.rejected_revision(),
            control.status.publication_pending.load(Acquire)
        );
        std::thread::yield_now();
    }
    // A real Pause passes the deferred Play/candidate and invokes the original
    // predecessor action (including its metronome reset), without clock work.
    let position = control.status.position.load(Acquire);
    let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let pause = control.command_with_ticket(PlaybackCommand::Pause).unwrap();
    data.owner_mut().unwrap().run(|c, _| {
        c.test_eligible_transfer_allowance =
            Some(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
        c.begin_block();
        c.render(&mut output).unwrap();
    });
    assert!(pause > play);
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        pause
    );
    assert_eq!(control.status.consumed_command.load(Acquire), pause);
    // The Pause applies at the end of its 128-frame fade (#4151).
    assert_eq!(control.status.position.load(Acquire), position + 128);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(control.status.publication_pending.load(Acquire));
    assert_eq!(
        control.cancel_audio_candidate(&b_ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        c.render(&mut output).unwrap();
    });
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    let ready_b = recover(&mut control, &b_ticket);
    assert_eq!(ready_b.disposed.identity(), b_id);
    assert_eq!(
        ready_b.return_kind,
        crate::AudioCandidateReturnKind::NeverInstalled
    );
    drop(ready_b);
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
    let ready_a = recover(&mut control, &a_ticket);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "position={} refused={} rejected={:?} pending={}",
            control.status.position.load(Acquire),
            control.status.stream_readiness_refused.load(Acquire),
            control.status.rejected_revision(),
            control.status.publication_pending.load(Acquire)
        );
        std::thread::yield_now();
    }
    drop(ready_a);
    drop(control);
    finish(&mut retirement);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn stalled_current_window_does_not_prevent_candidate_cancellation() {
    let _serial = serial();
    assert!(hex_engine::audio_storage_constructor_layout_supported());
    let project = project_with_frames(8192);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(8192);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(hex_stream::PageKey {
        source: metadata.key(),
        page_index: 1,
    });
    let context = Some(hex_engine::AudioSourceContext {
        project_generation: 0,
        editor_revision: 7,
    });
    let mut a = stream_fixture::owner_with_context(&project, metadata.clone(), context);
    let b = stream_fixture::owner_with_context(&project, metadata, context);
    let b_id = b.identity();
    let limits = reservation(&mut a);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(2).unwrap();
    let mut runtime = hex_stream::StreamRuntime::new_with_page_gate(
        hex_stream::StreamConfig::with_pool_pages(4),
        vec![definition],
        gate,
    )
    .unwrap();
    runtime
        .wait_until_ready(std::time::Duration::from_secs(2))
        .unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            runtime.split().ok().unwrap(),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let a_ticket = control
        .stage_eligible_audio_candidate(a, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 256];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut gate_entered = false;
    loop {
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
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
        gate_entered |= entered.try_recv().is_ok();
        if gate_entered
            && control.status.position.load(Acquire) == 4096
            && control.status.stream_buffering.load(Acquire)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "current window did not stall"
        );
        std::thread::yield_now();
    }
    let b_ticket = control
        .stage_eligible_audio_candidate(b, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        c.render(&mut output).unwrap();
    });
    assert!(control.status.publication_pending.load(Acquire));
    assert_eq!(
        control.cancel_audio_candidate(&b_ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        c.render(&mut output).unwrap();
    });
    let ready_b = recover(&mut control, &b_ticket);
    assert_eq!(ready_b.disposed.identity(), b_id);
    assert_eq!(
        ready_b.return_kind,
        crate::AudioCandidateReturnKind::NeverInstalled
    );
    assert!(control.status.stream_buffering.load(Acquire));
    release.send(()).unwrap();
    drop(ready_b);
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
    let ready_a = recover(&mut control, &a_ticket);
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(ready_a);
    drop(control);
    finish(&mut retirement);
}

#[test]
fn changed_source_during_playback_reports_fault_and_preserves_committed_audio() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let project = project_with_frames(8192);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(8192);
    let definition = wav.definition(asset, 11);
    let source_path = definition.path.clone();
    let original = std::fs::read(&source_path).unwrap();
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 9,
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
    let mut output = [[0.0; 2]; 256];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while control.status.applied_revision.load(Acquire) == 0 {
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
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
        assert!(!control.status.stream_source_changed.load(Acquire));
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output, [[0.25; 2]; 256]);
    std::fs::write(&source_path, b"changed source bytes").unwrap();
    while control.status.position.load(Acquire) < 4096 {
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
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
        assert_eq!(output, [[0.25; 2]; 256]);
        assert!(std::time::Instant::now() < deadline);
    }
    while !control.status.stream_readiness_refused.load(Acquire) {
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
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
        assert_eq!(output, [[0.0; 2]; 256]);
        assert_eq!(control.status.position.load(Acquire), 4096);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let reported = control.status.stream_source_changed.load(Acquire);
    assert!(!control.status.take_stream_read_failure());
    control.stop();
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
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
    assert_eq!(control.status.position.load(Acquire), 0);
    assert!(!control.status.playing.load(Acquire));
    let retained_after_stop = control.status.stream_source_changed.load(Acquire);
    // Cached audio can play after Stop, but it cannot reverify the changed file.
    control.command(PlaybackCommand::Play).unwrap();
    while control.status.position.load(Acquire) == 0 {
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut output).unwrap();
            })
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
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output, [[0.25; 2]; 256]);
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        c.render(&mut output).unwrap();
    });
    assert!(control.status.stream_source_changed.load(Acquire));
    control.stop();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    std::fs::write(&source_path, &original).unwrap();
    control.status.stream_read_failed.store(true, Release);
    let replacement_definition = wav.definition(asset, 11);
    let replacement = stream_fixture::owner_with_context(
        &project,
        replacement_definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 10,
        }),
    );
    let replacement_domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(replacement_definition),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .unwrap_or_else(|_| panic!("replacement domain"));
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let replacement_ticket = control
        .stage_eligible_audio_candidate(
            replacement,
            vec![replacement_domain],
            PublicationPolicy::InheritRuntime,
            0,
        )
        .unwrap_or_else(|refusal| panic!("replacement refused: {}", refusal.reason));
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
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
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    assert!(!control.status.stream_source_changed.load(Acquire));
    assert!(!control.status.take_stream_read_failure());
    let old = recover(&mut control, &ticket);
    assert_eq!(old.return_kind, crate::AudioCandidateReturnKind::Replaced);
    drop(old);
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
    let returned = recover(&mut control, &replacement_ticket);
    let mut recovered_old = false;
    let mut recovered_new = false;
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            recovered_old = true;
        }
        if let Some(report) = control.recover_audio_candidate_domain(replacement_domain) {
            assert!(report.worker_joined && report.accounting_complete);
            recovered_new = true;
        }
        if recovered_old && recovered_new {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(returned);
    drop(control);
    finish(&mut retirement);
    assert!(reported, "changed source must be reported");
    assert!(retained_after_stop, "Stop must not erase the warning");
    let (new_control, _) = crate::prepared::channel(48_000).unwrap();
    assert!(!new_control.status.stream_source_changed.load(Acquire));
}

#[test]
fn changed_source_after_catalog_admission_reports_recovery() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (project, _) = utility_fixture::project();
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::new();
    let definition = wav.definition(asset, 11);
    let source_path = definition.path.clone();
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 7,
        }),
    );
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let domain = control
        .begin_audio_candidate_domain_preparation(
            hex_stream::StreamConfig::with_pool_pages(4),
            vec![definition],
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match control
            .poll_audio_candidate_domain_preparation(domain)
            .unwrap()
        {
            crate::AudioCandidateDomainPreparationProgress::Ready => break,
            crate::AudioCandidateDomainPreparationProgress::Verifying => {}
            other => panic!("unexpected source preparation: {other:?}"),
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    std::fs::write(source_path, b"changed source bytes").unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[1.0; 2]; 8];
    while control.status.rejected_revision().is_none() {
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            });
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
        assert!(
            std::time::Instant::now() < deadline,
            "changed source must be refused"
        );
        std::thread::yield_now();
    }
    let rejection = control.status.rejected_revision().unwrap();
    assert_eq!(output, [[0.0; 2]; 8]);
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    let returned = recover(&mut control, &ticket);
    assert_eq!(
        returned.return_kind,
        crate::AudioCandidateReturnKind::NeverInstalled
    );
    drop(returned);
    assert!(control.retire_audio_candidate_domain(domain));
    loop {
        control.reclaim();
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
    assert_eq!(
        rejection.1.message(),
        "original media changed · reopen or relink it"
    );
}

#[test]
fn changed_file_pending_domain_refuses_without_loan_or_ack_and_keeps_pool_until_report() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let (project, _) = utility_fixture::project();
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::new();
    let definition = wav.definition(asset, 11);
    std::fs::write(&definition.path, b"changed source bytes").unwrap();
    let token = control
        .begin_audio_candidate_domain_preparation(
            hex_stream::StreamConfig::with_pool_pages(4),
            vec![definition],
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap();
    let deadline = std::time::Instant::now() + LIFECYCLE_BOUND;
    loop {
        match control
            .poll_audio_candidate_domain_preparation(token)
            .unwrap()
        {
            crate::AudioCandidateDomainPreparationProgress::Refused(
                crate::AudioCandidateDomainPreparationFailure::Verification(_),
            ) => break,
            crate::AudioCandidateDomainPreparationProgress::Verifying => {}
            other => panic!("unexpected preparation: {other:?}"),
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    // A normal verification refusal is accepted cleanup, not a poisoned Root
    // or a fabricated page/installation success. The real pool stays charged.
    assert!(control.candidate_root.eligible_claims_open());
    assert_eq!(observed_usage(&control).occupied_domains, 1);
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
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    loop {
        if let Some(report) = control.recover_audio_candidate_domain(token) {
            assert!(report.worker_joined && report.accounting_complete);
            assert_eq!(report.page_buffers, 4);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    drop(owner);
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}
