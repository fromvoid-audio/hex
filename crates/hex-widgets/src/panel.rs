//! Compact workspace surfaces with structural paint and pointer containment.
use gpui::{Div, IntoElement, div, prelude::*, px, rgb};

pub const PANEL_FRAME_RADIUS: f32 = 4.0;
/// Complete surround per edge, including the one-pixel outline.
pub const PANEL_FRAME_INSET: f32 = 4.0;
pub const PANEL_FRAME_BORDER_WIDTH: f32 = 1.0;

#[derive(Clone, Copy, Debug)]
pub struct PanelStyle {
    pub surface: u32,
    pub border: u32,
}

/// A rounded shell around one rectangular content viewport. Callers own its size.
/// GPUI overflow masks are rectangular: the structural inset places that viewport
/// wholly inside the rounded region, containing oversized descendant paint/hits.
pub fn panel_frame(style: PanelStyle, content: impl IntoElement) -> Div {
    div()
        .min_w_0()
        .min_h_0()
        .flex()
        .rounded(px(PANEL_FRAME_RADIUS))
        .border(px(PANEL_FRAME_BORDER_WIDTH))
        .border_color(rgb(style.border))
        .p(px(PANEL_FRAME_INSET - PANEL_FRAME_BORDER_WIDTH))
        .bg(rgb(style.surface))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .overflow_hidden()
                .child(content),
        )
}

#[cfg(test)]
mod tests;
