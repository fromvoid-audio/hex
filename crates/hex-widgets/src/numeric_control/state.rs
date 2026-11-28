//! Canonical value state, pointer scrubbing, stepping, and accessibility for `NumericControl`.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum PressTarget {
    Control,
    Value,
    // A slider handle scrubs like a readout but does not enter text on click.
    Fader,
}

#[derive(Clone, Copy)]
pub(super) struct Press {
    pub(super) origin: Point<Pixels>,
    pub(super) last_position: Point<Pixels>,
    pub(super) start_value: f64,
    // Pointer position, not an authored/draft value. Retains sub-frame motion
    // between integer grid crossings without emitting a fractional value.
    pub(super) integer_pointer_position: Option<f64>,
    pub(super) unit_pointer_value: f64,
    pub(super) target: PressTarget,
    pub(super) dragging: bool,
    pub(super) value_changed: bool,
}

#[derive(Clone, Copy)]
pub(super) struct PendingBusyPress {
    pub(super) origin: Point<Pixels>,
    pub(super) last_position: Point<Pixels>,
    pub(super) modifiers: gpui::Modifiers,
    pub(super) target: PressTarget,
}

/// One reusable knob/slider and its independently editable numeric value.
#[derive(Clone, Copy)]
pub(super) struct TextMouseSelection {
    pub(super) anchor: usize,
    pub(super) origin: Point<Pixels>,
    pub(super) dragging: bool,
}

/// A read-only view of a numeric control's current text-edit state.
/// This shares only the boolean state, not the control or its GPUI entity.
#[derive(Clone)]
pub struct NumericEditState(pub(super) Rc<Cell<bool>>);

impl NumericEditState {
    /// Whether the control currently owns a text-edit draft.
    pub fn is_editing(&self) -> bool {
        self.0.get()
    }
}

pub struct NumericControl {
    pub(super) accessibility_binding: Option<gpui::AccessibilityBinding>,
    pub(super) accessibility_pending: bool,
    pub(super) accessibility_lifetime: gpui::AccessibilityLifetime,
    pub(super) id: SharedString,
    pub(super) label: SharedString,
    pub(super) kind: NumericControlKind,
    pub(crate) spec: NumericSpec,
    pub(crate) value: f64,
    pub(crate) draft: f64,
    pub(super) availability: crate::ControlAvailability,
    pub(super) commit_unchanged_text: bool,
    pub(super) deferred_text: bool,
    pub(super) pending_text: Option<NumericTextRequestId>,
    pub(super) pending_text_request: Option<NumericTextRequest>,
    pub(super) external_display: Option<(f64, String)>,
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
    pub(super) press: Option<Press>,
    pub(super) pending_busy_press: Option<PendingBusyPress>,
    pub(super) active_gesture: Option<NumericGestureId>,
    pub(super) text_mouse: Option<TextMouseSelection>,
    pub(crate) text_click: crate::TextClickSelection,
    pub(crate) style: ControlStyle,
    pub(super) meter_readout: crate::DeviceMeterReadout,
    pub(super) inline_value_width: Option<f32>,
    pub(super) appearance: NumericControlAppearance,
    pub(super) dial_layout: DialLayout,
    pub(super) tab_navigation: bool,
    pub(super) command_fine_steps: bool,
    pub(super) value_drag_steps: bool,
    pub(super) slider_center_mark: bool,
    pub(crate) focus: FocusHandle,
    pub(super) focus_blur: Option<gpui::Subscription>,
    pub(super) window_activation: Option<gpui::Subscription>,
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
            deferred_text: false,
            pending_text: None,
            pending_text_request: None,
            external_display: None,
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
            pending_busy_press: None,
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

