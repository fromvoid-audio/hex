use crate::ControlStyle;
use gpui::{
    AnchoredPositionMode, App, Context, Corner, EventEmitter, FocusHandle, Focusable, KeyBinding,
    MouseButton, MouseDownEvent, PathBuilder, SharedString, Window, anchored, canvas, deferred,
    div, point, prelude::*, px, rgb,
};

gpui::actions!(
    hex_widgets_selector,
    [
        SelectorToggle,
        SelectorCancel,
        SelectorNext,
        SelectorPrevious
    ]
);

/// Install focused popup shortcuts before the host's more general actions.
/// Closed popups leave Escape and arrows with their host.
pub fn bind_selector_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", SelectorToggle, Some("hex_selector")),
        KeyBinding::new("space", SelectorToggle, Some("hex_selector")),
        KeyBinding::new("escape", SelectorCancel, Some("hex_selector_open")),
        KeyBinding::new("down", SelectorNext, Some("hex_selector_open")),
        KeyBinding::new("up", SelectorPrevious, Some("hex_selector_open")),
    ]);
}

#[derive(Debug, Clone)]
pub struct ToggleChanged {
    pub checked: bool,
}

/// Boolean presentations; selector-only geometry cannot be supplied here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToggleAppearance {
    #[default]
    Standard,
    CompactButton,
    CompactSquare,
    MicroSquare,
    CompactIconButton,
}
const COMPACT_SELECTOR_ROW_HEIGHT: f32 = 12.0;

/// Choice presentations; boolean-only geometry cannot be supplied here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectorAppearance {
    #[default]
    Standard,
    CompactSelector,
    CompactSegments,
    /// A stepped dial; pointer release commits one supported choice.
    CompactDial,
}

/// Validated toggle construction data for an infallible GPUI entity factory.
/// Choice-only presentations are rejected by the type system:
/// ```compile_fail
/// use hex_widgets::{ToggleConfig, SelectorAppearance};
/// fn wrong_family(config: ToggleConfig) {
///     config.with_appearance(SelectorAppearance::CompactSelector);
/// }
/// ```
pub struct ToggleConfig {
    id: SharedString,
    label: SharedString,
    checked: bool,
    style: ControlStyle,
    appearance: ToggleAppearance,
}

impl ToggleConfig {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        checked: bool,
        style: ControlStyle,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            id: id.into(),
            label: label.into(),
            checked,
            style: style.validate()?,
            appearance: ToggleAppearance::Standard,
        })
    }

    pub fn with_appearance(mut self, appearance: ToggleAppearance) -> Self {
        self.appearance = appearance;
        self
    }
}

/// Compact boolean control with caller-owned text and styling.
pub struct Toggle {
    accessibility_binding: Option<gpui::AccessibilityBinding>,
    accessibility_pending: bool,
    accessibility_lifetime: gpui::AccessibilityLifetime,
    id: SharedString,
    label: SharedString,
    checked: bool,
    availability: crate::ControlAvailability,
    style: ControlStyle,
    appearance: ToggleAppearance,
    tab_navigation: bool,
    focus: FocusHandle,
}

impl Toggle {
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
        checked: bool,
        style: ControlStyle,
    ) -> Result<Self, &'static str> {
        Ok(Self::from_config(
            cx,
            ToggleConfig::new(id, label, checked, style)?,
        ))
    }

    pub fn from_config(cx: &mut Context<Self>, config: ToggleConfig) -> Self {
        Self {
            accessibility_binding: None,
            accessibility_pending: false,
            accessibility_lifetime: Default::default(),
            id: config.id,
            label: config.label,
            checked: config.checked,
            availability: crate::ControlAvailability::Available,
            style: config.style,
            appearance: config.appearance,
            tab_navigation: false,
            focus: cx.focus_handle(),
        }
    }

    pub fn checked(&self) -> bool {
        self.checked
    }

    /// Update caller-owned styling without recreating the focused control.
    pub fn set_external_style(
        &mut self,
        style: ControlStyle,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        let style = style.validate()?;
        if self.style != style {
            self.style = style;
            cx.notify();
        }
        Ok(())
    }

    pub fn set_checked(&mut self, checked: bool, cx: &mut Context<Self>) {
        if self.checked != checked {
            self.checked = checked;
            cx.notify();
        }
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

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.set_availability(crate::ControlAvailability::for_edit(enabled, false), cx);
    }

    /// Busy blocks pointer, keyboard and accessibility edits without dimming.
    pub fn set_availability(
        &mut self,
        availability: crate::ControlAvailability,
        cx: &mut Context<Self>,
    ) {
        if self.availability == availability {
            return;
        }
        self.availability = availability;
        let tab_stop = self.tab_navigation && self.availability.interactive();
        if self.focus.tab_stop != tab_stop {
            self.focus = self.focus.clone().tab_index(0).tab_stop(tab_stop);
        }
        cx.notify();
    }

    fn foreground(&self) -> u32 {
        if self.checked
            && matches!(
                self.appearance,
                ToggleAppearance::CompactButton | ToggleAppearance::CompactIconButton
            )
        {
            self.style.accent_text
        } else if self.availability.painted_enabled() {
            self.style.text
        } else {
            self.style.muted
        }
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.availability.interactive() {
            self.checked = !self.checked;
            self.focus.focus(window);
            cx.emit(ToggleChanged {
                checked: self.checked,
            });
            cx.notify();
        }
    }

    fn mouse_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle(window, cx);
    }

    fn key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event.keystroke.key.as_str(), "space" | "enter") {
            cx.stop_propagation();
            self.toggle(window, cx);
        }
    }
}

