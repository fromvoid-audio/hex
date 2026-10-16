//! Compact Auto Pan composition using the shared Lattice controls.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView,
    parameter_control_with_presentation,
};
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*};
use hex_engine::{NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::ParameterId;
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::AUTO_PAN_FRAME_WIDTH;

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
    let mut controls = crate::device_composition::rail_column();
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
    crate::device_composition::centered_rails(crate::device_composition::dense_rail(controls))
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}
