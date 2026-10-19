//! Sparse EQ editing. Opaque edits use atomic canonical state replacement;
//! scalar graph drags use the existing paired-parameter gesture protocol.
use super::*;
use gpui::{Entity, Subscription};
use hex_engine::eq::{self, AutomationLane, AutomationPoint, EqState, Node, NodeId, NodeParameter};
use hex_lattice_contract::ParameterId;
use hex_session::device_gesture::{GestureTarget, ParameterPairValue};
use hex_widgets::{
    ResponseGraph, ResponseGraphEvent, ResponseGraphModel, ResponseGraphNode, ResponseGraphPhase,
};

mod axes;
mod response;
mod spectrum;
pub(super) use spectrum::SpectrumSelection;
pub(in crate::editor) use spectrum::reconcile as reconcile_spectrum;

const WINDOW_BEATS: u64 = 16;
// Outer five percent of the inset logarithmic plot; existing node hits win.
const CUT_CREATION_EDGE: f32 = 0.05;
// Keep each complete sixteen-beat page, including its exclusive end, inside
// unsigned Q32.32. The following page would end at 2^64 and overflow.
const MAX_AUTOMATION_PAGE: u32 = (1 << 28) - 2;

// Content geometry only: preserve shared device-action paint and tooltip/input
// behavior, while giving complete labels their intrinsic width and padding.
fn text_action_button(
    id: String,
    label: &'static str,
    tooltip: &'static str,
    role: theme::DeviceActionRole,
    selected: bool,
) -> Stateful<Div> {
    action_button(id, label, tooltip, role, selected)
        .w_auto()
        .min_w(px(hex_widgets::DEVICE_HEADER_ACTION_SIZE))
        .px(px(3.0))
        .text_size(px(8.0))
        .line_height(px(10.0))
        .whitespace_nowrap()
}

fn change_automation_page(page: u32, delta: i32) -> u32 {
    page.saturating_add_signed(delta).min(MAX_AUTOMATION_PAGE)
}

fn automation_page_start(page: u32) -> u64 {
    (u64::from(page.min(MAX_AUTOMATION_PAGE)) * WINDOW_BEATS) << 32
}

fn automation_point_beat(page: u32, x: f32) -> u64 {
    // Both terms are bounded so their sum fits even at the final page's end.
    automation_page_start(page)
        + ((WINDOW_BEATS << 32) as f64 * f64::from(x.clamp(0.0, 1.0))).round() as u64
}

pub(super) struct Panel {
    entity: Entity<ResponseGraph>,
    _subscription: Subscription,
    generation: u64,
    authored_state: NativeDeviceState,
    selected: Option<NodeId>,
    armed: bool,
    show_input: bool,
    focus_subscription: Option<Subscription>,
    bank: u8,
    automation: Option<NodeParameter>,
    selected_point: Option<usize>,
    page: u32,
    preview: Option<(NativeDeviceState, EqState)>,
    creation: Option<Creation>,
    creation_selection: Option<CreationSelection>,
    response: Option<(NativeDeviceState, u32, response::Curves)>,
}

struct Creation {
    expected: NativeDeviceState,
    expected_variant: hex_project::DeviceVariant,
    state: EqState,
    next_id: u64,
    id: NodeId,
    selected_before: Option<NodeId>,
    sample_rate: u32,
}

struct CreationSelection {
    state: NativeDeviceState,
    id: NodeId,
    previous: Option<NodeId>,
    bank: u8,
}

