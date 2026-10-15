use crate::ControlStyle;
use gpui::{Div, IntoElement, div, prelude::*, px, rgb};

/// Fixed vertical geometry shared by compact native device frames.
pub const DEVICE_FRAME_HEADER_HEIGHT: f32 = 18.0;
pub const COMPACT_DEVICE_FRAME_HEADER_HEIGHT: f32 = 14.0;
pub const DEVICE_FRAME_BORDER_WIDTH: f32 = 1.0;

/// Compact device frame. The caller supplies title content and header actions,
/// including any drag input; the frame owns only shared styling and geometry.
pub fn device_frame(
    title: impl IntoElement,
    style: ControlStyle,
    header_actions: impl IntoElement,
    body: impl IntoElement,
) -> Div {
    device_frame_inner(title, style, header_actions, body, false)
}

/// Device frame with the shared rounder outline and compact title-bar geometry.
pub fn compact_device_frame(
    title: impl IntoElement,
    style: ControlStyle,
    header_actions: impl IntoElement,
    body: impl IntoElement,
) -> Div {
    device_frame_inner(title, style, header_actions, body, true)
}

fn device_frame_inner(
    title: impl IntoElement,
    style: ControlStyle,
    header_actions: impl IntoElement,
    body: impl IntoElement,
    compact_header: bool,
) -> Div {
    let radius = crate::DEVICE_CORNER_RADIUS;
    let header_height = if compact_header {
        COMPACT_DEVICE_FRAME_HEADER_HEIGHT
    } else {
        DEVICE_FRAME_HEADER_HEIGHT
    };
    let header_padding = if compact_header { 2.0 } else { 4.0 };
    let header_gap = if compact_header { 2.0 } else { 4.0 };
    let frame = div()
        .flex()
        .flex_col()
        .min_w_0()
        .border_1()
        .border_color(rgb(style.border))
        .bg(rgb(style.surface))
        .child(
            div()
                .flex()
                .h(px(header_height))
                .px(px(header_padding))
                .gap(px(header_gap))
                .items_center()
                .bg(rgb(style.raised))
                // GPUI overflow masks are rectangular; round the header background too.
                .rounded_tl(px(radius - DEVICE_FRAME_BORDER_WIDTH))
                .rounded_tr(px(radius - DEVICE_FRAME_BORDER_WIDTH))
                .border_b_1()
                .border_color(rgb(style.border))
                .child(div().size(px(7.0)).rounded_full().bg(rgb(style.accent)))
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(rgb(style.text))
                        .text_size(px(style.font_size))
                        .child(title),
                )
                .child(header_actions),
        )
        .child(body);
    frame.rounded(px(radius)).overflow_hidden()
}