    /// Exact text painted by this control, including any matching external display.
    pub fn presented_text(&self) -> String {
        self.value_string()
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

    /// Delegate typed text to the owner. This mode is opt-in and does not
    /// change pointer, keyboard stepping or ordinary synchronous controls.
    pub fn set_deferred_text(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.deferred_text == enabled {
            return;
        }
        self.cancel(cx);
        self.deferred_text = enabled;
        self.pending_text = None;
        self.pending_text_request = None;
        if !enabled {
            self.external_display = None;
        }
        cx.notify();
    }

    /// Associate a source-owned display string with one validated value.
    /// An active draft is never replaced, even when its value is refreshed.
    pub fn set_external_display(
        &mut self,
        value: f64,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), NumericError> {
        if !self.deferred_text {
            return Err(NumericError::InvalidText);
        }
        let value = self.spec.validate(value)?;
        if text.len() > 512 || text.contains(['\0', '\n', '\r']) {
            return Err(NumericError::InvalidText);
        }
        if self
            .external_display
            .as_ref()
            .is_some_and(|(old_value, old_text)| *old_value == value && old_text == text)
        {
            return Ok(());
        }
        self.external_display = Some((value, text.to_owned()));
        cx.notify();
        Ok(())
    }

    pub(crate) fn invalidate_pending_text(&mut self) {
        self.pending_text = None;
        self.pending_text_request = None;
    }

    pub(crate) fn deferred_text_enabled(&self) -> bool {
        self.deferred_text
    }

    /// Take a newly submitted request once after observing this entity.
    /// The unresolved ID remains current until a reply or cancellation.
    pub fn take_deferred_text_request(&mut self) -> Option<NumericTextRequest> {
        self.pending_text_request.take()
    }

    pub(super) fn request_deferred_text(&mut self, cx: &mut Context<Self>) {
        if self.pending_text.is_some() || !self.editing.is_editing() {
            return;
        }
        let id = NumericTextRequestId(Rc::new(()));
        self.pending_text = Some(id.clone());
        self.invalid_text = false;
        self.pending_text_request = Some(NumericTextRequest {
            id,
            text: self.text.clone(),
        });
        cx.notify();
    }

    /// Apply only the currently pending parse. A refusal leaves the text
    /// editable; cancellation or a newer draft makes old replies inert.
    pub fn resolve_deferred_text(
        &mut self,
        id: &NumericTextRequestId,
        value: Option<f64>,
        display: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<bool, NumericError> {
        if !self.deferred_text
            || !self.editing.is_editing()
            || self.pending_text.as_ref() != Some(id)
            || !self.availability.interactive()
        {
            return Ok(false);
        }
        self.pending_text = None;
        self.pending_text_request = None;
        let value = match value.and_then(|value| self.spec.validate(value).ok()) {
            Some(value) => value,
            None => {
                self.invalid_text = true;
                cx.notify();
                return Ok(true);
            }
        };
        if let Some(display) = display {
            if let Err(error) = self.set_external_display(value, display, cx) {
                self.invalid_text = true;
                cx.notify();
                return Err(error);
            }
        } else {
            self.external_display = None;
        }
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
        Ok(true)
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.set_availability(crate::ControlAvailability::for_edit(enabled, false), cx);
    }

    /// Suspend input for a transient operation while retaining the current palette.
    pub fn suspend_input(&mut self, cx: &mut Context<Self>) {
        let availability =
            crate::ControlAvailability::for_edit(self.availability.painted_enabled(), true);
        self.set_availability(availability, cx);
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
        } else if let Some(pending) = self.pending_busy_press.take() {
            self.promote_pending_busy_press(pending, cx);
        }
        cx.notify();
    }

    fn promote_pending_busy_press(&mut self, pending: PendingBusyPress, cx: &mut Context<Self>) {
        if self.editing.is_editing() || self.press.is_some() {
            return;
        }
        self.press = Some(Press {
            origin: pending.origin,
            last_position: pending.origin,
            start_value: self.draft,
            integer_pointer_position: None,
            unit_pointer_value: self.draft,
            target: pending.target,
            dragging: false,
            value_changed: false,
        });
        let moved_x = f32::from(pending.last_position.x - pending.origin.x);
        let moved_y = f32::from(pending.last_position.y - pending.origin.y);
        if moved_x.hypot(moved_y) >= 3.0 {
            let synthetic_move = MouseMoveEvent {
                position: pending.last_position,
                pressed_button: Some(MouseButton::Left),
                modifiers: pending.modifiers,
            };
            self.pointer_move(&synthetic_move, cx);
        }
    }

    /// Call before removing an active control so its caller can cancel the edit.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.invalidate_pending_text();
        self.pending_busy_press = None;
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

    pub(super) fn begin_press(
        &mut self,
        target: PressTarget,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        if !self.availability.interactive() {
            if self.availability == crate::ControlAvailability::Busy
                && !self.editing.is_editing()
                && event.click_count == 1
                && self.capture_pointer(target, window, cx)
            {
                self.pending_busy_press = Some(PendingBusyPress {
                    origin: event.position,
                    last_position: event.position,
                    modifiers: event.modifiers,
                    target,
                });
                cx.notify();
            }
            return;
        }
        self.pending_busy_press = None;
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

    pub(super) fn reset(&mut self, cx: &mut Context<Self>) {
        self.invalidate_pending_text();
        self.pending_busy_press = None;
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

    pub(super) fn value_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        cx.stop_propagation();
        if !self.availability.interactive() {
            self.begin_press(PressTarget::Value, event, window, cx);
            if self.pending_busy_press.is_some() {
                self.focus.focus(window);
            }
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
            self.pending_busy_press = None;
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

    pub(super) fn control_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        cx.stop_propagation();
        self.text_mouse = None;
        self.text_click.clear();
        self.begin_press(PressTarget::Control, event, window, cx);
        if self.availability.interactive() || self.pending_busy_press.is_some() {
            self.focus.focus(window);
        }
    }

    pub(super) fn capture_pointer(
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
                    if control.press.is_some() || control.pending_busy_press.is_some() {
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

    pub(super) fn pointer_move(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> (bool, bool) {
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
        if let Some(mut pending) = self.pending_busy_press {
            if event.pressed_button != Some(MouseButton::Left)
                || self.availability != crate::ControlAvailability::Busy
            {
                self.pending_busy_press = None;
                return (true, false);
            }
            pending.last_position = event.position;
            pending.modifiers = event.modifiers;
            self.pending_busy_press = Some(pending);
            return (true, false);
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
            self.invalidate_pending_text();
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

    pub(super) fn pointer_up(
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
        if self.pending_busy_press.take().is_some() {
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
            let text = if self.deferred_text {
                self.external_display
                    .as_ref()
                    .filter(|(value, _)| *value == self.draft)
                    .map(|(_, text)| text.clone())
                    .unwrap_or_else(|| self.spec.format(self.draft).unwrap_or_default())
            } else {
                self.spec.format(self.draft).unwrap_or_default()
            };
            self.editing.0.set(true);
            self.text_modified = false;
            self.invalid_text = false;
            self.text = text;
            self.selected = 0..self.text.len();
            self.reversed = false;
            self.focus.focus(window);
        } else {
            self.focus.focus(window);
        }
        cx.notify();
        true
    }

    pub(super) fn emit_step(&mut self, direction: f64, fine: bool, cx: &mut Context<Self>) {
        if !self.availability.interactive() {
            return;
        }
        if self.editing.is_editing() {
            // A composing or invalid draft must never be replaced with the old
            // canonical value. Stepping remains local until Enter/blur commits.
            if self.marked.is_some() || self.deferred_text {
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

    pub(super) fn emit_accessible_value(&mut self, value: f64, cx: &mut Context<Self>) -> bool {
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

    pub(super) fn accessibility_action(
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
}
