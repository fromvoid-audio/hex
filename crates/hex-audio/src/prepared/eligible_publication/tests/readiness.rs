//! Actual sealed source/domain, native ring and callback render path.
use super::*;
#[test]
fn real_first_play_buffers_without_ack_clock_or_samples_then_installs_exact_worker_pages() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (project, _) = utility_fixture::project();
    let mut snapshot = project.snapshot();
    snapshot.tracks[0].devices.clear();
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::new();
    let definition = wav.definition(asset, 11);
    let mut owner = stream_fixture::owner_with_context(
        &project,
        definition.metadata.clone(),
        Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 7,
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
    let ticket = control
        .stage_eligible_audio_candidate(
            refused.owner,
            refused.domains,
            PublicationPolicy::InheritRuntime,
            0,
        )
        .ok()
        .unwrap();
    let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
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
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(output, [[0.25; 2]; 8]);
    assert_eq!(control.status.position.load(Acquire), 8);
    assert_eq!(control.status.performance_frame.load(Acquire), 8);
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        play
    );
    assert!(!control.status.stream_buffering.load(Acquire));
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
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(definition),
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
    assert_eq!(&output[..256], &[[0.25; 2]; 256]);
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
        .execute(hex_project::Command::AddTrack {
            name: "replacement".into(),
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
    let b_wav = stream_fixture::Wav::with_frames(8192);
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
            control.publication_pending.load(Acquire)
        );
        std::thread::yield_now();
    }
    // The observer facade uses the same real ready chunks and carries offsets
    // across them. EQ8 capture selection itself remains outside this Utility-
    // only eligible class; no fake EQ8 target is installed for this probe.
    let mut observed = [false; 512];
    let mut observed_output = [[0.0; 2]; 512];
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            let Some(ActivePublication::Eligible(active)) = c.active.as_mut() else {
                panic!("active")
            };
            let mut observe = |_identity, offset: usize, frames: &[Frame]| {
                assert!(offset + frames.len() <= observed.len());
                observed[offset..offset + frames.len()].fill(true);
            };
            let outcome = active.render_streamed(
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
    assert!(observed.into_iter().all(|seen| seen));
    let b_ticket = control
        .stage_eligible_audio_candidate(b, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
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
            control.publication_pending.load(Acquire)
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
    assert_eq!(control.status.position.load(Acquire), position);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(control.publication_pending.load(Acquire));
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
            control.publication_pending.load(Acquire)
        );
        std::thread::yield_now();
    }
    drop(ready_a);
    drop(control);
    finish(&mut retirement);
}
