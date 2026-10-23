//! Stateless device chrome. Owners supply meaning, state, input and colors.
use crate::{ActionStyle, ControlStyle};
use gpui::{Div, SharedString, div, prelude::*, px, rgb};

pub const DEVICE_HEADER_ACTION_SIZE: f32 = 12.0;

/// Compact action/segment; header consumers retain this shared twelve-pixel size.
/// Non-header compositions may supply their own width/height without style forks.
pub fn device_action(label: SharedString, style: ActionStyle) -> Div {
    crate::action(label, style)
        .size(px(DEVICE_HEADER_ACTION_SIZE))
        .text_size(px(8.0))
}

/// Compact identity cue with injected semantic color and selected/focus border.
/// It owns appearance only; focus, enable, navigation and editing stay outside.
pub fn device_badge(label: SharedString, style: ControlStyle) -> Div {
    div()
        .size(px(12.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(8.0))
        .rounded(px(1.0))
        .border_1()
        .border_color(rgb(style.border))
        .bg(rgb(style.surface))
        .text_color(rgb(style.text))
        .focus(move |focus| focus.border_2().border_color(rgb(style.focus_marker)))
        .child(label)
}

/// Shared section caption; owners may override size for dense table headings.
pub fn device_caption(label: SharedString, style: ControlStyle) -> Div {
    div()
        .w_full()
        .h(px(10.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(8.0))
        .text_color(rgb(style.muted))
        .child(label)
}

pub fn device_divider(style: ControlStyle) -> Div {
    div()
        .w(px(1.0))
        .h_full()
        .flex_shrink_0()
        .bg(rgb(style.border))
}

/// Directly selectable compact segment, with the shared readout corner radius.
pub fn device_segment(label: SharedString, style: ActionStyle) -> Div {
    device_action(label, style).rounded(px(crate::CONTROL_CORNER_RADIUS))
}

/// Focus boundary for owner-controlled navigation artwork.
pub fn device_focus_target(style: ControlStyle) -> Div {
    div()
        .border_1()
        .border_color(rgb(style.surface))
        .focus(move |focus| focus.border_color(rgb(style.focus_marker)))
}

pub fn device_identity_marker(color: u32) -> Div {
    div().size(px(5.0)).bg(rgb(color))
}

/// Intrinsic-width caption for labels alongside controls in compact rows.
pub fn device_inline_caption(label: SharedString, style: ControlStyle) -> Div {
    device_caption(label, style)
        .w_auto()
        .h_auto()
        .justify_start()
}
