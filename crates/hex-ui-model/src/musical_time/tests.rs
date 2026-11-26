use super::Viewport;
use super::{
    LandmarkDensity, MAX_GRID_BANDS, MAX_GRID_LINES, MAX_METER_MAP_POINTS, MAX_RULER_TICKS,
    SnapGrid, adaptive_grid_bands, grid_lines, ruler_candidates, ruler_ticks,
};
use hex_project::{BeatTime, Command, Editor, Meter, Project, Tempo};

#[test]
fn frozen_adaptive_input_matches_arranger_including_dense_meter_fallback() {
    for count in [3, 160] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(3).unwrap(),
                tempo: Tempo::new(90_000_000).unwrap(),
            })
            .unwrap();
        for index in 1..count {
            editor
                .execute(Command::SetMeterPoint {
                    beat: BeatTime::from_ratio(index, 16).unwrap(),
                    meter: Meter::new(3, 8).unwrap(),
                })
                .unwrap();
        }
        let project = editor.project();
        let viewport = Viewport::fit(384_000);
        for density in super::AdaptiveGridDensity::ALL {
            let grid = SnapGrid::AdaptiveDensity(density);
            let frozen = grid.freeze_input_grid(project, viewport, 480.0).unwrap();
            for frame in [
                0, 1, 6_000, 11_999, 25_000, 37_000, 120_000, 383_999, 400_000,
            ] {
                assert_eq!(
                    frozen.snap_frame(project.tempo_map(), frame).ok(),
                    grid.snap_frame(project, frame, viewport, 480.0),
                    "count={count} density={density:?} frame={frame}"
                );
            }
            let expected = frozen.snap_frame(project.tempo_map(), 37_000).unwrap();
            // Resolving a different zoom/density cannot mutate a retained grid.
            let _ = SnapGrid::Adaptive
                .freeze_input_grid(project, Viewport::fit(48_000), 1_440.0)
                .unwrap();
            assert_eq!(
                frozen.snap_frame(project.tempo_map(), 37_000).unwrap(),
                expected
            );
        }
        assert!(
            SnapGrid::Off
                .freeze_input_grid(project, viewport, 480.0)
                .is_err()
        );
        assert!(
            SnapGrid::Adaptive
                .freeze_input_grid(project, viewport, f32::NAN)
                .is_err()
        );
    }
}

#[test]
fn directional_nudge_landmarks_are_distinct_off_grid_and_across_meter_tempo_seams() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(2).unwrap(),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetMeterPoint {
            beat: BeatTime::from_ratio(5, 2).unwrap(),
            meter: Meter::new(3, 4).unwrap(),
        })
        .unwrap();
    let project = editor.project();
    let viewport = Viewport::fit(384_000);
    assert_eq!(
        SnapGrid::Eighth.adjacent_frame(project, 7200, viewport, 640.0, false),
        Some(0)
    );
    assert_eq!(
        SnapGrid::Eighth.adjacent_frame(project, 7200, viewport, 640.0, true),
        Some(12000)
    );
    assert_eq!(
        SnapGrid::Eighth.adjacent_frame(project, 12000, viewport, 640.0, true),
        Some(24000)
    );
    assert_eq!(
        SnapGrid::Quarter.adjacent_frame(project, 67200, viewport, 640.0, true),
        Some(72000)
    );
    assert_eq!(
        SnapGrid::Quarter.adjacent_frame(project, 72000, viewport, 640.0, false),
        Some(48000)
    );
    assert_eq!(
        SnapGrid::Quarter.adjacent_frame(project, 72000, viewport, 640.0, true),
        Some(120000)
    );
    for width in [160.0, 640.0, 1920.0] {
        let lines = grid_lines(project, viewport, width, SnapGrid::Adaptive);
        for pair in lines.windows(2) {
            assert_eq!(
                SnapGrid::Adaptive.adjacent_frame(project, pair[0].frame, viewport, width, true),
                Some(pair[1].frame)
            );
            assert_eq!(
                SnapGrid::Adaptive.adjacent_frame(project, pair[1].frame, viewport, width, false),
                Some(pair[0].frame)
            );
        }
    }
    assert_eq!(
        SnapGrid::Quarter.adjacent_beat(project, BeatTime::ZERO, viewport, 640.0, false),
        None
    );
    assert_eq!(
        SnapGrid::Quarter.adjacent_beat(
            project,
            BeatTime::from_raw(u64::MAX),
            viewport,
            640.0,
            true
        ),
        None
    );
    assert_eq!(
        SnapGrid::Quarter.adjacent_frame(project, u64::MAX, viewport, 640.0, true),
        None
    );
    assert_eq!(
        SnapGrid::Quarter.adjacent_frame(project, 0, viewport, f32::NAN, true),
        None
    );
}

