//! Bounded response graph with reusable node hit targets and pointer capture.
use crate::ControlStyle;
use crate::focus_marker::corner_markers;
use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, LayoutId, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathBuilder, Pixels, Point, SharedString, Style, Subscription, Window, canvas,
    div, point, prelude::*, px, rgb,
};

gpui::actions!(hex_response_graph, [RemoveNode, CancelPointer]);

/// Install opt-in graph Backspace and held-pointer Escape bindings.
pub fn bind_response_graph_keys(cx: &mut App) {
    cx.bind_keys([
        gpui::KeyBinding::new("backspace", RemoveNode, Some("hex_response_graph_remove")),
        gpui::KeyBinding::new("escape", CancelPointer, Some("hex_response_graph_remove")),
    ]);
}

mod axes;
pub use axes::ResponseGraphAxes;

const MAX_CURVE_POINTS: usize = 256;
const MAX_NODES: usize = 64;
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
    /// Owner-opted-in blank plot press; subsequent double-click presses do not create again.
    /// With creation dragging enabled, its pointer stream keeps node_id zero.
    Create,
    Select,
    /// Owner-opted-in Backspace on an idle, focused selected node.
    Remove,
    /// Physical pointer press on a node; does not start a parameter edit.
    Press,
    Begin,
    Change,
    Commit,
    /// Physical pointer release after either a stationary press or an edit.
    Release,
    /// Physical pointer release outside the graph after preserving any edit.
    ReleaseOutside,
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
    /// Optional non-editable analyzer trace, painted behind the device response.
    pub analyzer: Option<Vec<[f32; 2]>>,
    /// Optional aligned input spectrum, independently hidden by its owner.
    pub input_analyzer: Option<Vec<[f32; 2]>>,
    pub nodes: Vec<ResponseGraphNode>,
}

impl ResponseGraphModel {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.primary.len() < 2
            || self.primary.len() > MAX_CURVE_POINTS
            || self.secondary.len() != self.primary.len()
            || self.analyzer.as_ref().is_some_and(|curve| {
                curve.len() < 2 || curve.len() > MAX_CURVE_POINTS || !valid_curve(curve)
            })
            || self.input_analyzer.as_ref().is_some_and(|curve| {
                curve.len() < 2 || curve.len() > MAX_CURVE_POINTS || !valid_curve(curve)
            })
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

/// Product-resolved spectrum colors, separate from response and control colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResponseGraphAnalyzerStyle {
    pub input: u32,
    pub output: u32,
}

/// Compact DSP-independent graph. The owner maps normalized graph points to
/// domain values and sends the emitted drag phases through its normal edit API.
pub struct ResponseGraph {
    id: SharedString,
    model: ResponseGraphModel,
    style: ControlStyle,
    analyzer_style: Option<ResponseGraphAnalyzerStyle>,
    axes: Option<ResponseGraphAxes>,
    bounds: Option<Bounds<Pixels>>,
    drag: Option<Drag>,
    press: Option<Press>,
    editable: bool,
    creation_enabled: bool,
    creation_drag_enabled: bool,
    tab_navigation: bool,
    backspace_removal: bool,
    focus: FocusHandle,
    focus_blur: Option<Subscription>,
}

#[derive(Clone, Copy)]
struct Drag {
    node_id: u32,
    pointer_origin: [f32; 2],
    node_origin: [f32; 2],
    modifiers: Modifiers,
}

#[derive(Clone, Copy)]
struct Press {
    node_id: u32,
    pointer_origin: [f32; 2],
    node_origin: [f32; 2],
    modifiers: Modifiers,
}

impl ResponseGraph {
    pub fn new(cx: &mut Context<Self>, id: impl Into<SharedString>, style: ControlStyle) -> Self {
        let model = ResponseGraphModel {
            primary: vec![[0.0, 0.5], [1.0, 0.5]],
            secondary: vec![[0.0, 0.5], [1.0, 0.5]],
            analyzer: None,
            input_analyzer: None,
            nodes: Vec::new(),
        };
        Self {
            id: id.into(),
            model,
            style,
            analyzer_style: None,
            axes: None,
            bounds: None,
            drag: None,
            press: None,
            editable: false,
            creation_enabled: false,
            creation_drag_enabled: false,
            tab_navigation: false,
            backspace_removal: false,
            focus: cx.focus_handle(),
            focus_blur: None,
        }
    }

