//! Shared interval and floating-preview edge paint. Input, time projection,
//! source state and clip contents remain with their callers.
//!
//! A trailing boundary paints on the next interval's physical grid column,
//! outside the child layout, so adjacent intervals share one stroke without
//! changing trim or drag coordinates. Degenerate intervals retain lane-anchored
//! edges. Section outlines can supply an inset to match their chamfer.
use gpui::{Div, canvas, div, prelude::*, px, relative, rgb};
use std::{cell::Cell, rc::Rc};

// A test-only transparent element lets the scene helpers identify the actual
// canvas edge by its resolved bounds, including its paint order and mask.
#[cfg(any(test, feature = "ui-test-support"))]
struct EdgeWitness {
    child: gpui::AnyElement,
    anchor: Rc<Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
}

#[cfg(any(test, feature = "ui-test-support"))]
impl gpui::IntoElement for EdgeWitness {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[cfg(any(test, feature = "ui-test-support"))]
impl gpui::Element for EdgeWitness {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        let offset = self
            .anchor
            .get()
            .map_or(px(0.0), |anchor| anchor.right() - bounds.left());
        window.with_element_offset(gpui::point(offset, px(0.0)), |window| {
            self.child.prepaint(window, cx);
        });
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        self.child.paint(window, cx);
    }
}

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
    pub(super) fn extent(&self) -> f32 {
        self.0.as_ref().map_or(0.0, |value| value.get())
    }
}

pub(super) use hex_ui_model::clip_boundary_geometry::corner_extent;

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
    let mut outer = boundary_frame(outer_left, outer_width, hoverable).child(body);
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

fn boundary_frame(left: f32, width: f32, hoverable: bool) -> Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(relative(left))
        .w(relative(width))
        .when(hoverable, |outer| outer.group("clip-boundary"))
}