impl EventEmitter<ToggleChanged> for Toggle {}

impl Focusable for Toggle {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for Toggle {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state_color = if self.checked {
            self.style.accent
        } else {
            self.style.rail
        };
        let text_color = if self.availability.painted_enabled() {
            self.style.text
        } else {
            self.style.muted
        };
        let mut root = div()
            .id(self.id.clone())
            .debug_selector(|| self.id.to_string())
            .flex()
            .h(px(
                if matches!(
                    self.appearance,
                    ToggleAppearance::CompactButton | ToggleAppearance::CompactIconButton
                ) {
                    14.0
                } else {
                    20.0
                },
            ))
            .px(px(
                if matches!(
                    self.appearance,
                    ToggleAppearance::CompactButton | ToggleAppearance::CompactIconButton
                ) {
                    1.0
                } else {
                    3.0
                },
            ))
            .gap(px(if self.appearance == ToggleAppearance::CompactButton {
                3.0
            } else {
                5.0
            }))
            .items_center()
            .rounded(px(crate::TOGGLE_CORNER_RADIUS))
            .bg(rgb(
                if matches!(
                    self.appearance,
                    ToggleAppearance::CompactButton | ToggleAppearance::CompactIconButton
                ) && self.checked
                {
                    self.style.accent
                } else {
                    self.style.surface
                },
            ))
            .border_1()
            .border_color(rgb(if self.focus.is_focused(window) {
                self.style.accent
            } else {
                self.style.surface
            }))
            .track_focus(&self.focus)
            .when(self.availability.painted_enabled(), |root| {
                root.hover(|hover| hover.opacity(0.8))
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_key_down(cx.listener(Self::key_down))
            .cursor(if self.availability.interactive() {
                gpui::CursorStyle::PointingHand
            } else {
                gpui::CursorStyle::Arrow
            });
        let entity = cx.entity();
        let owner = entity.downgrade();
        root = root.accessibility(
            gpui::AccessibilityDescriptor {
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
                    .unwrap_or(gpui::AccessibilityValue::Toggle(self.checked)),
                enabled: self.availability.interactive()
                    && !self.accessibility_pending
                    && self
                        .accessibility_binding
                        .as_ref()
                        .is_none_or(|binding| !binding.pending && (binding.guard)(cx)),
                focused: self.focus.is_focused(window),
                status: if self.availability == crate::ControlAvailability::Busy
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
            },
            move |request, window, cx| {
                owner
                    .update(cx, |control, cx| {
                        if control
                            .accessibility_binding
                            .as_ref()
                            .is_some_and(|binding| binding.revision != request.revision)
                            || !control.availability.interactive()
                            || control.accessibility_pending
                            || control
                                .accessibility_binding
                                .as_ref()
                                .is_some_and(|binding| binding.pending || !(binding.guard)(cx))
                        {
                            return false;
                        }
                        match &request.action {
                            gpui::AccessibilityAction::Focus => control.focus.focus(window),
                            gpui::AccessibilityAction::Press => {
                                control.toggle(window, cx);
                                if control.accessibility_binding.is_some() {
                                    control.accessibility_pending = true;
                                    cx.notify();
                                }
                            }
                            _ => return false,
                        }
                        true
                    })
                    .unwrap_or(false)
            },
        );
        if matches!(
            self.appearance,
            ToggleAppearance::CompactSquare | ToggleAppearance::MicroSquare
        ) {
            let marker_size = if self.appearance == ToggleAppearance::MicroSquare {
                9.0
            } else {
                12.0
            };
            return root
                .w(px(14.0))
                .h(px(14.0))
                .p(px(0.0))
                .justify_center()
                .rounded(px(0.0))
                .border_color(gpui::rgba(0))
                .bg(gpui::rgba(0))
                .child(
                    div()
                        .debug_selector(|| format!("{}-indicator", self.id))
                        .size(px(marker_size))
                        .rounded(px(crate::TOGGLE_CORNER_RADIUS))
                        .border_1()
                        .border_color(rgb(if self.focus.is_focused(window) {
                            self.style.accent
                        } else {
                            self.style.text
                        }))
                        .bg(rgb(if self.checked {
                            self.style.accent
                        } else {
                            self.style.surface
                        })),
                );
        }
        if self.appearance == ToggleAppearance::CompactIconButton {
            let icon_color = rgb(self.foreground());
            root = root.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let left = bounds.origin.x + px(1.0);
                        let right = bounds.origin.x + bounds.size.width - px(1.0);
                        let top = bounds.origin.y + px(1.0);
                        let bottom = bounds.origin.y + bounds.size.height - px(1.0);
                        let mid = bounds.origin.y + bounds.size.height * 0.58;
                        let mut path = PathBuilder::stroke(px(1.1));
                        path.move_to(point(left, mid));
                        path.line_to(point(left, top + px(3.0)));
                        path.line_to(point(
                            bounds.origin.x + bounds.size.width * 0.3,
                            top + px(1.0),
                        ));
                        path.line_to(point(bounds.origin.x + bounds.size.width * 0.5, top));
                        path.line_to(point(
                            bounds.origin.x + bounds.size.width * 0.7,
                            top + px(1.0),
                        ));
                        path.line_to(point(right, top + px(3.0)));
                        path.line_to(point(right, mid));
                        path.move_to(point(left, mid));
                        path.line_to(point(left, bottom));
                        path.line_to(point(bounds.origin.x + bounds.size.width * 0.36, bottom));
                        path.line_to(point(bounds.origin.x + bounds.size.width * 0.36, mid));
                        path.line_to(point(left, mid));
                        path.move_to(point(bounds.origin.x + bounds.size.width * 0.64, mid));
                        path.line_to(point(bounds.origin.x + bounds.size.width * 0.64, bottom));
                        path.line_to(point(right, bottom));
                        path.line_to(point(right, mid));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, icon_color);
                        }
                    },
                )
                .size(px(10.0)),
            );
        } else if self.appearance == ToggleAppearance::CompactButton {
            root = root.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_center()
                    .text_size(px(8.0))
                    .text_color(rgb(self.foreground()))
                    .child(self.label.clone()),
            );
        } else {
            root = root
                .child(
                    div()
                        .size(px(12.0))
                        .rounded(px(crate::TOGGLE_CORNER_RADIUS))
                        .border_1()
                        .border_color(rgb(self.style.border))
                        .bg(rgb(state_color)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_size(px(self.style.font_size))
                        .text_color(rgb(text_color))
                        .child(self.label.clone()),
                );
        }
        if matches!(
            self.appearance,
            ToggleAppearance::CompactButton | ToggleAppearance::CompactIconButton
        ) {
            root = root.w_full();
        }
        root
    }
}