    /// Optional owner-authored normalized axes; all graphics and input share this plot.
    pub fn set_axes(
        &mut self,
        axes: Option<ResponseGraphAxes>,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        if axes.as_ref().is_some_and(|axes| !axes.valid()) {
            return Err("invalid response graph axes");
        }
        if self.axes != axes {
            self.axes = axes;
            cx.notify();
        }
        Ok(())
    }

    /// Actual normalized plot rectangle, excluding optional axis labels.
    pub fn plot_bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds
            .map(|bounds| axes::plot_bounds(bounds, self.axes.is_some()))
    }

    /// Allow blank plot presses to create one owner-defined point. Node hits
    /// keep select/drag semantics; double-click's second press is suppressed.
    pub fn set_creation_enabled(&mut self, enabled: bool) {
        self.creation_enabled = enabled;
    }

    /// Continue a blank press as Begin/Change/Commit/Release (or Cancel), using
    /// node_id zero throughout. The owner previews and commits its own stable
    /// identity; publication cannot redirect an already released pointer.
    /// Disabled by default so existing click-only owners retain their behavior.
    pub fn set_creation_drag_enabled(&mut self, enabled: bool) {
        self.creation_drag_enabled = enabled;
    }

    /// Capture Backspace only while this graph owns focus. Owners decide how
    /// an emitted stable node identity maps to their canonical removal command.
    /// Empty, disabled or held-pointer graphs consume the key without removal.
    /// Its scoped Escape binding preserves held-pointer cancellation before
    /// ancestor application actions can handle the key.
    pub fn set_backspace_removal(&mut self, enabled: bool) {
        self.backspace_removal = enabled;
    }

    /// Opt into rendered-order Tab traversal; unavailable controls are skipped.
    /// Owners choose participation without changing pointer or edit semantics.
    pub fn set_tab_navigation(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.tab_navigation = enabled;
        let tab_stop = self.tab_navigation && self.editable;
        if self.focus.tab_stop != tab_stop {
            self.focus = self.focus.clone().tab_index(0).tab_stop(tab_stop);
            cx.notify();
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
            || self.model.analyzer != model.analyzer
            || self.model.input_analyzer != model.input_analyzer
            || self.model.nodes != model.nodes
            || self.style != style
        {
            self.model = model;
            self.style = style;
            cx.notify();
        }
        Ok(())
    }

    /// Inject semantic analyzer colors without changing response appearance.
    pub fn set_analyzer_style(
        &mut self,
        style: ResponseGraphAnalyzerStyle,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        if style.input > 0xffffff || style.output > 0xffffff {
            return Err("invalid analyzer style");
        }
        if self.analyzer_style != Some(style) {
            self.analyzer_style = Some(style);
            cx.notify();
        }
        Ok(())
    }

    /// Current displayed node position, without waiting for the next paint.
    pub fn node_position(&self, id: u32) -> Option<[f32; 2]> {
        self.model
            .nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| node.position)
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Opt into focused keyboard node selection/editing and fine pointer drags.
    /// The default preserves the graph's existing select-and-drag behavior.
    pub fn set_editable(&mut self, editable: bool, cx: &mut Context<Self>) {
        if self.editable != editable {
            self.editable = editable;
            let tab_stop = self.tab_navigation && self.editable;
            if self.focus.tab_stop != tab_stop {
                self.focus = self.focus.clone().tab_index(0).tab_stop(tab_stop);
            }
            cx.notify();
        }
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
        } else if let Some(press) = self.press.take() {
            cx.emit(ResponseGraphEvent {
                phase: ResponseGraphPhase::Cancel,
                node_id: press.node_id,
                position: press.node_origin,
                modifiers: press.modifiers,
            });
            cx.notify();
        }
    }

    fn normalized_position(&self, position: Point<Pixels>) -> Option<[f32; 2]> {
        let bounds = self.plot_bounds()?;
        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
            return None;
        }
        Some([
            ((position.x - bounds.left()) / bounds.size.width).clamp(0.0, 1.0),
            (1.0 - (position.y - bounds.top()) / bounds.size.height).clamp(0.0, 1.0),
        ])
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left || self.drag.is_some() || self.press.is_some() {
            return;
        }
        if self.axes.is_some()
            && !self
                .plot_bounds()
                .is_some_and(|bounds| bounds.contains(&event.position))
        {
            return;
        }
        let Some(position) = self.normalized_position(event.position) else {
            return;
        };
        let Some(bounds) = self.plot_bounds() else {
            return;
        };
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
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
            if self.creation_enabled && event.click_count == 1 {
                if self.creation_drag_enabled {
                    self.press = Some(Press {
                        node_id: 0,
                        pointer_origin: position,
                        node_origin: position,
                        modifiers: event.modifiers,
                    });
                    self.focus.focus(window);
                }
                cx.emit(ResponseGraphEvent {
                    phase: ResponseGraphPhase::Create,
                    node_id: 0,
                    position,
                    modifiers: event.modifiers,
                });
                cx.stop_propagation();
                cx.notify();
            }
            return;
        };
        let node_origin = self
            .model
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .map_or(position, |node| node.position);
        self.press = Some(Press {
            node_id,
            pointer_origin: position,
            node_origin,
            modifiers: event.modifiers,
        });
        self.focus.focus(window);
        cx.emit(ResponseGraphEvent {
            phase: ResponseGraphPhase::Select,
            node_id,
            position: node_origin,
            modifiers: event.modifiers,
        });
        cx.emit(ResponseGraphEvent {
            phase: ResponseGraphPhase::Press,
            node_id,
            position: node_origin,
            modifiers: event.modifiers,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn cancel_unfocused_creation(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let creating = self.creation_drag_enabled
            && (self.press.is_some_and(|press| press.node_id == 0)
                || self.drag.is_some_and(|drag| drag.node_id == 0));
        if creating && !self.focus.is_focused(window) {
            // Focus callbacks compare painted focus paths. A press followed by
            // blur before the next focused paint can coalesce that callback.
            self.cancel(cx);
            return true;
        }
        false
    }

    fn pointer_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.cancel_unfocused_creation(window, cx) {
            return true;
        }
        let Some(position) = self.normalized_position(event.position) else {
            return false;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            if self.press.is_some() {
                self.cancel(cx);
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
            let Some(bounds) = self.plot_bounds() else {
                return false;
            };
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let dx = (position[0] - press.pointer_origin[0]) * width;
            let dy = (position[1] - press.pointer_origin[1]) * height;
            if !drag_threshold_crossed(dx, dy) {
                return false;
            }
            self.press = None;
            self.drag = Some(Drag {
                node_id: press.node_id,
                pointer_origin: press.pointer_origin,
                node_origin: press.node_origin,
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
            position: drag_position(position, drag, self.editable),
            modifiers: drag.modifiers,
        });
        cx.notify();
        true
    }

    fn pointer_up(
        &mut self,
        event: &MouseUpEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != MouseButton::Left {
            return false;
        }
        if self.cancel_unfocused_creation(window, cx) {
            return true;
        }
        let Some(drag) = self.drag.take() else {
            let Some(press) = self.press.take() else {
                return false;
            };
            let inside = point_inside_graph(self.bounds, event.position);
            let (phase, _) = pointer_end_phases(false, inside);
            cx.emit(ResponseGraphEvent {
                phase,
                node_id: press.node_id,
                position: press.node_origin,
                modifiers: press.modifiers,
            });
            cx.notify();
            return true;
        };
        let normalized = self.normalized_position(event.position);
        let inside = point_inside_graph(self.bounds, event.position);
        if let Some(position) = normalized {
            let (first, second) = pointer_end_phases(true, inside);
            cx.emit(ResponseGraphEvent {
                phase: first,
                node_id: drag.node_id,
                position: drag_position(position, drag, self.editable),
                modifiers: drag.modifiers,
            });
            if let Some(phase) = second {
                cx.emit(ResponseGraphEvent {
                    phase,
                    node_id: drag.node_id,
                    position: drag_position(position, drag, self.editable),
                    modifiers: drag.modifiers,
                });
            }
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

    fn cancel_pointer(&mut self, _: &CancelPointer, window: &mut Window, cx: &mut Context<Self>) {
        if self.backspace_removal
            && self.focus.is_focused(window)
            && (self.drag.is_some() || self.press.is_some())
        {
            cx.stop_propagation();
            self.cancel(cx);
        } else {
            cx.propagate();
        }
    }

    fn remove_selected(&mut self, _: &RemoveNode, window: &mut Window, cx: &mut Context<Self>) {
        if !self.backspace_removal || !self.focus.is_focused(window) {
            return;
        }
        cx.stop_propagation();
        if self.editable
            && self.drag.is_none()
            && self.press.is_none()
            && let Some(node) = self.model.nodes.iter().find(|node| node.selected)
        {
            cx.emit(ResponseGraphEvent {
                phase: ResponseGraphPhase::Remove,
                node_id: node.id,
                position: node.position,
                modifiers: Modifiers::default(),
            });
            cx.notify();
        }
    }

    fn key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.backspace_removal
            && self.focus.is_focused(window)
            && event.keystroke.key == "backspace"
        {
            // A focused empty/pending graph must not forward deletion to its
            // editor's track/clip action. Numeric/text siblings own their keys.
            cx.stop_propagation();
            let modifiers = event.keystroke.modifiers;
            if !modifiers.platform && !modifiers.control && !modifiers.alt && !modifiers.shift {
                self.remove_selected(&RemoveNode, window, cx);
            }
            return;
        }
        if event.keystroke.key == "escape" && (self.drag.is_some() || self.press.is_some()) {
            cx.stop_propagation();
            self.cancel(cx);
            return;
        }
        let modifiers = event.keystroke.modifiers;
        let node_cycle = event.keystroke.key == "tab"
            && (if self.focus.tab_stop {
                modifiers.control
            } else {
                !modifiers.control
            });
        if !self.editable
            || modifiers.platform
            || modifiers.alt
            || (modifiers.control && !node_cycle)
        {
            return;
        }
        // Owners opting into Tab traversal use Ctrl-Tab to cycle graph nodes.
        // Plain Tab/Shift-Tab bubble to the owner; other graphs retain their keys.
        if event.keystroke.key == "tab" && !node_cycle {
            return;
        }
        if node_cycle {
            let selected = self.model.nodes.iter().position(|node| node.selected);
            let Some(next) = cycle_node(
                selected,
                self.model.nodes.len(),
                event.keystroke.modifiers.shift,
            )
            .and_then(|index| self.model.nodes.get(index))
            .copied() else {
                return;
            };
            cx.stop_propagation();
            cx.emit(ResponseGraphEvent {
                phase: ResponseGraphPhase::Select,
                node_id: next.id,
                position: next.position,
                modifiers: event.keystroke.modifiers,
            });
            cx.notify();
            return;
        }
        let delta = match event.keystroke.key.as_str() {
            "left" => [-1.0, 0.0],
            "right" => [1.0, 0.0],
            "up" => [0.0, 1.0],
            "down" => [0.0, -1.0],
            _ => return,
        };
        let Some(node) = self.model.nodes.iter().find(|node| node.selected).copied() else {
            return;
        };
        let step = if event.keystroke.modifiers.shift {
            0.001
        } else {
            0.01
        };
        let position = [
            (node.position[0] + delta[0] * step).clamp(0.0, 1.0),
            (node.position[1] + delta[1] * step).clamp(0.0, 1.0),
        ];
        if position == node.position {
            return;
        }
        cx.stop_propagation();
        for phase in [
            ResponseGraphPhase::Begin,
            ResponseGraphPhase::Change,
            ResponseGraphPhase::Commit,
        ] {
            cx.emit(ResponseGraphEvent {
                phase,
                node_id: node.id,
                position: if phase == ResponseGraphPhase::Begin {
                    node.position
                } else {
                    position
                },
                modifiers: event.keystroke.modifiers,
            });
        }
        cx.notify();
    }
}

fn pointer_end_phases(
    dragging: bool,
    inside: bool,
) -> (ResponseGraphPhase, Option<ResponseGraphPhase>) {
    match (dragging, inside) {
        (false, true) => (ResponseGraphPhase::Release, None),
        (false, false) => (ResponseGraphPhase::ReleaseOutside, None),
        (true, false) => (
            ResponseGraphPhase::Commit,
            Some(ResponseGraphPhase::ReleaseOutside),
        ),
        (true, true) => (
            ResponseGraphPhase::Commit,
            Some(ResponseGraphPhase::Release),
        ),
    }
}

fn point_inside_graph(bounds: Option<Bounds<Pixels>>, position: Point<Pixels>) -> bool {
    bounds.is_some_and(|bounds| bounds.contains(&position))
}

fn drag_threshold_crossed(dx: f32, dy: f32) -> bool {
    dx.mul_add(dx, dy * dy) >= 9.0
}

fn drag_position(pointer: [f32; 2], drag: Drag, editable: bool) -> [f32; 2] {
    let precision = if editable && drag.modifiers.shift {
        0.1
    } else {
        1.0
    };
    [
        (drag.node_origin[0] + (pointer[0] - drag.pointer_origin[0]) * precision).clamp(0.0, 1.0),
        (drag.node_origin[1] + (pointer[1] - drag.pointer_origin[1]) * precision).clamp(0.0, 1.0),
    ]
}

fn cycle_node(selected: Option<usize>, count: usize, reverse: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (selected, reverse) {
        (Some(index), false) => (index + 1) % count,
        (Some(0), true) | (None, true) => count - 1,
        (Some(index), true) => index - 1,
        (None, false) => 0,
    })
}

impl EventEmitter<ResponseGraphEvent> for ResponseGraph {}

impl Focusable for ResponseGraph {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for ResponseGraph {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.cancel_unfocused_creation(window, cx);
        let primary = self.model.primary.clone();
        let secondary = self.model.secondary.clone();
        let analyzer = self.model.analyzer.clone();
        let input_analyzer = self.model.input_analyzer.clone();
        let analyzer_style = self.analyzer_style.unwrap_or(ResponseGraphAnalyzerStyle {
            input: self.style.muted,
            output: self.style.muted,
        });
        let nodes = self.model.nodes.clone();
        let style = self.style;
        let axes = self.axes.clone();
        let labels = self
            .axes
            .as_ref()
            .map(|axes| axes.labels(&self.id, style, self.bounds, window));
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
            } else {
                CursorStyle::PointingHand
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, app| {
                        bounds_entity.update(app, |graph, cx| {
                            let changed = graph.bounds.is_none_or(|old| old.size != bounds.size);
                            graph.bounds = Some(bounds);
                            if changed && graph.axes.is_some() {
                                cx.notify();
                            }
                        });
                        paint_graph(
                            bounds,
                            axes.as_ref(),
                            &primary,
                            &secondary,
                            analyzer.as_deref(),
                            input_analyzer.as_deref(),
                            analyzer_style,
                            &nodes,
                            style,
                            window,
                        );
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .children(labels)
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
    axes: Option<&ResponseGraphAxes>,
    primary: &[[f32; 2]],
    secondary: &[[f32; 2]],
    analyzer: Option<&[[f32; 2]]>,
    input_analyzer: Option<&[[f32; 2]]>,
    analyzer_style: ResponseGraphAnalyzerStyle,
    nodes: &[ResponseGraphNode],
    style: ControlStyle,
    window: &mut Window,
) {
    let bounds = axes::plot_bounds(bounds, axes.is_some());
    let width = bounds.size.width;
    let height = bounds.size.height;
    if width <= px(0.0) || height <= px(0.0) {
        return;
    }
    let origin = bounds.origin;
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
    if let Ok(grid) = grid.build() {
        window.paint_path(grid, rgb(style.rail));
    }
    if let Some(input) = input_analyzer {
        paint_curve(
            origin,
            width,
            height,
            input,
            px(0.7),
            analyzer_style.input,
            window,
        );
    }
    if let Some(analyzer) = analyzer {
        paint_curve(
            origin,
            width,
            height,
            analyzer,
            px(0.9),
            analyzer_style.output,
            window,
        );
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
            let handled = entity.update(cx, |graph, cx| graph.pointer_move(event, window, cx));
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
            let handled = entity.update(cx, |graph, cx| graph.pointer_up(event, window, cx));
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
            analyzer: None,
            input_analyzer: None,
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
        let mut with_analyzer = model();
        with_analyzer.analyzer = Some(vec![[0.0, 0.4], [0.5, 0.8], [1.0, 0.2]]);
        assert!(with_analyzer.validate().is_ok());
        with_analyzer.analyzer.as_mut().unwrap()[1][1] = f32::NAN;
        assert!(with_analyzer.validate().is_err());
        let mut with_input = model();
        with_input.input_analyzer = Some(vec![[0.0, 0.3], [1.0, 0.7]]);
        assert!(with_input.validate().is_ok());
        with_input.input_analyzer.as_mut().unwrap()[0][1] = f32::INFINITY;
        assert!(with_input.validate().is_err());
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
    fn graph_drag_threshold_uses_pointer_origin_and_fine_mode_preserves_origin() {
        assert!(!drag_threshold_crossed(2.0, 1.0));
        assert!(drag_threshold_crossed(3.0, 0.0));
        let drag = Drag {
            node_id: 3,
            pointer_origin: [0.5, 0.5],
            node_origin: [0.45, 0.55],
            modifiers: Modifiers::default(),
        };
        let normal = drag_position([0.7, 0.4], drag, true);
        assert!((normal[0] - 0.65).abs() < 0.00001);
        assert!((normal[1] - 0.45).abs() < 0.00001);
        let fine = Drag {
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            ..drag
        };
        let fine_position = drag_position([0.7, 0.4], fine, true);
        assert!((fine_position[0] - 0.47).abs() < 0.00001);
        assert!((fine_position[1] - 0.54).abs() < 0.00001);
        let unmodified = drag_position([0.7, 0.4], fine, false);
        assert!((unmodified[0] - 0.65).abs() < 0.00001);
        assert!((unmodified[1] - 0.45).abs() < 0.00001);
        assert_eq!(drag_position([5.0, -2.0], drag, true), [1.0, 0.0]);
    }

    #[test]
    fn pointer_press_lifecycle_distinguishes_outside_release_from_cancellation() {
        assert_eq!(
            pointer_end_phases(false, true),
            (ResponseGraphPhase::Release, None)
        );
        assert_eq!(
            pointer_end_phases(false, false),
            (ResponseGraphPhase::ReleaseOutside, None)
        );
        assert_eq!(
            pointer_end_phases(true, true),
            (
                ResponseGraphPhase::Commit,
                Some(ResponseGraphPhase::Release)
            )
        );
        assert_eq!(
            pointer_end_phases(true, false),
            (
                ResponseGraphPhase::Commit,
                Some(ResponseGraphPhase::ReleaseOutside)
            )
        );
    }

    #[test]
    fn release_hit_test_uses_graph_bounds_while_drag_positions_remain_clamped() {
        let bounds = Bounds {
            origin: point(px(10.0), px(20.0)),
            size: gpui::size(px(100.0), px(80.0)),
        };
        let inside = point(px(50.0), px(50.0));
        let outside = point(px(150.0), px(50.0));

        assert!(point_inside_graph(Some(bounds), inside));
        assert!(!point_inside_graph(Some(bounds), outside));
        assert!(!point_inside_graph(None, inside));
        assert_eq!(
            pointer_end_phases(true, point_inside_graph(Some(bounds), inside)).1,
            Some(ResponseGraphPhase::Release)
        );
        assert_eq!(
            pointer_end_phases(true, point_inside_graph(Some(bounds), outside)),
            (
                ResponseGraphPhase::Commit,
                Some(ResponseGraphPhase::ReleaseOutside)
            )
        );
    }

    #[test]
    fn keyboard_node_cycle_handles_coincident_nodes_and_empty_graphs() {
        assert_eq!(cycle_node(None, 0, false), None);
        assert_eq!(cycle_node(None, 4, false), Some(0));
        assert_eq!(cycle_node(Some(1), 4, false), Some(2));
        assert_eq!(cycle_node(Some(3), 4, false), Some(0));
        assert_eq!(cycle_node(None, 4, true), Some(3));
        assert_eq!(cycle_node(Some(0), 4, true), Some(3));
        assert_eq!(cycle_node(Some(2), 4, true), Some(1));
    }
}
