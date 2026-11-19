//! Optional normalized axes. Owners supply semantics; plot geometry is shared by paint and input.
use super::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResponseGraphAxes {
    pub horizontal: Vec<(f32, SharedString)>,
    pub vertical: Vec<(f32, SharedString)>,
    pub right: Vec<(f32, SharedString)>,
}

pub(super) fn plot_bounds(bounds: Bounds<Pixels>, axes: bool) -> Bounds<Pixels> {
    let (left, right, top, bottom) = if axes {
        (30.0, 34.0, 8.0, 20.0)
    } else {
        (PLOT_INSET, PLOT_INSET, PLOT_INSET, PLOT_INSET)
    };
    Bounds::new(
        point(bounds.left() + px(left), bounds.top() + px(top)),
        gpui::size(
            (bounds.size.width - px(left + right)).max(px(0.0)),
            (bounds.size.height - px(top + bottom)).max(px(0.0)),
        ),
    )
}

impl ResponseGraphAxes {
    pub(super) fn valid(&self) -> bool {
        [&self.horizontal, &self.vertical, &self.right]
            .into_iter()
            .all(|ticks| {
                ticks.len() <= 32
                    && ticks.iter().all(|(position, label)| {
                        position.is_finite() && (0.0..=1.0).contains(position) && label.len() <= 24
                    })
            })
    }
    pub(super) fn labels(
        &self,
        id: &SharedString,
        style: ControlStyle,
        bounds: Option<Bounds<Pixels>>,
        window: &Window,
    ) -> gpui::Div {
        let width = bounds
            .map(|b| f32::from(plot_bounds(b, true).size.width))
            .unwrap_or(400.0);
        let key = format!("{id}-plot");
        let mut labels = div()
            .debug_selector(move || key.clone())
            .absolute()
            .left(px(30.0))
            .right(px(34.0))
            .top(px(8.0))
            .bottom(px(20.0));
        let mut end = None;
        for (index, (x, label)) in self.horizontal.iter().enumerate() {
            if label.is_empty() {
                continue;
            }
            let run = gpui::TextRun {
                len: label.len(),
                font: window.text_style().font(),
                color: rgb(style.muted).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let measured = f32::from(
                window
                    .text_system()
                    .shape_line(label.clone(), px(8.0), &[run], None)
                    .width,
            );
            let left = (x * width - measured / 2.0).clamp(0.0, (width - measured).max(0.0));
            if end.is_some_and(|end| left < end + 5.0) {
                continue;
            }
            end = Some(left + measured);
            let key = format!("{id}-axis-x-{index}");
            labels = labels.child(
                div()
                    .absolute()
                    .left(px(left))
                    .bottom(px(-15.0))
                    .w(px(measured + 1.0))
                    .h(px(10.0))
                    .text_size(px(8.0))
                    .line_height(px(10.0))
                    .text_color(rgb(style.muted))
                    .debug_selector(move || key.clone())
                    .child(label.clone()),
            );
        }
        for (right, ticks) in [(false, &self.vertical), (true, &self.right)] {
            for (index, (y, label)) in ticks.iter().enumerate() {
                let key = format!("{id}-axis-{}-{index}", if right { "right" } else { "y" });
                let text = div()
                    .absolute()
                    .bottom(gpui::relative(*y))
                    .mb(px(-5.0))
                    .w(px(if right { 32.0 } else { 26.0 }))
                    .h(px(10.0))
                    .text_size(px(8.0))
                    .line_height(px(10.0))
                    .text_color(rgb(style.muted))
                    .debug_selector(move || key.clone())
                    .child(label.clone());
                labels = labels.child(if right {
                    text.right(px(-34.0))
                } else {
                    text.left(px(-29.0))
                });
            }
        }
        labels
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_axes_preserve_default_plot_and_reject_unbounded_ticks() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), gpui::size(px(400.0), px(160.0)));
        assert_eq!(
            plot_bounds(bounds, false),
            Bounds::new(point(px(17.0), px(27.0)), gpui::size(px(386.0), px(146.0)))
        );
        let plot = plot_bounds(bounds, true);
        assert_eq!(plot.origin, point(px(40.0), px(28.0)));
        assert_eq!(plot.size, gpui::size(px(336.0), px(132.0)));
        let mut axes = ResponseGraphAxes {
            horizontal: vec![(0.0, "tick".into()); 33],
            ..Default::default()
        };
        assert!(!axes.valid());
        axes.horizontal = vec![(f32::NAN, "tick".into())];
        assert!(!axes.valid());
        axes.horizontal = vec![(0.5, "tick".into())];
        assert!(axes.valid());
    }
}