#[test]
fn arranger_snap_toggles_between_adaptive_and_off() {
    assert_eq!(SnapGrid::default(), SnapGrid::Adaptive);
    assert_eq!(SnapGrid::Adaptive.toggled(), SnapGrid::Off);
    assert_eq!(SnapGrid::Off.toggled(), SnapGrid::Adaptive);
    assert_eq!(SnapGrid::Off.step_raw(), None);
    assert_eq!(SnapGrid::Adaptive.step_raw(), None);
    assert_eq!(SnapGrid::Quarter.step_raw(), Some(1_u64 << 32));
    assert_eq!(SnapGrid::Eighth.step_raw(), Some(1_u64 << 31));
    assert_eq!(SnapGrid::Sixteenth.step_raw(), Some(1_u64 << 30));
    assert_eq!(
        SnapGrid::Adaptive.with_platform_modifier(true),
        SnapGrid::Off
    );
    assert_eq!(
        SnapGrid::Off.with_platform_modifier(true),
        SnapGrid::Adaptive
    );
    assert_eq!(
        SnapGrid::Quarter.with_platform_modifier(true),
        SnapGrid::Off
    );
    assert_eq!(
        SnapGrid::Adaptive.with_platform_modifier(false),
        SnapGrid::Adaptive
    );
}

#[test]
fn snap_resolution_uses_canonical_map_and_rounds_exact_ties_later() {
    let project = Project::new(48_000).expect("project");
    let viewport = Viewport {
        start: 0,
        length: 288_000,
    };
    let tie_frame = project
        .tempo_map()
        .beat_to_frame(BeatTime::from_ratio(1, 2).expect("half beat"))
        .expect("frame");
    assert_eq!(
        SnapGrid::Off.snap_frame(&project, tie_frame, viewport, 640.0),
        Some(tie_frame)
    );
    assert_eq!(
        SnapGrid::Off.snap_frame(&project, u64::MAX, viewport, 640.0),
        Some(u64::MAX)
    );
    assert_eq!(
        SnapGrid::Quarter.snap_frame(&project, tie_frame, viewport, 640.0),
        Some(24_000)
    );

    let eighth_tie_frame = project
        .tempo_map()
        .beat_to_frame(BeatTime::from_ratio(3, 4).expect("eighth tie"))
        .expect("frame");
    assert_eq!(
        SnapGrid::Eighth.snap_frame(&project, eighth_tie_frame, viewport, 640.0),
        Some(24_000)
    );
    assert_eq!(
        SnapGrid::Quarter.snap_frame(&project, u64::MAX, viewport, 640.0),
        None
    );
}

