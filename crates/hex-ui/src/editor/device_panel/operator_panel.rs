//! Compact Operator composition backed by the shared device control entities.
use super::{
    ControlPresentation, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, finalize_controls, parameter_control_with_presentation,
};
use crate::diagnostics::ActionTraceContext as _;
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{NativeDeviceClass, NativeDeviceParameterView, NativeDeviceView};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::DeviceId;

pub(super) mod algorithms;
pub(super) mod context;
pub(super) mod envelope;
mod global;
pub(super) mod harmonics;
pub(super) mod preview;

pub(super) const OPERATOR_FRAME_WIDTH: f32 = crate::device_composition::VECTOR_WIDTH;
// Three shared tiles plus fixed selector, badge, gaps and retained breathing room.
const SHELL_WIDTH: f32 = crate::device_composition::VECTOR_SHELL_WIDTH;
const FIELD_WIDTH: f32 = 46.0;
// Retain the implementation while these additional harmonic tools are dormant.
const SHOW_HARMONIC_TOOLS: bool = false;

gpui::actions!(hex_operator, [ActivateRouting, ActivateSection]);

pub(in crate::editor) fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        gpui::KeyBinding::new("enter", ActivateSection, Some("hex_operator_section")),
        gpui::KeyBinding::new("enter", ActivateRouting, Some("hex_operator_routing")),
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
    Algorithm(u32),
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
        1..=12 if !value.is_multiple_of(3) => Some(((value - 1) / 3) as usize),
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
    harmonics::retire(view, cx, |key| {
        key.0 != device
            || (matches!(
                state.page,
                OperatorPage::Oscillator | OperatorPage::Envelope
            ) && key.1 == state.selected_operator)
    });
    if state.page == OperatorPage::Envelope {
        harmonics::hide_painter(view, cx, device, state.selected_operator);
    }
    let detail = match state.page {
        OperatorPage::Oscillator | OperatorPage::Envelope => oscillator_envelope_detail(
            view,
            cx,
            device,
            identity,
            parameters,
            state.selected_operator,
            state.page,
        ),
        OperatorPage::Routing => routing_detail(view, cx, device, identity, parameters),
        OperatorPage::Lfo => lfo_detail(view, cx, device, identity, parameters),
        OperatorPage::Filter => filter_detail(view, cx, device, identity, parameters),
    };
    let (detail_focus, _) = context::section_focus(view, cx, device, Section::Detail);
    let tabs = if state.page == OperatorPage::Routing {
        div()
    } else {
        page_tabs(view, cx, device, owner, state)
    };
    let detail = crate::device_composition::tabbed_detail(detail, tabs)
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
        .child(
            page_tab(
                cx,
                device,
                owner,
                OperatorPage::Envelope,
                "envelope",
                state.page,
                envelope_focus,
            )
            .flex_1(),
        )
        .child(
            page_tab(
                cx,
                device,
                owner,
                OperatorPage::Oscillator,
                "oscillator",
                state.page,
                oscillator_focus,
            )
            .w(px(120.0)),
        )
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
        ControlPresentation::FilledSlider,
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
        .pr(px(18.0))
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
            .w(px(60.0))
            .h(px(40.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(section_title("fixed"))
                    .child(fixed_control),
            )
            .child(level),
    );
    row = row.child(
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
    .on_action(cx.trace(move |view, _: &ActivateSection, _, cx| {
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
        .on_action(cx.trace(move |view, _: &ActivateSection, _, cx| {
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

fn oscillator_envelope_detail(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
    operator: usize,
    page: OperatorPage,
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
    let phase = compact_control_with_codec(
        view,
        cx,
        device,
        identity,
        parameters,
        id(operator as u32 * 3 + 3),
        ControlPresentation::InlineValue,
        Some("phase"),
        Some(super::numeric_display::NumericDisplayCodec::Percent),
    );
    let companion_base = 357 + operator as u32 * 8;
    let oscillator_velocity = compact_control_with_codec(
        view,
        cx,
        device,
        identity,
        parameters,
        id(companion_base + 1),
        ControlPresentation::InlineValue,
        Some("osc<vel"),
        Some(super::numeric_display::NumericDisplayCodec::Percent),
    );
    let restart = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(companion_base),
        ControlPresentation::CompactButton,
        Some("r"),
    );
    let quantize = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(companion_base + 2),
        ControlPresentation::CompactButton,
        Some("q"),
    );
    let feedback = compact_control_with_codec(
        view,
        cx,
        device,
        identity,
        parameters,
        id(78 + operator as u32),
        ControlPresentation::InlineValue,
        Some("feedback"),
        Some(super::numeric_display::NumericDisplayCodec::Percent),
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
    let (graph, waveform_preview) = harmonics::graph(view, cx, device, operator, parameters);
    let harmonic_range_control = harmonics::ranges(view, cx, device, operator, parameters);
    let harmonic_repeat_control = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(353 + operator as u32),
        ControlPresentation::CompactSelector,
        Some("repeat"),
    );
    let envelope_fields =
        envelope_fields(view, cx, device, identity, parameters, operator, multiplier)
            .when(SHOW_HARMONIC_TOOLS, |fields| {
                fields.child(harmonics::tools(view, cx, device, operator))
            });
    let oscillator_fields = div()
        .id("operator-oscillator-fields")
        .debug_selector(|| "operator-oscillator-fields".into())
        .w(px(120.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .child(
            div()
                .w_full()
                .h(px(21.0))
                .flex_shrink_0()
                .flex()
                .gap(px(4.0))
                .child(detail_field(
                    device,
                    id(57 + operator as u32),
                    "wave",
                    64.0,
                    waveform,
                ))
                .child(
                    div()
                        .id("operator-waveform-preview")
                        .debug_selector(|| "operator-waveform-preview".into())
                        .w(px(52.0))
                        .h(px(21.0))
                        .flex_shrink_0()
                        .child(waveform_preview),
                ),
        )
        .child(
            div()
                .w_full()
                .h(px(21.0))
                .flex_shrink_0()
                .flex()
                .gap(px(4.0))
                .child(detail_field(
                    device,
                    id(78 + operator as u32),
                    "feedback",
                    52.0,
                    feedback,
                ))
                .child(detail_field(
                    device,
                    id(353 + operator as u32),
                    "repeat",
                    64.0,
                    harmonic_repeat_control,
                )),
        )
        .child(
            div()
                .w_full()
                .h(px(21.0))
                .flex_shrink_0()
                .flex()
                .gap(px(4.0))
                .child(detail_field(
                    device,
                    id(operator as u32 * 3 + 3),
                    "phase",
                    58.0,
                    div()
                        .flex()
                        .gap(px(2.0))
                        .child(div().w(px(42.0)).child(phase))
                        .child(restart),
                ))
                .child(detail_field(
                    device,
                    id(companion_base + 1),
                    "osc<vel",
                    58.0,
                    div()
                        .flex()
                        .gap(px(2.0))
                        .child(div().w(px(42.0)).child(oscillator_velocity))
                        .child(quantize),
                )),
        );
    let upper_display = if page == OperatorPage::Envelope {
        let base = 33 + operator as u32 * 6;
        let values = std::array::from_fn(|field| {
            value(view, device, parameters, id(base + field as u32))
                .and_then(|value| match value {
                    ParameterValue::Float(value) if value.is_finite() => Some(value),
                    _ => None,
                })
                .unwrap_or(0.0)
        });
        envelope::graph(view, cx, device, operator, values)
    } else {
        div()
            .id("operator-waveform-cycle")
            .debug_selector(|| "operator-waveform-cycle".into())
            .w_full()
            .h(px(86.0))
            .flex_shrink_0()
            .flex()
            .gap(px(2.0))
            .child(div().flex_1().min_w_0().h_full().child(graph))
            .child(harmonic_range_control)
    };
    let selector = if page == OperatorPage::Envelope {
        format!("operator-envelope-{}-{operator}", device.value())
    } else {
        format!("operator-osc-detail-{}-{operator}", device.value())
    };
    div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .w_full()
        .h_full()
        .flex()
        .flex_col()
        .child(upper_display)
        // Both detail pages keep the same graph and tab slots.
        .child(div().h(px(16.0)).flex_shrink_0())
        .child(
            div()
                .w_full()
                .h(px(63.0))
                .flex_shrink_0()
                .flex()
                .gap(px(4.0))
                .child(
                    div()
                        .id("operator-envelope-fields")
                        .debug_selector(|| "operator-envelope-fields".into())
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(envelope_fields),
                )
                .child(oscillator_fields),
        )
}

// Display order is independent of stable serialized parameter choices.
pub(super) fn choice_order(
    identity: DeviceControlIdentity,
    parameter: ParameterId,
    count: usize,
) -> Vec<u32> {
    if identity.type_id.as_str() == Ok("hex.instrument.operator")
        && (57..=60).contains(&parameter.value())
        && count == 23
    {
        vec![
            0, 5, 6, 7, 8, 9, 10, 11, 12, 13, 1, 14, 15, 16, 17, 18, 19, 20, 2, 3, 21, 22, 4,
        ]
    } else {
        (0..count as u32).collect()
    }
}

fn waveform_cycle(choice: u32) -> Result<[[f32; 2]; 256], hex_engine::Invalid> {
    hex_engine::native_operator_waveform_preview(choice)
        .map_err(|_| hex_engine::Invalid("could not prepare oscillator waveform preview"))
        .map(|samples| {
            std::array::from_fn(|index| {
                [
                    index as f32 / 255.0,
                    (samples[index] * 0.5 + 0.5).clamp(0.0, 1.0),
                ]
            })
        })
}

fn detail_caption(label: &'static str) -> Div {
    hex_widgets::device_inline_caption(theme::text(label), super::control_style())
        .h(px(7.0))
        .text_size(px(7.0))
        .line_height(px(7.0))
        .overflow_hidden()
}

fn detail_field(
    device: DeviceId,
    parameter: ParameterId,
    label: &'static str,
    width: f32,
    control: Div,
) -> Stateful<Div> {
    let selector = format!("operator-field-{}-{}", device.value(), parameter.value());
    let caption_selector = format!("{selector}-label");
    let value_selector = format!("{selector}-value");
    div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .w(px(width))
        .h(px(21.0))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            detail_caption(label)
                .id(ElementId::Name(caption_selector.clone().into()))
                .debug_selector(move || caption_selector.clone()),
        )
        // Constrain even long typed drafts to this field's own hit/paint bounds.
        .child(
            div()
                .id(ElementId::Name(value_selector.clone().into()))
                .debug_selector(move || value_selector.clone())
                .w_full()
                .h(px(14.0))
                .overflow_hidden()
                .child(control),
        )
}

#[allow(clippy::too_many_arguments)] // Fixed multiplier shares the third inspector row.
fn envelope_fields(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
    operator: usize,
    multiplier: Option<Div>,
) -> Div {
    use super::numeric_display::NumericDisplayCodec as Codec;
    let base = 33 + operator as u32 * 6;
    let companion = 357 + operator as u32 * 8;
    let labels = ["initial", "peak", "sustain", "attack", "decay", "release"];
    let mut fields = div().w_full().flex().flex_col();
    for (indices, extra, label) in [
        ([3, 4, 5], companion + 3, "time<vel"),
        ([0, 1, 2], 74 + operator as u32, "vel"),
    ] {
        let mut row = div()
            .w_full()
            .h(px(21.0))
            .flex_shrink_0()
            .flex()
            .gap(px(4.0));
        for field in indices {
            let parameter = id(base + field as u32);
            let control = compact_control_with_codec(
                view,
                cx,
                device,
                identity,
                parameters,
                parameter,
                ControlPresentation::InlineValue,
                None,
                Some(if field < 3 {
                    Codec::AmplitudeDb
                } else {
                    Codec::Seconds
                }),
            );
            row = row.child(detail_field(
                device,
                parameter,
                labels[field],
                46.0,
                control,
            ));
        }
        let control = compact_control_with_codec(
            view,
            cx,
            device,
            identity,
            parameters,
            id(extra),
            ControlPresentation::InlineValue,
            None,
            Some(Codec::Percent),
        );
        fields = fields.child(row.child(detail_field(device, id(extra), label, 46.0, control)));
    }
    let loop_mode = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(companion + 5),
        ControlPresentation::CompactSelector,
        Some("loop"),
    );
    let loop_choice = value(view, device, parameters, id(companion + 5));
    let loop_parameter = match loop_choice {
        Some(ParameterValue::Choice(1)) => Some((
            companion + 6,
            "time",
            ControlPresentation::InlineValue,
            Some(Codec::Seconds),
        )),
        Some(ParameterValue::Choice(2 | 3)) => Some((
            companion + 7,
            "repeat",
            ControlPresentation::CompactSelector,
            None,
        )),
        _ => None,
    };
    let mut loop_cell = div()
        .w(px(100.0))
        .flex_shrink_0()
        .flex()
        .gap(px(4.0))
        .child(detail_field(
            device,
            id(companion + 5),
            "loop",
            52.0,
            loop_mode,
        ));
    if let Some((parameter, label, presentation, codec)) = loop_parameter {
        let control = compact_control_with_codec(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            presentation,
            None,
            codec,
        );
        loop_cell = loop_cell.child(detail_field(device, id(parameter), label, 44.0, control));
    }
    let key = compact_control_with_codec(
        view,
        cx,
        device,
        identity,
        parameters,
        id(companion + 4),
        ControlPresentation::InlineValue,
        None,
        Some(Codec::Percent),
    );
    let mut row = div()
        .w_full()
        .h(px(21.0))
        .flex_shrink_0()
        .flex()
        .gap(px(4.0))
        .child(loop_cell)
        .child(detail_field(device, id(companion + 4), "key", 46.0, key));
    if let Some(multiplier) = multiplier {
        row = row.child(detail_field(
            device,
            id(64 + operator as u32 * 3),
            "multi",
            46.0,
            multiplier,
        ));
    }
    fields.child(row)
}

fn routing_detail(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
) -> Stateful<Div> {
    let selected = value(view, device, parameters, id(600));
    div()
        .id(ElementId::Name(
            format!("operator-routing-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-routing-{}", device.value()))
        .w_full()
        .h_full()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(algorithms::selector(view, cx, device, selected))
        .child(global::expression_routing(
            view, cx, device, identity, parameters,
        ))
        .child(global::controls(view, cx, device, identity, parameters))
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

fn section_title(label: impl AsRef<str>) -> Div {
    hex_widgets::device_inline_caption(theme::text(label), super::control_style())
}

fn lfo_detail(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
) -> Stateful<Div> {
    let mut content = div().w_full().h_full().flex().flex_col().gap(px(1.0));
    let sync = matches!(
        value(view, device, parameters, id(84)),
        Some(ParameterValue::Bool(true))
    );
    // Shared widget entities must have exactly one rendered focus parent.
    // On/wave/active-rate/amount live in the summary; detail owns the remainder.
    for ids in [
        &[84, if sync { 85 } else { 86 }, 88][..],
        &[89, 90, 91, 92][..],
    ] {
        if ids[0] == 89 {
            let selected = ["a", "b", "c", "d"]
                .into_iter()
                .enumerate()
                .filter_map(|(index, label)| {
                    (value(view, device, parameters, id(89 + index as u32))
                        == Some(ParameterValue::Bool(true)))
                    .then_some(label)
                })
                .collect::<Vec<_>>()
                .join(" ");
            content = content.child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(section_title("pitch destinations").debug_selector(move || {
                        format!("operator-lfo-destinations-{}", device.value())
                    }))
                    .child(
                        section_title(if selected.is_empty() {
                            "selected: none".to_owned()
                        } else {
                            format!("selected: {selected}")
                        })
                        .debug_selector(move || {
                            format!("operator-lfo-selected-{}-{selected}", device.value())
                        }),
                    ),
            );
        }
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
    content = content.child(
        section_title("100% = +/-12 st; pitch only")
            .debug_selector(move || format!("operator-lfo-depth-{}", device.value())),
    );
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
    let active = view
        .device_panel
        .operator_pages
        .entry(device)
        .or_default()
        .page;
    let link_focus = navigation_focus(view, cx, device, NavigationTarget::Page(OperatorPage::Lfo));
    let link = page_tab(
        cx,
        device,
        context::Owner::capture(view, device),
        OperatorPage::Lfo,
        "lfo",
        active,
        link_focus,
    )
    .id(ElementId::Name(
        format!("operator-lfo-link-{}", device.value()).into(),
    ))
    .debug_selector(move || format!("operator-lfo-link-{}", device.value()))
    .w(px(40.0))
    .h(px(14.0));
    let enabled = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        id(82),
        ControlPresentation::CompactButton,
        Some(""),
    );
    row = row.child(
        div()
            .w(px(40.0))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(2.0))
            .child(link)
            .child(enabled),
    );
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
    fn operator_metadata_inventory_covers_panel_and_preserved_legacy_parameters() {
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
        exposed.extend(600..=608);
        exposed.extend(620..=639);
        exposed.extend(82..=388);
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
        assert_eq!(exposed.len(), 424);
        assert_eq!(exposed.first(), Some(&1));
        assert_eq!(exposed.last(), Some(&20_007));
        for diagonal in [13, 18, 23, 28] {
            assert!(!exposed.contains(&diagonal));
        }
    }

    #[test]
    fn waveform_previews_cover_the_catalog_and_reject_user_without_a_bank() {
        for choice in (0..23).filter(|choice| *choice != 4) {
            let points = waveform_cycle(choice).expect("builtin preview");
            assert_eq!(points.len(), 256);
            assert!(
                points
                    .iter()
                    .all(|[x, y]| x.is_finite() && y.is_finite() && (0.0..=1.0).contains(y))
            );
        }
        assert!(waveform_cycle(4).is_err());
        assert!(waveform_cycle(23).is_err());
    }
}