#[derive(Debug, Clone)]
pub struct SelectorChanged {
    pub index: usize,
    pub label: SharedString,
}

/// Validated choice construction data for an infallible GPUI entity factory.
/// Boolean-only presentations are rejected by the type system:
/// ```compile_fail
/// use hex_widgets::{SelectorConfig, ToggleAppearance};
/// fn wrong_family(config: SelectorConfig) {
///     config.with_appearance(ToggleAppearance::CompactButton);
/// }
/// ```
pub struct SelectorConfig {
    accessibility_name: SharedString,
    id: SharedString,
    choices: Vec<SharedString>,
    selected: usize,
    style: ControlStyle,
    appearance: SelectorAppearance,
    segment_columns: Option<usize>,
}

impl SelectorConfig {
    pub fn new(
        id: impl Into<SharedString>,
        choices: impl IntoIterator<Item = impl Into<SharedString>>,
        selected: usize,
        style: ControlStyle,
    ) -> Result<Self, &'static str> {
        let choices: Vec<_> = choices.into_iter().map(Into::into).collect();
        if choices.is_empty() || selected >= choices.len() {
            return Err("selector requires a valid selected option");
        }
        if choices.len() > 257 {
            return Err("selector exceeds 257 options");
        }
        Ok(Self {
            accessibility_name: "choice".into(),
            id: id.into(),
            choices,
            selected,
            style: style.validate()?,
            appearance: SelectorAppearance::Standard,
            segment_columns: None,
        })
    }

    /// Supply the parameter's human-readable name, separately from its machine ID.
    pub fn with_accessibility_name(mut self, name: impl Into<SharedString>) -> Self {
        self.accessibility_name = name.into();
        self
    }

    /// Arrange the same compact choice segments in a bounded keyboard grid.
    pub fn with_segment_columns(mut self, columns: usize) -> Result<Self, &'static str> {
        if !(1..=128).contains(&columns) {
            return Err("selector grid requires 1..128 columns");
        }
        self.appearance = SelectorAppearance::CompactSegments;
        self.segment_columns = Some(columns);
        Ok(self)
    }

    pub fn with_appearance(mut self, appearance: SelectorAppearance) -> Self {
        self.appearance = appearance;
        self
    }
}

