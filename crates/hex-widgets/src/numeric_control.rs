use crate::dial::{DialLayout, DialValue, paint_dial};
use crate::focus_marker::corner_markers;
use crate::numeric_text::NumericTextElement;
use crate::{ControlStyle, NumericError, NumericSpec};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ShapedLine,
    SharedString, Style, Window, actions, canvas, div, prelude::*, px, relative, rgb,
};
use std::ops::Range;

actions!(
    hex_widgets_numeric,
    [
        NumericBackspace,
        NumericDelete,
        NumericLeft,
        NumericRight,
        NumericSelectLeft,
        NumericSelectRight,
        NumericSelectAll,
        NumericHome,
        NumericEnd,
        NumericCopy,
        NumericCut,
        NumericPaste,
        NumericEnter,
        NumericEscape,
        NumericUp,
        NumericDown,
        NumericUpFine,
        NumericDownFine,
    ]
);

/// Install focused numeric-edit and adjustment shortcuts in the caller's app.
pub fn bind_numeric_control_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", NumericBackspace, Some("hex_numeric_control")),
        KeyBinding::new("delete", NumericDelete, Some("hex_numeric_control")),
        KeyBinding::new("left", NumericLeft, Some("hex_numeric_control")),
        KeyBinding::new("right", NumericRight, Some("hex_numeric_control")),
        KeyBinding::new("shift-left", NumericSelectLeft, Some("hex_numeric_control")),
        KeyBinding::new(
            "shift-right",
            NumericSelectRight,
            Some("hex_numeric_control"),
        ),
        KeyBinding::new("cmd-a", NumericSelectAll, Some("hex_numeric_control")),
        KeyBinding::new("cmd-c", NumericCopy, Some("hex_numeric_control")),
        KeyBinding::new("cmd-x", NumericCut, Some("hex_numeric_control")),
        KeyBinding::new("cmd-v", NumericPaste, Some("hex_numeric_control")),
        KeyBinding::new("home", NumericHome, Some("hex_numeric_control")),
        KeyBinding::new("end", NumericEnd, Some("hex_numeric_control")),
        KeyBinding::new("enter", NumericEnter, Some("hex_numeric_control")),
        KeyBinding::new("escape", NumericEscape, Some("hex_numeric_control")),
        KeyBinding::new("up", NumericUp, Some("hex_numeric_control")),
        KeyBinding::new("down", NumericDown, Some("hex_numeric_control")),
        KeyBinding::new("shift-up", NumericUpFine, Some("hex_numeric_control")),
        KeyBinding::new("shift-down", NumericDownFine, Some("hex_numeric_control")),
    ]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericControlKind {
    Knob,
    HorizontalSlider,
    VerticalSlider,
}

/// Presentation-only geometry that keeps the same shared edit entity and events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NumericControlAppearance {
    #[default]
    Standard,
    CompactDial,
    MiniDial,
    MicroDial,
    InlineValue,
    DeviceMeters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericEventPhase {
    Begin,
    Change,
    Commit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericControlEvent {
    pub phase: NumericEventPhase,
    pub value: f64,
}

/// Final value returned to an owner performing a lifecycle barrier. It lets
/// the owner synchronously submit the same edit before GPUI flushes emitters.
#[derive(Debug, Clone, PartialEq)]
pub struct NumericControlFinalValue {
    pub previous: f64,
    pub value: f64,
    formatted: String,
}

/// Fully checked construction data for use inside an infallible GPUI entity
/// factory. Invalid parameter metadata is rejected before creating the entity.
pub struct NumericControlConfig {
    id: SharedString,
    label: SharedString,
    kind: NumericControlKind,
    spec: NumericSpec,
    value: f64,
    text: String,
    style: ControlStyle,
    appearance: NumericControlAppearance,
    dial_layout: DialLayout,
}

impl NumericControlConfig {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        kind: NumericControlKind,
        spec: NumericSpec,
        value: f64,
        style: ControlStyle,
    ) -> Result<Self, NumericError> {
        spec.validate(value)?;
        style.validate().map_err(|_| NumericError::InvalidStyle)?;
        let text = spec.format(value)?;
        Ok(Self {
            id: id.into(),
            label: label.into(),
            kind,
            spec,
            value,
            text,
            style,
            appearance: NumericControlAppearance::Standard,
            dial_layout: DialLayout::ReadoutGap,
        })
    }

    /// Select rail geometry from caller-owned control semantics, never density.
    pub fn with_dial_layout(mut self, layout: DialLayout) -> Self {
        self.dial_layout = layout;
        self
    }

    pub fn with_appearance(mut self, appearance: NumericControlAppearance) -> Self {
        self.appearance = appearance;
        self
    }
}

