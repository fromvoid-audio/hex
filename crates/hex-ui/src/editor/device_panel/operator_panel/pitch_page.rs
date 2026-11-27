//! Vector pitch page: bipolar pitch-envelope breakpoint graph, destinations,
//! transpose, spread, and read-only routes/glide summary.
//!
//! Reuses `hex_widgets::ResponseGraph` and the shared canonical device-gesture
//! protocol (`begin_device_gesture` / `begin_device_pair_gesture`). Glide
//! controls (`642`/`643`) remain single-mounted in `global::voices` and are
//! summarized read-only here so no `ParameterControl` entity is duplicated.
use super::super::*;
use super::{detail_field, id, section_title};
use gpui::{Entity, Subscription};
use hex_lattice_contract::ParameterId;
use hex_session::device_gesture::{GestureTarget, ParameterPairValue};
use hex_widgets::{
    ResponseGraph, ResponseGraphEvent, ResponseGraphModel, ResponseGraphNode, ResponseGraphPhase,
    ResponseGraphReference,
};

const STAGE_WIDTH: f32 = 1.0 / 3.0;
const MIN_DECAY_RELEASE_SECONDS: f32 = 0.001;
const ATTACK_LIMIT_SECONDS: f32 = 20.0;
const DECAY_LIMIT_SECONDS: f32 = 60.0;
const RELEASE_LIMIT_SECONDS: f32 = 60.0;
const MAX_SEMITONES: f32 = 48.0;

#[derive(Clone, Copy, PartialEq)]
struct ModelInput {
    /// `[initial, peak, sustain, end, attack, decay, release]`
    values: [f32; 7],
    selected: u32,
    style: hex_widgets::ControlStyle,
}

#[derive(Clone, Copy)]
struct DragGesture {
    node_id: u32,
    initial: [f32; 7],
}

pub(in crate::editor::device_panel) struct GraphControl {
    entity: Entity<ResponseGraph>,
    selected_node: u32,
    model_input: Option<ModelInput>,
    gesture: Option<DragGesture>,
    _subscription: Subscription,
}

impl GraphControl {
    pub(in crate::editor::device_panel) fn release_pointer_capture(
        &self,
        cx: &mut Context<EditorView>,
    ) {
        self.entity.update(cx, |graph, cx| graph.cancel(cx));
    }
}

pub(super) fn hide(view: &mut EditorView, device: DeviceId) {
    if let Some(state) = view.device_panel.operator_previews.get_mut(&device) {
        if let Some(control) = state.pitch_envelope.as_mut() {
            control.gesture = None;
        }
        state.pitch_envelope = None;
    }
}

fn level_to_y(semitones: f32) -> f32 {
    if !semitones.is_finite() {
        return 0.5;
    }
    ((semitones.clamp(-MAX_SEMITONES, MAX_SEMITONES) + MAX_SEMITONES) / (2.0 * MAX_SEMITONES))
        .clamp(0.0, 1.0)
}

fn y_to_level(y: f32) -> f32 {
    if !y.is_finite() {
        return 0.0;
    }
    (y.clamp(0.0, 1.0) * (2.0 * MAX_SEMITONES) - MAX_SEMITONES).clamp(-MAX_SEMITONES, MAX_SEMITONES)
}

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