/// A passive pixel-positioned clip. Keep background, body and border columns
/// in the same containing coordinate space: nesting a percentage edge beneath
/// a fractional positioned parent can round its endpoint a second time.
pub(super) fn pixel_backdrop_shell<T: gpui::IntoElement + gpui::Styled>(
    body: T,
    bounds: gpui::Bounds<gpui::Pixels>,
    style: Backdrop,
    native_leading: bool,
    selector: impl FnOnce() -> String,
) -> Div {
    let notch = if native_leading {
        style.notch.clone()
    } else {
        Notch::default()
    };
    let mut body = body
        .absolute()
        .left(bounds.left())
        .top(bounds.top())
        .w(bounds.size.width)
        .h(bounds.size.height);
    let mut outer = div().absolute().inset_0();
    #[cfg(any(test, feature = "ui-test-support"))]
    let anchor = Rc::new(Cell::new(None));
    if native_leading {
        let observe = notch.clone();
        let paint_style = style.clone();
        #[cfg(any(test, feature = "ui-test-support"))]
        let observed_anchor = anchor.clone();
        outer = outer.child(
            canvas(
                move |bounds, _, _| {
                    observe.observe(bounds);
                    #[cfg(any(test, feature = "ui-test-support"))]
                    observed_anchor.set(Some(bounds));
                },
                move |bounds, _, window, _| {
                    let color = rgb(paint_style.outline).into();
                    paint_backdrop(bounds, &paint_style, color, window);
                    paint_resolved_trailing(bounds, &paint_style, color, window);
                },
            )
            .absolute()
            .left(bounds.left())
            .top(bounds.top())
            .w(bounds.size.width)
            .h(bounds.size.height),
        );
        body = body
            .bg(gpui::transparent_black())
            .border_color(gpui::transparent_black());
    }
    outer = outer.child(body);
    if native_leading {
        #[cfg(any(test, feature = "ui-test-support"))]
        {
            let witness = div()
                .debug_selector(selector)
                .absolute()
                .left(bounds.right())
                .top(bounds.top())
                .w(px(style.stroke))
                .h(bounds.size.height);
            return outer.children([EdgeWitness {
                child: witness.into_any_element(),
                anchor,
            }]);
        }
        #[cfg(not(any(test, feature = "ui-test-support")))]
        {
            let _ = selector;
            return outer;
        }
    }
    outer = outer.child(
        div()
            .absolute()
            .left(bounds.left())
            .top(bounds.top())
            .h(bounds.size.height)
            .w(px(style.stroke))
            .bg(rgb(style.outline)),
    );
    outer.child(
        trailing_edge(notch, style.outline, style.outline)
            .debug_selector(selector)
            .absolute()
            .left(bounds.right())
            .top(bounds.top())
            .w(px(style.stroke))
            .h(bounds.size.height),
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
    pub header_height: f32,
    pub header_gradient: Option<super::clip_header_loop::HeaderGradient>,
    pub notch: Notch,
}

/// Wide canonical clips use one background owner outside their content mask.
/// Their transparent layout borders still define the same content/hit bounds.
/// Callers reserve `header_height` for title content and keep body/grid paint
/// below it; passive titles and badges must also leave the upper-right cut clear.
/// This owner paints the background contour, not a GPU mask for arbitrary children.
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
    #[cfg(any(test, feature = "ui-test-support"))]
    let anchor = Rc::new(Cell::new(None));
    #[cfg(any(test, feature = "ui-test-support"))]
    let observed_anchor = anchor.clone();
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
                move |bounds, _, _| {
                    observe.observe(bounds);
                    #[cfg(any(test, feature = "ui-test-support"))]
                    observed_anchor.set(Some(bounds));
                },
                move |bounds, _, window, _| {
                    if visible {
                        let color = window.text_style().color;
                        window.paint_ordered_group(|window| {
                            paint_backdrop(bounds, &paint_style, color, window);
                        });
                        paint_resolved_trailing(bounds, &paint_style, color, window);
                    }
                },
            )
            .absolute()
            .inset_0(),
        );
    let framed = boundary_frame(left, width, backdrop.hover_outline != backdrop.outline)
        .child(paint)
        .child(body);
    #[cfg(any(test, feature = "ui-test-support"))]
    {
        if visible {
            let witness = div()
                .debug_selector(selector)
                .absolute()
                .left(relative(1.0))
                .top_0()
                .bottom_0()
                .w(px(backdrop.stroke));
            return framed.children([EdgeWitness {
                child: witness.into_any_element(),
                anchor,
            }]);
        }
    }
    #[cfg(not(any(test, feature = "ui-test-support")))]
    let _ = selector;
    framed
}

/// Paint on the resolved endpoint so child layout rounding cannot move the edge.
pub(super) fn paint_resolved_trailing(
    bounds: gpui::Bounds<gpui::Pixels>,
    style: &Backdrop,
    color: gpui::Hsla,
    window: &mut gpui::Window,
) {
    let cut = style.notch.enabled().then(|| style.notch.extent());
    paint_trailing_with_cut(bounds, style.stroke, cut, color, window);
}

pub(super) fn paint_trailing_with_cut(
    bounds: gpui::Bounds<gpui::Pixels>,
    stroke: f32,
    notch_cut: Option<f32>,
    color: gpui::Hsla,
    window: &mut gpui::Window,
) {
    let cut_extent = notch_cut.unwrap_or(0.0);
    if bounds.size.width <= px(0.0) || px(cut_extent) >= bounds.size.height {
        return;
    }
    let edge = gpui::Bounds::new(
        gpui::point(bounds.right(), bounds.top()),
        gpui::size(px(stroke), bounds.size.height),
    );
    let mask = notch_cut.map(|cut| gpui::ContentMask {
        bounds: gpui::Bounds::from_corners(
            gpui::point(edge.left(), edge.top() + px(cut)),
            edge.bottom_right(),
        ),
    });
    window.with_content_mask(mask, |window| window.paint_quad(gpui::fill(edge, color)));
}

