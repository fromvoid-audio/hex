//! Narrow vertical presentation of the shared numeric control. The rail and
//! triangle use normalized display position; all edits retain NumericControl's
//! bounded scrubbing, focus and typed event lifecycle. The readout is a sibling
//! below this rail, so neither text nor pointer targets need to fit its width.
use super::*;
use gpui::{PathBuilder, point};

impl NumericControl {
    pub(super) fn vertical_fader(&self, value: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let style = self.style;
        let meter = self.appearance == NumericControlAppearance::MeterFader;
        let well_width = if meter { 28.0 } else { 10.0 };
        let enabled = self.availability.painted_enabled();
        let rail_id = format!("{}-slider", self.id);
        let handle_id = format!("{}-slider-thumb", self.id);
        div()
            .flex()
            .flex_1()
            .min_h(px(16.0))
            .w(px(well_width + 18.0))
            // Reserve half the handle height at both ends of its travel.
            .py(px(4.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|control, event, window, cx| {
                    control.text_mouse = None;
                    control.text_click.clear();
                    control.begin_press(PressTarget::Fader, event, window, cx);
                    control.focus.focus(window);
                }),
            )
            .child(
                div()
                    .debug_selector(move || rail_id.clone())
                    .relative()
                    .w(px(well_width))
                    .h_full()
                    .bg(rgb(style.rail))
                    .border_1()
                    .border_color(rgb(style.border))
                    .when(meter, |rail| {
                        let amplitude = self.meter_readout.level;
                        let fraction = if amplitude.is_finite() && amplitude > 0.0 {
                            {
                                let (min, max) = self.spec.bounds();
                                ((f64::from(20.0 * amplitude.log10()) - min) / (max - min))
                                    .clamp(0.0, 1.0) as f32
                            }
                        } else {
                            0.0
                        };
                        let selector = format!("{}-input-fill", self.id);
                        rail.child(
                            div()
                                .debug_selector(move || selector.clone())
                                .absolute()
                                .bottom_0()
                                .w_full()
                                .h(relative(fraction))
                                .bg(rgb(if amplitude > 1.0 {
                                    self.meter_readout.clipping
                                } else {
                                    self.meter_readout.signal
                                })),
                        )
                    })
                    .when(self.spec.scale() == crate::NumericScale::Bipolar, |rail| {
                        rail.child(
                            div()
                                .absolute()
                                .top(relative(0.5))
                                .w_full()
                                .h(px(1.0))
                                .bg(rgb(style.muted)),
                        )
                    })
                    .child(
                        div()
                            .debug_selector(move || handle_id.clone())
                            .absolute()
                            .left(px(well_width + 2.0))
                            .bottom(relative(value))
                            .mb(px(-4.0))
                            .w(px(9.0))
                            .h(px(8.0))
                            .child(
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, window, _| {
                                        let mut triangle = PathBuilder::fill();
                                        triangle.move_to(point(bounds.left(), bounds.center().y));
                                        triangle.line_to(point(bounds.right(), bounds.top()));
                                        triangle.line_to(point(bounds.right(), bounds.bottom()));
                                        triangle.close();
                                        if let Ok(path) = triangle.build() {
                                            window.paint_path(
                                                path,
                                                rgb(if enabled {
                                                    if meter { style.accent } else { style.text }
                                                } else {
                                                    style.muted
                                                }),
                                            );
                                        }
                                    },
                                )
                                .size_full(),
                            ),
                    ),
            )
    }
}
