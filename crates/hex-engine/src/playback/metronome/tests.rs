use super::*;
use crate::playback::Playback;
use crate::{
    PlaybackAction, PlaybackActionError, PlaybackCommand, PreparedRevision, SessionPlayback,
    SessionPlaybackCommand, SineTable,
};
use hex_project::{Meter, MeterPoint, Project, Tempo, TempoMap, TempoPoint};
use std::{collections::BTreeMap, sync::Arc};

fn beat(numerator: u64, denominator: u64) -> BeatTime {
    BeatTime::from_ratio(numerator, denominator).unwrap()
}

fn revision(loop_region: Option<LoopRegion>) -> Arc<PreparedRevision> {
    let mut snapshot = Project::new(8_000).unwrap().snapshot();
    snapshot.tempo_points = vec![
        TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT),
        TempoPoint::new(beat(2, 1), Tempo::new(60_000_000).unwrap()),
    ];
    snapshot.meter_points = vec![
        MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME),
        MeterPoint::new(beat(3, 2), Meter::new(3, 8).unwrap()),
    ];
    snapshot.loop_region = loop_region;
    Arc::new(
        PreparedRevision::prepare(
            &Project::from_snapshot(snapshot).unwrap(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

fn playing(revision: Arc<PreparedRevision>) -> Playback {
    let mut playback = Playback::from_prepared(revision);
    playback
        .try_apply_action(PlaybackAction::SetMetronome(true))
        .unwrap();
    playback.apply(PlaybackCommand::Play);
    playback
}

fn render_partitioned(playback: &mut Playback, count: usize, block: usize) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; count];
    for chunk in output.chunks_mut(block) {
        playback.render(chunk).unwrap();
    }
    output
}

#[test]
fn tempo_and_meter_clicks_are_finite_accented_and_partition_invariant() {
    let source = revision(None);
    let mut contiguous = playing(Arc::clone(&source));
    let expected = render_partitioned(&mut contiguous, 20_000, 20_000);
    for block in [1, 63, 256, 1_023] {
        assert_eq!(
            render_partitioned(&mut playing(Arc::clone(&source)), 20_000, block),
            expected
        );
    }
    for (frame, amplitude) in [
        (0, 0.16),
        (4_000, 0.10),
        (6_000, 0.16),
        (8_000, 0.10),
        (12_000, 0.10),
        (16_000, 0.16),
    ] {
        assert_eq!(expected[frame], [amplitude; 2]);
        assert_eq!(expected[frame + 160], [0.0; 2]);
    }
    assert!(
        expected
            .iter()
            .flatten()
            .all(|sample| sample.is_finite() && sample.abs() <= 0.16)
    );
}

#[test]
fn off_pause_seek_stop_and_linear_export_have_no_stale_click() {
    let source = revision(None);
    let mut playback = Playback::from_prepared(Arc::clone(&source));
    playback.apply(PlaybackCommand::Play);
    assert_eq!(render_partitioned(&mut playback, 1, 1), vec![[0.0; 2]]);
    playback
        .try_apply_action(PlaybackAction::SetMetronome(true))
        .unwrap();
    playback.apply(PlaybackCommand::Stop);
    playback.apply(PlaybackCommand::Play);
    assert_eq!(render_partitioned(&mut playback, 1, 1), vec![[0.16; 2]]);
    playback.apply(PlaybackCommand::Pause);
    assert_eq!(
        render_partitioned(&mut playback, 400, 63),
        vec![[0.0; 2]; 400]
    );
    playback.apply(PlaybackCommand::PlayFrom(100));
    assert_eq!(
        render_partitioned(&mut playback, 400, 63),
        vec![[0.0; 2]; 400]
    );
    playback.apply(PlaybackCommand::Seek(4_000));
    assert_eq!(render_partitioned(&mut playback, 1, 1), vec![[0.1; 2]]);
    playback
        .try_apply_action(PlaybackAction::SetMetronome(false))
        .unwrap();
    assert_eq!(
        render_partitioned(&mut playback, 400, 63),
        vec![[0.0; 2]; 400]
    );
    let mut export = SessionPlayback::new_linear_arrangement_export(source).unwrap();
    export.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = vec![[1.0; 2]; 12_000];
    export.render(&mut output).unwrap();
    assert_eq!(output, vec![[0.0; 2]; 12_000]);
}

#[test]
fn loop_wraps_retrigger_from_arrangement_grid_and_cut_tail() {
    let source = revision(Some(LoopRegion::new(0, 4_050, true).unwrap()));
    let mut playback = playing(Arc::clone(&source));
    let expected = render_partitioned(&mut playback, 12_200, 12_200);
    assert_eq!(expected[0], [0.16; 2]);
    assert_eq!(expected[4_000], [0.1; 2]);
    assert_eq!(expected[4_050], [0.16; 2]);
    assert_eq!(expected[8_100], [0.16; 2]);
    for block in [1, 256, 4_049] {
        assert_eq!(
            render_partitioned(&mut playing(Arc::clone(&source)), 12_200, block),
            expected
        );
    }
    let mut seeked = playing(source);
    seeked.apply(PlaybackCommand::Seek(8_100));
    assert_eq!(render_partitioned(&mut seeked, 1, 1), vec![[0.0; 2]]);
    assert_eq!(seeked.position(), 8_101);
    seeked.apply(PlaybackCommand::Seek(12_000));
    assert_eq!(render_partitioned(&mut seeked, 1, 1), vec![[0.1; 2]]);
}

#[test]
fn compatible_revision_handoff_preserves_click_and_new_timing_reanchors() {
    let source = revision(None);
    let mut original = playing(Arc::clone(&source));
    render_partitioned(&mut original, 37, 37);
    let mut replacement = Playback::from_prepared(Arc::clone(&source));
    replacement.try_inherit_runtime_from(&mut original).unwrap();
    assert!(replacement.metronome_enabled());
    assert_eq!(
        render_partitioned(&mut replacement, 200, 63),
        render_partitioned(&mut original, 200, 63)
    );
    let mut snapshot = Project::new(8_000).unwrap().snapshot();
    snapshot.tempo_points = vec![TempoPoint::new(
        BeatTime::ZERO,
        Tempo::new(90_000_000).unwrap(),
    )];
    let changed = Arc::new(
        PreparedRevision::prepare(
            &Project::from_snapshot(snapshot).unwrap(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut replacement = Playback::from_prepared(changed);
    replacement.try_inherit_runtime_from(&mut original).unwrap();
    assert!(replacement.metronome_enabled());
    assert_eq!(render_partitioned(&mut replacement, 1, 1), vec![[0.0; 2]]);
}

#[test]
fn supported_rates_and_dense_fractional_meter_markers_terminate() {
    for rate in [8_000, 44_100, 48_000, 192_000] {
        let timing = PreparedTempoMap::prepare(
            TempoMap::constant(rate, Tempo::new(1_000_000_000).unwrap()).unwrap(),
            rate,
        )
        .unwrap();
        let meter = MeterMap::new(
            (0..1_024)
                .map(|raw| MeterPoint::new(BeatTime::from_raw(raw), Meter::new(99, 64).unwrap()))
                .collect(),
        )
        .unwrap();
        let mut metronome = Metronome::prepare(rate, 0).unwrap();
        metronome.set_enabled(true);
        let mut output = vec![[0.0; 2]; rate as usize];
        metronome.mix(&timing, &meter, 0, None, &mut output);
        assert_eq!(output[0], [0.16; 2]);
        assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    }
    assert!(Metronome::prepare(0, 0).is_err());
    assert!(Metronome::prepare(192_001, 0).is_err());
    let mut legacy = Playback::new(crate::Project::new(8_000, Vec::new()).unwrap());
    assert_eq!(
        legacy.try_apply_action(PlaybackAction::SetMetronome(true)),
        Err(PlaybackActionError::MetronomeUnavailable)
    );
    assert!(
        legacy
            .try_apply_action(PlaybackAction::SetMetronome(false))
            .is_ok()
    );
}

#[test]
fn prepared_toggle_cannot_survive_demotion_to_legacy_or_linear_export() {
    let source = revision(None);
    let old = playing(Arc::clone(&source));
    let mut legacy = Playback::from_shared(Arc::clone(&old.project));
    legacy.inherit_transport(&old).unwrap();
    assert!(!legacy.metronome_enabled());
    assert_eq!(
        legacy.try_apply_action(PlaybackAction::SetMetronome(true)),
        Err(PlaybackActionError::MetronomeUnavailable)
    );
    let mut runtime = SessionPlayback::new_linear_arrangement_export(source).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut export = Playback::from_arrangement_preroll(runtime).unwrap();
    export.inherit_transport(&old).unwrap();
    assert!(!export.metronome_enabled());
    assert_eq!(
        export.try_apply_action(PlaybackAction::SetMetronome(true)),
        Err(PlaybackActionError::MetronomeUnavailable)
    );
    assert_eq!(
        render_partitioned(&mut export, 400, 63),
        vec![[0.0; 2]; 400]
    );
}

#[test]
fn page_and_observer_rendering_mix_the_same_monitor_click() {
    struct NoPages;
    impl crate::MediaPages for NoPages {
        fn page(
            &mut self,
            _: crate::StreamSourceKey,
            _: u64,
        ) -> Option<crate::BorrowedMediaPage<'_>> {
            panic!("empty graph requested media")
        }
        fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {
            panic!("empty graph requested media")
        }
    }
    let source = revision(Some(LoopRegion::new(0, 4_050, true).unwrap()));
    let expected = render_partitioned(&mut playing(Arc::clone(&source)), 12_200, 31);
    for observed in [false, true] {
        let mut playback = playing(Arc::clone(&source));
        let mut output = vec![[0.; 2]; expected.len()];
        for chunk in output.chunks_mut(47) {
            let mut observer = |_: crate::PostNodeIdentity, _: usize, _: &[Frame]| {};
            playback
                .render_with_pages_and_post_node_observer(
                    chunk,
                    &mut NoPages,
                    if observed { Some(&mut observer) } else { None },
                )
                .unwrap();
        }
        assert_eq!(output, expected);
    }
}
