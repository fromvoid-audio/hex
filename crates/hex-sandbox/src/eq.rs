//! Audio-free EQ catalog using the production response graph and shared
//! `hex-ui-kit::eq_panel` axes, sampling, selection, and node geometry.
//! The fixture owns disposable EQ state only; it does not maintain a parallel
//! undo stack, async action queue, or production worker choreography.
use super::*;
use hex_devices::eq::{self, EqState, Node, NodeId, NodeKind};
use hex_ui_kit::eq_panel::{axes, group_move, node_geometry, sampling, selection};
use hex_widgets::{
    NumericControlEvent, NumericEventPhase, ResponseGraphEvent, ResponseGraphMarqueeEvent,
    ResponseGraphMarqueePhase, ResponseGraphPhase, SelectorChanged, ToggleChanged,
};

const SPECTRUM_TOGGLE: usize = 6;
const INPUT_TOGGLE: usize = 7;
pub(super) const AUDITION_TOGGLE: usize = 8;

const NODE_KINDS: [NodeKind; 6] = [
    NodeKind::Bell,
    NodeKind::LowShelf,
    NodeKind::HighShelf,
    NodeKind::LowCut,
    NodeKind::HighCut,
    NodeKind::Notch,
];

/// Ephemeral catalog state; it never owns a project, history stack, or audio resource.
pub(super) struct Local {
    state: EqState,
    selected: Option<NodeId>,
    bank: u8,
    group: Vec<NodeId>,
    drag_before: Option<(EqState, Option<NodeId>, Vec<NodeId>)>,
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
            group: Vec::new(),
            drag_before: None,
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
            20_000.0,
            1_000.0,
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
        subscriptions.push(cx.subscribe(
            &entity,
            move |view, _, event: &NumericControlEvent, cx| {
                if !matches!(event.phase, NumericEventPhase::Begin)
                    && let Some(fixture) = view.fixtures.get_mut(&Entry::Eq)
                {
                    apply_numeric(fixture, index, event.value);
                }
                cx.notify();
            },
        ));
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
                if index == 0
                    && let Some(fixture) = view.fixtures.get_mut(&Entry::Eq)
                    && let Some(local) = fixture.eq_local.as_mut()
                    && let Some(node) = local
                        .selected
                        .and_then(|id| local.state.nodes.iter_mut().find(|n| n.id == id))
                {
                    node.enabled = event.checked;
                } else if (1..=5).contains(&index)
                    && let Some(fixture) = view.fixtures.get_mut(&Entry::Eq)
                {
                    if event.checked {
                        let bank = (index - 1) as u8;
                        if let Err(error) = select_bank(fixture, bank, cx) {
                            view.error = Some(error);
                        }
                    }
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
        subscriptions.push(
            cx.subscribe(&entity, move |view, _, event: &SelectorChanged, cx| {
                if let Some(fixture) = view.fixtures.get_mut(&Entry::Eq)
                    && let Some(local) = fixture.eq_local.as_mut()
                {
                    match index {
                        0 if event.index <= 2 => local.state.mode = event.index as u8,
                        1 => {
                            if let (Some(&kind), Some(node)) = (
                                NODE_KINDS.get(event.index),
                                local.selected.and_then(|id| {
                                    local.state.nodes.iter_mut().find(|n| n.id == id)
                                }),
                            ) {
                                node.kind = kind;
                            }
                        }
                        2 if event.index <= 3 => {
                            if let Some(node) = local
                                .selected
                                .and_then(|id| local.state.nodes.iter_mut().find(|n| n.id == id))
                            {
                                node.slope = event.index as u8;
                            }
                        }
                        _ => {}
                    }
                }
                cx.notify();
            }),
        );
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
        graph.set_backspace_removal(true);
        graph.set_blank_marquee_enabled(true, cx);
        graph
    });
    subscriptions.push(
        cx.subscribe(&graph, |view, _, event: &ResponseGraphEvent, cx| {
            let cancel = event.phase == ResponseGraphPhase::Cancel;
            if !cancel && (view.disabled || view.entry != Entry::Eq) {
                return;
            }
            if let Some(fixture) = view.fixtures.get_mut(&Entry::Eq)
                && let Err(error) = handle_graph_event(fixture, *event, cx)
            {
                fixture.graph.update(cx, |graph, cx| graph.cancel(cx));
                view.error = Some(error);
            }
            cx.notify();
        }),
    );
    subscriptions.push(
        cx.subscribe(&graph, |view, _, event: &ResponseGraphMarqueeEvent, cx| {
            if event.phase != ResponseGraphMarqueePhase::Cancel
                && (view.disabled || view.entry != Entry::Eq)
            {
                return;
            }
            if let Some(fixture) = view.fixtures.get_mut(&Entry::Eq)
                && let Err(error) = handle_marquee(fixture, event, cx)
            {
                view.error = Some(error);
            }
            cx.notify();
        }),
    );
    let mut fixture = Fixture {
        entry: Entry::Eq,
        _subscriptions: subscriptions,
        numbers,
        toggles,
        selectors,
        graph,
        eq_local: Some(Local::new()?),
        discrete_height: 14.0,
    };
    if let Some(state) = current(&fixture, cx)
        && let Some(local) = fixture.eq_local.as_mut()
    {
        local.state = state;
    }
    Ok(fixture)
}

