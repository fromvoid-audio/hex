use super::*;

#[test]
fn changing_session_instrument_identity_carries_old_held_voice_once() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = session_only_project(&project(0.5, 0, 8 << 32));
    let old_revision = revision(&original, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut reference = SessionPlayback::new(old_revision).unwrap();
    for playback in [&mut previous, &mut reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();
    reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);

    let replaced = with_replaced_instrument_id(&original);
    let mut candidate = SessionPlayback::new(revision(&replaced, table)).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(candidate.releases.iter().any(|entry| {
        entry.track_id == candidate.routes[0].id
            && entry.continuation.is_some()
            && entry.timeline == ReleaseTimeline::Session
    }));

    let mut expected = [[0.0; 2]; 128];
    let mut actual = [[0.0; 2]; 128];
    reference.render(&mut expected).unwrap();
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn session_bypass_releases_old_held_voice_and_reenable_does_not_chase_it() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = session_only_project(&project(0.5, 0, 8 << 32));
    let old_revision = revision(&original, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut reference = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    reference
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();
    reference.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut expected_next = [[0.0; 2]; 1];
    reference.render(&mut expected_next).unwrap();
    assert!(prefix.iter().any(|frame| frame[0] != 0.0));

    let bypassed = with_instrument_bypassed(&original, true);
    let mut muted_revision = SessionPlayback::new(revision(&bypassed, Arc::clone(&table))).unwrap();
    muted_revision
        .try_inherit_state_from(&mut previous)
        .unwrap();
    assert!(muted_revision.releases.iter().any(|entry| {
        entry.track_id == muted_revision.routes[0].id
            && entry.continuation.is_none()
            && entry.end_frame() == muted_revision.performance_frame + 240
    }));
    assert!(muted_revision.suppressions.is_empty());

    let mut tail = vec![[0.0; 2]; 320];
    muted_revision.render(&mut tail).unwrap();
    assert!(
        (tail[0][0] - expected_next[0][0]).abs() < 0.0001,
        "instrument bypass discontinuity: {:?} -> {:?}",
        expected_next[0],
        tail[0]
    );
    assert!(tail.iter().any(|frame| frame[0] != 0.0));
    assert!(
        tail[240..].iter().all(|frame| *frame == [0.0; 2]),
        "session power-off exceeded 5 ms"
    );

    let enabled = with_instrument_bypassed(&bypassed, false);
    let mut reenabled = SessionPlayback::new(revision(&enabled, Arc::clone(&table))).unwrap();
    reenabled
        .try_inherit_state_from(&mut muted_revision)
        .unwrap();
    let mut after_reenable = [[0.0; 2]; 64];
    reenabled.render(&mut after_reenable).unwrap();
    assert!(after_reenable.iter().all(|frame| *frame == [0.0; 2]));
    assert!(reenabled.route_historical_floors[0].is_some());

    let removed = without_instrument_slot(&enabled);
    let mut removed_playback =
        SessionPlayback::new(revision(&removed, Arc::clone(&table))).unwrap();
    removed_playback
        .try_inherit_state_from(&mut reenabled)
        .unwrap();
    assert!(removed_playback.route_historical_floors[0].is_some());
    let readded = with_inserted_instrument_slot(
        &removed,
        original.snapshot().tracks[0]
            .instrument
            .as_ref()
            .unwrap()
            .state
            .clone(),
    );
    let mut readded_playback = SessionPlayback::new(revision(&readded, table)).unwrap();
    readded_playback
        .try_inherit_state_from(&mut removed_playback)
        .unwrap();
    assert!(readded_playback.route_historical_floors[0].is_some());
    let mut after_readd = [[0.0; 2]; 64];
    readded_playback.render(&mut after_readd).unwrap();
    assert!(after_readd.iter().all(|frame| *frame == [0.0; 2]));
    readded_playback
        .apply(SessionPlaybackCommand::Seek(0))
        .unwrap();
    assert_eq!(readded_playback.route_historical_floors[0], None);
    let mut after_seek = [[0.0; 2]; 64];
    readded_playback.render(&mut after_seek).unwrap();
    assert!(after_seek.iter().any(|frame| frame[0] != 0.0));
    readded_playback
        .apply(SessionPlaybackCommand::Stop)
        .unwrap();
    assert_eq!(readded_playback.route_historical_floors[0], None);
}

#[test]
fn reenable_floor_excludes_old_onsets_but_keeps_note_at_exact_cut() {
    let table = Arc::new(hex_devices::SineTable::new());
    let with_notes =
        with_session_note_at(&session_only_project(&project(0.5, 0, 8 << 32)), 1 << 32);
    let disabled = with_instrument_bypassed(&with_notes, true);
    let mut previous = SessionPlayback::new(revision(&disabled, Arc::clone(&table))).unwrap();
    let clip = previous.revision.session().midi_clips()[0].id;
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut silent_prefix = vec![[0.0; 2]; 24_000];
    previous.render(&mut silent_prefix).unwrap();
    assert!(silent_prefix.iter().all(|frame| *frame == [0.0; 2]));

    let enabled = with_instrument_bypassed(&with_notes, false);
    let mut candidate = SessionPlayback::new(revision(&enabled, Arc::clone(&table))).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(candidate.route_historical_floors[0], Some(BeatTime::ONE));

    let mut oracle_snapshot = enabled.snapshot();
    if let SessionCellContentSnapshot::Midi(clip) = &mut oracle_snapshot.session.cells[0].content {
        clip.source.notes.retain(|note| note.start_raw == 1 << 32);
    }
    let oracle_project = legacy_project_from_snapshot(oracle_snapshot).unwrap();
    let mut oracle = SessionPlayback::new(revision(&oracle_project, table)).unwrap();
    let oracle_clip = oracle.revision.session().midi_clips()[0].id;
    oracle
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: oracle_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut oracle_prefix = vec![[0.0; 2]; 24_000];
    oracle.render(&mut oracle_prefix).unwrap();
    assert!(oracle_prefix.iter().all(|frame| *frame == [0.0; 2]));

    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    candidate.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn reenabled_loop_source_skips_old_occurrences_and_plays_future_iteration() {
    let table = Arc::new(hex_devices::SineTable::new());
    let base = session_only_project(&project(0.5, 0, 8 << 32));
    let mut snapshot = base.snapshot();
    let SessionCellContentSnapshot::Midi(clip) = &mut snapshot.session.cells[0].content else {
        panic!("fixture has a Session MIDI clip")
    };
    clip.source.source_length_raw = 4 << 32;
    clip.source.loop_start_raw = 0;
    clip.source.loop_end_raw = 1 << 32;
    clip.source.looping = true;
    clip.source.notes = vec![MidiNoteSnapshot {
        active: true,
        id: clip.source.notes[0].id,
        start_raw: 1 << 30,
        duration_raw: 3 << 32,
        pitch: 60,
        channel: 0,
        velocity: 100,
        expression: Default::default(),
        expression_points: Vec::new(),
    }];
    let looping = legacy_project_from_snapshot(snapshot).unwrap();
    let disabled = with_instrument_bypassed(&looping, true);
    let mut previous = SessionPlayback::new(revision(&disabled, Arc::clone(&table))).unwrap();
    let clip_id = previous.revision.session().midi_clips()[0].id;
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: clip_id,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut disabled_prefix = vec![[0.0; 2]; 36_000];
    previous.render(&mut disabled_prefix).unwrap();
    assert!(disabled_prefix.iter().all(|frame| *frame == [0.0; 2]));

    let enabled = with_instrument_bypassed(&looping, false);
    let mut candidate = SessionPlayback::new(revision(&enabled, table)).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(
        candidate.route_historical_floors[0],
        Some(BeatTime::from_ratio(3, 2).unwrap())
    );

    let mut before_next_iteration = vec![[0.0; 2]; 18_000];
    candidate.render(&mut before_next_iteration).unwrap();
    assert!(before_next_iteration.iter().all(|frame| *frame == [0.0; 2]));
    let mut next_iteration = [[0.0; 2]; 128];
    candidate.render(&mut next_iteration).unwrap();
    assert!(next_iteration.iter().any(|frame| frame[0] != 0.0));
}

#[test]
fn removing_session_instrument_releases_old_voice_without_candidate_suppression() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = session_only_project(&project(0.5, 0, 8 << 32));
    let old_revision = revision(&original, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();

    let removed = without_instrument_slot(&original);
    let mut candidate = SessionPlayback::new(revision(&removed, Arc::clone(&table))).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(candidate.suppressions.is_empty());
    assert!(
        candidate.releases.iter().any(|entry| {
            entry.track_id == candidate.routes[0].id && entry.continuation.is_none()
        })
    );
    let mut tail = vec![[0.0; 2]; 2_500];
    candidate.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| frame[0] != 0.0));
    assert!(tail[2_400..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn session_instrument_change_at_note_onset_leaves_boundary_note_to_candidate() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = session_only_project(&project_at_tempo(0.5, 1 << 32, 8 << 32, Tempo::DEFAULT));
    let old_revision = revision(&original, Arc::clone(&table));
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    let mut reference = SessionPlayback::new(old_revision).unwrap();
    for playback in [&mut previous, &mut reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut before = vec![[0.0; 2]; 24_000];
    let mut reference_before = vec![[0.0; 2]; 24_000];
    previous.render(&mut before).unwrap();
    reference.render(&mut reference_before).unwrap();
    assert_eq!(before, reference_before);
    assert!(before.iter().all(|frame| *frame == [0.0; 2]));

    let replaced = with_replaced_instrument_id(&original);
    let mut candidate = SessionPlayback::new(revision(&replaced, Arc::clone(&table))).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(candidate.releases.is_empty());
    let mut expected = [[0.0; 2]; 2];
    let mut actual = [[0.0; 2]; 2];
    reference.render(&mut expected).unwrap();
    candidate.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    assert_ne!(actual[1], [0.0; 2]);
}

#[test]
fn arrangement_instrument_edits_keep_old_occurrence_once_and_use_new_law_afterward() {
    use hex_lattice::NativeInstrumentDefinition;

    let left = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.left"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let right = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.right"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_second_arrangement_note(&project(1.0, 0, 8 << 32));

    for (candidate_type, candidate_state, expected_onset, replace_id) in [
        (left.id, 0, 0.4_f32, false),
        (left.id, 1, 0.6_f32, false),
        (right.id, 0, 0.9_f32, false),
        (left.id, 0, 0.4_f32, true),
    ] {
        let old_project = with_test_instrument(&base, left.id, 0);
        let old_revision =
            revision_with_instrument_registry(&old_project, Arc::clone(&table), &registry);
        let mut previous = Playback::from_prepared(Arc::clone(&old_revision));
        let mut reference = Playback::from_prepared(old_revision);
        assert_eq!(
            previous.session_capability(),
            crate::playback::SessionCapability::Available
        );
        previous.apply(PlaybackCommand::Play);
        reference.apply(PlaybackCommand::Play);
        let mut prefix = [[0.0; 2]; 64];
        let mut reference_prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();
        reference.render(&mut reference_prefix).unwrap();
        assert_eq!(prefix, [[0.2; 2]; 64]);
        assert_eq!(prefix, reference_prefix);

        let changed = with_test_instrument(&base, candidate_type, candidate_state);
        let changed = if replace_id {
            with_replaced_instrument_id(&changed)
        } else {
            changed
        };
        let candidate_revision =
            revision_with_instrument_registry(&changed, Arc::clone(&table), &registry);
        let mut candidate = Playback::from_prepared(candidate_revision);
        candidate.try_inherit_runtime_from(&mut previous).unwrap();

        let mut held = [[0.0; 2]; 64];
        candidate.render(&mut held).unwrap();
        assert_eq!(held, [[0.2; 2]; 64]);
        let mut until_second = vec![[0.0; 2]; 24_000 - 128];
        candidate.render(&mut until_second).unwrap();
        let mut at_second = [[0.0; 2]; 1];
        candidate.render(&mut at_second).unwrap();
        assert_eq!(at_second[0], [expected_onset, expected_onset]);
    }
}

#[test]
fn instrument_edit_suppressions_end_at_the_loop_wrap_across_block_sizes() {
    use hex_lattice::NativeInstrumentDefinition;

    // Pulse #4135: the held note (beats 0..8) outlasts the loop over frames
    // 0..1_200. Two configuration edits before the wrap release the old
    // occurrence and then extend its carried suppression. Neither may outlive
    // the wrap: the next pass is a new onset and sounds under the new law.
    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.edit.loop"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut looped = project(1.0, 0, 8 << 32).snapshot();
    looped.loop_region = Some(LoopRegion::new(0, 1_200, true).unwrap());
    let looped = legacy_project_from_snapshot(looped).unwrap();
    let states = [0, 1, 0];
    let cuts = [301, 601];
    let wrap = 1_200;
    let mut outputs = Vec::new();
    for block in [1, 64, 512] {
        let prepare = |state| {
            SessionPlayback::new(revision_with_instrument_registry(
                &with_test_instrument(&looped, definition.id, state),
                Arc::clone(&table),
                &registry,
            ))
            .unwrap()
        };
        let mut playback = prepare(states[0]);
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let mut output = Vec::new();
        let mut buffer = vec![[0.0; 2]; block];
        for (edit, cut) in cuts.iter().enumerate() {
            while playback.performance_frame() < *cut {
                let width = block.min(usize::try_from(cut - playback.performance_frame()).unwrap());
                playback.render(&mut buffer[..width]).unwrap();
                output.extend_from_slice(&buffer[..width]);
            }
            let mut candidate = prepare(states[edit + 1]);
            candidate.try_inherit_state_from(&mut playback).unwrap();
            assert!(
                candidate
                    .suppressions
                    .iter()
                    .all(|entry| entry.until_frame <= wrap),
                "block={block} edit={edit}: {:?}",
                candidate
                    .suppressions
                    .iter()
                    .map(|entry| entry.until_frame)
                    .collect::<Vec<_>>()
            );
            playback = candidate;
        }
        while playback.performance_frame() < wrap + 64 {
            let width =
                block.min(usize::try_from(wrap + 64 - playback.performance_frame()).unwrap());
            playback.render(&mut buffer[..width]).unwrap();
            output.extend_from_slice(&buffer[..width]);
        }
        // The held occurrence keeps its old law until the wrap, as unedited
        // playback would, and the next pass starts again under the last
        // configuration (the same law here): no doubled or silenced pass.
        let mut reference = prepare(states[0]);
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        let mut expected = vec![[0.0; 2]; output.len()];
        for chunk in expected.chunks_mut(block) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(
            output[usize::try_from(wrap).unwrap()],
            [0.2; 2],
            "block={block}"
        );
        assert_eq!(output, expected, "block={block}");
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], outputs[2]);
}

#[test]
fn retained_arrangement_release_ends_when_a_later_edit_enables_a_loop() {
    use hex_lattice::NativeInstrumentDefinition;
    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.edit.enable_loop"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let unlooped = project(1.0, 0, 8 << 32);
    let mut snapshot = unlooped.snapshot();
    snapshot.loop_region = Some(LoopRegion::new(0, 1_200, true).unwrap());
    let looped = legacy_project_from_snapshot(snapshot).unwrap();
    for (candidate_state, expected) in [(0, 0.2), (1, 0.4)] {
        for block in [1, 64, 512] {
            let prepare = |project: &Project, state| {
                SessionPlayback::new(revision_with_instrument_registry(
                    &with_test_instrument(project, definition.id, state),
                    Arc::clone(&table),
                    &registry,
                ))
                .unwrap()
            };
            let mut old = prepare(&unlooped, 0);
            old.apply(SessionPlaybackCommand::Play).unwrap();
            let mut samples = vec![[0.0; 2]; 301];
            for chunk in samples.chunks_mut(block) {
                old.render(chunk).unwrap();
            }
            let mut changed = prepare(&unlooped, 1);
            changed.try_inherit_state_from(&mut old).unwrap();
            let mut samples = vec![[0.0; 2]; 300];
            for chunk in samples.chunks_mut(block) {
                changed.render(chunk).unwrap();
            }
            let mut next = prepare(&looped, candidate_state);
            next.try_inherit_state_from(&mut changed).unwrap();
            let mut samples = vec![[0.0; 2]; 600];
            for chunk in samples.chunks_mut(block) {
                next.render(chunk).unwrap();
            }
            assert_eq!(
                samples[599], [expected; 2],
                "block={block} state={candidate_state}"
            );
        }
    }
}

#[test]
fn arrangement_handoff_suppresses_through_candidate_release_end() {
    use hex_lattice::NativeInstrumentDefinition;

    let old_definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.short-release",
        ),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let new_definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.long-release",
        ),
        levels: [0.7, 0.9],
        release_frames: 512,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&old_definition, &new_definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 0, 1 << 32);
    let old_project = with_test_instrument(&base, old_definition.id, 0);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &old_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();
    assert_eq!(prefix, [[0.2; 2]; 64]);

    let new_project = with_test_instrument(&base, new_definition.id, 0);
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &new_project,
        table,
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let old_voice_end = 24_000 + old_definition.release_frames;
    let candidate_occurrence_end = 24_000 + new_definition.release_frames;
    assert_eq!(candidate.releases.len(), 1);
    assert_eq!(candidate.releases[0].voice.end_frame(), old_voice_end);
    assert_eq!(candidate.suppressions.len(), 1);
    assert_eq!(
        candidate.suppressions[0].until_frame,
        candidate_occurrence_end
    );

    let start = candidate.performance_frame;
    let mut rest = vec![[0.0; 2]; (candidate_occurrence_end - start) as usize];
    candidate.render(&mut rest).unwrap();
    let after_old_voice = (old_voice_end - start) as usize;
    assert!(
        rest[..after_old_voice]
            .iter()
            .all(|frame| *frame == [0.2; 2])
    );
    assert!(
        rest[after_old_voice..]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
}

