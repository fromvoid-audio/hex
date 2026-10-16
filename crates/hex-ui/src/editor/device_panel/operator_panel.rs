//! Compact Operator composition backed by the shared device control entities.
use super::{
    ControlPresentation, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, finalize_controls, parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{NativeDeviceClass, NativeDeviceParameterView, NativeDeviceView};
use hex_lattice_contract::{ParameterId, ParameterKind, ParameterValue};
use hex_project::DeviceId;

pub(super) mod context;
pub(super) mod envelope;
pub(super) mod preview;

pub(super) const OPERATOR_FRAME_WIDTH: f32 = crate::device_composition::VECTOR_WIDTH;
// Three shared tiles plus fixed selector, badge, gaps and retained breathing room.
const SHELL_WIDTH: f32 = crate::device_composition::VECTOR_SHELL_WIDTH;
const FIELD_WIDTH: f32 = 46.0;

gpui::actions!(hex_operator, [ActivateRouting, ActivateSection]);

pub(in crate::editor) fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        gpui::KeyBinding::new("enter", ActivateSection, Some("hex_operator_section")),
        gpui::KeyBinding::new("space", ActivateSection, Some("hex_operator_section")),
        gpui::KeyBinding::new("enter", ActivateRouting, Some("hex_operator_routing")),
        gpui::KeyBinding::new("space", ActivateRouting, Some("hex_operator_routing")),
    ]);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct OperatorPanelState {
    pub(super) selected_operator: usize,
    pub(super) page: OperatorPage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum OperatorPage {
    Oscillator,
    #[default]
    Envelope,
    Routing,
    Lfo,
    Filter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum NavigationTarget {
    Oscillator(usize),
    Page(OperatorPage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Section {
    Oscillator(usize),
    Preview(&'static str),
    Detail,
}

fn navigation_focus(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    target: NavigationTarget,
) -> gpui::FocusHandle {
    view.device_panel
        .operator_navigation_focus
        .entry((device, target))
        .or_insert_with(|| cx.focus_handle().tab_index(0).tab_stop(true))
        .clone()
}

const fn id(value: u32) -> ParameterId {
    ParameterId::from_static(value)
}

pub(super) fn shell_operator_for_numeric_parameter(parameter: ParameterId) -> Option<usize> {
    let value = parameter.value();
    match value {
        1..=12 if value % 3 != 0 => Some(((value - 1) / 3) as usize),
        63..=73 if (value - 63) % 3 != 2 => Some(((value - 63) / 3) as usize),
        _ => None,
    }
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let state = *view.device_panel.operator_pages.entry(device).or_default();
    let owner = context::Owner::capture(view, device);
    let parameters = &device_view.parameters;
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Instrument,
        type_id: device_view.type_id,
    };
    let mut shell = hex_widgets::device_rail(super::device_layout_style())
        .id(ElementId::Name(
            format!("operator-shell-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-shell-{}", device.value()))
        .w(px(SHELL_WIDTH))
        .flex_shrink_0()
        .h_full()
        .pr(px(4.0));
    for operator in (0..4).rev() {
        shell = shell.child(operator_row(
            view,
            cx,
            device,
            identity,
            parameters,
            operator,
            state.selected_operator == operator,
        ));
    }

    envelope::retire_hidden(
        view,
        cx,
        device,
        (state.page == OperatorPage::Envelope).then_some(state.selected_operator),
    );
    let detail = match state.page {
        OperatorPage::Oscillator => oscillator_detail(
            view,
            cx,
            device,
            identity,
            parameters,
            state.selected_operator,
        ),
        OperatorPage::Envelope => envelope_detail(
            view,
            cx,
            device,
            identity,
            parameters,
            state.selected_operator,
        ),
        OperatorPage::Routing => routing_detail(view, cx, device, identity, parameters),
        OperatorPage::Lfo => lfo_detail(view, cx, device, identity, parameters),
        OperatorPage::Filter => filter_detail(view, cx, device, identity, parameters),
    };
    let (detail_focus, _) = context::section_focus(view, cx, device, Section::Detail);
    let detail =
        crate::device_composition::tabbed_detail(detail, page_tabs(view, cx, device, owner, state))
            .on_children_prepainted(context::section_listener(view, cx, device, Section::Detail))
            .id(ElementId::Name(
                format!("operator-detail-{}", device.value()).into(),
            ))
            .debug_selector(move || format!("operator-detail-{}", device.value()))
            .track_focus(&detail_focus);
    let preview = preview::render(view, cx, device, device_view);
    crate::device_composition::three_pane(shell, detail, preview)
        .tab_group()
        .on_children_prepainted(context::focus_listener(view, cx, device))
        .id(ElementId::Name(
            format!("operator-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-body-{}", device.value()))
        .on_key_down(
            cx.listener(move |view, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab"
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt
                    && !event.keystroke.modifiers.platform
                    && owner.is_some_and(|owner| owner.is_current(view))
                {
                    if preview::finalize_controls(view, cx, device) && finalize_controls(view, cx) {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev();
                        } else {
                            window.focus_next();
                        }
                    }
                    cx.stop_propagation();
                }
            }),
        )
}

fn page_tabs(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    owner: Option<context::Owner>,
    state: OperatorPanelState,
) -> Div {
    let envelope_focus = navigation_focus(
        view,
        cx,
        device,
        NavigationTarget::Page(OperatorPage::Envelope),
    );
    let lfo_focus = navigation_focus(view, cx, device, NavigationTarget::Page(OperatorPage::Lfo));
    let oscillator_focus = navigation_focus(
        view,
        cx,
        device,
        NavigationTarget::Page(OperatorPage::Oscillator),
    );
    div()
        .flex()
        .w_full()
        .h(px(16.0))
        .items_center()
        .justify_between()
        .child(page_tab(
            cx,
            device,
            owner,
            OperatorPage::Envelope,
            "envelope",
            state.page,
            envelope_focus,
        ))
        .child(page_tab(
            cx,
            device,
            owner,
            OperatorPage::Lfo,
            "lfo",
            state.page,
            lfo_focus,
        ))
        .child(page_tab(
            cx,
            device,
            owner,
            OperatorPage::Oscillator,
            "oscillator",
            state.page,
            oscillator_focus,
        ))
}

fn operator_row(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
    operator: usize,
    selected: bool,
) -> Stateful<Div> {
    let base = operator as u32 * 3 + 1;
    let fixed_id = 62 + operator as u32 * 3;
    let fixed_value = value(view, device, parameters, id(fixed_id));
    let fixed = matches!(fixed_value, Some(ParameterValue::Bool(true)));
    let primary_id = if fixed { fixed_id + 1 } else { base };
    let primary = shell_control(
        view,
        cx,
        device,
        identity,
        parameters,
        operator,
        id(primary_id),
        ControlPresentation::MicroDial,
        Some(if fixed { "freq" } else { "coarse" }),
    );
    let fine = preview::control(
        view,
        cx,
        device,
        [
            preview::Field::FineA,
            preview::Field::FineB,
            preview::Field::FineC,
            preview::Field::FineD,
        ][operator],
    );
    let level = shell_control(
        view,
        cx,
        device,
        identity,
        parameters,
        operator,
        id(base + 1),
        ControlPresentation::MicroDial,
        Some("level"),
    );
    let (section_focus, focused) =
        context::section_focus(view, cx, device, Section::Oscillator(operator));
    let mut row = hex_widgets::device_grouped_row(super::device_layout_style(), focused)
        .on_children_prepainted(context::section_listener(
            view,
            cx,
            device,
            Section::Oscillator(operator),
        ))
        .id(ElementId::Name(
            format!("operator-row-{}-{operator}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-row-{}-{operator}", device.value()))
        .w_full()
        .h(px(40.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .pr(px(14.0))
        .relative()
        .track_focus(&section_focus)
        .child(primary);
    row = row.child(fine);
    let fixed_control = shell_control(
        view,
        cx,
        device,
        identity,
        parameters,
        operator,
        id(fixed_id),
        ControlPresentation::CompactButton,
        Some(""),
    );
    row = row.child(
        div()
            .w(px(30.0))
            .h(px(40.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(5.0))
            .child(section_title("fixed"))
            .child(fixed_control),
    );
    row = row.child(level).child(
        operator_label(view, cx, device, parameters, operator, selected)
            .absolute()
            .right(px(4.0))
            .top(px(14.0)),
    );
    row
}

fn operator_label(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    parameters: &[NativeDeviceParameterView],
    operator: usize,
    selected: bool,
) -> Stateful<Div> {
    let owner = context::Owner::capture(view, device);
    let enabled = matches!(
        value(view, device, parameters, id(513 + operator as u32)),
        Some(ParameterValue::Bool(true))
    );
    let focus = navigation_focus(view, cx, device, NavigationTarget::Oscillator(operator));
    hex_widgets::device_badge(
        theme::text(["a", "b", "c", "d"][operator]),
        theme::device_identity_style(operator, enabled, selected),
    )
    .id(ElementId::Name(
        format!("operator-select-{}-{operator}", device.value()).into(),
    ))
    .debug_selector(move || format!("operator-select-{}-{operator}", device.value()))
    .track_focus(&focus)
    .key_context("hex_operator_section")
    .cursor(gpui::CursorStyle::PointingHand)
    .on_click(cx.listener(move |view, _, window, cx| {
        if owner.is_some_and(|owner| owner.is_current(view))
            && finalize_controls(view, cx)
            && toggle_enable(view, device, operator)
        {
            focus.focus(window);
            let state = view.device_panel.operator_pages.entry(device).or_default();
            state.selected_operator = operator;
            if state.page != OperatorPage::Envelope {
                state.page = OperatorPage::Oscillator;
            }
            cx.notify();
        }
    }))
    .on_action(cx.listener(move |view, _: &ActivateSection, _, cx| {
        if owner.is_some_and(|owner| owner.is_current(view)) {
            if finalize_controls(view, cx) {
                let state = view.device_panel.operator_pages.entry(device).or_default();
                state.selected_operator = operator;
                if state.page != OperatorPage::Envelope {
                    state.page = OperatorPage::Oscillator;
                }
                cx.notify();
            }
            cx.stop_propagation();
        }
    }))
}

/// One ordinary canonical Boolean edit, with pending state retained on queue
/// refusal. Rapid clicks read the latest queued value rather than a render's
/// captured flag. Project history and audio publication remain session-owned.
pub(super) fn toggle_enable(view: &mut EditorView, device: DeviceId, operator: usize) -> bool {
    if operator >= 4 {
        return false;
    }
    let key = ParameterKey {
        device,
        parameter: id(513 + operator as u32),
    };
    let canonical = view
        .device_panel
        .views
        .get(&device)
        .and_then(|(_, _, device_view)| device_view.as_ref().ok())
        .and_then(|device_view| {
            device_view
                .parameters
                .iter()
                .find(|parameter| parameter.descriptor.id == key.parameter)
        })
        .map(|parameter| parameter.value);
    let previous_pending = view.device_panel.pending_values.get(&key).copied();
    let Some(ParameterValue::Bool(previous)) = previous_pending.or(canonical) else {
        view.error = Some("oscillator enable control is unavailable".into());
        return false;
    };
    let value = ParameterValue::Bool(!previous);
    view.device_panel.pending_values.insert(key, value);
    if !view.enqueue_device_barrier(hex_session::service::Request::SetDeviceParameter {
        device,
        parameter: key.parameter,
        value,
    }) {
        if let Some(previous) = previous_pending {
            view.device_panel.pending_values.insert(key, previous);
        } else {
            view.device_panel.pending_values.remove(&key);
        }
        return false;
    }
    true
}

fn page_tab(
    cx: &mut Context<EditorView>,
    device: DeviceId,
    owner: Option<context::Owner>,
    page: OperatorPage,
    label: &'static str,
    active: OperatorPage,
    focus: gpui::FocusHandle,
) -> Stateful<Div> {
    hex_widgets::panel_tab(theme::text(label), page == active, display_style())
        .w(px(90.0))
        .h(px(16.0))
        .flex_shrink_0()
        .id(ElementId::Name(
            format!("operator-page-{}-{label}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-page-{}-{label}", device.value()))
        .track_focus(&focus)
        .key_context("hex_operator_section")
        .cursor(gpui::CursorStyle::PointingHand)
        .on_click(cx.listener(move |view, _, window, cx| {
            if owner.is_some_and(|owner| owner.is_current(view)) && finalize_controls(view, cx) {
                focus.focus(window);
                view.device_panel
                    .operator_pages
                    .entry(device)
                    .or_default()
                    .page = page;
                cx.notify();
            }
        }))
        .on_action(cx.listener(move |view, _: &ActivateSection, _, cx| {
            if owner.is_some_and(|owner| owner.is_current(view)) {
                if finalize_controls(view, cx) {
                    view.device_panel
                        .operator_pages
                        .entry(device)
                        .or_default()
                        .page = page;
                    cx.notify();
                }
                cx.stop_propagation();
            }
        }))
}

fn oscillator_detail(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
    operator: usize,
) -> Stateful<Div> {
    let one_based = operator as u32 + 1;
    let waveform = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(56 + one_based),
        ControlPresentation::CompactSelector,
        Some("wave"),
    );
    let phase = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(operator as u32 * 3 + 3),
        ControlPresentation::InlineValue,
        Some("phase"),
    );
    let velocity = compact_control_with_codec(
        view,
        cx,
        device,
        identity,
        parameters,
        id(74 + operator as u32),
        ControlPresentation::InlineValue,
        Some("velocity"),
        Some(super::numeric_display::NumericDisplayCodec::Percent),
    );
    let feedback = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(78 + operator as u32),
        ControlPresentation::InlineValue,
        Some("feedback"),
    );
    let fixed = matches!(
        value(view, device, parameters, id(62 + operator as u32 * 3)),
        Some(ParameterValue::Bool(true))
    );
    let multiplier = fixed.then(|| {
        compact_control(
            view,
            cx,
            device,
            identity,
            parameters,
            id(64 + operator as u32 * 3),
            ControlPresentation::InlineValue,
            Some("multi"),
        )
    });
    let waveform_choice =
        value(view, device, parameters, id(56 + one_based)).and_then(|value| match value {
            ParameterValue::Choice(choice) => Some(choice),
            _ => None,
        });
    let cycle = waveform_choice.and_then(waveform_cycle).and_then(|points| {
        hex_widgets::curve_plot(points, display_style())
            .map(|plot| plot.grid().grid_cols(1).grid_rows(1))
    });
    div()
        .id(ElementId::Name(
            format!("operator-osc-detail-{}-{operator}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-osc-detail-{}-{operator}", device.value()))
        .w_full()
        .h(px(160.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .id("operator-waveform-cycle")
                .debug_selector(|| "operator-waveform-cycle".into())
                .w_full()
                .h(px(86.0))
                .flex_shrink_0()
                .child(cycle.unwrap_or_else(|| div())),
        )
        .child(div().h(px(16.0)).flex_shrink_0())
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(3.0))
                .child(waveform)
                .child(section_title("phase"))
                .child(phase),
        )
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(3.0))
                .child(section_title("velocity"))
                .child(velocity)
                .child(section_title("feedback"))
                .child(feedback),
        )
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(3.0))
                .children(fixed.then(|| section_title("multi")))
                .children(multiplier),
        )
}

fn waveform_cycle(choice: u32) -> Option<[[f32; 2]; 32]> {
    if choice > 3 {
        return None;
    }
    Some(std::array::from_fn(|index| {
        let phase = index as f32 / 31.0;
        let level = match choice {
            0 => (std::f32::consts::TAU * phase).sin() * 0.5 + 0.5,
            1 => 1.0 - phase,
            2 => {
                if phase < 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            _ if phase <= 0.5 => phase * 2.0,
            _ => (1.0 - phase) * 2.0,
        };
        [phase, level]
    }))
}

fn envelope_detail(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
    operator: usize,
) -> Stateful<Div> {
    let base = 33 + operator as u32 * 6;
    let labels = ["initial", "peak", "sustain", "attack", "decay", "release"];
    let values = std::array::from_fn(|field| {
        value(view, device, parameters, id(base + field as u32))
            .and_then(|value| match value {
                ParameterValue::Float(value) if value.is_finite() => Some(value),
                _ => None,
            })
            .unwrap_or(0.0)
    });
    let graph = envelope::graph(view, cx, device, operator, values);
    let mut fields = div().w_full().flex().flex_col().gap(px(2.0));
    for indices in [[3, 4, 5], [0, 1, 2]] {
        let mut row = div().w_full().flex().gap(px(3.0));
        for field in indices {
            row = row.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(section_title(labels[field]))
                    .child(compact_control_with_codec(
                        view,
                        cx,
                        device,
                        identity,
                        parameters,
                        id(base + field as u32),
                        ControlPresentation::InlineValue,
                        None,
                        Some(if field < 3 {
                            super::numeric_display::NumericDisplayCodec::AmplitudeDb
                        } else {
                            super::numeric_display::NumericDisplayCodec::Seconds
                        }),
                    )),
            );
        }
        fields = fields.child(row);
    }
    div()
        .id(ElementId::Name(
            format!("operator-envelope-{}-{operator}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-envelope-{}-{operator}", device.value()))
        .w_full()
        .h(px(160.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(graph)
        .child(div().h(px(16.0)).flex_shrink_0())
        .child(fields)
}

fn routing_detail(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
) -> Stateful<Div> {
    let preset = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(61),
        ControlPresentation::CompactSelector,
        Some("routing preset"),
    );
    let mut rows = div().flex().flex_col().gap(px(2.0));
    let mut header = div()
        .id("operator-routing-header")
        .debug_selector(|| "operator-routing-header".into())
        .h(px(10.0))
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(routing_header_cell(
            "operator-routing-source-header",
            "",
            11.0,
        ));
    for target in 0..4 {
        header = header.child(routing_header_cell(
            format!("operator-routing-target-{target}"),
            ["a", "b", "c", "d"][target],
            FIELD_WIDTH,
        ));
    }
    header = header.child(routing_header_cell(
        "operator-routing-out-header",
        "out",
        FIELD_WIDTH,
    ));
    for source in 0..4_u32 {
        let mut row = div().h(px(18.0)).flex().items_center().gap(px(2.0)).child(
            div()
                .id(ElementId::Name(
                    format!("operator-routing-source-{source}").into(),
                ))
                .debug_selector(move || format!("operator-routing-source-{source}"))
                .w(px(11.0))
                .flex()
                .justify_center()
                .child(section_title(["a", "b", "c", "d"][source as usize])),
        );
        for target in 0..4_u32 {
            if source == target {
                row = row.child(div().w(px(FIELD_WIDTH)).h(px(14.0)));
                continue;
            }
            let parameter_id = 12 + source * 4 + target + 1;
            row = row.child(routing_cell(
                view,
                cx,
                device,
                identity,
                parameters,
                id(parameter_id),
                super::numeric_display::NumericDisplayCodec::Turns,
            ));
        }
        let carrier = routing_cell(
            view,
            cx,
            device,
            identity,
            parameters,
            id(29 + source),
            super::numeric_display::NumericDisplayCodec::LinearGain,
        );
        row = row.child(carrier);
        rows = rows.child(row);
    }
    div()
        .id(ElementId::Name(
            format!("operator-routing-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-routing-{}", device.value()))
        .w_full()
        .h(px(160.0))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(
            div()
                .h(px(86.0))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(header)
                .child(rows),
        )
        .child(div().h(px(16.0)).flex_shrink_0())
        .child(div().flex().items_center().gap(px(3.0)).child(preset))
}

fn routing_header_cell(
    selector: impl Into<String>,
    text: &'static str,
    width: f32,
) -> Stateful<Div> {
    let selector = selector.into();
    let id = ElementId::Name(selector.clone().into());
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .w(px(width))
        .h(px(10.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(7.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(text))
}

#[allow(clippy::too_many_arguments)] // Keeps the existing parameter binding and view codec explicit.
fn routing_cell(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
    parameter: ParameterId,
    codec: super::numeric_display::NumericDisplayCodec,
) -> Div {
    let description = parameters
        .iter()
        .find(|entry| entry.descriptor.id == parameter)
        .map(|entry| {
            let unit = match entry.descriptor.kind {
                ParameterKind::Float { unit, .. } => unit.unwrap_or(""),
                _ => "",
            };
            theme::text(format!("{} · {unit}", entry.descriptor.label))
        })
        .unwrap_or_else(|| theme::text("unavailable routing parameter"));
    let selector = format!(
        "operator-routing-cell-{}-{}",
        device.value(),
        parameter.value()
    );
    let control = compact_control_with_codec(
        view,
        cx,
        device,
        identity,
        parameters,
        parameter,
        ControlPresentation::InlineValue,
        None,
        Some(codec),
    )
    .id(ElementId::Name(selector.clone().into()))
    .debug_selector(move || selector.clone())
    // Long typed drafts scroll within their cell, never into a neighbor.
    .overflow_hidden()
    .tooltip(move |_, cx| {
        cx.new(|_| crate::editor::transport::MiniTooltip {
            description: description.clone(),
        })
        .into()
    });
    div()
        .w(px(FIELD_WIDTH))
        .h(px(14.0))
        .flex_shrink_0()
        .child(control)
}

#[allow(clippy::too_many_arguments)] // Keeps GPUI context, binding identity and presentation explicit.
fn compact_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
    parameter_id: ParameterId,
    presentation: ControlPresentation,
    label: Option<&'static str>,
) -> Div {
    compact_control_with_codec(
        view,
        cx,
        device,
        identity,
        parameters,
        parameter_id,
        presentation,
        label,
        None,
    )
}

#[allow(clippy::too_many_arguments)] // Keeps row context and the shared control binding explicit.
fn shell_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
    operator: usize,
    parameter_id: ParameterId,
    presentation: ControlPresentation,
    label: Option<&'static str>,
) -> Div {
    let control = compact_control_with_codec(
        view,
        cx,
        device,
        identity,
        parameters,
        parameter_id,
        presentation,
        label,
        match label {
            Some("level") => Some(super::numeric_display::NumericDisplayCodec::AmplitudeDb),
            Some("coarse") => Some(super::numeric_display::NumericDisplayCodec::Number),
            _ => None,
        },
    );
    let key = ParameterKey {
        device,
        parameter: parameter_id,
    };
    let owner = context::Owner::capture(view, device);
    control.on_mouse_down(
        gpui::MouseButton::Left,
        cx.listener(move |view, _, _, cx| {
            if let Some(owner) = owner {
                context::select_oscillator(view, cx, owner, operator, &[key]);
            }
        }),
    )
}

#[allow(clippy::too_many_arguments)] // Codec is an optional view-only layer on the same canonical binding.
fn compact_control_with_codec(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
    parameter_id: ParameterId,
    presentation: ControlPresentation,
    label: Option<&'static str>,
    codec: Option<super::numeric_display::NumericDisplayCodec>,
) -> Div {
    let Some(parameter) = parameters
        .iter()
        .find(|parameter| parameter.descriptor.id == parameter_id)
    else {
        return div()
            .w(px(FIELD_WIDTH))
            .h(px(14.0))
            .text_size(px(7.0))
            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
            .child(theme::text("n/a"));
    };
    parameter_control_with_presentation(
        view,
        cx,
        device,
        identity,
        parameter,
        presentation,
        super::ControlText { label, codec },
    )
}

fn value(
    view: &mut EditorView,
    device: DeviceId,
    parameters: &[NativeDeviceParameterView],
    parameter_id: ParameterId,
) -> Option<ParameterValue> {
    let parameter = parameters
        .iter()
        .find(|parameter| parameter.descriptor.id == parameter_id)?;
    Some(effective_parameter_value(
        view,
        ParameterKey {
            device,
            parameter: parameter_id,
        },
        parameter.value,
    ))
}

pub(super) fn display_style() -> hex_widgets::ControlStyle {
    theme::device_style(theme::DeviceStyleRole::Display)
}

fn section_title(label: &'static str) -> Div {
    hex_widgets::device_inline_caption(theme::text(label), super::control_style())
}

fn lfo_detail(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
) -> Stateful<Div> {
    let mut content = div().w_full().h_full().flex().flex_col().gap(px(4.0));
    let sync = matches!(
        value(view, device, parameters, id(84)),
        Some(ParameterValue::Bool(true))
    );
    // Shared widget entities must have exactly one rendered focus parent.
    // On/wave/active-rate/amount live in the summary; detail owns the remainder.
    for ids in [
        &[84, if sync { 85 } else { 86 }][..],
        &[88, 89, 90, 91, 92][..],
    ] {
        let mut row = div().w_full().flex().items_center().gap(px(3.0));
        for parameter in ids {
            row = row.child(compact_control(
                view,
                cx,
                device,
                identity,
                parameters,
                id(*parameter),
                match *parameter {
                    82 | 84 | 88..=92 => ControlPresentation::CompactButton,
                    83 | 86 => ControlPresentation::CompactSelector,
                    _ => ControlPresentation::MicroDial,
                },
                Some(match *parameter {
                    82 => "on",
                    83 => "wave",
                    84 => "sync",
                    85 => "hz",
                    86 => "beats",
                    87 => "amount",
                    88 => "retrig",
                    89 => "a",
                    90 => "b",
                    91 => "c",
                    _ => "d",
                }),
            ));
        }
        content = content.child(row);
    }
    content.id(ElementId::Name(
        format!("operator-lfo-detail-{}", device.value()).into(),
    ))
}

pub(super) fn lfo_summary(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Instrument,
        type_id: device_view.type_id,
    };
    let parameters = &device_view.parameters;
    let sync = matches!(
        value(view, device, parameters, id(84)),
        Some(ParameterValue::Bool(true))
    );
    let section = Section::Preview("lfo");
    let (focus, focused) = context::section_focus(view, cx, device, section);
    let mut row = hex_widgets::device_grouped_row(super::device_layout_style(), focused)
        .on_children_prepainted(context::section_listener(view, cx, device, section))
        .id(ElementId::Name(
            format!("operator-preview-row-{}-lfo", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-preview-row-{}-lfo", device.value()))
        .w_full()
        .track_focus(&focus)
        .h(px(40.0))
        .flex()
        .items_center()
        .gap(px(2.0));
    row = row.child(compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(82),
        ControlPresentation::CompactButton,
        Some("lfo"),
    ));
    for (parameter, label) in [
        (83, "wave"),
        (if sync { 86 } else { 85 }, "rate"),
        (87, "amount"),
    ] {
        row = row.child(compact_control(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            if matches!(parameter, 83 | 86) {
                ControlPresentation::CompactSelector
            } else {
                ControlPresentation::MicroDial
            },
            Some(label),
        ));
    }
    row
}

fn filter_detail(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
) -> Stateful<Div> {
    let mut detail = div()
        .w_full()
        .h_full()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(section_title("filter"));
    for fields in [
        &[(20_003, "slope"), (20_004, "circuit")][..],
        &[(20_007, "morph")][..],
    ] {
        let mut row = div()
            .w_full()
            .h(px(48.0))
            .flex()
            .items_center()
            .gap(px(3.0));
        for &(parameter, label) in fields {
            row = row.child(compact_control(
                view,
                cx,
                device,
                identity,
                parameters,
                id(parameter),
                if parameter < 20_005 {
                    ControlPresentation::CompactSelector
                } else {
                    ControlPresentation::MicroDial
                },
                Some(label),
            ));
        }
        detail = detail.child(row);
    }
    detail.id(ElementId::Name(
        format!("operator-filter-detail-{}", device.value()).into(),
    ))
}

pub(super) fn filter_summary(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Instrument,
        type_id: device_view.type_id,
    };
    let section = Section::Preview("filter");
    let (focus, focused) = context::section_focus(view, cx, device, section);
    let mut row = hex_widgets::device_grouped_row(super::device_layout_style(), focused)
        .on_children_prepainted(context::section_listener(view, cx, device, section))
        .id(ElementId::Name(
            format!("operator-preview-row-{}-filter", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-preview-row-{}-filter", device.value()))
        .track_focus(&focus)
        .w_full()
        .h(px(40.0))
        .flex()
        .items_center()
        .gap(px(2.0));
    for (parameter, label) in [
        (20_001, "filter"),
        (20_002, "type"),
        (20_005, "freq"),
        (20_006, "res"),
    ] {
        row = row.child(compact_control(
            view,
            cx,
            device,
            identity,
            &device_view.parameters,
            id(parameter),
            if parameter == 20_001 {
                ControlPresentation::CompactButton
            } else if parameter < 20_005 {
                ControlPresentation::CompactSelector
            } else {
                ControlPresentation::MicroDial
            },
            Some(label),
        ));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_view_inventory_maps_every_descriptor_to_a_panel_page() {
        let mut exposed = Vec::new();
        for operator in 0..4_u32 {
            let base = operator * 3 + 1;
            exposed.extend([base, base + 1, base + 2]);
            exposed.extend(33 + operator * 6..39 + operator * 6);
            exposed.extend([
                57 + operator,
                62 + operator * 3,
                63 + operator * 3,
                64 + operator * 3,
            ]);
            exposed.extend([74 + operator, 78 + operator]);
        }
        exposed.extend([61]);
        exposed.extend(82..=92);
        exposed.extend(20_001..=20_007);
        exposed.extend(513..=516);
        for source in 0..4_u32 {
            for target in 0..4_u32 {
                if source != target {
                    exposed.push(12 + source * 4 + target + 1);
                }
            }
            exposed.push(29 + source);
        }
        exposed.sort_unstable();
        exposed.dedup();
        let registered = hex_engine::native_device_catalog_for_class(NativeDeviceClass::Instrument)
            .expect("registered instrument catalog")
            .into_iter()
            .find(|entry| entry.type_id.as_str() == Ok("hex.instrument.operator"))
            .expect("registered Operator descriptor");
        let mut actual: Vec<_> = registered
            .parameters
            .iter()
            .map(|descriptor| descriptor.id.value())
            .collect();
        actual.sort_unstable();
        assert_eq!(
            exposed, actual,
            "panel inventory matches registered metadata"
        );
        assert_eq!(exposed.len(), 99);
        assert_eq!(exposed.first(), Some(&1));
        assert_eq!(exposed.last(), Some(&20_007));
        for diagonal in [13, 18, 23, 28] {
            assert!(!exposed.contains(&diagonal));
        }
    }

    #[test]
    fn waveform_cycle_preview_matches_selected_fundamental_shape() {
        let sine = waveform_cycle(0).expect("sine preview");
        let saw = waveform_cycle(1).expect("saw preview");
        let square = waveform_cycle(2).expect("square preview");
        let triangle = waveform_cycle(3).expect("triangle preview");
        assert_eq!(sine.len(), 32);
        assert_eq!(saw[0], [0.0, 1.0]);
        assert_eq!(saw[31], [1.0, 0.0]);
        assert_eq!(square[0][1], 1.0);
        assert_eq!(square[31][1], 0.0);
        assert!(
            triangle
                .iter()
                .all(|[x, y]| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y))
        );
        assert!(waveform_cycle(4).is_none());
    }
}
