//! Compact Auto Pan composition using the shared Lattice controls.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView,
    parameter_control_with_presentation,
};
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px};
use hex_engine::{NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::ParameterId;
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = 144.0;

const PARAMETERS: [(ParameterId, &str); 3] = [
    (ParameterId::from_static(1), "amount"),
    (ParameterId::from_static(2), "rate"),
    (ParameterId::from_static(3), "phase"),
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
    let mut controls = div().flex().flex_col().gap(px(2.0));
    for (id, label) in PARAMETERS {
        let control = device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == id)
            .map(|parameter| {
                parameter_control_with_presentation(
                    view,
                    cx,
                    device,
                    identity,
                    parameter,
                    ControlPresentation::CompactDial,
                    ControlText {
                        label: Some(label),
                        ..ControlText::default()
                    },
                )
            })
            .unwrap_or_else(|| div().child(crate::theme::text("unsupported")));
        controls = controls.child(control);
    }
    div()
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
        .w_full()
        .h(px(super::UTILITY_FRAME_BODY_HEIGHT))
        .flex()
        .justify_center()
        .p(px(3.0))
        .child(controls)
}