impl Creation {
    fn update(&mut self, position: [f32; 2]) {
        if let Some(node) = self.state.nodes.iter_mut().find(|node| node.id == self.id) {
            node.frequency_hz = frequency_from_x(position[0], self.sample_rate);
            if matches!(node.kind, eq::NodeKind::LowCut | eq::NodeKind::HighCut) {
                node.q = q_from_y(position[1]);
            } else {
                node.gain_db = position[1] * 48.0 - 24.0;
            }
        }
    }
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
    canonical: &NativeDeviceState,
    device_view: &NativeDeviceView,
    sample_rate: u32,
) -> Stateful<Div> {
    let Ok(state) = EqState::decode(canonical) else {
        return div()
            .id("eq-unavailable")
            .child(theme::text("eq state unavailable"));
    };
    if view
        .device_panel
        .eq_panels
        .get(&device)
        .is_some_and(|p| p.generation != view.project_generation)
    {
        view.device_panel.eq_panels.remove(&device);
    }
    if !view.device_panel.eq_panels.contains_key(&device) {
        let style = theme::device_style(theme::DeviceStyleRole::Display);
        let entity = cx.new(|cx| {
            let mut graph =
                ResponseGraph::new(cx, format!("eq-response-{}", device.value()), style);
            graph.set_editable(true, cx);
            graph.set_tab_navigation(true, cx);
            graph.set_creation_enabled(true);
            graph.set_backspace_removal(true);
            graph
        });
        let generation = view.project_generation;
        let subscription = cx.subscribe(&entity, move |view, _, event: &ResponseGraphEvent, cx| {
            handle_event(view, cx, device, generation, *event);
        });
        view.device_panel.eq_panels.insert(
            device,
            Panel {
                entity,
                _subscription: subscription,
                generation,
                authored_state: canonical.clone(),
                selected: state.nodes.first().map(|n| n.id),
                armed: false,
                show_input: true,
                focus_subscription: None,
                bank: 0,
                automation: None,
                selected_point: None,
                page: 0,
                preview: None,
                creation: None,
                creation_selection: None,
                response: None,
            },
        );
    }
    let next_id = view.state.as_ref().map(|s| s.project.next_identity());
    let edits_idle = view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.device_gesture_barriers.is_empty();
    let Some(panel) = view.device_panel.eq_panels.get_mut(&device) else {
        return div().id("eq-unavailable");
    };
    if let Some(selection) = panel.creation_selection.take() {
        if &selection.state == canonical {
            if panel.selected == selection.previous
                && panel.bank == selection.bank
                && panel.automation.is_none()
            {
                panel.selected = Some(selection.id);
            }
        } else if !edits_idle && &panel.authored_state == canonical {
            panel.creation_selection = Some(selection);
        }
    }
    if panel.creation.as_ref().is_some_and(|creation| {
        &creation.expected != canonical
            || Some(creation.next_id) != next_id
            || panel.selected != Some(creation.id)
            || panel.automation.is_some()
            || creation
                .state
                .node(creation.id)
                .is_ok_and(|n| n.bank != panel.bank)
    }) {
        if let Some(creation) = panel.creation.take() {
            if panel.selected == Some(creation.id) {
                panel.selected = creation.selected_before;
            }
        }
        panel.entity.update(cx, |graph, cx| graph.cancel(cx));
    }
    if &panel.authored_state != canonical {
        // Point IDs are local lane indices. Any authored edit, including
        // create/delete/undo/redo, may shift them. Local drag previews keep the
        // same authored key until commit and are preserved during the gesture.
        panel.authored_state = canonical.clone();
        // Events share this persistent mode with rendering. Repair only when
        // canonical state changes: a newly created node is selected optimistically
        // before its ID appears in the worker's published state.
        panel.selected = panel.selected.filter(|id| state.node(*id).is_ok());
        if panel.selected.is_none() {
            panel.automation = None;
        }
        panel.selected_point = None;
        panel.preview = None;
    }
    let displayed = panel
        .creation
        .as_ref()
        .map_or_else(
            || panel.preview.as_ref().map_or(&state, |(_, s)| s),
            |creation| &creation.state,
        )
        .clone();
    let selected = panel.selected.filter(|id| displayed.node(*id).is_ok());
    let bank = panel.bank;
    let automation = panel.automation;
    let selected_point = panel.selected_point;
    let page = panel.page.min(MAX_AUTOMATION_PAGE);
    let entity = panel.entity.clone();
    spectrum::sync_selected(view, device, owner);
    let model = if let (Some(id), Some(field)) = (selected, automation) {
        automation_model(&displayed, id, field, page, selected_point)
    } else {
        response_model(
            view,
            device,
            &displayed,
            device_view,
            sample_rate,
            (bank, selected),
            owner,
        )
    };
    if let Some(model) = model {
        entity.update(cx, |graph, cx| {
            graph.set_creation_drag_enabled(automation.is_none());
            let _ = graph.set_axes(
                if automation.is_none() {
                    Some(axes::frequency_axes(sample_rate))
                } else {
                    None
                },
                cx,
            );
            let _ = graph.set_analyzer_style(theme::device_analyzer_style(), cx);
            let _ = graph.set_model(
                model,
                theme::device_style(theme::DeviceStyleRole::Display),
                cx,
            );
        });
    }
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: eq::EQ_TYPE_ID,
    };
    let mut toolbar = div()
        .debug_selector(move || format!("eq-toolbar-{}", device.value()))
        .flex()
        .items_center()
        .gap(px(3.0))
        .flex_wrap()
        .content_start()
        .w_full()
        .text_size(px(8.0))
        .line_height(px(10.0))
        .flex_shrink_0();
    toolbar = toolbar.child(control(
        view,
        cx,
        device,
        identity,
        device_view,
        eq::MODE,
        "mode",
    ));
    toolbar = toolbar.child(control(
        view,
        cx,
        device,
        identity,
        device_view,
        eq::OUTPUT_GAIN,
        "out",
    ));
    for (index, label) in ["st", "l", "r", "m", "s"].into_iter().enumerate() {
        toolbar = toolbar.child(
            text_action_button(
                format!("eq-bank-{}-{index}", device.value()),
                label,
                "select authored channel bank",
                theme::DeviceActionRole::Normal,
                index as u8 == bank,
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                if finalize_controls(view, cx) {
                    if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                        p.bank = index as u8;
                        p.automation = None;
                        p.preview = None;
                    }
                }
                cx.notify();
            })),
        );
    }
    toolbar = toolbar.child(theme::text(format!("{} / 64", state.nodes.len())));
    toolbar = {
        let control = spectrum::toggle(view, cx, device, owner);
        toolbar.child(control)
    };
    let show_input = view
        .device_panel
        .eq_panels
        .get(&device)
        .is_some_and(|panel| panel.show_input);
    toolbar = toolbar.child(
        text_action_button(
            format!("eq-spectrum-input-{}", device.value()),
            "input",
            "show actual pre-eq input spectrum; output stays visible",
            theme::DeviceActionRole::Normal,
            show_input,
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                panel.show_input = !panel.show_input;
            }
            cx.notify();
        })),
    );
    let armed = view
        .device_panel
        .eq_panels
        .get(&device)
        .is_some_and(|p| p.armed);
    toolbar = toolbar.child(
        text_action_button(
            format!("eq-audition-{}", device.value()),
            "listen",
            "hold a node to audition its filter",
            theme::DeviceActionRole::Normal,
            armed,
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                panel.armed = !panel.armed;
            }
            if let Some(audio) = view.audio.as_mut() {
                let _ = audio.control.audition_cancel();
            }
            cx.notify();
        })),
    );
    let mut bottom = div()
        .debug_selector(move || format!("eq-footer-{}", device.value()))
        .flex()
        .items_center()
        .gap(px(2.0))
        .flex_wrap()
        .content_start()
        .w_full()
        .flex_shrink_0();
    let mut actions = div().flex().items_center().gap(px(2.0)).h(px(14.0));
    if let Some(id) = selected {
        if automation.is_none() {
            for (field, label) in [
                (NodeParameter::Enabled, "on"),
                (NodeParameter::Kind, "shape"),
                (NodeParameter::Frequency, "freq"),
                (NodeParameter::Gain, "gain"),
                (NodeParameter::Q, "q"),
                (NodeParameter::Slope, "slope"),
            ] {
                if let Ok(parameter) = id.parameter(field) {
                    bottom = bottom.child(control(
                        view,
                        cx,
                        device,
                        identity,
                        device_view,
                        parameter,
                        label,
                    ));
                }
            }
        }
        actions = actions.child(
            text_action_button(
                format!("eq-auto-{}", device.value()),
                if automation.is_some() {
                    "response"
                } else {
                    "automate"
                },
                "edit selected node arrangement automation",
                theme::DeviceActionRole::Normal,
                true,
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                if finalize_controls(view, cx) {
                    if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                        p.automation = if p.automation.is_some() {
                            None
                        } else {
                            Some(NodeParameter::Gain)
                        };
                        p.selected_point = None;
                        p.preview = None;
                    }
                }
                cx.notify();
            })),
        );
        actions = actions.child(
            text_action_button(
                format!("eq-remove-{}", device.value()),
                "remove",
                "remove selected node and its automation",
                theme::DeviceActionRole::Normal,
                true,
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                edit(view, cx, device, 0, move |s, _| s.remove(id));
            })),
        );
    } else {
        bottom = bottom.child(theme::text(
            "click the response to add a node · select and drag to edit",
        ));
    }
    if let Some(field) = automation {
        let mut controls = div()
            .debug_selector(move || format!("eq-automation-toolbar-{}", device.value()))
            .flex()
            .items_center()
            .gap(px(2.0))
            .flex_wrap()
            .text_size(px(8.0));
        for (target, label) in [
            (NodeParameter::Frequency, "freq"),
            (NodeParameter::Gain, "gain"),
            (NodeParameter::Q, "q"),
            (NodeParameter::Kind, "shape"),
            (NodeParameter::Slope, "slope"),
            (NodeParameter::Enabled, "on"),
        ] {
            controls = controls.child(
                text_action_button(
                    format!("eq-auto-field-{}-{label}", device.value()),
                    label,
                    "select automation parameter",
                    theme::DeviceActionRole::Normal,
                    field == target,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                        p.automation = Some(target);
                        p.selected_point = None;
                        p.preview = None;
                    }
                    cx.notify();
                })),
            );
        }
        for (delta, label) in [(-1, "←"), (1, "→")] {
            controls = controls.child(
                text_action_button(
                    format!("eq-auto-page-{}-{delta}", device.value()),
                    label,
                    "show previous or next sixteen quarter-note beats",
                    theme::DeviceActionRole::Normal,
                    true,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                        p.page = change_automation_page(p.page, delta);
                        p.selected_point = None;
                        p.preview = None;
                    }
                    cx.notify();
                })),
            );
        }
        controls = controls.child(theme::text(format!(
            "{}–{} beats",
            u64::from(page) * 16,
            (u64::from(page) + 1) * 16
        )));
        if let (Some(id), Some(point)) = (selected, selected_point) {
            controls = controls.child(
                text_action_button(
                    format!("eq-auto-delete-{}", device.value()),
                    "− point",
                    "remove selected automation point",
                    theme::DeviceActionRole::Normal,
                    true,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    edit(view, cx, device, 0, move |s, _| {
                        let mut lane = lane(s, id, field);
                        if point < lane.points.len() {
                            lane.points.remove(point);
                        }
                        s.set_lane(lane)
                    });
                })),
            );
        }
        if let Some(id) = selected {
            controls = controls.child(
                div()
                    .debug_selector(move || format!("eq-playhead-{}", device.value()))
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .flex_shrink_0()
                    .child(theme::text(playhead_readout(view, &state, id, field))),
            );
        }
        toolbar = div()
            .flex()
            .flex_col()
            .h_auto()
            .flex_shrink_0()
            .child(toolbar)
            .child(controls);
    }
    let body = bottom;
    let sidebar = crate::device_composition::eq_sidebar(
        format!("eq-sidebar-{}", device.value()),
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(toolbar)
            .child(body),
        actions,
    )
    .debug_selector(move || format!("eq-sidebar-{}", device.value()));
    crate::device_composition::eq_graph_body(entity, sidebar)
        .on_children_prepainted(spectrum::focus_listener(view, cx, device))
        .id(ElementId::Name(
            format!("eq-body-{}", device.value()).into(),
        ))
}

