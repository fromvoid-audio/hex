//! Bounded, event-free focus decoration inside the shared section polygon.
use super::*;
use gpui::{div, rgba};

const MAX_HATCH_LINES: usize = 4096;

// Clip each parallel line against the visible rectangle and the chamfer's
// half-plane. The callback builds one path; no per-frame peak/project traversal
// or temporary segment allocation is needed.
fn segments(
    extent: [f32; 2],
    requested_cut: f32,
    visible: [f32; 4],
    scale: f32,
    style: theme::SectionFocusHatch,
    mut draw: impl FnMut([[f32; 2]; 2]),
) -> usize {
    let [width, height] = extent;
    if ![
        width,
        height,
        requested_cut,
        scale,
        style.angle_degrees,
        style.spacing,
        visible[0],
        visible[1],
        visible[2],
        visible[3],
    ]
    .into_iter()
    .all(f32::is_finite)
        || width <= 0.0
        || height <= 0.0
        || scale <= 0.0
        || style.spacing <= 0.0
        || !(0.1..89.9).contains(&style.angle_degrees)
    {
        return 0;
    }
    let inset = style.outline_device_px / scale;
    let [left, top, right, bottom] = [
        visible[0].max(inset),
        visible[1].max(inset),
        visible[2].min(width - inset),
        visible[3].min(height - inset),
    ];
    if !inset.is_finite() || left >= right || top >= bottom {
        return 0;
    }
    let slope = style.angle_degrees.to_radians().tan();
    let step = style.spacing * (1.0 + slope * slope).sqrt();
    let first = ((slope * left + top) / step).ceil();
    let last = ((slope * right + bottom) / step).floor();
    let count = (last - first + 1.0).max(0.0);
    let limit = width + height - cut(width, height, requested_cut) - inset * 2.0_f32.sqrt();
    if ![step, first, last, count, limit]
        .into_iter()
        .all(f32::is_finite)
        || step <= 0.0
        || count > MAX_HATCH_LINES as f32
    {
        return 0;
    }
    let mut drawn = 0;
    for index in 0..count as usize {
        let intercept = (first + index as f32) * step;
        let mut start = left.max((intercept - bottom) / slope);
        let mut end = right.min((intercept - top) / slope);
        let coefficient = 1.0 - slope;
        if coefficient.abs() < 0.000_01 {
            if intercept > limit {
                continue;
            }
        } else if coefficient > 0.0 {
            end = end.min((limit - intercept) / coefficient);
        } else {
            start = start.max((limit - intercept) / coefficient);
        }
        if start < end {
            draw([
                [start, (intercept - slope * start).clamp(top, bottom)],
                [end, (intercept - slope * end).clamp(top, bottom)],
            ]);
            drawn += 1;
        }
    }
    drawn
}

/// Insert before the miniature map and title. This layer owns no hitbox/input.
pub(crate) fn overlay(identity: ElementColor, chamfer: f32) -> Div {
    div().absolute().inset_0().children([canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scale = window.scale_factor();
            if !scale.is_finite() || scale <= 0.0 {
                return;
            }
            let visible = bounds.intersect(&window.content_mask().bounds);
            let style = theme::SECTION_FOCUS_HATCH;
            let mut path = PathBuilder::stroke(px(style.stroke_device_px / scale));
            let count = segments(
                [f32::from(bounds.size.width), f32::from(bounds.size.height)],
                chamfer,
                [
                    f32::from(visible.left() - bounds.left()),
                    f32::from(visible.top() - bounds.top()),
                    f32::from(visible.right() - bounds.left()),
                    f32::from(visible.bottom() - bounds.top()),
                ],
                scale,
                style,
                |[from, to]| {
                    path.move_to(point(
                        bounds.left() + px(from[0]),
                        bounds.top() + px(from[1]),
                    ));
                    path.line_to(point(bounds.left() + px(to[0]), bounds.top() + px(to[1])));
                },
            );
            if count > 0
                && let Ok(path) = path.build()
            {
                window.paint_path(path, rgba(theme::section_focus_ink(identity)));
            }
            paint_outline(
                bounds,
                chamfer,
                theme::color(theme::PaletteRole::Primary),
                style.outline_device_px,
                window,
            );
        },
    )
    .absolute()
    .inset_0()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hatches_stay_in_visible_chamfered_bounds_at_multiple_angles_and_scales() {
        for angle in [30.0, 45.0, 60.0] {
            for scale in [1.0, 1.5, 2.0] {
                let style = theme::SectionFocusHatch {
                    angle_degrees: angle,
                    ..theme::SECTION_FOCUS_HATCH
                };
                let mut lines = Vec::new();
                let count = segments(
                    [160.0, 60.0],
                    6.0,
                    [13.0, 5.0, 160.0, 60.0],
                    scale,
                    style,
                    |line| lines.push(line),
                );
                assert!(count > 0 && count < MAX_HATCH_LINES);
                assert_eq!(count, lines.len());
                for [from, to] in lines {
                    for [x, y] in [from, to] {
                        assert!(x >= 13.0 && x <= 160.0 && y >= 5.0 && y <= 60.0);
                        assert!(x + y <= 214.0 + 0.0001, "chamfer stays transparent");
                    }
                    let slope = (from[1] - to[1]) / (to[0] - from[0]);
                    assert!((slope - angle.to_radians().tan()).abs() < 0.001);
                }
            }
        }
    }

    #[test]
    fn malformed_tiny_empty_and_over_budget_hatches_do_no_work() {
        for (extent, clip, scale) in [
            ([f32::NAN, 20.0], [0.0, 0.0, 20.0, 20.0], 1.0),
            ([1.0, 1.0], [0.0, 0.0, 1.0, 1.0], 1.0),
            ([20.0, 20.0], [10.0, 10.0, 5.0, 5.0], 1.0),
            ([20.0, 20.0], [0.0, 0.0, 20.0, 20.0], 0.0),
            ([1_000_000.0, 20.0], [0.0, 0.0, 1_000_000.0, 20.0], 1.0),
        ] {
            assert_eq!(
                segments(
                    extent,
                    6.0,
                    clip,
                    scale,
                    theme::SECTION_FOCUS_HATCH,
                    |_| panic!("no draw")
                ),
                0
            );
        }
    }
}
