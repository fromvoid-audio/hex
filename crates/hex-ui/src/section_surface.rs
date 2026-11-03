//! Shared section paint; owners retain input, selection and project custody.
use crate::theme;
use gpui::{Div, Styled, prelude::*, px, relative, rgb, rgba};
use hex_project::ElementColor;

/// Main's section body and its opaque header share identity and hover colors.
pub(crate) fn surface(
    left: f32,
    width: f32,
    pixel_width: f32,
    stroke: f32,
    color: ElementColor,
    selected: bool,
    hovered: bool,
) -> Div {
    let colors = theme::section_colors(color, selected, hovered);
    let hover = theme::section_colors(color, selected, true);
    let has_border = hex_widgets::rectangle_border_width(pixel_width) > 0.0;
    hex_widgets::rectangle_surface(left, width, pixel_width)
        .border_l(px(if has_border { stroke } else { 0.0 }))
        .border_r(px(0.0))
        .cursor(gpui::CursorStyle::Arrow)
        .border_color(rgb(colors.2))
        .bg(rgba((colors.0 << 8) | 0x38))
        .text_color(rgb(colors.1))
        .hover(move |style| {
            style
                .bg(rgba((hover.0 << 8) | 0x38))
                .text_color(rgb(hover.1))
                .border_color(rgb(hover.2))
        })
        .text_xs()
}

/// Header height follows the host's lane minimum while sharing production paint.
pub(crate) fn header(color: ElementColor, selected: bool, hovered: bool, minimum: f32) -> Div {
    let colors = theme::section_colors(color, selected, hovered);
    let hover = theme::section_colors(color, selected, true);
    hex_widgets::rectangle_body(-1.0, minimum)
        .h(relative(hex_widgets::RANGE_HANDLE_FRACTION))
        .min_h(px(minimum))
        .max_h(relative(1.0))
        .bg(rgb(colors.0))
        .text_color(rgb(colors.1))
        .text_xs()
        .line_height(px(14.0))
        .px_1()
        .overflow_hidden()
        .hover(move |style| style.bg(rgb(hover.0)).text_color(rgb(hover.1)))
}
