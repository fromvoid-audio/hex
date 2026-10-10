//! Compact Operator composition backed by the shared device control entities.
use super::{
    ControlPresentation, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, finalize_controls, finalize_controls_except,
    parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{NativeDeviceClass, NativeDeviceParameterView, NativeDeviceView};
use hex_lattice_contract::{ParameterId, ParameterValue};
use hex_project::DeviceId;

pub(super) mod envelope;

pub(super) const OPERATOR_FRAME_WIDTH: f32 = 580.0;
const BODY_HEIGHT: f32 = 168.0;
const SHELL_WIDTH: f32 = 210.0;
const FIELD_WIDTH: f32 = 46.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct OperatorPanelState {
    pub(super) selected_operator: usize,
    pub(super) page: OperatorPage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum OperatorPage {
    #[default]
    Oscillator,
    Envelope,
    Routing,
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
    let parameters = &device_view.parameters;
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Instrument,
        type_id: device_view.type_id,
    };
    let mut shell = div()
        .id(ElementId::Name(
            format!("operator-shell-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-shell-{}", device.value()))
        .w(px(SHELL_WIDTH))
        .h_full()
        .flex()
        .flex_col()
        .gap(px(0.0))
        .pr(px(4.0))
        .border_r_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)));
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

    let page_tabs = div()
        .flex()
        .w_full()
        .h(px(16.0))
        .items_center()
        .gap(px(3.0))
        .child(page_tab(
            cx,
            device,
            OperatorPage::Oscillator,
            "osc",
            state.page,
        ))
        .child(page_tab(
            cx,
            device,
            OperatorPage::Envelope,
            "env",
            state.page,
        ))
        .child(page_tab(
            cx,
            device,
            OperatorPage::Routing,
            "routing",
            state.page,
        ));
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
    };
    div()
        .id(ElementId::Name(
            format!("operator-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-body-{}", device.value()))
        .w_full()
        .h(px(BODY_HEIGHT))
        .min_h_0()
        .overflow_hidden()
        .flex()
        .gap(px(4.0))
        .p(px(3.0))
        .child(shell)
        .child(
            div()
                .id(ElementId::Name(
                    format!("operator-detail-{}", device.value()).into(),
                ))
                .debug_selector(move || format!("operator-detail-{}", device.value()))
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(page_tabs)
                .child(detail),
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
    let multiplier_id = fixed_id + 2;
    let level = shell_control(
        view,
        cx,
        device,
        identity,
        parameters,
        operator,
        id(base + 1),
        ControlPresentation::MiniDial,
        Some("level"),
    );
    let primary = shell_control(
        view,
        cx,
        device,
        identity,
        parameters,
        operator,
        id(primary_id),
        ControlPresentation::MiniDial,
        Some(if fixed { "freq" } else { "ratio" }),
    );
    let mut row = div()
        .id(ElementId::Name(
            format!("operator-row-{}-{operator}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-row-{}-{operator}", device.value()))
        .w_full()
        .h(px(40.0))
        .flex()
        .items_center()
        .gap(px(2.0))
        .px(px(2.0))
        .rounded(px(hex_widgets::CONTROL_CORNER_RADIUS))
        .bg(rgb(if selected {
            theme::color(theme::PaletteRole::Hover)
        } else {
            theme::color(theme::PaletteRole::Background)
        }))
        .child(operator_label(cx, device, operator, selected))
        .child(level)
        .child(primary);
    if fixed {
        let multiplier = shell_control(
            view,
            cx,
            device,
            identity,
            parameters,
            operator,
            id(multiplier_id),
            ControlPresentation::MiniDial,
            Some("multi"),
        );
        row = row.child(multiplier);
    }
    let fixed_control = shell_control(
        view,
        cx,
        device,
        identity,
        parameters,
        operator,
        id(fixed_id),
        ControlPresentation::CompactButton,
        Some("fix"),
    );
    row = row.child(fixed_control);
    row
}

fn operator_label(
    cx: &mut Context<EditorView>,
    device: DeviceId,
    operator: usize,
    selected: bool,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(
            format!("operator-select-{}-{operator}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-select-{}-{operator}", device.value()))
        .w(px(11.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(9.0))
        .text_color(rgb(theme::color(if selected {
            theme::PaletteRole::Secondary
        } else {
            theme::PaletteRole::Text
        })))
        .cursor(gpui::CursorStyle::PointingHand)
        .child(theme::text(["a", "b", "c", "d"][operator]))
        .on_click(cx.listener(move |view, _, window, cx| {
            if finalize_controls(view, cx) {
                view.focus.focus(window);
                let state = view.device_panel.operator_pages.entry(device).or_default();
                state.selected_operator = operator;
                state.page = OperatorPage::Oscillator;
                cx.notify();
            }
        }))
}

fn page_tab(
    cx: &mut Context<EditorView>,
    device: DeviceId,
    page: OperatorPage,
    label: &'static str,
    active: OperatorPage,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(
            format!("operator-page-{}-{label}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-page-{}-{label}", device.value()))
        .h(px(14.0))
        .px(px(5.0))
        .flex()
        .items_center()
        .rounded(px(hex_widgets::CONTROL_CORNER_RADIUS))
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .bg(rgb(if page == active {
            theme::color(theme::PaletteRole::Raised)
        } else {
            theme::color(theme::PaletteRole::Surface)
        }))
        .text_size(px(8.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Text)))
        .cursor(gpui::CursorStyle::PointingHand)
        .child(theme::text(label))
        .on_click(cx.listener(move |view, _, window, cx| {
            if finalize_controls(view, cx) {
                view.focus.focus(window);
                view.device_panel
                    .operator_pages
                    .entry(device)
                    .or_default()
                    .page = page;
                cx.notify();
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
        ControlPresentation::MiniDial,
        Some("phase"),
    );
    let velocity = compact_control_with_codec(
        view,
        cx,
        device,
        identity,
        parameters,
        id(74 + operator as u32),
        ControlPresentation::MiniDial,
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
        ControlPresentation::MiniDial,
        Some("feedback"),
    );
    let waveform_choice =
        value(view, device, parameters, id(56 + one_based)).and_then(|value| match value {
            ParameterValue::Choice(choice) => Some(choice),
            _ => None,
        });
    let cycle = waveform_choice
        .and_then(waveform_cycle)
        .and_then(|points| hex_widgets::curve_plot(points, super::control_style()));
    div()
        .id(ElementId::Name(
            format!("operator-osc-detail-{}-{operator}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-osc-detail-{}-{operator}", device.value()))
        .w_full()
        .h(px(143.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .h(px(16.0))
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(section_title(
                    ["operator a", "operator b", "operator c", "operator d"][operator],
                ))
                .child(waveform),
        )
        .child(
            div()
                .id("operator-waveform-cycle")
                .debug_selector(|| "operator-waveform-cycle".into())
                .w_full()
                .h(px(76.0))
                .child(cycle.unwrap_or_else(|| div())),
        )
        .child(
            div()
                .w_full()
                .flex()
                .justify_end()
                .gap(px(2.0))
                .child(phase)
                .child(velocity)
                .child(feedback),
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
    let mut fields = div().w_full().flex().items_center().gap(px(2.0));
    for (field, label) in labels.iter().enumerate() {
        fields = fields.child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(section_title(label))
                .child(compact_control(
                    view,
                    cx,
                    device,
                    identity,
                    parameters,
                    id(base + field as u32),
                    ControlPresentation::InlineValue,
                    None,
                )),
        );
    }
    div()
        .id(ElementId::Name(
            format!("operator-envelope-{}-{operator}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-envelope-{}-{operator}", device.value()))
        .w_full()
        .h(px(143.0))
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(graph)
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
        let mut row = div().h(px(20.0)).flex().items_center().gap(px(2.0)).child(
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
            row = row.child(compact_control(
                view,
                cx,
                device,
                identity,
                parameters,
                id(parameter_id),
                ControlPresentation::InlineValue,
                None,
            ));
        }
        let carrier = compact_control(
            view,
            cx,
            device,
            identity,
            parameters,
            id(29 + source),
            ControlPresentation::InlineValue,
            None,
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
        .h(px(143.0))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(
            div()
                .h(px(16.0))
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(section_title("fm routing · source → target"))
                .child(preset),
        )
        .child(header)
        .child(rows)
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
    let control = compact_control(
        view,
        cx,
        device,
        identity,
        parameters,
        parameter_id,
        presentation,
        label,
    );
    let key = ParameterKey {
        device,
        parameter: parameter_id,
    };
    let target = OperatorPanelState {
        selected_operator: operator,
        page: OperatorPage::Oscillator,
    };
    control.on_mouse_down(
        gpui::MouseButton::Left,
        cx.listener(move |view, _, _, cx| {
            if view
                .device_panel
                .operator_pages
                .get(&device)
                .copied()
                .unwrap_or_default()
                == target
            {
                return;
            }
            if finalize_controls_except(view, cx, &[key]) {
                view.device_panel.operator_pages.insert(device, target);
                cx.notify();
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

fn section_title(label: &'static str) -> Div {
    div()
        .text_size(px(8.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(label))
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
        assert_eq!(exposed.len(), 77);
        assert_eq!(exposed.first(), Some(&1));
        assert_eq!(exposed.last(), Some(&81));
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
