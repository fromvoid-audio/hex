//! Shared interval and floating-preview edge paint. Input, time projection,
//! source state and clip contents remain with their callers.
//!
//! A trailing boundary paints on the next interval's physical grid column,
//! outside the child layout, so adjacent intervals share one stroke without
//! changing trim or drag coordinates. Degenerate intervals retain lane-anchored
//! edges; looping callers supply the existing rounded-corner bottom inset.
use gpui::{Div, div, prelude::*, px, relative, rgb};

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

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "clip_border_ui_test.rs"]
mod tests;
