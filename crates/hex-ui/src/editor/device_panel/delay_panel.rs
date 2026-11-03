//! Native Live-style delay composition. Only graph entity and disclosure are
//! view-local; every sound control uses the shared binding/Session contracts.
use super::*;
use crate::device_composition::plot;
use hex_engine::delay::*;
use hex_session::device_gesture::{GestureKey, ParameterPairValue};
use hex_widgets::{
    ResponseGraph, ResponseGraphEvent, ResponseGraphModel, ResponseGraphNode, ResponseGraphPhase,
};
pub(super) const FRAME_WIDTH: f32 = crate::device_composition::DELAY_FRAME_WIDTH;
pub(super) struct Panel {
    graph: Entity<ResponseGraph>,
    _subscription: Subscription,
    generation: u64,
    expanded: bool,
    gesture: Option<GestureKey>,
    preview: Option<(NativeDeviceState, u32, DelayFilterPreview)>,
}
fn row() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(2.0))
        .min_w_0()
        .flex_shrink_0()
}
fn value(
    view: &mut EditorView,
    metadata: &NativeDeviceView,
    device: DeviceId,
    id: hex_lattice_contract::ParameterId,
) -> Option<ParameterValue> {
    metadata
        .parameters
        .iter()
        .find(|p| p.descriptor.id == id)
        .map(|p| {
            effective_parameter_value(
                view,
                ParameterKey {
                    device,
                    parameter: id,
                },
                p.value,
            )
        })
}
fn control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    target: (DeviceId, &NativeDeviceView),
    id: hex_lattice_contract::ParameterId,
    presentation: ControlPresentation,
    label: Option<&'static str>,
    width: f32,
) -> Div {
    let (device, metadata) = target;
    let Some(parameter) = metadata.parameters.iter().find(|p| p.descriptor.id == id) else {
        return div().child(theme::text("unavailable"));
    };
    let codec = if [FEEDBACK, DRY_WET, LFO_DELAY, LFO_FILTER, LFO_MORPH].contains(&id) {
        Some(numeric_display::NumericDisplayCodec::Percent)
    } else {
        None
    };
    let rendered = parameter_control_with_presentation(
        view,
        cx,
        device,
        DeviceControlIdentity {
            class: NativeDeviceClass::Effect,
            type_id: TYPE_ID,
        },
        parameter,
        presentation,
        ControlText { label, codec },
    );
    if presentation == ControlPresentation::InlineValue {
        if let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device,
                parameter: id,
            })
        {
            entity.update(cx, |control, cx| {
                let _ = control.set_inline_value_width(width, cx);
            });
        }
    }
    rendered.w(px(width)).flex_shrink_0()
}
fn channel(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    metadata: &NativeDeviceView,
    right: bool,
) -> Div {
    let (sync, time, division, offset, label) = if right {
        (
            RIGHT_SYNC,
            RIGHT_TIME,
            RIGHT_DIVISION,
            RIGHT_OFFSET,
            "right",
        )
    } else {
        (LEFT_SYNC, TIME, LEFT_DIVISION, LEFT_OFFSET, "left")
    };
    let synced = value(view, metadata, device, sync) == Some(ParameterValue::Bool(true));
    let mode = control(
        view,
        cx,
        (device, metadata),
        sync,
        ControlPresentation::CompactButton,
        Some(if synced { "sync ≤16s" } else { "time" }),
        64.0,
    );
    let timing = if synced {
        control(
            view,
            cx,
            (device, metadata),
            division,
            ControlPresentation::CompactSegments { columns: 2 },
            None,
            64.0,
        )
    } else {
        div().h(px(62.0)).flex().items_center().child(control(
            view,
            cx,
            (device, metadata),
            time,
            ControlPresentation::InlineValue,
            None,
            64.0,
        ))
    };
    let offset = control(
        view,
        cx,
        (device, metadata),
        offset,
        ControlPresentation::InlineValue,
        None,
        64.0,
    );
    div()
        .debug_selector(move || format!("delay-{label}-{}", device.value()))
        .w(px(64.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(div().h(px(10.0)).child(theme::text(label)))
        .child(mode)
        .child(timing)
        .child(offset)
}
pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    canonical: &NativeDeviceState,
    metadata: &NativeDeviceView,
) -> Stateful<Div> {
    let generation = view.project_generation;
    if view
        .device_panel
        .delay_panels
        .get(&device)
        .is_some_and(|p| p.generation != generation)
    {
        view.device_panel.delay_panels.remove(&device);
    }
    view.device_panel
        .delay_panels
        .entry(device)
        .or_insert_with(|| {
            let graph = cx.new(|cx| {
                let mut graph = ResponseGraph::new(
                    cx,
                    format!("delay-filter-{}", device.value()),
                    theme::device_style(theme::DeviceStyleRole::Display),
                );
                graph.set_editable(true, cx);
                graph.set_tab_navigation(true, cx);
                graph
            });
            let subscription =
                cx.subscribe(&graph, move |view, _, event: &ResponseGraphEvent, cx| {
                    graph_event(view, cx, device, generation, *event);
                });
            Panel {
                graph,
                _subscription: subscription,
                generation,
                expanded: false,
                gesture: None,
                preview: None,
            }
        });
    let rate = view
        .state
        .as_ref()
        .map_or(48000, |s| s.project.sample_rate());
    let mut draft = canonical.clone();
    for id in [FREQUENCY, WIDTH, FILTER] {
        if let Some(value) = value(view, metadata, device, id) {
            if let Ok(next) = hex_engine::patch_native_device_parameter(&draft, id, value) {
                draft = next;
            }
        }
    }
    let client_key = view.device_gesture.snapshot().map(|s| s.key);
    let Some(panel) = view.device_panel.delay_panels.get_mut(&device) else {
        return div().id("delay-unavailable");
    };
    if panel.gesture.is_some() && panel.gesture != client_key {
        panel.gesture = None;
        panel.graph.update(cx, |graph, cx| graph.cancel(cx));
    }
    if panel
        .preview
        .as_ref()
        .is_none_or(|(state, r, _)| state != &draft || *r != rate)
    {
        if let Ok(preview) = filter_preview(&draft, rate) {
            let points = if preview.enabled {
                preview.points.to_vec()
            } else {
                preview.points.iter().map(|p| [p[0], 0.8]).collect()
            };
            let model = ResponseGraphModel {
                primary: points.clone(),
                secondary: points,
                analyzer: None,
                input_analyzer: None,
                analyzer_peak: None,
                nodes: vec![ResponseGraphNode {
                    id: 1,
                    position: preview.position,
                    enabled: true,
                    selected: true,
                }],
            };
            let result = panel.graph.update(cx, |graph, cx| {
                graph.set_model(
                    model,
                    theme::device_style(theme::DeviceStyleRole::Display),
                    cx,
                )
            });
            if let Err(error) = result {
                view.device_panel.message = Some(error.into());
            }
            panel.preview = Some((draft, rate, preview));
        }
    }
    let graph = panel.graph.clone();
    let expanded = panel.expanded;
    let left = channel(view, cx, device, metadata, false);
    let right = channel(view, cx, device, metadata, true);
    let link = control(
        view,
        cx,
        (device, metadata),
        LINK,
        ControlPresentation::CompactButton,
        Some("↔"),
        14.0,
    );
    let feedback = control(
        view,
        cx,
        (device, metadata),
        FEEDBACK,
        ControlPresentation::CompactDial,
        None,
        64.0,
    );
    let freeze = control(
        view,
        cx,
        (device, metadata),
        FREEZE,
        ControlPresentation::CompactButton,
        Some("✱"),
        14.0,
    );
    let timings = div()
        .w(px(146.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            row()
                .items_end()
                .child(left)
                .child(div().h(px(62.0)).flex().items_center().child(link))
                .child(right),
        )
        .child(row().justify_center().child(feedback).child(freeze));
    let smoothing = control(
        view,
        cx,
        (device, metadata),
        SMOOTHING,
        ControlPresentation::CompactSegments { columns: 1 },
        None,
        80.0,
    );
    let ping = control(
        view,
        cx,
        (device, metadata),
        PING_PONG,
        ControlPresentation::CompactButton,
        None,
        80.0,
    );
    let mix = control(
        view,
        cx,
        (device, metadata),
        DRY_WET,
        ControlPresentation::CompactDial,
        None,
        64.0,
    );
    let output = div()
        .w(px(80.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .items_center()
        .child(div().h(px(10.0)).child(theme::text("smoothing")))
        .child(smoothing)
        .child(div().flex_1())
        .child(ping)
        .child(mix);
    let filter = control(
        view,
        cx,
        (device, metadata),
        FILTER,
        ControlPresentation::CompactButton,
        None,
        38.0,
    );
    let frequency = control(
        view,
        cx,
        (device, metadata),
        FREQUENCY,
        ControlPresentation::InlineValue,
        None,
        72.0,
    );
    let width = control(
        view,
        cx,
        (device, metadata),
        WIDTH,
        ControlPresentation::InlineValue,
        None,
        56.0,
    );
    let filter_row = row()
        .child(filter)
        .child(theme::text("freq"))
        .child(frequency)
        .child(theme::text("width"))
        .child(width);
    let disclosure = segment_button(
        format!("delay-lfo-expand-{}", device.value()),
        if expanded { "▾ lfo" } else { "▸ lfo" },
        "show delay modulation controls",
        theme::DeviceActionRole::Normal,
        expanded,
    )
    .w(px(40.0))
    .on_click(cx.listener(move |view, _, _, cx| {
        if !finalize_controls(view, cx) {
            return;
        }
        if let Some(panel) = view.device_panel.delay_panels.get_mut(&device) {
            panel.expanded = !panel.expanded;
        }
        cx.notify();
    }));
    let delay_mod = control(
        view,
        cx,
        (device, metadata),
        LFO_DELAY,
        ControlPresentation::InlineValue,
        None,
        54.0,
    );
    let filter_mod = control(
        view,
        cx,
        (device, metadata),
        LFO_FILTER,
        ControlPresentation::InlineValue,
        None,
        54.0,
    );
    let modulation = row()
        .child(disclosure)
        .child(theme::text("delay"))
        .child(delay_mod)
        .child(theme::text("filter"))
        .child(filter_mod);
    let mut center = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(plot(graph).min_h(px(60.0)))
        .child(filter_row)
        .child(modulation);
    if expanded {
        let mode = value(view, metadata, device, LFO_MODE).unwrap_or(ParameterValue::Choice(0));
        let timing_id = match mode {
            ParameterValue::Choice(0) => LFO_RATE,
            ParameterValue::Choice(1) => LFO_TIME,
            _ => LFO_DIVISION,
        };
        let mode = control(
            view,
            cx,
            (device, metadata),
            LFO_MODE,
            ControlPresentation::CompactSelector,
            None,
            64.0,
        );
        let rate = control(
            view,
            cx,
            (device, metadata),
            timing_id,
            if [LFO_RATE, LFO_TIME].contains(&timing_id) {
                ControlPresentation::InlineValue
            } else {
                ControlPresentation::CompactSelector
            },
            None,
            64.0,
        );
        let wave = control(
            view,
            cx,
            (device, metadata),
            LFO_WAVE,
            ControlPresentation::CompactSelector,
            None,
            72.0,
        );
        let morph = control(
            view,
            cx,
            (device, metadata),
            LFO_MORPH,
            ControlPresentation::InlineValue,
            None,
            54.0,
        );
        let quality = control(
            view,
            cx,
            (device, metadata),
            HIGH_QUALITY,
            ControlPresentation::CompactButton,
            Some("hi quality"),
            60.0,
        );
        let loudness = control(
            view,
            cx,
            (device, metadata),
            EQUAL_LOUDNESS,
            ControlPresentation::CompactButton,
            Some("eq loudness"),
            64.0,
        );
        center = center
            .child(row().child(mode).child(rate).child(wave))
            .child(
                row()
                    .child(theme::text("morph"))
                    .child(morph)
                    .child(quality)
                    .child(loudness),
            );
    }
    crate::device_composition::body()
        .gap(px(2.0))
        .text_size(px(8.0))
        .line_height(px(10.0))
        .child(timings)
        .child(center)
        .child(output)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}
fn graph_event(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    generation: u64,
    event: ResponseGraphEvent,
) {
    if generation != view.project_generation {
        return;
    }
    let rate = view
        .state
        .as_ref()
        .map_or(48000, |s| s.project.sample_rate());
    let Ok([frequency, width]) = filter_values(event.position, rate) else {
        return;
    };
    let values = ParameterPairValue {
        first: ParameterValue::Float(frequency),
        second: ParameterValue::Float(width),
    };
    if event.phase == ResponseGraphPhase::Begin {
        if !finalize_controls(view, cx) {
            return;
        }
        if let Some(panel) = view.device_panel.delay_panels.get_mut(&device) {
            panel.gesture = None;
        }
        let initial = view
            .device_panel
            .delay_panels
            .get(&device)
            .and_then(|p| p.preview.as_ref())
            .and_then(|(state, _, _)| {
                let metadata = hex_engine::native_device_view(state).ok()?;
                Some(ParameterPairValue {
                    first: metadata
                        .parameters
                        .iter()
                        .find(|p| p.descriptor.id == FREQUENCY)?
                        .value,
                    second: metadata
                        .parameters
                        .iter()
                        .find(|p| p.descriptor.id == WIDTH)?
                        .value,
                })
            });
        if let Some(initial) = initial {
            if let Err(error) = view.begin_device_pair_gesture(device, FREQUENCY, WIDTH, initial) {
                view.device_panel.message = Some(error.into());
            } else if let Some(key) = view.device_gesture.snapshot().map(|s| s.key) {
                if let Some(panel) = view.device_panel.delay_panels.get_mut(&device) {
                    panel.gesture = Some(key);
                }
            }
        }
        return;
    }
    let key = view
        .device_panel
        .delay_panels
        .get(&device)
        .and_then(|p| p.gesture);
    if !view.device_gesture.snapshot().is_some_and(|s|Some(s.key)==key&&matches!(s.target,GestureTarget::DeviceParameterPair{device:d,first:FREQUENCY,second:WIDTH} if d==device)){return;}
    let result = match event.phase {
        ResponseGraphPhase::Change => view.change_device_pair_gesture(values),
        ResponseGraphPhase::Commit => view.finish_device_pair_gesture(values),
        ResponseGraphPhase::Cancel => view.cancel_device_gesture(),
        _ => return,
    };
    if let Err(error) = result {
        view.device_panel.message = Some(error.into());
    } else if event.phase != ResponseGraphPhase::Cancel {
        for (parameter, value) in [(FREQUENCY, values.first), (WIDTH, values.second)] {
            view.device_panel
                .pending_values
                .insert(ParameterKey { device, parameter }, value);
        }
    }
    if matches!(
        event.phase,
        ResponseGraphPhase::Commit | ResponseGraphPhase::Cancel
    ) {
        if let Some(panel) = view.device_panel.delay_panels.get_mut(&device) {
            panel.gesture = None;
        }
    }
    if event.phase == ResponseGraphPhase::Cancel {
        for parameter in [FREQUENCY, WIDTH] {
            view.device_panel
                .pending_values
                .remove(&ParameterKey { device, parameter });
        }
    }
    cx.notify();
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn graph_for_test(view: &EditorView, device: DeviceId) -> Entity<ResponseGraph> {
    view.device_panel.delay_panels[&device].graph.clone()
}