fn playhead_readout(
    view: &EditorView,
    state: &EqState,
    node: NodeId,
    field: NodeParameter,
) -> String {
    let base = state.node(node).ok().map(|node| node.value(field));
    let beat = view
        .audio
        .as_ref()
        .and_then(|audio| audio.control.status())
        .filter(|status| status.playing.load(std::sync::atomic::Ordering::Relaxed))
        .and_then(|status| {
            view.state
                .as_ref()?
                .project
                .tempo_map()
                .frame_to_beat(status.position.load(std::sync::atomic::Ordering::Relaxed))
                .ok()
        })
        .map(|beat| beat.raw());
    let automated = beat.and_then(|beat| lane(state, node, field).value_at(beat));
    let value = automated.or(base);
    let label = if automated.is_some() {
        "playhead"
    } else {
        "authored"
    };
    match value {
        Some(ParameterValue::Float(value)) => format!(
            "{label} {:.2} {}",
            value,
            match field {
                NodeParameter::Frequency => "hz",
                NodeParameter::Gain => "db",
                _ => "q",
            }
        ),
        Some(ParameterValue::Choice(value)) => format!(
            "{label} {}",
            match field {
                NodeParameter::Slope => format!("{} db/oct", 12 * (value + 1)),
                NodeParameter::Kind => [
                    "bell",
                    "low shelf",
                    "high shelf",
                    "low cut",
                    "high cut",
                    "notch"
                ]
                .get(value as usize)
                .copied()
                .unwrap_or("unknown")
                .to_owned(),
                _ => value.to_string(),
            }
        ),
        Some(ParameterValue::Bool(value)) => {
            format!("{label} {}", if value { "on" } else { "off" })
        }
        _ => "value unavailable".to_owned(),
    }
}
fn control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    device_view: &NativeDeviceView,
    id: ParameterId,
    label: &'static str,
) -> Stateful<Div> {
    let Some(parameter) = device_view
        .parameters
        .iter()
        .find(|p| p.descriptor.id == id)
    else {
        return div().id(ElementId::Name(format!("eq-missing-{}", id.value()).into()));
    };
    let numeric = matches!(
        parameter.descriptor.kind,
        hex_lattice_contract::ParameterKind::Float { .. }
    );
    let presentation = if numeric {
        ControlPresentation::InlineValue
    } else {
        match parameter.descriptor.kind {
            hex_lattice_contract::ParameterKind::Bool => ControlPresentation::CompactButton,
            _ => ControlPresentation::CompactSelector,
        }
    };
    let control = parameter_control_with_presentation(
        view,
        cx,
        device,
        identity,
        parameter,
        presentation,
        ControlText {
            label: Some(label),
            codec: None,
        },
    );
    let row = if numeric {
        let width = if label == "out" { 58.0 } else { 104.0 };
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
        let control = control.w(px(width)).flex_shrink_0();
        div()
            .flex()
            .items_center()
            .h(px(16.0))
            .w(px(if label == "out" { 78.0 } else { 140.0 }))
            .gap(px(2.0))
            .child(
                div()
                    .w(px(if label == "out" { 18.0 } else { 32.0 }))
                    .flex_shrink_0()
                    .text_size(px(8.0))
                    .child(theme::text(label)),
            )
            .child(control)
    } else {
        let width = if label == "mode" {
            72.0
        } else if label == "shape" {
            120.0
        } else if label == "on" {
            14.0
        } else {
            140.0
        };
        if label == "slope" {
            let control = control.w(px(104.0));
            div()
                .w(px(width))
                .h(px(14.0))
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(
                    div()
                        .w(px(32.0))
                        .text_size(px(8.0))
                        .child(theme::text(label)),
                )
                .child(control)
        } else {
            control.w(px(width)).h(px(14.0))
        }
    };
    row.id(ElementId::Name(
        format!("eq-control-{}-{}", device.value(), id.value()).into(),
    ))
    .debug_selector(move || format!("eq-control-{}-{}", device.value(), id.value()))
    .flex_shrink_0()
}

