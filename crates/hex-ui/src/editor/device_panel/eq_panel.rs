//! Sparse EQ editing. Opaque edits use atomic canonical state replacement;
//! scalar graph drags use the existing paired-parameter gesture protocol.
//! Selection is view state: `selected` drives the footer controls and a
//! marquee `group` holds a multi-selection. A blank press creates a node;
//! shift-drag draws a marquee, and shift-cmd toggles its hits. Shift-press on
//! a node adds it to the group. Dragging a grouped
//! node moves the group as one parameter-snapshot gesture, and removal deletes
//! the group in one atomic replacement, so one Undo reverts either.
use super::*;
use gpui::{Entity, Subscription};
use hex_engine::eq::{self, EqState, Node, NodeId, NodeParameter};
use hex_lattice_contract::ParameterId;
use hex_session::device_gesture::{
    EqNodeCreationSnapshot, GestureKey, GesturePhase, GestureTarget, ParameterPairValue,
};
use hex_widgets::{
    ResponseGraph, ResponseGraphEvent, ResponseGraphMarqueeEvent, ResponseGraphModel,
    ResponseGraphNode, ResponseGraphPhase,
};

mod axes;
mod gestures;
mod group_move;
use group_move::{frequency_from_x, q_from_y};
mod node_geometry;
use node_geometry::{creation_kind, node_position};
mod scroll_queue;
pub(in crate::editor) use scroll_queue::after_worker_event as after_scroll_worker_event;
mod response;
mod sampling;
mod selection;
mod slope;
mod spectrum;
pub(super) use spectrum::SpectrumSelection;
pub(in crate::editor) use spectrum::reconcile as reconcile_spectrum;
pub(super) use spectrum::{
    is_eq as is_spectrum_eq, result as spectrum_result, sync_visible as sync_visible_spectra,
    toggle as spectrum_toggle,
};

// Commands and latched settings share text geometry, retaining distinct paint.
fn text_action_button(
    id: String,
    label: &'static str,
    tooltip: &'static str,
    role: theme::DeviceActionRole,
    selected: bool,
) -> Stateful<Div> {
    text_button(action_button(id, label, tooltip, role, selected))
}

fn text_segment_button(
    id: String,
    label: &'static str,
    tooltip: &'static str,
    role: theme::DeviceActionRole,
    selected: bool,
) -> Stateful<Div> {
    text_button(segment_button(id, label, tooltip, role, selected))
}

fn text_button(surface: Stateful<Div>) -> Stateful<Div> {
    surface
        .w_auto()
        .min_w(px(hex_widgets::DEVICE_HEADER_ACTION_SIZE))
        .px(px(3.0))
        .text_size(px(8.0))
        .line_height(px(10.0))
        .whitespace_nowrap()
}

pub(super) struct Panel {
    entity: Entity<ResponseGraph>,
    _subscription: Subscription,
    _scroll_subscription: Subscription,
    _marquee_subscription: Subscription,
    generation: u64,
    authored_state: NativeDeviceState,
    selected: Option<NodeId>,
    /// Marquee multi-selection (two or more nodes, including `selected`).
    group: Vec<NodeId>,
    /// A stationary press on a grouped node collapses the group on release.
    collapse: Option<NodeId>,
    group_drag: Option<gestures::GroupDrag>,
    armed: bool,
    show_input: bool,
    focus_subscription: Option<Subscription>,
    pair_drag: Option<gestures::PairDrag>,
    slope_scroll: Option<gestures::SlopeScroll>,
    scroll_queue: scroll_queue::Queue,
    scroll_blur_subscription: Option<Subscription>,
    bank: u8,
    creation: Option<Creation>,
    cancel_creation_capture: bool,
    response: Option<(NativeDeviceState, u32, response::Curves)>,
}

struct Creation {
    key: GestureKey,
    terminal: Option<(GesturePhase, u64)>,
    expected: NativeDeviceState,
    expected_variant: hex_project::DeviceVariant,
    state: EqState,
    next_id: u64,
    id: NodeId,
    selected_before: Option<NodeId>,
    sample_rate: u32,
}

