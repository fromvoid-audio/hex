use super::*;

#[test]
fn gain_only_handoff_preserves_route_clock_and_active_audio() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    assert!(first.iter().any(|frame| frame[0] != 0.0));

    let changed = project(0.75, 0, 8 << 32);
    let mut next = SessionPlayback::new(revision(&changed, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(next.performance_frame(), 64);
    assert_eq!(
        next.owner(old.routes[0].id),
        Some(SessionPlaybackOwner::Session {
            clip: old.revision.session().midi_clips()[0].id
        })
    );
    let mut continued = [[0.0; 2]; 64];
    next.render(&mut continued).unwrap();
    assert!(continued.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn track_gain_handoff_ramps_midi_before_summing() {
    let registry = hex_devices::native_instrument_registry().unwrap();
    let original = project(0.5, 0, 8 << 32);
    let mut old = ordinary_mutable_playback(&original, &registry);
    let mut reference = ordinary_mutable_playback(&original, &registry);
    for playback in [&mut old, &mut reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: playback.revision.session().midi_clips()[0].id,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    let changed = project(0.75, 0, 8 << 32);
    let mut next = ordinary_mutable_playback(&changed, &registry);
    let source = old.revision.source_routing().source(0).unwrap();
    assert!(
        old.revision
            .retained_source_configuration_matches(&next.revision, source)
    );
    next.try_inherit_state_from(&mut old).unwrap();
    assert!(!next.releases.is_empty());
    assert!(next.releases.iter().all(|release| {
        release.location.tail_group().is_none()
            && release
                .source_origin
                .is_some_and(|origin| origin.follows_current_configuration)
    }));
    assert!(next.track_faders[0].active(), "fader was not retargeted");
    let mut actual = [[0.0; 2]; 256];
    let mut expected = [[0.0; 2]; 256];
    next.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert!(!next.track_faders[0].active(), "fader did not advance");
    let duration = next.revision.arrangement().sample_rate().div_ceil(200) as usize;
    for (index, (actual, reference)) in actual.iter().zip(expected).enumerate() {
        let gain_ratio = 1.0 + 0.5 * ((index + 1).min(duration) as f32 / duration as f32);
        for channel in 0..2 {
            assert!(
                (actual[channel] - reference[channel] * gain_ratio).abs() < 1e-5,
                "frame {index} channel {channel}: actual {actual:?}, reference {reference:?}"
            );
        }
    }
}

#[test]
fn paused_keyboard_monitor_uses_the_track_fader_ramp() {
    let project = project(1.0, 0, 8 << 32);
    let revision = revision(&project, Arc::new(hex_devices::SineTable::new()));
    let mut actual = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut reference = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut actual, &mut reference] {
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        runtime.computer_keyboard = Some(Box::new(
            crate::computer_keyboard::ComputerKeyboardRuntime::prepare(
                &project,
                project.tracks()[0].id(),
                input,
            )
            .unwrap(),
        ));
        runtime.render(&mut [[0.0; 2]; 128]).unwrap();
    }
    actual.track_faders[0].retarget([0.0; 2], 48000);
    let mut output = [[0.0; 2]; 256];
    let mut expected = output;
    actual.render(&mut output).unwrap();
    reference.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
    for (index, (output, expected)) in output.iter().zip(expected).enumerate() {
        let gain = 1.0 - (index + 1).min(240) as f32 / 240.0;
        for channel in 0..2 {
            assert!(
                (output[channel] - expected[channel] * gain).abs() < 1e-5,
                "frame {index} channel {channel}: {output:?} vs {expected:?}"
            );
        }
    }
}

#[test]
fn carried_voice_audio_is_partition_invariant_after_tempo_handoff() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    old.render(&mut prefix).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let next_revision = revision(&slower, Arc::clone(&table));
    let mut whole = SessionPlayback::new(Arc::clone(&next_revision)).unwrap();
    let mut split = SessionPlayback::new(next_revision).unwrap();
    whole.try_inherit_state_from(&mut old).unwrap();
    split.try_inherit_state_from(&mut old).unwrap();
    let mut expected = vec![[0.0; 2]; 513];
    whole.render(&mut expected).unwrap();
    let mut actual = Vec::new();
    let mut remaining = 513;
    for width in [1, 31, 256, 7, 218] {
        if remaining == 0 {
            break;
        }
        let width = width.min(remaining);
        let mut block = vec![[0.0; 2]; width];
        split.render(&mut block).unwrap();
        actual.extend(block);
        remaining -= width;
    }
    assert_eq!(remaining, 0);
    assert_eq!(actual, expected);
}

#[test]
fn repeated_loop_occurrence_descriptor_and_pcm_survive_tempo_handoff() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.source_start_raw = 1 << 32;
        clip.source.loop_start_raw = 2 << 32;
        clip.source.loop_end_raw = 4 << 32;
        clip.source.looping = true;
        clip.source.notes = vec![MidiNoteSnapshot {
            active: true,
            id: 6,
            start_raw: 5_u64 << 31, // loop phase 1/2; first repeat begins at 3.5
            duration_raw: 1 << 32,
            pitch: 67,
            channel: 0,
            velocity: 90,
            expression: Default::default(),
            expression_points: Vec::new(),
        }];
    }
    let original = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before_cut = vec![[0.0; 2]; 100_000];
    old.render(&mut before_cut).unwrap();
    assert!(before_cut.iter().any(|frame| frame[0] != 0.0));

    let mut old_spans = MidiSpanBuffer::default();
    let origin = match old.routes[0].owner {
        RouteOwner::Session { origin, .. } => origin,
        _ => panic!("Session source is selected"),
    };
    old.revision
        .session()
        .midi_clip(clip)
        .unwrap()
        .instrument_template()
        .query_with_origin(origin, &old.timing_view(), 100_000, 100_001, &mut old_spans)
        .unwrap();
    let repeated = old_spans
        .spans()
        .find(|span| span.note.value() == 6 && span.iteration == 1)
        .copied()
        .expect("first repeated occurrence remains held at the cut");
    assert_eq!(repeated.start_frame, 84_000);
    assert_eq!(repeated.gate_end_frame, 108_000);

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut slower_snapshot = slower.snapshot();
    slower_snapshot.session = original.snapshot().session;
    let slower = legacy_project_from_snapshot(slower_snapshot).unwrap();
    let next_revision = revision(&slower, Arc::clone(&table));
    let mut whole = SessionPlayback::new(Arc::clone(&next_revision)).unwrap();
    let mut split = SessionPlayback::new(next_revision).unwrap();
    whole.try_inherit_state_from(&mut old).unwrap();
    split.try_inherit_state_from(&mut old).unwrap();
    assert!(whole.releases.iter().any(|entry| {
        entry
            .continuation
            .is_some_and(|continuation| continuation.key.iteration == 1)
    }));
    assert!(whole.suppressions.iter().any(|entry| entry.iteration == 1));

    let mut expected = [[0.0; 2]; 256];
    let mut actual = Vec::new();
    whole.render(&mut expected).unwrap();
    for width in [1, 31, 64, 127, 33] {
        let mut block = vec![[0.0; 2]; width];
        split.render(&mut block).unwrap();
        actual.extend(block);
    }
    assert_eq!(actual.as_slice(), expected);
    assert!(expected.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn source_edit_at_exact_cut_remains_future_eligible() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = vec![[0.0; 2]; 20_000];
    old.render(&mut first).unwrap();
    let cut = old
        .timing_view()
        .frame_to_beat(old.performance_frame())
        .unwrap();

    let edited = with_session_note_id(&project(0.5, cut.raw(), 1 << 30), 77);
    let mut next = SessionPlayback::new(revision(&edited, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut at_cut = [[0.0; 2]; 64];
    next.render(&mut at_cut).unwrap();
    assert!(at_cut.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn inserted_strictly_historical_note_does_not_chase_after_handoff() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = vec![[0.0; 2]; 20_000];
    old.render(&mut first).unwrap();
    assert!(first.iter().any(|frame| frame[0] != 0.0));
    let cut = old
        .timing_view()
        .frame_to_beat(old.performance_frame())
        .unwrap();
    let edited = with_session_note_id(&project(0.5, cut.raw().saturating_sub(1), 1 << 30), 77);
    let mut next = SessionPlayback::new(revision(&edited, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut after = [[0.0; 2]; 64];
    next.render(&mut after).unwrap();
    assert!(after.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn tempo_handoff_preserves_active_voice_phase_and_remaps_held_gate() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 32];
    next.render(&mut actual).unwrap();

    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let gate_end = next
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(8 << 32))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let absolute = 64 + offset as u64;
        let expected = table.sample(voice, absolute, gate_end) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn tempo_handoff_keeps_already_releasing_voice_on_its_original_gate() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = vec![[0.0; 2]; 6_100];
    old.render(&mut before).unwrap();

    let slower = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 32];
    next.render(&mut actual).unwrap();

    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let old_gate = old
        .revision
        .session()
        .timing()
        .beat_to_frame(BeatTime::from_raw(1 << 30))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let absolute = 6_100 + offset as u64;
        let expected = table.sample(voice, absolute, old_gate) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn stopping_after_handoff_cuts_carried_held_voice_without_requerying_history() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    assert!(first.iter().any(|frame| frame[0] != 0.0));
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    let mut session_block = [[0.0; 2]; 64];
    old.render(&mut session_block).unwrap();
    assert!(session_block.iter().any(|frame| frame[0] != 0.0));

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(next.release_voice_count(), 2); // arrangement and carried session voice

    next.apply(SessionPlaybackCommand::StopTrack {
        track,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    assert_eq!(next.owner(track), Some(SessionPlaybackOwner::Silence));
    assert_eq!(next.release_voice_count(), 2);
    assert!(
        next.releases
            .iter()
            .filter(|entry| entry.continuation.is_none())
            .any(|entry| entry.voice.gate_end_frame() == next.performance_frame)
    );
}

#[test]
fn seek_after_anchored_handoff_uses_base_clock_and_resets_history() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut block = [[0.0; 2]; 64];
    old.render(&mut block).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert!(next.anchor.is_some());
    next.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    assert_eq!(next.performance_frame(), 0);
    assert_eq!(next.arrangement_frame(), 0);
    assert!(next.anchor.is_none());
    assert!(next.historical_floor.is_none());
    assert!(next.releases.is_empty());
    assert!(next.suppressions.is_empty());
    assert_eq!(
        next.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let before = next.performance_frame();
    assert_eq!(
        next.apply(SessionPlaybackCommand::Seek(u64::MAX)),
        Err(SessionPlaybackError::TempoConversion)
    );
    assert_eq!(next.performance_frame(), before);
    assert!(next.anchor.is_none());
}

#[test]
fn pause_and_stop_after_handoff_hold_then_reset_the_reanchored_clock() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let prepared = revision(&original, Arc::clone(&table));
    let mut old = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut oracle = SessionPlayback::new(prepared).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    for runtime in [&mut old, &mut oracle] {
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 64]).unwrap();
    }
    oracle
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut candidate = SessionPlayback::new(revision(&slower, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(candidate.anchor.is_some());
    let before = candidate.performance_frame();
    candidate.apply(SessionPlaybackCommand::Pause).unwrap();
    let mut paused = [[7.0; 2]; 32];
    candidate.render(&mut paused).unwrap();
    let mut released = [[0.0; 2]; 32];
    oracle.render(&mut released).unwrap();
    assert_eq!(
        paused, released,
        "reanchored historical gates release once at their original source age"
    );
    assert_eq!(candidate.performance_frame(), before);

    candidate.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(candidate.performance_frame(), 0);
    assert_eq!(candidate.arrangement_frame(), 0);
    assert!(candidate.anchor.is_none());
    assert!(candidate.historical_floor.is_none());
    assert!(candidate.releases.is_empty());
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert!(
        matches!(candidate.routes[0].owner, RouteOwner::Session { origin, .. } if origin == SessionMidiOrigin::launch(BeatTime::ZERO))
    );
}

#[test]
fn failed_handoff_capacity_is_atomic_and_can_be_retried() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut block = [[0.0; 2]; 64];
    old.render(&mut block).unwrap();
    assert!(block.iter().any(|frame| frame[0] != 0.0));
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    let mut session_block = [[0.0; 2]; 64];
    old.render(&mut session_block).unwrap();
    assert!(session_block.iter().any(|frame| frame[0] != 0.0));
    let old_frame = old.performance_frame;
    let old_release_count = old.release_voice_count();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let candidate_revision = revision(&slower, Arc::clone(&table));
    let mut candidate = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    let mut twin = SessionPlayback::new(candidate_revision).unwrap();
    twin.try_inherit_state_from(&mut old).unwrap();
    candidate.release_capacity = 1;
    assert_eq!(
        candidate.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(candidate.performance_frame(), 0);
    assert_eq!(candidate.release_voice_count(), 0);
    assert_eq!(old.performance_frame(), old_frame);
    assert_eq!(old.release_voice_count(), old_release_count);

    candidate.release_capacity = SESSION_RELEASE_CAPACITY;
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.performance_frame(), old_frame);
    assert!(candidate.release_voice_count() > 0);
    let mut after_refusal = [[0.0; 2]; 128];
    let mut twin_output = [[0.0; 2]; 128];
    candidate.render(&mut after_refusal).unwrap();
    twin.render(&mut twin_output).unwrap();
    assert_eq!(after_refusal, twin_output);
}

#[test]
fn failed_instrument_handoff_stages_route_floor_atomically_and_retries_pcm() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = session_only_project(&project(0.5, 0, 8 << 32));
    let replaced = with_replaced_instrument_id(&original);
    let old_revision = revision(&original, Arc::clone(&table));
    let candidate_revision = revision(&replaced, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;

    let mut old = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut old_twin = SessionPlayback::new(old_revision).unwrap();
    for playback in [&mut old, &mut old_twin] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut prefix = [[0.0; 2]; 64];
    let mut twin_prefix = [[0.0; 2]; 64];
    old.render(&mut prefix).unwrap();
    old_twin.render(&mut twin_prefix).unwrap();
    assert_eq!(prefix, twin_prefix);

    let mut candidate = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    let mut successful_twin = SessionPlayback::new(candidate_revision).unwrap();
    successful_twin
        .try_inherit_state_from(&mut old_twin)
        .unwrap();

    let old_routes = old.routes.clone();
    let old_release_facts = old
        .releases
        .iter()
        .map(|entry| {
            (
                entry.track_id,
                entry.location.track_index(),
                entry.order,
                entry.continuation.map(|continuation| {
                    (
                        continuation.key.clip,
                        continuation.key.note,
                        continuation.key.iteration,
                    )
                }),
                entry.voice.gate_end_frame(),
                entry.voice.end_frame(),
            )
        })
        .collect::<Vec<_>>();
    let old_suppressions = old.suppressions.clone();
    let old_clock = (
        old.performance_frame,
        old.arrangement_frame,
        old.anchor,
        old.historical_floor,
        old.route_historical_floors,
        old.playing,
    );
    candidate.release_capacity = 0;
    assert_eq!(
        candidate.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(candidate.routes[0].owner, RouteOwner::Arrangement);
    assert_eq!(candidate.performance_frame, 0);
    assert_eq!(candidate.route_historical_floors, [None; MAX_TRACKS]);
    assert!(candidate.releases.is_empty());
    assert!(candidate.suppressions.is_empty());
    assert_eq!(old.routes, old_routes);
    assert_eq!(
        old.releases
            .iter()
            .map(|entry| {
                (
                    entry.track_id,
                    entry.location.track_index(),
                    entry.order,
                    entry.continuation.map(|continuation| {
                        (
                            continuation.key.clip,
                            continuation.key.note,
                            continuation.key.iteration,
                        )
                    }),
                    entry.voice.gate_end_frame(),
                    entry.voice.end_frame(),
                )
            })
            .collect::<Vec<_>>(),
        old_release_facts
    );
    assert_eq!(old.suppressions, old_suppressions);
    assert_eq!(
        (
            old.performance_frame,
            old.arrangement_frame,
            old.anchor,
            old.historical_floor,
            old.route_historical_floors,
            old.playing,
        ),
        old_clock
    );

    candidate.release_capacity = SESSION_RELEASE_CAPACITY;
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(candidate.route_historical_floors[0].is_some());
    assert_eq!(
        candidate.route_historical_floors,
        successful_twin.route_historical_floors
    );
    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    candidate.render(&mut actual).unwrap();
    successful_twin.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn repeated_handoff_keeps_releasing_voice_when_new_gate_is_unrepresentable() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 1 << 30);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut near_gate = vec![[0.0; 2]; 6_100];
    old.render(&mut near_gate).unwrap(); // voice is already releasing

    let slower = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(60_000_000).unwrap());
    let mut middle = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    middle.try_inherit_state_from(&mut old).unwrap();
    let second_slower = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(1_000_000).unwrap());
    let mut final_revision = SessionPlayback::new(revision(&second_slower, table)).unwrap();
    let cut_beat = middle
        .timing_view()
        .frame_to_beat(middle.performance_frame())
        .unwrap();
    let base_beat = base_timing_for(
        final_revision.revision.session(),
        final_revision.periodic_tempo,
    )
    .frame_to_beat(middle.performance_frame())
    .unwrap();
    let final_timing = timing_view_for(
        final_revision.revision.session(),
        final_revision.periodic_tempo,
        Some(SessionClockAnchor {
            cut_frame: middle.performance_frame(),
            old_beat: cut_beat,
            new_beat: base_beat,
        }),
    );
    assert!(
        final_timing
            .beat_to_frame(BeatTime::from_raw(1 << 30))
            .is_err()
    );
    final_revision.try_inherit_state_from(&mut middle).unwrap();
    assert!(final_revision.release_voice_count() > 0);
}

#[test]
fn quantized_same_clip_retrigger_after_handoff_clears_old_occurrence_suppression() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut playback = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    playback.try_inherit_state_from(&mut old).unwrap();
    assert!(!playback.suppressions.is_empty());
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let due = playback.routes[0].pending.unwrap().frame;
    let width = usize::try_from(due - playback.performance_frame + 64).unwrap();
    let mut output = vec![[0.0; 2]; width];
    playback.render(&mut output).unwrap();
    assert_eq!(playback.performance_frame, due + 64);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert!(playback.suppressions.is_empty()); // outgoing occurrence key was cleared
    assert!(output[width - 64..].iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn repeated_tempo_handoffs_keep_historical_voice_without_requerying_onset() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut first = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    first
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: first.revision.session().midi_clips()[0].id,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    first.apply(SessionPlaybackCommand::Play).unwrap();
    let mut warm = [[0.0; 2]; 64];
    first.render(&mut warm).unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut second = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    second.try_inherit_state_from(&mut first).unwrap();
    let faster = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(180_000_000).unwrap());
    let mut third = SessionPlayback::new(revision(&faster, Arc::clone(&table))).unwrap();
    third.try_inherit_state_from(&mut second).unwrap();
    let mut actual = [[0.0; 2]; 32];
    third.render(&mut actual).unwrap();
    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let gate_end = third
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(8 << 32))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let expected = table.sample(voice, 64 + offset as u64, gate_end) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn repeated_handoffs_suppress_an_occurrence_exactly_at_the_previous_cut() {
    let table = Arc::new(hex_devices::SineTable::new());
    let baseline = project(0.5, 0, 8 << 32);
    let baseline_revision = revision(&baseline, Arc::clone(&table));
    let cut_beat = baseline_revision
        .session()
        .timing()
        .frame_to_beat(64)
        .unwrap();
    assert_eq!(
        baseline_revision
            .session()
            .timing()
            .beat_to_frame(cut_beat)
            .unwrap(),
        64
    );
    let original = project(0.5, cut_beat.raw(), 8 << 32);
    let mut first = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    first
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: first.revision.session().midi_clips()[0].id,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    first.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before_cut = [[0.0; 2]; 64];
    first.render(&mut before_cut).unwrap();

    let slower = project_at_tempo(
        0.5,
        cut_beat.raw(),
        8 << 32,
        Tempo::new(60_000_000).unwrap(),
    );
    let mut second = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    second.try_inherit_state_from(&mut first).unwrap();
    let faster = project_at_tempo(
        0.5,
        cut_beat.raw(),
        8 << 32,
        Tempo::new(180_000_000).unwrap(),
    );
    let mut third = SessionPlayback::new(revision(&faster, Arc::clone(&table))).unwrap();
    third.try_inherit_state_from(&mut second).unwrap();
    let mut actual = [[0.0; 2]; 32];
    third.render(&mut actual).unwrap();
    let voice = table.prepare_voice(48_000, 60, 100).unwrap();
    let gate = third
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(cut_beat.raw() + (8 << 32)))
        .unwrap();
    for (offset, frame) in actual.iter().enumerate() {
        let expected = table.sample(voice, offset as u64, gate - 64) * 0.5;
        assert_eq!(*frame, [expected; 2]);
    }
}

