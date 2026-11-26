use super::*;

#[test]
fn session_audio_matches_direct_frame_oracle_over_irregular_blocks_and_ends_silently() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [index as f32 * 0.013 - 0.17, 0.31 - index as f32 * 0.007])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = audio_project(5, 18, 4);
    let media = audio_media(&project, &audio, &other);
    let prepared = Arc::new(
        PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let clip = prepared.session().audio_clips()[0].id;
    let mut playback = SessionPlayback::new(prepared).unwrap();
    let track = playback.routes[0].id;
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut rendered = Vec::new();
    for width in [1, 5, 2, 7, 4, 9, 3] {
        let mut block = vec![[0.0; 2]; width];
        playback.render(&mut block).unwrap();
        rendered.extend(block);
    }
    for (elapsed, actual) in rendered.iter().enumerate() {
        let expected = if elapsed < 13 {
            source[5 + elapsed]
        } else {
            [0.0; 2]
        };
        assert_eq!(*actual, expected, "source-relative frame {elapsed}");
    }
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
}

#[test]
fn session_audio_seek_pause_stop_scene_stop_and_return_are_explicit_and_keep_midi_routes() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [index as f32 * 0.011 + 0.1, 0.8 - index as f32 * 0.009])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = mixed_audio_midi_project(5, 18);
    let media = audio_media(&project, &audio, &other);
    let revision = Arc::new(
        PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let audio_clip = revision.session().audio_clips()[0].id;
    let midi_clip = revision.session().midi_clips()[0].id;
    let audio_track = revision.session().audio_clips()[0].track;
    let midi_track = revision.session().midi_clips()[0].track;
    let scene = revision.session().scenes()[0].id;
    let stop_scene = revision.session().scenes()[1].id;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Session { clip: audio_clip })
    );
    assert_eq!(
        playback.owner(midi_track),
        Some(SessionPlaybackOwner::Session { clip: midi_clip })
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 3];
    playback.render(&mut first).unwrap();
    assert_eq!(first[0], source[5]);
    playback.apply(SessionPlaybackCommand::Pause).unwrap();
    let position = playback.performance_frame();
    let mut paused = [[1.0; 2]; 2];
    playback.render(&mut paused).unwrap();
    assert_eq!(paused, [[0.0; 2]; 2]);
    assert_eq!(playback.performance_frame(), position);
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut resumed = [[0.0; 2]; 1];
    playback.render(&mut resumed).unwrap();
    assert_eq!(resumed[0], source[8]);
    playback.apply(SessionPlaybackCommand::Seek(100)).unwrap();
    let mut seeked = [[0.0; 2]; 1];
    playback.render(&mut seeked).unwrap();
    assert_eq!(seeked[0], source[5]);
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(playback.performance_frame(), 0);
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Session { clip: audio_clip })
    );
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut restarted = [[0.0; 2]; 1];
    playback.render(&mut restarted).unwrap();
    assert_eq!(restarted[0], source[5]);
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene: stop_scene,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Silence)
    );
    assert_eq!(
        playback.owner(midi_track),
        Some(SessionPlaybackOwner::Session { clip: midi_clip })
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert_eq!(
        playback.owner(midi_track),
        Some(SessionPlaybackOwner::Arrangement)
    );
}