fn current(view: &EditorView, device: DeviceId) -> Option<NativeDeviceState> {
    Some(view.state.as_ref()?.project.device(device)?.state().clone())
}
fn ready(view: &mut EditorView, cx: &mut Context<EditorView>) -> bool {
    if !finalize_controls(view, cx) {
        return false;
    }
    if view.worker_requests_pending != 0
        || !view.pending_requests.is_empty()
        || !view.device_gesture_barriers.is_empty()
        || view.device_gesture.snapshot().is_some()
    {
        view.device_panel.message = Some("wait for the current device edit, then retry".into());
        return false;
    }
    true
}
fn submit(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    expected: NativeDeviceState,
    state: NativeDeviceState,
    reserve: u64,
) {
    if !ready(view, cx) {
        return;
    }
    let Some(project) = view.state.as_ref().map(|s| &s.project) else {
        return;
    };
    let next = project.next_identity();
    let Some(expected_variant) = project
        .device(device)
        .map(hex_project::Device::active_variant)
    else {
        return;
    };
    let reserved_ids = if reserve == 0 {
        0
    } else {
        EqState::decode(&state).map_or(reserve, |s| {
            u64::from(s.next_node_id).saturating_sub(next).max(reserve)
        })
    };
    view.enqueue_device_barrier(Request::Edit(
        Command::ReplaceDeviceStateWithIdentityReservation {
            device,
            expected_state: expected,
            expected_variant,
            state,
            expected_next_id: next,
            reserved_ids,
        },
    ));
    cx.notify();
}
fn edit(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    reserve: u64,
    change: impl FnOnce(&mut EqState, u64) -> Result<(), hex_lattice_contract::DeviceContractError>,
) {
    if !ready(view, cx) {
        return;
    }
    let Some(expected) = current(view, device) else {
        return;
    };
    let Some(next) = view.state.as_ref().map(|s| s.project.next_identity()) else {
        return;
    };
    let result = EqState::decode(&expected).and_then(|mut s| {
        let first = next.max(u64::from(s.next_node_id));
        change(&mut s, first)?;
        s.encode()
    });
    match result {
        Ok(state) => submit(view, cx, device, expected, state, reserve),
        Err(error) => {
            view.device_panel.message = Some(format!(
                "eq edit refused · {error} · limit: 64 nodes, 64 lanes, 160 points; identity space is finite"
            ))
        }
    }
    cx.notify();
}
pub(super) fn upgrade(view: &mut EditorView, cx: &mut Context<EditorView>, device: DeviceId) {
    if !ready(view, cx) {
        return;
    }
    let Some(expected) = current(view, device) else {
        return;
    };
    let Some(next) = view.state.as_ref().map(|s| s.project.next_identity()) else {
        return;
    };
    let rate = view
        .state
        .as_ref()
        .map_or(48000, |s| s.project.sample_rate());
    let result = EqState::upgrade_legacy(&expected, next)
        .and_then(|s| s.encode())
        .and_then(|s| {
            hex_engine::native_effect_frequency_response(&s, rate, &[1000.0])
                .map_err(|_| hex_lattice_contract::DeviceContractError::InvalidParameterValue)?;
            Ok(s)
        });
    match result {
        Ok(state) => submit(view, cx, device, expected, state, 40),
        Err(error) => {
            view.device_panel.message = Some(format!(
                "eq upgrade refused · {error}; legacy state preserved"
            ))
        }
    }
    cx.notify();
}
fn lane(state: &EqState, node: NodeId, parameter: NodeParameter) -> AutomationLane {
    state
        .lanes
        .iter()
        .find(|l| l.node == node && l.parameter == parameter)
        .cloned()
        .unwrap_or(AutomationLane {
            node,
            parameter,
            points: Vec::new(),
        })
}
fn response_model(
    view: &mut EditorView,
    device: DeviceId,
    state: &EqState,
    device_view: &NativeDeviceView,
    rate: u32,
    selection: (u8, Option<NodeId>),
    owner: hex_engine::ProcessorChainOwner,
) -> Option<ResponseGraphModel> {
    let (bank, selected) = selection;
    let mut authored = state.encode().ok()?;
    for parameter in &device_view.parameters {
        let effective = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: parameter.descriptor.id,
            },
            parameter.value,
        );
        if effective != parameter.value {
            authored = hex_engine::patch_native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                &authored,
                parameter.descriptor.id,
                effective,
            )
            .ok()?;
        }
    }
    let state = EqState::decode(&authored).ok()?;
    let upper = (rate as f32 * 0.49).min(20000.0);
    if view
        .device_panel
        .eq_panels
        .get(&device)
        .is_none_or(|panel| {
            panel
                .response
                .as_ref()
                .is_none_or(|(state, cached_rate, _)| state != &authored || *cached_rate != rate)
        })
    {
        let response = response::curves(&authored, &state, rate)?;
        view.device_panel.eq_panels.get_mut(&device)?.response =
            Some((authored.clone(), rate, response));
    }
    let response = &view
        .device_panel
        .eq_panels
        .get(&device)?
        .response
        .as_ref()?
        .2;
    let channel = usize::from(bank == 2 || bank == 4);
    let primary = response[channel].clone();
    let secondary = response[1 - channel].clone();
    let nodes = state
        .nodes
        .iter()
        .filter(|n| n.bank == bank)
        .map(|n| ResponseGraphNode {
            id: n.id.value(),
            position: [
                axes::x(n.frequency_hz, rate),
                if matches!(
                    n.kind,
                    eq::NodeKind::LowCut | eq::NodeKind::HighCut | eq::NodeKind::Notch
                ) {
                    (n.q / 0.1).ln() / (18.0_f32 / 0.1).ln()
                } else {
                    (n.gain_db + 24.0) / 48.0
                },
            ],
            enabled: n.enabled,
            selected: selected == Some(n.id),
        })
        .collect();
    let identity = hex_engine::PostNodeIdentity::new(owner, device.value(), eq::EQ_TYPE_ID);
    let spectra = view.device_panel.spectrum_result.as_ref().filter(|result| {
        view.audio
            .as_ref()
            .and_then(|audio| audio.control.spectrum_desired())
            .is_some_and(|target| {
                target.identity == identity
                    && target.token == result.output.stamp.target
                    && target.project_generation == result.output.stamp.generation
                    && target.sample_rate_hz == result.output.stamp.sample_rate_hz
                    && result.output.stamp.applied_revision >= target.activation_floor
            })
    });
    let show_input = view
        .device_panel
        .eq_panels
        .get(&device)
        .is_some_and(|panel| panel.show_input);
    let (analyzer, input_analyzer) = spectrum_layers(spectra, show_input, upper);
    Some(ResponseGraphModel {
        primary,
        secondary,
        analyzer,
        input_analyzer,
        nodes,
    })
}
type SpectrumCurve = Vec<[f32; 2]>;

