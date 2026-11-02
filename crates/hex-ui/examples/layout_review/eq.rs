//! Functional view-local EQ catalog: shared production geometry/widgets/axes.
//! Registered control-side response only; never fabricates a live analyzer.
use super::*;
use hex_devices::eq::{self, EqState, Node, NodeId, NodeKind};
use hex_widgets::{ResponseGraphEvent, ResponseGraphPhase};
#[path = "../../src/editor/device_panel/eq_panel/axes.rs"]
mod axes;
#[path = "../../src/editor/device_panel/eq_panel/group_move.rs"]
mod group_move;
#[path = "../../src/editor/device_panel/eq_panel/node_geometry.rs"]
mod node_geometry;
#[path = "../../src/editor/device_panel/eq_panel/sampling.rs"]
mod sampling;
use hex_widgets::{NumericControlEvent, SelectorChanged, ToggleChanged};

/// Ephemeral catalog state; it never owns a project or audio resource.
pub(super) struct Local {
    state: EqState,
    selected: Option<NodeId>,
    bank: u8,
    before: Option<(EqState, Option<NodeId>)>,
    created: Option<NodeId>,
}

impl Local {
    fn new() -> Result<Self, String> {
        let id = NodeId::new(1).map_err(|e| e.to_string())?;
        let mut state = EqState::default();
        state
            .add(Node::new(id, 1_000.0, 6.0, 0).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            state,
            selected: Some(id),
            bank: 0,
            before: None,
            created: None,
        })
    }
}

pub(super) fn has_selected(fixture: &Fixture) -> bool {
    fixture
        .eq_local
        .as_ref()
        .is_some_and(|local| local.selected.is_some())
}

