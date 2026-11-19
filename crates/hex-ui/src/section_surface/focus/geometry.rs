//! Bounded hatch clipping for a chamfered section surface.
use super::super::geometry::cut;

pub(super) const MAX_HATCH_LINES: usize = 4096;

#[derive(Clone, Copy)]
pub(super) struct HatchGeometry {
    pub(super) angle_degrees: f32,
    pub(super) spacing: f32,
    pub(super) outline_device_px: f32,
}

// Clip each parallel line against the visible rectangle and the chamfer's
// half-plane. The callback builds one path; no per-frame peak/project traversal
// or temporary segment allocation is needed.
pub(super) fn segments(
    extent: [f32; 2],
    requested_cut: f32,
    visible: [f32; 4],
    scale: f32,
    style: HatchGeometry,
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

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn hatches_stay_in_visible_chamfered_bounds_at_multiple_angles_and_scales() {
        for angle in [30.0, 45.0, 60.0] {
            for scale in [1.0, 1.5, 2.0] {
                let style = HatchGeometry {
                    angle_degrees: angle,
                    spacing: 8.0,
                    outline_device_px: 2.0,
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
                        assert!((13.0..=160.0).contains(&x) && (5.0..=60.0).contains(&y));
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
                    HatchGeometry {
                        angle_degrees: 45.0,
                        spacing: 8.0,
                        outline_device_px: 2.0,
                    },
                    |_| panic!("no draw")
                ),
                0
            );
        }
    }
}