fn graph_model(values: [f32; 7], selected: u32) -> ResponseGraphModel {
    let [initial, peak, sustain, end, attack, decay, release] = values;
    let x_attack = stage_width(attack, 0.0, ATTACK_LIMIT_SECONDS);
    let x_decay = x_attack + stage_width(decay, MIN_DECAY_RELEASE_SECONDS, DECAY_LIMIT_SECONDS);
    let x_release = STAGE_WIDTH * 2.0;
    let x_end = x_release + stage_width(release, MIN_DECAY_RELEASE_SECONDS, RELEASE_LIMIT_SECONDS);

    let y_init = level_to_y(initial);
    let y_peak = level_to_y(peak);
    let y_sust = level_to_y(sustain);
    let y_end = level_to_y(end);
    let points = vec![
        [0.0, y_init],
        [x_attack, y_peak],
        [x_decay, y_sust],
        [x_release, y_sust],
        [x_end, y_end],
    ];
    let mk_node = |id: u32, pos: [f32; 2]| ResponseGraphNode {
        accent: None,
        id,
        position: [pos[0].clamp(0.0, 1.0), pos[1].clamp(0.0, 1.0)],
        enabled: true,
        selected: id == selected,
    };
    ResponseGraphModel {
        primary: points.clone(),
        secondary: points,
        analyzer: None,
        input_analyzer: None,
        analyzer_peak: None,
        nodes: vec![
            mk_node(0, [0.0, y_init]),
            mk_node(1, [x_attack, y_peak]),
            mk_node(2, [x_decay, y_sust]),
            mk_node(3, [x_end, y_end]),
        ],
    }
}

fn read_envelope_values(
    view: &mut EditorView,
    device: DeviceId,
    parameters: &[NativeDeviceParameterView],
) -> [f32; 7] {
    [653, 654, 655, 656, 657, 658, 659].map(|param| {
        match super::value(view, device, parameters, id(param)) {
            Some(ParameterValue::Float(v)) if v.is_finite() => v,
            _ => 0.0,
        }
    })
}

fn handle_event(
    view: &mut EditorView,
    device: DeviceId,
    event: ResponseGraphEvent,
    cx: &mut Context<EditorView>,
) {
    if event.phase == ResponseGraphPhase::Select {
        let Some(graph_id) = view
            .device_panel
            .operator_previews
            .get(&device)
            .and_then(|s| s.pitch_envelope.as_ref())
            .map(|c| c.entity.entity_id())
        else {
            return;
        };
        if event.node_id > 3 || !finalize_controls_except_graph(view, cx, graph_id) {
            return;
        }
        if let Some(control) = view
            .device_panel
            .operator_previews
            .get_mut(&device)
            .and_then(|s| s.pitch_envelope.as_mut())
        {
            control.selected_node = event.node_id;
        }
        cx.notify();
        return;
    }
    if matches!(
        event.phase,
        ResponseGraphPhase::Press
            | ResponseGraphPhase::Release
            | ResponseGraphPhase::ReleaseOutside
    ) {
        return;
    }
    if event.phase == ResponseGraphPhase::Begin {
        begin_gesture(view, device, event, cx);
        return;
    }
    let Some(gesture) = view
        .device_panel
        .operator_previews
        .get(&device)
        .and_then(|s| s.pitch_envelope.as_ref())
        .and_then(|c| c.gesture)
    else {
        return;
    };
    if gesture.node_id != event.node_id {
        return;
    }
    if event.phase == ResponseGraphPhase::Cancel {
        if let Some(control) = view
            .device_panel
            .operator_previews
            .get_mut(&device)
            .and_then(|s| s.pitch_envelope.as_mut())
        {
            control.gesture = None;
        }
        let _ = view.cancel_device_gesture();
        cx.notify();
        return;
    }
    let Some(pair_or_single) = edit_values(gesture, event.position) else {
        return;
    };
    let result = match pair_or_single {
        EditValue::Single(param, val) => {
            view.device_panel.pending_values.insert(
                ParameterKey {
                    device,
                    parameter: param,
                },
                val,
            );
            if event.phase == ResponseGraphPhase::Commit {
                view.finish_device_gesture(val)
            } else {
                view.change_device_gesture(val)
            }
        }
        EditValue::Pair(first, second, val) => {
            view.device_panel.pending_values.insert(
                ParameterKey {
                    device,
                    parameter: first,
                },
                val.first,
            );
            view.device_panel.pending_values.insert(
                ParameterKey {
                    device,
                    parameter: second,
                },
                val.second,
            );
            if event.phase == ResponseGraphPhase::Commit {
                view.finish_device_pair_gesture(val)
            } else {
                view.change_device_pair_gesture(val)
            }
        }
    };
    if let Err(error) = result {
        view.device_panel.message = Some(error.to_owned());
    }
    if event.phase == ResponseGraphPhase::Commit
        && let Some(control) = view
            .device_panel
            .operator_previews
            .get_mut(&device)
            .and_then(|s| s.pitch_envelope.as_mut())
    {
        control.gesture = None;
    }
    cx.notify();
}