pub(super) fn paint_backdrop(
    bounds: gpui::Bounds<gpui::Pixels>,
    style: &Backdrop,
    color: gpui::Hsla,
    window: &mut gpui::Window,
) {
    paint_backdrop_with_cut(
        bounds,
        style.body,
        style.header,
        style.stroke,
        style.header_height,
        style.header_gradient.as_ref(),
        style.notch.extent(),
        color,
        window,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_backdrop_with_cut(
    bounds: gpui::Bounds<gpui::Pixels>,
    body: u32,
    header_color: u32,
    stroke: f32,
    header_height: f32,
    header_gradient: Option<&super::clip_header_loop::HeaderGradient>,
    cut: f32,
    color: gpui::Hsla,
    window: &mut gpui::Window,
) {
    use gpui::{Bounds, ContentMask, Corners, Edges, fill, point, quad};
    if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
        return;
    }
    let paint_square = |window: &mut gpui::Window| {
        window.paint_quad(fill(bounds, rgb(body)));
        // clip_name's -1 offsets compensate the containing layout border. Preserve
        // its old quad and ancestor mask exactly, including physical leading edges.
        let header = Bounds::from_corners(
            point(bounds.left() + px(stroke - 1.0), bounds.top()),
            point(bounds.right() + px(1.0), bounds.top() + px(header_height)),
        );
        let inner = Bounds::from_corners(
            point(bounds.left() + px(stroke), bounds.top() + px(1.0)),
            point(bounds.right(), bounds.bottom() - px(1.0)),
        );
        window.with_content_mask(Some(ContentMask { bounds: inner }), |window| {
            window.paint_quad(fill(header, rgb(header_color)));
        });
        window.paint_quad(quad(
            bounds,
            Corners::default(),
            gpui::transparent_black(),
            Edges {
                top: px(1.0),
                bottom: px(1.0),
                left: px(stroke),
                right: px(0.0),
            },
            color,
            Default::default(),
        ));
    };
    let paint_header_gradient = |window: &mut gpui::Window| {
        if let Some(gradient) = header_gradient {
            gradient.paint(bounds, header_height, stroke, cut, header_color, window);
        }
    };
    if cut == 0.0 {
        paint_square(window);
        paint_header_gradient(window);
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
    // When the header covers the notch band (cut <= header_height), skip the
    // occluded body quad in upper_left and paint only the header and border.
    window.with_content_mask(Some(ContentMask { bounds: upper_left }), |window| {
        if cut > header_height {
            paint_square(window);
            return;
        }
        let header = Bounds::from_corners(
            point(bounds.left() + px(stroke - 1.0), bounds.top()),
            point(bounds.right() + px(1.0), bounds.top() + px(header_height)),
        );
        let inner = Bounds::from_corners(
            point(bounds.left() + px(stroke), bounds.top() + px(1.0)),
            point(bounds.right(), bounds.bottom() - px(1.0)),
        );
        window.with_content_mask(Some(ContentMask { bounds: inner }), |window| {
            window.paint_quad(fill(header, rgb(header_color)));
        });
        window.paint_quad(quad(
            bounds,
            Corners::default(),
            gpui::transparent_black(),
            Edges {
                top: px(1.0),
                bottom: px(1.0),
                left: px(stroke),
                right: px(0.0),
            },
            color,
            Default::default(),
        ));
    });
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        if let Some(path) = notch_fill(bounds.right(), bounds.top(), px(cut), px(stroke)) {
            window.paint_path(path, rgb(header_color));
        }
        paint_header_gradient(window);
        if let Some(path) = notch_diagonal(bounds.right(), bounds.top(), px(cut), px(stroke)) {
            window.paint_path(path, color);
        }
    });
}