fn spectrum_layers(
    spectra: Option<&hex_audio::spectrum_worker::PublishedSpectra>,
    show_input: bool,
    upper: f32,
) -> (Option<SpectrumCurve>, Option<SpectrumCurve>) {
    let curve = |frame: &hex_audio::spectrum::SpectrumFrame| {
        frame
            .bins_dbfs
            .iter()
            .enumerate()
            .map(|(index, db)| {
                let (low, high) = hex_audio::spectrum::band_bounds_hz(frame.sample_rate_hz, index)
                    .unwrap_or((20.0, 20000.0));
                [
                    axes::x_with_upper((low * high).sqrt(), upper),
                    super::spectrum_display::level(*db),
                ]
            })
            .collect()
    };
    (
        spectra.map(|pair| curve(&pair.output.frame)),
        spectra
            .and_then(|pair| pair.input.as_ref())
            .filter(|_| show_input)
            .map(curve),
    )
}

fn automation_model(
    state: &EqState,
    node: NodeId,
    field: NodeParameter,
    page: u32,
    selected: Option<usize>,
) -> Option<ResponseGraphModel> {
    let lane = lane(state, node, field);
    let mut evaluated = lane.clone();
    evaluated.points.sort_by_key(|point| point.beat);
    let start = automation_page_start(page);
    let duration = WINDOW_BEATS << 32;
    let base = field.normalized(state.node(node).ok()?.value(field)).ok()?;
    let mut curve = Vec::with_capacity(96);
    for i in 0..96 {
        let beat = start + (duration as f64 * i as f64 / 95.0) as u64;
        let y = evaluated
            .value_at(beat)
            .and_then(|v| field.normalized(v).ok())
            .unwrap_or(base);
        curve.push([i as f32 / 95.0, y]);
    }
    let nodes = lane
        .points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.beat >= start && p.beat <= start + duration)
        .map(|(index, p)| ResponseGraphNode {
            id: index as u32 + 1,
            position: [(p.beat - start) as f32 / duration as f32, p.normalized],
            enabled: true,
            selected: selected == Some(index),
        })
        .collect();
    Some(ResponseGraphModel {
        primary: curve.clone(),
        secondary: curve,
        analyzer: None,
        input_analyzer: None,
        nodes,
    })
}
fn audition(view: &mut EditorView, device: DeviceId, id: NodeId) {
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let owner = state
        .project
        .tracks()
        .iter()
        .find(|track| track.devices().iter().any(|d| d.id() == device))
        .map(|track| hex_engine::ProcessorChainOwner::Track(track.id().value()))
        .or_else(|| {
            state
                .project
                .return_tracks()
                .iter()
                .find(|track| track.devices().iter().any(|d| d.id() == device))
                .map(|track| hex_engine::ProcessorChainOwner::Return(track.id().value()))
        });
    let Some(owner) = owner else {
        return;
    };
    let Some(floor) = view
        .audio_admission
        .published_audio_revision(view.project_generation, state.revision)
    else {
        view.device_panel.message =
            Some("wait for the current eq revision before auditioning".into());
        return;
    };
    let Some(encoded) = current(view, device) else {
        return;
    };
    let Ok(settings) = EqState::decode(&encoded) else {
        return;
    };
    let Ok(node) = settings.node(id) else {
        return;
    };
    let lane = if node.bank == 2 || node.bank == 4 {
        1
    } else {
        0
    };
    if let Some(audio) = view.audio.as_mut() {
        let target = hex_audio::processor_audition::ProcessorAuditionTarget {
            identity: hex_engine::PostNodeIdentity::new(owner, device.value(), eq::EQ_TYPE_ID),
            project_generation: view.project_generation,
            activation_floor: floor,
            sample_rate_hz: audio.sample_rate,
            selection: hex_engine::ProcessorAuditionSelection {
                index: id.value(),
                lane,
            },
        };
        if audio.control.audition_select(target).is_err() {
            view.device_panel.message = Some("eq audition unavailable for this node".into());
        }
    }
}
fn frequency_from_x(x: f32, rate: u32) -> f32 {
    let upper = (rate as f32 * 0.49).clamp(20.0, 20000.0);
    (20.0 * ((upper / 20.0).ln() * x.clamp(0.0, 1.0)).exp()).clamp(20.0, upper)
}
fn q_from_y(y: f32) -> f32 {
    (0.1 * ((18.0_f32 / 0.1).ln() * y.clamp(0.0, 1.0)).exp()).clamp(0.1, 18.0)
}
fn creation_kind(x: f32) -> eq::NodeKind {
    if x <= CUT_CREATION_EDGE {
        eq::NodeKind::LowCut
    } else if x >= 1.0 - CUT_CREATION_EDGE {
        eq::NodeKind::HighCut
    } else {
        eq::NodeKind::Bell
    }
}
fn begin_creation(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    position: [f32; 2],
    bank: u8,
) {
    if !ready(view, cx) {
        return;
    }
    let Some(expected) = current(view, device) else {
        return;
    };
    let Some(project) = view.state.as_ref().map(|s| &s.project) else {
        return;
    };
    let next_id = project.next_identity();
    let Some(expected_variant) = project
        .device(device)
        .map(hex_project::Device::active_variant)
    else {
        return;
    };
    let sample_rate = project.sample_rate();
    let result = EqState::decode(&expected).and_then(|mut state| {
        let id = NodeId::new(next_id.max(u64::from(state.next_node_id)))?;
        let mut node = Node::new(id, frequency_from_x(position[0], sample_rate), 0.0, bank)?;
        node.kind = creation_kind(position[0]);
        state.add(node)?;
        let mut creation = Creation {
            expected,
            expected_variant,
            state,
            next_id,
            id,
            selected_before: None,
            sample_rate,
        };
        creation.update(position);
        creation.state.encode()?;
        Ok(creation)
    });
    match result {
        Ok(mut creation) => {
            if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                creation.selected_before = panel.selected;
                panel.selected = Some(creation.id);
                panel.creation = Some(creation);
            }
        }
        Err(error) => {
            view.device_panel.message = Some(format!(
                "eq edit refused · {error} · limit: 64 nodes; identity space is finite"
            ));
        }
    }
    cx.notify();
}