#[test]
fn handoff_remaps_pending_quantized_target_by_beat_and_cancels_missing_clip() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    let track = old.routes[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();

    let slower = project_at_tempo(0.5, 0, 8 << 32, Tempo::new(60_000_000).unwrap());
    let mut next = SessionPlayback::new(revision(&slower, Arc::clone(&table))).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let target = next
        .timing_view()
        .beat_to_frame(BeatTime::from_raw(4 << 32))
        .unwrap();
    assert!(target > next.performance_frame());
    let mut until = vec![[0.0; 2]; (target - next.performance_frame()) as usize];
    next.render(&mut until).unwrap();
    assert_eq!(next.owner(track), Some(SessionPlaybackOwner::Arrangement));
    let mut at_target = [[0.0; 2]; 1];
    next.render(&mut at_target).unwrap();
    assert_eq!(
        next.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut first).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();
    let mut snapshot = original.snapshot();
    snapshot.session.cells.clear();
    let deleted = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&deleted, table)).unwrap();
    let report = candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(report.cancelled_pending_tracks(), &[Some(track)]);
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
}

#[test]
fn pending_scene_launch_keeps_its_resolved_clip_when_scene_contents_change() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first = [[0.0; 2]; 64];
    old.render(&mut first).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    let track = old.routes[0].id;
    old.apply(SessionPlaybackCommand::LaunchScene {
        scene: old.revision.session().scenes()[0].id,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();
    assert_eq!(
        old.routes[0].pending.unwrap().target,
        RouteTarget::Session(clip)
    );

    let mut snapshot = original.snapshot();
    snapshot.session.scenes.push(SceneSnapshot {
        id: 7,
        name: "moved clip".into(),
    });
    snapshot.session.cells[0].scene = 7;
    snapshot.session.cells.push(SessionCellSnapshot {
        track: 1,
        scene: 4,
        content: SessionCellContentSnapshot::Stop,
    });
    let edited = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&edited, table)).unwrap();
    let report = candidate.try_inherit_state_from(&mut old).unwrap();
    assert!(report.cancelled_pending_tracks().is_empty());
    let route = candidate
        .routes
        .iter()
        .find(|route| route.id == track)
        .unwrap();
    assert_eq!(route.pending.unwrap().target, RouteTarget::Session(clip));
}

