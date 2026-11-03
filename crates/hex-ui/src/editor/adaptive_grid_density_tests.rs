use super::*;
use hex_project::{Command, Editor, Meter, Tempo};

#[test]
fn adaptive_density_presets_resolve_distinct_current_zoom_without_changing_default() {
    let project = Project::new(48_000).unwrap();
    let viewport = Viewport::fit(288_000);
    let mut intervals = Vec::new();
    for density in AdaptiveGridDensity::ALL {
        let grid = SnapGrid::Adaptive.with_density(density);
        let lines = grid_lines(&project, viewport, 640.0, grid);
        intervals.push(lines[1].frame - lines[0].frame);
        assert_eq!(grid.density(), density);
        assert!(density.label().starts_with("adaptive grid"));
    }
    assert_eq!(intervals, [6_000, 12_000, 24_000]);
    assert_eq!(SnapGrid::default().density(), AdaptiveGridDensity::Medium);
    assert_eq!(
        grid_lines(&project, viewport, 640.0, SnapGrid::Adaptive),
        grid_lines(
            &project,
            viewport,
            640.0,
            SnapGrid::Adaptive.with_density(AdaptiveGridDensity::Medium)
        )
    );
}

#[test]
fn adaptive_density_survives_toggle_and_temporary_inversion_without_enabling_off() {
    let project = Project::new(48_000).unwrap();
    for density in AdaptiveGridDensity::ALL {
        let enabled = SnapGrid::Adaptive.with_density(density);
        let disabled = enabled.toggled();
        assert!(disabled.is_off());
        assert_eq!(disabled.density(), density);
        assert_eq!(disabled.toggled(), enabled);
        assert_eq!(enabled.with_platform_modifier(true), disabled);
        assert_eq!(disabled.with_platform_modifier(true), enabled);
        assert_eq!(disabled.with_platform_modifier(false), disabled);
        assert_eq!(
            disabled.snap_frame(&project, 12_345, Viewport::fit(288_000), 640.0),
            Some(12_345)
        );
        assert!(grid_lines(&project, Viewport::fit(288_000), 640.0, disabled).is_empty());
        for replacement in AdaptiveGridDensity::ALL {
            assert!(disabled.with_density(replacement).is_off());
            assert_eq!(disabled.with_density(replacement).density(), replacement);
        }
    }
}

#[test]
fn adaptive_density_painted_snap_and_directional_landmarks_match_through_zoom_and_map_seams() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
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
    for density in AdaptiveGridDensity::ALL {
        let grid = SnapGrid::Adaptive.with_density(density);
        let mut counts = Vec::new();
        for length in [72_000, 288_000, 1_152_000] {
            let viewport = Viewport::fit(length);
            let lines = grid_lines(project, viewport, 640.0, grid);
            assert!(!lines.is_empty());
            assert!(lines.len() <= MAX_GRID_LINES);
            counts.push(lines.len());
            for pair in lines.windows(2) {
                let candidate = pair[0].frame + (pair[1].frame - pair[0].frame) / 2;
                let snap = grid
                    .snap_frame(project, candidate, viewport, 640.0)
                    .unwrap();
                assert!(
                    snap == pair[0].frame || snap == pair[1].frame,
                    "{density:?} {viewport:?} {pair:?} -> {snap}"
                );
                assert_eq!(
                    grid.adjacent_frame(project, pair[0].frame, viewport, 640.0, true),
                    Some(pair[1].frame)
                );
            }
        }
        assert!(counts.iter().all(|count| *count > 1));
    }
}

