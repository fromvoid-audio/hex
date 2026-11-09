//! Editable Operator envelope graphs translated through canonical device gestures.
use super::super::*;
use super::id;
use hex_session::device_gesture::{GestureTarget, ParameterPairValue};
use hex_widgets::{
    ResponseGraph, ResponseGraphEvent, ResponseGraphModel, ResponseGraphNode,
    ResponseGraphReference,
};

const STAGE_WIDTH: f32 = 1.0 / 3.0;
const MIN_DECAY_RELEASE_SECONDS: f32 = 0.001;
const ATTACK_LIMIT_SECONDS: f32 = 20.0;
const DECAY_LIMIT_SECONDS: f32 = 60.0;
const RELEASE_LIMIT_SECONDS: f32 = 60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::editor::device_panel) struct GraphKey {
    device: DeviceId,
    operator: usize,
}

pub(in crate::editor::device_panel) struct GraphControl {
    entity: Entity<ResponseGraph>,
    selected_node: u32,
    model_input: Option<ModelInput>,
    reference_input: Option<ReferenceInput>,
    _subscription: Subscription,
}

#[derive(Clone, Copy, PartialEq)]
struct ModelInput {
    values: [f32; 6],
    selected: u32,
    style: hex_widgets::ControlStyle,
}

#[derive(Clone, Copy, PartialEq)]
struct ReferenceInput {
    envelopes: [[f32; 6]; 3],
    colors: [u32; 3],
    enabled: [bool; 3],
}

#[derive(Clone, Copy)]
pub(in crate::editor::device_panel) struct GraphGesture {
    node_id: u32,
    initial: [f32; 6],
}

pub(super) fn graph(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    operator: usize,
    values: [f32; 6],
    parameters: &[NativeDeviceParameterView],
) -> Stateful<Div> {
    let key = GraphKey { device, operator };
    view.device_panel
        .operator_envelope_graphs
        .entry(key)
        .or_insert_with(|| {
            let style = super::display_style();
            let entity = cx.new(|graph_cx| {
                let mut graph = ResponseGraph::new(
                    graph_cx,
                    format!("operator-envelope-{}-{operator}", device.value()),
                    style,
                );
                graph.set_editable(true, graph_cx);
                graph.set_tab_navigation(true, graph_cx);
                graph
            });
            let subscription =
                cx.subscribe(&entity, move |view, _, event: &ResponseGraphEvent, cx| {
                    handle_event(view, key, *event, cx);
                });
            GraphControl {
                entity,
                selected_node: 1,
                model_input: None,
                reference_input: None,
                _subscription: subscription,
            }
        });
    let selected = view
        .device_panel
        .operator_envelope_graphs
        .get(&key)
        .map_or(1, |graph| graph.selected_node);
    let input = ModelInput {
        values,
        selected,
        style: super::display_style(),
    };
    let others: [usize; 3] =
        std::array::from_fn(|index| if index < operator { index } else { index + 1 });
    let reference_input = ReferenceInput {
        envelopes: others.map(|other| {
            std::array::from_fn(|field| {
                super::value(
                    view,
                    device,
                    parameters,
                    id(33 + other as u32 * 6 + field as u32),
                )
                .and_then(|value| match value {
                    ParameterValue::Float(value) if value.is_finite() => Some(value),
                    _ => None,
                })
                .unwrap_or(0.0)
            })
        }),
        colors: others
            .map(|other| theme::oscillator_identity_color(other).unwrap_or(input.style.muted)),
        enabled: others.map(|other| {
            super::value(view, device, parameters, id(513 + other as u32))
                == Some(ParameterValue::Bool(true))
        }),
    };
    let Some(control) = view.device_panel.operator_envelope_graphs.get_mut(&key) else {
        return div().id("operator-envelope-graph-unavailable");
    };
    let graph_entity = control.entity.clone();
    if control.model_input != Some(input) {
        let model = graph_model(values, selected);
        let updated = graph_entity.update(cx, |graph, cx| {
            graph.set_model(model, input.style, cx).is_ok()
        });
        if updated {
            control.model_input = Some(input);
        }
    }
    if control.reference_input != Some(reference_input) {
        let references = (0..3)
            .map(|index| ResponseGraphReference {
                points: graph_model(reference_input.envelopes[index], 0)
                    .primary
                    .into(),
                color: reference_input.colors[index],
                opacity: if reference_input.enabled[index] {
                    0.28
                } else {
                    0.12
                },
            })
            .collect();
        if graph_entity.update(cx, |graph, cx| graph.set_references(references, cx).is_ok()) {
            control.reference_input = Some(reference_input);
        }
    }
    div()
        .id(ElementId::Name(
            format!("operator-envelope-graph-{}-{operator}", device.value()).into(),
        ))
        .debug_selector(|| "operator-envelope-graph".into())
        .relative()
        .w_full()
        .h(px(86.0))
        .child(graph_entity)
}

