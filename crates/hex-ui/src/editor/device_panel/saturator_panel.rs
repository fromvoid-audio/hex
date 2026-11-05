//! Compact Saturator composition using the shared Lattice controls and plot.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, numeric_display::NumericDisplayCodec,
    parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{NativeDeviceClass, NativeDeviceView, SaturatorTransferCurve};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::SATURATOR_FRAME_WIDTH;
const DRIVE: ParameterId = ParameterId::from_static(1);
const OUTPUT: ParameterId = ParameterId::from_static(2);
const WET: ParameterId = ParameterId::from_static(3);
const CURVE: ParameterId = ParameterId::from_static(4);
const PARAMETERS: [ParameterId; 4] = [DRIVE, OUTPUT, WET, CURVE];

pub(super) struct PreviewCache {
    state: NativeDeviceState,
    values: [ParameterValue; 4],
    curve: Result<SaturatorTransferCurve, String>,
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn cached_curve(view: &EditorView, device: DeviceId) -> Option<&SaturatorTransferCurve> {
    view.device_panel
        .saturator_previews
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
) -> Result<SaturatorTransferCurve, String> {
    let mut values = [ParameterValue::Float(0.0); 4];
    for (index, id) in PARAMETERS.iter().enumerate() {
        let parameter = device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == *id)
            .ok_or_else(|| "missing saturator parameter".to_owned())?;
        values[index] = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: *id,
            },
            parameter.value,
        );
    }
    if let Some(cached) = view.device_panel.saturator_previews.get(&device)
        && cached.state == *state
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
        hex_engine::native_saturator_transfer_curve(&draft).map_err(|error| error.to_string())
    })();
    view.device_panel.saturator_previews.insert(
        device,
        PreviewCache {
            state: state.clone(),
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
    let curve = preview(view, device, state, device_view);
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let mut control = |id, presentation| match device_view
        .parameters
        .iter()
        .find(|parameter| parameter.descriptor.id == id)
    {
        Some(parameter) => parameter_control_with_presentation(
            view,
            cx,
            device,
            identity,
            parameter,
            presentation,
            ControlText {
                label: None,
                codec: (id == WET).then_some(NumericDisplayCodec::Percent),
            },
        ),
        None => div().child(theme::text("unsupported")),
    };
    let chooser = control(CURVE, ControlPresentation::CompactWideSelector);
    let drive = control(DRIVE, ControlPresentation::CompactDial);
    let output = control(OUTPUT, ControlPresentation::CompactDial);
    let wet = control(WET, ControlPresentation::CompactDial);
    let mut graph = div()
        .id(ElementId::Name(
            format!("saturator-transfer-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("saturator-transfer-{}", device.value()))
        .w_full()
        .flex_1()
        .min_h_0();
    let range = match curve {
        Ok(curve) => {
            if let Some(plot) = hex_widgets::curve_plot(
                curve.points,
                theme::device_style(theme::DeviceStyleRole::Display),
                hex_widgets::CurvePlotGrid::Centered,
            ) {
                graph = graph.child(plot);
            }
            format!("in ±1 · out ±{:.2}", curve.output_peak)
        }
        Err(error) => {
            graph = graph.child(theme::text(format!("preview unavailable · {error}")));
            "".to_owned()
        }
    };
    let range = div()
        .h(px(10.0))
        .flex_shrink_0()
        .text_size(px(7.0))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(range));
    let controls = crate::device_composition::control_row()
        .h(px(52.0))
        .flex_shrink_0()
        .justify_between()
        .child(drive)
        .child(output)
        .child(wet);
    crate::device_composition::stacked_plot(chooser, graph, range, controls)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}