#[test]
fn adaptive_density_bands_hold_at_least_four_units_and_stay_bounded() {
    let project = Project::new(48_000).unwrap();
    for density in AdaptiveGridDensity::ALL {
        let grid = SnapGrid::Adaptive.with_density(density);
        let viewport = Viewport::fit(288_000);
        let lines = grid_lines(&project, viewport, 640.0, grid);
        let bands = adaptive_grid_bands_for_grid(&project, viewport, 640.0, grid);
        // Bands hold at least four grid units, widened to a musical level.
        let span = bands[0].end_frame - bands[0].start_frame;
        assert!(span >= (lines[1].frame - lines[0].frame) * 4);
        assert!(span == 24_000 || span % 96_000 == 0, "{span}");
        for length in [1, 72_000, u64::MAX / 2] {
            let viewport = Viewport::fit(length);
            assert!(grid_lines(&project, viewport, 100_000.0, grid).len() <= MAX_GRID_LINES);
            assert!(
                adaptive_grid_bands_for_grid(&project, viewport, 640.0, grid).len()
                    <= MAX_GRID_BANDS
            );
        }
        assert!(grid_lines(&project, viewport, f32::NAN, grid).is_empty());
        assert!(adaptive_grid_bands_for_grid(&project, viewport, f32::INFINITY, grid).is_empty());
    }
}

#[test]
fn adaptive_density_wide_view_never_snaps_to_hidden_subdivision() {
    let project = Project::new(48_000).unwrap();
    for density in AdaptiveGridDensity::ALL {
        for width in [160.0, 640.0, 1920.0, 8192.0, 100_000.0] {
            for length in [72_000, 288_000, 1_152_000, 1_000_000_000, u64::MAX / 2] {
                let viewport = Viewport::fit(length);
                let grid = SnapGrid::Adaptive.with_density(density);
                let lines = grid_lines(&project, viewport, width, grid);
                assert!(lines.len() <= MAX_GRID_LINES);
                for pair in lines.windows(2) {
                    let candidate = pair[0].frame + (pair[1].frame - pair[0].frame) / 2;
                    let snap = grid
                        .snap_frame(&project, candidate, viewport, width)
                        .unwrap();
                    assert!(
                        lines.iter().any(|line| line.frame == snap),
                        "hidden snap {snap}: density={density:?} width={width} length={length} pair={pair:?}"
                    );
                    assert_eq!(
                        grid.adjacent_frame(&project, pair[0].frame, viewport, width, true),
                        Some(pair[1].frame),
                        "directional density={density:?} width={width} length={length}"
                    );
                }
            }
        }
    }
}

#[test]
fn adaptive_density_over_budget_meter_markers_share_bounded_visible_targets() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    for index in 1..=260 {
        editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::from_quarters(index).unwrap(),
                meter: Meter::new(if index % 2 == 0 { 4 } else { 3 }, 4).unwrap(),
            })
            .unwrap();
    }
    let project = editor.project();
    let viewport = Viewport::fit(6_500_000);
    for density in AdaptiveGridDensity::ALL {
        let grid = SnapGrid::Adaptive.with_density(density);
        let lines = grid_lines(project, viewport, 8192.0, grid);
        assert!(!lines.is_empty());
        assert!(lines.len() <= MAX_GRID_LINES);
        for pair in lines.windows(2).step_by((lines.len() / 8).max(1)) {
            let candidate = pair[0].frame + (pair[1].frame - pair[0].frame) / 2;
            let snap = grid
                .snap_frame(project, candidate, viewport, 8192.0)
                .unwrap();
            assert!(lines.iter().any(|line| line.frame == snap));
            assert_eq!(
                grid.adjacent_frame(project, pair[0].frame, viewport, 8192.0, true),
                Some(pair[1].frame)
            );
        }
    }
}

#[test]
fn adaptive_review_panning_preserves_subdivision_at_fixed_zoom() {
    let project = Project::new(48_000).unwrap();
    for density in AdaptiveGridDensity::ALL {
        let grid = SnapGrid::Adaptive.with_density(density);
        let mut intervals = Vec::new();
        for start in [0, 1, 749, 1_499, 1_500] {
            let viewport = Viewport {
                start,
                length: 382_501,
            };
            let lines = grid_lines(&project, viewport, 100_000.0, grid);
            assert!(lines.len() > 2 && lines.len() <= MAX_GRID_LINES);
            intervals.push(lines[1].frame - lines[0].frame);
            for pair in lines.windows(2) {
                let snapped = grid
                    .snap_frame(&project, pair[0].frame + 1, viewport, 100_000.0)
                    .unwrap();
                assert!(lines.iter().any(|line| line.frame == snapped));
            }
        }
        assert!(
            intervals.iter().all(|step| *step == intervals[0]),
            "fixed zoom {density:?} must not change density with pan phase: {intervals:?}"
        );
    }
}

