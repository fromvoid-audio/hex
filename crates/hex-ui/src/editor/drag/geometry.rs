//! Pointer and lane geometry shared by clip dragging and its consumers.

/// Clip edge selected once from the original horizontal press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) enum ClipSnapEdge {
    Leading,
    Trailing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::editor) struct LaneBounds {
    pub(in crate::editor) left: f32,
    pub(in crate::editor) width: f32,
    pub(in crate::editor) top: f32,
    pub(in crate::editor) height: f32,
}

pub(super) fn lane_contains_pointer(pointer_x: f32, pointer_y: f32, lane: LaneBounds) -> bool {
    let bottom = lane.top + lane.height;
    let right = lane.left + lane.width;
    pointer_x.is_finite()
        && pointer_y.is_finite()
        && lane.top.is_finite()
        && lane.left.is_finite()
        && lane.height.is_finite()
        && lane.width.is_finite()
        && lane.height > 0.0
        && lane.width > 0.0
        && pointer_x >= lane.left
        && pointer_x < right
        && pointer_y >= lane.top
        && pointer_y < bottom
}

/// Convert the pointer's position in the visible part of a clip to its offset
/// from the clip's true start, including any portion cropped by the viewport.
pub(in crate::editor) fn grab_offset(
    local_x: f32,
    element_width: f32,
    visible_start: u64,
    visible_end: u64,
    clip_start: u64,
) -> Option<u64> {
    let visible_frames = visible_end.checked_sub(visible_start)?;
    if visible_frames == 0 || !local_x.is_finite() {
        return None;
    }
    if !element_width.is_finite() || element_width <= 0.0 {
        return None;
    }
    let fraction = (f64::from(local_x) / f64::from(element_width)).clamp(0.0, 1.0);
    let within_visible = (fraction * visible_frames as f64).floor() as u64;
    visible_start
        .checked_add(within_visible.min(visible_frames))?
        .checked_sub(clip_start)
}

/// Map the original mouse-down point, not GPUI's later threshold-crossing point, into clip frames.
pub(in crate::editor) fn grab_offset_from_pointer(
    mouse_down_x: f32,
    element_left: f32,
    element_width: f32,
    visible_start: u64,
    visible_end: u64,
    clip_start: u64,
) -> Option<u64> {
    let local_x = f64::from(mouse_down_x) - f64::from(element_left);
    if !local_x.is_finite() || local_x.abs() > f64::from(f32::MAX) {
        return None;
    }
    grab_offset(
        local_x as f32,
        element_width,
        visible_start,
        visible_end,
        clip_start,
    )
}

/// Choose once from the original press and full clip span, never from its
/// viewport-cropped fragment. Round the boundary to the pointer's logical pixel
/// representation so an exact two-thirds press belongs to the trailing third.
pub(in crate::editor) fn clip_snap_edge(
    pointer_x: f32,
    element_left: f32,
    element_width: f32,
    visible_start: u64,
    visible_end: u64,
    clip_start: u64,
    clip_end: u64,
) -> Option<ClipSnapEdge> {
    let visible = visible_end.checked_sub(visible_start)?;
    let duration = clip_end.checked_sub(clip_start)?;
    if visible == 0
        || duration == 0
        || visible_start < clip_start
        || visible_end > clip_end
        || !pointer_x.is_finite()
        || !element_left.is_finite()
        || !element_width.is_finite()
        || element_width <= 0.0
    {
        return None;
    }
    let start = (i128::from(clip_start) - i128::from(visible_start)) as f64;
    let threshold = f64::from(element_left)
        + (start + duration as f64 * (2.0 / 3.0)) / visible as f64 * f64::from(element_width);
    if !threshold.is_finite() || threshold.abs() > f64::from(f32::MAX) {
        return None;
    }
    Some(if pointer_x >= threshold as f32 {
        ClipSnapEdge::Trailing
    } else {
        ClipSnapEdge::Leading
    })
}

