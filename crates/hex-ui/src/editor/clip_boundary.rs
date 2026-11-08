//! Shared interval and floating-preview edge paint. Input, time projection,
//! source state and clip contents remain with their callers.
//!
//! A trailing boundary paints on the next interval's physical grid column,
//! outside the child layout, so adjacent intervals share one stroke without
//! changing trim or drag coordinates. Degenerate intervals retain lane-anchored
//! edges; looping callers supply the existing rounded-corner bottom inset.
use gpui::{Div, canvas, div, prelude::*, px, relative, rgb};
use std::{cell::Cell, rc::Rc};

/// Per-render geometry shared by the backdrop and its independently raised edge.
/// Resolve after layout, so a resize cannot leave a cached-width corner gap.
#[derive(Clone, Default)]
pub(super) struct Notch(Option<Rc<Cell<f32>>>);

impl Notch {
    pub fn new(enabled: bool) -> Self {
        Self(enabled.then(|| Rc::new(Cell::new(0.0))))
    }
    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }
    fn observe(&self, bounds: gpui::Bounds<gpui::Pixels>) {
        if let Some(extent) = &self.0 {
            extent.set(corner_extent(
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ));
        }
    }
    fn extent(&self) -> f32 {
        self.0.as_ref().map_or(0.0, |value| value.get())
    }
}

fn corner_extent(width: f32, height: f32) -> f32 {
    if !width.is_finite() || !height.is_finite() {
        return 0.0;
    }
    5.0_f32.min(width.max(0.0) / 2.0).min(height.max(0.0) / 2.0)
}

/// Keep the diagnostic and input bounds of a physical edge; mask only its paint.
pub(super) fn trailing_edge(notch: Notch, color: u32, hover: u32) -> Div {
    if !notch.enabled() {
        return div().bg(rgb(color)).when(hover != color, |line| {
            line.group_hover("clip-boundary", move |style| style.bg(rgb(hover)))
        });
    }
    div()
        .text_color(rgb(color))
        .when(hover != color, |line| {
            line.group_hover("clip-boundary", move |style| style.text_color(rgb(hover)))
        })
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let mask = gpui::Bounds::from_corners(
                        gpui::point(bounds.left(), bounds.top() + px(notch.extent())),
                        bounds.bottom_right(),
                    );
                    window.with_content_mask(Some(gpui::ContentMask { bounds: mask }), |window| {
                        window.paint_quad(gpui::fill(bounds, window.text_style().color));
                    });
                },
            )
            .absolute()
            .inset_0(),
        )
}

/// Keep the time interval's layout and input bounds unchanged while painting
/// its trailing stroke on the same physical column as the next interval.
#[allow(clippy::too_many_arguments)]
pub(super) fn clip_boundary_shell<T: gpui::IntoElement + gpui::Styled>(
    body: T,
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
    boundary_shell(
        body,
        left,
        width,
        stroke,
        color,
        hover,
        visible,
        native_leading,
        bottom_inset,
        Notch::default(),
        selector,
    )
}

#[allow(clippy::too_many_arguments)]
fn boundary_shell<T: gpui::IntoElement + gpui::Styled>(
    mut body: T,
    left: f32,
    width: f32,
    stroke: f32,
    color: u32,
    hover: u32,
    visible: bool,
    native_leading: bool,
    bottom_inset: f32,
    notch: Notch,
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
            trailing_edge(notch, color, hover)
                .debug_selector(selector)
                .absolute()
                .left(relative(trailing))
                .top_0()
                .bottom(px(bottom_inset))
                .w(px(stroke)),
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
#[derive(Clone)]
pub(super) struct Backdrop {
    pub body: u32,
    pub header: u32,
    pub outline: u32,
    pub hover_outline: u32,
    pub stroke: f32,
    pub bottom_radius: f32,
    pub header_height: f32,
    pub notch: Notch,
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
    let observe = backdrop.notch.clone();
    let paint_style = backdrop.clone();
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
                move |bounds, _, _| observe.observe(bounds),
                move |bounds, _, window, _| {
                    if visible {
                        paint_backdrop(bounds, &paint_style, window);
                    }
                },
            )
            .absolute()
            .inset_0(),
        );
    let contents = div().absolute().inset_0().child(paint).child(body);
    boundary_shell(
        contents,
        left,
        width,
        backdrop.stroke,
        backdrop.outline,
        backdrop.hover_outline,
        visible,
        true,
        backdrop.bottom_radius,
        backdrop.notch,
        selector,
    )
}

fn paint_backdrop(bounds: gpui::Bounds<gpui::Pixels>, style: &Backdrop, window: &mut gpui::Window) {
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
    let cut = style.notch.extent();
    let paint_square = |window: &mut gpui::Window| {
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
    };
    if cut == 0.0 {
        paint_square(window);
        return;
    }
    let below = Bounds::from_corners(
        point(bounds.left(), bounds.top() + px(cut)),
        bounds.bottom_right(),
    );
    window.with_content_mask(Some(ContentMask { bounds: below }), paint_square);
    let upper_left = Bounds::from_corners(
        bounds.origin,
        point(bounds.right() - px(cut), bounds.top() + px(cut)),
    );
    window.with_content_mask(Some(ContentMask { bounds: upper_left }), paint_square);
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        // The remaining triangle is actual clip fill, not a lane-colored cover.
        // Extend the two interior joins under their neighboring quads to avoid AA seams.
        let overlap = px(style.stroke);
        let mut triangle = gpui::PathBuilder::fill();
        triangle.move_to(point(bounds.right() - px(cut) - overlap, bounds.top()));
        triangle.line_to(point(bounds.right() - px(cut), bounds.top()));
        triangle.line_to(point(bounds.right(), bounds.top() + px(cut)));
        triangle.line_to(point(bounds.right(), bounds.top() + px(cut) + overlap));
        triangle.line_to(point(
            bounds.right() - px(cut) - overlap,
            bounds.top() + px(cut) + overlap,
        ));
        triangle.close();
        if let Ok(path) = triangle.build() {
            window.paint_path(path, rgb(style.header));
        }
        let inset = px(style.stroke / (2.0 * std::f32::consts::SQRT_2));
        let mut diagonal = gpui::PathBuilder::stroke(px(style.stroke));
        diagonal.move_to(point(
            bounds.right() - px(cut) - inset,
            bounds.top() + inset,
        ));
        diagonal.line_to(point(
            bounds.right() - inset,
            bounds.top() + px(cut) + inset,
        ));
        if let Ok(path) = diagonal.build() {
            window.paint_path(path, window.text_style().color);
        }
    });
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "clip_border_ui_test.rs"]
mod tests;
