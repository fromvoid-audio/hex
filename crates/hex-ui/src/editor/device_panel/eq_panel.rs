//! Sparse EQ editing. Opaque edits use atomic canonical state replacement;
//! scalar graph drags use the existing paired-parameter gesture protocol.
use super::*;
use gpui::{Entity, Subscription};
use hex_engine::eq::{self, EqState, Node, NodeId, NodeParameter};
use hex_lattice_contract::ParameterId;
use hex_session::device_gesture::{GestureTarget, ParameterPairValue};
use hex_widgets::{
    ResponseGraph, ResponseGraphCreationHint, ResponseGraphEvent, ResponseGraphModel,
    ResponseGraphNode, ResponseGraphPhase,
};

mod axes;
mod gestures;
mod scroll_queue;
pub(in crate::editor) use scroll_queue::after_worker_event as after_scroll_worker_event;
mod response;
mod spectrum;
pub(super) use spectrum::SpectrumSelection;
pub(in crate::editor) use spectrum::reconcile as reconcile_spectrum;

// Outer five percent of the inset plot: side cuts precede bottom notches.
// Existing node hits retain priority through the response graph owner.
const CUT_CREATION_EDGE: f32 = 0.05;

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

pub(super) struct Panel {
    entity: Entity<ResponseGraph>,
    _subscription: Subscription,
    _scroll_subscription: Subscription,
    generation: u64,
    authored_state: NativeDeviceState,
    selected: Option<NodeId>,
    armed: bool,
    show_input: bool,
    focus_subscription: Option<Subscription>,
    pair_drag: Option<gestures::PairDrag>,
    slope_scroll: Option<gestures::SlopeScroll>,
    scroll_queue: scroll_queue::Queue,
    scroll_blur_subscription: Option<Subscription>,
    bank: u8,
    creation: Option<Creation>,
    pending_creation: Option<PendingCreation>,
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

struct PendingCreation {
    receipt: crate::editor::pending_edit::Receipt,
    expected: NativeDeviceState,
    expected_variant: hex_project::DeviceVariant,
    next_id: u64,
    preview: EqState,
    state: NativeDeviceState,
    id: NodeId,
    previous: Option<NodeId>,
    bank: u8,
}

impl Creation {
    fn update(&mut self, position: [f32; 2]) {
        if let Some(node) = self.state.nodes.iter_mut().find(|node| node.id == self.id) {
            node.frequency_hz = frequency_from_x(position[0], self.sample_rate);
            if matches!(
                node.kind,
                eq::NodeKind::LowCut | eq::NodeKind::HighCut | eq::NodeKind::Notch
            ) {
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
        let scroll_subscription = cx.subscribe(
            &entity,
            move |view, _, event: &hex_widgets::ResponseGraphScrollEvent, cx| {
                gestures::scroll_event(view, cx, device, generation, *event);
            },
        );
        view.device_panel.eq_panels.insert(
            device,
            Panel {
                entity,
                _subscription: subscription,
                _scroll_subscription: scroll_subscription,
                generation,
                authored_state: canonical.clone(),
                selected: state.nodes.first().map(|n| n.id),
                armed: false,
                show_input: true,
                focus_subscription: None,
                pair_drag: None,
                slope_scroll: None,
                scroll_queue: Default::default(),
                scroll_blur_subscription: None,
                bank: 0,
                creation: None,
                pending_creation: None,
                response: None,
            },
        );
    }
    let next_id = view.state.as_ref().map(|s| s.project.next_identity());
    let active_variant = view.state.as_ref().and_then(|s| {
        s.project
            .device(device)
            .map(hex_project::Device::active_variant)
    });
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
    // Pending creation has its own ordinary-edit receipt. Queue idleness or an
    // unrelated canonical revision cannot retire the released node's geometry.
    if panel.pending_creation.as_ref().is_some_and(|pending| {
        &pending.expected != canonical
            || Some(pending.expected_variant) != active_variant
            || Some(pending.next_id) != next_id
    }) {
        panel.pending_creation = None;
    }
    if panel.creation.as_ref().is_some_and(|creation| {
        &creation.expected != canonical
            || Some(creation.next_id) != next_id
            || panel.selected != Some(creation.id)
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
        panel.authored_state = canonical.clone();
        // Events share this persistent mode with rendering. Repair only when
        // canonical state changes: a newly created node is selected optimistically
        // before its ID appears in the worker's published state.
        panel.selected = panel.selected.filter(|id| state.node(*id).is_ok());
    }
    let displayed = panel
        .creation
        .as_ref()
        .map(|creation| &creation.state)
        .or_else(|| {
            panel
                .pending_creation
                .as_ref()
                .map(|selection| &selection.preview)
        })
        .unwrap_or(&state)
        .clone();
    let selected = panel
        .pending_creation
        .as_ref()
        .filter(|pending| panel.selected == pending.previous && panel.bank == pending.bank)
        .map(|pending| pending.id)
        .or(panel.selected)
        .filter(|id| displayed.node(*id).is_ok());
    let show_creation_hint = creation_idle
        && displayed.nodes.len() < eq::MAX_NODES
        && panel.creation.is_none()
        && panel.pending_creation.is_none();
    let bank = panel.bank;
    let entity = panel.entity.clone();
    spectrum::sync_selected(view, device, owner);
    let model = response_model(
        view,
        device,
        &displayed,
        device_view,
        sample_rate,
        (bank, selected),
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
            graph.set_creation_hint_resolver(show_creation_hint.then_some(creation_hint), cx);
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
fn frequency_from_x(x: f32, rate: u32) -> f32 {
    let upper = (rate as f32 * 0.49).clamp(20.0, 20000.0);
    (20.0 * ((upper / 20.0).ln() * x.clamp(0.0, 1.0)).exp()).clamp(20.0, upper)
}
fn q_from_y(y: f32) -> f32 {
    (0.1 * ((18.0_f32 / 0.1).ln() * y.clamp(0.0, 1.0)).exp()).clamp(0.1, 18.0)
}
fn creation_kind([x, y]: [f32; 2]) -> eq::NodeKind {
    if x <= CUT_CREATION_EDGE {
        eq::NodeKind::LowCut
    } else if x >= 1.0 - CUT_CREATION_EDGE {
        eq::NodeKind::HighCut
    } else if y <= CUT_CREATION_EDGE {
        eq::NodeKind::Notch
    } else {
        eq::NodeKind::Bell
    }
}

/// Caller-owned shapes let the generic widget indicate the exact same action
/// as the canonical EQ creation resolver without depending on device/DSP types.
fn creation_hint(position: [f32; 2]) -> ResponseGraphCreationHint {
    let (id, points): (&'static str, &'static [[f32; 2]]) = match creation_kind(position) {
        eq::NodeKind::LowCut => (
            "low-cut",
            &[
                [0.0, 0.05],
                [0.25, 0.1],
                [0.5, 0.7],
                [0.75, 0.8],
                [1.0, 0.8],
            ],
        ),
        eq::NodeKind::HighCut => (
            "high-cut",
            &[
                [0.0, 0.8],
                [0.25, 0.8],
                [0.5, 0.7],
                [0.75, 0.1],
                [1.0, 0.05],
            ],
        ),
        eq::NodeKind::Notch => (
            "notch",
            &[
                [0.0, 0.8],
                [0.25, 0.8],
                [0.5, 0.05],
                [0.75, 0.8],
                [1.0, 0.8],
            ],
        ),
        _ => (
            "bell",
            &[
                [0.0, 0.3],
                [0.25, 0.35],
                [0.5, 0.95],
                [0.75, 0.35],
                [1.0, 0.3],
            ],
        ),
    };
    ResponseGraphCreationHint { id, points }
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
        node.kind = creation_kind(position);
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
                let reserved_ids =
                    u64::from(creation.state.next_node_id).saturating_sub(creation.next_id);
                if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                    panel.pending_creation = Some(PendingCreation {
                        receipt: crate::editor::pending_edit::Receipt::new(view.project_generation),
                        state: state.clone(),
                        expected: creation.expected.clone(),
                        expected_variant: creation.expected_variant,
                        next_id: creation.next_id,
                        preview: creation.state,
                        id: creation.id,
                        previous: creation.selected_before,
                        bank: panel.bank,
                    });
                }
                let admitted = view.enqueue_device_barrier(Request::Edit(
                    Command::ReplaceDeviceStateWithIdentityReservation {
                        device,
                        expected_state: creation.expected,
                        expected_variant: creation.expected_variant,
                        state,
                        expected_next_id: creation.next_id,
                        reserved_ids,
                    },
                ));
                if !admitted && let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                    panel.pending_creation = None;
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
    let (selected, bank) = (panel.selected, panel.bank);
    if event.phase == ResponseGraphPhase::Remove {
        if panel.creation.is_some()
            || panel.pending_creation.is_some()
            || current(view, device).as_ref() != Some(&panel.authored_state)
        {
            return;
        }
        if let Some(id) = selected.filter(|id| id.value() == event.node_id) {
            edit(view, cx, device, 0, move |state, _| state.remove(id));
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
        ResponseGraphPhase::Begin
        | ResponseGraphPhase::Change
        | ResponseGraphPhase::Commit
        | ResponseGraphPhase::Cancel => {
            gestures::paired_event(view, device, *node, event);
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
pub(super) fn graph_for_test(view: &EditorView, device: DeviceId) -> Entity<ResponseGraph> {
    view.device_panel.eq_panels[&device].entity.clone()
}

impl EditorView {
    pub(in crate::editor) fn pending_eq_creation_matches(
        &self,
        request: &Request,
    ) -> Option<DeviceId> {
        let Request::Edit(Command::ReplaceDeviceStateWithIdentityReservation {
            device,
            expected_state,
            expected_variant,
            state,
            expected_next_id,
            reserved_ids,
        }) = request
        else {
            return None;
        };
        let panel = self.device_panel.eq_panels.get(device)?;
        let pending = panel.pending_creation.as_ref()?;
        (pending.receipt.is_queued()
            && panel.generation == self.project_generation
            && &pending.expected == expected_state
            && pending.expected_variant == *expected_variant
            && &pending.state == state
            && pending.next_id == *expected_next_id
            && u64::from(pending.preview.next_node_id).saturating_sub(pending.next_id)
                == *reserved_ids)
            .then_some(*device)
    }

    pub(in crate::editor) fn eq_creation_submitted(&mut self, device: DeviceId) {
        if let Some(pending) = self
            .device_panel
            .eq_panels
            .get_mut(&device)
            .and_then(|panel| panel.pending_creation.as_mut())
        {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }

    pub(in crate::editor) fn retire_pending_eq_creation(&mut self, device: DeviceId) {
        if let Some(panel) = self.device_panel.eq_panels.get_mut(&device) {
            panel.pending_creation = None;
        }
    }

    pub(in crate::editor) fn eq_creation_request_complete(
        &mut self,
        event: &hex_session::service::Event,
    ) {
        use crate::editor::pending_edit::Disposition;
        for (device, panel) in &mut self.device_panel.eq_panels {
            let Some(pending) = panel.pending_creation.as_mut() else {
                continue;
            };
            let disposition = pending.receipt.observe(
                self.project_generation,
                event.project_replaced,
                event.request_complete,
            );
            if disposition == Disposition::Waiting {
                continue;
            }
            if disposition == Disposition::Terminal
                && event.error.is_none()
                && event
                    .state
                    .project
                    .device(*device)
                    .is_some_and(|entry| entry.state() == &pending.state)
                && panel.selected == pending.previous
                && panel.bank == pending.bank
            {
                panel.selected = Some(pending.id);
            }
            panel.pending_creation = None;
        }
    }

    pub(in crate::editor) fn retire_all_eq_creations(&mut self) -> bool {
        let mut retired = false;
        for panel in self.device_panel.eq_panels.values_mut() {
            retired |= panel.pending_creation.take().is_some();
        }
        retired
    }

    pub(in crate::editor) fn retire_abandoned_eq_creation(&mut self) -> bool {
        if !self.device_panel.eq_panels.values().any(|panel| {
            panel
                .pending_creation
                .as_ref()
                .is_some_and(|p| p.receipt.is_queued())
        }) {
            return false;
        }
        let abandoned: Vec<_> = self
            .device_panel
            .eq_panels
            .keys()
            .copied()
            .filter(|device| {
                let pending = self
                    .device_panel
                    .eq_panels
                    .get(device)
                    .and_then(|panel| panel.pending_creation.as_ref());
                pending.is_some_and(|pending| {
                    pending.receipt.is_queued()
                        && !self
                            .pending_requests
                            .iter()
                            .chain(self.device_gesture_barriers.iter())
                            .any(|request| {
                                self.pending_eq_creation_matches(request) == Some(*device)
                            })
                })
            })
            .collect();
        for device in &abandoned {
            self.retire_pending_eq_creation(*device);
        }
        !abandoned.is_empty()
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
        assert_eq!(creation_kind([0.0, 0.5]), eq::NodeKind::LowCut);
        assert_eq!(
            creation_kind([CUT_CREATION_EDGE, 0.5]),
            eq::NodeKind::LowCut
        );
        assert_eq!(
            creation_kind([CUT_CREATION_EDGE + 0.001, 0.5]),
            eq::NodeKind::Bell
        );
        assert_eq!(creation_kind([0.5, 0.5]), eq::NodeKind::Bell);
        assert_eq!(
            creation_kind([1.0 - CUT_CREATION_EDGE - 0.001, 0.5]),
            eq::NodeKind::Bell
        );
        assert_eq!(
            creation_kind([1.0 - CUT_CREATION_EDGE, 0.5]),
            eq::NodeKind::HighCut
        );
        assert_eq!(creation_kind([1.0, 0.5]), eq::NodeKind::HighCut);
        for x in [0.0, CUT_CREATION_EDGE] {
            assert_eq!(creation_kind([x, 0.0]), eq::NodeKind::LowCut);
        }
        for x in [1.0 - CUT_CREATION_EDGE, 1.0] {
            assert_eq!(creation_kind([x, 0.0]), eq::NodeKind::HighCut);
        }
        for y in [0.0, CUT_CREATION_EDGE] {
            assert_eq!(creation_kind([0.5, y]), eq::NodeKind::Notch);
            assert_eq!(creation_hint([0.5, y]).id, "notch");
        }
        assert_eq!(
            creation_kind([0.5, CUT_CREATION_EDGE + 0.001]),
            eq::NodeKind::Bell
        );
        assert_eq!(creation_hint([0.0, 0.0]).id, "low-cut");
        assert_eq!(creation_hint([1.0, 0.0]).id, "high-cut");
        assert_eq!(creation_hint([0.5, 0.5]).id, "bell");
        assert_eq!(q_from_y(-1.0), 0.1);
        assert!((q_from_y(2.0) - 18.0).abs() < 0.0001);
    }
}
