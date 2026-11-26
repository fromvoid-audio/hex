//! Authored carrier-filter response curve and draggable frequency/resonance handle.
use super::super::finalize_controls_except_graph;
use super::*;
use gpui::{Entity, Subscription};
use hex_lattice_contract::NativeDeviceState;
use hex_session::device_gesture::{GestureTarget, ParameterPairValue};
use hex_widgets::{
    ControlStyle, ResponseGraph, ResponseGraphAxes, ResponseGraphEvent, ResponseGraphModel,
    ResponseGraphNode, ResponseGraphPhase,
};

const POINTS: usize = 128;
const MIN_DB: f32 = -60.0;
const MAX_DB: f32 = 60.0;
const MIN_HZ: f32 = 20.0;
const MAX_HZ: f32 = 20_000.0;
const MIN_RES: f32 = 0.0;
const MAX_RES: f32 = 100.0;
const NODE_ID: u32 = 1;
const FILTER_ENABLED: ParameterId = ParameterId::from_static(20_001);
const FILTER_FREQUENCY: ParameterId = ParameterId::from_static(20_005);
const FILTER_RESONANCE: ParameterId = ParameterId::from_static(20_006);

#[derive(Clone, PartialEq)]
struct Input {
    state: NativeDeviceState,
    rate: u32,
    style: ControlStyle,
}
pub(in crate::editor::device_panel) struct Control {
    entity: Entity<ResponseGraph>,
    _subscription: Subscription,
    cache: Option<(Input, ResponseGraphModel)>,
}

impl Control {
    pub(in crate::editor::device_panel) fn entity_id(&self) -> gpui::EntityId {
        self.entity.entity_id()
    }

    pub(in crate::editor::device_panel) fn release_pointer_capture(
        &self,
        cx: &mut Context<EditorView>,
    ) {
        self.entity.update(cx, |graph, cx| graph.cancel(cx));
    }
}

pub(super) fn hide(view: &mut EditorView, device: DeviceId) {
    if let Some(state) = view.device_panel.operator_previews.get_mut(&device) {
        state.filter_response = None;
    }
}

fn input(
    view: &mut EditorView,
    device: DeviceId,
    parameters: &[NativeDeviceParameterView],
) -> Option<Input> {
    let owner = context::Owner::capture(view, device)?;
    let mut state = owner.state(view)?.clone();
    for parameter in parameters {
        let effective = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: parameter.descriptor.id,
            },
            parameter.value,
        );
        if effective != parameter.value {
            state = hex_engine::patch_native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                &state,
                parameter.descriptor.id,
                effective,
            )
            .ok()?;
        }
    }
    Some(Input {
        state,
        rate: view.state.as_ref()?.project.sample_rate(),
        style: super::display_style(),
    })
}

fn upper(rate: u32) -> f32 {
    (rate as f32 * 0.49).min(MAX_HZ)
}
fn x(frequency: f32, upper: f32) -> f32 {
    let clamped = frequency.clamp(MIN_HZ, upper.max(MIN_HZ));
    let span = (upper / MIN_HZ).ln();
    if !span.is_finite() || span <= 0.0 {
        return 0.0;
    }
    ((clamped / MIN_HZ).ln() / span).clamp(0.0, 1.0)
}
fn x_to_freq(pos_x: f32, upper: f32) -> f32 {
    let upper = upper.max(MIN_HZ);
    (MIN_HZ * (upper / MIN_HZ).powf(pos_x.clamp(0.0, 1.0))).clamp(MIN_HZ, MAX_HZ)
}
fn res_to_y(resonance: f32) -> f32 {
    ((resonance - MIN_RES) / (MAX_RES - MIN_RES)).clamp(0.0, 1.0)
}
fn y_to_res(pos_y: f32) -> f32 {
    (MIN_RES + pos_y.clamp(0.0, 1.0) * (MAX_RES - MIN_RES)).clamp(MIN_RES, MAX_RES)
}