fn review_dense_meter_project() -> Editor {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    for index in 1..=260 {
        editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::from_quarters(index).unwrap(),
                meter: Meter::new(if index % 2 == 0 { 4 } else { 3 }, 4).unwrap(),
            })
            .unwrap();
    }
    editor
}

#[test]
fn adaptive_review_dense_visible_markers_use_available_bounded_budget() {
    let editor = review_dense_meter_project();
    let viewport = Viewport::fit(6_500_000);
    for density in AdaptiveGridDensity::ALL {
        let grid = SnapGrid::Adaptive.with_density(density);
        let lines = grid_lines(editor.project(), viewport, 8192.0, grid);
        assert_eq!(
            lines.len(),
            MAX_GRID_LINES,
            "enough real candidates exist; do not discard half the paint budget"
        );
        assert!(lines.windows(2).all(|pair| pair[0].frame < pair[1].frame));
        for pair in lines.windows(2).step_by(17) {
            let snapped = grid
                .snap_frame(editor.project(), pair[0].frame + 1, viewport, 8192.0)
                .unwrap();
            assert!(lines.iter().any(|line| line.frame == snapped));
        }
    }
}

#[test]
fn adaptive_review_offscreen_meter_history_does_not_allocate_paint_lists_for_snap() {
    let editor = review_dense_meter_project();
    let project = editor.project();
    let viewport = Viewport {
        start: 7_200_000,
        length: 288_000,
    };
    for density in AdaptiveGridDensity::ALL {
        let grid = SnapGrid::Adaptive.with_density(density);
        let before = GRID_LINE_BUILDS.with(std::cell::Cell::get);
        let snapped = grid
            .snap_frame(project, viewport.start + 12_345, viewport, 640.0)
            .unwrap();
        assert_eq!(
            GRID_LINE_BUILDS.with(std::cell::Cell::get),
            before,
            "offscreen history must not reconstruct an allocating painted landmark list on each snap"
        );
        let next = grid
            .adjacent_frame(project, snapped, viewport, 640.0, true)
            .unwrap();
        assert_eq!(
            GRID_LINE_BUILDS.with(std::cell::Cell::get),
            before,
            "offscreen history must not reconstruct paint lists for directional edits either"
        );
        let lines = grid_lines(project, viewport, 640.0, grid);
        assert!(lines.iter().any(|line| line.frame == snapped));
        assert!(lines.iter().any(|line| line.frame == next));
    }
}

#[test]
fn adaptive_review_meter_origins_do_not_consume_a_second_landmark_quota() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    for index in 1..100 {
        editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::from_quarters(index * 64).unwrap(),
                meter: Meter::new(if index % 2 == 0 { 4 } else { 3 }, 4).unwrap(),
            })
            .unwrap();
    }
    let project = editor.project();
    let length = project
        .tempo_map()
        .beat_to_frame(BeatTime::from_quarters(6_400).unwrap())
        .unwrap();
    for density in AdaptiveGridDensity::ALL {
        let grid = SnapGrid::Adaptive.with_density(density);
        for start in [0, 1, 3_000] {
            let viewport = Viewport { start, length };
            let lines = grid_lines(project, viewport, 100_000.0, grid);
            assert!(
                lines.len() >= 199 && lines.len() <= MAX_GRID_LINES,
                "100 long sections have room for two regular landmarks each; origins already share meter markers: {density:?} start={start} lines={}",
                lines.len()
            );
            assert!(lines.windows(2).all(|pair| pair[0].frame < pair[1].frame));
            for pair in lines.windows(2).step_by(17) {
                let candidate = pair[0].frame + 1;
                let snapped = grid
                    .snap_frame(project, candidate, viewport, 100_000.0)
                    .unwrap();
                assert!(
                    lines.iter().any(|line| line.frame == snapped),
                    "snap must remain painted"
                );
                assert_eq!(
                    grid.adjacent_frame(project, pair[0].frame, viewport, 100_000.0, true),
                    Some(pair[1].frame)
                );
                assert_eq!(
                    grid.with_platform_modifier(true)
                        .snap_frame(project, candidate, viewport, 100_000.0),
                    Some(candidate)
                );
            }
        }
    }
}