#[test]
fn due_pending_quantization_uses_actual_cut_beat_on_shared_frame_plateau() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut old_snapshot = project(0.5, 0, 8 << 32).snapshot();
    old_snapshot.sample_rate = 48_000;
    let boundary = 4_u64 << 32;
    let later_same_frame = boundary + 80_000;
    old_snapshot.tempo_points = vec![
        TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT),
        TempoPoint::new(
            BeatTime::from_raw(later_same_frame),
            Tempo::new(60_000_000).unwrap(),
        ),
    ];
    let old_project = legacy_project_from_snapshot(old_snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::Seek(95_999)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    })
    .unwrap();
    assert_eq!(old.routes[0].pending.unwrap().frame, 96_000);
    assert_eq!(old.routes[0].pending.unwrap().beat.raw(), boundary);
    let mut at_cut = [[0.0; 2]; 1];
    old.render(&mut at_cut).unwrap();
    assert_eq!(old.performance_frame(), 96_000);
    assert_eq!(old.routes[0].pending.unwrap().frame, 96_000);
    let effective_cut = old.timing_view().frame_to_beat(96_000).unwrap();
    assert!(effective_cut.raw() > boundary);
    assert_eq!(
        old.timing_view().beat_to_frame(effective_cut).unwrap(),
        96_000
    );

    let mut new_snapshot = old_project.snapshot();
    new_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(60_000_000).unwrap(),
    )];
    let new_project = legacy_project_from_snapshot(new_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&new_project, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.routes[0].pending.unwrap().frame, 96_000);
    assert_eq!(candidate.routes[0].pending.unwrap().beat.raw(), boundary);
    let mut launch = [[0.0; 2]; 1];
    candidate.render(&mut launch).unwrap();
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    let RouteOwner::Session { origin, .. } = candidate.routes[0].owner else {
        panic!("due launch selects the Session clip");
    };
    assert_eq!(origin, SessionMidiOrigin::launch(effective_cut));
    let mut spans = MidiSpanBuffer::default();
    candidate
        .revision
        .session()
        .midi_clip(clip)
        .unwrap()
        .instrument_template()
        .query_with_origin(origin, &candidate.timing_view(), 96_000, 96_001, &mut spans)
        .unwrap();
    assert!(spans.spans().any(|span| span.start_frame == 96_000));
}