#[test]
fn later_arrangement_handoffs_extend_suppression_after_old_voice_ends() {
    use hex_lattice::NativeInstrumentDefinition;

    let short = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.repeat-short",
        ),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let medium = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.repeat-medium",
        ),
        levels: [0.5, 0.6],
        release_frames: 256,
    };
    let long = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.repeat-long",
        ),
        levels: [0.7, 0.9],
        release_frames: 1_024,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 3] = [&short, &medium, &long];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 0, 1 << 32);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, short.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();

    let mut medium_revision = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, medium.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    medium_revision
        .try_inherit_state_from(&mut previous)
        .unwrap();
    let mut until_old_end = vec![[0.0; 2]; 24_020 - 64];
    medium_revision.render(&mut until_old_end).unwrap();
    assert!(medium_revision.releases.is_empty());
    assert_eq!(medium_revision.suppressions[0].until_frame, 24_256);

    let mut long_revision = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, long.id, 0),
        table,
        &registry,
    ))
    .unwrap();
    long_revision
        .try_inherit_state_from(&mut medium_revision)
        .unwrap();
    assert!(long_revision.releases.is_empty());
    assert_eq!(long_revision.suppressions[0].until_frame, 25_024);
    let mut rest = vec![[0.0; 2]; 25_024 - 24_020];
    long_revision.render(&mut rest).unwrap();
    assert!(rest.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn reordered_multitrack_handoff_extends_carried_suppression_after_new_entry() {
    use hex_lattice::NativeInstrumentDefinition;

    let short = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.multitrack-short",
        ),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let medium = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.multitrack-medium",
        ),
        levels: [0.5, 0.6],
        release_frames: 256,
    };
    let long = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static(
            "test.instrument.arrangement.multitrack-long",
        ),
        levels: [0.7, 0.9],
        release_frames: 1_024,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 3] = [&short, &medium, &long];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let make_project = |first_type, second_type, reverse_tracks| {
        let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
        let mut later_track = snapshot.tracks[0].clone();
        later_track.id = 20;
        later_track.name = "later track".into();
        if let Some(instrument) = &mut later_track.instrument {
            instrument.id = 21;
            instrument.state =
                hex_lattice_contract::NativeDeviceState::new(second_type, 1, vec![0]).unwrap();
        }
        let midi_clips = match &mut later_track.content {
            TrackContentSnapshot::Instrument(clips) => Some(clips),
            TrackContentSnapshot::Hybrid { midi_clips, .. } => Some(midi_clips),
            TrackContentSnapshot::Audio(_) => None,
        };
        if let Some(clip) = midi_clips.and_then(|clips| clips.first_mut()) {
            clip.id = 22;
            if let Some(note) = clip.notes.first_mut() {
                note.id = 23;
                note.duration_raw = 1 << 32;
            }
        }
        if let Some(instrument) = &mut snapshot.tracks[0].instrument {
            instrument.state =
                hex_lattice_contract::NativeDeviceState::new(first_type, 1, vec![0]).unwrap();
        }
        snapshot.next_id = 24;
        snapshot.tracks.push(later_track);
        if reverse_tracks {
            snapshot.tracks.reverse();
        }
        legacy_project_from_snapshot(snapshot).unwrap()
    };

    let initial = make_project(short.id, short.id, false);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &initial,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut start = [[0.0; 2]; 64];
    previous.render(&mut start).unwrap();

    // Track 1 is visited first and Track 20 second. This creates one carried
    // suppression for Track 8 while leaving Track 1's longer note unchanged.
    let first_edit = make_project(short.id, medium.id, false);
    let mut middle = SessionPlayback::new(revision_with_instrument_registry(
        &first_edit,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    middle.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(middle.suppressions.len(), 1);
    assert_eq!(middle.suppressions[0].track.value(), 20);
    let mut until_old_release_ends = vec![[0.0; 2]; 24_020 - 64];
    middle.render(&mut until_old_release_ends).unwrap();
    assert!(middle.releases.is_empty());

    // The candidate reorders tracks. The old route order still processes Track
    // 1 first, appending its new suppression after Track 8's carried entry;
    // extending Track 8 must search only the sorted carried prefix.
    let reordered_edit = make_project(medium.id, long.id, true);
    let mut final_revision = SessionPlayback::new(revision_with_instrument_registry(
        &reordered_edit,
        table,
        &registry,
    ))
    .unwrap();
    final_revision.try_inherit_state_from(&mut middle).unwrap();

    let first_suppression = final_revision
        .suppressions
        .iter()
        .find(|entry| entry.track.value() == 1)
        .unwrap();
    let later_suppression = final_revision
        .suppressions
        .iter()
        .find(|entry| entry.track.value() == 20)
        .unwrap();
    assert_eq!(first_suppression.track.value(), 1);
    assert_eq!(later_suppression.track.value(), 20);
    assert_eq!(later_suppression.until_frame, 25_024);
    assert_eq!(final_revision.suppressions.len(), 2);
}

#[test]
fn repeated_arrangement_instrument_edits_do_not_recapture_suppressed_occurrences() {
    use hex_lattice::NativeInstrumentDefinition;

    let first = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.repeat-a"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let second = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.repeat-b"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&first, &second];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 0, 8 << 32);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, first.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first_block = [[0.0; 2]; 64];
    previous.render(&mut first_block).unwrap();

    let mut second_revision = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, second.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    second_revision
        .try_inherit_state_from(&mut previous)
        .unwrap();
    let mut second_block = [[0.0; 2]; 64];
    second_revision.render(&mut second_block).unwrap();

    let mut third_revision = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, first.id, 0),
        table,
        &registry,
    ))
    .unwrap();
    third_revision
        .try_inherit_state_from(&mut second_revision)
        .unwrap();
    assert_eq!(third_revision.releases.len(), 1);
    assert_eq!(
        third_revision.releases[0].timeline,
        ReleaseTimeline::Arrangement
    );
    let mut after_two_edits = [[0.0; 2]; 64];
    third_revision.render(&mut after_two_edits).unwrap();
    assert_eq!(after_two_edits, [[0.2; 2]; 64]);
}

