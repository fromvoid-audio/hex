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
use std::{cell::Cell, ops::Range, rc::Rc};

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
        // Some owners also expose Command as a fine-step alias. Shift is always fine.
        KeyBinding::new(
            "cmd-up",
            NumericUpFine,
            Some("hex_numeric_control && hex_numeric_command_steps"),
        ),
        KeyBinding::new(
            "cmd-down",
            NumericDownFine,
            Some("hex_numeric_control && hex_numeric_command_steps"),
        ),
    ]);
}

/// Intrinsic compact-knob tile dimensions in logical pixels. Owners allocating
/// a fixed tile must use these values rather than duplicating render geometry.
pub const COMPACT_DIAL_WIDTH: f32 = 64.0;
pub const COMPACT_DIAL_HEIGHT: f32 = 52.0;
/// Default inline-readout dimensions; explicit caller width overrides remain supported.
pub const INLINE_VALUE_WIDTH: f32 = 46.0;
pub const INLINE_VALUE_HEIGHT: f32 = 14.0;

/// Pointer scrubbing increases values upward for every control kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericControlKind {
    Knob,
    HorizontalSlider,
    VerticalSlider,
}

/// A complete visual presentation. Each variant determines both control kind
/// and geometry, so a knob cannot be paired with a slider-only appearance.
/// All presentations retain the same shared numeric edit entity and events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericControlPresentation {
    Knob,
    CompactDial,
    MicroDial,
    WideMicroDial,
    HorizontalSlider,
    InlineValue,
    CompactSlider,
    LabeledSlider,
    FilledSlider,
    /// Textless12×32 strip; scrub/keyboard/accessibility edits, with no text-entry target.
    FilledVerticalSlider,
    VerticalSlider,
    /// Input-level well with a ceiling triangle and editable numeric readout.
    MeterFader,
}
impl NumericControlPresentation {
    pub fn kind(self) -> NumericControlKind {
        match self {
            Self::Knob | Self::CompactDial | Self::MicroDial | Self::WideMicroDial => {
                NumericControlKind::Knob
            }
            Self::HorizontalSlider
            | Self::InlineValue
            | Self::CompactSlider
            | Self::LabeledSlider
            | Self::FilledSlider => NumericControlKind::HorizontalSlider,
            Self::VerticalSlider | Self::FilledVerticalSlider | Self::MeterFader => {
                NumericControlKind::VerticalSlider
            }
        }
    }
    fn appearance(self) -> NumericControlAppearance {
        match self {
            Self::Knob | Self::HorizontalSlider | Self::VerticalSlider => {
                NumericControlAppearance::Standard
            }
            Self::CompactDial => NumericControlAppearance::CompactDial,
            Self::MicroDial => NumericControlAppearance::MicroDial,
            Self::WideMicroDial => NumericControlAppearance::WideMicroDial,
            Self::InlineValue => NumericControlAppearance::InlineValue,
            Self::CompactSlider => NumericControlAppearance::CompactSlider,
            Self::LabeledSlider => NumericControlAppearance::LabeledSlider,
            Self::FilledSlider | Self::FilledVerticalSlider => {
                NumericControlAppearance::FilledSlider
            }
            Self::MeterFader => NumericControlAppearance::MeterFader,
        }
    }
}
impl From<NumericControlKind> for NumericControlPresentation {
    fn from(kind: NumericControlKind) -> Self {
        match kind {
            NumericControlKind::Knob => Self::Knob,
            NumericControlKind::HorizontalSlider => Self::HorizontalSlider,
            NumericControlKind::VerticalSlider => Self::VerticalSlider,
        }
    }
}

/// Presentation-only geometry that keeps the same shared edit entity and events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum NumericControlAppearance {
    #[default]
    Standard,
    CompactDial,
    MicroDial,
    WideMicroDial,
    /// A compact editable readout with a square frame and recessed background.
    InlineValue,
    /// A compact filled rectangle with an internal editable readout.
    CompactSlider,
    /// A centered label above a thin horizontal fill with an editable readout.
    LabeledSlider,
    /// One filled mixer field with centered editable text and upward scrubbing.
    /// Bipolar specs fill from the marked center; other scales fill from the left.
    FilledSlider,
    MeterFader,
}

#[cfg(test)]
mod compact_slider_tests;
mod compact_vertical;
#[cfg(test)]
mod edit_state_tests;
mod vertical_fader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericEventPhase {
    Begin,
    Change,
    Commit,
    Cancel,
}

/// Identity of one pointer drag, unique while any queued event or owner holds it.
/// Clones share the same identity. No counter or rollover is involved.
#[derive(Debug, Clone)]
pub struct NumericGestureId(Rc<()>);

impl PartialEq for NumericGestureId {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for NumericGestureId {}

#[derive(Debug, Clone, PartialEq)]
pub struct NumericControlEvent {
    pub phase: NumericEventPhase,
    pub value: f64,
    /// Present for pointer drags; other edit paths do not promise an identity.
    pub gesture: Option<NumericGestureId>,
}

/// Final value returned to an owner performing a lifecycle barrier. It lets
/// the owner synchronously submit the same edit before GPUI flushes emitters.
#[derive(Debug, Clone, PartialEq)]
pub struct NumericControlFinalValue {
    /// The user edited this draft, even when modified text resolves to the
    /// previous value. Passive clicks/blur are not parameter-touch intent.
    pub explicit_edit: bool,
    pub previous: f64,
    pub value: f64,
    /// Identity of an active pointer drag finalized by the owner, if any.
    pub gesture: Option<NumericGestureId>,
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
    command_fine_steps: bool,
    value_drag_steps: bool,
    slider_center_mark: bool,
}

impl NumericControlConfig {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        presentation: impl Into<NumericControlPresentation>,
        spec: NumericSpec,
        value: f64,
        style: ControlStyle,
    ) -> Result<Self, NumericError> {
        let presentation = presentation.into();
        let value = spec.validate(value)?;
        style.validate().map_err(|_| NumericError::InvalidStyle)?;
        let text = spec.format(value)?;
        Ok(Self {
            id: id.into(),
            label: label.into(),
            kind: presentation.kind(),
            spec,
            value,
            text,
            style,
            appearance: presentation.appearance(),
            dial_layout: DialLayout::ReadoutGap,
            command_fine_steps: false,
            value_drag_steps: false,
            slider_center_mark: true,
        })
    }