#[test]
fn later_chunk_capacity_refusal_rolls_back_carried_voice_and_retries_against_twin() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project_at_tempo(0.5, 0, 1 << 30, Tempo::new(600_000_000).unwrap());
    let mut snapshot = base.snapshot();
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.notes.push(MidiNoteSnapshot {
            active: true,
            id: 13,
            start_raw: 1_u64 << 27, // begins 1/32 beat after the carried note
            duration_raw: 1 << 30,
            pitch: 67,
            channel: 0,
            velocity: 90,
            expression: Default::default(),
            expression_points: Vec::new(),
        });
    }
    // Keep the Arrangement-only note outside the handoff window so launching
    // Session does not create a second, unrelated release-bank entry.
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    {
        clips[0].notes[0].start_raw = 8 << 32;
    }
    snapshot.next_id = 14;
    let old_project = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut warmup = [[0.0; 2]; 1];
    old.render(&mut warmup).unwrap();

    let mut changed_snapshot = old_project.snapshot();
    changed_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(500_000_000).unwrap(),
    )];
    let changed = legacy_project_from_snapshot(changed_snapshot).unwrap();
    let candidate_revision = revision(&changed, Arc::clone(&table));
    let mut failed = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    failed.release_capacity = 1;
    failed.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(failed.release_voice_count(), 1);
    failed.routes[0].pending = Some(PendingRoute {
        note_chase: crate::NoteChase::On,
        frame: 257,
        beat: BeatTime::from_raw(1 << 32),
        target: RouteTarget::Silence,
        scene: None,
        current_reservation: None,
    });

    let mut prefix_probe = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    prefix_probe.release_capacity = 1;
    prefix_probe.try_inherit_state_from(&mut old).unwrap();
    prefix_probe.routes[0].pending = failed.routes[0].pending;
    let mut first_chunk = [[0.0; 2]; 256];
    prefix_probe.render(&mut first_chunk).unwrap();
    assert!(first_chunk.iter().any(|frame| frame[0] != 0.0));
    assert_eq!(prefix_probe.performance_frame(), 257);

    let mut twin = SessionPlayback::new(candidate_revision).unwrap();
    twin.release_capacity = 1;
    twin.try_inherit_state_from(&mut old).unwrap();
    twin.routes[0].pending = failed.routes[0].pending;

    let routes = failed.routes.clone();
    let releases = failed.releases.clone();
    let suppressions = failed.suppressions.clone();
    let release_order = failed.release_order;
    let route_generation = failed.route_state_generation();
    let mut whole = [[9.0; 2]; 512];
    assert_eq!(
        failed.render(&mut whole),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(whole, [[0.0; 2]; 512]);
    assert_eq!(failed.performance_frame(), 1);
    assert_eq!(failed.arrangement_frame(), 1);
    assert_eq!(failed.route_state_generation(), route_generation);
    assert_eq!(failed.routes, routes);
    assert_eq!(failed.release_order, release_order);
    assert_eq!(failed.releases.len(), releases.len());
    assert_eq!(failed.suppressions, suppressions);
    assert_eq!(
        failed.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert_eq!(failed.routes[0].pending, routes[0].pending);
    assert_eq!(
        failed.releases[0].voice.end_frame(),
        releases[0].voice.end_frame()
    );

    failed
        .apply(SessionPlaybackCommand::CancelPending { track: None })
        .unwrap();
    twin.apply(SessionPlaybackCommand::CancelPending { track: None })
        .unwrap();
    let mut retry = [[0.0; 2]; 512];
    let mut expected = [[0.0; 2]; 512];
    failed.render(&mut retry).unwrap();
    twin.render(&mut expected).unwrap();
    assert_eq!(retry, expected);
}

#[test]
fn streamed_later_chunk_refusal_zeros_block_and_rolls_back_after_page_requests() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut original_snapshot = mixed_audio_midi_project(5, 18).snapshot();
    let original_note_id = original_snapshot.next_id;
    let Some(SessionCellContentSnapshot::Midi(original_clip)) = original_snapshot
        .session
        .cells
        .iter_mut()
        .find(|cell| cell.track == 6)
        .map(|cell| &mut cell.content)
    else {
        panic!("Session MIDI fixture expected")
    };
    original_clip.source.notes.push(MidiNoteSnapshot {
        active: true,
        id: original_note_id,
        start_raw: 0,
        duration_raw: 1 << 30,
        pitch: 60,
        channel: 0,
        velocity: 100,
        expression: Default::default(),
        expression_points: Vec::new(),
    });
    original_snapshot.next_id += 1;
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut original_snapshot.tracks[1].content
    {
        clips[0].notes.clear();
    }
    let original = legacy_project_from_snapshot(original_snapshot).unwrap();
    let stream_asset = original
        .assets()
        .find(|asset| asset.id().value() == 4)
        .unwrap();
    let stream_source = StreamedSourceMetadata::new(
        StreamSourceKey::new(71, stream_asset.id()),
        stream_asset.source_sample_rate(),
        original.sample_rate(),
        stream_asset.source_frames(),
        stream_asset.playback_frames(),
        stream_asset.conversion_version(),
        [0x71; 32],
    )
    .unwrap();
    let make_sources = |project: &Project| {
        let resident = crate::Audio::new(48_000, vec![[0.25, -0.25]; 32]).unwrap();
        project
            .assets()
            .map(|asset| {
                let source = if asset.id().value() == 4 {
                    PreparedAudioSource::Streamed(Arc::new(stream_source.clone()))
                } else {
                    PreparedAudioSource::Resident(resident.clone())
                };
                (asset.id(), source)
            })
            .collect::<BTreeMap<_, _>>()
    };
    let old_revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &original,
            &make_sources(&original),
            71,
            Arc::clone(&table),
            None,
        )
        .unwrap(),
    );
    let mut old = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut provider_refused = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    provider_refused
        .apply(SessionPlaybackCommand::Play)
        .unwrap();
    let mut refused_output = [[0.0; 2]; 1];
    assert_eq!(
        provider_refused.render(&mut refused_output),
        Err(SessionPlaybackError::MediaPagesUnavailable)
    );
    let mut unused_predecessor = SessionPlayback::new(old_revision).unwrap();
    assert_eq!(
        provider_refused.try_inherit_state_from(&mut unused_predecessor),
        Err(SessionPlaybackError::CandidateAlreadyUsed)
    );
    let midi_clip = old
        .revision
        .session()
        .midi_clips()
        .iter()
        .find(|clip| clip.track.value() == 6)
        .unwrap()
        .id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: midi_clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut warmup_pages = ContinuityMissingPages;
    let mut warmup = [[0.0; 2]; 1];
    old.render_with_pages(&mut warmup, &mut warmup_pages)
        .unwrap();

    let mut changed_snapshot = original.snapshot();
    let added_note_id = changed_snapshot.next_id;
    let Some(SessionCellContentSnapshot::Midi(clip)) = changed_snapshot
        .session
        .cells
        .iter_mut()
        .find(|cell| cell.track == 6)
        .map(|cell| &mut cell.content)
    else {
        panic!("Session MIDI fixture expected")
    };
    clip.source.notes.push(MidiNoteSnapshot {
        active: true,
        id: added_note_id,
        start_raw: 1_u64 << 27,
        duration_raw: 1 << 30,
        pitch: 67,
        channel: 0,
        velocity: 90,
        expression: Default::default(),
        expression_points: Vec::new(),
    });
    changed_snapshot.next_id += 1;
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut changed_snapshot.tracks[1].content
    {
        clips[0].notes.clear();
    }
    changed_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(500_000_000).unwrap(),
    )];
    let changed = legacy_project_from_snapshot(changed_snapshot).unwrap();
    let candidate_revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &changed,
            &make_sources(&changed),
            71,
            Arc::clone(&table),
            Some(old.revision.session()),
        )
        .unwrap(),
    );
    let mut failed = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    failed.release_capacity = 1;
    failed.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(failed.release_voice_count(), 1);
    let audio_clip = failed.revision.session().audio_clips()[0].id;
    failed
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    failed.routes[1].pending = Some(PendingRoute {
        note_chase: crate::NoteChase::On,
        frame: 256,
        beat: BeatTime::from_raw(1 << 32),
        target: RouteTarget::Silence,
        scene: None,
        current_reservation: None,
    });

    #[derive(Default)]
    struct MissingPages(Vec<(StreamSourceKey, u64)>);
    impl MediaPages for MissingPages {
        fn page(
            &mut self,
            _key: StreamSourceKey,
            _page_index: u64,
        ) -> Option<BorrowedMediaPage<'_>> {
            None
        }

        fn request_page(&mut self, key: StreamSourceKey, page_index: u64) {
            self.0.push((key, page_index));
        }
    }

    let routes = failed.routes.clone();
    let release_end = failed.releases[0].voice.end_frame();
    let route_generation = failed.route_state_generation();
    // The 500 bpm edit remaps the Arrangement cut by beat (Pulse #3935).
    let arrangement_frame = failed.arrangement_frame();
    let mut pages = MissingPages::default();
    let mut output = [[9.0; 2]; 512];
    assert_eq!(
        failed.render_with_pages(&mut output, &mut pages),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(pages.0, [(stream_source.key(), 0)]);
    assert_eq!(failed.performance_frame(), 1);
    assert_eq!(failed.arrangement_frame(), arrangement_frame);
    assert_eq!(failed.route_state_generation(), route_generation);
    assert_eq!(failed.routes, routes);
    assert_eq!(failed.release_voice_count(), 1);
    assert_eq!(failed.releases[0].voice.end_frame(), release_end);
}

