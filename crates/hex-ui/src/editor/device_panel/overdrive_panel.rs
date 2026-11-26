//! Compact Overdrive panel with interactive X/Y bandpass display and shared Lattice dials.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effect_preview, effective_parameter_value, finalize_controls_except_graph,
    numeric_display::NumericDisplayCodec, parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Entity, Stateful, Subscription, div, prelude::*, px};
use hex_engine::{NativeDeviceClass, NativeDeviceView, OverdriveFilterResponse};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use hex_project::DeviceId;
use hex_session::device_gesture::{GestureTarget, ParameterPairValue};
use hex_widgets::{
    ResponseGraph, ResponseGraphEvent, ResponseGraphModel, ResponseGraphNode, ResponseGraphPhase,
};

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::OVERDRIVE_FRAME_WIDTH;
const FREQUENCY: ParameterId = ParameterId::from_static(1);
const BANDWIDTH: ParameterId = ParameterId::from_static(2);
const DRIVE: ParameterId = ParameterId::from_static(3);
const TONE: ParameterId = ParameterId::from_static(4);
const DYNAMICS: ParameterId = ParameterId::from_static(5);
const WET: ParameterId = ParameterId::from_static(6);
const PARAMETERS: [ParameterId; 2] = [FREQUENCY, BANDWIDTH];

const MIN_PLOT_HZ: f32 = 20.0;
const MAX_PLOT_HZ: f32 = 20_000.0;
const MIN_FREQ_HZ: f32 = 50.0;
const MAX_FREQ_HZ: f32 = 20_000.0;
const MIN_BW_OCT: f32 = 0.5;
const MAX_BW_OCT: f32 = 9.0;
const DIAL_GAP: f32 = 6.0;
const DIAL_RAIL_WIDTH: f32 = hex_widgets::MICRO_DIAL_WIDTH * 2.0 + DIAL_GAP + 12.0;

pub(super) type PreviewCache = effect_preview::PreviewCache<OverdriveFilterResponse, 2, u32>;

pub(super) struct GraphControl {
    pub(super) entity: Entity<ResponseGraph>,
    _subscription: Subscription,
}

impl GraphControl {
    pub(super) fn entity_id(&self) -> gpui::EntityId {
        self.entity.entity_id()
    }

