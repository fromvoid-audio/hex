use crate::ControlStyle;
use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, SharedString,
    Window, div, prelude::*, px, rgb,
};

#[derive(Debug, Clone)]
pub struct ToggleChanged {
    pub checked: bool,
}

/// Compact boolean control with caller-owned text and styling.
pub struct Toggle {
    id: SharedString,
    label: SharedString,
    checked: bool,
    enabled: bool,
    style: ControlStyle,
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
        Ok(Self {
            id: id.into(),
            label: label.into(),
            checked,
            enabled: true,
            style: style.validate()?,
            focus: cx.focus_handle(),
        })
    }

    pub fn checked(&self) -> bool {
        self.checked
    }

    pub fn set_checked(&mut self, checked: bool, cx: &mut Context<Self>) {
        if self.checked != checked {
            self.checked = checked;
            cx.notify();
        }
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled != enabled {
            self.enabled = enabled;
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
        div()
            .id(self.id.clone())
            .debug_selector(|| self.id.to_string())
            .flex()
            .h(px(20.0))
            .px(px(3.0))
            .gap(px(5.0))
            .items_center()
            .rounded(px(2.0))
            .bg(rgb(self.style.surface))
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
            })
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
            )
    }
}

#[derive(Debug, Clone)]
pub struct SelectorChanged {
    pub index: usize,
    pub label: SharedString,
}

/// Compact choice field with a bounded inline popup.
pub struct Selector {
    id: SharedString,
    choices: Vec<SharedString>,
    selected: usize,
    open: bool,
    enabled: bool,
    style: ControlStyle,
    focus: FocusHandle,
    focus_blur: Option<gpui::Subscription>,
}

impl Selector {
    pub fn new(
        cx: &mut Context<Self>,
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
            open: false,
            enabled: true,
            style: style.validate()?,
            focus: cx.focus_handle(),
            focus_blur: None,
        })
    }

    pub fn selected(&self) -> usize {
        self.selected
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

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.open = false;
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
        let selected = self.choices[self.selected].clone();
        let mut root = div()
            .id(self.id.clone())
            .flex()
            .flex_col()
            .min_w_0()
            .track_focus(&self.focus)
            .border_1()
            .border_color(rgb(if self.focus.is_focused(window) {
                self.style.accent
            } else {
                self.style.surface
            }))
            .child(
                div()
                    .flex()
                    .h(px(20.0))
                    .px(px(4.0))
                    .gap(px(4.0))
                    .items_center()
                    .border_1()
                    .border_color(rgb(self.style.border))
                    .bg(rgb(self.style.raised))
                    .text_size(px(self.style.font_size))
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
        if self.open && self.enabled {
            let options = self.choices.clone();
            let current = self.selected;
            for (index, choice) in options.into_iter().enumerate() {
                let listener = cx.listener(move |selector, _: &MouseDownEvent, window, cx| {
                    selector.choose(index, window, cx)
                });
                root = root.child(
                    div()
                        .id(SharedString::from(format!("{}-option-{index}", self.id)))
                        .debug_selector(|| format!("{}-option-{index}", self.id))
                        .flex()
                        .h(px(19.0))
                        .px(px(4.0))
                        .items_center()
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
        }
        root.debug_selector(|| self.id.to_string())
            .on_key_down(cx.listener(Self::key_down))
    }
}