#[derive(Clone, Copy)]
enum PressTarget {
    Control,
    Value,
}

#[derive(Clone, Copy)]
struct Press {
    origin: Point<Pixels>,
    last_position: Point<Pixels>,
    start_value: f64,
    target: PressTarget,
    dragging: bool,
}

/// One reusable knob/slider and its independently editable numeric value.
pub struct NumericControl {
    id: SharedString,
    label: SharedString,
    kind: NumericControlKind,
    pub(crate) spec: NumericSpec,
    pub(crate) value: f64,
    pub(crate) draft: f64,
    enabled: bool,
    pub(crate) editing: bool,
    pub(crate) invalid_text: bool,
    pub(crate) text: String,
    pub(crate) selected: Range<usize>,
    pub(crate) reversed: bool,
    pub(crate) marked: Option<Range<usize>>,
    pub(crate) last_layout: Option<ShapedLine>,
    pub(crate) last_bounds: Option<Bounds<Pixels>>,
    pub(crate) last_origin_x: Option<Pixels>,
    press: Option<Press>,
    pub(crate) style: ControlStyle,
    meter_readout: crate::DeviceMeterReadout,
    appearance: NumericControlAppearance,
    dial_layout: DialLayout,
    tab_navigation: bool,
    pub(crate) focus: FocusHandle,
    focus_blur: Option<gpui::Subscription>,
    window_activation: Option<gpui::Subscription>,
}

impl NumericControl {
    pub fn new(
        cx: &mut Context<Self>,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        kind: NumericControlKind,
        spec: NumericSpec,
        value: f64,
        style: ControlStyle,
    ) -> Result<Self, NumericError> {
        Ok(Self::from_config(
            cx,
            NumericControlConfig::new(id, label, kind, spec, value, style)?,
        ))
    }

    pub fn from_config(cx: &mut Context<Self>, config: NumericControlConfig) -> Self {
        let NumericControlConfig {
            id,
            label,
            kind,
            spec,
            value,
            text,
            style,
            appearance,
            dial_layout,
        } = config;
        Self {
            id,
            label,
            kind,
            spec,
            value,
            draft: value,
            enabled: true,
            editing: false,
            invalid_text: false,
            text,
            selected: 0..0,
            reversed: false,
            marked: None,
            last_layout: None,
            last_bounds: None,
            last_origin_x: None,
            press: None,
            style,
            appearance,
            dial_layout,
            meter_readout: Default::default(),
            tab_navigation: false,
            focus: cx.focus_handle(),
            focus_blur: None,
            window_activation: None,
        }
    }