#[test]
fn deleting_the_last_track_keeps_its_copied_session_voice_tail() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();
    assert!(before.iter().any(|frame| frame[0] != 0.0));
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip: old.revision.session().midi_clips()[0].id,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    let mut session_block = [[0.0; 2]; 64];
    old.render(&mut session_block).unwrap();
    assert!(session_block.iter().any(|frame| frame[0] != 0.0));
    let mut snapshot = original.snapshot();
    snapshot.tracks.clear();
    snapshot.session.cells.clear();
    let deleted = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&deleted, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.release_voice_count(), 2);
    assert_eq!(candidate.performance_frame(), old.performance_frame());
    assert_eq!(candidate.arrangement_frame(), old.arrangement_frame());
    candidate
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    candidate
        .apply(SessionPlaybackCommand::StopAllClips {
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(candidate.release_voice_count(), 2);
    let mut tail = [[0.0; 2]; 64];
    candidate.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn reordered_tracks_keep_project_order_and_deleted_arrangement_tail_uses_old_mix() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    snapshot.next_id = 15;
    for (track_id, clip_id, note_id, gain) in [(7, 13, 14, 0.25), (10, 11, 12, 0.75)] {
        snapshot.tracks.push(TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track_id,
            name: format!("track {track_id}"),
            gain,
            balance: 0.2,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                content_view: None,
                active: true,
                playback_offset_raw: 0,
                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                volume: hex_project::MidiClipVolume::default(),
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                id: clip_id,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 32 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 32 << 32,
                looping: false,
                notes: vec![MidiNoteSnapshot {
                    active: true,
                    id: note_id,
                    start_raw: 0,
                    duration_raw: 8 << 32,
                    pitch: 60 + track_id as u8,
                    channel: 0,
                    velocity: 90,
                    expression: Default::default(),
                    expression_points: Vec::new(),
                }],
                expression_lanes: Vec::new(),
            }]),
            sends: Vec::new(),
        });
    }
    let original = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();

    let mut edited = original.snapshot();
    edited.tracks = vec![edited.tracks[1].clone(), edited.tracks[0].clone()];
    let candidate_project = legacy_project_from_snapshot(edited).unwrap();
    let candidate_revision = revision(&candidate_project, Arc::clone(&table));
    let mut candidate = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(candidate.routes[0].id.value(), 7);
    assert_eq!(candidate.routes[1].id.value(), 1);
    let deleted_tail = candidate
        .releases
        .iter()
        .find(|entry| entry.track_id.value() == 10)
        .copied()
        .unwrap();

    let mut actual = [[0.0; 2]; 64];
    candidate.render(&mut actual).unwrap();
    let mut expected = [[0.0; 2]; 64];
    for track in candidate_revision.arrangement().tracks() {
        track
            .render_additive(candidate.arrangement_frame - 64, &mut expected)
            .unwrap();
    }
    for (offset, frame) in expected.iter_mut().enumerate() {
        let absolute = candidate.performance_frame - 64 + offset as u64;
        if let Some(sample) = deleted_tail.voice.sample_at(
            absolute,
            candidate_revision.arrangement().shared_contexts(),
            &candidate.retained_contexts,
        ) {
            deleted_tail.mix.add_frame(frame, sample);
        }
    }
    candidate_revision
        .arrangement()
        .master_mix()
        .apply(&mut expected);
    assert_eq!(actual, expected);
}

