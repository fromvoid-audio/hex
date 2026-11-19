//! Shared Arrangement header presentation. Track and Return owners retain input,
//! resize, mixer and device behavior; this module owns only common visual geometry.
use crate::theme;
use gpui::{Div, ElementId, PathBuilder, Stateful, canvas, div, point, prelude::*, px, rgb};

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

/// The name column fills its cell with the track color.
pub(super) fn name_plate(selector: String, color: u32) -> Div {
    div()
        .absolute()
        .inset_0()
        .debug_selector(move || selector.clone())
        .bg(rgb(color))
}

/// Compact selection inset: its left edge is taller than its right edge.
/// Groups reserve this space even when unselected to keep the fold control stable.
pub(super) fn selection_marker(selector: String, visible: bool) -> Div {
    let fill = theme::control_style().rail;
    div()
        .w(px(4.5))
        .h(px(15.0))
        .mr(px(2.0))
        .flex_shrink_0()
        .debug_selector(move || selector.clone())
        .children(visible.then(|| {
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let at = |x, y| bounds.origin + point(px(x), px(y));
                    let mut path = PathBuilder::fill();
                    path.move_to(at(0.0, 1.3));
                    path.curve_to(at(1.2, 0.5), at(0.0, 0.0));
                    path.line_to(at(3.8, 1.8));
                    path.curve_to(at(4.5, 2.8), at(4.5, 2.1));
                    path.line_to(at(4.5, 12.2));
                    path.curve_to(at(3.8, 13.2), at(4.5, 12.9));
                    path.line_to(at(1.2, 14.5));
                    path.curve_to(at(0.0, 13.7), at(0.0, 15.0));
                    path.close();
                    if let Ok(path) = path.build() {
                        window.paint_path(path, rgb(fill));
                    }
                },
            )
            .size_full()
        }))
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
