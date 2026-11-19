//! Readout-free filled rail. Reuse Fader capture so a click focuses the slider
//! without entering an invisible text draft; keyboard and accessibility edits
//! remain with the shared numeric owner.
use super::*;

impl NumericControl {
    pub(super) fn compact_vertical(&self, value: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.availability.painted_enabled();
        let style = self.style;
        let origin = if self.spec.scale() == crate::NumericScale::Bipolar {
            0.5
        } else {
            0.0
        };
        let rail_id = format!("{}-slider", self.id);
        let fill_id = format!("{}-bar-fill", self.id);
        div()
            .debug_selector(move || rail_id.clone())
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(style.rail))
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
                    .debug_selector(move || fill_id.clone())
                    .absolute()
                    .bottom(relative(value.min(origin)))
                    .w_full()
                    .h(relative((value - origin).abs()))
                    .bg(rgb(if enabled {
                        style.numeric_fill
                    } else {
                        style.muted
                    }))
                    .when(!enabled, |fill| fill.opacity(0.35)),
            )
            .when(origin == 0.5 && self.slider_center_mark, |rail| {
                rail.child(
                    div()
                        .absolute()
                        .bottom(relative(0.5))
                        .w_full()
                        .h(px(1.0))
                        .bg(rgb(style.muted))
                        .opacity(0.6),
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .border_1()
                    .border_color(rgb(style.border)),
            )
    }
}

#[cfg(test)]
mod tests;