#[test]
fn source_start_and_loop_edit_releases_active_intro_occurrence_without_reusing_phase_key() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    {
        let clip = &mut clips[0];
        clip.source_start_raw = 1 << 32;
        clip.loop_start_raw = 2 << 32;
        clip.loop_end_raw = 4 << 32;
        clip.looping = true;
        clip.notes = vec![
            MidiNoteSnapshot {
                active: true,
                id: 3,
                start_raw: 3_u64 << 31, // intro onset, gate crosses the first loop seam
                duration_raw: 4 << 32,
                pitch: 60,
                channel: 0,
                velocity: 100,
                expression: Default::default(),
                expression_points: Vec::new(),
            },
            MidiNoteSnapshot {
                active: true,
                id: 13,
                start_raw: 5_u64 << 31, // repeated loop phase
                duration_raw: 1 << 32,
                pitch: 67,
                channel: 0,
                velocity: 90,
                expression: Default::default(),
                expression_points: Vec::new(),
            },
        ];
    }
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.source_start_raw = 1 << 32;
        clip.source.loop_start_raw = 2 << 32;
        clip.source.loop_end_raw = 4 << 32;
        clip.source.looping = true;
        clip.source.notes = vec![
            MidiNoteSnapshot {
                active: true,
                id: 14,
                start_raw: 3_u64 << 31,
                duration_raw: 4 << 32,
                pitch: 60,
                channel: 0,
                velocity: 100,
                expression: Default::default(),
                expression_points: Vec::new(),
            },
            MidiNoteSnapshot {
                active: true,
                id: 15,
                start_raw: 5_u64 << 31,
                duration_raw: 1 << 32,
                pitch: 67,
                channel: 0,
                velocity: 90,
                expression: Default::default(),
                expression_points: Vec::new(),
            },
        ];
    }
    snapshot.next_id = 16;
    let original = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut intro = vec![[0.0; 2]; 24_000];
    old.render(&mut intro).unwrap();

    let mut edited_snapshot = original.snapshot();
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut edited_snapshot.tracks[0].content
    {
        clips[0].source_start_raw = 0;
        clips[0].loop_start_raw = 2 << 32;
        clips[0].loop_end_raw = 5 << 32;
    }
    if let SessionCellContentSnapshot::Midi(clip) = &mut edited_snapshot.session.cells[0].content {
        clip.source.source_start_raw = 0;
        clip.source.loop_start_raw = 2 << 32;
        clip.source.loop_end_raw = 5 << 32;
    }
    let edited = legacy_project_from_snapshot(edited_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&edited, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        candidate.owner(old.routes[0].id),
        Some(SessionPlaybackOwner::Session { clip })
    );
    assert!(
        candidate
            .releases
            .iter()
            .any(|entry| entry.continuation.is_none() && entry.voice.end_frame() > 24_000)
    );
}

