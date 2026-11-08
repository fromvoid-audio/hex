//! Shared interval and floating-preview edge paint. Input, time projection,
//! source state and clip contents remain with their callers.
//!
//! A trailing boundary paints on the next interval's physical grid column,
//! outside the child layout, so adjacent intervals share one stroke without
//! changing trim or drag coordinates. Degenerate intervals retain lane-anchored
//! edges; looping callers supply the existing rounded-corner bottom inset.
use gpui::{Div, canvas, div, prelude::*, px, relative, rgb};

/// Keep the time interval's layout and input bounds unchanged while painting
/// its trailing stroke on the same physical column as the next interval.
#[allow(clippy::too_many_arguments)]
pub(super) fn clip_boundary_shell<T: gpui::IntoElement + gpui::Styled>(
    mut body: T,
    left: f32,
    width: f32,
    stroke: f32,
    color: u32,
    hover: u32,
    visible: bool,
    native_leading: bool,
    bottom_inset: f32,
    selector: impl FnOnce() -> String,
) -> Div {
    // A sub-pixel interval may round to an empty child. Keep its paint edges
    // anchored in the lane, without giving the clip a minimum hit width.
    let (outer_left, outer_width, leading, trailing) = if native_leading {
        (left, width, 0.0, 1.0)
    } else {
        body = body.left(relative(left)).w(relative(width));
        (0.0, 1.0, left, left + width)
    };
    let hoverable = native_leading && hover != color;
    let mut outer = div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(relative(outer_left))
        .w(relative(outer_width))
        .when(hoverable, |outer| outer.group("clip-boundary"))
        .child(body);
    if visible && !native_leading {
        outer = outer.child(
            div()
                .absolute()
                .left(relative(leading))
                .top_0()
                .bottom(px(bottom_inset))
                .w(px(stroke))
                .bg(rgb(color))
                .when(hoverable, |line| {
                    line.group_hover("clip-boundary", move |style| style.bg(rgb(hover)))
                }),
        );
    }
    if visible {
        outer = outer.child(
            div()
                .debug_selector(selector)
                .absolute()
                .left(relative(trailing))
                .top_0()
                .bottom(px(bottom_inset))
                .w(px(stroke))
                .bg(rgb(color))
                .when(hoverable, |line| {
                    line.group_hover("clip-boundary", move |style| style.bg(rgb(hover)))
                }),
        );
    }
    outer
}

/// Floating previews use the same physical-pixel boundary without changing
/// the rectangle used for their contents or pointer geometry.
#[allow(clippy::too_many_arguments)]
pub(super) fn pixel_boundary_shell(
    body: impl gpui::IntoElement,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    stroke: f32,
    color: u32,
    native_leading: bool,
) -> Div {
    let mut outer = div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(width))
        .h(px(height))
        .child(body);
    if !native_leading {
        outer = outer.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(stroke))
                .bg(rgb(color)),
        );
    }
    outer.child(
        div()
            .absolute()
            .left(relative(1.0))
            .top_0()
            .bottom_0()
            .w(px(stroke))
            .bg(rgb(color)),
    )
}

/// Paint supplied by the frame, while the clip body retains layout/input and
/// the name component retains text, aliases, its divider and rename state.
#[derive(Clone, Copy)]
pub(super) struct Backdrop {
    pub body: u32,
    pub header: u32,
    pub outline: u32,
    pub hover_outline: u32,
    pub stroke: f32,
    pub bottom_radius: f32,
    pub header_height: f32,
}

/// Wide canonical clips use one background owner outside their content mask.
/// Their transparent layout borders still define the same content/hit bounds.
/// Narrow intervals keep the legacy lane-anchored boundary path.
pub(super) fn backdrop_shell(
    body: impl gpui::IntoElement,
    left: f32,
    width: f32,
    backdrop: Backdrop,
    visible: bool,
    selector: impl FnOnce() -> String,
) -> Div {
    // The paint-only layer inherits currentColor so GPUI owns group hover,
    // occlusion and drag suppression. Clip contents remain a separate sibling.
    let paint = div()
        .absolute()
        .inset_0()
        .text_color(rgb(backdrop.outline))
        .when(backdrop.hover_outline != backdrop.outline, |paint| {
            paint.group_hover("clip-boundary", move |style| {
                style.text_color(rgb(backdrop.hover_outline))
            })
        })
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    if visible {
                        paint_backdrop(bounds, backdrop, window);
                    }
                },
            )
            .absolute()
            .inset_0(),
        );
    let contents = div().absolute().inset_0().child(paint).child(body);
    clip_boundary_shell(
        contents,
        left,
        width,
        backdrop.stroke,
        backdrop.outline,
        backdrop.hover_outline,
        visible,
        true,
        backdrop.bottom_radius,
        selector,
    )
}

fn paint_backdrop(bounds: gpui::Bounds<gpui::Pixels>, style: Backdrop, window: &mut gpui::Window) {
    use gpui::{Bounds, ContentMask, Corners, Edges, fill, point, quad};
    if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
        return;
    }
    let corners = Corners {
        top_left: px(0.0),
        top_right: px(0.0),
        bottom_left: px(style.bottom_radius),
        bottom_right: px(style.bottom_radius),
    }
    .clamp_radii_for_quad_size(bounds.size);
    window.paint_quad(fill(bounds, rgb(style.body)).corner_radii(corners));
    // clip_name's -1 offsets compensate the containing layout border. Preserve
    // its old quad and ancestor mask exactly, including physical leading edges.
    let header = Bounds::from_corners(
        point(bounds.left() + px(style.stroke - 1.0), bounds.top()),
        point(
            bounds.right() + px(1.0),
            bounds.top() + px(style.header_height),
        ),
    );
    let inner = Bounds::from_corners(
        point(bounds.left() + px(style.stroke), bounds.top() + px(1.0)),
        point(bounds.right(), bounds.bottom() - px(1.0)),
    );
    window.with_content_mask(Some(ContentMask { bounds: inner }), |window| {
        window.paint_quad(fill(header, rgb(style.header)));
    });
    let color = window.text_style().color;
    window.paint_quad(quad(
        bounds,
        corners,
        gpui::transparent_black(),
        Edges {
            top: px(1.0),
            bottom: px(1.0),
            left: px(style.stroke),
            right: px(0.0),
        },
        color,
        Default::default(),
    ));
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "clip_border_ui_test.rs"]
mod tests;
