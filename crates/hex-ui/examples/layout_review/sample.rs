//! Disposable owners for widget events. All edits stop at this view's local values.
#[path = "rectangle_demo.rs"]
mod rectangle_demo;
#[path = "response_demo.rs"]
mod response_demo;
#[path = "waveform_demo.rs"]
mod waveform_demo;
use super::catalog::{Demo, Item};
use super::workspace::{self, Workspace};
use crate::theme;
use gpui::{
    Context, Div, Entity, Focusable, Render, Subscription, Window, div, prelude::*, px, rgb,
};
use hex_widgets::*;
use rectangle_demo::RectangleDemo;
use response_demo::ResponseDemo;
use std::{cell::RefCell, rc::Rc};

pub(super) struct Prepared {
    item: Item,
    prefix: String,
    numbers: Vec<NumericControlConfig>,
    toggle: ToggleConfig,
    selector: SelectorConfig,
    picker: ColorPickerConfig,
    workspace: Option<workspace::Prepared>,
}
pub(super) struct Sample {
    pub item: Item,
    prefix: String,
    numbers: Vec<Entity<NumericControl>>,
    toggle: Entity<Toggle>,
    selector: Entity<Selector>,
    graph: Entity<ResponseGraph>,
    response: ResponseDemo,
    rectangle: Entity<RectangleDemo>,
    waveform_cache: Rc<RefCell<waveform_demo::Cache>>,
    bars: Entity<BarEditor>,
    values: [f32; 64],
    bars_before: Option<[f32; 64]>,
    picker: Entity<ColorPicker>,
    _subscriptions: Vec<Subscription>,
    paint_subscriptions: Vec<Subscription>,
    style: ControlStyle,
    pub enabled: bool,
    pub status: String,
    pub narrow: bool,
    actions: u32,
    workspace: Option<Entity<Workspace>>,
}
impl Sample {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub fn prepare(item: Item) -> Result<Prepared, String> {
        Self::prepare_scoped(item, "")
    }
    pub fn prepare_scoped(item: Item, prefix: &str) -> Result<Prepared, String> {
        let id = |name: &str| format!("{prefix}{name}");
        let style = theme::control_style();
        let (appearance, layout, scale) = match item.demo {
            Demo::Numeric(a, l, s) => (a, l, s),
            _ => (
                NumericControlPresentation::CompactSlider,
                DialLayout::ReadoutGap,
                NumericScale::Linear,
            ),
        };
        let (min, max, default, unit) = match scale {
            NumericScale::Logarithmic => (20.0, 20_000.0, 440.0, "hz"),
            NumericScale::Bipolar => (-100.0, 100.0, 0.0, "%"),
            NumericScale::LinearAmplitudeDecibels => (0.0, 2.0, 1.0, "db"),
            NumericScale::Linear => (0.0, 100.0, 50.0, "%"),
        };
        let spec =
            NumericSpec::new(min, max, default, scale, 2, Some(unit)).map_err(|e| e.to_string())?;
        let mut numbers = Vec::new();
        for (i, initial) in [0.0, 0.5, 1.0].into_iter().enumerate() {
            let config = NumericControlConfig::new(
                id(&format!("sandbox-number-{i}")),
                theme::text(if matches!(item.demo, Demo::Waveform(_)) {
                    ["selection start", "amplitude", "selection end"][i]
                } else {
                    ["minimum", "middle", "maximum"][i]
                }),
                appearance,
                spec.clone(),
                spec.from_normalized(initial).map_err(|e| e.to_string())?,
                style,
            )
            .map_err(|e| e.to_string())?
            .with_dial_layout(layout);
            numbers.push(config);
        }
        let toggle_config =
            ToggleConfig::new(id("sandbox-toggle"), theme::text("enabled"), true, style)?
                .with_appearance(match item.demo {
                    Demo::Toggle(a) => a,
                    Demo::Frame("device header" | "device frame") => {
                        ToggleAppearance::CompactSquare
                    }
                    _ => ToggleAppearance::Standard,
                });
        let choices = if matches!(item.demo, Demo::Curve) {
            vec![theme::text("horizontal grid"), theme::text("centered grid")]
        } else {
            ["sine", "triangle", "square"].map(theme::text).to_vec()
        };
        let selector_config = SelectorConfig::new(id("sandbox-selector"), choices, 0, style)?
            .with_appearance(match item.demo {
                Demo::Selector(a) => a,
                _ => SelectorAppearance::Standard,
            })
            .with_accessibility_name(theme::text(if matches!(item.demo, Demo::Curve) {
                "plot grid"
            } else {
                "wave shape"
            }));
        let picker = ColorPickerConfig::new(
            id("sandbox-color"),
            RgbColor::new(75, 150, 210),
            210.0,
            style,
        )?;
        Ok(Prepared {
            item,
            prefix: prefix.to_owned(),
            numbers,
            toggle: toggle_config,
            selector: selector_config,
            picker,
            workspace: if let Demo::Workspace(kind) = item.demo {
                Some(Workspace::prepare_scoped(kind, prefix)?)
            } else {
                None
            },
        })
    }
    pub fn new(prepared: Prepared, cx: &mut Context<Self>) -> Self {
        let Prepared {
            item,
            prefix,
            numbers: configs,
            toggle: toggle_config,
            selector: selector_config,
            picker: picker_config,
            workspace,
        } = prepared;
        let style = theme::control_style();
        let mut status = "local fixture · ready".to_string();
        let mut subscriptions = Vec::new();
        let mut numbers = Vec::new();
        for (i, config) in configs.into_iter().enumerate() {
            let number = cx.new(|cx| NumericControl::from_config(cx, config));
            number.update(cx, |n, cx| n.set_tab_navigation(true, cx));
            subscriptions.push(
                cx.subscribe(&number, move |s, _, e: &NumericControlEvent, cx| {
                    s.status = format!("value {} · {:?} · {:.4}", i + 1, e.phase, e.value);
                    cx.notify();
                }),
            );
            numbers.push(number);
        }
        let toggle = cx.new(|cx| Toggle::from_config(cx, toggle_config));
        toggle.update(cx, |t, cx| t.set_tab_navigation(true, cx));
        subscriptions.push(cx.subscribe(&toggle, |s, _, e: &ToggleChanged, cx| {
            s.status = format!("toggle · {}", e.checked);
            cx.notify();
        }));
        let selector = cx.new(|cx| Selector::from_config(cx, selector_config));
        selector.update(cx, |s, cx| s.set_tab_navigation(true, cx));
        subscriptions.push(cx.subscribe(&selector, |s, _, e: &SelectorChanged, cx| {
            s.status = format!("choice {} · {}", e.index + 1, e.label);
            cx.notify();
        }));
        let response = ResponseDemo::new();
        let graph = cx.new(|cx| ResponseGraph::new(cx, format!("{prefix}sandbox-graph"), style));
        if let Err(e) = graph.update(cx, |g, cx| {
            g.set_editable(true, cx);
            g.set_creation_enabled(true);
            g.set_creation_drag_enabled(true);
            g.set_backspace_removal(true);
            g.set_tab_navigation(true, cx);
            g.set_marquee_enabled(true, cx);
            g.set_axes(
                Some(ResponseGraphAxes {
                    horizontal: [
                        (20.0_f32, "20 hz"),
                        (100.0, "100 hz"),
                        (1000.0, "1 khz"),
                        (10_000.0, "10 khz"),
                        (20_000.0, "20 khz"),
                    ]
                    .into_iter()
                    .map(|(hz, label)| ((hz / 20.0).ln() / 1000.0_f32.ln(), theme::text(label)))
                    .collect(),
                    vertical: [(0.0, "-12 db"), (0.5, "0 db"), (1.0, "+12 db")]
                        .into_iter()
                        .map(|(y, label)| (y, theme::text(label)))
                        .collect(),
                    right: vec![],
                }),
                cx,
            )?;
            g.set_model(response.model.clone(), style, cx)
        }) {
            status = e.into();
        }
        subscriptions.push(cx.subscribe(&graph, |s, _, e: &ResponseGraphEvent, cx| {
            s.graph_event(e, cx)
        }));
        subscriptions.push(
            cx.subscribe(&graph, |s, _, e: &ResponseGraphMarqueeEvent, cx| {
                if !s.enabled && e.phase != ResponseGraphMarqueePhase::Cancel {
                    return;
                }
                s.response.marquee(e);
                if let Err(error) = s.graph.update(cx, |g, cx| {
                    g.set_model(s.response.model.clone(), theme::control_style(), cx)
                }) {
                    s.status = error.into();
                }
                cx.notify();
            }),
        );
        let values = std::array::from_fn(|i| 0.25 + (i % 8) as f32 * 0.08);
        let bars = cx.new(|cx| BarEditor::new(cx, format!("{prefix}sandbox-bars"), style));
        if let Err(e) = bars.update(cx, |b, cx| b.set_values(values, 32, cx)) {
            status = e.into();
        }
        let mut paint_subscriptions = Vec::new();
        paint_subscriptions
            .push(cx.subscribe(&bars, |s, _, e: &BarEditorEvent, cx| s.bar_event(e, cx)));
        let picker = cx.new(|cx| ColorPicker::from_config(cx, picker_config));
        paint_subscriptions.push(cx.subscribe(&picker, |s, _, e: &ColorPickerEvent, cx| {
            s.status = format!("color · #{:06x} · hue {:.1}", e.color.packed(), e.hsv.hue);
            cx.notify();
        }));
        let rectangle = cx.new(|cx| {
            RectangleDemo::new(
                match item.demo {
                    Demo::Rectangle(true) => rectangle_demo::Kind::Section,
                    Demo::TimeRange => rectangle_demo::Kind::TimeRange,
                    _ => rectangle_demo::Kind::Clip,
                },
                format!("{prefix}sandbox-rectangle"),
                cx,
            )
        });
        Self {
            item,
            prefix,
            numbers,
            toggle,
            selector,
            graph,
            response,
            rectangle,
            waveform_cache: Rc::new(RefCell::new(waveform_demo::Cache::default())),
            bars,
            values,
            bars_before: None,
            picker,
            _subscriptions: subscriptions,
            paint_subscriptions,
            style,
            enabled: true,
            status,
            narrow: false,
            actions: 0,
            workspace: workspace.map(|p| cx.new(|cx| Workspace::new(p, cx))),
        }
    }
    fn id(&self, name: &str) -> String {
        format!("{}{name}", self.prefix)
    }
    fn bar_event(&mut self, e: &BarEditorEvent, cx: &mut Context<Self>) {
        match e.phase {
            BarEditorPhase::Begin => self.bars_before = Some(self.values),
            BarEditorPhase::Cancel => {
                if let Some(before) = self.bars_before.take() {
                    self.values = before;
                }
            }
            _ => {}
        }
        if e.phase != BarEditorPhase::Cancel {
            if matches!(e.mode, BarEditorMode::Clear) {
                let [first, last] = e.columns;
                if first <= last && last < 32 {
                    self.values[first..=last].fill(0.0);
                }
            } else {
                let i = ((e.position[0] * 32.0) as usize).min(31);
                self.values[i] = e.position[1].clamp(0.0, 1.0);
            }
        }
        if e.phase == BarEditorPhase::Commit {
            self.bars_before = None;
        }
        if let Err(e) = self
            .bars
            .update(cx, |b, cx| b.set_values(self.values, 32, cx))
        {
            self.status = e.into();
            return;
        }
        self.status = format!(
            "bar · {:?} · {:.3}, {:.3}",
            e.phase, e.position[0], e.position[1]
        );
        cx.notify();
    }
    fn graph_event(&mut self, e: &ResponseGraphEvent, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        let Some(node_id) = self.response.event(e) else {
            self.status = "point edit refused · up to 16 points".into();
            self.graph.update(cx, |g, cx| g.cancel(cx));
            cx.notify();
            return;
        };
        let can_create = self.response.can_create();
        if let Err(error) = self.graph.update(cx, |g, cx| {
            g.set_creation_enabled(can_create);
            g.set_model(self.response.model.clone(), theme::control_style(), cx)
        }) {
            self.status = error.into();
            return;
        }
        self.status = if let Some(node) = self
            .response
            .model
            .nodes
            .iter()
            .find(|node| node.id == node_id)
        {
            format!(
                "node {node_id} · {:?} · {:.0} hz · {:+.1} db",
                e.phase,
                response_demo::frequency(node.position[0]),
                response_demo::gain(node.position[1])
            )
        } else {
            format!("node {node_id} · {:?}", e.phase)
        };
        cx.notify();
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.rectangle
            .update(cx, |rectangle, cx| rectangle.cancel(cx));
        if let Some(w) = &self.workspace {
            w.update(cx, |w, cx| w.cancel(cx));
        }
        for n in &self.numbers {
            n.update(cx, |n, cx| n.cancel(cx));
        }
        self.graph.update(cx, |g, cx| g.cancel(cx));
        self.bars.update(cx, |b, cx| b.cancel(cx));
        // Restore synchronously before replacing entities/subscriptions on theme
        // changes. Cancel notifications may arrive after that replacement.
        if self.response.cancel() {
            if let Err(e) = self.graph.update(cx, |g, cx| {
                g.set_model(self.response.model.clone(), theme::control_style(), cx)
            }) {
                self.status = e.into();
            }
        }
        let can_create = self.enabled && self.response.can_create();
        self.graph
            .update(cx, |g, _| g.set_creation_enabled(can_create));
        if let Some(before) = self.bars_before.take() {
            self.values = before;
            if let Err(e) = self
                .bars
                .update(cx, |b, cx| b.set_values(self.values, 32, cx))
            {
                self.status = e.into();
            }
        }
    }
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.cancel(cx);
        self.enabled = enabled;
        self.rectangle
            .update(cx, |rectangle, cx| rectangle.set_enabled(enabled, cx));
        if let Some(w) = &self.workspace {
            w.update(cx, |w, cx| w.set_enabled(enabled, cx));
        }
        for n in &self.numbers {
            n.update(cx, |n, cx| n.set_enabled(enabled, cx));
        }
        self.toggle.update(cx, |t, cx| t.set_enabled(enabled, cx));
        self.selector.update(cx, |s, cx| s.set_enabled(enabled, cx));
        let can_create = enabled && self.response.can_create();
        self.graph.update(cx, |g, cx| {
            g.set_editable(enabled, cx);
            g.set_creation_enabled(can_create);
        });
        self.bars
            .update(cx, |bars, cx| bars.set_enabled(enabled, cx));
        cx.notify();
    }
    pub fn restyle(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        self.cancel(cx);
        let style = theme::control_style();
        if let Some(w) = &self.workspace {
            w.update(cx, |w, cx| w.restyle(cx))?;
        }
        if self.style == style {
            return Ok(());
        }
        for n in &self.numbers {
            n.update(cx, |n, cx| n.set_external_style(style, cx))
                .map_err(|e| e.to_string())?;
        }
        self.toggle
            .update(cx, |t, cx| t.set_external_style(style, cx))?;
        self.selector
            .update(cx, |s, cx| s.set_external_style(style, cx))?;
        self.graph.update(cx, |g, cx| {
            g.set_model(self.response.model.clone(), style, cx)
        })?;
        // These two widgets have no external style setter. Rebuild only their view
        // entities, preserving authored local values and reconnecting subscriptions.
        let color = self.picker.read(cx).color();
        let hue = self.picker.read(cx).hsv().hue;
        let config = ColorPickerConfig::new(self.id("sandbox-color"), color, hue, style)?;
        self.picker = cx.new(|cx| ColorPicker::from_config(cx, config));
        self.bars = cx.new(|cx| BarEditor::new(cx, self.id("sandbox-bars"), style));
        self.bars
            .update(cx, |b, cx| b.set_values(self.values, 32, cx))?;
        self.bars
            .update(cx, |bars, cx| bars.set_enabled(self.enabled, cx));
        self.paint_subscriptions.clear();
        self.paint_subscriptions
            .push(cx.subscribe(&self.bars, |s, _, e: &BarEditorEvent, cx| {
                s.bar_event(e, cx)
            }));
        self.paint_subscriptions.push(cx.subscribe(
            &self.picker,
            |s, _, e: &ColorPickerEvent, cx| {
                s.status = format!("color · #{:06x}", e.color.packed());
                cx.notify();
            },
        ));
        self.style = style;
        cx.notify();
        Ok(())
    }
    pub fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.item.demo {
            Demo::Workspace(_) => {
                if let Some(w) = &self.workspace {
                    w.update(cx, |w, cx| w.focus_first(window, cx));
                }
            }
            Demo::Toggle(_) | Demo::Chrome(_) | Demo::Frame("device header" | "device frame") => {
                self.toggle.focus_handle(cx).focus(window)
            }
            Demo::Color => {}
            Demo::Selector(_) => self.selector.focus_handle(cx).focus(window),
            Demo::Rectangle(_) | Demo::TimeRange => self.rectangle.focus_handle(cx).focus(window),
            Demo::Bars => self.bars.focus_handle(cx).focus(window),
            Demo::Graph => self.graph.focus_handle(cx).focus(window),
            _ => {
                if let Some(n) = self.numbers.first() {
                    n.focus_handle(cx).focus(window);
                }
            }
        }
    }
    fn controls(&self) -> Div {
        device_layout(DeviceLayout::Wrapped).children(self.numbers.iter().map(|n| {
            div()
                .w(px(132.0))
                .h(px(52.0))
                .flex_shrink_0()
                .child(n.clone())
        }))
    }
    fn chrome(&self, name: &str, cx: &mut Context<Self>) -> Div {
        let s = theme::control_style();
        let label = theme::text("sample");
        let child = match name {
            "action" => action(label, theme::action_style()),
            "device action" => device_action(theme::text("+"), theme::action_style()),
            "device badge" => device_badge(theme::text("a"), s),
            "device caption" => device_caption(label, s),
            "device divider" => device_divider(s),
            "device focus target" => device_focus_target(s).w(px(100.0)).h(px(28.0)).child(label),
            "device identity marker" => device_identity_marker(s.accent),
            "device inline caption" => device_inline_caption(label, s),
            "device segment" => device_segment(
                theme::text("a"),
                theme::device_segment_style(self.actions % 2 == 1),
            ),
            _ => panel_tab(label, self.actions % 2 == 1, s),
        };
        let child = if matches!(name, "action" | "device action") {
            child
                .id("chrome-action")
                .debug_selector({
                    let id = self.id("chrome-action");
                    move || id.clone()
                })
                .when(self.enabled, |button| {
                    pressed_action_surface(button, theme::action_press_style(theme::action_style()))
                })
                .into_any_element()
        } else {
            child.into_any_element()
        };
        div().child(
            div()
                .id("chrome-hit")
                .debug_selector({
                    let id = self.id("chrome-hit");
                    move || id.clone()
                })
                .track_focus(&self.toggle.focus_handle(cx))
                .flex()
                .items_center()
                .h(px(48.0))
                .min_w(px(120.0))
                .child(child)
                .cursor_pointer()
                .on_key_down(cx.listener(|s, e: &gpui::KeyDownEvent, _, cx| {
                    if s.enabled && matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        s.actions = s.actions.saturating_add(1);
                        s.status = format!("local action · {}", s.actions);
                        cx.notify();
                        cx.stop_propagation();
                    }
                }))
                .on_click(cx.listener(|s, _, _, cx| {
                    if s.enabled {
                        s.actions = s.actions.saturating_add(1);
                        s.status = format!("local action · {}", s.actions);
                        cx.notify();
                    }
                })),
        )
    }
    fn header_action(&self, cx: &mut Context<Self>) -> Div {
        div().child(
            device_action(theme::text("+"), theme::action_style())
                .id("header-action")
                .debug_selector({
                    let id = self.id("header-action");
                    move || id.clone()
                })
                .on_click(cx.listener(|s, _, _, cx| {
                    if s.enabled {
                        s.actions = s.actions.saturating_add(1);
                        s.status = format!("local action · {}", s.actions);
                        cx.notify();
                    }
                })),
        )
    }
    fn content(&self, cx: &mut Context<Self>) -> Div {
        let s = theme::control_style();
        match self.item.demo {
            Demo::Numeric(appearance, _, _) => {
                let kind = appearance.kind();
                device_layout(DeviceLayout::Wrapped).children(self.numbers.iter().map(|n| {
                    div()
                        .w(px(
                            if appearance == NumericControlPresentation::WideMicroDial {
                                88.0
                            } else {
                                140.0
                            },
                        ))
                        .h(px(if kind == NumericControlKind::VerticalSlider {
                            180.0
                        } else {
                            72.0
                        }))
                        .flex_shrink_0()
                        .child(n.clone())
                }))
            }
            Demo::Toggle(_) => div().w(px(180.0)).h(px(28.0)).child(self.toggle.clone()),
            Demo::Selector(_) => div().w(px(220.0)).h(px(28.0)).child(self.selector.clone()),
            Demo::Rectangle(_) | Demo::TimeRange => {
                div().w_full().h(px(180.0)).child(self.rectangle.clone())
            }
            Demo::Bars => div().w_full().h(px(180.0)).child(self.bars.clone()),
            Demo::Graph => div().w_full().h(px(220.0)).child(self.graph.clone()),
            Demo::Color => div().w(px(240.0)).h(px(220.0)).child(self.picker.clone()),
            Demo::Waveform(stereo) => div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(180.0))
                        .child(waveform_demo::view(
                            self.id("sandbox-waveform"),
                            stereo,
                            std::array::from_fn(|index| self.numbers[index].read(cx).value()),
                            Rc::clone(&self.waveform_cache),
                            s,
                        )),
                )
                .child(self.controls()),
            Demo::Curve | Demo::Spectrum => {
                let amount = self.numbers[1].read(cx).value() as f32 / 100.0;
                let points = [
                    [0.0, 0.0],
                    [0.25, amount * 0.5],
                    [0.5, amount],
                    [0.75, 0.5 + amount * 0.5],
                    [1.0, 1.0],
                ];
                let plot = match self.item.demo {
                    Demo::Curve => curve_plot(
                        points,
                        s,
                        if self.selector.read(cx).selected() == 0 {
                            CurvePlotGrid::Horizontal
                        } else {
                            CurvePlotGrid::Centered
                        },
                    ),
                    _ => SpectrumPlotLayers::new(vec![
                        SpectrumPlotLayer {
                            points: points.to_vec(),
                            peak: None,
                            color: s.accent,
                        },
                        SpectrumPlotLayer {
                            points: points.map(|[x, y]| [x, 1.0 - y]).to_vec(),
                            peak: None,
                            color: s.muted,
                        },
                    ])
                    .map(|layers| layered_spectrum_plot(layers, s)),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .when(matches!(self.item.demo, Demo::Curve), |column| {
                        column.child(self.selector.clone())
                    })
                    .child(div().h(px(180.0)).children(plot))
                    .child(self.controls())
            }
            Demo::Chrome(name) => self.chrome(name, cx),
            Demo::Layout(layout) => device_layout(layout).children(
                self.numbers
                    .iter()
                    .map(|n| div().w(px(132.0)).h(px(52.0)).child(n.clone())),
            ),
            Demo::Section(density, surface) => match density {
                0 => device_section(s, surface),
                1 => device_compact_section(s, surface),
                _ => device_dense_section(s, surface),
            }
            .child(self.controls()),
            Demo::Frame(name) => match name {
                "device split" => device_split(
                    div().child(self.numbers[0].clone()),
                    div().child(self.numbers[1].clone()),
                ),
                "device body" => device_body(s).child(self.controls()),
                "windowed body" => device_windowed_body(s, self.controls()),
                "compact row · idle" | "compact row · active" => {
                    device_compact_row(s, name.ends_with("active")).child(self.controls())
                }
                "grouped row · idle" | "grouped row · active" => {
                    device_grouped_row(s, name.ends_with("active")).child(self.controls())
                }
                "device rail" => device_rail(s).child(self.controls()),
                "device display" => device_display(s).child(self.controls()),
                "device frame" => device_frame(
                    theme::text("fixture frame"),
                    s,
                    self.toggle.clone(),
                    self.header_action(cx),
                    device_body(s).child(self.controls()),
                ),
                _ => device_header(
                    theme::text("fixture header"),
                    s,
                    self.toggle.clone(),
                    self.header_action(cx),
                ),
            },
            Demo::Compositions => div(),
            Demo::Workspace(_) => div().w_full().children(self.workspace.clone()),
        }
    }
}
impl Render for Sample {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().gap(px(12.0)).w_full()
            .child(theme::text(if matches!(self.item.demo, Demo::Graph) { "frequency / gain response shape" } else { self.item.name }))
            .child(div().id("widget-preview").debug_selector({let id = self.id("widget-preview"); move || id.clone()}).w_full().max_w(px(if self.narrow{260.0}else{680.0})).p(px(8.0)).overflow_x_scroll().border_1().border_color(rgb(theme::control_style().border)).child(self.content(cx)))
            .child(theme::text(&self.status))
            .when(!matches!(self.item.demo, Demo::Graph), |sample| sample.child(theme::text("numeric: drag / double-click to type / arrows / escape · tab and shift-tab move focus")))
            .child(theme::text(match self.item.demo {Demo::TimeRange=>"upper third moves · lower body paints/repaints · shift paints · escape cancels held input · accept or refuse pending paint",Demo::Rectangle(_)=>"drag body / brackets / touching seam · shift selects · escape cancels held input · accept or refuse pending paint",Demo::Workspace(_)=>"shared workspace controls and actual menu/status compositions · local values only",Demo::Graph=>"drag a diamond to shape the curve · click and drag empty space to add · up to 16 points · backspace removes · escape cancels · flat line: 0 db reference · illustration only, no dsp",Demo::Bars=>"paint bars with pointer or arrows · right-drag clears crossed bars · escape restores the stroke",Demo::Color=>"drag saturation / brightness and hue · local color feedback below",Demo::Waveform(_)=>"generated peak fixture · selection start / amplitude / selection end · silence and dc regions · read-only display, no audio",Demo::Curve|Demo::Spectrum=>"read-only plot · edit middle value to change fixture points",Demo::Chrome(_)=>"appearance primitive · surrounding owner supplies the local click counter",_=>"local values only · columns start at minimum / middle / maximum"}))
            .child(theme::text(if matches!(self.item.demo,Demo::Color){"this widget has no disabled API; enable control applies to the other interactive widgets"}else{"disable controls to inspect the library's disabled state"}))
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
    #[gpui::test]
    fn waveform_fixture_edits_amplitude_reuses_range_geometry_and_obeys_disabled_state(
        cx: &mut TestAppContext,
    ) {
        cx.update(bind_numeric_control_keys);
        for stereo in [false, true] {
            let prepared = Sample::prepare(Item {
                name: "waveform",
                category: "graphics",
                demo: Demo::Waveform(stereo),
            })
            .unwrap();
            let handle = cx.add_window(|_, cx| Sample::new(prepared, cx));
            let view = handle.root(cx).unwrap();
            let mut visual = VisualTestContext::from_window(*handle, cx);
            visual.update(|window, _| window.activate_window());
            visual.simulate_resize(gpui::size(px(640.0), px(720.0)));
            visual.run_until_parked();
            let retained = |cx: &TestAppContext| {
                view.read_with(cx, |sample, _| {
                    sample.waveform_cache.borrow().retained().unwrap()
                })
            };
            let mut previous = retained(cx);
            assert_eq!(previous.channels().len(), if stereo { 2 } else { 1 });
            for index in [1, 0] {
                let before =
                    view.read_with(cx, |sample, cx| sample.numbers[index].read(cx).value());
                let selector = if index == 1 {
                    "sandbox-number-1-value"
                } else {
                    "sandbox-number-0-value"
                };
                let bounds = visual.debug_bounds(selector).unwrap();
                visual.simulate_click(bounds.center(), Modifiers::default());
                // Compact readouts enter text editing on click; leave that draft
                // before exercising the control's focused numeric arrow action.
                visual.simulate_keystrokes("escape");
                visual.simulate_keystrokes("up");
                visual.run_until_parked();
                view.read_with(cx, |sample, cx| {
                    assert!(sample.numbers[index].read(cx).value() > before)
                });
                let current = retained(cx);
                assert_eq!(
                    std::sync::Arc::ptr_eq(&previous, &current),
                    index == 0,
                    "range paint reuses paths; amplitude changes the actual envelope"
                );
                previous = current;
            }
            let before = view.read_with(cx, |sample, cx| sample.numbers[0].read(cx).value());
            view.update(cx, |sample, cx| sample.set_enabled(false, cx));
            visual.simulate_keystrokes("up");
            visual.run_until_parked();
            view.read_with(cx, |sample, cx| {
                assert_eq!(sample.numbers[0].read(cx).value(), before)
            });
            assert!(std::sync::Arc::ptr_eq(&previous, &retained(cx)));
            view.update(cx, |sample, cx| {
                sample.narrow = true;
                cx.notify();
            });
            visual.run_until_parked();
            assert!(visual.debug_bounds("sandbox-waveform").unwrap().size.width <= px(260.0));
            assert!(!std::sync::Arc::ptr_eq(&previous, &retained(cx)));
        }
    }
    #[gpui::test]
    fn sandbox_right_drag_clears_skipped_bars_and_disable_survives_restyle(
        cx: &mut TestAppContext,
    ) {
        let item = Item {
            name: "bar editor",
            category: "graphics",
            demo: Demo::Bars,
        };
        let handle = cx.add_window(|_, cx| Sample::new(Sample::prepare(item).unwrap(), cx));
        let view = handle.root(cx).unwrap();
        view.update(cx, |sample, cx| {
            sample.values = [0.8; 64];
            sample.bars.update(cx, |bars, cx| {
                bars.set_values(sample.values, 32, cx).unwrap()
            });
        });
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("sandbox-bars").unwrap();
        let at = |index: f32| {
            point(
                bounds.left() + bounds.size.width * (index + 0.5) / 32.0,
                bounds.top() + px(2.0),
            )
        };
        visual.simulate_mouse_down(at(3.0), MouseButton::Right, Modifiers::default());
        view.read_with(cx, |s, _| assert_eq!(s.values[3], 0.0));
        visual.simulate_mouse_move(at(25.0), Some(MouseButton::Right), Modifiers::default());
        visual.simulate_mouse_up(at(25.0), MouseButton::Right, Modifiers::default());
        view.read_with(cx, |s, _| {
            assert!(s.values[3..=25].iter().all(|v| *v == 0.0));
            assert_eq!(s.values[2], 0.8);
            assert_eq!(s.values[26], 0.8);
        });
        visual.simulate_mouse_down(at(26.0), MouseButton::Right, Modifiers::default());
        visual.simulate_keystrokes("escape");
        view.read_with(cx, |s, _| assert_eq!(s.values[26], 0.8));
        view.update(cx, |s, cx| {
            s.set_enabled(false, cx);
            s.restyle(cx).unwrap();
        });
        visual.run_until_parked();
        visual.simulate_mouse_down(at(26.0), MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(at(31.0), MouseButton::Right, Modifiers::default());
        view.read_with(cx, |s, _| {
            assert!(s.values[26..32].iter().all(|v| *v == 0.8))
        });
    }

    #[gpui::test]
    fn inline_value_catalog_entry_has_no_dial(cx: &mut TestAppContext) {
        let item = super::super::catalog::inventory()
            .into_iter()
            .find(|i| i.name == "inline value")
            .unwrap();
        let prepared = Sample::prepare(item).unwrap();
        let handle = cx.add_window(|_, cx| Sample::new(prepared, cx));
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("sandbox-number-0-value").is_some());
        assert!(
            visual.debug_bounds("sandbox-number-0-knob").is_none(),
            "inline value must not silently render the standard knob fallback"
        );
    }
    #[gpui::test]
    fn graph_fixture_marquee_replaces_toggles_and_cancels_selection(cx: &mut TestAppContext) {
        cx.update(hex_widgets::bind_response_graph_keys);
        let item = Item {
            name: "response graph",
            category: "graphics",
            demo: Demo::Graph,
        };
        let prepared = Sample::prepare(item).unwrap();
        let handle = cx.add_window(|_, cx| Sample::new(prepared, cx));
        handle
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.run_until_parked();
        let plot = view.read_with(cx, |s, cx| s.graph.read(cx).plot_bounds().unwrap());
        let at = |x, y| {
            point(
                plot.left() + plot.size.width * x,
                plot.bottom() - plot.size.height * y,
            )
        };
        visual.simulate_click(at(0.25, 0.3), Modifiers::default());
        visual.simulate_click(at(0.8, 0.5), Modifiers::default());
        visual.run_until_parked();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let toggle = Modifiers {
            shift: true,
            platform: true,
            ..Modifiers::default()
        };
        let selected = |visual: &mut VisualTestContext| {
            view.read_with(visual, |s, _| {
                s.response
                    .model
                    .nodes
                    .iter()
                    .filter(|n| n.selected)
                    .map(|n| n.id)
                    .collect::<Vec<_>>()
            })
        };
        visual.simulate_mouse_down(at(0.2, 0.15), MouseButton::Left, shift);
        visual.simulate_mouse_move(at(0.58, 0.82), Some(MouseButton::Left), shift);
        visual.simulate_mouse_up(at(0.58, 0.82), MouseButton::Left, shift);
        visual.run_until_parked();
        assert_eq!(selected(&mut visual), vec![1, 2]);
        visual.simulate_mouse_down(at(0.7, 0.35), MouseButton::Left, toggle);
        visual.simulate_mouse_move(at(0.9, 0.65), Some(MouseButton::Left), toggle);
        visual.simulate_mouse_up(at(0.9, 0.65), MouseButton::Left, toggle);
        visual.run_until_parked();
        assert_eq!(selected(&mut visual), vec![1, 2, 3]);
        visual.simulate_mouse_down(at(0.15, 0.15), MouseButton::Left, shift);
        visual.simulate_mouse_move(at(0.3, 0.4), Some(MouseButton::Left), shift);
        visual.run_until_parked();
        assert_eq!(selected(&mut visual), vec![2]);
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert_eq!(selected(&mut visual), vec![1, 2, 3]);
        visual.simulate_mouse_up(at(0.3, 0.4), MouseButton::Left, shift);
        visual.simulate_click(at(0.5, 0.7), Modifiers::default());
        visual.simulate_click(at(0.25, 0.3), shift);
        visual.run_until_parked();
        assert_eq!(selected(&mut visual), vec![1, 2]);
        visual.simulate_click(at(0.95, 0.1), Modifiers::default());
        visual.run_until_parked();
        assert_eq!(selected(&mut visual), vec![4]);
        visual.simulate_mouse_down(at(0.2, 0.15), MouseButton::Left, shift);
        visual.simulate_mouse_move(at(0.58, 0.82), Some(MouseButton::Left), shift);
        visual.run_until_parked();
        assert_eq!(selected(&mut visual), vec![1, 2]);
        view.update(&mut visual, |s, cx| s.set_enabled(false, cx));
        visual.run_until_parked();
        assert_eq!(selected(&mut visual), vec![4]);
    }