#[test]
fn ruler_subdivision_labels_use_dots_without_changing_positions() {
    let common = hex_project::MeterMap::common_time();
    for (beat, expected) in [
        (BeatTime::ZERO, "1"),
        (BeatTime::from_quarters(2).unwrap(), "1.3"),
        (BeatTime::from_ratio(21, 8).unwrap(), "1.3.6"),
        (BeatTime::from_ratio(1, 64).unwrap(), "1.1.0"),
        (BeatTime::from_quarters(4).unwrap(), "2"),
    ] {
        assert_eq!(super::meter_label(common.position(beat).unwrap()), expected);
    }
    let meter = hex_project::MeterMap::new(vec![
        hex_project::MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME),
        hex_project::MeterPoint::new(
            BeatTime::from_ratio(3, 2).unwrap(),
            Meter::new(3, 8).unwrap(),
        ),
    ])
    .unwrap();
    assert_eq!(
        super::meter_label(meter.position(BeatTime::from_ratio(3, 2).unwrap()).unwrap()),
        "2"
    );
    assert_eq!(
        super::meter_label(
            meter
                .position(BeatTime::from_ratio(17, 8).unwrap())
                .unwrap()
        ),
        "2.2.1"
    );
    let project = Project::new(48_000).unwrap();
    let coarse = ruler_ticks(&project, Viewport::fit(960_000), 600.0);
    assert!(
        coarse
            .iter()
            .all(|tick| tick.bar_start && !tick.label.contains('.'))
    );
    let fine = ruler_ticks(
        &project,
        Viewport {
            start: 60_000,
            length: 12_000,
        },
        600.0,
    );
    assert!(
        fine.iter()
            .any(|tick| tick.frame == 63_000 && tick.label == "1.3.6")
    );
    assert!(fine.iter().all(|tick| !tick.label.contains('+')));
}

#[test]
fn ruler_landmarks_remain_even_and_anchored_while_panning_in_common_time() {
    let project = Project::new(48_000).unwrap();
    let viewport = Viewport {
        start: 576_000,
        length: 384_000,
    };
    let ticks = ruler_ticks(&project, viewport, 600.0);
    let frames: Vec<_> = ticks.iter().map(|tick| tick.frame).collect();
    let grid = grid_lines(&project, viewport, 600.0, SnapGrid::Adaptive);
    assert!(
        frames
            .iter()
            .all(|frame| grid.iter().any(|line| line.frame == *frame))
    );
    let gaps: Vec<_> = frames.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert!(!gaps.is_empty());
    assert!(
        gaps.iter().all(|gap| *gap == gaps[0]),
        "uneven ruler gaps: {frames:?}"
    );
    let panned = ruler_ticks(
        &project,
        Viewport {
            start: viewport.start + 1_000,
            ..viewport
        },
        600.0,
    );
    let overlap_end = viewport.end().unwrap();
    let before: Vec<_> = ticks
        .iter()
        .filter(|tick| tick.frame >= viewport.start + 1_000 && tick.frame < overlap_end)
        .map(|tick| tick.frame)
        .collect();
    let after: Vec<_> = panned
        .iter()
        .filter(|tick| tick.frame < overlap_end)
        .map(|tick| tick.frame)
        .collect();
    assert_eq!(
        before, after,
        "panning changed landmarks in the shared range"
    );
}

#[test]
fn ruler_stride_does_not_change_at_visible_count_threshold() {
    let project = Project::new(48_000).unwrap();
    for length in [295_200, 319_200] {
        let viewport = Viewport { start: 0, length };
        let panned = Viewport {
            start: 12_000,
            ..viewport
        };
        let before = ruler_ticks(&project, viewport, 600.0);
        let after = ruler_ticks(&project, panned, 600.0);
        let overlapping_before: Vec<_> = before
            .iter()
            .filter(|tick| tick.frame >= panned.start)
            .map(|tick| tick.frame)
            .collect();
        let overlapping_after: Vec<_> = after
            .iter()
            .filter(|tick| tick.frame < viewport.end().unwrap())
            .map(|tick| tick.frame)
            .collect();
        assert_eq!(
            overlapping_before, overlapping_after,
            "viewport length {length}"
        );
    }
}

#[test]
fn ruler_and_adaptive_grid_share_landmarks_across_zoom_pan_and_meter_changes() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(8).unwrap(),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetMeterPoint {
            beat: BeatTime::from_quarters(12).unwrap(),
            meter: Meter::new(3, 4).unwrap(),
        })
        .unwrap();
    let project = editor.project();
    for (start, length, width) in [
        (0, 96_000, 640.0),
        (12_000, 192_000, 640.0),
        (120_000, 384_000, 640.0),
        (240_000, 768_000, 1_200.0),
    ] {
        let viewport = Viewport { start, length };
        let grid = grid_lines(project, viewport, width, SnapGrid::Adaptive);
        let ruler = ruler_ticks(project, viewport, width);
        assert!(!ruler.is_empty() && !grid.is_empty());
        for tick in ruler {
            assert!(
                grid.iter().any(|line| line.frame == tick.frame),
                "ruler landmark {} is absent from grid at {viewport:?}",
                tick.frame
            );
        }
    }
}