// Fixed stage fractions, independent of all other parameters. The 1ms pivot
// keeps short attacks usable; decay/release use their authored 1ms minimum.
// This logarithmic display law is original, not measured Live numerical parity.
fn stage_width(seconds: f32, min: f32, max: f32) -> f32 {
    let seconds = f64::from(seconds.clamp(min, max));
    let fraction = if min == 0.0 {
        (seconds / 0.001).ln_1p() / (f64::from(max) / 0.001).ln_1p()
    } else {
        (seconds / f64::from(min)).ln() / (f64::from(max) / f64::from(min)).ln()
    };
    fraction as f32 * STAGE_WIDTH
}

fn stage_seconds(width: f32, min: f32, max: f32) -> f32 {
    let fraction = f64::from((width / STAGE_WIDTH).clamp(0.0, 1.0));
    if fraction == 0.0 {
        return min;
    }
    // Subtracting the fixed 2/3 anchor can lose an f32 ulp at the right edge.
    if fraction >= 1.0 - f64::from(f32::EPSILON) * 2.0 {
        return max;
    }
    let seconds = if min == 0.0 {
        (fraction * (f64::from(max) / 0.001).ln_1p()).exp_m1() * 0.001
    } else {
        f64::from(min) * (fraction * (f64::from(max) / f64::from(min)).ln()).exp()
    };
    (seconds as f32).clamp(min, max)
}

fn graph_model(values: [f32; 6], selected: u32) -> ResponseGraphModel {
    let [initial, peak, sustain, attack, decay, release] = values;
    let x_attack = stage_width(attack, 0.0, ATTACK_LIMIT_SECONDS);
    let x_decay = x_attack + stage_width(decay, MIN_DECAY_RELEASE_SECONDS, DECAY_LIMIT_SECONDS);
    let x_release = STAGE_WIDTH * 2.0;
    let x_end = x_release + stage_width(release, MIN_DECAY_RELEASE_SECONDS, RELEASE_LIMIT_SECONDS);
    // Match the existing original DSP laws: linear attack, exponential
    // decay/release with a 1e-5 relative cutoff and exact stage endpoints.
    // Stage widths use the display mapping; normalized stage shape retains the
    // original DSP law. This plot is not a uniform time axis.
    let mut points = Vec::with_capacity(68);
    points.extend([
        [0.0, initial.clamp(0.0, 1.0)],
        [x_attack, peak.clamp(0.0, 1.0)],
    ]);
    append_decay(&mut points, x_attack, x_decay, peak, sustain);
    points.push([x_release, sustain.clamp(0.0, 1.0)]);
    append_decay(&mut points, x_release, x_end, sustain, 0.0);
    ResponseGraphModel {
        primary: points.clone(),
        secondary: points,
        analyzer: None,
        input_analyzer: None,
        analyzer_peak: None,
        nodes: vec![
            node(0, [0.0, initial], selected),
            node(1, [x_attack, peak], selected),
            node(2, [x_decay, sustain], selected),
            node(3, [x_end, 0.0], selected),
        ],
    }
}

// Fixed original exponential shape at 1/32-stage intervals, computed once
// independently of parameter values. Curve samples do not evaluate powers.
const DECAY_FACTORS: [f32; 31] = [
    6.978306e-01,
    4.869675e-01,
    3.398208e-01,
    2.371374e-01,
    1.654817e-01,
    1.154782e-01,
    8.058422e-02,
    5.623413e-02,
    3.92419e-02,
    2.73842e-02,
    1.910953e-02,
    1.333521e-02,
    9.30572e-03,
    6.493816e-03,
    4.531584e-03,
    3.162278e-03,
    2.206734e-03,
    1.539927e-03,
    1.074608e-03,
    7.498942e-04,
    5.232991e-04,
    3.651741e-04,
    2.548297e-04,
    1.778279e-04,
    1.240938e-04,
    8.659643e-05,
    6.042964e-05,
    4.216965e-05,
    2.942727e-05,
    2.053525e-05,
    1.433013e-05,
];