#[test]
fn moving_selected_clip_to_another_track_releases_old_route() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(0.5, 0, 8 << 32);
    let mut old = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let old_track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before = [[0.0; 2]; 64];
    old.render(&mut before).unwrap();
    assert!(before.iter().any(|frame| frame[0] != 0.0));
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    let mut session_block = [[0.0; 2]; 64];
    old.render(&mut session_block).unwrap();
    assert!(session_block.iter().any(|frame| frame[0] != 0.0));

    let mut snapshot = original.snapshot();
    snapshot.tracks.push(TrackSnapshot {
        group_output_mutes: Vec::new(),
        tags: Vec::new(),
        kind: hex_project::TrackKind::Ordinary,
        parent_group: None,
        color: hex_project::ElementColor::default(),
        gain_envelope: Vec::new(),
        gain_touch_spans: Vec::new(),
        gain_touch_epoch: 0,
        id: 7,
        name: "other".into(),
        gain: 1.0,
        balance: 0.0,
        muted: false,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        instrument_position: None,
        content: TrackContentSnapshot::Instrument(Vec::new()),
        sends: Vec::new(),
    });
    snapshot.session.cells[0].track = 7;
    let moved = legacy_project_from_snapshot(snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&moved, table)).unwrap();
    candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        candidate.owner(old_track),
        Some(SessionPlaybackOwner::Silence)
    );
    assert_eq!(candidate.release_voice_count(), 2);
}