#[test]
fn removing_track_after_arrangement_handoff_keeps_only_the_copied_voice() {
    use hex_lattice::NativeInstrumentDefinition;
    use hex_project::{Command, Editor};

    let first = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.remove-a"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let second = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.remove-b"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&first, &second];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 0, 8 << 32);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, first.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();

    let second_project = with_test_instrument(&base, second.id, 0);
    let mut intermediate = SessionPlayback::new(revision_with_instrument_registry(
        &second_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    intermediate.try_inherit_state_from(&mut previous).unwrap();

    let track = second_project.tracks()[0].id();
    let mut editor = Editor::new(second_project, 8).unwrap();
    editor.execute(Command::RemoveTrack(track)).unwrap();
    let mut removed = SessionPlayback::new(revision_with_instrument_registry(
        editor.project(),
        table,
        &registry,
    ))
    .unwrap();
    removed.try_inherit_state_from(&mut intermediate).unwrap();
    assert_eq!(removed.releases.len(), 1);
    let mut after_removal = [[0.0; 2]; 64];
    removed.render(&mut after_removal).unwrap();
    assert_eq!(after_removal, [[0.2; 2]; 64]);
}

#[test]
fn bypass_after_arrangement_handoff_does_not_recapture_the_old_occurrence() {
    use hex_lattice::NativeInstrumentDefinition;

    let first = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.bypass-a"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let second = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.bypass-b"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&first, &second];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 0, 8 << 32);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &with_test_instrument(&base, first.id, 0),
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();

    let second_project = with_test_instrument(&base, second.id, 0);
    let mut intermediate = SessionPlayback::new(revision_with_instrument_registry(
        &second_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    intermediate.try_inherit_state_from(&mut previous).unwrap();

    let bypassed_project = with_instrument_bypassed(&second_project, true);
    let mut bypassed = SessionPlayback::new(revision_with_instrument_registry(
        &bypassed_project,
        table,
        &registry,
    ))
    .unwrap();
    bypassed.try_inherit_state_from(&mut intermediate).unwrap();
    assert_eq!(bypassed.releases.len(), 1);
    let mut after_bypass = [[0.0; 2]; 64];
    bypassed.render(&mut after_bypass).unwrap();
    assert_eq!(after_bypass, [[0.2; 2]; 64]);
}

#[test]
fn arrangement_instrument_handoff_uses_new_sampler_for_exact_cut_onset() {
    use hex_lattice::NativeInstrumentDefinition;

    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.cut"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = project(1.0, 1 << 32, 1 << 32);
    let old_project = with_test_instrument(&base, definition.id, 0);
    let changed_project = with_test_instrument(&base, definition.id, 1);
    let old_revision =
        revision_with_instrument_registry(&old_project, Arc::clone(&table), &registry);
    let mut previous = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut to_cut = vec![[0.0; 2]; 24_000];
    previous.render(&mut to_cut).unwrap();
    assert!(to_cut.iter().all(|frame| *frame == [0.0, 0.0]));
    assert_eq!(previous.performance_frame(), 24_000);
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &changed_project,
        table,
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(candidate.release_voice_count(), 0);
    let mut at_cut = [[0.0; 2]; 1];
    candidate.render(&mut at_cut).unwrap();
    // The note begins exactly at the publication frame, so it belongs
    // exclusively to the candidate sampler.
    assert_eq!(at_cut[0], [0.4, 0.4]);
}

#[test]
fn mutable_handoff_refuses_pre_cut_note_on_new_track_then_allows_cleared_retry() {
    use hex_lattice::NativeInstrumentDefinition;

    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.new-track.chase"),
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(1.0, 1 << 32, 1 << 32);
    let mut previous = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 2];
    previous.render(&mut prefix).unwrap();
    assert_eq!(previous.performance_frame(), 2);

    let make_candidate = |arrangement_start: u64, session_start: u64| {
        let mut snapshot = original.snapshot();
        let old_track = snapshot.tracks[0].id;
        snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
        snapshot.tracks[0].instrument = None;
        snapshot.tracks[0].instrument_position = None;
        snapshot
            .session
            .cells
            .retain(|cell| cell.track != old_track);
        let track = 20;
        let instrument_id = 21;
        let arrangement_clip = 22;
        let arrangement_note = 23;
        let session_clip = 24;
        let session_note = 25;
        snapshot.tracks.push(TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track,
            name: "new mutable track".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: Some(DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: instrument_id,
                bypassed: false,
                state: hex_lattice_contract::NativeDeviceState::new(
                    definition.metadata.id,
                    1,
                    vec![0],
                )
                .unwrap(),
            }),
            instrument_position: Some(0),
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                content_view: None,
                active: true,
                playback_offset_raw: 0,
                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                volume: hex_project::MidiClipVolume::default(),
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                id: arrangement_clip,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 32 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 32 << 32,
                looping: false,
                notes: vec![MidiNoteSnapshot {
                    active: true,
                    id: arrangement_note,
                    start_raw: arrangement_start,
                    duration_raw: 1 << 32,
                    pitch: 64,
                    channel: 0,
                    velocity: 100,
                    expression: Default::default(),
                    expression_points: Vec::new(),
                }],
                expression_lanes: Vec::new(),
            }]),
            sends: Vec::new(),
        });
        snapshot.session.cells.push(SessionCellSnapshot {
            track,
            scene: snapshot.session.scenes[0].id,
            content: SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                active: true,
                playback_offset_raw: 0,
                volume: hex_project::MidiClipVolume::default(),
                flavor: hex_project::ClipFlavor::default(),
                id: session_clip,
                name: "new mutable clip".into(),
                source: MidiSourceSnapshot {
                    one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                    source_length_raw: 32 << 32,
                    source_start_raw: 0,
                    loop_start_raw: 0,
                    loop_end_raw: 32 << 32,
                    looping: false,
                    notes: vec![MidiNoteSnapshot {
                        active: true,
                        id: session_note,
                        start_raw: session_start,
                        duration_raw: 1 << 32,
                        pitch: 64,
                        channel: 0,
                        velocity: 100,
                        expression: Default::default(),
                        expression_points: Vec::new(),
                    }],
                    expression_lanes: Vec::new(),
                },
            }),
        });
        snapshot.next_id = 26;
        legacy_project_from_snapshot(snapshot).unwrap()
    };

    let sounding = make_candidate(0, 0);
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &sounding,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    assert_eq!(candidate.routes.len(), 2);
    assert!(candidate.mutable_instruments[1].is_some());
    let mut candidate_span_active = false;
    candidate.revision.arrangement().tracks()[1]
        .for_each_mutable_midi_span(2, 3, |span| {
            candidate_span_active |= span.start < 2 && span.end > 2;
        })
        .unwrap();
    assert!(candidate_span_active);
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::MutableInstrumentChase)
    );
    assert_eq!(previous.performance_frame(), 2);
    let old_track = previous.routes[0].id;
    assert_eq!(
        previous.owner(old_track),
        Some(SessionPlaybackOwner::Arrangement)
    );

    let cleared = make_candidate(1 << 32, 1 << 32);
    let mut retry = SessionPlayback::new(revision_with_instrument_registry(
        &cleared, table, &registry,
    ))
    .unwrap();
    retry.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(retry.performance_frame(), 2);
    assert_eq!(retry.release_voice_count(), 0);
}