/// Direct two-triangle diagonal stroke for the 45-degree notch corner, avoiding
/// per-clip Lyon path stroke tessellation across dense timelines.
fn notch_diagonal(
    right: gpui::Pixels,
    top: gpui::Pixels,
    cut: gpui::Pixels,
    stroke: gpui::Pixels,
) -> Option<gpui::Path<gpui::Pixels>> {
    use gpui::{Path, point};

    let cut_f = f32::from(cut);
    let stroke_f = f32::from(stroke);
    if !cut_f.is_finite() || !stroke_f.is_finite() || cut_f <= 0.0 || stroke_f <= 0.0 {
        return None;
    }
    let inset = px(stroke_f / (2.0 * std::f32::consts::SQRT_2));
    let p0 = point(right - cut - inset, top + inset);
    let p1 = point(right - inset, top + cut + inset);
    let a = point(p0.x - inset, p0.y + inset);
    let b = point(p0.x + inset, p0.y - inset);
    let c = point(p1.x + inset, p1.y - inset);
    let d = point(p1.x - inset, p1.y + inset);
    let solid = point(0.0, 1.0);
    let mut path = Path::new(a);
    path.push_triangle((a, b, c), (solid, solid, solid));
    path.push_triangle((a, c, d), (solid, solid, solid));
    Some(path)
}

/// Fill the five-point convex header corner. Its bottom and left edges
/// overlap the adjacent quads, which keeps their antialiased joins covered.
fn notch_fill(
    right: gpui::Pixels,
    top: gpui::Pixels,
    cut: gpui::Pixels,
    stroke: gpui::Pixels,
) -> Option<gpui::Path<gpui::Pixels>> {
    use gpui::{Path, PathBuilder, point};

    let top_left = point(right - cut - stroke, top);
    let notch_start = point(right - cut, top);
    let notch_end = point(right, top + cut);
    let bottom_right = point(right, top + cut + stroke);
    let bottom_left = point(top_left.x, bottom_right.y);
    if top_left.x < notch_start.x
        && notch_start.x < right
        && top < notch_end.y
        && notch_end.y < bottom_right.y
    {
        let solid = point(0.0, 1.0);
        let mut path = Path::new(top_left);
        for triangle in [
            (notch_start, top_left, notch_end),
            (notch_end, top_left, bottom_left),
            (notch_end, bottom_left, bottom_right),
        ] {
            path.push_triangle(triangle, (solid, solid, solid));
        }
        return Some(path);
    }

    // Preserve the generic path for collapsed vertices after layout rounding.
    let mut path = PathBuilder::fill();
    path.move_to(top_left);
    path.line_to(notch_start);
    path.line_to(notch_end);
    path.line_to(bottom_right);
    path.line_to(bottom_left);
    path.close();
    path.build().ok()
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "clip_border_ui_test.rs"]
pub(super) mod tests;

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    #[test]
    fn corner_geometry_is_bounded_and_resolves_again_after_resize() {
        use super::{Notch, corner_extent};
        assert_eq!(corner_extent(100.0, 44.0), 5.0);
        assert_eq!(corner_extent(6.0, 44.0), 3.0);
        assert_eq!(corner_extent(100.0, 4.0), 2.0);
        assert_eq!(corner_extent(0.0, 44.0), 0.0);
        assert_eq!(corner_extent(-1.0, 44.0), 0.0);
        assert_eq!(corner_extent(f32::NAN, 44.0), 0.0);
        let notch = Notch::new(true);
        let edge = notch.clone();
        for width in [100.0, 6.0, 100.0] {
            notch.observe(gpui::Bounds::new(
                gpui::point(gpui::px(10.0), gpui::px(20.0)),
                gpui::size(gpui::px(width), gpui::px(44.0)),
            ));
            assert_eq!(edge.extent(), corner_extent(width, 44.0));
        }
        let cropped = Notch::default();
        cropped.observe(gpui::Bounds::new(
            gpui::point(gpui::px(0.0), gpui::px(0.0)),
            gpui::size(gpui::px(100.0), gpui::px(44.0)),
        ));
        assert_eq!(cropped.extent(), 0.0);
    }
}