    /// Opt into continuous value-readout scrubbing in explicit units per pixel.
    /// Keyboard uses the same steps; rails and default controls are unchanged.
    pub fn with_value_drag_steps(mut self, coarse: f64, fine: f64) -> Result<Self, NumericError> {
        if self.spec.integer_values() {
            return Err(NumericError::InvalidScale);
        }
        self.spec = self.spec.with_keyboard_steps(coarse, fine)?;
        self.value_drag_steps = true;
        Ok(self)
    }

    /// Add Command-fine arrows as aliases for the universal Shift-fine keys.
    /// Every control supports stepping a valid local text draft.
    pub fn with_command_fine_steps(mut self) -> Self {
        self.command_fine_steps = true;
        self
    }

    /// Show or hide the center line of bipolar filled fields. The fill still
    /// grows from the center; input geometry and all value semantics stay intact.
    pub fn with_slider_center_mark(mut self, visible: bool) -> Self {
        self.slider_center_mark = visible;
        self
    }

    /// Select rail geometry from caller-owned control semantics, never density.
    pub fn with_dial_layout(mut self, layout: DialLayout) -> Self {
        self.dial_layout = layout;
        self
    }

    /// Replace the complete presentation, including its kind, as one valid value.
    pub fn with_presentation(mut self, presentation: NumericControlPresentation) -> Self {
        self.kind = presentation.kind();
        self.appearance = presentation.appearance();
        self
    }
}

#[derive(Clone, Copy)]
enum PressTarget {
    Control,
    Value,
    // A slider handle scrubs like a readout but does not enter text on click.
    Fader,
}

#[derive(Clone, Copy)]
struct Press {
    origin: Point<Pixels>,
    last_position: Point<Pixels>,
    start_value: f64,
    // Pointer position, not an authored/draft value. Retains sub-frame motion
    // between integer grid crossings without emitting a fractional value.
    integer_pointer_position: Option<f64>,
    unit_pointer_value: f64,
    target: PressTarget,
    dragging: bool,
    value_changed: bool,
}

/// One reusable knob/slider and its independently editable numeric value.
#[derive(Clone, Copy)]
struct TextMouseSelection {
    anchor: usize,
    origin: Point<Pixels>,
    dragging: bool,
}

/// A read-only view of a numeric control's current text-edit state.
/// This shares only the boolean state, not the control or its GPUI entity.
#[derive(Clone)]
pub struct NumericEditState(Rc<Cell<bool>>);

impl NumericEditState {
    /// Whether the control currently owns a text-edit draft.
    pub fn is_editing(&self) -> bool {
        self.0.get()
    }
}

pub struct NumericControl {
    accessibility_binding: Option<gpui::AccessibilityBinding>,
    accessibility_pending: bool,
    accessibility_lifetime: gpui::AccessibilityLifetime,
    id: SharedString,
    label: SharedString,
    kind: NumericControlKind,
    pub(crate) spec: NumericSpec,
    pub(crate) value: f64,
    pub(crate) draft: f64,
    availability: crate::ControlAvailability,
    commit_unchanged_text: bool,
    pub(crate) editing: NumericEditState,
    pub(crate) invalid_text: bool,
    pub(crate) text: String,
    pub(crate) text_modified: bool,
    pub(crate) caret_activity: u64,
    pub(crate) selected: Range<usize>,
    pub(crate) reversed: bool,
    pub(crate) marked: Option<Range<usize>>,
    pub(crate) last_layout: Option<ShapedLine>,
    pub(crate) last_bounds: Option<Bounds<Pixels>>,
    pub(crate) last_origin_x: Option<Pixels>,
    press: Option<Press>,
    active_gesture: Option<NumericGestureId>,
    text_mouse: Option<TextMouseSelection>,
    pub(crate) text_click: crate::TextClickSelection,
    pub(crate) style: ControlStyle,
    meter_readout: crate::DeviceMeterReadout,
    inline_value_width: Option<f32>,
    appearance: NumericControlAppearance,
    dial_layout: DialLayout,
    tab_navigation: bool,
    command_fine_steps: bool,
    value_drag_steps: bool,
    slider_center_mark: bool,
    pub(crate) focus: FocusHandle,
    focus_blur: Option<gpui::Subscription>,
    window_activation: Option<gpui::Subscription>,
}

impl Drop for NumericControl {
    fn drop(&mut self) {
        self.editing.0.set(false);
    }
}

impl NumericControl {
    /// Format an acknowledged value using this control's actual canonical codec.
    pub fn accessibility_value(
        &self,
        value: f64,
    ) -> Result<gpui::AccessibilityValue, NumericError> {
        Ok(gpui::AccessibilityValue::Number {
            value: self.spec.validate(value)?,
            min: self.spec.bounds().0,
            max: self.spec.bounds().1,
            text: self.spec.format(value)?.into(),
        })
    }

