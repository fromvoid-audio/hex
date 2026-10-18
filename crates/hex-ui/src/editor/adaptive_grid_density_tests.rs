use super::*;
use hex_project::{Editor, Tempo};

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
fn adaptive_density_bands_share_selected_four_unit_span_and_stay_bounded() {
    let project = Project::new(48_000).unwrap();
    for density in AdaptiveGridDensity::ALL {
        let grid = SnapGrid::Adaptive.with_density(density);
        let viewport = Viewport::fit(288_000);
        let lines = grid_lines(&project, viewport, 640.0, grid);
        let bands = adaptive_grid_bands_for_grid(&project, viewport, 640.0, grid);
        assert_eq!(
            bands[0].end_frame - bands[0].start_frame,
            (lines[1].frame - lines[0].frame) * 4
        );
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
