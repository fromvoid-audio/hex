//! Stateless action presentation shared by device panels and the workspace.
use crate::{ActionStyle, CONTROL_CORNER_RADIUS};
use gpui::{Div, SharedString, div, prelude::*, px, rgb};

/// Shared action surface. Owners supply normalized text, identity, dimensions,
/// focus and event handlers; this primitive never changes canonical state.
/// No fixed width is imposed, so compact glyphs and text actions use one control.
pub fn action(label: SharedString, style: ActionStyle) -> Div {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(CONTROL_CORNER_RADIUS))
        .cursor_pointer()
        .bg(rgb(style.background))
        .text_color(rgb(style.text))
        .hover(move |hover| {
            hover
                .bg(rgb(style.hover_background))
                .text_color(rgb(style.hover_text))
        })
        .child(label)
}