    pub fn set_meter_readout(
        &mut self,
        readout: crate::DeviceMeterReadout,
        cx: &mut Context<Self>,
    ) {
        if self.meter_readout != readout {
            self.meter_readout = readout;
            cx.notify();
        }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// Text currently shown in the editable value field.
    pub fn display_text(&self) -> &str {
        &self.text
    }

    /// Update caller-owned styling without replacing this control or its draft.
    pub fn set_external_style(
        &mut self,
        style: ControlStyle,
        cx: &mut Context<Self>,
    ) -> Result<(), NumericError> {
        let style = style.validate().map_err(|_| NumericError::InvalidStyle)?;
        if self.style != style {
            self.style = style;
            cx.notify();
        }
        Ok(())
    }

    pub fn is_interacting(&self) -> bool {
        self.press.is_some() || self.editing
    }

    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// Synchronize from caller state without disrupting an active local draft.
    pub fn set_external_value(
        &mut self,
        value: f64,
        cx: &mut Context<Self>,
    ) -> Result<(), NumericError> {
        self.spec.validate(value)?;
        if !self.is_interacting() && self.value != value {
            self.value = value;
            self.draft = value;
            self.text = self.spec.format(value)?;
            self.invalid_text = false;
            cx.notify();
        }
        Ok(())
    }

    /// Opt into rendered-order Tab traversal; unavailable controls are skipped.
    /// Owners choose participation without changing pointer or edit semantics.
    pub fn set_tab_navigation(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.tab_navigation = enabled;
        let tab_stop = self.tab_navigation && self.enabled;
        if self.focus.tab_stop != tab_stop {
            self.focus = self.focus.clone().tab_index(0).tab_stop(tab_stop);
            cx.notify();
        }
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        let tab_stop = self.tab_navigation && self.enabled;
        if self.focus.tab_stop != tab_stop {
            self.focus = self.focus.clone().tab_index(0).tab_stop(tab_stop);
        }
        if !enabled {
            self.cancel(cx);
        }
        cx.notify();
    }

    /// Call before removing an active control so its caller can cancel the edit.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(press) = self.press.take() {
            if press.dragging {
                self.value = press.start_value;
                self.draft = press.start_value;
                self.text = self.spec.format(self.draft).unwrap_or_default();
                cx.emit(NumericControlEvent {
                    phase: NumericEventPhase::Cancel,
                    value: self.draft,
                });
            }
        }
        if self.editing {
            self.editing = false;
            self.invalid_text = false;
            self.text = self.spec.format(self.value).unwrap_or_default();
            self.selected = 0..0;
            self.marked = None;
        }
        cx.notify();
    }

    fn begin_press(&mut self, target: PressTarget, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if !self.enabled || event.button != MouseButton::Left {
            return;
        }
        if (matches!(self.kind, NumericControlKind::Knob)
            || self.appearance == NumericControlAppearance::DeviceMeters)
            && matches!(target, PressTarget::Control)
            && event.click_count >= 2
        {
            self.reset(cx);
            return;
        }
        self.press = Some(Press {
            origin: event.position,
            last_position: event.position,
            start_value: self.draft,
            target,
            dragging: false,
        });
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        let start = self.draft;
        let value = self.spec.default();
        // Reset finishes the local edit even when the value is already at its
        // default. A pending invalid text/IME draft must not retain focus-mode
        // or pointer capture and block later keyboard/external updates.
        self.press = None;
        self.editing = false;
        self.invalid_text = false;
        self.selected = 0..0;
        self.reversed = false;
        self.marked = None;
        self.text = self.spec.format(value).unwrap_or_default();
        if start != value {
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Begin,
                value: start,
            });
            self.value = value;
            self.draft = value;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Change,
                value,
            });
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value,
            });
        }
        cx.notify();
    }

    fn dial_element(
        &self,
        size: f32,
        value: DialValue,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let style = self.style;
        let enabled = self.enabled;
        let id = self.id.clone();
        let marker_id = self.id.clone();
        div()
            .id(SharedString::from(format!("{}-knob", self.id)))
            .debug_selector(move || format!("{id}-knob"))
            .relative()
            .size(px(size))
            .on_mouse_down(MouseButton::Left, self.control_mouse_down_listener(cx))
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        paint_dial(bounds, size, value, enabled, style, window);
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .when(self.dial_layout == DialLayout::Pan, |dial| {
                dial.child(
                    div()
                        .id(SharedString::from(format!("{}-pan-reset", self.id)))
                        .debug_selector(move || format!("{marker_id}-pan-reset"))
                        .absolute()
                        .top(px(0.0))
                        .left(px(size / 2.0 - 3.0))
                        .w(px(6.0))
                        .h(px(4.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|control, _, window, cx| {
                                cx.stop_propagation();
                                if control.enabled {
                                    control.focus.focus(window);
                                    control.reset(cx);
                                }
                            }),
                        ),
                )
            })
    }

    fn value_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_press(PressTarget::Value, event, cx);
        self.focus.focus(window);
    }

    fn control_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_press(PressTarget::Control, event, cx);
        self.focus.focus(window);
    }

    fn pointer_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) -> (bool, bool) {
        let Some(mut press) = self.press else {
            return (false, false);
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.press = None;
            if press.dragging {
                self.value = press.start_value;
                self.draft = press.start_value;
                self.text = self.spec.format(self.draft).unwrap_or_default();
                cx.emit(NumericControlEvent {
                    phase: NumericEventPhase::Cancel,
                    value: self.draft,
                });
                cx.notify();
                return (true, true);
            }
            return (true, false);
        }
        let total_x = f32::from(event.position.x - press.origin.x);
        let total_y = f32::from(event.position.y - press.origin.y);
        if !press.dragging && total_x.hypot(total_y) >= 3.0 {
            press.dragging = true;
            // Cursor visibility changes even at a value bound or on horizontal motion.
            cx.notify();
            self.editing = false;
            self.invalid_text = false;
            self.text = self.spec.format(press.start_value).unwrap_or_default();
            self.selected = 0..0;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Begin,
                value: press.start_value,
            });
        }
        if !press.dragging {
            return (true, false);
        }
        let delta_x = f32::from(event.position.x - press.last_position.x);
        let delta_y = f32::from(event.position.y - press.last_position.y);
        press.last_position = event.position;
        self.press = Some(press);
        let shift = event.modifiers.shift;
        let primary_delta = match (press.target, self.kind) {
            (PressTarget::Control, NumericControlKind::HorizontalSlider) => delta_x,
            // Rectangular readouts always scrub vertically, independent of the
            // track/dial presentation their owner uses.
            _ => -delta_y,
        };
        if primary_delta == 0.0 {
            return (true, false);
        }
        let sensitivity = if shift { 1200.0 } else { 120.0 };
        let next = match self
            .spec
            .adjust(self.draft, f64::from(primary_delta) / sensitivity)
        {
            Ok(value) => value,
            Err(_) => return (true, false),
        };
        if next != self.draft {
            self.draft = next;
            self.value = next;
            self.text = self.spec.format(next).unwrap_or_default();
            self.invalid_text = false;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Change,
                value: next,
            });
            cx.notify();
            (true, true)
        } else {
            (true, false)
        }
    }

    fn pointer_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != MouseButton::Left {
            return false;
        }
        let Some(press) = self.press.take() else {
            return false;
        };
        if press.dragging {
            self.value = self.draft;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value: self.draft,
            });
        } else if matches!(press.target, PressTarget::Value)
            && f32::from(event.position.x - press.origin.x)
                .hypot(f32::from(event.position.y - press.origin.y))
                < 3.0
        {
            self.editing = true;
            self.invalid_text = false;
            self.text = self.spec.format(self.draft).unwrap_or_default();
            self.selected = 0..self.text.len();
            self.reversed = false;
            self.focus.focus(window);
        } else {
            self.focus.focus(window);
        }
        cx.notify();
        true
    }

    fn emit_step(&mut self, direction: f64, fine: bool, cx: &mut Context<Self>) {
        if !self.enabled || self.editing {
            return;
        }
        if let Ok(value) = self.spec.step(self.draft, direction, fine)
            && value != self.draft
        {
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Begin,
                value: self.draft,
            });
            self.draft = value;
            self.value = value;
            self.text = self.spec.format(value).unwrap_or_default();
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Change,
                value,
            });
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value,
            });
            cx.notify();
        }
    }

    fn enter(&mut self, _: &NumericEnter, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if !self.editing {
            return;
        }
        match self.spec.parse(&self.text) {
            Ok(value) => {
                let old = self.value;
                if old != value {
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Begin,
                        value: old,
                    });
                    self.value = value;
                    self.draft = value;
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Change,
                        value,
                    });
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Commit,
                        value,
                    });
                }
                self.text = self.spec.format(value).unwrap_or_default();
                self.editing = false;
                self.invalid_text = false;
                self.marked = None;
                cx.notify();
            }
            Err(_) => {
                self.invalid_text = true;
                cx.notify();
            }
        }
    }

    /// Commit a typed value or finish an active pointer gesture before its
    /// owner hides or removes the control. Invalid text remains visible and
    /// editing so the owner can refuse that lifecycle change.
    pub fn finish_edit(&mut self, cx: &mut Context<Self>) -> Result<(), NumericError> {
        if let Some(press) = self.press.take() {
            if press.dragging {
                cx.emit(NumericControlEvent {
                    phase: NumericEventPhase::Commit,
                    value: self.draft,
                });
            }
            cx.notify();
        }
        if !self.editing {
            return Ok(());
        }
        let value = match self.spec.parse(&self.text) {
            Ok(value) => value,
            Err(error) => {
                self.invalid_text = true;
                cx.notify();
                return Err(error);
            }
        };
        let old = self.value;
        if old != value {
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Begin,
                value: old,
            });
            self.value = value;
            self.draft = value;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Change,
                value,
            });
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value,
            });
        }
        self.text = self.spec.format(value)?;
        self.editing = false;
        self.invalid_text = false;
        self.selected = 0..0;
        self.marked = None;
        cx.notify();
        Ok(())
    }

    /// Validate and read a local draft without clearing it or emitting events.
    /// Owners use this phase to admit the canonical edit before finalizing UI
    /// state; a refused edit therefore remains available for correction/retry.
    pub fn prepare_finish_edit_for_owner(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<Option<NumericControlFinalValue>, NumericError> {
        if let Some(press) = self.press {
            if press.dragging {
                return Ok(Some(NumericControlFinalValue {
                    previous: press.start_value,
                    value: self.draft,
                    formatted: self.spec.format(self.draft)?,
                }));
            }
            return Ok(Some(NumericControlFinalValue {
                previous: self.value,
                value: self.value,
                formatted: self.spec.format(self.value)?,
            }));
        }
        if !self.editing {
            return Ok(None);
        }
        let value = match self.spec.parse(&self.text) {
            Ok(value) => value,
            Err(error) => {
                self.invalid_text = true;
                cx.notify();
                return Err(error);
            }
        };
        let previous = self.value;
        let formatted = self.spec.format(value)?;
        Ok(Some(NumericControlFinalValue {
            previous,
            value,
            formatted,
        }))
    }

    /// Complete a previously prepared draft after its owner admitted the
    /// canonical change. This method emits no subscription events.
    pub fn accept_finish_edit_for_owner(
        &mut self,
        finalized: NumericControlFinalValue,
        cx: &mut Context<Self>,
    ) {
        if let Some(press) = self.press.take() {
            if press.dragging {
                self.value = finalized.value;
                self.draft = finalized.value;
                self.text = finalized.formatted;
            }
            self.editing = false;
            self.invalid_text = false;
            self.selected = 0..0;
            self.marked = None;
            cx.notify();
            return;
        }
        if !self.editing {
            return;
        }
        self.value = finalized.value;
        self.draft = finalized.value;
        self.text = finalized.formatted;
        self.editing = false;
        self.invalid_text = false;
        self.selected = 0..0;
        self.marked = None;
        cx.notify();
    }

    pub(crate) fn finish_text_edit_on_blur(&mut self, cx: &mut Context<Self>) {
        let _ = self.finish_edit(cx);
    }

    fn escape(&mut self, _: &NumericEscape, _: &mut Window, cx: &mut Context<Self>) {
        if self.press.is_some() {
            self.cancel(cx);
        } else if self.editing {
            self.editing = false;
            self.invalid_text = false;
            self.text = self.spec.format(self.value).unwrap_or_default();
            self.selected = 0..0;
            self.marked = None;
            cx.notify();
        }
    }

    fn edit_left(&mut self, extend: bool, cx: &mut Context<Self>) {
        if self.editing {
            let cursor = self.cursor();
            let target = previous_boundary(&self.text, cursor);
            if extend {
                self.select_to(target);
            } else {
                self.move_to(target);
            }
            cx.notify();
        } else {
            self.emit_step(-1.0, false, cx);
        }
    }

    fn edit_right(&mut self, extend: bool, cx: &mut Context<Self>) {
        if self.editing {
            let cursor = self.cursor();
            let target = next_boundary(&self.text, cursor);
            if extend {
                self.select_to(target);
            } else {
                self.move_to(target);
            }
            cx.notify();
        } else {
            self.emit_step(1.0, false, cx);
        }
    }

    fn select_all(&mut self, _: &NumericSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing {
            self.selected = 0..self.text.len();
            self.reversed = false;
            cx.notify();
        }
    }

    fn home(&mut self, _: &NumericHome, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing {
            self.move_to(0);
            cx.notify();
        }
    }

    fn end(&mut self, _: &NumericEnd, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing {
            self.move_to(self.text.len());
            cx.notify();
        }
    }

    fn backspace(&mut self, _: &NumericBackspace, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing {
            self.delete_backward(window, cx);
        }
    }

    fn delete(&mut self, _: &NumericDelete, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing {
            self.delete_forward(window, cx);
        }
    }

    fn copy(&mut self, _: &NumericCopy, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing
            && let Some(text) = self.text.get(self.selected.clone())
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        }
    }

    fn cut(&mut self, _: &NumericCut, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing
            && let Some(text) = self.text.get(self.selected.clone())
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn paste(&mut self, _: &NumericPaste, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing
            && let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
        {
            let clean = text.replace(['\n', '\r'], " ");
            self.replace_text_in_range(None, &clean, window, cx);
        }
    }

    fn value_mouse_down_listener(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(Self::value_mouse_down)
    }

    fn control_mouse_down_listener(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(Self::control_mouse_down)
    }

    pub(crate) fn value_string(&self) -> String {
        if self.editing {
            self.text.clone()
        } else {
            self.spec.format(self.draft).unwrap_or_default()
        }
    }

    pub(crate) fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    pub(crate) fn move_to(&mut self, offset: usize) {
        let offset = offset.min(self.text.len());
        self.selected = offset..offset;
        self.reversed = false;
    }

    pub(crate) fn select_to(&mut self, offset: usize) {
        let offset = offset.min(self.text.len());
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
    }

    pub(crate) fn delete_backward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.selected = previous_boundary(&self.text, self.cursor())..self.cursor();
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    pub(crate) fn delete_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let cursor = self.cursor();
            self.selected = cursor..next_boundary(&self.text, cursor);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .char_indices()
        .last()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[offset..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(index, _)| offset + index)
}

