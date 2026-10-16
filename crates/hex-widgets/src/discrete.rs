use crate::ControlStyle;
use gpui::{
    AnchoredPositionMode, Context, Corner, EventEmitter, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, PathBuilder, SharedString, Window, anchored, canvas, deferred, div, point,
    prelude::*, px, rgb,
};

#[derive(Debug, Clone)]
pub struct ToggleChanged {
    pub checked: bool,
}

/// Presentation-only density for the shared boolean/choice entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiscreteControlAppearance {
    #[default]
    Standard,
    CompactButton,
    /// Unlabelled square activator; caller supplies the surrounding label.
    CompactSquare,
    /// Smaller marker in the same operable square hit region.
    MicroSquare,
    CompactIconButton,
    CompactSelector,
    /// Small directly selectable options without a popup.
    CompactSegments,
}

/// Validated toggle construction data for an infallible GPUI entity factory.
pub struct ToggleConfig {
    id: SharedString,
    label: SharedString,
    checked: bool,
    style: ControlStyle,
    appearance: DiscreteControlAppearance,
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
            appearance: DiscreteControlAppearance::Standard,
        })
    }

    pub fn with_appearance(mut self, appearance: DiscreteControlAppearance) -> Self {
        self.appearance = appearance;
        self
    }
}

/// Compact boolean control with caller-owned text and styling.
pub struct Toggle {
    id: SharedString,
    label: SharedString,
    checked: bool,
    enabled: bool,
    style: ControlStyle,
    appearance: DiscreteControlAppearance,
    tab_navigation: bool,
    focus: FocusHandle,
}

