//! Small read-only curve plot for compact device panels.
use crate::ControlStyle;
use gpui::{Div, PathBuilder, canvas, div, point, prelude::*, px, rgb};

/// Grid presentation for the shared normalized curve plot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurvePlotGrid {
    /// Horizontal quarter divisions for waveform/envelope previews.
    Horizontal,
    /// Quarter divisions on both axes with emphasized center lines.
    Centered,
}

/// Draws a bounded normalized curve with a caller-selected grid.
/// The caller owns the musical meaning and axis ranges. Invalid, unordered or
/// non-finite coordinates are refused instead of producing malformed geometry.
pub fn curve_plot<const N: usize>(
    points: [[f32; 2]; N],
    style: ControlStyle,
    grid: CurvePlotGrid,
) -> Option<Div> {
    let centered_axes = grid == CurvePlotGrid::Centered;
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
    use super::{CurvePlotGrid, curve_plot};
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
        accent_text: 0x202020,
        numeric_fill: 0x31515a,
        hover_text: 0xe0e0e0,
        focus_marker: 0xaaaaaa,
        dial_rail: 0x000000,
        hover: 0x404040,
        selection: 0x3350a0,
        invalid: 0xff554c,
        font_size: 8.0,
        text_case: crate::TextCase::Preserve,
    };

    struct CurveFixture(
        std::rc::Rc<std::cell::RefCell<Vec<gpui::Bounds<gpui::Pixels>>>>,
        CurvePlotGrid,
    );
    impl gpui::Render for CurveFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::prelude::*;
            let bounds = self.0.clone();
            gpui::div().w(gpui::px(64.0)).h(gpui::px(14.0)).child(
                curve_plot([[0.0, 0.5], [0.5, 1.0], [1.0, 0.5]], STYLE, self.1)
                    .unwrap()
                    .on_children_prepainted(move |children, _, _| *bounds.borrow_mut() = children),
            )
        }
    }

    #[gpui::test]
    fn compact_curve_canvas_fills_its_display(cx: &mut gpui::TestAppContext) {
        for grid in [CurvePlotGrid::Horizontal, CurvePlotGrid::Centered] {
            let bounds = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let window = cx.add_window(|_, _| CurveFixture(bounds.clone(), grid));
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
    }

    #[test]
    fn curve_plot_accepts_normalized_ordered_points_and_refuses_invalid_data() {
        assert!(
            curve_plot(
                [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]],
                STYLE,
                CurvePlotGrid::Horizontal
            )
            .is_some()
        );
        assert!(
            curve_plot(
                [[0.0, 0.0], [1.0, f32::INFINITY]],
                STYLE,
                CurvePlotGrid::Horizontal
            )
            .is_none()
        );
        assert!(curve_plot([[0.0, 0.0]], STYLE, CurvePlotGrid::Horizontal).is_none());
        assert!(curve_plot([[0.5, 0.5]; 257], STYLE, CurvePlotGrid::Horizontal).is_none());
        assert!(
            curve_plot(
                [[0.0, 0.0], [0.2, 1.0], [0.5, 0.7], [0.8, 0.7], [1.0, 0.0]],
                STYLE,
                CurvePlotGrid::Horizontal
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
                STYLE,
                CurvePlotGrid::Horizontal
            )
            .is_none()
        );
        assert!(
            curve_plot(
                [[0.0, 0.0], [0.6, 1.0], [0.5, 0.7], [0.8, 0.7], [1.0, 0.0]],
                STYLE,
                CurvePlotGrid::Horizontal
            )
            .is_none()
        );
    }
}