#[test]
fn adaptive_review_frame_quantized_edge_markers_remain_painted_snap_targets() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let viewport = Viewport {
        start: 1,
        length: 1_000,
    };
    let first = editor.project().tempo_map().frame_to_beat(1).unwrap();
    let last = editor.project().tempo_map().frame_to_beat(1_000).unwrap();
    for (beat, numerator) in [
        (BeatTime::from_raw(first.raw() - 1), 3),
        (BeatTime::from_raw(last.raw() + 1), 4),
    ] {
        editor
            .execute(Command::SetMeterPoint {
                beat,
                meter: Meter::new(numerator, 4).unwrap(),
            })
            .unwrap();
    }
    let project = editor.project();
    assert_eq!(
        project
            .tempo_map()
            .beat_to_frame(BeatTime::from_raw(first.raw() - 1))
            .unwrap(),
        1
    );
    assert_eq!(
        project
            .tempo_map()
            .beat_to_frame(BeatTime::from_raw(last.raw() + 1))
            .unwrap(),
        1_000
    );
    for density in AdaptiveGridDensity::ALL {
        let grid = SnapGrid::Adaptive.with_density(density);
        let lines = grid_lines(project, viewport, 100_000.0, grid);
        assert_eq!(
            lines.iter().map(|line| line.frame).collect::<Vec<_>>(),
            vec![1, 1_000]
        );
        for candidate in [2, 500, 999] {
            let snapped = grid
                .snap_frame(project, candidate, viewport, 100_000.0)
                .unwrap();
            assert!(lines.iter().any(|line| line.frame == snapped));
        }
        assert_eq!(
            grid.adjacent_frame(project, 1, viewport, 100_000.0, true),
            Some(1_000)
        );
    }
}

#[test]
fn adaptive_review_section_remainder_preserves_painted_and_snap_density() {
    for section_count in [128, 129] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        for index in 1..section_count {
            editor
                .execute(Command::SetMeterPoint {
                    beat: BeatTime::from_quarters(index * 64).unwrap(),
                    meter: Meter::new(if index % 2 == 0 { 4 } else { 3 }, 4).unwrap(),
                })
                .unwrap();
        }
        let project = editor.project();
        let length = project
            .tempo_map()
            .beat_to_frame(BeatTime::from_quarters(section_count * 64).unwrap())
            .unwrap();
        for density in AdaptiveGridDensity::ALL {
            let grid = SnapGrid::Adaptive.with_density(density);
            for start in [0, 1, 3_000] {
                let viewport = Viewport { start, length };
                let lines = grid_lines(project, viewport, 100_000.0, grid);
                eprintln!(
                    "section remainder sections={section_count} density={density:?} start={start} painted={}",
                    lines.len()
                );
                // The same 256-slot budget supports 128*2 and 127*2+2*1.
                // Panning can exclude the first origin, never half the grid.
                assert_eq!(lines.len(), MAX_GRID_LINES - usize::from(start != 0));
                assert!(lines.windows(2).all(|pair| pair[0].frame < pair[1].frame));
                for pair in lines.windows(2).step_by(17) {
                    let candidate = pair[0].frame + 1;
                    assert_eq!(
                        grid.snap_frame(project, pair[0].frame, viewport, 100_000.0),
                        Some(pair[0].frame)
                    );
                    let snapped = grid
                        .snap_frame(project, candidate, viewport, 100_000.0)
                        .unwrap();
                    assert!(lines.iter().any(|line| line.frame == snapped));
                    assert_eq!(
                        grid.adjacent_frame(project, pair[0].frame, viewport, 100_000.0, true),
                        Some(pair[1].frame)
                    );
                    assert_eq!(
                        grid.with_platform_modifier(true)
                            .snap_frame(project, candidate, viewport, 100_000.0),
                        Some(candidate)
                    );
                }
            }
        }
    }
}

