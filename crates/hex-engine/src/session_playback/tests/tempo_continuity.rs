//! A running base-tempo edit keeps the Arrangement cursor on the same beat
//! (Pulse #3935): the cut frame is remapped through the old and new maps.
use super::*;

const BEAT_FRAMES_AT_120: u64 = 24_000;

fn arrangement_beat(runtime: &SessionPlayback) -> BeatTime {
    base_timing_for(runtime.revision.session(), runtime.periodic_tempo())
        .frame_to_beat(runtime.arrangement_frame())
        .unwrap()
}

/// One held Arrangement note over beats 0..8 and an optional loop in frames.
fn arrangement_at(micro_bpm: u32, loop_region: Option<(u64, u64)>) -> Project {
    let tempo = Tempo::new(micro_bpm).unwrap();
    let mut snapshot = project_at_tempo(0.5, 0, 8 << 32, tempo).snapshot();
    snapshot.loop_region =
        loop_region.map(|(start, end)| LoopRegion::new(start, end, true).unwrap());
    legacy_project_from_snapshot(snapshot).unwrap()
}

/// A loop over beats 0..16, placed for `micro_bpm` as a tempo edit retimes it.
fn beat_loop(micro_bpm: u32) -> Option<(u64, u64)> {
    Some((
        0,
        16 * BEAT_FRAMES_AT_120 * 120_000_000 / u64::from(micro_bpm),
    ))
}

fn render_frames(runtime: &mut SessionPlayback, block: usize, mut frames: usize) {
    let mut buffer = vec![[0.0; 2]; block];
    while frames > 0 {
        let width = block.min(frames);
        runtime.render(&mut buffer[..width]).unwrap();
        frames -= width;
    }
}

#[test]
fn running_tempo_edit_keeps_arrangement_beat_continuous_across_block_sizes() {
    let table = Arc::new(hex_devices::SineTable::new());
    // A cut that is not aligned to any tested block size.
    let cut = 3_001;
    for looped in [false, true] {
        for block in [1, 64, 512] {
            let region = |micro_bpm| looped.then(|| beat_loop(micro_bpm)).flatten();
            let old_project = arrangement_at(120_000_000, region(120_000_000));
            let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            render_frames(&mut old, block, cut);
            assert_eq!(old.arrangement_frame(), cut as u64);
            let beat = arrangement_beat(&old);
            let performance = old.performance_frame();

            let slower = revision(
                &arrangement_at(60_000_000, region(60_000_000)),
                Arc::clone(&table),
            );
            let mut next = SessionPlayback::new(slower).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            assert!(next.playing(), "looped={looped} block={block}");
            assert_eq!(next.performance_frame(), performance);
            // Half the tempo: the same beat lies at twice the frame.
            assert_eq!(
                next.arrangement_frame(),
                2 * cut as u64,
                "looped={looped} block={block}"
            );
            assert_eq!(
                arrangement_beat(&next),
                beat,
                "looped={looped} block={block}"
            );

            // Playback continues forward from that beat at the new tempo.
            render_frames(&mut next, block, 1_000);
            assert_eq!(next.arrangement_frame(), 2 * cut as u64 + 1_000);
        }
    }
}

