use crate::ControlStyle;
use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, PathBuilder,
    SharedString, Window, canvas, div, point, prelude::*, px, rgb,
};

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

    pub(super) fn foreground(&self) -> u32 {
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
