//! One native library review surface, with bounded lazy local fixtures.
#[path = "catalog.rs"]
mod catalog;
#[path = "sample.rs"]
mod sample;
#[cfg(all(test, feature = "ui-test-support"))]
#[path = "sandbox_tests.rs"]
mod tests;
use crate::{gallery, theme};
use catalog::{CATEGORIES, Demo, Item, inventory};
use gpui::{Context, Entity, FocusHandle, KeyDownEvent, Render, Window, div, prelude::*, px, rgb};
use sample::Sample;
use std::collections::HashMap;

pub(super) struct Sandbox {
    items: Vec<Item>,
    selected: usize,
    category: &'static str,
    samples: HashMap<usize, Entity<Sample>>,
    compositions: Entity<gallery::Gallery>,
    focus: FocusHandle,
    alternate: bool,
    error: Option<String>,
}
impl Sandbox {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let mut s = Self {
            items: inventory(),
            selected: 0,
            category: "numeric",
            samples: HashMap::new(),
            compositions: cx.new(gallery::Gallery::new),
            focus: cx.focus_handle().tab_index(0).tab_stop(true),
            alternate: false,
            error: None,
        };
        s.ensure(cx);
        s
    }
    pub(super) fn focus_first(&self, window: &mut Window, _: &mut Context<Self>) {
        self.focus.focus(window);
    }
    fn ensure(&mut self, cx: &mut Context<Self>) {
        if self.samples.contains_key(&self.selected)
            || matches!(self.items[self.selected].demo, Demo::Compositions)
        {
            return;
        }
        let item = self.items[self.selected];
        match Sample::prepare(item) {
            Ok(prepared) => {
                self.samples
                    .insert(self.selected, cx.new(|cx| Sample::new(prepared, cx)));
            }
            Err(e) => self.error = Some(e),
        }
    }
    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.items.len() {
            return;
        }
        if let Some(s) = self.samples.get(&self.selected) {
            s.update(cx, |s, cx| s.cancel(cx));
        }
        self.selected = index;
        self.category = self.items[index].category;
        self.ensure(cx);
        if let Some(sample) = self.samples.get(&self.selected) {
            if let Err(e) = sample.update(cx, |s, cx| s.restyle(cx)) {
                self.error = Some(e);
            }
        }
        self.focus.focus(window);
        cx.notify();
    }
    fn theme(&mut self, cx: &mut Context<Self>) {
        self.alternate = theme::preferences() == hex_preferences::ThemePreferences::default();
        theme::apply_preferences(if self.alternate {
            hex_preferences::ThemePreferences {
                background: hex_preferences::RgbColor::new(225, 229, 238),
                text: hex_preferences::RgbColor::new(24, 27, 34),
                primary: hex_preferences::RgbColor::new(145, 25, 45),
                secondary: hex_preferences::RgbColor::new(16, 70, 145),
            }
        } else {
            hex_preferences::ThemePreferences::default()
        });
        for sample in self.samples.values() {
            if let Err(e) = sample.update(cx, |s, cx| s.restyle(cx)) {
                self.error = Some(e);
            }
        }
        cx.notify();
    }
    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "tab" => {
                if e.keystroke.modifiers.shift {
                    window.focus_prev();
                } else {
                    window.focus_next();
                }
            }
            "up" | "down" if self.focus.is_focused(window) => {
                let indices: Vec<_> = self
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| i.category == self.category)
                    .map(|(i, _)| i)
                    .collect();
                if let Some(position) = indices.iter().position(|i| *i == self.selected) {
                    let next = if e.keystroke.key == "up" {
                        position.saturating_sub(1)
                    } else {
                        (position + 1).min(indices.len().saturating_sub(1))
                    };
                    if let Some(i) = indices.get(next) {
                        self.choose(*i, window, cx);
                    }
                }
            }
            "enter" if self.focus.is_focused(window) => {
                if let Some(s) = self.samples.get(&self.selected) {
                    s.update(cx, |s, cx| s.focus_first(window, cx));
                }
            }
            _ => {}
        }
    }
}
impl Render for Sandbox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure(cx);
        let style = theme::control_style();
        let mut categories = div().flex().flex_wrap().gap(px(4.0));
        for category in CATEGORIES {
            categories = categories.child(
                hex_widgets::action(
                    theme::text(category),
                    theme::device_segment_style(category == self.category),
                )
                .id(category)
                .debug_selector(move || category.into())
                .on_click(cx.listener(move |s, _, window, cx| {
                    if let Some(i) = s.items.iter().position(|i| i.category == category) {
                        s.choose(i, window, cx);
                    }
                })),
            );
        }
        let mut nav = div()
            .id("widget-inventory")
            .debug_selector(|| "widget-inventory".into())
            .w(px(236.0))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.0));
        for (index, item) in self
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.category == self.category)
        {
            nav = nav.child(
                hex_widgets::action(
                    theme::text(item.name),
                    theme::device_segment_style(index == self.selected),
                )
                .id(index)
                .debug_selector(move || format!("inventory-{index}"))
                .w_full()
                .min_h(px(26.0))
                .flex_shrink_0()
                .justify_start()
                .on_click(cx.listener(move |s, _, window, cx| s.choose(index, window, cx))),
            );
        }
        let mut tools = div().flex().flex_wrap().gap(px(4.0)).child(
            hex_widgets::action(
                theme::text(if self.alternate {
                    "dark theme"
                } else {
                    "light theme"
                }),
                theme::action_style(),
            )
            .id("sandbox-theme")
            .debug_selector(|| "sandbox-theme".into())
            .on_click(cx.listener(|s, _, _, cx| s.theme(cx))),
        );
        if let Some(sample) = self.samples.get(&self.selected).cloned() {
            let enabled = sample.read(cx).enabled;
            tools = tools.child(
                hex_widgets::action(
                    theme::text(if enabled { "disable" } else { "enable" }),
                    theme::action_style(),
                )
                .id("sandbox-enable")
                .debug_selector(|| "sandbox-enable".into())
                .on_click(cx.listener(move |_, _, _, cx| {
                    sample.update(cx, |s, cx| s.set_enabled(!s.enabled, cx))
                })),
            );
            if let Some(sample) = self.samples.get(&self.selected).cloned() {
                tools = tools.child(
                    hex_widgets::action(theme::text("narrow / wide"), theme::action_style())
                        .id("sandbox-width")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            sample.update(cx, |s, cx| {
                                s.narrow = !s.narrow;
                                cx.notify();
                            })
                        })),
                );
            }
            tools = tools.child(
                hex_widgets::action(theme::text("reset sample"), theme::action_style())
                    .id("sandbox-reset")
                    .on_click(cx.listener(|s, _, _, cx| {
                        if let Some(sample) = s.samples.remove(&s.selected) {
                            sample.update(cx, |s, cx| s.cancel(cx));
                        }
                        s.ensure(cx);
                        cx.notify();
                    })),
            );
        }
        let preview = div()
            .id("sandbox-content")
            .debug_selector(|| "sandbox-content".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .overflow_x_scroll()
            .when(
                matches!(self.items[self.selected].demo, Demo::Compositions),
                |v| {
                    v.child(
                        div()
                            .min_w(px(760.0))
                            .h(px(600.0))
                            .child(self.compositions.clone()),
                    )
                },
            )
            .children(
                self.samples
                    .get(&self.selected)
                    .filter(|_| !matches!(self.items[self.selected].demo, Demo::Compositions))
                    .cloned(),
            );
        div().size_full().flex().flex_col().gap(px(6.0)).p(px(6.0)).bg(rgb(style.surface)).text_color(rgb(style.text)).font_family(theme::FONT_FAMILY).text_size(px(10.0)).track_focus(&self.focus).on_key_down(cx.listener(Self::key))
            .child(div().pl(px(theme::TITLEBAR_LEADING_INSET)).h(px(22.0)).child(theme::text(format!("hex · native widget sandbox · {} named entries",self.items.len()))))
            .child(categories).child(tools)
            .child(div().flex().flex_1().min_h_0().gap(px(8.0)).child(nav).child(preview))
            .children(self.error.as_ref().map(|e|div().child(theme::text(e))))
            .child(theme::text("fixture only · no audio, project, file or settings i/o · inventory: up/down, enter to focus sample"))
    }
}