#[test]
fn remapped_cursor_past_an_unretimed_loop_end_wraps_inside_the_loop() {
    let table = Arc::new(hex_devices::SineTable::new());
    // The loop keeps frames 0..4_800; beat 0.1875 at 60 bpm lies past its end.
    let region = Some((0, 4_800));
    let mut old = SessionPlayback::new(revision(
        &arrangement_at(120_000_000, region),
        Arc::clone(&table),
    ))
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    render_frames(&mut old, 64, 4_500);
    let pass = old.arrangement_loop_pass;
    let mut next =
        SessionPlayback::new(revision(&arrangement_at(60_000_000, region), table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(next.arrangement_frame(), 9_000 - 4_800);
    assert_eq!(next.arrangement_loop_pass, pass + 1);
    assert!(next.active_loop_region().is_some());
}

#[test]
fn held_note_across_a_remapped_cut_is_released_once_without_a_new_onset() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut old = SessionPlayback::new(revision(
        &arrangement_at(120_000_000, None),
        Arc::clone(&table),
    ))
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    render_frames(&mut old, 64, 3_001);
    assert!(old.releases.is_empty());
    let mut next =
        SessionPlayback::new(revision(&arrangement_at(60_000_000, None), table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    // The note held over beats 0..8 is released at the cut exactly once,
    // and its remapped occurrence is suppressed rather than chased.
    assert_eq!(next.releases.len(), 1);
    let release_end = next.releases[0].end_frame();
    assert!(release_end > next.performance_frame());
    assert_eq!(next.suppressions.len(), 1);
    assert!(next.suppressions[0].until_frame > release_end);
    let mut tail = vec![[0.0; 2]; (release_end - next.performance_frame()) as usize];
    next.render(&mut tail).unwrap();
    assert!(tail.iter().any(|frame| *frame != [0.0; 2]));
    // After the release tail, the still-active remapped span stays silent.
    let mut after = vec![[0.0; 2]; 4_096];
    next.render(&mut after).unwrap();
    assert!(after.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn remapped_suppression_ends_at_the_loop_wrap_so_the_next_pass_sounds() {
    let table = Arc::new(hex_devices::SineTable::new());
    // The held note (beats 0..8) outlasts the loop over frames 0..48_000.
    let region = Some((0, 48_000));
    let mut old = SessionPlayback::new(revision(
        &arrangement_at(120_000_000, region),
        Arc::clone(&table),
    ))
    .unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    render_frames(&mut old, 512, 3_001);
    let mut next =
        SessionPlayback::new(revision(&arrangement_at(60_000_000, region), table)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(next.arrangement_frame(), 6_002);
    let wrap = next.performance_frame() + (48_000 - 6_002);
    assert_eq!(next.suppressions.len(), 1);
    assert_eq!(next.suppressions[0].until_frame, wrap);
    // The next loop pass starts the note again.
    render_frames(&mut next, 512, (48_000 - 6_002) as usize);
    let mut pass = [[0.0; 2]; 256];
    next.render(&mut pass).unwrap();
    assert!(pass.iter().any(|frame| *frame != [0.0; 2]));
}

#[test]
fn remapped_release_of_a_note_longer_than_the_loop_ends_at_the_wrap() {
    let table = Arc::new(hex_devices::SineTable::new());
    // Pulse #4135: the held note (beats 0..8) outlasts the loop over frames
    // 0..4_800. The cut remaps to 10 frames before the candidate wrap, so the
    // released donor voice's 2_400-frame tail would sound over the next pass.
    let region = Some((0, 4_800));
    let candidate = arrangement_at(60_000_000, region);
    // A fresh pass from the loop start: what the next pass must sound like.
    let mut fresh = SessionPlayback::new(revision(&candidate, Arc::clone(&table))).unwrap();
    fresh.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = vec![[0.0; 2]; 2_560];
    fresh.render(&mut expected).unwrap();
    // Control: unedited looped playback ends the occurrence at the wrap.
    let mut unedited = SessionPlayback::new(revision(&candidate, Arc::clone(&table))).unwrap();
    unedited.apply(SessionPlaybackCommand::Play).unwrap();
    render_frames(&mut unedited, 512, 4_800);
    let mut wrapped = vec![[0.0; 2]; 2_560];
    unedited.render(&mut wrapped).unwrap();
    assert_eq!(wrapped, expected, "unedited wrap");
    for block in [1, 64, 512] {
        let mut old = SessionPlayback::new(revision(
            &arrangement_at(120_000_000, region),
            Arc::clone(&table),
        ))
        .unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        render_frames(&mut old, block, 2_395);
        let mut next = SessionPlayback::new(revision(&candidate, Arc::clone(&table))).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        assert_eq!(next.arrangement_frame(), 4_790, "block={block}");
        let wrap = next.performance_frame() + 10;
        render_frames(&mut next, block, 10);
        assert_eq!(next.performance_frame(), wrap);
        let mut after = vec![[0.0; 2]; 2_560];
        for chunk in after.chunks_mut(block) {
            next.render(chunk).unwrap();
        }
        assert_eq!(after, expected, "block={block}");
    }
}

#[test]
fn remapped_release_follows_only_the_candidate_loop_when_one_side_loops() {
    // Pulse #4135: the wrap that bounds a released donor voice is the
    // candidate's. A donor-only loop leaves the full tail; a candidate-only
    // loop ends the tail at the candidate wrap so its next pass sounds alone.
    let table = Arc::new(hex_devices::SineTable::new());
    let region = Some((0, 4_800));
    let release = |donor, candidate, block| {
        let mut old = SessionPlayback::new(revision(
            &arrangement_at(120_000_000, donor),
            Arc::clone(&table),
        ))
        .unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        render_frames(&mut old, block, 2_395);
        let mut next = SessionPlayback::new(revision(
            &arrangement_at(60_000_000, candidate),
            Arc::clone(&table),
        ))
        .unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        assert_eq!(next.arrangement_frame(), 4_790, "block={block}");
        assert_eq!(next.releases.len(), 1, "block={block}");
        let tail = next.releases[0].end_frame() - next.performance_frame();
        (next, tail)
    };
    for block in [1, 64, 512] {
        let (donor_only, full_tail) = release(region, None, block);
        assert!(donor_only.active_loop_region().is_none());
        assert!(full_tail > 10, "block={block}");
        let (_, bounded_tail) = release(None, region, block);
        assert_eq!(bounded_tail, 10, "block={block}");
    }
}

#[test]
fn paused_restart_relocates_to_the_same_arrangement_beat() {
    let table = Arc::new(hex_devices::SineTable::new());
    let cut = 3_001;
    for looped in [false, true] {
        for block in [1, 64, 512] {
            let region = |micro_bpm| looped.then(|| beat_loop(micro_bpm)).flatten();
            let old_project = arrangement_at(120_000_000, region(120_000_000));
            let mut old = SessionPlayback::new(revision(&old_project, Arc::clone(&table))).unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            render_frames(&mut old, block, cut);
            old.apply(SessionPlaybackCommand::Pause).unwrap();
            let beat = arrangement_beat(&old);
            let slower = revision(
                &arrangement_at(60_000_000, region(60_000_000)),
                Arc::clone(&table),
            );
            let mut next = SessionPlayback::new(slower).unwrap();
            // The live reset handoff installs the paused cursor directly.
            next.request_live_restart();
            next.try_inherit_state_live_from(&mut old).unwrap();
            assert!(!next.playing(), "looped={looped} block={block}");
            assert_eq!(
                next.arrangement_frame(),
                2 * cut as u64,
                "looped={looped} block={block}"
            );
            assert_eq!(arrangement_beat(&next), beat);
        }
    }
}

fn render_linear(playback: &mut crate::playback::Playback, block: usize, mut frames: usize) {
    let mut buffer = vec![[0.0; 2]; block];
    while frames > 0 {
        let width = block.min(frames);
        playback.render(&mut buffer[..width]).unwrap();
        frames -= width;
    }
}

fn linear_at(project: &Project, table: &Arc<hex_devices::SineTable>) -> crate::playback::Playback {
    let revision = revision(project, Arc::clone(table));
    crate::playback::Playback::from_shared(Arc::clone(revision.arrangement()))
}

#[test]
fn legacy_linear_tempo_edit_keeps_the_arrangement_beat() {
    let table = Arc::new(hex_devices::SineTable::new());
    let cut = 3_001;
    for looped in [false, true] {
        for block in [1, 64, 512] {
            let region = |micro_bpm| looped.then(|| beat_loop(micro_bpm)).flatten();
            let mut old = linear_at(&arrangement_at(120_000_000, region(120_000_000)), &table);
            old.apply(PlaybackCommand::Play);
            render_linear(&mut old, block, cut);
            assert_eq!(old.position(), cut as u64);
            let mut next = linear_at(&arrangement_at(60_000_000, region(60_000_000)), &table);
            next.try_inherit_runtime_from(&mut old).unwrap();
            assert!(next.playing(), "looped={looped} block={block}");
            assert_eq!(
                next.position(),
                2 * cut as u64,
                "looped={looped} block={block}"
            );
            render_linear(&mut next, block, 1_000);
            assert_eq!(next.position(), 2 * cut as u64 + 1_000);
        }
    }
    // A cursor remapped past an unretimed loop end wraps inside the loop.
    let region = Some((0, 4_800));
    let mut old = linear_at(&arrangement_at(120_000_000, region), &table);
    old.apply(PlaybackCommand::Play);
    render_linear(&mut old, 64, 4_500);
    let mut next = linear_at(&arrangement_at(60_000_000, region), &table);
    next.try_inherit_runtime_from(&mut old).unwrap();
    assert_eq!(next.position(), 9_000 - 4_800);
}

#[test]
fn legacy_linear_held_note_continues_without_retrigger_and_ends_at_its_new_gate() {
    let table = Arc::new(hex_devices::SineTable::new());
    // One note over beats 0..1: frames 0..24_000 at 120 bpm, 0..48_000 at 60.
    let note = |micro_bpm| project_at_tempo(0.5, 0, 1 << 32, Tempo::new(micro_bpm).unwrap());
    let mut old = linear_at(&note(120_000_000), &table);
    old.apply(PlaybackCommand::Play);
    render_linear(&mut old, 64, 3_001);
    let mut next = linear_at(&note(60_000_000), &table);
    next.try_inherit_runtime_from(&mut old).unwrap();
    assert_eq!(next.position(), 6_002);
    // The held note keeps sounding from its remapped position (no gap)...
    let mut held = vec![[0.0; 2]; 48_000 - 6_002];
    next.render(&mut held).unwrap();
    assert!(held[..64].iter().all(|frame| *frame != [0.0; 2]));
    // ...releases at the new gate (beat 1) and does not sound again.
    let mut after = vec![[0.0; 2]; 48_000];
    next.render(&mut after).unwrap();
    assert!(
        after[after.len() - 4_096..]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
}

#[test]
fn tempo_remap_with_instrument_reenable_releases_suppression_at_new_loop_end() {
    use hex_lattice::NativeInstrumentDefinition;

    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.tempo.reenable"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    snapshot.loop_region = Some(LoopRegion::new(0, 12_000, true).unwrap());
    let enabled = with_test_instrument(
        &legacy_project_from_snapshot(snapshot).unwrap(),
        definition.id,
        0,
    );
    let mut disabled_snapshot = enabled.snapshot();
    disabled_snapshot.tracks[0]
        .instrument
        .as_mut()
        .unwrap()
        .bypassed = true;
    let disabled = Project::from_legacy_snapshot(disabled_snapshot).unwrap();
    let mut edited = Editor::new(enabled, 8).unwrap();
    edited
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();

    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &disabled,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    render_frames(&mut previous, 64, 3_001);
    let mut next = SessionPlayback::new(revision_with_instrument_registry(
        edited.project(),
        table,
        &registry,
    ))
    .unwrap();
    next.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(next.arrangement_frame(), 6_002);
    let until_wrap = 24_000 - 6_002;
    let mut output = vec![[0.0; 2]; until_wrap + 64];
    for chunk in output.chunks_mut(64) {
        next.render(chunk).unwrap();
    }
    assert!(output[..until_wrap].iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(output[until_wrap], [0.2; 2]);
}

/// Pulse #4117: a running base-tempo edit releases a held native mutable
/// Arrangement voice at the donor's cut (NoteOff at the first candidate
/// frame), keeps the beat, and never chases or retriggers the cut note. A
/// voice already releasing keeps its tail exactly.
#[test]
fn tempo_remap_releases_held_mutable_instrument_at_the_cut_across_block_sizes() {
    use hex_lattice::NativeInstrumentDefinition;

    const RELEASE: u64 = 128;
    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.tempo.release"),
            levels: [0.2, 0.4],
            release_frames: RELEASE,
        },
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    // The note ends at 48_000 frames after the edit; render well past it.
    let horizon = 50_000;
    for looped in [false, true] {
        // One mutable note over beats 0..1 (24_000 frames at 120 bpm), with
        // an optional loop over beats 0..16 that the edit retimes.
        let mut snapshot = project(1.0, 0, 1 << 32).snapshot();
        snapshot.loop_region = looped
            .then(|| beat_loop(120_000_000))
            .flatten()
            .map(|(start, end)| LoopRegion::new(start, end, true).unwrap());
        let old_project = with_test_instrument(
            &legacy_project_from_snapshot(snapshot).unwrap(),
            definition.metadata.id,
            0,
        );
        let mut edited = Editor::new(old_project.clone(), 8).unwrap();
        edited
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        for (block, cut, released, live) in [64, 512].into_iter().flat_map(|block| {
            [(3_001_u64, false), (24_001, true)]
                .into_iter()
                .flat_map(move |(cut, released)| {
                    [false, true].map(|live| (block, cut, released, live))
                })
        }) {
            {
                let context = format!("looped={looped} block={block} cut={cut} live={live}");
                let mut previous = ordinary_mutable_playback(&old_project, &registry);
                let mut reference = ordinary_mutable_playback(&old_project, &registry);
                for playback in [&mut previous, &mut reference] {
                    playback.apply(SessionPlaybackCommand::Play).unwrap();
                    render_frames(playback, block, cut as usize);
                }
                let proofs = |playback: &SessionPlayback| {
                    playback.mutable_instruments[0]
                        .as_ref()
                        .unwrap()
                        .proofs()
                        .collect::<Vec<_>>()
                };
                let before = proofs(&previous);
                assert_eq!(before.len(), 1, "{context}");
                assert_eq!(before[0].note_off_sent, released, "{context}");

                let mut next = ordinary_mutable_playback(edited.project(), &registry);
                if live {
                    next.try_inherit_state_live_from(&mut previous)
                } else {
                    next.try_inherit_state_from(&mut previous)
                }
                .unwrap_or_else(|error| panic!("{context}: {error:?}"));
                // The beat is continuous: twice the frames at half the tempo.
                assert_eq!(next.arrangement_frame(), 2 * cut, "{context}");
                assert_eq!(next.performance_frame(), cut, "{context}");
                assert!(next.playing(), "{context}");
                assert_eq!(next.active_loop_region().is_some(), looped, "{context}");

                let mut actual = vec![[0.0; 2]; horizon];
                for chunk in actual.chunks_mut(block) {
                    next.render(chunk).unwrap();
                }
                assert_eq!(next.arrangement_frame(), 2 * cut + horizon as u64);
                // The same occurrence, never a second onset.
                let after = proofs(&next);
                assert!(after.len() <= 1, "{context}: {}", after.len());
                assert!(
                    after
                        .iter()
                        .all(|proof| proof.id == before[0].id && proof.note_off_sent),
                    "{context}"
                );
                // The old voice sounds its release from the cut, then stays
                // silent: no chase or retrigger of the remapped note.
                assert_ne!(actual[0], [0.0; 2], "{context}");
                let quiet = RELEASE as usize + 16;
                assert!(
                    actual[quiet..].iter().all(|frame| *frame == [0.0; 2]),
                    "{context}"
                );
                if released {
                    // An already releasing voice keeps its exact tail.
                    let mut expected = vec![[0.0; 2]; quiet];
                    for chunk in expected.chunks_mut(block) {
                        reference.render(chunk).unwrap();
                    }
                    assert_eq!(actual[..quiet], expected[..], "{context}");
                } else {
                    // The donor's held level releases from frame zero.
                    assert!(
                        actual[RELEASE as usize..quiet]
                            .iter()
                            .all(|frame| frame[0] < actual[0][0]),
                        "{context}"
                    );
                }
            }
        }
    }
}

#[test]
fn tempo_remap_after_instrument_changes_does_not_duplicate_old_note() {
    use hex_lattice::NativeInstrumentDefinition;
    let first = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.tempo.repeat-a"),
        levels: [0.2, 0.4],
        release_frames: 8,
    };
    let second = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.tempo.repeat-b"),
        levels: [0.7, 0.9],
        release_frames: 8,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&first, &second];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    snapshot.loop_region = Some(LoopRegion::new(0, 12_000, true).unwrap());
    let base = legacy_project_from_snapshot(snapshot).unwrap();
    let first_project = with_test_instrument(&base, first.id, 0);
    let second_project = with_test_instrument(&base, second.id, 0);
    let mut previous = SessionPlayback::new(revision_with_instrument_registry(
        &first_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    render_frames(&mut previous, 64, 64);
    let mut middle = SessionPlayback::new(revision_with_instrument_registry(
        &second_project,
        Arc::clone(&table),
        &registry,
    ))
    .unwrap();
    middle.try_inherit_state_from(&mut previous).unwrap();
    render_frames(&mut middle, 64, 64);

    let mut edited = Editor::new(first_project, 8).unwrap();
    edited
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let mut next = SessionPlayback::new(revision_with_instrument_registry(
        edited.project(),
        table,
        &registry,
    ))
    .unwrap();
    next.try_inherit_state_from(&mut middle).unwrap();
    assert_eq!(next.arrangement_frame(), 256);
    let wrap = next.performance_frame() + (24_000 - 256);
    assert!(!next.suppressions.is_empty());
    assert!(next.suppressions.iter().all(|s| s.until_frame <= wrap));
    let mut output = [[0.0; 2]; 64];
    next.render(&mut output).unwrap();
    assert_eq!(output[..8], [[0.2; 2]; 8]);
    assert!(output[8..].iter().all(|frame| *frame == [0.0; 2]));
    render_frames(&mut next, 512, 24_000 - 256 - 64);
    let mut after_wrap = [[0.0; 2]; 1];
    next.render(&mut after_wrap).unwrap();
    assert_eq!(after_wrap, [[0.2; 2]]);
}

#[test]
fn tempo_remap_preserves_session_tail_after_return_to_arrangement() {
    use hex_lattice::NativeInstrumentDefinition;
    let definition = ContinuityInstrumentDefinition {
        id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.tempo.session-tail"),
        levels: [0.2, 0.4],
        release_frames: 1_024,
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let table = Arc::new(hex_devices::SineTable::new());
    let base = session_only_project(&project(1.0, 0, 8 << 32));
    let original = with_test_instrument(&base, definition.id, 0);
    let old_revision = revision_with_instrument_registry(&original, Arc::clone(&table), &registry);
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
        render_frames(playback, 64, 128);
        playback
            .apply(SessionPlaybackCommand::BackToArrangement {
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        assert!(
            playback
                .releases
                .iter()
                .any(|entry| entry.timeline == ReleaseTimeline::Session)
        );
    }
    let mut edited = Editor::new(original, 8).unwrap();
    edited
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let mut snapshot = edited.project().snapshot();
    snapshot.loop_region = Some(LoopRegion::new(0, 300, true).unwrap());
    let candidate = legacy_project_from_snapshot(snapshot).unwrap();
    let mut next = SessionPlayback::new(revision_with_instrument_registry(
        &candidate, table, &registry,
    ))
    .unwrap();
    next.try_inherit_state_from(&mut previous).unwrap();
    assert!(
        next.releases
            .iter()
            .any(|entry| entry.timeline == ReleaseTimeline::Session)
    );
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    next.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
    assert_eq!(actual, expected);
}

#[test]
fn legacy_transport_copy_refuses_missing_timing_without_mutation() {
    let table = Arc::new(hex_devices::SineTable::new());
    let mut old = linear_at(&arrangement_at(120_000_000, None), &table);
    old.apply(PlaybackCommand::Play);
    render_linear(&mut old, 64, 3_001);
    let mut next = Playback::new(crate::Project::new(48_000, Vec::new()).unwrap());
    next.apply(PlaybackCommand::Seek(77));
    let before = (next.position(), next.performance_frame(), next.playing());
    assert!(matches!(
        next.inherit_transport(&old),
        Err(crate::playback::PlaybackHandoffError::Session(
            SessionPlaybackError::TempoConversion
        ))
    ));
    assert_eq!(
        (next.position(), next.performance_frame(), next.playing()),
        before
    );
    assert_eq!(old.position(), 3_001);
    assert!(old.playing());
}

#[test]
fn legacy_transport_copy_accepts_zero_without_timing_metadata() {
    let table = Arc::new(hex_devices::SineTable::new());
    let old = linear_at(&arrangement_at(120_000_000, None), &table);
    let mut next = Playback::new(crate::Project::new(48_000, Vec::new()).unwrap());
    next.apply(PlaybackCommand::Seek(77));
    next.inherit_transport(&old).unwrap();
    assert_eq!(next.position(), 0);
}

/// No note sounds at the tested cuts (one note over beats 12..13), so legacy
/// state can be promoted to and demoted from a Session runtime.
fn quiet_at(micro_bpm: u32, loop_region: Option<(u64, u64)>) -> Project {
    let tempo = Tempo::new(micro_bpm).unwrap();
    let mut snapshot = project_at_tempo(0.5, 12 << 32, 1 << 32, tempo).snapshot();
    snapshot.loop_region =
        loop_region.map(|(start, end)| LoopRegion::new(start, end, true).unwrap());
    legacy_project_from_snapshot(snapshot).unwrap()
}

fn prepared_at(
    project: &Project,
    table: &Arc<hex_devices::SineTable>,
) -> crate::playback::Playback {
    crate::playback::Playback::try_from_prepared_with_session(revision(project, Arc::clone(table)))
        .unwrap()
}

#[test]
fn legacy_promotion_and_demotion_keep_the_arrangement_beat() {
    // Pulse #4135: the legacy-to-Session promotion and Session-to-legacy
    // demotion arms keep the beat across a tempo-map change, like #4118.
    let table = Arc::new(hex_devices::SineTable::new());
    let cut = 3_001;
    for looped in [false, true] {
        for block in [1, 64, 512] {
            let region = |micro_bpm| looped.then(|| beat_loop(micro_bpm)).flatten();
            let context = format!("looped={looped} block={block}");

            let mut legacy = linear_at(&quiet_at(120_000_000, region(120_000_000)), &table);
            legacy.apply(PlaybackCommand::Play);
            render_linear(&mut legacy, block, cut);
            let mut promoted = prepared_at(&quiet_at(60_000_000, region(60_000_000)), &table);
            promoted.try_inherit_runtime_from(&mut legacy).unwrap();
            assert!(promoted.playing(), "{context}");
            assert_eq!(promoted.position(), 2 * cut as u64, "promotion {context}");
            assert_eq!(promoted.performance_frame(), cut as u64, "{context}");
            render_linear(&mut promoted, block, 1_000);
            assert_eq!(promoted.position(), 2 * cut as u64 + 1_000, "{context}");

            let mut session = prepared_at(&quiet_at(120_000_000, region(120_000_000)), &table);
            session.apply(PlaybackCommand::Play);
            render_linear(&mut session, block, cut);
            assert_eq!(session.position(), cut as u64);
            let mut demoted = linear_at(&quiet_at(60_000_000, region(60_000_000)), &table);
            demoted.try_inherit_runtime_from(&mut session).unwrap();
            assert!(demoted.playing(), "{context}");
            assert_eq!(demoted.position(), 2 * cut as u64, "demotion {context}");
            assert_eq!(demoted.performance_frame(), cut as u64, "{context}");
            render_linear(&mut demoted, block, 1_000);
            assert_eq!(demoted.position(), 2 * cut as u64 + 1_000, "{context}");
        }
    }
    // A remapped promotion past an unretimed loop end wraps inside the loop.
    let region = Some((0, 4_800));
    let mut legacy = linear_at(&quiet_at(120_000_000, region), &table);
    legacy.apply(PlaybackCommand::Play);
    render_linear(&mut legacy, 64, 4_500);
    let mut promoted = prepared_at(&quiet_at(60_000_000, region), &table);
    promoted.try_inherit_runtime_from(&mut legacy).unwrap();
    assert_eq!(promoted.position(), 9_000 - 4_800);
}

#[test]
fn transport_copy_keeps_the_arrangement_beat() {
    // Pulse #4135: the legacy `inherit_transport` follows the same remap.
    let table = Arc::new(hex_devices::SineTable::new());
    let cut = 3_001;
    for looped in [false, true] {
        for block in [1, 64, 512] {
            let region = |micro_bpm| looped.then(|| beat_loop(micro_bpm)).flatten();
            let mut old = linear_at(&arrangement_at(120_000_000, region(120_000_000)), &table);
            old.apply(PlaybackCommand::Play);
            render_linear(&mut old, block, cut);
            let mut next = linear_at(&arrangement_at(60_000_000, region(60_000_000)), &table);
            next.inherit_transport(&old).unwrap();
            assert!(next.playing(), "looped={looped} block={block}");
            assert_eq!(
                next.position(),
                2 * cut as u64,
                "looped={looped} block={block}"
            );
            render_linear(&mut next, block, 1_000);
            assert_eq!(next.position(), 2 * cut as u64 + 1_000);
        }
    }
    // An unchanged map keeps the frame exactly.
    let project = arrangement_at(120_000_000, None);
    let mut old = linear_at(&project, &table);
    old.apply(PlaybackCommand::Play);
    render_linear(&mut old, 64, cut);
    let mut next = linear_at(&project, &table);
    next.inherit_transport(&old).unwrap();
    assert_eq!(next.position(), cut as u64);
}