enum EditValue {
    Single(ParameterId, ParameterValue),
    Pair(ParameterId, ParameterId, ParameterPairValue),
}

fn edit_values(gesture: DragGesture, position: [f32; 2]) -> Option<EditValue> {
    let x = position[0].clamp(0.0, 1.0);
    let level = y_to_level(position[1]);
    match gesture.node_id {
        0 => Some(EditValue::Single(id(653), ParameterValue::Float(level))),
        1 => {
            let attack = stage_seconds(x, 0.0, ATTACK_LIMIT_SECONDS);
            Some(EditValue::Pair(
                id(657),
                id(654),
                ParameterPairValue {
                    first: ParameterValue::Float(attack),
                    second: ParameterValue::Float(level),
                },
            ))
        }
        2 => {
            let x_attack = stage_width(gesture.initial[4], 0.0, ATTACK_LIMIT_SECONDS);
            let decay = stage_seconds(
                (x - x_attack).max(0.0),
                MIN_DECAY_RELEASE_SECONDS,
                DECAY_LIMIT_SECONDS,
            );
            Some(EditValue::Pair(
                id(658),
                id(655),
                ParameterPairValue {
                    first: ParameterValue::Float(decay),
                    second: ParameterValue::Float(level),
                },
            ))
        }
        3 => {
            let release = stage_seconds(
                (x - STAGE_WIDTH * 2.0).max(0.0),
                MIN_DECAY_RELEASE_SECONDS,
                RELEASE_LIMIT_SECONDS,
            );
            Some(EditValue::Pair(
                id(659),
                id(656),
                ParameterPairValue {
                    first: ParameterValue::Float(release),
                    second: ParameterValue::Float(level),
                },
            ))
        }
        _ => None,
    }
}

fn begin_gesture(
    view: &mut EditorView,
    device: DeviceId,
    event: ResponseGraphEvent,
    cx: &mut Context<EditorView>,
) {
    let Some(graph_id) = view
        .device_panel
        .operator_previews
        .get(&device)
        .and_then(|s| s.pitch_envelope.as_ref())
        .map(|c| c.entity.entity_id())
    else {
        return;
    };
    if !super::context::Owner::capture(view, device).is_some_and(|o| o.is_current(view))
        || !finalize_controls_except_graph(view, cx, graph_id)
        || view.device_panel.gesture.snapshot().is_some()
    {
        return;
    }
    let Some(parameters) = view
        .device_panel
        .views
        .get(&device)
        .and_then(|(_, _, res)| res.as_ref().ok())
        .map(|v| v.parameters.clone())
    else {
        return;
    };
    let initial = read_envelope_values(view, device, &parameters);
    let started = match event.node_id {
        0 => view.begin_device_gesture(
            GestureTarget::DeviceParameter {
                device,
                parameter: id(653),
            },
            ParameterValue::Float(initial[0]),
        ),
        1 => view.begin_device_pair_gesture(
            device,
            id(657),
            id(654),
            ParameterPairValue {
                first: ParameterValue::Float(initial[4]),
                second: ParameterValue::Float(initial[1]),
            },
        ),
        2 => view.begin_device_pair_gesture(
            device,
            id(658),
            id(655),
            ParameterPairValue {
                first: ParameterValue::Float(initial[5]),
                second: ParameterValue::Float(initial[2]),
            },
        ),
        3 => view.begin_device_pair_gesture(
            device,
            id(659),
            id(656),
            ParameterPairValue {
                first: ParameterValue::Float(initial[6]),
                second: ParameterValue::Float(initial[3]),
            },
        ),
        _ => return,
    };
    match started {
        Ok(_) => {
            if let Some(control) = view
                .device_panel
                .operator_previews
                .get_mut(&device)
                .and_then(|s| s.pitch_envelope.as_mut())
            {
                control.selected_node = event.node_id;
                control.gesture = Some(DragGesture {
                    node_id: event.node_id,
                    initial,
                });
            }
        }
        Err(error) => {
            view.device_panel.message = Some(error.to_owned());
        }
    }
    cx.notify();
}