#[test]
fn bounded_ruler_marks_quarter_positions_and_fragmentary_meter_changes() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let beat = BeatTime::from_ratio(3, 2).expect("meter beat");
    let meter = Meter::new(3, 4).expect("meter");
    let _ = editor
        .execute(Command::SetMeterPoint { beat, meter })
        .expect("meter point");
    assert_eq!(editor.project().meter_map().points().len(), 2);

    let ticks = ruler_ticks(
        editor.project(),
        Viewport {
            start: 0,
            length: 120_000,
        },
        600.0,
    );
    assert!(ticks.len() <= 13);
    let marker = ticks
        .iter()
        .find(|tick| tick.meter_marker)
        .expect("visible meter marker");
    assert_eq!(marker.frame, 36_000);
    assert!(marker.fragmentary_bar);
    assert_eq!(marker.label, "2");
}

#[test]
fn ruler_keeps_only_visible_landmarks_across_section_end() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::SetMeterPoint {
            beat: BeatTime::from_quarters(4).unwrap(),
            meter: Meter::new(3, 4).unwrap(),
        })
        .unwrap();
    // At 120 bpm the second, open-ended section begins at frame 96_000.
    for (start, expected_frames) in [
        (72_000, vec![72_000, 96_000, 120_000]),
        (96_000, vec![96_000, 120_000, 144_000]),
        (120_000, vec![120_000, 144_000, 168_000]),
    ] {
        let viewport = Viewport {
            start,
            length: 72_000,
        };
        let ticks = ruler_candidates(
            editor.project(),
            viewport,
            viewport.end().unwrap(),
            editor.project().tempo_map().frame_to_beat(start).unwrap(),
            editor
                .project()
                .tempo_map()
                .frame_to_beat(viewport.end().unwrap() - 1)
                .unwrap(),
            8,
            LandmarkDensity::Meter,
        )
        .ticks;
        let mut frames = ticks.iter().map(|tick| tick.frame).collect::<Vec<_>>();
        frames.sort_unstable();
        frames.dedup();
        assert_eq!(frames, expected_frames);
        assert_eq!(
            ticks.iter().filter(|tick| tick.meter_marker).count(),
            usize::from(start <= 96_000)
        );
    }
}

#[test]
fn six_eight_ruler_uses_denominator_beats() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let _ = editor
        .execute(Command::SetMeterPoint {
            beat: BeatTime::ZERO,
            meter: Meter::new(6, 8).expect("meter"),
        })
        .expect("meter point");

    let ticks = ruler_ticks(
        editor.project(),
        Viewport {
            start: 0,
            length: 80_000,
        },
        600.0,
    );
    assert!(
        ticks
            .iter()
            .any(|tick| tick.frame == 12_000 && tick.label == "1.2")
    );
    assert!(
        ticks
            .iter()
            .any(|tick| tick.frame == 24_000 && tick.label == "1.3")
    );
}

#[test]
fn fractional_meter_section_starts_its_denominator_beat_landmarks() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let _ = editor
        .execute(Command::SetMeterPoint {
            beat: BeatTime::from_ratio(1, 2).expect("fractional point"),
            meter: Meter::new(3, 4).expect("meter"),
        })
        .expect("meter point");

    let ticks = ruler_ticks(
        editor.project(),
        Viewport {
            start: 0,
            length: 100_000,
        },
        600.0,
    );
    assert!(
        ticks
            .iter()
            .any(|tick| tick.frame == 12_000 && tick.label == "2")
    );
    assert!(
        ticks
            .iter()
            .any(|tick| tick.frame == 36_000 && tick.label == "2.2")
    );
}