    pub(super) fn release_pointer_capture(&self, cx: &mut Context<EditorView>) {
        self.entity.update(cx, |graph, cx| graph.cancel(cx));
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn cached_curve(
    view: &EditorView,
    device: DeviceId,
) -> Option<&OverdriveFilterResponse> {
    view.device_panel
        .overdrive_previews
        .get(&device)?
        .ok_curve()
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn graph_for_test(view: &EditorView, device: DeviceId) -> Entity<ResponseGraph> {
    view.device_panel
        .overdrive_graphs
        .get(&device)
        .expect("overdrive graph")
        .entity
        .clone()
}

fn freq_to_x(hz: f32) -> f32 {
    let hz = hz.clamp(MIN_FREQ_HZ, MAX_FREQ_HZ);
    ((hz / MIN_PLOT_HZ).ln() / (MAX_PLOT_HZ / MIN_PLOT_HZ).ln()).clamp(0.0, 1.0)
}

fn x_to_freq(x: f32) -> f32 {
    (MIN_PLOT_HZ * (MAX_PLOT_HZ / MIN_PLOT_HZ).powf(x.clamp(0.0, 1.0)))
        .clamp(MIN_FREQ_HZ, MAX_FREQ_HZ)
}

fn bw_to_y(bw: f32) -> f32 {
    ((bw - MIN_BW_OCT) / (MAX_BW_OCT - MIN_BW_OCT)).clamp(0.0, 1.0)
}

fn y_to_bw(y: f32) -> f32 {
    (MIN_BW_OCT + y.clamp(0.0, 1.0) * (MAX_BW_OCT - MIN_BW_OCT)).clamp(MIN_BW_OCT, MAX_BW_OCT)
}

fn effective_float(
    view: &mut EditorView,
    device: DeviceId,
    device_view: &NativeDeviceView,
    id: ParameterId,
    fallback: f32,
) -> f32 {
    let canonical = device_view
        .parameters
        .iter()
        .find(|p| p.descriptor.id == id)
        .map(|p| p.value)
        .unwrap_or(ParameterValue::Float(fallback));
    match effective_parameter_value(
        view,
        ParameterKey {
            device,
            parameter: id,
        },
        canonical,
    ) {
        ParameterValue::Float(v) if v.is_finite() => v,
        _ => fallback,
    }
}

fn handle_graph_event(
    view: &mut EditorView,
    device: DeviceId,
    graph_id: gpui::EntityId,
    event: ResponseGraphEvent,
    cx: &mut Context<EditorView>,
) {
    if matches!(
        event.phase,
        ResponseGraphPhase::Select
            | ResponseGraphPhase::Remove
            | ResponseGraphPhase::Press
            | ResponseGraphPhase::Release
            | ResponseGraphPhase::ReleaseOutside
    ) {
        return;
    }
    if event.phase == ResponseGraphPhase::Begin {
        if !finalize_controls_except_graph(view, cx, graph_id) {
            return;
        }
        if view.device_panel.gesture.snapshot().is_some() {
            return;
        }
        let Some(canonical) = view
            .state
            .as_ref()
            .and_then(|state| state.project.device(device))
            .map(|dev| dev.state().clone())
        else {
            return;
        };
        let canon_freq = hex_engine::native_device_parameter(&canonical, FREQUENCY)
            .unwrap_or(ParameterValue::Float(1_250.0));
        let canon_bw = hex_engine::native_device_parameter(&canonical, BANDWIDTH)
            .unwrap_or(ParameterValue::Float(6.5));
        let freq = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: FREQUENCY,
            },
            canon_freq,
        );
        let bw = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: BANDWIDTH,
            },
            canon_bw,
        );
        let pair = ParameterPairValue {
            first: freq,
            second: bw,
        };
        if let Err(error) = view.begin_device_pair_gesture(device, FREQUENCY, BANDWIDTH, pair) {
            view.device_panel.message = Some(error.to_owned());
            return;
        }
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: FREQUENCY,
            },
            pair.first,
        );
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: BANDWIDTH,
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
        || first != FREQUENCY
        || second != BANDWIDTH
        || view.device_panel.gesture.has_pending_terminal()
    {
        return;
    }
    if event.phase == ResponseGraphPhase::Cancel {
        view.device_panel.pending_values.remove(&ParameterKey {
            device,
            parameter: FREQUENCY,
        });
        view.device_panel.pending_values.remove(&ParameterKey {
            device,
            parameter: BANDWIDTH,
        });
        let _ = view.cancel_device_gesture();
        cx.notify();
        return;
    }
    let pair = ParameterPairValue {
        first: ParameterValue::Float(x_to_freq(event.position[0])),
        second: ParameterValue::Float(y_to_bw(event.position[1])),
    };
    view.device_panel.pending_values.insert(
        ParameterKey {
            device,
            parameter: FREQUENCY,
        },
        pair.first,
    );
    view.device_panel.pending_values.insert(
        ParameterKey {
            device,
            parameter: BANDWIDTH,
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

fn preview(
    view: &mut EditorView,
    device: DeviceId,
    state: &NativeDeviceState,
    device_view: &NativeDeviceView,
) -> Result<OverdriveFilterResponse, String> {
    let sample_rate = view
        .state
        .as_ref()
        .map_or(48_000, |s| s.project.sample_rate());
    effect_preview::resolve(
        view,
        effect_preview::PreviewQuery {
            device,
            state,
            device_view,
            parameters: PARAMETERS,
            missing_error: "missing overdrive parameter",
            extra: sample_rate,
        },
        |view| &mut view.device_panel.overdrive_previews,
        |draft| {
            hex_engine::native_overdrive_filter_response(draft, sample_rate)
                .map_err(|error| error.to_string())
        },
    )
}

fn graph_entity(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    response: Result<OverdriveFilterResponse, String>,
    device_view: &NativeDeviceView,
) -> Div {
    let style = theme::device_style(theme::DeviceStyleRole::Display);
    let mut container = div()
        .debug_selector(move || format!("overdrive-filter-{}", device.value()))
        .w_full()
        .h_full()
        .flex()
        .flex_1()
        .min_h_0();
    let Ok(response) = response else {
        return container.child(theme::text("preview unavailable"));
    };
    let freq = effective_float(view, device, device_view, FREQUENCY, 1_250.0);
    let bw = effective_float(view, device, device_view, BANDWIDTH, 6.5);
    let points: Vec<[f32; 2]> = response.points.to_vec();
    let model = ResponseGraphModel {
        primary: points.clone(),
        secondary: points,
        analyzer: None,
        input_analyzer: None,
        analyzer_peak: None,
        nodes: vec![ResponseGraphNode {
            accent: Some(style.accent),
            id: 1,
            position: [freq_to_x(freq), bw_to_y(bw)],
            enabled: true,
            selected: true,
        }],
    };
    let generation = view.project_generation;
    let entity = view
        .device_panel
        .overdrive_graphs
        .entry(device)
        .or_insert_with(|| {
            let entity = cx.new(|graph_cx| {
                let mut graph = ResponseGraph::new(
                    graph_cx,
                    format!("overdrive-xy-graph-{}", device.value()),
                    style,
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
            GraphControl {
                entity,
                _subscription: subscription,
            }
        })
        .entity
        .clone();
    entity.update(cx, |graph, cx| {
        let _ = graph.set_model(model, style, cx);
    });
    container = container.child(entity);
    container
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    state: &NativeDeviceState,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let response = preview(view, device, state, device_view);
    let graph = graph_entity(view, cx, device, response, device_view);
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let mut control = |id, presentation, label: Option<&'static str>| {
        device_view
            .parameters
            .iter()
            .find(|p| p.descriptor.id == id)
            .map(|parameter| {
                parameter_control_with_presentation(
                    view,
                    cx,
                    device,
                    identity,
                    parameter,
                    presentation,
                    ControlText {
                        label,
                        codec: if id == FREQUENCY {
                            Some(NumericDisplayCodec::Hertz)
                        } else if [DRIVE, TONE, DYNAMICS, WET].contains(&id) {
                            Some(NumericDisplayCodec::Percent)
                        } else {
                            Some(NumericDisplayCodec::Number)
                        },
                    },
                )
            })
            .unwrap_or_else(|| div().child(theme::text("unsupported")))
    };
    let toolbar = crate::device_composition::control_row()
        .h(px(22.0))
        .flex_shrink_0()
        .justify_between()
        .child(field(
            "freq",
            control(FREQUENCY, ControlPresentation::InlineValue, None),
        ))
        .child(field(
            "bw",
            control(BANDWIDTH, ControlPresentation::InlineValue, None),
        ));
    let left = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(crate::device_composition::plot(graph))
        .child(toolbar);
    let top_dials = div()
        .flex()
        .justify_around()
        .gap(px(DIAL_GAP))
        .child(control(DRIVE, ControlPresentation::MicroDial, None))
        .child(control(TONE, ControlPresentation::MicroDial, None));
    let bottom_dials = div()
        .flex()
        .justify_around()
        .gap(px(DIAL_GAP))
        .child(control(
            DYNAMICS,
            ControlPresentation::MicroDial,
            Some("dyn"),
        ))
        .child(control(WET, ControlPresentation::MicroDial, Some("mix")));
    let right = crate::device_composition::dense_rail(
        crate::device_composition::rail_column()
            .justify_center()
            .gap(px(6.0))
            .child(top_dials)
            .child(bottom_dials),
    )
    .w(px(DIAL_RAIL_WIDTH))
    .flex_shrink_0();
    crate::device_composition::split_body(left, right)
        .py(px(3.0))
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

fn field(label: &'static str, control: Div) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .text_size(px(8.0))
        .child(theme::text(label))
        .child(control)
}