/// Bounded drawing samples; the owner supplies normalized stage endpoints.
fn append_decay(points: &mut Vec<[f32; 2]>, start: f32, end: f32, from: f32, to: f32) {
    if end > start {
        for (index, factor) in DECAY_FACTORS.iter().enumerate() {
            let t = (index + 1) as f32 / 32.0;
            let level = to + (from - to) * factor;
            points.push([start + (end - start) * t, level.clamp(0.0, 1.0)]);
        }
    }
    points.push([end, to.clamp(0.0, 1.0)]);
}

fn node(id: u32, position: [f32; 2], selected: u32) -> ResponseGraphNode {
    ResponseGraphNode {
        id,
        position: [position[0].clamp(0.0, 1.0), position[1].clamp(0.0, 1.0)],
        enabled: true,
        selected: id == selected,
    }
}

fn handle_event(
    view: &mut EditorView,
    key: GraphKey,
    event: ResponseGraphEvent,
    cx: &mut Context<EditorView>,
) {
    if event.phase == hex_widgets::ResponseGraphPhase::Select {
        if event.node_id > 3 || !finalize_controls(view, cx) {
            return;
        }
        if let Some(graph) = view.device_panel.operator_envelope_graphs.get_mut(&key) {
            graph.selected_node = event.node_id;
        }
        cx.notify();
        return;
    }
    if matches!(
        event.phase,
        hex_widgets::ResponseGraphPhase::Press
            | hex_widgets::ResponseGraphPhase::Release
            | hex_widgets::ResponseGraphPhase::ReleaseOutside
    ) {
        return;
    }
    if event.phase == hex_widgets::ResponseGraphPhase::Begin {
        begin_gesture(view, key, event, cx);
        return;
    }
    let Some(gesture) = view
        .device_panel
        .operator_envelope_gestures
        .get(&key)
        .copied()
    else {
        return;
    };
    if gesture.node_id != event.node_id {
        return;
    }
    if event.phase == hex_widgets::ResponseGraphPhase::Cancel {
        cancel_key_gesture(view, key);
        cx.notify();
        return;
    }
    let Some((target, values)) = edit_values(key.operator, gesture, event.position) else {
        return;
    };
    store_pending(view, key, target, values);
    let result = if matches!(event.phase, hex_widgets::ResponseGraphPhase::Commit) {
        match values {
            EditValues::Single(value) => view.finish_device_gesture(value),
            EditValues::Pair(value) => view.finish_device_pair_gesture(value),
        }
    } else {
        match values {
            EditValues::Single(value) => view.change_device_gesture(value),
            EditValues::Pair(value) => view.change_device_pair_gesture(value),
        }
    };
    if let Err(error) = &result {
        view.device_panel.message = Some((*error).to_owned());
    }
    if event.phase == hex_widgets::ResponseGraphPhase::Commit {
        view.device_panel.operator_envelope_gestures.remove(&key);
    }
    if event.phase == hex_widgets::ResponseGraphPhase::Change && result.is_ok() {
        // Update the retained graph/readout entities in the input turn. A drag
        // must not wait for rebuilding the entire editor to derive its geometry.
        update_drag_display(view, key, gesture, cx);
    } else {
        cx.notify();
    }
}