#[test]
fn huge_fractional_meter_viewport_keeps_tick_attempts_and_markers_bounded() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let beat = BeatTime::from_ratio(1, 16).expect("fractional meter point");
    let meter = Meter::new(1, 1).expect("meter");
    let _ = editor
        .execute(Command::SetMeterPoint { beat, meter })
        .expect("meter point");

    let viewport = Viewport {
        start: 0,
        length: 1_000_000_000_000,
    };
    let viewport_end = viewport.end().expect("viewport end");
    let start_beat = editor
        .project()
        .tempo_map()
        .frame_to_beat(viewport.start)
        .expect("start beat");
    let last_beat = editor
        .project()
        .tempo_map()
        .frame_to_beat(viewport_end - 1)
        .unwrap_or_else(|_| BeatTime::from_raw(u64::MAX));
    let candidates = ruler_candidates(
        editor.project(),
        viewport,
        viewport_end,
        start_beat,
        last_beat,
        13,
        LandmarkDensity::Meter,
    );
    assert!(candidates.landmark_attempts <= 13);
    assert!(candidates.marker_attempts <= 1_024);
    let ticks = ruler_ticks(editor.project(), viewport, 600.0);
    assert!(ticks.len() <= 13);
    assert!(
        ticks
            .iter()
            .any(|tick| tick.meter_marker && tick.frame == 1_500)
    );

    let sub_quarter_ticks = ruler_ticks(
        editor.project(),
        Viewport {
            start: 1_400,
            length: 200,
        },
        200.0,
    );
    assert!(
        sub_quarter_ticks
            .iter()
            .any(|tick| tick.meter_marker && tick.frame == 1_500)
    );
}

#[test]
fn ruler_landmarks_follow_tempo_boundaries_and_deduplicate_quantized_markers() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let _ = editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(2).expect("tempo beat"),
            tempo: Tempo::new(60_000_000).expect("tempo"),
        })
        .expect("tempo point");
    let ticks = ruler_ticks(
        editor.project(),
        Viewport {
            start: 0,
            length: 150_000,
        },
        600.0,
    );
    assert!(
        ticks
            .iter()
            .any(|tick| tick.frame == 48_000 && tick.label == "1.3")
    );
    assert!(
        ticks
            .iter()
            .any(|tick| tick.frame == 96_000 && tick.label == "1.4")
    );

    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    for (raw, meter) in [
        (1, Meter::new(3, 4).expect("first meter")),
        (2, Meter::new(6, 8).expect("second meter")),
    ] {
        let _ = editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::from_raw(raw),
                meter,
            })
            .expect("meter point");
    }
    let ticks = ruler_ticks(
        editor.project(),
        Viewport {
            start: 0,
            length: 200,
        },
        200.0,
    );
    let at_zero: Vec<_> = ticks.iter().filter(|tick| tick.frame == 0).collect();
    assert_eq!(at_zero.len(), 1);
    assert!(at_zero[0].meter_marker);
    assert_eq!(at_zero[0].label, "3");
}

#[test]
fn full_meter_map_keeps_marker_priority_spacing_and_tick_cap() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let meter = Meter::new(3, 4).expect("meter");
    for index in 1..MAX_METER_MAP_POINTS {
        let beat = BeatTime::from_ratio(index as u64, 64).expect("meter beat");
        let _ = editor
            .execute(Command::SetMeterPoint { beat, meter })
            .expect("meter point");
    }
    let viewport = Viewport {
        start: 0,
        length: 384_000,
    };
    let viewport_end = viewport.end().expect("viewport end");
    let start_beat = BeatTime::ZERO;
    let last_beat = editor
        .project()
        .tempo_map()
        .frame_to_beat(viewport_end - 1)
        .expect("last beat");
    let candidates = ruler_candidates(
        editor.project(),
        viewport,
        viewport_end,
        start_beat,
        last_beat,
        MAX_RULER_TICKS,
        LandmarkDensity::Meter,
    );
    assert_eq!(candidates.marker_attempts, MAX_METER_MAP_POINTS);
    assert!(candidates.landmark_attempts <= MAX_RULER_TICKS);

    let ticks = ruler_ticks(editor.project(), viewport, 6_400.0);
    assert_eq!(ticks.len(), MAX_RULER_TICKS);
    assert!(ticks.iter().all(|tick| tick.meter_marker));
    assert!(ticks.windows(2).all(|pair| {
        let left = (pair[0].frame as f64 / viewport.length as f64) * 6_400.0;
        let right = (pair[1].frame as f64 / viewport.length as f64) * 6_400.0;
        right - left >= 48.0
    }));
}

