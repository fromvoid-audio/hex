//! Compact Auto Pan composition using shared controls and provider-owned preview.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, parameter_control_with_presentation,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px};
use hex_engine::{AutoPanWaveformPreview, NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::ParameterId;
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::AUTO_PAN_FRAME_WIDTH;
const PARAMETERS: [(ParameterId, &str); 3] = [
    (ParameterId::from_static(1), "amount"),
    (ParameterId::from_static(2), "rate"),
    (ParameterId::from_static(3), "phase"),
];

pub(super) fn preview(
    view: &mut EditorView,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Result<AutoPanWaveformPreview, String> {
    let mut state = view
        .state
        .as_ref()
        .and_then(|s| s.project.device(device))
        .ok_or("missing auto pan")?
        .state()
        .clone();
    for parameter in &device_view.parameters {
        let id = parameter.descriptor.id;
        let value = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: id,
            },
            parameter.value,
        );
        if value != parameter.value {
            state = hex_engine::patch_native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                &state,
                id,
                value,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    hex_engine::native_auto_pan_waveform_preview(&state).map_err(|e| e.to_string())
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let curve = preview(view, device, device_view);
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let mut control = |id, label, presentation| {
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
                        label: Some(label),
                        ..ControlText::default()
                    },
                )
            })
            .unwrap_or_else(|| div().child(theme::text("unsupported")))
    };
    let mut controls = crate::device_composition::rail_column();
    for (id, label) in PARAMETERS {
        controls = controls.child(control(id, label, ControlPresentation::CompactDial));
    }
    let chooser = control(
        ParameterId::from_static(4),
        "wave",
        ControlPresentation::CompactSelector,
    );
    let mut graph = div()
        .debug_selector(move || format!("auto-pan-waveform-{device}", device = device.value()))
        .w_full()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col();
    match curve {
        Ok(curve) => {
            for points in [curve.left, curve.right] {
                if let Some(plot) = hex_widgets::curve_plot(
                    points,
                    theme::device_style(theme::DeviceStyleRole::Display),
                ) {
                    graph = graph.child(div().flex_1().min_h_0().child(plot));
                }
            }
        }
        Err(_) => graph = graph.child(theme::text("preview unavailable")),
    }
    let rail = crate::device_composition::dense_rail(controls)
        .w(px(64.0))
        .flex_shrink_0();
    let main = crate::device_composition::plot_column(
        div().h(px(14.0)).flex_shrink_0().child(chooser),
        graph,
        div().child(theme::text("static l/r")),
    );
    crate::device_composition::centered_rails(rail)
        .gap(px(4.0))
        .child(main)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}
