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
mod render;
mod state;
mod text_edit;
mod vertical_fader;

#[cfg(test)]
use state::Press;
use state::PressTarget;
pub use state::{NumericControl, NumericEditState};

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

/// Identity of one externally resolved text submission. Clones do not retain
/// the control and are never interchangeable with another submission.
#[derive(Debug, Clone)]
pub struct NumericTextRequestId(Rc<()>);

impl PartialEq for NumericTextRequestId {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for NumericTextRequestId {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumericTextRequest {
    pub id: NumericTextRequestId,
    pub text: String,
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