impl Creation {
    fn update(&mut self, position: [f32; 2]) {
        if let Some(node) = self.state.nodes.iter_mut().find(|node| node.id == self.id) {
            update_creation_node(node, position, self.sample_rate);
        }
    }
    fn snapshot(&self) -> Result<EqNodeCreationSnapshot, &'static str> {
        let node = *self
            .state
            .node(self.id)
            .map_err(|_| "eq creation node is missing")?;
        EqNodeCreationSnapshot::new(node, self.expected_variant, self.next_id)
    }
}
fn update_creation_node(node: &mut Node, position: [f32; 2], sample_rate: u32) {
    node.frequency_hz = frequency_from_x(position[0], sample_rate);
    if matches!(
        node.kind,
        eq::NodeKind::LowCut | eq::NodeKind::HighCut | eq::NodeKind::Notch
    ) {
        node.q = q_from_y(position[1]);
    } else {
        node.gain_db = position[1] * 48.0 - 24.0;
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
            graph.set_circular_nodes(true, cx);
            graph.set_editable(true, cx);
            graph.set_tab_navigation(true, cx);
            graph.set_creation_enabled(true);
            graph.set_marquee_enabled(true, cx);
            graph.set_backspace_removal(true);
            graph
        });
        let generation = view.project_generation;
        let subscription = cx.subscribe(&entity, move |view, _, event: &ResponseGraphEvent, cx| {
            handle_event(view, cx, device, generation, *event);
        });
        let scroll_subscription = cx.subscribe(
            &entity,
            move |view, _, event: &hex_widgets::ResponseGraphScrollEvent, cx| {
                gestures::scroll_event(view, cx, device, generation, *event);
            },
        );
        let marquee_subscription = cx.subscribe(
            &entity,
            move |view, _, event: &ResponseGraphMarqueeEvent, cx| {
                marquee_event(view, cx, device, generation, event);
            },
        );
        view.device_panel.eq_panels.insert(
            device,
            Panel {
                entity,
                _subscription: subscription,
                _scroll_subscription: scroll_subscription,
                _marquee_subscription: marquee_subscription,
                generation,
                authored_state: canonical.clone(),
                selected: state.nodes.first().map(|n| n.id),
                group: Vec::new(),
                collapse: None,
                group_drag: None,
                armed: false,
                show_input: true,
                focus_subscription: None,
                pair_drag: None,
                slope_scroll: None,
                scroll_queue: Default::default(),
                scroll_blur_subscription: None,
                bank: 0,
                creation: None,
                cancel_creation_capture: false,
                response: None,
            },
        );
    }
    reconcile_creation(view, device, canonical);
    let creation_idle = !view.opening_project
        && !view.close_prompt_open
        && view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.device_gesture_barriers.is_empty()
        && view.device_gesture.snapshot().is_none()
        && !view.device_panel.has_pending_draft(cx);
    let Some(panel) = view.device_panel.eq_panels.get_mut(&device) else {
        return div().id("eq-unavailable");
    };
    if panel.cancel_creation_capture {
        panel.cancel_creation_capture = false;
        if panel
            .creation
            .as_ref()
            .is_none_or(|creation| creation.terminal.is_some())
            && panel.pair_drag.is_none()
            && panel.slope_scroll.is_none()
        {
            panel.entity.update(cx, |graph, cx| graph.cancel(cx));
        }
    }
    // Canonical replacement retires held drafts through the keyed worker event.
    // A released draft remains frozen until its exact terminal is installed.
    if &panel.authored_state != canonical {
        panel.authored_state = canonical.clone();
        // Events share this persistent mode with rendering. Repair only when
        // canonical state changes: a newly created node is selected optimistically
        // before its ID appears in the worker's published state.
        panel.selected = panel.selected.filter(|id| state.node(*id).is_ok());
        panel.group.retain(|id| state.node(*id).is_ok());
        if panel.group.len() < 2 {
            panel.group.clear();
        }
    }
    let displayed = panel
        .creation
        .as_ref()
        .map(|creation| &creation.state)
        .unwrap_or(&state)
        .clone();
    let selected = panel.selected.filter(|id| displayed.node(*id).is_ok());
    let show_creation_hint =
        creation_idle && displayed.nodes.len() < eq::MAX_NODES && panel.creation.is_none();
    let bank = panel.bank;
    let group = panel.group.clone();
    let entity = panel.entity.clone();
    let model = response_model(
        view,
        device,
        &displayed,
        device_view,
        sample_rate,
        (bank, selected, &group),
        owner,
    );
    let scroll_nodes = gestures::scroll_nodes(view, device, &displayed, cx);
    let relative_option_y_nodes = displayed
        .nodes
        .iter()
        .filter(|node| node.kind == eq::NodeKind::Bell)
        .map(|node| node.id.value())
        .collect();
    if let Some(model) = model {
        entity.update(cx, |graph, cx| {
            graph.set_creation_drag_enabled(true);
            graph.set_creation_hint_resolver(
                show_creation_hint.then_some(node_geometry::creation_hint),
                cx,
            );
            let _ = graph.set_axes(Some(axes::frequency_axes(sample_rate)), cx);
            let _ = graph.set_analyzer_style(theme::device_analyzer_style(), cx);
            let _ = graph.set_scroll_nodes(scroll_nodes, cx);
            let _ = graph.set_relative_option_y_nodes(relative_option_y_nodes, cx);
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
            text_segment_button(
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
                        p.group.clear();
                    }
                }
                cx.notify();
            })),
        );
    }
    toolbar = toolbar.child(theme::text(format!("{} / 64", state.nodes.len())));
    toolbar = {
        let control = spectrum::toggle(view, cx, device, owner, eq::EQ_TYPE_ID);
        toolbar.child(control)
    };
    let show_input = view
        .device_panel
        .eq_panels
        .get(&device)
        .is_some_and(|panel| panel.show_input);
    toolbar = toolbar.child(
        text_segment_button(
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
        text_segment_button(
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
        let removed = if group.len() > 1 { group } else { vec![id] };
        actions = actions.child(
            text_action_button(
                format!("eq-remove-{}", device.value()),
                "remove",
                "remove selected nodes and their automation",
                theme::DeviceActionRole::Normal,
                true,
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                remove_nodes(view, cx, device, removed.clone());
            })),
        );
    } else {
        bottom = bottom.child(theme::text(
            "click to add a node · option-drag to place and shape · shift-drag to select",
        ));
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
fn response_model(
    view: &mut EditorView,
    device: DeviceId,
    state: &EqState,
    device_view: &NativeDeviceView,
    rate: u32,
    selection: (u8, Option<NodeId>, &[NodeId]),
    owner: hex_engine::ProcessorChainOwner,
) -> Option<ResponseGraphModel> {
    let (bank, selected, group) = selection;
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
            position: node_position(n, rate),
            enabled: n.enabled,
            selected: selected == Some(n.id) || group.contains(&n.id),
        })
        .collect();
    let identity = hex_engine::PostNodeIdentity::new(owner, device.value(), eq::EQ_TYPE_ID);
    let spectra = spectrum::result(view, identity);
    let show_input = view
        .device_panel
        .eq_panels
        .get(&device)
        .is_some_and(|panel| panel.show_input);
    let (analyzer, input_analyzer, analyzer_peak) = spectrum_layers(spectra, show_input, upper);
    Some(ResponseGraphModel {
        primary,
        secondary,
        analyzer,
        input_analyzer,
        analyzer_peak,
        nodes,
    })
}
type SpectrumCurve = Vec<[f32; 2]>;