#[test]
fn session_audio_midi_and_arrangement_routes_mix_additively_against_independent_renders() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [0.01 * (index as f32 + 1.0), -0.006 * (index as f32 + 2.0)])
        .collect();
    let audio = crate::Audio::new(48_000, source).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = session_audio_midi_arrangement_mix_project();
    let media = audio_media(&project, &audio, &other);
    let render = |launch_audio: bool, launch_midi: bool| {
        let revision = Arc::new(
            PreparedRevision::prepare(
                &project,
                &media,
                Arc::new(hex_devices::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let audio_clip = revision.session().audio_clips()[0].id;
        let midi_clip = revision.session().midi_clips()[0].id;
        let mut playback = SessionPlayback::new(revision).unwrap();
        if launch_audio {
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: audio_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        if launch_midi {
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: midi_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 32];
        playback.render(&mut output).unwrap();
        output
    };
    let both = render(true, true);
    let audio_only = render(true, false);
    let midi_only = render(false, true);
    let arrangement_only = render(false, false);
    for index in 0..both.len() {
        for channel in 0..2 {
            let expected = audio_only[index][channel] + midi_only[index][channel]
                - arrangement_only[index][channel];
            assert!(
                (both[index][channel] - expected).abs() <= 1.0e-6,
                "frame {index} channel {channel}: actual {}, independent sum {expected}",
                both[index][channel]
            );
        }
    }
    assert!(both.iter().any(|frame| frame[0].abs() > 0.02));
}

#[test]
fn session_audio_quantization_cancel_and_tempo_handoff_keep_frame_domain() {
    let source_a: Vec<Frame> = (0..32)
        .map(|index| [index as f32 + 1.0, -(index as f32 + 2.0)])
        .collect();
    let source_b: Vec<Frame> = (0..32)
        .map(|index| [200.0 + index as f32, -400.0 - index as f32])
        .collect();
    let audio_a = crate::Audio::new(48_000, source_a.clone()).unwrap();
    let audio_b = crate::Audio::new(48_000, source_b.clone()).unwrap();
    let base = audio_project(5, 18, 4);
    let media = audio_media(&base, &audio_a, &audio_b);
    let mut playback = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(&base, &media, Arc::new(hex_devices::SineTable::new()), None)
            .unwrap(),
    ))
    .unwrap();
    let clip = playback.revision.session().audio_clips()[0].id;
    let track = playback.routes[0].id;
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut initial_frame = [[0.0; 2]; 1];
    playback.render(&mut initial_frame).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert!(playback.route_status(0).unwrap().pending.is_some());
    playback
        .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
        .unwrap();
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut prefix = [[0.0; 2]; 5];
    playback.render(&mut prefix).unwrap();

    let mut replacement_snapshot = base.snapshot();
    replacement_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(60_000_000).unwrap(),
    )];
    let SessionCellContentSnapshot::Audio(audio_clip) =
        &mut replacement_snapshot.session.cells[0].content
    else {
        panic!("audio cell expected")
    };
    audio_clip.asset = 5;
    audio_clip.source_start = 2;
    audio_clip.source_end = 20;
    let replacement = legacy_project_from_snapshot(replacement_snapshot).unwrap();
    let replacement_media = audio_media(&replacement, &audio_a, &audio_b);
    let candidate_revision = Arc::new(
        PreparedRevision::prepare(
            &replacement,
            &replacement_media,
            Arc::clone(playback.revision.session().sine_table()),
            Some(playback.revision.session()),
        )
        .unwrap(),
    );
    let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
    candidate.try_inherit_state_from(&mut playback).unwrap();
    let mut next = [[0.0; 2]; 1];
    candidate.render(&mut next).unwrap();
    assert_eq!(next[0], source_b[7]);
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert_eq!(candidate.performance_frame(), 7);

    let mut short_snapshot = replacement.snapshot();
    let SessionCellContentSnapshot::Audio(audio_clip) =
        &mut short_snapshot.session.cells[0].content
    else {
        panic!("audio cell expected")
    };
    audio_clip.source_start = 2;
    audio_clip.source_end = 4;
    let short = legacy_project_from_snapshot(short_snapshot).unwrap();
    let short_media = audio_media(&short, &audio_a, &audio_b);
    let short_revision = Arc::new(
        PreparedRevision::prepare(
            &short,
            &short_media,
            Arc::clone(candidate.revision.session().sine_table()),
            Some(candidate.revision.session()),
        )
        .unwrap(),
    );
    let mut shortened = SessionPlayback::new(short_revision).unwrap();
    shortened.try_inherit_state_from(&mut candidate).unwrap();
    let mut silent = [[1.0; 2]; 3];
    shortened.render(&mut silent).unwrap();
    assert_eq!(silent, [[0.0; 2]; 3]);
    assert_eq!(
        shortened.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
}

#[test]
fn session_audio_quantized_launch_begins_on_the_due_performance_frame() {
    let source: Vec<Frame> = (0..32)
        .map(|index| [index as f32 + 0.25, -((index as f32) + 0.5)])
        .collect();
    let audio = crate::Audio::new(48_000, source.clone()).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap();
    let project = audio_project(5, 18, 4);
    let media = audio_media(&project, &audio, &other);
    let revision = Arc::new(
        PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let clip = revision.session().audio_clips()[0].id;
    let track = revision.session().audio_clips()[0].track;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first_frame = [[0.0; 2]; 1];
    playback.render(&mut first_frame).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    assert_eq!(
        playback
            .route_status(0)
            .unwrap()
            .pending
            .unwrap()
            .performance_frame,
        96_000
    );

    let mut before_due = vec![[1.0; 2]; 96_000 - 1];
    playback.render(&mut before_due).unwrap();
    assert!(before_due.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(playback.performance_frame(), 96_000);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );

    let mut rendered = Vec::new();
    for width in [1, 3, 2, 5, 4, 7] {
        let mut block = vec![[0.0; 2]; width];
        playback.render(&mut block).unwrap();
        rendered.extend(block);
    }
    for (elapsed, actual) in rendered.iter().enumerate() {
        let expected = if elapsed < 13 {
            source[5 + elapsed]
        } else {
            [0.0; 2]
        };
        assert_eq!(*actual, expected, "due-relative frame {elapsed}");
    }
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
}

#[test]
fn quantized_scene_44100_launch_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(44100, false);
}