#[test]
fn adaptive_grid_stays_visible_and_bounded_at_zoom_extremes() {
    let project = Project::new(48_000).expect("project");
    for viewport in [
        Viewport {
            start: 0,
            length: 38_400,
        },
        Viewport {
            start: 0,
            length: 384_000,
        },
        Viewport {
            start: 0,
            length: 3_840_000,
        },
    ] {
        let lines = grid_lines(&project, viewport, 640.0, SnapGrid::Adaptive);
        assert!(!lines.is_empty(), "a valid viewport keeps grid landmarks");
        assert!(lines.len() <= MAX_GRID_LINES);
        assert!(lines.windows(2).all(|pair| pair[0].frame < pair[1].frame));
        assert!(
            lines.iter().all(|line| {
                line.frame >= viewport.start && line.frame < viewport.end().unwrap()
            })
        );
        let gaps = lines
            .windows(2)
            .map(|pair| (pair[1].frame - pair[0].frame) as f64 / viewport.length as f64 * 640.0)
            .collect::<Vec<_>>();
        if !gaps.is_empty() {
            let mut sorted_gaps = gaps;
            sorted_gaps.sort_by(f64::total_cmp);
            let median_gap = sorted_gaps[sorted_gaps.len() / 2];
            assert!(
                (8.0..=40.0).contains(&median_gap),
                "adaptive grid spacing is readable at {viewport:?}: {median_gap}px"
            );
        }
    }
    let zoomed_out = grid_lines(
        &project,
        Viewport {
            start: 0,
            length: 3_840_000,
        },
        640.0,
        SnapGrid::Adaptive,
    );
    assert!(zoomed_out.iter().any(|line| line.bar_start));
}

#[test]
fn adaptive_background_bands_are_bounded_and_pan_anchored() {
    let project = Project::new(48_000).expect("project");
    let first_view = Viewport {
        start: 0,
        length: 384_000,
    };
    let panned_view = Viewport {
        start: 24_000,
        length: 384_000,
    };
    let first = adaptive_grid_bands(&project, first_view, 640.0);
    let panned = adaptive_grid_bands(&project, panned_view, 640.0);
    assert!(!first.is_empty());
    assert!(first.len() <= MAX_GRID_BANDS);
    assert!(first.iter().any(|band| band.start_frame == 0));
    assert!(first.iter().any(|band| band.alternate));
    assert!(first.iter().all(|band| band.start_frame < band.end_frame));
    assert!(panned.iter().all(|band| band.start_frame < band.end_frame));
    let shared = panned
        .iter()
        .find_map(|band| {
            first
                .iter()
                .find(|original| original.start_frame == band.start_frame)
                .map(|original| (original, band))
        })
        .expect("panning retains at least one shared anchored band");
    assert_eq!(shared.0.alternate, shared.1.alternate);
}

#[test]
fn zebra_bands_hold_musical_levels_across_zoom_with_a_one_beat_minimum() {
    // 120 bpm at 48 kHz: one quarter-note beat is 24 000 frames.
    let project = Project::new(48_000).expect("project");
    let beat = 24_000;
    let width = |length: u64| {
        let bands = adaptive_grid_bands(&project, Viewport { start: 0, length }, 640.0);
        assert_eq!(bands.first().map(|band| band.start_frame), Some(0));
        bands[0].end_frame - bands[0].start_frame
    };
    assert_eq!(width(100), beat, "extreme zoom keeps a one-beat minimum");
    assert_eq!(width(beat / 4), beat);
    let widths: Vec<_> = (0..12).map(|doubling| width(12_000 << doubling)).collect();
    let mut changes = 0;
    for pair in widths.windows(2) {
        assert!(
            pair[1] == pair[0] || pair[1] == pair[0] * 4,
            "levels step beat, bar, 4 bars, 16 bars…: {widths:?}"
        );
        changes += usize::from(pair[1] != pair[0]);
    }
    assert!(
        (1..=6).contains(&changes),
        "eleven zoom doublings change the zebra at most every other step: {widths:?}"
    );
    assert!(widths.iter().all(|w| *w == beat || *w % (4 * beat) == 0));
}

