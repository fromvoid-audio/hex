//! Compact Gate composition using the shared Lattice controls.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView,
    parameter_control_with_presentation,
};
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px};
use hex_engine::{NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::ParameterId;
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = 142.0;

const PARAMETERS: [ParameterId; 6] = [
    ParameterId::from_static(1),
    ParameterId::from_static(2),
    ParameterId::from_static(3),
    ParameterId::from_static(4),
    ParameterId::from_static(5),
    ParameterId::from_static(6),
];

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    hex_widgets::device_body(super::device_layout_style())
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
        .w_full()
        .h(px(super::DEVICE_FRAME_BODY_HEIGHT))
        .flex()
        .justify_between()
        .gap(px(2.0))
        .py(px(3.0))
        .child(gate_column(
            view,
            cx,
            device,
            identity,
            device_view,
            &PARAMETERS[..3],
        ))
        .child(gate_column(
            view,
            cx,
            device,
            identity,
            device_view,
            &PARAMETERS[3..],
        ))
}

fn gate_column(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    device_view: &NativeDeviceView,
    parameters: &[ParameterId],
) -> Div {
    let mut controls = div().flex().flex_col().gap(px(2.0));
    for id in parameters {
        let control = device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == *id)
            .map(|parameter| {
                parameter_control_with_presentation(
                    view,
                    cx,
                    device,
                    identity,
                    parameter,
                    ControlPresentation::CompactDial,
                    ControlText::default(),
                )
            })
            .unwrap_or_else(|| div().child(crate::theme::text("unsupported")));
        controls = controls.child(control);
    }
    controls
}