#[test]
fn adaptive_review_mixed_short_sections_reassign_unused_paint_and_snap_capacity() {
    // Independent one-slot-per-round oracle, including empty/zero/huge demands.
    // The helper has at most nine level probes plus its final sum, separately
    // from demand evaluation cost; a256-wide inclusive domain has257 outcomes.
    for demands in [
        vec![],
        vec![0],
        vec![1],
        vec![usize::MAX],
        vec![0, 1, usize::MAX, 1],
        vec![2, 2, 2],
        vec![0, 0, 0],
    ] {
        for budget in [0, 1, 255, 256] {
            let probes = std::cell::Cell::new(0);
            let (level, mut remainder) = capped_fair_share(demands.len(), budget, |index| {
                probes.set(probes.get() + 1);
                demands[index]
            });
            assert!(probes.get() <= 10 * demands.len());
            let actual: Vec<_> = demands
                .iter()
                .map(|demand| {
                    let extra = usize::from(*demand > level && remainder > 0);
                    remainder -= extra;
                    demand.min(&level) + extra
                })
                .collect();
            let mut expected = vec![0; demands.len()];
            let mut remaining = budget;
            while remaining > 0 {
                let before = remaining;
                for (value, demand) in expected.iter_mut().zip(&demands) {
                    if remaining > 0 && *value < *demand {
                        *value += 1;
                        remaining -= 1;
                    }
                }
                if before == remaining {
                    break;
                }
            }
            assert_eq!(actual, expected);
        }
    }
    let mut counts = Vec::new();
    for long_first in [false, true] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let mut quarter = 0;
        for section in 0..129 {
            if section > 0 {
                editor
                    .execute(Command::SetMeterPoint {
                        beat: BeatTime::from_quarters(quarter).unwrap(),
                        meter: Meter::new(if section % 2 == 0 { 4 } else { 3 }, 4).unwrap(),
                    })
                    .unwrap();
            }
            quarter += if section == if long_first { 0 } else { 128 } {
                8192
            } else {
                1
            };
        }
        let project = editor.project();
        // Keep all three pans inside the same last section: no extra
        // 0.125-quarter tail beyond the authored test span changes its quota.
        let length = project
            .tempo_map()
            .beat_to_frame(BeatTime::from_quarters(quarter).unwrap())
            .unwrap()
            - 3000;
        for density in AdaptiveGridDensity::ALL {
            let grid = SnapGrid::Adaptive.with_density(density);
            for start in [0, 1, 3000] {
                let viewport = Viewport { start, length };
                let paint_start = std::time::Instant::now();
                let lines = grid_lines(project, viewport, 100_000.0, grid);
                let paint_us = paint_start.elapsed().as_micros();
                let snap_start = std::time::Instant::now();
                let _ = grid.snap_frame(project, viewport.start + 1, viewport, 100_000.0);
                let snap_us = snap_start.elapsed().as_micros();
                let adjacent_start = std::time::Instant::now();
                let _ = grid.adjacent_frame(project, viewport.start, viewport, 100_000.0, true);
                let adjacent_us = adjacent_start.elapsed().as_micros();
                eprintln!(
                    "debug_visible_work sections=129 paint_us={paint_us} snap_us={snap_us} adjacent_us={adjacent_us}; no optimized/native deadline claim"
                );
                eprintln!(
                    "mixed sections=129 short=128 long_first={long_first} density={density:?} start={start} painted={}",
                    lines.len()
                );
                counts.push((long_first, density, start, lines.len()));
                assert!(lines.windows(2).all(|p| p[0].frame < p[1].frame));
                for pair in lines.windows(2).step_by(17) {
                    assert_eq!(
                        grid.snap_frame(project, pair[0].frame, viewport, 100_000.0),
                        Some(pair[0].frame)
                    );
                    let candidate = pair[0].frame + 1;
                    let snapped = grid.snap_frame(project, candidate, viewport, 100_000.0);
                    assert!(lines.iter().any(|line| Some(line.frame) == snapped));
                    assert_eq!(
                        grid.adjacent_frame(project, pair[0].frame, viewport, 100_000.0, true),
                        Some(pair[1].frame)
                    );
                    assert_eq!(
                        grid.with_platform_modifier(true)
                            .snap_frame(project, candidate, viewport, 100_000.0),
                        Some(candidate)
                    );
                }
            }
        }
    }
    for (long_first, density, start, count) in counts {
        assert_eq!(
            count,
            MAX_GRID_LINES - usize::from(start != 0),
            "unused short-section capacity: long_first={long_first} density={density:?} start={start}"
        );
    }
}

