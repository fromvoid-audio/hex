//! Display-only clipping acknowledgement for one continuously rendered meter.
//! A click clears the latched cap, never the live signal or held peak. Boolean
//! telemetry rearms on an observed clear-to-clipped edge; it cannot recover
//! unreported clipping between UI observations or retain history after the meter unmounts.
use gpui::{
    App, IntoElement, MouseButton, RenderOnce, SharedString, Window, div, prelude::*, px, rgb,
};

struct ClipLatch {
    latched: bool,
    previous_signal: bool,
    previous_held: bool,
}

#[derive(IntoElement)]
pub(crate) struct ClipIndicator {
    pub id: SharedString,
    pub clipping: bool,
    pub held_clipping: bool,
    pub color: u32,
}

impl RenderOnce for ClipIndicator {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(
            SharedString::from(format!("{}-latch", self.id)),
            cx,
            |_, _| ClipLatch {
                latched: false,
                previous_signal: false,
                previous_held: false,
            },
        );
        let visible = state.update(cx, |state, _| {
            if (self.clipping && !state.previous_signal)
                || (self.held_clipping && !state.previous_held)
            {
                state.latched = true;
            }
            state.previous_signal = self.clipping;
            state.previous_held = self.held_clipping;
            state.latched
        });
        let selector = self.id.to_string();
        div()
            .id(self.id)
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(px(if visible { 4.0 } else { 0.0 }))
            .when(visible, |cap| {
                cap.debug_selector(move || selector.clone())
                    .bg(rgb(self.color))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        state.update(cx, |state, cx| {
                            state.latched = false;
                            cx.notify();
                        });
                    })
            })
    }
}