impl EventEmitter<NumericControlEvent> for NumericControl {}

impl Focusable for NumericControl {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for NumericControl {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self
            .press
            .is_some_and(|press| matches!(press.target, PressTarget::Value))
            && !self.focus.is_focused(window)
        {
            self.cancel(cx);
        }
        // A text draft owns printable keys even beneath editor shortcut contexts.
        // Keep numeric editing actions available in the same focused node.
        let key_context = if self.editing {
            "hex_numeric_control hex_text_editing"
        } else {
            "hex_numeric_control"
        };
        let entity = cx.entity();
        let enabled = self.enabled;
        let style = self.style;
        let value = self.spec.to_normalized(self.draft).unwrap_or(0.0) as f32;
        let dial_value = DialValue::new(value, self.spec.scale(), self.dial_layout);
        let label = self.label.clone();
        let value_text = NumericTextElement {
            input: entity.clone(),
        };

        if self.focus_blur.is_none() {
            let focus = self.focus.clone();
            self.focus_blur = Some(cx.on_blur(&focus, window, |control, _, cx| {
                if control
                    .press
                    .is_some_and(|press| matches!(press.target, PressTarget::Value))
                {
                    control.cancel(cx);
                } else {
                    control.finish_text_edit_on_blur(cx);
                }
            }));
        }