#[test]
fn mutable_handoff_refuses_inherited_future_analytic_route_boundary() {
    use hex_lattice::NativeInstrumentDefinition;

    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.pending-boundary"),
            levels: [0.2, 0.4],
            release_frames: 8,
        },
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project(1.0, 1 << 32, 1 << 32);
    let mut previous = SessionPlayback::new(revision(&original, Arc::clone(&table))).unwrap();
    let clip = previous.revision.session().midi_clips()[0].id;
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 1];
    previous.render(&mut prefix).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let pending = previous.routes[0].pending;
    assert!(pending.is_some_and(|entry| entry.frame > previous.performance_frame));

    let changed = with_test_instrument(&original, definition.metadata.id, 0);
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &changed,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous),
        Err(SessionPlaybackError::MutableInstrumentRouteBoundary)
    );
    assert_eq!(previous.performance_frame(), 1);
    assert_eq!(previous.routes[0].pending, pending);
    let track = previous.routes[0].id;
    assert_eq!(
        previous.owner(track),
        Some(SessionPlaybackOwner::Arrangement)
    );

    previous
        .apply(SessionPlaybackCommand::CancelPending { track: Some(track) })
        .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(candidate.performance_frame(), 1);
}

#[test]
fn arrangement_instrument_bypass_releases_old_voice_and_reenable_does_not_chase() {
    use hex_lattice::NativeInstrumentDefinition;

    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.arrangement.bypass"),
        levels: [0.2, 0.4],
        release_frames: 600,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_second_arrangement_note(&project(1.0, 0, 8 << 32));
    let enabled = with_test_instrument(&base, definition.id, 0);
    let mut bypassed_snapshot = enabled.snapshot();
    bypassed_snapshot.tracks[0]
        .instrument
        .as_mut()
        .unwrap()
        .bypassed = true;
    let bypassed = Project::from_legacy_snapshot(bypassed_snapshot).unwrap();

    let old_revision = revision_with_instrument_registry(&enabled, Arc::clone(&table), &registry);
    let mut previous = Playback::from_prepared(old_revision);
    previous.apply(PlaybackCommand::Play);
    let mut audible_prefix = [[0.0; 2]; 64];
    previous.render(&mut audible_prefix).unwrap();
    assert_eq!(audible_prefix, [[0.2; 2]; 64]);
    let candidate_revision =
        revision_with_instrument_registry(&bypassed, Arc::clone(&table), &registry);
    let mut candidate = Playback::from_prepared(candidate_revision);
    candidate.try_inherit_runtime_from(&mut previous).unwrap();
    let mut release = [[0.0; 2]; 320];
    candidate.render(&mut release).unwrap();
    assert_eq!(release[0], [0.2; 2]);
    assert!(
        release[240..].iter().all(|frame| *frame == [0.0; 2]),
        "arrangement power-off exceeded 5 ms"
    );
    assert_eq!(release[120], [0.1; 2]);

    let disabled_revision =
        revision_with_instrument_registry(&bypassed, Arc::clone(&table), &registry);
    let mut disabled = Playback::from_prepared(disabled_revision);
    disabled.apply(PlaybackCommand::Play);
    let mut silent_prefix = [[0.0; 2]; 64];
    disabled.render(&mut silent_prefix).unwrap();
    assert_eq!(silent_prefix, [[0.0; 2]; 64]);
    let enabled_revision = revision_with_instrument_registry(&enabled, table, &registry);
    let mut reenabled = Playback::from_prepared(enabled_revision);
    reenabled.try_inherit_runtime_from(&mut disabled).unwrap();
    let mut no_chase = vec![[0.0; 2]; 24_000 - 64];
    reenabled.render(&mut no_chase).unwrap();
    assert!(no_chase.iter().all(|frame| *frame == [0.0; 2]));
    let mut future_onset = [[0.0; 2]; 1];
    reenabled.render(&mut future_onset).unwrap();
    assert_eq!(future_onset[0], [0.2; 2]);
}

