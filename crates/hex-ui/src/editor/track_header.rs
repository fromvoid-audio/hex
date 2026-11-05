//! Shared Arrangement header presentation. Track and Return owners retain input,
//! resize, mixer and device behavior; this module owns only common visual geometry.
use crate::theme;
use gpui::{Div, ElementId, Stateful, div, prelude::*, px, rgb};

pub(super) const NAME_HEIGHT: f32 = 18.0;

pub(super) fn surface(
    id: impl Into<ElementId>,
    width: f32,
    height: f32,
    selected: bool,
    fill: Option<u32>,
) -> Stateful<Div> {
    let background = fill.unwrap_or_else(|| {
        theme::color(if selected {
            theme::PaletteRole::Raised
        } else {
            theme::PaletteRole::Surface
        })
    });
    let hover = fill.unwrap_or_else(|| theme::color(theme::PaletteRole::Hover));
    div()
        .id(id.into())
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .relative()
        .pb(px(3.0))
        .justify_center()
        .px_1()
        .border_l_2()
        .border_b_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(background))
        .hover(move |style| style.bg(rgb(hover)))
}

pub(super) fn name() -> Div {
    div()
        .w_full()
        .flex_shrink_0()
        .min_w_0()
        .h(px(NAME_HEIGHT))
        .cursor_pointer()
        .truncate()
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Text)))
}

pub(super) fn selection_outline(selector: String) -> Div {
    div()
        .absolute()
        // Account for the surface's existing left/bottom layout borders.
        .left(px(-1.0))
        .right(px(1.0))
        .top(px(1.0))
        .bottom_0()
        .border_1()
        .debug_selector(move || selector.clone())
        .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
}