        if self.window_activation.is_none() {
            self.window_activation =
                Some(cx.observe_window_activation(window, |control, window, cx| {
                    if !window.is_window_active()
                        && control
                            .press
                            .is_some_and(|press| matches!(press.target, PressTarget::Value))
                    {
                        control.cancel(cx);
                    }
                }));
        }

        let mut root = match (self.kind, self.appearance) {
            (_, NumericControlAppearance::DeviceMeters) => div()
                .id(self.id.clone())
                .w(px(23.0))
                .h(px(self.meter_readout.height))
                .flex()
                .gap(px(3.0))
                .px(px(2.0))
                .py(px(4.0))
                .track_focus(&self.focus)
                .key_context("hex_numeric_control")
                .child(crate::device_meters::well(
                    self.id.clone(),
                    0,
                    &self.meter_readout,
                    style,
                ))
                .child(
                    crate::device_meters::well(self.id.clone(), 1, &self.meter_readout, style)
                        .cursor(gpui::CursorStyle::ResizeUpDown)
                        .on_mouse_down(MouseButton::Left, self.control_mouse_down_listener(cx)),
                ),
            (
                NumericControlKind::Knob,
                NumericControlAppearance::MicroDial | NumericControlAppearance::MiniDial,
            ) => div()
                .id(self.id.clone())
                .relative()
                .w(px(crate::MICRO_DIAL_WIDTH))
                .h(px(crate::MICRO_DIAL_HEIGHT))
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(
                    div()
                        .absolute()
                        .debug_selector(|| format!("{}-label", self.id))
                        .top(px(0.0))
                        .left(px(2.0))
                        .w(px(crate::MICRO_DIAL_WIDTH - 4.0))
                        .h(px(9.0))
                        .text_size(px(8.0))
                        .text_color(rgb(if enabled { style.muted } else { style.border }))
                        .overflow_hidden()
                        .child(label),
                )
                .child(
                    self.dial_element(crate::MICRO_DIAL_DIAMETER, dial_value, cx)
                        .absolute()
                        .top(px(12.0))
                        .left(px(2.0)),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(25.0))
                        .left(px(13.0))
                        .child(value_field_sized(
                            self.id.clone(),
                            value_text,
                            ValueFieldPresentation {
                                invalid: self.invalid_text,
                                editing: self.editing,
                                enabled,
                                style: ControlStyle {
                                    font_size: 8.0,
                                    ..style
                                },
                                width: px(crate::MICRO_DIAL_WIDTH - 13.0),
                                height: px(12.0),
                            },
                            self.value_mouse_down_listener(cx),
                        )),
                ),
            (NumericControlKind::Knob, NumericControlAppearance::CompactDial) => {
                let (width, height, label_height, label_size, knob_size, value_height, gap) =
                    (64.0, 52.0, 10.0, 9.0, 26.0, 14.0, 1.0);
                div()
                    .id(self.id.clone())
                    .flex()
                    .flex_col()
                    .w(px(width))
                    .h(px(height))
                    .gap(px(gap))
                    .items_center()
                    .track_focus(&self.focus)
                    .key_context(key_context)
                    .child(
                        div()
                            .w(px(width))
                            .h(px(label_height))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(label_size))
                            .text_color(rgb(if enabled { style.muted } else { style.border }))
                            .overflow_hidden()
                            .child(label),
                    )
                    .child(self.dial_element(knob_size, dial_value, cx))
                    .child(value_field_sized(
                        self.id.clone(),
                        value_text,
                        ValueFieldPresentation {
                            invalid: self.invalid_text,
                            editing: self.editing,
                            enabled,
                            style,
                            width: px(width),
                            height: px(value_height),
                        },
                        self.value_mouse_down_listener(cx),
                    ))
            }
            (NumericControlKind::HorizontalSlider, NumericControlAppearance::InlineValue) => div()
                .id(self.id.clone())
                .flex()
                .w(px(46.0))
                .h(px(14.0))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(value_field_sized(
                    self.id.clone(),
                    value_text,
                    ValueFieldPresentation {
                        invalid: self.invalid_text,
                        editing: self.editing,
                        enabled,
                        style,
                        width: px(46.0),
                        height: px(14.0),
                    },
                    self.value_mouse_down_listener(cx),
                )),
            (NumericControlKind::Knob, _) => div()
                .id(self.id.clone())
                .flex()
                .flex_col()
                .w(px(48.0))
                .h(px(67.0))
                .gap(px(1.0))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(
                    div()
                        .w(px(48.0))
                        .h(px(14.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(style.font_size))
                        .text_color(rgb(if enabled { style.muted } else { style.border }))
                        .overflow_hidden()
                        .child(label),
                )
                .child(self.dial_element(34.0, dial_value, cx))
                .child(value_field(
                    self.id.clone(),
                    value_text,
                    self.invalid_text,
                    self.editing,
                    enabled,
                    style,
                    self.value_mouse_down_listener(cx),
                )),
            (NumericControlKind::HorizontalSlider, _) => div()
                .id(self.id.clone())
                .flex()
                .h(px(24.0))
                .min_w(px(96.0))
                .gap(px(5.0))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(
                    div()
                        .w(px(42.0))
                        .min_w(px(24.0))
                        .overflow_hidden()
                        .text_size(px(style.font_size))
                        .text_color(rgb(if enabled { style.muted } else { style.border }))
                        .child(label),
                )
                .child(slider_track(
                    self.id.clone(),
                    value,
                    false,
                    enabled,
                    style,
                    self.control_mouse_down_listener(cx),
                ))
                .child(value_field(
                    self.id.clone(),
                    value_text,
                    self.invalid_text,
                    self.editing,
                    enabled,
                    style,
                    self.value_mouse_down_listener(cx),
                )),
            (NumericControlKind::VerticalSlider, _) => div()
                .id(self.id.clone())
                .flex()
                .flex_col()
                .w(px(36.0))
                .h(px(86.0))
                .gap(px(2.0))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(
                    div()
                        .w_full()
                        .h(px(14.0))
                        .overflow_hidden()
                        .text_size(px(style.font_size))
                        .text_color(rgb(if enabled { style.muted } else { style.border }))
                        .text_center()
                        .child(label),
                )
                .child(slider_track(
                    self.id.clone(),
                    value,
                    true,
                    enabled,
                    style,
                    self.control_mouse_down_listener(cx),
                ))
                .child(value_field(
                    self.id.clone(),
                    value_text,
                    self.invalid_text,
                    self.editing,
                    enabled,
                    style,
                    self.value_mouse_down_listener(cx),
                )),
        };