impl Toggle {
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
            id: config.id,
            label: config.label,
            checked: config.checked,
            enabled: true,
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
        let tab_stop = self.tab_navigation && self.enabled;
        if self.focus.tab_stop != tab_stop {
            self.focus = self.focus.clone().tab_index(0).tab_stop(tab_stop);
            cx.notify();
        }
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled != enabled {
            self.enabled = enabled;
            let tab_stop = self.tab_navigation && self.enabled;
            if self.focus.tab_stop != tab_stop {
                self.focus = self.focus.clone().tab_index(0).tab_stop(tab_stop);
            }
            cx.notify();
        }
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.enabled {
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
        let text_color = if self.enabled {
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
                    DiscreteControlAppearance::CompactButton
                        | DiscreteControlAppearance::CompactIconButton
                ) {
                    14.0
                } else {
                    20.0
                },
            ))
            .px(px(
                if matches!(
                    self.appearance,
                    DiscreteControlAppearance::CompactButton
                        | DiscreteControlAppearance::CompactIconButton
                ) {
                    1.0
                } else {
                    3.0
                },
            ))
            .gap(px(
                if self.appearance == DiscreteControlAppearance::CompactButton {
                    3.0
                } else {
                    5.0
                },
            ))
            .items_center()
            .rounded(px(
                if self.appearance == DiscreteControlAppearance::CompactButton {
                    1.5
                } else {
                    2.0
                },
            ))
            .bg(rgb(
                if matches!(
                    self.appearance,
                    DiscreteControlAppearance::CompactButton
                        | DiscreteControlAppearance::CompactIconButton
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
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_key_down(cx.listener(Self::key_down))
            .cursor(if self.enabled {
                gpui::CursorStyle::PointingHand
            } else {
                gpui::CursorStyle::Arrow
            });
        if matches!(
            self.appearance,
            DiscreteControlAppearance::CompactSquare | DiscreteControlAppearance::MicroSquare
        ) {
            let marker_size = if self.appearance == DiscreteControlAppearance::MicroSquare {
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
        if self.appearance == DiscreteControlAppearance::CompactIconButton {
            let enabled = self.enabled;
            let icon_color = rgb(if enabled {
                self.style.text
            } else {
                self.style.muted
            });
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
        } else if self.appearance == DiscreteControlAppearance::CompactButton {
            root = root.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_center()
                    .text_size(px(8.0))
                    .text_color(rgb(if self.enabled {
                        self.style.text
                    } else {
                        self.style.muted
                    }))
                    .child(self.label.clone()),
            );
        } else {
            root = root
                .child(
                    div()
                        .size(px(12.0))
                        .rounded(px(2.0))
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
            DiscreteControlAppearance::CompactButton | DiscreteControlAppearance::CompactIconButton
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
pub struct SelectorConfig {
    id: SharedString,
    choices: Vec<SharedString>,
    selected: usize,
    style: ControlStyle,
    appearance: DiscreteControlAppearance,
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
        if choices.len() > 128 {
            return Err("selector exceeds 128 options");
        }
        Ok(Self {
            id: id.into(),
            choices,
            selected,
            style: style.validate()?,
            appearance: DiscreteControlAppearance::Standard,
        })
    }

    pub fn with_appearance(mut self, appearance: DiscreteControlAppearance) -> Self {
        self.appearance = appearance;
        self
    }
}

/// Compact choice field with a bounded foreground popup.
pub struct Selector {
    id: SharedString,
    choices: Vec<SharedString>,
    selected: usize,
    open: bool,
    enabled: bool,
    style: ControlStyle,
    appearance: DiscreteControlAppearance,
    tab_navigation: bool,
    focus: FocusHandle,
    focus_blur: Option<gpui::Subscription>,
    measured_size: std::rc::Rc<std::cell::Cell<gpui::Size<gpui::Pixels>>>,
}

impl Selector {
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
            id: config.id,
            choices: config.choices,
            selected: config.selected,
            open: false,
            enabled: true,
            style: config.style,
            appearance: config.appearance,
            tab_navigation: false,
            focus: cx.focus_handle(),
            focus_blur: None,
            measured_size: std::rc::Rc::new(std::cell::Cell::new(gpui::size(px(0.0), px(0.0)))),
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
            let tab_stop = self.tab_navigation && self.enabled;
            if self.focus.tab_stop != tab_stop {
                self.focus = self.focus.clone().tab_index(0).tab_stop(tab_stop);
            }
            cx.notify();
        }
    }

    fn open_or_close(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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

    fn key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        if self.appearance == DiscreteControlAppearance::CompactSegments {
            let next = match event.keystroke.key.as_str() {
                "right" | "down" | "space" => (self.selected + 1) % self.choices.len(),
                "left" | "up" => (self.selected + self.choices.len() - 1) % self.choices.len(),
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
                if selector.open {
                    selector.open = false;
                    cx.notify();
                }
            }));
        }
        if self.appearance == DiscreteControlAppearance::CompactSegments {
            let mut row = div()
                .id(self.id.clone())
                .debug_selector(|| self.id.to_string())
                .w_full()
                .h(px(14.0))
                .flex()
                .gap(px(2.0))
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::key_down));
            for (index, label) in self.choices.iter().cloned().enumerate() {
                row = row.child(
                    div()
                        .id(index)
                        .flex_1()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(rgb(if self.focus.is_focused(window) {
                            self.style.accent
                        } else {
                            self.style.border
                        }))
                        .bg(rgb(if index == self.selected {
                            self.style.accent
                        } else {
                            self.style.surface
                        }))
                        .text_size(px(8.0))
                        .text_color(rgb(if index == self.selected {
                            self.style.background
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
            return row;
        }
        let selected = self.choices[self.selected].clone();
        let mut root = div()
            .id(self.id.clone())
            .flex()
            .flex_col()
            .min_w_0()
            .relative()
            .track_focus(&self.focus)
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
                        if self.appearance == DiscreteControlAppearance::CompactSelector {
                            10.0
                        } else {
                            20.0
                        },
                    ))
                    .px(px(
                        if self.appearance == DiscreteControlAppearance::CompactSelector {
                            2.0
                        } else {
                            4.0
                        },
                    ))
                    .gap(px(4.0))
                    .items_center()
                    .rounded(px(
                        if self.appearance == DiscreteControlAppearance::CompactSelector {
                            1.5
                        } else {
                            2.0
                        },
                    ))
                    .border_1()
                    .border_color(rgb(self.style.border))
                    .bg(rgb(self.style.raised))
                    .text_size(px(
                        if self.appearance == DiscreteControlAppearance::CompactSelector {
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
        let measured_size = self.measured_size.clone();
        root = root.child(
            canvas(
                move |bounds, window, _| {
                    if measured_size.replace(bounds.size) != bounds.size {
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
        if self.appearance == DiscreteControlAppearance::CompactSelector {
            root = root.w_full();
        }
        if self.open && self.enabled {
            let options = self.choices.clone();
            let current = self.selected;
            let compact = self.appearance == DiscreteControlAppearance::CompactSelector;
            let field_height = if compact { 16.0 } else { 22.0 };
            let option_height = if compact { 18.0 } else { 19.0 };
            let style = self.style;
            // Absolute children use the selector's content box; account for its one-pixel border
            // on each side so the overlay aligns with the visible field frame.
            let popup_width = self.measured_size.get().width + px(2.0);
            let max_popup_height = (window.viewport_size().height - px(8.0)).max(px(1.0));
            let mut popup = div()
                .id(SharedString::from(format!("{}-popup", self.id)))
                .debug_selector(|| format!("{}-popup", self.id))
                .flex()
                .flex_col()
                .w(popup_width)
                .max_h(max_popup_height)
                .overflow_y_scroll()
                .border_1()
                .border_color(rgb(style.border))
                .rounded(px(2.0))
                .bg(rgb(style.surface))
                .shadow_md()
                .occlude();
            for (index, choice) in options.into_iter().enumerate() {
                let listener = cx.listener(move |selector, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    selector.choose(index, window, cx)
                });
                popup = popup.child(
                    div()
                        .id(SharedString::from(format!("{}-option-{index}", self.id)))
                        .debug_selector(|| format!("{}-option-{index}", self.id))
                        .flex()
                        .flex_shrink_0()
                        .h(px(option_height))
                        .px(px(4.0))
                        .items_center()
                        .rounded(px(2.0))
                        .border_b_1()
                        .border_color(rgb(self.style.border))
                        .bg(rgb(if index == current {
                            self.style.hover
                        } else {
                            self.style.surface
                        }))
                        .text_size(px(self.style.font_size))
                        .text_color(rgb(self.style.text))
                        .cursor(gpui::CursorStyle::PointingHand)
                        .on_mouse_down(MouseButton::Left, listener)
                        .child(choice),
                );
            }
            root = root.child(
                deferred(
                    anchored()
                        .anchor(Corner::TopLeft)
                        .position(point(px(0.0), px(field_height)))
                        .position_mode(AnchoredPositionMode::Local)
                        .child(popup),
                )
                .with_priority(100),
            );
        }
        root.debug_selector(|| self.id.to_string())
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
