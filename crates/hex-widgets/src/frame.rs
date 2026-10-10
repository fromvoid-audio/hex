use crate::ControlStyle;
use gpui::{Div, IntoElement, div, prelude::*, px, rgb};

/// Fixed vertical geometry shared by compact native device frames.
pub const DEVICE_FRAME_HEADER_HEIGHT: f32 = 18.0;
pub const COMPACT_DEVICE_FRAME_HEADER_HEIGHT: f32 = 14.0;
pub const DEVICE_FRAME_BORDER_WIDTH: f32 = 1.0;

/// Compact device header and body frame. Header actions remain caller-owned.
pub fn device_frame(
    title: impl Into<gpui::SharedString>,
    style: ControlStyle,
    header_actions: impl IntoElement,
    body: impl IntoElement,
) -> Div {
    device_frame_inner(title, style, header_actions, body, None, false)
}

/// Device frame with a caller-selected rounded outer edge.
pub fn rounded_device_frame(
    title: impl Into<gpui::SharedString>,
    style: ControlStyle,
    header_actions: impl IntoElement,
    body: impl IntoElement,
    radius: f32,
) -> Div {
    device_frame_inner(title, style, header_actions, body, Some(radius), false)
}

/// Rounded device frame with compact, reusable title-bar geometry.
pub fn compact_rounded_device_frame(
    title: impl Into<gpui::SharedString>,
    style: ControlStyle,
    header_actions: impl IntoElement,
    body: impl IntoElement,
    radius: f32,
) -> Div {
    device_frame_inner(title, style, header_actions, body, Some(radius), true)
}

fn device_frame_inner(
    title: impl Into<gpui::SharedString>,
    style: ControlStyle,
    header_actions: impl IntoElement,
    body: impl IntoElement,
    radius: Option<f32>,
    compact_header: bool,
) -> Div {
    let title = title.into();
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
                .when_some(radius, |header, radius| {
                    let inner_radius = px((radius - DEVICE_FRAME_BORDER_WIDTH).max(0.0));
                    header.rounded_tl(inner_radius).rounded_tr(inner_radius)
                })
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
        .child(body);
    if let Some(radius) = radius {
        frame.rounded(px(radius)).overflow_hidden()
    } else {
        frame
    }
}