#[test]
fn unrepresentable_matching_gate_remap_refuses_without_candidate_commit() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(0.5, 0, 8 << 32);
    let mut snapshot = base.snapshot();
    if let TrackContentSnapshot::Hybrid {
        midi_clips: clips, ..
    } = &mut snapshot.tracks[0].content
    {
        clips[0].length_raw = u64::MAX;
        clips[0].source_length_raw = u64::MAX;
        clips[0].loop_end_raw = u64::MAX;
        clips[0].notes[0].duration_raw = u64::MAX;
    }
    if let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content {
        clip.source.source_length_raw = u64::MAX;
        clip.source.loop_end_raw = u64::MAX;
        clip.source.notes[0].duration_raw = u64::MAX;
    }
    let long = legacy_project_from_snapshot(snapshot).unwrap();
    let mut old = SessionPlayback::new(revision(&long, Arc::clone(&table))).unwrap();
    let track = old.routes[0].id;
    let clip = old.revision.session().midi_clips()[0].id;
    old.apply(SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::Immediate,
    })
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let cut = old
        .revision
        .session()
        .timing()
        .beat_to_frame(BeatTime::from_raw(1_u64 << 62))
        .unwrap();
    old.performance_frame = cut;
    old.arrangement_frame = cut;

    let mut faster_snapshot = long.snapshot();
    faster_snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(240_000_000).unwrap(),
    )];
    let faster = legacy_project_from_snapshot(faster_snapshot).unwrap();
    let mut candidate = SessionPlayback::new(revision(&faster, table)).unwrap();
    assert_eq!(
        candidate.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::TempoConversion)
    );
    assert_eq!(candidate.performance_frame(), 0);
    assert_eq!(
        candidate.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    assert_eq!(candidate.release_voice_count(), 0);
}

#[test]
fn reduced_shared_bank_refuses_a_cut_atomically_and_recovers_after_expiry() {
    let table = Arc::new(hex_devices::SineTable::new());
    let project = project(0.5, 0, 8 << 32);
    let mut playback = SessionPlayback::new(revision(&project, table)).unwrap();
    let track = playback.routes[0].id;
    let clip = playback.revision.session().midi_clips()[0].id;
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut initial = [[0.0; 2]; 2_500];
    playback.render(&mut initial).unwrap();
    playback.release_capacity = 1;
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(playback.release_voice_count(), 1);
    let old_release_boundaries = (
        playback.releases[0].voice.gate_end_frame(),
        playback.releases[0].voice.end_frame(),
    );
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();

    // A full bank still permits cancelling an unheard exact-onset occurrence.
    let unheard_frame = playback.performance_frame();
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(playback.performance_frame(), unheard_frame);
    assert_eq!(playback.release_voice_count(), 1);
    assert_eq!(
        (
            playback.releases[0].voice.gate_end_frame(),
            playback.releases[0].voice.end_frame(),
        ),
        old_release_boundaries
    );
    assert_eq!(playback.owner(track), Some(SessionPlaybackOwner::Silence));
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();

    // Once that occurrence has sounded, its tail needs another release slot.
    let mut relaunched = [[0.0; 2]; 1];
    playback.render(&mut relaunched).unwrap();
    let cut_frame = playback.performance_frame();
    assert_eq!(
        playback.apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        }),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(playback.performance_frame(), cut_frame);
    assert_eq!(playback.release_voice_count(), 1);
    assert_eq!(
        (
            playback.releases[0].voice.gate_end_frame(),
            playback.releases[0].voice.end_frame(),
        ),
        old_release_boundaries
    );
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip })
    );

    let mut after_expiry = [[0.0; 2]; 2_500];
    playback.render(&mut after_expiry).unwrap();
    assert_eq!(playback.release_voice_count(), 0);
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert_eq!(playback.owner(track), Some(SessionPlaybackOwner::Silence));
}

#[test]
fn route_status_generation_tracks_committed_commands_and_due_transitions() {
    let table = Arc::new(hex_devices::SineTable::new());
    let project = project(0.5, 0, 8 << 32);
    let revision = revision(&project, Arc::clone(&table));
    let track = project.tracks()[0].id();
    let clip = match project.session_cells()[0].content() {
        hex_project::SessionCellContent::Midi(clip) => clip.id(),
        hex_project::SessionCellContent::Audio(_) => panic!("expected Session MIDI clip"),
        hex_project::SessionCellContent::Stop => panic!("expected Session clip"),
    };
    let mut playback = SessionPlayback::new(revision).unwrap();
    let initial = playback.route_state_generation();
    let route = playback.route_status(0).unwrap();
    assert_eq!(route.track, track);
    assert_eq!(route.owner, SessionPlaybackOwner::Arrangement);
    assert_eq!(route.active_origin, None);
    assert_eq!(route.pending, None);

    playback.apply(SessionPlaybackCommand::Play).unwrap();
    assert_eq!(playback.route_state_generation(), initial);
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let launched = playback.route_state_generation();
    assert_eq!(launched, initial.wrapping_add(1));
    let route = playback.route_status(0).unwrap();
    assert_eq!(route.owner, SessionPlaybackOwner::Session { clip });
    assert_eq!(
        route.active_origin,
        Some(SessionMidiOrigin::launch(BeatTime::ZERO))
    );

    let mut advance = [[0.0; 2]; 64];
    playback.render(&mut advance).unwrap();
    let before_pending = playback.route_state_generation();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let pending_generation = playback.route_state_generation();
    assert_eq!(pending_generation, before_pending.wrapping_add(1));
    let route = playback.route_status(0).unwrap();
    let pending = route.pending.unwrap();
    assert_eq!(pending.target, SessionRouteTarget::Session { clip });
    assert_eq!(pending.performance_frame, 96_000);
    assert_eq!(pending.beat, BeatTime::from_quarters(4).unwrap());

    playback
        .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
        .unwrap();
    assert_eq!(
        playback.route_state_generation(),
        pending_generation.wrapping_add(1)
    );
    assert_eq!(playback.route_status(0).unwrap().pending, None);

    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let due_generation = playback.route_state_generation();
    let mut until_due = vec![[0.0; 2]; 96_000 - 64];
    playback.render(&mut until_due).unwrap();
    let mut first_due_block = [[0.0; 2]; 1];
    playback.render(&mut first_due_block).unwrap();
    assert_eq!(
        playback.route_state_generation(),
        due_generation.wrapping_add(1)
    );
    assert_eq!(playback.route_status(0).unwrap().pending, None);

    let before_stop = playback.route_state_generation();
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(
        playback.route_state_generation(),
        before_stop.wrapping_add(1)
    );
    assert_eq!(playback.performance_frame(), 0);
    assert_eq!(
        playback.route_status(0).unwrap().active_origin,
        Some(SessionMidiOrigin::launch(BeatTime::ZERO))
    );
}