pub(super) fn fixture(cx: &mut Context<Gallery>) -> Result<Fixture, String> {
    let mut subscriptions = Vec::new();
    let mut numbers = Vec::new();
    for (index, (label, min, max, value, scale, unit)) in [
        (
            "out",
            0.0,
            4.0,
            1.0,
            NumericScale::LinearAmplitudeDecibels,
            "db",
        ),
        (
            "freq",
            20.0,
            20000.0,
            1000.0,
            NumericScale::Logarithmic,
            "hz",
        ),
        ("gain", -24.0, 24.0, 6.0, NumericScale::Bipolar, "db"),
        ("q", 0.1, 18.0, 0.707, NumericScale::Logarithmic, ""),
    ]
    .into_iter()
    .enumerate()
    {
        let width = if index == 0 { 58.0 } else { 104.0 };
        let config = NumericControlConfig::new(
            format!("fixture-eq-numeric-{index}"),
            theme::text(label),
            NumericControlPresentation::InlineValue,
            NumericSpec::new(min, max, value, scale, 2, Some(unit)).map_err(|e| e.to_string())?,
            value,
            theme::device_style(theme::DeviceStyleRole::Readout),
        )
        .map_err(|e| e.to_string())?;
        let entity = cx.new(|cx| {
            let mut control = NumericControl::from_config(cx, config);
            let _ = control.set_inline_value_width(width, cx);
            control.set_tab_navigation(true, cx);
            control
        });
        subscriptions.push(cx.subscribe(&entity, |_, _, _: &NumericControlEvent, cx| cx.notify()));
        numbers.push(Number {
            entity,
            role: theme::DeviceStyleRole::Readout,
            width,
            height: 14.0,
        });
    }
    let mut toggles = Vec::new();
    for (index, label) in [
        "on", "st", "l", "r", "m", "s", "spectrum", "input", "listen",
    ]
    .into_iter()
    .enumerate()
    {
        let config = ToggleConfig::new(
            format!("fixture-eq-toggle-{index}"),
            theme::text(label),
            matches!(index, 0 | 1 | 6 | 7),
            theme::device_style(theme::DeviceStyleRole::Control),
        )
        .map_err(str::to_owned)?
        .with_appearance(ToggleAppearance::CompactButton);
        let entity = cx.new(|cx| Toggle::from_config(cx, config));
        subscriptions.push(
            cx.subscribe(&entity, move |view, _, event: &ToggleChanged, cx| {
                if (1..=5).contains(&index) && event.checked && !view.finish_drafts(cx) {
                    if let Some(fixture) = view.fixtures.get(&Entry::Eq) {
                        let bank = fixture
                            .eq_local
                            .as_ref()
                            .map_or(0, |local| usize::from(local.bank));
                        for other in 1..=5 {
                            fixture.toggles[other]
                                .update(cx, |toggle, cx| toggle.set_checked(other == bank + 1, cx));
                        }
                    }
                    cx.notify();
                    return;
                }
                if (1..=5).contains(&index) {
                    if let Some(fixture) = view.fixtures.get_mut(&Entry::Eq) {
                        if event.checked {
                            if let Err(error) = select_bank(fixture, (index - 1) as u8, cx) {
                                view.error = Some(error);
                            }
                            for other in 1..=5 {
                                if other != index {
                                    fixture.toggles[other]
                                        .update(cx, |toggle, cx| toggle.set_checked(false, cx));
                                }
                            }
                        } else if fixture
                            .eq_local
                            .as_ref()
                            .is_some_and(|local| usize::from(local.bank) + 1 == index)
                        {
                            fixture.toggles[index]
                                .update(cx, |toggle, cx| toggle.set_checked(true, cx));
                        }
                    }
                }
                cx.notify();
            }),
        );
        toggles.push(entity);
    }
    let mut selectors = Vec::new();
    for (index, labels) in [
        vec!["stereo", "left/right", "mid/side"],
        vec![
            "bell",
            "low shelf",
            "high shelf",
            "low cut",
            "high cut",
            "notch",
        ],
        vec!["12 db/oct", "24 db/oct", "36 db/oct", "48 db/oct"],
    ]
    .into_iter()
    .enumerate()
    {
        let config = SelectorConfig::new(
            format!("fixture-eq-selector-{index}"),
            labels.into_iter().map(theme::text),
            0,
            theme::device_style(theme::DeviceStyleRole::Control),
        )
        .map_err(str::to_owned)?
        .with_appearance(SelectorAppearance::CompactSelector);
        let entity = cx.new(|cx| Selector::from_config(cx, config));
        subscriptions.push(cx.subscribe(&entity, |_, _, _: &SelectorChanged, cx| cx.notify()));
        selectors.push(entity);
    }
    let graph = cx.new(|cx| {
        let mut graph = ResponseGraph::new(
            cx,
            "fixture-eq-graph",
            theme::device_style(theme::DeviceStyleRole::Display),
        );
        graph.set_editable(true, cx);
        graph.set_creation_enabled(true);
        graph.set_creation_drag_enabled(true);
        graph
    });
    subscriptions.push(
        cx.subscribe(&graph, |view, _, event: &ResponseGraphEvent, cx| {
            let cancel = event.phase == ResponseGraphPhase::Cancel;
            if !cancel && (view.disabled || view.entry != Entry::Eq) {
                return;
            }
            if matches!(
                event.phase,
                ResponseGraphPhase::Create | ResponseGraphPhase::Select
            ) && !view.finish_drafts(cx)
            {
                if let Some(fixture) = view.fixtures.get(&Entry::Eq) {
                    fixture.graph.update(cx, |graph, cx| graph.cancel(cx));
                }
                cx.notify();
                return;
            }
            if let Some(fixture) = view.fixtures.get_mut(&Entry::Eq) {
                if let Err(error) = event_graph(fixture, *event, cx) {
                    view.error = Some(error);
                }
            }
            cx.notify();
        }),
    );
    Ok(Fixture {
        entry: Entry::Eq,
        _subscriptions: subscriptions,
        numbers,
        toggles,
        selectors,
        graph,
        eq_local: Some(Local::new()?),
        discrete_height: 14.0,
    })
}