fn handle_graph_event(
    view: &mut EditorView,
    device: DeviceId,
    graph_id: gpui::EntityId,
    event: ResponseGraphEvent,
    cx: &mut Context<EditorView>,
) {
    if event.node_id != NODE_ID
        || matches!(
            event.phase,
            ResponseGraphPhase::Select
                | ResponseGraphPhase::Remove
                | ResponseGraphPhase::Press
                | ResponseGraphPhase::Release
                | ResponseGraphPhase::ReleaseOutside
        )
    {
        return;
    }
    if event.phase == ResponseGraphPhase::Begin {
        if !finalize_controls_except_graph(view, cx, graph_id) {
            return;
        }
        if view.device_panel.gesture.snapshot().is_some() {
            return;
        }
        let Some(canonical) = context::Owner::capture(view, device)
            .and_then(|owner| owner.state(view))
            .cloned()
        else {
            return;
        };
        let canon_freq = hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            &canonical,
            FILTER_FREQUENCY,
        )
        .unwrap_or(ParameterValue::Float(1_000.0));
        let canon_res = hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            &canonical,
            FILTER_RESONANCE,
        )
        .unwrap_or(ParameterValue::Float(0.0));
        let freq = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: FILTER_FREQUENCY,
            },
            canon_freq,
        );
        let res = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: FILTER_RESONANCE,
            },
            canon_res,
        );
        let pair = ParameterPairValue {
            first: freq,
            second: res,
        };
        if let Err(error) =
            view.begin_device_pair_gesture(device, FILTER_FREQUENCY, FILTER_RESONANCE, pair)
        {
            view.device_panel.message = Some(error.to_owned());
            return;
        }
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: FILTER_FREQUENCY,
            },
            pair.first,
        );
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: FILTER_RESONANCE,
            },
            pair.second,
        );
        cx.notify();
        return;
    }
    let Some(snapshot) = view.device_panel.gesture.snapshot() else {
        return;
    };
    let GestureTarget::DeviceParameterPair {
        device: target_device,
        first,
        second,
    } = snapshot.target
    else {
        return;
    };
    if target_device != device
        || first != FILTER_FREQUENCY
        || second != FILTER_RESONANCE
        || view.device_panel.gesture.has_pending_terminal()
    {
        return;
    }
    if event.phase == ResponseGraphPhase::Cancel {
        view.device_panel.pending_values.remove(&ParameterKey {
            device,
            parameter: FILTER_FREQUENCY,
        });
        view.device_panel.pending_values.remove(&ParameterKey {
            device,
            parameter: FILTER_RESONANCE,
        });
        let _ = view.cancel_device_gesture();
        cx.notify();
        return;
    }
    if !event.position[0].is_finite() || !event.position[1].is_finite() {
        return;
    }
    let rate = view
        .state
        .as_ref()
        .map_or(48_000, |state| state.project.sample_rate());
    let pair = ParameterPairValue {
        first: ParameterValue::Float(x_to_freq(event.position[0], upper(rate))),
        second: ParameterValue::Float(y_to_res(event.position[1])),
    };
    view.device_panel.pending_values.insert(
        ParameterKey {
            device,
            parameter: FILTER_FREQUENCY,
        },
        pair.first,
    );
    view.device_panel.pending_values.insert(
        ParameterKey {
            device,
            parameter: FILTER_RESONANCE,
        },
        pair.second,
    );
    let result = match event.phase {
        ResponseGraphPhase::Change => view.change_device_pair_gesture(pair),
        ResponseGraphPhase::Commit => view.finish_device_pair_gesture(pair),
        _ => return,
    };
    if let Err(error) = result {
        view.device_panel.message = Some(error.to_owned());
    }
    cx.notify();
}

fn model(input: &Input) -> Option<(ResponseGraphModel, ResponseGraphAxes)> {
    let upper = upper(input.rate);
    let frequencies: [f32; POINTS] = std::array::from_fn(|index| {
        MIN_HZ * (upper / MIN_HZ).powf(index as f32 / (POINTS - 1) as f32)
    });
    let response = hex_engine::native_operator_filter_frequency_response(
        &input.state,
        input.rate,
        &frequencies,
    )
    .ok()?;
    let primary: Vec<_> = response
        .into_iter()
        .enumerate()
        .map(|(index, gain)| {
            let db = (20.0 * gain.max(1e-6).log10()).clamp(MIN_DB, MAX_DB);
            [
                index as f32 / (POINTS - 1) as f32,
                (db - MIN_DB) / (MAX_DB - MIN_DB),
            ]
        })
        .collect();
    let enabled = matches!(
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            &input.state,
            FILTER_ENABLED,
        ),
        Ok(ParameterValue::Bool(true))
    );
    let frequency = match hex_engine::native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &input.state,
        FILTER_FREQUENCY,
    ) {
        Ok(ParameterValue::Float(value)) if value.is_finite() => value,
        _ => 1_000.0,
    };
    let resonance = match hex_engine::native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &input.state,
        FILTER_RESONANCE,
    ) {
        Ok(ParameterValue::Float(value)) if value.is_finite() => value,
        _ => 0.0,
    };
    let model = ResponseGraphModel {
        primary: primary.clone(),
        secondary: primary,
        analyzer: None,
        input_analyzer: None,
        analyzer_peak: None,
        nodes: vec![ResponseGraphNode {
            accent: Some(input.style.accent),
            id: NODE_ID,
            position: [x(frequency, upper), res_to_y(resonance)],
            enabled,
            selected: true,
        }],
    };
    model.validate().ok()?;
    let axes = ResponseGraphAxes {
        horizontal: [
            (20.0, "20"),
            (100.0, "100"),
            (1000.0, "1k"),
            (10_000.0, "10k"),
            (20_000.0, "20k"),
        ]
        .into_iter()
        .filter(|(hz, _)| *hz <= upper)
        .map(|(hz, label)| (x(hz, upper), theme::text(label)))
        .collect(),
        vertical: vec![
            (0.0, theme::text("−60")),
            (0.5, theme::text("0")),
            (1.0, theme::text("+60")),
        ],
        right: Vec::new(),
    };
    Some((model, axes))
}

