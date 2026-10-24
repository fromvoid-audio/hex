//! Captured splitter input with caller-owned local sizing snapshots. A binding
//! paints one handle and installs frame-scoped native window listeners; it is
//! not a global capture manager. The host retains the sole active gesture and
//! decides focus, source-generation, collapse, viewport and persistence policy.
//! Only finite pixel displacement and typed local intents cross this boundary.
use gpui::{
    App, Context, Div, Entity, IntoElement, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, Stateful, Window, WindowId, canvas,
    div, prelude::*, px, rgb,
};

/// Movement axis: horizontal input resizes left/right; vertical input resizes
/// up/down. The host decides which side grows for a positive displacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitterAxis {
    Horizontal,
    Vertical,
}

/// Validated handle geometry and resolved paint; no sizing policy is inferred.
#[derive(Clone, Copy, Debug)]
pub struct SplitterHandleConfig {
    axis: SplitterAxis,
    thickness: f32,
    color: u32,
}
impl SplitterHandleConfig {
    /// Reject nonfinite/nonpositive thickness and grips wider than 64 logical pixels.
    pub fn new(axis: SplitterAxis, thickness: f32, color: u32) -> Option<Self> {
        (thickness.is_finite() && thickness > 0.0 && thickness <= 64.0).then_some(Self {
            axis,
            thickness,
            color,
        })
    }
    pub const fn thin(axis: SplitterAxis, color: u32) -> Self {
        Self {
            axis,
            thickness: 4.0,
            color,
        }
    }
}

/// Pointer origin retained inside the host's baseline snapshot, rather than in
/// a second widget/entity state owner. A gesture belongs to exactly one window.
#[derive(Clone, Copy, Debug)]
pub struct SplitterGesture {
    axis: SplitterAxis,
    origin: Point<Pixels>,
    window: WindowId,
}
impl SplitterGesture {
    pub fn start(axis: SplitterAxis, origin: Point<Pixels>, window: &Window) -> Option<Self> {
        finite(origin).then_some(Self {
            axis,
            origin,
            window: window.window_handle().window_id(),
        })
    }
    pub fn matches_window(self, window: &Window) -> bool {
        self.window == window.window_handle().window_id()
    }
    fn displacement(self, position: Point<Pixels>) -> Option<f32> {
        if !finite(position) {
            return None;
        }
        let delta = match self.axis {
            SplitterAxis::Horizontal => f32::from(position.x) - f32::from(self.origin.x),
            SplitterAxis::Vertical => f32::from(position.y) - f32::from(self.origin.y),
        };
        delta.is_finite().then_some(delta)
    }
    fn event(self, position: Point<Pixels>, window: &Window, commit: bool) -> SplitterEvent {
        if !self.matches_window(window) {
            return SplitterEvent::Cancel(SplitterCancel::WindowChanged);
        }
        match self.displacement(position) {
            Some(delta) if commit => SplitterEvent::Commit(delta),
            Some(delta) => SplitterEvent::Change(delta),
            None => SplitterEvent::Cancel(SplitterCancel::InvalidPosition),
        }
    }
}
fn finite(point: Point<Pixels>) -> bool {
    f32::from(point.x).is_finite() && f32::from(point.y).is_finite()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitterCancel {
    ButtonLost,
    Escape,
    InvalidPosition,
    WindowChanged,
}

/// Change/Commit carry displacement from the original press, never incremental
/// deltas. Commit uses the actual release coordinate, including outside release.
#[derive(Clone, Copy, Debug)]
pub enum SplitterEvent {
    Begin(SplitterGesture),
    Change(f32),
    Commit(f32),
    Cancel(SplitterCancel),
}

/// The host reports its current gesture and consumes an intent exactly once.
/// Returning true means input was handled; only then does this binding stop
/// propagation and refresh. Disabled/refused Begin must leave no host gesture.
pub struct SplitterInput<V: 'static> {
    owner: Entity<V>,
    gesture: fn(&V) -> Option<SplitterGesture>,
    event: fn(&mut V, SplitterEvent, &mut Window, &mut Context<V>) -> bool,
}
impl<V: 'static> Clone for SplitterInput<V> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            gesture: self.gesture,
            event: self.event,
        }
    }
}
impl<V: 'static> SplitterInput<V> {
    pub fn new(
        owner: Entity<V>,
        gesture: fn(&V) -> Option<SplitterGesture>,
        event: fn(&mut V, SplitterEvent, &mut Window, &mut Context<V>) -> bool,
    ) -> Self {
        Self {
            owner,
            gesture,
            event,
        }
    }
    fn send(&self, event: SplitterEvent, window: &mut Window, cx: &mut App) {
        let handled = self
            .owner
            .update(cx, |owner, cx| (self.event)(owner, event, window, cx));
        if handled {
            cx.stop_propagation();
            window.refresh();
        }
    }
    fn current(&self, cx: &App) -> Option<SplitterGesture> {
        (self.gesture)(self.owner.read(cx))
    }
    /// Optional local Escape route, installed once on the focused ancestor with
    /// `capture_key_down`. Hosts with a broader cancel action dispatch the same
    /// `Cancel` intent there, retaining other gesture owners' cancellation order.
    /// A window mouse listener is global; key listeners follow the focus tree,
    /// so the mouse-only overlay must not own this cancellation route.
    pub fn escape_listener(&self) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + use<V> {
        let keys = self.clone();
        move |event, window, cx| {
            if event.keystroke.key == "escape"
                && event.keystroke.modifiers == Modifiers::default()
                && keys.current(cx).is_some()
            {
                keys.send(SplitterEvent::Cancel(SplitterCancel::Escape), window, cx);
            }
        }
    }
    pub fn handle(&self, id: SharedString, config: SplitterHandleConfig) -> Stateful<Div> {
        let selector = id.to_string();
        let binding = self.clone();
        let handle = div()
            .id(id)
            .debug_selector(move || selector.clone())
            .flex_shrink_0()
            .bg(rgb(config.color));
        let handle = match config.axis {
            SplitterAxis::Horizontal => handle
                .w(px(config.thickness))
                .h_full()
                .cursor(gpui::CursorStyle::ResizeLeftRight),
            SplitterAxis::Vertical => handle
                .h(px(config.thickness))
                .w_full()
                .cursor(gpui::CursorStyle::ResizeUpDown),
        };
        handle.on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                if let Some(gesture) = SplitterGesture::start(config.axis, event.position, window) {
                    binding.send(SplitterEvent::Begin(gesture), window, cx);
                }
            },
        )
    }
    /// Mount once per consumer in its owning window, including while a drag
    /// temporarily collapses/hides its handle. Host context reconciliation must
    /// cancel the snapshot on focus/source invalidation or consumer replacement.
    pub fn pointer_layer(&self) -> impl IntoElement + use<V> {
        let binding = self.clone();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = binding.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() {
                        return;
                    }
                    let Some(gesture) = moved.current(cx) else {
                        return;
                    };
                    let intent = if event.pressed_button != Some(MouseButton::Left) {
                        SplitterEvent::Cancel(SplitterCancel::ButtonLost)
                    } else {
                        gesture.event(event.position, window, false)
                    };
                    moved.send(intent, window, cx);
                });
                let released = binding.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.capture() || event.button != MouseButton::Left {
                        return;
                    }
                    if let Some(gesture) = released.current(cx) {
                        released.send(gesture.event(event.position, window, true), window, cx);
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

#[cfg(test)]
#[path = "splitter_tests.rs"]
mod tests;