        let selected =
            enabled && (self.focus.is_focused(window) || self.press.is_some() || self.editing);
        root = root
            .relative()
            .debug_selector(|| self.id.to_string())
            // Retain the existing one-pixel layout inset; only its paint changes.
            .border_1()
            .border_color(gpui::rgba(0))
            .when(selected, |root| {
                root.child(corner_markers(&self.id, style.focus_marker))
            })
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(|view, _: &NumericLeft, _, cx| {
                cx.stop_propagation();
                view.edit_left(false, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericRight, _, cx| {
                cx.stop_propagation();
                view.edit_right(false, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericSelectLeft, _, cx| {
                cx.stop_propagation();
                view.edit_left(true, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericSelectRight, _, cx| {
                cx.stop_propagation();
                view.edit_right(true, cx)
            }))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(|view, _: &NumericUp, _, cx| {
                cx.stop_propagation();
                view.emit_step(1.0, false, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericDown, _, cx| {
                cx.stop_propagation();
                view.emit_step(-1.0, false, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericUpFine, _, cx| {
                cx.stop_propagation();
                view.emit_step(1.0, true, cx)
            }))
            .on_action(cx.listener(|view, _: &NumericDownFine, _, cx| {
                cx.stop_propagation();
                view.emit_step(-1.0, true, cx)
            }));

        if self.press.is_some() {
            root.child(GestureCaptureElement {
                move_entity: entity.clone(),
                up_entity: entity,
            })
        } else {
            root
        }
    }
}

struct GestureCaptureElement {
    move_entity: Entity<NumericControl>,
    up_entity: Entity<NumericControl>,
}

impl IntoElement for GestureCaptureElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for GestureCaptureElement {
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
        // Capture installs window listeners only; it must never enter flex flow
        // or add a gap/shrink the dial while a pointer is held.
        let mut style = Style {
            position: gpui::Position::Absolute,
            ..Style::default()
        };
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
        if self
            .move_entity
            .read(cx)
            .press
            .is_some_and(|press| press.dragging && matches!(press.target, PressTarget::Value))
            && window.is_window_active()
        {
            window.set_window_cursor_style(CursorStyle::Hidden);
        }
        let move_entity = self.move_entity.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if !phase.capture() {
                return;
            }
            let (handled, changed) =
                move_entity.update(cx, |control, cx| control.pointer_move(event, cx));
            if handled {
                cx.stop_propagation();
            }
            if changed {
                window.refresh();
            }
        });

        let up_entity = self.up_entity.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if !phase.capture() {
                return;
            }
            let finished =
                up_entity.update(cx, |control, cx| control.pointer_up(event, window, cx));
            if finished {
                cx.stop_propagation();
                window.refresh();
            }
        });
    }
}

fn value_field(
    id: SharedString,
    child: NumericTextElement,
    invalid: bool,
    editing: bool,
    enabled: bool,
    style: ControlStyle,
    on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    value_field_sized(
        id,
        child,
        ValueFieldPresentation {
            invalid,
            editing,
            enabled,
            style,
            width: px(46.0),
            height: px(18.0),
        },
        on_down,
    )
}

#[derive(Clone, Copy)]
struct ValueFieldPresentation {
    invalid: bool,
    editing: bool,
    enabled: bool,
    style: ControlStyle,
    width: Pixels,
    height: Pixels,
}

fn value_field_sized(
    id: SharedString,
    child: NumericTextElement,
    presentation: ValueFieldPresentation,
    on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let ValueFieldPresentation {
        invalid,
        editing,
        enabled,
        style,
        width,
        height,
    } = presentation;
    div()
        .id(SharedString::from(format!("{}-value", id)))
        .debug_selector(|| format!("{}-value", id))
        .flex()
        .w(width)
        .h(height)
        .rounded(px(crate::CONTROL_CORNER_RADIUS))
        .px(px(2.0))
        .items_center()
        .justify_center()
        .border_1()
        .border_color(if invalid {
            rgb(0xff554c)
        } else {
            gpui::rgba(0)
        })
        .when(editing || invalid, |field| field.bg(rgb(style.background)))
        .text_size(px(style.font_size))
        .text_color(rgb(if enabled { style.text } else { style.muted }))
        .cursor(CursorStyle::Arrow)
        .on_mouse_down(MouseButton::Left, on_down)
        .child(child)
}

fn slider_track(
    id: SharedString,
    normalized: f32,
    vertical: bool,
    enabled: bool,
    style: ControlStyle,
    on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let fill_color = if enabled { style.accent } else { style.muted };
    let thumb = div()
        .absolute()
        .size(if vertical { px(12.0) } else { px(10.0) })
        .rounded(px(crate::CONTROL_CORNER_RADIUS))
        .border_1()
        .border_color(rgb(style.border))
        .bg(rgb(style.text));
    if vertical {
        div()
            .id(SharedString::from(format!("{}-slider", id)))
            .debug_selector(|| format!("{}-slider", id))
            .relative()
            .flex()
            .flex_1()
            .w(px(24.0))
            .min_h(px(32.0))
            .items_center()
            .justify_center()
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .on_mouse_down(MouseButton::Left, on_down)
            .child(
                div()
                    .absolute()
                    .top(relative(0.0))
                    .bottom(relative(0.0))
                    .w(px(3.0))
                    .rounded_full()
                    .bg(rgb(style.rail)),
            )
            .child(
                div()
                    .absolute()
                    .bottom(relative(normalized))
                    .w(px(3.0))
                    .h(relative(1.0))
                    .rounded_full()
                    .bg(rgb(fill_color)),
            )
            .child(thumb.left(relative(0.5)).bottom(relative(normalized)))
    } else {
        div()
            .id(SharedString::from(format!("{}-slider", id)))
            .debug_selector(|| format!("{}-slider", id))
            .relative()
            .flex()
            .flex_1()
            .min_w(px(36.0))
            .h(px(20.0))
            .items_center()
            .cursor(if enabled {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .on_mouse_down(MouseButton::Left, on_down)
            .child(
                div()
                    .absolute()
                    .left(relative(0.0))
                    .right(relative(0.0))
                    .h(px(3.0))
                    .rounded_full()
                    .bg(rgb(style.rail)),
            )
            .child(
                div()
                    .absolute()
                    .left(relative(0.0))
                    .w(relative(normalized))
                    .h(px(3.0))
                    .rounded_full()
                    .bg(rgb(fill_color)),
            )
            .child(thumb.left(relative(normalized)))
    }
}