    /// Bind real acknowledged owner state without altering pointer/keyboard behavior.
    pub fn set_accessibility_binding(
        &mut self,
        binding: gpui::AccessibilityBinding,
        cx: &mut Context<Self>,
    ) {
        let changed = self.accessibility_binding.as_ref().is_none_or(|old| {
            old.name != binding.name
                || old.revision != binding.revision
                || old.value != binding.value
                || old.pending != binding.pending
        }) || self.accessibility_pending;
        self.accessibility_pending = false;
        self.accessibility_binding = Some(binding);
        if changed {
            cx.notify();
        }
    }

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
            command_fine_steps,
            value_drag_steps,
            slider_center_mark,
        } = config;
        Self {
            accessibility_binding: None,
            accessibility_pending: false,
            accessibility_lifetime: Default::default(),
            id,
            label,
            kind,
            spec,
            value,
            draft: value,
            availability: crate::ControlAvailability::Available,
            commit_unchanged_text: false,
            editing: NumericEditState(Rc::new(Cell::new(false))),
            text_modified: false,
            caret_activity: 0,
            invalid_text: false,
            text,
            selected: 0..0,
            reversed: false,
            marked: None,
            last_layout: None,
            last_bounds: None,
            last_origin_x: None,
            press: None,
            active_gesture: None,
            text_mouse: None,
            text_click: crate::TextClickSelection::default(),
            style,
            appearance,
            dial_layout,
            meter_readout: Default::default(),
            inline_value_width: None,
            tab_navigation: false,
            command_fine_steps,
            value_drag_steps,
            slider_center_mark,
            focus: cx.focus_handle(),
            focus_blur: None,
            window_activation: None,
        }
    }

    /// Size a compact numeric field without changing its codec or input protocol.
    /// Existing owners retain the forty-six-pixel default.
    pub fn set_inline_value_width(
        &mut self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Result<(), NumericError> {
        if !width.is_finite() || !(24.0..=256.0).contains(&width) {
            return Err(NumericError::InvalidStyle);
        }
        if self.inline_value_width != Some(width) {
            self.inline_value_width = Some(width);
            cx.notify();
        }
        Ok(())
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

    /// Original editable draft, preserving parser and clipboard semantics.
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
        self.press.is_some() || self.editing.is_editing()
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_editing()
    }

    /// Read the live text-edit state without borrowing the GPUI entity.
    pub fn edit_state(&self) -> NumericEditState {
        self.editing.clone()
    }

    /// Synchronize from caller state without disrupting an active local draft.
    pub fn set_external_value(
        &mut self,
        value: f64,
        cx: &mut Context<Self>,
    ) -> Result<(), NumericError> {
        let value = self.spec.validate(value)?;
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
        let tab_stop = self.tab_navigation && self.availability.interactive();
        if self.focus.tab_stop != tab_stop {
            self.focus = self.focus.clone().tab_index(0).tab_stop(tab_stop);
            cx.notify();
        }
    }

    /// Opt into a Commit for explicitly modified, accepted text even when its
    /// number is unchanged. This serves semantic owners such as replacing a
    /// tempo map. Defaults off; cancellation and untouched text never commit.
    pub fn set_commit_unchanged_text(&mut self, enabled: bool) {
        self.commit_unchanged_text = enabled;
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.set_availability(crate::ControlAvailability::for_edit(enabled, false), cx);
    }

    /// Update input admission without making transient worker activity dim the
    /// control. Busy cancels unsubmitted drafts; it does not commit owner state.
    pub fn set_availability(
        &mut self,
        availability: crate::ControlAvailability,
        cx: &mut Context<Self>,
    ) {
        if self.availability == availability {
            return;
        }
        let enabled = availability.interactive();
        self.availability = availability;
        let tab_stop = self.tab_navigation && enabled;
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
        self.text_mouse = None;
        self.text_click.clear();
        if let Some(press) = self.press.take()
            && press.dragging
        {
            self.value = press.start_value;
            self.draft = press.start_value;
            self.text = self.spec.format(self.draft).unwrap_or_default();
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Cancel,
                value: self.draft,
                gesture: self.active_gesture.clone(),
            });
        }
        self.active_gesture = None;
        if self.editing.is_editing() {
            self.editing.0.set(false);
            self.text_modified = false;
            self.invalid_text = false;
            self.text = self.spec.format(self.value).unwrap_or_default();
            self.selected = 0..0;
            self.marked = None;
        }
        cx.notify();
    }

    fn begin_press(
        &mut self,
        target: PressTarget,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.availability.interactive() || event.button != MouseButton::Left {
            return;
        }
        if matches!(self.kind, NumericControlKind::Knob)
            && matches!(target, PressTarget::Control)
            && event.click_count >= 2
        {
            self.reset(cx);
            return;
        }
        if !self.capture_pointer(target, window, cx) {
            return;
        }
        self.press = Some(Press {
            origin: event.position,
            last_position: event.position,
            start_value: self.draft,
            integer_pointer_position: None,
            unit_pointer_value: self.draft,
            target,
            dragging: false,
            value_changed: false,
        });
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.text_mouse = None;
        self.text_click.clear();
        let start = self.draft;
        let value = self.spec.default();
        // Reset finishes the local edit even when the value is already at its
        // default. A pending invalid text/IME draft must not retain focus-mode
        // or pointer capture and block later keyboard/external updates.
        self.press = None;
        self.active_gesture = None;
        self.editing.0.set(false);
        self.invalid_text = false;
        self.selected = 0..0;
        self.reversed = false;
        self.marked = None;
        self.text = self.spec.format(value).unwrap_or_default();
        if start != value {
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Begin,
                value: start,
                gesture: None,
            });
            self.value = value;
            self.draft = value;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Change,
                value,
                gesture: None,
            });
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value,
                gesture: None,
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
        let enabled = self.availability.painted_enabled();
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
                                if control.availability.interactive() {
                                    control.focus.focus(window);
                                    control.reset(cx);
                                }
                            }),
                        ),
                )
            })
    }

    fn text_index_for_point(&self, at: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds, self.last_layout.as_ref()) else {
            return 0;
        };
        if at.y < bounds.top() {
            return 0;
        }
        if at.y > bounds.bottom() {
            return self.text.len();
        }
        let shaped = line.closest_index_for_x(at.x - self.last_origin_x.unwrap_or(bounds.left()));
        self.style.text_case.source_offset(&self.text, shaped)
    }

    fn value_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.availability.interactive() {
            return;
        }
        let focused = self.focus.is_focused(window);
        if !focused || event.modifiers.shift {
            self.text_click.clear();
        }
        let mut gesture = self.text_click;
        let toggle = (!event.modifiers.shift)
            .then(|| {
                gesture.selection_for_down(
                    event.click_count,
                    focused
                        && self.editing.is_editing()
                        && !self.text.is_empty()
                        && self.selected == (0..self.text.len()),
                )
            })
            .flatten();
        if self.editing.is_editing() {
            if !self.capture_pointer(PressTarget::Value, window, cx) {
                return;
            }
            let offset = self.text_index_for_point(event.position);
            let anchor = if event.modifiers.shift && focused {
                if self.reversed {
                    self.selected.end
                } else {
                    self.selected.start
                }
            } else {
                offset
            };
            self.marked = None;
            if !focused || toggle == Some(true) {
                self.selected = 0..self.text.len();
                self.reversed = false;
            } else if event.modifiers.shift {
                self.select_to(offset);
            } else {
                self.move_to(offset);
            }
            self.press = None;
            self.active_gesture = None;
            self.text_mouse = Some(TextMouseSelection {
                anchor,
                origin: event.position,
                dragging: false,
            });
            self.text_click = gesture;
            self.focus.focus(window);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.text_mouse = None;
        self.text_click = gesture;
        self.begin_press(PressTarget::Value, event, window, cx);
        self.focus.focus(window);
    }

    fn control_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text_mouse = None;
        self.text_click.clear();
        self.begin_press(PressTarget::Control, event, window, cx);
        self.focus.focus(window);
    }

    fn capture_pointer(
        &self,
        target: PressTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            if matches!(target, PressTarget::Value) {
                CursorStyle::Arrow
            } else {
                CursorStyle::PointingHand
            },
            move |event, window, cx| {
                let _ = owner.update(cx, |control, cx| {
                    if let Some(event) = event.downcast_ref::<MouseMoveEvent>() {
                        control.pointer_move(event, cx);
                    } else if let Some(event) = event.downcast_ref::<MouseUpEvent>() {
                        control.pointer_up(event, window, cx);
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |control, cx| {
                    if control.press.is_some() {
                        control.cancel(cx);
                    } else if control.text_mouse.take().is_some() {
                        // Losing a selection anchor must not discard a newer
                        // keyboard/IME draft or an already accepted owner edit.
                        control.text_click.clear();
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }

    fn pointer_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) -> (bool, bool) {
        if let Some(mut selection) = self.text_mouse {
            if !self.editing.is_editing() || event.pressed_button != Some(MouseButton::Left) {
                self.text_mouse = None;
                self.text_click.clear();
                return (true, false);
            }
            if !selection.dragging
                && f32::from(event.position.x - selection.origin.x)
                    .hypot(f32::from(event.position.y - selection.origin.y))
                    < 2.0
            {
                return (true, false);
            }
            selection.dragging = true;
            self.text_mouse = Some(selection);
            self.text_click.clear();
            let offset = self.text_index_for_point(event.position);
            let selected = selection.anchor.min(offset)..selection.anchor.max(offset);
            let reversed = offset < selection.anchor;
            let changed = self.selected != selected || self.reversed != reversed;
            self.selected = selected;
            self.reversed = reversed;
            if changed {
                cx.notify();
            }
            return (true, changed);
        }
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
                    gesture: self.active_gesture.clone(),
                });
                self.active_gesture = None;
                cx.notify();
                return (true, true);
            }
            return (true, false);
        }
        let total_x = f32::from(event.position.x - press.origin.x);
        let total_y = f32::from(event.position.y - press.origin.y);
        if !press.dragging && total_x.hypot(total_y) >= 3.0 {
            press.dragging = true;
            self.active_gesture = Some(NumericGestureId(Rc::new(())));
            // Cursor visibility changes even at a value bound or on horizontal motion.
            cx.notify();
            self.editing.0.set(false);
            self.invalid_text = false;
            self.text = self.spec.format(press.start_value).unwrap_or_default();
            self.selected = 0..0;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Begin,
                value: press.start_value,
                gesture: self.active_gesture.clone(),
            });
        }
        if !press.dragging {
            return (true, false);
        }
        let delta_y = f32::from(event.position.y - press.last_position.y);
        press.last_position = event.position;
        self.press = Some(press);
        let shift = event.modifiers.shift;
        let primary_delta = -delta_y;
        if primary_delta == 0.0 {
            return (true, false);
        }
        let sensitivity = if shift { 1200.0 } else { 120.0 };
        let delta = f64::from(primary_delta) / sensitivity;
        let adjusted = if self.value_drag_steps
            && matches!(press.target, PressTarget::Value | PressTarget::Fader)
        {
            // Accumulate each movement with its current modifier, preserving
            // fractional motion. Clamping here prevents overshoot debt.
            match self
                .spec
                .step(press.unit_pointer_value, f64::from(primary_delta), shift)
            {
                Ok(value) => {
                    press.unit_pointer_value = value;
                    self.press = Some(press);
                    Ok(value)
                }
                Err(error) => Err(error),
            }
        } else if self.spec.integer_values() {
            let position = match press.integer_pointer_position {
                Some(position) => position,
                None => match self.spec.to_normalized(press.start_value) {
                    Ok(position) => position,
                    Err(_) => return (true, false),
                },
            };
            let position = (position + delta).clamp(0.0, 1.0);
            press.integer_pointer_position = Some(position);
            self.press = Some(press);
            self.spec.from_normalized(position)
        } else {
            self.spec.adjust(self.draft, delta)
        };
        let next = match adjusted {
            Ok(value) => value,
            Err(_) => return (true, false),
        };
        if next != self.draft {
            press.value_changed = true;
            self.press = Some(press);
            self.draft = next;
            self.value = next;
            self.text = self.spec.format(next).unwrap_or_default();
            self.invalid_text = false;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Change,
                value: next,
                gesture: self.active_gesture.clone(),
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
        if self.text_mouse.take().is_some() {
            cx.notify();
            return true;
        }
        let Some(press) = self.press.take() else {
            return false;
        };
        if press.dragging {
            self.value = self.draft;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value: self.draft,
                gesture: self.active_gesture.clone(),
            });
            self.active_gesture = None;
        } else if matches!(press.target, PressTarget::Value)
            && f32::from(event.position.x - press.origin.x)
                .hypot(f32::from(event.position.y - press.origin.y))
                < 3.0
        {
            self.editing.0.set(true);
            self.text_modified = false;
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
        if !self.availability.interactive() {
            return;
        }
        if self.editing.is_editing() {
            // A composing or invalid draft must never be replaced with the old
            // canonical value. Stepping remains local until Enter/blur commits.
            if self.marked.is_some() {
                return;
            }
            if let Ok(current) = self.spec.parse_with_current(&self.text, Some(self.value))
                && let Ok(value) = self.spec.step(current, direction, fine)
                && let Ok(text) = self.spec.format(value)
            {
                self.text = text;
                self.text_modified = true;
                self.invalid_text = false;
                self.selected = 0..self.text.len();
                self.reversed = false;
                cx.notify();
            }
            return;
        }
        if let Ok(value) = self.spec.step(self.draft, direction, fine) {
            self.emit_accessible_value(value, cx);
        }
    }

    fn emit_accessible_value(&mut self, value: f64, cx: &mut Context<Self>) -> bool {
        if !self.availability.interactive() || self.editing.is_editing() {
            return false;
        }
        let Ok(value) = self.spec.validate(value) else {
            return false;
        };
        if value != self.draft {
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Begin,
                value: self.draft,
                gesture: None,
            });
            self.draft = value;
            self.value = value;
            self.text = self.spec.format(value).unwrap_or_default();
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Change,
                value,
                gesture: None,
            });
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value,
                gesture: None,
            });
            cx.notify();
        }
        true
    }

    fn accessibility_action(
        &mut self,
        action: &gpui::AccessibilityAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use gpui::AccessibilityAction;
        if !self.availability.interactive() {
            return false;
        }
        if matches!(action, AccessibilityAction::Focus) {
            self.focus.focus(window);
            return true;
        }
        if self.is_interacting()
            || self.accessibility_pending
            || self
                .accessibility_binding
                .as_ref()
                .is_some_and(|binding| binding.pending || !(binding.guard)(cx))
        {
            return false;
        }
        let value = match action {
            AccessibilityAction::SetNumber(value) => Ok(*value),
            AccessibilityAction::Increment => self.spec.step(self.draft, 1., false),
            AccessibilityAction::Decrement => self.spec.step(self.draft, -1., false),
            _ => return false,
        };
        value.is_ok_and(|value| {
            let previous = self.draft;
            let accepted = self.emit_accessible_value(value, cx);
            if accepted && self.draft != previous && self.accessibility_binding.is_some() {
                self.accessibility_pending = true;
                cx.notify();
            }
            accepted
        })
    }

    fn enter(&mut self, _: &NumericEnter, _: &mut Window, cx: &mut Context<Self>) {
        self.text_mouse = None;
        self.text_click.clear();
        cx.stop_propagation();
        if !self.editing.is_editing() {
            return;
        }
        match self.spec.parse_with_current(&self.text, Some(self.value)) {
            Ok(value) => {
                let old = self.value;
                if old != value {
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Begin,
                        value: old,
                        gesture: None,
                    });
                    self.value = value;
                    self.draft = value;
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Change,
                        value,
                        gesture: None,
                    });
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Commit,
                        value,
                        gesture: None,
                    });
                } else if self.commit_unchanged_text && self.text_modified {
                    cx.emit(NumericControlEvent {
                        phase: NumericEventPhase::Commit,
                        value,
                        gesture: None,
                    });
                }
                self.text = self.spec.format(value).unwrap_or_default();
                self.editing.0.set(false);
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
        self.text_mouse = None;
        self.text_click.clear();
        if let Some(press) = self.press.take() {
            if press.dragging {
                cx.emit(NumericControlEvent {
                    phase: NumericEventPhase::Commit,
                    value: self.draft,
                    gesture: self.active_gesture.clone(),
                });
            }
            self.active_gesture = None;
            cx.notify();
        }
        if !self.editing.is_editing() {
            return Ok(());
        }
        let value = match self.finalized_text_value() {
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
                gesture: None,
            });
            self.value = value;
            self.draft = value;
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Change,
                value,
                gesture: None,
            });
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value,
                gesture: None,
            });
        } else if self.commit_unchanged_text && self.text_modified {
            cx.emit(NumericControlEvent {
                phase: NumericEventPhase::Commit,
                value,
                gesture: None,
            });
        }
        self.text = self.spec.format(value)?;
        self.editing.0.set(false);
        self.text_modified = false;
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
                    explicit_edit: press.value_changed,
                    previous: press.start_value,
                    value: self.draft,
                    gesture: self.active_gesture.clone(),
                    formatted: self.spec.format(self.draft)?,
                }));
            }
            return Ok(Some(NumericControlFinalValue {
                explicit_edit: false,
                previous: self.value,
                value: self.value,
                gesture: None,
                formatted: self.spec.format(self.value)?,
            }));
        }
        if !self.editing.is_editing() {
            return Ok(None);
        }
        let value = match self.finalized_text_value() {
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
            explicit_edit: self.text_modified || previous != value,
            previous,
            value,
            gesture: None,
            formatted,
        }))
    }

    fn finalized_text_value(&self) -> Result<f64, NumericError> {
        // A display codec may round the exact value. Focusing then leaving its
        // readout is not an edit; explicit input of that same text is an edit.
        if !self.text_modified && self.text == self.spec.format(self.value)? {
            Ok(self.value)
        } else {
            self.spec.parse_with_current(&self.text, Some(self.value))
        }
    }

    /// Complete a previously prepared draft after its owner admitted the
    /// canonical change. This method emits no subscription events.
    pub fn accept_finish_edit_for_owner(
        &mut self,
        finalized: NumericControlFinalValue,
        cx: &mut Context<Self>,
    ) {
        self.text_mouse = None;
        self.text_click.clear();
        if let Some(press) = self.press.take() {
            if press.dragging {
                self.value = finalized.value;
                self.draft = finalized.value;
                self.text = finalized.formatted;
            }
            self.editing.0.set(false);
            self.text_modified = false;
            self.invalid_text = false;
            self.selected = 0..0;
            self.marked = None;
            self.active_gesture = None;
            cx.notify();
            return;
        }
        if !self.editing.is_editing() {
            return;
        }
        self.value = finalized.value;
        self.draft = finalized.value;
        self.text = finalized.formatted;
        self.editing.0.set(false);
        self.text_modified = false;
        self.invalid_text = false;
        self.selected = 0..0;
        self.marked = None;
        cx.notify();
    }

    pub(crate) fn finish_text_edit_on_blur(&mut self, cx: &mut Context<Self>) {
        let _ = self.finish_edit(cx);
    }

    fn escape(&mut self, _: &NumericEscape, _: &mut Window, cx: &mut Context<Self>) {
        self.text_mouse = None;
        self.text_click.clear();
        if self.press.is_some() {
            self.cancel(cx);
        } else if self.editing.is_editing() {
            self.editing.0.set(false);
            self.invalid_text = false;
            self.text = self.spec.format(self.value).unwrap_or_default();
            self.selected = 0..0;
            self.marked = None;
            cx.notify();
        }
    }

    fn edit_left(&mut self, extend: bool, cx: &mut Context<Self>) {
        if self.editing.is_editing() {
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
        if self.editing.is_editing() {
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
        self.invalidate_text_pointer_selection();
        cx.stop_propagation();
        if self.editing.is_editing() {
            self.selected = 0..self.text.len();
            self.reversed = false;
            cx.notify();
        }
    }

    fn home(&mut self, _: &NumericHome, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing() {
            self.move_to(0);
            cx.notify();
        }
    }

    fn end(&mut self, _: &NumericEnd, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing() {
            self.move_to(self.text.len());
            cx.notify();
        }
    }

    fn backspace(&mut self, _: &NumericBackspace, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing() {
            self.delete_backward(window, cx);
        }
    }

    fn delete(&mut self, _: &NumericDelete, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing() {
            self.delete_forward(window, cx);
        }
    }

    fn copy(&mut self, _: &NumericCopy, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing()
            && let Some(text) = self.text.get(self.selected.clone())
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        }
    }

    fn cut(&mut self, _: &NumericCut, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing()
            && let Some(text) = self.text.get(self.selected.clone())
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn paste(&mut self, _: &NumericPaste, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.editing.is_editing()
            && let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
        {
            let clean = text.replace(['\n', '\r'], " ");
            self.replace_text_in_range(None, &clean, window, cx);
        }
    }

    fn slider_element(
        &self,
        value: f32,
        text: NumericTextElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        filled_slider(
            self.id.clone(),
            text,
            SliderPresentation {
                value,
                bipolar: self.spec.scale() == crate::NumericScale::Bipolar,
                center_mark: self.slider_center_mark,
                field: ValueFieldPresentation {
                    invalid: self.invalid_text,
                    editing: self.editing.is_editing(),
                    enabled: self.availability.painted_enabled(),
                    style: self.style,
                    width: px(60.0),
                    height: px(18.0),
                },
            },
            self.value_mouse_down_listener(cx),
        )
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
        if self.editing.is_editing() {
            self.style.text_case.present(&self.text)
        } else {
            self.style
                .text_case
                .present(&self.spec.format(self.draft).unwrap_or_default())
        }
    }

    pub(crate) fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    pub(crate) fn invalidate_text_pointer_selection(&mut self) {
        self.text_mouse = None;
        self.text_click.clear();
    }

    pub(crate) fn move_to(&mut self, offset: usize) {
        self.caret_activity = self.caret_activity.wrapping_add(1);
        self.invalidate_text_pointer_selection();
        let offset = offset.min(self.text.len());
        self.selected = offset..offset;
        self.reversed = false;
    }

    pub(crate) fn select_to(&mut self, offset: usize) {
        self.caret_activity = self.caret_activity.wrapping_add(1);
        self.invalidate_text_pointer_selection();
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
            .is_some_and(|press| matches!(press.target, PressTarget::Value | PressTarget::Fader))
            && !self.focus.is_focused(window)
        {
            self.cancel(cx);
        }
        // A text draft owns printable keys even beneath editor shortcut contexts.
        // Keep numeric editing actions available in the same focused node.
        let key_context = match (self.editing.is_editing(), self.command_fine_steps) {
            (true, true) => "hex_numeric_control hex_text_editing hex_numeric_command_steps",
            (false, true) => "hex_numeric_control hex_numeric_command_steps",
            (true, false) => "hex_numeric_control hex_text_editing",
            (false, false) => "hex_numeric_control",
        };
        let entity = cx.entity();
        let enabled = self.availability.painted_enabled();
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
                if control.press.is_some() {
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
                        && control.press.is_some_and(|press| {
                            matches!(press.target, PressTarget::Value | PressTarget::Fader)
                        })
                    {
                        control.cancel(cx);
                    }
                }));
        }

        let micro_dial_width = if self.appearance == NumericControlAppearance::WideMicroDial {
            crate::WIDE_MICRO_DIAL_WIDTH
        } else {
            crate::MICRO_DIAL_WIDTH
        };
        let mut root = match (self.kind, self.appearance) {
            (
                NumericControlKind::Knob,
                NumericControlAppearance::MicroDial | NumericControlAppearance::WideMicroDial,
            ) => div()
                .id(self.id.clone())
                .relative()
                .w(px(micro_dial_width))
                .h(px(crate::MICRO_DIAL_HEIGHT))
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(
                    div()
                        .absolute()
                        .debug_selector(|| format!("{}-label", self.id))
                        .top(px(0.0))
                        .left(px(2.0))
                        .w(px(micro_dial_width - 4.0))
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
                                editing: self.editing.is_editing(),
                                enabled,
                                style: ControlStyle {
                                    font_size: 8.0,
                                    ..style
                                },
                                width: px(micro_dial_width - 13.0),
                                height: px(12.0),
                            },
                            self.value_mouse_down_listener(cx),
                        )),
                ),
            (NumericControlKind::Knob, NumericControlAppearance::CompactDial) => {
                let (width, height, label_height, label_size, knob_size, value_height, gap) = (
                    COMPACT_DIAL_WIDTH,
                    COMPACT_DIAL_HEIGHT,
                    10.0,
                    9.0,
                    26.0,
                    14.0,
                    1.0,
                );
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
                            editing: self.editing.is_editing(),
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
                .w(px(self.inline_value_width.unwrap_or(INLINE_VALUE_WIDTH)))
                .h(px(INLINE_VALUE_HEIGHT))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(
                    value_field_sized(
                        self.id.clone(),
                        value_text,
                        ValueFieldPresentation {
                            invalid: self.invalid_text,
                            editing: self.editing.is_editing(),
                            enabled,
                            style,
                            width: px(self
                                .inline_value_width
                                .map_or(INLINE_VALUE_WIDTH, |width| width - 2.0)),
                            height: px(INLINE_VALUE_HEIGHT),
                        },
                        self.value_mouse_down_listener(cx),
                    )
                    .rounded_none()
                    .bg(rgb(style.background))
                    .border_color(rgb(if self.invalid_text {
                        style.invalid
                    } else {
                        style.border
                    })),
                ),
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
                    self.editing.is_editing(),
                    enabled,
                    style,
                    self.value_mouse_down_listener(cx),
                )),
            (NumericControlKind::HorizontalSlider, NumericControlAppearance::LabeledSlider) => {
                div()
                    .id(self.id.clone())
                    .flex()
                    .flex_col()
                    .w_full()
                    .min_w(px(60.0))
                    .h(px(32.0))
                    .gap(px(1.0))
                    .items_center()
                    .track_focus(&self.focus)
                    .key_context(key_context)
                    .on_mouse_down(MouseButton::Left, self.value_mouse_down_listener(cx))
                    .child(
                        div()
                            .w_full()
                            .h(px(14.0))
                            .text_center()
                            .overflow_hidden()
                            .text_size(px(style.font_size))
                            .text_color(rgb(if enabled { style.muted } else { style.border }))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex()
                            .w_full()
                            .h(px(14.0))
                            .child(self.slider_element(value, value_text, cx)),
                    )
            }
            (
                NumericControlKind::HorizontalSlider,
                NumericControlAppearance::FilledSlider | NumericControlAppearance::CompactSlider,
            ) => div()
                .id(self.id.clone())
                .flex()
                .w_full()
                .min_w(px(60.0))
                .h(px(
                    if self.appearance == NumericControlAppearance::CompactSlider {
                        21.0
                    } else {
                        18.0
                    },
                ))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .on_mouse_down(MouseButton::Left, self.value_mouse_down_listener(cx))
                .child(self.slider_element(value, value_text, cx)),
            (NumericControlKind::HorizontalSlider, _) => div()
                .id(self.id.clone())
                .flex()
                .h(px(24.0))
                .min_w(px(110.0))
                .gap(px(5.0))
                .items_center()
                .track_focus(&self.focus)
                .key_context(key_context)
                .on_mouse_down(MouseButton::Left, self.value_mouse_down_listener(cx))
                .child(
                    div()
                        .w(px(42.0))
                        .min_w(px(24.0))
                        .overflow_hidden()
                        .text_size(px(style.font_size))
                        .text_color(rgb(if enabled { style.muted } else { style.border }))
                        .child(label),
                )
                .child(self.slider_element(value, value_text, cx)),
            (NumericControlKind::VerticalSlider, NumericControlAppearance::FilledSlider) => div()
                .id(self.id.clone())
                .w(px(12.0))
                .h(px(32.0))
                .track_focus(&self.focus)
                .key_context(key_context)
                .child(self.compact_vertical(value, cx)),
            (NumericControlKind::VerticalSlider, _) => div()
                .id(self.id.clone())
                .flex()
                .flex_col()
                .w(px(60.0))
                .h(px(
                    if self.appearance == NumericControlAppearance::MeterFader {
                        154.0
                    } else {
                        86.0
                    },
                ))
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
                .child(self.vertical_fader(value, cx))
                .child(value_field(
                    self.id.clone(),
                    value_text,
                    self.invalid_text,
                    self.editing.is_editing(),
                    enabled,
                    style,
                    self.value_mouse_down_listener(cx),
                )),
        };

        let selected = enabled
            && (self.focus.is_focused(window) || self.press.is_some() || self.editing.is_editing());
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

        let owner = entity.downgrade();
        let descriptor = gpui::AccessibilityDescriptor {
            id: gpui::AccessibilityId(entity.entity_id().as_u64()),
            revision: self
                .accessibility_binding
                .as_ref()
                .map_or(0, |binding| binding.revision),
            lifetime: self.accessibility_lifetime.weak(),
            name: self
                .accessibility_binding
                .as_ref()
                .map_or_else(|| self.label.clone(), |binding| binding.name.clone()),
            value: self
                .accessibility_binding
                .as_ref()
                .map(|binding| binding.value.clone())
                .unwrap_or(gpui::AccessibilityValue::Number {
                    value: self.value,
                    min: self.spec.bounds().0,
                    max: self.spec.bounds().1,
                    text: self.spec.format(self.value).unwrap_or_default().into(),
                }),
            enabled: self.availability.interactive()
                && !self.is_interacting()
                && !self.accessibility_pending
                && self
                    .accessibility_binding
                    .as_ref()
                    .is_none_or(|binding| !binding.pending && (binding.guard)(cx)),
            focused: self.focus.is_focused(window),
            status: if self.is_interacting() {
                Some("local draft or gesture active".into())
            } else if self.availability == crate::ControlAvailability::Busy
                || self.accessibility_pending
                || self
                    .accessibility_binding
                    .as_ref()
                    .is_some_and(|binding| binding.pending)
            {
                Some("pending project edit".into())
            } else {
                None
            },
        };
        root = root.accessibility(descriptor, move |request, window, cx| {
            owner
                .update(cx, |control, cx| {
                    if control
                        .accessibility_binding
                        .as_ref()
                        .is_some_and(|binding| binding.revision != request.revision)
                    {
                        return false;
                    }
                    control.accessibility_action(&request.action, window, cx)
                })
                .unwrap_or(false)
        });
        if self.press.is_some() || self.text_mouse.is_some() {
            root.child(GestureCaptureElement { entity })
        } else {
            root
        }
    }
}

