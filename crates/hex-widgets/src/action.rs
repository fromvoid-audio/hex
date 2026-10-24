//! Stateless action presentation shared by device panels and the workspace.
use crate::{ActionStyle, CONTROL_CORNER_RADIUS};
use gpui::{Div, SharedString, div, prelude::*, px, rgb};

/// Compact default diameter for round momentary actions. Header compositions
/// retain their explicit density; labeled pills extend this circle horizontally.
pub const ROUND_ACTION_SIZE: f32 = 14.0;

/// Fully rounded regular action. Owners supply normalized text, identity, dimensions,
/// focus and event handlers; this primitive never changes canonical state.
/// No fixed width is imposed, so compact glyphs and text actions use one control.
/// Device settings use `device_segment` to retain their readout corner style.
pub fn action(label: SharedString, style: ActionStyle) -> Div {
    action_surface(style).rounded_full().child(label)
}

/// The same action surface with caller-composed content, such as a swatch,
/// label and current-item marker. Owners retain input and command dispatch.
pub fn action_surface(style: ActionStyle) -> Div {
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
            let hover = hover.bg(rgb(style.hover_background));
            if let Some(text) = style.hover_text {
                hover.text_color(rgb(text))
            } else {
                hover
            }
        })
}

/// Momentary action presentation with a circular or pill outline. Equal caller
/// dimensions form a circle; wider dimensions form a labeled pill. The owner
/// supplies content, identity, focus, accessible meaning and its one-shot callback.
/// This surface has no checked/selected state: pressed paint is transient and
/// clears on release. Use it for commands, never for a latched boolean setting.
pub fn round_action_surface(
    id: impl Into<gpui::ElementId>,
    style: ActionStyle,
    border: u32,
) -> gpui::Stateful<Div> {
    action_surface(style)
        .id(id)
        .size(px(ROUND_ACTION_SIZE))
        .rounded_full()
        .border_1()
        .border_color(rgb(border))
        .active(move |pressed| {
            pressed
                .opacity(0.7)
                .bg(rgb(style.hover_background))
                .text_color(rgb(style.hover_text.unwrap_or(style.text)))
        })
}

#[cfg(test)]
#[path = "action_tests.rs"]
mod tests;