pub(super) fn apply_numeric(fixture: &mut Fixture, index: usize, value: f64) {
    let Some(local) = fixture.eq_local.as_mut() else {
        return;
    };
    let value = value as f32;
    if index == 0 {
        local.state.output_gain = value;
    } else if let Some(node) = local
        .selected
        .and_then(|id| local.state.nodes.iter_mut().find(|n| n.id == id))
    {
        match index {
            1 => node.frequency_hz = value,
            2 => node.gain_db = value,
            3 => node.q = value,
            _ => {}
        }
    }
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
        node.kind = NODE_KINDS[fixture.selectors[1].read(cx).selected()];
    }
    Some(state)
}

fn select_bank(fixture: &mut Fixture, bank: u8, cx: &mut Context<Gallery>) -> Result<(), String> {
    let state = current(fixture, cx).ok_or("eq fixture unavailable")?;
    let local = fixture.eq_local.as_mut().ok_or("eq fixture unavailable")?;
    local.state = state;
    local.bank = bank;
    local.group.clear();
    local.selected = local
        .state
        .nodes
        .iter()
        .find(|node| node.bank == bank)
        .map(|node| node.id);
    if let Some(node) = local
        .selected
        .and_then(|id| local.state.node(id).ok().copied())
    {
        set_controls(fixture, node, cx)?;
    }
    Ok(())
}

