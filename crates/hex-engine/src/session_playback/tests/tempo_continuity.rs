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
fn infallible_transport_copy_keeps_the_arrangement_beat() {
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
            next.inherit_transport(&old);
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
    next.inherit_transport(&old);
    assert_eq!(next.position(), cut as u64);
}
