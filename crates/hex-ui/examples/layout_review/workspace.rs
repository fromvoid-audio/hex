//! Local owners for workspace primitives and the actual shared menu/meter
//! compositions. No EditorView, canonical commands or audio/file service exists.
use crate::{
    context_menu::{MenuItem, PopupMenu},
    output_meter::{self, MeterInput},
    theme,
};
use gpui::{
    Context, Div, Entity, FocusHandle, Focusable, Render, Subscription, Window, div, prelude::*,
    px, rgb,
};
use hex_widgets::*;
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MeterChannels {
    Mono,
    Stereo,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum RoundActionKind {
    Label,
    Icon,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Kind {
    Text(TextInputKind),
    Menu(bool),
    OutputMeter,
    LevelMeter,
    MeterIndicator(MeterChannels),
    RoundAction(RoundActionKind),
    Button(&'static str),
    ScrollingList,
}
pub(super) struct Prepared {
    kind: Kind,
    prefix: String,
    numbers: [NumericControlConfig; 2],
    selector: SelectorConfig,
    text: TextInputConfig,
}
pub(super) struct Workspace {
    kind: Kind,
    prefix: String,
    numbers: [Entity<NumericControl>; 2],
    selector: Entity<Selector>,
    text: Entity<TextInput>,
    focus: FocusHandle,
    accessibility_lifetime: gpui::AccessibilityLifetime,
    menu: Option<PopupMenu<usize>>,
    menu_anchor: Rc<Cell<Option<gpui::Point<gpui::Pixels>>>>,
    enabled: bool,
    active: bool,
    count: u64,
    scroll: gpui::ScrollHandle,
    status: String,
    _subscriptions: Vec<Subscription>,
}
impl Workspace {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub fn prepare(kind: Kind) -> Result<Prepared, String> {
        Self::prepare_scoped(kind, "")
    }
    pub fn prepare_scoped(kind: Kind, prefix: &str) -> Result<Prepared, String> {
        let id = |name: &str| format!("{prefix}{name}");
        let style = theme::control_style();
        let calibrated_meter = matches!(kind, Kind::MeterIndicator(_));
        let spec = if calibrated_meter {
            NumericSpec::new(-60.0, 6.0, -24.0, NumericScale::Linear, 1, Some("dbfs"))
        } else {
            NumericSpec::new(0.0, 2.0, 0.5, NumericScale::Linear, 2, None::<&str>)
        }
        .map_err(|e| e.to_string())?;
        let number = |index, label, value| {
            NumericControlConfig::new(
                id(&format!("workspace-level-{index}")),
                theme::text(label),
                NumericControlKind::HorizontalSlider,
                spec.clone(),
                value,
                style,
            )
            .map(|c| c.with_appearance(NumericControlAppearance::FilledSlider))
            .map_err(|e| e.to_string())
        };
        let text_kind = if let Kind::Text(kind) = kind {
            kind
        } else {
            TextInputKind::Value
        };
        Ok(Prepared {
            kind,
            prefix: prefix.to_owned(),
            numbers: if calibrated_meter {
                [
                    number(0, "level dbfs", -24.0)?,
                    number(1, "held peak dbfs", -12.0)?,
                ]
            } else {
                [number(0, "left", 0.25)?, number(1, "right", 0.5)?]
            },
            selector: SelectorConfig::new(
                id("workspace-meter-state"),
                if calibrated_meter {
                    vec!["signal", "silence", "invalid signal", "overload"]
                } else {
                    vec!["signal", "offline", "stopped", "fault", "invalid signal"]
                }
                .into_iter()
                .map(theme::text),
                0,
                style,
            )?,
            text: TextInputConfig {
                id: id("workspace-text").into(),
                kind: text_kind,
                style,
                action_style: theme::action_style(),
                font_family: theme::FONT_FAMILY.into(),
                placeholder: theme::text("type a local query"),
                clear_name: theme::text("clear local query"),
                clear_glyph: theme::text("×"),
                max_bytes: if text_kind == TextInputKind::Value {
                    1024
                } else {
                    65_536
                },
            },
        })
    }
    pub fn new(p: Prepared, cx: &mut Context<Self>) -> Self {
        let [left, right] = p.numbers;
        let numbers = [
            cx.new(|cx| NumericControl::from_config(cx, left)),
            cx.new(|cx| NumericControl::from_config(cx, right)),
        ];
        let selector = cx.new(|cx| Selector::from_config(cx, p.selector));
        let text = cx.new(|cx| TextInput::new(cx, p.text));
        text.update(cx, |t, cx| {
            t.set_tab_navigation(true, cx);
            if matches!(p.kind, Kind::Text(TextInputKind::Value)) {
                t.set_text("Verse S 🎹", cx);
            }
        });
        let mut subscriptions = Vec::new();
        for number in &numbers {
            number.update(cx, |n, cx| n.set_tab_navigation(true, cx));
            subscriptions
                .push(cx.subscribe(number, |_, _, _: &NumericControlEvent, cx| cx.notify()));
        }
        selector.update(cx, |s, cx| s.set_tab_navigation(true, cx));
        subscriptions.push(cx.subscribe(&selector, |s, _, e: &SelectorChanged, cx| {
            s.status = format!("local meter state · {}", e.label);
            cx.notify();
        }));
        subscriptions.push(cx.subscribe(&text, |s, text, _: &TextInputChanged, cx| {
            s.status = format!("local text · {}", text.read(cx).text());
            cx.notify();
        }));
        Self {
            kind: p.kind,
            prefix: p.prefix,
            numbers,
            selector,
            text,
            focus: cx.focus_handle(),
            accessibility_lifetime: gpui::AccessibilityLifetime::default(),
            menu: None,
            menu_anchor: Rc::new(Cell::new(None)),
            enabled: true,
            active: false,
            count: 0,
            scroll: gpui::ScrollHandle::new(),
            status: "local fixture · ready".into(),
            _subscriptions: subscriptions,
        }
    }
    fn id(&self, name: &str) -> String {
        format!("{}{name}", self.prefix)
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        for n in &self.numbers {
            n.update(cx, |n, cx| n.cancel(cx));
        }
        self.menu = None;
        cx.notify();
    }
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.cancel(cx);
        self.enabled = enabled;
        self.text.update(cx, |t, cx| t.set_enabled(enabled, cx));
        self.selector.update(cx, |s, cx| s.set_enabled(enabled, cx));
        for n in &self.numbers {
            n.update(cx, |n, cx| n.set_enabled(enabled, cx));
        }
        cx.notify();
    }
    pub fn restyle(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let style = theme::control_style();
        self.text.update(cx, |t, cx| {
            t.set_external_style(style, theme::action_style(), cx)
        })?;
        self.selector
            .update(cx, |s, cx| s.set_external_style(style, cx))?;
        for n in &self.numbers {
            n.update(cx, |n, cx| n.set_external_style(style, cx))
                .map_err(|e| e.to_string())?;
        }
        cx.notify();
        Ok(())
    }
    pub fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.kind {
            Kind::Text(_) => self.text.focus_handle(cx).focus(window),
            Kind::OutputMeter | Kind::LevelMeter | Kind::MeterIndicator(_) => {
                self.numbers[0].focus_handle(cx).focus(window)
            }
            _ => self.focus.focus(window),
        }
    }
    fn open_menu(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let grid = matches!(self.kind, Kind::Menu(true));
        let labels = if grid {
            vec!["none", "circle", "triangle", "square", "add"]
        } else {
            vec!["first choice", "second choice", "third choice"]
        };
        let items = labels
            .into_iter()
            .enumerate()
            .map(|(i, label)| MenuItem {
                label,
                swatch: (!grid).then_some([0x77b6bb, 0x809fca, 0xcf7773][i % 3]),
                checked: i == self.count as usize % if grid { 5 } else { 3 },
                action: i,
            })
            .collect();
        let focus = cx.focus_handle();
        focus.focus(window);
        self.menu = Some(PopupMenu {
            position,
            icon_grid: grid.then(|| vec!["○", "△", "□", "+"]),
            items,
            selected: 0,
            focus,
            previous_focus: Some(self.focus.clone()),
        });
        cx.notify();
    }
    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.enabled {
            self.count = index as u64;
            self.status = format!("local menu choice · {}", index + 1);
        }
        self.dismiss(window, cx);
    }
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.take() {
            if let Some(focus) = menu.previous_focus {
                focus.focus(window);
            }
        }
        cx.notify();
    }
    fn navigate(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            let len = menu.items.len();
            if len > 0 {
                menu.selected = if forward {
                    (menu.selected + 1) % len
                } else {
                    (menu.selected + len - 1) % len
                };
                cx.notify();
            }
        }
    }
    fn press(&mut self, cx: &mut Context<Self>) {
        if self.enabled {
            self.active = !self.active;
            self.count = self.count.saturating_add(1);
            self.status = format!(
                "local button · {} · {}",
                self.count,
                if self.active { "on" } else { "off" }
            );
            cx.notify();
        }
    }
    fn press_action(&mut self, cx: &mut Context<Self>) {
        if self.enabled {
            self.count = self.count.saturating_add(1);
            self.status = format!("local action · {} calls", self.count);
            cx.notify();
        }
    }
    fn indicator_readout(&self, channels: MeterChannels, cx: &gpui::App) -> MeterIndicatorReadout {
        let state = self.selector.read(cx).selected();
        let channel = |offset| MeterChannelReadout {
            level_dbfs: match state {
                1 => f32::NEG_INFINITY,
                2 => f32::NAN,
                _ => self.numbers[0].read(cx).value() as f32 + offset,
            },
            held_peak_dbfs: match state {
                1 => None,
                2 => Some(f32::INFINITY),
                _ => Some(self.numbers[1].read(cx).value() as f32 + offset),
            },
            clipping: state == 3,
        };
        match channels {
            MeterChannels::Mono => MeterIndicatorReadout::Mono(channel(0.0)),
            MeterChannels::Stereo => MeterIndicatorReadout::Stereo([channel(0.0), channel(6.0)]),
        }
    }
    fn indicator(&self, channels: MeterChannels, cx: &mut Context<Self>) -> Div {
        let style = theme::control_style();
        let readout = self.indicator_readout(channels, cx);
        let meter = match meter_indicator(
            self.id("workspace-indicator").into(),
            readout,
            METER_INDICATOR_TICKS_DBFS.map(|dbfs| theme::text(format!("{:.0}", -dbfs))),
            MeterIndicatorStyle {
                controls: style,
                signal: style.accent,
                peak: style.text,
                overload: theme::color(theme::PaletteRole::Secondary),
            },
        ) {
            Ok(meter) => div().h(px(148.0)).child(meter),
            Err(error) => div().child(theme::text(format!(
                "meter indicator unavailable · {error}"
            ))),
        };
        div().flex().flex_col().gap(px(6.0)).w_full()
            .child(meter)
            .child(div().w_full().max_w(px(220.0)).child(self.selector.clone()))
            .children(self.numbers.iter().map(|number|
                div().w_full().max_w(px(220.0)).h(px(20.0)).child(number.clone())))
            .child(theme::text(match channels {
                MeterChannels::Mono => "local dbfs level and held peak · overload is explicit · no audio",
                MeterChannels::Stereo => "local dbfs level and held peak · right channel adds 6 db · overload is explicit · no audio",
            }))
    }
    fn round_action(&self, kind: RoundActionKind, window: &Window, cx: &mut Context<Self>) -> Div {
        let colors = theme::control_style();
        let background = colors.accent;
        let hover = theme::color(theme::PaletteRole::PrimaryDark);
        let style = ActionStyle {
            background,
            text: theme::readable_text(background, colors.text),
            hover_background: hover,
            hover_text: Some(theme::readable_text(hover, colors.text)),
        };
        let name = match kind {
            RoundActionKind::Label => "run local labeled action",
            RoundActionKind::Icon => "run local icon action",
        };
        let entity = cx.entity();
        let owner = entity.downgrade();
        let button = round_action_surface(style, colors.border)
            .id("workspace-round-action")
            .debug_selector({
                let id = self.id("workspace-round-action");
                move || id.clone()
            })
            .w(px(if kind == RoundActionKind::Label {
                40.0
            } else {
                20.0
            }))
            .h(px(20.0))
            .child(theme::text(if kind == RoundActionKind::Label {
                "ext"
            } else {
                "▶"
            }))
            .track_focus(&self.focus)
            .focus(move |focus| focus.border_color(rgb(colors.focus_marker)))
            .opacity(if self.enabled { 1.0 } else { 0.4 })
            .when(!self.enabled, |button| {
                button.active(|pressed| pressed.opacity(0.4))
            })
            .tooltip(move |_, cx| cx.new(|_| LocalTooltip(theme::text(name))).into())
            .on_click(cx.listener(|view, _, window, cx| {
                if view.enabled {
                    view.focus.focus(window);
                    view.press_action(cx);
                }
            }))
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    if !event.is_held {
                        view.press_action(cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .accessibility(
                gpui::AccessibilityDescriptor {
                    id: gpui::AccessibilityId(entity.entity_id().as_u64()),
                    revision: 0,
                    lifetime: self.accessibility_lifetime.weak(),
                    name: theme::text(name),
                    value: gpui::AccessibilityValue::Button,
                    enabled: self.enabled,
                    focused: self.focus.is_focused(window),
                    status: None,
                },
                move |request, window, cx| {
                    owner
                        .update(cx, |view, cx| {
                            if !view.enabled {
                                return false;
                            }
                            match request.action {
                                gpui::AccessibilityAction::Focus => {
                                    view.focus.focus(window);
                                    true
                                }
                                gpui::AccessibilityAction::Press => {
                                    view.focus.focus(window);
                                    view.press_action(cx);
                                    true
                                }
                                _ => false,
                            }
                        })
                        .unwrap_or(false)
                },
            );
        div().child(button).child(theme::text(
            "momentary command · each click, enter or space runs once · no toggle state",
        ))
    }
    fn signal(&self, cx: &gpui::App) -> MeterInput {
        match self.selector.read(cx).selected() {
            1 => MeterInput::Offline,
            2 => MeterInput::Stopped,
            3 => MeterInput::Fault,
            4 => MeterInput::Last {
                left: f32::NAN,
                right: f32::INFINITY,
            },
            _ => MeterInput::Last {
                left: self.numbers[0].read(cx).value() as f32,
                right: self.numbers[1].read(cx).value() as f32,
            },
        }
    }
    fn body(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        match self.kind {
            Kind::Text(kind) => div()
                .w_full()
                .max_w(px(220.0))
                .child(self.text.clone())
                .child(theme::text(if kind == TextInputKind::Value {
                    "source case is preserved · copy to inspect · 1024-byte input limit"
                } else {
                    "local query · clear button keeps its slot · 65536-byte input limit"
                })),
            Kind::Menu(_) => div()
                .on_children_prepainted({
                    let anchor = self.menu_anchor.clone();
                    move |bounds, _, _| {
                        anchor.set(bounds.first().map(|b| gpui::point(b.left(), b.bottom())))
                    }
                })
                .child(
                    action(theme::text("open local menu"), theme::action_style())
                        .id("workspace-open-menu")
                        .debug_selector({
                            let id = self.id("workspace-open-menu");
                            move || id.clone()
                        })
                        .w(px(140.0))
                        .h(px(22.0))
                        .track_focus(&self.focus)
                        .border_1()
                        .border_color(rgb(theme::control_style().surface))
                        .focus(|style| style.border_color(rgb(theme::control_style().focus_marker)))
                        .opacity(if self.enabled { 1.0 } else { 0.4 })
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|s, e: &gpui::MouseDownEvent, w, cx| {
                                s.open_menu(e.position, w, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .on_key_down(cx.listener(|s, e: &gpui::KeyDownEvent, w, cx| {
                            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                                s.open_menu(
                                    s.menu_anchor.get().unwrap_or(w.mouse_position()),
                                    w,
                                    cx,
                                );
                                cx.stop_propagation();
                            }
                        })),
                ),
            Kind::OutputMeter | Kind::LevelMeter => {
                let meter = if self.kind == Kind::OutputMeter {
                    div().child(
                        div()
                            .id("workspace-meter-slot")
                            .w_full()
                            .overflow_x_scroll()
                            .child(output_meter::render(self.signal(cx))),
                    )
                } else {
                    div().flex().gap(px(8.0)).children({
                        let levels = match self.signal(cx) {
                            MeterInput::Last { left, right } => [left, right],
                            _ => [f32::NAN; 2],
                        };
                        levels.into_iter().enumerate().map(|(i, value)| {
                            level_meter(
                                self.id(&format!("workspace-gauge-{i}")).into(),
                                value,
                                value > 1.0,
                                LevelMeterStyle {
                                    background: theme::control_style().rail,
                                    fill: theme::control_style().accent,
                                    above: theme::control_style().text,
                                },
                            )
                            .w(px(12.0))
                            .h(px(100.0))
                        })
                    })
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .w_full()
                    .child(meter)
                    .child(div().w_full().max_w(px(220.0)).child(self.selector.clone()))
                    .children(
                        self.numbers
                            .iter()
                            .map(|n| div().w_full().max_w(px(220.0)).h(px(20.0)).child(n.clone())),
                    )
                    .child(theme::text(if self.kind == Kind::OutputMeter {
                        format!(
                            "local fixture · {}",
                            output_meter::description(self.signal(cx))
                        )
                    } else {
                        "local normalized gauge · over-unity values show a marker · no audio".into()
                    }))
            }
            Kind::MeterIndicator(channels) => self.indicator(channels, cx),
            Kind::RoundAction(kind) => self.round_action(kind, window, cx),
            Kind::Button(name) => {
                let glyph = match name {
                    "play" => "▶",
                    "stop" => "■",
                    "record" | "arm" => "●",
                    "mute" => "m",
                    "track" => "1",
                    "solo" => "s",
                    "fold" => {
                        if self.active {
                            "▾"
                        } else {
                            "▸"
                        }
                    }
                    _ => "+",
                };
                let style = if self.active {
                    ActionStyle {
                        background: theme::control_style().accent,
                        text: theme::control_style().background,
                        ..theme::action_style()
                    }
                } else {
                    theme::action_style()
                };
                div().child(
                    action(theme::text(glyph), style)
                        .id("workspace-button")
                        .debug_selector({
                            let id = self.id("workspace-button");
                            move || id.clone()
                        })
                        .w(px(20.0))
                        .h(px(20.0))
                        .track_focus(&self.focus)
                        .border_1()
                        .border_color(rgb(theme::control_style().surface))
                        .focus(|style| style.border_color(rgb(theme::control_style().focus_marker)))
                        .opacity(if self.enabled { 1.0 } else { 0.4 })
                        .tooltip(move |_, cx| {
                            cx.new(|_| {
                                LocalTooltip(theme::text(format!("{name} · local ui fixture")))
                            })
                            .into()
                        })
                        .on_click(cx.listener(|s, _, window, cx| {
                            if s.enabled {
                                s.focus.focus(window);
                                s.press(cx);
                            }
                        }))
                        .on_key_down(cx.listener(|s, e: &gpui::KeyDownEvent, _, cx| {
                            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                                s.press(cx);
                                cx.stop_propagation();
                            }
                        })),
                )
            }
            Kind::ScrollingList => div().child(
                div()
                    .id("workspace-scroll-list")
                    .debug_selector({
                        let id = self.id("workspace-scroll-list");
                        move || id.clone()
                    })
                    .w(px(200.0))
                    .h(px(110.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .track_focus(&self.focus)
                    .border_1()
                    .border_color(rgb(theme::control_style().surface))
                    .focus(|style| style.border_color(rgb(theme::control_style().focus_marker)))
                    .on_key_down(cx.listener(|s, e: &gpui::KeyDownEvent, _, cx| {
                        if !s.enabled {
                            return;
                        }
                        s.count = match e.keystroke.key.as_str() {
                            "up" => s.count.saturating_sub(1),
                            "down" => s.count.saturating_add(1).min(23),
                            "home" => 0,
                            "end" => 23,
                            _ => return,
                        };
                        s.scroll.scroll_to_item(s.count as usize);
                        s.status = format!("local list row · {}", s.count + 1);
                        cx.notify();
                        cx.stop_propagation();
                    }))
                    .children((0..24).map(|i| {
                        action(
                            theme::text(format!("local row {}", i + 1)),
                            if self.count == i as u64 {
                                ActionStyle {
                                    background: theme::control_style().accent,
                                    text: theme::control_style().background,
                                    ..theme::action_style()
                                }
                            } else {
                                theme::action_style()
                            },
                        )
                        .id(i)
                        .debug_selector({
                            let id = self.id(&format!("workspace-row-{i}"));
                            move || id.clone()
                        })
                        .justify_start()
                        .px(px(4.0))
                        .h(px(20.0))
                        .w_full()
                        .on_click(cx.listener(move |s, _, window, cx| {
                            if s.enabled {
                                s.focus.focus(window);
                                s.count = i as u64;
                                s.status = format!("local list row · {}", i + 1);
                                cx.notify();
                            }
                        }))
                    })),
            ),
        }
    }
}
impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let popup = self.menu.as_ref().map(|menu| {
            gpui::deferred(
                gpui::anchored()
                    .position(gpui::point(px(0.0), px(0.0)))
                    .child(
                        div()
                            .w(window.bounds().size.width)
                            .h(window.bounds().size.height)
                            .child(crate::context_menu::popup(
                                menu,
                                window,
                                cx,
                                Self::choose,
                                Self::dismiss,
                                Self::navigate,
                            )),
                    ),
            )
            .with_priority(100)
        });
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(self.body(window, cx))
            .child(theme::text(&self.status))
            .children(popup)
    }
}
struct LocalTooltip(gpui::SharedString);
impl Render for LocalTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .font_family(theme::FONT_FAMILY)
            .text_xs()
            .bg(rgb(theme::control_style().raised))
            .text_color(rgb(theme::control_style().text))
            .p(px(4.0))
            .child(self.0.clone())
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext, point};
    struct OffsetRoot(Entity<Workspace>, bool);
    impl Render for OffsetRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().h(px(70.0)).flex_shrink_0())
                .child(
                    div()
                        .id("workspace-test-parent")
                        .debug_selector(|| "workspace-test-parent".into())
                        .ml(px(160.0))
                        .w(px(220.0))
                        .h(px(if self.1 { 40.0 } else { 260.0 }))
                        .overflow_hidden()
                        .child(self.0.clone()),
                )
        }
    }
    fn fixture(cx: &mut TestAppContext, kind: Kind) -> (VisualTestContext, Entity<Workspace>) {
        cx.update(bind_numeric_control_keys);
        cx.update(bind_text_input_keys);
        let prepared = Workspace::prepare(kind).unwrap();
        let handle = cx.add_window(|window, cx| {
            window.activate_window();
            let entity = cx.new(|cx| Workspace::new(prepared, cx));
            entity.update(cx, |s, cx| s.focus_first(window, cx));
            OffsetRoot(entity, matches!(kind, Kind::Menu(_)))
        });
        let entity = handle.read_with(cx, |root, _| root.0.clone()).unwrap();
        let visual = VisualTestContext::from_window(*handle, cx);
        visual.run_until_parked();
        (visual, entity)
    }
    #[gpui::test]
    fn actual_popup_composition_is_foreground_at_window_coordinates(cx: &mut TestAppContext) {
        for grid in [false, true] {
            let (mut visual, view) = fixture(cx, Kind::Menu(grid));
            let button = visual.debug_bounds("workspace-open-menu").unwrap();
            visual.simulate_click(button.center(), Modifiers::default());
            visual.run_until_parked();
            let panel = visual.debug_bounds("context-menu-panel").unwrap();
            assert!(
                (panel.left() - button.center().x).abs() < px(1.0),
                "window position must not include the fixture offset twice"
            );
            let choice = visual
                .debug_bounds(if grid {
                    "context-menu-choice-3"
                } else {
                    "context-menu-item"
                })
                .unwrap();
            assert!(
                choice.center().y
                    > visual
                        .debug_bounds("workspace-test-parent")
                        .unwrap()
                        .bottom()
            );
            visual.simulate_click(choice.center(), Modifiers::default());
            visual.run_until_parked();
            view.read_with(&visual, |s, _| {
                assert!(s.menu.is_none());
                assert_eq!(s.count, if grid { 3 } else { 2 });
            });
            visual.simulate_keystrokes("enter down enter");
            visual.run_until_parked();
            view.read_with(&visual, |s, _| {
                assert!(s.menu.is_none());
                assert_eq!(s.count, 1);
            });
            view.update(&mut visual, |s, cx| s.set_enabled(false, cx));
            visual.run_until_parked();
            visual.simulate_click(button.center(), Modifiers::default());
            view.read_with(&visual, |s, _| assert!(s.menu.is_none()));
        }
    }
    #[gpui::test]
    fn text_buttons_and_scrolling_rows_have_real_local_input(cx: &mut TestAppContext) {
        for kind in [TextInputKind::Value, TextInputKind::Search] {
            let (mut visual, view) = fixture(cx, Kind::Text(kind));
            visual.simulate_keystrokes("cmd-a");
            visual.simulate_input("MiXeD 🎹");
            visual.run_until_parked();
            view.read_with(&visual, |s, cx| {
                assert_eq!(
                    s.text.read(cx).text(),
                    if kind == TextInputKind::Value {
                        "MiXeD 🎹"
                    } else {
                        "mixed 🎹"
                    }
                )
            });
            view.update(&mut visual, |s, cx| s.set_enabled(false, cx));
            visual.run_until_parked();
            visual.simulate_input("ignored");
            view.read_with(&visual, |s, cx| {
                assert!(!s.text.read(cx).text().contains("ignored"))
            });
        }
        let (mut visual, view) = fixture(cx, Kind::Button("play"));
        let button = visual.debug_bounds("workspace-button").unwrap();
        assert_eq!(button.size.width, px(20.0));
        assert_eq!(button.size.height, px(20.0));
        // A click must restore this control's keyboard target after toolbar focus.
        visual.update(|window, _| window.blur());
        visual.simulate_click(button.center(), Modifiers::default());
        visual.update(|window, cx| view.read_with(cx, |s, _| assert!(s.focus.is_focused(window))));
        visual.simulate_keystrokes("space");
        visual.run_until_parked();
        view.read_with(&visual, |s, _| {
            assert_eq!(s.count, 2);
            assert!(!s.active);
        });
        view.update(&mut visual, |s, cx| s.set_enabled(false, cx));
        visual.run_until_parked();
        visual.simulate_click(button.center(), Modifiers::default());
        view.read_with(&visual, |s, _| assert_eq!(s.count, 2));
        let (mut visual, view) = fixture(cx, Kind::ScrollingList);
        let list = visual.debug_bounds("workspace-scroll-list").unwrap();
        let before = visual.debug_bounds("workspace-row-0").unwrap();
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: list.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-200.0))),
            touch_phase: gpui::TouchPhase::Moved,
            ..Default::default()
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("workspace-row-0").unwrap().top() < before.top());
        let row = visual.debug_bounds("workspace-row-10").unwrap();
        assert!(list.contains(&row.center()));
        visual.simulate_click(row.center(), Modifiers::default());
        view.read_with(&visual, |s, _| assert!(s.status.contains("row · 11")));
        visual.simulate_keystrokes("down");
        visual.run_until_parked();
        view.read_with(&visual, |s, _| assert_eq!(s.count, 11));
    }
    #[gpui::test]
    fn both_meter_fixtures_follow_local_levels_and_unavailable_choices(cx: &mut TestAppContext) {
        for kind in [Kind::LevelMeter, Kind::OutputMeter] {
            let (mut visual, view) = fixture(cx, kind);
            let fill = if kind == Kind::LevelMeter {
                "workspace-gauge-0-fill"
            } else {
                "output-meter-left-fill"
            };
            let before = visual.debug_bounds(fill).unwrap();
            visual.simulate_keystrokes("up up up up up up up up");
            visual.run_until_parked();
            assert!(
                visual.debug_bounds(fill).unwrap().size.height > before.size.height,
                "{kind:?}"
            );
            let choice = visual.debug_bounds("workspace-meter-state").unwrap();
            visual.simulate_click(choice.center(), Modifiers::default());
            visual.run_until_parked();
            let invalid = visual
                .debug_bounds("workspace-meter-state-option-4")
                .unwrap();
            visual.simulate_click(invalid.center(), Modifiers::default());
            visual.run_until_parked();
            assert_eq!(visual.debug_bounds(fill).unwrap().size.height, px(0.0));
            view.read_with(&visual, |s, cx| {
                assert!(matches!(s.signal(cx),MeterInput::Last {left,..} if left.is_nan()))
            });
        }
    }
    #[gpui::test]
    fn round_actions_are_one_shot_for_pointer_keyboard_and_accessibility(cx: &mut TestAppContext) {
        for kind in [RoundActionKind::Label, RoundActionKind::Icon] {
            let (mut visual, view) = fixture(cx, Kind::RoundAction(kind));
            let button = visual.debug_bounds("workspace-round-action").unwrap();
            assert_eq!(button.size.height, px(20.0));
            assert_eq!(
                button.size.width,
                px(if kind == RoundActionKind::Label {
                    40.0
                } else {
                    20.0
                })
            );
            visual.simulate_mouse_move(point(px(500.0), px(400.0)), None, Modifiers::default());
            visual.run_until_parked();
            let idle = visual.rendered_background("workspace-round-action");
            visual.simulate_mouse_down(
                button.center(),
                gpui::MouseButton::Left,
                Modifiers::default(),
            );
            visual.run_until_parked();
            view.read_with(&visual, |view, _| assert_eq!(view.count, 0));
            visual.simulate_mouse_up(
                button.center(),
                gpui::MouseButton::Left,
                Modifiers::default(),
            );
            visual.simulate_mouse_move(point(px(500.0), px(400.0)), None, Modifiers::default());
            visual.run_until_parked();
            view.read_with(&visual, |view, _| {
                assert_eq!(view.count, 1);
                assert!(!view.active);
            });
            assert_eq!(visual.rendered_background("workspace-round-action"), idle);
            visual.simulate_keystrokes("space enter");
            visual.simulate_event(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse("space").unwrap(),
                is_held: true,
            });
            visual.run_until_parked();
            view.read_with(&visual, |view, _| {
                assert_eq!(view.count, 3);
                assert!(!view.active);
            });
            let id = visual.update(|window, _| {
                let node = window
                    .accessibility_nodes()
                    .find(|node| {
                        node.descriptor.name.as_ref()
                            == if kind == RoundActionKind::Label {
                                "run local labeled action"
                            } else {
                                "run local icon action"
                            }
                    })
                    .unwrap();
                assert_eq!(node.descriptor.value, gpui::AccessibilityValue::Button);
                node.descriptor.id
            });
            assert!(
                visual.update(|window, cx| window.perform_accessibility_action(
                    id,
                    gpui::AccessibilityAction::Press,
                    cx
                ))
            );
            visual.run_until_parked();
            view.read_with(&visual, |view, _| {
                assert_eq!(view.count, 4);
                assert!(!view.active);
            });
            assert_eq!(visual.rendered_background("workspace-round-action"), idle);
            view.update(&mut visual, |view, cx| view.set_enabled(false, cx));
            visual.run_until_parked();
            visual.simulate_click(button.center(), Modifiers::default());
            visual.simulate_keystrokes("space enter");
            assert!(
                !visual.update(|window, cx| window.perform_accessibility_action(
                    id,
                    gpui::AccessibilityAction::Press,
                    cx
                ))
            );
            view.read_with(&visual, |view, _| {
                assert_eq!(view.count, 4);
                assert!(!view.active);
            });
        }
    }
    #[gpui::test]
    fn calibrated_meter_fixtures_show_local_dbfs_peaks_and_explicit_states(
        cx: &mut TestAppContext,
    ) {
        for channels in [MeterChannels::Mono, MeterChannels::Stereo] {
            let (mut visual, view) = fixture(cx, Kind::MeterIndicator(channels));
            let first_fill = visual
                .debug_bounds("workspace-indicator-channel-0-fill")
                .unwrap();
            let first_peak = visual
                .debug_bounds("workspace-indicator-channel-0-peak")
                .unwrap();
            assert!(first_fill.size.height > px(0.0));
            assert_eq!(
                visual
                    .debug_bounds("workspace-indicator-channel-1")
                    .is_some(),
                channels == MeterChannels::Stereo
            );
            visual.simulate_keystrokes("up up");
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("workspace-indicator-channel-0-fill")
                    .unwrap()
                    .size
                    .height
                    > first_fill.size.height
            );
            visual.update(|window, cx| {
                view.read_with(cx, |view, cx| {
                    view.numbers[1].focus_handle(cx).focus(window)
                })
            });
            visual.simulate_keystrokes("up up");
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("workspace-indicator-channel-0-peak")
                    .unwrap()
                    .top()
                    < first_peak.top()
            );
            for selected in [1, 2, 3, 0] {
                let selector = visual.debug_bounds("workspace-meter-state").unwrap();
                visual.simulate_click(selector.center(), Modifiers::default());
                visual.run_until_parked();
                let option = visual
                    .debug_bounds(match selected {
                        1 => "workspace-meter-state-option-1",
                        2 => "workspace-meter-state-option-2",
                        3 => "workspace-meter-state-option-3",
                        _ => "workspace-meter-state-option-0",
                    })
                    .unwrap();
                visual.simulate_click(option.center(), Modifiers::default());
                visual.run_until_parked();
                let fill = visual
                    .debug_bounds("workspace-indicator-channel-0-fill")
                    .unwrap();
                if matches!(selected, 1 | 2) {
                    assert_eq!(fill.size.height, px(0.0));
                    assert!(
                        visual
                            .debug_bounds("workspace-indicator-channel-0-peak")
                            .is_none()
                    );
                } else {
                    assert!(fill.size.height > px(0.0));
                    assert!(
                        visual
                            .debug_bounds("workspace-indicator-channel-0-peak")
                            .is_some()
                    );
                }
                assert_eq!(
                    visual
                        .debug_bounds("workspace-indicator-channel-0-above")
                        .is_some(),
                    selected == 3
                );
            }
        }
    }
}