#[derive(Clone, Copy)]
struct DialPress {
    origin_y: f32,
    start: usize,
    preview: usize,
}

/// Choice control with popup, segmented or stepped-dial presentation.
pub struct Selector {
    accessibility_binding: Option<gpui::AccessibilityBinding>,
    accessibility_pending: bool,
    accessibility_lifetime: gpui::AccessibilityLifetime,
    accessibility_name: SharedString,
    id: SharedString,
    choices: Vec<SharedString>,
    selected: usize,
    open: bool,
    enabled: bool,
    style: ControlStyle,
    appearance: SelectorAppearance,
    segment_columns: Option<usize>,
    tab_navigation: bool,
    focus: FocusHandle,
    focus_blur: Option<gpui::Subscription>,
    measured_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    dial_press: Option<DialPress>,
}

impl Selector {
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
        choices: impl IntoIterator<Item = impl Into<SharedString>>,
        selected: usize,
        style: ControlStyle,
    ) -> Result<Self, &'static str> {
        Ok(Self::from_config(
            cx,
            SelectorConfig::new(id, choices, selected, style)?,
        ))
    }

    pub fn from_config(cx: &mut Context<Self>, config: SelectorConfig) -> Self {
        Self {
            accessibility_binding: None,
            accessibility_pending: false,
            accessibility_lifetime: Default::default(),
            accessibility_name: config.accessibility_name,
            id: config.id,
            choices: config.choices,
            selected: config.selected,
            open: false,
            enabled: true,
            style: config.style,
            appearance: config.appearance,
            segment_columns: config.segment_columns,
            tab_navigation: false,
            focus: cx.focus_handle(),
            focus_blur: None,
            measured_bounds: Default::default(),
            dial_press: None,
        }
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Update caller-owned styling without discarding popup or focus state.
    pub fn set_external_style(
        &mut self,
        style: ControlStyle,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        let style = style.validate()?;
        if self.style != style {
            self.style = style;
            cx.notify();
        }
        Ok(())
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_selected(
        &mut self,
        selected: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        if selected >= self.choices.len() {
            return Err("selector index is outside its options");
        }
        if selected != self.selected {
            // An external acknowledged choice invalidates a captured local dial draft.
            self.dial_press = None;
            self.selected = selected;
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
        if self.enabled != enabled {
            self.enabled = enabled;
            self.open = false;
            self.dial_press = None;
            let tab_stop = self.tab_navigation && self.enabled;
            if self.focus.tab_stop != tab_stop {
                self.focus = self.focus.clone().tab_index(0).tab_stop(tab_stop);
            }
            cx.notify();
        }
    }

    fn open_or_close(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_popup(window, cx);
    }

    fn toggle_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.enabled {
            self.open = !self.open;
            self.focus.focus(window);
            cx.notify();
        }
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.enabled && index < self.choices.len() {
            self.selected = index;
            self.open = false;
            self.focus.focus(window);
            cx.emit(SelectorChanged {
                index,
                label: self.choices[index].clone(),
            });
            cx.notify();
        }
    }

    fn dial_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        cx.stop_propagation();
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        if !window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            gpui::CursorStyle::ResizeUpDown,
            move |event, window, cx| {
                let _ = owner.update(cx, |selector, cx| {
                    if let Some(event) = event.downcast_ref::<gpui::MouseMoveEvent>() {
                        if let Some(press) = selector.dial_press.as_mut() {
                            let steps = ((press.origin_y - f32::from(event.position.y)) / 24.0)
                                .round() as isize;
                            press.preview = (press.start as isize + steps)
                                .clamp(0, selector.choices.len() as isize - 1)
                                as usize;
                            cx.notify();
                        }
                    } else if event.downcast_ref::<gpui::MouseUpEvent>().is_some()
                        && let Some(press) = selector.dial_press.take()
                    {
                        if press.preview != press.start {
                            selector.choose(press.preview, window, cx);
                        }
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |selector, cx| {
                    selector.dial_press = None;
                    cx.notify();
                });
            },
            cx,
        ) {
            return;
        }
        self.dial_press = Some(DialPress {
            origin_y: event.position.y.into(),
            start: self.selected,
            preview: self.selected,
        });
        self.focus.focus(window);
        cx.notify();
    }

    fn accessible_root(
        &self,
        root: gpui::Stateful<gpui::Div>,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let entity = cx.entity();
        let owner = entity.downgrade();
        root.accessibility(
            gpui::AccessibilityDescriptor {
                id: gpui::AccessibilityId(entity.entity_id().as_u64()),
                revision: self
                    .accessibility_binding
                    .as_ref()
                    .map_or(0, |binding| binding.revision),
                lifetime: self.accessibility_lifetime.weak(),
                name: self.accessibility_binding.as_ref().map_or_else(
                    || self.accessibility_name.clone(),
                    |binding| binding.name.clone(),
                ),
                value: self
                    .accessibility_binding
                    .as_ref()
                    .map(|binding| binding.value.clone())
                    .unwrap_or(gpui::AccessibilityValue::Choice {
                        selected: self.selected,
                        options: self.choices.clone(),
                    }),
                enabled: self.enabled
                    && self.dial_press.is_none()
                    && !self.accessibility_pending
                    && self
                        .accessibility_binding
                        .as_ref()
                        .is_none_or(|binding| !binding.pending && (binding.guard)(cx)),
                focused: self.focus.is_focused(window),
                status: if self.accessibility_pending
                    || self
                        .accessibility_binding
                        .as_ref()
                        .is_some_and(|binding| binding.pending)
                {
                    Some("pending project edit".into())
                } else {
                    None
                },
            },
            move |request, window, cx| {
                owner
                    .update(cx, |control, cx| {
                        if control
                            .accessibility_binding
                            .as_ref()
                            .is_some_and(|binding| binding.revision != request.revision)
                            || control.dial_press.is_some()
                            || !control.enabled
                            || control.accessibility_pending
                            || control
                                .accessibility_binding
                                .as_ref()
                                .is_some_and(|binding| binding.pending || !(binding.guard)(cx))
                        {
                            return false;
                        }
                        match &request.action {
                            gpui::AccessibilityAction::Focus => control.focus.focus(window),
                            gpui::AccessibilityAction::Press => {
                                control.open = !control.open;
                                control.focus.focus(window);
                                cx.notify();
                            }
                            gpui::AccessibilityAction::Choose(index)
                                if *index < control.choices.len() =>
                            {
                                control.choose(*index, window, cx);
                                if control.accessibility_binding.is_some() {
                                    control.accessibility_pending = true;
                                    cx.notify();
                                }
                            }
                            _ => return false,
                        }
                        true
                    })
                    .unwrap_or(false)
            },
        )
    }

    fn key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        if self.appearance == SelectorAppearance::CompactDial && event.keystroke.key == "escape" {
            self.dial_press = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.appearance == SelectorAppearance::CompactDial {
            if self.dial_press.is_some() {
                cx.stop_propagation();
                return;
            }
            let next = match event.keystroke.key.as_str() {
                "right" | "up" | "space" => (self.selected + 1).min(self.choices.len() - 1),
                "left" | "down" => self.selected.saturating_sub(1),
                _ => return,
            };
            cx.stop_propagation();
            if next != self.selected {
                self.choose(next, window, cx);
            }
            return;
        }
        if self.appearance == SelectorAppearance::CompactSegments {
            let next = match event.keystroke.key.as_str() {
                "right" | "space" => (self.selected + 1) % self.choices.len(),
                "left" => (self.selected + self.choices.len() - 1) % self.choices.len(),
                "down" => (self.selected + self.segment_columns.unwrap_or(1)) % self.choices.len(),
                "up" => {
                    (self.selected + self.choices.len()
                        - self.segment_columns.unwrap_or(1) % self.choices.len())
                        % self.choices.len()
                }
                _ => return,
            };
            cx.stop_propagation();
            self.choose(next, window, cx);
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                self.open = false;
            }
            "enter" | "space" => {
                cx.stop_propagation();
                self.open = !self.open;
            }
            "down" if self.open => {
                cx.stop_propagation();
                self.choose((self.selected + 1) % self.choices.len(), window, cx)
            }
            "up" if self.open => {
                cx.stop_propagation();
                self.choose(
                    (self.selected + self.choices.len() - 1) % self.choices.len(),
                    window,
                    cx,
                )
            }
            _ => return,
        }
        cx.notify();
    }
}

