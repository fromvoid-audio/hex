//! GPUI rendering, static/dynamic curve painting, and pointer capture for `ResponseGraph`.
use super::*;

impl gpui::Render for ResponseGraph {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // GPUI's blur subscription runs after installing the newly painted
        // focus path. Retire before paint too, so the first blurred frame does
        // not replay an icon belonging to the formerly focused graph.
        let focused = self.focus.is_focused(window);
        if self.creation_hint_was_focused && !focused {
            self.clear_creation_hint(cx);
        }
        self.creation_hint_was_focused = focused;
        self.cancel_unfocused_creation(window, cx);
        let analyzer = self.model.analyzer.clone();
        let input_analyzer = self.model.input_analyzer.clone();
        let analyzer_peak = self.model.analyzer_peak.clone();
        let analyzer_style = self.analyzer_style.unwrap_or(ResponseGraphAnalyzerStyle {
            input: self.style.muted,
            output: self.style.muted,
        });
        let nodes = self.model.nodes.clone();
        let circular_nodes = self.circular_nodes;
        let hovered_node = self.hovered_node;
        let style = self.style;
        let has_axes = self.axes.is_some();
        let labels = self
            .axes
            .as_ref()
            .map(|axes| axes.labels(&self.id, style, self.bounds, window));
        let band = self
            .marquee
            .as_ref()
            .filter(|press| press.active)
            .map(|press| press.band);
        let bounds_entity = cx.entity();
        if self.focus_blur.is_none() {
            let focus = self.focus.clone();
            self.focus_blur = Some(cx.on_blur(&focus, window, |graph, _, cx| graph.cancel(cx)));
        }
        div()
            .id(self.id.clone())
            .debug_selector(|| self.id.to_string())
            .relative()
            .w_full()
            .h_full()
            .track_focus(&self.focus)
            .when(self.backspace_removal, |graph| {
                graph
                    .key_context("hex_response_graph_remove")
                    .on_action(cx.listener(Self::remove_selected))
                    .on_action(cx.listener(Self::cancel_pointer))
            })
            .on_key_down(cx.listener(Self::key_down))
            .cursor(if self.drag.is_some() {
                CursorStyle::ResizeColumn
            } else if self.creation_hover.is_some() || self.marquee.is_some() {
                CursorStyle::Crosshair
            } else {
                CursorStyle::PointingHand
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(|graph, event: &MouseMoveEvent, _, cx| {
                graph.hover_creation_hint(event, cx);
                if graph.circular_nodes {
                    graph.node_hover_pointer = Some(event.position);
                    graph.refresh_node_hover(cx);
                }
            }))
            .on_hover(cx.listener(|graph, hovered: &bool, _, cx| {
                if !hovered {
                    graph.clear_creation_hint(cx);
                    graph.node_hover_pointer = None;
                    graph.refresh_node_hover(cx);
                }
            }))
            .on_scroll_wheel(cx.listener(|graph, event, window, cx| {
                graph.clear_creation_hint(cx);
                graph.scroll_wheel(event, window, cx);
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, app| {
                        let static_paths = bounds_entity.update(app, |graph, cx| {
                            let changed = graph.bounds.is_none_or(|old| old.size != bounds.size);
                            let moved = graph.bounds != Some(bounds);
                            graph.bounds = Some(bounds);
                            if moved {
                                graph.refresh_creation_hint(cx);
                                graph.refresh_node_hover(cx);
                            }
                            if changed && graph.axes.is_some() {
                                cx.notify();
                            }
                            graph.ensure_static_paths(bounds)
                        });
                        if let Some(static_paths) = static_paths {
                            paint_graph(
                                bounds,
                                has_axes,
                                &static_paths,
                                analyzer.as_deref(),
                                input_analyzer.as_deref(),
                                analyzer_peak.as_deref(),
                                analyzer_style,
                                &nodes,
                                circular_nodes,
                                hovered_node,
                                style,
                                window,
                            );
                        }
                        if let Some(band) = band {
                            band.paint(axes::plot_bounds(bounds, has_axes), style.accent, window);
                        }
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .children(labels)
            .children(self.creation_hint_element())
            .when(
                self.focus.tab_stop && self.focus.is_focused(window),
                |root| root.child(corner_markers(&self.id, style.focus_marker)),
            )
            .child(ResponseGraphCaptureElement {
                entity: cx.entity(),
            })
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_graph(
    bounds: Bounds<Pixels>,
    has_axes: bool,
    static_paths: &CachedStaticPaths,
    analyzer: Option<&[[f32; 2]]>,
    input_analyzer: Option<&[[f32; 2]]>,
    analyzer_peak: Option<&[[f32; 2]]>,
    analyzer_style: ResponseGraphAnalyzerStyle,
    nodes: &[ResponseGraphNode],
    circular_nodes: bool,
    hovered_node: Option<u32>,
    style: ControlStyle,
    window: &mut Window,
) {
    let bounds = axes::plot_bounds(bounds, has_axes);
    let width = bounds.size.width;
    let height = bounds.size.height;
    if width <= px(0.0) || height <= px(0.0) {
        return;
    }
    let origin = bounds.origin;
    let plot = bounds;
    let spectra = [
        input_analyzer.map(|points| SpectrumLayer {
            points,
            peak: None,
            color: analyzer_style.input,
            stroke_width: spectrum_painter::SECONDARY_TRACE_WIDTH,
        }),
        analyzer.map(|points| SpectrumLayer {
            points,
            peak: analyzer_peak,
            color: analyzer_style.output,
            stroke_width: spectrum_painter::PRIMARY_TRACE_WIDTH,
        }),
    ]
    .into_iter()
    .flatten();
    spectrum_painter::paint(plot, spectra.clone(), SpectrumPass::Fill, window);
    if let Some(grid) = &static_paths.grid {
        window.paint_path(grid.clone(), rgb(style.rail));
    }
    spectrum_painter::paint(plot, spectra.clone(), SpectrumPass::Peak, window);
    spectrum_painter::paint(plot, spectra, SpectrumPass::Trace, window);
    for (path, color) in &static_paths.references {
        window.paint_path(path.clone(), *color);
    }
    if let Some(secondary) = &static_paths.secondary {
        window.paint_path(secondary.clone(), rgb(style.muted));
    }
    if let Some(primary) = &static_paths.primary {
        window.paint_path(primary.clone(), rgb(style.accent));
    }
    for node in nodes {
        let center = point(
            origin.x + width * node.position[0],
            origin.y + height * (1.0 - node.position[1]),
        );
        let radius = px(if node.selected { 4.0 } else { 3.0 });
        if circular_nodes {
            window.paint_quad(circular_node_quad(
                center,
                radius,
                *node,
                hovered_node,
                style,
            ));
            continue;
        }
        let mut marker = PathBuilder::stroke(px(1.0));
        marker.move_to(point(center.x - radius, center.y));
        marker.line_to(point(center.x, center.y - radius));
        marker.line_to(point(center.x + radius, center.y));
        marker.line_to(point(center.x, center.y + radius));
        marker.line_to(point(center.x - radius, center.y));
        if let Ok(marker) = marker.build() {
            window.paint_path(marker, node_color(*node, style));
        }
    }
}

fn node_color(node: ResponseGraphNode, style: ControlStyle) -> gpui::Hsla {
    match node.accent {
        Some(accent) => {
            let color: gpui::Hsla = rgb(accent).into();
            color.opacity(if node.enabled { 1.0 } else { 0.55 })
        }
        None => rgb(if node.enabled {
            style.accent
        } else {
            style.muted
        })
        .into(),
    }
}

pub(super) fn circular_node_quad(
    center: Point<Pixels>,
    radius: Pixels,
    node: ResponseGraphNode,
    hovered_node: Option<u32>,
    style: ControlStyle,
) -> gpui::PaintQuad {
    let color = node_color(node, style);
    let fill = if node.selected || hovered_node == Some(node.id) {
        color
    } else {
        gpui::transparent_black()
    };
    quad(
        Bounds::new(
            point(center.x - radius, center.y - radius),
            size(radius * 2.0, radius * 2.0),
        ),
        radius,
        fill,
        px(1.0),
        color,
        Default::default(),
    )
}

pub(super) fn build_grid_path(
    origin: Point<Pixels>,
    width: Pixels,
    height: Pixels,
    axes: Option<&ResponseGraphAxes>,
) -> Option<gpui::Path<Pixels>> {
    let mut grid = PathBuilder::stroke(px(0.5));
    if let Some(axes) = axes {
        for (y, _) in &axes.vertical {
            let y = origin.y + height * (1.0 - y);
            grid.move_to(point(origin.x, y));
            grid.line_to(point(origin.x + width, y));
        }
        for (x, _) in &axes.horizontal {
            let x = origin.x + width * *x;
            grid.move_to(point(x, origin.y));
            grid.line_to(point(x, origin.y + height));
        }
    } else {
        for division in 1..4 {
            let y = origin.y + height * division as f32 / 4.0;
            grid.move_to(point(origin.x, y));
            grid.line_to(point(origin.x + width, y));
        }
        for division in 1..8 {
            let x = origin.x + width * division as f32 / 8.0;
            grid.move_to(point(x, origin.y));
            grid.line_to(point(x, origin.y + height));
        }
    }
    grid.build().ok()
}

pub(super) fn build_curve_path(
    origin: Point<Pixels>,
    width: Pixels,
    height: Pixels,
    points: &[[f32; 2]],
    stroke_width: Pixels,
) -> Option<gpui::Path<Pixels>> {
    let mut path = PathBuilder::stroke(stroke_width);
    for (index, [x, y]) in points.iter().enumerate() {
        let point = point(origin.x + width * *x, origin.y + height * (1.0 - *y));
        if index == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    path.build().ok()
}

struct ResponseGraphCaptureElement {
    entity: Entity<ResponseGraph>,
}

impl IntoElement for ResponseGraphCaptureElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ResponseGraphCaptureElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = px(0.0).into();
        style.size.height = px(0.0).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let entity = self.entity.clone();
        window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
            if phase.capture() {
                entity.update(cx, |graph, cx| graph.clear_creation_hint(cx));
            }
        });
        let graph = self.entity.read(cx);
        if (graph.press.is_some() || graph.drag.is_some() || graph.marquee.is_some())
            && graph.focus.is_focused(window)
        {
            window.renew_pointer_capture(
                self.entity.entity_id(),
                if graph.drag.is_some() {
                    CursorStyle::ResizeColumn
                } else if graph.marquee.is_some() {
                    CursorStyle::Crosshair
                } else {
                    CursorStyle::PointingHand
                },
            );
        }
        // This is ordinary hint cleanup only; actual pointer edits use the lease.
        let entity = self.entity.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase.capture() {
                entity.update(cx, |graph, cx| {
                    if event.pressed_button.is_some()
                        || graph
                            .plot_bounds()
                            .is_none_or(|plot| !plot.contains(&event.position))
                    {
                        graph.clear_creation_hint(cx);
                    }
                });
            }
        });
        let entity = self.entity.clone();
        window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
            if phase.capture()
                && !event.is_momentum
                && matches!(
                    event.touch_phase,
                    gpui::TouchPhase::Ended | gpui::TouchPhase::Cancelled
                )
            {
                // Observe only the terminal. Normal hit-tested bubble routing
                // retains first refusal and other panels still receive Ended.
                entity.update(cx, |graph, cx| {
                    graph.observe_scroll_end(
                        matches!(event.touch_phase, gpui::TouchPhase::Cancelled),
                        window,
                        cx,
                    )
                });
            }
        });
    }
}