fn update_drag_display(
    view: &mut EditorView,
    key: GraphKey,
    gesture: GraphGesture,
    cx: &mut Context<EditorView>,
) {
    let base = 33 + key.operator as u32 * 6;
    let mut values = gesture.initial;
    for (index, value) in values.iter_mut().enumerate() {
        let parameter = ParameterKey {
            device: key.device,
            parameter: id(base + index as u32),
        };
        if let Some(ParameterValue::Float(pending)) =
            view.device_panel.pending_values.get(&parameter)
        {
            *value = *pending;
        }
        if let Some(super::super::ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&parameter)
        {
            entity.update(cx, |control, cx| {
                let _ = control.set_external_value(f64::from(*value), cx);
            });
        }
    }
    if let Some(control) = view.device_panel.operator_envelope_graphs.get_mut(&key) {
        let input = ModelInput {
            values,
            selected: control.selected_node,
            style: super::display_style(),
        };
        if control.model_input != Some(input) {
            let model = graph_model(values, input.selected);
            if control.entity.update(cx, |graph, cx| {
                graph.set_model(model, input.style, cx).is_ok()
            }) {
                control.model_input = Some(input);
            }
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor::device_panel) fn dispatch_change(
    view: &mut EditorView,
    device: DeviceId,
    operator: usize,
    node_id: u32,
    position: [f32; 2],
    cx: &mut Context<EditorView>,
) {
    handle_event(
        view,
        GraphKey { device, operator },
        ResponseGraphEvent {
            phase: hex_widgets::ResponseGraphPhase::Change,
            node_id,
            position,
            modifiers: gpui::Modifiers::default(),
        },
        cx,
    );
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor::device_panel) fn displayed_node(
    view: &EditorView,
    device: DeviceId,
    operator: usize,
    node: u32,
    cx: &gpui::App,
) -> Option<[f32; 2]> {
    view.device_panel
        .operator_envelope_graphs
        .get(&GraphKey { device, operator })?
        .entity
        .read(cx)
        .node_position(node)
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor::device_panel) fn displayed_references(
    view: &EditorView,
    device: DeviceId,
    operator: usize,
    cx: &gpui::App,
) -> Option<Vec<ResponseGraphReference>> {
    Some(
        view.device_panel
            .operator_envelope_graphs
            .get(&GraphKey { device, operator })?
            .entity
            .read(cx)
            .references()
            .to_vec(),
    )
}

fn begin_gesture(
    view: &mut EditorView,
    key: GraphKey,
    event: ResponseGraphEvent,
    cx: &mut Context<EditorView>,
) {
    if event.node_id > 3 || !finalize_controls(view, cx) || view.device_gesture.snapshot().is_some()
    {
        return;
    }
    let Some([initial, peak, sustain, attack, decay, release]) = envelope_values(view, key) else {
        return;
    };
    let base = 33 + key.operator as u32 * 6;
    let gesture = GraphGesture {
        node_id: event.node_id,
        initial: [initial, peak, sustain, attack, decay, release],
    };
    let begin_result = match event.node_id {
        0 => view.begin_device_gesture(
            GestureTarget::DeviceParameter {
                device: key.device,
                parameter: id(base),
            },
            ParameterValue::Float(initial),
        ),
        1 => view.begin_device_pair_gesture(
            key.device,
            id(base + 1),
            id(base + 3),
            ParameterPairValue {
                first: ParameterValue::Float(peak),
                second: ParameterValue::Float(attack),
            },
        ),
        2 => view.begin_device_pair_gesture(
            key.device,
            id(base + 2),
            id(base + 4),
            ParameterPairValue {
                first: ParameterValue::Float(sustain),
                second: ParameterValue::Float(decay),
            },
        ),
        3 => view.begin_device_gesture(
            GestureTarget::DeviceParameter {
                device: key.device,
                parameter: id(base + 5),
            },
            ParameterValue::Float(release),
        ),
        _ => return,
    };
    if let Err(error) = begin_result {
        view.device_panel.message = Some(error.to_owned());
        return;
    }
    view.device_panel
        .operator_envelope_gestures
        .insert(key, gesture);
    if let Some(graph) = view.device_panel.operator_envelope_graphs.get_mut(&key) {
        graph.selected_node = event.node_id;
    }
    let Some((target, values)) = edit_values(key.operator, gesture, event.position) else {
        let _ = view.cancel_device_gesture();
        view.device_panel.operator_envelope_gestures.remove(&key);
        return;
    };
    store_pending(view, key, target, values);
    cx.notify();
}

#[derive(Clone, Copy)]
enum EditValues {
    Single(ParameterValue),
    Pair(ParameterPairValue),
}

fn edit_values(
    operator: usize,
    gesture: GraphGesture,
    [x, y]: [f32; 2],
) -> Option<(u32, EditValues)> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let attack = gesture.initial[3];
    let x = x.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let base = 33 + operator as u32 * 6;
    Some(match gesture.node_id {
        0 => (base, EditValues::Single(ParameterValue::Float(y))),
        1 => {
            let next_attack = stage_seconds(x, 0.0, ATTACK_LIMIT_SECONDS);
            (
                base + 1,
                EditValues::Pair(ParameterPairValue {
                    first: ParameterValue::Float(y),
                    second: ParameterValue::Float(next_attack),
                }),
            )
        }
        2 => {
            let next_decay = stage_seconds(
                x - stage_width(attack, 0.0, ATTACK_LIMIT_SECONDS),
                MIN_DECAY_RELEASE_SECONDS,
                DECAY_LIMIT_SECONDS,
            );
            (
                base + 2,
                EditValues::Pair(ParameterPairValue {
                    first: ParameterValue::Float(y),
                    second: ParameterValue::Float(next_decay),
                }),
            )
        }
        3 => {
            let next_release = stage_seconds(
                x - STAGE_WIDTH * 2.0,
                MIN_DECAY_RELEASE_SECONDS,
                RELEASE_LIMIT_SECONDS,
            );
            (
                base + 5,
                EditValues::Single(ParameterValue::Float(next_release)),
            )
        }
        _ => return None,
    })
}

fn store_pending(view: &mut EditorView, key: GraphKey, target: u32, values: EditValues) {
    let base = 33 + key.operator as u32 * 6;
    match values {
        EditValues::Single(value) => {
            view.device_panel.pending_values.insert(
                ParameterKey {
                    device: key.device,
                    parameter: id(target),
                },
                value,
            );
        }
        EditValues::Pair(pair) => {
            let second = match target {
                parameter if parameter == base + 1 => base + 3,
                parameter if parameter == base + 2 => base + 4,
                _ => return,
            };
            view.device_panel.pending_values.insert(
                ParameterKey {
                    device: key.device,
                    parameter: id(target),
                },
                pair.first,
            );
            view.device_panel.pending_values.insert(
                ParameterKey {
                    device: key.device,
                    parameter: id(second),
                },
                pair.second,
            );
        }
    }
}

fn clear_pending(view: &mut EditorView, key: GraphKey, node_id: u32) {
    let base = 33 + key.operator as u32 * 6;
    let indices: &[u32] = match node_id {
        0 => &[0],
        1 => &[1, 3],
        2 => &[2, 4],
        3 => &[5],
        _ => &[],
    };
    for index in indices {
        view.device_panel.pending_values.remove(&ParameterKey {
            device: key.device,
            parameter: id(base + index),
        });
    }
}

fn envelope_values(view: &mut EditorView, key: GraphKey) -> Option<[f32; 6]> {
    let parameters = view
        .device_panel
        .views
        .get(&key.device)
        .and_then(|(_, _, view)| view.as_ref().ok())?
        .parameters
        .clone();
    let base = 33 + key.operator as u32 * 6;
    let mut values = [0.0; 6];
    for (index, value) in values.iter_mut().enumerate() {
        *value = match super::value(view, key.device, &parameters, id(base + index as u32))? {
            ParameterValue::Float(value) if value.is_finite() => value,
            _ => return None,
        };
    }
    Some(values)
}

pub(in crate::editor::device_panel) fn retire_removed(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    alive: &HashSet<DeviceId>,
) {
    let stale: Vec<_> = view
        .device_panel
        .operator_envelope_graphs
        .keys()
        .filter(|key| !alive.contains(&key.device))
        .copied()
        .collect();
    for key in stale {
        cancel_key_gesture(view, key);
        if let Some(graph) = view.device_panel.operator_envelope_graphs.remove(&key) {
            graph.entity.update(cx, |graph, cx| graph.cancel(cx));
        }
    }
}

pub(in crate::editor::device_panel) fn retire_hidden(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    visible_operator: Option<usize>,
) {
    let stale: Vec<_> = view
        .device_panel
        .operator_envelope_graphs
        .keys()
        .filter(|key| key.device == device && Some(key.operator) != visible_operator)
        .copied()
        .collect();
    for key in stale {
        cancel_key_gesture(view, key);
        if let Some(graph) = view.device_panel.operator_envelope_graphs.remove(&key) {
            graph.entity.update(cx, |graph, cx| graph.cancel(cx));
        }
    }
}

pub(in crate::editor::device_panel) fn cancel_all(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) {
    let active: Vec<_> = view
        .device_panel
        .operator_envelope_gestures
        .keys()
        .copied()
        .collect();
    for key in active {
        cancel_key_gesture(view, key);
    }
    let graphs = std::mem::take(&mut view.device_panel.operator_envelope_graphs);
    for (_, graph) in graphs {
        graph.entity.update(cx, |graph, cx| graph.cancel(cx));
    }
    view.device_panel.operator_envelope_gestures.clear();
}

fn cancel_key_gesture(view: &mut EditorView, key: GraphKey) {
    let Some(gesture) = view.device_panel.operator_envelope_gestures.remove(&key) else {
        return;
    };
    clear_pending(view, key, gesture.node_id);
    let Some(snapshot) = view.device_gesture.snapshot() else {
        return;
    };
    if gesture_target_matches(key, gesture.node_id, snapshot.target) {
        let _ = view.cancel_device_gesture();
    }
}

fn gesture_target_matches(key: GraphKey, node_id: u32, target: GestureTarget) -> bool {
    let base = 33 + key.operator as u32 * 6;
    match (node_id, target) {
        (0, GestureTarget::DeviceParameter { device, parameter }) => {
            device == key.device && parameter == id(base)
        }
        (
            1,
            GestureTarget::DeviceParameterPair {
                device,
                first,
                second,
            },
        ) => device == key.device && first == id(base + 1) && second == id(base + 3),
        (
            2,
            GestureTarget::DeviceParameterPair {
                device,
                first,
                second,
            },
        ) => device == key.device && first == id(base + 2) && second == id(base + 4),
        (3, GestureTarget::DeviceParameter { device, parameter }) => {
            device == key.device && parameter == id(base + 5)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_sections_match_minimum_and_maximum_reference_positions() {
        let min = graph_model([0.0, 1.0, 0.0, 0.0, 0.001, 0.001], 1);
        let max = graph_model([0.0, 1.0, 0.0, 20.0, 60.0, 60.0], 1);
        assert_eq!(
            min.nodes
                .iter()
                .map(|node| node.position[0])
                .collect::<Vec<_>>(),
            vec![0.0, 0.0, 0.0, 2.0 / 3.0]
        );
        assert_eq!(
            max.nodes
                .iter()
                .map(|node| node.position[0])
                .collect::<Vec<_>>(),
            vec![0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]
        );
        for model in [min, max] {
            model.validate().unwrap();
            assert_eq!(
                model.primary.last().copied(),
                Some(model.nodes[3].position),
                "curve stops at the release handle, including minimum times"
            );
        }
    }

    #[test]
    fn stage_edits_keep_scale_fixed_with_cumulative_attack_decay_and_anchored_release() {
        let before = [0.2, 1.0, 0.3, 0.1, 0.2, 0.4];
        let mut edited = before;
        edited[3] = 1.0;
        let a = graph_model(before, 1);
        let b = graph_model(edited, 1);
        assert_eq!(a.nodes[0].position, b.nodes[0].position);
        assert_eq!(a.nodes[3].position, b.nodes[3].position);
        let shift = b.nodes[1].position[0] - a.nodes[1].position[0];
        assert!(shift > 0.0);
        assert!((b.nodes[2].position[0] - a.nodes[2].position[0] - shift).abs() < 1e-6);
        for (node_id, min, max) in [(1, 0.0, 20.0), (2, 0.001, 60.0), (3, 0.001, 60.0)] {
            let gesture = GraphGesture {
                node_id,
                initial: before,
            };
            for (x, expected) in [(0.0, min), (1.0, max)] {
                let (_, values) = edit_values(0, gesture, [x, 0.5]).unwrap();
                let duration = match values {
                    EditValues::Pair(pair) => pair.second,
                    EditValues::Single(value) => value,
                };
                assert_eq!(duration, ParameterValue::Float(expected));
            }
        }
    }

    #[test]
    fn logarithmic_stage_mapping_is_invertible_and_preserves_smooth_dsp_shape() {
        for (min, max) in [(0.0, 20.0), (0.001, 60.0)] {
            for seconds in [min, 0.005, 0.1, 1.0, max] {
                let back = stage_seconds(stage_width(seconds, min, max), min, max);
                assert!((back - seconds).abs() <= seconds.max(0.001) * 2e-6);
            }
        }
        let model = graph_model([0.2, 1.0, 0.3, 0.1, 0.2, 0.4], 2);
        model.validate().unwrap();
        let decay_mid = model.primary[17];
        assert!((decay_mid[1] - (0.3 + 0.7 * 0.00001_f32.sqrt())).abs() < 1e-6);
        assert!(model.nodes[2].selected);
        assert_eq!(model.nodes[3].position[1], 0.0);
    }
}
