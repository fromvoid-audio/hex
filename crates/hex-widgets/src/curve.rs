//! Small read-only curve plot for compact device panels.
use crate::ControlStyle;
use gpui::{Div, PathBuilder, canvas, div, point, prelude::*, px, rgb};

/// Draws a bounded set of normalized points as a lightweight curve preview.
///
/// The caller owns the musical meaning of each point. Invalid, unordered or
/// non-finite coordinates are refused instead of producing malformed geometry.
pub fn curve_plot<const N: usize>(points: [[f32; 2]; N], style: ControlStyle) -> Option<Div> {
    plot(points, style, false)
}

/// Read-only normalized transfer curve with centered input/output axes.
/// The caller supplies the data and owns the axis ranges and musical meaning.
pub fn transfer_plot<const N: usize>(points: [[f32; 2]; N], style: ControlStyle) -> Option<Div> {
    plot(points, style, true)
}

fn plot<const N: usize>(
    points: [[f32; 2]; N],
    style: ControlStyle,
    centered_axes: bool,
) -> Option<Div> {
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
            .child(
                canvas(
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
                            if centered_axes {
                                let x = bounds.origin.x + px(width * division as f32 / 4.0);
                                grid.move_to(point(x, bounds.origin.y));
                                grid.line_to(point(x, bounds.origin.y + bounds.size.height));
                            }
                        }
                        if let Ok(grid) = grid.build() {
                            window.paint_path(grid, rgb(style.rail));
                        }
                        if centered_axes {
                            let mut axes = PathBuilder::stroke(px(0.5));
                            let x = bounds.origin.x + px(width * 0.5);
                            let y = bounds.origin.y + px(height * 0.5);
                            axes.move_to(point(bounds.origin.x, y));
                            axes.line_to(point(bounds.origin.x + bounds.size.width, y));
                            axes.move_to(point(x, bounds.origin.y));
                            axes.line_to(point(x, bounds.origin.y + bounds.size.height));
                            if let Ok(axes) = axes.build() {
                                window.paint_path(axes, rgb(style.border));
                            }
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
                )
                .size_full(),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::{curve_plot, transfer_plot};
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
        focus_marker: 0xaaaaaa,
        dial_rail: 0x000000,
        hover: 0x404040,
        font_size: 8.0,
    };

    struct CurveFixture(std::rc::Rc<std::cell::RefCell<Vec<gpui::Bounds<gpui::Pixels>>>>);
    impl gpui::Render for CurveFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::prelude::*;
            let bounds = self.0.clone();
            gpui::div().w(gpui::px(64.0)).h(gpui::px(14.0)).child(
                curve_plot([[0.0, 0.5], [0.5, 1.0], [1.0, 0.5]], STYLE)
                    .unwrap()
                    .on_children_prepainted(move |children, _, _| *bounds.borrow_mut() = children),
            )
        }
    }

    #[gpui::test]
    fn compact_curve_canvas_fills_its_display(cx: &mut gpui::TestAppContext) {
        let bounds = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let window = cx.add_window(|_, _| CurveFixture(bounds.clone()));
        let visual = gpui::VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        let bounds = bounds.borrow();
        assert_eq!(bounds.len(), 1);
        assert!(bounds[0].size.width >= gpui::px(60.0));
        assert!(
            bounds[0].size.height >= gpui::px(10.0),
            "curve canvas must have height to paint its waveform"
        );
    }

    #[test]
    fn curve_plot_accepts_normalized_ordered_points_and_refuses_invalid_data() {
        assert!(transfer_plot([[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]], STYLE).is_some());
        assert!(transfer_plot([[0.0, 0.0], [1.0, f32::INFINITY]], STYLE).is_none());
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
