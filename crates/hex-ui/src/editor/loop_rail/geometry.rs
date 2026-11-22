//! Pure loop-rail hit-testing, pixel projection, and musical-length geometry.
use super::super::seek;
use super::super::viewport::Viewport;
use gpui::Bounds;
use hex_project::LoopRegion;

pub(super) const EDGE_HIT_PIXELS: f32 = 6.0;

#[derive(Clone, Copy, Debug)]
pub(super) enum EditMode {
    Paint { anchor: u64 },
    Move { original: LoopRegion, anchor: u64 },
    ResizeStart { fixed_end: u64, enabled: bool },
    ResizeEnd { fixed_start: u64, enabled: bool },
}

// Share the exact hit budget between hover and capture. Narrow bars retain a
// central body; the full outside endpoint radius remains usable at every width.
pub(super) fn region_edit_mode(
    region: LoopRegion,
    viewport: Viewport,
    bounds: Bounds<gpui::Pixels>,
    x: f32,
) -> Option<EditMode> {
    let left = f32::from(bounds.origin.x);
    let width = f32::from(bounds.size.width);
    let frame = seek::pointer_to_frame(x, left, width, viewport)?;
    let (start_x, end_x) = visible_region_pixels(region, viewport, left, width)?;
    let radius = if end_x > start_x && x >= start_x && x <= end_x {
        EDGE_HIT_PIXELS.min((end_x - start_x) / 3.0)
    } else {
        EDGE_HIT_PIXELS
    };
    let start_distance = if region.start() >= viewport.start {
        (x - start_x).abs()
    } else {
        f32::INFINITY
    };
    let end_distance = if viewport.end().is_some_and(|end| region.end() <= end) {
        (x - end_x).abs()
    } else {
        f32::INFINITY
    };
    if start_distance <= radius && start_distance <= end_distance {
        Some(EditMode::ResizeStart {
            fixed_end: region.end(),
            enabled: region.enabled(),
        })
    } else if end_distance <= radius {
        Some(EditMode::ResizeEnd {
            fixed_start: region.start(),
            enabled: region.enabled(),
        })
    } else if (frame >= region.start() && frame < region.end())
        || (end_x > start_x && x >= start_x && x <= end_x)
    {
        // A visible body remains a body when frame rounding reaches its end.
        Some(EditMode::Move {
            original: region,
            anchor: frame,
        })
    } else {
        None
    }
}

// Closed viewport edges retain point intersections for endpoint resize capture.
pub(super) fn visible_region_pixels(
    region: LoopRegion,
    viewport: Viewport,
    left: f32,
    width: f32,
) -> Option<(f32, f32)> {
    let viewport_end = viewport.end()?;
    if viewport.length == 0 || width <= 0.0 {
        return None;
    }
    let start = region.start().max(viewport.start);
    let end = region.end().min(viewport_end);
    if start > end {
        return None;
    }
    let start_x = left + ((start - viewport.start) as f64 / viewport.length as f64) as f32 * width;
    let end_x = left + ((end - viewport.start) as f64 / viewport.length as f64) as f32 * width;
    Some((start_x, end_x))
}

/// Full-frame loop-length steps relative to the original start, including the
/// origin when a further full negative/positive step would exceed checked bounds.
pub(super) fn stepped_start(region: LoopRegion, delta: i128) -> Option<u64> {
    let length = region.end().checked_sub(region.start())?;
    if length == 0 {
        return None;
    }
    let magnitude = (delta.unsigned_abs() + u128::from(length) / 2) / u128::from(length);
    let magnitude = i128::try_from(magnitude).ok()?;
    let steps = if delta < 0 { -magnitude } else { magnitude };
    let steps = steps.clamp(
        -i128::from(region.start() / length),
        i128::from((u64::MAX - region.end()) / length),
    );
    u64::try_from(i128::from(region.start()) + steps * i128::from(length)).ok()
}