impl EventEmitter<SelectorChanged> for Selector {}

impl Focusable for Selector {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for Selector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_blur.is_none() {
            let focus = self.focus.clone();
            self.focus_blur = Some(cx.on_blur(&focus, window, |selector, _, cx| {
                if selector.open || selector.dial_press.is_some() {
                    selector.open = false;
                    selector.dial_press = None;
                    cx.notify();
                }
            }));
        }
        if self.appearance == SelectorAppearance::CompactDial {
            let selected = self.dial_press.map_or(self.selected, |press| press.preview);
            let position = selected as f32 / self.choices.len().saturating_sub(1).max(1) as f32;
            let style = self.style;
            let enabled = self.enabled;
            let knob_id = format!("{}-knob", self.id);
            let held = self.dial_press.is_some();
            let owner = cx.entity_id();
            let row = div()
                .id(self.id.clone())
                .relative()
                .debug_selector(|| self.id.to_string())
                .w(px(64.0))
                .h(px(52.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(1.0))
                .when(self.focus.is_focused(window), |row| {
                    row.child(crate::focus_marker::corner_markers(
                        &self.id,
                        style.focus_marker,
                    ))
                })
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::key_down))
                .child(
                    div()
                        .h(px(10.0))
                        .text_size(px(9.0))
                        .text_color(rgb(style.muted))
                        .child(self.accessibility_name.clone()),
                )
                .child(
                    div()
                        .debug_selector(move || knob_id.clone())
                        .size(px(26.0))
                        .cursor(gpui::CursorStyle::ResizeUpDown)
                        .on_mouse_down(MouseButton::Left, cx.listener(Self::dial_down))
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, _, window, _| {
                                    if held {
                                        window.renew_pointer_capture(
                                            owner,
                                            gpui::CursorStyle::ResizeUpDown,
                                        );
                                    }
                                    crate::dial::paint_dial(
                                        bounds,
                                        26.0,
                                        crate::dial::DialValue::new(
                                            position,
                                            crate::NumericScale::Linear,
                                            crate::DialLayout::BottomGap,
                                        ),
                                        enabled,
                                        style,
                                        window,
                                    );
                                },
                            )
                            .size_full(),
                        ),
                )
                .child(
                    div()
                        .h(px(14.0))
                        .text_size(px(9.0))
                        .child(self.choices[selected].clone()),
                );
            return self.accessible_root(row, window, cx);
        }
        if self.appearance == SelectorAppearance::CompactSegments {
            let columns = self.segment_columns.unwrap_or(self.choices.len());
            let height = self.choices.len().div_ceil(columns) as f32 * 16.0 - 2.0;
            let mut row = div()
                .id(self.id.clone())
                .debug_selector(|| self.id.to_string())
                .w_full()
                .h(px(height))
                .grid()
                .grid_cols(columns as u16)
                .gap(px(2.0))
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::key_down));
            for (index, label) in self.choices.iter().cloned().enumerate() {
                row = row.child(
                    div()
                        .id(index)
                        .debug_selector(|| format!("{}-option-{index}", self.id))
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .h(px(14.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(rgb(
                            if self.enabled
                                && index == self.selected
                                && self.focus.is_focused(window)
                            {
                                self.style.text
                            } else {
                                self.style.border
                            },
                        ))
                        .bg(rgb(if index == self.selected {
                            self.style.accent
                        } else {
                            self.style.surface
                        }))
                        .when(self.enabled && index != self.selected, |option| {
                            option.hover(|hover| {
                                hover
                                    .bg(rgb(self.style.hover))
                                    .text_color(rgb(self.style.hover_text))
                            })
                        })
                        .text_size(px(8.0))
                        .text_color(rgb(if index == self.selected {
                            self.style.accent_text
                        } else {
                            self.style.text
                        }))
                        .cursor(if self.enabled {
                            gpui::CursorStyle::PointingHand
                        } else {
                            gpui::CursorStyle::Arrow
                        })
                        .child(label)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |selector, _, window, cx| {
                                cx.stop_propagation();
                                selector.choose(index, window, cx);
                            }),
                        ),
                );
            }
            return self.accessible_root(row, window, cx);
        }
        let selected = self.choices[self.selected].clone();
        let mut root = div()
            .id(self.id.clone())
            .flex()
            .flex_col()
            .min_w_0()
            .relative()
            .track_focus(&self.focus)
            .when(self.enabled, |root| {
                root.key_context(if self.open {
                    "hex_selector hex_selector_open"
                } else {
                    "hex_selector"
                })
            })
            .on_action(cx.listener(|selector, _: &SelectorToggle, window, cx| {
                selector.toggle_popup(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|selector, _: &SelectorCancel, _, cx| {
                selector.open = false;
                cx.notify();
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|selector, _: &SelectorNext, window, cx| {
                selector.choose((selector.selected + 1) % selector.choices.len(), window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|selector, _: &SelectorPrevious, window, cx| {
                selector.choose(
                    (selector.selected + selector.choices.len() - 1) % selector.choices.len(),
                    window,
                    cx,
                );
                cx.stop_propagation();
            }))
            .rounded(px(2.0))
            .border_1()
            .border_color(rgb(if self.focus.is_focused(window) {
                self.style.accent
            } else {
                self.style.surface
            }))
            .child(
                div()
                    .flex()
                    .h(px(
                        if self.appearance == SelectorAppearance::CompactSelector {
                            COMPACT_SELECTOR_ROW_HEIGHT - 2.0
                        } else {
                            20.0
                        },
                    ))
                    .px(px(
                        if self.appearance == SelectorAppearance::CompactSelector {
                            2.0
                        } else {
                            4.0
                        },
                    ))
                    .gap(px(4.0))
                    .items_center()
                    .rounded(px(
                        if self.appearance == SelectorAppearance::CompactSelector {
                            0.0
                        } else {
                            2.0
                        },
                    ))
                    .border_1()
                    .border_color(rgb(self.style.border))
                    .bg(rgb(self.style.raised))
                    .text_size(px(
                        if self.appearance == SelectorAppearance::CompactSelector {
                            8.0
                        } else {
                            self.style.font_size
                        },
                    ))
                    .text_color(rgb(if self.enabled {
                        self.style.text
                    } else {
                        self.style.muted
                    }))
                    .cursor(if self.enabled {
                        gpui::CursorStyle::PointingHand
                    } else {
                        gpui::CursorStyle::Arrow
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::open_or_close))
                    .child(div().flex_1().min_w_0().overflow_hidden().child(selected))
                    .child(div().text_color(rgb(self.style.muted)).child("▾")),
            );
        let measured_bounds = self.measured_bounds.clone();
        root = root.child(
            canvas(
                move |bounds, window, _| {
                    if measured_bounds.replace(bounds) != bounds {
                        window.refresh();
                    }
                },
                |_, _, _, _| (),
            )
            .absolute()
            .left(px(0.0))
            .right(px(0.0))
            .top(px(0.0))
            .bottom(px(0.0)),
        );
        if self.appearance == SelectorAppearance::CompactSelector {
            root = root.w_full().rounded_none();
        }
        if self.open && self.enabled {
            let options = self.choices.clone();
            let current = self.selected;
            let compact = self.appearance == SelectorAppearance::CompactSelector;
            let field_bounds = self.measured_bounds.get();
            let field_height = if compact {
                f32::from(field_bounds.size.height)
            } else {
                22.0
            };
            let option_height = if compact {
                COMPACT_SELECTOR_ROW_HEIGHT
            } else {
                19.0
            };
            let style = self.style;
            // Absolute children use the selector's content box; account for its one-pixel border
            // on each side so the overlay aligns with the visible field frame.
            // A compact field may abbreviate its selection; its menu must fit
            // the full choice labels in the inherited application typography.
            let font = window.text_style().font();
            let label_width = options
                .iter()
                .map(|label| {
                    window
                        .text_system()
                        .shape_line(
                            label.clone(),
                            px(style.font_size),
                            &[gpui::TextRun {
                                len: label.len(),
                                font: font.clone(),
                                color: rgb(style.text).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            None,
                        )
                        .width
                })
                .fold(px(0.0), |width, next| width.max(next));
            let field_width = field_bounds.size.width + px(2.0);
            let popup_width = if compact {
                field_width
            } else {
                field_width.max(label_width + px(12.0))
            }
            .min((window.viewport_size().width - px(if compact { 0.0 } else { 8.0 })).max(px(1.0)));
            let preferred_height = px(options.len() as f32 * option_height + 2.0);
            let below = (window.viewport_size().height - field_bounds.bottom()).max(px(1.0));
            let above = field_bounds.top().max(px(1.0));
            let open_above = compact && preferred_height > below && above > below;
            let max_popup_height = if compact {
                if open_above { above } else { below }
            } else {
                (window.viewport_size().height - px(8.0)).max(px(1.0))
            };
            let popup_height = preferred_height.min(max_popup_height);
            let popup_y = if open_above {
                -popup_height
            } else {
                px(field_height)
            };
            let mut popup = div()
                .id(SharedString::from(format!("{}-popup", self.id)))
                .debug_selector(|| format!("{}-popup", self.id))
                .flex()
                .flex_col()
                .w(popup_width)
                .max_h(max_popup_height)
                .overflow_y_scroll()
                .border_1()
                .border_color(rgb(if compact { style.accent } else { style.border }))
                .rounded(px(if compact { 0.0 } else { 2.0 }))
                .bg(rgb(style.surface))
                .when(!compact, |popup| popup.shadow_md())
                .occlude();
            for (index, choice) in options.into_iter().enumerate() {
                let listener = cx.listener(move |selector, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    selector.choose(index, window, cx)
                });
                let marker_id = format!("{}-option-marker-{index}", self.id);
                popup = popup.child(
                    div()
                        .id(SharedString::from(format!("{}-option-{index}", self.id)))
                        .debug_selector(|| format!("{}-option-{index}", self.id))
                        .relative()
                        .flex()
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .flex_shrink_0()
                        .h(px(option_height))
                        .px(px(if compact { 3.0 } else { 4.0 }))
                        .items_center()
                        .rounded(px(if compact { 0.0 } else { 2.0 }))
                        .border_b_1()
                        .border_color(rgb(self.style.border))
                        .bg(rgb(if index == current {
                            self.style.hover
                        } else {
                            self.style.surface
                        }))
                        .hover(move |hover| {
                            hover.bg(rgb(style.hover)).text_color(rgb(style.hover_text))
                        })
                        .text_size(px(if compact { 8.0 } else { style.font_size }))
                        .text_color(rgb(if index == current {
                            style.hover_text
                        } else {
                            style.text
                        }))
                        .cursor(gpui::CursorStyle::PointingHand)
                        .on_mouse_down(MouseButton::Left, listener)
                        .child(choice.clone())
                        .when(compact, |row| {
                            row.tooltip(move |_, cx| {
                                cx.new(|_| SelectorOptionTooltip(choice.clone(), style))
                                    .into()
                            })
                        })
                        // Keep the actual selection distinct while another row is hovered.
                        .when(index == current, |option| {
                            option.child(
                                div()
                                    .debug_selector(move || marker_id.clone())
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(px(2.0))
                                    .bg(rgb(style.accent)),
                            )
                        }),
                );
            }
            root = root.child(
                deferred(
                    anchored()
                        .anchor(Corner::TopLeft)
                        .position(point(px(if compact { -1.0 } else { 0.0 }), popup_y))
                        .position_mode(AnchoredPositionMode::Local)
                        .child(popup),
                )
                .with_priority(100),
            );
        }
        self.accessible_root(root, window, cx)
            .debug_selector(|| self.id.to_string())
            .on_mouse_down_out(cx.listener(|selector, _, _, cx| {
                if selector.open {
                    selector.open = false;
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(Self::key_down))
    }
}

/// Flat compact panel navigation. The caller owns lowercase text, identity,
/// focus and page-switch semantics; the shared library owns its appearance.
pub fn panel_tab(label: gpui::SharedString, active: bool, style: ControlStyle) -> gpui::Div {
    gpui::div()
        .h(gpui::px(16.0))
        .px(gpui::px(3.0))
        .flex()
        .items_center()
        .gap(gpui::px(3.0))
        .border_b_1()
        .border_color(gpui::rgb(if active { style.accent } else { style.border }))
        .focus(move |focus| focus.border_color(gpui::rgb(style.focus_marker)))
        .text_size(gpui::px(8.0))
        .text_color(gpui::rgb(style.text))
        .child(
            gpui::div()
                .size(gpui::px(6.0))
                .border_1()
                .border_color(gpui::rgb(style.accent))
                .when(active, |marker| marker.bg(gpui::rgb(style.accent))),
        )
        .child(label)
}

// A compact popup keeps the field width; full normalized option text remains
// available on hover instead of widening the detached panel.
struct SelectorOptionTooltip(SharedString, ControlStyle);
impl gpui::Render for SelectorOptionTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(4.0))
            .py(px(2.0))
            .bg(rgb(self.1.raised))
            .text_color(rgb(self.1.text))
            .text_size(px(self.1.font_size))
            .child(self.0.clone())
    }
}

#[cfg(test)]
mod selector_tests;

#[cfg(test)]
mod contrast_tests {
    use super::*;
    use crate::tests::STYLE;
    #[gpui::test]
    fn checked_compact_labels_and_icons_use_dark_ink_on_bright_fill(cx: &mut gpui::TestAppContext) {
        for appearance in [
            ToggleAppearance::CompactButton,
            ToggleAppearance::CompactIconButton,
        ] {
            let toggle = cx.new(|cx| {
                Toggle::from_config(
                    cx,
                    ToggleConfig::new("contrast", "on", true, STYLE)
                        .unwrap()
                        .with_appearance(appearance),
                )
            });
            toggle.read_with(cx, |toggle, _| assert_eq!(toggle.foreground(), 0x202020));
        }
    }
}