fn current(fixture: &Fixture, cx: &gpui::App) -> Option<EqState> {
    let local = fixture.eq_local.as_ref()?;
    let mut state = local.state.clone();
    let value = |i: usize| fixture.numbers[i].entity.read(cx).value() as f32;
    state.mode = fixture.selectors[0].read(cx).selected() as u8;
    state.output_gain = value(0);
    if let Some(node) = local
        .selected
        .and_then(|id| state.nodes.iter_mut().find(|n| n.id == id))
    {
        node.frequency_hz = value(1);
        node.gain_db = value(2);
        node.q = value(3);
        node.slope = fixture.selectors[2].read(cx).selected() as u8;
        node.enabled = fixture.toggles[0].read(cx).checked();
        node.kind = [
            NodeKind::Bell,
            NodeKind::LowShelf,
            NodeKind::HighShelf,
            NodeKind::LowCut,
            NodeKind::HighCut,
            NodeKind::Notch,
        ][fixture.selectors[1].read(cx).selected()];
    }
    Some(state)
}

fn select_bank(fixture: &mut Fixture, bank: u8, cx: &mut Context<Gallery>) -> Result<(), String> {
    let state = current(fixture, cx).ok_or("eq fixture unavailable")?;
    let local = fixture.eq_local.as_mut().ok_or("eq fixture unavailable")?;
    local.state = state;
    local.bank = bank;
    local.selected = local
        .state
        .nodes
        .iter()
        .find(|node| node.bank == bank)
        .map(|node| node.id);
    let selected = local
        .selected
        .and_then(|id| local.state.node(id).ok().copied());
    if let Some(node) = selected {
        set_controls(fixture, node, cx)?;
    }
    Ok(())
}

fn set_controls(fixture: &Fixture, node: Node, cx: &mut Context<Gallery>) -> Result<(), String> {
    for (index, value) in [(1, node.frequency_hz), (2, node.gain_db), (3, node.q)] {
        fixture.numbers[index]
            .entity
            .update(cx, |control, cx| {
                control.set_external_value(f64::from(value), cx)
            })
            .map_err(|e| e.to_string())?;
    }
    fixture.selectors[1]
        .update(cx, |selector, cx| {
            selector.set_selected(node.kind as usize, cx)
        })
        .map_err(str::to_owned)?;
    fixture.selectors[2]
        .update(cx, |selector, cx| {
            selector.set_selected(node.slope as usize, cx)
        })
        .map_err(str::to_owned)?;
    fixture.toggles[0].update(cx, |toggle, cx| toggle.set_checked(node.enabled, cx));
    for index in 1..=5 {
        fixture.toggles[index].update(cx, |toggle, cx| {
            toggle.set_checked(index == usize::from(node.bank) + 1, cx)
        });
    }
    Ok(())
}

fn set_position(node: &mut Node, position: [f32; 2]) {
    let (frequency, vertical) = group_move::values(node.kind, position, [0.0, 0.0], 48_000);
    node.frequency_hz = frequency;
    match vertical {
        group_move::Vertical::Gain(value) => node.gain_db = value,
        group_move::Vertical::Q(value) => node.q = value,
    }
}

fn event_graph(
    fixture: &mut Fixture,
    event: ResponseGraphEvent,
    cx: &mut Context<Gallery>,
) -> Result<(), String> {
    if event
        .position
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err("invalid eq graph position".into());
    }
    let mut state = current(fixture, cx).ok_or("eq fixture unavailable")?;
    let local = fixture.eq_local.as_mut().ok_or("eq fixture unavailable")?;
    let id = if event.node_id == 0 {
        local.created
    } else {
        NodeId::new(u64::from(event.node_id)).ok()
    };
    match event.phase {
        ResponseGraphPhase::Create => {
            let id = NodeId::new(u64::from(state.next_node_id)).map_err(|e| e.to_string())?;
            let mut node = Node::new(id, 1_000.0, 0.0, 0).map_err(|e| e.to_string())?;
            node.kind = node_geometry::creation_kind(event.position);
            node.bank = local.bank;
            set_position(&mut node, event.position);
            let before = (state.clone(), local.selected);
            state.add(node).map_err(|e| e.to_string())?;
            local.before = Some(before);
            local.selected = Some(id);
            local.created = Some(id);
        }
        ResponseGraphPhase::Select => {
            let Some(id) = id else { return Ok(()) };
            if state.node(id).is_err() {
                return Ok(());
            }
            local.selected = Some(id);
        }
        ResponseGraphPhase::Begin => {
            if event.node_id != 0 {
                local.before = Some((state.clone(), local.selected));
            }
        }
        ResponseGraphPhase::Change | ResponseGraphPhase::Commit => {
            let Some(id) = id else { return Ok(()) };
            let Some(node) = state.nodes.iter_mut().find(|node| node.id == id) else {
                return Ok(());
            };
            set_position(node, event.position);
            if event.phase == ResponseGraphPhase::Commit {
                local.before = None;
            }
        }
        ResponseGraphPhase::Cancel => {
            if let Some((before, selected)) = local.before.take() {
                state = before;
                local.selected = selected;
            }
            local.created = None;
        }
        ResponseGraphPhase::Release | ResponseGraphPhase::ReleaseOutside => {
            local.before = None;
            local.created = None;
        }
        _ => {}
    }
    local.state = state;
    let selected = local
        .selected
        .and_then(|id| local.state.node(id).ok().copied());
    if let Some(node) = selected {
        set_controls(fixture, node, cx)?;
    }
    Ok(())
}