#[test]
fn reenabled_instrument_suppression_ends_at_the_loop_wrap_across_block_sizes() {
    use hex_lattice::NativeInstrumentDefinition;

    // Pulse #4124: the held note (beats 0..8) outlasts the loop over frames
    // 0..12_000. Re-enabling must not chase it, but the next pass sounds.
    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.reenable.loop"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut looped = project(1.0, 0, 8 << 32).snapshot();
    looped.loop_region = Some(LoopRegion::new(0, 12_000, true).unwrap());
    let enabled = with_test_instrument(
        &legacy_project_from_snapshot(looped).unwrap(),
        definition.id,
        0,
    );
    let mut bypassed_snapshot = enabled.snapshot();
    bypassed_snapshot.tracks[0]
        .instrument
        .as_mut()
        .unwrap()
        .bypassed = true;
    let bypassed = Project::from_legacy_snapshot(bypassed_snapshot).unwrap();
    let cut = 3_001;
    let mut outputs = Vec::new();
    for block in [1, 64, 512] {
        let mut disabled = SessionPlayback::new(revision_with_instrument_registry(
            &bypassed,
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        disabled.apply(SessionPlaybackCommand::Play).unwrap();
        let mut buffer = vec![[0.0; 2]; block];
        let mut remaining = cut;
        while remaining > 0 {
            let width = block.min(remaining);
            disabled.render(&mut buffer[..width]).unwrap();
            remaining -= width;
        }
        let mut reenabled = SessionPlayback::new(revision_with_instrument_registry(
            &enabled,
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        reenabled.try_inherit_state_from(&mut disabled).unwrap();
        let mut output = vec![[0.0; 2]; 12_000 - cut + 64];
        for chunk in output.chunks_mut(block) {
            reenabled.render(chunk).unwrap();
        }
        // No chase before the wrap; the next pass starts the note again.
        assert!(
            output[..12_000 - cut]
                .iter()
                .all(|frame| *frame == [0.0; 2])
        );
        assert_eq!(output[12_000 - cut], [0.2; 2], "block={block}");
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(outputs[0], outputs[2]);
}

#[test]
fn arrangement_instrument_release_capacity_refusal_is_atomic_and_retryable() {
    use hex_lattice::NativeInstrumentDefinition;

    let left = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.capacity.old"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let right = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.capacity.new"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = with_second_arrangement_note(&project(1.0, 0, 8 << 32));
    let old_project = with_test_instrument(&base, left.id, 0);
    let changed_project = with_test_instrument(&base, right.id, 0);
    let mut old = SessionPlayback::new(revision_with_instrument_registry(
        &old_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    let mut old_reference = SessionPlayback::new(revision_with_instrument_registry(
        &old_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    old_reference.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    old.render(&mut prefix).unwrap();
    old_reference.render(&mut reference_prefix).unwrap();
    assert_eq!(prefix, reference_prefix);

    let changed_revision =
        revision_with_instrument_registry(&changed_project, Arc::clone(&table), &registry);
    let mut retry = SessionPlayback::new(Arc::clone(&changed_revision)).unwrap();
    let mut success = SessionPlayback::new(changed_revision).unwrap();
    retry.release_capacity = 0;
    assert_eq!(
        retry.try_inherit_state_from(&mut old),
        Err(SessionPlaybackError::ReleaseBankFull)
    );
    assert_eq!(old.performance_frame(), 64);
    assert_eq!(old.arrangement_frame(), 64);
    assert_eq!(old.release_voice_count(), 0);
    assert_eq!(old.suppressions.len(), 0);
    assert_eq!(retry.performance_frame(), 0);
    assert_eq!(retry.arrangement_frame(), 0);
    assert_eq!(retry.release_voice_count(), 0);
    assert_eq!(retry.suppressions.len(), 0);

    retry.release_capacity = 1;
    retry.try_inherit_state_from(&mut old).unwrap();
    success.try_inherit_state_from(&mut old_reference).unwrap();
    let mut retry_output = [[0.0; 2]; 64];
    let mut success_output = [[0.0; 2]; 64];
    retry.render(&mut retry_output).unwrap();
    success.render(&mut success_output).unwrap();
    assert_eq!(retry_output, success_output);
}

#[test]
fn session_state_and_type_changes_keep_old_voice_law_and_use_new_law_for_later_onsets() {
    use hex_lattice::NativeInstrumentDefinition;

    let left = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.continuity.left"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let right = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.continuity.right"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());

    for (candidate_type, candidate_state, expected_future) in
        [(left.id, 1, 0.3_f32), (right.id, 0, 0.45_f32)]
    {
        let base = with_second_session_note(&session_only_project(&project(0.5, 0, 8 << 32)));
        let old_project = with_test_instrument(&base, left.id, 0);
        let old_revision =
            revision_with_instrument_registry(&old_project, Arc::clone(&table), &registry);
        let clip = old_revision.session().midi_clips()[0].id;
        let mut previous = SessionPlayback::new(old_revision).unwrap();
        previous
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 64];
        previous.render(&mut prefix).unwrap();
        assert!(prefix.iter().all(|frame| *frame == [0.1, 0.1]));

        let changed = with_test_instrument(&base, candidate_type, candidate_state);
        let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
            &changed,
            Arc::clone(&table),
            &registry,
        ))
        .unwrap();
        candidate.try_inherit_state_from(&mut previous).unwrap();
        assert!(candidate.releases.iter().any(|entry| {
            entry.track_id == candidate.routes[0].id && entry.continuation.is_some()
        }));

        let mut immediate = [[0.0; 2]; 8];
        candidate.render(&mut immediate).unwrap();
        assert!(immediate.iter().all(|frame| *frame == [0.1, 0.1]));

        // The first onset under the candidate configuration is at beat one
        // (frame 24,000). It adds its new law while the captured old note keeps
        // the amplitude and gate authored by the previous revision.
        let mut until_future_note = vec![[0.0; 2]; 24_000 - 72];
        candidate.render(&mut until_future_note).unwrap();
        assert!(until_future_note.iter().all(|frame| *frame == [0.1, 0.1]));
        let mut at_future_note = [[0.0; 2]; 1];
        candidate.render(&mut at_future_note).unwrap();
        assert_eq!(at_future_note[0], [expected_future, expected_future]);
    }
}

#[test]
fn changed_instrument_does_not_rewrite_an_already_releasing_session_voice() {
    use hex_lattice::NativeInstrumentDefinition;

    let left = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.release.left"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let right = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.release.right"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let original =
        with_test_instrument(&session_only_project(&project(0.5, 0, 1 << 32)), left.id, 0);
    let old_revision = revision_with_instrument_registry(&original, Arc::clone(&table), &registry);
    let clip = old_revision.session().midi_clips()[0].id;
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut before_release = vec![[0.0; 2]; 24_004];
    previous.render(&mut before_release).unwrap();

    let changed = with_test_instrument(&original, right.id, 0);
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &changed,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    let release = candidate
        .releases
        .iter()
        .find(|entry| entry.continuation.is_some())
        .expect("matching candidate preserves suppression for the old release");
    assert_eq!(release.voice.gate_end_frame(), 24_000);
    assert_eq!(release.voice.end_frame(), 24_008);
    assert_eq!(candidate.suppressions.len(), 1);

    let mut tail = [[0.0; 2]; 8];
    candidate.render(&mut tail).unwrap();
    assert_eq!(&tail[..4], &[[0.1, 0.1]; 4]);
    assert_eq!(&tail[4..], &[[0.0, 0.0]; 4]);
}

#[test]
fn instrument_edit_captures_only_the_changed_session_track() {
    use hex_lattice::NativeInstrumentDefinition;
    use hex_project::{Command, Created};

    let left = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.route.left"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let right = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.route.right"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let first = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "first".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("first MIDI track"),
    };
    let second = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "second".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("second MIDI track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(scene) => scene,
        _ => panic!("Session scene"),
    };
    for (track, pitch) in [(first, 60), (second, 64)] {
        let clip = match editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "source".into(),
                source_length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        {
            Created::SessionClip(clip) => clip,
            _ => panic!("Session MIDI clip"),
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(4).unwrap(),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    for track in [first, second] {
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: registry.default_state(left.id).unwrap(),
            })
            .unwrap();
    }
    let old_project = editor.project().clone();
    let old_revision =
        revision_with_instrument_registry(&old_project, Arc::clone(&table), &registry);
    let first_clip = old_revision
        .session()
        .midi_clips()
        .iter()
        .find(|clip| clip.track == first)
        .unwrap()
        .id;
    let mut previous = SessionPlayback::new(old_revision).unwrap();
    previous
        .apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    previous.render(&mut prefix).unwrap();
    assert!(prefix.iter().all(|frame| *frame == [0.4, 0.4]));

    let first_device = editor
        .project()
        .track(first)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: first_device,
            state: hex_lattice_contract::NativeDeviceState::new(left.id, 1, vec![1]).unwrap(),
        })
        .unwrap();
    let changed = editor.project().clone();
    let mut candidate = SessionPlayback::new(revision_with_instrument_registry(
        &changed,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert!(
        candidate
            .releases
            .iter()
            .any(|entry| { entry.track_id == first && entry.continuation.is_some() })
    );
    assert!(
        !candidate
            .releases
            .iter()
            .any(|entry| entry.track_id == second)
    );
    assert!(
        candidate
            .suppressions
            .iter()
            .all(|entry| entry.track == first)
    );
    assert!(candidate.revision.session().midi_clip(first_clip).is_some());
    let first_route_index = candidate
        .routes
        .iter()
        .position(|route| route.id == first)
        .unwrap();
    let second_route_index = candidate
        .routes
        .iter()
        .position(|route| route.id == second)
        .unwrap();
    assert!(candidate.route_historical_floors[first_route_index].is_some());
    assert_eq!(candidate.route_historical_floors[second_route_index], None);
    let mut output = [[0.0; 2]; 64];
    candidate.render(&mut output).unwrap();
    assert!(
        output.iter().all(|frame| *frame == [0.4, 0.4]),
        "unrelated route changed across instrument edit: {:?}",
        (
            &output[..4],
            &candidate.releases,
            &candidate.suppressions,
            &candidate.routes
        )
    );

    let mut source_snapshot = old_project.snapshot();
    source_snapshot
        .tracks
        .iter_mut()
        .find(|track| track.id == first.value())
        .unwrap()
        .muted = true;
    let muted_source = legacy_project_from_snapshot(source_snapshot).unwrap();
    let mut changed_source_snapshot = muted_source.snapshot();
    let first_cell = changed_source_snapshot
        .session
        .cells
        .iter_mut()
        .find(|cell| cell.track == first.value())
        .unwrap();
    let SessionCellContentSnapshot::Midi(first_source) = &mut first_cell.content else {
        panic!("first route has Session MIDI")
    };
    first_source.source.notes[0].duration_raw = 2 << 32;
    let changed_source = legacy_project_from_snapshot(changed_source_snapshot).unwrap();
    let mut source_previous = SessionPlayback::new(revision_with_instrument_registry(
        &muted_source,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    let mut source_reference = SessionPlayback::new(revision_with_instrument_registry(
        &muted_source,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    for playback in [&mut source_previous, &mut source_reference] {
        playback
            .apply(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut source_prefix = [[0.0; 2]; 64];
    let mut source_reference_prefix = [[0.0; 2]; 64];
    source_previous.render(&mut source_prefix).unwrap();
    source_reference
        .render(&mut source_reference_prefix)
        .unwrap();
    assert_eq!(source_prefix, source_reference_prefix);
    assert!(source_prefix.iter().all(|frame| *frame == [0.2, 0.2]));
    let mut source_candidate = SessionPlayback::new(revision_with_instrument_registry(
        &changed_source,
        table,
        &registry,
    ))
    .unwrap();
    source_candidate
        .try_inherit_state_from(&mut source_previous)
        .unwrap();
    assert!(
        source_candidate
            .releases
            .iter()
            .any(|entry| { entry.track_id == second && entry.continuation.is_some() })
    );
    let mut source_actual = [[0.0; 2]; 64];
    let mut source_expected = [[0.0; 2]; 64];
    source_candidate.render(&mut source_actual).unwrap();
    source_reference.render(&mut source_expected).unwrap();
    assert_eq!(source_actual, source_expected);
}