#[test]
fn quantized_scene_44100_cancel_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(44100, true);
}

#[test]
fn quantized_scene_48000_launch_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(48000, false);
}

#[test]
fn quantized_scene_48000_cancel_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(48000, true);
}

#[test]
fn quantized_scene_96000_launch_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(96000, false);
}

#[test]
fn quantized_scene_96000_cancel_matches_partitioned_pcm() {
    assert_quantized_scene_pcm(96000, true);
}

#[test]
fn prepared_session_audio_refuses_missing_rate_or_length_mismatched_pcm_and_reuses_shared_pcm() {
    let project = audio_project(1, 7, 4);
    let audio = crate::Audio::new(48_000, vec![[0.2, -0.3]; 32]).unwrap();
    let other = crate::Audio::new(48_000, vec![[0.4, -0.5]; 32]).unwrap();
    let media = audio_media(&project, &audio, &other);
    let first = PreparedSessionInventory::prepare_with_media(
        &project,
        &media,
        Arc::new(hex_devices::SineTable::new()),
        None,
    )
    .unwrap();
    let second = PreparedSessionInventory::prepare_with_media(
        &project,
        &media,
        Arc::clone(first.sine_table()),
        Some(&first),
    )
    .unwrap();
    assert!(
        second
            .audio_clip(first.audio_clips()[0].id)
            .unwrap()
            .source_matches(&first.audio_clips()[0])
    );
    let changed_pcm = crate::Audio::new(48_000, vec![[9.0, -9.0]; 32]).unwrap();
    let changed_media = audio_media(&project, &changed_pcm, &other);
    let changed = PreparedSessionInventory::prepare_with_media(
        &project,
        &changed_media,
        Arc::clone(first.sine_table()),
        Some(&first),
    )
    .unwrap();
    assert!(
        !changed
            .audio_clip(first.audio_clips()[0].id)
            .unwrap()
            .source_matches(&first.audio_clips()[0])
    );
    assert_eq!(
        changed
            .audio_clip(first.audio_clips()[0].id)
            .unwrap()
            .resident_audio()
            .unwrap()
            .frames()[0],
        [9.0, -9.0]
    );
    assert!(
        PreparedSessionInventory::prepare_with_media(
            &project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            None
        )
        .is_err()
    );
    let wrong_rate = crate::Audio::new(44_100, vec![[0.0; 2]; 32]).unwrap();
    let bad_media = audio_media(&project, &wrong_rate, &other);
    assert!(
        PreparedSessionInventory::prepare_with_media(
            &project,
            &bad_media,
            Arc::new(hex_devices::SineTable::new()),
            None
        )
        .is_err()
    );
    let short_pcm = crate::Audio::new(48_000, vec![[0.0; 2]; 31]).unwrap();
    let bad_length = audio_media(&project, &short_pcm, &other);
    assert!(
        PreparedSessionInventory::prepare_with_media(
            &project,
            &bad_length,
            Arc::new(hex_devices::SineTable::new()),
            None
        )
        .is_err()
    );
}

#[test]
fn used_or_accepted_session_runtime_cannot_be_admitted_as_a_fresh_candidate() {
    let table = Arc::new(hex_devices::SineTable::new());
    let project = project(0.5, 0, 8 << 32);
    let prepared = revision(&project, Arc::clone(&table));
    let mut predecessor = SessionPlayback::new(Arc::clone(&prepared)).unwrap();

    let mut applied = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    applied.apply(SessionPlaybackCommand::Pause).unwrap();
    assert_eq!(
        applied.try_inherit_state_from(&mut predecessor),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );

    let mut rendered = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    rendered.render(&mut []).unwrap();
    assert_eq!(
        rendered.try_inherit_state_from(&mut predecessor),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );

    let mut page_rendered = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    page_rendered
        .render_with_pages(&mut [], &mut ContinuityMissingPages)
        .unwrap();
    assert_eq!(
        page_rendered.try_inherit_state_from(&mut predecessor),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );

    let mut seeded = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    seeded.seed_legacy_transport(0, 0, false).unwrap();
    assert_eq!(
        seeded.try_inherit_state_from(&mut predecessor),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );
}

