//! Small read-only curve plot for compact device panels.
use crate::ControlStyle;
use gpui::{Div, PathBuilder, canvas, div, point, prelude::*, px, rgb};

/// Draws a bounded set of normalized points as a lightweight curve preview.
///
/// The caller owns the musical meaning of each point. Invalid, unordered or
/// non-finite coordinates are refused instead of producing malformed geometry.
pub fn curve_plot<const N: usize>(points: [[f32; 2]; N], style: ControlStyle) -> Option<Div> {
    if !(2..=256).contains(&N)
        || points.iter().any(|[x, y]| {
            !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y)
        })
        || points.windows(2).any(|pair| pair[0][0] > pair[1][0])
    {
        return None;
    }
    Some(
        div()
            .relative()
            .size_full()
            .border_1()
            .border_color(rgb(style.border))
            .bg(rgb(style.background))
            .child(canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    if width <= 0.0 || height <= 0.0 {
                        return;
                    }
                    let mut grid = PathBuilder::stroke(px(0.5));
                    for division in 1..4 {
                        let y = bounds.origin.y + px(height * division as f32 / 4.0);
                        grid.move_to(point(bounds.origin.x, y));
                        grid.line_to(point(bounds.origin.x + bounds.size.width, y));
                    }
                    if let Ok(grid) = grid.build() {
                        window.paint_path(grid, rgb(style.rail));
                    }
                    let mut curve = PathBuilder::stroke(px(1.0));
                    for (index, [x, y]) in points.iter().enumerate() {
                        let point = point(
                            bounds.origin.x + px(*x * width),
                            bounds.origin.y + px((1.0 - *y) * height),
                        );
                        if index == 0 {
                            curve.move_to(point);
                        } else {
                            curve.line_to(point);
                        }
                    }
                    if let Ok(curve) = curve.build() {
                        window.paint_path(curve, rgb(style.accent));
                    }
                },
            )),
    )
}

#[cfg(test)]
mod tests {
    use super::curve_plot;
    use crate::ControlStyle;

    const STYLE: ControlStyle = ControlStyle {
        background: 0x202020,
        surface: 0x303030,
        raised: 0x383838,
        rail: 0x282828,
        border: 0x505050,
        text: 0xe0e0e0,
        muted: 0xaaaaaa,
        accent: 0x55ddee,
        hover: 0x404040,
        font_size: 8.0,
    };

    #[test]
    fn curve_plot_accepts_normalized_ordered_points_and_refuses_invalid_data() {
        assert!(curve_plot([[0.0, 0.0]], STYLE).is_none());
        assert!(curve_plot([[0.5, 0.5]; 257], STYLE).is_none());
        assert!(
            curve_plot(
                [[0.0, 0.0], [0.2, 1.0], [0.5, 0.7], [0.8, 0.7], [1.0, 0.0]],
                STYLE
            )
            .is_some()
        );
        assert!(
            curve_plot(
                [
                    [0.0, 0.0],
                    [0.2, f32::NAN],
                    [0.5, 0.7],
                    [0.8, 0.7],
                    [1.0, 0.0]
                ],
                STYLE
            )
            .is_none()
        );
        assert!(
            curve_plot(
                [[0.0, 0.0], [0.6, 1.0], [0.5, 0.7], [0.8, 0.7], [1.0, 0.0]],
                STYLE
            )
            .is_none()
        );
    }
}
