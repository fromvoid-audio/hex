//! Bounded normalized bar editor. Domain values and paint interpolation belong
//! to the owner. Clear events identify the inclusive, bounded crossed columns;
//! owners set those values to canonical zero in the same gesture transaction.
//! A held stroke owns the shared Window pointer lease; cancellation emits its
//! existing typed intent, independently of canonical acknowledgement/history.
use crate::ControlStyle;
use gpui::{
    App, Bounds, ContentMask, Context, CursorStyle, Element, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, LayoutId, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, SharedString, Style, Subscription,
    Window, canvas, div, point, prelude::*, px, rgb,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarEditorPhase {
    Begin,
    Change,
    Commit,
    Cancel,
}
#[derive(Clone, Copy, Debug)]
pub enum BarEditorMode {
    Paint,
    Clear,
}
#[derive(Clone, Copy, Debug)]
pub struct BarEditorEvent {
    pub phase: BarEditorPhase,
    pub position: [f32; 2],
    pub modifiers: Modifiers,
    pub mode: BarEditorMode,
    /// Inclusive crossed column indices, each strictly below the visible count.
    pub columns: [usize; 2],
}
#[derive(Clone, Copy)]
struct Stroke {
    modifiers: Modifiers,
    button: MouseButton,
}
/// Fixed 64-column storage; the owner chooses a visible prefix.
pub struct BarEditor {
    id: SharedString,
    values: [f32; 64],
    count: usize,
    style: ControlStyle,
    bounds: Option<Bounds<Pixels>>,
    drag: Option<Stroke>,
    enabled: bool,
    last: [f32; 2],
    selected: usize,
    focus: FocusHandle,
    blur: Option<Subscription>,
}
impl BarEditor {
    pub fn new(cx: &mut Context<Self>, id: impl Into<SharedString>, style: ControlStyle) -> Self {
        Self {
            id: id.into(),
            values: [0.0; 64],
            count: 64,
            style,
            bounds: None,
            drag: None,
            enabled: true,
            last: [0.0; 2],
            selected: 0,
            focus: cx.focus_handle().tab_index(0).tab_stop(true),
            blur: None,
        }
    }
    /// Disabling cancels an active preview before refusing further input.
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled != enabled {
            if !enabled {
                self.cancel(cx);
            }
            self.enabled = enabled;
            cx.notify();
        }
    }
    pub fn set_values(
        &mut self,
        values: [f32; 64],
        count: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        if count == 0
            || count > 64
            || values
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("invalid bar values");
        }
        if self.values != values || self.count != count {
            self.values = values;
            self.count = count;
            self.selected = self.selected.min(count - 1);
            cx.notify();
        }
        Ok(())
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(stroke) = self.drag.take() {
            cx.emit(BarEditorEvent {
                phase: BarEditorPhase::Cancel,
                position: self.last,
                modifiers: stroke.modifiers,
                mode: stroke.mode(),
                columns: [self.selected; 2],
            });
            cx.notify();
        }
    }
    fn position(&self, point: Point<Pixels>) -> Option<[f32; 2]> {
        let b = self.bounds?;
        if b.size.width <= px(0.0) || b.size.height <= px(0.0) {
            return None;
        }
        let position = [
            (point.x - b.origin.x) / b.size.width,
            1.0 - (point.y - b.origin.y) / b.size.height,
        ];
        position
            .iter()
            .all(|v| v.is_finite())
            .then(|| position.map(|v| v.clamp(0.0, 1.0)))
    }
    fn emit(
        &mut self,
        phase: BarEditorPhase,
        position: [f32; 2],
        modifiers: Modifiers,
        mode: BarEditorMode,
        cx: &mut Context<Self>,
    ) {
        let selected = ((position[0] * self.count as f32) as usize).min(self.count - 1);
        let columns = if matches!(phase, BarEditorPhase::Begin) {
            [selected; 2]
        } else {
            [self.selected.min(selected), self.selected.max(selected)]
        };
        let position = if matches!(mode, BarEditorMode::Clear) {
            [position[0], 0.0]
        } else {
            position
        };
        self.last = position;
        self.selected = selected;
        cx.emit(BarEditorEvent {
            phase,
            position,
            modifiers,
            mode,
            columns,
        });
        cx.notify();
    }
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled
            || self.drag.is_some()
            || !matches!(event.button, MouseButton::Left | MouseButton::Right)
        {
            return;
        }
        if let Some(position) = self.position(event.position) {
            let owner = cx.entity().downgrade();
            let cancelled = owner.clone();
            if !window.capture_pointer(
                cx.entity_id(),
                event.button,
                CursorStyle::Arrow,
                move |event, _, cx| {
                    let _ = owner.update(cx, |editor, cx| {
                        if let Some(event) = event.downcast_ref::<MouseMoveEvent>() {
                            editor.pointer_move(event, cx);
                        } else if let Some(event) = event.downcast_ref::<MouseUpEvent>() {
                            editor.pointer_up(event, cx);
                        }
                    });
                },
                move |_, cx| {
                    let _ = cancelled.update(cx, |editor, cx| editor.cancel(cx));
                },
                cx,
            ) {
                return;
            }
            let stroke = Stroke {
                modifiers: event.modifiers,
                button: event.button,
            };
            self.drag = Some(stroke);
            self.focus.focus(window);
            self.emit(
                BarEditorPhase::Begin,
                position,
                event.modifiers,
                stroke.mode(),
                cx,
            );
            cx.stop_propagation();
        }
    }
    fn pointer_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        let Some(stroke) = self.drag else {
            return false;
        };
        if event.pressed_button != Some(stroke.button) {
            self.cancel(cx);
            return true;
        }
        if let Some(position) = self.position(event.position) {
            self.emit(
                BarEditorPhase::Change,
                position,
                stroke.modifiers,
                stroke.mode(),
                cx,
            );
        }
        true
    }
    fn pointer_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) -> bool {
        let Some(stroke) = self.drag else {
            return false;
        };
        if event.button != stroke.button {
            return false;
        }
        self.drag = None;
        self.emit(
            BarEditorPhase::Commit,
            self.position(event.position).unwrap_or(self.last),
            stroke.modifiers,
            stroke.mode(),
            cx,
        );
        true
    }
}
impl Stroke {
    fn mode(self) -> BarEditorMode {
        if self.button == MouseButton::Right {
            BarEditorMode::Clear
        } else {
            BarEditorMode::Paint
        }
    }
}
impl EventEmitter<BarEditorEvent> for BarEditor {}
impl Focusable for BarEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl gpui::Render for BarEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.blur.is_none() {
            let focus = self.focus.clone();
            self.blur = Some(cx.on_blur(&focus, window, |editor, _, cx| editor.cancel(cx)));
        }
        let entity = cx.entity();
        let values = self.values;
        let count = self.count;
        let style = self.style;
        let selected = self.selected;
        let focused = self.focus.is_focused(window);
        div()
            .id(self.id.clone())
            .debug_selector(|| self.id.to_string())
            .relative()
            .w_full()
            .h_full()
            .track_focus(&self.focus)
            .when(!self.enabled, |element| element.opacity(0.5))
            .on_key_down(cx.listener(|editor, event: &gpui::KeyDownEvent, _, cx| {
                if !editor.enabled {
                    return;
                }
                if editor.drag.is_some() && event.keystroke.key != "escape" {
                    cx.stop_propagation();
                    return;
                }
                match event.keystroke.key.as_str() {
                    "escape" => editor.cancel(cx),
                    "left" => editor.selected = editor.selected.saturating_sub(1),
                    "right" => editor.selected = (editor.selected + 1).min(editor.count - 1),
                    "up" | "down" => {
                        let y = (editor.values[editor.selected]
                            + if event.keystroke.key == "up" {
                                0.01
                            } else {
                                -0.01
                            })
                        .clamp(0.0, 1.0);
                        let p = [(editor.selected as f32 + 0.5) / editor.count as f32, y];
                        editor.emit(
                            BarEditorPhase::Begin,
                            p,
                            event.keystroke.modifiers,
                            BarEditorMode::Paint,
                            cx,
                        );
                        editor.emit(
                            BarEditorPhase::Commit,
                            p,
                            event.keystroke.modifiers,
                            BarEditorMode::Paint,
                            cx,
                        );
                    }
                    _ => return,
                }
                cx.notify();
                cx.stop_propagation();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, app| {
                        entity.update(app, |editor, _| editor.bounds = Some(bounds));
                        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
                            return;
                        }
                        let width = bounds.size.width / count as f32;
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            // Keep every visible harmonic legible even in an empty bank.
                            // One-pixel guides sit behind the filled amplitude strokes.
                            let mut guides = PathBuilder::stroke(px(1.0));
                            for index in 1..count {
                                let x = bounds.origin.x + width * index as f32;
                                guides.move_to(point(x, bounds.origin.y));
                                guides.line_to(point(x, bounds.origin.y + bounds.size.height));
                            }
                            if let Ok(guides) = guides.build() {
                                let mut guide_color = rgb(style.muted);
                                guide_color.a = 0.35;
                                window.paint_path(guides, guide_color);
                            }
                            for (index, value) in values[..count].iter().enumerate() {
                                let x = bounds.origin.x + width * (index as f32 + 0.5);
                                let mut path = PathBuilder::stroke((width - px(1.0)).max(px(1.0)));
                                path.move_to(point(x, bounds.origin.y + bounds.size.height));
                                path.line_to(point(
                                    x,
                                    bounds.origin.y + bounds.size.height * (1.0 - value.max(0.01)),
                                ));
                                if let Ok(path) = path.build() {
                                    window.paint_path(
                                        path,
                                        rgb(if focused && index == selected {
                                            style.focus_marker
                                        } else {
                                            style.accent
                                        }),
                                    );
                                }
                            }
                        });
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .child(BarEditorCaptureElement {
                entity: cx.entity(),
            })
    }
}
struct BarEditorCaptureElement {
    entity: Entity<BarEditor>,
}

impl IntoElement for BarEditorCaptureElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for BarEditorCaptureElement {
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
        if self.entity.read(cx).drag.is_some() {
            window.renew_pointer_capture(self.entity.entity_id(), CursorStyle::Arrow);
        }
    }
}

#[cfg(test)]
#[path = "bar_editor_tests.rs"]
mod tests;