/// Frame endpoints retain quantized musical intervals, not authored beat IDs.
/// Accept exactly one binary Q32.32 duration whose canonical forward rounding can
/// place its two beat endpoints at these stored frames (no pixel/epsilon test).
/// Arrangement time-selection markers share this same read-only predicate.
pub(crate) fn binary_duration(project: &hex_project::Project, region: LoopRegion) -> bool {
    let map = project.tempo_map();
    let interval = |frame: u64| -> Option<(u128, u128)> {
        let first = map.beat_lower_bound_for_frame(frame).ok()?;
        let after = map.beat_lower_bound_for_frame(frame.checked_add(1)?).ok()?;
        (first < after && first < hex_project::TempoMap::BEAT_RANGE_SENTINEL)
            .then_some((first, after.checked_sub(1)?))
    };
    let Some((start_lo, start_hi)) = interval(region.start()) else {
        return false;
    };
    let Some((end_lo, end_hi)) = interval(region.end()) else {
        return false;
    };
    let minimum = end_lo.saturating_sub(start_hi).max(1);
    let maximum = end_hi.saturating_sub(start_lo);
    minimum
        .checked_next_power_of_two()
        .is_some_and(|candidate| {
            candidate <= maximum
                && candidate < hex_project::TempoMap::BEAT_RANGE_SENTINEL
                && candidate.checked_mul(2).is_none_or(|next| next > maximum)
        })
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{
        EditMode, binary_duration, region_edit_mode, stepped_start, visible_region_pixels,
    };
    use crate::editor::viewport::Viewport;
    use hex_project::LoopRegion;

    #[test]
    fn visible_range_clips_loop_edges_to_zoomed_viewport() {
        let viewport = Viewport {
            start: 100,
            length: 100,
        };
        let region = LoopRegion::new(50, 150, true).expect("valid loop");
        assert_eq!(
            visible_region_pixels(region, viewport, 10.0, 200.0),
            Some((10.0, 110.0))
        );
    }

    #[test]
    fn edge_point_ranges_keep_the_existing_resize_owner() {
        let viewport = Viewport {
            start: 100,
            length: 100,
        };
        let bounds = gpui::Bounds::new(
            gpui::point(gpui::px(10.0), gpui::px(0.0)),
            gpui::size(gpui::px(200.0), gpui::px(18.0)),
        );
        assert!(matches!(
            region_edit_mode(
                LoopRegion::new(50, 100, true).unwrap(),
                viewport,
                bounds,
                12.0
            ),
            Some(EditMode::ResizeEnd {
                fixed_start: 50,
                ..
            })
        ));
        assert!(matches!(
            region_edit_mode(
                LoopRegion::new(200, 250, true).unwrap(),
                viewport,
                bounds,
                208.0
            ),
            Some(EditMode::ResizeStart { fixed_end: 250, .. })
        ));
        for (start, end) in [(50, 99), (201, 250)] {
            assert!(
                region_edit_mode(
                    LoopRegion::new(start, end, true).unwrap(),
                    viewport,
                    bounds,
                    12.0
                )
                .is_none()
            );
        }
    }

    #[test]
    fn binary_marker_refuses_ambiguous_subframe_musical_durations() {
        let project = hex_project::Project::new(48_000).unwrap();
        assert!(!binary_duration(
            &project,
            LoopRegion::new(0, 1, false).unwrap()
        ));
    }

    #[test]
    fn musical_marker_uses_forward_rounding_across_tempo_points() {
        use hex_project::{BeatTime, Command, Editor, Project, Tempo};
        let mut editor = Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
        for (beat, bpm) in [(0, 137_000_000), (2, 93_000_000)] {
            editor
                .execute(Command::SetTempoPoint {
                    beat: BeatTime::from_raw(beat << 32),
                    tempo: Tempo::new(bpm).unwrap(),
                })
                .unwrap();
        }
        let project = editor.project();
        for start_raw in [0, 1 << 31, 1 << 32] {
            for (length, expected) in [
                (1 << 30, true),
                (1 << 31, true),
                (2 << 32, true),
                (3 << 32, false),
                (4 << 32, true),
            ] {
                let start = project
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(start_raw))
                    .unwrap();
                let end = project
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(start_raw + length))
                    .unwrap();
                assert_eq!(
                    binary_duration(project, LoopRegion::new(start, end, true).unwrap()),
                    expected,
                    "start={start_raw}, duration={length}, frames={start}..{end}"
                );
            }
        }
        assert!(!binary_duration(
            project,
            LoopRegion::new(u64::MAX - 1, u64::MAX, false).unwrap()
        ));
    }

    #[test]
    fn hit_budget_preserves_outside_capture_clipped_bodies_and_wide_edges() {
        use gpui::{point, px};
        let bounds = gpui::Bounds::new(point(px(100.0), px(0.0)), gpui::size(px(300.0), px(18.0)));
        let viewport = Viewport {
            start: 0,
            length: 300,
        };
        let kind = |region, viewport, x| match region_edit_mode(region, viewport, bounds, x) {
            Some(EditMode::Move { .. }) => "move",
            Some(EditMode::ResizeStart { .. }) => "start",
            Some(EditMode::ResizeEnd { .. }) => "end",
            Some(EditMode::Paint { .. }) | None => "none",
        };
        for width in [1, 8, 12, 18, 48] {
            let r = LoopRegion::new(100, 100 + width, true).unwrap();
            assert_eq!(kind(r, viewport, 200.0), "start");
            assert_eq!(kind(r, viewport, 200.0 + width as f32), "end");
            assert_eq!(kind(r, viewport, 194.0), "start");
            assert_eq!(kind(r, viewport, 193.9), "none");
            assert_eq!(kind(r, viewport, 206.0 + width as f32), "end");
            assert_eq!(kind(r, viewport, 206.1 + width as f32), "none");
            assert_eq!(kind(r, viewport, 200.0 + width as f32 / 2.0), "move");
        }
        let wide = LoopRegion::new(100, 148, false).unwrap();
        assert_eq!(kind(wide, viewport, 206.0), "start");
        assert_eq!(kind(wide, viewport, 206.1), "move");
        assert_eq!(kind(wide, viewport, 241.9), "move");
        assert_eq!(kind(wide, viewport, 242.0), "end");
        // A clipped-away start cannot steal the visible body, even when only 3px remain.
        let clipped = Viewport {
            start: 145,
            length: 300,
        };
        assert_eq!(kind(wide, clipped, 100.0), "move");
        assert_eq!(kind(wide, clipped, 103.0), "end");
        let clipped_end = Viewport {
            start: 0,
            length: 110,
        };
        assert_eq!(kind(wide, clipped_end, 400.0), "move");
    }

    #[test]
    fn length_steps_keep_relative_origin_ties_and_checked_timeline_bounds() {
        let region = LoopRegion::new(65, 95, false).unwrap();
        for (delta, expected) in [
            (-1000, 5),
            (-46, 5),
            (-45, 5),
            (-44, 35),
            (-15, 35),
            (-14, 65),
            (14, 65),
            (15, 95),
            (44, 95),
            (45, 125),
        ] {
            assert_eq!(stepped_start(region, delta), Some(expected));
        }
        let last = LoopRegion::new(u64::MAX - 65, u64::MAX - 35, true).unwrap();
        assert_eq!(stepped_start(last, 1000), Some(u64::MAX - 35));
        assert_eq!(stepped_start(last, i128::MAX), Some(u64::MAX - 35));
        assert_eq!(stepped_start(region, i128::MIN), Some(5));
    }
}