/// Output level, optional input level and output held-peak curves.
fn spectrum_layers(
    spectra: Option<&hex_audio::spectrum_worker::PublishedSpectra>,
    show_input: bool,
    upper: f32,
) -> (
    Option<SpectrumCurve>,
    Option<SpectrumCurve>,
    Option<SpectrumCurve>,
) {
    let x = |hz| axes::x_with_upper(hz, upper);
    let output = spectra.map(|pair| super::spectrum_display::curves(&pair.output.frame, x));
    let input = spectra
        .and_then(|pair| pair.input.as_ref())
        .filter(|_| show_input)
        .map(|frame| super::spectrum_display::curves(frame, x).level);
    match output {
        Some(curves) => (Some(curves.level), input, Some(curves.peak)),
        None => (None, input, None),
    }
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
    let owner = owner.or_else(|| {
        state
            .project
            .main_devices()
            .iter()
            .any(|d| d.id() == device)
            .then_some(hex_engine::ProcessorChainOwner::Main)
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
/// Removes `ids` in one atomic replacement, so one Undo restores them all.
fn remove_nodes(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    ids: Vec<NodeId>,
) {
    edit(view, cx, device, 0, move |state, _| {
        ids.iter().try_for_each(|id| state.remove(*id))
    });
}
fn marquee_event(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    generation: u64,
    event: &ResponseGraphMarqueeEvent,
) {
    if generation != view.project_generation || !finalize_controls(view, cx) {
        return;
    }
    let Some(panel) = view.device_panel.eq_panels.get_mut(&device) else {
        return;
    };
    selection::apply_marquee(&mut panel.selected, &mut panel.group, &event.selected);
    panel.collapse = None;
    cx.notify();
}
// Input cancellation is separate from admitted preview custody. The last
// geometry stays with its key until the terminal/canonical installation event.
fn reconcile_creation(view: &mut EditorView, device: DeviceId, canonical: &NativeDeviceState) {
    let Some((key, valid, finishing)) =
        view.device_panel.eq_panels.get(&device).and_then(|panel| {
            panel.creation.as_ref().map(|creation| {
                (
                    creation.key,
                    &creation.expected == canonical
                        && panel.selected == Some(creation.id)
                        && view.state.as_ref().is_some_and(|state| {
                            state.project.next_identity() == creation.next_id
                                && state.project.device(device).is_some_and(|entry| {
                                    entry.active_variant() == creation.expected_variant
                                })
                        })
                        && creation
                            .state
                            .node(creation.id)
                            .is_ok_and(|node| node.bank == panel.bank),
                    creation.terminal.is_some(),
                )
            })
        })
    else {
        return;
    };
    let owns_key = view
        .device_gesture
        .snapshot()
        .is_some_and(|snapshot| snapshot.key == key);
    if owns_key && finishing {
        return;
    }
    if owns_key && valid && view.device_gesture.terminal().is_none() {
        return;
    }
    if owns_key && !valid && !view.device_gesture.has_pending_terminal() {
        let _ = view.cancel_device_gesture();
    }
    let terminal = view.device_gesture.terminal();
    if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
        panel.cancel_creation_capture = true;
        if owns_key {
            if let Some(creation) = panel.creation.as_mut() {
                creation.terminal = terminal;
            }
        } else if let Some(creation) = panel.creation.take()
            && panel.selected == Some(creation.id)
        {
            panel.selected = creation.selected_before;
        }
    }
}

fn begin_creation(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    position: [f32; 2],
    bank: u8,
) {
    if view.device_gesture.is_recovery_pending() && view.device_gesture.snapshot().is_some_and(|snapshot| matches!(snapshot.target, GestureTarget::EqNodeCreation { device: target } if target == device)) {
        let _ = view.cancel_eq_creation();
        view.device_panel.message = Some("previous eq edit is canceling · retry after it finishes".into());
        cx.notify();
        return;
    }
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
    let result: Result<Creation, String> = (|| {
        let mut state = EqState::decode(&expected).map_err(|e| e.to_string())?;
        let id =
            NodeId::new(next_id.max(u64::from(state.next_node_id))).map_err(|e| e.to_string())?;
        let mut node = Node::new(id, frequency_from_x(position[0], sample_rate), 0.0, bank)
            .map_err(|e| e.to_string())?;
        node.kind = creation_kind(position);
        update_creation_node(&mut node, position, sample_rate);
        state.add(node).map_err(|e| e.to_string())?;
        state.encode().map_err(|e| e.to_string())?;
        let snapshot =
            EqNodeCreationSnapshot::new(node, expected_variant, next_id).map_err(str::to_owned)?;
        let key = view
            .device_gesture
            .begin_eq_creation(view.project_generation, device, snapshot)
            .map_err(str::to_owned)?;
        if let Err(error) = view.device_gesture.change_eq_creation(snapshot) {
            let _ = view.cancel_device_gesture();
            return Err(error.to_owned());
        }
        Ok(Creation {
            key,
            terminal: None,
            expected,
            expected_variant,
            state,
            next_id,
            id,
            selected_before: None,
            sample_rate,
        })
    })();
    match result {
        Ok(mut creation) => {
            if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                creation.selected_before = panel.selected;
                panel.selected = Some(creation.id);
                panel.cancel_creation_capture = false;
                panel.creation = Some(creation);
            }
        }
        Err(error) => {
            view.device_panel.message = Some(format!(
                "eq edit refused · {error} · limit: 64 nodes; identity space is finite"
            ))
        }
    }
    cx.notify();
}

// Node ID zero belongs to this whole pointer stream. It can never address an
// ordinary node drag after release, cancellation, publication or ID reuse.
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
    if creation.terminal.is_some() {
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.creation = Some(creation);
        }
        return;
    }
    let owns_key = view.device_gesture.snapshot().is_some_and(|snapshot| snapshot.key == creation.key
        && matches!(snapshot.target, GestureTarget::EqNodeCreation { device: target } if target == device));
    let valid = owns_key
        && current(view, device).as_ref() == Some(&creation.expected)
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
                    && creation
                        .state
                        .node(creation.id)
                        .is_ok_and(|node| node.bank == panel.bank)
            });
    if !valid || event.phase == ResponseGraphPhase::Cancel {
        if owns_key && !view.device_gesture.has_pending_terminal() {
            let _ = view.cancel_device_gesture();
        }
        creation.terminal = view.device_gesture.terminal();
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            if panel.selected == Some(creation.id) {
                panel.selected = creation.selected_before;
            }
            if owns_key && creation.terminal.is_some() {
                panel.creation = Some(creation);
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
        if !view.device_gesture.has_pending_terminal() {
            creation.update(event.position);
            let result = creation
                .snapshot()
                .and_then(|snapshot| view.device_gesture.finish_eq_creation(snapshot));
            if let Err(error) = result {
                view.device_panel.message = Some(error.into());
            }
        }
        creation.terminal = view.device_gesture.terminal();
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.creation = Some(creation);
        }
    } else {
        if event.phase == ResponseGraphPhase::Change && !view.device_gesture.has_pending_terminal()
        {
            creation.update(event.position);
            let result = creation
                .snapshot()
                .and_then(|snapshot| view.device_gesture.change_eq_creation(snapshot));
            if let Err(error) = result {
                view.device_panel.message = Some(error.into());
            }
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
    let (selected, bank) = (panel.selected, panel.bank);
    let group = panel.group.clone();
    if event.phase == ResponseGraphPhase::Remove {
        if view.selection_delete_blocked()
            || panel.creation.is_some()
            || current(view, device).as_ref() != Some(&panel.authored_state)
        {
            return;
        }
        if group.len() > 1 {
            remove_nodes(view, cx, device, group);
        } else if let Some(id) = selected.filter(|id| id.value() == event.node_id) {
            remove_nodes(view, cx, device, vec![id]);
        }
        return;
    }
    if event.node_id == 0 {
        if event.phase == ResponseGraphPhase::Create {
            begin_creation(view, cx, device, event.position, bank);
        } else {
            creation_event(view, cx, device, event);
        }
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
            if let Some(p) = view.device_panel.eq_panels.get_mut(&device)
                && p.collapse.take() == Some(id)
            {
                p.group.clear();
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
                    selection::select_node(
                        &mut p.selected,
                        &mut p.group,
                        &mut p.collapse,
                        id,
                        event.modifiers.shift,
                    );
                }
            }
        }
        ResponseGraphPhase::Begin => {
            if let Some(p) = view.device_panel.eq_panels.get_mut(&device) {
                p.collapse = None;
            }
            if group.len() > 1 && group.contains(&id) {
                gestures::group_event(view, device, &group, event);
            } else {
                gestures::paired_event(view, device, *node, event);
            }
        }
        ResponseGraphPhase::Change | ResponseGraphPhase::Commit | ResponseGraphPhase::Cancel => {
            if view
                .device_panel
                .eq_panels
                .get(&device)
                .is_some_and(|p| p.group_drag.is_some())
            {
                gestures::group_event(view, device, &group, event);
            } else {
                gestures::paired_event(view, device, *node, event);
            }
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
pub(super) fn queued_scrolls_for_test(view: &EditorView, device: DeviceId) -> (usize, bool) {
    scroll_queue::counts(view, device)
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn selection_for_test(
    view: &EditorView,
    device: DeviceId,
) -> (Option<NodeId>, Vec<NodeId>) {
    let panel = &view.device_panel.eq_panels[&device];
    (panel.selected, panel.group.clone())
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn graph_for_test(view: &EditorView, device: DeviceId) -> Entity<ResponseGraph> {
    view.device_panel.eq_panels[&device].entity.clone()
}

impl EditorView {
    pub(in crate::editor) fn cancel_eq_creation(&mut self) -> bool {
        let Some(snapshot) = self.device_gesture.snapshot() else {
            return false;
        };
        let GestureTarget::EqNodeCreation { device } = snapshot.target else {
            return false;
        };
        if !self.device_gesture.has_pending_terminal() {
            let _ = self.cancel_device_gesture();
        }
        let terminal = self.device_gesture.terminal();
        if let Some(panel) = self.device_panel.eq_panels.get_mut(&device)
            && let Some(creation) = panel
                .creation
                .as_mut()
                .filter(|creation| creation.key == snapshot.key)
        {
            creation.terminal = terminal;
            panel.cancel_creation_capture = true;
            if panel.selected == Some(creation.id) {
                panel.selected = creation.selected_before;
            }
        }
        true
    }

    pub(in crate::editor) fn eq_creation_request_complete(
        &mut self,
        event: &hex_session::service::Event,
    ) {
        let terminal = self.device_gesture.terminal();
        let client_key = self.device_gesture.snapshot().map(|snapshot| snapshot.key);
        for (device, panel) in &mut self.device_panel.eq_panels {
            let Some(mut creation) = panel.creation.take() else {
                continue;
            };
            if event.project_replaced {
                panel.cancel_creation_capture = true;
                if panel.selected == Some(creation.id) {
                    panel.selected = creation.selected_before;
                }
                continue;
            }
            let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == creation.key)
            else {
                panel.creation = Some(creation);
                continue;
            };
            let expected = if client_key == Some(creation.key) {
                terminal.or(creation.terminal)
            } else {
                creation.terminal
            };
            let refused_begin = ack.phase == GesturePhase::Begin && !ack.accepted;
            let replaced_source = event
                .state
                .project
                .device(*device)
                .is_none_or(|entry| entry.state() != &creation.expected)
                || event.state.project.next_identity() != creation.next_id;
            let exact_terminal = expected == Some((ack.phase, ack.sequence));
            let retires_creation = refused_begin
                || exact_terminal
                || (replaced_source && ack.phase == GesturePhase::Cancel);
            if !retires_creation {
                panel.creation = Some(creation);
                continue;
            }
            panel.cancel_creation_capture = true;
            if ack.preview_retained {
                let accepted = event
                    .device_preview
                    .as_ref()
                    .map(|preview| &preview.state.project)
                    .unwrap_or(&event.state.project);
                if let Some(entry) = accepted.device(*device)
                    && let Ok(state) = EqState::decode(entry.state())
                {
                    creation.state = state;
                }
                if panel.selected == Some(creation.id) && creation.state.node(creation.id).is_err()
                {
                    panel.selected = creation.selected_before;
                }
                creation.terminal = Some((ack.phase, ack.sequence));
                panel.creation = Some(creation);
                continue;
            }
            let committed = ack.accepted
                && ack.phase == GesturePhase::Commit
                && event
                    .state
                    .project
                    .device(*device)
                    .and_then(|entry| EqState::decode(entry.state()).ok())
                    .is_some_and(|state| state == creation.state);
            if committed {
                if panel.bank
                    == creation
                        .state
                        .node(creation.id)
                        .map_or(panel.bank, |node| node.bank)
                    && (panel.selected == creation.selected_before
                        || panel.selected == Some(creation.id))
                {
                    panel.selected = Some(creation.id);
                }
            } else if panel.selected == Some(creation.id) {
                panel.selected = creation.selected_before;
            }
        }
    }
    pub(in crate::editor) fn retire_all_eq_creations(&mut self) -> bool {
        let mut retired = false;
        for panel in self.device_panel.eq_panels.values_mut() {
            if let Some(creation) = panel.creation.take() {
                panel.cancel_creation_capture = true;
                if panel.selected == Some(creation.id) {
                    panel.selected = creation.selected_before;
                }
                retired = true;
            }
        }
        retired
    }
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
            peak_dbfs: [-36.0; hex_audio::spectrum::SPECTRUM_BANDS],
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
        assert_eq!(hidden.2, shown.2, "the held peak follows the output");
        assert_eq!(spectrum_layers(None, true, 20_000.0), (None, None, None));
    }

    #[test]
    fn creation_zones_and_vertical_domains_have_explicit_boundaries() {
        assert_eq!(creation_kind([0.0, 0.5]), eq::NodeKind::LowCut);
        assert_eq!(
            creation_kind([node_geometry::CUT_CREATION_EDGE, 0.5]),
            eq::NodeKind::LowCut
        );
        assert_eq!(
            creation_kind([node_geometry::CUT_CREATION_EDGE + 0.001, 0.5]),
            eq::NodeKind::Bell
        );
        assert_eq!(creation_kind([0.5, 0.5]), eq::NodeKind::Bell);
        assert_eq!(
            creation_kind([1.0 - node_geometry::CUT_CREATION_EDGE - 0.001, 0.5]),
            eq::NodeKind::Bell
        );
        assert_eq!(
            creation_kind([1.0 - node_geometry::CUT_CREATION_EDGE, 0.5]),
            eq::NodeKind::HighCut
        );
        assert_eq!(creation_kind([1.0, 0.5]), eq::NodeKind::HighCut);
        for x in [0.0, node_geometry::CUT_CREATION_EDGE] {
            assert_eq!(creation_kind([x, 0.0]), eq::NodeKind::LowCut);
        }
        for x in [1.0 - node_geometry::CUT_CREATION_EDGE, 1.0] {
            assert_eq!(creation_kind([x, 0.0]), eq::NodeKind::HighCut);
        }
        for y in [0.0, node_geometry::CUT_CREATION_EDGE] {
            assert_eq!(creation_kind([0.5, y]), eq::NodeKind::Notch);
            assert_eq!(node_geometry::creation_hint([0.5, y]).id, "notch");
        }
        assert_eq!(
            creation_kind([0.5, node_geometry::CUT_CREATION_EDGE + 0.001]),
            eq::NodeKind::Bell
        );
        assert_eq!(node_geometry::creation_hint([0.0, 0.0]).id, "low-cut");
        assert_eq!(node_geometry::creation_hint([1.0, 0.0]).id, "high-cut");
        assert_eq!(node_geometry::creation_hint([0.5, 0.5]).id, "bell");
        assert_eq!(q_from_y(-1.0), 0.1);
        assert!((q_from_y(2.0) - 18.0).abs() < 0.0001);
    }
}
