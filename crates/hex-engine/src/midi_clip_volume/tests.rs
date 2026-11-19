use super::*;
use crate::prepared_tempo::{
    AnchoredSessionTempoView, PreparedPeriodicSessionTempo, SessionBaseTimingView,
};
use crate::{MidiTiming, PreparedTempoMap, SessionClockAnchor};
use hex_project::{MidiClipGain, MidiClipVolumePoint, Tempo, TempoMap, TempoPoint};
fn b(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn curve(points: &[(BeatTime, f64, f64)], length: BeatTime) -> MidiClipVolume {
    MidiClipVolume::new(
        points
            .iter()
            .map(|&(beat, incoming, value)| MidiClipVolumePoint {
                beat,
                incoming: MidiClipGain::new(incoming).unwrap(),
                value: MidiClipGain::new(value).unwrap(),
            })
            .collect(),
        length,
    )
    .unwrap()
}
fn clock() -> PreparedTempoMap {
    PreparedTempoMap::prepare(
        TempoMap::constant(8_000, Tempo::new(120_000_000).unwrap()).unwrap(),
        8_000,
    )
    .unwrap()
}
fn changes() -> PreparedTempoMap {
    PreparedTempoMap::prepare(
        TempoMap::new(
            8_000,
            vec![
                TempoPoint::new(b(0), Tempo::new(120_000_000).unwrap()),
                TempoPoint::new(b(1), Tempo::new(60_000_000).unwrap()),
                TempoPoint::new(b(2), Tempo::new(240_000_000).unwrap()),
            ],
        )
        .unwrap(),
        8_000,
    )
    .unwrap()
}
fn arrangement() -> VolumePlacement {
    VolumePlacement::Arrangement { start: b(0) }
}
fn session(origin: SessionMidiOrigin) -> VolumePlacement {
    VolumePlacement::Session { origin }
}

#[test]
fn clip_gain_multiplies_curve_and_implicit_unity_without_editing_points() {
    let timing = clock();
    let gain = hex_project::ClipGain::new(-6_000).unwrap();
    let coefficient = f64::from(gain.linear());
    let source = MidiSourcePlayback::one_shot(b(4)).unwrap();
    let original = curve(&[(b(0), 0.25, 0.25), (b(4), 0.75, 0.75)], b(4));
    let adjusted = original.clone().with_clip_gain(gain);
    let program = PreparedMidiClipVolume::prepare(&adjusted, source, &timing).unwrap();
    let ramp = program
        .segment_at(&timing, arrangement(), 0, 256)
        .unwrap()
        .unwrap();
    assert_eq!(ramp.start_gain, 0.25 * coefficient);
    assert_eq!(ramp.end_gain, 0.75 * coefficient);
    assert_eq!(adjusted.points(), original.points());
    assert!(!adjusted.same_revision(&original));

    let scalar_only = MidiClipVolume::default().with_clip_gain(gain);
    let program = PreparedMidiClipVolume::prepare(&scalar_only, source, &timing).unwrap();
    let ramp = program
        .segment_at(&timing, arrangement(), 0, 256)
        .unwrap()
        .unwrap();
    assert_eq!(ramp.start_gain, coefficient);
    assert_eq!(ramp.end_gain, coefficient);
    assert!(!scalar_only.is_unity());
}

#[test]
fn literal_f64_ramps_and_jumps_keep_original_endpoints_through_uneven_partitions() {
    let timing = clock();
    let volume = curve(
        &[(b(0), 0.0, 0.0), (b(2), 2.0, 0.5), (b(4), 4.0, 4.0)],
        b(4),
    );
    let program = PreparedMidiClipVolume::prepare(
        &volume,
        MidiSourcePlayback::one_shot(b(4)).unwrap(),
        &timing,
    )
    .unwrap();
    assert!(program.volume.same_revision(&volume));
    assert_eq!(
        program.retained_control_metadata_bytes(),
        volume.retained_control_metadata_bytes().unwrap()
    );
    let first = program
        .segment_at(&timing, arrangement(), 0, 256)
        .unwrap()
        .unwrap();
    assert_eq!(
        first,
        VolumeRamp {
            start_frame: 0,
            end_frame: 8000,
            start_gain: 0.0,
            end_gain: 2.0,
            next_boundary: Some(8000)
        }
    );
    let mut at = 0;
    for frames in [1, 127, 13, 256, 3, 217, 256, 1] {
        let part = program
            .segment_at(&timing, arrangement(), at, frames)
            .unwrap()
            .unwrap();
        assert_eq!(part, first, "callback boundaries do not restart the ramp");
        for frame in at..at + frames as u64 {
            assert_eq!(part.gain_at(frame).unwrap(), 2.0 * (frame as f64 / 8000.0));
        }
        at += frames as u64;
    }
    assert_eq!(
        program
            .segment_at(&timing, arrangement(), 8000, 1)
            .unwrap()
            .unwrap(),
        VolumeRamp {
            start_frame: 8000,
            end_frame: 16000,
            start_gain: 0.5,
            end_gain: 4.0,
            next_boundary: Some(16000)
        }
    );
    assert_eq!(
        program
            .segment_at(&timing, arrangement(), 16000, 1)
            .unwrap(),
        None
    );
}

#[test]
fn finite_nonzero_placement_and_midcurve_console_cue_use_distinct_source_domains() {
    let timing = clock();
    let volume = curve(&[(b(0), 0.0, 0.0), (b(8), 4.0, 4.0)], b(8));
    let source = MidiSourcePlayback::new(b(8), b(1), None, b(2)).unwrap();
    let program = PreparedMidiClipVolume::prepare(&volume, source, &timing).unwrap();
    let placement = VolumePlacement::Arrangement { start: b(5) };
    assert_eq!(
        program.segment_at(&timing, placement, 19999, 1).unwrap(),
        None
    );
    assert_eq!(
        program
            .segment_at(&timing, placement, 20050, 1)
            .unwrap()
            .unwrap(),
        VolumeRamp {
            start_frame: 20000,
            end_frame: 28000,
            start_gain: 0.5,
            end_gain: 1.5,
            next_boundary: Some(28000)
        }
    );
    assert_eq!(
        program.segment_at(&timing, placement, 28000, 1).unwrap(),
        None
    );
    let cue = session(SessionMidiOrigin::new(b(10), b(3)));
    assert_eq!(program.segment_at(&timing, cue, 39999, 1).unwrap(), None);
    assert_eq!(
        program.segment_at(&timing, cue, 40050, 1).unwrap().unwrap(),
        VolumeRamp {
            start_frame: 40000,
            end_frame: 56000,
            start_gain: 2.0,
            end_gain: 4.0,
            next_boundary: Some(56000)
        }
    );
    assert_eq!(program.segment_at(&timing, cue, 56000, 1).unwrap(), None);
    let exhausted = session(SessionMidiOrigin::new(b(10), b(7)));
    assert_eq!(
        program.segment_at(&timing, exhausted, 40000, 1).unwrap(),
        None
    );
}

#[test]
fn source_loops_preserve_left_limits_right_values_and_direct_late_cycle_lookup() {
    let timing = clock();
    let volume = curve(
        &[(b(0), 0.0, 0.0), (b(2), 1.0, 2.0), (b(4), 0.0, 0.0)],
        b(4),
    );
    let source = MidiSourcePlayback::new(b(4), b(1), Some((b(2), b(4))), b(2)).unwrap();
    let program = PreparedMidiClipVolume::prepare(&volume, source, &timing).unwrap();
    let placement = session(SessionMidiOrigin::launch(b(0)));
    assert_eq!(
        program
            .segment_at(&timing, placement, 8000, 1)
            .unwrap()
            .unwrap(),
        VolumeRamp {
            start_frame: 4000,
            end_frame: 12000,
            start_gain: 2.0,
            end_gain: 0.0,
            next_boundary: Some(12000)
        }
    );
    assert_eq!(
        program
            .segment_at(&timing, placement, 12000, 1)
            .unwrap()
            .unwrap(),
        VolumeRamp {
            start_frame: 12000,
            end_frame: 20000,
            start_gain: 2.0,
            end_gain: 0.0,
            next_boundary: Some(20000)
        }
    );
    let late = timing.beat_to_frame(b(1_000_000_003)).unwrap();
    let ramp = program
        .segment_at(&timing, placement, late, 1)
        .unwrap()
        .unwrap();
    assert_eq!(ramp.start_frame, late);
    assert_eq!(ramp.end_frame, late + 8000);
    assert_eq!((ramp.start_gain, ramp.end_gain), (2.0, 0.0));
}

#[test]
fn effective_periodic_and_anchored_tempo_boundaries_split_original_source_ramps() {
    let map = changes();
    let volume = curve(&[(b(0), 0.0, 0.0), (b(4), 4.0, 4.0)], b(4));
    let source = MidiSourcePlayback::new(b(4), b(0), Some((b(0), b(4))), b(4)).unwrap();
    let periodic = PreparedPeriodicSessionTempo::prepare(
        &map,
        hex_project::LoopRegion::new(0, 12000, true).unwrap(),
    )
    .unwrap();
    let view = periodic.view(&map);
    let program = PreparedMidiClipVolume::prepare(&volume, source, &view).unwrap();
    let placement = session(SessionMidiOrigin::launch(b(0)));
    assert_eq!(
        program
            .segment_at(&view, placement, 12500, 1)
            .unwrap()
            .unwrap(),
        VolumeRamp {
            start_frame: 12000,
            end_frame: 16000,
            start_gain: 2.0,
            end_gain: 3.0,
            next_boundary: Some(16000)
        }
    );
    let anchor = AnchoredSessionTempoView::new(
        SessionBaseTimingView::Canonical(&map),
        SessionClockAnchor {
            cut_frame: 4000,
            old_beat: b(7),
            new_beat: b(1),
        },
    );
    let program = PreparedMidiClipVolume::prepare(&volume, source, &anchor).unwrap();
    let cue = session(SessionMidiOrigin::new(b(7), b(2)));
    assert_eq!(
        program.segment_at(&anchor, cue, 4050, 1).unwrap().unwrap(),
        VolumeRamp {
            start_frame: 4000,
            end_frame: 12000,
            start_gain: 2.0,
            end_gain: 3.0,
            next_boundary: Some(12000)
        }
    );
}

#[test]
fn equal_frame_authored_knots_last_win_for_both_adjacent_ramp_endpoints() {
    let timing = clock();
    let after = BeatTime::from_raw(b(1).raw() + 1);
    let volume = curve(
        &[
            (b(0), 0.0, 0.0),
            (b(1), 1.0, 2.0),
            (after, 3.0, 0.0),
            (b(2), 4.0, 4.0),
        ],
        b(2),
    );
    let program = PreparedMidiClipVolume::prepare(
        &volume,
        MidiSourcePlayback::one_shot(b(2)).unwrap(),
        &timing,
    )
    .unwrap();
    assert_eq!(
        timing.beat_to_frame(b(1)).unwrap(),
        timing.beat_to_frame(after).unwrap()
    );
    assert_eq!(
        program
            .segment_at(&timing, arrangement(), 3999, 1)
            .unwrap()
            .unwrap(),
        VolumeRamp {
            start_frame: 0,
            end_frame: 4000,
            start_gain: 0.0,
            end_gain: 3.0,
            next_boundary: Some(4000)
        }
    );
    assert_eq!(
        program
            .segment_at(&timing, arrangement(), 4000, 1)
            .unwrap()
            .unwrap(),
        VolumeRamp {
            start_frame: 4000,
            end_frame: 8000,
            start_gain: 0.0,
            end_gain: 4.0,
            next_boundary: Some(8000)
        }
    );
}

#[test]
fn collapsed_future_jump_does_not_hide_a_preceding_tempo_boundary() {
    let timing = changes();
    let after = BeatTime::from_raw(b(2).raw() + 1);
    let volume = curve(
        &[
            (b(0), 1.0, 1.0),
            (b(2), 1.0, 2.0),
            (after, 3.0, 0.0),
            (b(4), 4.0, 4.0),
        ],
        b(4),
    );
    let program = PreparedMidiClipVolume::prepare(
        &volume,
        MidiSourcePlayback::one_shot(b(4)).unwrap(),
        &timing,
    )
    .unwrap();
    assert_eq!(
        program
            .segment_at(&timing, arrangement(), 0, 1)
            .unwrap()
            .unwrap(),
        VolumeRamp {
            start_frame: 0,
            end_frame: 4000,
            start_gain: 1.0,
            end_gain: 1.0,
            next_boundary: Some(4000)
        }
    );
    assert_eq!(
        program
            .segment_at(&timing, arrangement(), 4500, 1)
            .unwrap()
            .unwrap(),
        VolumeRamp {
            start_frame: 4000,
            end_frame: 12000,
            start_gain: 1.0,
            end_gain: 3.0,
            next_boundary: Some(12000)
        }
    );
}

#[test]
fn constant_curves_skip_tempo_splits_dense_source_refuses_and_domain_errors_are_atomic() {
    let timing = changes();
    let length = BeatTime::from_raw(1);
    let dense_source = MidiSourcePlayback::new(length, b(0), Some((b(0), length)), b(1)).unwrap();
    let varying = curve(&[(b(0), 0.0, 0.0), (length, 1.0, 1.0)], length);
    assert!(matches!(
        PreparedMidiClipVolume::prepare(&varying, dense_source, &timing),
        Err(Invalid("clip volume density exceeds capacity"))
    ));
    let fixed = curve(&[(b(0), 0.5, 0.5), (length, 0.5, 0.5)], length);
    let constant = PreparedMidiClipVolume::prepare(&fixed, dense_source, &timing).unwrap();
    let placement = session(SessionMidiOrigin::launch(b(0)));
    let ramp = constant
        .segment_at(&timing, placement, 4500, 256)
        .unwrap()
        .unwrap();
    assert_eq!(
        ramp,
        VolumeRamp {
            start_frame: 0,
            end_frame: u64::MAX,
            start_gain: 0.5,
            end_gain: 0.5,
            next_boundary: None
        }
    );
    for frames in [0, 257] {
        assert!(constant.segment_at(&timing, placement, 0, frames).is_err());
    }
    assert!(
        constant
            .segment_at(&timing, placement, u64::MAX, 1)
            .is_err()
    );
    let finite = VolumePlacement::Arrangement {
        start: BeatTime::from_raw(u64::MAX),
    };
    assert!(constant.segment_at(&timing, finite, 0, 1).is_err());
    let volume = curve(&[(b(0), 0.0, 0.0), (b(1), 1.0, 1.0)], b(1));
    let source = MidiSourcePlayback::new(b(1), b(0), Some((b(0), b(1))), b(1)).unwrap();
    let program = PreparedMidiClipVolume::prepare(&volume, source, &timing).unwrap();
    let near_end = timing
        .beat_to_frame(BeatTime::from_raw(u64::MAX - b(3).raw()))
        .unwrap();
    let ramp = program
        .segment_at(&timing, placement, near_end, 1)
        .unwrap()
        .unwrap();
    assert!(ramp.end_frame > near_end);
}

#[test]
fn clock_rebase_keeps_predating_console_origin_and_stable_cut_ramps() {
    let base = changes();
    let timing = AnchoredSessionTempoView::new(
        SessionBaseTimingView::Canonical(&base),
        SessionClockAnchor {
            cut_frame: 4000,
            old_beat: b(7),
            new_beat: b(1),
        },
    );
    // Launch zero cannot be mapped through the new clock, but global beat seven
    // still means elapsed seven / source phase three. Do not reset it to zero.
    assert!(timing.beat_to_frame(b(0)).is_err());
    let volume = curve(&[(b(0), 0.0, 0.0), (b(4), 4.0, 4.0)], b(4));
    let source = MidiSourcePlayback::new(b(4), b(0), Some((b(0), b(4))), b(4)).unwrap();
    let program = PreparedMidiClipVolume::prepare(&volume, source, &base).unwrap();
    let placement = session(SessionMidiOrigin::launch(b(0)));
    let expected = VolumeRamp {
        start_frame: 4000,
        end_frame: 12000,
        start_gain: 3.0,
        end_gain: 4.0,
        next_boundary: Some(12000),
    };
    for (frame, frames) in [(4000, 1), (4050, 256), (4321, 17), (11999, 1)] {
        let actual = program
            .segment_at(&timing, placement, frame, frames)
            .unwrap()
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(
            actual.gain_at(frame).unwrap(),
            3.0 + (frame - 4000) as f64 / 8000.0
        );
    }
    assert!(program.segment_at(&timing, placement, 3999, 1).is_err());
    let constant = curve(&[(b(0), 2.0, 2.0)], b(4));
    let held = PreparedMidiClipVolume::prepare(&constant, source, &base).unwrap();
    assert_eq!(
        held.segment_at(&timing, placement, 4050, 1)
            .unwrap()
            .unwrap(),
        VolumeRamp {
            start_frame: 4000,
            end_frame: u64::MAX,
            start_gain: 2.0,
            end_gain: 2.0,
            next_boundary: None,
        }
    );
    // Ended finite and one-shot clips are silent without mapping an obsolete end.
    assert_eq!(
        program.segment_at(&timing, arrangement(), 4050, 1).unwrap(),
        None
    );
    let one_shot = PreparedMidiClipVolume::prepare(
        &volume,
        MidiSourcePlayback::one_shot(b(4)).unwrap(),
        &base,
    )
    .unwrap();
    assert_eq!(
        one_shot.segment_at(&timing, placement, 4050, 1).unwrap(),
        None
    );
    // A later cue keeps its own floor, rather than being moved back to the cut.
    let cue = session(SessionMidiOrigin::new(b(9), b(1)));
    assert_eq!(held.segment_at(&timing, cue, 4050, 1).unwrap(), None);
    assert_eq!(
        held.segment_at(&timing, cue, 14000, 1)
            .unwrap()
            .unwrap()
            .start_frame,
        14000
    );
}

#[test]
fn finite_endpoint_freezes_exact_left_limit_without_playing_terminal_jump() {
    let timing = clock();
    let volume = curve(
        &[(b(0), 0.0, 0.0), (b(2), 0.5, 2.0), (b(4), 3.0, 4.0)],
        b(4),
    );
    let source = MidiSourcePlayback::new(b(4), b(0), None, b(2)).unwrap();
    let program = PreparedMidiClipVolume::prepare(&volume, source, &timing).unwrap();
    assert_eq!(
        program.finite_endpoint(&timing, arrangement()).unwrap(),
        Some(VolumeEndpoint {
            frame: 8000,
            gain: 0.5
        })
    );
    assert_eq!(
        program
            .finite_endpoint(&timing, session(SessionMidiOrigin::new(b(5), b(1))))
            .unwrap(),
        Some(VolumeEndpoint {
            frame: 32000,
            gain: 3.0
        })
    );
    // One-shot source exhaustion precedes the longer Arrangement extent.
    let short = MidiSourcePlayback::new(b(4), b(2), None, b(8)).unwrap();
    let program = PreparedMidiClipVolume::prepare(&volume, short, &timing).unwrap();
    assert_eq!(
        program.finite_endpoint(&timing, arrangement()).unwrap(),
        Some(VolumeEndpoint {
            frame: 8000,
            gain: 3.0
        })
    );
    assert_eq!(
        program
            .finite_endpoint(&timing, session(SessionMidiOrigin::new(b(5), b(2))))
            .unwrap(),
        None
    );
    // At a finite loop seam, freeze the left limit at loop_end, not loop_start.
    let looping = MidiSourcePlayback::new(b(4), b(0), Some((b(0), b(2))), b(4)).unwrap();
    let program = PreparedMidiClipVolume::prepare(&volume, looping, &timing).unwrap();
    assert_eq!(
        program.finite_endpoint(&timing, arrangement()).unwrap(),
        Some(VolumeEndpoint {
            frame: 16000,
            gain: 0.5
        })
    );
    assert_eq!(
        program
            .finite_endpoint(&timing, session(SessionMidiOrigin::new(b(5), b(1))))
            .unwrap(),
        None
    );
    let rebased = AnchoredSessionTempoView::new(
        SessionBaseTimingView::Canonical(&timing),
        SessionClockAnchor {
            cut_frame: 4000,
            old_beat: b(7),
            new_beat: b(1),
        },
    );
    assert!(matches!(
        program.finite_endpoint(&rebased, arrangement()),
        Err(Invalid("clip volume endpoint precedes clock rebase"))
    ));
}

#[test]
fn retained_forward_and_reverse_windows_match_the_canonical_materialized_curve_without_unrolling() {
    let timing = clock();
    let authored = curve(
        &[
            (b(0), 0.25, 0.25),
            (b(2), 1.0, 0.5),
            (b(5), 0.75, 2.0),
            (b(8), 1.0, 1.0),
        ],
        b(8),
    );
    for reversed in [false, true] {
        let playback = MidiSourcePlayback::new(b(8), b(1), Some((b(2), b(5))), b(7))
            .unwrap()
            .with_content_window(
                hex_project::ContentWindow::new(b(3).raw(), b(10).raw(), reversed).unwrap(),
            )
            .unwrap();
        let expected = authored.materialized(playback, b(0), b(7)).unwrap();
        let prepared = PreparedMidiClipVolume::prepare(&authored, playback, &timing).unwrap();
        assert!(prepared.source_revision_matches(&authored));
        for frame in [
            0, 1, 3999, 4000, 4001, 8000, 11999, 12000, 15999, 20000, 23999, 27999,
        ] {
            let ramp = prepared
                .segment_at(&timing, arrangement(), frame, 1)
                .unwrap()
                .unwrap();
            let actual = ramp.gain_at(frame).unwrap();
            let expected = expected
                .gain_at(timing.frame_to_beat(frame).unwrap())
                .linear();
            assert!(
                (actual - expected).abs() < 1e-8,
                "reversed={reversed}, frame={frame}: {actual} vs {expected}"
            );
        }
    }
}
