//! Functional view-local fixtures; no engine, project, audio or settings I/O.
#[path = "compositions.rs"]
mod compositions;
mod eq;
#[cfg(all(test, feature = "ui-test-support"))]
#[path = "tests.rs"]
mod tests;

use crate::{device_composition, theme};
use gpui::{
    Context, Div, Entity, FocusHandle, Focusable, KeyDownEvent, Render, Window, div, prelude::*,
    px, rgb,
};
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlPresentation, NumericScale, NumericSpec,
    ResponseGraph, ResponseGraphModel, Selector, SelectorAppearance, SelectorConfig, Toggle,
    ToggleAppearance, ToggleConfig,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Entry {
    Vector,
    Utility,
    Saturator,
    Eq8,
    Eq,
    Compressor,
    Gate,
    AutoPan,
    Instrument,
    Effect,
    Unsupported,
    Parts,
}
const ENTRIES: [Entry; 12] = [
    Entry::Vector,
    Entry::Utility,
    Entry::Saturator,
    Entry::Eq8,
    Entry::Eq,
    Entry::Compressor,
    Entry::Gate,
    Entry::AutoPan,
    Entry::Instrument,
    Entry::Effect,
    Entry::Unsupported,
    Entry::Parts,
];
impl Entry {
    fn label(self) -> &'static str {
        match self {
            Self::Vector => "vector / operator",
            Self::Utility => "utility",
            Self::Saturator => "saturator",
            Self::Eq8 => "eq8",
            Self::Eq => "eq",
            Self::Compressor => "compressor",
            Self::Gate => "gate",
            Self::AutoPan => "auto pan",
            Self::Instrument => "generic instrument",
            Self::Effect => "generic effect",
            Self::Unsupported => "unsupported state",
            Self::Parts => "shared parts",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Vector => "vector",
            Self::Utility => "utility",
            Self::Saturator => "saturator",
            Self::Eq8 => "eq8",
            Self::Eq => "eq",
            Self::Compressor => "compressor",
            Self::Gate => "gate",
            Self::AutoPan => "auto-pan",
            Self::Instrument => "instrument",
            Self::Effect => "effect",
            Self::Unsupported => "unsupported",
            Self::Parts => "parts",
        }
    }
    fn width(self) -> f32 {
        match self {
            Self::Vector => device_composition::VECTOR_WIDTH,
            Self::Utility => device_composition::UTILITY_FRAME_WIDTH,
            Self::AutoPan => device_composition::AUTO_PAN_FRAME_WIDTH,
            Self::Saturator => device_composition::SATURATOR_FRAME_WIDTH,
            Self::Eq8 | Self::Eq => device_composition::EQ8_FRAME_WIDTH,
            Self::Compressor => device_composition::COMPRESSOR_FRAME_WIDTH,
            Self::Gate => device_composition::GATE_FRAME_WIDTH,
            _ => device_composition::GENERIC_FRAME_WIDTH,
        }
    }
    fn is_effect(self) -> bool {
        matches!(
            self,
            Self::Utility
                | Self::Saturator
                | Self::Eq8
                | Self::Eq
                | Self::Compressor
                | Self::Gate
                | Self::AutoPan
                | Self::Effect
                | Self::Unsupported
        )
    }
    fn counts(self) -> (usize, usize, usize) {
        match self {
            Self::Vector => (26, 4, 1),
            Self::Utility => (4, 6, 1),
            Self::Saturator => (3, 0, 1),
            Self::Eq8 => (4, 8, 9),
            Self::Eq => (4, 9, 3),
            Self::Compressor => (6, 0, 0),
            Self::Gate => (6, 0, 0),
            Self::AutoPan => (3, 0, 0),
            Self::Unsupported => (0, 0, 0),
            Self::Parts => (3, 1, 1),
            _ => (3, 1, 1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Track,
    Return,
}
impl Owner {
    fn label(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Return => "return",
        }
    }
}

struct Number {
    entity: Entity<NumericControl>,
    role: theme::DeviceStyleRole,
    width: f32,
    height: f32,
}
struct Fixture {
    entry: Entry,
    _subscriptions: Vec<gpui::Subscription>,
    numbers: Vec<Number>,
    toggles: Vec<Entity<Toggle>>,
    selectors: Vec<Entity<Selector>>,
    graph: Entity<ResponseGraph>,
    eq_local: Option<eq::Local>,
    discrete_height: f32,
}
impl Fixture {
    fn new(entry: Entry, cx: &mut Context<Gallery>) -> Result<Self, String> {
        if entry == Entry::Eq {
            return eq::fixture(cx);
        }
        let (numbers, toggles, selectors) = entry.counts();
        let generic = matches!(entry, Entry::Instrument | Entry::Effect);
        let mut numeric = Vec::new();
        for index in 0..numbers {
            let (appearance, width, height) = match entry {
                Entry::Vector if index >= 24 => {
                    (NumericControlPresentation::InlineValue, 46.0, 14.0)
                }
                Entry::Vector | Entry::Eq8 => (
                    NumericControlPresentation::MicroDial,
                    hex_widgets::MICRO_DIAL_WIDTH,
                    hex_widgets::MICRO_DIAL_HEIGHT,
                ),
                Entry::Compressor if index >= 3 => {
                    (NumericControlPresentation::InlineValue, 46.0, 14.0)
                }
                Entry::Utility if index == 3 => {
                    (NumericControlPresentation::InlineValue, 46.0, 14.0)
                }
                Entry::Instrument | Entry::Effect => {
                    (NumericControlPresentation::HorizontalSlider, 136.0, 26.0)
                }
                _ => (NumericControlPresentation::CompactDial, 64.0, 52.0),
            };
            let role = if appearance == NumericControlPresentation::InlineValue {
                theme::DeviceStyleRole::Readout
            } else {
                theme::DeviceStyleRole::Control
            };
            let spec = NumericSpec::new(0.0, 100.0, 50.0, NumericScale::Linear, 2, Some("%"))
                .map_err(|error| error.to_string())?;
            let config = NumericControlConfig::new(
                format!("fixture-{}-numeric-{index}", entry.key()),
                theme::text(["amount", "fine", "level"][index % 3]),
                appearance,
                spec,
                50.0,
                theme::device_style(role),
            )
            .map_err(|error| error.to_string())?;
            let entity = cx.new(|cx| {
                let mut control = NumericControl::from_config(cx, config);
                control.set_tab_navigation(true, cx);
                control
            });
            numeric.push(Number {
                entity,
                role,
                width,
                height,
            });
        }
        let mut switches = Vec::new();
        for index in 0..toggles {
            let config = ToggleConfig::new(
                format!("fixture-{}-toggle-{index}", entry.key()),
                theme::text("on"),
                true,
                theme::device_style(theme::DeviceStyleRole::Control),
            )
            .map_err(str::to_owned)?
            .with_appearance(if generic {
                ToggleAppearance::Standard
            } else {
                ToggleAppearance::CompactButton
            });
            switches.push(cx.new(|cx| {
                let mut control = Toggle::from_config(cx, config);
                control.set_tab_navigation(true, cx);
                control
            }));
        }
        let mut choices = Vec::new();
        for index in 0..selectors {
            let config = SelectorConfig::new(
                format!("fixture-{}-selector-{index}", entry.key()),
                ["sine", "saw", "square", "triangle", "long fixture option"]
                    .map(theme::text)
                    .into_iter()
                    // Keep the compact menu taller than the clipped device body too.
                    .chain((0..8).map(|index| theme::text(format!("extra choice {index}")))),
                0,
                theme::device_style(theme::DeviceStyleRole::Control),
            )
            .map_err(str::to_owned)?
            .with_appearance(if generic {
                SelectorAppearance::Standard
            } else {
                SelectorAppearance::CompactSelector
            });
            choices.push(cx.new(|cx| {
                let mut control = Selector::from_config(cx, config);
                control.set_tab_navigation(true, cx);
                control
            }));
        }
        let graph = cx.new(|cx| {
            ResponseGraph::new(
                cx,
                format!("fixture-{}-graph", entry.key()),
                theme::device_style(theme::DeviceStyleRole::Display),
            )
        });
        Ok(Self {
            entry,
            _subscriptions: Vec::new(),
            numbers: numeric,
            toggles: switches,
            selectors: choices,
            graph,
            eq_local: None,
            discrete_height: if generic { 22.0 } else { 14.0 },
        })
    }
    fn sync(&self, disabled: bool, cx: &mut Context<Gallery>) {
        let has_node = self.entry != Entry::Eq || eq::has_selected(self);
        for (index, number) in self.numbers.iter().enumerate() {
            number.entity.update(cx, |control, cx| {
                let _ = control.set_external_style(theme::device_style(number.role), cx);
                control.set_enabled(!disabled && (has_node || index == 0), cx);
            });
        }
        for (index, entity) in self.toggles.iter().enumerate() {
            entity.update(cx, |control, cx| {
                let _ = control
                    .set_external_style(theme::device_style(theme::DeviceStyleRole::Control), cx);
                control.set_enabled(!disabled && (has_node || index != 0), cx);
            });
        }
        for (index, entity) in self.selectors.iter().enumerate() {
            entity.update(cx, |control, cx| {
                let _ = control
                    .set_external_style(theme::device_style(theme::DeviceStyleRole::Control), cx);
                control.set_enabled(!disabled && (has_node || index == 0), cx);
            });
        }
        if self.entry == Entry::Eq {
            eq::sync(self, disabled, cx);
            return;
        }
        self.graph.update(cx, |graph, cx| {
            let _ = graph.set_model(
                ResponseGraphModel {
                    primary: vec![
                        [0.0, 0.1],
                        [0.25, 0.4],
                        [0.5, 0.75],
                        [0.75, 0.9],
                        [1.0, 0.95],
                    ],
                    secondary: vec![
                        [0.0, 0.0],
                        [0.25, 0.25],
                        [0.5, 0.5],
                        [0.75, 0.75],
                        [1.0, 1.0],
                    ],
                    analyzer: None,
                    input_analyzer: None,
                    analyzer_peak: None,
                    nodes: Vec::new(),
                },
                theme::device_style(theme::DeviceStyleRole::Display),
                cx,
            );
        });
    }
    fn number(&self, index: usize) -> Div {
        self.numbers.get(index).map_or_else(div, |number| {
            div()
                .w(px(number.width))
                .h(px(number.height))
                .flex_shrink_0()
                .child(number.entity.clone())
        })
    }
    fn toggle(&self, index: usize, width: f32) -> Div {
        div()
            .w(px(width))
            .h(px(self.discrete_height))
            .flex_shrink_0()
            .children(self.toggles.get(index).cloned())
    }
    fn selector(&self, index: usize, width: f32) -> Div {
        div()
            .w(px(width))
            .h(px(self.discrete_height))
            .flex_shrink_0()
            .children(self.selectors.get(index).cloned())
    }
}

pub(super) struct Gallery {
    entry: Entry,
    owner: Owner,
    selected: bool,
    disabled: bool,
    bypassed: bool,
    alternate: bool,
    active_tab: usize,
    section: Option<usize>,
    section_focus_subscriptions: Vec<gpui::Subscription>,
    header_focus: FocusHandle,
    page_focus: [FocusHandle; 2],
    fixtures: HashMap<Entry, Fixture>,
    error: Option<String>,
}
impl Gallery {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            entry: Entry::Vector,
            owner: Owner::Track,
            selected: false,
            disabled: false,
            bypassed: false,
            alternate: false,
            active_tab: 0,
            section: None,
            section_focus_subscriptions: Vec::new(),
            header_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            page_focus: std::array::from_fn(|_| cx.focus_handle().tab_index(0).tab_stop(true)),
            fixtures: HashMap::new(),
            error: None,
        };
        view.ensure_fixture(cx);
        view
    }
    pub(super) fn focus_first(&self, window: &mut Window, _: &mut Context<Self>) {
        self.header_focus.focus(window);
    }
    fn ensure_fixture(&mut self, cx: &mut Context<Self>) {
        if self.fixtures.contains_key(&self.entry) {
            return;
        }
        match Fixture::new(self.entry, cx) {
            Ok(fixture) => {
                self.fixtures.insert(self.entry, fixture);
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn observe_vector_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.entry != Entry::Vector || !self.section_focus_subscriptions.is_empty() {
            return;
        }
        let Some(fixture) = self.fixtures.get(&Entry::Vector) else {
            return;
        };
        let mut handles = Vec::new();
        for (index, number) in fixture.numbers.iter().enumerate() {
            handles.push((
                number.entity.focus_handle(cx),
                if index < 24 { index / 3 } else { 8 },
            ));
        }
        for (index, toggle) in fixture.toggles.iter().enumerate() {
            handles.push((toggle.focus_handle(cx), index));
        }
        for handle in &self.page_focus {
            handles.push((handle.clone(), 8));
        }
        for (handle, section) in handles {
            self.section_focus_subscriptions.push(cx.on_focus(
                &handle,
                window,
                move |view, window, cx| {
                    if view.entry == Entry::Vector && !view.disabled {
                        view.section = Some(section);
                        // Match production: focus events arrive after paint.
                        cx.defer_in(window, |_, _, cx| cx.notify());
                    }
                },
            ));
        }
    }

    fn finish_drafts(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(fixture) = self.fixtures.get(&self.entry) {
            let mut prepared = Vec::new();
            for (index, number) in fixture.numbers.iter().enumerate() {
                match number
                    .entity
                    .update(cx, |control, cx| control.prepare_finish_edit_for_owner(cx))
                {
                    Ok(Some(value)) => prepared.push((index, number.entity.clone(), value)),
                    Ok(None) => {}
                    Err(error) => {
                        self.error = Some(format!("finish the fixture value · {error}"));
                        return false;
                    }
                }
            }
            if self.entry == Entry::Eq && !eq::can_finish_numeric(fixture, prepared.len()) {
                self.error = Some("too many pending eq edits; try again".into());
                return false;
            }
            for (index, entity, value) in prepared {
                if self.entry == Entry::Eq {
                    if let Some(fixture) = self.fixtures.get_mut(&Entry::Eq) {
                        eq::finish_numeric(
                            fixture,
                            index,
                            value.previous,
                            value.value,
                            value.gesture.clone(),
                            cx,
                        );
                    }
                }
                entity.update(cx, |control, cx| {
                    control.accept_finish_edit_for_owner(value, cx)
                });
            }
        }
        self.error = None;
        true
    }
    fn choose_entry(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if !self.finish_drafts(cx) {
            cx.notify();
            return;
        }
        if self.entry == Entry::Eq && entry != Entry::Eq {
            if let Some(fixture) = self.fixtures.get(&Entry::Eq) {
                fixture.graph.update(cx, |graph, cx| graph.cancel(cx));
            }
        }
        self.entry = entry;
        self.selected = false;
        self.disabled = false;
        self.bypassed = false;
        self.active_tab = 0;
        self.section = None;
        self.ensure_fixture(cx);
        self.focus_first(window, cx);
        cx.notify();
    }
    fn set_alternate(&mut self, alternate: bool, cx: &mut Context<Self>) {
        let palette = if alternate {
            alternate_palette()
        } else {
            hex_preferences::ThemePreferences::default()
        };
        if theme::colors_are_legible(palette) {
            self.alternate = alternate;
            theme::apply_preferences(palette);
        }
        cx.notify();
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.entry == Entry::Eq
            && !self.disabled
            && event.keystroke.key == "z"
            && event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.control
        {
            if self.finish_drafts(cx) {
                if let Some(fixture) = self.fixtures.get_mut(&Entry::Eq) {
                    if !eq::defer_history(fixture, event.keystroke.modifiers.shift)
                        && let Err(error) =
                            eq::history(fixture, event.keystroke.modifiers.shift, cx)
                    {
                        self.error = Some(error);
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        match event.keystroke.key.as_str() {
            "tab" => {
                if self.finish_drafts(cx) {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev();
                    } else {
                        window.focus_next();
                    }
                }
            }
            "escape" => self.selected = false,
            "enter" | "space" if self.header_focus.is_focused(window) && !self.disabled => {
                self.selected = true
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    fn button(&self, id: &'static str, label: &str) -> gpui::Stateful<Div> {
        hex_widgets::device_action(theme::text(label), theme::device_segment_style(false))
            .id(id)
            .debug_selector(move || id.into())
            .w(px(112.0))
            .h(px(22.0))
            .cursor_pointer()
    }
}
fn alternate_palette() -> hex_preferences::ThemePreferences {
    hex_preferences::ThemePreferences {
        background: hex_preferences::RgbColor::new(225, 229, 238),
        text: hex_preferences::RgbColor::new(24, 27, 34),
        primary: hex_preferences::RgbColor::new(145, 25, 45),
        secondary: hex_preferences::RgbColor::new(16, 70, 145),
    }
}
impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_fixture(cx);
        self.observe_vector_focus(window, cx);
        let mut navigation = div()
            .w(px(154.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(3.0));
        for entry in ENTRIES {
            navigation = navigation.child(
                hex_widgets::device_segment(
                    theme::text(entry.label()),
                    theme::device_segment_style(entry == self.entry),
                )
                .id(entry.key())
                .debug_selector(move || entry.key().into())
                .w_full()
                .h(px(24.0))
                .cursor_pointer()
                .on_click(
                    cx.listener(move |view, _, window, cx| view.choose_entry(entry, window, cx)),
                ),
            );
        }
        let sample = self.sample(window, cx);
        let controls = div()
            .flex()
            .flex_wrap()
            .gap(px(4.0))
            .child(
                self.button(
                    "select-sample",
                    if self.selected {
                        "clear selection"
                    } else {
                        "select device"
                    },
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    if !view.disabled {
                        view.selected = !view.selected;
                        cx.notify();
                    }
                })),
            )
            .child(
                self.button(
                    "disable-sample",
                    if self.disabled {
                        "enable controls"
                    } else {
                        "disable controls"
                    },
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.disabled = !view.disabled;
                    cx.notify();
                })),
            )
            .child(
                self.button(
                    "bypass-sample",
                    if self.bypassed {
                        "enable fixture"
                    } else {
                        "bypass fixture"
                    },
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    if !view.disabled {
                        view.bypassed = !view.bypassed;
                        cx.notify();
                    }
                })),
            )
            .child(
                self.button(
                    "theme-sample",
                    if self.alternate {
                        "default theme"
                    } else {
                        "alternate theme"
                    },
                )
                .on_click(cx.listener(|view, _, _, cx| view.set_alternate(!view.alternate, cx))),
            )
            .when(self.entry.is_effect(), |controls| {
                controls.child(self.button("owner-sample", self.owner.label()).on_click(
                    cx.listener(|view, _, _, cx| {
                        view.owner = if view.owner == Owner::Track {
                            Owner::Return
                        } else {
                            Owner::Track
                        };
                        cx.notify();
                    }),
                ))
            });
        let status = self.error.clone().unwrap_or_else(|| {
            format!(
                "{} fixture · {} · {}",
                self.entry.label(),
                self.owner.label(),
                if self.disabled {
                    "disabled"
                } else if self.bypassed {
                    "bypassed"
                } else {
                    "enabled"
                }
            )
        });
        div().id("layout-review").size_full().flex().flex_col().p(px(8.0)).gap(px(8.0))
            .bg(rgb(theme::color(theme::PaletteRole::Background))).font_family(theme::FONT_FAMILY)
            .text_size(px(10.0)).text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .on_key_down(cx.listener(Self::key_down))
            .child(div().pl(px(theme::TITLEBAR_LEADING_INSET)).h(px(22.0)).child(theme::text("hex · native layout review · fixture only")))
            .child(div().flex().flex_1().min_h_0().gap(px(10.0)).child(navigation)
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(8.0))
                    .child(theme::text(status)).child(controls)
                    .child(div().id("sample-viewport").debug_selector(|| "sample-viewport".into()).w_full().min_w_0().h(px(208.0)).flex_shrink_0().overflow_x_scroll().child(sample))
                    .child(theme::text("local controls · tab / shift-tab focus · arrows edit · escape cancels"))
                    .child(theme::text("selection paints only the header · click any card area to select"))
                    .child(theme::text("type a long value to inspect readout clipping; selector choices open a real popup"))))
            .child(theme::text("fixture only · functional view-local controls · static normalized graphs · no dsp, audio, project or settings i/o"))
    }
}