#[test]
fn adaptive_operation_work_profile() {
    for count in [1, 129, 256, 1024] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        for index in 1..count {
            editor
                .execute(Command::SetMeterPoint {
                    beat: BeatTime::from_quarters(index * 64).unwrap(),
                    meter: Meter::new(if index % 2 == 0 { 4 } else { 3 }, 4).unwrap(),
                })
                .unwrap();
        }
        let project = editor.project();
        let length = project
            .tempo_map()
            .beat_to_frame(BeatTime::from_quarters(count * 64).unwrap())
            .unwrap();
        let viewport = Viewport::fit(length);
        for density in AdaptiveGridDensity::ALL {
            let grid = SnapGrid::Adaptive.with_density(density);
            for operation in ["paint", "ruler", "snap", "next", "previous", "bands"] {
                GRID_RESOLUTION_WORK.with(|work| work.set([0; 4]));
                let start = std::time::Instant::now();
                match operation {
                    "paint" => {
                        std::hint::black_box(grid_lines(project, viewport, 100_000.0, grid));
                    }
                    "ruler" => {
                        std::hint::black_box(ruler_ticks(project, viewport, 100_000.0));
                    }
                    "snap" => {
                        std::hint::black_box(grid.snap_frame(
                            project,
                            length / 2 + 123,
                            viewport,
                            100_000.0,
                        ));
                    }
                    "next" => {
                        std::hint::black_box(grid.adjacent_frame(
                            project,
                            length / 2,
                            viewport,
                            100_000.0,
                            true,
                        ));
                    }
                    "previous" => {
                        std::hint::black_box(grid.adjacent_frame(
                            project,
                            length / 2,
                            viewport,
                            100_000.0,
                            false,
                        ));
                    }
                    "bands" => {
                        std::hint::black_box(adaptive_grid_bands_for_grid(
                            project, viewport, 100_000.0, grid,
                        ));
                    }
                    _ => unreachable!(),
                }
                let ns = start.elapsed().as_nanos();
                let work = GRID_RESOLUTION_WORK.with(std::cell::Cell::get);
                // One natural-demand pass per operation, even when nearest/
                // directional lookup falls back to the painted identities.
                assert!(
                    work[3] <= count.min(MAX_GRID_LINES as u64) as usize,
                    "{operation}: {work:?}"
                );
                assert!(work[2] <= 2, "{operation}: {work:?}");
                println!(
                    "GRID_WORK count={count} density={density:?} operation={operation} step={} base={} shares={} demand={} ns={ns}",
                    work[0], work[1], work[2], work[3]
                );
            }
        }
    }
}
