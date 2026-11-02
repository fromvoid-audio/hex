//! Functional view-local EQ catalog: shared production geometry/widgets/axes.
//! Registered control-side response only; never fabricates a live analyzer.
use super::*;
use hex_devices::eq::{self, EqState};
#[path = "../../src/editor/device_panel/eq_panel/axes.rs"]
mod axes;
#[path = "../../src/editor/device_panel/eq_panel/node_geometry.rs"]
mod node_geometry;
#[path = "../../src/editor/device_panel/eq_panel/sampling.rs"]
mod sampling;
use hex_widgets::{NumericControlEvent, SelectorChanged, ToggleChanged};

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
                if (1..=5).contains(&index) && event.checked {
                    if let Some(fixture) = view.fixtures.get(&Entry::Eq) {
                        for other in 1..=5 {
                            if other != index {
                                fixture.toggles[other]
                                    .update(cx, |toggle, cx| toggle.set_checked(false, cx));
                            }
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
        ResponseGraph::new(
            cx,
            "fixture-eq-graph",
            theme::device_style(theme::DeviceStyleRole::Display),
        )
    });
    Ok(Fixture {
        entry: Entry::Eq,
        _subscriptions: subscriptions,
        numbers,
        toggles,
        selectors,
        graph,
        discrete_height: 14.0,
    })
}

pub(super) fn model(fixture: &Fixture, cx: &gpui::App) -> Option<ResponseGraphModel> {
    use hex_devices::eq::{Node, NodeId, NodeKind};
    let value = |i: usize| fixture.numbers[i].entity.read(cx).value() as f32;
    let mut state = EqState {
        mode: fixture.selectors[0].read(cx).selected() as u8,
        output_gain: value(0),
        ..Default::default()
    };
    let Ok(id) = NodeId::new(1) else {
        return None;
    };
    let bank = (1..=5)
        .find(|i| fixture.toggles[*i].read(cx).checked())
        .unwrap_or(1)
        - 1;
    let Ok(mut node) = Node::new(id, value(1), value(2), bank as u8) else {
        return None;
    };
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
    if state.add(node).is_err() {
        return None;
    }
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
        nodes: vec![hex_widgets::ResponseGraphNode {
            id: 1,
            position: node_geometry::node_position(&node, 48_000),
            enabled: node.enabled,
            selected: true,
        }],
    };
    Some(model)
}

pub(super) fn sync(fixture: &Fixture, cx: &mut Context<Gallery>) {
    let Some(model) = model(fixture, cx) else {
        return;
    };
    fixture.graph.update(cx, |graph, cx| {
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