#[test]
fn adaptive_background_bands_restart_at_meter_origins() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
    editor
        .execute(Command::SetMeterPoint {
            beat: BeatTime::from_quarters(8).expect("meter beat"),
            meter: Meter::new(3, 4).expect("meter"),
        })
        .expect("meter point");
    let project = editor.project();
    let bands = adaptive_grid_bands(
        project,
        Viewport {
            start: 0,
            length: 384_000,
        },
        640.0,
    );
    let meter_origin = project
        .tempo_map()
        .beat_to_frame(BeatTime::from_quarters(8).expect("meter beat"))
        .expect("meter origin frame");
    let band = bands
        .iter()
        .find(|band| band.start_frame == meter_origin)
        .expect("new meter section begins a band at its musical origin");
    assert!(!band.alternate, "band parity restarts at the meter origin");
}

#[test]
fn conversion_failures_cannot_make_band_candidate_enumeration_unbounded() {
    let attempts = std::cell::Cell::new(0);
    let produced = super::bounded_band_indices(0, u64::MAX, MAX_GRID_BANDS)
        .filter_map(|_| {
            attempts.set(attempts.get() + 1);
            None::<super::GridBand>
        })
        .count();
    assert_eq!(produced, 0, "simulate every frame conversion failing");
    let attempted = attempts.get();
    assert_eq!(attempted, MAX_GRID_BANDS);
}

#[test]
fn grid_returns_empty_when_time_conversion_and_viewport_bounds_are_invalid() {
    let project = Project::new(48_000).expect("project");
    assert!(
        grid_lines(
            &project,
            Viewport {
                start: u64::MAX - 1,
                length: 1,
            },
            640.0,
            SnapGrid::Adaptive,
        )
        .is_empty()
    );
    assert!(grid_lines(&project, Viewport::fit(100), f32::NAN, SnapGrid::Adaptive).is_empty());
}

#[test]
fn adaptive_snap_matches_painted_landmarks_across_tempo_meter_and_zoom_seams() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let meter_beat = BeatTime::from_ratio(5, 2).expect("meter beat");
    let _ = editor
        .execute(Command::SetMeterPoint {
            beat: meter_beat,
            meter: Meter::new(3, 4).expect("meter"),
        })
        .expect("meter point");
    let _ = editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(2).expect("tempo beat"),
            tempo: Tempo::new(60_000_000).expect("tempo"),
        })
        .expect("tempo point");
    let project = editor.project();
    for viewport in [
        Viewport {
            start: 0,
            length: 72_000,
        },
        Viewport {
            start: 0,
            length: 288_000,
        },
        Viewport {
            start: 0,
            length: 1_152_000,
        },
    ] {
        let lines = grid_lines(project, viewport, 640.0, SnapGrid::Adaptive);
        assert!(!lines.is_empty());
        for pair in lines.windows(2) {
            let candidate = pair[0].frame + (pair[1].frame - pair[0].frame) / 2;
            let snapped = SnapGrid::Adaptive
                .snap_frame(project, candidate, viewport, 640.0)
                .expect("adaptive landmark");
            assert!(
                snapped == pair[0].frame || snapped == pair[1].frame,
                "snap {snapped} should use a painted neighboring landmark in {viewport:?}, pair={pair:?}, lines={lines:?}"
            );
        }
        assert!(lines.len() <= MAX_GRID_LINES);
    }

    let seam = project
        .tempo_map()
        .beat_to_frame(meter_beat)
        .expect("meter seam");
    let viewport = Viewport {
        start: 0,
        length: 288_000,
    };
    let lines = grid_lines(project, viewport, 640.0, SnapGrid::Adaptive);
    assert!(
        lines
            .iter()
            .any(|line| line.frame == seam && line.meter_marker)
    );
    for candidate in [seam.saturating_sub(1), seam, seam.saturating_add(1)] {
        let snapped = SnapGrid::Adaptive
            .snap_frame(project, candidate, viewport, 640.0)
            .expect("landmark around meter seam");
        assert!(lines.iter().any(|line| line.frame == snapped));
    }
}