// The widget keeps node_id zero for this entire pointer stream. It cannot turn
// into a canonical node drag after release, publication, deletion or ID reuse.
fn creation_event(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    event: ResponseGraphEvent,
) {
    let Some(mut creation) = view
        .device_panel
        .eq_panels
        .get_mut(&device)
        .and_then(|panel| panel.creation.take())
    else {
        return;
    };
    let valid = current(view, device).as_ref() == Some(&creation.expected)
        && view.state.as_ref().is_some_and(|s| {
            s.project.next_identity() == creation.next_id
                && s.project
                    .device(device)
                    .is_some_and(|entry| entry.active_variant() == creation.expected_variant)
        })
        && view
            .device_panel
            .eq_panels
            .get(&device)
            .is_some_and(|panel| {
                panel.selected == Some(creation.id)
                    && panel.automation.is_none()
                    && creation
                        .state
                        .node(creation.id)
                        .is_ok_and(|node| node.bank == panel.bank)
            });
    if !valid || event.phase == ResponseGraphPhase::Cancel {
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            if panel.selected == Some(creation.id) {
                panel.selected = creation.selected_before;
            }
        }
        if !valid {
            view.device_panel.message = Some("eq creation cancelled · source changed".into());
        }
    } else if matches!(
        event.phase,
        ResponseGraphPhase::Commit
            | ResponseGraphPhase::Release
            | ResponseGraphPhase::ReleaseOutside
    ) {
        // Stationary releases retain the press position; dragging commits its
        // final clamped pointer, including a release outside the graph.
        creation.update(event.position);
        // Restore the prior canonical selection even when admission or worker
        // validation fails. Select the new ID only after that exact state is
        // published, and only if the user has not changed selection meanwhile.
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.selected = creation.selected_before;
        }
        if ready(view, cx) {
            if let Ok(state) = creation.state.encode() {
                let selected_state = state.clone();
                let admitted = view.enqueue_device_barrier(Request::Edit(
                    Command::ReplaceDeviceStateWithIdentityReservation {
                        device,
                        expected_state: creation.expected,
                        expected_variant: creation.expected_variant,
                        state,
                        expected_next_id: creation.next_id,
                        reserved_ids: u64::from(creation.state.next_node_id)
                            .saturating_sub(creation.next_id),
                    },
                ));
                if admitted {
                    if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                        panel.creation_selection = Some(CreationSelection {
                            state: selected_state,
                            id: creation.id,
                            previous: creation.selected_before,
                            bank: panel.bank,
                        });
                    }
                }
            }
        }
    } else {
        if event.phase == ResponseGraphPhase::Change {
            creation.update(event.position);
        }
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.creation = Some(creation);
        }
    }
    cx.notify();
}
fn handle_event(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    generation: u64,
    event: ResponseGraphEvent,
) {
    if generation != view.project_generation {
        return;
    }
    let Some(panel) = view.device_panel.eq_panels.get(&device) else {
        return;
    };
    let (selected, automation, bank, page) =
        (panel.selected, panel.automation, panel.bank, panel.page);
    if event.phase == ResponseGraphPhase::Remove {
        // Automation point IDs are lane-local indices, never stable band IDs.
        // The graph consumes the key in that mode, without deleting a band.
        if automation.is_some()
            || panel.creation.is_some()
            || panel.preview.is_some()
            || panel.creation_selection.is_some()
            || current(view, device).as_ref() != Some(&panel.authored_state)
        {
            return;
        }
        if let Some(id) = selected.filter(|id| id.value() == event.node_id) {
            edit(view, cx, device, 0, move |state, _| state.remove(id));
        }
        return;
    }
    if automation.is_none() && event.node_id == 0 {
        if event.phase == ResponseGraphPhase::Create {
            begin_creation(view, cx, device, event.position, bank);
        } else {
            creation_event(view, cx, device, event);
        }
        return;
    }
    if let (Some(id), Some(field)) = (selected, automation) {
        let point = event.node_id.checked_sub(1).map(|p| p as usize);
        match event.phase {
            ResponseGraphPhase::Select => {
                if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                    p.selected_point = point;
                }
            }
            ResponseGraphPhase::Create => {
                let beat = automation_point_beat(page, event.position[0]);
                edit(view, cx, device, 0, move |s, _| {
                    let mut lane = lane(s, id, field);
                    if let Some(p) = lane.points.iter_mut().find(|p| p.beat == beat) {
                        p.normalized = event.position[1];
                    } else {
                        lane.points.push(AutomationPoint {
                            beat,
                            normalized: event.position[1],
                        });
                        lane.points.sort_by_key(|p| p.beat);
                    }
                    s.set_lane(lane)
                });
            }
            ResponseGraphPhase::Begin => {
                if !ready(view, cx) {
                    return;
                }
                if let Some(expected) = current(view, device) {
                    if let Ok(state) = EqState::decode(&expected) {
                        if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                            p.preview = Some((expected, state));
                        }
                    }
                }
            }
            ResponseGraphPhase::Change | ResponseGraphPhase::Commit => {
                let Some(index) = point else {
                    return;
                };
                let Some((expected, mut state)) = view
                    .device_panel
                    .eq_panels
                    .get(&device)
                    .and_then(|p| p.preview.clone())
                else {
                    return;
                };
                let mut lane = lane(&state, id, field);
                let Some(point) = lane.points.get_mut(index) else {
                    return;
                };
                let beat = automation_point_beat(page, event.position[0]);
                point.beat = beat;
                point.normalized = event.position[1];
                // During dragging point indices stay attached to the same lane;
                // ordering changes only once at the canonical commit.
                if event.phase == ResponseGraphPhase::Commit {
                    lane.points.sort_by_key(|p| p.beat);
                    if lane.points.windows(2).any(|p| p[0].beat == p[1].beat) {
                        view.device_panel.message =
                            Some("automation points need distinct beat positions".into());
                    } else if state.set_lane(lane).is_ok() {
                        if let Ok(encoded) = state.encode() {
                            submit(view, cx, device, expected, encoded, 0);
                        }
                    }
                    if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                        p.preview = None;
                        p.selected_point = None;
                    }
                } else {
                    state.lanes.retain(|l| l.node != id || l.parameter != field);
                    state.lanes.push(lane);
                    if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                        p.preview = Some((expected, state));
                    }
                }
            }
            ResponseGraphPhase::Cancel => {
                if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                    p.preview = None;
                }
            }
            _ => {}
        }
        cx.notify();
        return;
    }
    let Ok(id) = NodeId::new(u64::from(event.node_id)) else {
        return;
    };
    let Some(expected) = current(view, device) else {
        return;
    };
    let Ok(state) = EqState::decode(&expected) else {
        return;
    };
    let Ok(node) = state.node(id) else {
        return;
    };
    let first = match id.parameter(NodeParameter::Frequency) {
        Ok(v) => v,
        Err(_) => return,
    };
    let q_drag = matches!(
        node.kind,
        eq::NodeKind::LowCut | eq::NodeKind::HighCut | eq::NodeKind::Notch
    );
    let second = match id.parameter(if q_drag {
        NodeParameter::Q
    } else {
        NodeParameter::Gain
    }) {
        Ok(v) => v,
        Err(_) => return,
    };
    match event.phase {
        ResponseGraphPhase::Press => {
            if view
                .device_panel
                .eq_panels
                .get(&device)
                .is_some_and(|p| p.armed)
            {
                audition(view, device, id);
            }
        }
        ResponseGraphPhase::Release => {
            if let Some(audio) = view.audio.as_mut() {
                let _ = audio.control.audition_release();
            }
        }
        ResponseGraphPhase::ReleaseOutside => {
            if let Some(audio) = view.audio.as_mut() {
                let _ = audio.control.audition_cancel();
            }
        }
        ResponseGraphPhase::Select => {
            if finalize_controls(view, cx) {
                if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                    p.selected = Some(id);
                }
            }
        }
        ResponseGraphPhase::Begin => {
            if let Err(error) = view.begin_device_pair_gesture(
                device,
                first,
                second,
                ParameterPairValue {
                    first: ParameterValue::Float(node.frequency_hz),
                    second: ParameterValue::Float(if q_drag { node.q } else { node.gain_db }),
                },
            ) {
                view.device_panel.message = Some(error.into());
            }
        }
        ResponseGraphPhase::Change | ResponseGraphPhase::Commit => {
            if !view.device_gesture.snapshot().is_some_and(|s|matches!(s.target,GestureTarget::DeviceParameterPair{device:d,first:f,second:s}if d==device&&f==first&&s==second)){return;}
            let rate = view
                .state
                .as_ref()
                .map_or(48000, |s| s.project.sample_rate());
            let values = ParameterPairValue {
                first: ParameterValue::Float(frequency_from_x(event.position[0], rate)),
                second: ParameterValue::Float(if q_drag {
                    (0.1 * ((18.0_f32 / 0.1).ln() * event.position[1]).exp()).clamp(0.1, 18.0)
                } else {
                    event.position[1] * 48.0 - 24.0
                }),
            };
            view.device_panel.pending_values.insert(
                ParameterKey {
                    device,
                    parameter: first,
                },
                values.first,
            );
            view.device_panel.pending_values.insert(
                ParameterKey {
                    device,
                    parameter: second,
                },
                values.second,
            );
            let result = if event.phase == ResponseGraphPhase::Commit {
                view.finish_device_pair_gesture(values)
            } else {
                view.change_device_pair_gesture(values).map(|_| ())
            };
            if let Err(error) = result {
                view.device_panel.message = Some(error.into());
            }
        }
        ResponseGraphPhase::Cancel => {
            let _ = view.cancel_device_gesture();
            view.device_panel.pending_values.remove(&ParameterKey {
                device,
                parameter: first,
            });
            view.device_panel.pending_values.remove(&ParameterKey {
                device,
                parameter: second,
            });
        }
        _ => {}
    }
    cx.notify();
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn spectrum_intent_for_test(view: &EditorView, device: DeviceId) -> (bool, bool) {
    let panel = &view.device_panel.eq_panels[&device];
    (
        view.device_panel.eq_spectrum.enabled(device),
        panel.show_input,
    )
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn graph_for_test(view: &EditorView, device: DeviceId) -> Entity<ResponseGraph> {
    view.device_panel.eq_panels[&device].entity.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiding_input_retains_output_and_uses_identical_spectrum_coordinates() {
        use hex_audio::{
            spectrum::SpectrumFrame,
            spectrum_worker::{
                PublishedSpectra, PublishedSpectrum, SpectrumStamp, SpectrumTargetId,
            },
        };
        let frame = SpectrumFrame {
            sample_rate_hz: 48_000,
            bins_dbfs: [-36.0; hex_audio::spectrum::SPECTRUM_BANDS],
        };
        let pair = PublishedSpectra {
            input: Some(frame.clone()),
            output: PublishedSpectrum {
                stamp: SpectrumStamp {
                    target: SpectrumTargetId(1),
                    generation: 2,
                    sample_rate_hz: 48_000,
                    applied_revision: 3,
                },
                frame,
                captured_at: std::time::Instant::now(),
                processed_at: std::time::Instant::now(),
            },
        };
        let shown = spectrum_layers(Some(&pair), true, 20_000.0);
        assert_eq!(shown.0, shown.1, "equal signals align on the same axes");
        let hidden = spectrum_layers(Some(&pair), false, 20_000.0);
        assert_eq!(hidden.0, shown.0, "input visibility cannot hide output");
        assert!(hidden.1.is_none());
        assert_eq!(spectrum_layers(None, true, 20_000.0), (None, None));
    }

    #[test]
    fn creation_zones_and_vertical_domains_have_explicit_boundaries() {
        assert_eq!(creation_kind(0.0), eq::NodeKind::LowCut);
        assert_eq!(creation_kind(CUT_CREATION_EDGE), eq::NodeKind::LowCut);
        assert_eq!(creation_kind(CUT_CREATION_EDGE + 0.001), eq::NodeKind::Bell);
        assert_eq!(creation_kind(0.5), eq::NodeKind::Bell);
        assert_eq!(
            creation_kind(1.0 - CUT_CREATION_EDGE - 0.001),
            eq::NodeKind::Bell
        );
        assert_eq!(
            creation_kind(1.0 - CUT_CREATION_EDGE),
            eq::NodeKind::HighCut
        );
        assert_eq!(creation_kind(1.0), eq::NodeKind::HighCut);
        assert_eq!(q_from_y(-1.0), 0.1);
        assert!((q_from_y(2.0) - 18.0).abs() < 0.0001);
    }

    #[test]
    fn automation_page_navigation_and_point_endpoints_stay_in_q32_32() {
        assert_eq!(change_automation_page(0, -1), 0);
        assert_eq!(
            change_automation_page(MAX_AUTOMATION_PAGE, 1),
            MAX_AUTOMATION_PAGE
        );
        assert_eq!(change_automation_page(u32::MAX, 1), MAX_AUTOMATION_PAGE);
        assert_eq!(automation_point_beat(0, 0.0), 0);
        assert_eq!(automation_point_beat(0, 1.0), WINDOW_BEATS << 32);
        let last_start = automation_page_start(MAX_AUTOMATION_PAGE);
        let last_end = last_start.checked_add(WINDOW_BEATS << 32).unwrap();
        assert_eq!(automation_point_beat(u32::MAX, -1.0), last_start);
        assert_eq!(automation_point_beat(u32::MAX, 2.0), last_end);
        assert_eq!(
            automation_point_beat(MAX_AUTOMATION_PAGE, 0.5),
            last_start + (WINDOW_BEATS << 31)
        );
        assert!(last_end < u64::MAX);
    }
}
