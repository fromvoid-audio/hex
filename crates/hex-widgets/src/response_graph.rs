//! Bounded response graph with reusable node hit targets and pointer capture.
use crate::ControlStyle;
use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, LayoutId, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathBuilder, Pixels, Point, SharedString, Style, Subscription, Window, canvas,
    div, point, prelude::*, px, rgb,
};

const MAX_CURVE_POINTS: usize = 256;
const MAX_NODES: usize = 32;
const PLOT_INSET: f32 = 7.0;
const NODE_RADIUS: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResponseGraphNode {
    pub id: u32,
    pub position: [f32; 2],
    pub enabled: bool,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseGraphPhase {
    Select,
    Begin,
    Change,
    Commit,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResponseGraphEvent {
    pub phase: ResponseGraphPhase,
    pub node_id: u32,
    pub position: [f32; 2],
    /// Modifiers captured when the pointer press began. Owners can select a
    /// paired edit target once at Begin and keep it stable for the gesture.
    pub modifiers: Modifiers,
}

#[derive(Clone, Debug)]
pub struct ResponseGraphModel {
    pub primary: Vec<[f32; 2]>,
    pub secondary: Vec<[f32; 2]>,
    pub nodes: Vec<ResponseGraphNode>,
}

impl ResponseGraphModel {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.primary.len() < 2
            || self.primary.len() > MAX_CURVE_POINTS
            || self.secondary.len() != self.primary.len()
            || self.nodes.len() > MAX_NODES
            || !valid_curve(&self.primary)
            || !valid_curve(&self.secondary)
            || self
                .nodes
                .iter()
                .enumerate()
                .any(|(index, node)| self.nodes[..index].iter().any(|prior| prior.id == node.id))
            || self.nodes.iter().any(|node| {
                !node.position[0].is_finite()
                    || !node.position[1].is_finite()
                    || !(0.0..=1.0).contains(&node.position[0])
                    || !(0.0..=1.0).contains(&node.position[1])
            })
        {
            return Err("invalid response graph model");
        }
        Ok(())
    }
}

fn valid_curve(points: &[[f32; 2]]) -> bool {
    points.iter().all(|[x, y]| {
        x.is_finite() && y.is_finite() && (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y)
    }) && points.windows(2).all(|pair| pair[0][0] <= pair[1][0])
}

/// Compact DSP-independent graph. The owner maps normalized graph points to
/// domain values and sends the emitted drag phases through its normal edit API.
pub struct ResponseGraph {
    id: SharedString,
    model: ResponseGraphModel,
    style: ControlStyle,
    bounds: Option<Bounds<Pixels>>,
    drag: Option<Drag>,
    press: Option<Press>,
    focus: FocusHandle,
    focus_blur: Option<Subscription>,
}

#[derive(Clone, Copy)]
struct Drag {
    node_id: u32,
    pickup_offset: [f32; 2],
    modifiers: Modifiers,
}

#[derive(Clone, Copy)]
struct Press {
    node_id: u32,
    pointer_origin: [f32; 2],
    node_origin: [f32; 2],
    pickup_offset: [f32; 2],
    modifiers: Modifiers,
}

impl ResponseGraph {
    pub fn new(cx: &mut Context<Self>, id: impl Into<SharedString>, style: ControlStyle) -> Self {
        let model = ResponseGraphModel {
            primary: vec![[0.0, 0.5], [1.0, 0.5]],
            secondary: vec![[0.0, 0.5], [1.0, 0.5]],
            nodes: Vec::new(),
        };
        Self {
            id: id.into(),
            model,
            style,
            bounds: None,
            drag: None,
            press: None,
            focus: cx.focus_handle(),
            focus_blur: None,
        }
    }

    pub fn set_model(
        &mut self,
        model: ResponseGraphModel,
        style: ControlStyle,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        model.validate()?;
        let style = style.validate()?;
        if self.model.primary != model.primary
            || self.model.secondary != model.secondary
            || self.model.nodes != model.nodes
            || self.style != style
        {
            self.model = model;
            self.style = style;
            cx.notify();
        }
        Ok(())
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take() {
            let position = self
                .model
                .nodes
                .iter()
                .find(|node| node.id == drag.node_id)
                .map_or([0.5, 0.5], |node| node.position);
            cx.emit(ResponseGraphEvent {
                phase: ResponseGraphPhase::Cancel,
                node_id: drag.node_id,
                position,
                modifiers: drag.modifiers,
            });
            cx.notify();
        } else {
            self.press = None;
        }
    }

    fn normalized_position(&self, position: Point<Pixels>) -> Option<[f32; 2]> {
        let bounds = self.bounds?;
        let inset = px(PLOT_INSET);
        let width = f32::from(bounds.size.width - inset * 2.0);
        let height = f32::from(bounds.size.height - inset * 2.0);
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        Some([
            ((position.x - bounds.origin.x - inset) / px(width)).clamp(0.0, 1.0),
            (1.0 - (position.y - bounds.origin.y - inset) / px(height)).clamp(0.0, 1.0),
        ])
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left || self.drag.is_some() || self.press.is_some() {
            return;
        }
        let Some(position) = self.normalized_position(event.position) else {
            return;
        };
        let Some(bounds) = self.bounds else {
            return;
        };
        let width = f32::from(bounds.size.width - px(PLOT_INSET * 2.0));
        let height = f32::from(bounds.size.height - px(PLOT_INSET * 2.0));
        let hit = self
            .model
            .nodes
            .iter()
            .filter_map(|node| {
                let dx = (node.position[0] - position[0]) * width;
                let dy = (node.position[1] - position[1]) * height;
                let distance = dx.mul_add(dx, dy * dy);
                (distance <= NODE_RADIUS * NODE_RADIUS).then_some((distance, node.id))
            })
            .min_by(|left, right| left.0.total_cmp(&right.0));
        let Some((_, node_id)) = hit else {
            return;
        };
        let node_origin = self
            .model
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .map_or(position, |node| node.position);
        let pickup_offset = [position[0] - node_origin[0], position[1] - node_origin[1]];
        self.press = Some(Press {
            node_id,
            pointer_origin: position,
            node_origin,
            pickup_offset,
            modifiers: event.modifiers,
        });
        self.focus.focus(window);
        cx.emit(ResponseGraphEvent {
            phase: ResponseGraphPhase::Select,
            node_id,
            position: node_origin,
            modifiers: event.modifiers,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn pointer_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        let Some(position) = self.normalized_position(event.position) else {
            return false;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            if self.press.is_some() {
                self.press = None;
                return true;
            }
            let was_dragging = self.drag.is_some();
            self.cancel(cx);
            return was_dragging;
        }
        if self.drag.is_none() {
            let Some(press) = self.press else {
                return false;
            };
            let Some(bounds) = self.bounds else {
                return false;
            };
            let width = f32::from(bounds.size.width - px(PLOT_INSET * 2.0));
            let height = f32::from(bounds.size.height - px(PLOT_INSET * 2.0));
            let dx = (position[0] - press.pointer_origin[0]) * width;
            let dy = (position[1] - press.pointer_origin[1]) * height;
            if !drag_threshold_crossed(dx, dy) {
                return false;
            }
            self.press = None;
            self.drag = Some(Drag {
                node_id: press.node_id,
                pickup_offset: press.pickup_offset,
                modifiers: press.modifiers,
            });
            cx.emit(ResponseGraphEvent {
                phase: ResponseGraphPhase::Begin,
                node_id: press.node_id,
                position: press.node_origin,
                modifiers: press.modifiers,
            });
        }
        let Some(drag) = self.drag else {
            return false;
        };
        cx.emit(ResponseGraphEvent {
            phase: ResponseGraphPhase::Change,
            node_id: drag.node_id,
            position: node_position_from_pointer(position, drag.pickup_offset),
            modifiers: drag.modifiers,
        });
        cx.notify();
        true
    }

    fn pointer_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) -> bool {
        if event.button != MouseButton::Left {
            return false;
        }
        let Some(drag) = self.drag.take() else {
            return self.press.take().is_some();
        };
        if let Some(position) = self.normalized_position(event.position) {
            cx.emit(ResponseGraphEvent {
                phase: ResponseGraphPhase::Commit,
                node_id: drag.node_id,
                position: node_position_from_pointer(position, drag.pickup_offset),
                modifiers: drag.modifiers,
            });
        } else {
            cx.emit(ResponseGraphEvent {
                phase: ResponseGraphPhase::Cancel,
                node_id: drag.node_id,
                position: [0.5, 0.5],
                modifiers: drag.modifiers,
            });
        }
        cx.notify();
        true
    }

    fn key_down(&mut self, event: &gpui::KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" && (self.drag.is_some() || self.press.is_some()) {
            cx.stop_propagation();
            self.cancel(cx);
        }
    }
}

fn drag_threshold_crossed(dx: f32, dy: f32) -> bool {
    dx.mul_add(dx, dy * dy) >= 9.0
}

fn node_position_from_pointer(pointer: [f32; 2], pickup_offset: [f32; 2]) -> [f32; 2] {
    [
        (pointer[0] - pickup_offset[0]).clamp(0.0, 1.0),
        (pointer[1] - pickup_offset[1]).clamp(0.0, 1.0),
    ]
}

impl EventEmitter<ResponseGraphEvent> for ResponseGraph {}

impl Focusable for ResponseGraph {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for ResponseGraph {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let primary = self.model.primary.clone();
        let secondary = self.model.secondary.clone();
        let nodes = self.model.nodes.clone();
        let style = self.style;
        let bounds_entity = cx.entity();
        if self.focus_blur.is_none() {
            let focus = self.focus.clone();
            self.focus_blur = Some(cx.on_blur(&focus, window, |graph, _, cx| graph.cancel(cx)));
        }
        let mut root = div()
            .id(self.id.clone())
            .debug_selector(|| self.id.to_string())
            .relative()
            .w_full()
            .h_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .cursor(if self.drag.is_some() {
                CursorStyle::ResizeColumn
            } else {
                CursorStyle::PointingHand
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .child(canvas(
                |_, _, _| (),
                move |bounds, _, window, app| {
                    bounds_entity.update(app, |graph, _| graph.bounds = Some(bounds));
                    paint_graph(bounds, &primary, &secondary, &nodes, style, window);
                },
            ));
        if self.drag.is_some() || self.press.is_some() {
            root = root.child(ResponseGraphCaptureElement {
                entity: cx.entity(),
            });
        }
        root
    }
}

fn paint_graph(
    bounds: Bounds<Pixels>,
    primary: &[[f32; 2]],
    secondary: &[[f32; 2]],
    nodes: &[ResponseGraphNode],
    style: ControlStyle,
    window: &mut Window,
) {
    let inset = px(PLOT_INSET);
    let width = bounds.size.width - inset * 2.0;
    let height = bounds.size.height - inset * 2.0;
    if width <= px(0.0) || height <= px(0.0) {
        return;
    }
    let origin = point(bounds.origin.x + inset, bounds.origin.y + inset);
    let mut grid = PathBuilder::stroke(px(0.5));
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
    if let Ok(grid) = grid.build() {
        window.paint_path(grid, rgb(style.rail));
    }
    paint_curve(
        origin,
        width,
        height,
        secondary,
        px(0.75),
        style.muted,
        window,
    );
    paint_curve(
        origin,
        width,
        height,
        primary,
        px(1.15),
        style.accent,
        window,
    );
    for node in nodes {
        let center = point(
            origin.x + width * node.position[0],
            origin.y + height * (1.0 - node.position[1]),
        );
        let radius = px(if node.selected { 4.0 } else { 3.0 });
        let mut marker = PathBuilder::stroke(px(1.0));
        marker.move_to(point(center.x - radius, center.y));
        marker.line_to(point(center.x, center.y - radius));
        marker.line_to(point(center.x + radius, center.y));
        marker.line_to(point(center.x, center.y + radius));
        marker.line_to(point(center.x - radius, center.y));
        if let Ok(marker) = marker.build() {
            window.paint_path(
                marker,
                rgb(if node.enabled {
                    style.accent
                } else {
                    style.muted
                }),
            );
        }
    }
}

fn paint_curve(
    origin: Point<Pixels>,
    width: Pixels,
    height: Pixels,
    points: &[[f32; 2]],
    stroke_width: Pixels,
    color: u32,
    window: &mut Window,
) {
    let mut path = PathBuilder::stroke(stroke_width);
    for (index, [x, y]) in points.iter().enumerate() {
        let point = point(origin.x + width * *x, origin.y + height * (1.0 - *y));
        if index == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, rgb(color));
    }
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
        _: &mut App,
    ) {
        let entity = self.entity.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if !phase.capture() {
                return;
            }
            let handled = entity.update(cx, |graph, cx| graph.pointer_move(event, cx));
            if handled {
                cx.stop_propagation();
                window.refresh();
            }
        });
        let entity = self.entity.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if !phase.capture() {
                return;
            }
            let handled = entity.update(cx, |graph, cx| graph.pointer_up(event, cx));
            if handled {
                cx.stop_propagation();
                window.refresh();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ResponseGraphModel {
        ResponseGraphModel {
            primary: vec![[0.0, 0.0], [0.5, 1.0], [1.0, 0.0]],
            secondary: vec![[0.0, 0.1], [0.5, 0.5], [1.0, 0.1]],
            nodes: vec![ResponseGraphNode {
                id: 3,
                position: [0.5, 0.5],
                enabled: true,
                selected: true,
            }],
        }
    }

    #[test]
    fn response_graph_accepts_bounded_curves_and_rejects_invalid_models() {
        assert!(model().validate().is_ok());
        let mut invalid = model();
        invalid.nodes[0].position[0] = f32::NAN;
        assert!(invalid.validate().is_err());
        let mut invalid = model();
        invalid.primary[1][0] = 1.5;
        assert!(invalid.validate().is_err());
        let mut invalid = model();
        invalid.secondary.pop();
        assert!(invalid.validate().is_err());
        let mut invalid = model();
        invalid.nodes.push(ResponseGraphNode {
            id: invalid.nodes[0].id,
            position: [0.75, 0.5],
            enabled: true,
            selected: false,
        });
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn graph_drag_threshold_uses_pointer_origin_and_preserves_pickup_offset() {
        assert!(!drag_threshold_crossed(2.0, 1.0));
        assert!(drag_threshold_crossed(3.0, 0.0));
        let adjusted = node_position_from_pointer([0.7, 0.4], [0.1, -0.1]);
        assert!((adjusted[0] - 0.6).abs() < 0.00001);
        assert_eq!(adjusted[1], 0.5);
        assert_eq!(
            node_position_from_pointer([0.0, 1.0], [0.2, -0.2]),
            [0.0, 1.0]
        );
    }
}