pub(super) fn model(fixture: &Fixture, cx: &gpui::App) -> Option<ResponseGraphModel> {
    let state = current(fixture, cx)?;
    let bank = usize::from(fixture.eq_local.as_ref()?.bank);
    let Ok(authored) = state.encode() else {
        return None;
    };
    let mut curves = sampling::curves(&state, 48_000, |frequencies| {
        hex_devices::native_effect_frequency_response(&authored, 48_000, frequencies).ok()?
    })?;
    if bank == 2 || bank == 4 {
        curves.swap(0, 1);
    }
    let [primary, secondary] = curves;
    let model = ResponseGraphModel {
        primary,
        secondary,
        analyzer: None,
        input_analyzer: None,
        analyzer_peak: None,
        nodes: state
            .nodes
            .iter()
            .filter(|node| usize::from(node.bank) == bank)
            .map(|node| hex_widgets::ResponseGraphNode {
                id: node.id.value(),
                position: node_geometry::node_position(node, 48_000),
                enabled: node.enabled,
                selected: fixture
                    .eq_local
                    .as_ref()
                    .is_some_and(|local| local.selected == Some(node.id)),
            })
            .collect(),
    };
    Some(model)
}

pub(super) fn sync(fixture: &Fixture, disabled: bool, cx: &mut Context<Gallery>) {
    let Some(model) = model(fixture, cx) else {
        return;
    };
    fixture.graph.update(cx, |graph, cx| {
        if disabled {
            graph.cancel(cx);
        }
        graph.set_editable(!disabled, cx);
        graph.set_creation_enabled(!disabled);
        let _ = graph.set_axes(Some(axes::frequency_axes(48000)), cx);
        let _ = graph.set_model(
            model,
            theme::device_style(theme::DeviceStyleRole::Display),
            cx,
        );
    });
}

