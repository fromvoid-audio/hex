//! Shared section paint; owners retain input, selection and project custody.
use crate::theme;
use gpui::{
    Bounds, Div, PathBuilder, Pixels, Styled, Window, canvas, fill, point, prelude::*, px,
    relative, rgb, rgba, size,
};
use hex_project::{BeatTime, ElementColor, SectionId};

#[path = "section_surface/focus.rs"]
pub(crate) mod focus;

/// The gesture owner retains this display state until its edit is acknowledged.
#[derive(Clone, Copy, Default)]
pub(crate) struct Hover {
    pub held: [Option<SectionId>; 2],
    pub pointer: Option<BeatTime>,
}

impl Hover {
    pub(crate) fn contains(self, id: Option<SectionId>, start: BeatTime, end: BeatTime) -> bool {
        id.is_some_and(|id| self.held.contains(&Some(id)))
            || self
                .pointer
                .is_some_and(|beat| start <= beat && beat <= end)
    }
}

/// Main's section body and its opaque header share identity and hover colors.
pub(crate) fn surface(
    left: f32,
    width: f32,
    pixel_width: f32,
    chamfer: f32,
    color: ElementColor,
    selected: bool,
    hovered: bool,
) -> Div {
    let colors = theme::section_colors(color, selected, hovered);
    let hover = theme::section_colors(color, selected, true);
    hex_widgets::rectangle_surface(left, width, pixel_width)
        .border_0()
        .cursor(gpui::CursorStyle::Arrow)
        .text_color(rgb(colors.1))
        .hover(move |style| style.text_color(rgb(hover.1)))
        .text_xs()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let over = hovered || bounds.contains(&window.mouse_position());
                    let colors = theme::section_colors(color, selected, over);
                    paint(
                        bounds,
                        chamfer,
                        colors.0,
                        (selected || over).then_some(colors.2),
                        window,
                    );
                },
            )
            .absolute()
            .inset_0(),
        )
}

/// A quiet marker body with an explicit stronger outline. Input/custody stay
/// with the host; this layer is retained with its exact released preview.
pub(crate) fn marker_only_overlay(color: ElementColor, chamfer: f32, selected: bool) -> Div {
    gpui::div().absolute().inset_0().child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let (fill, outline) = theme::section_marker_only_colors(color, selected);
                paint(bounds, chamfer, fill, None, window);
                paint_outline(
                    bounds,
                    chamfer,
                    outline,
                    theme::SECTION_MARKER_ONLY_OUTLINE_DEVICE_PX,
                    window,
                );
            },
        )
        .absolute()
        .inset_0(),
    )
}

/// Match device-chain insertion/replace feedback through its shared secondary
/// palette token and two-pixel insertion width. Hosts project original targets.
pub(crate) fn insertion_indicator(left: f32) -> Div {
    gpui::div()
        .absolute()
        .left(relative(left))
        .top_0()
        .bottom_0()
        .w(px(2.0))
        .bg(rgb(theme::color(theme::PaletteRole::Secondary)))
}
pub(crate) fn replacement_indicator(left: f32, width: f32) -> Div {
    let color = theme::color(theme::PaletteRole::Secondary);
    gpui::div()
        .absolute()
        .left(relative(left))
        .w(relative(width))
        .top_0()
        .bottom_0()
        .border_2()
        .border_color(rgb(color))
        .bg(rgba((color << 8) | 0x22))
}

/// Header height follows the host's lane minimum while sharing production paint.
pub(crate) fn header(color: ElementColor, selected: bool, hovered: bool, minimum: f32) -> Div {
    let colors = theme::section_colors(color, selected, hovered);
    let hover = theme::section_colors(color, selected, true);
    hex_widgets::rectangle_body(0.0, minimum)
        .h(px(minimum))
        .min_h(px(minimum))
        .max_h(gpui::relative(1.0))
        .text_color(rgb(colors.1))
        .text_xs()
        .line_height(px(14.0))
        .px_1()
        .overflow_hidden()
        .hover(move |style| style.text_color(rgb(hover.1)))
}

#[path = "section_surface/geometry.rs"]
mod geometry;
pub(crate) use geometry::{CHAMFER, cut, miniature_right};

pub(crate) fn paint(
    bounds: Bounds<Pixels>,
    requested: f32,
    color: u32,
    outline: Option<u32>,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return;
    }
    let cut = cut(width, height, requested);
    window.paint_quad(fill(
        Bounds::new(bounds.origin, size(bounds.size.width, px(height - cut))),
        rgb(color),
    ));
    if cut > 0.0 {
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.left(), bounds.top() + px(height - cut)),
                size(px(miniature_right(width, height, cut, height)), px(cut)),
            ),
            rgb(color),
        ));
        // Overlap only the internal joins by one physical pixel so their
        // antialiasing cannot leave hairline gaps between the filled pieces.
        let overlap = cut.min(1.0 / window.scale_factor());
        let mut corner = PathBuilder::fill();
        for (index, [x, y]) in [
            [width - cut - overlap, height - cut - overlap],
            [width, height - cut - overlap],
            [width, height - cut],
            [width - cut, height],
            [width - cut - overlap, height],
        ]
        .into_iter()
        .enumerate()
        {
            let p = point(bounds.left() + px(x), bounds.top() + px(y));
            if index == 0 {
                corner.move_to(p);
            } else {
                corner.line_to(p);
            }
        }
        corner.close();
        if let Ok(path) = corner.build() {
            window.paint_path(path, rgb(color));
        }
    }
    if let Some(color) = outline {
        paint_outline(bounds, requested, color, 1.0, window);
    }
}

fn paint_outline(
    bounds: Bounds<Pixels>,
    requested: f32,
    color: u32,
    device_width: f32,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let scale = window.scale_factor();
    if ![width, height, scale, device_width]
        .into_iter()
        .all(f32::is_finite)
        || scale <= 0.0
        || device_width <= 0.0
    {
        return;
    }
    if width >= 2.0 * device_width / scale && height >= 2.0 * device_width / scale {
        let cut = cut(width, height, requested);
        let inset = device_width / (2.0 * scale);
        let mut path = PathBuilder::stroke(px(device_width / scale));
        for (i, [x, y]) in [
            [inset, inset],
            [width - inset, inset],
            [width - inset, height - cut - inset],
            [width - cut - inset, height - inset],
            [inset, height - inset],
        ]
        .into_iter()
        .enumerate()
        {
            let p = point(bounds.left() + px(x), bounds.top() + px(y));
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path.close();
        if let Ok(path) = path.build() {
            window.paint_path(path, rgb(color));
        }
    }
}