#[test]
fn accepted_session_candidate_refuses_second_handoff_before_either_runtime_changes() {
    let table = Arc::new(hex_devices::SineTable::new());
    let project = project(0.5, 0, 8 << 32);
    let prepared = revision(&project, Arc::clone(&table));
    let clip = prepared.session().midi_clips()[0].id;

    let mut first_predecessor = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    first_predecessor
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    first_predecessor
        .apply(SessionPlaybackCommand::Play)
        .unwrap();
    let mut prefix = [[0.0; 2]; 64];
    first_predecessor.render(&mut prefix).unwrap();
    let mut accepted = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    accepted
        .try_inherit_state_from(&mut first_predecessor)
        .unwrap();

    let mut previous = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut previous_twin = SessionPlayback::new(prepared).unwrap();
    for playback in [&mut previous, &mut previous_twin] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut prefix).unwrap();
    }

    let accepted_frame = accepted.performance_frame;
    let accepted_routes = accepted.routes.clone();
    let accepted_release_signature: Vec<_> = accepted
        .releases
        .iter()
        .map(|entry| (entry.track_id, entry.order, entry.voice.end_frame()))
        .collect();
    let accepted_generation = accepted.route_state_generation;
    let previous_frame = previous.performance_frame;
    let previous_arrangement_frame = previous.arrangement_frame;
    let previous_routes = previous.routes.clone();
    let previous_release_signature: Vec<_> = previous
        .releases
        .iter()
        .map(|entry| (entry.track_id, entry.order, entry.voice.end_frame()))
        .collect();
    let previous_generation = previous.route_state_generation;
    assert_eq!(
        accepted.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );
    assert_eq!(accepted.performance_frame, accepted_frame);
    assert_eq!(accepted.routes, accepted_routes);
    assert_eq!(
        accepted
            .releases
            .iter()
            .map(|entry| (entry.track_id, entry.order, entry.voice.end_frame()))
            .collect::<Vec<_>>(),
        accepted_release_signature
    );
    assert_eq!(accepted.route_state_generation, accepted_generation);
    assert_eq!(previous.performance_frame, previous_frame);
    assert_eq!(previous.arrangement_frame, previous_arrangement_frame);
    assert_eq!(previous.routes, previous_routes);
    assert_eq!(
        previous
            .releases
            .iter()
            .map(|entry| (entry.track_id, entry.order, entry.voice.end_frame()))
            .collect::<Vec<_>>(),
        previous_release_signature
    );
    assert_eq!(previous.route_state_generation, previous_generation);

    let mut previous_output = [[0.0; 2]; 128];
    let mut twin_output = [[0.0; 2]; 128];
    previous.render(&mut previous_output).unwrap();
    previous_twin.render(&mut twin_output).unwrap();
    assert_eq!(previous_output, twin_output);
    // The previously accepted candidate is also an audible owner. Refusal of
    // another handoff must not damage its inherited DSP continuation.
    let mut accepted_output = [[0.0; 2]; 128];
    accepted.render(&mut accepted_output).unwrap();
    assert_eq!(accepted_output, twin_output);
    assert!(
        accepted_output
            .iter()
            .flatten()
            .any(|sample| *sample != 0.0)
    );
}

#[test]
fn release_bank_refusal_keeps_fresh_candidate_retryable_after_predecessor_drains() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let original_revision = revision(&original, Arc::clone(&table));
    let mut previous = SessionPlayback::new(Arc::clone(&original_revision)).unwrap();
    let mut uninterrupted = SessionPlayback::new(original_revision).unwrap();
    let track = previous.routes[0].id;
    let clip = previous.revision.session().midi_clips()[0].id;
    let mut prefix = [[0.0; 2]; 64];
    for playback in [&mut previous, &mut uninterrupted] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut prefix).unwrap();
    }

    let removed_clip_revision = revision(&without_session_cells(&original), table);
    let mut candidate = SessionPlayback::new(removed_clip_revision).unwrap();
    candidate.release_capacity = 0;
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert!(candidate.candidate_admission_open);

    let mut after_refusal = [[0.0; 2]; 256];
    let mut expected = [[0.0; 2]; 256];
    previous.render(&mut after_refusal).unwrap();
    uninterrupted.render(&mut expected).unwrap();
    assert_eq!(
        after_refusal, expected,
        "the donor remains audible after refusal"
    );
    assert!(after_refusal.iter().flatten().any(|sample| *sample != 0.0));
    let mut drain = vec![[0.0; 2]; 200_000 - 256];
    previous.render(&mut drain).unwrap();
    assert_eq!(previous.release_voice_count(), 0);
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(!candidate.candidate_admission_open);
    assert_eq!(candidate.performance_frame(), previous.performance_frame());
    assert_eq!(candidate.owner(track), Some(SessionPlaybackOwner::Silence));
}