pub(super) fn body(fixture: &Fixture) -> Div {
    let row = || {
        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .w_full()
            .flex_shrink_0()
    };
    let number = |i, label: &str| {
        row()
            .h(px(16.0))
            .child(
                div()
                    .w(px(if i == 0 { 18.0 } else { 32.0 }))
                    .child(theme::text(label)),
            )
            .child(fixture.number(i))
    };
    let mut banks = row();
    for i in 1..=5 {
        banks = banks.child(fixture.toggle(i, 14.0));
    }
    let controls = div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            row()
                .child(fixture.selector(0, 72.0))
                .child(number(0, "out").w(px(78.0))),
        )
        .child(banks)
        .child(
            row()
                .child(fixture.toggle(6, 44.0))
                .child(fixture.toggle(7, 32.0))
                .child(fixture.toggle(8, 38.0)),
        )
        .child(
            row()
                .child(fixture.toggle(0, 14.0))
                .child(fixture.selector(1, 120.0)),
        )
        .child(number(1, "freq"))
        .child(number(2, "gain"))
        .child(number(3, "q"))
        .child(
            row()
                .child(div().w(px(32.0)).child(theme::text("slope")))
                .child(fixture.selector(2, 104.0)),
        )
        .child(theme::text("fixture · no audio"));
    device_composition::eq_graph_body(
        fixture.graph.clone(),
        device_composition::eq_sidebar(
            "fixture-eq-sidebar",
            controls,
            div().child(theme::text("no audio")),
        ),
    )
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn event(phase: ResponseGraphPhase, node_id: u32, position: [f32; 2]) -> ResponseGraphEvent {
        ResponseGraphEvent {
            phase,
            node_id,
            position,
            modifiers: gpui::Modifiers::default(),
        }
    }

    #[gpui::test]
    fn node_drag_cancel_restores_controls_and_authored_state(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let before = current(fixture, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Begin, 1, [0.5, 0.625]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Change, 1, [0.7, 0.8]),
                cx,
            )
            .unwrap();
            assert_ne!(current(fixture, cx).unwrap(), before);
            event_graph(
                fixture,
                event(ResponseGraphPhase::Cancel, 1, [0.7, 0.8]),
                cx,
            )
            .unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
            assert_eq!(fixture.numbers[2].entity.read(cx).value(), 6.0);
        });
    }

    #[gpui::test]
    fn creation_cancel_restores_original_node_and_selection(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let before = current(fixture, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            assert_eq!(current(fixture, cx).unwrap().nodes.len(), 2);
            event_graph(
                fixture,
                event(ResponseGraphPhase::Change, 0, [0.4, 0.8]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Cancel, 0, [0.4, 0.8]),
                cx,
            )
            .unwrap();
            assert_eq!(current(fixture, cx).unwrap(), before);
            assert_eq!(
                fixture.eq_local.as_ref().unwrap().selected.unwrap().value(),
                1
            );
        });
    }

    #[gpui::test]
    fn selecting_created_nodes_restores_their_controls_and_response(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let before = model(fixture, cx).unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Release, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            fixture.numbers[2].entity.update(cx, |control, cx| {
                control.set_external_value(-12.0, cx).unwrap()
            });
            event_graph(
                fixture,
                event(ResponseGraphPhase::Select, 1, [0.5, 0.6]),
                cx,
            )
            .unwrap();
            assert_eq!(fixture.numbers[2].entity.read(cx).value(), 6.0);
            event_graph(
                fixture,
                event(ResponseGraphPhase::Select, 2, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            assert_eq!(fixture.numbers[2].entity.read(cx).value(), -12.0);
            let after = model(fixture, cx).unwrap();
            assert_eq!(after.nodes.len(), 2);
            assert!(after.nodes[1].selected);
            assert_ne!(after.primary, before.primary);
        });
    }

    #[gpui::test]
    fn switching_banks_keeps_nodes_in_their_authored_bank(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            select_bank(fixture, 1, cx).unwrap();
            assert!(model(fixture, cx).unwrap().nodes.is_empty());
            event_graph(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            event_graph(
                fixture,
                event(ResponseGraphPhase::Release, 0, [0.3, 0.7]),
                cx,
            )
            .unwrap();
            let state = current(fixture, cx).unwrap();
            assert_eq!(
                state.nodes.iter().map(|node| node.bank).collect::<Vec<_>>(),
                [0, 1]
            );
            select_bank(fixture, 0, cx).unwrap();
            let model = model(fixture, cx).unwrap();
            assert_eq!(model.nodes.len(), 1);
            assert_eq!(model.nodes[0].id, 1);
        });
    }

    #[gpui::test]
    fn invalid_control_draft_refuses_graph_creation(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_numeric_control_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let field = visual
            .debug_bounds("fixture-eq-numeric-2-value")
            .unwrap()
            .center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid eq draft");
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = point(
            plot.left() + plot.size.width * 0.25,
            plot.top() + plot.size.height * 0.3,
        );
        visual.simulate_click(position, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(view.error.is_some());
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                1
            );
        });
    }

    #[gpui::test]
    fn bank_buttons_preserve_node_bank_and_keep_one_choice_selected(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let left = visual.debug_bounds("fixture-eq-toggle-2").unwrap().center();
        visual.simulate_click(left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            let fixture = &view.fixtures[&Entry::Eq];
            assert!(fixture.toggles[2].read(cx).checked());
            assert_eq!(current(fixture, cx).unwrap().nodes[0].bank, 0);
            assert!(model(fixture, cx).unwrap().nodes.is_empty());
        });
        let frequency = visual
            .debug_bounds("fixture-eq-numeric-1-value")
            .unwrap()
            .center();
        visual.simulate_click(frequency, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(
                !view.fixtures[&Entry::Eq].numbers[1]
                    .entity
                    .read(cx)
                    .is_editing()
            )
        });
        visual.simulate_click(left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(view.fixtures[&Entry::Eq].toggles[2].read(cx).checked())
        });
    }

    #[gpui::test]
    fn graph_click_creates_a_node_and_existing_node_remains_selectable(cx: &mut TestAppContext) {
        use gpui::{Modifiers, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_response_graph_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let at = |position: [f32; 2]| {
            point(
                plot.left() + plot.size.width * position[0],
                plot.top() + plot.size.height * (1.0 - position[1]),
            )
        };
        visual.simulate_click(at([0.25, 0.7]), Modifiers::default());
        visual.run_until_parked();
        let model_after = view.read_with(cx, |view, cx| {
            model(&view.fixtures[&Entry::Eq], cx).unwrap()
        });
        assert_eq!(model_after.nodes.len(), 2);
        assert!(model_after.nodes[1].selected);
        visual.simulate_click(at(model_after.nodes[0].position), Modifiers::default());
        visual.run_until_parked();
        let selected = view.read_with(cx, |view, cx| {
            model(&view.fixtures[&Entry::Eq], cx).unwrap()
        });
        assert!(selected.nodes[0].selected);
        assert!(!selected.nodes[1].selected);
    }

    #[gpui::test]
    fn graph_creates_and_disabling_during_capture_cancels_preview(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_response_graph_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = point(
            plot.left() + plot.size.width * 0.25,
            plot.top() + plot.size.height * 0.3,
        );
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                2
            )
        });
        visual.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.disabled = true;
                cx.notify();
            })
        });
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                1
            )
        });
    }

    #[gpui::test]
    fn escape_during_graph_creation_restores_prior_nodes(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        cx.update(hex_widgets::bind_response_graph_keys);
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = point(
            plot.left() + plot.size.width * 0.25,
            plot.top() + plot.size.height * 0.3,
        );
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                2
            )
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                1
            )
        });
    }

    #[gpui::test]
    fn leaving_eq_during_capture_cancels_only_the_preview(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.simulate_resize(size(px(1040.0), px(720.0)));
        visual.run_until_parked();
        let graph = view.read_with(cx, |view, _| view.fixtures[&Entry::Eq].graph.clone());
        let plot = graph.read_with(&visual, |graph, _| graph.plot_bounds().unwrap());
        let position = point(
            plot.left() + plot.size.width * 0.25,
            plot.top() + plot.size.height * 0.3,
        );
        visual.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                2
            )
        });
        visual.update(|window, cx| {
            view.update(cx, |view, cx| view.choose_entry(Entry::Utility, window, cx))
        });
        visual.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                model(&view.fixtures[&Entry::Eq], cx).unwrap().nodes.len(),
                1
            )
        });
    }

    #[gpui::test]
    fn cut_and_notch_positions_follow_q(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let (kind, q) = view.read_with(cx, |view, _| {
            let fixture = &view.fixtures[&Entry::Eq];
            (
                fixture.selectors[1].clone(),
                fixture.numbers[3].entity.clone(),
            )
        });
        q.update(cx, |q, cx| q.set_external_value(18.0, cx).unwrap());
        for index in [3, 4, 5] {
            kind.update(cx, |kind, cx| kind.set_selected(index, cx).unwrap());
            let model = view.read_with(cx, |view, cx| {
                model(&view.fixtures[&Entry::Eq], cx).unwrap()
            });
            assert!(
                (model.nodes[0].position[1] - 1.0).abs() < 0.0001,
                "kind {index}: {:?}",
                model.nodes[0].position
            );
        }
    }
}