fn handle_marquee(
    fixture: &mut Fixture,
    event: &ResponseGraphMarqueeEvent,
    cx: &mut Context<Gallery>,
) -> Result<(), String> {
    let state = current(fixture, cx).ok_or("eq fixture unavailable")?;
    let local = fixture.eq_local.as_mut().ok_or("eq fixture unavailable")?;
    local.state = state;
    let members: Vec<_> = event
        .selected
        .iter()
        .copied()
        .filter(|raw| {
            NodeId::new(u64::from(*raw))
                .ok()
                .and_then(|id| local.state.node(id).ok())
                .is_some_and(|node| node.bank == local.bank)
        })
        .collect();
    selection::apply_marquee(&mut local.selected, &mut local.group, &members);
    if let Some(node) = local
        .selected
        .and_then(|id| local.state.node(id).ok().copied())
    {
        set_controls(fixture, node, cx)?;
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

fn handle_graph_event(
    fixture: &mut Fixture,
    event: ResponseGraphEvent,
    cx: &mut Context<Gallery>,
) -> Result<(), String> {
    let mut state = current(fixture, cx).ok_or("eq fixture unavailable")?;
    let local = fixture.eq_local.as_mut().ok_or("eq fixture unavailable")?;
    match event.phase {
        ResponseGraphPhase::Create => {
            if state.nodes.len() >= eq::MAX_NODES {
                return Err("eq band limit reached".into());
            }
            let id = NodeId::new(u64::from(state.next_node_id)).map_err(|e| e.to_string())?;
            let mut node = Node::new(id, 1_000.0, 0.0, local.bank).map_err(|e| e.to_string())?;
            node.kind = node_geometry::creation_kind(event.position);
            set_position(&mut node, event.position);
            state.add(node).map_err(|e| e.to_string())?;
            local.state = state;
            local.selected = Some(id);
            local.group.clear();
            set_controls(fixture, node, cx)?;
        }
        ResponseGraphPhase::Select => {
            let id = NodeId::new(u64::from(event.node_id)).map_err(|e| e.to_string())?;
            let node = *state.node(id).map_err(|e| e.to_string())?;
            local.state = state;
            local.selected = Some(id);
            local.group.clear();
            set_controls(fixture, node, cx)?;
        }
        ResponseGraphPhase::Remove => {
            let id = NodeId::new(u64::from(event.node_id)).map_err(|e| e.to_string())?;
            state.remove(id).map_err(|e| e.to_string())?;
            local.state = state;
            local.group.retain(|member| *member != id);
            local.selected = local
                .state
                .nodes
                .iter()
                .find(|node| node.bank == local.bank)
                .map(|node| node.id);
            if let Some(node) = local
                .selected
                .and_then(|id| local.state.node(id).ok().copied())
            {
                set_controls(fixture, node, cx)?;
            }
        }
        ResponseGraphPhase::Begin => {
            let id = NodeId::new(u64::from(event.node_id)).map_err(|e| e.to_string())?;
            local.drag_before = Some((state.clone(), local.selected, local.group.clone()));
            local.selected = Some(id);
            local.state = state;
        }
        ResponseGraphPhase::Change | ResponseGraphPhase::Commit => {
            let id = NodeId::new(u64::from(event.node_id)).map_err(|e| e.to_string())?;
            let mut updated = None;
            if let Some(node) = state.nodes.iter_mut().find(|n| n.id == id) {
                set_position(node, event.position);
                updated = Some(*node);
            }
            local.state = state;
            if event.phase == ResponseGraphPhase::Commit {
                local.drag_before = None;
            }
            if let Some(node) = updated {
                set_controls(fixture, node, cx)?;
            }
        }
        ResponseGraphPhase::Cancel => {
            if let Some((before_state, selected, group)) = local.drag_before.take() {
                local.state = before_state;
                local.selected = selected;
                local.group = group;
                if let Some(node) = local
                    .selected
                    .and_then(|id| local.state.node(id).ok().copied())
                {
                    set_controls(fixture, node, cx)?;
                }
            }
        }
        ResponseGraphPhase::Press
        | ResponseGraphPhase::Release
        | ResponseGraphPhase::ReleaseOutside => {}
    }
    Ok(())
}

fn set_controls(fixture: &Fixture, node: Node, cx: &mut Context<Gallery>) -> Result<(), String> {
    for (index, value) in [
        (1, f64::from(node.frequency_hz)),
        (2, f64::from(node.gain_db)),
        (3, f64::from(node.q)),
    ] {
        fixture.numbers[index]
            .entity
            .update(cx, |control, cx| control.set_external_value(value, cx))
            .map_err(|e| e.to_string())?;
    }
    fixture.toggles[0].update(cx, |toggle, cx| toggle.set_checked(node.enabled, cx));
    let kind_index = NODE_KINDS
        .iter()
        .position(|kind| *kind == node.kind)
        .unwrap_or(0);
    fixture.selectors[1]
        .update(cx, |selector, cx| selector.set_selected(kind_index, cx))
        .map_err(str::to_owned)?;
    fixture.selectors[2]
        .update(cx, |selector, cx| {
            selector.set_selected(usize::from(node.slope), cx)
        })
        .map_err(str::to_owned)?;
    Ok(())
}

pub(super) fn model(fixture: &Fixture, cx: &gpui::App) -> Option<ResponseGraphModel> {
    let state = current(fixture, cx)?;
    let bank = usize::from(fixture.eq_local.as_ref()?.bank);
    let authored = state.encode().ok()?;
    let mut curves = sampling::curves(&state, 48_000, |frequencies| {
        hex_devices::native_effect_frequency_response(&authored, 48_000, frequencies).ok()?
    })?;
    if bank == 2 || bank == 4 {
        curves.swap(0, 1);
    }
    let [primary, secondary] = curves;
    let (analyzer, input_analyzer, analyzer_peak) = super::eq_spectrum::layers(
        fixture.toggles[SPECTRUM_TOGGLE].read(cx).checked(),
        fixture.toggles[INPUT_TOGGLE].read(cx).checked(),
    );
    Some(ResponseGraphModel {
        primary,
        secondary,
        analyzer,
        input_analyzer,
        analyzer_peak,
        nodes: state
            .nodes
            .iter()
            .filter(|node| usize::from(node.bank) == bank)
            .map(|node| hex_widgets::ResponseGraphNode {
                accent: None,
                id: node.id.value(),
                position: node_geometry::node_position(node, 48_000),
                enabled: node.enabled,
                selected: fixture.eq_local.as_ref().is_some_and(|local| {
                    local.selected == Some(node.id) || local.group.contains(&node.id)
                }),
            })
            .collect(),
    })
}

pub(super) fn sync(fixture: &Fixture, disabled: bool, cx: &mut Context<Gallery>) {
    let Some(model) = model(fixture, cx) else {
        return;
    };
    let show_hint = !disabled
        && fixture.eq_local.as_ref().is_some_and(|local| {
            local.drag_before.is_none() && local.state.nodes.len() < eq::MAX_NODES
        });
    fixture.graph.update(cx, |graph, cx| {
        if disabled {
            graph.cancel(cx);
        }
        graph.set_editable(!disabled, cx);
        graph.set_creation_enabled(!disabled);
        graph.set_creation_hint_resolver(show_hint.then_some(node_geometry::creation_hint), cx);
        graph.set_blank_marquee_enabled(!disabled, cx);
        let _ = graph.set_axes(Some(axes::frequency_axes(48_000)), cx);
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

    #[gpui::test]
    fn eq_fixture_exercises_shared_graph_create_drag_cancel_and_remove(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| {
            let mut view = Gallery::new(cx);
            view.entry = Entry::Eq;
            view.ensure_fixture(cx);
            view
        });
        let view = window.root(cx).unwrap();
        let event = |phase, node_id, position| ResponseGraphEvent {
            phase,
            node_id,
            position,
            modifiers: gpui::Modifiers::default(),
        };

        // Create a second node using shared eq_panel geometry.
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            handle_graph_event(
                fixture,
                event(ResponseGraphPhase::Create, 0, [0.25, 0.75]),
                cx,
            )
            .unwrap();
            assert_eq!(fixture.eq_local.as_ref().unwrap().state.nodes.len(), 2);
        });

        // Begin, change, and cancel restores pre-drag state.
        let before = view.read_with(cx, |view, _| {
            view.fixtures[&Entry::Eq]
                .eq_local
                .as_ref()
                .unwrap()
                .state
                .clone()
        });
        view.update(cx, |view, cx| {
            let fixture = view.fixtures.get_mut(&Entry::Eq).unwrap();
            let id = fixture.eq_local.as_ref().unwrap().selected.unwrap().value();
            handle_graph_event(
                fixture,
                event(ResponseGraphPhase::Begin, id, [0.25, 0.75]),
                cx,
            )
            .unwrap();
            handle_graph_event(
                fixture,
                event(ResponseGraphPhase::Change, id, [0.6, 0.3]),
                cx,
            )
            .unwrap();
            handle_graph_event(
                fixture,
                event(ResponseGraphPhase::Cancel, id, [0.6, 0.3]),
                cx,
            )
            .unwrap();
            assert_eq!(fixture.eq_local.as_ref().unwrap().state, before);
            handle_graph_event(
                fixture,
                event(ResponseGraphPhase::Remove, id, [0.25, 0.75]),
                cx,
            )
            .unwrap();
            assert_eq!(fixture.eq_local.as_ref().unwrap().state.nodes.len(), 1);
        });
    }
}