/// Resolve the drop point to a project frame and clamp a dragged clip at zero.
pub(in crate::editor) fn drop_start(
    pointer_x: f32,
    lane: LaneBounds,
    viewport_start: u64,
    viewport_length: u64,
    grab_offset: u64,
) -> Option<u64> {
    if !pointer_x.is_finite()
        || !lane.left.is_finite()
        || !lane.width.is_finite()
        || lane.width <= 0.0
        || viewport_length == 0
    {
        return None;
    }
    let fraction =
        ((f64::from(pointer_x) - f64::from(lane.left)) / f64::from(lane.width)).clamp(0.0, 1.0);
    let within_view = (fraction * viewport_length as f64).floor() as u64;
    let pointer_frame = viewport_start.checked_add(within_view.min(viewport_length))?;
    Some(pointer_frame.saturating_sub(grab_offset))
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{LaneBounds, drop_start, grab_offset, lane_contains_pointer};

    #[test]
    fn pointer_math_preserves_cropped_grab_offset_and_clamps_before_zero() {
        // The cropped leading 80 frames plus half of the visible 100 frames
        // place the grab point 130 frames from the clip's true start.
        assert_eq!(grab_offset(25.0, 50.0, 120, 220, 40), Some(130));
        let lane = LaneBounds {
            left: 10.0,
            width: 100.0,
            top: 0.0,
            height: 44.0,
        };
        assert_eq!(drop_start(10.0, lane, 0, 200, 15), Some(0));
        assert_eq!(drop_start(60.0, lane, 0, 200, 15), Some(85));
        assert_eq!(
            drop_start(
                f32::MAX,
                LaneBounds {
                    left: -f32::MAX,
                    width: 1.0,
                    top: 0.0,
                    height: 44.0,
                },
                0,
                200,
                0
            ),
            Some(200)
        );
    }

    #[test]
    fn pointer_math_rejects_invalid_geometry_and_overflow() {
        assert_eq!(grab_offset(1.0, 0.0, 0, 2, 0), None);
        assert_eq!(grab_offset(f32::NAN, 100.0, 0, 2, 0), None);
        assert_eq!(
            drop_start(
                1.0,
                LaneBounds {
                    left: 0.0,
                    width: 1.0,
                    top: 0.0,
                    height: 44.0,
                },
                u64::MAX,
                2,
                0,
            ),
            None
        );
    }

    #[test]
    fn initial_snap_edge_uses_full_span_and_includes_the_two_thirds_boundary() {
        use super::{
            ClipSnapEdge::{Leading, Trailing},
            clip_snap_edge,
        };
        for (x, expected) in [
            (100.0, Leading),
            (299.0, Leading),
            (300.0, Trailing),
            (390.0, Trailing),
        ] {
            assert_eq!(
                clip_snap_edge(x, 100.0, 300.0, 0, 900, 0, 900),
                Some(expected)
            );
        }
        // Only the last third is visible: its left edge is the real threshold.
        assert_eq!(
            clip_snap_edge(100.0, 100.0, 100.0, 600, 900, 0, 900),
            Some(Trailing)
        );
        // Only the first third is visible: even its right edge stays leading.
        assert_eq!(
            clip_snap_edge(200.0, 100.0, 100.0, 0, 300, 0, 900),
            Some(Leading)
        );
        assert_eq!(clip_snap_edge(f32::NAN, 0.0, 100.0, 0, 300, 0, 900), None);
        assert_eq!(clip_snap_edge(10.0, 0.0, 0.0, 0, 300, 0, 900), None);
        assert_eq!(clip_snap_edge(10.0, 0.0, 100.0, 0, 300, 1, 900), None);
    }

    #[test]
    fn hovered_track_geometry_rejects_gaps_and_non_finite_rows() {
        let first = LaneBounds {
            left: 0.0,
            width: 500.0,
            top: 100.0,
            height: 44.0,
        };
        let second = LaneBounds {
            top: 148.0,
            ..first
        };
        assert!(lane_contains_pointer(250.0, 100.0, first));
        assert!(lane_contains_pointer(250.0, 143.999, first));
        assert!(!lane_contains_pointer(250.0, 144.0, first));
        assert!(lane_contains_pointer(250.0, 148.0, second));
        assert!(!lane_contains_pointer(-1.0, 120.0, first));
        assert!(!lane_contains_pointer(501.0, 120.0, first));
        assert!(!lane_contains_pointer(f32::NAN, 120.0, first));
        assert!(!lane_contains_pointer(250.0, f32::NAN, first));
    }
}