pub(super) fn graph(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    parameters: &[NativeDeviceParameterView],
) -> Div {
    let generation = view.project_generation;
    let result = input(view, device, parameters).and_then(|input| {
        let control = view
            .device_panel
            .operator_previews
            .entry(device)
            .or_default()
            .filter_response
            .get_or_insert_with(|| {
                let entity = cx.new(|graph_cx| {
                    let mut graph = ResponseGraph::new(
                        graph_cx,
                        format!("operator-filter-response-{}", device.value()),
                        input.style,
                    );
                    graph.set_editable(true, graph_cx);
                    graph.set_tab_navigation(true, graph_cx);
                    graph
                });
                let subscription = cx.subscribe(
                    &entity,
                    move |view, graph, event: &ResponseGraphEvent, cx| {
                        if view.project_generation == generation {
                            handle_graph_event(view, device, graph.entity_id(), *event, cx);
                        }
                    },
                );
                Control {
                    entity,
                    _subscription: subscription,
                    cache: None,
                }
            });
        if control
            .cache
            .as_ref()
            .is_none_or(|(cached, _)| cached != &input)
        {
            let (model, axes) = model(&input)?;
            let updated = control.entity.update(cx, |graph, cx| {
                graph.set_axes(Some(axes), cx).is_ok()
                    && graph.set_model(model.clone(), input.style, cx).is_ok()
            });
            if !updated {
                return None;
            }
            control.cache = Some((input, model));
        }
        Some(control.entity.clone())
    });
    let mut content = div()
        .debug_selector(|| "operator-filter-response".into())
        .flex()
        .flex_1()
        .h_full()
        .min_h_0()
        .w_full();
    if let Some(graph) = result {
        content = content.child(graph);
    } else {
        hide(view, device);
        content = content.child(theme::text("filter response unavailable"));
    }
    content
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor::device_panel) fn displayed(
    view: &EditorView,
    device: DeviceId,
) -> Option<(&[[f32; 2]], &NativeDeviceState, u32)> {
    let (input, model) = view
        .device_panel
        .operator_previews
        .get(&device)?
        .filter_response
        .as_ref()?
        .cache
        .as_ref()?;
    Some((&model.primary, &input.state, input.rate))
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor::device_panel) fn graph_for_test(
    view: &EditorView,
    device: DeviceId,
) -> Option<Entity<ResponseGraph>> {
    Some(
        view.device_panel
            .operator_previews
            .get(&device)?
            .filter_response
            .as_ref()?
            .entity
            .clone(),
    )
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn bypass_curve_and_axes_remain_valid_at_both_native_rate_limits() {
        let state = hex_engine::default_native_device_state_for_class(
            NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        for rate in [8000, 192_000] {
            let input = Input {
                state: state.clone(),
                rate,
                style: super::super::display_style(),
            };
            let (curve, axes) = model(&input).unwrap();
            assert_eq!(curve.primary.len(), POINTS);
            assert!(curve.primary.iter().all(|point| point[1] == 0.5));
            assert_eq!(curve.nodes.len(), 1);
            assert_eq!(curve.nodes[0].id, NODE_ID);
            assert!(!curve.nodes[0].enabled);
            let round_trip = x_to_freq(curve.nodes[0].position[0], upper(rate));
            assert!((round_trip - 1_000.0).abs() < 1e-2);
            assert_eq!(y_to_res(curve.nodes[0].position[1]), 0.0);
            assert!(curve.validate().is_ok());
            assert!(axes.horizontal.iter().all(|(x, _)| (0.0..=1.0).contains(x)));
            assert_eq!(
                axes.horizontal.last().unwrap().1.as_ref(),
                if rate == 8000 { "1k" } else { "20k" }
            );
        }
        assert!(
            model(&Input {
                state,
                rate: 0,
                style: super::super::display_style()
            })
            .is_none()
        );
    }
}
