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

#[test]
fn tempo_remap_refuses_a_still_releasing_mutable_instrument_atomically() {
    use hex_lattice::NativeInstrumentDefinition;

    let definition = MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id: hex_lattice_contract::DeviceTypeId::from_static("test.instrument.tempo.release"),
            levels: [0.2, 0.4],
            release_frames: 128,
        },
    };
    let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let old_project = with_test_instrument(&project(1.0, 0, 1 << 32), definition.metadata.id, 0);
    let mut edited = Editor::new(old_project.clone(), 8).unwrap();
    edited
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    for (frame, released) in [(3_001, false), (24_001, true)] {
        for live in [true, false] {
            let mut previous = ordinary_mutable_playback(&old_project, &registry);
            previous.apply(SessionPlaybackCommand::Play).unwrap();
            render_frames(&mut previous, 64, frame);
            assert!(
                previous.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .proofs()
                    .any(|proof| proof.note_off_sent == released)
            );
            let before = previous.arrangement_frame();
            let mut candidate = ordinary_mutable_playback(edited.project(), &registry);
            let result = if live {
                candidate.try_inherit_state_live_from(&mut previous)
            } else {
                candidate.try_inherit_state_from(&mut previous)
            };
            assert!(
                matches!(result, Err(SessionPlaybackError::MutableInstrumentActive)),
                "frame={frame} live={live}: {result:?}"
            );
            assert_eq!(previous.arrangement_frame(), before);
            assert!(previous.playing());
        }
    }
}
