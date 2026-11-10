//! Bounded, event-free focus decoration inside the shared section polygon.
use super::*;
use gpui::{div, rgba};

#[path = "focus/geometry.rs"]
mod geometry;
use geometry::{HatchGeometry, segments};

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
                HatchGeometry {
                    angle_degrees: style.angle_degrees,
                    spacing: style.spacing,
                    outline_device_px: style.outline_device_px,
                },
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

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;

    #[test]
    fn theme_hatch_style_produces_bounded_visible_segments() {
        let style = theme::SECTION_FOCUS_HATCH;
        for angle in [30.0, 45.0, 60.0] {
            for scale in [1.0, 1.5, 2.0] {
                let mut lines = Vec::new();
                let count = segments(
                    [160.0, 60.0],
                    6.0,
                    [13.0, 5.0, 160.0, 60.0],
                    scale,
                    HatchGeometry {
                        angle_degrees: angle,
                        spacing: style.spacing,
                        outline_device_px: style.outline_device_px,
                    },
                    |line| lines.push(line),
                );
                assert!(count > 0 && count < geometry::MAX_HATCH_LINES);
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
}
