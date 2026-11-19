//! Native library catalog: control families contain pages per visual kind and live configurations.
//! Fixtures are created lazily per page and retain local values across navigation.
//! Device compositions retain their own bounded fixture navigation.
#[path = "catalog.rs"]
mod catalog;
#[path = "sample.rs"]
mod sample;
#[cfg(all(test, feature = "ui-test-support"))]
#[path = "sandbox_tests.rs"]
mod tests;
#[path = "workspace.rs"]
mod workspace;
use crate::{gallery, theme};
use catalog::{CATEGORIES, Demo, Item, Page, inventory, pages};
use gpui::{
    Context, Div, Entity, FocusHandle, KeyDownEvent, Render, Window, div, prelude::*, px, rgb,
};
use sample::Sample;
use std::collections::HashMap;

pub(super) struct Sandbox {
    items: Vec<Item>,
    pages: Vec<Page>,
    selected: usize,
    category: &'static str,
    samples: HashMap<usize, Entity<Sample>>,
    compositions: Entity<gallery::Gallery>,
    scroll: gpui::ScrollHandle,
    focus: FocusHandle,
    error: Option<String>,
}
impl Sandbox {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let items = inventory();
        let mut s = Self {
            pages: pages(&items),
            items,
            selected: 0,
            category: "numeric",
            samples: HashMap::new(),
            compositions: cx.new(gallery::Gallery::new),
            scroll: gpui::ScrollHandle::new(),
            focus: cx.focus_handle().tab_index(0).tab_stop(true),
            error: None,
        };
        s.ensure(cx);
        if let Some(index) = s.configurations().first() {
            s.selected = *index;
        }
        s
    }
    pub(super) fn focus_first(&self, window: &mut Window, _: &mut Context<Self>) {
        self.focus.focus(window);
    }
    fn page(&self) -> Page {
        self.items[self.selected].page()
    }
    fn configurations(&self) -> Vec<usize> {
        let page = self.page();
        let mut indices: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.page() == page)
            .map(|(index, _)| index)
            .collect();
        // Stable ordering keeps named appearances together within each kind.
        indices.sort_by_key(|index| self.items[*index].subgroup());
        indices
    }
    fn ensure(&mut self, cx: &mut Context<Self>) {
        for index in self.configurations() {
            if self.samples.contains_key(&index)
                || matches!(self.items[index].demo, Demo::Compositions)
            {
                continue;
            }
            match Sample::prepare_scoped(self.items[index], &format!("sample-{index}-")) {
                Ok(prepared) => {
                    self.samples
                        .insert(index, cx.new(|cx| Sample::new(prepared, cx)));
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.items.len() {
            return;
        }
        for current in self.configurations() {
            if let Some(sample) = self.samples.get(&current) {
                sample.update(cx, |sample, cx| sample.cancel(cx));
            }
        }
        self.selected = index;
        self.category = self.page().category;
        self.ensure(cx);
        let indices = self.configurations();
        for current in &indices {
            if let Some(sample) = self.samples.get(current)
                && let Err(error) = sample.update(cx, |sample, cx| sample.restyle(cx))
            {
                self.error = Some(error);
            }
        }
        if let Some(position) = indices.iter().position(|current| *current == index) {
            self.scroll.scroll_to_top_of_item(position);
        }
        self.focus.focus(window);
        cx.notify();
    }
    fn theme(&mut self, cx: &mut Context<Self>) {
        let light = theme::preferences() == hex_preferences::ThemePreferences::default();
        theme::apply_preferences(if light {
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
            if let Err(error) = sample.update(cx, |sample, cx| sample.restyle(cx)) {
                self.error = Some(error);
            }
        }
        cx.notify();
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "tab" => {
                if event.keystroke.modifiers.shift {
                    window.focus_prev();
                } else {
                    window.focus_next();
                }
            }
            "up" | "down" if self.focus.is_focused(window) => {
                let pages: Vec<_> = self
                    .pages
                    .iter()
                    .copied()
                    .filter(|page| page.category == self.category)
                    .collect();
                if let Some(position) = pages.iter().position(|page| *page == self.page()) {
                    let next = if event.keystroke.key == "up" {
                        position.saturating_sub(1)
                    } else {
                        (position + 1).min(pages.len().saturating_sub(1))
                    };
                    if let Some(index) = pages
                        .get(next)
                        .and_then(|page| self.items.iter().position(|item| item.page() == *page))
                    {
                        self.choose(index, window, cx);
                    }
                }
            }
            "enter" if self.focus.is_focused(window) => {
                if let Some(sample) = self.samples.get(&self.selected) {
                    sample.update(cx, |sample, cx| sample.focus_first(window, cx));
                }
            }
            _ => {}
        }
    }
    fn sample_tools(&self, index: usize, cx: &mut Context<Self>) -> Div {
        let mut tools = div().flex().flex_wrap().gap(px(4.0));
        if let Some(sample) = self.samples.get(&index).cloned() {
            let enabled = sample.read(cx).enabled;
            tools = tools.child(
                hex_widgets::toggle_action(
                    theme::text(if enabled { "disable" } else { "enable" }),
                    theme::action_style(),
                )
                .px(px(4.0))
                .py(px(2.0))
                .id(("sample-enable", index))
                .debug_selector(move || format!("sample-{index}-enable"))
                .on_click(cx.listener(move |_, _, _, cx| {
                    sample.update(cx, |sample, cx| sample.set_enabled(!sample.enabled, cx));
                })),
            );
        }
        if let Some(sample) = self.samples.get(&index).cloned() {
            tools = tools.child(
                hex_widgets::toggle_action(theme::text("narrow / wide"), theme::action_style())
                    .px(px(4.0))
                    .py(px(2.0))
                    .id(("sample-width", index))
                    .debug_selector(move || format!("sample-{index}-width"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        sample.update(cx, |sample, cx| {
                            sample.narrow = !sample.narrow;
                            cx.notify();
                        });
                    })),
            );
        }
        tools.child(
            hex_widgets::action(theme::text("reset sample"), theme::action_style())
                .px(px(4.0))
                .py(px(2.0))
                .id(("sample-reset", index))
                .debug_selector(move || format!("sample-{index}-reset"))
                .on_click(cx.listener(move |s, _, _, cx| {
                    if let Some(sample) = s.samples.remove(&index) {
                        sample.update(cx, |sample, cx| sample.cancel(cx));
                    }
                    s.ensure(cx);
                    cx.notify();
                })),
        )
    }
}
impl Render for Sandbox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure(cx);
        let style = theme::control_style();
        let page = self.page();
        let mut categories = div().flex().flex_wrap().gap(px(4.0));
        for category in CATEGORIES {
            categories = categories.child(
                hex_widgets::toggle_action(
                    theme::text(category),
                    theme::device_segment_style(category == self.category),
                )
                .px(px(4.0))
                .py(px(2.0))
                .id(category)
                .debug_selector(move || category.into())
                .on_click(cx.listener(move |s, _, window, cx| {
                    if let Some(index) = s
                        .items
                        .iter()
                        .position(|item| item.page().category == category)
                    {
                        s.choose(index, window, cx);
                    }
                })),
            );
        }
        let mut nav = div()
            .id("widget-inventory")
            .debug_selector(|| "widget-inventory".into())
            .w(px(206.0))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.0));
        for (index, candidate) in self
            .pages
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, candidate)| candidate.category == self.category)
        {
            let count = self
                .items
                .iter()
                .filter(|item| item.page() == candidate)
                .count();
            nav = nav.child(
                hex_widgets::toggle_action(
                    theme::text(format!("{} · {count}", candidate.name)),
                    theme::device_segment_style(candidate == page),
                )
                .id(index)
                .debug_selector(move || format!("page-{index}"))
                .w_full()
                .h(px(22.0))
                .px(px(4.0))
                .overflow_hidden()
                .flex_shrink_0()
                .justify_start()
                .on_click(cx.listener(move |s, _, window, cx| {
                    if let Some(index) = s.items.iter().position(|item| item.page() == candidate) {
                        s.choose(index, window, cx);
                    }
                })),
            );
        }
        let indices = self.configurations();
        let mut preview = div()
            .id("sandbox-content")
            .debug_selector(|| "sandbox-content".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .overflow_x_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .gap(px(6.0));
        let mut previous_group = None;
        for index in &indices {
            let index = *index;
            let item = self.items[index];
            let subgroup = item.subgroup();
            let mut section = div()
                .id(index)
                .debug_selector(move || format!("configuration-{index}"))
                .w_full()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .pb(px(8.0))
                .border_b_1()
                .border_color(rgb(style.border));
            if previous_group != Some(subgroup) {
                section = section.child(
                    div()
                        .text_color(rgb(style.accent))
                        .child(theme::text(subgroup)),
                );
            }
            previous_group = Some(subgroup);
            if matches!(item.demo, Demo::Compositions) {
                section = section.child(
                    div()
                        .min_w(px(760.0))
                        .h(px(600.0))
                        .child(self.compositions.clone()),
                );
            } else {
                section = section
                    .child(theme::text(item.name))
                    .child(theme::text(item.configuration()))
                    .child(self.sample_tools(index, cx))
                    .children(self.samples.get(&index).cloned());
            }
            preview = preview.child(section.on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |s, _, _, cx| {
                    if s.selected != index {
                        if let Some(previous) = s.samples.get(&s.selected) {
                            previous.update(cx, |sample, cx| sample.cancel(cx));
                        }
                        s.selected = index;
                    }
                }),
            ));
        }
        let theme_button = hex_widgets::toggle_action(
            theme::text(
                if theme::preferences() == hex_preferences::ThemePreferences::default() {
                    "light theme"
                } else {
                    "dark theme"
                },
            ),
            theme::action_style(),
        )
        .px(px(4.0))
        .py(px(2.0))
        .id("sandbox-theme")
        .debug_selector(|| "sandbox-theme".into())
        .on_click(cx.listener(|s, _, _, cx| s.theme(cx)));
        div().size_full().flex().flex_col().gap(px(6.0)).p(px(6.0))
            .bg(rgb(style.surface)).text_color(rgb(style.text))
            .font_family(theme::FONT_FAMILY).text_size(px(10.0)).track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .child(div().pl(px(theme::TITLEBAR_LEADING_INSET)).h(px(22.0))
                .child(theme::text(format!("hex · native widget sandbox · {} pages · {} named configurations", self.pages.len(), self.items.len()))))
            .child(categories)
            .child(div().flex().items_center().gap(px(8.0)).child(theme_button)
                .child(theme::text(format!("{} · owner: {} · {} configurations", page.name, page.owner, indices.len()))))
            .child(div().flex().flex_1().min_h_0().gap(px(8.0)).child(nav).child(preview))
            .children(self.error.as_ref().map(|error| div().child(theme::text(error))))
            .child(theme::text("fixture only · no audio, project, file or settings i/o · pages: up/down · enter to focus selected sample"))
    }
}