    #[gpui::test]
    fn graph_fixture_line_follows_pointer_edits_and_cancel(cx: &mut TestAppContext) {
        cx.update(hex_widgets::bind_response_graph_keys);
        let item = Item {
            name: "response graph",
            category: "graphics",
            demo: Demo::Graph,
        };
        let prepared = Sample::prepare(item).expect("valid fixture");
        let handle = cx.add_window(|_, cx| Sample::new(prepared, cx));
        handle
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        let view = handle.root(cx).expect("sample");
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.run_until_parked();
        let plot = view.read_with(cx, |s, cx| s.graph.read(cx).plot_bounds().unwrap());
        let at = |x, y| {
            point(
                plot.left() + plot.size.width * x,
                plot.bottom() - plot.size.height * y,
            )
        };
        let before = view.read_with(cx, |s, _| s.response.model.primary.clone());
        visual.simulate_mouse_down(at(0.5, 0.7), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.65, 0.8), Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |s, _| {
            assert_ne!(s.response.model.primary, before);
            assert!(
                s.response
                    .model
                    .primary
                    .contains(&s.response.model.nodes[0].position)
            );
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        view.read_with(cx, |s, _| assert_eq!(s.response.model.primary, before));
        visual.simulate_mouse_up(at(0.65, 0.8), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_down(at(0.3, 0.2), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at(0.3, 0.2), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |s, _| {
            assert_eq!(s.response.model.nodes.len(), 2);
            assert!(
                s.response
                    .model
                    .primary
                    .contains(&s.response.model.nodes[1].position)
            );
            assert!(s.status.starts_with("node 2"), "{}", s.status);
        });
        visual.simulate_click(at(0.3, 0.2), Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("backspace");
        visual.run_until_parked();
        view.read_with(cx, |s, _| {
            assert_eq!(s.response.model.nodes.len(), 1, "{}", s.status);
            assert_eq!(s.response.model.primary, before);
        });
    }

    #[gpui::test]
    fn graph_creation_drags_immediately_and_escape_removes_uncommitted_point(
        cx: &mut TestAppContext,
    ) {
        cx.update(hex_widgets::bind_response_graph_keys);
        let prepared = Sample::prepare(Item {
            name: "response graph",
            category: "graphics",
            demo: Demo::Graph,
        })
        .unwrap();
        let handle = cx.add_window(|_, cx| Sample::new(prepared, cx));
        handle
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.run_until_parked();
        let plot = view.read_with(cx, |s, cx| s.graph.read(cx).plot_bounds().unwrap());
        let at = |x, y| {
            point(
                plot.left() + plot.size.width * x,
                plot.bottom() - plot.size.height * y,
            )
        };
        visual.simulate_mouse_down(at(0.25, 0.25), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_move(at(0.7, 0.8), Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |s, _| {
            let node = s
                .response
                .model
                .nodes
                .iter()
                .find(|node| node.id == 2)
                .unwrap();
            assert!((node.position[0] - 0.7).abs() < 1e-4 && (node.position[1] - 0.8).abs() < 1e-4);
            assert!(s.response.model.primary.contains(&node.position));
        });
        visual.simulate_mouse_up(at(0.7, 0.8), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let committed = view.read_with(cx, |s, _| {
            (
                s.response.model.nodes.clone(),
                s.response.model.primary.clone(),
            )
        });
        visual.simulate_mouse_down(at(0.15, 0.6), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.2, 0.65), Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(at(0.2, 0.65), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |s, _| {
            assert_eq!(s.response.model.nodes, committed.0);
            assert_eq!(s.response.model.primary, committed.1);
        });
        view.update(cx, |s, cx| s.set_enabled(false, cx));
        visual.run_until_parked();
        visual.simulate_click(at(0.9, 0.2), Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |s, _| assert_eq!(s.response.model.nodes, committed.0));
    }

    #[gpui::test]
    fn theme_rebuild_cancels_live_bar_stroke_and_preserves_committed_color(
        cx: &mut TestAppContext,
    ) {
        let original = theme::preferences();
        let item = Item {
            name: "bar editor",
            category: "graphics",
            demo: Demo::Bars,
        };
        let prepared = Sample::prepare(item).expect("valid fixture");
        let handle = cx.add_window(|_, cx| Sample::new(prepared, cx));
        let view = handle.root(cx).expect("sample");
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.run_until_parked();
        let before = view.read_with(cx, |s, cx| (s.values, s.picker.read(cx).color()));
        let bars = visual.debug_bounds("sandbox-bars").expect("bar hit area");
        let start = point(bars.left() + px(20.0), bars.top() + px(12.0));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            start + point(px(35.0), px(24.0)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        visual.run_until_parked();
        view.read_with(cx, |s, _| assert_ne!(s.values, before.0));
        theme::apply_preferences(hex_preferences::ThemePreferences {
            background: hex_preferences::RgbColor::new(225, 229, 238),
            text: hex_preferences::RgbColor::new(24, 27, 34),
            primary: hex_preferences::RgbColor::new(145, 25, 45),
            secondary: hex_preferences::RgbColor::new(16, 70, 145),
        });
        view.update(cx, |s, cx| s.restyle(cx).expect("restyle"));
        visual.run_until_parked();
        view.read_with(cx, |s, cx| {
            assert_eq!(s.values, before.0);
            assert_eq!(s.picker.read(cx).color(), before.1);
        });
        visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |s, _| assert_eq!(s.values, before.0));
        theme::apply_preferences(original);
    }
}