struct GestureCaptureElement {
    entity: Entity<NumericControl>,
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
        let control = self.entity.read(cx);
        let hidden = control.press.is_some_and(|press| {
            press.dragging && matches!(press.target, PressTarget::Value | PressTarget::Fader)
        }) && window.is_window_active();
        let cursor = if hidden {
            CursorStyle::Hidden
        } else if control
            .press
            .is_some_and(|press| matches!(press.target, PressTarget::Control | PressTarget::Fader))
        {
            CursorStyle::PointingHand
        } else {
            CursorStyle::Arrow
        };
        window.renew_pointer_capture(self.entity.entity_id(), cursor);
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
) -> gpui::Stateful<gpui::Div> {
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
            rgb(style.invalid)
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

/// Horizontal filled field; vertical controls use their separate fader geometry.
struct SliderPresentation {
    value: f32,
    bipolar: bool,
    center_mark: bool,
    field: ValueFieldPresentation,
}

fn filled_slider(
    id: SharedString,
    text: NumericTextElement,
    presentation: SliderPresentation,
    on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let SliderPresentation {
        value,
        bipolar,
        center_mark,
        field,
    } = presentation;
    let origin = if bipolar { 0.5 } else { 0.0 };
    let fill_id = format!("{id}-bar-fill");
    let center_id = format!("{id}-bar-center");
    let slider_id = format!("{id}-slider");
    let fill = div()
        .debug_selector(move || fill_id.clone())
        .absolute()
        .left(relative(value.min(origin)))
        .w(relative((value - origin).abs()))
        .h_full()
        .bg(rgb(if field.enabled {
            field.style.numeric_fill
        } else {
            field.style.muted
        }))
        .when(!field.enabled, |fill| fill.opacity(0.35));
    div()
        .debug_selector(move || slider_id.clone())
        .relative()
        .flex()
        .flex_1()
        .min_w(px(46.0))
        // Keep the surrounding row's hit area while thinning the painted field.
        .h(px(14.0))
        .overflow_hidden()
        .bg(rgb(field.style.rail))
        .child(fill)
        .when(bipolar && center_mark, |bar| {
            bar.child(
                div()
                    .debug_selector(move || center_id.clone())
                    .absolute()
                    .left(relative(0.5))
                    .w(px(1.0))
                    .h_full()
                    .bg(rgb(field.style.muted))
                    .opacity(0.6),
            )
        })
        .child(
            value_field_sized(id, text, field, on_down)
                .relative()
                .w_full()
                .h_full(),
        )
        // Paint the frame over the field without shrinking its editable hit area.
        .child(
            div()
                .absolute()
                .inset_0()
                .border_1()
                .border_color(rgb(if field.invalid {
                    field.style.invalid
                } else {
                    field.style.border
                })),
        )
}