fn routes_and_glide_summary(
    view: &mut EditorView,
    device: DeviceId,
    parameters: &[NativeDeviceParameterView],
) -> String {
    let mut destinations = Vec::new();
    for (param, label) in [
        (666_u32, "a"),
        (667, "b"),
        (668, "c"),
        (669, "d"),
        (670, "lfo"),
    ] {
        if super::value(view, device, parameters, id(param)) == Some(ParameterValue::Bool(true)) {
            destinations.push(label);
        }
    }
    let glide_on =
        super::value(view, device, parameters, id(642)) == Some(ParameterValue::Bool(true));
    let glide_s = match super::value(view, device, parameters, id(643)) {
        Some(ParameterValue::Float(v)) if v.is_finite() => v,
        _ => 0.05,
    };
    let dest_text = if destinations.is_empty() {
        "none".to_owned()
    } else {
        destinations.join(" ")
    };
    let glide_text = if glide_on {
        if glide_s < 1.0 {
            format!("glide {:.0}ms", glide_s * 1000.0)
        } else {
            format!("glide {glide_s:.2}s")
        }
    } else {
        "glide off".to_owned()
    };
    format!("routes: {dest_text} · {glide_text}")
}

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
) -> Stateful<Div> {
    use super::super::numeric_display::NumericDisplayCodec as Codec;

    let values = read_envelope_values(view, device, parameters);
    let style = super::display_style();
    let preview_state = view
        .device_panel
        .operator_previews
        .entry(device)
        .or_default();
    let control = preview_state.pitch_envelope.get_or_insert_with(|| {
        let entity = cx.new(|graph_cx| {
            let mut graph = ResponseGraph::new(
                graph_cx,
                format!("operator-pitch-envelope-{}", device.value()),
                style,
            );
            graph.set_editable(true, graph_cx);
            graph.set_tab_navigation(true, graph_cx);
            graph
        });
        let subscription = cx.subscribe(&entity, move |view, _, event: &ResponseGraphEvent, cx| {
            handle_event(view, device, *event, cx);
        });
        GraphControl {
            entity,
            selected_node: 1,
            model_input: None,
            gesture: None,
            _subscription: subscription,
        }
    });
    let input = ModelInput {
        values,
        selected: control.selected_node,
        style,
    };
    let graph_entity = control.entity.clone();
    if control.model_input != Some(input) {
        let model = graph_model(input.values, input.selected);
        let refs = vec![
            // Zero semitone reference line at y = 0.5.
            ResponseGraphReference {
                points: vec![[0.0, 0.5], [1.0, 0.5]].into(),
                color: style.muted,
                opacity: 0.45,
            },
            // Note-off marker at x = 2/3.
            ResponseGraphReference {
                points: vec![[STAGE_WIDTH * 2.0, 0.0], [STAGE_WIDTH * 2.0, 1.0]].into(),
                color: style.muted,
                opacity: 0.35,
            },
        ];
        if graph_entity.update(cx, |graph, cx| {
            graph.set_model(model, input.style, cx).is_ok()
                && graph.set_references(refs, cx).is_ok()
        }) {
            control.model_input = Some(input);
        }
    }

    let graph_box = div()
        .id(ElementId::Name(
            format!("operator-pitch-envelope-graph-{}", device.value()).into(),
        ))
        .debug_selector(|| "operator-pitch-envelope-graph".into())
        .relative()
        .w_full()
        .h(px(84.0))
        .flex_shrink_0()
        .child(graph_entity);

    // Row 1 below tab rail: on, amount, initial, peak, sustain, end.
    let mut row_levels = div()
        .w_full()
        .h(px(22.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0));
    for (parameter, label, width, presentation) in [
        (650_u32, "pitch", 36.0, ControlPresentation::CompactButton),
        (651, "amt", 48.0, ControlPresentation::InlineValue),
        (653, "init", 48.0, ControlPresentation::InlineValue),
        (654, "peak", 48.0, ControlPresentation::InlineValue),
        (655, "sust", 48.0, ControlPresentation::InlineValue),
        (656, "end", 48.0, ControlPresentation::InlineValue),
    ] {
        let control = super::compact_control(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            presentation,
            if presentation == ControlPresentation::CompactButton {
                Some("on")
            } else {
                None
            },
        );
        row_levels = row_levels.child(detail_field(device, id(parameter), label, width, control));
    }

    // Row 2: attack, decay, release, transpose (640), spread (641).
    let mut row_times = div()
        .w_full()
        .h(px(22.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0));
    for (parameter, label, width, codec) in [
        (657_u32, "attack", 50.0, Some(Codec::Seconds)),
        (658, "decay", 50.0, Some(Codec::Seconds)),
        (659, "release", 50.0, Some(Codec::Seconds)),
        (640, "transp", 52.0, None),
        (641, "spread", 52.0, None),
    ] {
        let control = super::compact_control_with_codec(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            ControlPresentation::InlineValue,
            None,
            codec,
        );
        row_times = row_times.child(detail_field(device, id(parameter), label, width, control));
    }

    // Row 3: destination chips a, b, c, d, lfo + read-only routes/glide summary.
    let mut row_dest = div()
        .w_full()
        .h(px(20.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(3.0));
    for (parameter, label) in [
        (666_u32, "a"),
        (667, "b"),
        (668, "c"),
        (669, "d"),
        (670, "lfo"),
    ] {
        row_dest = row_dest.child(super::compact_control(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            ControlPresentation::CompactButton,
            Some(label),
        ));
    }
    let summary = routes_and_glide_summary(view, device, parameters);
    row_dest = row_dest.child(
        section_title(summary)
            .id(ElementId::Name(
                format!("operator-pitch-routes-{}", device.value()).into(),
            ))
            .debug_selector(move || format!("operator-pitch-routes-{}", device.value()))
            .flex_1()
            .min_w_0()
            .overflow_hidden(),
    );

    let lower = div()
        .w_full()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .pt(px(2.0))
        .child(row_levels)
        .child(row_times)
        .child(row_dest);

    div()
        .id(ElementId::Name(
            format!("operator-pitch-detail-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-pitch-detail-{}", device.value()))
        .w_full()
        .h_full()
        .flex()
        .flex_col()
        .child(graph_box)
        // Reserve the shared tab strip slot (y = 86..102).
        .child(div().h(px(18.0)).flex_shrink_0())
        .child(lower)
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn bipolar_pitch_envelope_coordinates_round_trip_and_place_zero_at_midpoint() {
        assert!((level_to_y(0.0) - 0.5).abs() < 1e-6);
        assert!((level_to_y(48.0) - 1.0).abs() < 1e-6);
        assert!((level_to_y(-48.0) - 0.0).abs() < 1e-6);
        for st in [-48.0_f32, -24.0, -7.0, 0.0, 12.0, 48.0] {
            let round = y_to_level(level_to_y(st));
            assert!((round - st).abs() < 1e-4, "round trip failed for {st}");
        }
        assert_eq!(level_to_y(f32::NAN), 0.5);
        assert_eq!(y_to_level(f32::INFINITY), 0.0);

        let model = graph_model([-12.0, 24.0, 0.0, -24.0, 0.5, 1.0, 2.0], 1);
        assert_eq!(model.nodes.len(), 4);
        assert!(model.nodes[1].selected);
        assert!(model.nodes[1].position[1] > 0.5);
        assert!(model.nodes[0].position[1] < 0.5);
        assert!((model.nodes[2].position[1] - 0.5).abs() < 1e-6);
        assert!(model.nodes[3].position[1] < 0.5);
        assert!(
            model
                .primary
                .iter()
                .all(|&[x, y]| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y))
        );
    }
}
