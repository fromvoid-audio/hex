//! Compact overdrive composition using the shared Lattice controls and plot.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, numeric_display::NumericDisplayCodec,
    parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px};
use hex_engine::{NativeDeviceClass, NativeDeviceView, OverdriveFilterResponse};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::OVERDRIVE_FRAME_WIDTH;
const FREQUENCY: ParameterId = ParameterId::from_static(1);
const BANDWIDTH: ParameterId = ParameterId::from_static(2);
const DRIVE: ParameterId = ParameterId::from_static(3);
const TONE: ParameterId = ParameterId::from_static(4);
const DYNAMICS: ParameterId = ParameterId::from_static(5);
const WET: ParameterId = ParameterId::from_static(6);
const PARAMETERS: [ParameterId; 2] = [FREQUENCY, BANDWIDTH];

pub(super) struct PreviewCache {
    state: NativeDeviceState,
    sample_rate: u32,
    values: [ParameterValue; 2],
    curve: Result<OverdriveFilterResponse, String>,
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn cached_curve(
    view: &EditorView,
    device: DeviceId,
) -> Option<&OverdriveFilterResponse> {
    view.device_panel
        .overdrive_previews
        .get(&device)?
        .curve
        .as_ref()
        .ok()
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
    let mut values = [ParameterValue::Float(0.0); 2];
    for (index, id) in PARAMETERS.iter().enumerate() {
        let parameter = device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == *id)
            .ok_or_else(|| "missing overdrive parameter".to_owned())?;
        values[index] = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: *id,
            },
            parameter.value,
        );
    }
    if let Some(cached) = view.device_panel.overdrive_previews.get(&device)
        && cached.state == *state
        && cached.sample_rate == sample_rate
        && cached.values == values
    {
        return cached.curve.clone();
    }
    let curve = (|| {
        let mut draft = state.clone();
        for (id, value) in PARAMETERS.into_iter().zip(values) {
            if hex_engine::native_device_parameter_for_class(NativeDeviceClass::Effect, &draft, id)
                .map_err(|error| error.to_string())?
                != value
            {
                draft = hex_engine::patch_native_device_parameter_for_class(
                    NativeDeviceClass::Effect,
                    &draft,
                    id,
                    value,
                )
                .map_err(|error| error.to_string())?;
            }
        }
        hex_engine::native_overdrive_filter_response(&draft, sample_rate)
            .map_err(|error| error.to_string())
    })();
    view.device_panel.overdrive_previews.insert(
        device,
        PreviewCache {
            state: state.clone(),
            sample_rate,
            values,
            curve: curve.clone(),
        },
    );
    curve
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    state: &NativeDeviceState,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let response = preview(view, device, state, device_view);
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let mut control = |id, presentation| {
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
                        label: None,
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
    let mut graph = div()
        .debug_selector(move || format!("overdrive-filter-{}", device.value()))
        .w_full()
        .flex_1()
        .min_h_0();
    match response {
        Ok(response) => {
            if let Some(plot) = hex_widgets::curve_plot(
                response.points,
                theme::device_style(theme::DeviceStyleRole::Display),
                hex_widgets::CurvePlotGrid::Centered,
            ) {
                graph = graph.child(plot);
            }
        }
        Err(_) => graph = graph.child(theme::text("preview unavailable")),
    }
    let toolbar = crate::device_composition::control_row()
        .h(px(24.0))
        .flex_shrink_0()
        .justify_between()
        .child(field(
            "freq",
            control(FREQUENCY, ControlPresentation::InlineValue),
        ))
        .child(field(
            "bw",
            control(BANDWIDTH, ControlPresentation::InlineValue),
        ));
    let dials = crate::device_composition::control_row()
        .h(px(hex_widgets::COMPACT_DIAL_HEIGHT))
        .flex_shrink_0()
        .justify_around()
        .child(control(DRIVE, ControlPresentation::CompactDial))
        .child(control(TONE, ControlPresentation::CompactDial));
    let dynamics = control(DYNAMICS, ControlPresentation::InlineValue);
    let wet = control(WET, ControlPresentation::InlineValue);
    let plot = crate::device_composition::plot(graph);
    crate::device_composition::body()
        .flex_col()
        .gap(px(2.0))
        .py(px(3.0))
        .child(plot)
        .child(toolbar)
        .child(dials)
        .child(field("dynamics", dynamics).h(px(18.0)).flex_shrink_0())
        .child(field("dry/wet", wet).h(px(18.0)).flex_shrink_0())
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
