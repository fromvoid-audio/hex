use crate::ControlStyle;
use gpui::{IntoElement, div, prelude::*, px, rgb};

/// Compact device header and body frame. Header actions remain caller-owned.
pub fn device_frame(
    title: impl Into<gpui::SharedString>,
    style: ControlStyle,
    header_actions: impl IntoElement,
    body: impl IntoElement,
) -> impl IntoElement {
    let title = title.into();
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .border_1()
        .border_color(rgb(style.border))
        .bg(rgb(style.surface))
        .child(
            div()
                .flex()
                .h(px(18.0))
                .px(px(4.0))
                .gap(px(4.0))
                .items_center()
                .bg(rgb(style.raised))
                .border_b_1()
                .border_color(rgb(style.border))
                .child(div().size(px(7.0)).rounded_full().bg(rgb(style.accent)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(rgb(style.text))
                        .text_size(px(style.font_size))
                        .child(title),
                )
                .child(header_actions),
        )
        .child(body)
}
